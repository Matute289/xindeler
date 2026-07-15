//! EM-3.9c — [`SpriteWindMaterialExt`]: the wind-sway sprite material,
//! `ExtendedMaterial<StandardMaterial, SpriteWindMaterialExt>`.
//!
//! Replaces EM-3.9/3.9b's stock vertex-coloured `StandardMaterial` for block
//! sprites (`xindeler-client::sprite_view::sprite_material`) with a shader
//! that sways swayable sprites in the wind by ROTATING both position and
//! normal together (normal-consistent by construction) — see
//! `sprite_wind.wgsl`'s module doc comment for the full design writeup and
//! why the EM-3.9b attempt (position-only displacement) broke lighting.
//!
//! ## Why an extension (mirrors [`crate::material::VoxelMaterialExt`]/
//! [`crate::material::WaterMaterialExt`])
//! `StandardMaterial` has no vertex-stage hook to read the sprite's own
//! per-vertex sway weight or rotate position+normal together — the same
//! `ExtendedMaterial<StandardMaterial, X>` + custom WGSL pattern the terrain
//! and water materials already established.
//!
//! ## The one tunable
//! [`SpriteWindMaterialExt::wind_strength`] is a single `[0, ~2]` knob (`1.0`
//! = tuned default, `0.0` disables sway entirely at zero extra vertex-shader
//! cost beyond the branch's own comparison) — a future graphics-quality tier
//! or the `XINDELER_SPRITE_WIND` env var (see `xindeler-client::sprite_view`)
//! can dial it down/off without swapping materials. All the other knobs
//! (angle cap, wind speed/direction) are cheap-to-eyeball VISUAL constants
//! kept as `const`s in the WGSL (same spirit as `water.wgsl`'s ripple
//! frequency table).

use bevy::{
    app::{App, Plugin},
    asset::{Asset, embedded_asset},
    mesh::MeshVertexBufferLayoutRef,
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin, StandardMaterial,
    },
    reflect::Reflect,
    render::render_resource::{AsBindGroup, SpecializedMeshPipelineError},
    shader::ShaderRef,
};

use crate::convert::ATTRIBUTE_SPRITE_SWAY;

/// The full sprite material type, as stored in `Assets` / `MeshMaterial3d`.
pub type SpriteWindMaterial = ExtendedMaterial<StandardMaterial, SpriteWindMaterialExt>;

/// Shader location for the sway-weight attribute (see module + shader docs).
const SPRITE_SWAY_SHADER_LOCATION: u32 = 8;

/// Extension driving the wind-sway shader. `base` (the wrapped
/// `StandardMaterial`) still carries `base_color: WHITE` +
/// `double_sided`/`cull_mode: None` (`sprite_view::sprite_material`'s
/// existing settings, unchanged) so per-voxel vertex colour shows through
/// from both faces of thin grass cards — this extension only adds the
/// time+position-driven sway rotation + a strength knob.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SpriteWindMaterialExt {
    /// Sway strength, roughly `[0, ~2]` (`1.0` = the tuned default). `0.0`
    /// disables the whole rotation branch per-vertex (see shader).
    #[uniform(100)]
    pub wind_strength: f32,
}

impl Default for SpriteWindMaterialExt {
    fn default() -> Self { Self { wind_strength: 1.0 } }
}

impl MaterialExtension for SpriteWindMaterialExt {
    fn vertex_shader() -> ShaderRef {
        // Registered by `SpriteWindMaterialPlugin` via `embedded_asset!` (the
        // crate `src/` prefix is trimmed by the embedded source).
        "embedded://xindeler_render_voxel/material/sprite_wind.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://xindeler_render_voxel/material/sprite_wind.wgsl".into()
    }

    // BL-82 EM-3.11 round 21: shadows OFF for this material. `MaterialExtension`
    // has separate `prepass_vertex_shader`/`deferred_vertex_shader` hooks for the
    // depth-only passes Bevy's shadow maps actually render through
    // (`bevy_pbr::material::queue_shadows` specializes the shadow pipeline from
    // `M::prepass_vertex_shader`, NOT `M::vertex_shader` — verified against
    // bevy_pbr 0.19's own source) — neither is overridden here (unlike
    // `vertex_shader`/`fragment_shader` above), so they silently fall back to
    // `ShaderRef::Default`, i.e. `StandardMaterial`'s stock STATIC prepass
    // vertex shader. That means every swaying sprite's SHADOW is baked from its
    // unswayed rest position every frame, while `sprite_wind.wgsl`'s `vertex()`
    // keeps rotating the VISIBLE mesh via `globals.time` — a permanent,
    // per-frame mismatch between what's on screen and its own shadow, on every
    // grass/flower/mushroom/reed sprite with nonzero baked sway. Confirmed live
    // (offscreen burst-capture A/B, `XINDELER_SPRITE_WIND=0` vs. default):
    // disabling sway cut the biggest per-frame pixel jumps (p99.9) roughly in
    // half on its own, on top of the round-21 sun-throttle fix. A correct fix
    // would give this material its OWN prepass vertex shader applying the same
    // Rodrigues rotation so the shadow tracks the sway — high-risk to
    // hand-author without an extensive live-render pass (motion-vector /
    // `MOTION_VECTOR_PREPASS` correctness feeds TAA reprojection directly; a
    // subtly wrong prepass here risks reintroducing round 6's ghost-hand class
    // of bug). Rigid props (furniture/dungeon décor) already bake sway to
    // `0.0` (`sprite.rs::bake_sway_weights` docs) so they lose nothing real by
    // this — same trade EM-3.11q made disabling `contact_shadows` (drop a
    // confirmed-broken, minor effect rather than risk a shader rewrite for a
    // "detail" gain). Revisit once Bevy exposes (or this project authors) a
    // simpler prepass override path.
    fn enable_shadows() -> bool { false }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Same append-don't-replace argument as `VoxelMaterialExt::specialize`
        // (material/mod.rs docs): this also runs for the prepass (depth/motion
        // vectors), which ignores the extra attribute. Shadows themselves are
        // off for this material (`enable_shadows` above), so this no longer
        // needs to reason about shadow correctness — only prepass depth/motion
        // vectors, which are unaffected by the missing sway attribute (they
        // read the same STATIC position the shadow pass used to, which is
        // correct there: no shadow depends on it anymore).
        let extra = layout
            .0
            .get_layout(&[ATTRIBUTE_SPRITE_SWAY.at_shader_location(SPRITE_SWAY_SHADER_LOCATION)])?;
        if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
            debug_assert_eq!(buffer.array_stride, extra.array_stride);
            buffer.attributes.extend(extra.attributes);
        }
        Ok(())
    }
}

/// Registers the embedded WGSL + the `MaterialPlugin` for
/// [`SpriteWindMaterial`]. Added by [`crate::material::VoxelMaterialPlugin`]
/// (terrain/water/sprite materials are registered together — all three feed
/// client-owned spawning paths that need a shared `Assets<T>` resource to
/// exist before they run).
pub(crate) struct SpriteWindMaterialPlugin;

impl Plugin for SpriteWindMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "sprite_wind.wgsl");
        app.add_plugins(MaterialPlugin::<SpriteWindMaterial>::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BL-82 EM-3.11 round 21 regression: this material's `vertex_shader`
    /// applies a per-frame time-driven sway rotation but does NOT override
    /// `prepass_vertex_shader` (verified in the trait impl above), so Bevy's
    /// shadow-map pass — which specializes from `prepass_vertex_shader`, not
    /// `vertex_shader` — would render every swaying sprite's shadow from its
    /// static rest pose, permanently desynced from the visible swaying mesh.
    /// `enable_shadows` must stay `false` until this material gets its own
    /// sway-aware prepass shader, or the confirmed flicker silently comes
    /// back the next time someone "cleans up" this override.
    #[test]
    fn sprite_wind_material_does_not_cast_shadows() {
        assert!(
            !SpriteWindMaterialExt::enable_shadows(),
            "SpriteWindMaterialExt casts shadows from its STATIC prepass vertex shader while its \
             main pass sways vertices via globals.time — re-enabling shadows without also giving \
             this material a sway-aware prepass_vertex_shader brings back the round-21 vegetation \
             shadow flicker"
        );
    }
}
