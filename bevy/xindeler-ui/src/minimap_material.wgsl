// BL-82 EM-5.17 Phase 3 — `MinimapFadeMaterial`'s fragment shader.
//
// Samples `minimap_texture` at a crop-adjusted UV (`crop_min + in.uv *
// crop_size` — the shader-side replacement for `ImageNode::rect`, since
// `MaterialNode` has no `rect` field to crop a sub-region of the source
// texture the way `ImageNode` does), then multiplies the sampled alpha by a
// radial falloff computed from the PANEL's own local UV (`in.uv`, always
// [0,1]^2 regardless of the crop window) — full opacity inside INNER_RADIUS,
// smoothly fading (smoothstep) to fully transparent by OUTER_RADIUS, so the
// minimap reads as a soft circular vignette dissolve rather than a hard
// square/circle clip (Matías's explicit override of spec §3.3's frame-PNG
// recommendation — see `minimap_material.rs`'s module doc comment for the
// full rationale).
//
// `inner_radius`/`outer_radius` are real uniforms (bindings 4/5), not baked
// -in constants — this avoids a hand-duplicated Rust/WGSL literal pair that
// nothing could catch drifting apart (`minimap_material.rs`'s
// `MinimapFadeMaterial::default_uv_radii` is the single source of truth,
// uploaded here the same way `crop_min`/`crop_size` already are).

#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0)
var minimap_texture: texture_2d<f32>;
@group(1) @binding(1)
var minimap_sampler: sampler;
@group(1) @binding(2)
var<uniform> crop_min: vec2<f32>;
@group(1) @binding(3)
var<uniform> crop_size: vec2<f32>;
@group(1) @binding(4)
var<uniform> inner_radius: f32;
@group(1) @binding(5)
var<uniform> outer_radius: f32;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let sample_uv = crop_min + in.uv * crop_size;
    var color = textureSample(minimap_texture, minimap_sampler, sample_uv);

    let dist = distance(in.uv, vec2<f32>(0.5, 0.5));
    let falloff = 1.0 - smoothstep(inner_radius, outer_radius, dist);
    color.a = color.a * falloff;
    return color;
}
