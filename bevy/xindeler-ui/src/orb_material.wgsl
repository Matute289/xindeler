// BL-82 EM-5.17 — `OrbLiquidMaterial`'s fragment shader.
//
// v2 wave (replaces the old v1 flat clip-window boundary — see
// `orb_material.rs`'s module doc comment for the full history): the
// liquid/empty-vessel boundary is a WAVY, animated line (a sum of two sines
// at different frequency/phase/speed, offset by `time`) instead of a flat
// horizontal cut, and the region ABOVE that line (the depleted portion of
// the orb) reveals a dark grey stone/metal tint instead of discarding to
// transparent — the "empty vessel" read Matías asked for.
//
// `in.uv.y` runs 0.0 at the top of the node to 1.0 at the bottom (bevy_ui's
// standard UV convention), so "liquid" is `uv.y >= cutoff` where `cutoff =
// 1.0 - fill_fraction`, wave-perturbed per-column.
//
// `crop_min_px`/`crop_size_px` are a PIXEL-space sub-rect of the liquid
// texture's own native size (mirrors the old `ImageNode::rect` crop the v1
// CPU-clip path used — `MaterialNode` has no `rect` field, see
// `minimap_material.wgsl`'s identical crop rationale). `textureDimensions`
// gives the real loaded texture size in-shader, so no canvas-pixel-size
// constant needs to be hand-duplicated on the Rust side. `crop_size_px ==
// vec2(0.0)` (the `fill_source_crop: None` case) means "no crop, sample the
// full [0,1]^2 UV" — the sentinel `OrbLiquidMaterial::new` uses when the
// caller passes no crop.

#import bevy_ui::ui_vertex_output::UiVertexOutput

// Wave shape constants (a v1-reasonable "sloshing water" look, not a full
// FFT ocean shader) — two sines at different spatial frequency/speed/phase
// so the surface never reads as a single, obviously-periodic ripple.
const WAVE_AMPLITUDE: f32 = 0.028;
const WAVE_FREQ_A: f32 = 14.0;
const WAVE_FREQ_B: f32 = 9.0;
const WAVE_SPEED_A: f32 = 1.7;
const WAVE_SPEED_B: f32 = -1.15;
const WAVE_PHASE_B: f32 = 1.9;

// The exposed "empty vessel" material underneath the liquid — a flat dark
// grey metal/stone tint (deliberately not a full PBR stone material, see
// `orb_material.rs`'s module doc comment on why a flat tint is the right v1
// scope here). Darkened (Matías live-test follow-up, BL-82 EM-5.17: the
// original 0.16/0.17/0.20 read as washed-out light grey, not "empty metal
// vessel") to a near-black gunmetal, sampled against the HUD-D4 orb frame
// art's own darkest ring pixels (`orb_frame_{angel,cuthulhu,stamina}.png`,
// which sit in the 0–0x30 charcoal range) with a slight cool tint so it
// reads as metal rather than flat matte black.
const STONE_COLOR: vec3<f32> = vec3<f32>(0.012, 0.013, 0.016);

// A thin brighter foam/highlight band right at the wavy surface line so the
// boundary reads as a moving waterline rather than a flat-tinted seam.
const FOAM_BAND_UV: f32 = 0.012;
const FOAM_COLOR: vec3<f32> = vec3<f32>(0.85, 0.9, 0.95);
const FOAM_STRENGTH: f32 = 0.55;

@group(1) @binding(0)
var liquid_texture: texture_2d<f32>;
@group(1) @binding(1)
var liquid_sampler: sampler;
@group(1) @binding(2)
var<uniform> fill_fraction: f32;
@group(1) @binding(3)
var<uniform> time: f32;
@group(1) @binding(4)
var<uniform> crop_min_px: vec2<f32>;
@group(1) @binding(5)
var<uniform> crop_size_px: vec2<f32>;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    var sample_uv = in.uv;
    if crop_size_px.x > 0.0 && crop_size_px.y > 0.0 {
        let tex_size = vec2<f32>(textureDimensions(liquid_texture));
        sample_uv = (crop_min_px + in.uv * crop_size_px) / tex_size;
    }
    let liquid_sample = textureSample(liquid_texture, liquid_sampler, sample_uv);

    let clamped_fraction = clamp(fill_fraction, 0.0, 1.0);
    let base_cutoff = 1.0 - clamped_fraction;

    // Agitated liquid surface: perturb the cutoff line horizontally so it's
    // never a flat clip, even while the fraction itself holds roughly
    // steady (the "sloshing water" read Matías asked for).
    let wave = WAVE_AMPLITUDE * sin(in.uv.x * WAVE_FREQ_A + time * WAVE_SPEED_A)
        + WAVE_AMPLITUDE * 0.6 * sin(in.uv.x * WAVE_FREQ_B + time * WAVE_SPEED_B + WAVE_PHASE_B);
    let cutoff = clamp(base_cutoff + wave, 0.0, 1.0);

    if in.uv.y < cutoff {
        // Above the wavy surface line: the depleted portion of the orb.
        // Reveal the dark stone/metal tint instead of discarding to
        // transparent — masked by the liquid texture's OWN alpha channel so
        // the reveal still respects the source art's circular footprint
        // (that alpha shape is already what carves the circle in the v1
        // CPU-clip path too; no separate mask is needed).
        return vec4<f32>(STONE_COLOR, liquid_sample.a);
    }

    // Below the line: the remaining liquid, with a thin brighter foam
    // highlight right at the waterline.
    let dist_from_surface = in.uv.y - cutoff;
    let foam = (1.0 - smoothstep(0.0, FOAM_BAND_UV, dist_from_surface)) * FOAM_STRENGTH;
    let color = mix(liquid_sample.rgb, FOAM_COLOR, foam);
    return vec4<f32>(color, liquid_sample.a);
}
