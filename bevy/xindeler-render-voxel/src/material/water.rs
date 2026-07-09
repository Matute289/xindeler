//! EM-3.9b — [`WaterMaterialExt`]: the animated water surface material,
//! `ExtendedMaterial<StandardMaterial, WaterMaterialExt>`.
//!
//! Replaces EM-3.9's interim stock transparent `StandardMaterial` for fluid
//! chunk meshes ([`crate::pipeline::FluidChunkMesh`]) with a scroll/ripple
//! shader that reads the EM-3.2/3.9
//! [`crate::convert::ATTRIBUTE_RIVER_VELOCITY`] vertex attribute the fluid
//! converter already uploads. No normal map, no extra texture — v1 keeps it a
//! pure procedural (sine-ripple + time-scrolled UV + fresnel-ish edge sheen)
//! effect so it never needs a new binary asset (isolation law: `assets/**`
//! stays frozen/read-only for this task).
//!
//! ## Why an extension (mirrors [`crate::material::VoxelMaterialExt`])
//! `StandardMaterial` alone has no vertex-stage hook to read a custom
//! attribute or scroll UVs, so the water surface needs the SAME
//! `ExtendedMaterial<StandardMaterial, X>` + custom WGSL vertex/fragment
//! pattern the terrain material already established. The extension carries a
//! single tunable — [`WaterMaterialExt::ripple_strength`] — the shimmer/
//! fresnel intensity; the other tuning constants (scroll speed/direction,
//! ripple frequency) are cheap-to-eyeball VISUAL constants, not gameplay
//! balance, so they stay `const` in `water.wgsl` (same spirit as
//! `voxel.wgsl`'s tangent-basis table) — only the "how strong" knob is
//! exposed for a future graphics-quality setting.
//!
//! ## Time source
//! `globals.time` (`bevy_pbr::mesh_view_bindings::globals`, `bevy_render`'s
//! per-frame uniform at group 0 binding 11) — no extra Xindeler-side uniform
//! needed; every material can read it for free.
//!
//! ## Vertex attribute (verified against bevy_pbr-0.19.0, same argument as
//! `VoxelMaterialExt`'s doc comment)
//! `specialize` appends [`crate::convert::ATTRIBUTE_RIVER_VELOCITY`] at
//! shader location 8 — free on the fluid mesh's buffer layout (0-2 standard;
//! fluid meshes carry no skinning/AO/layer attributes) and on the
//! prepass/shadow pipelines too (they ignore the extra attribute — this
//! extension only supplies main-pass shaders, so StandardMaterial's default
//! prepass/shadow shaders keep working unmodified).

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

use crate::convert::ATTRIBUTE_RIVER_VELOCITY;

/// The full water material type, as stored in `Assets` / `MeshMaterial3d`.
pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterMaterialExt>;

/// Shader location for the river-velocity attribute (see module docs).
const RIVER_VELOCITY_SHADER_LOCATION: u32 = 8;

/// Extension driving the animated water shader. `base` (the wrapped
/// `StandardMaterial`) still carries the palette-derived base color/alpha/
/// roughness (`palette_material.rs`) — this extension only adds the
/// time-driven scroll/ripple + a strength knob.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct WaterMaterialExt {
    /// Shimmer + fresnel-edge intensity, roughly `[0, ~2]` (`1.0` = the tuned
    /// default). `0.0` still scrolls (the ambient/flow UV drift is
    /// unconditional) but drops the ripple brightness modulation and edge
    /// sheen — a future "low" graphics tier can dial it down without losing
    /// the animation entirely.
    #[uniform(100)]
    pub ripple_strength: f32,
}

impl Default for WaterMaterialExt {
    fn default() -> Self {
        Self {
            ripple_strength: 1.0,
        }
    }
}

impl MaterialExtension for WaterMaterialExt {
    fn vertex_shader() -> ShaderRef {
        // Registered by `WaterMaterialPlugin` via `embedded_asset!` (the
        // crate `src/` prefix is trimmed by the embedded source).
        "embedded://xindeler_render_voxel/material/water.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://xindeler_render_voxel/material/water.wgsl".into()
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
        let extra = layout.0.get_layout(&[
            ATTRIBUTE_RIVER_VELOCITY.at_shader_location(RIVER_VELOCITY_SHADER_LOCATION)
        ])?;
        if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
            debug_assert_eq!(buffer.array_stride, extra.array_stride);
            buffer.attributes.extend(extra.attributes);
        }
        Ok(())
    }
}

/// Registers the embedded WGSL + the `MaterialPlugin` for [`WaterMaterial`].
/// Added by [`crate::material::VoxelMaterialPlugin`] (terrain + water
/// materials are registered together — both feed the same async chunk
/// pipeline).
pub(crate) struct WaterMaterialPlugin;

impl Plugin for WaterMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "water.wgsl");
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default());
    }
}
