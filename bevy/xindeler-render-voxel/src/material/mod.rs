//! EM-3.3 — [`VoxelMaterialExt`]: the terrain block material,
//! `ExtendedMaterial<StandardMaterial, VoxelMaterialExt>`.
//!
//! Lives HERE (render-voxel, `material` feature) rather than in the client:
//! the async chunk pipeline (EM-3.5) and listen-server terrain path (EM-3.6)
//! both consume it, and it is inseparable from the EM-3.2 vertex attributes
//! it reads. The client only builds the texture arrays (data) and spawns
//! entities.
//!
//! ## Why an extension (spec §4.2)
//! `StandardMaterial` cannot bind `texture_2d_array` (bevy #20134), so the
//! extension binds THREE texture arrays — albedo, normal, MRA — indexed per
//! fragment by the flat [`ATTRIBUTE_BLOCK_LAYER`] vertex attribute (one layer
//! per block type ⇒ no atlas bleeding, greedy-compatible via the converter's
//! world-space planar UVs). MRA channel convention (documented, kept until
//! EM-3.4's block palette formalises it): `R = metallic`, `G = perceptual
//! roughness`, `B = texture AO`, `A = emissive mask` (× albedo ×
//! [`VoxelMaterialExt::emissive_strength`] — lava/crystal glow).
//!
//! ## Custom vertex path (verified against bevy_pbr-0.19.0 source)
//! The extension supplies BOTH main-pass shaders (`vertex` + `fragment` in
//! `voxel.wgsl`). `MaterialExtension::specialize` APPENDS the two custom
//! attributes to the vertex buffer layout at shader locations 8/9 — free in
//! both the main mesh pipeline (0–5 standard, 6/7 skinning;
//! `bevy_pbr::render::mesh.rs::specialize`) and the prepass pipeline (0–7;
//! `bevy_pbr::prepass::mod.rs`). Appending (instead of replacing
//! `descriptor.vertex.buffers`) matters because `Material::specialize` — and
//! therefore the extension's — ALSO runs for the prepass/shadow pipelines
//! (`PrepassPipelineSpecializer` calls `user_specialize`): those keep their
//! default vertex shaders, and wgpu permits buffer attributes a shader does
//! not consume, so depth/normal/motion prepasses (TAA/SSAO inputs) stay
//! byte-identical to StandardMaterial's.
//!
//! ## AO insertion point (spec §4.3, verified in bevy_pbr-0.19.0 WGSL)
//! `PbrInput::diffuse_occlusion` / `specular_occlusion` are multiplied by
//! `pbr_functions::apply_pbr_lighting` into `indirect_light` ONLY (irradiance
//! volumes, environment light, `ambient::ambient_light`) — direct
//! sun/point/spot light is untouched, so shadow-mapped light stays physical.
//! The fragment multiplies the interpolated vertex AO (× the MRA texture AO
//! channel) into both, composing with SSAO (bevy takes `min(texture AO,
//! ssao)` upstream of our multiply).

use bevy::{
    app::{App, Plugin},
    asset::{Asset, Handle, embedded_asset},
    image::Image,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin, StandardMaterial,
    },
    reflect::Reflect,
    render::render_resource::{AsBindGroup, SpecializedMeshPipelineError},
    shader::ShaderRef,
};

use crate::convert::{ATTRIBUTE_BLOCK_LAYER, ATTRIBUTE_VOXEL_AO};

/// The full terrain material type, as stored in `Assets` / `MeshMaterial3d`.
pub type VoxelMaterial = ExtendedMaterial<StandardMaterial, VoxelMaterialExt>;

/// Shader locations for the custom attributes (see module docs for why 8/9).
const VOXEL_AO_SHADER_LOCATION: u32 = 8;
const BLOCK_LAYER_SHADER_LOCATION: u32 = 9;

/// Extension binding the block texture arrays + nearest samplers.
///
/// Bindings start at 100 (slots 0–99 belong to `StandardMaterial`, same
/// convention as bevy's `extended_material` example). No `#[bindless]`: the
/// extension being non-bindless forces the whole `ExtendedMaterial` down the
/// non-bindless path (`ExtendedMaterial::bindless_slot_count` requires BOTH),
/// keeping the WGSL single-path.
///
/// All three images must be `TextureDimension::D2` with
/// `depth_or_array_layers = layer count` and an
/// `ImageSampler::nearest()`-style descriptor (crisp texels, spec §4.4);
/// albedo is sRGB, normal/MRA are linear.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct VoxelMaterialExt {
    /// sRGB albedo array, one layer per block type.
    #[texture(100, dimension = "2d_array")]
    #[sampler(101)]
    pub albedo: Handle<Image>,
    /// Linear tangent-space normal array (RGB = xyz * 0.5 + 0.5).
    #[texture(102, dimension = "2d_array")]
    #[sampler(103)]
    pub normal: Handle<Image>,
    /// Linear Metallic/Roughness/AO(/emissive-mask) array — channel
    /// convention in the module docs.
    #[texture(104, dimension = "2d_array")]
    #[sampler(105)]
    pub mra: Handle<Image>,
    /// Luminance scale for the MRA alpha emissive mask (HDR units, feeds
    /// bloom). TODO(EM-3.4): per-block emissive comes from the palette RON.
    #[uniform(106)]
    pub emissive_strength: f32,
    /// Vertex-AO response: occlusion is remapped as
    /// `1 - (1 - ao) * ao_strength` (1.0 stays fixed, darkening scales).
    /// Needed because the v1 CPU bake samples the ColLight atlas only at the
    /// 4 corners of each greedy quad (convert.rs docs): crease-corner
    /// vertices bottom out around ~0.74, which is imperceptible once
    /// multiplied into the indirect share of the lighting — pixel-A/B
    /// measured −0.4% at 1.0. Values ~2..3 restore the visible Bedrock
    /// corner. TODO(EM-3.5): per-fragment ColLight atlas sampling makes this
    /// mostly redundant.
    #[uniform(107)]
    pub ao_strength: f32,
}

impl MaterialExtension for VoxelMaterialExt {
    fn vertex_shader() -> ShaderRef {
        // Registered by `VoxelRenderPlugin` via `embedded_asset!` (the
        // crate `src/` prefix is trimmed by the embedded source).
        "embedded://xindeler_render_voxel/material/voxel.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://xindeler_render_voxel/material/voxel.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Resolve the two custom attributes' offsets within the mesh's single
        // interleaved vertex buffer, then APPEND them (module docs: this also
        // runs for prepass/shadow pipelines, which ignore the extra
        // attributes — never replace `buffers` wholesale here).
        let extra = layout.0.get_layout(&[
            ATTRIBUTE_VOXEL_AO.at_shader_location(VOXEL_AO_SHADER_LOCATION),
            ATTRIBUTE_BLOCK_LAYER.at_shader_location(BLOCK_LAYER_SHADER_LOCATION),
        ])?;
        if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
            debug_assert_eq!(buffer.array_stride, extra.array_stride);
            buffer.attributes.extend(extra.attributes);
        }
        Ok(())
    }
}

/// Registers the embedded WGSL + the `MaterialPlugin` for [`VoxelMaterial`].
/// Added by [`crate::VoxelRenderPlugin`].
pub(crate) struct VoxelMaterialPlugin;

impl Plugin for VoxelMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "voxel.wgsl");
        app.add_plugins(MaterialPlugin::<VoxelMaterial>::default());
    }
}
