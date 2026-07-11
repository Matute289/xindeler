//! EM-3.5 acceptance — headless pipeline runs (`MinimalPlugins` + task
//! pools, no render/window; `AssetPlugin` + `ChunkMeshPipelinePlugin` over a
//! synthetic 5×5-chunk provider):
//!
//! 1. 25 chunks with budget 2: every chunk completes, NO update ever applies
//!    more chunks than the budget (⇒ ≥ ⌈25/2⌉ updates), the in-flight cap
//!    holds.
//! 2. Palette-hot-reload semantics: an IN-PLACE `ChunkLayerMap` swap + re-mark
//!    re-meshes every chunk with the NEW layers (M1 regression).
//! 3. Unload semantics (`remove_chunk`, EM-3.6 path): cancels queued marks,
//!    in-flight tasks and live entities; provider-`None` re-marks cancel stale
//!    tasks.
//!
//! Completion ORDER is deliberately not asserted (task scheduling +
//! `HashMap` drain order are nondeterministic — documented pipeline
//! contract; determinism is not required by the acceptance).

#![cfg(feature = "pipeline")]

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use bevy::{
    app::App,
    asset::{AssetApp, AssetPlugin, Assets, Handle},
    color::{Color, Luminance},
    ecs::{entity::Entity, query::With},
    mesh::{Mesh3d, VertexAttributeValues},
    pbr::{MeshMaterial3d, StandardMaterial},
    prelude::MinimalPlugins,
};
use common::{
    terrain::{Block, BlockKind, MapSizeLg, SpriteKind, TerrainChunk, TerrainChunkMeta},
    vol::WriteVol,
    volumes::vol_grid_2d::VolGrid2d,
};
use vek::{Rgb, Vec2 as VVec2, Vec3 as VVec3};
use xindeler_render_voxel::{
    convert::ATTRIBUTE_BLOCK_LAYER,
    pipeline::{
        ChunkLayerMap, ChunkMaterials, ChunkMeshIndex, ChunkMeshPipelinePlugin, ChunkMeshQueue,
        ChunkUploadBudget, ChunkUploadStats, ChunkVolume, ChunkVolumeProvider, FluidChunkMesh,
        PlaceholderChunkMesh, PlaceholderHazeTint, TerrainChunkMesh,
    },
};

const CHUNK: i32 = 32;
/// Meshed chunk keys: 1..=GRID on both axes (their ±1 neighbours are
/// populated too so the mesher sees continuous terrain across borders).
const GRID: i32 = 5;
const MAX_Z: i32 = 6;

/// Deterministic synthetic terrain: a low slab whose height varies per
/// world column (some greedy variety, always non-empty).
fn column_height(wx: i32, wy: i32) -> i32 { 2 + (wx.rem_euclid(7) + wy.rem_euclid(5)) % 4 }

/// Builds the 7×7 populated grid (keys 0..=GRID+1) shared by the provider.
fn build_world() -> Arc<VolGrid2d<TerrainChunk>> {
    let map_size_lg = MapSizeLg::new(VVec2::new(3, 3)).expect("valid test map size");
    let default = Arc::new(TerrainChunk::new(
        0,
        Block::empty(),
        Block::empty(),
        TerrainChunkMeta::void(),
    ));
    let mut grid = VolGrid2d::new(map_size_lg, default).expect("chunk size is a power of two");
    for kx in 0..=GRID + 1 {
        for ky in 0..=GRID + 1 {
            let mut chunk =
                TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
            for lx in 0..CHUNK {
                for ly in 0..CHUNK {
                    let h = column_height(kx * CHUNK + lx, ky * CHUNK + ly);
                    for z in 0..h {
                        chunk
                            .set(
                                VVec3::new(lx, ly, z),
                                Block::new(BlockKind::Rock, Rgb::new(120, 120, 120)),
                            )
                            .expect("in-bounds chunk write");
                    }
                }
            }
            grid.insert(VVec2::new(kx, ky), Arc::new(chunk));
        }
    }
    Arc::new(grid)
}

/// App with the pipeline plugin, a provider over [`build_world`] gated by
/// `available` (serving only the 5×5 window while true), and the given
/// upload budget.
fn test_app(budget: u32, available: Arc<AtomicBool>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .init_asset::<bevy::mesh::Mesh>()
        .add_plugins(ChunkMeshPipelinePlugin);

    let world_grid = build_world();
    app.insert_resource(ChunkVolumeProvider::new(move |key| {
        (available.load(Ordering::Relaxed)
            && (1..=GRID).contains(&key.x)
            && (1..=GRID).contains(&key.y))
        .then(|| ChunkVolume::with_z_bounds(world_grid.clone(), key, 0, MAX_Z))
    }))
    .insert_resource(ChunkLayerMap::default())
    .insert_resource(ChunkMaterials {
        terrain: Handle::default(),
        fluid: Handle::default(),
    })
    .insert_resource(ChunkUploadBudget {
        max_uploads_per_frame: budget,
    });
    app
}

fn mark_all(app: &mut App) {
    let mut queue = app.world_mut().resource_mut::<ChunkMeshQueue>();
    for kx in 1..=GRID {
        for ky in 1..=GRID {
            queue.mark_dirty(VVec2::new(kx, ky));
        }
    }
}

/// Updates until the index holds `expected` chunks and nothing is in
/// flight; panics after 5000 updates. Returns (updates, max uploads seen in
/// one update).
fn run_until_complete(app: &mut App, expected: usize) -> (u32, u32) {
    let mut updates = 0;
    let mut max_uploads = 0;
    loop {
        app.update();
        updates += 1;
        let stats = *app.world().resource::<ChunkUploadStats>();
        max_uploads = max_uploads.max(stats.uploads_last_frame);
        let done = app.world().resource::<ChunkMeshIndex>().len() == expected
            && stats.in_flight == 0
            && app.world().resource::<ChunkMeshQueue>().is_empty();
        if done {
            return (updates, max_uploads);
        }
        assert!(
            updates < 5_000,
            "pipeline did not settle at {expected} chunks (index: {}, in flight: {})",
            app.world().resource::<ChunkMeshIndex>().len(),
            stats.in_flight
        );
        // Give the async compute workers a moment between polls.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// All BLOCK_LAYER attribute values across every live terrain chunk mesh.
fn all_terrain_layers(app: &mut App) -> Vec<u32> {
    let world = app.world_mut();
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, With<TerrainChunkMesh>>()
        .iter(world)
        .collect();
    let mut layers = Vec::new();
    for entity in entities {
        let handle = world
            .get::<Mesh3d>(entity)
            .expect("terrain chunk has a mesh")
            .0
            .clone();
        let meshes = world.resource::<Assets<bevy::mesh::Mesh>>();
        let mesh = meshes.get(&handle).expect("mesh asset exists");
        let Some(VertexAttributeValues::Uint32(values)) = mesh.attribute(ATTRIBUTE_BLOCK_LAYER.id)
        else {
            panic!("BlockLayer must be Uint32");
        };
        layers.extend(values);
    }
    layers
}

fn terrain_entity_count(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<TerrainChunkMesh>>()
        .iter(world)
        .count()
}

fn fluid_entity_count(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<FluidChunkMesh>>()
        .iter(world)
        .count()
}

/// A 3×3 grid of chunks whose surface is a rock slab topped with a layer of
/// water blocks — the mesher emits BOTH an opaque and a fluid mesh, exercising
/// the EM-3.9 fluid spawn path.
fn build_water_world() -> Arc<VolGrid2d<TerrainChunk>> {
    let map_size_lg = MapSizeLg::new(VVec2::new(3, 3)).expect("valid test map size");
    let default = Arc::new(TerrainChunk::new(
        0,
        Block::empty(),
        Block::empty(),
        TerrainChunkMeta::void(),
    ));
    let mut grid = VolGrid2d::new(map_size_lg, default).expect("chunk size is a power of two");
    for kx in 0..=2 {
        for ky in 0..=2 {
            let mut chunk =
                TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
            for lx in 0..CHUNK {
                for ly in 0..CHUNK {
                    // Rock floor z=0..2, then a water block at z=2.
                    for z in 0..2 {
                        chunk
                            .set(
                                VVec3::new(lx, ly, z),
                                Block::new(BlockKind::Rock, Rgb::new(120, 120, 120)),
                            )
                            .expect("in-bounds chunk write");
                    }
                    chunk
                        .set(VVec3::new(lx, ly, 2), Block::water(SpriteKind::Empty))
                        .expect("in-bounds water write");
                }
            }
            grid.insert(VVec2::new(kx, ky), Arc::new(chunk));
        }
    }
    Arc::new(grid)
}

/// App over the water world (center key (1,1) meshed, neighbours populated).
fn water_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .init_asset::<bevy::mesh::Mesh>()
        .add_plugins(ChunkMeshPipelinePlugin);

    let world_grid = build_water_world();
    app.insert_resource(ChunkVolumeProvider::new(move |key| {
        (key.x == 1 && key.y == 1)
            .then(|| ChunkVolume::with_z_bounds(world_grid.clone(), key, 0, 4))
    }))
    .insert_resource(ChunkLayerMap::default())
    .insert_resource(ChunkMaterials {
        terrain: Handle::default(),
        fluid: Handle::default(),
    })
    .insert_resource(ChunkUploadBudget {
        max_uploads_per_frame: 2,
    });
    app
}

/// EM-3.9: a chunk with water meshes into BOTH a terrain and a fluid entity
/// (the fluid spawn path the listen-server water rendering relies on).
#[test]
fn water_chunk_spawns_a_fluid_entity() {
    let mut app = water_test_app();
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(VVec2::new(1, 1));
    run_until_complete(&mut app, 1);

    assert_eq!(
        terrain_entity_count(&mut app),
        1,
        "the rock floor meshes into a terrain entity"
    );
    assert_eq!(
        fluid_entity_count(&mut app),
        1,
        "the water layer meshes into a fluid entity (EM-3.9 spawn path)"
    );
    let index = app.world().resource::<ChunkMeshIndex>();
    let entities = index
        .get(VVec2::new(1, 1))
        .expect("the meshed key is indexed");
    assert!(entities.terrain.is_some());
    assert!(
        entities.fluid.is_some(),
        "water chunk carries a fluid entity"
    );
}

#[test]
fn pipeline_meshes_25_chunks_within_budget() {
    const BUDGET: u32 = 2;
    let mut app = test_app(BUDGET, Arc::new(AtomicBool::new(true)));

    {
        let mut queue = app.world_mut().resource_mut::<ChunkMeshQueue>();
        for kx in 1..=GRID {
            for ky in 1..=GRID {
                queue.mark_dirty(VVec2::new(kx, ky));
            }
        }
        // Duplicates dedupe; an out-of-window key is dropped by the provider.
        queue.mark_dirty(VVec2::new(1, 1));
        queue.mark_dirty(VVec2::new(100, 100));
        assert_eq!(queue.len(), 26);
    }

    let mut updates: u32 = 0;
    let mut max_uploads_seen: u32 = 0;
    loop {
        app.update();
        updates += 1;
        let stats = *app.world().resource::<ChunkUploadStats>();
        assert!(
            stats.uploads_last_frame <= BUDGET,
            "update {updates} uploaded {} chunks (> budget {BUDGET})",
            stats.uploads_last_frame
        );
        // m5: the in-flight cap (budget × 8) must hold.
        assert!(
            stats.in_flight <= (BUDGET * 8) as usize,
            "update {updates} had {} tasks in flight (> cap)",
            stats.in_flight
        );
        max_uploads_seen = max_uploads_seen.max(stats.uploads_last_frame);
        if app.world().resource::<ChunkMeshIndex>().len() == 25 && stats.in_flight == 0 {
            break;
        }
        assert!(
            updates < 5_000,
            "pipeline did not finish 25 chunks (index: {}, in flight: {})",
            app.world().resource::<ChunkMeshIndex>().len(),
            stats.in_flight
        );
        // Give the async compute workers a moment between polls.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let stats = *app.world().resource::<ChunkUploadStats>();
    assert_eq!(stats.total_uploads, 25, "every chunk applied exactly once");
    assert!(
        updates >= 25_u32.div_ceil(BUDGET),
        "budget {BUDGET} cannot finish 25 chunks in {updates} updates"
    );
    assert!(
        max_uploads_seen > 0,
        "the stats counter must actually count"
    );

    // 25 terrain entities with the marker exist; the synthetic slab has no
    // water, so no fluid entities.
    assert_eq!(terrain_entity_count(&mut app), 25);
    let index = app.world().resource::<ChunkMeshIndex>();
    for kx in 1..=GRID {
        for ky in 1..=GRID {
            let entities = index
                .get(VVec2::new(kx, ky))
                .expect("every meshed key is indexed");
            assert!(entities.terrain.is_some(), "slab terrain is never empty");
            assert!(entities.fluid.is_none(), "no water in the test slab");
        }
    }
}

/// BL-82 EM-3.11n — a large dirty backlog (all 25 chunks marked at once, far
/// more than fit under the in-flight cap in one go) must not FETCH more than
/// a small, bounded number of volumes in a SINGLE frame.
/// `ChunkVolumeProvider::fetch` runs synchronously on
/// `spawn_chunk_mesh_tasks`'s own (main) thread — only `generate_mesh` itself
/// is off-thread — so an unbounded per-frame fetch count would be a real
/// main-thread cost spike proportional to backlog size. This is exactly the
/// mechanism investigated for the "diagonal movement feels choppier" report
/// (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md` round 8): a
/// diagonal streaming frontier backlogs structurally more distinct chunks
/// per unit distance than a straight one for the same real speed
/// (`xindeler-client`'s `terrain_stream.rs::neighbourhood_3x3` docs), so its
/// bursts are bigger — this test guards that ANY burst, regardless of size,
/// gets its main-thread fetch cost spread over multiple frames instead of
/// paid all at once.
#[test]
fn large_backlog_does_not_fetch_more_than_the_spawn_burst_cap_in_one_frame() {
    const BUDGET: u32 = 2;
    // Mirrors `pipeline.rs`'s private `SPAWN_BURST_FACTOR` (4) — pinned here
    // as a literal since the constant isn't public; update this expectation
    // too if that factor ever changes.
    const EXPECTED_SPAWN_CAP: usize = (BUDGET * 4) as usize;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .init_asset::<bevy::mesh::Mesh>()
        .add_plugins(ChunkMeshPipelinePlugin);

    let world_grid = build_world();
    let fetch_count = Arc::new(AtomicUsize::new(0));
    let counter = fetch_count.clone();
    app.insert_resource(ChunkVolumeProvider::new(move |key| {
        counter.fetch_add(1, Ordering::Relaxed);
        ((1..=GRID).contains(&key.x) && (1..=GRID).contains(&key.y))
            .then(|| ChunkVolume::with_z_bounds(world_grid.clone(), key, 0, MAX_Z))
    }))
    .insert_resource(ChunkLayerMap::default())
    .insert_resource(ChunkMaterials {
        terrain: Handle::default(),
        fluid: Handle::default(),
    })
    .insert_resource(ChunkUploadBudget {
        max_uploads_per_frame: BUDGET,
    });

    // 25 chunks, all dirty before the pipeline ever runs a single frame —
    // the biggest possible burst for this window.
    {
        let mut queue = app.world_mut().resource_mut::<ChunkMeshQueue>();
        for kx in 1..=GRID {
            for ky in 1..=GRID {
                queue.mark_dirty(VVec2::new(kx, ky));
            }
        }
    }

    app.update(); // exactly one frame

    let fetched = fetch_count.load(Ordering::Relaxed);
    assert!(
        fetched > 0,
        "sanity: the pipeline must still make progress on the very first frame"
    );
    assert!(
        fetched <= EXPECTED_SPAWN_CAP,
        "expected at most {EXPECTED_SPAWN_CAP} fetches in one frame from a 25-chunk backlog, got \
         {fetched} — the per-frame spawn-burst cap regressed"
    );
}

/// M1 regression: the palette hot-reload contract is "swap the
/// `ChunkLayerMap` Arc IN PLACE (ResMut), then re-mark" — after that, every
/// re-meshed chunk must carry the NEW layers. (A deferred
/// `commands.insert_resource` would let the re-marked chunks drain against
/// the stale map; this test drives the exact sequence the client performs.)
#[test]
fn in_place_layer_map_swap_remaps_meshes() {
    let mut app = test_app(25, Arc::new(AtomicBool::new(true)));

    mark_all(&mut app);
    run_until_complete(&mut app, 25);
    let layers = all_terrain_layers(&mut app);
    assert!(!layers.is_empty());
    assert!(
        layers.iter().all(|&layer| layer == 0),
        "default LUT maps everything to layer 0"
    );

    // Simulated palette reload: in-place swap + re-mark all (client M1 fix).
    app.world_mut().resource_mut::<ChunkLayerMap>().0 = Arc::new([7; 256]);
    mark_all(&mut app);
    run_until_complete(&mut app, 25);

    let layers = all_terrain_layers(&mut app);
    assert!(!layers.is_empty());
    assert!(
        layers.iter().all(|&layer| layer == 7),
        "after the in-place swap + re-mark, every mesh must use the new layers"
    );
    let stats = *app.world().resource::<ChunkUploadStats>();
    assert_eq!(stats.total_uploads, 50, "all 25 chunks re-meshed");
}

/// M2: `remove_chunk` unload path + the provider-`None` cancellation edge.
#[test]
fn remove_chunk_cancels_and_despawns() {
    let available = Arc::new(AtomicBool::new(true));
    let key = VVec2::new(2, 2);
    let mut app = test_app(4, available.clone());

    // Phase A — mark → remove BEFORE any pipeline run: the mark is
    // cancelled, nothing meshes, nothing spawns.
    {
        let mut queue = app.world_mut().resource_mut::<ChunkMeshQueue>();
        queue.mark_dirty(key);
        queue.remove_chunk(key);
        assert!(queue.is_empty(), "remove cancels the pending mark");
    }
    app.update();
    let stats = *app.world().resource::<ChunkUploadStats>();
    assert_eq!(stats.in_flight, 0);
    assert_eq!(stats.total_uploads, 0);
    assert!(app.world().resource::<ChunkMeshIndex>().is_empty());
    assert_eq!(terrain_entity_count(&mut app), 0);

    // Phase B — mesh the chunk for real, then remove it: entity + index
    // entry are gone on the next pipeline run and never come back.
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    run_until_complete(&mut app, 1);
    assert_eq!(terrain_entity_count(&mut app), 1);
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .remove_chunk(key);
    app.update();
    assert!(app.world().resource::<ChunkMeshIndex>().is_empty());
    assert_eq!(terrain_entity_count(&mut app), 0);
    let uploads_after_remove = app.world().resource::<ChunkUploadStats>().total_uploads;

    // Phase C — provider-None re-mark cancels an in-flight task ("last
    // write wins" also covers unloads): mark, let the task spawn, then flip
    // the provider off and re-mark. Whatever the task-timing race (it may
    // already have applied inside the first update), the END state is
    // deterministic once we remove: no task, no entity, no further uploads.
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update(); // task spawned (and possibly already applied)
    available.store(false, Ordering::Relaxed);
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update(); // provider None ⇒ cancels the in-flight task if any
    assert_eq!(
        app.world().resource::<ChunkUploadStats>().in_flight,
        0,
        "a provider-None re-mark must cancel the in-flight task"
    );
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .remove_chunk(key);
    app.update();
    // A cancelled task must never apply afterwards.
    for _ in 0..25 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(app.world().resource::<ChunkMeshIndex>().is_empty());
    assert_eq!(terrain_entity_count(&mut app), 0);
    let final_uploads = app.world().resource::<ChunkUploadStats>().total_uploads;
    assert!(
        final_uploads <= uploads_after_remove + 1,
        "at most the phase-C pre-flip apply may have landed (got {final_uploads})"
    );
}

// ---------------------------------------------------------------------------
// BL-82 EM-3.11h — first-load placeholder (the "black frame" fix)
// ---------------------------------------------------------------------------

/// App identical to [`test_app`] but ALSO registers `Assets<StandardMaterial>`
/// — [`spawn_chunk_mesh_tasks`]'s first-load placeholder only spawns when
/// that asset store exists (a deliberately defensive gate so hosts/tests
/// that don't register it, like every OTHER test in this file, are
/// unaffected — see the pipeline module docs). Every EM-3.11h-specific test
/// below uses this builder instead of the plain one.
///
/// [`spawn_chunk_mesh_tasks`]: xindeler_render_voxel::pipeline
fn test_app_with_placeholders(budget: u32, available: Arc<AtomicBool>) -> App {
    let mut app = test_app(budget, available);
    app.init_asset::<StandardMaterial>();
    app
}

fn placeholder_entity_count(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<PlaceholderChunkMesh>>()
        .iter(world)
        .count()
}

/// The core BL-82 EM-3.11h regression: a key's FIRST ever mark must never
/// leave a frame where NOTHING is indexed for it — that gap is exactly what
/// let a real gameplay capture show ~2 fully black frames (only the UI
/// overlay visible) while walking into a cave, because the far-mesh's
/// camera-proximity cutout (`xindeler-client::far_terrain`) deliberately
/// never covers this band either, trusting the near pipeline to. One
/// `app.update()` after `mark_dirty` must already show a spawned, indexed
/// entity (the synchronous placeholder in the common case — real
/// completion within one update is not impossible but never observed
/// elsewhere in this suite's much larger 25-chunk timing tests, which all
/// need many updates + sleeps to converge, so either outcome proves the
/// invariant this test cares about: never nothing).
#[test]
fn first_mark_never_leaves_a_frame_with_nothing_indexed() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();

    assert!(
        app.world().resource::<ChunkMeshIndex>().get(key).is_some(),
        "the very first update after marking a NEW key dirty must already index an entity for it \
         (placeholder or real) — never nothing"
    );
    assert_eq!(
        terrain_entity_count(&mut app),
        1,
        "exactly one TerrainChunkMesh entity must cover the key's footprint"
    );

    // Eventually the real mesh replaces the placeholder (the pre-existing
    // atomic despawn-old+spawn-new swap, unmodified) — no leftover
    // placeholder once the pipeline settles.
    run_until_complete(&mut app, 1);
    assert_eq!(
        placeholder_entity_count(&mut app),
        0,
        "the placeholder must be gone once the real mesh has uploaded"
    );
    assert_eq!(terrain_entity_count(&mut app), 1);
}

/// A SECOND `mark_dirty` of the SAME never-yet-indexed key (e.g. a border
/// re-mark racing the first one) must not spawn a second placeholder:
/// `spawn_chunk_mesh_tasks` only inserts one while the key is absent from
/// the index, and the index already gained an entry in the same update the
/// first placeholder spawned.
#[test]
fn repeated_marks_of_a_pending_key_spawn_only_one_placeholder() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();

    assert!(
        placeholder_entity_count(&mut app) <= 1,
        "a repeated mark of the same pending key must never spawn a second placeholder"
    );
}

/// EM-3.11h cleanup edge case: if the volume disappears (provider starts
/// returning `None` for the key) WHILE its first-load placeholder is still
/// up (task not yet finished — the common case; see
/// `first_mark_never_leaves_a_frame_with_nothing_indexed`'s doc for why a
/// single-update completion race is possible but unlikely), the same
/// provider-`None` re-mark path that already cancelled the in-flight task
/// (pre-existing behaviour, `remove_chunk_cancels_and_despawns`'s Phase C)
/// must ALSO despawn the placeholder — not leak it forever. Asserting
/// `placeholder_entity_count == 0` (rather than a stronger, race-prone
/// `ChunkMeshIndex` emptiness check) stays deterministic even in the rare
/// case the task finished first: either way, no placeholder may survive a
/// provider-`None` re-mark of its own key.
#[test]
fn abandoned_placeholder_is_cleaned_up_when_the_volume_disappears() {
    let available = Arc::new(AtomicBool::new(true));
    let key = VVec2::new(3, 3);
    let mut app = test_app_with_placeholders(1, available.clone());

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();

    available.store(false, Ordering::Relaxed);
    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();

    assert_eq!(
        placeholder_entity_count(&mut app),
        0,
        "no placeholder may survive a provider-None re-mark of its own key"
    );
}

/// EM-3.11i regression: a real gameplay capture showed the placeholder could
/// still render as a solid, hard-edged BLACK box in dim/cave terrain —
/// exactly the black-frame symptom EM-3.11h existed to fix — because a
/// normally-lit `StandardMaterial` box viewed from its own interior
/// self-shadows to black regardless of `base_color` (pipeline module docs).
/// This headless suite has no renderer, so it cannot measure on-screen
/// pixels; what IS checkable at the material-definition level is the
/// property that makes black-under-any-lighting impossible in the first
/// place: `unlit: true` (an unlit fragment outputs `base_color`
/// unconditionally, with no lighting term to zero out) plus a `base_color`
/// whose own luminance sits comfortably above zero. Whether the result reads
/// as "acceptably visible, not garish" in actual gameplay is a human call
/// the module docs are honest about — this test only guards the regression
/// that made the placeholder go pitch black.
#[test]
fn placeholder_material_is_unlit_with_a_visible_floor() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();

    let world = app.world_mut();
    let handle = world
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<PlaceholderChunkMesh>>()
        .iter(world)
        .next()
        .expect("the placeholder must have spawned with a material")
        .0
        .clone();
    let materials = world.resource::<Assets<StandardMaterial>>();
    let material = materials
        .get(&handle)
        .expect("the placeholder's material handle must resolve to a real asset");

    assert!(
        material.unlit,
        "the placeholder material must be unlit: a normally-lit material can legitimately render \
         fully black wherever the scene provides it no light (EM-3.11i finding — the camera \
         routinely stands INSIDE this box, self-shadowing its own interior), which defeats the \
         whole point of a first-load placeholder"
    );
    let floor = material.base_color.luminance();
    assert!(
        floor > 0.05,
        "the placeholder's base_color must have a comfortably-visible floor luminance (got \
         {floor}) — with `unlit: true` this IS its on-screen brightness in every scene, dark cave \
         included"
    );
}

/// BL-82 EM-3.11 round 14 regression: Matías reported the distant tree/
/// mountain background periodically disappearing for 1-2 frames while
/// walking, root-caused (pipeline module docs, round-14 section) to
/// `DistanceFog` washing this UNLIT placeholder's own "obviously a
/// placeholder" rock-grey out to the pale fog/sky colour at the exact
/// render-distance band where a never-before-meshed chunk typically
/// appears — reading as empty sky rather than a crude stand-in box. Pins
/// `fog_enabled: false` so a future edit can't silently reintroduce the
/// washout (this headless suite has no renderer to measure the actual
/// on-screen fog blend, so the material-definition property that makes the
/// washout impossible in the first place is what's checkable here, same
/// approach as the sibling `unlit`/floor-luminance assertions above).
#[test]
fn placeholder_material_is_immune_to_distance_fog() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();

    let world = app.world_mut();
    let handle = world
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<PlaceholderChunkMesh>>()
        .iter(world)
        .next()
        .expect("the placeholder must have spawned with a material")
        .0
        .clone();
    let materials = world.resource::<Assets<StandardMaterial>>();
    let material = materials
        .get(&handle)
        .expect("the placeholder's material handle must resolve to a real asset");

    assert!(
        !material.fog_enabled,
        "the placeholder material must disable DistanceFog (BL-82 EM-3.11 round 14): fog \
         application is gated only by `fog_enabled`, never by `unlit`, so at the ~90-99%-opaque \
         render-distance band where new chunks stream in, a fog-enabled placeholder washes out to \
         the pale fog/sky colour and reads as empty background rather than a visible placeholder"
    );
}

/// BL-82 EM-3.11 follow-up (2026-07-11): with no host-installed
/// [`PlaceholderHazeTint`] — the common case for any host that doesn't track
/// atmosphere (e.g. the synthetic voxel-demo) — the placeholder must stay
/// its plain rock-grey, byte-for-byte the same as round 14. `sync_
/// placeholder_haze` must be a strict no-op in this configuration, not just
/// "close enough".
#[test]
fn placeholder_material_stays_untinted_without_a_host_haze() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();
    app.update(); // give `sync_placeholder_haze` a chance to run at all

    let world = app.world_mut();
    let handle = world
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<PlaceholderChunkMesh>>()
        .iter(world)
        .next()
        .expect("the placeholder must have spawned with a material")
        .0
        .clone();
    let materials = world.resource::<Assets<StandardMaterial>>();
    let material = materials
        .get(&handle)
        .expect("the placeholder's material handle must resolve to a real asset");

    assert_eq!(
        material.base_color.to_srgba(),
        Color::srgb(0.35, 0.33, 0.30).to_srgba(),
        "with no PlaceholderHazeTint installed, base_color must stay exactly the round-14 \
         rock-grey — no host means no tint, not a default tint"
    );
}

/// BL-82 EM-3.11 follow-up (2026-07-11) — the reviewer-flagged, now-confirmed
/// seam artifact (pipeline module docs): a live frozen-camera capture showed
/// the round-14 fog-immune placeholder reading as a starkly flat, hard-edged
/// box against the already-heavily-hazed far mesh/real-terrain right at
/// `chunk_render_distance`. `PlaceholderHazeTint` fixes this by blending the
/// placeholder's base colour toward a host-installed haze colour — this test
/// pins that the blend actually happens, and that it is CAPPED (not a full
/// wash to the tint colour, which would just reintroduce round 14's bug by a
/// different mechanism).
#[test]
fn placeholder_material_blends_toward_a_host_installed_haze_tint() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    let tint = Color::srgb(0.66, 0.73, 0.81); // a plausible pale fog colour
    app.world_mut().insert_resource(PlaceholderHazeTint(tint));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();
    app.update(); // let sync_placeholder_haze run against the now-existing material

    let world = app.world_mut();
    let handle = world
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<PlaceholderChunkMesh>>()
        .iter(world)
        .next()
        .expect("the placeholder must have spawned with a material")
        .0
        .clone();
    let materials = world.resource::<Assets<StandardMaterial>>();
    let material = materials
        .get(&handle)
        .expect("the placeholder's material handle must resolve to a real asset");

    let base = Color::srgb(0.35, 0.33, 0.30).to_srgba();
    let got = material.base_color.to_srgba();
    let haze = tint.to_srgba();

    assert_ne!(
        got, base,
        "with a PlaceholderHazeTint installed, base_color must move away from the plain rock-grey"
    );
    assert_ne!(
        got, haze,
        "the blend must be CAPPED, not a full wash to the tint colour — a full wash would \
         reintroduce round 14's bug (the placeholder becoming indistinguishable from the fog/sky \
         colour) by a different mechanism"
    );
    // Every channel must sit strictly between base and haze (a partial lerp),
    // not overshoot past either endpoint.
    for (b, h, g) in [
        (base.red, haze.red, got.red),
        (base.green, haze.green, got.green),
        (base.blue, haze.blue, got.blue),
    ] {
        let (lo, hi) = (b.min(h), b.max(h));
        assert!(
            g >= lo - 1e-4 && g <= hi + 1e-4,
            "blended channel {g} must lie between base {b} and haze {h}"
        );
    }
}

/// BL-82 EM-3.11 round 16: a live frozen-camera capture (findings log) showed
/// round 15's 0.2 blend left the placeholder's ABSOLUTE colour genuinely
/// near-neutral (verified: sampled real pixels around `(98,98,98)`-
/// `(103,103,101)`) yet reading as a distinctly warm "tan/beige" patch by
/// SIMULTANEOUS CONTRAST against the cooler blue-hazed far mesh/sky it
/// typically sits next to — a live before/after capture confirmed a stronger
/// blend measurably softens this (the box's own sampled colour moved from
/// R≈G≈B to a clearly blue-leaning B>G>R, matching the direction of its
/// actual surroundings instead of standing out as the one neutral thing in a
/// blue scene). This pins the blend at (or past) the midpoint — closer to
/// the haze tint than to the plain base colour — so a future accidental
/// revert back toward round 15's under-correcting 0.2 fails a test instead
/// of silently reintroducing the tan/beige patch.
#[test]
fn placeholder_material_blend_leans_more_toward_the_haze_tint_than_round_15_did() {
    let mut app = test_app_with_placeholders(2, Arc::new(AtomicBool::new(true)));
    // A cool, desaturated blue haze tint (matches `AtmosphereProfile::
    // default().fog_color`, `(0.66, 0.73, 0.81)`) against a warm-leaning base
    // — the exact real-world shape of the round-16 bug.
    let tint = Color::srgb(0.66, 0.73, 0.81);
    app.world_mut().insert_resource(PlaceholderHazeTint(tint));
    let key = VVec2::new(3, 3);

    app.world_mut()
        .resource_mut::<ChunkMeshQueue>()
        .mark_dirty(key);
    app.update();
    app.update();

    let world = app.world_mut();
    let handle = world
        .query_filtered::<&MeshMaterial3d<StandardMaterial>, With<PlaceholderChunkMesh>>()
        .iter(world)
        .next()
        .expect("the placeholder must have spawned with a material")
        .0
        .clone();
    let materials = world.resource::<Assets<StandardMaterial>>();
    let material = materials
        .get(&handle)
        .expect("the placeholder's material handle must resolve to a real asset");

    let base = Color::srgb(0.35, 0.33, 0.30).to_srgba();
    let got = material.base_color.to_srgba();
    let haze = tint.to_srgba();

    for (b, h, g, channel) in [
        (base.red, haze.red, got.red, "red"),
        (base.green, haze.green, got.green, "green"),
        (base.blue, haze.blue, got.blue, "blue"),
    ] {
        let dist_to_base = (g - b).abs();
        let dist_to_haze = (g - h).abs();
        assert!(
            dist_to_haze <= dist_to_base + 1e-4,
            "round 16: the {channel} channel must sit at or past the midpoint between base ({b}) \
             and haze ({h}) — got {g}, which is closer to the plain base colour than to the haze \
             tint (distance to base {dist_to_base} vs distance to haze {dist_to_haze}); round \
             15's under-correcting 0.2 blend left an absolute near-neutral colour that still read \
             as a warm tan/beige patch by simultaneous contrast against a cooler scene — see the \
             findings log's round-16 section"
        );
    }
}
