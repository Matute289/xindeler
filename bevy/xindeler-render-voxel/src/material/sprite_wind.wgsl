// EM-3.9c — SpriteWindMaterialExt main-pass shaders (vertex + fragment):
// wind-sway v2 for block sprites (grass/flowers/props), replacing EM-3.9's
// static vertex-coloured `StandardMaterial`.
//
// ## Why v1 (EM-3.9b) was reverted
// The first attempt displaced `world_position` in the vertex shader with a
// per-vertex sine offset but left `world_normal` as the STOCK, unperturbed
// value. For a real 3-D voxel-meshed sprite (many faces at every axis
// orientation, unlike a flat billboard card) that mismatch drove the PBR
// diffuse term toward zero for a large share of faces/angles — sprites
// rendered "largely black" (see `xindeler-client::sprite_view`'s module docs
// for the full root-cause writeup).
//
// ## v2: a RIGID ROTATION applied identically to position AND normal
// Instead of a raw shear, this sways each vertex by rotating it (and its
// normal) by the SAME angle about a horizontal axis through the sprite's own
// world-space base pivot (the instance's translation — sprites are placed at
// the block floor, see `sprite.rs`'s placement docs). Because a rotation is
// applied IDENTICALLY to both `world_position` (relative to the pivot) and
// `world_normal`, the two stay geometrically consistent BY CONSTRUCTION at
// every vertex — there is no separate "recompute the normal" step that could
// drift out of sync, unlike a shear (which changes the true local surface
// normal without changing the attribute) or the reverted v1 (which changed
// neither the true surface answer nor bothered to update the attribute).
// A per-vertex weight ([`crate::convert::ATTRIBUTE_SPRITE_SWAY`], baked at
// mesh-build time from the vertex's own height and the sprite kind's
// category — rigid props like furniture/dungeon décor bake to exactly 0.0)
// scales both the rotation angle and, implicitly, is a no-op for anything
// baked to zero (branch skipped per-vertex at zero extra cost beyond the
// `wind_strength > 0.0` check).
//
// ## Wind source (v1 of v2 — no shared field yet)
// There is no client-side synced wind field yet (the old client's own
// `wind_vel` comes from the weather simulation grid, `scene/mod.rs`'s
// `weather.wind_vel()` — not ported). This shader is the FIRST consumer of
// wind on the Bevy client; rather than block on that plumbing, it derives a
// self-contained phase from `globals.time` (already free, see `water.wgsl`)
// plus each instance's own world-space XZ position (spatial variety, so
// nearby sprites don't all sway in lockstep) — a fixed WIND_DIR constant, no
// per-frame CPU uniform update needed. EM-7.10 (Phase 7, currently blocked
// pending Phase 6) is planned to introduce the REAL shared, weather-synced
// wind field feeding foliage/rain/clouds coherently and explicitly
// "supersedes the reverted EM-3.9c sprite-sway" per the engine-migration
// backlog — when it lands, this shader's fixed WIND_DIR/phase should be
// swapped for that field's sampled direction/strength, but the position+
// normal ROTATION technique below stays valid unchanged.
//
// Verified against bevy_pbr-0.19.0 WGSL sources (same set `voxel.wgsl`/
// `water.wgsl` check — see `voxel.wgsl`'s header for the citation list).
// Sprite meshes (`figure::segment_to_bevy`) always carry
// POSITION(0)/NORMAL(1)/COLOR(5) — no UV, no tangent, no skinning — plus
// SPRITE_SWAY appended by `specialize()` at location 8.

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
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> wind_strength: f32;

struct SpriteVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(5) color: vec4<f32>,
    // EM-3.9c: per-vertex sway weight baked by `sprite::bake_sway_weights`
    // (location appended by `specialize()`).
    @location(8) sway: f32,
}

struct SpriteVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(5) color: vec4<f32>,
    @location(6) @interpolate(flat) instance_index: u32,
}

// Small-angle cap (radians) at full per-vertex weight and full
// `wind_strength` — tuned to read as a gentle grass-in-a-breeze sway, not a
// cartoonish flail (roughly matches the old client's own subtle magnitude,
// `sprite-vert.glsl`'s `SCALE_FACTOR` chain). Cheap-to-eyeball VISUAL
// constant (not gameplay balance) — same spirit as `water.wgsl`'s ripple
// frequency/speed consts.
const MAX_SWAY_RADIANS: f32 = 0.12;
const WIND_SPEED: f32 = 1.3;
const WIND_SPATIAL_FREQ: f32 = 0.15;
// Fixed world-space wind direction (Bevy XZ ground plane) — placeholder
// single global wind; see module doc comment for the EM-7.10 replacement.
const WIND_DIR: vec2<f32> = vec2<f32>(0.8, 0.6);

@vertex
fn vertex(vertex: SpriteVertex) -> SpriteVertexOutput {
    var out: SpriteVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    var world_normal =
        mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);

    if wind_strength > 0.0 && vertex.sway > 0.0 {
        // Pivot at the instance's own world-space base (sprites are placed
        // with their translation AT the block floor — sprite.rs docs), so
        // the rotation bends the sprite about its own root, not the mesh
        // origin of some unrelated shared space.
        let pivot = world_from_local[3].xyz;
        let dir = normalize(WIND_DIR);
        let phase = dot(pivot.xz, dir) * WIND_SPATIAL_FREQ + globals.time * WIND_SPEED;
        let theta = sin(phase) * vertex.sway * wind_strength * MAX_SWAY_RADIANS;
        // Horizontal axis perpendicular to the wind direction — rotating
        // about it tips the sprite's top over IN the wind direction.
        let axis = vec3<f32>(-dir.y, 0.0, dir.x);
        let c = cos(theta);
        let s = sin(theta);

        // Rodrigues' rotation formula, applied IDENTICALLY to the relative
        // position and to the normal — the whole point of "normal-consistent
        // by construction" (module doc comment above).
        let rel = world_position.xyz - pivot;
        let rotated_rel = rel * c + cross(axis, rel) * s + axis * dot(axis, rel) * (1.0 - c);
        world_position = vec4<f32>(pivot + rotated_rel, world_position.w);
        world_normal = normalize(
            world_normal * c + cross(axis, world_normal) * s + axis * dot(axis, world_normal) * (1.0 - c)
        );
    }

    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
    out.world_normal = world_normal;
    out.color = vertex.color;
    out.instance_index = vertex.instance_index;
    return out;
}

@fragment
fn fragment(
    in: SpriteVertexOutput,
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
