//! EM-3.10 — LOD & culling v1: distance-band culling of streamed terrain +
//! sprites (listen-server only).
//!
//! ## What Bevy already gives us for FREE (verified against bevy 0.19 sources)
//! - `VisibilityPlugin::calculate_bounds` (system set
//!   `VisibilitySystems::CalculateBounds`) auto-adds an `Aabb` to every entity
//!   that carries `Mesh3d` and lacks `NoFrustumCulling` — that is the chunk
//!   opaque + fluid meshes (`pipeline.rs`) AND every per-instance sprite child
//!   (`sprite_view.rs`). So `check_visibility` FRUSTUM-culls all of them with
//!   zero work on our part: chunks/sprites behind or beside the camera are
//!   never drawn. A per-chunk sprite PARENT holds only children (no `Mesh3d`) →
//!   it gets no `Aabb` → `check_visibility` treats it as always-visible, but it
//!   never draws anything itself; its children are each frustum-culled on their
//!   own `Aabb`.
//! - `VisibilityPlugin` also `register_required_components::<Mesh3d,
//!   Visibility>()`, so every meshed entity already owns a `Visibility` we can
//!   toggle here (no need to insert one at spawn).
//!
//! ## What Bevy does NOT do → this module
//! Bevy culls by frustum + (opt-in) occlusion, but NOT by DISTANCE from the
//! viewer, and it does nothing about the sprite population being the dominant
//! entity count. So EM-3.10 v1 adds two cheap per-frame distance bands measured
//! from the camera, toggling `Visibility` only (we never despawn — that stays
//! the terrain stream's job; we just hide/show what it already spawned, so we
//! never fight its lifecycle):
//! - **Chunk band** ([`CullingConfig::chunk_render_distance`]): a hard render
//!   cap that hides chunk meshes whose chunk-centre is beyond it. It sits just
//!   inside the far corners of the streamed square, so the near/mid view is
//!   untouched; frustum culling already drops everything behind the camera, so
//!   this only trims the far ring the stream keeps loaded but the player can't
//!   meaningfully see.
//! - **Sprite band** ([`CullingConfig::sprite_render_distance`], NEARER): hides
//!   a whole per-chunk sprite PARENT beyond it. Sprites are the dominant entity
//!   count (up to `MAX_SPRITES_PER_CHUNK` per chunk), so hiding a far parent
//!   skips its ENTIRE subtree through `InheritedVisibility` in a single
//!   component write — the single biggest per-frame win, and far cheaper than
//!   letting `check_visibility` frustum-test every distant grass blade.
//!
//! ## Deferred to EM-3.10b (documented, NOT half-shipped here)
//! - **GPU occlusion culling**: Bevy 0.19 has `OcclusionCulling` (two-phase
//!   HZB) but it REQUIRES a `DepthPrepass` on the view and is, per Bevy's own
//!   docs, a *measured* optimisation ("Only enable it if you measure it to be a
//!   speedup on your scene") that also builds an HZB acceleration structure
//!   every frame. Our v1 smoke world is open highlands with few large
//!   occluders, so the HZB overhead is unlikely to pay off yet, and enabling it
//!   risks the headless offscreen smoke capture. Opt-in recipe for 3.10b: add
//!   `DepthPrepass` +
//!   `bevy::render::experimental::occlusion_culling::OcclusionCulling` to the
//!   `Camera3d` (TAA already brings a depth prepass when enabled) and
//!   benchmark.
//! - **Far-mesh from the lod-alt heightmap**: the coarse far-terrain mesh needs
//!   the downscaled `lod_alt`/`lod_horizon` grids, which live on the embedded
//!   `client::Client` inside `xindeler-sim-bridge` (`world_data().lod_alt`) —
//!   NOT on the pure-Bevy client. Shipping it is a whole new data path (a
//!   protocol message carrying the grids + a bridge sender + a coarse CPU/GPU
//!   mesh, à la voxygen `scene/lod.rs`). Too big for a clean v1 → EM-3.10b.
//!   Until then the horizon beyond `chunk_render_distance` falls back to sky +
//!   `DistanceFog` (already applied by the atmosphere rig), which reads
//!   acceptably.
//!
//! ## Purity
//! 100% Bevy + the public `xindeler-render-voxel` chunk-mesh markers + this
//! crate's `sprite_view`/`terrain_stream` — NO specs. Compiled only under the
//! `listen-server` feature (sprite parents + streamed chunks only exist there).

use bevy::prelude::*;
use xindeler_render_voxel::pipeline::{ChunkKey, FluidChunkMesh, TerrainChunkMesh};

use crate::{sprite_view::SpriteChunkParent, terrain_stream::CHUNK_EDGE};

/// Distance-band radii (Bevy metres), measured horizontally (xz plane) from the
/// camera. Data-driven so they can move into `GraphicsSettings` later.
///
/// TODO(EM-3.10b): promote to `xindeler-app::GraphicsSettings` (a RON-backed,
/// user-facing "view distance" slider), exactly like the EM-3.5 chunk upload
/// budget and the EM-3.9 sprite budget — both parked as standalone resources
/// with the same follow-up note. The defaults below are keyed off the streamed
/// view distance (`server::MIN_VD` = 6 chunks) expressed in Bevy metres.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CullingConfig {
    /// Beyond this horizontal distance a chunk mesh (opaque or fluid) is
    /// hidden. Kept just inside the streamed square's far corners
    /// (Chebyshev-6 → Euclidean ≈ 8.5 chunks), so it trims the far ring without
    /// ever blanking the near/mid view.
    pub chunk_render_distance: f32,
    /// Beyond this (nearer) horizontal distance a whole per-chunk sprite parent
    /// is hidden. Sprites dominate the entity count, so a nearer band here is
    /// the biggest win.
    pub sprite_render_distance: f32,
}

impl Default for CullingConfig {
    fn default() -> Self {
        Self {
            // ~7 chunks: inside the streamed square's ~8.5-chunk far corners.
            chunk_render_distance: 7.0 * CHUNK_EDGE,
            // ~4 chunks: a much nearer band for the dominant sprite population.
            sprite_render_distance: 4.0 * CHUNK_EDGE,
        }
    }
}

/// Per-frame culling instrumentation (the perf story). `pub` so a harness/HUD
/// can read how many entities each band hid last frame.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct CullStats {
    /// Chunk meshes (opaque + fluid) hidden by the chunk band last frame.
    pub chunks_hidden: u32,
    /// Chunk meshes shown (within the chunk band) last frame.
    pub chunks_visible: u32,
    /// Sprite parents hidden by the sprite band last frame.
    pub sprite_parents_hidden: u32,
    /// Sprite parents shown (within the sprite band) last frame.
    pub sprite_parents_visible: u32,
}

/// Installs the EM-3.10 distance-band culling (listen-server only).
pub struct LodCullingPlugin;

impl Plugin for LodCullingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CullingConfig>()
            .init_resource::<CullStats>()
            // Both bands run in Update, reading the camera's propagated
            // `GlobalTransform` (a 1-frame lag is irrelevant to culling); the
            // `Visibility` they set is consumed by Bevy's PostUpdate
            // `visibility_propagate_system` + `check_visibility` the SAME frame.
            .add_systems(Update, (cull_chunk_meshes, cull_sprite_chunks));
    }
}

/// Bevy-space centre of a chunk: origin `(32·kx, 0, −32·ky)` (the converter's
/// z-up→y-up map, mirroring `pipeline::chunk_transform`) plus half a chunk on x
/// and −half on z.
#[must_use]
fn chunk_center_bevy(key: ChunkKey) -> Vec3 {
    #[expect(clippy::cast_precision_loss, reason = "chunk coords ≪ 2^24")]
    let origin = Vec3::new(key.x as f32 * CHUNK_EDGE, 0.0, -(key.y as f32 * CHUNK_EDGE));
    origin + Vec3::new(0.5 * CHUNK_EDGE, 0.0, -0.5 * CHUNK_EDGE)
}

/// Squared horizontal (xz) distance between two Bevy points — culling ignores
/// height so a camera panned up/down doesn't pop terrain in and out.
#[must_use]
fn horizontal_dist_sq(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

/// The band decision, factored out so it is unit-testable without an App:
/// `true` (visible) iff `point` is within `max_distance` horizontally of `eye`.
#[must_use]
fn within_band(point: Vec3, eye: Vec3, max_distance: f32) -> bool {
    horizontal_dist_sq(point, eye) <= max_distance * max_distance
}

/// Sets `vis` to the band result, counting the flip for the stats. Kept tiny so
/// both chunk queries share it.
fn apply_band(visible: bool, vis: &mut Visibility, shown: &mut u32, hidden: &mut u32) {
    let want = if visible {
        *shown += 1;
        Visibility::Visible
    } else {
        *hidden += 1;
        Visibility::Hidden
    };
    // Only write when it actually changes, so we don't needlessly trip
    // `Changed<Visibility>` (and the visibility propagation it drives).
    if *vis != want {
        *vis = want;
    }
}

/// Hard chunk render cap: hides chunk meshes (opaque + fluid) beyond
/// [`CullingConfig::chunk_render_distance`]. The two `&mut Visibility` queries
/// are provably disjoint (a terrain-mesh entity never carries `FluidChunkMesh`
/// and vice-versa — the pipeline spawns them as separate entities), stated via
/// `Without` filters so Bevy's access checker (B0001) is satisfied.
fn cull_chunk_meshes(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    config: Res<CullingConfig>,
    mut terrain: Query<(&TerrainChunkMesh, &mut Visibility), Without<FluidChunkMesh>>,
    mut fluid: Query<(&FluidChunkMesh, &mut Visibility), Without<TerrainChunkMesh>>,
    mut stats: ResMut<CullStats>,
) {
    let Some(eye) = camera.iter().next().map(GlobalTransform::translation) else {
        return; // no camera yet — leave everything as-is
    };
    let max = config.chunk_render_distance;
    let (mut shown, mut hidden) = (0u32, 0u32);
    for (marker, mut vis) in &mut terrain {
        let visible = within_band(chunk_center_bevy(marker.key), eye, max);
        apply_band(visible, &mut vis, &mut shown, &mut hidden);
    }
    for (marker, mut vis) in &mut fluid {
        let visible = within_band(chunk_center_bevy(marker.key), eye, max);
        apply_band(visible, &mut vis, &mut shown, &mut hidden);
    }
    stats.chunks_visible = shown;
    stats.chunks_hidden = hidden;
}

/// Sprite density band: hides a whole per-chunk sprite parent beyond
/// [`CullingConfig::sprite_render_distance`]. One `Visibility` write per chunk
/// parent hides its entire instance subtree via `InheritedVisibility` — the
/// dominant per-frame saving. Uses the parent's stored world-space vegetation
/// `centroid` as its position.
fn cull_sprite_chunks(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    config: Res<CullingConfig>,
    mut parents: Query<(&SpriteChunkParent, &mut Visibility)>,
    mut stats: ResMut<CullStats>,
) {
    let Some(eye) = camera.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    let max = config.sprite_render_distance;
    let (mut shown, mut hidden) = (0u32, 0u32);
    for (parent, mut vis) in &mut parents {
        let visible = within_band(parent.centroid, eye, max);
        apply_band(visible, &mut vis, &mut shown, &mut hidden);
    }
    stats.sprite_parents_visible = shown;
    stats.sprite_parents_hidden = hidden;
}

#[cfg(test)]
mod tests {
    use vek::Vec2 as VVec2;

    use super::*;

    #[test]
    fn chunk_center_maps_z_up_to_y_up() {
        // Chunk (1, 2): origin Veloren (32, 64, 0) → Bevy (32, 0, −64); centre
        // + (16, 0, −16) = (48, 0, −80).
        let c = chunk_center_bevy(VVec2::new(1, 2));
        assert_eq!(c, Vec3::new(48.0, 0.0, -80.0));
    }

    #[test]
    fn band_ignores_height() {
        let eye = Vec3::new(0.0, 1000.0, 0.0); // camera 1 km up
        let point = Vec3::new(10.0, 0.0, 0.0); // 10 m away horizontally
        // A 20 m band keeps it visible despite the 1 km vertical gap.
        assert!(within_band(point, eye, 20.0));
        // A 5 m band hides it (10 m > 5 m horizontally).
        assert!(!within_band(point, eye, 5.0));
    }

    #[test]
    fn band_is_a_hard_radius() {
        let eye = Vec3::ZERO;
        // Exactly on the radius = visible (inclusive); just past = hidden.
        assert!(within_band(Vec3::new(10.0, 0.0, 0.0), eye, 10.0));
        assert!(!within_band(Vec3::new(10.01, 0.0, 0.0), eye, 10.0));
    }

    /// Headless system test: a camera at the origin, one near chunk and one far
    /// chunk. After the cull system runs, the near chunk is `Visible` and the
    /// far chunk is `Hidden` — no GPU, no render plugins.
    #[test]
    fn far_chunks_are_hidden_near_chunks_shown() {
        let mut app = App::new();
        app.insert_resource(CullingConfig {
            chunk_render_distance: 3.0 * CHUNK_EDGE,
            sprite_render_distance: 2.0 * CHUNK_EDGE,
        })
        .init_resource::<CullStats>()
        .add_systems(Update, cull_chunk_meshes);

        // Camera at the world origin.
        app.world_mut().spawn((
            Camera3d::default(),
            GlobalTransform::from_translation(Vec3::ZERO),
        ));

        // Near chunk (key 0,0 → centre ~(16,0,−16), ~22 m) inside the 96 m band.
        let near = app
            .world_mut()
            .spawn((
                TerrainChunkMesh {
                    key: VVec2::new(0, 0),
                },
                Visibility::Visible,
            ))
            .id();
        // Far chunk (key 10,0 → centre x≈336 m) well outside the band.
        let far = app
            .world_mut()
            .spawn((
                TerrainChunkMesh {
                    key: VVec2::new(10, 0),
                },
                Visibility::Visible,
            ))
            .id();

        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(near).unwrap(),
            Visibility::Visible,
            "the near chunk must stay visible"
        );
        assert_eq!(
            *app.world().get::<Visibility>(far).unwrap(),
            Visibility::Hidden,
            "the far chunk must be hidden by the chunk band"
        );
        let stats = *app.world().resource::<CullStats>();
        assert_eq!(stats.chunks_visible, 1);
        assert_eq!(stats.chunks_hidden, 1);
    }

    /// Headless system test for the sprite band: a far sprite parent is hidden,
    /// hiding its whole subtree, while a near one stays visible.
    #[test]
    fn far_sprite_parents_are_hidden() {
        let mut app = App::new();
        app.insert_resource(CullingConfig {
            chunk_render_distance: 10.0 * CHUNK_EDGE,
            sprite_render_distance: 2.0 * CHUNK_EDGE, // 64 m
        })
        .init_resource::<CullStats>()
        .add_systems(Update, cull_sprite_chunks);

        app.world_mut().spawn((
            Camera3d::default(),
            GlobalTransform::from_translation(Vec3::ZERO),
        ));

        let near = app
            .world_mut()
            .spawn((
                SpriteChunkParent {
                    centroid: Vec3::new(20.0, 0.0, -10.0), // ~22 m < 64 m
                    count: 500,
                },
                Visibility::Visible,
            ))
            .id();
        let far = app
            .world_mut()
            .spawn((
                SpriteChunkParent {
                    centroid: Vec3::new(200.0, 0.0, 0.0), // 200 m > 64 m
                    count: 500,
                },
                Visibility::Visible,
            ))
            .id();

        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(near).unwrap(),
            Visibility::Visible
        );
        assert_eq!(
            *app.world().get::<Visibility>(far).unwrap(),
            Visibility::Hidden
        );
        let stats = *app.world().resource::<CullStats>();
        assert_eq!(stats.sprite_parents_visible, 1);
        assert_eq!(stats.sprite_parents_hidden, 1);
    }

    /// A chunk that moves back into range flips from `Hidden` to `Visible` on a
    /// later frame — the band is not one-shot.
    #[test]
    fn visibility_flips_when_camera_moves() {
        let mut app = App::new();
        app.insert_resource(CullingConfig {
            chunk_render_distance: 3.0 * CHUNK_EDGE,
            sprite_render_distance: 2.0 * CHUNK_EDGE,
        })
        .init_resource::<CullStats>()
        .add_systems(Update, cull_chunk_meshes);

        let cam = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                GlobalTransform::from_translation(Vec3::ZERO),
            ))
            .id();
        let chunk = app
            .world_mut()
            .spawn((
                TerrainChunkMesh {
                    key: VVec2::new(10, 0), // ~336 m from origin
                },
                Visibility::Visible,
            ))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(chunk).unwrap(),
            Visibility::Hidden,
            "far from origin → hidden"
        );

        // Move the camera next to the chunk.
        *app.world_mut().get_mut::<GlobalTransform>(cam).unwrap() =
            GlobalTransform::from_translation(chunk_center_bevy(VVec2::new(10, 0)));
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(chunk).unwrap(),
            Visibility::Visible,
            "camera moved into range → visible again"
        );
    }
}
