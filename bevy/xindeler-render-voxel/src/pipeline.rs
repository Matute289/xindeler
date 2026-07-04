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
    asset::Assets,
    ecs::{
        component::Component,
        entity::Entity,
        resource::Resource,
        schedule::{IntoScheduleConfigs, SystemCondition, common_conditions::resource_exists},
        system::{Commands, Res, ResMut},
    },
    mesh::{Mesh as BevyMesh, Mesh3d},
    pbr::{MeshMaterial3d, StandardMaterial},
    tasks::{AsyncComputeTaskPool, Task, block_on},
    transform::components::Transform,
};
use common::{terrain::TerrainChunk, vol::RectRasterableVol, volumes::vol_grid_2d::VolGrid2d};
use vek::{Aabb, Vec2 as VVec2, Vec3 as VVec3};

use crate::{
    convert::{fluid_mesh_to_bevy, terrain_mesh_to_bevy},
    material::VoxelMaterial,
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
/// interim fluid material (stock transparent until EM-3.9).
#[derive(Resource, Clone)]
pub struct ChunkMaterials {
    pub terrain: bevy::asset::Handle<VoxelMaterial>,
    pub fluid: bevy::asset::Handle<StandardMaterial>,
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
}

impl ChunkMeshIndex {
    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    #[must_use]
    pub fn get(&self, key: ChunkKey) -> Option<&ChunkEntities> { self.0.get(&key) }
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
            tracing::debug!(?key, "chunk mesh request dropped: provider has no volume");
            continue;
        };
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
        index.0.insert(key, ChunkEntities { terrain, fluid });
        stats.uploads_last_frame += 1;
        stats.total_uploads += 1;
    }
    stats.in_flight = tasks.0.len();
}
