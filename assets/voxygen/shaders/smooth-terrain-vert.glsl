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

#include <globals.glsl>

// Vertex layout matches SmoothTerrainVertex (24 bytes):
//   location 0: pos        — vec3 float, chunk-local coords
//   location 1: norm       — uint, 10-10-10-2 snorm packed normal
//   location 2: col_light  — uint, RGBA+light packed via TerrainVertex::make_col_light
//   location 3: block_kind — uint, normal-map layer index (0-7)
layout(location = 0) in vec3 v_pos;
layout(location = 1) in uint v_norm;
layout(location = 2) in uint v_col_light;
layout(location = 3) in uint v_block_kind;

// Locals at set 2 (smooth pipeline has no atlas at set 2, unlike terrain at set 3).
layout(std140, set = 2, binding = 0) uniform u_locals {
    mat4 model_mat;
    ivec4 atlas_offs; // unused in smooth path, kept for layout compatibility
    float load_time;
};

layout(location = 0) out vec3 f_pos;
// Decoded as interpolatable floats so color/light blend smoothly across Transvoxel
// triangles (flat uint would snap at each triangle edge and create a faceted look).
layout(location = 1) out vec4 f_color;       // .rgb = terrain color, .a = ambient occlusion
layout(location = 2) out vec3 f_norm;
layout(location = 3) flat out uint f_block_kind;
layout(location = 4) out vec2 f_light_glow;  // .x = baked light [0,1], .y = glow [0,1]

void main() {
    // Transform chunk-local float position to world-relative (focus-offset) space.
    f_pos = (model_mat * vec4(v_pos, 1.0)).xyz - focus_off.xyz;

    // Decode 10-10-10-2 snorm: each component is 10 bits, sign-extended from bit 9.
    int nx = int(v_norm & 0x3FFu);
    int ny = int((v_norm >> 10u) & 0x3FFu);
    int nz = int((v_norm >> 20u) & 0x3FFu);
    if (nx >= 512) nx -= 1024;
    if (ny >= 512) ny -= 1024;
    if (nz >= 512) nz -= 1024;
    f_norm = normalize(vec3(float(nx), float(ny), float(nz)) / 511.0);

    // Decode TerrainVertex::make_col_light packed uint into interpolatable floats.
    //   b0 = (light[4:0] << 3) | (r[3:1])          — bits  7:0
    //   b1 = (glow[4:0]  << 3) | (b[3:1])           — bits 15:8
    //   b2 = (r[7:4])          | (b[7:4])            — bits 23:16
    //   b3 = (g[7:1])          | ao                  — bits 31:24
    uint b0 = v_col_light & 0xFFu;
    uint b1 = (v_col_light >> 8u)  & 0xFFu;
    uint b2 = (v_col_light >> 16u) & 0xFFu;
    uint b3 = (v_col_light >> 24u) & 0xFFu;
    float col_r  = float((b2 & 0xF0u) | ((b0 & 0x7u) << 1u)) / 255.0;
    float col_g  = float(b3 & 0xFEu) / 255.0;
    float col_b  = float(((b2 & 0xFu) << 4u) | ((b1 & 0x7u) << 1u)) / 255.0;
    float col_ao = float(b3 & 0x1u);
    f_color      = vec4(col_r, col_g, col_b, col_ao);
    f_light_glow = vec2(float(b0 >> 3u) / 31.0, float(b1 >> 3u) / 31.0);

    f_block_kind = v_block_kind;

    gl_Position = all_mat * vec4(f_pos, 1.0);
}
