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
    atomic::{AtomicBool, Ordering},
};

use bevy::{
    app::App,
    asset::{AssetApp, AssetPlugin, Assets, Handle},
    ecs::{entity::Entity, query::With},
    mesh::{Mesh3d, VertexAttributeValues},
    prelude::MinimalPlugins,
};
use common::{
    terrain::{Block, BlockKind, MapSizeLg, TerrainChunk, TerrainChunkMeta},
    vol::WriteVol,
    volumes::vol_grid_2d::VolGrid2d,
};
use vek::{Rgb, Vec2 as VVec2, Vec3 as VVec3};
use xindeler_render_voxel::{
    convert::ATTRIBUTE_BLOCK_LAYER,
    pipeline::{
        ChunkLayerMap, ChunkMaterials, ChunkMeshIndex, ChunkMeshPipelinePlugin, ChunkMeshQueue,
        ChunkUploadBudget, ChunkUploadStats, ChunkVolume, ChunkVolumeProvider, TerrainChunkMesh,
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
