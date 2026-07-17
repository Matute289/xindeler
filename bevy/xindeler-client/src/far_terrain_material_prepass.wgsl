// BL-82 EM-3.11 round 24 — FarTerrainExtension's prepass fragment shader.
//
// The main-pass fragment shader (far_terrain_material.wgsl) discards any
// fragment inside the near real-terrain band (`near_band`) so the LOD-object
// zone proxies never draw on top of the real near-terrain trees/houses. That
// alone is NOT sufficient (bevy-migration-reviewer + rust-perf-reviewer
// finding, round 24 review): Bevy's depth/normal prepass (active whenever TAA
// or occlusion culling is on — this project's `camera.rs` enables both)
// specializes its OWN fragment shader (`MaterialExtension::
// prepass_fragment_shader`), which defaults to `ShaderRef::Default` when not
// overridden — i.e. StandardMaterial's stock `pbr_prepass.wgsl`, a SEPARATE
// shader with no knowledge of `near_band`. Without this file, a near-band
// proxy would still WRITE DEPTH during the prepass even though its main-pass
// fragment discards, and since the main opaque pass reads that same depth
// buffer (reverse-Z `GreaterEqual`), a nearer proxy depth can depth-reject
// the real terrain/house fragment that was SUPPOSED to render there — turning
// the z-fight into a background-coloured hole instead of fixing it.
//
// This shader deliberately does NOT override `prepass_vertex_shader` — the
// stock prepass vertex shader's output (`prepass_io::VertexOutput`) is reused
// unchanged (this material's own main-pass vertex bend is provably a no-op
// inside `near_band`, since `bend_start` is always >= `near_band` — see
// far_terrain_material.wgsl's vertex-shader `beyond = max(d - bend_start,
// 0.0)` clamp — so there is no bent-vs-unbent position mismatch to worry
// about within the band this file cares about). Only the FRAGMENT stage
// differs from stock: an early `near_band` discard, then the identical
// normal/motion-vector encoding `bevy_pbr::pbr_prepass` already performs for
// an untextured, always-lit, always-double-sided (`cull_mode: None`) opaque
// material — this material never sets a normal map, UV, tangent, or
// non-Opaque `alpha_mode` (see far_terrain.rs/lod_objects.rs's fixed
// `StandardMaterial { base_color, cull_mode: None, perceptual_roughness,
// reflectance, ..default() }` construction), so the branches stock
// `pbr_prepass.wgsl` guards with `STANDARD_MATERIAL_NORMAL_MAP`/alpha-mask
// logic never apply to it and are intentionally not reproduced here — the
// encode formulas below are ported verbatim from bevy_pbr 0.19.0's
// `src/render/pbr_prepass.wgsl`, the only difference being the added
// discard and the pruned always-false branches.
//
// A SEPARATE embedded file (not a second `@fragment fn` in
// far_terrain_material.wgsl) is deliberate: wgpu/naga resolves a shader
// stage's entry point by requiring exactly one function tagged for that
// stage per requested module, so the main-pass fragment and this prepass
// fragment must live in distinct modules even though both are named
// `fragment` — exactly how bevy_pbr itself splits `pbr.wgsl` from
// `pbr_prepass.wgsl` rather than packing both into one file.

#import bevy_pbr::{
    prepass_io,
    pbr_functions,
    pbr_prepass_functions,
    mesh_view_bindings::view,
}

// Same binding far_terrain_material.wgsl declares at @binding(105) — see that
// file's doc comment for the full explanation of what this value means. Only
// the ONE uniform this shader actually reads is re-declared here; the rest of
// the material's bind group (unused by this stage) is omitted, matching the
// existing `water.wgsl`/`voxel.wgsl` convention of only declaring the
// bindings a given shader stage touches.
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<uniform> near_band: f32;

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(
    in: prepass_io::VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> prepass_io::FragmentOutput {
    let cam_dist_xz = length(in.world_position.xz - view.world_position.xz);
    if near_band > 0.0 && cam_dist_xz < near_band {
        discard;
    }

    var out: prepass_io::FragmentOutput;

#ifdef NORMAL_PREPASS
    // Always lit, always double-sided (`cull_mode: None`) — matches this
    // material's fixed base-StandardMaterial configuration exactly, so the
    // unlit-flag branch stock `pbr_prepass.wgsl` guards against is never
    // reached by this material and is not reproduced.
    let world_normal = pbr_functions::prepare_world_normal(in.world_normal, true, is_front);
    out.normal = vec4<f32>(world_normal * 0.5 + vec3<f32>(0.5), 1.0);
#endif

#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(
        in.world_position, in.previous_world_position
    );
#endif

    return out;
}
#else
@fragment
fn fragment(in: prepass_io::VertexOutput) {
    let cam_dist_xz = length(in.world_position.xz - view.world_position.xz);
    if near_band > 0.0 && cam_dist_xz < near_band {
        discard;
    }
}
#endif
