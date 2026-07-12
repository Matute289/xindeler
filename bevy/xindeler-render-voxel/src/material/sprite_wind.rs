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

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Same append-don't-replace argument as `VoxelMaterialExt::specialize`
        // (material/mod.rs docs): this also runs for prepass/shadow, which
        // ignore the extra attribute.
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
