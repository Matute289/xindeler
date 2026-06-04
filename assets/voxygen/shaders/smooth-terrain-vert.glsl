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

// Vertex layout matches SmoothTerrainVertex (20 bytes):
//   location 0: pos       — vec3 float, chunk-local coords
//   location 1: norm      — uint, 10-10-10-2 snorm packed normal
//   location 2: col_light — uint, RGBA+light packed via TerrainVertex::make_col_light
layout(location = 0) in vec3 v_pos;
layout(location = 1) in uint v_norm;
layout(location = 2) in uint v_col_light;

// Locals at set 2 (smooth pipeline has no atlas at set 2, unlike terrain at set 3).
layout(std140, set = 2, binding = 0) uniform u_locals {
    mat4 model_mat;
    ivec4 atlas_offs; // unused in smooth path, kept for layout compatibility
    float load_time;
};

layout(location = 0) out vec3 f_pos;
layout(location = 1) flat out uint f_col_light;
layout(location = 2) out vec3 f_norm;

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

    f_col_light = v_col_light;

    gl_Position = all_mat * vec4(f_pos, 1.0);
}
