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
//! world-space planar UVs). MRA channel convention (formalised by the
//! EM-3.4 block palette, which bakes it in [`crate::palette::
//! build_block_texture_arrays`]): `R = metallic`, `G = perceptual
//! roughness`, `B = texture AO`, `A = emissive mask` (× albedo ×
//! [`VoxelMaterialExt::emissive_strength`] — lava/crystal glow).
//!
//! ## Per-fragment atlas AO — evaluated for EM-3.5, deferred (design sketch)
//! Upstream samples the ColLight atlas PER FRAGMENT; our v1 bakes it per
//! vertex (convert.rs docs). Doing it per fragment here is NOT a drop-in:
//! the ColLight atlas is PER CHUNK, so the material would need a 4th,
//! per-chunk texture binding — i.e. one `VoxelMaterial` asset per chunk.
//! That forfeits the single shared material every chunk entity reuses today
//! (one bind group, cheap budgeted uploads, trivial hot-reload-in-place) and
//! adds an atlas image upload per chunk to the EM-3.5 budget. Sketch for
//! when a chunk shows a crease the corner bake misses (the known v1 loss):
//! 1. converter emits `ATTRIBUTE_ATLAS_UV` (Float32x2, `atlas_pos /
//!    atlas_size`, shader location 10 — free in main + prepass, same argument
//!    as 8/9),
//! 2. extension grows `#[texture(108)] #[sampler(109)] col_light:
//!    Handle<Image>` (linear filter, per-chunk asset built from
//!    `TerrainAtlasData::col_lights`),
//! 3. fragment replaces `in.voxel_ao` with `textureSample(col_light, …,
//!    atlas_uv).a` and `ao_strength` returns to ~1.0,
//! 4. pipeline gains a small per-chunk material cache (`HashMap<ChunkKey,
//!    Handle<VoxelMaterial>>`) and counts the atlas upload against the frame
//!    budget.
//!
//! Not scheduled: terrain-ish content splits greedy quads at exactly the
//! creases that matter (EM-3.2 histogram test), so the corner bake plus the
//! `ao_strength` remap covers what the eye sees today.
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
    /// bloom). EM-3.4: data — `block_palette.ron`
    /// (`material.emissive_strength`); the per-block mask itself is baked
    /// into the MRA alpha by the palette's array builder.
    #[uniform(106)]
    pub emissive_strength: f32,
    /// Vertex-AO response: occlusion is remapped as
    /// `1 - (1 - ao) * ao_strength` (1.0 stays fixed, darkening scales).
    /// Needed because the v1 CPU bake samples the ColLight atlas only at the
    /// 4 corners of each greedy quad (convert.rs docs): crease-corner
    /// vertices bottom out around ~0.74, which is imperceptible once
    /// multiplied into the indirect share of the lighting — pixel-A/B
    /// measured −0.4% at 1.0. Values ~2..3 restore the visible Bedrock
    /// corner. EM-3.4: data — `block_palette.ron` (`material.ao_strength`).
    /// Per-fragment ColLight sampling would retire the remap — evaluated
    /// and deferred with a design sketch (module docs, EM-3.5 decision).
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
