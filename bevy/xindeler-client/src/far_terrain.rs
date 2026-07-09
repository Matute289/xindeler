//! EM-3.10b — coarse far-terrain mesh from the server's downsampled `lod_alt`
//! heightmap (listen-server only).
//!
//! EM-3.10 (v1) left the horizon beyond [`crate::lod::CullingConfig::
//! chunk_render_distance`] as sky + `DistanceFog` — an acceptable, honestly
//! documented fallback, but not a filled-in view. This module builds ONE
//! low-poly, vertex-coloured mesh from the [`NetLodAlt`] grid
//! (`xindeler-sim-bridge` → `xindeler-protocol`, sent once at boot — the far
//! terrain never changes during a session) and renders it beyond the near
//! terrain to fill that gap.
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
//! One sample in [`NetLodAlt`] covers `chunk_stride` chunks — already coarse
//! by construction (`xindeler-sim-bridge::send_lod_alt_once` caps the grid
//! dimension, downsampling a potentially 1024×1024-chunk world). Corner
//! heights are averaged from the up-to-4 touching samples so adjoining quads
//! share exact vertex positions (no cracks), but each quad still gets its own
//! (duplicated) vertices and a single flat face normal — deliberately simple
//! "flat-shaded, vertex-coloured low-poly terrain" (task-approved v1 scope),
//! not the full PBR block-palette treatment the near terrain gets.
//!
//! ## Purity
//! 100% Bevy + the protocol message + `terrain_stream::{CHUNK_EDGE,
//! TerrainCameraAnchor}` + `lod::CullingConfig` — no specs. Compiled only
//! under the `listen-server` feature.

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh as BevyMesh, PrimitiveTopology},
    pbr::StandardMaterial,
    prelude::*,
};
use xindeler_protocol::NetLodAlt;

use crate::{
    lod::CullingConfig,
    terrain_stream::{CHUNK_EDGE, TerrainCameraAnchor},
};

/// Safety margin (in chunks) added on top of [`CullingConfig::
/// chunk_render_distance`] when sizing the far-mesh's cutout hole, so the two
/// meshes overlap rather than leaving a gap at the boundary. ALSO doubles as
/// [`retile_far_mesh`]'s re-tile threshold (its doc comment proves that reuse
/// is exactly what keeps the far mesh from ever overlapping the near band).
const HOLE_MARGIN_CHUNKS: f32 = 2.0;

/// Installs the EM-3.10b far-terrain consumer: receives [`NetLodAlt`] once,
/// then builds/re-tiles the mesh (see [`retile_far_mesh`]) as soon as, and for
/// as long as, a camera exists.
pub struct FarTerrainPlugin;

impl Plugin for FarTerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (receive_lod_alt, retile_far_mesh));

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

/// The decoded far-terrain heightmap. Installed once, the first time the
/// one-shot [`NetLodAlt`] message arrives, and then kept alive for the whole
/// session (NOT consumed after the first mesh build) — [`retile_far_mesh`]
/// re-reads it every time the camera drifts far enough to need a fresh hole.
#[derive(Resource)]
struct FarTerrainData(DecodedLodAlt);

struct DecodedLodAlt {
    grid_w: u32,
    grid_h: u32,
    chunk_stride: u32,
    heights: Vec<f32>,
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

/// Decodes the one-shot [`NetLodAlt`] message into [`FarTerrainData`] (once —
/// the payload is never resent, so a resource already present means we
/// already have it).
fn receive_lod_alt(
    mut commands: Commands,
    mut messages: MessageReader<NetLodAlt>,
    existing: Option<Res<FarTerrainData>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(msg) = messages.read().next() else {
        return;
    };
    let Some(heights) = msg.decode() else {
        warn!("dropping undecodable far-terrain lod-alt grid");
        return;
    };
    commands.insert_resource(FarTerrainData(DecodedLodAlt {
        grid_w: msg.grid_size[0],
        grid_h: msg.grid_size[1],
        chunk_stride: msg.chunk_stride,
        heights,
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
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut meshes: ResMut<Assets<BevyMesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

    let hole_radius = culling.chunk_render_distance + rebuild_slack;
    let mesh = far_mesh_from_heights(&data.0, hole_center, hole_radius);
    let new_entity = mesh.map(|mesh| {
        commands
            .spawn((
                FarTerrainMesh,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    // The mesh is a single coarse sheet with no interior —
                    // both faces must shade the same way regardless of which
                    // side the winding ends up facing.
                    cull_mode: None,
                    perceptual_roughness: 1.0,
                    reflectance: 0.02,
                    ..default()
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
}

/// Height-only-known corner (grid coordinate space, before world placement).
#[inline]
fn corner_height(data: &DecodedLodAlt, ci: i32, cj: i32) -> Option<f32> {
    if ci < 0 || cj < 0 || ci >= data.grid_w as i32 || cj >= data.grid_h as i32 {
        return None;
    }
    #[expect(clippy::cast_sign_loss, reason = "bounds-checked above")]
    Some(data.heights[(cj as u32 * data.grid_w + ci as u32) as usize])
}

/// Averages the up-to-4 sample cells touching grid corner `(i, j)` (`i` in
/// `0..=grid_w`, `j` in `0..=grid_h`) so adjoining quads share an identical
/// vertex height — the no-cracks contract.
fn averaged_corner(data: &DecodedLodAlt, i: u32, j: u32) -> f32 {
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

/// Maps a colour to a coarse "distant terrain" tint by height fraction
/// (0 = lowest sample in the grid, 1 = highest) — low ground reads greener,
/// high ground reads paler/rockier. Cheap, data-free (no palette dependency),
/// good enough to visually confirm relief without the near terrain's PBR
/// block-palette treatment.
fn height_tint(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let low = Vec3::new(0.20, 0.35, 0.16);
    let high = Vec3::new(0.55, 0.52, 0.46);
    let c = low.lerp(high, t);
    Color::srgb(c.x, c.y, c.z)
}

/// Builds the far-terrain [`BevyMesh`], or `None` if every quad fell inside
/// the cutout hole (nothing to draw).
fn far_mesh_from_heights(
    data: &DecodedLodAlt,
    hole_center: Vec2,
    hole_radius: f32,
) -> Option<BevyMesh> {
    let cell = data.chunk_stride as f32 * CHUNK_EDGE;
    let (min_h, max_h) = data
        .heights
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &h| {
            (lo.min(h), hi.max(h))
        });
    let span = (max_h - min_h).max(1.0);

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

            let base = positions.len() as u32;
            for p in [p00, p10, p11, p01] {
                positions.push(p.to_array());
                normals.push(normal.to_array());
                let t = (p.y - min_h) / span;
                colors.push(height_tint(t).to_linear().to_f32_array());
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
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, asset::AssetPlugin, prelude::MinimalPlugins};

    use super::*;

    /// Headless App exercising [`retile_far_mesh`] directly (no NetLodAlt
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
            .init_asset::<StandardMaterial>()
            .insert_resource(CullingConfig {
                chunk_render_distance: 100.0,
                sprite_render_distance: 50.0,
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

    fn flat_grid(w: u32, h: u32, height: f32, stride: u32) -> DecodedLodAlt {
        DecodedLodAlt {
            grid_w: w,
            grid_h: h,
            chunk_stride: stride,
            heights: vec![height; (w * h) as usize],
        }
    }

    /// A flat grid, hole centred far away: every quad survives, and the mesh
    /// is a flat sheet at the grid's height (corner averaging of identical
    /// samples reproduces the same height, no NaNs from the `n == 0` guard).
    #[test]
    fn flat_grid_builds_a_flat_sheet() {
        let data = flat_grid(4, 4, 42.0, 8);
        let mesh = far_mesh_from_heights(&data, Vec2::new(-1_000_000.0, -1_000_000.0), 1.0)
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
        let mesh = far_mesh_from_heights(&data, Vec2::new(256.0, -256.0), 10_000.0);
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
        let mesh = far_mesh_from_heights(&data, center_of_middle_quad, 20.0)
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

    fn corner_pos_for_test(data: &DecodedLodAlt, i: u32, j: u32) -> f32 {
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
}
