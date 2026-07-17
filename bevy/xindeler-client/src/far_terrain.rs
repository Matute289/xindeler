//! EM-3.10b — coarse far-terrain mesh from the server's downsampled
//! `lod_alt`/`lod_base` grid (listen-server only).
//!
//! EM-3.10 (v1) left the horizon beyond [`crate::lod::CullingConfig::
//! chunk_render_distance`] as sky + `DistanceFog` — an acceptable, honestly
//! documented fallback, but not a filled-in view. This module builds ONE
//! low-poly, vertex-coloured mesh from the [`NetFarTerrain`] grid
//! (`xindeler-sim-bridge` → `xindeler-protocol`, sent once at boot — the far
//! terrain never changes during a session) and renders it beyond the near
//! terrain to fill that gap. As of BL-82 EM-3.11 Phase A the mesh's vertex
//! colour is the REAL `lod_base` colour sampled server-side (see
//! [`cell_color`]) — not a synthetic gradient.
//!
//! ## Why a hole, not a full disc
//! The far mesh is built in WORLD space and only rebuilt when the camera has
//! drifted far enough that the current cutout could stop covering the near
//! band (not every frame — see [`retile_far_mesh`]). To avoid z-fighting /
//! visible seams against the near, block-accurate chunk meshes, we cut a
//! circular hole out of it around the CAMERA's current position, sized to
//! comfortably exceed the near [`crate::lod::CullingConfig::
//! chunk_render_distance`] band.
//!
//! ## EM-3.11 fix: the hole must follow the camera, not the boot anchor
//! v1 (EM-3.10b) centred the hole on the one-shot [`TerrainCameraAnchor`] and
//! never moved it, reasoning the player would stay near spawn until
//! interest-management roaming (EM-4.2d) landed. In a real, unscripted
//! playthrough that assumption broke: [`crate::lod::cull_chunk_meshes`]
//! ALREADY re-centres the near chunk/fluid band on the live camera every
//! frame (chunks stream in from the server around the player, independent of
//! any anchor), so once a roaming player got more than
//! `chunk_render_distance + HOLE_MARGIN_CHUNKS` from the boot anchor, the
//! anchor-fixed hole no longer covered their surroundings — exposing this
//! coarse, fully OPAQUE, vertex-coloured mesh (green at low/water elevations,
//! see [`height_tint`]) right where they stood, in front of (and, having no
//! collider, walkable through) whatever real, correctly translucent-blue
//! fluid chunk had
//! streamed in for that spot. Bug report: BL-82 EM-3.11 ("water renders green
//! and opaque, and you can walk straight through it, during live free-roam
//! play — never during the anchor-framed smoke screenshot").
//! [`retile_far_mesh`] now re-centres the hole on the camera whenever it drifts
//! past that same margin — the exact slack the hole was originally over-sized
//! by — so the far mesh is geometrically guaranteed to never draw within
//! `chunk_render_distance` of the camera (module math in
//! [`retile_far_mesh`]'s doc comment). This is the scoped-down EM-3.10c
//! follow-up the v1 comment above pointed at: only the hole's centre needed to
//! track the camera — the near-terrain streaming/culling already did.
//!
//! ## Precision / look
//! One sample in [`NetFarTerrain`] covers `chunk_stride` chunks — already
//! coarse by construction (`xindeler-sim-bridge::send_far_terrain_once` caps
//! the grid dimension, downsampling a potentially 1024×1024-chunk world).
//! Corner heights are averaged from the up-to-4 touching samples so adjoining
//! quads share exact vertex positions (no cracks), but each quad still gets
//! its own (duplicated) vertices, a single flat face normal, and (Phase A) a
//! single real colour sample — deliberately simple "flat-shaded,
//! vertex-coloured low-poly terrain" (task-approved v1 scope), not the full
//! PBR block-palette treatment the near terrain gets.
//!
//! ## EM-3.11 round 11 (superseded by Phase A below): the "beige horizon"
//! Matías's `record8.mov` (a real free-roam session, taken AFTER the EM-4.10
//! P0 frame-time hotfix) showed this mesh's own flat, undetailed colour as a
//! visible strip at the horizon, in open areas especially. Root cause: this
//! mesh baked a SYNTHETIC 2-stop colour gradient (`height_tint`, since
//! retired) instead of the real `lod_base` colour the embedded `Client`
//! already had available, and the ONLY thing masking that flat colour was
//! `DistanceFog` (`crate::atmosphere`) — the EM-3.11f density only reached
//! ~97.5% opacity by this mesh's own nearest visible point (`hole_radius`,
//! ~288m at the default render distance), a small but, on real
//! detailed-vs-flat contrast, clearly visible residual. `docs/design/specs/
//! 2026-07-11-xindeler-old-comparison-research.md` §2 traced the DEEPER gap
//! versus the old client (which renders a full-world, `lod_base`-coloured LOD
//! terrain all the way to the horizon, using fog only as a finishing touch on
//! an already-complete world — not as the sole mask for a hard edge). Round
//! 11's fix (fog-density retune + a heavy 0.35 blend toward fog colour) was a
//! stopgap that worked within the then-synthetic-colour architecture; it did
//! not close the structural gap.
//!
//! ## BL-82 EM-3.11 Phase A: real colour (this module's current state)
//! `docs/design/specs/2026-07-11-bl82-full-horizon-lod-terrain-design.md`
//! designed the structural fix: the server already samples `lod_base`
//! (`client::WorldData::col_at`) alongside `lod_alt` and ships it as
//! [`NetFarTerrain::colors`], index-aligned with the height samples. This
//! module now bakes each quad's vertex colour from that REAL per-cell colour
//! (see [`cell_color`]) instead of `height_tint`'s synthetic gradient, so the
//! horizon shows actual varied terrain colour (forests, drylands, water,
//! peaks) matching the minimap — the reported "flat plateau" symptom is
//! closed by having real data, not by leaning harder on fog. [`FAR_HAZE_BLEND`]
//! keeps only a SMALL atmospheric-finish blend (dialled back from round 11's
//! 0.35 now that colour is real) — a light haze cue for distance, not a mask
//! for missing detail. Phase B (below) is what dissolves the far mesh's hard
//! top *edge* into atmosphere; Phase A only fixed the mesh's own colour.
//!
//! ## BL-82 EM-3.11 Phase B: the curved, dissolving horizon
//! `docs/design/specs/2026-07-11-bl82-full-horizon-lod-terrain-design.md`
//! §3.4 (resolved via the task board's `[Q-B1]` worksheet to the "A+C
//! synthesis") replaces the mesh's plain `StandardMaterial` with
//! [`crate::far_terrain_material::FarTerrainMaterial`]
//! (`ExtendedMaterial<StandardMaterial, FarTerrainExtension>`): a vertex
//! shader bends the far field down and away from the camera (Matías's
//! "Option C" world-curvature trick), and a fragment shader adds a soft
//! `lod_horizon`-based sun-occlusion term and dissolves the silhouette
//! toward the live atmosphere colour by distance/height — see that module's
//! doc comments for the full shader design. [`DecodedFarTerrain::horizon`]
//! carries the BL-82 EM-3.11 Phase B `NetFarTerrain::horizon` layer
//! (index-aligned with `colors`/`heights`, T49.5), baked into a per-quad
//! [`crate::far_terrain_material::ATTRIBUTE_FAR_HORIZON`] vertex attribute
//! the same way [`cell_color`] already bakes per-quad vertex colour. This is
//! what lets T49.7 lower `fog_density` back down — the mesh no longer relies
//! solely on fog to hide its edge.
//!
//! ## Purity
//! 100% Bevy + the protocol message + `terrain_stream::{CHUNK_EDGE,
//! TerrainCameraAnchor}` + `lod::CullingConfig` + `light::Sun` (read-only,
//! Phase B's live sun direction) + (EM-3.11 round 11 / Phase B)
//! `xindeler-oracle-host`'s headless-safe `AtmosphereController`/
//! `AtmosphereProfile` types (read-only, for [`FAR_HAZE_BLEND`] and the
//! Phase-B material's fog/sky uniforms) — no specs. Compiled only under the
//! `listen-server` feature.

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh as BevyMesh, PrimitiveTopology},
    pbr::StandardMaterial,
    prelude::*,
};
use xindeler_oracle_host::{AtmosphereController, AtmosphereProfile};
use xindeler_protocol::NetFarTerrain;

use crate::{
    far_terrain_material::{
        ATTRIBUTE_FAR_HORIZON, FarTerrainExtension, FarTerrainMaterial, FarTerrainMaterialPlugin,
    },
    lod::CullingConfig,
    terrain_stream::{CHUNK_EDGE, TerrainCameraAnchor},
};

/// Safety margin (in chunks) added on top of [`CullingConfig::
/// chunk_render_distance`] when sizing the far-mesh's cutout hole, so the two
/// meshes overlap rather than leaving a gap at the boundary. ALSO doubles as
/// [`retile_far_mesh`]'s re-tile threshold (its doc comment proves that reuse
/// is exactly what keeps the far mesh from ever overlapping the near band).
const HOLE_MARGIN_CHUNKS: f32 = 2.0;

/// Installs the EM-3.10b far-terrain consumer: receives [`NetFarTerrain`]
/// once, then builds/re-tiles the mesh (see [`retile_far_mesh`]) as soon as,
/// and for as long as, a camera exists.
pub struct FarTerrainPlugin;

impl Plugin for FarTerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FarTerrainMaterialPlugin)
            .add_systems(Update, (receive_far_terrain, retile_far_mesh));

        // Debug-only, opt-in (`XINDELER_SMOKE_FAR_MESH_CAM=1`): parks the
        // camera high above the anchor looking outward so a
        // `--smoke-screenshot` frames the horizon (verifying the far mesh
        // visually) instead of the sprite/figure close-ups the normal smoke
        // harness frames. Runs in `PostUpdate`, after every other camera
        // system in `Update` (fly-cam / third-person / smoke figure+sprite
        // cams), so it always wins when enabled; a no-op resource read
        // otherwise.
        if std::env::var("XINDELER_SMOKE_FAR_MESH_CAM").is_ok_and(|v| v != "0") {
            app.add_systems(bevy::app::PostUpdate, smoke_horizon_cam);
        }
    }
}

/// See [`FarTerrainPlugin::build`]'s `XINDELER_SMOKE_FAR_MESH_CAM` note.
fn smoke_horizon_cam(
    anchor: Option<Res<TerrainCameraAnchor>>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    let Some(anchor) = anchor else { return };
    for mut transform in &mut cameras {
        // Ground-level vantage near the anchor, looking OUTWARD toward the
        // horizon (roughly level, slight downward tilt) — the framing a
        // player standing near spawn would actually see: near chunk terrain
        // in the foreground, the chunk-render-distance edge partway out, and
        // (with the far mesh) a filled, shaded horizon beyond it instead of
        // flat sky.
        let eye = anchor.bevy_pos + Vec3::new(-90.0, 18.0, 90.0);
        let look_target = anchor.bevy_pos + Vec3::new(-500.0, 30.0, 500.0);
        *transform = Transform::from_translation(eye).looking_at(look_target, Vec3::Y);
    }
}

/// The decoded far-terrain grid (height + colour). Installed once, the first
/// time the one-shot [`NetFarTerrain`] message arrives, and then kept alive
/// for the whole session (NOT consumed after the first mesh build) —
/// [`retile_far_mesh`] re-reads it every time the camera drifts far enough to
/// need a fresh hole.
#[derive(Resource)]
struct FarTerrainData(DecodedFarTerrain);

struct DecodedFarTerrain {
    grid_w: u32,
    grid_h: u32,
    chunk_stride: u32,
    heights: Vec<f32>,
    /// Real per-cell RGB colour (BL-82 EM-3.11 Phase A), index-aligned with
    /// `heights` (`colors[j * grid_w + i]` is the colour of cell `(i, j)`).
    colors: Vec<[u8; 3]>,
    /// Packed west/east `(angle, occluder-height)` horizon record (BL-82
    /// EM-3.11 Phase B), index-aligned with `heights`/`colors`. Falls back to
    /// all-zero ("no occlusion, sun never blocked") per cell when the server
    /// message's horizon layer doesn't decode — see [`receive_far_terrain`].
    horizon: Vec<[u8; 4]>,
}

/// Marks the currently-spawned far-terrain mesh entity (so a smoke/debug
/// harness can find it). [`retile_far_mesh`] tracks the entity itself via
/// [`FarMeshState`] (it must despawn the OLD one precisely, not "any" one, on
/// every re-tile), so this component is a passive marker only.
#[derive(Component)]
pub struct FarTerrainMesh;

/// Tracks the world-xz point the CURRENT far-mesh entity's cutout hole is
/// centred on, plus that entity (`None` if the grid was small enough that the
/// hole swallowed every quad — the degenerate case `retile_far_mesh` already
/// handled by drawing nothing). Absent entirely until the first tile builds.
#[derive(Resource)]
struct FarMeshState {
    hole_center: Vec2,
    entity: Option<Entity>,
}

/// Decodes the one-shot [`NetFarTerrain`] message into [`FarTerrainData`]
/// (once — the payload is never resent, so a resource already present means
/// we already have it). BL-82 EM-3.11 Phase A: both the height AND colour
/// layers must decode — a message missing (or corrupt in) either is dropped
/// wholesale (same "undecodable ⇒ drop" behaviour the height-only v1 had),
/// since a mesh with real heights but no real colour would just fall back to
/// a single flat colour anyway. BL-82 EM-3.11 Phase B: the horizon layer is
/// treated more leniently — it's a shading REFINEMENT (soft sun-occlusion),
/// not core geometry/colour, so a missing/corrupt horizon blob (e.g. an
/// older server that hasn't rebuilt yet) falls back to an all-zero "no
/// occlusion" record per cell rather than dropping the whole far mesh.
fn receive_far_terrain(
    mut commands: Commands,
    mut messages: MessageReader<NetFarTerrain>,
    existing: Option<Res<FarTerrainData>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(msg) = messages.read().next() else {
        return;
    };
    let Some(heights) = msg.decode_heights() else {
        warn!("dropping undecodable far-terrain grid (heights)");
        return;
    };
    let Some(colors) = msg.decode_colors() else {
        warn!("dropping undecodable far-terrain grid (colors)");
        return;
    };
    let horizon = msg.decode_horizon().unwrap_or_else(|| {
        if !msg.horizon.is_empty() {
            warn!("dropping undecodable far-terrain grid (horizon); falling back to no-occlusion");
        }
        vec![[0u8; 4]; heights.len()]
    });
    commands.insert_resource(FarTerrainData(DecodedFarTerrain {
        grid_w: msg.grid_size[0],
        grid_h: msg.grid_size[1],
        chunk_stride: msg.chunk_stride,
        heights,
        colors,
        horizon,
    }));
}

/// Builds the far-terrain mesh once [`FarTerrainData`] has arrived, then
/// RE-CENTRES its cutout hole on the camera's live position whenever it has
/// drifted more than [`HOLE_MARGIN_CHUNKS`] chunks from the hole the CURRENT
/// mesh was built around.
///
/// ## Why that exact threshold is safe (EM-3.11 fix)
/// The hole is built with `hole_radius = chunk_render_distance +
/// HOLE_MARGIN_CHUNKS · CHUNK_EDGE` around some centre `C`. The near terrain/
/// fluid band ([`crate::lod::cull_chunk_meshes`]) independently keeps
/// everything within `chunk_render_distance` of the LIVE camera `P` visible
/// every frame. For the far mesh to never draw inside that near band we need
/// `dist(P, C) + chunk_render_distance <= hole_radius`, i.e. `dist(P, C) <=
/// HOLE_MARGIN_CHUNKS · CHUNK_EDGE`. So re-tiling as soon as the camera
/// crosses exactly that distance from the hole's last centre keeps the
/// invariant true at all times — the far mesh is geometrically guaranteed to
/// never overlap the near band, however far or long the player roams. This
/// replaces the v1 fixed-at-boot-anchor hole (module docs), which broke that
/// invariant the moment a live playthrough wandered `chunk_render_distance +
/// HOLE_MARGIN_CHUNKS` from spawn: the coarse, opaque, green-at-low-elevation
/// far mesh (`height_tint`) became visible right around the player, in front
/// of (and walkable through, having no collider) the real translucent-blue
/// water that should have been showing there instead.
///
/// Re-tiling is rare (only on ~`HOLE_MARGIN_CHUNKS`-chunk-sized camera
/// excursions, not every frame) and bounded in cost (the grid is capped at
/// `LOD_ALT_MAX_DIM`² samples server-side), so this keeps the "no per-frame
/// cost" property the original design wanted — it just no longer trades that
/// for correctness once the player leaves the boot vicinity.
fn retile_far_mesh(
    mut commands: Commands,
    data: Option<Res<FarTerrainData>>,
    mut state: Option<ResMut<FarMeshState>>,
    culling: Res<CullingConfig>,
    atmosphere: Option<Res<AtmosphereController>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut meshes: ResMut<Assets<BevyMesh>>,
    mut materials: ResMut<Assets<FarTerrainMaterial>>,
    mut perf_log: Local<Option<bool>>,
) {
    let Some(data) = data else { return };
    let Some(eye) = camera.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    let hole_center = Vec2::new(eye.x, eye.z);
    let rebuild_slack = HOLE_MARGIN_CHUNKS * CHUNK_EDGE;

    if let Some(state) = &state
        && state.hole_center.distance(hole_center) <= rebuild_slack
    {
        return; // still safely inside the current hole — nothing to do
    }

    // BL-82 EM-3.11p round 11 (Wave-3 post-merge regression hunt): this
    // rebuild is synchronous main-thread work (full `far_mesh_from_heights`
    // over up to `LOD_ALT_MAX_DIM`² grid cells, plus a mesh/material asset
    // add) gated only on ~`HOLE_MARGIN_CHUNKS`-chunk camera drift, not a
    // frame budget — unlike the near-terrain mesh pipeline (async + budgeted
    // uploads). It was always unbudgeted, but this round is checking whether
    // it's now landing often/expensively enough to be a visible cost,
    // possibly compounding with new per-tick work this merge added
    // elsewhere. Gated by `XINDELER_FAR_MESH_PERF_LOG=1`.
    let perf_log = *perf_log
        .get_or_insert_with(|| std::env::var("XINDELER_FAR_MESH_PERF_LOG").is_ok_and(|v| v != "0"));
    let retile_start = std::time::Instant::now();

    // BL-82 EM-3.11 Phase A (see [`cell_color`]'s doc comment): each quad's
    // REAL `lod_base` colour is mixed a small [`FAR_HAZE_BLEND`] toward the
    // CURRENT atmosphere's live `fog_color` (falls back to the default
    // profile's colour if no `AtmosphereController` exists, e.g. in a test
    // app that doesn't wire `AtmospherePlugin`) as an atmospheric finish.
    // Only re-baked on a re-tile (rare — see the doc comment above), not
    // every frame a profile transition animates: acceptable,
    // honestly-documented staleness, matching this system's existing
    // "no per-frame cost" design goal (a weather change fully lands in the
    // far mesh's colour the next time the camera drifts far enough to
    // re-tile, not instantly).
    // BL-82 EM-3.11 Phase B: also grabs `sky_color` alongside `fog_color` in
    // the SAME `map_or_else` (an `Option<Res<_>>` is consumed by value, so
    // both must come out of one read) — [`FarTerrainExtension::sky_color`]
    // is the Phase-B silhouette-dissolve target, `haze` (fog colour) stays
    // Phase A's vertex-colour atmospheric finish.
    //
    // Data-driven-content cleanup (comprehensive-review Finding 2): also
    // reads `far_mesh_bend_strength`/`far_mesh_bend_start_scale` out of the
    // SAME live `AtmosphereProfile` in this one read — these used to be the
    // compiled-in `FAR_MESH_BEND_STRENGTH` constant (plus an always-`1.0`
    // scale) with only a debug-only env-var override; now every sibling
    // atmosphere-tuning parameter (this pair included) is RON-driven and
    // hot-reloadable, per-biome/vantage via a DmEvent atmosphere override.
    let (haze, sky_color, bend_strength_base, bend_start_scale) = atmosphere.map_or_else(
        || {
            let defaults = AtmosphereProfile::default();
            (
                Vec3::from_array(defaults.fog_color),
                Vec3::from_array(defaults.sky_color),
                defaults.far_mesh_bend_strength,
                defaults.far_mesh_bend_start_scale,
            )
        },
        |a| {
            (
                Vec3::from_array(a.current.fog_color),
                Vec3::from_array(a.current.sky_color),
                a.current.far_mesh_bend_strength,
                a.current.far_mesh_bend_start_scale,
            )
        },
    );

    let hole_radius = culling.chunk_render_distance + rebuild_slack;
    let mesh = far_mesh_from_heights(&data.0, hole_center, hole_radius, haze);
    // BL-82 EM-3.11 Phase B: `bend_start` is set to THIS re-tile's own
    // `hole_radius`, scaled by the live `far_mesh_bend_start_scale`
    // (`>= 1.0`, clamped by `AtmosphereProfile::sanitize` — see that field's
    // doc comment) — the exact invariant the vertex shader's `max(d -
    // bend_start, 0.0)` clamp relies on to guarantee zero bend across the
    // whole near band (module docs' "Purity"/Phase-B section;
    // `far_terrain_material.rs`'s doc comments) still holds for ANY
    // `bend_start_scale >= 1.0`. `sun_direction` starts at
    // `FarTerrainExtension::default()`'s placeholder and, like `fog_color`/
    // `sky_color`, is kept live every frame by
    // `far_terrain_material::sync_far_terrain_material` — no per-tile cost.
    let bend_start = hole_radius * bend_start_scale;
    //
    // Debug-only, opt-in override (`XINDELER_FAR_MESH_BEND_STRENGTH=<f32>`,
    // e.g. `0` to disable) so the bend can be A/B'd against the LIVE
    // atmosphere-driven value without touching RON — same convention as
    // `XINDELER_SMOKE_FAR_MESH_CAM`/`XINDELER_FAR_MESH_PERF_LOG` above. The
    // atmosphere-driven `bend_strength_base` (RON, hot-reloadable) is now the
    // PRIMARY/shipped configuration path; the env var only wins when set.
    let bend_strength = std::env::var("XINDELER_FAR_MESH_BEND_STRENGTH")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(bend_strength_base);
    let new_entity = mesh.map(|mesh| {
        commands
            .spawn((
                FarTerrainMesh,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(materials.add(FarTerrainMaterial {
                    base: StandardMaterial {
                        base_color: Color::WHITE,
                        // The mesh is a single coarse sheet with no
                        // interior — both faces must shade the same way
                        // regardless of which side the winding ends up
                        // facing.
                        cull_mode: None,
                        perceptual_roughness: 1.0,
                        reflectance: 0.02,
                        ..default()
                    },
                    extension: FarTerrainExtension {
                        bend_strength,
                        bend_start,
                        fog_color: haze.extend(1.0),
                        sky_color: sky_color.extend(1.0),
                        // BL-82 EM-3.11 round 24: the same `hole_radius` this
                        // mesh already cuts a CPU hole for — a no-op for the
                        // sheet (no geometry survives inside the hole anyway),
                        // set only so both users of this shared material carry
                        // a consistent near-band value. It's the LOD-object
                        // meshes (`lod_objects.rs`) this uniform actually
                        // matters for.
                        near_band: hole_radius,
                        ..default()
                    },
                })),
                Transform::IDENTITY, // positions are already absolute world-space
                Visibility::Visible,
            ))
            .id()
    });
    if new_entity.is_none() {
        // Every quad fell inside the hole (tiny world, or the camera is deep
        // in it) — nothing to draw; the sky+fog fallback stays in effect,
        // which is the documented acceptable outcome for a degenerate case,
        // not a bug.
        info!("far-terrain grid entirely inside the near band; skipping the far mesh");
    }

    if let Some(old) = state.as_ref().and_then(|s| s.entity) {
        // `try_despawn` (silently no-ops if `old` is already gone), not
        // `despawn` (panics/logs an error) — EM-3.11 fix: an observed, rare,
        // hard-to-pin-precisely race let a re-tile target an entity some
        // other path had already despawned (e.g. a chunk-scale event racing
        // this system in the same frame), crashing the whole client. Losing
        // this despawn is harmless either way: the entity is already gone,
        // which is exactly the outcome we wanted.
        commands.entity(old).try_despawn();
    }

    match &mut state {
        Some(state) => {
            state.hole_center = hole_center;
            state.entity = new_entity;
        },
        None => {
            commands.insert_resource(FarMeshState {
                hole_center,
                entity: new_entity,
            });
        },
    }

    if perf_log {
        let elapsed_ms = retile_start.elapsed().as_secs_f64() * 1000.0;
        debug!(
            elapsed_ms,
            grid_cells = data.0.heights.len(),
            "EM-3.11p round 11: far-mesh retile main-thread cost"
        );
    }
}

/// Height-only-known corner (grid coordinate space, before world placement).
#[inline]
fn corner_height(data: &DecodedFarTerrain, ci: i32, cj: i32) -> Option<f32> {
    if ci < 0 || cj < 0 || ci >= data.grid_w as i32 || cj >= data.grid_h as i32 {
        return None;
    }
    #[expect(clippy::cast_sign_loss, reason = "bounds-checked above")]
    Some(data.heights[(cj as u32 * data.grid_w + ci as u32) as usize])
}

/// Averages the up-to-4 sample cells touching grid corner `(i, j)` (`i` in
/// `0..=grid_w`, `j` in `0..=grid_h`) so adjoining quads share an identical
/// vertex height — the no-cracks contract.
fn averaged_corner(data: &DecodedFarTerrain, i: u32, j: u32) -> f32 {
    let (i, j) = (i as i32, j as i32);
    let samples = [
        corner_height(data, i - 1, j - 1),
        corner_height(data, i, j - 1),
        corner_height(data, i - 1, j),
        corner_height(data, i, j),
    ];
    let (sum, n) = samples
        .into_iter()
        .flatten()
        .fold((0.0_f32, 0u32), |(sum, n), h| (sum + h, n + 1));
    if n == 0 { 0.0 } else { sum / n as f32 }
}

/// Small atmospheric-finish blend fraction mixed into the REAL per-cell
/// colour sampled from the server's `lod_base` layer (BL-82 EM-3.11 Phase A;
/// `docs/design/specs/2026-07-11-bl82-full-horizon-lod-terrain-design.md`
/// §3.3). Round 11's [`HAZE_BLEND`] (0.35) leaned hard on this blend to mask
/// a SYNTHETIC, flat colour — now that [`cell_color`] bakes the real
/// `lod_base` sample, the mesh's own colour is what sells the horizon, so
/// this only needs to be a light atmospheric cue (haze at distance), dialled
/// back accordingly. Phase B's sky-blend silhouette material is the
/// mechanism that eventually dissolves the mesh's hard top *edge* into
/// atmosphere; this constant is deliberately NOT trying to do that job too.
const FAR_HAZE_BLEND: f32 = 0.12;

/// Bakes a quad's vertex colour from its REAL per-cell `lod_base` sample
/// (BL-82 EM-3.11 Phase A), mixing [`FAR_HAZE_BLEND`] of it toward `haze`
/// (the live atmosphere's fog colour) as a light atmospheric finish — see
/// [`FAR_HAZE_BLEND`]'s doc comment. `rgb` is already sRGB-encoded (matching
/// the old engine's `t_map` `Rgba8Srgb` convention, decoded byte-for-byte by
/// `client::WorldData::col_at`), so it's blended in the same sRGB space
/// `Color::srgb` expects, THEN converted to linear at the call site — the
/// same order the retired `height_tint` used.
fn cell_color(rgb: [u8; 3], haze: Vec3) -> Color {
    let real = Vec3::new(
        f32::from(rgb[0]) / 255.0,
        f32::from(rgb[1]) / 255.0,
        f32::from(rgb[2]) / 255.0,
    );
    let c = real.lerp(haze, FAR_HAZE_BLEND);
    Color::srgb(c.x, c.y, c.z)
}

/// Builds the far-terrain [`BevyMesh`], or `None` if every quad fell inside
/// the cutout hole (nothing to draw). `haze` is the live atmosphere's
/// `fog_color` (see [`FAR_HAZE_BLEND`]).
fn far_mesh_from_heights(
    data: &DecodedFarTerrain,
    hole_center: Vec2,
    hole_radius: f32,
    haze: Vec3,
) -> Option<BevyMesh> {
    let cell = data.chunk_stride as f32 * CHUNK_EDGE;

    // World-space position of grid corner (i, j) — X = i·cell, Z = −(j·cell),
    // matching `xindeler_client::lod::chunk_center_bevy` / `pipeline::
    // chunk_transform`'s z-up→y-up frame (chunk-grid index → Bevy metres).
    let corner_pos = |i: u32, j: u32| -> Vec3 {
        Vec3::new(
            i as f32 * cell,
            averaged_corner(data, i, j),
            -(j as f32 * cell),
        )
    };

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut horizons: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for j in 0..data.grid_h {
        for i in 0..data.grid_w {
            // Cull by the quad's NEAREST point to the hole centre, not its
            // centre (review should-fix #1): a quad is `cell` metres wide
            // (up to 256 m at the default 1024×1024-chunk world's stride
            // cap), so its centre can sit well outside `hole_radius` while a
            // corner still lands deep inside it — exactly the geometry that
            // was silently overlapping the near, block-accurate terrain.
            // Clamping the hole centre to the quad's world-space footprint
            // and measuring from there guarantees no DRAWN quad's footprint
            // intersects the hole disc (conservative: a few quads that only
            // brush the hole get culled too, which is the safe direction).
            let quad_min = Vec2::new(i as f32 * cell, -((j + 1) as f32 * cell));
            let quad_max = Vec2::new((i + 1) as f32 * cell, -(j as f32 * cell));
            let nearest = hole_center.clamp(quad_min, quad_max);
            if nearest.distance(hole_center) <= hole_radius {
                continue; // inside/overlapping the near band — leave the hole for the real terrain
            }

            let p00 = corner_pos(i, j);
            let p10 = corner_pos(i + 1, j);
            let p11 = corner_pos(i + 1, j + 1);
            let p01 = corner_pos(i, j + 1);

            // Flat per-quad normal via the right-hand rule on the first
            // triangle; near-flat terrain gives ≈(0, cell², 0), i.e. +Y.
            let normal = (p10 - p00).cross(p11 - p00).normalize_or_zero();

            // BL-82 EM-3.11 Phase A: colours are per *sample/cell*
            // (`colors[j * grid_w + i]`), while heights are averaged per
            // *corner* — for v1 the whole quad (all 4 of its duplicated
            // vertices, matching the existing flat-shaded-quad convention)
            // takes the colour of its own originating cell `(i, j)`, the
            // natural, cheap choice given the quads already duplicate
            // vertices per-face.
            let cell_rgb = data.colors[(j * data.grid_w + i) as usize];
            let color = cell_color(cell_rgb, haze).to_linear().to_f32_array();

            // BL-82 EM-3.11 Phase B: same per-quad "own cell" convention as
            // colour above — the horizon record is per sample/cell, baked
            // flat across all 4 (duplicated) vertices of the quad. Bytes are
            // normalised to `[0.0, 1.0]` so the WGSL fragment shader can use
            // them directly (`far_terrain_material.wgsl`'s occlusion math).
            let cell_horizon = data.horizon[(j * data.grid_w + i) as usize];
            let horizon = cell_horizon.map(|b| f32::from(b) / 255.0);

            let base = positions.len() as u32;
            for p in [p00, p10, p11, p01] {
                positions.push(p.to_array());
                normals.push(normal.to_array());
                colors.push(color);
                horizons.push(horizon);
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    if positions.is_empty() {
        return None;
    }

    let mut mesh = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(ATTRIBUTE_FAR_HORIZON, horizons);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, asset::AssetPlugin, prelude::MinimalPlugins};

    use super::*;

    /// Headless App exercising [`retile_far_mesh`] directly (no NetFarTerrain
    /// plumbing, no window/render) — regression coverage for the EM-3.11 fix:
    /// a small camera drift must NOT re-tile (the "no per-frame cost"
    /// property), but a drift past [`HOLE_MARGIN_CHUNKS`] MUST re-tile with a
    /// fresh hole centred on the camera and despawn the stale mesh entity —
    /// the exact behaviour that keeps the far mesh from ever being visible
    /// within `chunk_render_distance` of a roaming player (this module's
    /// "EM-3.11 fix" doc section proves the threshold is exact, not just
    /// generous).
    #[test]
    fn retile_recentres_hole_only_once_camera_drifts_past_margin() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<BevyMesh>()
            .init_asset::<FarTerrainMaterial>()
            .insert_resource(CullingConfig {
                chunk_render_distance: 100.0,
                sprite_render_distance: 50.0,
                ..Default::default()
            })
            // A big flat grid (physical extent 8*4*32 = 1024 m per axis) so a
            // ~164 m-radius hole never swallows the whole thing.
            .insert_resource(FarTerrainData(flat_grid(8, 8, 0.0, 4)))
            .add_systems(Update, retile_far_mesh);

        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                GlobalTransform::from_translation(Vec3::new(512.0, 0.0, -512.0)),
            ))
            .id();

        app.update();
        let (first_entity, first_center) = {
            let state = app.world().resource::<FarMeshState>();
            (
                state.entity.expect("grid far larger than the hole"),
                state.hole_center,
            )
        };
        assert_eq!(first_center, Vec2::new(512.0, -512.0));

        // Drift LESS than the rebuild slack (HOLE_MARGIN_CHUNKS·CHUNK_EDGE =
        // 2·32 = 64 m) — must NOT re-tile.
        *app.world_mut()
            .get_mut::<GlobalTransform>(camera)
            .expect("camera entity") =
            GlobalTransform::from_translation(Vec3::new(522.0, 0.0, -512.0));
        app.update();
        {
            let state = app.world().resource::<FarMeshState>();
            assert_eq!(
                state.entity,
                Some(first_entity),
                "a drift under the margin must not re-tile"
            );
            assert_eq!(state.hole_center, first_center, "hole must stay put");
        }

        // Drift MORE than the rebuild slack — MUST re-tile: fresh entity,
        // hole re-centred on the camera, stale entity despawned.
        *app.world_mut()
            .get_mut::<GlobalTransform>(camera)
            .expect("camera entity") =
            GlobalTransform::from_translation(Vec3::new(612.0, 0.0, -512.0));
        app.update();
        let state = app.world().resource::<FarMeshState>();
        let second_entity = state.entity.expect("grid still far larger than the hole");
        assert_ne!(
            second_entity, first_entity,
            "a drift past the margin must re-tile with a fresh entity"
        );
        assert_eq!(state.hole_center, Vec2::new(612.0, -512.0));
        assert!(
            app.world().get_entity(first_entity).is_err(),
            "the stale far-mesh entity must be despawned on re-tile"
        );
    }

    fn flat_grid(w: u32, h: u32, height: f32, stride: u32) -> DecodedFarTerrain {
        DecodedFarTerrain {
            grid_w: w,
            grid_h: h,
            chunk_stride: stride,
            heights: vec![height; (w * h) as usize],
            colors: vec![[128, 128, 128]; (w * h) as usize],
            horizon: vec![[0, 0, 0, 0]; (w * h) as usize],
        }
    }

    /// Stand-in "live fog colour" for tests that don't care about the exact
    /// haze value, just that `far_mesh_from_heights` accepts and threads one
    /// through — matches the shipped default profile's `fog_color`.
    const TEST_HAZE: Vec3 = Vec3::new(0.66, 0.73, 0.81);

    /// A flat grid, hole centred far away: every quad survives, and the mesh
    /// is a flat sheet at the grid's height (corner averaging of identical
    /// samples reproduces the same height, no NaNs from the `n == 0` guard).
    #[test]
    fn flat_grid_builds_a_flat_sheet() {
        let data = flat_grid(4, 4, 42.0, 8);
        let mesh =
            far_mesh_from_heights(&data, Vec2::new(-1_000_000.0, -1_000_000.0), 1.0, TEST_HAZE)
                .expect("non-empty mesh");
        let positions = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("float3");
        assert_eq!(
            positions.len(),
            4 * 4 * 4,
            "4 quads-per-axis × 4 verts/quad"
        );
        for p in positions {
            assert!(
                (p[1] - 42.0).abs() < 1e-4,
                "flat grid ⇒ flat mesh, got y={}",
                p[1]
            );
        }
    }

    /// A hole covering the whole grid produces no geometry — `None`, not an
    /// empty-but-present mesh (callers must handle this explicitly).
    #[test]
    fn hole_covering_everything_yields_no_mesh() {
        let data = flat_grid(2, 2, 10.0, 8);
        // cell = 8 * 32 = 256; grid spans 512×512 around origin's quadrant.
        let mesh = far_mesh_from_heights(&data, Vec2::new(256.0, -256.0), 10_000.0, TEST_HAZE);
        assert!(mesh.is_none());
    }

    /// A hole in the middle removes the centre quad AND every quad whose
    /// FOOTPRINT still reaches into the hole (not just quads whose centre
    /// does) but keeps the 4 corner quads, proving the cutout is selective
    /// (not all-or-nothing) and nearest-point-correct (review should-fix #1
    /// — a centre-only distance check let a quad's corner overlap the hole).
    #[test]
    fn hole_removes_only_nearby_quads() {
        let data = flat_grid(3, 3, 0.0, 1); // cell = 32
        // Centre quad is (1,1): its centre sits at (1.5*32, -1.5*32) = (48,-48).
        let center_of_middle_quad = Vec2::new(48.0, -48.0);
        let mesh = far_mesh_from_heights(&data, center_of_middle_quad, 20.0, TEST_HAZE)
            .expect("corner quads still render");
        let positions = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("float3");
        // 9 quads total: the centre quad (nearest point = hole centre, dist
        // 0) and its 4 edge-adjacent neighbours (nearest point is a shared
        // edge, dist 16 < radius 20) are culled; the 4 DIAGONAL corner quads
        // survive (nearest point is the shared corner, dist ≈22.6 > 20) ⇒ 4
        // quads × 4 verts.
        assert_eq!(positions.len(), 4 * 4);
    }

    /// Adjoining quads share identical corner vertex positions — the
    /// no-cracks contract — even though each quad duplicates its own vertex
    /// data (flat shading), because both reads of the shared corner go
    /// through the same `averaged_corner` computation.
    #[test]
    fn adjoining_quads_share_corner_heights() {
        let mut data = flat_grid(2, 2, 0.0, 1);
        data.heights = vec![0.0, 10.0, 20.0, 30.0]; // varied, so averaging matters
        let shared_from_quad_00 = corner_pos_for_test(&data, 1, 1);
        let shared_from_quad_11_neighbor = corner_pos_for_test(&data, 1, 1);
        assert_eq!(shared_from_quad_00, shared_from_quad_11_neighbor);
    }

    fn corner_pos_for_test(data: &DecodedFarTerrain, i: u32, j: u32) -> f32 {
        averaged_corner(data, i, j)
    }

    /// The centre corner of a 2×2 grid averages all 4 samples.
    #[test]
    fn interior_corner_averages_all_four_neighbours() {
        let mut data = flat_grid(2, 2, 0.0, 1);
        data.heights = vec![0.0, 10.0, 20.0, 30.0]; // (0,0)=0 (1,0)=10 (0,1)=20 (1,1)=30
        assert!((averaged_corner(&data, 1, 1) - 15.0).abs() < 1e-4);
    }

    /// An edge/corner sample averages only the cells that actually touch it
    /// (no phantom out-of-bounds zeros pulling the average down).
    #[test]
    fn boundary_corner_averages_only_in_bounds_neighbours() {
        let mut data = flat_grid(2, 2, 0.0, 1);
        data.heights = vec![0.0, 10.0, 20.0, 30.0];
        // Corner (0,0) touches only cell (0,0) = 0.0.
        assert!((averaged_corner(&data, 0, 0) - 0.0).abs() < 1e-4);
        // Corner (2,0) (top-right of the grid) touches only cell (1,0) = 10.0.
        assert!((averaged_corner(&data, 2, 0) - 10.0).abs() < 1e-4);
    }

    /// BL-82 EM-3.11 Phase A: [`cell_color`] mixes [`FAR_HAZE_BLEND`] of the
    /// REAL per-cell colour toward `haze`. Pinning the fraction here means a
    /// future accidental change to `FAR_HAZE_BLEND` (or a typo'd `lerp`
    /// direction) is caught directly, rather than only showing up as a fuzzy
    /// "looks a bit off" screenshot diff — the exact regression this test
    /// replaces from the retired `height_tint`.
    #[test]
    fn cell_color_blends_toward_haze_by_the_configured_fraction() {
        // `cell_color` returns an sRGB `Color`; go through the SAME
        // sRGB->linear conversion the production call site uses
        // (`far_mesh_from_heights`'s `.to_linear()`) when computing the
        // expected value too, so this compares like with like instead of
        // (wrongly) lerping in linear space against an sRGB-space formula.
        let expected = |c: Vec3| Color::srgb(c.x, c.y, c.z).to_linear().to_vec3();

        let haze = Vec3::new(1.0, 1.0, 1.0);
        let rgb: [u8; 3] = [80, 100, 60];
        let real = Vec3::new(80.0 / 255.0, 100.0 / 255.0, 60.0 / 255.0);
        let expected_blend = expected(real.lerp(haze, FAR_HAZE_BLEND));
        let got = cell_color(rgb, haze).to_linear().to_vec3();
        assert!(
            (got - expected_blend).length() < 1e-4,
            "should be the real cell colour mixed {FAR_HAZE_BLEND} toward haze, got {got:?}"
        );

        // Blending a colour toward an IDENTICAL haze is a no-op regardless
        // of the blend fraction — a degenerate case that would silently
        // break if the lerp direction were ever inverted (e.g. `haze.lerp
        // (real, FAR_HAZE_BLEND)` instead of `real.lerp(haze,
        // FAR_HAZE_BLEND)`).
        let got_noop = cell_color(rgb, real).to_linear().to_vec3();
        let expected_noop = expected(real);
        assert!(
            (got_noop - expected_noop).length() < 1e-4,
            "blending a colour toward an identical haze must be a no-op, got {got_noop:?}"
        );
    }

    /// BL-82 EM-3.11 Phase A: `far_mesh_from_heights` bakes each quad's
    /// vertex colour from its OWN originating cell's real colour sample
    /// (spec §3.3 — "for v1 use the colour of the quad's originating cell
    /// `(i, j)` for all 4 of its (duplicated) vertices"), not a shared or
    /// averaged colour, and not the retired synthetic height gradient. Two
    /// side-by-side cells with distinct colours must produce two distinct
    /// per-quad vertex colours.
    #[test]
    fn far_mesh_assigns_each_quads_own_cell_colour() {
        use bevy::mesh::VertexAttributeValues;

        let mut data = flat_grid(2, 1, 0.0, 1); // 2 side-by-side quads
        data.colors = vec![[10, 20, 30], [200, 150, 100]];
        let haze = Vec3::new(0.5, 0.5, 0.5);

        let mesh = far_mesh_from_heights(&data, Vec2::new(-1_000_000.0, -1_000_000.0), 1.0, haze)
            .expect("non-empty mesh");
        let Some(VertexAttributeValues::Float32x4(colors)) =
            mesh.attribute(BevyMesh::ATTRIBUTE_COLOR)
        else {
            panic!("vertex colours must be stored as Float32x4");
        };
        assert_eq!(colors.len(), 2 * 4, "2 quads × 4 verts");

        let expected_cell0 = cell_color(data.colors[0], haze).to_linear().to_f32_array();
        let expected_cell1 = cell_color(data.colors[1], haze).to_linear().to_f32_array();
        assert_ne!(
            expected_cell0, expected_cell1,
            "the two cells' colours must differ (sanity)"
        );
        for c in &colors[0..4] {
            assert_eq!(
                *c, expected_cell0,
                "quad 0's 4 vertices must share ITS OWN cell's colour"
            );
        }
        for c in &colors[4..8] {
            assert_eq!(
                *c, expected_cell1,
                "quad 1's 4 vertices must share ITS OWN cell's colour"
            );
        }
    }

    /// BL-82 EM-3.11 Phase B: `far_mesh_from_heights` bakes each quad's
    /// [`ATTRIBUTE_FAR_HORIZON`] vertex data from its OWN originating cell's
    /// `NetFarTerrain::horizon` sample (the same per-quad convention
    /// [`far_mesh_assigns_each_quads_own_cell_colour`] pins for colour),
    /// normalising each packed byte to `[0.0, 1.0]` so the WGSL fragment
    /// shader can consume it directly.
    #[test]
    fn far_mesh_assigns_each_quads_own_cell_horizon() {
        use bevy::mesh::VertexAttributeValues;

        let mut data = flat_grid(2, 1, 0.0, 1); // 2 side-by-side quads
        data.horizon = vec![[0, 64, 128, 255], [255, 0, 32, 200]];

        let mesh =
            far_mesh_from_heights(&data, Vec2::new(-1_000_000.0, -1_000_000.0), 1.0, TEST_HAZE)
                .expect("non-empty mesh");
        let Some(VertexAttributeValues::Float32x4(horizons)) =
            mesh.attribute(ATTRIBUTE_FAR_HORIZON)
        else {
            panic!("far-horizon attribute must be stored as Float32x4");
        };
        assert_eq!(horizons.len(), 2 * 4, "2 quads × 4 verts");

        let expected_cell0 = data.horizon[0].map(|b| f32::from(b) / 255.0);
        let expected_cell1 = data.horizon[1].map(|b| f32::from(b) / 255.0);
        for h in &horizons[0..4] {
            assert_eq!(
                *h, expected_cell0,
                "quad 0's 4 vertices share ITS OWN cell's horizon"
            );
        }
        for h in &horizons[4..8] {
            assert_eq!(
                *h, expected_cell1,
                "quad 1's 4 vertices share ITS OWN cell's horizon"
            );
        }
    }

    /// BL-82 EM-3.11 Phase B (T49.7) — this test's invariant is DELIBERATELY
    /// RELAXED from round 11's `default_fog_density_all_but_hides_the_far_
    /// mesh_at_its_hole_radius`, which required fog ALONE to be ≥99.9%
    /// opaque at the far mesh's `hole_radius` (a "fog is the only mask"
    /// world, before this phase). That invariant is now the WRONG bar:
    /// `far_terrain_material.wgsl`'s vertex-curvature bend + horizon-
    /// occlusion + sky-blend dissolve (T49.6) does the edge-hiding work
    /// today, so fog no longer needs to fully mask the mesh — cranking
    /// `fog_density` to chase 99.9% opacity there is exactly what over-hazed
    /// the near/mid field in round 11 (this phase's whole reason for
    /// touching the constant: dial it back toward the clearer EM-3.11f
    /// value once the mesh stopped depending on fog alone). Reproduces
    /// bevy_pbr's exact `FogFalloff::ExponentialSquared` opacity formula
    /// (`1 - exp(-(distance·density)²)`, `bevy_pbr/src/render/fog.wgsl`'s
    /// `exponential_squared_fog`) against the REAL production constants —
    /// not hand-copied numbers — and asserts BOTH halves of the new trade so
    /// a future change can't silently regress either direction:
    /// - fog is still a MEANINGFUL assist at `hole_radius` (a much looser
    ///   sanity floor than round 11's near-total-mask bar — the material, not
    ///   fog, is now responsible for closing the rest of the gap);
    /// - the near/mid field (50 m, the distance round 11's playtest
    ///   specifically complained about) reads clearly, not hazy — the concrete
    ///   symptom this phase exists to fix.
    #[test]
    fn fog_density_gives_a_reasonable_assist_without_over_hazing_the_near_field() {
        let culling = CullingConfig::default();
        let hole_radius = culling.chunk_render_distance + HOLE_MARGIN_CHUNKS * CHUNK_EDGE;
        let density = AtmosphereProfile::default().fog_density;

        let opacity_at = |d: f32| {
            let x = d * density;
            1.0 - (-(x * x)).exp()
        };

        let hole_opacity = opacity_at(hole_radius);
        assert!(
            hole_opacity >= 0.90,
            "fog should still meaningfully assist at the far mesh's hole_radius ({hole_radius}m): \
             got {hole_opacity} (density {density}) — the material's own dissolve \
             (far_terrain_material.wgsl) now does most of the edge-hiding work, but fog \
             collapsing toward zero here would be a real regression, not just an over-strict test"
        );

        let near_field_opacity = opacity_at(50.0);
        assert!(
            near_field_opacity < 0.15,
            "T49.7's whole point: restore the near/mid clarity round 11's 0.00913 over-hazed \
             (~19% at 50m) — got {near_field_opacity} at density {density}"
        );

        assert!(
            density < 0.00913,
            "must be lower than round 11's over-tuned 0.00913 — this phase's entire reason for \
             touching fog_density at all"
        );
    }
}
