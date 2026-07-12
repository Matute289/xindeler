//! EM-3.6 — client-side terrain consumer (listen-server mode only).
//!
//! Receives the [`CompressedChunk`] / [`RemoveChunk`] / [`TerrainAnchor`]
//! server messages the [`xindeler_sim_bridge`] emits, decodes chunks into a
//! shared store, and drives the EM-3.5 [`ChunkMeshPipelinePlugin`] via a
//! [`ChunkVolumeProvider`] — the SAME pipeline the synthetic 5×5 demo uses,
//! now fed the REAL Veloren world.
//!
//! ## Snapshot contract (why a fresh `VolGrid2d` per fetch)
//! The pipeline's `ChunkVolumeProvider::fetch` returns an `Arc<VolGrid2d>` it
//! treats as an immutable snapshot read on a worker thread (pipeline.rs docs).
//! We therefore never hand out a grid we keep mutating: each fetch materializes
//! a fresh `VolGrid2d` holding the requested chunk + its 8 neighbours (cheap —
//! chunks are `Arc<TerrainChunk>`, so it clones a handful of `Arc`s), and every
//! `CompressedChunk` re-marks its key dirty so the pipeline re-meshes.
//!
//! ## Purity
//! This module uses ONLY `common` terrain types (type-library, like the demo)
//! and the protocol messages — no `specs`, no server crate. The isolation
//! guard greps this crate's `src` for `specs`; this file must stay clean. It is
//! compiled only under the `listen-server` feature.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use bevy::prelude::*;
#[cfg(feature = "listen-server")]
use common::vol::BaseVol;
use common::{
    terrain::{Block, BlockKind, MapSizeLg, TerrainChunk, TerrainChunkMeta},
    vol::{ReadVol, RectRasterableVol},
    volumes::vol_grid_2d::VolGrid2d,
};
// vek only for the terrain grid keys/coords; `Vec3` here is Bevy's (prelude).
use vek::{Vec2 as VVec2, Vec3 as VVec3};
use xindeler_protocol::{CompressedChunk, RemoveChunk, TerrainAnchor};
use xindeler_render_voxel::pipeline::{
    ChunkKey, ChunkMeshIndex, ChunkMeshPipelineSet, ChunkMeshQueue, ChunkVolume,
    ChunkVolumeProvider,
};

use crate::camera::FlyCam;

/// The received-chunk store, shared between the receiving systems (writers)
/// and the [`ChunkVolumeProvider`] closure (reader, possibly on a worker
/// thread). An `RwLock` because fetches (mesh spawns) read while the receive
/// systems write; contention is low (writes are a handful of chunks/frame).
#[derive(Clone)]
struct TerrainStore {
    /// The map size the sim uses — needed to build `VolGrid2d`s whose
    /// out-of-bounds default matches the sim's (so border chunks close
    /// cleanly).
    map_size_lg: MapSizeLg,
    /// Default (void) chunk for the grids, matching the demo's void default.
    default: Arc<TerrainChunk>,
    /// key → decoded chunk. `[i32;2]` keys mirror the protocol/`TerrainGrid`.
    chunks: HashMap<[i32; 2], Arc<TerrainChunk>>,
}

impl TerrainStore {
    fn new() -> Self {
        // The Bevy client is a pure view — it does not know the sim's real map
        // size. The MAX legal map-size exponent is `MAX_WORLD_BLOCKS_LG (19) -
        // TERRAIN_CHUNK_BLOCKS_LG (5) = 14`; using it makes every received key
        // in-bounds (so `get_key_arc` never substitutes the default for a real
        // chunk), while the default chunk still closes gaps at the meshed
        // window's edge. This is the same trick the demo uses at smaller scale.
        let map_size_lg = MapSizeLg::new(VVec2::new(14, 14)).expect("valid max map size");
        let default = Arc::new(TerrainChunk::new(
            0,
            Block::empty(),
            Block::empty(),
            TerrainChunkMeta::void(),
        ));
        Self {
            map_size_lg,
            default,
            chunks: HashMap::new(),
        }
    }

    /// Materializes a fresh `VolGrid2d` for `key` + its ±1 xy neighbours (the
    /// mesher reads across chunk borders). Returns `None` if the center chunk
    /// isn't stored yet (the pipeline drops the request + cancels any stale
    /// task — pipeline.rs contract).
    fn volume_for(&self, key: ChunkKey) -> Option<ChunkVolume> {
        let center = self.chunks.get(&[key.x, key.y])?;
        let mut grid = VolGrid2d::new(self.map_size_lg, Arc::clone(&self.default))
            .expect("chunk size is a power of two");
        let mut min_z = center.get_min_z();
        let mut max_z = center.get_max_z();
        for dy in -1..=1 {
            for dx in -1..=1 {
                let nk = [key.x + dx, key.y + dy];
                if let Some(chunk) = self.chunks.get(&nk) {
                    if dx == 0 && dy == 0 {
                        // already accounted; keep bounds
                    } else {
                        min_z = min_z.min(chunk.get_min_z());
                        max_z = max_z.max(chunk.get_max_z());
                    }
                    grid.insert(VVec2::new(nk[0], nk[1]), Arc::clone(chunk));
                }
                // Missing neighbours read as the grid's default chunk.
            }
        }
        Some(ChunkVolume::with_z_bounds(
            Arc::new(grid),
            key,
            min_z,
            max_z,
        ))
    }

    /// BL-82 EM-3.12: cast the third-person camera's collision boom ray
    /// against this store's OWN terrain snapshot. Unlike [`Self::volume_for`]
    /// (the mesher's fetch path — see the follow-up note below for why it
    /// doesn't apply here), this borrows [`ChunkStoreView`] directly over
    /// `self.chunks` — no `VolGrid2d`, no `HashMap` construction, no
    /// `Arc::clone` — then delegates the actual ray math to
    /// [`crate::player_input::collide_boom`] so the clamp/pad/min logic lives
    /// in exactly one place (also unit-tested there against a hand-built
    /// grid). If the pivot's own chunk isn't streamed yet, `ChunkStoreView`'s
    /// `get` errors at the very first sample and `collide_boom`'s
    /// `.ignore_error()` lets the ray run out without ever matching `until`,
    /// so the full `desired` distance comes back — never clamps on missing
    /// data, matching the reference engine's `.ignore_error()` behaviour.
    ///
    /// ## Follow-up to the v1 `volume_for`-reuse cost (fixed here)
    /// The original v1 landed reusing [`Self::volume_for`] here, the SAME
    /// fetch the async mesh pipeline calls per dirty chunk (infrequently, off
    /// the render thread) — but THIS call site runs on the MAIN THREAD EVERY
    /// RENDERED FRAME, so every call allocated a fresh `VolGrid2d` (a
    /// `HashMap` + up to 9 `Arc::clone`s) just to run one short ray through
    /// it and immediately discard it. Both `bevy-migration-reviewer` and
    /// `rust-perf-reviewer` flagged it as worth fixing before merge (PR #64
    /// wasn't merged yet). [`ChunkStoreView`] replaces that with a zero-
    /// allocation borrow: each DDA step is one `HashMap` lookup into
    /// `self.chunks`, no construction, no clones.
    ///
    /// ## BL-82 EM-3.11 round 19 — gated on the chunk's REAL render mesh too
    /// Matías reported colliding with something invisible ("me choqué contra
    /// algo invisible, ya me había pasado") — a real gameplay capture showed
    /// the camera slamming in close against a rock formation that was NOT on
    /// screen a fraction of a second earlier. Root cause: this cast used to
    /// read `self.chunks` directly, i.e. the client's raw DECODED voxel data,
    /// which becomes available the instant a `CompressedChunk` arrives —
    /// completely independent of whether [`xindeler_render_voxel::pipeline`]'s
    /// async, budgeted mesh pipeline has actually built+uploaded a mesh for
    /// that chunk yet (the same "physics/collision truth outran what's drawn"
    /// family as the terrain-placeholder investigation this round closed —
    /// see the pipeline module docs' round-19 section). A freshly-streamed
    /// chunk can sit "solid to collision" but invisible for a genuinely
    /// perceptible window (round 17's own measurement: placeholder episodes,
    /// i.e. not-yet-real chunks, commonly lasted ~300+ ms), long enough for
    /// the boom to clip against it before the player ever saw it appear.
    /// [`ChunkMeshIndex::has_real_terrain_mesh`] is now checked in
    /// [`ChunkStoreView::get`] alongside the streamed-or-not check already
    /// there: a chunk whose data has arrived but whose real mesh hasn't
    /// landed is treated exactly like "not yet streamed" (no clip), matching
    /// what the player actually sees on screen.
    #[cfg(feature = "listen-server")]
    fn boom_cast(
        &self,
        pivot_sim: VVec3<f32>,
        dir_sim: VVec3<f32>,
        desired: f32,
        mesh_index: &ChunkMeshIndex,
    ) -> f32 {
        let view = ChunkStoreView {
            map_size_lg: self.map_size_lg,
            default: &self.default,
            chunks: &self.chunks,
            mesh_index,
        };
        crate::player_input::collide_boom(&view, pivot_sim, dir_sim, desired)
    }
}

/// BL-82 EM-3.12 follow-up (perf review on PR #64, fixed before merge): a
/// zero-allocation [`ReadVol`] view straight over [`TerrainStore::chunks`],
/// used ONLY by [`TerrainStore::boom_cast`]'s single per-frame ray query.
/// Mirrors `VolGrid2d<TerrainChunk>::get`'s exact chunk-key / chunk-offset /
/// out-of-bounds-default-fallback logic (`common/src/volumes/vol_grid_2d.rs`)
/// but reads directly from the store's own map — no `HashMap` built to hold a
/// copy, no `Arc::clone`s, just an integer division (via the SAME
/// `VolGrid2d::chunk_key`/`chunk_offs` helpers, reused as free functions so
/// the arithmetic can't drift from the mesher's) and a lookup per DDA step.
/// [`TerrainStore::volume_for`] is unchanged and keeps serving its only other
/// caller, the async mesh pipeline, which genuinely needs an owned,
/// `Send`-friendly snapshot to hand across the worker-thread boundary.
///
/// `#[cfg(feature = "listen-server")]`: this exists solely for
/// [`TerrainStore::boom_cast`], itself gated the same way (the third-person
/// camera it serves doesn't exist under a pure `net-client` build) — gating
/// it too keeps it from sitting dead-code-unused outside that build.
#[cfg(feature = "listen-server")]
struct ChunkStoreView<'a> {
    map_size_lg: MapSizeLg,
    default: &'a Arc<TerrainChunk>,
    chunks: &'a HashMap<[i32; 2], Arc<TerrainChunk>>,
    /// BL-82 EM-3.11 round 19 — see [`TerrainStore::boom_cast`]'s doc comment:
    /// a chunk whose raw data has streamed in but whose real render mesh
    /// hasn't landed yet must not clip the camera boom, or the player
    /// collides with something they can't see on screen.
    mesh_index: &'a ChunkMeshIndex,
}

/// Unit error for [`ChunkStoreView`]: `collide_boom`'s ray always finishes
/// with `.ignore_error()`, so the value itself is never inspected — this
/// exists only to satisfy `BaseVol::Error: Debug`.
#[cfg(feature = "listen-server")]
#[derive(Debug)]
struct ChunkStoreViewError;

#[cfg(feature = "listen-server")]
impl BaseVol for ChunkStoreView<'_> {
    type Error = ChunkStoreViewError;
    type Vox = Block;
}

#[cfg(feature = "listen-server")]
impl ReadVol for ChunkStoreView<'_> {
    fn get(&self, pos: VVec3<i32>) -> Result<&Block, ChunkStoreViewError> {
        let key = VolGrid2d::<TerrainChunk>::chunk_key(VVec2::new(pos.x, pos.y));
        let chunk = match self.chunks.get(&[key.x, key.y]) {
            // BL-82 EM-3.11 round 19: data has streamed in, but treat it as
            // solid ONLY once its real render mesh is up too — see
            // `TerrainStore::boom_cast`'s doc comment. A chunk that's
            // streamed-but-not-yet-meshed falls through to the same "genuine
            // miss" arm below (no clip), exactly like a chunk that hasn't
            // streamed at all.
            Some(chunk) if self.mesh_index.has_real_terrain_mesh(key) => chunk,
            // Counterintuitively (mirroring `VolGrid2d::get_key`), a key
            // outside the map's max bounds always resolves to the default
            // (void) chunk rather than an error — only an IN-BOUNDS but
            // not-yet-streamed (or streamed-but-not-yet-visually-real) chunk
            // is a genuine miss.
            None if !self.map_size_lg.contains_chunk(key) => self.default,
            Some(_) | None => return Err(ChunkStoreViewError),
        };
        let offs = VolGrid2d::<TerrainChunk>::chunk_offs(pos);
        Ok(chunk.get_unchecked(offs))
    }
}

/// Handle shared with the provider closure. `pub(crate)` (BL-82 EM-3.12): the
/// third-person camera (`crate::player_input`) needs a `Res<SharedTerrain>`
/// to cast its collision boom against the SAME streamed snapshot the mesher
/// reads — see [`SharedTerrain::boom_cast`]. The inner `TerrainStore` stays
/// private; only this newtype (and its methods) are crate-visible.
#[derive(Resource, Clone)]
pub(crate) struct SharedTerrain(Arc<RwLock<TerrainStore>>);

/// BL-82 EM-3.12 (listen-server only: the third-person camera that consumes
/// this doesn't exist under a pure `net-client` build).
#[cfg(feature = "listen-server")]
impl SharedTerrain {
    /// Cast a camera boom ray (sim/z-up world coords) against this store's
    /// terrain snapshot, returning the collision-limited distance. Takes the
    /// read lock only for the duration of this call (a handful of voxel
    /// `get`s — microseconds); the mesher's own lock usage elsewhere is
    /// unaffected. See [`TerrainStore::boom_cast`] for the (zero-allocation)
    /// view assembly and the round-19 "invisible collision" fix.
    pub(crate) fn boom_cast(
        &self,
        pivot_sim: VVec3<f32>,
        dir_sim: VVec3<f32>,
        desired: f32,
        mesh_index: &ChunkMeshIndex,
    ) -> f32 {
        let Ok(store) = self.0.read() else {
            return desired; // poisoned lock: fail open (no clip) rather than panic
        };
        store.boom_cast(pivot_sim, dir_sim, desired, mesh_index)
    }
}

/// Unlike [`SharedTerrain`]'s `boom_cast` impl above (listen-server-only
/// third-person camera collision), this method has no listen-server-specific
/// dependency, so it lives in its own, UNGATED `impl` block — `SharedTerrain`
/// itself (and `TerrainStreamPlugin`) is installed under BOTH the
/// `listen-server` and `net-client` features (`net_client.rs` also adds
/// `TerrainStreamPlugin` + `SpriteViewPlugin`), so a caller compiled under
/// either feature (e.g. `sprite_view`) needs this available regardless.
impl SharedTerrain {
    /// Returns the already-decoded chunk at `key`, if this store has received
    /// it (BL-82 EM-3.9c). Lets other client-side consumers reuse the SAME
    /// decoded `TerrainChunk` this store already holds instead of
    /// independently lz4-decompressing + bincode-deserializing the same
    /// `CompressedChunk` bytes a SECOND time — `sprite_view`'s per-chunk
    /// sprite build used to do exactly that (its own module docs called it
    /// out as an accepted-but-unresolved v1 cost, deferred by EM-3.9b to this
    /// epic). Cheap: one read-lock + one `Arc::clone`, no re-decode.
    ///
    /// `None` means "not (yet) in this store" — could be a chunk that hasn't
    /// streamed in yet, one this client never received, or one removed since.
    /// Callers that also hold the original `CompressedChunk` message should
    /// fall back to `CompressedChunk::decode` in that case rather than
    /// treating it as a hard error (this store's insertion timing relative to
    /// another system's own message read is not a contract this method makes).
    pub(crate) fn get_chunk(&self, key: [i32; 2]) -> Option<Arc<TerrainChunk>> {
        self.0.read().ok()?.chunks.get(&key).cloned()
    }
}

/// Where the spectator camera should look — the anchor world position, mapped
/// into Bevy space. Set once the first [`TerrainAnchor`] arrives; consumed by
/// [`crate::camera`]'s listen-server camera placement.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TerrainCameraAnchor {
    /// Anchor position in Bevy space (y-up), z-forward negated (converter
    /// contract: Veloren `(x, y, z)` → Bevy `(x, z, −y)`).
    pub bevy_pos: Vec3,
    /// Whether the camera has already been snapped to this anchor (one-shot).
    pub applied: bool,
}

/// Marks that at least one chunk has been received + meshed (so the camera
/// snap waits for real geometry).
#[derive(Resource, Default)]
pub struct FirstChunkReceived(pub bool);

/// Debug-only, opt-in (`XINDELER_SMOKE_TREE_CAM=1`, BL-82 EM-3.11): world
/// position (Bevy space) of the first `Wood`/`Leaves` block found in any
/// streamed chunk. The default anchor cam (`place_camera_on_anchor`) frames
/// open terrain near spawn, which is why the tree-color bug was never caught
/// by an earlier `--smoke-screenshot` — this lets a smoke run instead verify
/// tree rendering specifically. Only present as a resource when the env var
/// is set (see [`TerrainStreamPlugin::build`]); `None` until a tree block is
/// found (one-shot, first hit wins, matching [`TerrainCameraAnchor`]).
#[derive(Resource, Default)]
pub struct SmokeTreeAnchor(pub Option<Vec3>);

/// EM-3.6 client terrain consumer. Installs the shared store + provider and the
/// receive systems. Enabled only in listen-server mode; the synthetic demo
/// (`voxel_demo`) is NOT added then, so the pipeline meshes real terrain.
pub struct TerrainStreamPlugin;

impl Plugin for TerrainStreamPlugin {
    fn build(&self, app: &mut App) {
        let store = Arc::new(RwLock::new(TerrainStore::new()));
        app.insert_resource(SharedTerrain(store.clone()))
            .init_resource::<FirstChunkReceived>()
            .add_systems(Startup, install_provider)
            .add_systems(
                Update,
                (
                    receive_anchor,
                    // BL-82 EM-4.11 Phase D (ecs-design-reviewer follow-up):
                    // `receive_chunks` reads `Res<ChunkMeshIndex>` (see its
                    // doc comment / `dirty_keys_for_arrival`) to decide which
                    // neighbours are already meshed, and `ChunkMeshIndex` is
                    // written by the pipeline's `spawn_chunk_mesh_tasks`/
                    // `apply_chunk_meshes` (both `.in_set(ChunkMeshPipelineSet)`,
                    // `pipeline.rs`) — a cross-plugin conflicting-resource-access
                    // pair with no prior explicit edge. Making it structural
                    // (rather than relying on Bevy's implicit ambiguity
                    // tie-break) also fixes the latency in the right direction:
                    // running BEFORE the pipeline set means a brand-new
                    // arrival's OWN key gets queued in time to be picked up by
                    // `spawn_chunk_mesh_tasks` the SAME frame, while neighbour
                    // decisions read `ChunkMeshIndex` as of the end of the
                    // PREVIOUS frame — safe per the "one-frame-bounded
                    // staleness, not lost-forever" trace in this PR's review.
                    receive_chunks.before(ChunkMeshPipelineSet),
                    receive_removes,
                    place_camera_on_anchor,
                ),
            );

        // Debug-only, opt-in (`XINDELER_SMOKE_TREE_CAM=1`, BL-82 EM-3.11):
        // `SmokeTreeAnchor` only exists as a resource when this is set, so
        // `receive_chunks`'s `Option<ResMut<SmokeTreeAnchor>>` param is `None`
        // (zero scan cost) otherwise. `smoke_tree_cam` runs in `PostUpdate`,
        // after every `Update` camera system (fly-cam / third-person /
        // `place_camera_on_anchor` / the other smoke cams), so it always wins
        // once a tree is found; a no-op (target still `None`) until then.
        if std::env::var("XINDELER_SMOKE_TREE_CAM").is_ok_and(|v| v != "0") {
            app.init_resource::<SmokeTreeAnchor>()
                .add_systems(bevy::app::PostUpdate, smoke_tree_cam);
        }
    }
}

/// Installs the [`ChunkVolumeProvider`] backed by the shared store. The closure
/// reads the store under a read lock and materializes a fresh snapshot grid per
/// fetch (see [`TerrainStore::volume_for`]).
fn install_provider(mut commands: Commands, shared: Res<SharedTerrain>) {
    let store = shared.0.clone();
    commands.insert_resource(ChunkVolumeProvider::new(move |key: ChunkKey| {
        store.read().ok().and_then(|store| store.volume_for(key))
    }));
}

/// Decodes incoming chunks into the store and marks them (and any GENUINELY
/// affected already-meshed neighbours) dirty so the pipeline (re)meshes with
/// correct borders. See [`dirty_keys_for_arrival`]'s docs (BL-82 EM-4.11
/// Phase D) for why only already-meshed neighbours are re-marked.
fn receive_chunks(
    mut chunks: MessageReader<CompressedChunk>,
    shared: Res<SharedTerrain>,
    mut queue: ResMut<ChunkMeshQueue>,
    mesh_index: Res<ChunkMeshIndex>,
    mut first: ResMut<FirstChunkReceived>,
    mut tree_anchor: Option<ResMut<SmokeTreeAnchor>>,
    camera_anchor: Option<Res<TerrainCameraAnchor>>,
) {
    let mut touched: Vec<[i32; 2]> = Vec::new();
    {
        let Ok(mut store) = shared.0.write() else {
            return;
        };
        for msg in chunks.read() {
            match msg.decode() {
                Some(chunk) => {
                    store.chunks.insert(msg.key, Arc::new(chunk));
                    touched.push(msg.key);
                },
                None => {
                    warn!(key = ?msg.key, "dropping undecodable terrain chunk");
                },
            }
        }
    }
    if touched.is_empty() {
        return;
    }
    // BL-82 EM-4.2b: a plain, always-on log line the FIRST time any
    // `CompressedChunk` is decoded — used by the EM-4.2b acceptance test
    // (`bevy/xindeler-server-app/tests/replicon_quinnet_dual_stack.rs`) to
    // confirm at least one terrain chunk crossed the NEW replicon+quinnet
    // transport, by grepping the net-client process's stdout. Cheap (fires
    // exactly once per process) and harmless under every other mode
    // (listen-server's loopback also passes through here).
    if !first.0 {
        info!(keys = ?touched, "first terrain chunk(s) received over the network");
    }
    for key in &touched {
        // BL-82 EM-4.11 Phase D: only mark a neighbour dirty if it is
        // ALREADY meshed (its border may have just changed) — see
        // [`dirty_keys_for_arrival`]'s docs for the full reasoning and
        // [`neighbourhood_3x3`]'s docs for why the footprint mattered for
        // EM-3.11n's diagonal-streaming finding in the first place.
        // `ChunkMeshQueue` also dedupes an already-queued key, so re-marking
        // `key` itself here is harmless.
        for nk in dirty_keys_for_arrival(*key, |nk| {
            mesh_index.get(VVec2::new(nk[0], nk[1])).is_some()
        }) {
            queue.mark_dirty(VVec2::new(nk[0], nk[1]));
        }
    }
    first.0 = true;

    // `XINDELER_SMOKE_TREE_CAM` (see `TerrainStreamPlugin::build`): scan
    // EVERY currently-stored chunk (not just the ones that just arrived —
    // whether a candidate qualifies can change as more neighbours stream in,
    // see below) for a Wood/Leaves block, until one is found. Only runs at
    // all when the resource exists (env var set); a no-op every call after
    // that (bounded scan of a debug-only, opt-in feature).
    if let Some(tree_anchor) = tree_anchor.as_deref_mut()
        && tree_anchor.0.is_none()
        && let Ok(store) = shared.0.read()
    {
        let edge = TerrainChunk::RECT_SIZE.x as i32;
        let mut best: Option<(f32, Vec3)> = None;
        for (key, chunk) in &store.chunks {
            // Require the full 3x3 neighbourhood too: the greedy mesher reads
            // across chunk borders (pipeline.rs), so a frontier-of-streaming
            // chunk missing neighbours meshes with broken/partial borders —
            // exactly the kind of misleading artifact a bug-verification
            // screenshot must not show. Skip until the whole neighbourhood is
            // in (re-checked every call, since it fills in over time).
            let neighbours_complete = (-1..=1).all(|dy| {
                (-1..=1).all(|dx| {
                    (dx, dy) == (0, 0) || store.chunks.contains_key(&[key[0] + dx, key[1] + dy])
                })
            });
            if !neighbours_complete {
                continue;
            }
            'chunk: for lx in 0..edge {
                for ly in 0..edge {
                    for z in chunk.get_min_z()..chunk.get_max_z() {
                        let Ok(block) = chunk.get(VVec3::new(lx, ly, z)) else {
                            continue;
                        };
                        if matches!(block.kind(), BlockKind::Wood | BlockKind::Leaves) {
                            let wx = key[0] * edge + lx;
                            let wy = key[1] * edge + ly;
                            // Converter contract: Veloren (x, y, z) -> Bevy (x, z, -y).
                            let bevy_pos = Vec3::new(wx as f32, z as f32, -(wy as f32));
                            // Prefer the candidate closest to the spawn
                            // anchor: it sits comfortably inside the normal
                            // near-chunk render band, avoiding the far-mesh
                            // hole boundary (`far_terrain.rs`) that a distant
                            // pick could land on/near and render misleadingly.
                            let dist_key = camera_anchor
                                .as_ref()
                                .map_or(0.0, |a| a.bevy_pos.distance_squared(bevy_pos));
                            if best.is_none_or(|(d, _)| dist_key < d) {
                                best = Some((dist_key, bevy_pos));
                            }
                            continue 'chunk;
                        }
                    }
                }
            }
        }
        if let Some((_, bevy_pos)) = best {
            info!(
                ?bevy_pos,
                "XINDELER_SMOKE_TREE_CAM: found a Wood/Leaves block with a complete \
                 neighbourhood, snapping camera"
            );
            tree_anchor.0 = Some(bevy_pos);
        }
    }
}

/// Drops unloaded chunks from the store and the mesh pipeline, re-meshing the
/// removed chunk's surviving neighbours (their borders opened up).
fn receive_removes(
    mut removes: MessageReader<RemoveChunk>,
    shared: Res<SharedTerrain>,
    mut queue: ResMut<ChunkMeshQueue>,
) {
    let mut removed: Vec<[i32; 2]> = Vec::new();
    {
        let Ok(mut store) = shared.0.write() else {
            return;
        };
        for msg in removes.read() {
            if store.chunks.remove(&msg.key).is_some() {
                removed.push(msg.key);
            }
        }
    }
    for key in &removed {
        queue.remove_chunk(VVec2::new(key[0], key[1]));
        // The removed chunk itself has nothing left to remesh — only its
        // surviving neighbours (their shared border just opened up).
        for nk in neighbourhood_3x3(*key) {
            if nk == *key {
                continue;
            }
            queue.mark_dirty(VVec2::new(nk[0], nk[1]));
        }
    }
}

/// The 3×3 neighbourhood of `key` — itself plus its 8 neighbours — as a fixed
/// 9-element array. Used by [`receive_removes`] (marks just the 8
/// neighbours, skipping the removed key itself — a chunk unloading always
/// genuinely opens up its surviving neighbours' shared border, so that path
/// is unconditional) and by [`dirty_keys_for_arrival`] (the arrival path,
/// which — since BL-82 EM-4.11 Phase D — filters this footprint down to only
/// the genuinely-affected neighbours; see its docs).
///
/// ## BL-82 EM-3.11n: why this exact footprint matters for the
/// ## diagonal-movement "choppier" report
/// `docs/design/specs/2026-07-09-bl82-em311-findings-log.md` (round 8) traces
/// Matías's "diagonal movement feels choppier" report partly to this
/// function's fan-out: consecutive STRAIGHT-line chunk arrivals (e.g.
/// `(i,0)`, `(i+1,0)`, …) have 3×3 neighbourhoods that overlap by a full
/// 2×3 = 6 cells, so each new arrival only adds 3 genuinely NEW distinct
/// chunks to the dirty/remesh set. Consecutive DIAGONAL arrivals (e.g.
/// `(i,i)`, `(i+1,i+1)`, …) only overlap by 2×2 = 4 cells, so each new
/// arrival adds 5 new distinct chunks — for the same number of newly-streamed
/// chunks (i.e. the same real distance travelled, since the server streams
/// one new chunk per grid-line crossing regardless of direction), a diagonal
/// streaming frontier churns through ~1.6× as many distinct chunks as a
/// straight one, IF every arrival unconditionally marks its whole 3×3 (see
/// the `diagonal_streaming_touches_more_distinct_chunks_*` test below for the
/// exact counts of that OLD, now-superseded behaviour). More distinct chunks
/// marked dirty means more async re-mesh tasks competing for the same
/// fixed-size `ChunkUploadBudget` (`pipeline.rs`) and `AsyncComputeTaskPool`
/// capacity — this doesn't by itself prove a frame-time regression (meshing
/// is off the main thread and uploads are budget-capped regardless of
/// backlog size), but it was a real, structural, direction-dependent
/// difference in streaming load, addressed by
/// [`dirty_keys_for_arrival`] (BL-82 EM-4.11 Phase D) and verified
/// empirically alongside the `--smoke-perf-run` live A/B harness
/// (`crate::smoke`).
fn neighbourhood_3x3(key: [i32; 2]) -> [[i32; 2]; 9] {
    let mut out = [[0; 2]; 9];
    let mut i = 0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            out[i] = [key[0] + dx, key[1] + dy];
            i += 1;
        }
    }
    out
}

/// BL-82 EM-4.11 Phase D — the keys to mark dirty for ONE newly-arrived
/// chunk at `key`: the chunk itself, always (it needs its own first/updated
/// mesh), plus each of its 8 neighbours ONLY IF `is_meshed` reports that
/// neighbour already has geometry (a real mesh OR the EM-3.11h synchronous
/// placeholder — either way it has an entry in `ChunkMeshIndex`, so its
/// current border may be stale/wrong now that `key`'s real data exists).
///
/// ## Why skipping a not-yet-meshed neighbour is safe
/// `receive_chunks` used to unconditionally mark the WHOLE 3×3 neighbourhood
/// dirty on every arrival (see [`neighbourhood_3x3`]'s docs — this is
/// EM-3.11n's root cause for the diagonal-vs-straight asymmetry: a diagonal
/// frontier's 3×3 windows overlap less, so more NEW, not-yet-existing
/// neighbour keys got redundantly marked per arrival). A neighbour that has
/// never streamed in at all yet gets NO benefit from being marked here: the
/// `ChunkVolumeProvider::fetch` this queue entry eventually drives reads the
/// CURRENT store contents (`TerrainStore::volume_for`), so once that
/// neighbour genuinely arrives and meshes for the first time, it picks up
/// `key`'s real data automatically — there is no stale state to correct,
/// because there was never a mesh to begin with. Marking it here is pure
/// churn: an extra `ChunkMeshQueue` entry (deduped, so cheap, but still an
/// extra queue slot / potential extra `AsyncComputeTaskPool` fetch if it
/// happens to get popped before its own real arrival lands).
///
/// A neighbour that IS already meshed, by contrast, has real geometry whose
/// border facing `key` was generated against whatever was there before
/// (often the grid's void default) — `key`'s arrival can genuinely change
/// that border, so it must be re-marked. This function is deliberately a
/// pure, `Res`-free helper (takes `is_meshed` as a closure) so it can be
/// exercised directly by a unit test without spinning up an ECS `App`.
fn dirty_keys_for_arrival(
    key: [i32; 2],
    mut is_meshed: impl FnMut([i32; 2]) -> bool,
) -> Vec<[i32; 2]> {
    let mut out = vec![key];
    for nk in neighbourhood_3x3(key) {
        if nk != key && is_meshed(nk) {
            out.push(nk);
        }
    }
    out
}

/// Stores the camera anchor the first time it arrives (mapped to Bevy space).
/// Only the first anchor matters for v1 (`.next()` drops the rest).
fn receive_anchor(mut anchors: MessageReader<TerrainAnchor>, mut commands: Commands) {
    if let Some(msg) = anchors.read().next() {
        let [x, y, z] = msg.wpos;
        // Converter contract (pipeline.rs `chunk_transform`): Veloren
        // (x, y, z) → Bevy (x, z, −y).
        let bevy_pos = Vec3::new(x, z, -y);
        commands.insert_resource(TerrainCameraAnchor {
            bevy_pos,
            applied: false,
        });
        info!(?bevy_pos, "terrain camera anchor received");
    }
}

/// Snaps the spectator fly-cam over the anchor once BOTH the anchor and the
/// first meshed chunk are in — a one-shot. Positions the camera above and
/// south-east of the anchor looking down at it, so the smoke screenshot frames
/// the streamed terrain.
fn place_camera_on_anchor(
    anchor: Option<ResMut<TerrainCameraAnchor>>,
    first: Res<FirstChunkReceived>,
    mut cameras: Query<(&mut Transform, &mut FlyCam)>,
) {
    let Some(mut anchor) = anchor else { return };
    if anchor.applied || !first.0 {
        return;
    }
    let target = anchor.bevy_pos;
    // EM-3.7: a near, low, mostly-horizontal vantage so the ~1-2 m placeholder
    // entity capsules that spawn in a ~12 m ring around the anchor are clearly
    // visible (not sub-pixel dots seen from straight above), while still showing
    // terrain behind them. Stand ~18 m back and only ~5 m up, and aim slightly
    // ABOVE the ground so the capsules sit in the lower third of frame rather
    // than being occluded by foreground terrain.
    let eye = target + Vec3::new(-0.5 * CHUNK_EDGE, 0.22 * CHUNK_EDGE, -0.5 * CHUNK_EDGE);
    let look_at = target + Vec3::new(0.0, 1.5, 0.0);
    for (mut transform, mut cam) in &mut cameras {
        *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
        let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        cam.yaw = yaw;
        cam.pitch = pitch;
    }
    anchor.applied = true;
    info!(?eye, ?target, "spectator camera placed over terrain anchor");
}

/// The Bevy-space size of one chunk edge (used by the camera placement),
/// derived from the upstream constant rather than a literal so it can never
/// silently drift from `TERRAIN_CHUNK_BLOCKS_LG`. This also keeps
/// `RectRasterableVol` genuinely used (it is needed transitively by
/// `with_z_bounds`), so no unused-import guard is required.
pub const CHUNK_EDGE: f32 = TerrainChunk::RECT_SIZE.x as f32;

/// See [`TerrainStreamPlugin::build`]'s `XINDELER_SMOKE_TREE_CAM` note.
/// Frames the found tree from ~34 m away and ~28 m up. `target` is wherever a
/// Wood/Leaves block happened to be found — often deep INSIDE a trunk or
/// canopy, not its exterior — so the offset has to comfortably clear a full
/// tree's canopy radius (temperate/redwood canopies commonly span 10-20+
/// blocks) or the shot ends up with the near clip plane slicing through
/// foliage/branches a few blocks from the lens (large flat mis-shaded panels
/// with hard seams — a camera-placement artifact, not the color bug this is
/// meant to verify). Re-runs every frame (cheap: a no-op until
/// `SmokeTreeAnchor` is `Some`), so it wins over `place_camera_on_anchor` and
/// the fly-cam the instant a tree is found, and keeps holding once found.
fn smoke_tree_cam(
    tree_anchor: Option<Res<SmokeTreeAnchor>>,
    // Non-optional `&mut FlyCam` (review should-fix #1): the previous
    // `Option<&mut FlyCam>` matched EVERY `Transform`-bearing entity, not
    // just the camera — including every streamed chunk mesh (`pipeline.rs`'s
    // `chunk_transform` gives each one a world-space `Transform`) — so once a
    // tree was found this system collapsed the whole scene's transforms onto
    // one point every frame. Mirrors `place_camera_on_anchor`'s query shape.
    mut cameras: Query<(&mut Transform, &mut FlyCam)>,
) {
    let Some(target) = tree_anchor.and_then(|a| a.0) else {
        return;
    };
    let eye = target + Vec3::new(-24.0, 28.0, 24.0);
    let look_at = target + Vec3::new(0.0, 6.0, 0.0);
    for (mut transform, mut cam) in &mut cameras {
        *transform = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
        let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        cam.yaw = yaw;
        cam.pitch = pitch;
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        app::App,
        asset::{AssetApp, AssetPlugin, Handle},
        prelude::MinimalPlugins,
    };
    use common::{terrain::BlockKind, vol::WriteVol};
    use vek::{Rgb, Vec3 as VVec3};
    use xindeler_render_voxel::pipeline::{
        ChunkLayerMap, ChunkMaterials, ChunkMeshIndex, ChunkMeshPipelinePlugin, ChunkUploadStats,
    };

    use super::*;

    const CHUNK: i32 = 32;

    /// A small solid chunk (a low slab) — non-empty so it meshes.
    fn solid_chunk(height: i32) -> TerrainChunk {
        let mut chunk =
            TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
        for lx in 0..CHUNK {
            for ly in 0..CHUNK {
                for z in 0..height {
                    chunk
                        .set(
                            VVec3::new(lx, ly, z),
                            Block::new(BlockKind::Rock, Rgb::new(120, 120, 120)),
                        )
                        .expect("in-bounds write");
                }
            }
        }
        chunk
    }

    /// Headless App running the real chunk pipeline + the terrain-stream
    /// consumer over a message-fed store (no assets, no window, no sim).
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<bevy::mesh::Mesh>()
            // Register the protocol MESSAGES without replicon (we `write_message`
            // them directly — the consumer just needs `Messages<T>` to exist).
            .add_message::<CompressedChunk>()
            .add_message::<RemoveChunk>()
            .add_message::<TerrainAnchor>()
            .add_plugins(ChunkMeshPipelinePlugin)
            .add_plugins(TerrainStreamPlugin)
            // Stub the pipeline's material inputs (no GPU needed to exercise
            // meshing + entity spawn/despawn).
            .insert_resource(ChunkLayerMap::default())
            .insert_resource(ChunkMaterials {
                terrain: Handle::default(),
                fluid: Handle::default(),
            });
        // Install the provider (Startup system) + settle the app.
        app.finish();
        app.update();
        app
    }

    fn drain_until<F: Fn(&App) -> bool>(app: &mut App, max: u32, done: F) {
        for _ in 0..max {
            app.update();
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("pipeline did not reach the expected state within {max} updates");
    }

    /// A `CompressedChunk` (+ its neighbours) → decode → store → mesh: a chunk
    /// entity appears. Then a `RemoveChunk` → the entity is despawned.
    #[test]
    fn streamed_chunk_meshes_then_removes() {
        let mut app = test_app();

        // Feed a 3×3 neighbourhood so the center's borders mesh cleanly.
        for dx in -1..=1 {
            for dy in -1..=1 {
                let key = [dx, dy];
                app.world_mut()
                    .write_message(CompressedChunk::encode(key, &solid_chunk(4)));
            }
        }
        drain_until(&mut app, 500, |app| {
            let idx = app.world().resource::<ChunkMeshIndex>();
            let stats = app.world().resource::<ChunkUploadStats>();
            idx.get(VVec2::new(0, 0)).is_some() && stats.in_flight == 0
        });
        assert!(
            app.world()
                .resource::<ChunkMeshIndex>()
                .get(VVec2::new(0, 0))
                .is_some(),
            "the streamed center chunk must mesh into an entity"
        );
        assert!(
            app.world().resource::<FirstChunkReceived>().0,
            "receiving a chunk must set FirstChunkReceived"
        );

        // Now unload the center chunk.
        app.world_mut().write_message(RemoveChunk { key: [0, 0] });
        drain_until(&mut app, 500, |app| {
            app.world()
                .resource::<ChunkMeshIndex>()
                .get(VVec2::new(0, 0))
                .is_none()
        });
        assert!(
            app.world()
                .resource::<ChunkMeshIndex>()
                .get(VVec2::new(0, 0))
                .is_none(),
            "a RemoveChunk must despawn the chunk's mesh entity"
        );
    }

    /// BL-82 EM-3.9c: [`SharedTerrain::get_chunk`] returns the already-decoded
    /// chunk once `receive_chunks` has processed it (so `sprite_view` can
    /// reuse it instead of decoding the same `CompressedChunk` bytes again),
    /// and `None` for a key this store never received.
    #[test]
    fn shared_terrain_get_chunk_reuses_the_decoded_store() {
        let mut app = test_app();
        app.world_mut()
            .write_message(CompressedChunk::encode([0, 0], &solid_chunk(4)));
        drain_until(&mut app, 500, |app| {
            app.world()
                .resource::<SharedTerrain>()
                .get_chunk([0, 0])
                .is_some()
        });

        let shared = app.world().resource::<SharedTerrain>();
        assert!(
            shared.get_chunk([0, 0]).is_some(),
            "a streamed chunk must be fetchable from the shared store"
        );
        assert!(
            shared.get_chunk([99, 99]).is_none(),
            "an unstreamed key must miss, not fabricate a chunk"
        );
    }

    /// BL-82 EM-4.11 Phase D — real-schedule (not just pure-function) proof
    /// that the selective-marking fix's TWO cases both hold true end-to-end,
    /// across REAL frames (not one batch): an ALREADY-meshed neighbour of a
    /// later arrival gets genuinely re-meshed (a fresh entity, via the
    /// existing atomic despawn-old+spawn-new swap), while a neighbour that
    /// has never streamed in stays un-indexed (never spawned at all) rather
    /// than being redundantly queued.
    #[test]
    fn later_arrival_remeshes_an_already_meshed_neighbour_but_skips_an_unstreamed_one() {
        let mut app = test_app();

        // Frame batch 1: mesh a full 3×3 around the origin (as in
        // `streamed_chunk_meshes_then_removes`) and let it fully settle.
        for dx in -1..=1 {
            for dy in -1..=1 {
                let key = [dx, dy];
                app.world_mut()
                    .write_message(CompressedChunk::encode(key, &solid_chunk(4)));
            }
        }
        drain_until(&mut app, 500, |app| {
            let idx = app.world().resource::<ChunkMeshIndex>();
            let stats = app.world().resource::<ChunkUploadStats>();
            (-1..=1).all(|dx| {
                (-1..=1).all(|dy| idx.get(VVec2::new(dx, dy)).is_some()) && stats.in_flight == 0
            })
        });

        // [1, 0] and [1, 1] are ALREADY-meshed neighbours the next arrival
        // will touch; record their current entity so a later re-mesh (a
        // fresh entity, per the atomic swap) is observable. [2, -1] is a
        // neighbour of the next arrival too, but has NEVER streamed in — it
        // must stay un-indexed both before and after.
        let entity_before_1_0 = app
            .world()
            .resource::<ChunkMeshIndex>()
            .get(VVec2::new(1, 0))
            .and_then(|e| e.terrain)
            .expect("[1,0] meshed in batch 1");
        let entity_before_1_1 = app
            .world()
            .resource::<ChunkMeshIndex>()
            .get(VVec2::new(1, 1))
            .and_then(|e| e.terrain)
            .expect("[1,1] meshed in batch 1");
        assert!(
            app.world()
                .resource::<ChunkMeshIndex>()
                .get(VVec2::new(2, -1))
                .is_none(),
            "[2,-1] must not be indexed before it has ever streamed in"
        );

        // Frame batch 2 (later frames, not the same batch): a genuinely NEW
        // arrival at [2, 0], neighbouring [1,-1]/[1,0]/[1,1] (already meshed)
        // and [2,-1]/[2,1]/[3,-1]/[3,0]/[3,1] (never streamed).
        app.world_mut()
            .write_message(CompressedChunk::encode([2, 0], &solid_chunk(4)));
        drain_until(&mut app, 500, |app| {
            let idx = app.world().resource::<ChunkMeshIndex>();
            let stats = app.world().resource::<ChunkUploadStats>();
            idx.get(VVec2::new(2, 0)).is_some() && stats.in_flight == 0
        });
        // The already-meshed neighbours must have been re-marked dirty and
        // genuinely re-meshed by the new arrival — settle a few more frames
        // so their re-mesh (queued alongside [2,0]'s own first mesh) has
        // time to complete too.
        drain_until(&mut app, 500, |app| {
            let idx = app.world().resource::<ChunkMeshIndex>();
            let stats = app.world().resource::<ChunkUploadStats>();
            stats.in_flight == 0
                && idx.get(VVec2::new(1, 0)).and_then(|e| e.terrain) != Some(entity_before_1_0)
                && idx.get(VVec2::new(1, 1)).and_then(|e| e.terrain) != Some(entity_before_1_1)
        });

        assert!(
            app.world()
                .resource::<ChunkMeshIndex>()
                .get(VVec2::new(2, -1))
                .is_none(),
            "[2,-1] must STILL be un-indexed: it never streamed in, so the selective rule must \
             never have queued it just because it's a neighbour of [2,0]"
        );
    }

    /// A `TerrainAnchor` becomes a `TerrainCameraAnchor` resource (Veloren
    /// z-up → Bevy y-up mapping).
    #[test]
    fn anchor_message_sets_camera_resource() {
        let mut app = test_app();
        app.world_mut().write_message(TerrainAnchor {
            wpos: [100.0, 200.0, 50.0],
        });
        app.update();
        app.update();
        let anchor = app
            .world()
            .get_resource::<TerrainCameraAnchor>()
            .expect("anchor message installs TerrainCameraAnchor");
        // Veloren (100, 200, 50) → Bevy (100, 50, −200).
        assert_eq!(anchor.bevy_pos, Vec3::new(100.0, 50.0, -200.0));
    }

    /// BL-82 EM-3.11n regression test, kept as the HISTORICAL baseline — see
    /// [`neighbourhood_3x3`]'s doc comment for the full reasoning. This
    /// documents the OLD, now-superseded behaviour (`receive_chunks`
    /// unconditionally marking every arrival's whole 3×3 neighbourhood,
    /// regardless of whether a neighbour was ever meshed): deterministic, no
    /// GPU/timing involved, for the SAME number `N` of newly-streamed chunk
    /// keys a diagonal streaming frontier (`(0,0),(1,1),(2,2),…`) touches
    /// strictly MORE distinct chunk keys than a straight one
    /// (`(0,0),(1,0),(2,0),…`) because consecutive diagonal arrivals' 3×3
    /// neighbourhoods overlap by only 2×2=4 cells vs. the straight case's
    /// 2×3=6 cells. `neighbourhood_3x3` itself is unchanged (still used
    /// as-is by `receive_removes` and internally by
    /// [`dirty_keys_for_arrival`]), so this property still holds for the raw
    /// footprint — the fix below is about which of these cells actually get
    /// marked on the ARRIVAL path.
    #[test]
    fn old_unconditional_marking_touches_more_distinct_chunks_diagonally() {
        use std::collections::HashSet;

        /// Total distinct chunk keys touched (dirty-marked) across an entire
        /// streaming run under the OLD rule — the union of every arrival's
        /// whole 3×3 neighbourhood, unconditionally.
        fn total_distinct_touches(keys: &[[i32; 2]]) -> usize {
            let mut all: HashSet<[i32; 2]> = HashSet::new();
            for key in keys {
                all.extend(neighbourhood_3x3(*key));
            }
            all.len()
        }

        const N: i32 = 30;
        let straight: Vec<[i32; 2]> = (0..N).map(|i| [i, 0]).collect();
        let diagonal: Vec<[i32; 2]> = (0..N).map(|i| [i, i]).collect();

        let straight_touches = total_distinct_touches(&straight);
        let diagonal_touches = total_distinct_touches(&diagonal);

        // Exact counts for N=30 (verified independently): straight = 3N+6 =
        // 96, diagonal = 5N+4 = 154 — asserted exactly so a future change to
        // the neighbourhood shape (e.g. widening past 3×3) is caught, not
        // just "still greater than".
        assert_eq!(
            straight_touches, 96,
            "straight frontier distinct-touch count (old, unconditional rule)"
        );
        assert_eq!(
            diagonal_touches, 154,
            "diagonal frontier distinct-touch count (old, unconditional rule)"
        );
        assert!(
            diagonal_touches > straight_touches,
            "diagonal frontier ({diagonal_touches} distinct touched chunks) must exceed the \
             straight frontier ({straight_touches}) for the same {N}-chunk streaming run"
        );
        // The ratio approaches 5/3 ≈ 1.667 as N grows; at N=30 it's already
        // past 1.5 — a real, structural ~60% more remesh churn for the same
        // streamed distance under the OLD rule.
        let ratio = f64::from(diagonal_touches as u32) / f64::from(straight_touches as u32);
        assert!(
            ratio > 1.5,
            "expected the diagonal/straight distinct-touch ratio to approach ~1.667 (5/3 marginal \
             cells per arrival), got {ratio:.3}"
        );
    }

    /// [`dirty_keys_for_arrival`] in isolation, no streaming run: an arriving
    /// chunk always marks itself; an ALREADY-meshed neighbour is marked too;
    /// a not-yet-meshed neighbour is skipped.
    #[test]
    fn dirty_keys_for_arrival_only_marks_already_meshed_neighbours() {
        use std::collections::HashSet;

        let meshed: HashSet<[i32; 2]> = [[1, 0], [0, 1]].into_iter().collect();
        let mut out = dirty_keys_for_arrival([0, 0], |k| meshed.contains(&k));
        out.sort_unstable();
        // Self, plus only the two neighbours that are already meshed — the
        // other 6 candidates in the 3×3 (including [1,1], [-1,-1], etc.) are
        // skipped because they have no `ChunkMeshIndex` entry yet.
        assert_eq!(out, vec![[0, 0], [0, 1], [1, 0]]);
    }

    /// BL-82 EM-4.11 Phase D regression test — the selective-marking fix.
    /// Reuses EM-3.11n's exact synthetic model (a single new chunk streams
    /// per grid-line crossing, matching the "server streams one new chunk
    /// per crossing regardless of direction" framing above): under that
    /// model's OWN information, a neighbour off the direct line of travel
    /// (e.g. `(i,-1)` for the straight path, or `(i,i-1)` for the diagonal
    /// path) never independently streams in, so it can NEVER be
    /// already-meshed — [`dirty_keys_for_arrival`] can therefore never find
    /// it eligible for a re-mark. That collapses BOTH directions to the
    /// theoretical minimum (`N`, exactly one touch per arrival, zero
    /// redundant neighbour dirtying), closing the EXACT asymmetry the
    /// previous test measures (154 vs 96, ratio ~1.667) down to a dead heat
    /// (`N` vs `N`, ratio 1.0) for this synthetic corridor.
    ///
    /// A single-cell-wide corridor is an idealization of a real player's
    /// disc-shaped streaming radius (`chunk_render_distance`, default 7
    /// chunks — `far_terrain.rs`/`lod.rs`): a real newly-arriving edge chunk
    /// usually DOES have several already-meshed neighbours (the rest of the
    /// already-streamed disc), and those remain genuinely, correctly
    /// re-marked by the fix — that is not a regression, it is the fix
    /// working as intended (a real border changed). So this unit test proves
    /// the new rule is CORRECT and eliminates 100% of this specific
    /// synthetic asymmetry, but the realistic, non-idealized magnitude of
    /// the improvement is what `--smoke-perf-run`'s live straight-vs-diagonal
    /// A/B (Phase D's actual acceptance instrument) measures.
    #[test]
    fn selective_marking_eliminates_the_synthetic_diagonal_asymmetry() {
        use std::collections::HashSet;

        /// Simulates an entire streaming run under the NEW selective rule:
        /// at each arrival, `is_meshed` reflects every key touched by a
        /// PRIOR arrival's `dirty_keys_for_arrival` call (self or a
        /// genuinely-affected neighbour) — modelling that, given normal
        /// movement speed, a chunk marked dirty on an earlier grid-line
        /// crossing has long since been meshed (or at least placeholder-
        /// indexed, `pipeline.rs`'s EM-3.11h note) by the time the NEXT
        /// crossing happens.
        fn total_distinct_touches_selective(keys: &[[i32; 2]]) -> usize {
            let mut already_meshed: HashSet<[i32; 2]> = HashSet::new();
            let mut touched: HashSet<[i32; 2]> = HashSet::new();
            for key in keys {
                let step = dirty_keys_for_arrival(*key, |k| already_meshed.contains(&k));
                touched.extend(step.iter().copied());
                already_meshed.extend(step);
            }
            touched.len()
        }

        const N: i32 = 30;
        let straight: Vec<[i32; 2]> = (0..N).map(|i| [i, 0]).collect();
        let diagonal: Vec<[i32; 2]> = (0..N).map(|i| [i, i]).collect();

        let straight_touches = total_distinct_touches_selective(&straight);
        let diagonal_touches = total_distinct_touches_selective(&diagonal);

        assert_eq!(
            straight_touches, N as usize,
            "straight frontier distinct-touch count (new, selective rule): exactly one touch per \
             arrival, no redundant neighbour dirtying"
        );
        assert_eq!(
            diagonal_touches, N as usize,
            "diagonal frontier distinct-touch count (new, selective rule): exactly one touch per \
             arrival, no redundant neighbour dirtying"
        );
        assert_eq!(
            straight_touches, diagonal_touches,
            "the selective rule must eliminate this synthetic model's direction-dependent \
             asymmetry entirely (both directions never re-mark a neighbour that hasn't \
             independently streamed in)"
        );
    }

    // -----------------------------------------------------------------------
    // BL-82 EM-3.12 follow-up — `ChunkStoreView` (the zero-allocation
    // `boom_cast` path) mirrors `VolGrid2d::get`'s fallback semantics
    // -----------------------------------------------------------------------

    /// Exercises `ChunkStoreView::get`'s branches directly (present-and-real
    /// chunk / present-but-not-yet-real chunk / out-of-map-bounds default /
    /// genuine in-bounds miss) — the same fallback cases `VolGrid2d::
    /// get_key_arc` distinguishes (`common/src/volumes/vol_grid_2d.rs`), plus
    /// the BL-82 EM-3.11 round-19 "streamed but not yet visually real" case
    /// this follow-up adds. The real-terrain integration test below
    /// (`boom_cast_clamps_against_real_generated_terrain`) only ever
    /// exercises the "present and real" branch (the anchor's own
    /// already-fully-meshed chunk); this test guards the others directly and
    /// cheaply — reviewer-suggested coverage for the zero-allocation view
    /// added in the original follow-up (PR #64), extended for round 19's
    /// "invisible collision" fix.
    ///
    /// The "present and real" case is proven against a GENUINE
    /// `ChunkMeshIndex` produced by the real pipeline (not a hand-faked one —
    /// `ChunkMeshIndex` deliberately has no public insert API, since its
    /// whole contract is "only ever populated once a real mesh lands").
    #[cfg(feature = "listen-server")]
    #[test]
    fn chunk_store_view_mirrors_vol_grid_2d_fallback_semantics() {
        // A real one-chunk pipeline run, purely to get a genuine
        // `ChunkMeshIndex` saying key (0,0) has its real mesh up — decoupled
        // from the hand-built `chunks` map below (this app's own internal
        // terrain store is irrelevant here; only its `ChunkMeshIndex` state
        // is borrowed).
        let mut real_app = test_app();
        real_app
            .world_mut()
            .write_message(CompressedChunk::encode([0, 0], &solid_chunk(5)));
        drain_until(&mut real_app, 500, |app| {
            app.world()
                .resource::<ChunkMeshIndex>()
                .has_real_terrain_mesh(VVec2::new(0, 0))
        });
        let real_mesh_index = real_app.world().resource::<ChunkMeshIndex>();

        let map_size_lg = MapSizeLg::new(VVec2::new(6, 6)).expect("valid map size");
        let default = Arc::new(TerrainChunk::new(
            0,
            Block::empty(),
            Block::empty(),
            TerrainChunkMeta::void(),
        ));
        let mut chunks = HashMap::new();
        chunks.insert([0, 0], Arc::new(solid_chunk(5)));
        // (2, 0) streamed (raw data present) but never meshed — round 19's
        // new case: must behave exactly like "not yet streamed", not "solid".
        // A DIFFERENT key from (1, 0) below (which stays genuinely
        // never-inserted) so the two distinct "miss" scenarios don't collide.
        chunks.insert([2, 0], Arc::new(solid_chunk(5)));
        let empty_mesh_index = ChunkMeshIndex::default();

        let view = ChunkStoreView {
            map_size_lg,
            default: &default,
            chunks: &chunks,
            mesh_index: real_mesh_index,
        };

        // Present AND real chunk: reads the real solid block back.
        assert!(
            view.get(VVec3::new(1, 1, 2))
                .is_ok_and(|b: &Block| b.is_solid()),
            "a stored chunk's own solid block must read back solid once its real mesh is up"
        );

        // Out-of-map-bounds key (map is 2^6 = 64 chunks per axis): falls back
        // to the default (void, non-solid) chunk rather than erroring —
        // mirrors `VolGrid2d::get_key_arc`'s "areas outside the map are
        // *always* considered in it" contract.
        let out_of_bounds = VVec3::new(1000 * CHUNK, 1000 * CHUNK, 2);
        assert!(
            view.get(out_of_bounds).is_ok_and(|b: &Block| !b.is_solid()),
            "an out-of-map-bounds key must resolve to the void default chunk"
        );

        // In-bounds but genuinely not-yet-streamed neighbour key: a real miss.
        let unstreamed_neighbour = VVec3::new(CHUNK, 1, 2);
        assert!(
            view.get(unstreamed_neighbour).is_err(),
            "an in-bounds chunk that was never inserted must be a genuine miss, not a default \
             fallback"
        );

        // BL-82 EM-3.11 round 19: present chunk DATA (key (2,0)) whose real
        // mesh has NOT landed — using the EMPTY index this time — must also
        // be a genuine miss (no clip), not solid. This is the exact
        // "colliding with something invisible" bug this round fixes.
        let streamed_but_unmeshed = ChunkStoreView {
            map_size_lg,
            default: &default,
            chunks: &chunks,
            mesh_index: &empty_mesh_index,
        };
        assert!(
            streamed_but_unmeshed
                .get(VVec3::new(2 * CHUNK + 1, 1, 2))
                .is_err(),
            "a chunk whose raw data streamed in but whose real mesh hasn't landed must NOT clip \
             the boom — the player can't see it yet"
        );
    }

    /// BL-82 EM-3.12 — real-embedded-world integration test. Boots the REAL
    /// Veloren sim (the exact recipe `xindeler-sim-bridge`'s own
    /// `streams_real_chunks_to_local_client` test uses), streams its terrain
    /// through the SAME `TerrainStreamPlugin` this crate ships in production
    /// (loopback `CompressedChunk`s via replicon, decoded into a real
    /// `SharedTerrain`), then casts a downward camera boom against that real
    /// snapshot. This proves the client-side `VolGrid2d` cast genuinely hits
    /// REAL generated terrain end-to-end — the guard the design doc calls
    /// for against the sim/z-up axis convention (a wrong "which way is down"
    /// would clamp against the wrong geometry, or find nothing at all).
    ///
    /// Boots a real world (assets + LFS map blobs) — `#[ignore]`d like its
    /// sim-bridge counterpart; run locally with `VELOREN_ASSETS` set, e.g.:
    /// `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-client
    /// --features listen-server boom_cast_clamps_against_real_generated_terrain
    /// -- --ignored`.
    #[cfg(feature = "listen-server")]
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with VELOREN_ASSETS"]
    fn boom_cast_clamps_against_real_generated_terrain() {
        use std::time::Duration;

        use bevy::{
            state::app::StatesPlugin,
            time::{Fixed, TimeUpdateStrategy},
        };
        use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
        use xindeler_protocol::XindelerProtocolPlugin;
        use xindeler_sim_bridge::{
            SIM_TICK_HZ, SimBridgePlugin, SimTerrainStreamPlugin, boot_test_server,
        };

        const MAX_TICKS: u32 = 8000;
        // Comfortably clears real terrain height variance/voxel-step slop —
        // this is a real generated world, not a hand-built grid, so the
        // tolerance is looser than the pure-function unit tests above.
        const TOLERANCE: f32 = 2.0;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(StatesPlugin)
            .add_plugins(AssetPlugin::default())
            .init_asset::<bevy::mesh::Mesh>()
            // Replicate on every update (default FixedPostUpdate may not run
            // in a manually-stepped app) — same as the sim-bridge test.
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((XindelerProtocolPlugin, SimBridgePlugin, SimTerrainStreamPlugin))
            .add_plugins(ChunkMeshPipelinePlugin)
            .add_plugins(TerrainStreamPlugin)
            .insert_resource(ChunkLayerMap::default())
            .insert_resource(ChunkMaterials {
                terrain: Handle::default(),
                fluid: Handle::default(),
            });
        // Pin FixedUpdate to run exactly once per `app.update()`.
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);
        app.finish();
        app.update(); // Startup: install_provider.

        // First wait for the anchor broadcast (arrives quickly — as soon as
        // the sim can read a spawn altitude, independent of the persister's
        // full view-distance streaming), THEN keep ticking until the
        // anchor's OWN chunk key specifically has streamed in (the
        // persister's queue is not guaranteed centre-first, so waiting on a
        // generic "N chunks streamed" count could keep missing the exact
        // column this test needs).
        let mut anchor: Option<TerrainCameraAnchor> = None;
        let mut anchor_key: Option<[i32; 2]> = None;
        for tick in 0..MAX_TICKS {
            app.update();
            if anchor.is_none()
                && let Some(a) = app.world().get_resource::<TerrainCameraAnchor>()
            {
                anchor = Some(*a);
                // Undo the converter (Veloren (x, y, z) -> Bevy (x, z, -y))
                // to get back the anchor's sim-space horizontal position.
                let anchor_sim_xy = VVec2::new(a.bevy_pos.x, -a.bevy_pos.z);
                let key = VolGrid2d::<TerrainChunk>::chunk_key(VVec2::new(
                    anchor_sim_xy.x.floor() as i32,
                    anchor_sim_xy.y.floor() as i32,
                ));
                anchor_key = Some([key.x, key.y]);
                eprintln!("anchor arrived at tick {tick}: sim_xy={anchor_sim_xy:?} key={key:?}");
            }
            if let Some(key) = anchor_key {
                let has_center = app
                    .world()
                    .resource::<SharedTerrain>()
                    .0
                    .read()
                    .expect("lock")
                    .chunks
                    .contains_key(&key);
                // BL-82 EM-3.11 round 19: `boom_cast` now also requires the
                // chunk's REAL render mesh, not just its raw streamed data
                // (module docs on `TerrainStore::boom_cast`) — wait for both,
                // or this test's later clamp assertion would spuriously see
                // "not yet real" ⇒ no clip.
                let has_real_mesh = app
                    .world()
                    .resource::<ChunkMeshIndex>()
                    .has_real_terrain_mesh(VVec2::new(key[0], key[1]));
                if has_center && has_real_mesh {
                    eprintln!("anchor's own chunk {key:?} streamed AND meshed by tick {tick}");
                    break;
                }
            }
        }
        let anchor = anchor.expect("terrain anchor must have arrived");
        let anchor_key = anchor_key.expect("anchor key must have been computed");

        // Undo the converter (Veloren (x, y, z) -> Bevy (x, z, -y)) to get
        // back the anchor's sim-space horizontal position.
        let anchor_sim_xy = VVec2::new(anchor.bevy_pos.x, -anchor.bevy_pos.z);

        // Find the REAL highest solid block in the anchor's own chunk column
        // by scanning the actual streamed data directly — independent of
        // `boom_cast`/`collide_boom`, so this is a genuine cross-check, not
        // a tautology.
        let ground_top_z = {
            let store = app
                .world()
                .resource::<SharedTerrain>()
                .0
                .read()
                .expect("lock");
            let key = VolGrid2d::<TerrainChunk>::chunk_key(VVec2::new(
                anchor_sim_xy.x.floor() as i32,
                anchor_sim_xy.y.floor() as i32,
            ));
            assert_eq!(
                [key.x, key.y],
                anchor_key,
                "recomputed key must match the one waited on"
            );
            let chunk = store
                .chunks
                .get(&[key.x, key.y])
                .expect("the anchor's own chunk must be streamed by now (waited on above)");
            let edge = CHUNK_EDGE as i32;
            let local = VVec2::new(
                anchor_sim_xy.x.floor() as i32 - key.x * edge,
                anchor_sim_xy.y.floor() as i32 - key.y * edge,
            );
            (chunk.get_min_z()..chunk.get_max_z())
                .rev()
                .find(|&z| {
                    chunk
                        .get(VVec3::new(local.x, local.y, z))
                        .is_ok_and(|b: &Block| b.is_solid())
                })
                .expect("the anchor's column must have some solid ground under it")
        };

        // Cast a downward boom from above the real ground surface. `AIR_GAP`
        // is deliberately modest (well under `CAM_RAY_MAX_ITER`'s reach) —
        // see the note below on why a LARGE gap is the wrong choice for a
        // perfectly-axis-aligned probe ray specifically.
        //
        // ## A real property of the shared DDA (`common/src/ray.rs`), not a
        // ## bug in this feature
        // A ray whose direction is EXACTLY axis-aligned (here, straight down:
        // `dir = (0, 0, -1)`) starting from a position whose coordinate along
        // that axis is itself an exact integer lands EXACTLY back on an
        // integer after every step, which makes `Ray::cast`'s per-step
        // "distance to the next voxel boundary" collapse to the `PLANCK`
        // floor every OTHER iteration (alternating a ~0 step with a ~1.0
        // step) — roughly HALVING the effective distance covered per
        // `max_iter` budget versus a generic, non-axis-aligned ray. Real
        // camera rays (continuous yaw/pitch floats) essentially never hit
        // this exactly, and `CAM_BACK` (9 m) is comfortably inside
        // `CAM_RAY_MAX_ITER`'s reach even at this halved worst-case rate —
        // but a synthetic, perfectly-vertical integration probe over a large
        // gap easily runs the budget out. Keeping `AIR_GAP` modest here
        // avoids exercising that (real, pre-existing, out-of-scope-for-this-
        // feature) DDA property while still genuinely proving the real-world
        // cast/clamp pipeline end-to-end.
        const AIR_GAP: f32 = 12.0;
        let pivot = VVec3::new(
            anchor_sim_xy.x,
            anchor_sim_xy.y,
            ground_top_z as f32 + 1.0 + AIR_GAP,
        );
        let down = VVec3::new(0.0, 0.0, -1.0);
        let terrain = app.world().resource::<SharedTerrain>();
        let mesh_index = app.world().resource::<ChunkMeshIndex>();

        let hit_dist = terrain.boom_cast(pivot, down, AIR_GAP + 10.0, mesh_index);
        let expected = AIR_GAP - crate::player_input::CAM_NEAR_PAD;
        assert!(
            (hit_dist - expected).abs() < TOLERANCE,
            "downward boom should clamp near the real ground surface: got {hit_dist}, expected ≈ \
             {expected}"
        );

        // A short boom that never reaches the ground returns the FULL
        // desired distance — open air, no clip.
        let clear = terrain.boom_cast(pivot, down, 5.0, mesh_index);
        assert_eq!(
            clear, 5.0,
            "a boom cast well short of the real ground must return the full desired distance"
        );
    }
}
