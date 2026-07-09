// EM-3.9b — WaterMaterialExt main-pass shaders (vertex + fragment): animates
// the translucent water surface (scroll + ripple + a fresnel-ish edge sheen)
// instead of EM-3.9's static flat `StandardMaterial` placeholder.
//
// Verified against bevy_pbr-0.19.0 WGSL sources (same set `voxel.wgsl` checks
// — see that file's header for the citation list) PLUS `bevy_render`'s
// `globals.wgsl` (`Globals.time`, seconds since startup, group 0 binding 11,
// imported via `bevy_pbr::mesh_view_bindings::globals` — free to read from
// any material's shaders, no extra Xindeler-side time uniform needed).
//
// Used for the MAIN opaque(-ish, alpha-blended) pass only; prepass/shadow
// keep `StandardMaterial`'s default shaders (see `water.rs`). Meshes come
// from `convert::fluid_mesh_to_bevy`: POSITION/NORMAL/UV_0 always present,
// plus RIVER_VELOCITY @8 appended by `specialize()`. No skinning/morph/
// tangent/color/uv_b path — fluid meshes never carry them.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io,
    mesh_view_bindings::globals,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, alpha_discard},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

// Extension binding (slot >= 100; 0-99 = StandardMaterial). Group index is
// templated by the material pipeline (same convention as voxel.wgsl).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> water_ripple_strength: f32;

struct WaterVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // EM-3.9/3.9b: river-flow velocity, Bevy ground-plane xz (location
    // appended by specialize()).
    @location(8) river_velocity: vec2<f32>,
}

struct WaterVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(6) @interpolate(flat) instance_index: u32,
    @location(8) river_velocity: vec2<f32>,
}

@vertex
fn vertex(vertex: WaterVertex) -> WaterVertexOutput {
    var out: WaterVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal =
        mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
    out.uv = vertex.uv;
    out.instance_index = vertex.instance_index;
    out.river_velocity = vertex.river_velocity;
    return out;
}

// Ambient drift: EVERY water surface animates a little, even a still lake or
// ocean with zero `river_velocity` (only actual river chunks carry nonzero
// flow) — a fixed diagonal scroll direction/speed. River flow ADDS on top,
// scaled down (river_velocity is world-space blocks-ish, not UV-sized) so
// fast rivers still look plausible rather than swimming past instantly.
// These are cheap-to-eyeball VISUAL constants (not gameplay balance), same
// spirit as voxel.wgsl's tangent-basis table; only the "how strong" knob
// (`water_ripple_strength`) is exposed as a real uniform (see water.rs).
const AMBIENT_DIR: vec2<f32> = vec2<f32>(0.6, 0.8);
const AMBIENT_SPEED: f32 = 0.05;
const FLOW_SCROLL_SCALE: f32 = 0.08;
const RIPPLE_FREQ_A: f32 = 0.55;
const RIPPLE_FREQ_B: f32 = 0.9;
const RIPPLE_SPEED_A: f32 = 0.7;
const RIPPLE_SPEED_B: f32 = -1.1;
const RIPPLE_DEPTH: f32 = 0.28;
// High power -> a THIN rim only at near-grazing angles (a fluid mesh's
// shore-facing "wall" quads sit close to 90 degrees from most camera angles,
// so a low power here washed the whole wall out to blown-out white — tuned up
// after visual smoke-screenshot review, EM-3.9b). Magnitude is small and
// capped below so it reads as a subtle sheen, never a highlight blowout.
const FRESNEL_POWER: f32 = 6.0;
const FRESNEL_MAX: f32 = 0.18;

@fragment
fn fragment(
    in: WaterVertexOutput,
    @builtin(front_facing) is_front: bool,
) -> forward_io::FragmentOutput {
    // Reconstruct the stock VertexOutput so the whole StandardMaterial
    // fragment path stays reusable (same trick as voxel.wgsl).
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

    // ---- UV scroll: ambient drift + river flow, both time-driven ----
    let scroll = AMBIENT_DIR * AMBIENT_SPEED * globals.time
        + in.river_velocity * FLOW_SCROLL_SCALE * globals.time;
    let suv = in.uv + scroll;

    // ---- two overlapping sine ripples (no normal map needed for v1) ----
    // `ripple` is in [-1, 1]; RIPPLE_DEPTH compresses the multiplicative
    // swing to roughly [0.72, 1.28] at the default strength — a clearly
    // visible shimmer (verified in the EM-3.9b smoke screenshot diff) without
    // ever flashing the surface toward pure black/white at an unlucky phase.
    let ripple = sin(suv.x * RIPPLE_FREQ_A * 6.283185 + globals.time * RIPPLE_SPEED_A)
        * cos(suv.y * RIPPLE_FREQ_B * 6.283185 + globals.time * RIPPLE_SPEED_B);
    let shimmer = 1.0 + ripple * water_ripple_strength * RIPPLE_DEPTH;
    pbr_input.material.base_color = vec4<f32>(
        pbr_input.material.base_color.rgb * shimmer,
        pbr_input.material.base_color.a,
    );

    // ---- fresnel-ish edge sheen (grazing angle -> brighter), mirroring the
    // spirit of voxygen's water opacity/reflection falloff term without its
    // full sky-reflection machinery. Capped at FRESNEL_MAX (small) so it
    // reads as a sheen, never a highlight blowout on near-grazing faces
    // (e.g. a pool's shore-facing "wall" quads). ----
    let ndotv = clamp(dot(pbr_input.N, pbr_input.V), 0.0, 1.0);
    let fresnel = min(pow(1.0 - ndotv, FRESNEL_POWER), 1.0) * FRESNEL_MAX * water_ripple_strength;
    pbr_input.material.emissive = vec4<f32>(
        pbr_input.material.emissive.rgb + fresnel * vec3<f32>(0.5, 0.65, 0.85),
        pbr_input.material.emissive.a,
    );

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: forward_io::FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
