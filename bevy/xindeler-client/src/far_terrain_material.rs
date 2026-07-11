//! BL-82 EM-3.11 Phase B — `FarTerrainExtension`: the far-terrain material's
//! world-curvature vertex bend + horizon-occlusion/sky-blend fragment shader.
//!
//! `far_terrain.rs`'s mesh was plain, fully-opaque `StandardMaterial` (Phase
//! A): real per-cell vertex colour, but the mesh's hard top *edge* against
//! the sky was masked ONLY by `DistanceFog` (`crate::atmosphere`), which had
//! to be cranked (round 11) to fully hide it — over-hazing the near/mid
//! field to compensate for a structural gap, not a tuning problem. `docs/
//! design/specs/2026-07-11-bl82-full-horizon-lod-terrain-design.md` §3.4
//! designed the fix as `ExtendedMaterial<StandardMaterial,
//! FarTerrainExtension>` (Path 1: keep the CPU-baked mesh, add a custom
//! material) and `tasks/50-...-tasks.md` T49.6 resolved the "Option C"
//! worksheet fork to a SYNTHESIS with Matías's proposed world-curvature bend
//! (the "rolling-log" trick — Animal Crossing / Distant-Horizons "earth
//! curve ratio" / Bevy discussion #10062): Option A's fragment dissolve
//! removes the hard *edge*; Option C's vertex bend makes the plateau
//! visually *recede* AND (T49.7) lets `fog_density` drop back down, since
//! the mesh no longer needs fog to do all the masking.
//!
//! ## Vertex stage — the Option-C bend
//! Every vertex's world-space Y is displaced downward by `bend_strength *
//! max(d - bend_start, 0.0)²`, where `d` is the HORIZONTAL (XZ) distance
//! from the LIVE camera (`view.world_position.xz`, read straight off Bevy's
//! per-frame view uniform — no extra Xindeler-side "camera position"
//! uniform needed) to the vertex. `bend_start` is set to the far mesh's
//! `hole_radius` (`far_terrain::retile_far_mesh`), so the `max(.., 0.0)`
//! clamp makes the drop EXACTLY zero for every vertex at or inside that
//! radius — the bent far mesh stays flush with the unbent near-voxel
//! terrain at the seam, no crack (verified by
//! `bend_is_zero_at_and_inside_bend_start` below). Because the distance is
//! measured from the LIVE camera every frame (not baked into the mesh at
//! tile time), the bend follows the camera with NO mesh rebuild — it is
//! computed once per vertex per frame, a few cheap ALU ops, no extra
//! geometry.
//!
//! ## Fragment stage
//! (a) A soft sun-occlusion factor from the [`ATTRIBUTE_FAR_HORIZON`]
//! per-vertex record (BL-82 EM-3.11 Phase B's `lod_horizon` sample, T49.5) —
//! a CHEAP analytic proxy for the old engine's `horizon_at2`
//! (`xindeler-old/assets/voxygen/shaders/include/lod.glsl` 165–251)
//! angle/height compare, NOT a literal port (that function ray-marches a
//! precise altitude/horizon intersection the old engine's texture-sampled
//! bicubic terrain could afford; this mesh is coarse and flat-shaded, so a
//! smoothstep over an "occluder elevation angle" derived the same way — pick
//! one of the two packed occluder records by the sun's east/west sign,
//! scale its stored angle by its stored occluder-height — reads as a
//! plausible soft self-shadow at a fraction of the cost). (b) blends the
//! shaded colour toward the atmosphere's live fog/sky colour by
//! camera-relative distance AND by how far the vertex bend has sunk this
//! fragment below eye level, so the silhouette dissolves into haze instead
//! of presenting a hard top line — composes with (does not replace) the
//! engine's own `DistanceFog`, applied afterward by
//! `main_pass_post_lighting_processing` exactly as `WaterMaterialExt` does
//! (`xindeler-render-voxel/src/material/water.rs`, the template this file
//! follows for the "reconstruct `forward_io::VertexOutput`, call the stock
//! PBR fragment helpers" pattern).
//!
//! ## `bend_strength = 0.0` is a first-class disable
//! With `bend_strength` at zero every vertex's `drop` is zero regardless of
//! distance, so the mesh renders exactly as flat Phase-A geometry — the
//! fragment dissolve (horizon occlusion + sky-blend) still applies. This is
//! the escape hatch the design note requires for flat-biome / high-altitude
//! vantages where curvature could look wrong, without a code change (see
//! [`FAR_MESH_BEND_STRENGTH`]'s doc comment for the shipped default and the
//! reasoning behind it).

use bevy::{
    asset::embedded_asset,
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef, VertexFormat},
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin, StandardMaterial,
    },
    prelude::*,
    reflect::Reflect,
    render::render_resource::{AsBindGroup, SpecializedMeshPipelineError},
    shader::ShaderRef,
};
use xindeler_oracle_host::AtmosphereController;

use crate::{far_terrain::FarTerrainMesh, light::Sun};

/// The full far-terrain material type, as stored in `Assets` /
/// `MeshMaterial3d`.
pub type FarTerrainMaterial = ExtendedMaterial<StandardMaterial, FarTerrainExtension>;

/// Shader location for [`ATTRIBUTE_FAR_HORIZON`] (see module docs + the
/// `VoxelMaterialExt`/`WaterMaterialExt` precedent in
/// `xindeler-render-voxel/src/material/{mod,water}.rs`: 8 is free on a mesh
/// carrying only POSITION(0)/NORMAL(1)/COLOR(5) — no UV, tangent, or
/// skinning attributes — in both the main and prepass/shadow pipelines).
const FAR_HORIZON_SHADER_LOCATION: u32 = 8;

/// Packed west/east `(angle, occluder-height)` horizon-occlusion record
/// (BL-82 EM-3.11 Phase B, `NetFarTerrain::horizon` decoded + normalised to
/// `[0.0, 1.0]` per component), one per far-mesh vertex — index-aligned with
/// [`bevy::mesh::Mesh::ATTRIBUTE_COLOR`] the SAME way `far_mesh_from_heights`
/// already duplicates colour per quad (module docs `far_terrain.rs`).
///
/// Numeric id chosen to sit right after `xindeler-render-voxel::convert`'s
/// `988_540_917..=919` custom-attribute family (grep both crates before
/// reusing a value — `MeshVertexAttribute` ids must be globally unique
/// across the whole app, not just this crate).
pub const ATTRIBUTE_FAR_HORIZON: MeshVertexAttribute =
    MeshVertexAttribute::new("FarHorizon", 988_540_930, VertexFormat::Float32x4);

/// Smoke-tuned default (BL-82 EM-3.11 Phase B) for
/// [`FarTerrainExtension::bend_strength`] — metres of downward displacement
/// per (metre² of camera-relative distance beyond `bend_start`).
///
/// Reasoning: the round-11-lowered fog (T49.7 drops density back toward
/// `0.00667`) is already ≈99% opaque by ~300–350 m past the far mesh's
/// `hole_radius` (≈288 m at the default render distance) — i.e. the bend
/// only needs to visibly matter in roughly a 0–500 m band beyond
/// `bend_start`; anything farther is already hidden by haze regardless of
/// how much it has bent. `5e-5` gives: ~0 m drop at `bend_start` (the seam,
/// by construction), ~2 m by 200 m beyond it (a barely-there dip — the near/
/// mid field the round-11 retune wanted to keep clear stays visually flat),
/// ~12 m by 500 m beyond it (a clear, gentle "rolling away" right where the
/// fragment dissolve + fog are already taking over) — imperceptible AS
/// curvature (no fisheye/globe look) at normal eye height, reading only as
/// "the ground recedes out there" exactly per the design note's ask.
/// `0.0` is a valid, fully-supported disable (module docs).
///
/// ## Data-driven-content cleanup (comprehensive-review Finding 2)
/// This constant is no longer the LIVE source of truth — `far_terrain::
/// retile_far_mesh` now reads the actual bend strength (and the sibling
/// `bend_start` scale) from `xindeler_oracle_host::AtmosphereController`'s
/// live `AtmosphereProfile` (`far_mesh_bend_strength`/
/// `far_mesh_bend_start_scale`), backed by `assets/xindeler/atmosphere/
/// default.atmo.ron` — hot-reloadable, and overridable per-biome/vantage via
/// a DmEvent atmosphere override, exactly like every sibling atmosphere-
/// tuning parameter. This constant remains: (a) the compiled-in fallback
/// `retile_far_mesh` uses when no `AtmosphereController` exists (e.g. a bare
/// test app, via `FarTerrainExtension::default`), and (b) the value the
/// shipped `default.atmo.ron` MUST equal so the first applied profile is a
/// behavior-preserving no-op — pinned by this module's own
/// `atmosphere_default_bend_strength_matches_the_compiled_in_fallback` test.
pub const FAR_MESH_BEND_STRENGTH: f32 = 0.00005;

/// Extension driving the far-terrain vertex bend + horizon/sky-blend
/// fragment shader (module docs).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct FarTerrainExtension {
    /// Metres of downward vertex displacement per metre² of camera-relative
    /// distance beyond [`Self::bend_start`]. `0.0` disables the bend
    /// entirely (module docs) — see [`FAR_MESH_BEND_STRENGTH`] for the
    /// shipped default + reasoning.
    #[uniform(100)]
    pub bend_strength: f32,
    /// Radial distance (world metres, camera-relative) at which the bend
    /// begins ramping — set to the far mesh's `hole_radius`, scaled by the
    /// live `AtmosphereProfile::far_mesh_bend_start_scale` (`>= 1.0`), so the
    /// bend is EXACTLY zero across the whole near band (no seam against the
    /// near, block-accurate terrain) for any scale in that range.
    #[uniform(101)]
    pub bend_start: f32,
    /// Live "direction from a fragment toward the sun" (world space, xyz
    /// used; `w` unused/padding) — mirrors bevy_pbr's own
    /// `dir_to_light: light.transform.back()` convention
    /// (`bevy_pbr::render::light`), so [`sync_far_terrain_material`] can
    /// read it straight off the [`Sun`] entity's `Transform` with no sign
    /// flip.
    #[uniform(102)]
    pub sun_direction: Vec4,
    /// Live atmosphere fog colour (sRGB) — one of the two "dissolve into
    /// haze" targets the fragment shader blends toward at distance (the
    /// other is [`Self::sky_color`]; see the module docs' fragment-stage
    /// section).
    #[uniform(103)]
    pub fog_color: Vec4,
    /// Live atmosphere sky/void colour (sRGB) — the target blended toward
    /// once a fragment has sunk well below eye level (the bend's own
    /// "rolled past the horizon" signal), so the silhouette against the sky
    /// reads as continuous rather than a hard top edge.
    #[uniform(104)]
    pub sky_color: Vec4,
}

impl Default for FarTerrainExtension {
    fn default() -> Self {
        Self {
            bend_strength: FAR_MESH_BEND_STRENGTH,
            bend_start: 0.0,
            sun_direction: Vec4::new(0.0, 1.0, 0.0, 0.0),
            fog_color: Vec4::new(0.66, 0.73, 0.81, 1.0),
            sky_color: Vec4::new(0.168_627, 0.172_549, 0.184_314, 1.0),
        }
    }
}

impl MaterialExtension for FarTerrainExtension {
    fn vertex_shader() -> ShaderRef {
        // Registered by `FarTerrainMaterialPlugin` via `embedded_asset!` (the
        // crate `src/` prefix is trimmed by the embedded source).
        "embedded://xindeler_client/far_terrain_material.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://xindeler_client/far_terrain_material.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Same append-don't-replace argument as `VoxelMaterialExt`/
        // `WaterMaterialExt::specialize` (their doc comments): this also runs
        // for prepass/shadow pipelines, which ignore the extra attribute —
        // never replace `buffers` wholesale here.
        let extra = layout
            .0
            .get_layout(&[ATTRIBUTE_FAR_HORIZON.at_shader_location(FAR_HORIZON_SHADER_LOCATION)])?;
        if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
            debug_assert_eq!(buffer.array_stride, extra.array_stride);
            buffer.attributes.extend(extra.attributes);
        }
        Ok(())
    }
}

/// Registers the embedded WGSL + the [`MaterialPlugin`] for
/// [`FarTerrainMaterial`], plus [`sync_far_terrain_material`] (keeps the
/// LIVE sun direction / atmosphere colours flowing into whichever far-mesh
/// material asset is currently spawned, every frame — cheap: at most one far
/// mesh entity exists at a time, see `far_terrain::FarMeshState`). Added by
/// [`crate::far_terrain::FarTerrainPlugin`].
pub(crate) struct FarTerrainMaterialPlugin;

impl Plugin for FarTerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "far_terrain_material.wgsl");
        app.add_plugins(MaterialPlugin::<FarTerrainMaterial>::default())
            .add_systems(Update, sync_far_terrain_material);
    }
}

/// Every frame, pushes the LIVE sun direction + atmosphere fog/sky colours
/// into whichever [`FarTerrainMaterial`] asset the currently-spawned far
/// mesh entity uses (`bend_strength`/`bend_start` are set once at material
/// creation in `far_terrain::retile_far_mesh` and left alone here — they
/// only change on a rare re-tile, not every frame).
///
/// Falls back to [`AtmosphereProfile::default`]'s colours (via
/// `FarTerrainExtension::default`, never mutated) when no
/// [`AtmosphereController`] exists (e.g. a test app that doesn't wire
/// `AtmospherePlugin`) and to a straight-up sun direction when no [`Sun`]
/// entity exists — both honestly-degraded fallbacks, not panics, matching
/// this module's "never crash a test/tool app that skipped a plugin"
/// convention (see `far_terrain.rs::retile_far_mesh`'s `haze` fallback).
fn sync_far_terrain_material(
    far_meshes: Query<&MeshMaterial3d<FarTerrainMaterial>, With<FarTerrainMesh>>,
    mut materials: ResMut<Assets<FarTerrainMaterial>>,
    suns: Query<&Transform, With<Sun>>,
    atmosphere: Option<Res<AtmosphereController>>,
) {
    if far_meshes.is_empty() {
        return;
    }
    // bevy_pbr's own convention (`render/light.rs`: `dir_to_light:
    // light.transform.back()`) — no sign flip needed at the call site.
    let sun_direction = suns
        .iter()
        .next()
        .map_or(Vec3::Y, |transform| transform.back().as_vec3());
    let (fog_color, sky_color) = atmosphere.map_or_else(
        || {
            let defaults = FarTerrainExtension::default();
            (defaults.fog_color, defaults.sky_color)
        },
        |a| {
            (
                Vec3::from_array(a.current.fog_color).extend(1.0),
                Vec3::from_array(a.current.sky_color).extend(1.0),
            )
        },
    );

    for handle in &far_meshes {
        if let Some(mut material) = materials.get_mut(&handle.0) {
            material.extension.sun_direction = sun_direction.extend(0.0);
            material.extension.fog_color = fog_color;
            material.extension.sky_color = sky_color;
        }
    }
}

#[cfg(test)]
mod tests {
    /// The Option-C bend formula (module docs): zero at/inside `bend_start`,
    /// then grows as `bend_strength * (d - bend_start)²` — pinned directly so
    /// a future edit can't silently reintroduce a seam (drop != 0 right at
    /// the boundary) or invert the curve's growth direction. Mirrors the
    /// WGSL vertex shader's `max(d - bend_start, 0.0)` clamp + square exactly
    /// (kept in Rust so it's unit-testable without a renderer — the WGSL
    /// itself is exercised by the `XINDELER_SMOKE_FAR_MESH_CAM=1` visual
    /// smoke, not by this test).
    fn bend_drop(bend_strength: f32, bend_start: f32, d: f32) -> f32 {
        let beyond = (d - bend_start).max(0.0);
        bend_strength * beyond * beyond
    }

    #[test]
    fn bend_is_zero_at_and_inside_bend_start() {
        assert_eq!(
            bend_drop(0.00005, 288.0, 288.0),
            0.0,
            "zero exactly AT bend_start"
        );
        assert_eq!(
            bend_drop(0.00005, 288.0, 100.0),
            0.0,
            "zero (clamped, not negative) INSIDE bend_start — the near band"
        );
        assert_eq!(bend_drop(0.00005, 288.0, 0.0), 0.0);
    }

    #[test]
    fn bend_grows_monotonically_beyond_bend_start() {
        let bend_strength = 0.00005;
        let bend_start = 288.0;
        let d200 = bend_drop(bend_strength, bend_start, bend_start + 200.0);
        let d500 = bend_drop(bend_strength, bend_start, bend_start + 500.0);
        let d1000 = bend_drop(bend_strength, bend_start, bend_start + 1000.0);
        assert!(
            d200 > 0.0 && d200 < d500 && d500 < d1000,
            "strictly increasing beyond bend_start"
        );
        // Matches the doc-comment reasoning on FAR_MESH_BEND_STRENGTH.
        assert!((d200 - 2.0).abs() < 0.1, "got {d200}");
        assert!((d500 - 12.5).abs() < 0.1, "got {d500}");
    }

    #[test]
    fn bend_strength_zero_disables_the_bend_at_any_distance() {
        for d in [0.0, 288.0, 500.0, 5_000.0, 50_000.0] {
            assert_eq!(
                bend_drop(0.0, 288.0, d),
                0.0,
                "bend_strength=0.0 must be a clean disable at every distance"
            );
        }
    }

    /// Data-driven-content cleanup (comprehensive-review Finding 2): the
    /// compiled-in [`super::FAR_MESH_BEND_STRENGTH`] fallback and the shipped
    /// `default.atmo.ron`'s `far_mesh_bend_strength` (via
    /// `AtmosphereProfile::default()`, which `atmosphere.rs`'s own test pins
    /// to the shipped RON file) MUST agree — see [`super::
    /// FAR_MESH_BEND_STRENGTH`]'s doc comment for why both need to exist and
    /// stay in sync (a headless crate can't reference this render-crate
    /// constant directly, so the two copies are hand-kept-equal; this test
    /// is what actually enforces it).
    #[test]
    fn atmosphere_default_bend_strength_matches_the_compiled_in_fallback() {
        use xindeler_oracle_host::AtmosphereProfile;

        assert!(
            (AtmosphereProfile::default().far_mesh_bend_strength - super::FAR_MESH_BEND_STRENGTH)
                .abs()
                < f32::EPSILON,
            "AtmosphereProfile::default().far_mesh_bend_strength must equal the compiled-in \
             FAR_MESH_BEND_STRENGTH fallback so the first applied profile is a visual no-op"
        );
    }
}
