# SmoothTerrainVertex Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a dedicated `SmoothTerrainPipeline` that renders Transvoxel meshes with float positions and 10-10-10-2 packed normals, eliminating the faceted appearance caused by integer-quantized vertices in the current `TerrainVertex` path.

**Architecture:** The smooth pipeline is completely separate from the greedy terrain pipeline. It introduces `SmoothTerrainVertex` (20 bytes: `[f32; 3]` pos + `u32` 10-10-10-2 norm + `u32` packed col_light). The pipeline binds locals (model matrix) at set 2 instead of set 3, since no atlas texture is needed — color is baked per-vertex at mesh time via `TerrainVertex::make_col_light`. Two new GLSL shaders handle decoding. The Transvoxel mesh path in `mesh/terrain.rs` is updated to emit `Mesh<SmoothTerrainVertex>` (returned as the third `MeshGen` slot, previously `_shadow_mesh`). `TerrainChunkData` gets a `smooth_opaque_model` field, and the scene's `render()` conditionally draws it via a new `SmoothTerrainDrawer`.

**Tech Stack:** Rust/wgpu (pipeline creation), GLSL 440 (shaders compiled at startup via shaderc), bytemuck (Pod/Zeroable), vek (Vec3<f32>), existing veloren rendering infrastructure.

---

## File Map

| Status | File | Change |
|--------|------|--------|
| Create | `voxygen/src/render/pipelines/smooth_terrain.rs` | `SmoothTerrainVertex`, `pack_norm_10_10_10_2`, `SmoothTerrainPipeline` |
| Modify | `voxygen/src/render/pipelines/mod.rs` | add `pub mod smooth_terrain;` + re-export |
| Modify | `voxygen/src/render/mod.rs` | re-export vertex + pipeline types |
| Modify | `voxygen/src/render/renderer/pipeline_creation.rs` | `ShaderModules`, `IngamePipelines`, `Pipelines`, shader loading, pipeline creation |
| Create | `assets/voxygen/shaders/smooth-terrain-vert.glsl` | vertex shader: float pos, decode 10-10-10-2 normal |
| Create | `assets/voxygen/shaders/smooth-terrain-frag.glsl` | fragment shader: decode per-vertex col_light, full PBR lighting |
| Modify | `voxygen/src/render/renderer/drawer.rs` | `SmoothTerrainDrawer` struct + `draw_smooth_terrain()` on `FirstPassDrawer` |
| Modify | `voxygen/src/scene/terrain/mod.rs` | `MeshWorkerResponseMesh` + `TerrainChunkData` + `render_smooth()` |
| Modify | `voxygen/src/mesh/terrain.rs` | Transvoxel path → emit `Mesh<SmoothTerrainVertex>` via third `MeshGen` slot |

---

## Task 1: Create `smooth_terrain.rs` pipeline module

**Files:**
- Create: `voxygen/src/render/pipelines/smooth_terrain.rs`

- [ ] **Step 1.1: Create the file with vertex format and packing helper**

```rust
// voxygen/src/render/pipelines/smooth_terrain.rs
use super::{
    super::{AaMode, GlobalsLayouts, Vertex as VertexTrait},
    terrain::TerrainLayout,
};
use bytemuck::{Pod, Zeroable};
use std::mem;
use vek::*;

/// Vertex format for the smooth Transvoxel terrain pipeline.
///
/// Positions are stored as raw floats (chunk-local coords) so the interpolated
/// Transvoxel vertices are not rounded to the integer block grid.
/// Normals are packed as 10-10-10-2 snorm so smooth gradient normals are
/// preserved rather than being quantized to one of 6 axis-aligned directions.
/// Color is baked per-vertex at mesh time so no atlas texture sampler is
/// needed at draw time.
#[repr(C)]
#[derive(Copy, Clone, Debug, Zeroable, Pod)]
pub struct SmoothTerrainVertex {
    pos: [f32; 3],      // chunk-local float position (12 bytes)
    norm: u32,          // 10-10-10-2 snorm packed normal (4 bytes)
    col_light: u32,     // RGBA + light packed via TerrainVertex::make_col_light (4 bytes)
}

/// Pack a unit normal into 10-10-10-2 snorm representation.
/// Each component is mapped from [-1, 1] to a signed 10-bit integer [-511, 511].
/// The top 2 bits (w component) are unused and set to 0.
pub fn pack_norm_10_10_10_2(norm: Vec3<f32>) -> u32 {
    let pack = |v: f32| (v.clamp(-1.0, 1.0) * 511.0) as i32 as u32 & 0x3FF;
    pack(norm.x) | (pack(norm.y) << 10) | (pack(norm.z) << 20)
}

impl SmoothTerrainVertex {
    pub fn new(pos: Vec3<f32>, norm: Vec3<f32>, col_light: u32) -> Self {
        Self {
            pos: pos.into_array(),
            norm: pack_norm_10_10_10_2(norm),
            col_light,
        }
    }

    pub fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Uint32];
        wgpu::VertexBufferLayout {
            array_stride: Self::STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        }
    }
}

impl VertexTrait for SmoothTerrainVertex {
    const QUADS_INDEX: Option<wgpu::IndexFormat> = None; // triangles, no index buffer
    const STRIDE: wgpu::BufferAddress = mem::size_of::<Self>() as wgpu::BufferAddress;
}

pub struct SmoothTerrainPipeline {
    pub pipeline: wgpu::RenderPipeline,
}

impl SmoothTerrainPipeline {
    pub fn new(
        device: &wgpu::Device,
        vs_module: &wgpu::ShaderModule,
        fs_module: &wgpu::ShaderModule,
        global_layout: &GlobalsLayouts,
        terrain_layout: &TerrainLayout,
        aa_mode: AaMode,
        format: wgpu::TextureFormat,
    ) -> Self {
        common_base::span!(_guard, "SmoothTerrainPipeline::new");

        // The smooth pipeline has no atlas texture at set 2.
        // Locals (model matrix) are bound at set 2, not set 3.
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Smooth terrain pipeline layout"),
                push_constant_ranges: &[],
                bind_group_layouts: &[
                    &global_layout.globals,          // set 0
                    &global_layout.shadow_textures,  // set 1
                    &terrain_layout.locals,          // set 2 (locals only, no atlas)
                ],
            });

        let samples = aa_mode.samples();

        let render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Smooth terrain pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: vs_module,
                    entry_point: Some("main"),
                    buffers: &[SmoothTerrainVertex::desc()],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::GreaterEqual,
                    stencil: wgpu::StencilState {
                        front: wgpu::StencilFaceState::IGNORE,
                        back: wgpu::StencilFaceState::IGNORE,
                        read_mask: !0,
                        write_mask: 0,
                    },
                    bias: wgpu::DepthBiasState {
                        constant: 0,
                        slope_scale: 0.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: fs_module,
                    entry_point: Some("main"),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Uint,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            });

        Self {
            pipeline: render_pipeline,
        }
    }
}
```

- [ ] **Step 1.2: Compile check**

```bash
cd /Users/mgrinberg/Workspace/RustroverProjects/veloren
cargo build -p veloren-voxygen 2>&1 | head -40
```

Expected: error about `smooth_terrain` not being declared as a module in `pipelines/mod.rs`. That's fine — we fix it in Task 2.

- [ ] **Step 1.3: Commit**

```bash
git add voxygen/src/render/pipelines/smooth_terrain.rs
git commit -m "feat(render): add SmoothTerrainVertex format and SmoothTerrainPipeline"
```

---

## Task 2: Wire pipeline into Rust infrastructure

**Files:**
- Modify: `voxygen/src/render/pipelines/mod.rs`
- Modify: `voxygen/src/render/mod.rs`
- Modify: `voxygen/src/render/renderer/pipeline_creation.rs`

### Step 2.1: `pipelines/mod.rs` — expose submodule

- [ ] Open `voxygen/src/render/pipelines/mod.rs`

Add after the last `pub mod` line (currently `pub mod ui;`):

```rust
pub mod smooth_terrain;
```

Also add to the re-exports at the bottom of the file (after `pub use self::{figure::FigureSpriteAtlasData, terrain::TerrainAtlasData};`):

```rust
pub use self::smooth_terrain::{SmoothTerrainPipeline, SmoothTerrainVertex};
```

### Step 2.2: `render/mod.rs` — re-export for crate consumers

- [ ] Open `voxygen/src/render/mod.rs`

Find the existing terrain re-export line (around line 42):
```rust
terrain::{Locals as TerrainLocals, TerrainLayout, Vertex as TerrainVertex},
```

Add a new line alongside the terrain re-exports:
```rust
smooth_terrain::{SmoothTerrainPipeline, SmoothTerrainVertex},
```

(Exact placement: immediately after the `terrain::` re-export, keeping alphabetical order in the use block.)

### Step 2.3: `pipeline_creation.rs` — add to `ShaderModules` struct

- [ ] Open `voxygen/src/render/renderer/pipeline_creation.rs`

**3a.** In `struct ShaderModules { ... }` (around line 118), add two new fields after `terrain_frag`:

```rust
smooth_terrain_vert: wgpu::ShaderModule,
smooth_terrain_frag: wgpu::ShaderModule,
```

**3b.** In `ShaderModules::new()` (the `Ok(Self { ... })` block around line 337), add after `terrain_frag: create_shader(...)`:

```rust
smooth_terrain_vert: create_shader("smooth-terrain-vert", ShaderStage::Vertex)?,
smooth_terrain_frag: create_shader("smooth-terrain-frag", ShaderStage::Fragment)?,
```

### Step 2.4: `pipeline_creation.rs` — add to `IngamePipelines` and `Pipelines`

**4a.** In `pub struct IngamePipelines { ... }` (around line 46), add:

```rust
pub smooth_terrain: smooth_terrain::SmoothTerrainPipeline,
```

**4b.** In `pub struct Pipelines { ... }` (around line 22), add:

```rust
pub smooth_terrain: smooth_terrain::SmoothTerrainPipeline,
```

**4c.** In `impl Pipelines { fn consolidate(...) -> Self { Self { ... } } }` (around line 92), add inside the `Self { ... }` block:

```rust
smooth_terrain: ingame.smooth_terrain,
```

### Step 2.5: `pipeline_creation.rs` — add to task count and create the pipeline

**5a.** In `fn create_ingame_and_shadow_pipelines`, change `tasks: [Task; 20]` to `tasks: [Task; 21]`.

**5b.** In the array destructuring of tasks (around line 499), add `smooth_terrain_task` at the end:

```rust
let [
    debug_task,
    skybox_task,
    figure_task,
    terrain_task,
    fluid_task,
    sprite_task,
    lod_object_task,
    particle_task,
    rope_task,
    trail_task,
    lod_terrain_task,
    clouds_task,
    bloom_task,
    postprocess_task,
    point_shadow_task,
    terrain_directed_shadow_task,
    figure_directed_shadow_task,
    debug_directed_shadow_task,
    terrain_directed_rain_occlusion_task,
    figure_directed_rain_occlusion_task,
    smooth_terrain_task,  // new
] = tasks;
```

**5c.** After the existing `create_terrain` closure (around line 593), add:

```rust
// Pipeline for rendering smooth (Transvoxel) terrain
let create_smooth_terrain = || {
    smooth_terrain_task.run(
        || {
            smooth_terrain::SmoothTerrainPipeline::new(
                device,
                &shaders.smooth_terrain_vert,
                &shaders.smooth_terrain_frag,
                &layouts.global,
                &layouts.terrain,
                pipeline_modes.aa,
                format,
            )
        },
        "smooth terrain pipeline creation",
    )
};
```

**5d.** In the final join tree (around line 882), add `create_smooth_terrain` to one of the join groups. Simplest: expand `j8` (currently just `create_rope`):

Change:
```rust
let j8 = create_rope;
```
To:
```rust
let j8 = || pool.join(create_rope, create_smooth_terrain);
```

**5e.** Update the result tuple destructuring to capture `smooth_terrain`. Find the block starting with `let (((debug, ...` (around line 907) and the matching `IngameAndShadowPipelines { ingame: IngamePipelines { ... } }` block.

Change the outer destructuring from:
```rust
    ((lod_object, (terrain_directed_rain_occlusion, figure_directed_rain_occlusion)), rope),
```
To:
```rust
    ((lod_object, (terrain_directed_rain_occlusion, figure_directed_rain_occlusion)), (rope, smooth_terrain)),
```

**5f.** Add `smooth_terrain` to `IngamePipelines { ... }` inside `IngameAndShadowPipelines`:

```rust
ingame: IngamePipelines {
    debug,
    figure,
    fluid,
    lod_terrain,
    particle,
    rope,
    trail,
    clouds,
    bloom,
    postprocess,
    skybox,
    sprite,
    lod_object,
    terrain,
    smooth_terrain,  // new
},
```

Also add to `Pipelines::consolidate()`:
```rust
smooth_terrain: ingame.smooth_terrain,
```

### Step 2.6: Update the call sites for `create_ingame_and_shadow_pipelines`

The function is called twice in `pipeline_creation.rs` (around lines 1022 and 1122). Both pass `progress.create_tasks()` which uses `const N: usize` inference. Since we changed the array size to 21, both call sites will automatically infer `N = 21` from the new array destructuring. No changes needed at call sites.

### Step 2.7: Compile check

```bash
cargo build -p veloren-voxygen 2>&1 | head -60
```

Expected: errors about missing shader files (shaders are loaded at runtime from the asset directory — so the Rust build will compile, but the game will panic at startup until the shader files exist). If there are Rust compile errors, fix them before continuing.

- [ ] **Step 2.8: Commit**

```bash
git add voxygen/src/render/pipelines/mod.rs \
        voxygen/src/render/mod.rs \
        voxygen/src/render/renderer/pipeline_creation.rs
git commit -m "feat(render): wire SmoothTerrainPipeline into pipeline infrastructure"
```

---

## Task 3: Create `smooth-terrain-vert.glsl`

**Files:**
- Create: `assets/voxygen/shaders/smooth-terrain-vert.glsl`

- [ ] **Step 3.1: Create the vertex shader**

```glsl
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
layout(location = 2) flat out vec3 f_norm;

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
```

- [ ] **Step 3.2: Commit**

```bash
git add assets/voxygen/shaders/smooth-terrain-vert.glsl
git commit -m "feat(shaders): add smooth-terrain-vert.glsl"
```

---

## Task 4: Create `smooth-terrain-frag.glsl`

**Files:**
- Create: `assets/voxygen/shaders/smooth-terrain-frag.glsl`

The smooth fragment shader replicates the lighting pipeline of `terrain-frag.glsl` with three differences:
1. Color decoded per-vertex from `f_col_light` uint (no atlas texture sampling).
2. Normal comes directly from `f_norm` vec3 (no decoding from `f_pos_norm` bits).
3. `faces_fluid` is always `false` (smooth terrain is always opaque surface).

- [ ] **Step 4.1: Create the fragment shader**

```glsl
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
layout(location = 1) flat in uint f_col_light;
layout(location = 2) flat in vec3 f_norm;

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

void main() {
    // -----------------------------------------------------------------------
    // Decode per-vertex color packed by TerrainVertex::make_col_light:
    //   b0 = (light[4:0] << 3) | (r[3:1])          — bits  7:0  of col_light
    //   b1 = (glow[4:0]  << 3) | (b[3:1])           — bits 15:8
    //   b2 = (r[7:4])          | (b[7:4])            — bits 23:16
    //   b3 = (g[7:1])          | ao                  — bits 31:24
    uint b0 = f_col_light & 0xFFu;
    uint b1 = (f_col_light >> 8u)  & 0xFFu;
    uint b2 = (f_col_light >> 16u) & 0xFFu;
    uint b3 = (f_col_light >> 24u) & 0xFFu;

    float f_light = float(b0 >> 3u) / 31.0;
    float f_glow  = float(b1 >> 3u) / 31.0;

    // Reconstruct 7-bit color channels (bit 0 of each component is lost at encode time).
    float r = float((b2 & 0xF0u) | ((b0 & 0x7u) << 1u)) / 255.0;
    float g = float(b3 & 0xFEu)                          / 255.0;
    float b = float(((b2 & 0xFu) << 4u) | ((b1 & 0x7u) << 1u)) / 255.0;
    vec3 f_col = vec3(r, g, b);
    float f_ao = float(b3 & 0x1u); // 0.0 = no AO, 1.0 = ambient occlusion

    // -----------------------------------------------------------------------
    // Normal — comes directly as a vec3 from the vertex shader, already normalized.
    vec3 face_norm = normalize(f_norm);
    vec3 f_norm_n  = face_norm;

    // Smooth terrain is never underwater / fluid-facing.
    bool faces_fluid = false;
    float fluid_alt  = f_pos.z + 1.0;

    // -----------------------------------------------------------------------
    // Camera and lighting setup (identical to terrain-frag.glsl)
    vec3 cam_to_frag = normalize(f_pos - cam_pos.xyz);
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
    f_light = f_light * sqrt(f_light);
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
```

- [ ] **Step 4.2: Commit**

```bash
git add assets/voxygen/shaders/smooth-terrain-frag.glsl
git commit -m "feat(shaders): add smooth-terrain-frag.glsl with per-vertex color decode"
```

---

## Task 5: Add `SmoothTerrainDrawer` to `drawer.rs`

**Files:**
- Modify: `voxygen/src/render/renderer/drawer.rs`

The `TerrainDrawer` binds atlas at set 2 and locals at set 3. The `SmoothTerrainDrawer` only needs locals at set 2 (no atlas), and uses a plain `draw()` instead of `draw_indexed()` (triangles, not quads).

- [ ] **Step 5.1: Add import for smooth terrain types**

At the top of `drawer.rs`, find the existing import of terrain types (something like):
```rust
use super::super::pipelines::{
    ...
    terrain::{self, TerrainAtlasData},
    ...
};
```

Add `smooth_terrain` to that import:
```rust
smooth_terrain,
```

- [ ] **Step 5.2: Add `draw_smooth_terrain()` to `FirstPassDrawer`**

Inside `impl<'pass> FirstPassDrawer<'pass>`, after `draw_terrain()` (around line 1082), add:

```rust
pub fn draw_smooth_terrain(&mut self) -> SmoothTerrainDrawer<'_, 'pass> {
    let mut render_pass = self.render_pass.scope("smooth terrain");
    render_pass.set_pipeline(&self.pipelines.smooth_terrain.pipeline);
    // SmoothTerrainVertex::QUADS_INDEX = None, so no index buffer needed.
    SmoothTerrainDrawer { render_pass }
}
```

- [ ] **Step 5.3: Add `SmoothTerrainDrawer` struct and impl**

After the `TerrainDrawer` impl block (around line 1223), add:

```rust
#[must_use]
pub struct SmoothTerrainDrawer<'pass_ref, 'pass: 'pass_ref> {
    render_pass: Scope<'pass_ref, wgpu::RenderPass<'pass>>,
}

impl<'pass_ref, 'pass: 'pass_ref> SmoothTerrainDrawer<'pass_ref, 'pass> {
    pub fn draw<'data: 'pass>(
        &mut self,
        model: &'data Model<smooth_terrain::SmoothTerrainVertex>,
        locals: &'data terrain::BoundLocals,
    ) {
        if model.len() == 0 {
            return;
        }
        // Locals at set 2 (smooth pipeline has no atlas at set 2).
        self.render_pass.set_bind_group(2, &locals.bind_group, &[]);
        self.render_pass.set_vertex_buffer(0, model.buf().slice(..));
        // Direct triangle draw — no index buffer (QUADS_INDEX = None).
        self.render_pass.draw(0..model.len() as u32, 0..1);
    }
}
```

- [ ] **Step 5.4: Compile check**

```bash
cargo build -p veloren-voxygen 2>&1 | head -40
```

Expected: clean, or errors about `smooth_opaque_model` not existing yet (that's Task 6).

- [ ] **Step 5.5: Commit**

```bash
git add voxygen/src/render/renderer/drawer.rs
git commit -m "feat(render): add SmoothTerrainDrawer and draw_smooth_terrain to FirstPassDrawer"
```

---

## Task 6: Update terrain data structures

**Files:**
- Modify: `voxygen/src/scene/terrain/mod.rs` (structs only, no render logic yet)
- Modify: `voxygen/src/mesh/terrain.rs` (return type + Transvoxel mesh emission)

### Step 6.1: Update `MeshWorkerResponseMesh`

- [ ] In `voxygen/src/scene/terrain/mod.rs`, find `struct MeshWorkerResponseMesh` (around line 133). Add the field:

```rust
smooth_opaque_mesh: Mesh<SmoothTerrainVertex>,
```

The full struct becomes:
```rust
pub struct MeshWorkerResponseMesh {
    z_bounds: (f32, f32),
    sun_occluder_z_bounds: (f32, f32),
    opaque_mesh: Mesh<TerrainVertex>,
    fluid_mesh: Mesh<FluidVertex>,
    smooth_opaque_mesh: Mesh<SmoothTerrainVertex>,  // new
    atlas_texture_data: TerrainAtlasData,
    atlas_size: Vec2<u16>,
    light_map: LightMapFn,
    glow_map: LightMapFn,
    alt_indices: AltIndices,
}
```

Also ensure `SmoothTerrainVertex` is imported at the top of the file:
```rust
use crate::render::SmoothTerrainVertex;
```

### Step 6.2: Update `TerrainChunkData`

- [ ] In the same file, find `pub struct TerrainChunkData` (around line 78). Add field:

```rust
smooth_opaque_model: Option<Model<SmoothTerrainVertex>>,
```

The field goes after `opaque_model`:
```rust
opaque_model: Option<Model<TerrainVertex>>,
smooth_opaque_model: Option<Model<SmoothTerrainVertex>>,  // new
fluid_model: Option<Model<FluidVertex>>,
```

Also add `Model` import if not already present (it's typically already imported via `crate::render::Model`).

### Step 6.3: Update `generate_mesh` return type in `mesh/terrain.rs`

- [ ] Open `voxygen/src/mesh/terrain.rs`. Find `pub fn generate_mesh` (line 235). The current return type has `TerrainVertex` as the third `MeshGen` type parameter (the `_shadow_mesh` slot). Change it to `SmoothTerrainVertex`:

```rust
pub fn generate_mesh<'a>(
    vol: &'a VolGrid2d<TerrainChunk>,
    (range, max_texture_size, _boi, smoothing): (
        Aabb<i32>,
        Vec2<u16>,
        &'a BlocksOfInterest,
        TerrainSmoothingMode,
    ),
) -> MeshGen<
    TerrainVertex,
    FluidVertex,
    SmoothTerrainVertex,   // changed: was TerrainVertex (_shadow_mesh slot)
    (
        Aabb<f32>,
        TerrainAtlasData,
        Vec2<u16>,
        Arc<dyn Fn(Vec3<i32>) -> f32 + Send + Sync>,
        Arc<dyn Fn(Vec3<i32>) -> f32 + Send + Sync>,
        AltIndices,
        (f32, f32),
    ),
>
```

Also add `SmoothTerrainVertex` to the imports at the top of the file. Find the existing `use crate::render::{...}` block and add `SmoothTerrainVertex`:
```rust
use crate::render::{
    FluidVertex, Mesh, SmoothTerrainVertex, TerrainVertex, Tri,
    pipelines::terrain::TerrainAtlasData,
};
```

### Step 6.4: Update the Transvoxel early-return in `mesh/terrain.rs`

- [ ] The current Transvoxel path (lines 313–356) builds `Mesh<TerrainVertex>` and returns:
```rust
return (
    opaque_mesh,   // Mesh<TerrainVertex>
    Mesh::new(),
    Mesh::new(),
    (...),
);
```

Replace the entire "Emit mesh" section (lines 309–356) and the `opaque_mesh` declaration (line 313) with:

```rust
// ----------------------------------------------------------------
// Emit smooth mesh — SmoothTerrainVertex with float pos, 10-10-10-2
// normal, and per-vertex color baked from the atlas above.
// ----------------------------------------------------------------
let mut smooth_opaque_mesh: Mesh<SmoothTerrainVertex> = Mesh::new();
// Field (1,1,1) = world range.min → delta = (-1, -1, range.min.z - 1).
let mesh_delta = Vec3::new(-1.0f32, -1.0, (range.min.z - 1) as f32);
let atlas_pos_for = |pos: Vec3<f32>| -> Vec2<u16> {
    let cp = pos + mesh_delta;
    let ax = cp.x.round() as i32;
    let ay = cp.y.round() as i32;
    Vec2::new(
        ax.clamp(0, atlas_w as i32 - 1) as u16,
        ay.clamp(0, atlas_h as i32 - 1) as u16,
    )
};
let col_light_for = |pos: Vec3<f32>| -> u32 {
    let apos = atlas_pos_for(pos);
    let cl = col_lights[apos.x as usize * atlas_h + apos.y as usize];
    u32::from_le_bytes(cl)
};
for tri in &tris {
    let [p0, p1, p2] = tri.positions;
    let [n0, n1, n2] = tri.normals;
    smooth_opaque_mesh.push_tri(Tri::new(
        SmoothTerrainVertex::new(p0 + mesh_delta, n0, col_light_for(p0)),
        SmoothTerrainVertex::new(p1 + mesh_delta, n1, col_light_for(p1)),
        SmoothTerrainVertex::new(p2 + mesh_delta, n2, col_light_for(p2)),
    ));
}

let bounds = Aabb {
    min: range.min.map(|e| e as f32),
    max: range.max.map(|e| e as f32),
};
let sun_occluder_z_bounds = (bounds.min.z, bounds.max.z);
return (
    Mesh::new(),           // opaque_mesh = empty (smooth replaces greedy here)
    Mesh::new(),           // fluid_mesh = empty for Transvoxel path
    smooth_opaque_mesh,    // slot 3: Mesh<SmoothTerrainVertex>
    (
        bounds,
        atlas_data,
        atlas_size,
        Arc::new(|_| 1.0f32),
        Arc::new(|_| 0.0f32),
        AltIndices {
            deep_end: 0,
            underground_end: 0,
        },
        sun_occluder_z_bounds,
    ),
);
```

### Step 6.5: Update the greedy path return in `mesh/terrain.rs`

- [ ] The greedy path's final return statement currently returns `(opaque_mesh, fluid_mesh, shadow_mesh, (...))`. Change the third element from the greedy shadow mesh to an empty smooth mesh:

Find the final `(opaque_mesh, fluid_mesh, ` at the end of `generate_mesh` (after all the greedy meshing). The third value (previously a `Mesh<TerrainVertex>` for shadows, now unused) should be changed to:

```rust
Mesh::<SmoothTerrainVertex>::new(),
```

(Exact change: replace whatever the third slot currently is — it was an empty `Mesh::new()` — with `Mesh::<SmoothTerrainVertex>::new()`. The type annotation helps the compiler.)

### Step 6.6: Update call site in `scene/terrain/mod.rs`

- [ ] Find the `generate_mesh` call and destructuring (around line 277). Change:

```rust
let (
    opaque_mesh,
    fluid_mesh,
    _shadow_mesh,
    (...),
) = generate_mesh(...);
```

To:

```rust
let (
    opaque_mesh,
    fluid_mesh,
    smooth_opaque_mesh,   // was _shadow_mesh
    (...),
) = generate_mesh(...);
```

Then update the `MeshWorkerResponseMesh { ... }` constructor (around line 299) to include the new field:

```rust
mesh = Some(MeshWorkerResponseMesh {
    z_bounds: (bounds.min.z, bounds.max.z),
    sun_occluder_z_bounds,
    opaque_mesh,
    fluid_mesh,
    smooth_opaque_mesh,   // new
    atlas_texture_data,
    atlas_size,
    light_map,
    glow_map,
    alt_indices,
});
```

### Step 6.7: Populate `smooth_opaque_model` when inserting a chunk

- [ ] In `scene/terrain/mod.rs`, find the `self.insert_chunk(response.pos, TerrainChunkData { ... })` block (around line 1225). Add:

```rust
smooth_opaque_model: renderer.create_model(&mesh.smooth_opaque_mesh),
```

Right after `opaque_model: renderer.create_model(&mesh.opaque_mesh)`.

### Step 6.8: Compile check

```bash
cargo build -p veloren-voxygen 2>&1 | head -60
```

Fix any type errors. The most likely issue: missing imports, or `Mesh::<SmoothTerrainVertex>::new()` type annotation needed at the greedy path return.

- [ ] **Step 6.9: Commit**

```bash
git add voxygen/src/scene/terrain/mod.rs \
        voxygen/src/mesh/terrain.rs
git commit -m "feat(terrain): emit Mesh<SmoothTerrainVertex> from Transvoxel path; add smooth_opaque_model to TerrainChunkData"
```

---

## Task 7: Wire smooth rendering in scene

**Files:**
- Modify: `voxygen/src/scene/terrain/mod.rs` — add `render_smooth()` and call it

### Step 7.1: Add `render_smooth()` method to `Terrain`

- [ ] In `voxygen/src/scene/terrain/mod.rs`, find the `pub fn render<'a>` method (around line 1608). After it, add a new method:

```rust
pub fn render_smooth<'a>(
    &'a self,
    drawer: &mut FirstPassDrawer<'a>,
    focus_pos: Vec3<f32>,
    culling_mode: CullingMode,
) {
    span!(_guard, "render_smooth", "Terrain::render_smooth");
    let mut drawer = drawer.draw_smooth_terrain();

    let focus_chunk = Vec2::from(focus_pos).map2(TerrainChunk::RECT_SIZE, |e: f32, sz| {
        (e as i32).div_euclid(sz as i32)
    });

    Spiral2d::new()
        .filter_map(|rpos| {
            let pos = focus_chunk + rpos;
            Some((rpos, self.chunks.get(&pos)?))
        })
        .take(self.chunks.len())
        .filter(|(_, chunk)| chunk.visible.is_visible())
        .filter_map(|(rpos, chunk)| {
            Some((rpos, chunk.smooth_opaque_model.as_ref()?, &chunk.locals))
        })
        .for_each(|(rpos, model, locals)| {
            let _culling_mode = if rpos.magnitude_squared() < NEVER_CULL_DIST.pow(2) {
                CullingMode::None
            } else {
                culling_mode
            };
            drawer.draw(model, locals)
        });
}
```

### Step 7.2: Call `render_smooth()` from the scene render loop

- [ ] Find where `terrain.render(...)` is called in the Voxygen scene (likely in `voxygen/src/scene/mod.rs` or `voxygen/src/scene/figure/mod.rs` or similar). Search:

```bash
grep -rn "terrain\.render\b\|\.render_terrain\|render_smooth" \
    /Users/mgrinberg/Workspace/RustroverProjects/veloren/voxygen/src/scene/ | head -20
```

- [ ] Once found, add a call to `terrain.render_smooth(...)` immediately after `terrain.render(...)`, passing the same `drawer`, `focus_pos`, and `culling_mode` arguments. The smooth render replaces the greedy render for smooth-mode chunks; both can coexist since one will have `None` for the other's model.

Example (adapt to actual call site):
```rust
terrain.render(drawer, focus_pos, culling_mode);
terrain.render_smooth(drawer, focus_pos, culling_mode);
```

### Step 7.3: Compile check

```bash
cargo build -p veloren-voxygen 2>&1 | head -60
```

Fix any remaining errors (e.g., lifetime issues in `render_smooth` — the pattern is identical to `render`, so follow the same lifetime annotations).

- [ ] **Step 7.4: Commit**

```bash
git add voxygen/src/scene/terrain/mod.rs
git commit -m "feat(scene): add Terrain::render_smooth; call from scene render loop"
```

---

## Task 8: Compile, run, and verify

### Step 8.1: Full clippy check

- [ ] Run clippy (matches CI):

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo clippy \
    --all-targets --locked \
    --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
    -- -D warnings 2>&1 | grep -E "^error|^warning\[" | head -40
```

Fix all warnings-as-errors before continuing.

- [ ] Run publish-profile clippy:

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings 2>&1 | head -30
```

### Step 8.2: Run the game

- [ ] Launch:

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo run --bin veloren-voxygen 2>&1 | head -60
```

Watch for shader compilation errors at startup (they appear in the console before the window opens). Common issues:
- `#include` file not found → check spelling of included file names
- Undefined variable (e.g., `f_light` used before assignment) → check GLSL variable scoping
- Type mismatch (e.g., `float` vs `uint`) → check GLSL typing

### Step 8.3: Visual verification

- [ ] In Settings → Graphics, set Terrain Smoothing to `Soft` or `Smooth` (whichever corresponds to `TerrainSmoothingMode::Soft`/`Smooth`).

Expected:
- Terrain surface appears **smooth** — no blocky stepped edges between blocks
- Normals are **smooth** (lighting transitions gradually across surface, not at 90° block faces)
- No geometry gaps between chunks
- Colors are correct (match the block types underneath)

- [ ] Set Terrain Smoothing back to `Disabled`.

Expected:
- Terrain looks identical to before this branch (greedy mesh, blocky)
- No regression: regular terrain still renders, no missing chunks

### Step 8.4: Update the spec tracking table

- [ ] Edit `docs/superpowers/specs/2026-06-04-terrain-resolution-design.md`. Find the tracking table and update:

```markdown
| Fase 1 — SmoothTerrainVertex pipeline | ✅ Completa | Pipeline, shaders, drawer, mesh emission all wired |
```

- [ ] **Step 8.5: Final commit**

```bash
git add docs/superpowers/specs/2026-06-04-terrain-resolution-design.md
git commit -m "docs: mark Fase 1 SmoothTerrainVertex pipeline as complete"
```

---

## Known Limitations (follow-up tasks, not in scope here)

1. **Shadow casting**: `render_shadow_directed` still uses `opaque_model` (greedy mesh). When smoothing is enabled, `opaque_model` is `None`, so smooth terrain chunks don't cast directional shadows. Fix requires a `SmoothTerrainShadowPipeline` (out of scope for this task).

2. **Rain occlusion**: Same issue — `render_rain_occlusion` uses `opaque_model`. No impact on rendering correctness, but rain won't be occluded by smooth terrain.

3. **AltIndices culling**: `SmoothTerrainDrawer::draw()` ignores `CullingMode` (renders all triangles regardless of underground/surface split). Implementing this requires sorting Transvoxel triangles by Z at mesh time — future optimization.

4. **LOD**: All smooth chunks are rendered at full resolution regardless of distance. LOD support (3 levels) is a separate spec item.
