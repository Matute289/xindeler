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
//! ## EM-3.10b (landed on top of this module)
//! - **GPU occlusion culling**: implemented as an opt-in
//!   [`crate::camera::OcclusionCullingConfig`] (`DepthPrepass` +
//!   `bevy::render::occlusion_culling::OcclusionCulling` on the `Camera3d`,
//!   Bevy's own documented recipe). Measured on the listen-server smoke scene
//!   via the `XINDELER_PERF_LOG` rolling frame-time log — see
//!   `camera::OcclusionCullingConfig`'s doc comment for the verdict and the
//!   task report for the raw numbers. Shipped **opt-in, default OFF**
//!   (`XINDELER_OCCLUSION_CULLING=1` to try it): the sparse-occluder prediction
//!   below held, so the HZB overhead isn't earning its keep on this scene yet.
//! - **Far-mesh from the lod-alt heightmap**: landed in `xindeler-client::
//!   far_terrain` (+ `xindeler-sim-bridge::send_far_terrain_once` /
//!   `xindeler_protocol::NetFarTerrain`) — a coarse, vertex-coloured mesh built
//!   once from the server's downsampled `lod_alt` grid (real `lod_base` colour
//!   as of BL-82 EM-3.11 Phase A), filling the horizon beyond
//!   [`CullingConfig::chunk_render_distance`] with a fixed cutout hole around
//!   the boot anchor so it never overlaps the near terrain. A camera-following
//!   (rather than anchor-fixed) hole is deferred to EM-3.10c, once the player
//!   can roam far from the boot anchor (needs interest management, EM-4.2d,
//!   first). Until then the previous sky + `DistanceFog` fallback still covers
//!   the (today unreachable) case where no embedded player exists to source the
//!   heightmap.
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
    /// BL-82 EM-3.11 round 20 — hysteresis dead-zone width (Bevy metres)
    /// straddling BOTH bands' nominal `*_render_distance`. An entity is SHOWN
    /// once it comes within `render_distance - cull_hysteresis/2`, HIDDEN once
    /// it passes `render_distance + cull_hysteresis/2`, and KEEPS its current
    /// visibility anywhere in between. Without this, both bands were a hard
    /// `dist <= radius` cutoff (no dead-zone): an object sitting near a band
    /// edge toggled `Visibility` Visible↔Hidden every time the eye-to-object
    /// distance crossed the radius by even a hair — and in third person the eye
    /// ORBITS the pivot by ~`CAM_BACK` (9 m) during mouse-look, so any object
    /// in a ~18 m-wide annulus at the radius re-crossed it twice per camera
    /// sweep, reading as terrain/trees (chunk band, 224 m) and vegetation
    /// (sprite band, 128 m) flickering "disappear then reappear" as the player
    /// looks around. A dead-zone comfortably wider than that orbit amplitude
    /// (1 chunk = 32 m) makes a single boundary crossing latch, so an object
    /// only appears/disappears ONCE as the player genuinely approaches/recedes,
    /// never chattering. Measured live (`XINDELER_CULL_PERF_LOG`, round-20
    /// findings): on the VISIBLE sprite band, a same-world straight-walk-plus-
    /// mouse-look A/B cut the worst object's flicker from 29 flips to 5 (and
    /// total flip events ~40 %), eliminating the rapid boundary chatter.
    /// Matches the reference engines: neither `xindeler-old`'s LOD nor
    /// Minecraft's render distance toggles an already-drawn object on/off
    /// at a bare radius with no grace band.
    ///
    /// Perceptual note for anyone re-tuning the bands later: because the
    /// dead-zone is symmetric, the EFFECTIVE first-appearance radius is
    /// `render_distance - cull_hysteresis/2`, not `render_distance` — e.g. the
    /// sprite band shows vegetation from 112 m rather than 128 m (and hides it
    /// at 144 m). Both edges stay comfortably inside the streamed radius, so
    /// nothing is ever shown past what the stream actually holds.
    pub cull_hysteresis: f32,
}

impl Default for CullingConfig {
    fn default() -> Self {
        Self {
            // ~7 chunks: inside the streamed square's ~8.5-chunk far corners.
            chunk_render_distance: 7.0 * CHUNK_EDGE,
            // ~4 chunks: a much nearer band for the dominant sprite population.
            sprite_render_distance: 4.0 * CHUNK_EDGE,
            // 1 chunk of dead-zone — wider than the ~9 m third-person camera
            // orbit, so mouse-look never re-crosses both edges (see the field
            // docs).
            cull_hysteresis: CHUNK_EDGE,
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

/// The band decision, factored out so it is unit-testable without an App.
/// BL-82 EM-3.11 round 20 — HYSTERESIS: `point` is SHOWN once it comes within
/// `render_distance - hysteresis/2`, HIDDEN once it passes `render_distance +
/// hysteresis/2`, and KEEPS `currently_visible` anywhere in the dead-zone
/// between (see [`CullingConfig::cull_hysteresis`] for why — stops a
/// boundary object toggling as the third-person camera eye orbits during
/// mouse-look). A `hysteresis` of `0.0` collapses to the old hard `dist <=
/// render_distance` cutoff, so callers/tests can still exercise a bare radius.
#[must_use]
fn within_band(
    point: Vec3,
    eye: Vec3,
    render_distance: f32,
    hysteresis: f32,
    currently_visible: bool,
) -> bool {
    let d_sq = horizontal_dist_sq(point, eye);
    let show = (render_distance - 0.5 * hysteresis).max(0.0);
    let hide = render_distance + 0.5 * hysteresis;
    if d_sq <= show * show {
        true // definitely inside → show
    } else if d_sq > hide * hide {
        false // definitely outside → hide
    } else {
        currently_visible // dead-zone → latch whatever it already is
    }
}

/// Sets `vis` to the band result, counting the flip for the stats. Kept tiny so
/// both chunk queries share it. `flips` counts an ACTUAL band-boundary
/// crossing this frame (visible→hidden or vice versa) — separate from
/// `shown`/`hidden`, which count the current STATE regardless of whether it
/// changed. BL-82 EM-3.11p round 11: instrumented to test a new hypothesis —
/// a diagonal camera path may cross this circular distance-band boundary at a
/// different rate than a straight one (same grid-vs-direction geometry
/// argument as EM-3.11n's terrain-streaming dirty-marking finding, but
/// applied to this STEADY-STATE per-frame system instead of new-chunk
/// arrival), which would cost extra `Changed<Visibility>` propagation work
/// even with NO terrain streaming in flight.
///
/// Takes the query's `Mut<Visibility>` by `&mut` (NOT `&mut Visibility`) and
/// writes through [`bevy::prelude::DetectChangesMut::set_if_neq`]: that is what
/// keeps `Changed<Visibility>` (and the `InheritedVisibility` propagation it
/// drives across every sprite parent's subtree) from firing every frame for the
/// non-flipping majority. Coercing `&mut Mut<Visibility>` to `&mut Visibility`
/// would deref-mut and trip the change tick unconditionally BEFORE the equality
/// guard runs, defeating the whole point (rust-perf-reviewer, round 20).
fn apply_band(
    visible: bool,
    vis: &mut Mut<Visibility>,
    shown: &mut u32,
    hidden: &mut u32,
    flips: &mut u32,
) {
    let want = if visible {
        *shown += 1;
        Visibility::Visible
    } else {
        *hidden += 1;
        Visibility::Hidden
    };
    // `set_if_neq` writes (and trips change detection) ONLY on a real change,
    // and returns whether it changed — so a steady, non-flipping frame does
    // zero `Visibility` writes and zero propagation work.
    if vis.set_if_neq(want) {
        *flips += 1;
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
    mut perf_log: Local<Option<bool>>,
) {
    let Some(eye) = camera.iter().next().map(GlobalTransform::translation) else {
        return; // no camera yet — leave everything as-is
    };
    let max = config.chunk_render_distance;
    let hyst = config.cull_hysteresis;
    let (mut shown, mut hidden, mut flips) = (0u32, 0u32, 0u32);
    for (marker, mut vis) in &mut terrain {
        let currently_visible = !matches!(*vis, Visibility::Hidden);
        let visible = within_band(
            chunk_center_bevy(marker.key),
            eye,
            max,
            hyst,
            currently_visible,
        );
        apply_band(visible, &mut vis, &mut shown, &mut hidden, &mut flips);
    }
    for (marker, mut vis) in &mut fluid {
        let currently_visible = !matches!(*vis, Visibility::Hidden);
        let visible = within_band(
            chunk_center_bevy(marker.key),
            eye,
            max,
            hyst,
            currently_visible,
        );
        apply_band(visible, &mut vis, &mut shown, &mut hidden, &mut flips);
    }
    stats.chunks_visible = shown;
    stats.chunks_hidden = hidden;

    // BL-82 EM-3.11p round 11: opt-in per-frame flip-rate log (see
    // `apply_band`'s docs) — gated so the always-on cost is a cached bool
    // check, not a per-frame env read.
    let log_enabled = *perf_log
        .get_or_insert_with(|| std::env::var("XINDELER_CULL_PERF_LOG").is_ok_and(|v| v != "0"));
    if log_enabled && flips > 0 {
        debug!(
            flips,
            shown, hidden, "EM-3.11p round 11: chunk cull band flips this frame"
        );
    }
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
    mut perf_log: Local<Option<bool>>,
) {
    let Some(eye) = camera.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    let max = config.sprite_render_distance;
    let hyst = config.cull_hysteresis;
    let (mut shown, mut hidden, mut flips) = (0u32, 0u32, 0u32);
    for (parent, mut vis) in &mut parents {
        let currently_visible = !matches!(*vis, Visibility::Hidden);
        let visible = within_band(parent.centroid, eye, max, hyst, currently_visible);
        apply_band(visible, &mut vis, &mut shown, &mut hidden, &mut flips);
    }
    stats.sprite_parents_visible = shown;
    stats.sprite_parents_hidden = hidden;

    // See `cull_chunk_meshes`'s matching log for why (BL-82 EM-3.11p round
    // 11): sprite parents dominate entity count, so a per-frame flip burst
    // here (hiding/showing a whole subtree via `InheritedVisibility`) is a
    // plausible steady-state cost that could differ by movement heading.
    let log_enabled = *perf_log
        .get_or_insert_with(|| std::env::var("XINDELER_CULL_PERF_LOG").is_ok_and(|v| v != "0"));
    if log_enabled && flips > 0 {
        debug!(
            flips,
            shown, hidden, "EM-3.11p round 11: sprite cull band flips this frame"
        );
    }
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
        // A 20 m band keeps it visible despite the 1 km vertical gap (no
        // hysteresis → hard radius; current-visibility argument is irrelevant).
        assert!(within_band(point, eye, 20.0, 0.0, false));
        // A 5 m band hides it (10 m > 5 m horizontally).
        assert!(!within_band(point, eye, 5.0, 0.0, true));
    }

    #[test]
    fn band_is_a_hard_radius_without_hysteresis() {
        let eye = Vec3::ZERO;
        // hysteresis 0 → exactly on the radius = visible (inclusive); just past
        // = hidden, regardless of prior state.
        assert!(within_band(
            Vec3::new(10.0, 0.0, 0.0),
            eye,
            10.0,
            0.0,
            false
        ));
        assert!(!within_band(
            Vec3::new(10.01, 0.0, 0.0),
            eye,
            10.0,
            0.0,
            true
        ));
    }

    /// BL-82 EM-3.11 round 20 — the hysteresis dead-zone latches an object's
    /// visibility so a boundary crossing doesn't chatter as the camera eye
    /// jitters/orbits. This is the regression test for the round-20 flicker
    /// fix (see [`CullingConfig::cull_hysteresis`]).
    #[test]
    fn band_has_a_hysteresis_dead_zone() {
        let eye = Vec3::ZERO;
        let hyst = 8.0; // dead-zone spans [10-4, 10+4] = [6, 14]
        let render = 10.0;
        // Well inside the show edge (< 6 m) → shown regardless of prior state.
        assert!(within_band(
            Vec3::new(5.0, 0.0, 0.0),
            eye,
            render,
            hyst,
            false
        ));
        // Well outside the hide edge (> 14 m) → hidden regardless of prior state.
        assert!(!within_band(
            Vec3::new(15.0, 0.0, 0.0),
            eye,
            render,
            hyst,
            true
        ));
        // In the dead-zone (10 m, between 6 and 14): LATCH the current state —
        // a currently-visible object stays visible, a hidden one stays hidden.
        // This is exactly what stops the flicker: the same point does NOT flip
        // just because it drifted a hair across the nominal radius.
        assert!(within_band(
            Vec3::new(10.0, 0.0, 0.0),
            eye,
            render,
            hyst,
            true
        ));
        assert!(!within_band(
            Vec3::new(10.0, 0.0, 0.0),
            eye,
            render,
            hyst,
            false
        ));
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
            cull_hysteresis: 0.0,
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
            cull_hysteresis: 0.0,
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
            cull_hysteresis: 0.0,
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

    /// BL-82 EM-3.11 round 20 — the flicker, reproduced at the SYSTEM level and
    /// shown fixed: a chunk parked right AT the nominal render distance while
    /// the camera oscillates a hair back and forth across it (mimicking the
    /// third-person eye orbiting during mouse-look). With the default 1-chunk
    /// hysteresis the chunk's `Visibility` latches after the first frame and
    /// never flips again; with hysteresis disabled it flips on every crossing.
    /// The `flips`-accumulator lives in `apply_band`, but `CullStats` doesn't
    /// expose it, so this asserts on the observable `Visibility` staying put.
    #[test]
    fn hysteresis_stops_a_boundary_chunk_flickering_as_the_eye_oscillates() {
        // Chunk (10,0): centre x ≈ 336 m. Put the render distance exactly there
        // so the chunk sits ON the nominal boundary.
        let center = chunk_center_bevy(VVec2::new(10, 0));
        let radius = center.length(); // horizontal distance from origin

        // Small oscillation amplitude — far smaller than 1 chunk (32 m), like a
        // camera-orbit jitter around the boundary.
        let nudge = 4.0_f32;

        for (hysteresis, expect_flip) in [(CHUNK_EDGE, false), (0.0, true)] {
            let mut app = App::new();
            app.insert_resource(CullingConfig {
                chunk_render_distance: radius,
                sprite_render_distance: 2.0 * CHUNK_EDGE,
                cull_hysteresis: hysteresis,
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
                        key: VVec2::new(10, 0),
                    },
                    Visibility::Visible,
                ))
                .id();

            // Settle one frame at the boundary, then record the state.
            app.update();
            let settled = *app.world().get::<Visibility>(chunk).unwrap();

            // Oscillate the eye a few metres nearer / farther across the
            // radius: `+dir*nudge` moves TOWARD the chunk (distance
            // radius-nudge, inside), `-dir*nudge` moves away (radius+nudge,
            // outside). nudge (4 m) ≪ the 1-chunk (32 m) dead-zone, so with
            // hysteresis both extremes stay inside the dead-zone and latch.
            let dir = center.normalize();
            let mut any_flip = false;
            for i in 0..8 {
                let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                *app.world_mut().get_mut::<GlobalTransform>(cam).unwrap() =
                    GlobalTransform::from_translation(dir * (sign * nudge));
                app.update();
                if *app.world().get::<Visibility>(chunk).unwrap() != settled {
                    any_flip = true;
                }
            }
            assert_eq!(
                any_flip, expect_flip,
                "hysteresis={hysteresis}: boundary chunk flip-on-oscillation should be \
                 {expect_flip}"
            );
        }
    }
}
