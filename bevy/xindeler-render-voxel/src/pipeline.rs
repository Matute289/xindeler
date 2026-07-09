//! EM-3.5 — async chunk meshing pipeline with budgeted uploads.
//!
//! Flow (spec §4.1): dirty-chunk queue → one [`AsyncComputeTaskPool`] task
//! per chunk (`generate_mesh` + EM-3.2 conversion, entirely off the main
//! thread) → per-frame drain that uploads AT MOST
//! [`ChunkUploadBudget::max_uploads_per_frame`] finished chunks, spawning /
//! replacing their `Mesh3d` entities. Meshes are
//! `RenderAssetUsages::RENDER_WORLD` (the converter sets it — no CPU copy,
//! matters for dimension GC).
//!
//! ## Generic over the volume source
//! The pipeline never generates terrain: a host-installed
//! [`ChunkVolumeProvider`] closure hands it the `VolGrid2d` + mesh range for
//! a key. The EM-3.3-era demo plugs a synthetic generator in; EM-3.6 plugs
//! the replicated `TerrainGrid` in — the pipeline is byte-identical in both.
//!
//! ## Ordering & budget semantics
//! - Uploads are budgeted per `Update` (default 2). Meshing itself runs on the
//!   task pool's worker threads, but the number of OUTSTANDING tasks is capped
//!   at budget × [`IN_FLIGHT_FACTOR`] — a mass re-mesh (palette reload, EM-3.6
//!   teleport) parks the remainder in the dedup queue instead of retaining
//!   hundreds of finished meshes in memory.
//! - Completion order is NOT deterministic (task scheduling + `HashMap` drain
//!   order); callers must not rely on it. Documented per the EM-3.5 acceptance
//!   — determinism is not required.
//! - Re-marking an in-flight key replaces its task (the dropped
//!   `bevy_tasks::Task` is cancelled) — last write wins. A re-mark whose
//!   provider now returns `None` ALSO cancels the in-flight task, so a stale
//!   result can never land after the volume went away.
//! - Unload ([`ChunkMeshQueue::remove_chunk`], the EM-3.6 streaming path):
//!   cancels the pending dirty mark AND the in-flight task, then despawns the
//!   chunk's entities + index entry at the start of the next pipeline run
//!   (removals are processed BEFORE the dirty drain, so `remove` → `mark`
//!   within one frame nets out to a fresh chunk — last write wins here too).
//! - Replacing a chunk despawns the old entities and spawns the new ones in the
//!   SAME command batch, so there is no visible hole.
//!
//! ## BL-82 EM-3.11h fix: first-load placeholder (no more black frames)
//! The "no visible hole" guarantee above only ever covered RE-meshing an
//! already-spawned chunk (old entity stays up until the new one is ready).
//! It said nothing about a chunk's FIRST ever mesh: between a fresh key
//! being marked dirty and its `AsyncComputeTaskPool` task finishing +
//! clearing the upload budget, that key had **no entity at all** — for
//! however many frames the greedy mesher + the budget (default 2/`Update`)
//! took. Bug report: BL-82 EM-3.11h, a real gameplay capture, showed ~2
//! fully black frames (nothing drawn — no sky, no terrain, no character;
//! only the UI overlay) while walking into a cave, immediately followed by
//! the cave popping in fully rendered. Root cause, confirmed by reading
//! `xindeler-client`'s `far_terrain.rs`: its far-mesh cutout hole is
//! DELIBERATELY excluded within `chunk_render_distance` of the live camera
//! (so the coarse LOD sheet never z-fights the block-accurate near terrain)
//! — the near pipeline (this module) was trusted to always cover that
//! band. It didn't, for a never-before-seen chunk: no near mesh (not ready
//! yet) AND no far mesh (deliberately excluded) = the bare `ClearColor`,
//! which reads as a hard black frame whenever the current atmosphere
//! profile's sky colour is dark (dusk/night/cave shadow — exactly the
//! reported moment).
//!
//! [`spawn_chunk_mesh_tasks`] now spawns a cheap, SYNCHRONOUS placeholder
//! entity (a flat-shaded box spanning the chunk's footprint and z-range,
//! `PlaceholderChunkMesh`, sharing a `TerrainChunkMesh` marker so it obeys
//! the same distance culling as real chunks) the instant a never-before-
//! indexed key starts its async task — so there is something solid to draw
//! at that spot from frame 1, not after the mesh finishes. When the real
//! mesh lands, [`apply_chunk_meshes`]'s existing despawn-old+spawn-new
//! atomic swap replaces it exactly like any other re-mesh (zero special-
//! casing needed there — a placeholder is just another `ChunkEntities`
//! entry). Already-indexed keys (re-meshes of a chunk that already has real
//! geometry, e.g. a border re-mesh when a neighbour streams in) are
//! untouched — they keep relying on the pre-existing atomic swap, no
//! placeholder ever inserted for them.
//!
//! Instrumentation: `tracing` spans around each mesh task
//! (`chunk_mesh_task`) and each upload (`chunk_mesh_upload`), plus the
//! [`ChunkUploadStats`] resource (uploads last frame / total / in-flight).
//!
//! The upload budget belongs in `GraphicsSettings` (EM-3.5 board note); that
//! struct lives in `xindeler-app`, outside this task's crate set, so v1
//! hosts insert [`ChunkUploadBudget`] directly — the settings hookup is a
//! one-line follow-up there.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use bevy::{
    app::{App, Plugin, Update},
    asset::{Assets, Handle, RenderAssetUsages},
    color::Color,
    ecs::{
        component::Component,
        entity::Entity,
        resource::Resource,
        schedule::{IntoScheduleConfigs, SystemCondition, common_conditions::resource_exists},
        system::{Commands, Local, Res, ResMut},
    },
    math::Vec3,
    mesh::{Indices, Mesh as BevyMesh, Mesh3d, PrimitiveTopology},
    pbr::{MeshMaterial3d, StandardMaterial},
    tasks::{AsyncComputeTaskPool, Task, block_on},
    transform::components::Transform,
};
use common::{terrain::TerrainChunk, vol::RectRasterableVol, volumes::vol_grid_2d::VolGrid2d};
use vek::{Aabb, Vec2 as VVec2, Vec3 as VVec3};

use crate::{
    convert::{fluid_mesh_to_bevy, terrain_mesh_to_bevy},
    material::{VoxelMaterial, WaterMaterial},
    mesh::terrain::generate_mesh,
};

/// 2D chunk key, upstream convention (`TerrainGrid` keys).
pub type ChunkKey = VVec2<i32>;

/// Upstream's max-texture-size hint for the greedy atlas (same value the
/// EM-3.3 demo used).
const MAX_ATLAS_SIZE: VVec2<u16> = VVec2 { x: 4096, y: 4096 };

/// Everything a mesh task needs for one chunk, as returned by the
/// [`ChunkVolumeProvider`].
pub struct ChunkVolume {
    /// Volume containing the chunk AND its ±1 xy neighbours (the mesher
    /// reads across the border; missing neighbours read as the grid's
    /// default chunk).
    pub grid: Arc<VolGrid2d<TerrainChunk>>,
    /// Mesh range, upstream convention (voxygen scene/terrain/mod.rs):
    /// xy = chunk ± 1 border, z = `[min_z - 2, max_z + 2]`. The mesher emits
    /// xy relative to the CHUNK ORIGIN (`range.min.xy + 1`) and ABSOLUTE z
    /// (terrain.rs `mesh_delta`), which is what [`chunk_transform`] assumes.
    pub range: Aabb<i32>,
}

impl ChunkVolume {
    /// Builds the canonical mesh range for `key` from z bounds (helper so
    /// every provider constructs the same contract — see [`Self::range`]).
    #[must_use]
    pub fn with_z_bounds(
        grid: Arc<VolGrid2d<TerrainChunk>>,
        key: ChunkKey,
        min_z: i32,
        max_z: i32,
    ) -> Self {
        let sz = TerrainChunk::RECT_SIZE.map(|e| e as i32);
        let range = Aabb {
            min: VVec3::new(key.x * sz.x - 1, key.y * sz.y - 1, min_z - 2),
            max: VVec3::new((key.x + 1) * sz.x + 1, (key.y + 1) * sz.y + 1, max_z + 2),
        };
        Self { grid, range }
    }
}

/// Host-installed volume source (see module docs). Returning `None` drops
/// the request (unknown / unloaded chunk) and cancels any in-flight task
/// for the key.
///
/// ## Snapshot contract (EM-3.6)
/// The `Arc<VolGrid2d>` a fetch returns is treated as an IMMUTABLE snapshot:
/// the mesh task reads it on another thread with no further synchronisation.
/// A streaming host must NOT mutate a grid it already handed out — it
/// materialises a fresh `VolGrid2d` per fetch (cheap: chunks are
/// `Arc<TerrainChunk>`, so building a grid is cloning a handful of `Arc`s)
/// and calls [`ChunkMeshQueue::mark_dirty`] again after every terrain edit;
/// the pipeline never re-meshes on its own.
#[derive(Resource, Clone)]
pub struct ChunkVolumeProvider(Arc<dyn Fn(ChunkKey) -> Option<ChunkVolume> + Send + Sync>);

impl ChunkVolumeProvider {
    pub fn new(provider: impl Fn(ChunkKey) -> Option<ChunkVolume> + Send + Sync + 'static) -> Self {
        Self(Arc::new(provider))
    }

    #[must_use]
    pub fn fetch(&self, key: ChunkKey) -> Option<ChunkVolume> { (self.0)(key) }
}

/// `BlockKind as u8` → texture-array layer, snapshotted from the block
/// palette ([`crate::palette::BlockPalette::layer_lut`]). A task captures
/// the `Arc` at spawn time, so a palette hot reload only affects chunks
/// marked dirty AFTER the new map is installed. IMPORTANT for reload hosts:
/// swap the `Arc` IN PLACE through `ResMut` (then re-mark) — a deferred
/// `commands.insert_resource` lands at the end of the frame, so the freshly
/// re-marked chunks could drain first and capture the stale map.
#[derive(Resource, Clone)]
pub struct ChunkLayerMap(pub Arc<[u32; 256]>);

impl Default for ChunkLayerMap {
    fn default() -> Self { Self(Arc::new([0; 256])) }
}

/// Materials for spawned chunk entities. ONE shared terrain material for all
/// chunks (bind-group reuse is what keeps budgeted uploads cheap) + the
/// shared water material (EM-3.9b — animated scroll/ripple, see
/// [`crate::material::WaterMaterialExt`]).
#[derive(Resource, Clone)]
pub struct ChunkMaterials {
    pub terrain: bevy::asset::Handle<VoxelMaterial>,
    pub fluid: bevy::asset::Handle<WaterMaterial>,
}

/// Per-frame upload budget. Default 2 (EM-3.5). Belongs in
/// `GraphicsSettings` eventually — see the module docs for why it is a
/// standalone resource v1.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ChunkUploadBudget {
    /// Max finished chunks applied (mesh assets added + entities swapped)
    /// per `Update`. Clamped to ≥ 1 at use (0 would stall forever).
    pub max_uploads_per_frame: u32,
}

impl Default for ChunkUploadBudget {
    fn default() -> Self {
        Self {
            max_uploads_per_frame: 2,
        }
    }
}

/// Dirty-chunk queue (FIFO, deduplicated) + unload requests. Hosts call
/// [`Self::mark_dirty`] / [`Self::remove_chunk`]; the pipeline drains both
/// every frame (removals first — module docs).
#[derive(Resource, Default)]
pub struct ChunkMeshQueue {
    dirty: VecDeque<ChunkKey>,
    queued: HashSet<ChunkKey>,
    removals: Vec<ChunkKey>,
}

impl ChunkMeshQueue {
    /// Enqueues `key` for (re)meshing; a key already queued is a no-op.
    pub fn mark_dirty(&mut self, key: ChunkKey) {
        if self.queued.insert(key) {
            self.dirty.push_back(key);
        }
    }

    /// Unloads `key` (the EM-3.6 streaming path): cancels a pending dirty
    /// mark and, on the next pipeline run, the in-flight task, the spawned
    /// entities and the [`ChunkMeshIndex`] entry. Last write wins — a later
    /// [`Self::mark_dirty`] re-creates the chunk.
    pub fn remove_chunk(&mut self, key: ChunkKey) {
        self.queued.remove(&key);
        self.removals.push(key);
    }

    fn pop(&mut self) -> Option<ChunkKey> {
        while let Some(key) = self.dirty.pop_front() {
            // Entries no longer in `queued` were cancelled by remove_chunk
            // (or superseded by a mark_dirty re-add later in the deque).
            if self.queued.remove(&key) {
                return Some(key);
            }
        }
        None
    }

    fn take_removals(&mut self) -> Vec<ChunkKey> { core::mem::take(&mut self.removals) }

    /// Chunks waiting to be meshed (excludes cancelled entries).
    #[must_use]
    pub fn len(&self) -> usize { self.queued.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.queued.is_empty() }
}

/// Output of one mesh task.
struct MeshedChunk {
    terrain: Option<BevyMesh>,
    fluid: Option<BevyMesh>,
}

/// In-flight mesh tasks, one per chunk key (re-marking replaces → cancels).
#[derive(Resource, Default)]
struct ChunkMeshTasks(HashMap<ChunkKey, Task<MeshedChunk>>);

/// Spawned entities per chunk key (so re-meshing replaces, not duplicates).
#[derive(Resource, Default)]
pub struct ChunkMeshIndex(HashMap<ChunkKey, ChunkEntities>);

pub struct ChunkEntities {
    pub terrain: Option<Entity>,
    pub fluid: Option<Entity>,
    /// EM-3.11h: `true` while `terrain` is the synchronous first-load
    /// placeholder box (see module docs), not the real greedy-meshed
    /// geometry. Private — only this module ever needs to tell the
    /// difference (the atomic despawn-old+spawn-new swap in
    /// [`apply_chunk_meshes`] treats a placeholder exactly like any other
    /// entry, on purpose).
    is_placeholder: bool,
}

impl ChunkMeshIndex {
    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    #[must_use]
    pub fn get(&self, key: ChunkKey) -> Option<&ChunkEntities> { self.0.get(&key) }

    /// Keys of every currently-spawned chunk. Used by palette hot reload to
    /// re-mark all live chunks dirty (their per-vertex layers changed).
    pub fn keys(&self) -> impl Iterator<Item = ChunkKey> + '_ { self.0.keys().copied() }
}

/// Upload instrumentation (complements the tracing spans).
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ChunkUploadStats {
    /// Chunks applied during the last `Update` (always ≤ the budget).
    pub uploads_last_frame: u32,
    /// Chunks applied since startup.
    pub total_uploads: u64,
    /// Mesh tasks currently in flight.
    pub in_flight: usize,
}

/// Marker on spawned opaque-terrain chunk entities.
#[derive(Component)]
pub struct TerrainChunkMesh {
    pub key: ChunkKey,
}

/// Marker on spawned fluid chunk entities.
#[derive(Component)]
pub struct FluidChunkMesh {
    pub key: ChunkKey,
}

/// Marker on the EM-3.11h synchronous first-load placeholder (see module
/// docs): a coarse box standing in for a chunk's real mesh while its async
/// task runs. Always co-spawned with a `TerrainChunkMesh` (so it obeys
/// whatever chunk-distance culling band the host applies) — this is an
/// additional tag for callers that need to tell it apart from real
/// geometry (debugging, tests), not a replacement for that marker.
#[derive(Component)]
pub struct PlaceholderChunkMesh;

/// Entity transform for a chunk mesh: the mesher emits xy relative to the
/// chunk origin and ABSOLUTE z (see [`ChunkVolume::range`]), so the entity
/// sits at the chunk origin mapped through the converter's z-up → y-up
/// rotation: Veloren `(32·kx, 32·ky, 0)` → Bevy `(32·kx, 0, −32·ky)`.
/// INTEGER translation only (converter contract: world-space texture tiling
/// with period 1 stays chunk-continuous).
#[must_use]
pub fn chunk_transform(key: ChunkKey) -> Transform {
    let sz = TerrainChunk::RECT_SIZE.map(|e| e as i32);
    #[expect(clippy::cast_precision_loss, reason = "chunk coords ≪ 2^24")]
    Transform::from_xyz((key.x * sz.x) as f32, 0.0, -(key.y * sz.y) as f32)
}

/// EM-3.11h — first-load placeholder assets, cached [`Local`] to
/// [`spawn_chunk_mesh_tasks`]: every placeholder chunk reuses the SAME
/// unit-box mesh (stretched to the chunk's footprint/height via its
/// per-entity `Transform` scale, [`placeholder_transform`]) and the SAME
/// material, so spawning one costs a component insert, not a fresh asset.
#[derive(Default)]
struct PlaceholderAssets {
    mesh: Option<Handle<BevyMesh>>,
    material: Option<Handle<StandardMaterial>>,
}

/// EM-3.11h — the placeholder's world transform: a unit box (built once by
/// [`placeholder_box_mesh`]) scaled to the chunk's `32×32` footprint and
/// `[z_lo, z_hi]` height range, positioned at the chunk's own origin (same
/// xz convention as [`chunk_transform`] — Veloren `(32·kx, 32·ky)` → Bevy
/// `(32·kx, −32·ky)`, box growing toward −z/+x/+y from there).
fn placeholder_transform(key: ChunkKey, z_lo: f32, z_hi: f32) -> Transform {
    let sz = TerrainChunk::RECT_SIZE.map(|e| e as f32);
    let height = (z_hi - z_lo).max(1.0);
    #[expect(clippy::cast_precision_loss, reason = "chunk coords ≪ 2^24")]
    let origin = Vec3::new(key.x as f32 * sz.x, z_lo, -(key.y as f32 * sz.y) - sz.y);
    Transform::from_translation(origin).with_scale(Vec3::new(sz.x, height, sz.y))
}

/// EM-3.11h — a flat-shaded, axis-aligned unit box (each face gets its own
/// 4 duplicated vertices + normal). Reused for every placeholder via a
/// non-uniform `Transform` scale ([`placeholder_transform`]) rather than
/// rebuilt per chunk. Paired with [`placeholder_material`]'s `cull_mode:
/// None` so a camera standing INSIDE a not-yet-meshed chunk (the exact
/// "walked into a cave" case this fix targets) still sees the box's inner
/// faces instead of nothing.
fn placeholder_box_mesh() -> BevyMesh {
    let corners = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(0.0, 1.0, 1.0),
    ];
    // (corner indices wound for an outward-facing first triangle, outward
    // normal) per face of the unit cube.
    let faces: [([usize; 4], Vec3); 6] = [
        ([0, 1, 2, 3], Vec3::new(0.0, 0.0, -1.0)), // -Z
        ([5, 4, 7, 6], Vec3::new(0.0, 0.0, 1.0)),  // +Z
        ([4, 0, 3, 7], Vec3::new(-1.0, 0.0, 0.0)), // -X
        ([1, 5, 6, 2], Vec3::new(1.0, 0.0, 0.0)),  // +X
        ([4, 5, 1, 0], Vec3::new(0.0, -1.0, 0.0)), // -Y
        ([3, 2, 6, 7], Vec3::new(0.0, 1.0, 0.0)),  // +Y
    ];

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut indices: Vec<u32> = Vec::with_capacity(36);
    for (face_corners, normal) in faces {
        let base = positions.len() as u32;
        for corner_index in face_corners {
            positions.push(corners[corner_index].to_array());
            normals.push(normal.to_array());
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut mesh = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// EM-3.11h — a neutral, unlit-ish rock grey so the placeholder reads as
/// plausible (if crude) geometry rather than a garish debug colour; matches
/// `far_terrain.rs`'s own placeholder-quality material (same
/// `perceptual_roughness`/`reflectance`, same `cull_mode: None` rationale).
fn placeholder_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(0.35, 0.33, 0.30),
        cull_mode: None,
        perceptual_roughness: 1.0,
        reflectance: 0.02,
        ..Default::default()
    }
}

/// In-flight meshing cap factor: [`spawn_chunk_mesh_tasks`] stops draining
/// the dirty queue once `budget × IN_FLIGHT_FACTOR` tasks are outstanding.
/// Bounds the finished-but-not-yet-applied meshes retained in memory during
/// mass re-meshes (palette reload, EM-3.6 teleports); the dedup queue holds
/// the remainder at ~24 bytes/key instead of a full mesh each.
const IN_FLIGHT_FACTOR: u32 = 8;

/// Registers the queue/tasks/budget/stats resources and the pipeline
/// systems (removals → spawn → apply). Spawn/apply idle until the host
/// inserts a [`ChunkVolumeProvider`], a [`ChunkLayerMap`] and
/// [`ChunkMaterials`]; removal processing is always live.
pub struct ChunkMeshPipelinePlugin;

impl Plugin for ChunkMeshPipelinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkMeshQueue>()
            .init_resource::<ChunkMeshTasks>()
            .init_resource::<ChunkMeshIndex>()
            .init_resource::<ChunkUploadStats>()
            .init_resource::<ChunkUploadBudget>()
            .add_systems(
                Update,
                (
                    process_chunk_removals,
                    spawn_chunk_mesh_tasks.run_if(
                        resource_exists::<ChunkVolumeProvider>
                            .and_then(resource_exists::<ChunkLayerMap>),
                    ),
                    apply_chunk_meshes.run_if(resource_exists::<ChunkMaterials>),
                )
                    .chain(),
            );
    }
}

/// Executes [`ChunkMeshQueue::remove_chunk`] requests: cancels the key's
/// in-flight task (drop = cancel, so a stale mesh can never apply after the
/// remove) and despawns its entities + index entry. Runs BEFORE the dirty
/// drain, so `remove` → `mark_dirty` within one frame nets out to a freshly
/// meshed chunk.
fn process_chunk_removals(
    mut commands: Commands,
    mut queue: ResMut<ChunkMeshQueue>,
    mut tasks: ResMut<ChunkMeshTasks>,
    mut index: ResMut<ChunkMeshIndex>,
) {
    if queue.removals.is_empty() {
        return;
    }
    for key in queue.take_removals() {
        tracing::debug!(key_x = key.x, key_y = key.y, "chunk unloaded");
        tasks.0.remove(&key);
        if let Some(old) = index.0.remove(&key) {
            if let Some(entity) = old.terrain {
                commands.entity(entity).despawn();
            }
            if let Some(entity) = old.fluid {
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Drains the dirty queue into `AsyncComputeTaskPool` tasks (one per chunk:
/// greedy meshing + EM-3.2 conversion, fully off the main thread), holding
/// back once `budget × IN_FLIGHT_FACTOR` tasks are outstanding.
fn spawn_chunk_mesh_tasks(
    provider: Res<ChunkVolumeProvider>,
    layer_map: Res<ChunkLayerMap>,
    budget: Res<ChunkUploadBudget>,
    mut queue: ResMut<ChunkMeshQueue>,
    mut tasks: ResMut<ChunkMeshTasks>,
    mut commands: Commands,
    mut index: ResMut<ChunkMeshIndex>,
    mut meshes: ResMut<Assets<BevyMesh>>,
    mut placeholder_materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut placeholder_assets: Local<PlaceholderAssets>,
) {
    if queue.is_empty() {
        return;
    }
    let in_flight_cap = (budget.max_uploads_per_frame.max(1) * IN_FLIGHT_FACTOR) as usize;
    let pool = AsyncComputeTaskPool::get();
    while tasks.0.len() < in_flight_cap {
        let Some(key) = queue.pop() else {
            break;
        };
        let Some(volume) = provider.fetch(key) else {
            // The provider no longer has this chunk: cancel any in-flight
            // task too, so a stale mesh can't land later (last write wins).
            tasks.0.remove(&key);
            // EM-3.11h: also clean up an abandoned first-load placeholder —
            // its real mesh is never coming now (the volume is gone), so
            // nothing should be left behind to linger forever.
            if index
                .0
                .get(&key)
                .is_some_and(|entities| entities.is_placeholder)
                && let Some(entity) = index.0.remove(&key).and_then(|entities| entities.terrain)
            {
                commands.entity(entity).despawn();
            }
            tracing::debug!(?key, "chunk mesh request dropped: provider has no volume");
            continue;
        };

        // EM-3.11h: a key with no entity at all yet — its first ever mesh —
        // gets an instant, synchronous placeholder so there is always
        // SOMETHING to draw at this chunk's footprint while the async task
        // + upload budget catch up (module docs: this is what closes the
        // "black frame" gap the far mesh's camera-proximity hole relied on
        // the near pipeline to cover). A key that already has an entity
        // (real geometry from a previous upload, OR a placeholder already
        // up from an earlier mark of this same key) is left alone — this
        // only ever fires once per chunk, on its very first mark.
        if !index.0.contains_key(&key)
            && let Some(materials) = placeholder_materials.as_deref_mut()
        {
            let mesh = placeholder_assets
                .mesh
                .get_or_insert_with(|| meshes.add(placeholder_box_mesh()))
                .clone();
            let material = placeholder_assets
                .material
                .get_or_insert_with(|| materials.add(placeholder_material()))
                .clone();
            #[expect(
                clippy::cast_precision_loss,
                reason = "world z bounds ≪ 2^24, same contract as chunk_transform"
            )]
            let (z_lo, z_hi) = (volume.range.min.z as f32, volume.range.max.z as f32);
            let entity = commands
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    placeholder_transform(key, z_lo, z_hi),
                    TerrainChunkMesh { key },
                    PlaceholderChunkMesh,
                ))
                .id();
            index.0.insert(key, ChunkEntities {
                terrain: Some(entity),
                fluid: None,
                is_placeholder: true,
            });
        }

        let lut = layer_map.0.clone();
        let task = pool.spawn(async move {
            let _span =
                tracing::info_span!("chunk_mesh_task", key_x = key.x, key_y = key.y).entered();
            let (opaque, fluid, _shadow, (_bounds, atlas, atlas_size, ..)) =
                generate_mesh(&volume.grid, (volume.range, MAX_ATLAS_SIZE, ()));
            MeshedChunk {
                terrain: (!opaque.is_empty()).then(|| {
                    terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, |k| lut[usize::from(k)])
                }),
                fluid: (!fluid.is_empty()).then(|| fluid_mesh_to_bevy(&fluid)),
            }
        });
        // Insert replaces any in-flight task for the key; the dropped Task
        // is cancelled (bevy_tasks/async_task semantics) — last write wins.
        tasks.0.insert(key, task);
    }
}

/// Applies at most [`ChunkUploadBudget::max_uploads_per_frame`] FINISHED
/// tasks per frame: adds the mesh assets and swaps the chunk's entities
/// (despawn old + spawn new in the same command batch — no visible hole).
fn apply_chunk_meshes(
    mut commands: Commands,
    mut tasks: ResMut<ChunkMeshTasks>,
    mut index: ResMut<ChunkMeshIndex>,
    mut stats: ResMut<ChunkUploadStats>,
    budget: Res<ChunkUploadBudget>,
    materials: Res<ChunkMaterials>,
    mut meshes: ResMut<Assets<BevyMesh>>,
) {
    // Skip entirely while idle so the stats resource's change tick (and the
    // stats themselves) stay quiet — but only once the stats already say
    // idle (tasks can drain without an upload, e.g. a provider-None or
    // remove_chunk cancellation, and `in_flight` must not go stale).
    if tasks.0.is_empty() && stats.uploads_last_frame == 0 && stats.in_flight == 0 {
        return;
    }
    stats.uploads_last_frame = 0;

    let budget = budget.max_uploads_per_frame.max(1) as usize;
    let ready: Vec<ChunkKey> = tasks
        .0
        .iter()
        .filter(|(_, task)| task.is_finished())
        .map(|(key, _)| *key)
        .take(budget)
        .collect();

    for key in ready {
        let Some(task) = tasks.0.remove(&key) else {
            continue;
        };
        let _span =
            tracing::info_span!("chunk_mesh_upload", key_x = key.x, key_y = key.y).entered();
        let meshed = block_on(task); // finished — returns immediately

        if let Some(old) = index.0.remove(&key) {
            if let Some(entity) = old.terrain {
                commands.entity(entity).despawn();
            }
            if let Some(entity) = old.fluid {
                commands.entity(entity).despawn();
            }
        }

        let transform = chunk_transform(key);
        let terrain = meshed.terrain.map(|mesh| {
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.terrain.clone()),
                    transform,
                    TerrainChunkMesh { key },
                ))
                .id()
        });
        let fluid = meshed.fluid.map(|mesh| {
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.fluid.clone()),
                    transform,
                    FluidChunkMesh { key },
                ))
                .id()
        });
        index.0.insert(key, ChunkEntities {
            terrain,
            fluid,
            is_placeholder: false,
        });
        stats.uploads_last_frame += 1;
        stats.total_uploads += 1;
    }
    stats.in_flight = tasks.0.len();
}
