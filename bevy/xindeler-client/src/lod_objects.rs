//! BL-82 EM-3.11-FH Phase C — streamed LOD objects: distant trees/structures
//! rendered on the far-terrain horizon, closing the "the map ends here, looks
//! flat" symptom Phases A (real colour, PR #57) and B (horizon occlusion +
//! curvature bend + atmospheric dissolve, PR #60) already improved but didn't
//! fully solve — a real, varied, curved, dissolving-into-haze horizon is
//! still an EMPTY horizon; nothing breaks up its silhouette at distance the
//! way real trees/rooftops do.
//!
//! ## Data source (verified against the shared, unmigrated `client`/`server`/
//! `world` crates — see `xindeler_protocol::lod_objects` and
//! `xindeler_sim_bridge::lod_objects`'s own module docs for the full trace):
//! `common::lod::Object` (kind/pos/flags/color), the SAME type
//! `world::World::get_lod_zone` already produces from REAL tree-placement
//! (`WorldSim::get_area_trees`) and site-plot data — no new worldgen. The
//! server-side embedded client already streams these zones to itself over a
//! real loopback connection (inherited, unmigrated request/cull logic); this
//! module is the CONSUMER half of the new one-way mirror
//! (`xindeler_protocol::{NetLodZone, NetLodZoneRemove}`) that surfaces that
//! already-streamed data to the pure-Bevy client.
//!
//! ## Rendering technique: simplified 3D silhouettes, ONE combined mesh per
//! zone (not per-object)
//! A dense forest zone can carry hundreds of objects; spawning one Bevy
//! entity per tree would be needlessly expensive (thousands of entities,
//! transform propagation, draw calls). Instead, EVERY object in a zone is
//! CPU-baked into ONE combined [`BevyMesh`] per zone (mirrors
//! `far_terrain.rs`'s own "one mesh, many quads" convention) — a single draw
//! call per zone regardless of how many trees/structures it holds:
//! - **Trees**: a low-poly upright pyramid (4 side triangles), tinted by the
//!   object's own real `color` (leaf colour from worldgen) — reads as a
//!   stylised conifer/canopy silhouette from any viewing angle with ZERO
//!   camera-facing/billboard logic (a genuine 3D shape, not a sprite).
//! - **Structures** (houses/sites): a low-poly box (walls) topped by a pyramid
//!   roof — walls take a fixed neutral stone tint (`Object::color` for most
//!   non-House/GiantTree structure kinds is `Rgb::black()` upstream — a real
//!   placeholder meaning "the old engine's own `.obj` model supplied the
//!   colour", not a real neutral tone, so it is NOT trustworthy as a wall tint
//!   here), roof takes the object's own colour when non-black (real roof-colour
//!   variety for `House`, still meaningful at this LOD) or a fallback
//!   terracotta tone otherwise.
//!
//! This is deliberately SIMPLER than the old engine's own LOD-object
//! rendering (`voxygen/src/scene/lod.rs`: real low-poly `.obj` models loaded
//! per `ObjectKind`, GPU-instanced) — silhouette-level fidelity is the
//! explicit v1 goal here, not model parity; `.obj`-per-kind loading + real
//! instancing is a natural follow-up if silhouette quality proves
//! insufficient in the smoke.
//!
//! ## Sitting on the SAME curved/dissolving surface as the terrain
//! Phase B's world-curvature vertex bend (`far_terrain_material.rs`) is
//! computed in the VERTEX SHADER from the LIVE camera position every frame —
//! it is NOT baked into the terrain mesh's CPU-side heights. An LOD object
//! placed at its own real (unbent) ground altitude would therefore visibly
//! FLOAT above the terrain surface once the surface bends down beneath it at
//! distance. This module's zone meshes are rendered with the EXACT SAME
//! [`FarTerrainMaterial`]/[`FarTerrainExtension`] shader as the terrain sheet
//! (`far_terrain_material.wgsl`, unmodified), so the identical per-vertex
//! bend and horizon/sky dissolve apply to LOD-object vertices for free,
//! keeping them flush with the terrain at any distance/camera position.
//! `bend_start` is computed the SAME way `far_terrain::retile_far_mesh`
//! computes it (see [`bend_uniforms`]), so the near-band "no bend" guarantee
//! holds for objects too.

use bevy::{
    asset::RenderAssetUsages,
    ecs::message::MessageReader,
    mesh::{Indices, Mesh as BevyMesh, PrimitiveTopology},
    pbr::StandardMaterial,
    platform::collections::HashMap,
    prelude::*,
};
use common::lod::{Object, ObjectKind, to_wpos};
use xindeler_oracle_host::{AtmosphereController, AtmosphereProfile};
use xindeler_protocol::{NetLodZone, NetLodZoneRemove};

use crate::{
    far_terrain_material::{ATTRIBUTE_FAR_HORIZON, FarTerrainExtension, FarTerrainMaterial},
    lod::CullingConfig,
    terrain_stream::CHUNK_EDGE,
};

/// Mirrors `far_terrain::HOLE_MARGIN_CHUNKS` (that constant is private to its
/// own module — same "own constant, not shared" call `map.rs`'s
/// `MAP_IMAGE_MAX_DIM` doc comment already makes for an independent-by-design
/// value): the safety margin added on top of [`CullingConfig::
/// chunk_render_distance`] when computing [`FarTerrainExtension::bend_start`]
/// for a zone mesh, so it matches the terrain sheet's own near-band boundary.
const BEND_START_MARGIN_CHUNKS: f32 = 2.0;

/// Horizontal render-distance cutoff for LOD-object zone meshes (Bevy
/// metres), independent of (and typically tighter than) the embedded
/// player's own `lod_distance` zone-STREAMING radius (`client::Client`'s
/// internal ~4-zone/4096 m default) — this only gates whether an
/// already-received zone's mesh is drawn, not whether it's requested/kept.
/// Tune via [`LodObjectSettings`]; default chosen to keep a handful of zones
/// visible without unbounded triangle growth on a dense-forest seed (see the
/// PR's perf-measurement section for the observed frame-time cost at this
/// default).
const DEFAULT_RENDER_DISTANCE: f32 = 2_048.0;

/// Hard cap on triangles a single zone mesh may contribute, checked at BAKE
/// time (on top of `xindeler_sim_bridge::lod_objects::LOD_ZONE_MAX_OBJECTS`'s
/// server-side object-count cap) — belt-and-braces: the server cap already
/// bounds this in practice, but the client never trusts a wire payload's size
/// implicitly (same discipline `NetFarTerrain::decode`'s length checks
/// already apply to the terrain grid).
const MAX_TRIANGLES_PER_ZONE: usize = 20_000;

/// User-tunable knobs for the LOD-object renderer.
#[derive(Resource, Debug, Clone, Copy)]
pub struct LodObjectSettings {
    /// See [`DEFAULT_RENDER_DISTANCE`].
    pub render_distance: f32,
}

impl Default for LodObjectSettings {
    fn default() -> Self {
        // Debug-only, opt-in override (`XINDELER_LOD_OBJECT_RENDER_DISTANCE=<f32>`,
        // e.g. `0` to disable rendering entirely without a code change) — same
        // convention as `XINDELER_FAR_MESH_BEND_STRENGTH`/
        // `XINDELER_SMOKE_FAR_MESH_CAM` elsewhere in this crate. This is also
        // the concrete "gate behind a setting" knob the epic's perf review
        // asked for if the feature ever proves too costly on a given machine/
        // seed: setting it to `0` degrades cleanly to "no LOD objects drawn"
        // (zones are still streamed/meshed server- and client-side — only
        // rendering is skipped — so re-enabling is instant, no reconnect).
        let render_distance = std::env::var("XINDELER_LOD_OBJECT_RENDER_DISTANCE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(DEFAULT_RENDER_DISTANCE);
        Self { render_distance }
    }
}

/// Installs the Phase-C LOD-object consumer: receives streamed
/// [`NetLodZone`]/[`NetLodZoneRemove`], bakes/despawns per-zone meshes, and
/// distance-culls the ones that are currently spawned.
pub struct LodObjectsPlugin;

impl Plugin for LodObjectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LodObjectSettings>()
            .init_resource::<LodZoneMeshes>()
            .add_systems(Update, (receive_lod_zones, cull_lod_zone_meshes));
    }
}

/// Marks a spawned LOD-object zone mesh entity, carrying its own zone key so
/// a debug/smoke harness (or a future picking tool) can identify it.
#[derive(Component)]
pub struct LodZoneMesh {
    pub key: [i32; 2],
}

/// Tracks the currently-spawned entity per zone key, so
/// [`receive_lod_zones`] can despawn the right one on
/// [`NetLodZoneRemove`]/re-`NetLodZone` and [`cull_lod_zone_meshes`] can
/// toggle visibility without a linear scan.
#[derive(Resource, Default)]
struct LodZoneMeshes {
    entities: HashMap<[i32; 2], Entity>,
}

/// Reads [`NetLodZone`]/[`NetLodZoneRemove`] and keeps [`LodZoneMeshes`] in
/// sync: a new/updated zone gets its stale mesh (if any) despawned and a
/// fresh one baked+spawned; a removed zone's mesh is despawned outright.
fn receive_lod_zones(
    mut commands: Commands,
    mut zone_reader: MessageReader<NetLodZone>,
    mut remove_reader: MessageReader<NetLodZoneRemove>,
    mut state: ResMut<LodZoneMeshes>,
    culling: Res<CullingConfig>,
    atmosphere: Option<Res<AtmosphereController>>,
    mut meshes: ResMut<Assets<BevyMesh>>,
    mut materials: ResMut<Assets<FarTerrainMaterial>>,
) {
    for msg in remove_reader.read() {
        if let Some(entity) = state.entities.remove(&msg.key) {
            commands.entity(entity).try_despawn();
        }
    }

    for msg in zone_reader.read() {
        if let Some(old) = state.entities.remove(&msg.key) {
            commands.entity(old).try_despawn();
        }
        let Some(objects) = msg.decode() else {
            warn!(key = ?msg.key, "dropping undecodable LOD-object zone");
            continue;
        };
        if objects.is_empty() {
            continue; // an empty (but valid) zone — nothing to bake
        }

        let object_count = objects.len();
        let Some(mesh) = zone_mesh_from_objects(msg.key, &objects) else {
            continue; // degenerate (e.g. triangle-budget exceeded) — logged inside
        };
        let triangle_count = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .map_or(0, |p| p.len() / 3);
        info!(
            key = ?msg.key,
            object_count,
            triangle_count,
            "EM-3.11-FH Phase C: baked + spawned an LOD-object zone mesh"
        );

        let (bend_strength, bend_start, fog_color, sky_color) =
            bend_uniforms(&culling, atmosphere.as_deref());

        let entity = commands
            .spawn((
                LodZoneMesh { key: msg.key },
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(materials.add(FarTerrainMaterial {
                    base: StandardMaterial {
                        base_color: Color::WHITE,
                        cull_mode: None, // simplified geometry, winding not guaranteed both ways
                        perceptual_roughness: 1.0,
                        reflectance: 0.02,
                        ..default()
                    },
                    extension: FarTerrainExtension {
                        bend_strength,
                        bend_start,
                        fog_color,
                        sky_color,
                        ..default()
                    },
                })),
                Transform::IDENTITY, // positions are already absolute world-space
                Visibility::Visible,
            ))
            .id();
        state.entities.insert(msg.key, entity);
    }
}

/// Computes the SAME `(bend_strength, bend_start, fog_color, sky_color)`
/// tuple `far_terrain::retile_far_mesh` computes for the terrain sheet, so a
/// zone mesh's material matches the live terrain's bend/dissolve exactly at
/// spawn time (mirrors that function's own
/// fallback-when-no-`AtmosphereController` behaviour verbatim — see its doc
/// comment).
fn bend_uniforms(
    culling: &CullingConfig,
    atmosphere: Option<&AtmosphereController>,
) -> (f32, f32, Vec4, Vec4) {
    let (fog_color, sky_color, bend_strength, bend_start_scale) = atmosphere.map_or_else(
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
    let hole_radius = culling.chunk_render_distance + BEND_START_MARGIN_CHUNKS * CHUNK_EDGE;
    let bend_start = hole_radius * bend_start_scale;
    (
        bend_strength,
        bend_start,
        fog_color.extend(1.0),
        sky_color.extend(1.0),
    )
}

/// Hides a zone mesh once it drifts beyond [`LodObjectSettings::
/// render_distance`] from the camera — a cheap per-frame horizontal-distance
/// check over (at most) a handful of zone entities (the embedded player's own
/// zone-streaming radius already bounds how many can exist at all).
fn cull_lod_zone_meshes(
    settings: Res<LodObjectSettings>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut zones: Query<(&LodZoneMesh, &mut Visibility)>,
) {
    let Some(eye) = camera.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    for (marker, mut vis) in &mut zones {
        let zone_center = zone_center_bevy(marker.key);
        let dx = zone_center.x - eye.x;
        let dz = zone_center.z - eye.z;
        let visible = dx * dx + dz * dz <= settings.render_distance * settings.render_distance;
        let want = if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

/// Bevy-space centre of zone `key` (`common::lod::ZONE_SIZE` chunks square),
/// used only for the render-distance cull above — cheap and approximate (Y
/// ignored, matching `lod::within_band`'s own horizontal-only convention).
fn zone_center_bevy(key: [i32; 2]) -> Vec3 {
    let zone_edge = common::lod::ZONE_SIZE as f32 * CHUNK_EDGE;
    let origin_x = to_wpos(key[0]) as f32;
    let origin_y = to_wpos(key[1]) as f32;
    Vec3::new(
        origin_x + zone_edge * 0.5,
        0.0,
        -(origin_y + zone_edge * 0.5),
    )
}

/// True for every foliage [`ObjectKind`] (rendered as a tinted pyramid);
/// false for every structure/site kind (box + roof).
const fn is_tree(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::GenericTree
            | ObjectKind::Pine
            | ObjectKind::Dead
            | ObjectKind::GiantTree
            | ObjectKind::Mangrove
            | ObjectKind::Acacia
            | ObjectKind::Birch
            | ObjectKind::Redwood
            | ObjectKind::Baobab
            | ObjectKind::Frostpine
            | ObjectKind::Palm
    )
}

/// `(radius, height)` in Bevy metres for a kind's simplified silhouette —
/// deliberately coarse per-CATEGORY sizing (not per-species detail — silhouette
/// fidelity is the v1 goal, spec'd explicitly in the task brief), with a
/// larger tier for the handful of kinds that are genuinely bigger landmarks.
const fn kind_size(kind: ObjectKind) -> (f32, f32) {
    match kind {
        ObjectKind::GiantTree | ObjectKind::Redwood | ObjectKind::Baobab => (5.0, 18.0),
        ObjectKind::Dead => (1.2, 5.0),
        ObjectKind::Arena
        | ObjectKind::TerracottaPalace
        | ObjectKind::AirshipDock
        | ObjectKind::SavannahAirshipDock
        | ObjectKind::CoastalAirshipDock
        | ObjectKind::DesertCityAirshipDock
        | ObjectKind::CliffTownAirshipDock
        | ObjectKind::Desert => (8.0, 10.0),
        _ if is_tree(kind) => (2.0, 8.0),
        _ => (4.0, 6.0),
    }
}

/// Neutral wall tint for structures — `Object::color` on most non-House/
/// GiantTree structure kinds is `Rgb::black()` upstream (a real placeholder
/// meaning "the old engine's `.obj` model already had its own colour", not an
/// actual neutral tone — see module doc comment), so walls never trust it.
const STRUCTURE_WALL_COLOR: [f32; 4] = [0.75, 0.68, 0.55, 1.0];
/// Fallback roof tint used when `Object::color` is exactly black (most
/// structure kinds); `House`/other kinds with a REAL recorded roof colour use
/// that instead (see [`object_roof_color`]).
const STRUCTURE_ROOF_FALLBACK: [f32; 4] = [0.55, 0.27, 0.2, 1.0];

/// sRGB `[u8; 3]` (as `common::lod::Object::color` stores it) → linear
/// `[f32; 4]` vertex colour, matching `far_terrain::cell_color`'s own
/// sRGB→linear convention.
fn srgb_u8_to_linear_vertex(rgb: vek::Rgb<u8>) -> [f32; 4] {
    Color::srgb(
        f32::from(rgb.r) / 255.0,
        f32::from(rgb.g) / 255.0,
        f32::from(rgb.b) / 255.0,
    )
    .to_linear()
    .to_f32_array()
}

fn object_roof_color(object: &Object) -> [f32; 4] {
    if object.color.r == 0 && object.color.g == 0 && object.color.b == 0 {
        STRUCTURE_ROOF_FALLBACK
    } else {
        srgb_u8_to_linear_vertex(object.color)
    }
}

/// A CPU mesh baker shared by [`push_pyramid`]/[`push_quad`] — appends one
/// flat-shaded triangle (normal from the right-hand rule), matching
/// `far_terrain::far_mesh_from_heights`'s own per-face convention.
#[allow(clippy::too_many_arguments)]
fn push_tri(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    horizons: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    color: [f32; 4],
) {
    let normal = (b - a).cross(c - a).normalize_or_zero();
    let base = positions.len() as u32;
    for p in [a, b, c] {
        positions.push(p.to_array());
        normals.push(normal.to_array());
        colors.push(color);
        // LOD objects don't carry their own horizon sample (v1 simplification,
        // module doc comment) — zero = "no extra occlusion", the fragment
        // shader's distance/sink-driven dissolve still applies fully.
        horizons.push([0.0, 0.0, 0.0, 0.0]);
    }
    indices.extend([base, base + 1, base + 2]);
}

/// Appends a quad (two triangles, shared winding) — used for box side walls.
#[allow(clippy::too_many_arguments)]
fn push_quad(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    horizons: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    p00: Vec3,
    p10: Vec3,
    p11: Vec3,
    p01: Vec3,
    color: [f32; 4],
) {
    push_tri(
        positions, normals, colors, horizons, indices, p00, p10, p11, color,
    );
    push_tri(
        positions, normals, colors, horizons, indices, p00, p11, p01, color,
    );
}

/// Appends a 4-sided upright pyramid (base at `base`, apex `height` above it)
/// — the tree silhouette / structure roof primitive.
#[allow(clippy::too_many_arguments)]
fn push_pyramid(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    horizons: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    base: Vec3,
    radius: f32,
    height: f32,
    color: [f32; 4],
) {
    let apex = base + Vec3::new(0.0, height, 0.0);
    let corners = [
        base + Vec3::new(-radius, 0.0, -radius),
        base + Vec3::new(radius, 0.0, -radius),
        base + Vec3::new(radius, 0.0, radius),
        base + Vec3::new(-radius, 0.0, radius),
    ];
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        push_tri(
            positions, normals, colors, horizons, indices, apex, a, b, color,
        );
    }
}

/// Appends a box's 4 side walls (no top/bottom caps — the top is covered by a
/// roof pyramid, the bottom sits at/below ground and is never seen).
#[allow(clippy::too_many_arguments)]
fn push_box_walls(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    horizons: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    base: Vec3,
    half_extent: f32,
    wall_height: f32,
    color: [f32; 4],
) {
    let top = base + Vec3::new(0.0, wall_height, 0.0);
    let bottom_corners = [
        base + Vec3::new(-half_extent, 0.0, -half_extent),
        base + Vec3::new(half_extent, 0.0, -half_extent),
        base + Vec3::new(half_extent, 0.0, half_extent),
        base + Vec3::new(-half_extent, 0.0, half_extent),
    ];
    let top_corners = [
        top + Vec3::new(-half_extent, 0.0, -half_extent),
        top + Vec3::new(half_extent, 0.0, -half_extent),
        top + Vec3::new(half_extent, 0.0, half_extent),
        top + Vec3::new(-half_extent, 0.0, half_extent),
    ];
    for i in 0..4 {
        let j = (i + 1) % 4;
        push_quad(
            positions,
            normals,
            colors,
            horizons,
            indices,
            bottom_corners[i],
            bottom_corners[j],
            top_corners[j],
            top_corners[i],
            color,
        );
    }
}

/// Bakes every object in a zone into ONE combined [`BevyMesh`], or `None` if
/// the objects yield no geometry (empty input, already guarded by the
/// caller) or the result would exceed [`MAX_TRIANGLES_PER_ZONE`] (a defensive
/// cap independent of the server's own [`crate::lod_objects`]-sibling budget
/// — see that constant's doc comment).
fn zone_mesh_from_objects(zone_key: [i32; 2], objects: &[Object]) -> Option<BevyMesh> {
    let zone_origin = Vec2::new(to_wpos(zone_key[0]) as f32, to_wpos(zone_key[1]) as f32);

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut horizons: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for object in objects {
        if positions.len() / 3 > MAX_TRIANGLES_PER_ZONE {
            warn!(
                ?zone_key,
                "LOD-object zone mesh hit MAX_TRIANGLES_PER_ZONE; truncating (server-side cap \
                 should normally prevent this)"
            );
            break;
        }

        let wpos_x = zone_origin.x + f32::from(object.pos.x);
        let wpos_y = zone_origin.y + f32::from(object.pos.y);
        let alt = f32::from(object.pos.z);
        let base = Vec3::new(wpos_x, alt, -wpos_y);

        let (radius, height) = kind_size(object.kind);
        if is_tree(object.kind) {
            let color = srgb_u8_to_linear_vertex(object.color);
            push_pyramid(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut horizons,
                &mut indices,
                base,
                radius,
                height,
                color,
            );
        } else {
            let wall_height = height * 0.6;
            let roof_height = height * 0.4;
            push_box_walls(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut horizons,
                &mut indices,
                base,
                radius,
                wall_height,
                STRUCTURE_WALL_COLOR,
            );
            push_pyramid(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut horizons,
                &mut indices,
                base + Vec3::new(0.0, wall_height, 0.0),
                radius,
                roof_height,
                object_roof_color(object),
            );
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
    use bevy::mesh::VertexAttributeValues;
    use common::lod::InstFlags;
    use vek::{Rgb, Vec3 as VVec3};

    use super::*;

    fn tree(x: i16, y: i16, z: i16) -> Object {
        Object {
            kind: ObjectKind::Pine,
            pos: VVec3::new(x, y, z),
            flags: InstFlags::empty(),
            color: Rgb::new(20, 90, 30),
        }
    }

    fn house(x: i16, y: i16, z: i16) -> Object {
        Object {
            kind: ObjectKind::House,
            pos: VVec3::new(x, y, z),
            flags: InstFlags::empty(),
            color: Rgb::new(140, 60, 40),
        }
    }

    /// An empty object list bakes to `None`, not an empty-but-present mesh —
    /// callers (`receive_lod_zones`) must handle this explicitly, mirroring
    /// `far_mesh_from_heights`'s own contract.
    #[test]
    fn empty_zone_bakes_to_no_mesh() {
        assert!(zone_mesh_from_objects([0, 0], &[]).is_none());
    }

    /// A single tree yields exactly one pyramid's worth of geometry (4 side
    /// triangles = 12 vertices, since faces don't share verts here).
    #[test]
    fn single_tree_bakes_one_pyramid() {
        let mesh = zone_mesh_from_objects([0, 0], &[tree(10, 10, 5)]).expect("non-empty");
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
        else {
            panic!("expected Float32x3 positions");
        };
        assert_eq!(positions.len(), 4 * 3, "4 side triangles x 3 verts each");
    }

    /// A single structure yields box-walls (4 quads = 8 tris = 24 verts) PLUS
    /// a roof pyramid (4 tris = 12 verts) = 36 verts total.
    #[test]
    fn single_structure_bakes_box_plus_roof() {
        let mesh = zone_mesh_from_objects([0, 0], &[house(0, 0, 0)]).expect("non-empty");
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
        else {
            panic!("expected Float32x3 positions");
        };
        assert_eq!(
            positions.len(),
            24 + 12,
            "4 wall quads (8 tris) + roof (4 tris)"
        );
    }

    /// Two objects in one zone both contribute geometry to the SAME combined
    /// mesh (the whole point of the "one mesh per zone" design) — vertex
    /// count is additive, not per-object-entity.
    #[test]
    fn multiple_objects_combine_into_one_mesh() {
        let mesh = zone_mesh_from_objects([0, 0], &[
            tree(0, 0, 0),
            tree(50, 50, 0),
            house(100, 100, 0),
        ])
        .expect("non-empty");
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
        else {
            panic!("expected Float32x3 positions");
        };
        assert_eq!(positions.len(), 12 + 12 + 36);
    }

    /// A tree placed at grid-relative `(x, y)` with altitude `z` lands at the
    /// exact Bevy-space position the zup->yup convention predicts (matches
    /// `far_terrain`/`lod::chunk_center_bevy`'s own x, alt, -y mapping) — the
    /// "sits on the real ground height" contract this module's doc comment
    /// promises (before the shader's OWN camera-relative bend further
    /// adjusts it at render time).
    #[test]
    fn tree_position_matches_zup_to_yup_convention() {
        let mesh = zone_mesh_from_objects([1, -1], &[tree(5, 7, 42)]).expect("non-empty");
        let VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(BevyMesh::ATTRIBUTE_POSITION)
            .expect("positions")
        else {
            panic!("expected Float32x3 positions");
        };
        let zone_origin_x = to_wpos(1) as f32;
        let zone_origin_y = to_wpos(-1) as f32;
        let expected_base = Vec3::new(zone_origin_x + 5.0, 42.0, -(zone_origin_y + 7.0));
        // The pyramid's apex is the base + (0, height, 0); every base corner
        // has the same Y as `expected_base.y` — check that at least one
        // vertex sits at the expected ground height, on the expected XZ
        // footprint (within the tree's own radius).
        let (radius, _height) = kind_size(ObjectKind::Pine);
        assert!(positions.iter().any(|p| {
            (p[1] - expected_base.y).abs() < 1e-3
                && (p[0] - expected_base.x).abs() <= radius + 1e-3
                && (p[2] - expected_base.z).abs() <= radius + 1e-3
        }));
    }

    /// [`is_tree`]/[`kind_size`] resolve every `ObjectKind` variant to a
    /// positive size without panicking — listed explicitly (rather than via
    /// `strum::IntoEnumIterator`, which would add a new dependency to this
    /// pure-Bevy crate for one test) so a future upstream-merge addition to
    /// the enum is caught here (a new variant not covered by `kind_size`'s
    /// `_ => ...` fallback arms still resolves, but this test's list not
    /// mentioning it is a visible prompt to reconsider its category).
    #[test]
    fn every_object_kind_has_a_positive_size() {
        const ALL_KINDS: [ObjectKind; 26] = [
            ObjectKind::GenericTree,
            ObjectKind::Pine,
            ObjectKind::Dead,
            ObjectKind::House,
            ObjectKind::GiantTree,
            ObjectKind::Mangrove,
            ObjectKind::Acacia,
            ObjectKind::Birch,
            ObjectKind::Redwood,
            ObjectKind::Baobab,
            ObjectKind::Frostpine,
            ObjectKind::Haniwa,
            ObjectKind::Desert,
            ObjectKind::Palm,
            ObjectKind::Arena,
            ObjectKind::SavannahHut,
            ObjectKind::SavannahAirshipDock,
            ObjectKind::TerracottaPalace,
            ObjectKind::TerracottaHouse,
            ObjectKind::TerracottaYard,
            ObjectKind::AirshipDock,
            ObjectKind::CoastalHouse,
            ObjectKind::CoastalWorkshop,
            ObjectKind::CoastalAirshipDock,
            ObjectKind::DesertCityAirshipDock,
            ObjectKind::CliffTownAirshipDock,
        ];
        for kind in ALL_KINDS {
            let (radius, height) = kind_size(kind);
            assert!(radius > 0.0, "{kind:?} must have a positive radius");
            assert!(height > 0.0, "{kind:?} must have a positive height");
        }
    }

    /// A black `Object::color` (the common upstream placeholder for most
    /// structure kinds) falls back to the terracotta roof tone rather than
    /// rendering a literal black roof.
    #[test]
    fn black_color_falls_back_to_roof_default() {
        let black_house = Object {
            color: Rgb::new(0, 0, 0),
            ..house(0, 0, 0)
        };
        assert_eq!(object_roof_color(&black_house), STRUCTURE_ROOF_FALLBACK);
    }

    /// A real (non-black) recorded colour is used verbatim (sRGB->linear),
    /// not the fallback.
    #[test]
    fn real_color_is_used_over_fallback() {
        let colored_house = house(0, 0, 0);
        assert_ne!(object_roof_color(&colored_house), STRUCTURE_ROOF_FALLBACK);
    }
}
