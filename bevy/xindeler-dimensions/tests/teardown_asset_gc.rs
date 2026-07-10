//! BL-82 EM-4.6 (T47.8) measured acceptance: VRAM (mesh/material/texture
//! handle) release verified via Bevy's OWN asset-count diagnostics
//! (`Assets<Mesh>::len()` back to baseline after a `DimensionRoot`
//! cascade-despawn) — spec §1.9's own acceptance bar, "not just 'should be
//! freed by Rust's Drop.'"
//!
//! `xindeler-dimensions` is a SERVER-only crate (no real render pipeline
//! lives here — that's `xindeler-render-voxel`, client-side) so this test
//! proves the GENERIC underlying mechanism instead: Bevy's asset
//! ref-counting (a strong `Handle<Mesh>` keeps `Mesh` alive; the last strong
//! handle dropping frees it from `Assets<Mesh>`) composed with Bevy
//! relationships' cascade-despawn (`DimensionRoot`/`DimensionMembers`,
//! `linked_spawn`) — the SAME two primitives `teardown_completed_dimensions`
//! relies on for a real client's terrain-chunk-mesh/figure entities, just
//! exercised here with a minimal `Component` wrapper instead of the real
//! `Mesh3d`/`MeshMaterial3d` render components (which need `bevy_pbr`, a
//! dependency this crate has no other reason to take on). This is a fast,
//! non-`#[ignore]`d test — no real assets/procgen needed, `DimensionState`
//! is built the same direct-API way `registry.rs`'s own unit tests use.

use std::sync::Arc;

use bevy::{
    MinimalPlugins,
    app::PluginGroup,
    asset::{AssetApp, AssetPlugin, Assets, Handle, RenderAssetUsages},
    ecs::component::Component,
    mesh::{Mesh, PrimitiveTopology},
    prelude::*,
};
use xindeler_dimensions::{DimensionId, DimensionRegistry, DimensionRoot, DimensionsPlugin};

/// Minimal stand-in for the client's real `Mesh3d(Handle<Mesh>)` render
/// component — this crate doesn't depend on `bevy_pbr`/`bevy_render`, so it
/// can't use `Mesh3d` itself, but a plain wrapper `Component` around a
/// strong `Handle<Mesh>` exercises the IDENTICAL ref-counting mechanism.
#[derive(Component)]
struct TestMeshHandle(#[allow(dead_code)] Handle<Mesh>);

fn empty_triangle_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
}

#[test]
fn dimension_teardown_frees_mesh_assets_back_to_baseline() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.build())
        .add_plugins(AssetPlugin::default())
        .init_asset::<Mesh>()
        .add_plugins(DimensionsPlugin);

    // Baseline: nothing loaded yet.
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);

    // Spin up a dimension the SAME direct-API way `registry.rs`'s own tests
    // do (no async worldgen needed — this test is about asset GC, not
    // terrain generation).
    let root = app.world_mut().spawn(DimensionId(1)).id();
    {
        let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
        registry
            .insert_spinning_up(DimensionId(1), root, 0)
            .unwrap();
        let (world, index) = server::World::empty();
        registry
            .complete_spinup(DimensionId(1), Arc::new(world), index)
            .unwrap();
    }

    // Two "mesh-carrying" members, each holding a REAL strong `Handle<Mesh>`
    // to a REAL asset registered in `Assets<Mesh>`.
    let (handle_a, handle_b) = {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        (
            meshes.add(empty_triangle_mesh()),
            meshes.add(empty_triangle_mesh()),
        )
    };
    app.world_mut().spawn((
        DimensionId(1),
        DimensionRoot(root),
        TestMeshHandle(handle_a),
    ));
    app.world_mut().spawn((
        DimensionId(1),
        DimensionRoot(root),
        TestMeshHandle(handle_b),
    ));
    assert_eq!(
        app.world().resource::<Assets<Mesh>>().len(),
        2,
        "both meshes should be live while their handles are held"
    );

    // Drive the dimension to Teardown with zero registered occupants (the
    // registry's own documented "already-empty dimension tears down
    // immediately" rule — nothing to do with the two mesh-holding entities
    // above, which are DimensionMembers, not tracked "occupants").
    {
        let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
        registry
            .begin_draining(DimensionId(1))
            .expect("Active -> Draining is legal");
    }

    // First update: `teardown_completed_dimensions` removes the registry
    // entry + queues the root's despawn. Subsequent updates: the deferred
    // despawn flushes (cascading to both mesh-holding members via
    // `linked_spawn`), and Bevy's own asset-drop-processing system (part of
    // `AssetPlugin`) reclaims the now-unreferenced `Mesh` assets.
    for _ in 0..4 {
        app.update();
    }

    assert!(
        app.world().get_entity(root).is_err(),
        "root should be despawned"
    );
    assert_eq!(
        app.world().resource::<Assets<Mesh>>().len(),
        0,
        "Assets<Mesh> should return to baseline once the last strong Handle<Mesh> drops"
    );
}
