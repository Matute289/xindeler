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
use common::{
    terrain::{Block, BlockKind, MapSizeLg, TerrainChunk, TerrainChunkMeta},
    vol::{ReadVol, RectRasterableVol},
    volumes::vol_grid_2d::VolGrid2d,
};
// vek only for the terrain grid keys/coords; `Vec3` here is Bevy's (prelude).
use vek::{Vec2 as VVec2, Vec3 as VVec3};
use xindeler_protocol::{CompressedChunk, RemoveChunk, TerrainAnchor};
use xindeler_render_voxel::pipeline::{ChunkKey, ChunkMeshQueue, ChunkVolume, ChunkVolumeProvider};

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
}

/// Handle shared with the provider closure.
#[derive(Resource, Clone)]
struct SharedTerrain(Arc<RwLock<TerrainStore>>);

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
                    receive_chunks,
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

/// Decodes incoming chunks into the store and marks them (and their meshed
/// neighbours) dirty so the pipeline (re)meshes with correct borders.
fn receive_chunks(
    mut chunks: MessageReader<CompressedChunk>,
    shared: Res<SharedTerrain>,
    mut queue: ResMut<ChunkMeshQueue>,
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
    for key in &touched {
        // A new/changed chunk affects its neighbours' shared-border meshing
        // too, so mark the whole 3×3 neighbourhood dirty (`ChunkMeshQueue`
        // dedupes an already-queued key, so re-marking `key` itself here is
        // harmless — see [`neighbourhood_3x3`]'s docs for why this exact
        // footprint matters for BL-82 EM-3.11n's diagonal-streaming finding).
        for nk in neighbourhood_3x3(*key) {
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
/// 9-element array. Shared by [`receive_chunks`] (marks the WHOLE 3×3 dirty:
/// a new/changed chunk affects its neighbours' shared-border meshing too) and
/// [`receive_removes`] (marks just the 8 neighbours, skipping the removed
/// key itself).
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
/// straight one (see the `diagonal_streaming_touches_more_distinct_chunks_*`
/// test below for the exact numbers). More distinct chunks marked dirty means
/// more async re-mesh tasks competing for the same fixed-size
/// `ChunkUploadBudget` (`pipeline.rs`) and `AsyncComputeTaskPool` capacity —
/// this doesn't by itself prove a frame-time regression (meshing is off the
/// main thread and uploads are budget-capped regardless of backlog size), but
/// it is a real, structural, direction-dependent difference in streaming load
/// worth ruling in/out empirically alongside the `--smoke-perf-run` live A/B
/// harness (`crate::smoke`).
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

    /// BL-82 EM-3.11n regression test — see [`neighbourhood_3x3`]'s doc
    /// comment for the full reasoning. Deterministic, no GPU/timing involved:
    /// for the SAME number `N` of newly-streamed chunk keys (i.e. the same
    /// real distance travelled, since the server streams one new chunk per
    /// grid-line crossing regardless of direction), a diagonal streaming
    /// frontier (`(0,0),(1,1),(2,2),…`) touches (dirty-marks) strictly MORE
    /// distinct chunk keys than a straight one (`(0,0),(1,0),(2,0),…`)
    /// because consecutive diagonal arrivals' 3×3 neighbourhoods overlap by
    /// only 2×2=4 cells vs. the straight case's 2×3=6 cells.
    #[test]
    fn diagonal_streaming_touches_more_distinct_chunks_than_straight() {
        use std::collections::HashSet;

        /// Total distinct chunk keys touched (dirty-marked) across an entire
        /// streaming run — the union of every arrival's 3×3 neighbourhood.
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
            "straight frontier distinct-touch count"
        );
        assert_eq!(
            diagonal_touches, 154,
            "diagonal frontier distinct-touch count"
        );
        assert!(
            diagonal_touches > straight_touches,
            "diagonal frontier ({diagonal_touches} distinct touched chunks) must exceed the \
             straight frontier ({straight_touches}) for the same {N}-chunk streaming run"
        );
        // The ratio approaches 5/3 ≈ 1.667 as N grows; at N=30 it's already
        // past 1.5 — a real, structural ~60% more remesh churn for the same
        // streamed distance.
        let ratio = f64::from(diagonal_touches as u32) / f64::from(straight_touches as u32);
        assert!(
            ratio > 1.5,
            "expected the diagonal/straight distinct-touch ratio to approach ~1.667 (5/3 marginal \
             cells per arrival), got {ratio:.3}"
        );
    }
}
