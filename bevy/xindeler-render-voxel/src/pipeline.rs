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
//! - A key with NO entity at all (never marked, or marked but not yet meshed)
//!   simply has nothing drawn at its footprint — see the round-19 section below
//!   for why that is the correct, INTENDED behaviour, not a gap to be papered
//!   over.
//!
//! ## BL-82 EM-3.11 round 19 — the first-load placeholder box is GONE
//! Rounds 14-18 (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md`)
//! spent five consecutive rounds retuning a synchronous "first-load
//! placeholder" box (`PlaceholderChunkMesh`, introduced EM-3.11h to close a
//! real ~2-frame black-screen bug) that stood in for a chunk's real mesh
//! while it streamed in: unlit material (round 7), immune to `DistanceFog`
//! (round 14), haze-tinted toward the live atmosphere (round 14 follow-up,
//! retuned round 16), and finally a per-chunk REAL colour hint sourced from
//! the far-terrain grid with a cave-safety veto (round 17, corrected round
//! 18). Every round made the box look more correct in isolation, and every
//! round left a residual visible artifact (Matías, round 19: "no se está
//! arreglando... queda muy feo mientras se juega") — because the premise
//! itself, not the tuning, was the defect: a flat, hard-edged, textureless
//! box can never look like real, detailed terrain, no matter how well its
//! flat colour is chosen, and a cluster of them (round 17 measured up to 16
//! simultaneous) reads as a uniform "wall"/"strip" regardless.
//!
//! Round 19 compared this port against two mature references instead of
//! retuning a 6th knob:
//! - **Minecraft** never renders a stand-in for an unloaded/not-yet-meshed
//!   chunk. A chunk column simply does not exist in the renderable world until
//!   it is generated AND meshed — the player only ever sees either (a)
//!   render-distance fog/fade hiding the edge, tuned so generation stays ahead
//!   of what becomes visible, or (b) genuine void/sky where a chunk will be.
//!   Loaded-but-not-ready = invisible, never a fake solid object.
//! - **`xindeler-old`** (the mature, pre-Bevy Veloren-derived client) has NO
//!   placeholder-mesh concept at all, confirmed by reading its code, not
//!   assumed: `voxygen/src/scene/terrain/mod.rs`'s `Terrain::chunks: HashMap<
//!   _, TerrainChunkData>` holds only real GPU mesh data (no "stand-in" variant
//!   exists); a chunk awaiting meshing lives in a SEPARATE `mesh_todo` map
//!   holding only bookkeeping (no geometry); `insert_chunk` is only ever called
//!   once a real mesh comes back from the mesh-worker thread; and the render
//!   loop's own chunk iteration (`Spiral2d::new().filter_map(|rpos|
//!   self.chunks.get(&pos)?)...filter_map(|chunk| chunk.opaque_model.as_ref()?
//!   ...)`) yields `None` — draws NOTHING — for any position without a real
//!   mesh yet. The gap this would otherwise leave is filled by a SEPARATE,
//!   always-present coarse LOD terrain mesh (`voxygen/src/scene/lod.rs`) that
//!   covers the world out to the horizon continuously, refined by real terrain
//!   popping in on top of it as chunks mesh — never a void, never a synthetic
//!   stand-in.
//!
//! Both references agree: the RIGHT fix is to stop drawing a placeholder at
//! all, not to keep improving one. [`spawn_chunk_mesh_tasks`] no longer
//! spawns anything when a key is first marked dirty — it only starts the
//! async mesh task. [`apply_chunk_meshes`] is now the ONLY place a chunk's
//! entities are ever created, exactly mirroring `xindeler-old`'s
//! `insert_chunk`: a key has zero entities from the moment it's marked dirty
//! until its real mesh finishes and uploads, then it appears once, fully
//! formed — never a box, never a partial/synthetic stand-in. This is a
//! straightforward reversion of the whole EM-3.11h-through-18 mechanism
//! (`PlaceholderChunkMesh`, its material/haze-tint/colour-hint/viewer-height
//! machinery, and the `is_placeholder` bookkeeping this module used to carry)
//! — see the findings log's round-19 entry for the full before/after
//! evidence and how the resulting gap is covered (a paired fix outside this
//! crate: `xindeler-client`'s camera-boom collision, which used to clip
//! against a chunk's raw voxel data before its mesh existed — an unrelated
//! but same-family "physics/collision outran rendering" bug the same
//! investigation found and fixed).
//!
//! [`ChunkMeshIndex::has_real_terrain_mesh`] keeps its exact name/signature
//! (a host, e.g. `xindeler-client::sprite_view`'s EM-3.11 round-18 fix, still
//! needs "does this chunk have its real mesh yet") but its answer now
//! collapses to "is this key indexed at all" — an index entry can no longer
//! ever be a placeholder, so the distinction it used to draw is moot, not
//! removed.

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
        schedule::{
            IntoScheduleConfigs, SystemCondition, SystemSet, common_conditions::resource_exists,
        },
        system::{Commands, Res, ResMut},
    },
    mesh::{Mesh as BevyMesh, Mesh3d},
    pbr::MeshMaterial3d,
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
/// BL-82 EM-3.11 round 19: a key is only ever present here once its REAL
/// mesh has landed (see the module docs' round-19 section) — there is no
/// longer a placeholder/interim state to distinguish.
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

    /// Keys of every currently-spawned chunk. Used by palette hot reload to
    /// re-mark all live chunks dirty (their per-vertex layers changed).
    pub fn keys(&self) -> impl Iterator<Item = ChunkKey> + '_ { self.0.keys().copied() }

    /// `true` once `key` has its real, greedy-meshed terrain up. BL-82
    /// EM-3.11 round 19: an index entry can no longer ever be a placeholder
    /// (module docs), so this collapses to "is the key indexed at all" — kept
    /// as a named method (not inlined at call sites) because two hosts
    /// outside this crate depend on the QUESTION, not the implementation:
    /// `xindeler-client::sprite_view`'s EM-3.11 round-18 fix gates vegetation
    /// spawn on it, and `xindeler-client::terrain_stream`'s camera-boom
    /// collision (round 19) gates solidity on it too (a chunk's raw voxel
    /// data streams in before its mesh does — this is the same "is it
    /// actually visible yet" signal both consumers need).
    #[must_use]
    pub fn has_real_terrain_mesh(&self, key: ChunkKey) -> bool { self.0.contains_key(&key) }
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

/// BL-82 EM-3.11n — per-frame cap on how many NEW mesh tasks
/// [`spawn_chunk_mesh_tasks`] STARTS in one call (pops off the dirty queue,
/// synchronously fetches a volume snapshot via [`ChunkVolumeProvider::fetch`],
/// and hands off to the task pool) — separate from `in_flight_cap` above
/// (the total OUTSTANDING task ceiling). `fetch` runs on the MAIN thread
/// (only `generate_mesh` itself runs off-thread, inside `pool.spawn`), so
/// popping+fetching many keys in a single frame is a real main-thread cost
/// that scales with how many distinct chunks the dirty queue backlogged
/// since the last frame — previously uncapped whenever `in_flight_cap` had
/// headroom (e.g. right after an idle period, when few tasks are
/// outstanding, a burst could pop+fetch up to `budget × IN_FLIGHT_FACTOR`
/// keys in ONE frame).
///
/// Investigated for the "diagonal movement feels choppier than straight"
/// report (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md` round
/// 8; `xindeler-client`'s `terrain_stream.rs::neighbourhood_3x3` docs): a
/// diagonal streaming frontier backlogs ~1.6× as many distinct chunks per
/// unit distance as a straight one for the same real ground speed (proven
/// deterministically by `terrain_stream.rs`'s
/// `diagonal_streaming_touches_more_distinct_chunks_than_straight` test), so
/// its bursts are structurally bigger — and a live `--smoke-perf-run` A/B
/// (straight vs. diagonal, same fresh world, `xindeler-client`) measured
/// diagonal movement's frame-time distribution consistently skewing toward
/// larger max/stdev than straight's across repeated trials, even though mean
/// frame time was statistically unchanged — consistent with OCCASIONAL
/// bigger spawn bursts causing occasional bigger frame-time spikes, not a
/// sustained per-frame cost increase. Spreading the fetch cost over more
/// frames bounds the single-frame spike regardless of burst size, trading a
/// slightly longer time-to-fully-meshed for a smoother frame time. Smaller
/// than `IN_FLIGHT_FACTOR` (a fetch is far cheaper than a GPU upload, but
/// this cap exists specifically to smooth BURSTS, not to throttle steady
/// throughput).
const SPAWN_BURST_FACTOR: u32 = 4;

// BL-82 EM-3.11p follow-up
// (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md` round 10):
// re-tested after Matías reported the diagonal stutter felt unchanged.
// Confirmed with live `debug!` instrumentation (`spawn_chunk_mesh_tasks`/
// `apply_chunk_meshes`'s timers below) that the cap DOES engage during real
// diagonal movement (30-45 times per 45s) — this mitigation is not a no-op —
// but the main-thread cost it bounds stayed under ~2ms even while engaged, both
// for the fetch loop and for `apply_chunk_meshes`'s upload/spawn/despawn work.
// That rules this pipeline out as the dominant cost behind the 50-800ms
// frame-time spikes Matías experiences: EM-3.11n's fix is real but small,
// addressing a structurally confirmed (~1.6× more distinct chunks touched per
// unit diagonal distance) but practically minor effect. The dominant cost
// remains unidentified as of this round; see the findings log for what else was
// ruled out (sim-side per-system timing via `XINDELER_SLOW_SYS_MS`, a full
// `bevy/trace` + `trace_chrome` capture attempt) and the honest scope of what
// this investigation could and couldn't establish given shared-machine
// measurement noise.

/// BL-82 EM-4.11 Phase D — system-ordering label for the pipeline's
/// removals→spawn→apply chain, so a HOST crate (e.g. `xindeler-client`'s
/// `receive_chunks`, which reads [`ChunkMeshIndex`] to decide which
/// neighbours are genuinely affected by a new arrival) can order itself
/// relative to this pipeline WITHOUT reaching into its private system
/// functions (`process_chunk_removals`/`spawn_chunk_mesh_tasks`/
/// `apply_chunk_meshes` are, and stay, private — this label is the only
/// ordering handle exposed). Runs in `Update` (see
/// [`ChunkMeshPipelinePlugin`]'s doc comment).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkMeshPipelineSet;

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
                    .chain()
                    .in_set(ChunkMeshPipelineSet),
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
/// back once `budget × IN_FLIGHT_FACTOR` tasks are outstanding OR once
/// `budget × SPAWN_BURST_FACTOR` NEW tasks have started THIS frame (BL-82
/// EM-3.11n — see [`SPAWN_BURST_FACTOR`]'s docs).
///
/// BL-82 EM-3.11 round 19: this no longer spawns anything synchronously — no
/// entity of any kind exists for a key between it being marked dirty and its
/// real mesh landing via [`apply_chunk_meshes`] (module docs' round-19
/// section). The EM-3.11h-through-18 first-load placeholder used to be
/// spawned here.
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
    // BL-82 EM-3.11p: wall-clock this whole system at `debug` level.
    // `provider.fetch` runs synchronously on the main thread (module docs),
    // so THIS is where a diagonal-heavier backlog actually costs a frame —
    // not just in queue depth. EM-3.11n bounded the FETCH COUNT (below) on
    // the theory that a bigger diagonal backlog meant a bigger main-thread
    // cost; this timer answers "how big, actually" without re-instrumenting
    // from scratch next time someone re-opens the diagonal-stutter
    // investigation (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md`
    // round 9 vs. the EM-3.11p follow-up: even with the cap engaging on
    // every diagonal-movement burst, measured cost stayed under ~2ms —
    // nowhere near the 50+ms frame-time spikes Matías reports, so this path
    // was never the dominant cost).
    let fetch_loop_start = std::time::Instant::now();
    let in_flight_cap = (budget.max_uploads_per_frame.max(1) * IN_FLIGHT_FACTOR) as usize;
    let spawn_cap_this_frame = (budget.max_uploads_per_frame.max(1) * SPAWN_BURST_FACTOR) as usize;
    let pool = AsyncComputeTaskPool::get();
    let mut spawned_this_frame = 0usize;
    while tasks.0.len() < in_flight_cap && spawned_this_frame < spawn_cap_this_frame {
        let Some(key) = queue.pop() else {
            break;
        };
        spawned_this_frame += 1;
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
    // BL-82 EM-3.11p: confirms `SPAWN_BURST_FACTOR` is actually engaging
    // (the dirty queue backlogs past the cap in a real run) rather than
    // sitting at a value so generous it never binds — verified live during a
    // scripted diagonal-movement `--smoke-perf-run` before this round's
    // findings were written up (30-45 engagements per 45s).
    if spawned_this_frame >= spawn_cap_this_frame && !queue.is_empty() {
        tracing::debug!(
            spawned_this_frame,
            spawn_cap_this_frame,
            queue_remaining = queue.len(),
            "EM-3.11p: spawn-burst cap engaged this frame"
        );
    }
    let fetch_loop_elapsed_ms = fetch_loop_start.elapsed().as_secs_f64() * 1000.0;
    if fetch_loop_elapsed_ms > 0.1 {
        tracing::debug!(
            elapsed_ms = fetch_loop_elapsed_ms,
            spawned_this_frame,
            "EM-3.11p: spawn_chunk_mesh_tasks main-thread cost this frame"
        );
    }
}

/// Applies at most [`ChunkUploadBudget::max_uploads_per_frame`] FINISHED
/// tasks per frame: adds the mesh assets and swaps the chunk's entities
/// (despawn old + spawn new in the same command batch — no visible hole).
///
/// BL-82 EM-3.11 round 19: this is now the ONLY place a chunk's entities are
/// ever created — the first time a key appears here IS the first time
/// anything is drawn for it (module docs' round-19 section), mirroring
/// `xindeler-old`'s `insert_chunk`.
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
    // BL-82 EM-3.11p: wall-clock the upload/despawn/spawn side too, same
    // rationale as `spawn_chunk_mesh_tasks`'s timer above — this budget is
    // separately capped (`max_uploads_per_frame`, default 2/frame), so it was
    // already suspected small; measured live during diagonal movement it
    // never exceeded the threshold below either.
    let apply_start = std::time::Instant::now();
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
    let apply_elapsed_ms = apply_start.elapsed().as_secs_f64() * 1000.0;
    if apply_elapsed_ms > 0.5 {
        tracing::debug!(
            elapsed_ms = apply_elapsed_ms,
            uploads = stats.uploads_last_frame,
            "EM-3.11p: apply_chunk_meshes main-thread cost this frame"
        );
    }
}
