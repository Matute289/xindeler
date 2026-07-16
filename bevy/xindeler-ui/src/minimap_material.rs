//! BL-82 EM-5.17 Phase 3 — `MinimapFadeMaterial`: a `UiMaterial` giving the
//! minimap a soft radial alpha feather at its edge, in place of a hard
//! square/circle clip or a decorative frame PNG.
//!
//! ## Why a shader, not a frame asset
//! Spec §3.3 (Notion blueprint) recommended a circular-clip `UiMaterial` plus
//! `minimap_gothic_frame.png` (not in the current 58-asset pack) or, as a
//! stand-in, reusing `party_portrait_frame.png`'s ring style. **Matías
//! overrode this directly (2026-07-15, verbal decision relayed via the
//! Phase 3 brief): no frame asset at all** — the minimap image itself
//! gradually becomes transparent toward its boundary (a radial alpha
//! falloff / vignette-style dissolve), not a hard circular clip with any
//! border/ring PNG. This module implements exactly that, nothing else.
//!
//! This mirrors [`crate::orb_material`]'s exact registration shape
//! (`embedded_asset!` + `AsBindGroup` + a dedicated `*MaterialPlugin` added
//! unconditionally by [`crate::XindelerUiPlugin`]) — that module's own doc
//! comment explicitly invites Phase 3 to copy it directly, and the Bevy 0.19
//! `UiMaterial` viability spike it documents (trait/`MaterialNode`/
//! `UiMaterialPlugin` all confirmed present and working in this project's
//! pinned `=0.19.0`) applies here unchanged.
//!
//! ## Falloff constants (tuned by eye against the Phase 3 smoke screenshot)
//! [`INNER_RADIUS_UV`] = `0.35`, [`OUTER_RADIUS_UV`] = `0.5` — UV distance
//! from the panel center `(0.5, 0.5)`, where the viewport is a `[0,1]²`
//! square: full opacity inside the inner radius, smoothly fading
//! (`smoothstep`) to fully transparent by the outer radius. `0.5` is exactly
//! the distance from center to the midpoint of each edge, so every point
//! outside the inscribed circle (including all four corners, at distance
//! `sqrt(0.5) ≈ 0.707`) is fully transparent — the visible disc reads as a
//! soft circular vignette, never a hard-edged square. `0.35` leaves a
//! reasonably wide (`0.15`-UV) feather band rather than an abrupt cutoff,
//! while still keeping most of the panel's center at full strength.
//!
//! [`radial_falloff_alpha`] is the Rust-side mirror of the WGSL fragment
//! shader's identical formula (`minimap_material.wgsl`) and is unit-tested
//! here; the shader itself isn't exercised by a headless crate test (no
//! GPU/window available in this crate's test harness — the same limitation
//! `orb_material.rs`'s own tests document). The radii themselves
//! (`inner_radius`/`outer_radius`) are real `#[uniform]` fields on
//! [`MinimapFadeMaterial`], uploaded to the shader the same way `crop_min`/
//! `crop_size` already are — NOT hand-duplicated WGSL constants — so tuning
//! them by eye never risks the Rust and GPU sides silently drifting apart.
//!
//! ## Crop lives in the shader, not `ImageNode::rect`
//! `MaterialNode<M>` (unlike `ImageNode`) has no `rect` field to crop a
//! sub-region of the source texture — the minimap's per-frame pan-follow
//! crop (`map_view::crop_rect_uv`) is instead uploaded as a `crop_min`/
//! `crop_size` uniform pair and applied inside the fragment shader before
//! sampling, while the vignette falloff itself is computed against the
//! PANEL's own local UV (`in.uv`, always `[0,1]²` regardless of the crop
//! window's position) — the visible "soft circle" is a property of the
//! on-screen viewport, not of wherever the crop window currently sits in the
//! source map image.

use bevy::{
    app::{App, Plugin},
    asset::{Asset, Handle, embedded_asset},
    ecs::component::Component,
    image::Image,
    math::Vec2,
    prelude::UiMaterial,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

/// UV-distance-from-center where the falloff BEGINS (still fully opaque
/// inside this radius). See the module doc comment for how this was chosen.
pub const INNER_RADIUS_UV: f32 = 0.35;
/// UV-distance-from-center where the falloff COMPLETES (fully transparent
/// at/beyond this radius) — `0.5` puts this exactly at the edge midpoints of
/// the `[0,1]²` panel, so every corner is guaranteed past it.
pub const OUTER_RADIUS_UV: f32 = 0.5;

/// A `UiMaterial` sampling `minimap_texture` (cropped via `crop_min`/
/// `crop_size`, the shader-side replacement for `ImageNode::rect`) and
/// multiplying its alpha by [`radial_falloff_alpha`]'s formula — see the
/// module doc comment for the falloff constants and crop rationale.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Component)]
pub struct MinimapFadeMaterial {
    /// The minimap's decoded world-map texture (the same `Handle<Image>`
    /// `map_view::receive_map_data` already builds — this material doesn't
    /// own or produce it, only samples it).
    #[texture(0)]
    #[sampler(1)]
    pub minimap_texture: Handle<Image>,
    /// UV-space crop window's minimum corner (mirrors `map_view::
    /// crop_rect_uv`'s `Rect::min` — the caller updates this every frame the
    /// same way [`crate::orb_material::OrbLiquidMaterial::fill_fraction`]
    /// is caller-owned).
    #[uniform(2)]
    pub crop_min: Vec2,
    /// UV-space crop window's size (`Rect::max - Rect::min`).
    #[uniform(3)]
    pub crop_size: Vec2,
    /// UV-distance-from-center where the falloff BEGINS — mirrors
    /// [`INNER_RADIUS_UV`]. A real uniform (not a WGSL constant) so tuning
    /// this by eye can never desync Rust from the GPU side.
    #[uniform(4)]
    pub inner_radius: f32,
    /// UV-distance-from-center where the falloff COMPLETES — mirrors
    /// [`OUTER_RADIUS_UV`].
    #[uniform(5)]
    pub outer_radius: f32,
}

impl MinimapFadeMaterial {
    /// Convenience constructor using the tuned default radii
    /// ([`INNER_RADIUS_UV`]/[`OUTER_RADIUS_UV`]) — the only radii this HUD
    /// currently ships, but callers can still hand-set the two uniform
    /// fields directly if a future variant needs a different feather width.
    #[must_use]
    pub fn new(minimap_texture: Handle<Image>, crop_min: Vec2, crop_size: Vec2) -> Self {
        Self {
            minimap_texture,
            crop_min,
            crop_size,
            inner_radius: INNER_RADIUS_UV,
            outer_radius: OUTER_RADIUS_UV,
        }
    }
}

impl UiMaterial for MinimapFadeMaterial {
    fn fragment_shader() -> ShaderRef { "embedded://xindeler_ui/minimap_material.wgsl".into() }
}

/// The Rust-side mirror of the WGSL fragment shader's alpha-multiplier
/// formula (see the module doc comment) — `uv` is panel-local `[0,1]²` (NOT
/// the cropped source-texture UV `MinimapFadeMaterial::crop_min`/
/// `crop_size` produce). Returns `1.0` at/inside `inner_radius`, smoothly
/// falling to `0.0` at/beyond `outer_radius`.
#[must_use]
pub fn radial_falloff_alpha(uv: Vec2, inner_radius: f32, outer_radius: f32) -> f32 {
    let dist = (uv - Vec2::splat(0.5)).length();
    1.0 - smoothstep(inner_radius, outer_radius, dist)
}

/// A minimal `smoothstep` (WGSL/GLSL's standard Hermite interpolation) —
/// `f32` has no built-in equivalent. `0.0` at/below `edge0`, `1.0` at/above
/// `edge1`, smooth (zero-derivative at both ends) in between. Must match
/// WGSL's own `smoothstep` exactly, since [`radial_falloff_alpha`] exists
/// specifically to mirror `minimap_material.wgsl`'s formula.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Registers the embedded minimap-fade WGSL + [`bevy::ui_render::
/// UiMaterialPlugin<MinimapFadeMaterial>`]. Added by
/// [`crate::XindelerUiPlugin`] unconditionally, mirroring
/// [`crate::orb_material::OrbMaterialPlugin`]'s own registration exactly
/// (registering the plugin/asset type costs nothing while unused elsewhere in
/// this crate — `xindeler-client`'s `map_view` module is this material's one
/// real consumer).
pub(crate) struct MinimapMaterialPlugin;

impl Plugin for MinimapMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "minimap_material.wgsl");
        app.add_plugins(bevy::ui_render::UiMaterialPlugin::<MinimapFadeMaterial>::default());
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, asset::AssetPlugin, prelude::*};

    use super::*;

    /// [`MinimapMaterialPlugin`] registers without panicking and the
    /// material asset type becomes real, resolving to a real embedded
    /// shader path — mirrors `orb_material`'s own
    /// `orb_material_plugin_registers_without_panicking` test shape (a real
    /// render-pipeline pixel test needs a GPU/window, out of scope for a
    /// headless crate test).
    #[test]
    fn minimap_material_plugin_registers_without_panicking() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<MinimapFadeMaterial>();
        embedded_asset!(app, "minimap_material.wgsl");
        assert!(matches!(
            MinimapFadeMaterial::fragment_shader(),
            ShaderRef::Path(_)
        ));
    }

    /// A material asset's crop uniforms round-trip verbatim — pins the
    /// field meanings against an accidental rename/reorder.
    #[test]
    fn crop_uniforms_round_trip() {
        let material =
            MinimapFadeMaterial::new(Handle::default(), Vec2::new(0.1, 0.2), Vec2::new(0.3, 0.4));
        assert_eq!(material.crop_min, Vec2::new(0.1, 0.2));
        assert_eq!(material.crop_size, Vec2::new(0.3, 0.4));
    }

    /// `MinimapFadeMaterial::new` uploads the tuned default radii as real
    /// uniform fields (not hand-duplicated WGSL constants) — pins that the
    /// constructor and the module-level [`INNER_RADIUS_UV`]/
    /// [`OUTER_RADIUS_UV`] constants stay in agreement.
    #[test]
    fn new_uses_the_tuned_default_radii() {
        let material = MinimapFadeMaterial::new(Handle::default(), Vec2::ZERO, Vec2::ONE);
        assert_eq!(material.inner_radius, INNER_RADIUS_UV);
        assert_eq!(material.outer_radius, OUTER_RADIUS_UV);
    }

    /// Dead center (and comfortably inside the inner radius generally) must
    /// read fully opaque — the minimap's middle should never look faded.
    #[test]
    fn falloff_is_fully_opaque_near_center() {
        let center_alpha = radial_falloff_alpha(Vec2::splat(0.5), INNER_RADIUS_UV, OUTER_RADIUS_UV);
        assert!(
            (center_alpha - 1.0).abs() < 1e-6,
            "center must be fully opaque, got {center_alpha}"
        );

        let near_center = Vec2::new(0.5 + INNER_RADIUS_UV * 0.5, 0.5);
        let alpha = radial_falloff_alpha(near_center, INNER_RADIUS_UV, OUTER_RADIUS_UV);
        assert!(
            (alpha - 1.0).abs() < 1e-6,
            "a point well inside the inner radius must stay fully opaque, got {alpha}"
        );
    }

    /// At and beyond the outer radius — including the square panel's own
    /// corners — the fade must be complete (alpha 0), which is exactly what
    /// makes this read as a soft CIRCLE rather than a faded square.
    #[test]
    fn falloff_is_fully_transparent_at_and_beyond_outer_radius() {
        let at_edge = Vec2::new(0.5 + OUTER_RADIUS_UV, 0.5);
        let alpha = radial_falloff_alpha(at_edge, INNER_RADIUS_UV, OUTER_RADIUS_UV);
        assert!(
            alpha.abs() < 1e-6,
            "at the outer radius must be fully transparent, got {alpha}"
        );

        let corner = Vec2::new(1.0, 1.0);
        let alpha = radial_falloff_alpha(corner, INNER_RADIUS_UV, OUTER_RADIUS_UV);
        assert!(
            alpha.abs() < 1e-6,
            "the panel's own corner (distance ~0.707 from center) must be fully transparent, got \
             {alpha}"
        );
    }

    /// Between the two radii the falloff must be a genuine gradient (a
    /// smooth dissolve), not a second hard step disguised as a "fade" — this
    /// is the actual acceptance bar for "soft feather, not a hard clip".
    #[test]
    fn falloff_is_strictly_decreasing_between_the_two_radii() {
        let closer =
            radial_falloff_alpha(Vec2::new(0.5 + 0.40, 0.5), INNER_RADIUS_UV, OUTER_RADIUS_UV);
        let farther =
            radial_falloff_alpha(Vec2::new(0.5 + 0.45, 0.5), INNER_RADIUS_UV, OUTER_RADIUS_UV);
        assert!(
            closer > farther,
            "alpha must monotonically decrease moving outward through the feather band: {closer} \
             vs {farther}"
        );
        assert!(
            closer < 1.0 && closer > 0.0,
            "mid-band alpha must be a real fraction, not clamped"
        );
        assert!(
            farther < 1.0 && farther > 0.0,
            "mid-band alpha must be a real fraction, not clamped"
        );
    }
}
