// EM-3.3 — VoxelMaterialExt main-pass shaders (vertex + fragment).
//
// Verified against bevy_pbr-0.19.0 WGSL sources: forward_io.wgsl (VertexOutput
// locations 0-7), mesh.wgsl (the default vertex stage this replicates),
// pbr_fragment.wgsl (pbr_input_from_standard_material), pbr_functions.wgsl
// (diffuse/specular_occlusion multiply INDIRECT light only — spec §4.3's
// required insertion point), pbr.wgsl (the default fragment this replaces).
//
// Used for the MAIN opaque pass only: prepass/shadow/deferred keep
// StandardMaterial's default shaders (see material/mod.rs). Meshes MUST come
// from the EM-3.2 converter: POSITION/NORMAL/UV_0 always present (so the
// VERTEX_UVS_A / VERTEX_NORMALS shader defs are always set for this
// pipeline), plus VOXEL_AO @8 and BLOCK_LAYER @9 appended by specialize().
// No skinning/morph/tangent/color/uv_b path — terrain never has them (the
// converter never emits ATTRIBUTE_COLOR: an unhandled VERTEX_COLORS def
// would zero base_color in the reconstruction below).

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, alpha_discard},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

// Extension bindings (slots >= 100; 0-99 = StandardMaterial). Group index is
// templated by the material pipeline (MATERIAL_BIND_GROUP shader def,
// bevy_pbr::material.rs).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var voxel_albedo: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var voxel_albedo_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var voxel_normal: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var voxel_normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var voxel_mra: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var voxel_mra_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<uniform> voxel_emissive_strength: f32;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<uniform> voxel_ao_strength: f32;

struct VoxelVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // EM-3.2 custom attributes (locations appended in specialize()).
    @location(8) voxel_ao: f32,
    @location(9) block_layer: u32,
}

// forward_io::VertexOutput's fields at their canonical locations (0-2, 6) +
// our varyings at the free locations 8/9. AO interpolates linearly across the
// quad (the smooth Bedrock corner gradient); the layer is @interpolate(flat).
struct VoxelVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(6) @interpolate(flat) instance_index: u32,
    @location(8) voxel_ao: f32,
    @location(9) @interpolate(flat) block_layer: u32,
}

@vertex
fn vertex(vertex: VoxelVertex) -> VoxelVertexOutput {
    var out: VoxelVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal =
        mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
    out.uv = vertex.uv;
    out.instance_index = vertex.instance_index;
    out.voxel_ao = vertex.voxel_ao;
    out.block_layer = vertex.block_layer;
    return out;
}

@fragment
fn fragment(
    in: VoxelVertexOutput,
    @builtin(front_facing) is_front: bool,
) -> forward_io::FragmentOutput {
    // Reconstruct the stock VertexOutput so the whole StandardMaterial
    // fragment path stays reusable. Unset ifdef'd fields (uv_b/tangent/color/
    // visibility dither) zero-init, which is correct because the mesh
    // pipeline never sets their defs for converter meshes (header note).
    var std_in: forward_io::VertexOutput;
    std_in.position = in.position;
    std_in.world_position = in.world_position;
    std_in.world_normal = in.world_normal;
#ifdef VERTEX_UVS_A
    std_in.uv = in.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    std_in.instance_index = in.instance_index;
#endif

    var pbr_input = pbr_input_from_standard_material(std_in, is_front);

    // ---- texture-array lookups, layer = flat EM-3.2 vertex attribute ----
    let layer = i32(in.block_layer);
    let albedo = textureSample(voxel_albedo, voxel_albedo_sampler, in.uv, layer);
    let mra = textureSample(voxel_mra, voxel_mra_sampler, in.uv, layer);
    let normal_sample = textureSample(voxel_normal, voxel_normal_sampler, in.uv, layer).xyz;

    pbr_input.material.base_color *= albedo;
    // MRA convention (material/mod.rs): R=metallic, G=roughness, B=AO,
    // A=emissive mask.
    pbr_input.material.metallic = mra.r;
    pbr_input.material.perceptual_roughness = clamp(mra.g, 0.045, 1.0);
    pbr_input.material.emissive = vec4<f32>(
        pbr_input.material.emissive.rgb + albedo.rgb * mra.a * voxel_emissive_strength,
        pbr_input.material.emissive.a,
    );

    // Normal mapping with an ANALYTIC tangent basis: greedy faces are always
    // axis-aligned, and the converter's planar UV projection fixes the
    // tangent per dominant-normal axis (keep in sync with convert.rs
    // `planar_uv`: T = d(pos)/du, B = d(pos)/dv). No ATTRIBUTE_TANGENT
    // needed. Valid only for unrotated chunk transforms (converter contract).
    let n_geo = normalize(in.world_normal);
    let n_abs = abs(n_geo);
    var t: vec3<f32>;
    var b: vec3<f32>;
    if n_abs.y >= n_abs.x && n_abs.y >= n_abs.z {
        t = vec3<f32>(1.0, 0.0, 0.0); // uv = (x, z)
        b = vec3<f32>(0.0, 0.0, 1.0);
    } else if n_abs.x >= n_abs.z {
        t = vec3<f32>(0.0, 0.0, 1.0); // uv = (z, y)
        b = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        t = vec3<f32>(1.0, 0.0, 0.0); // uv = (x, y)
        b = vec3<f32>(0.0, 1.0, 0.0);
    }
    let n_ts = normal_sample * 2.0 - 1.0;
    pbr_input.N = normalize(t * n_ts.x + b * n_ts.y + n_geo * n_ts.z);

    // ---- spec §4.3: vertex AO darkens INDIRECT light only ----
    // apply_pbr_lighting multiplies these into irradiance-volume /
    // environment / ambient terms exclusively; direct (shadow-mapped) light
    // is untouched. Composes with SSAO and the MRA texture AO channel.
    // Strength remap (1.0 fixed point): see material/mod.rs `ao_strength`.
    let vertex_ao = saturate(1.0 - (1.0 - in.voxel_ao) * voxel_ao_strength);
    let occlusion = vertex_ao * mra.b;
    pbr_input.diffuse_occlusion *= occlusion;
    pbr_input.specular_occlusion *= occlusion;

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: forward_io::FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    // Distance fog / tonemapping-for-non-HDR etc., same tail as pbr.wgsl.
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
