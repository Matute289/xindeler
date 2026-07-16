// BL-82 EM-5.17 T57.9 — `OrbLiquidMaterial`'s fragment shader.
//
// v1: samples the liquid texture and discards every fragment above the
// current `fill_fraction` cutoff line (a flat horizontal cut, bottom-up —
// `in.uv.y` runs 0.0 at the top of the node to 1.0 at the bottom, matching
// `bevy_ui`'s standard UV convention, so "visible" is `uv.y >= 1.0 -
// fill_fraction").
//
// TODO(v2 wave): perturb the cutoff line with a time-based sine term (e.g.
// `cutoff + amplitude * sin(uv.x * frequency + globals.time * speed)`) for
// the AAA "wavy liquid surface" polish the design spec calls for. Left
// unimplemented deliberately (module doc comment in `orb_material.rs`
// explains why: Phase 2 is expected to ship the CPU-clip orb bar for v1
// instead of this material, so there is no live caller depending on the
// wave yet) — a real `globals.time` uniform is already available via the
// standard bind group below, so this is a small, self-contained follow-up.

#import bevy_ui::ui_vertex_output::UiVertexOutput

@group(1) @binding(0)
var liquid_texture: texture_2d<f32>;
@group(1) @binding(1)
var liquid_sampler: sampler;
@group(1) @binding(2)
var<uniform> fill_fraction: f32;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let cutoff = 1.0 - clamp(fill_fraction, 0.0, 1.0);
    if in.uv.y < cutoff {
        discard;
    }
    return textureSample(liquid_texture, liquid_sampler, in.uv);
}
