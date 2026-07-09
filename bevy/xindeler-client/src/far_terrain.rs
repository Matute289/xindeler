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
//! The far mesh is built ONCE, in WORLD space, and never rebuilt as the camera
//! moves (no per-frame cost, no re-streaming). To avoid z-fighting / visible
//! seams against the near, block-accurate chunk meshes, we cut a circular hole
//! out of it around the (also one-shot) [`TerrainCameraAnchor`] position, sized
//! to comfortably exceed the near [`crate::lod::CullingConfig::
//! chunk_render_distance`] band. Because v1's player stays near that single
//! anchor (no interest-management roaming yet — EM-4.2d), a fixed hole is
//! correct today; a camera-following hole (rebuilt/shader-masked) is future
//! work once the player can roam far from the boot anchor (EM-3.10c).
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
/// meshes overlap rather than leaving a gap at the boundary even though the
/// hole is centred on the static boot anchor rather than the live camera.
const HOLE_MARGIN_CHUNKS: f32 = 2.0;

/// Installs the EM-3.10b far-terrain consumer: receives [`NetLodAlt`] once,
/// builds the mesh once (as soon as the anchor is also known), spawns it.
pub struct FarTerrainPlugin;

impl Plugin for FarTerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingLodAlt>()
            .add_systems(Update, (receive_lod_alt, build_far_mesh_when_ready));

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

/// The decoded (but not-yet-meshed) heightmap, held until the anchor arrives
/// too (message order between the two one-shot broadcasts is not guaranteed).
#[derive(Resource, Default)]
struct PendingLodAlt(Option<DecodedLodAlt>);

struct DecodedLodAlt {
    grid_w: u32,
    grid_h: u32,
    chunk_stride: u32,
    heights: Vec<f32>,
}

/// Marks the spawned far-terrain mesh entity (so [`build_far_mesh_when_ready`]
/// never spawns a second one, and so a smoke/debug harness can find it).
#[derive(Component)]
pub struct FarTerrainMesh;

/// Decodes the one-shot [`NetLodAlt`] message into [`PendingLodAlt`].
fn receive_lod_alt(mut messages: MessageReader<NetLodAlt>, mut pending: ResMut<PendingLodAlt>) {
    if pending.0.is_some() {
        return; // already have it (or already meshed and consumed)
    }
    let Some(msg) = messages.read().next() else {
        return;
    };
    let Some(heights) = msg.decode() else {
        warn!("dropping undecodable far-terrain lod-alt grid");
        return;
    };
    pending.0 = Some(DecodedLodAlt {
        grid_w: msg.grid_size[0],
        grid_h: msg.grid_size[1],
        chunk_stride: msg.chunk_stride,
        heights,
    });
}

/// Once BOTH the decoded heightmap and the [`TerrainCameraAnchor`] (for the
/// hole centre) are available, builds and spawns the far-terrain mesh exactly
/// once, then drops [`PendingLodAlt`]'s payload so this never re-runs.
fn build_far_mesh_when_ready(
    mut commands: Commands,
    mut pending: ResMut<PendingLodAlt>,
    anchor: Option<Res<TerrainCameraAnchor>>,
    culling: Res<CullingConfig>,
    mut meshes: ResMut<Assets<BevyMesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<(), With<FarTerrainMesh>>,
) {
    if !existing.is_empty() {
        return;
    }
    let Some(anchor) = anchor else { return };
    let Some(data) = pending.0.take() else { return };

    let hole_radius = culling.chunk_render_distance + HOLE_MARGIN_CHUNKS * CHUNK_EDGE;
    let hole_center = Vec2::new(anchor.bevy_pos.x, anchor.bevy_pos.z);

    let Some(mesh) = far_mesh_from_heights(&data, hole_center, hole_radius) else {
        // Every quad fell inside the hole (tiny world) — nothing to draw; the
        // sky+fog fallback stays in effect, which is the documented
        // acceptable outcome for a degenerate case, not a bug.
        info!("far-terrain grid entirely inside the near band; skipping the far mesh");
        return;
    };

    commands.spawn((
        FarTerrainMesh,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            // The mesh is a single coarse sheet with no interior — both faces
            // must shade the same way regardless of which side the (fixed,
            // one-shot) winding ends up facing.
            cull_mode: None,
            perceptual_roughness: 1.0,
            reflectance: 0.02,
            ..default()
        })),
        Transform::IDENTITY, // positions are already absolute world-space
        Visibility::Visible,
    ));
    info!(
        grid_w = data.grid_w,
        grid_h = data.grid_h,
        stride = data.chunk_stride,
        "far-terrain mesh built (EM-3.10b)"
    );
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
    use super::*;

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
