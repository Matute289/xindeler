// BL-82 EM-3.11 Phase B — FarTerrainExtension main-pass shaders (vertex +
// fragment): world-curvature vertex bend (Matías's "Option C") + soft
// horizon-occlusion/sky-blend fragment dissolve. See `far_terrain_material.
// rs`'s module docs for the full design rationale and the template this
// follows (`xindeler-render-voxel/src/material/water.wgsl` — same
// "reconstruct forward_io::VertexOutput, call the stock PBR helpers"
// pattern, verified against the same bevy_pbr-0.19.0 sources that file
// cites, PLUS `bevy_render`'s `view.wgsl` for `view.world_position`, the
// live camera position every material can read for free from group 0).
//
// Used for the MAIN pass only; prepass/shadow keep `StandardMaterial`'s
// default shaders (see `far_terrain_material.rs`'s `specialize`). Meshes
// come from `far_terrain::far_mesh_from_heights`: POSITION/NORMAL/COLOR
// always present (no UV/tangent/skinning), plus FAR_HORIZON @8 appended by
// `specialize()`.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io,
    mesh_view_bindings::view,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, alpha_discard},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

// Extension bindings (slots >= 100; 0-99 = StandardMaterial). Group index is
// templated by the material pipeline (same convention as voxel.wgsl/water.wgsl).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> bend_strength: f32;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<uniform> bend_start: f32;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> sun_direction: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var<uniform> haze_fog_color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var<uniform> haze_sky_color: vec4<f32>;

struct FarTerrainVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
    // BL-82 EM-3.11 Phase B: packed occluder record (rg = first packed
    // sample, ba = second — see the fragment shader's selection comment),
    // each byte normalised to [0, 1] CPU-side (`far_terrain.rs`'s
    // `far_mesh_from_heights`).
    @location(8) horizon: vec4<f32>,
}

struct FarTerrainVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
    @location(6) @interpolate(flat) instance_index: u32,
    @location(8) horizon: vec4<f32>,
}

@vertex
fn vertex(vertex: FarTerrainVertex) -> FarTerrainVertexOutput {
    var out: FarTerrainVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0)
    );

    // ---- Option C: world-curvature vertex bend (T49.6) ----
    // Camera-relative HORIZONTAL (XZ) distance — Bevy's frame is Y-up, and
    // the bend must not care how high the camera itself is, only how far out
    // the vertex sits. `view.world_position` is the live camera position,
    // free on group 0 (no extra Xindeler-side uniform needed — same as
    // `water.wgsl` reading `globals.time` for free).
    let cam_to_vertex_xz = world_position.xz - view.world_position.xz;
    let d = length(cam_to_vertex_xz);
    // Zero for every vertex at or inside `bend_start` (the far mesh's
    // `hole_radius`) — this is what keeps the bent far mesh flush with the
    // UNBENT near-voxel terrain at the seam (no crack). Grows as the square
    // of the distance beyond it.
    let beyond = max(d - bend_start, 0.0);
    let drop = bend_strength * beyond * beyond;
    world_position.y = world_position.y - drop;

    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
    out.world_normal =
        mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
    out.instance_index = vertex.instance_index;
    out.horizon = vertex.horizon;
    return out;
}

const PI_2: f32 = 1.5707963267948966;
// How much the stored occluder ANGLE is scaled toward its full value by the
// stored occluder-HEIGHT byte — a short occluder shows less of its angle
// (weak horizon shadow), a tall one shows it in full. Cheap analytic stand-in
// for the old engine's `horizon_at2` height/angle ray intersection
// (`xindeler-old/assets/voxygen/shaders/include/lod.glsl` 165-251) — NOT a
// literal port (module docs: no ray-march here).
const HORIZON_HEIGHT_INFLUENCE_MIN: f32 = 0.3;
// Soft transition width (radians) around the occluder elevation angle —
// mirrors horizon_at2's own smoothstep soft-shadow shape (its `w = 0.1`).
const HORIZON_SOFTNESS: f32 = 0.12;
// Floor so a fully-occluded fragment doesn't go pure black (a cheap ambient-
// bounce stand-in — horizon_at2's own `MIN_LIGHT` floor).
const HORIZON_MIN_LIGHT: f32 = 0.35;

// Sky-blend shaping (T49.6's "dissolve into haze" — cheap-to-eyeball VISUAL
// constants, not gameplay balance, same spirit as water.wgsl's ripple
// consts). `SKY_BLEND_DIST_RANGE` is metres BEYOND `bend_start` over which
// the fragment fades fully to the haze colour; `SKY_BLEND_SINK_RANGE` is
// metres of camera-relative Y-sink (driven by the SAME bend as above) that
// alone also drives a full fade — so a stronger `bend_strength` naturally
// dissolves the mesh faster too, not just makes it recede.
const SKY_BLEND_DIST_RANGE: f32 = 350.0;
const SKY_BLEND_SINK_RANGE: f32 = 60.0;

@fragment
fn fragment(
    in: FarTerrainVertexOutput,
    @builtin(front_facing) is_front: bool,
) -> forward_io::FragmentOutput {
    // Reconstruct the stock VertexOutput so the whole StandardMaterial
    // fragment path stays reusable (same trick as voxel.wgsl/water.wgsl).
    var std_in: forward_io::VertexOutput;
    std_in.position = in.position;
    std_in.world_position = in.world_position;
    std_in.world_normal = in.world_normal;
#ifdef VERTEX_COLORS
    std_in.color = in.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    std_in.instance_index = in.instance_index;
#endif

    var pbr_input = pbr_input_from_standard_material(std_in, is_front);

    // ---- (a) soft sun-occlusion from the horizon record ----
    // Selects the first (rg) or second (ba) packed occluder record by the
    // sun's east/west sign — mirrors the old engine's own selection rule
    // exactly (`horizon_at2`: `mix(f_horizons.rg, f_horizons.ba,
    // bvec2(light_dir.x < 0.0))`); which physical side (west/east) `rg`/`ba`
    // encode doesn't matter here, only that the same rule picks the same
    // record the old engine would have.
    let sun_dir = normalize(sun_direction.xyz);
    let occluder = select(in.horizon.xy, in.horizon.zw, sun_dir.x < 0.0);
    let occluder_angle = occluder.x * PI_2;
    let occluder_strength = mix(HORIZON_HEIGHT_INFLUENCE_MIN, 1.0, occluder.y);
    let occluder_elevation = occluder_angle * occluder_strength;
    let sun_elevation = asin(clamp(sun_dir.y, -1.0, 1.0));
    let lit = smoothstep(
        occluder_elevation - HORIZON_SOFTNESS,
        occluder_elevation + HORIZON_SOFTNESS,
        sun_elevation,
    );
    let occlusion = max(lit, HORIZON_MIN_LIGHT);
    pbr_input.material.base_color = vec4<f32>(
        pbr_input.material.base_color.rgb * occlusion,
        pbr_input.material.base_color.a,
    );

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: forward_io::FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }

    // ---- (b) dissolve the silhouette into atmosphere ----
    // Distance-driven fade (from `bend_start`, the mesh's own near-band
    // boundary) PLUS a fade driven by how far the bend has sunk this
    // fragment below eye level — either alone can push the blend to full;
    // composes with (does not replace) the engine's own `DistanceFog`
    // applied below by `main_pass_post_lighting_processing`.
    //
    // The dissolve target is `haze_fog_color`, NOT `haze_sky_color`:
    // `sky_color` is the world `ClearColor` (a dark "void" tone, only ever
    // visible where literally nothing draws), while `fog_color` is the pale
    // haze tone `DistanceFog` itself fades toward and closely matches the
    // physically-based `Atmosphere` sky's OWN horizon gradient — blending
    // toward the dark void colour left a visibly hard, unsoftened ridge
    // silhouetted against the much brighter rendered sky (caught in the
    // `XINDELER_SMOKE_FAR_MESH_CAM=1` visual smoke, not by any unit test).
    // `haze_sky_color` still contributes a SMALL extra darkening once a
    // fragment has sunk deep below eye level (capped at 25%), rather than
    // being unused — a subtle "fading toward nothing" cue for the most
    // extreme recession, without ever letting it dominate and defeat the
    // near-horizon dissolve into the bright sky.
    let dist = length(in.world_position.xz - view.world_position.xz);
    let dist_fade = smoothstep(bend_start, bend_start + SKY_BLEND_DIST_RANGE, dist);
    let sink = view.world_position.y - in.world_position.y; // > 0 once bent below eye level
    let sink_fade = smoothstep(0.0, SKY_BLEND_SINK_RANGE, sink);
    let sky_blend = clamp(max(dist_fade, sink_fade), 0.0, 1.0);
    let haze = mix(haze_fog_color.rgb, haze_sky_color.rgb, clamp(sink_fade, 0.0, 1.0) * 0.25);
    out.color = vec4<f32>(mix(out.color.rgb, haze, sky_blend), out.color.a);

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
