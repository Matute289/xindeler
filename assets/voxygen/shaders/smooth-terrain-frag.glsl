#version 440 core

#include <constants.glsl>

#define LIGHTING_TYPE LIGHTING_TYPE_REFLECTION
#define LIGHTING_REFLECTION_KIND LIGHTING_REFLECTION_KIND_GLOSSY

#if (FLUID_MODE == FLUID_MODE_LOW)
    #define LIGHTING_TRANSPORT_MODE LIGHTING_TRANSPORT_MODE_IMPORTANCE
#elif (FLUID_MODE >= FLUID_MODE_MEDIUM)
    #define LIGHTING_TRANSPORT_MODE LIGHTING_TRANSPORT_MODE_RADIANCE
#endif

#define LIGHTING_DISTRIBUTION_SCHEME LIGHTING_DISTRIBUTION_SCHEME_MICROFACET
#define LIGHTING_DISTRIBUTION LIGHTING_DISTRIBUTION_BECKMANN

#define HAS_SHADOW_MAPS

#include <globals.glsl>
#include <random.glsl>

// Inputs from smooth-terrain-vert.glsl
layout(location = 0) in vec3 f_pos;
layout(location = 1) in vec4 f_color;       // .rgb = terrain color, .a = ambient occlusion
layout(location = 2) in vec3 f_norm;
layout(location = 4) in vec2 f_light_glow;  // .x = baked light [0,1], .y = glow [0,1]

// Normal map texture array — 8 layers, one per terrain material category.
// Layer indices: 0=rock, 1=grass, 2=sand, 3=snow, 4=earth, 5=wood, 6=ice, 7=leaves
layout(set = 3, binding = 0) uniform texture2DArray t_terrain_normals;
layout(set = 3, binding = 1) uniform sampler s_terrain_normals;

layout(location = 3) flat in uint f_block_kind;

// Locals at set 2 (same layout as vert shader)
layout(std140, set = 2, binding = 0) uniform u_locals {
    mat4 model_mat;
    ivec4 atlas_offs;
    float load_time;
};

layout(location = 0) out vec4 tgt_color;
layout(location = 1) out uvec4 tgt_mat;

#include <sky.glsl>
#include <light.glsl>
#include <lod.glsl>

// Triplanar normal map sampling.
// Samples the normal map from three orthogonal projections and blends them
// by the absolute value of the geometric normal components.
vec3 triplanar_normal(vec3 world_pos, vec3 geom_norm, float layer, float scale) {
    vec2 uv_x = fract(world_pos.yz * scale);
    vec2 uv_y = fract(world_pos.xz * scale);
    vec2 uv_z = fract(world_pos.xy * scale);

    vec3 tx = textureLod(sampler2DArray(t_terrain_normals, s_terrain_normals), vec3(uv_x, layer), 0.0).rgb;
    vec3 ty = textureLod(sampler2DArray(t_terrain_normals, s_terrain_normals), vec3(uv_y, layer), 0.0).rgb;
    vec3 tz = textureLod(sampler2DArray(t_terrain_normals, s_terrain_normals), vec3(uv_z, layer), 0.0).rgb;

    // Decode from [0,1] to [-1,1] tangent-space normals
    vec3 n_x = tx * 2.0 - 1.0;
    vec3 n_y = ty * 2.0 - 1.0;
    vec3 n_z = tz * 2.0 - 1.0;

    // Swizzle tangent-space normals to world space per projection
    n_x = vec3(n_x.z, n_x.y, n_x.x);
    n_y = vec3(n_y.x, n_y.z, n_y.y);
    // n_z stays as-is

    // Blend weights from absolute normal components, sharpened with ^4
    vec3 w = pow(abs(geom_norm), vec3(4.0));
    w /= (w.x + w.y + w.z + 0.001);

    return n_x * w.x + n_y * w.y + n_z * w.z;
}

void main() {
    // -----------------------------------------------------------------------
    // Color/light decoded in the vertex shader and passed as interpolatable floats.
    float f_light = f_light_glow.x;
    float f_glow  = f_light_glow.y;
    vec3  f_col   = f_color.rgb;
    float f_ao    = f_color.a;

    // -----------------------------------------------------------------------
    // Normal — comes directly as a vec3 from the vertex shader, already normalized.
    vec3 face_norm = normalize(f_norm);

    // Camera direction — needed both for parallax (Ultra) and lighting below.
    vec3 cam_to_frag = normalize(f_pos - cam_pos.xyz);

    // Triplanar normal map perturbation — tiles every 4 world units.
    const float NORMAL_MAP_SCALE    = 1.0 / 4.0;
    const float NORMAL_MAP_STRENGTH = 0.4;
#ifdef TERRAIN_SMOOTHING_ULTRA
    // Parallax offset: shift UV by view direction, scaled by height from normal map alpha.
    const float PARALLAX_SCALE = 0.04;
    vec2 par_uv = fract(f_pos.xy * NORMAL_MAP_SCALE);
    float height = textureLod(
        sampler2DArray(t_terrain_normals, s_terrain_normals),
        vec3(par_uv, float(f_block_kind)), 0.0
    ).a;
    vec2 view_ts = vec2(dot(cam_to_frag, vec3(1, 0, 0)),
                        dot(cam_to_frag, vec3(0, 1, 0)));
    vec2 par_offset = view_ts * (height - 0.5) * PARALLAX_SCALE;
    vec2 par_uv_z = fract((f_pos.xy + par_offset) * NORMAL_MAP_SCALE);
    vec3 tz_par = textureLod(
        sampler2DArray(t_terrain_normals, s_terrain_normals),
        vec3(par_uv_z, float(f_block_kind)), 0.0
    ).rgb;
    vec3 n_z_par = tz_par * 2.0 - 1.0;
    vec3 detail_no_par = triplanar_normal(f_pos, face_norm, float(f_block_kind), NORMAL_MAP_SCALE);
    vec3 w_abs = pow(abs(face_norm), vec3(4.0));
    float w_z = w_abs.z / (w_abs.x + w_abs.y + w_abs.z + 0.001);
    vec3 detail = detail_no_par + (n_z_par - detail_no_par) * w_z;
#else
    vec3 detail = triplanar_normal(f_pos, face_norm, float(f_block_kind), NORMAL_MAP_SCALE);
#endif
    vec3 f_norm_n = normalize(face_norm + detail * NORMAL_MAP_STRENGTH);

    // Smooth terrain is never underwater / fluid-facing.
    float fluid_alt  = f_pos.z + 1.0;

    // -----------------------------------------------------------------------
    // Camera and lighting setup (identical to terrain-frag.glsl)
    // Note: cam_to_frag is declared above (needed for parallax UV offset).
    vec3 view_dir    = -cam_to_frag;

    const float n2      = 1.5;
    const float R_s2s0  = pow(abs((1.0 - n2) / (1.0 + n2)), 2.0);
    float R_s = R_s2s0; // no fluid blending

    vec3 k_a = vec3(1.0);
    vec3 k_d = vec3(1.0);
    vec3 k_s = vec3(R_s);

    const float f_alpha = 1.0;
    const float alpha   = 1.0;

#if (SHADOW_MODE == SHADOW_MODE_CHEAP || SHADOW_MODE == SHADOW_MODE_MAP || FLUID_MODE >= FLUID_MODE_MEDIUM)
    float f_alt = alt_at(f_pos.xy);
#elif (SHADOW_MODE == SHADOW_MODE_NONE || FLUID_MODE == FLUID_MODE_LOW)
    float f_alt = f_pos.z;
#endif

    float not_underground = clamp((f_pos.z - f_alt) / 128.0 + 1.0, 0.0, 1.0);

    vec3 mu = vec3(0.0); // not underwater
    vec3 cam_attenuation = compute_attenuation_point(
        f_pos, -view_dir, mu, fluid_alt, cam_pos.xyz
    );

#if (SHADOW_MODE == SHADOW_MODE_CHEAP || SHADOW_MODE == SHADOW_MODE_MAP)
    vec4 f_shadow       = textureMaybeBicubic(t_horizon, s_horizon, pos_to_tex(f_pos.xy));
    float sun_shade_frac = horizon_at2(f_shadow, f_alt, f_pos, sun_dir);
#elif (SHADOW_MODE == SHADOW_MODE_NONE)
    float sun_shade_frac = 1.0;
#endif
    float moon_shade_frac = 1.0;

    DirectionalLight sun_info  = get_sun_info(sun_dir,  sun_shade_frac,  f_pos);
    DirectionalLight moon_info = get_moon_info(moon_dir, moon_shade_frac);

    // -----------------------------------------------------------------------
    // Lighting accumulation
    float max_light      = 0.0;
    vec3  emitted_light  = vec3(1.0);
    vec3  reflected_light = vec3(1.0);

    float sun_diffuse = get_sun_diffuse2(
        sun_info, moon_info, f_norm_n, view_dir, f_pos,
        mu, cam_attenuation, fluid_alt,
        k_a, k_d, k_s, alpha, f_norm_n, 1.0,
        emitted_light, reflected_light
    );
    max_light += sun_diffuse;

    // Apply baked light (same formula as terrain-frag)
#if (FLUID_MODE == FLUID_MODE_LOW)
    f_light = f_light * sqrt(f_light);
#else
    f_light = not_underground * f_light * sqrt(f_light);
#endif

    emitted_light  *= f_light;
    reflected_light *= f_light;
    max_light       *= f_light;

    // Glow from nearby light sources
    vec3 glow = glow_light(f_pos)
        * (pow(f_glow, 3.0) * 5.0 + pow(f_glow, 2.0) * 2.0)
        * pow(max(dot(face_norm, f_norm_n), 0.0), 2.0);
    reflected_light += glow * cam_attenuation;

    max_light += lights_at(
        f_pos, f_norm_n, view_dir, mu, cam_attenuation, fluid_alt,
        k_a, k_d, k_s, alpha, f_norm_n, 1.0,
        emitted_light, reflected_light
    );

    // Ambient occlusion
    emitted_light  *= mix(1.0, f_ao, 0.5);
    reflected_light *= mix(1.0, f_ao, 0.5);

    float point_shadow = shadow_at(f_pos, f_norm_n);
    reflected_light *= point_shadow;
    emitted_light   *= point_shadow;

    // -----------------------------------------------------------------------
    // Per-fragment noise for subtle color variation (same as terrain-frag)
    vec3 f_chunk_pos = f_pos - (model_mat[3].xyz - focus_off.xyz);

    #ifdef EXPERIMENTAL_NONOISE
        float noise = 0.0;
    #else
        float noise = hash(vec4(floor(f_chunk_pos * 3.0 - f_norm_n * 0.5), 0.0));
    #endif

    const float A            = 0.055;
    const float W_INV        = 1.0 / (1.0 + A);
    const float W_2          = W_INV * W_INV;
    const float NOISE_FACTOR = 0.015;
    vec3 noise_delta = sqrt(f_col) * W_INV + noise * NOISE_FACTOR;
    vec3 col = noise_delta * noise_delta * W_2;

    vec3 surf_color = illuminate(max_light, view_dir, col * emitted_light, col * reflected_light);

    tgt_color = vec4(surf_color, f_alpha);
    tgt_mat   = uvec4(uvec3((f_norm_n + 1.0) * 127.0), MAT_BLOCK);
}
