// Vignette + gamma placeholder post-process pass (BL-82 EM-2.6).
//
// Runs as a `FullscreenMaterial` (bevy_core_pipeline 0.19) in
// Core3dSystems::PostProcess, before tonemapping — the input is therefore
// HDR linear light. Bind group layout is fixed by FullscreenMaterialPlugin:
// (0) screen texture, (1) sampler, (2) our uniform.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;

struct VignettePost {
    strength: f32,
    gamma: f32,
}
@group(0) @binding(2) var<uniform> settings: VignettePost;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(screen_texture, texture_sampler, in.uv).rgb;

    // Gamma placeholder (neutral at gamma = 1.0).
    color = pow(max(color, vec3(0.0)), vec3(1.0 / max(settings.gamma, 0.01)));

    // Radial vignette: full brightness in the middle, darkened corners.
    let dist = distance(in.uv, vec2(0.5, 0.5));
    let vignette = 1.0 - settings.strength * smoothstep(0.35, 0.72, dist);

    return vec4(color * vignette, 1.0);
}
