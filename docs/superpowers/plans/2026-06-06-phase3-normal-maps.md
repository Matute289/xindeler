# Phase 3 — Normal Maps + Micro-detail — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-block-kind triplanar normal maps to the smooth terrain (Transvoxel) pipeline, giving rock, grass, sand, snow, etc. distinct surface texture without changing gameplay geometry.

**Architecture:** Block kind is tracked through the density field → transvoxel mesher → GPU vertex. A 8-layer wgpu texture array (one layer per material category) is bound at set 3 of the smooth terrain pipeline. The fragment shader samples it with triplanar projection and blends the result into the geometric normal from Phase 1.

**Tech Stack:** Rust (bytemuck, wgpu, image crate), GLSL 440, veloren common assets system. Normal maps are **generated procedurally at startup** using a self-contained value noise implementation — no external assets, no artistic work required. Each material category has distinct noise parameters (frequency, octaves, pattern) that make rock look rocky, grass look grassy, etc. All parameters are Rust constants, tunable by re-running the game.

---

## File Map

**Modify:**
- `common/src/terrain/block.rs` — add `normal_map_index()` to `BlockKind`
- `common/src/terrain/density.rs` — add `kinds: Vec<u8>` to `DensityField`, fill from volume
- `voxygen/src/mesh/transvoxel.rs` — add `kinds: [u8; 3]` to `TransvoxelTriangle`, fill in mesher
- `voxygen/src/render/pipelines/smooth_terrain.rs` — add `block_kind: u32` to vertex, add `NormalMapLayout` / `NormalMapBindGroup`, update pipeline layout to include set 3
- `voxygen/src/render/pipelines/mod.rs` — re-export new types
- `voxygen/src/render/mod.rs` — re-export `NormalMapBindGroup`
- `voxygen/src/mesh/terrain.rs` — pass `block_kind` when building `SmoothTerrainVertex`
- `voxygen/src/render/renderer/mod.rs` — add `terrain_normal_maps` texture array field, load at startup
- `voxygen/src/render/renderer/pipeline_creation.rs` — add `NormalMapLayout` to `Layouts`, pass to pipeline
- `voxygen/src/render/renderer/binding.rs` — add `bind_smooth_terrain_normal_maps()`
- `voxygen/src/render/renderer/drawer.rs` — `draw_smooth_terrain()` binds `NormalMapBindGroup` at set 3, `SmoothTerrainDrawer::draw()` signature unchanged
- `voxygen/src/scene/terrain/mod.rs` — store `NormalMapBindGroup`, pass to drawer
- `assets/voxygen/shaders/smooth-terrain-vert.glsl` — add `v_block_kind` input, `f_block_kind` output
- `assets/voxygen/shaders/smooth-terrain-frag.glsl` — add `t_terrain_normals` sampler, triplanar function, blend into `f_norm_n`

---

## Task 1: BlockKind::normal_map_index()

**Files:**
- Modify: `common/src/terrain/block.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block at the bottom of `common/src/terrain/block.rs`:

```rust
#[test]
fn normal_map_index_covers_all_kinds() {
    use strum::IntoEnumIterator;
    for kind in BlockKind::iter() {
        let idx = kind.normal_map_index();
        assert!(idx < 8, "BlockKind::{kind:?} returned out-of-range index {idx}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common normal_map_index 2>&1 | tail -5
```

Expected: `error[E0599]: no method named 'normal_map_index'`

- [ ] **Step 3: Implement the method**

Add after the `fn kind(&self) -> BlockKind` impl block in `common/src/terrain/block.rs` (around line 725, inside `impl Block`). This goes on `BlockKind` itself so add it in `impl BlockKind`:

```rust
impl BlockKind {
    // ... existing methods ...

    /// Index into the terrain normal map texture array (0-7).
    /// Layer 0 = rock (default for unrecognised kinds).
    pub fn normal_map_index(&self) -> u8 {
        match self {
            BlockKind::Rock
            | BlockKind::WeakRock
            | BlockKind::GlowingRock
            | BlockKind::GlowingWeakRock
            | BlockKind::Lava
            | BlockKind::Misc => 0,
            BlockKind::Grass => 1,
            BlockKind::Sand => 2,
            BlockKind::Snow | BlockKind::ArtSnow => 3,
            BlockKind::Earth => 4,
            BlockKind::Wood => 5,
            BlockKind::Ice => 6,
            BlockKind::Leaves | BlockKind::ArtLeaves | BlockKind::GlowingMushroom => 7,
            BlockKind::Air | BlockKind::Water => 0,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common normal_map_index 2>&1 | tail -5
```

Expected: `test result: ok. 1 passed`

- [ ] **Step 5: Commit**

```bash
git add common/src/terrain/block.rs
git commit -m "feat(phase3): add BlockKind::normal_map_index() mapping to 8 normal map layers"
```

---

## Task 2: Kind field in DensityField

**Files:**
- Modify: `common/src/terrain/density.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block in `common/src/terrain/density.rs`:

```rust
#[test]
fn density_field_kind_roundtrip() {
    let mut field = DensityField::new(Vec3::new(4, 4, 4));
    let pos = Vec3::new(2, 1, 3);
    field.set_kind(pos, 0x10); // Rock
    assert_eq!(field.get_kind(pos), 0x10);
}

#[test]
fn convert_chunk_fills_kinds() {
    // Verify that the filled-block path populates kinds with the block kind byte.
    // Use a minimal volume: a single layer of Rock at z=0.
    use crate::{terrain::Block, vol::ReadVol};
    use vek::Vec3;
    struct OneLayerVol;
    impl ReadVol for OneLayerVol {
        type Error = ();
        type Vox = Block;
        fn get(&self, pos: Vec3<i32>) -> Result<&Block, ()> {
            static ROCK: std::sync::OnceLock<Block> = std::sync::OnceLock::new();
            static AIR: std::sync::OnceLock<Block> = std::sync::OnceLock::new();
            let rock = ROCK.get_or_init(|| Block::new(BlockKind::Rock, Rgb::broadcast(128)));
            let air = AIR.get_or_init(|| Block::air(SpriteKind::Empty));
            if pos.z == 0 { Ok(rock) } else { Ok(air) }
        }
    }
    let field = convert_chunk_to_density_field(&OneLayerVol, Vec3::zero(), Vec3::new(4, 4, 4));
    // z=0 is solid Rock → kind should be 0x10 (BlockKind::Rock as u8)
    assert_eq!(field.get_kind(Vec3::new(1, 1, 0)), 0x10);
    // z=1 is air → kind should be 0
    assert_eq!(field.get_kind(Vec3::new(1, 1, 1)), 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common density_field_kind 2>&1 | tail -5
```

Expected: `error[E0599]: no method named 'set_kind'`

- [ ] **Step 3: Add `kinds` field to `DensityField`**

In `common/src/terrain/density.rs`, update the struct and impl:

```rust
pub struct DensityField {
    pub data: Vec<u8>,
    pub kinds: Vec<u8>, // BlockKind as u8 per voxel, parallel to data; 0 = air/unfilled
    pub size: Vec3<u32>,
}

impl DensityField {
    pub fn new(size: Vec3<u32>) -> Self {
        let n = (size.x * size.y * size.z) as usize;
        Self {
            data: vec![0u8; n],
            kinds: vec![0u8; n],
            size,
        }
    }

    // ... existing flat_index, get, get_or_zero, set ...

    pub fn set_kind(&mut self, pos: Vec3<i32>, kind: u8) {
        if let Some(i) = self.flat_index(pos) {
            self.kinds[i] = kind;
        }
    }

    pub fn get_kind(&self, pos: Vec3<i32>) -> u8 {
        self.flat_index(pos)
            .and_then(|i| self.kinds.get(i).copied())
            .unwrap_or(0)
    }

    pub fn get_kind_or_default(&self, pos: Vec3<i32>) -> u8 {
        self.get_kind(pos)
    }
}
```

- [ ] **Step 4: Update `convert_chunk_to_density_field` to fill kinds**

In the inner loop of `convert_chunk_to_density_field`:

```rust
let pos = Vec3::new(x, y, z);
let (val, kind_byte) = match vol.get(offset + pos) {
    Ok(block) if block.is_filled() => (255, block.kind() as u8),
    Ok(_) => (0, 0),
    Err(_) => (255, crate::terrain::block::BlockKind::Rock as u8),
};
field.set(pos, val);
field.set_kind(pos, kind_byte);
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common density 2>&1 | tail -10
```

Expected: all density tests pass.

- [ ] **Step 6: Compile check**

```bash
cargo check -p veloren-common 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add common/src/terrain/density.rs
git commit -m "feat(phase3): add kinds field to DensityField, fill from volume in convert_chunk"
```

---

## Task 3: Kind tracking in TransvoxelTriangle

**Files:**
- Modify: `voxygen/src/mesh/transvoxel.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block in `voxygen/src/mesh/transvoxel.rs`:

```rust
#[test]
fn transvoxel_vertex_kind_is_rock_for_rock_chunk() {
    use common::terrain::density::{DensityField, smooth_density_field};
    use vek::Vec3;

    let mut field = DensityField::new(Vec3::new(6, 6, 6));
    // Fill bottom 3 z-slices as solid Rock (kind = 0x10)
    for x in 0..6i32 {
        for y in 0..6i32 {
            for z in 0..3i32 {
                field.set(Vec3::new(x, y, z), 255);
                field.set_kind(Vec3::new(x, y, z), 0x10); // Rock
            }
        }
    }
    smooth_density_field(&mut field, 1);
    let tris = mesh_transvoxel(&field, THRESHOLD);
    assert!(!tris.is_empty(), "expected triangles at solid/air boundary");
    // All vertices should have Rock kind (0x10) or fallback (also 0x10)
    for tri in &tris {
        for &k in &tri.kinds {
            assert!(k == 0x10 || k == 0, "unexpected kind {k:#x}");
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen transvoxel_vertex_kind 2>&1 | tail -5
```

Expected: `error[E0609]: no field 'kinds' on type 'TransvoxelTriangle'`

- [ ] **Step 3: Add `kinds` to `TransvoxelTriangle`**

```rust
#[derive(Clone, Debug)]
pub struct TransvoxelTriangle {
    pub positions: [Vec3<f32>; 3],
    pub normals:   [Vec3<f32>; 3],
    pub kinds:     [u8; 3],        // BlockKind as u8 per vertex
}
```

- [ ] **Step 4: Add helper function**

Add above `mesh_transvoxel`:

```rust
/// Find the block kind at a fractional density-field position.
/// Samples the 8 surrounding integer voxels, returns the kind of the most
/// dense solid voxel among them. Falls back to Rock (0x10) if none are solid.
fn kind_at_vertex(field: &DensityField, pos: Vec3<f32>, threshold: u8) -> u8 {
    let base = pos.map(|e| e.floor() as i32);
    let mut best_kind = 0u8;
    let mut best_density = 0u8;
    for dz in 0..=1i32 {
        for dy in 0..=1i32 {
            for dx in 0..=1i32 {
                let p = base + Vec3::new(dx, dy, dz);
                let d = field.get_or_zero(p);
                if d > threshold && d > best_density {
                    best_density = d;
                    let k = field.get_kind_or_default(p);
                    if k != 0 {
                        best_kind = k;
                    }
                }
            }
        }
    }
    if best_kind == 0 {
        0x10 // Rock fallback
    } else {
        best_kind
    }
}
```

- [ ] **Step 5: Fill `kinds` in `mesh_transvoxel`**

In the section that pushes `TransvoxelTriangle`, after computing `vtx_pos` and `vtx_norm`, add kind computation:

```rust
let mut vtx_kinds = [0u8; 3]; // was not present before
for v in 0..vtx_count {
    vtx_kinds[v] = kind_at_vertex(field, vtx_pos[v], threshold);
}

// ...existing push:
triangles.push(TransvoxelTriangle {
    positions: [vtx_pos[tri[0]], vtx_pos[tri[1]], vtx_pos[tri[2]]],
    normals:   [vtx_norm[tri[0]], vtx_norm[tri[1]], vtx_norm[tri[2]]],
    kinds:     [vtx_kinds[tri[0]], vtx_kinds[tri[1]], vtx_kinds[tri[2]]],
});
```

- [ ] **Step 6: Run test to verify it passes**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen transvoxel 2>&1 | tail -10
```

Expected: all transvoxel tests pass.

- [ ] **Step 7: Commit**

```bash
git add voxygen/src/mesh/transvoxel.rs
git commit -m "feat(phase3): add per-vertex block kind to TransvoxelTriangle"
```

---

## Task 4: block_kind field in SmoothTerrainVertex

**Files:**
- Modify: `voxygen/src/render/pipelines/smooth_terrain.rs`
- Modify: `voxygen/src/mesh/terrain.rs`

- [ ] **Step 1: Add `block_kind: u32` to vertex struct**

In `voxygen/src/render/pipelines/smooth_terrain.rs`, update the struct:

```rust
#[repr(C)]
#[derive(Copy, Clone, Debug, Zeroable, Pod)]
pub struct SmoothTerrainVertex {
    pos:        [f32; 3], // 12 bytes: chunk-local float position
    norm:       u32,      // 4 bytes: 10-10-10-2 snorm packed normal
    col_light:  u32,      // 4 bytes: RGBA + light packed via make_col_light
    block_kind: u32,      // 4 bytes: BlockKind as u8, stored as u32 for GPU alignment
    // Total: 24 bytes
}
```

Update the `new` constructor:

```rust
impl SmoothTerrainVertex {
    pub fn new(pos: Vec3<f32>, norm: Vec3<f32>, col_light: u32, block_kind: u8) -> Self {
        Self {
            pos:        pos.into_array(),
            norm:       pack_norm_10_10_10_2(norm),
            col_light,
            block_kind: block_kind as u32,
        }
    }

    pub fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 4] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Uint32, 3 => Uint32];
        wgpu::VertexBufferLayout {
            array_stride: Self::STRIDE,
            step_mode:    wgpu::VertexStepMode::Vertex,
            attributes:   &ATTRIBUTES,
        }
    }
}
```

- [ ] **Step 2: Fix compilation error in terrain.rs**

In `voxygen/src/mesh/terrain.rs`, the `SmoothTerrainVertex::new` call now requires a 4th argument.

Find the block around line 371–375 where vertices are constructed:

```rust
// BEFORE:
SmoothTerrainVertex::new(p0 + mesh_delta, n0, col_light_for(p0)),
SmoothTerrainVertex::new(p1 + mesh_delta, n1, col_light_for(p1)),
SmoothTerrainVertex::new(p2 + mesh_delta, n2, col_light_for(p2)),
```

Change to:

```rust
// AFTER:
SmoothTerrainVertex::new(p0 + mesh_delta, n0, col_light_for(p0), tri.kinds[0]),
SmoothTerrainVertex::new(p1 + mesh_delta, n1, col_light_for(p1), tri.kinds[1]),
SmoothTerrainVertex::new(p2 + mesh_delta, n2, col_light_for(p2), tri.kinds[2]),
```

The `tri` variable already exists in the `for tri in &tris` loop.

- [ ] **Step 3: Compile check**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

Expected: no errors (the STRIDE constant auto-updates from `mem::size_of::<Self>()`).

- [ ] **Step 4: Commit**

```bash
git add voxygen/src/render/pipelines/smooth_terrain.rs voxygen/src/mesh/terrain.rs
git commit -m "feat(phase3): add block_kind u32 to SmoothTerrainVertex, wire from TransvoxelTriangle"
```

---

## Task 5: Procedural normal map generation

**Files:**
- Modify: `voxygen/src/render/renderer/mod.rs` (add noise infrastructure and material definitions)

All normal maps are generated at runtime by pure Rust code. No external files, no external tools.

**How height → normal works:**

Given a 256×256 float height field `h[x][y]`, compute the tangent-space normal at each pixel via central differences:

```
dh_dx = (h[x+1][y] - h[x-1][y]) / 2.0
dh_dy = (h[x][y+1] - h[x][y-1]) / 2.0
normal = normalize(vec3(-dh_dx * strength, -dh_dy * strength, 1.0))
encoded = (normal * 0.5 + 0.5) * 255   → RGBA bytes (A = height for parallax)
```

**Noise primitive (no external dependency):**

```rust
/// Deterministic hash → [0, 1] float. Used as a simple value noise primitive.
fn hash_f32(x: i32, y: i32, seed: u32) -> f32 {
    let h = (x as u32)
        .wrapping_mul(2246822519)
        .wrapping_add((y as u32).wrapping_mul(3266489917))
        .wrapping_add(seed);
    let h = h ^ (h >> 13);
    let h = h.wrapping_mul(1274126177);
    let h = h ^ (h >> 16);
    (h as f32) / (u32::MAX as f32)
}

/// Bilinear value noise in [0, 1].
fn value_noise(x: f64, y: f64, seed: u32) -> f64 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - x.floor();
    let yf = y - y.floor();
    // Smoothstep
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let a = hash_f32(xi,     yi,     seed) as f64;
    let b = hash_f32(xi + 1, yi,     seed) as f64;
    let c = hash_f32(xi,     yi + 1, seed) as f64;
    let d = hash_f32(xi + 1, yi + 1, seed) as f64;
    a + u * (b - a) + v * (c - a) + u * v * (a - b - c + d)
}

/// Fractal Brownian Motion — sum of `octaves` noise layers.
fn fbm(x: f64, y: f64, octaves: u32, seed: u32) -> f64 {
    let mut val = 0.0f64;
    let mut amp = 0.5f64;
    let mut freq = 1.0f64;
    for i in 0..octaves {
        val  += value_noise(x * freq, y * freq, seed.wrapping_add(i * 12345)) * amp;
        amp  *= 0.5;
        freq *= 2.0;
    }
    val  // range approximately [0, 1]
}
```

**Material parameters:**

```rust
struct MaterialNoise {
    octaves:   u32,
    frequency: f64,  // base spatial frequency (higher = smaller features)
    amplitude: f32,  // normal map "bumpiness" — strength of the effect
    seed:      u32,
}

// One entry per layer (same order as BlockKind::normal_map_index):
// 0=rock  1=grass  2=sand  3=snow  4=earth  5=wood  6=ice  7=leaves
const MATERIAL_NOISE: [MaterialNoise; 8] = [
    MaterialNoise { octaves: 5, frequency: 4.0,  amplitude: 2.0,  seed: 0xDEAD_BEEF }, // rock
    MaterialNoise { octaves: 4, frequency: 8.0,  amplitude: 0.8,  seed: 0x0BAD_F00D }, // grass
    MaterialNoise { octaves: 2, frequency: 3.0,  amplitude: 0.6,  seed: 0xCAFE_BABE }, // sand
    MaterialNoise { octaves: 3, frequency: 6.0,  amplitude: 0.5,  seed: 0x1337_C0DE }, // snow
    MaterialNoise { octaves: 4, frequency: 5.0,  amplitude: 1.2,  seed: 0xFEED_FACE }, // earth
    MaterialNoise { octaves: 3, frequency: 12.0, amplitude: 1.0,  seed: 0xABCD_1234 }, // wood
    MaterialNoise { octaves: 2, frequency: 2.0,  amplitude: 0.3,  seed: 0x4567_89AB }, // ice
    MaterialNoise { octaves: 4, frequency: 6.0,  amplitude: 0.9,  seed: 0xBEEF_DEAD }, // leaves
];
```

Sand gets a directional ripple warp on top of fbm (simulates wind ripples). Wood gets vertical grain lines.

- [ ] **Step 1: Write a test for height→normal encoding**

Add to the test section of `voxygen/src/render/renderer/mod.rs` (or a separate `normalmap_gen` module):

```rust
#[test]
fn height_to_normal_flat_gives_up_vector() {
    let h = vec![0.5f32; 256 * 256];
    let pix = height_to_normal_pixel(0.5, 0.5, 0.5, 0.5, 2.0);
    // Flat height field → normal = (0, 0, 1) → encoded as (127, 127, 255)
    assert_eq!(pix[0], 127);  // x ≈ 0
    assert_eq!(pix[1], 127);  // y ≈ 0
    assert!(pix[2] > 250);    // z ≈ 1
}
```

`height_to_normal_pixel(h_left, h_right, h_down, h_up, strength)` is the helper you'll define in Step 2.

- [ ] **Step 2: Run test to verify it fails**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen height_to_normal 2>&1 | tail -5
```

Expected: `error[E0425]: cannot find function 'height_to_normal_pixel'`

- [ ] **Step 3: Implement the noise and height→normal helpers**

Add to `voxygen/src/render/renderer/mod.rs` (before `load_terrain_normal_map_array`):

```rust
// ----- Procedural normal map generation (no external crate needed) -----------

fn hash_f32(x: i32, y: i32, seed: u32) -> f32 {
    let h = (x as u32)
        .wrapping_mul(2246822519)
        .wrapping_add((y as u32).wrapping_mul(3266489917))
        .wrapping_add(seed);
    let h = h ^ (h >> 13);
    let h = h.wrapping_mul(1274126177);
    let h = h ^ (h >> 16);
    (h as f32) / (u32::MAX as f32)
}

fn value_noise(x: f64, y: f64, seed: u32) -> f64 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - x.floor();
    let yf = y - y.floor();
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let a = hash_f32(xi,     yi,     seed) as f64;
    let b = hash_f32(xi + 1, yi,     seed) as f64;
    let c = hash_f32(xi,     yi + 1, seed) as f64;
    let d = hash_f32(xi + 1, yi + 1, seed) as f64;
    a + u * (b - a) + v * (c - a) + u * v * (a - b - c + d)
}

fn fbm(x: f64, y: f64, octaves: u32, seed: u32) -> f64 {
    let (mut val, mut amp, mut freq) = (0.0f64, 0.5f64, 1.0f64);
    for i in 0..octaves {
        val  += value_noise(x * freq, y * freq, seed.wrapping_add(i * 12345)) * amp;
        amp  *= 0.5;
        freq *= 2.0;
    }
    val
}

/// Encode four neighboring height values into a tangent-space normal RGBA pixel.
/// `h_l/r/d/u` = height at left/right/down/up neighbor (central difference).
/// `strength` scales the bumpiness.
pub(super) fn height_to_normal_pixel(h_l: f32, h_r: f32, h_d: f32, h_u: f32, strength: f32) -> [u8; 4] {
    let dx = (h_r - h_l) * strength;
    let dy = (h_u - h_d) * strength;
    let len = (dx * dx + dy * dy + 1.0).sqrt();
    let nx = (-dx / len * 0.5 + 0.5).clamp(0.0, 1.0);
    let ny = (-dy / len * 0.5 + 0.5).clamp(0.0, 1.0);
    let nz = (1.0  / len * 0.5 + 0.5).clamp(0.0, 1.0);
    [
        (nx * 255.0) as u8,
        (ny * 255.0) as u8,
        (nz * 255.0) as u8,
        255, // alpha reserved for future parallax height (opaque for now)
    ]
}

struct MaterialNoise {
    octaves:   u32,
    frequency: f64,
    amplitude: f32,
    seed:      u32,
}

const MATERIAL_NOISE: [MaterialNoise; 8] = [
    MaterialNoise { octaves: 5, frequency: 4.0,  amplitude: 2.0,  seed: 0xDEAD_BEEF }, // rock
    MaterialNoise { octaves: 4, frequency: 8.0,  amplitude: 0.8,  seed: 0x0BAD_F00D }, // grass
    MaterialNoise { octaves: 2, frequency: 3.0,  amplitude: 0.6,  seed: 0xCAFE_BABE }, // sand
    MaterialNoise { octaves: 3, frequency: 6.0,  amplitude: 0.5,  seed: 0x1337_C0DE }, // snow
    MaterialNoise { octaves: 4, frequency: 5.0,  amplitude: 1.2,  seed: 0xFEED_FACE }, // earth
    MaterialNoise { octaves: 3, frequency: 12.0, amplitude: 1.0,  seed: 0xABCD_1234 }, // wood
    MaterialNoise { octaves: 2, frequency: 2.0,  amplitude: 0.3,  seed: 0x4567_89AB }, // ice
    MaterialNoise { octaves: 4, frequency: 6.0,  amplitude: 0.9,  seed: 0xBEEF_DEAD }, // leaves
];

/// Generate one 256×256 RGBA layer of a terrain normal map for a given material.
/// Returns raw bytes (SIZE*SIZE*4), ready to upload to a wgpu texture layer.
fn generate_normal_map_layer(m: &MaterialNoise) -> Vec<u8> {
    const SIZE: usize = 256;
    let mut heights = vec![0.0f32; SIZE * SIZE];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = (x as f64 / SIZE as f64) * m.frequency;
            let fy = (y as f64 / SIZE as f64) * m.frequency;

            // Sand (layer 2, seed 0xCAFE_BABE): add directional ripple warp
            let h = if m.seed == 0xCAFE_BABE {
                let warp = value_noise(fx * 0.5, fy * 0.5, m.seed.wrapping_add(9999)) * 0.4;
                fbm(fx + warp, fy * 0.1, m.octaves, m.seed)
            // Wood (layer 5, seed 0xABCD_1234): vertical grain lines
            } else if m.seed == 0xABCD_1234 {
                let grain = (fx * 3.0 * std::f64::consts::TAU).sin() * 0.5 + 0.5;
                grain * 0.7 + fbm(fx, fy, m.octaves, m.seed) * 0.3
            } else {
                fbm(fx, fy, m.octaves, m.seed)
            };

            heights[y * SIZE + x] = h as f32;
        }
    }

    let mut pixels = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let idx = (y * SIZE + x) * 4;
            let h = |px: i32, py: i32| {
                let cx = px.rem_euclid(SIZE as i32) as usize;
                let cy = py.rem_euclid(SIZE as i32) as usize;
                heights[cy * SIZE + cx]
            };
            let pix = height_to_normal_pixel(
                h(x as i32 - 1, y as i32),
                h(x as i32 + 1, y as i32),
                h(x as i32, y as i32 - 1),
                h(x as i32, y as i32 + 1),
                m.amplitude,
            );
            pixels[idx..idx + 4].copy_from_slice(&pix);
        }
    }
    pixels
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen height_to_normal 2>&1 | tail -5
```

Expected: `test result: ok. 1 passed`

- [ ] **Step 5: Compile check**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

- [ ] **Step 6: Commit**

```bash
git add voxygen/src/render/renderer/mod.rs
git commit -m "feat(phase3): procedural terrain normal map generation (value noise FBM, no external assets)"
```

---

## Task 6: Normal map texture array in Renderer

**Files:**
- Modify: `voxygen/src/render/renderer/mod.rs`

- [ ] **Step 1: Add `create_terrain_normal_map_array()` using procedural layers**

Add this function after the `generate_normal_map_layer` function from Task 5:

```rust
/// Create the 8-layer wgpu texture array for terrain normal maps.
/// All layers are generated procedurally — no asset files needed.
fn create_terrain_normal_map_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Texture {
    const LAYER_COUNT: u32 = 8;
    const SIZE: u32 = 256;

    let tex_info = wgpu::TextureDescriptor {
        label: Some("terrain_normal_map_array"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: LAYER_COUNT,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm, // NOT sRGB — normals are linear data
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    let view_info = wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        array_layer_count: Some(LAYER_COUNT),
        ..Default::default()
    };
    let sampler_info = wgpu::SamplerDescriptor {
        label: Some("terrain_normal_map_sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    };

    let texture = Texture::new_raw(device, &tex_info, &view_info, &sampler_info);

    for (layer, m) in MATERIAL_NOISE.iter().enumerate() {
        let pixel_data = generate_normal_map_layer(m);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                aspect: wgpu::TextureAspect::All,
            },
            &pixel_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
            wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        );
    }

    texture
}
```

- [ ] **Step 2: Add `terrain_normal_maps` field to `Renderer`**

Find the `pub struct Renderer` definition in `voxygen/src/render/renderer/mod.rs` and add the field:

```rust
pub struct Renderer {
    // ... existing fields ...
    noise_tex: Texture,
    terrain_normal_maps: Texture, // NEW: 8-layer array for Phase 3 normal maps
    // ... rest of fields ...
}
```

- [ ] **Step 3: Initialize `terrain_normal_maps` in `Renderer::new()`**

Find where `noise_tex` is created (around line 530) and add right after:

```rust
let terrain_normal_maps = create_terrain_normal_map_array(&device, &queue);
```

Then add `terrain_normal_maps` to the struct literal that constructs `Renderer`:

```rust
Renderer {
    // ...
    noise_tex,
    terrain_normal_maps,   // NEW
    // ...
}
```

- [ ] **Step 4: Expose a getter**

Add a public method to `impl Renderer`:

```rust
pub fn terrain_normal_maps(&self) -> &Texture { &self.terrain_normal_maps }
```

- [ ] **Step 5: Compile check**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add voxygen/src/render/renderer/mod.rs
git commit -m "feat(phase3): create procedural terrain normal map texture array in Renderer"
```

---

## Task 7: NormalMapLayout and NormalMapBindGroup

**Files:**
- Modify: `voxygen/src/render/pipelines/smooth_terrain.rs`
- Modify: `voxygen/src/render/pipelines/mod.rs`
- Modify: `voxygen/src/render/mod.rs`

- [ ] **Step 1: Add layout and bind group types**

At the end of `voxygen/src/render/pipelines/smooth_terrain.rs`, after `SmoothTerrainPipeline`, add:

```rust
/// Bind group layout for the terrain normal map texture array (set 3).
pub struct NormalMapLayout {
    pub layout: wgpu::BindGroupLayout,
}

impl NormalMapLayout {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smooth_terrain_normal_map_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        Self { layout }
    }

    pub fn bind(&self, device: &wgpu::Device, texture: &super::super::texture::Texture) -> NormalMapBindGroup {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smooth_terrain_normal_map_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&texture.sampler),
                },
            ],
        });
        NormalMapBindGroup { bind_group }
    }
}

pub struct NormalMapBindGroup {
    pub bind_group: wgpu::BindGroup,
}
```

- [ ] **Step 2: Update `SmoothTerrainPipeline::new()` to include set 3**

In the `pipeline_layout` creation inside `SmoothTerrainPipeline::new()`, add the `NormalMapLayout` parameter and include it at set 3.

Change the function signature from:
```rust
pub fn new(
    device: &wgpu::Device,
    vs_module: &wgpu::ShaderModule,
    fs_module: &wgpu::ShaderModule,
    global_layout: &GlobalsLayouts,
    terrain_layout: &TerrainLayout,
    aa_mode: AaMode,
    format: wgpu::TextureFormat,
) -> Self {
```

to:
```rust
pub fn new(
    device: &wgpu::Device,
    vs_module: &wgpu::ShaderModule,
    fs_module: &wgpu::ShaderModule,
    global_layout: &GlobalsLayouts,
    terrain_layout: &TerrainLayout,
    normal_map_layout: &NormalMapLayout,
    aa_mode: AaMode,
    format: wgpu::TextureFormat,
) -> Self {
```

And update the `pipeline_layout` descriptor:
```rust
let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
    label: Some("Smooth terrain pipeline layout"),
    push_constant_ranges: &[],
    bind_group_layouts: &[
        &global_layout.globals,          // set 0
        &global_layout.shadow_textures,  // set 1
        &terrain_layout.locals,          // set 2
        &normal_map_layout.layout,       // set 3  ← NEW
    ],
});
```

- [ ] **Step 3: Re-export new types**

In `voxygen/src/render/pipelines/mod.rs`, add to the smooth_terrain re-exports:

```rust
pub use smooth_terrain::{
    NormalMapBindGroup, NormalMapLayout,
    SmoothTerrainPipeline, SmoothTerrainVertex,
};
```

In `voxygen/src/render/mod.rs`, add to the public re-exports:

```rust
pub use pipelines::{NormalMapBindGroup, NormalMapLayout, /* ... existing ... */};
```

- [ ] **Step 4: Compile check (will fail at pipeline_creation.rs call site)**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -20
```

Expected: errors only in `pipeline_creation.rs` where `SmoothTerrainPipeline::new` is called (wrong number of args). That's fixed in Task 8.

- [ ] **Step 5: Commit (as WIP — won't fully compile until Task 8)**

```bash
git add voxygen/src/render/pipelines/smooth_terrain.rs \
        voxygen/src/render/pipelines/mod.rs \
        voxygen/src/render/mod.rs
git commit -m "feat(phase3): add NormalMapLayout + NormalMapBindGroup to smooth terrain pipeline"
```

---

## Task 8: Wire normal map layout through pipeline_creation

**Files:**
- Modify: `voxygen/src/render/renderer/pipeline_creation.rs`

- [ ] **Step 1: Add `NormalMapLayout` to `Layouts`**

Find the `pub struct Layouts` definition in `pipeline_creation.rs` and add:

```rust
pub struct Layouts {
    // ... existing layouts ...
    pub smooth_terrain_normal_map: NormalMapLayout,
}
```

- [ ] **Step 2: Initialize it in the `Layouts::new()` function**

Find where layouts are created and add:

```rust
let smooth_terrain_normal_map = NormalMapLayout::new(device);
```

Add to the struct literal:

```rust
Layouts {
    // ... existing ...
    smooth_terrain_normal_map,
}
```

- [ ] **Step 3: Pass to `SmoothTerrainPipeline::new()`**

Find the call to `SmoothTerrainPipeline::new(` (around line 606) and add `&needs.layouts.smooth_terrain_normal_map`:

```rust
smooth_terrain::SmoothTerrainPipeline::new(
    needs.device,
    &needs.shaders.smooth_terrain_vert,
    &needs.shaders.smooth_terrain_frag,
    &needs.layouts.global,
    &needs.layouts.terrain,
    &needs.layouts.smooth_terrain_normal_map,  // NEW
    needs.pipeline_modes.aa,
    needs.surface_config.format,
)
```

- [ ] **Step 4: Compile check — should now be clean**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/render/renderer/pipeline_creation.rs
git commit -m "feat(phase3): wire NormalMapLayout into Layouts and SmoothTerrainPipeline::new"
```

---

## Task 9: Renderer creates and exposes NormalMapBindGroup

**Files:**
- Modify: `voxygen/src/render/renderer/binding.rs`

- [ ] **Step 1: Add `bind_smooth_terrain_normal_maps()` to Renderer**

In `voxygen/src/render/renderer/binding.rs`, add:

```rust
pub fn bind_smooth_terrain_normal_maps(&self) -> NormalMapBindGroup {
    self.layouts
        .smooth_terrain_normal_map
        .bind(&self.device, &self.terrain_normal_maps)
}
```

The import for `NormalMapBindGroup` is already available through the existing pipelines import at the top of binding.rs.

- [ ] **Step 2: Compile check**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add voxygen/src/render/renderer/binding.rs
git commit -m "feat(phase3): add bind_smooth_terrain_normal_maps() to Renderer"
```

---

## Task 10: Drawer binds NormalMapBindGroup at set 3

**Files:**
- Modify: `voxygen/src/render/renderer/drawer.rs`

- [ ] **Step 1: Update `draw_smooth_terrain()` to accept and bind normal maps**

Find `draw_smooth_terrain()` in `voxygen/src/render/renderer/drawer.rs` (around line 1084):

```rust
// BEFORE:
pub fn draw_smooth_terrain(&mut self) -> SmoothTerrainDrawer<'_, 'pass> {
    let mut render_pass = self.render_pass.scope("smooth terrain");
    render_pass.set_pipeline(&self.pipelines.smooth_terrain.pipeline);
    SmoothTerrainDrawer { render_pass }
}
```

Change to:

```rust
pub fn draw_smooth_terrain<'data: 'pass>(
    &mut self,
    normal_maps: &'data smooth_terrain::NormalMapBindGroup,
) -> SmoothTerrainDrawer<'_, 'pass> {
    let mut render_pass = self.render_pass.scope("smooth terrain");
    render_pass.set_pipeline(&self.pipelines.smooth_terrain.pipeline);
    render_pass.set_bind_group(3, &normal_maps.bind_group, &[]);
    SmoothTerrainDrawer { render_pass }
}
```

The `SmoothTerrainDrawer::draw()` inner method does NOT change — it still only sets bind groups 2 and the vertex buffer.

- [ ] **Step 2: Fix the call site in scene/terrain/mod.rs**

Find the call `drawer.draw_smooth_terrain()` in `voxygen/src/scene/terrain/mod.rs` (around line 1663) — it will now fail to compile. This is fixed in Task 11.

- [ ] **Step 3: Commit as WIP**

```bash
git add voxygen/src/render/renderer/drawer.rs
git commit -m "feat(phase3): draw_smooth_terrain() binds normal map array at set 3"
```

---

## Task 11: Scene terrain creates and passes NormalMapBindGroup

**Files:**
- Modify: `voxygen/src/scene/terrain/mod.rs`

- [ ] **Step 1: Add `NormalMapBindGroup` field to `Terrain`**

Find `pub struct Terrain` in `voxygen/src/scene/terrain/mod.rs` and add:

```rust
pub struct Terrain<V: RectRasterableVol> {
    // ... existing fields ...
    normal_map_bind_group: NormalMapBindGroup,
}
```

- [ ] **Step 2: Initialize in `Terrain::new()`**

Find `Terrain::new()` and add the bind group creation using the renderer:

```rust
let normal_map_bind_group = renderer.bind_smooth_terrain_normal_maps();
```

Add to the struct initialization:

```rust
Terrain {
    // ... existing ...
    normal_map_bind_group,
}
```

The `renderer` parameter is already available in `Terrain::new()`.

- [ ] **Step 3: Pass to `draw_smooth_terrain()` in `render_smooth()`**

Find `render_smooth()` (around line 1661):

```rust
// BEFORE:
let mut drawer = drawer.draw_smooth_terrain();

// AFTER:
let mut drawer = drawer.draw_smooth_terrain(&self.normal_map_bind_group);
```

- [ ] **Step 4: Full compile check**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/scene/terrain/mod.rs
git commit -m "feat(phase3): Terrain stores NormalMapBindGroup, passes to draw_smooth_terrain"
```

---

## Task 12: Vertex shader — pass block_kind through

**Files:**
- Modify: `assets/voxygen/shaders/smooth-terrain-vert.glsl`

- [ ] **Step 1: Add input and output for block_kind**

The vertex shader currently has 3 inputs (locations 0-2) and 3 outputs (locations 0-2). Add location 3:

```glsl
// After existing inputs:
layout(location = 3) in uint v_block_kind;   // BlockKind as u8, stored as u32

// After existing outputs:
layout(location = 3) flat out uint f_block_kind;
```

At the end of `main()`, add:

```glsl
f_block_kind = v_block_kind;
```

Full updated vert shader (only the changed sections shown):

```glsl
// Add after line 25 (after `layout(location = 2) in uint v_col_light;`):
layout(location = 3) in uint v_block_kind;

// Add after line 36 (after `layout(location = 2) out vec3 f_norm;`):
layout(location = 3) flat out uint f_block_kind;

// Add at end of main(), before closing brace:
f_block_kind = v_block_kind;
```

- [ ] **Step 2: Verify GLSL compiles (check at runtime or via naga)**

```bash
cargo run --bin veloren-voxygen 2>&1 | grep -i "shader\|error\|compile" | head -20
```

Expected: game starts without shader compilation errors. (The frag shader will fail until Task 13 adds the matching input — keep note of this.)

- [ ] **Step 3: Commit**

```bash
git add assets/voxygen/shaders/smooth-terrain-vert.glsl
git commit -m "feat(phase3): smooth-terrain-vert passes block_kind (location 3) to fragment shader"
```

---

## Task 13: Fragment shader — triplanar normal mapping

**Files:**
- Modify: `assets/voxygen/shaders/smooth-terrain-frag.glsl`

- [ ] **Step 1: Add input, sampler declarations, and helper function**

After the existing layout declarations (after line 32, before `#include <sky.glsl>`), add:

```glsl
// Normal map texture array — 8 layers, one per terrain material category.
// Layer indices: 0=rock, 1=grass, 2=sand, 3=snow, 4=earth, 5=wood, 6=ice, 7=leaves
layout(set = 3, binding = 0) uniform texture2DArray t_terrain_normals;
layout(set = 3, binding = 1) uniform sampler s_terrain_normals;

layout(location = 3) flat in uint f_block_kind;
```

- [ ] **Step 2: Add the triplanar helper function**

Add this function before `void main()`:

```glsl
// Triplanar normal map sampling.
// Samples the normal map from three orthogonal projections and blends them
// by the absolute value of the geometric normal components.
// world_pos: fragment world position (f_pos)
// geom_norm: geometric normal from Transvoxel (face_norm, already normalized)
// layer: texture array layer index (f_block_kind)
// scale: UV tiling frequency (1.0/4.0 = tile every 4 world units)
// Returns: perturbed normal in world space, not yet normalized.
vec3 triplanar_normal(vec3 world_pos, vec3 geom_norm, float layer, float scale) {
    // UV coordinates for each projection axis
    vec2 uv_x = fract(world_pos.yz * scale);
    vec2 uv_y = fract(world_pos.xz * scale);
    vec2 uv_z = fract(world_pos.xy * scale);

    // Sample normal maps
    vec3 tx = textureLod(sampler2DArray(t_terrain_normals, s_terrain_normals), vec3(uv_x, layer), 0.0).rgb;
    vec3 ty = textureLod(sampler2DArray(t_terrain_normals, s_terrain_normals), vec3(uv_y, layer), 0.0).rgb;
    vec3 tz = textureLod(sampler2DArray(t_terrain_normals, s_terrain_normals), vec3(uv_z, layer), 0.0).rgb;

    // Decode from [0,1] to [-1,1] tangent-space normals
    vec3 n_x = tx * 2.0 - 1.0;
    vec3 n_y = ty * 2.0 - 1.0;
    vec3 n_z = tz * 2.0 - 1.0;

    // Swizzle tangent-space normals to world space per projection:
    // X-facing surface: TS-X→WS-Z, TS-Y→WS-Y, TS-Z→WS-X
    n_x = vec3(n_x.z, n_x.y, n_x.x);
    // Y-facing surface: TS-X→WS-X, TS-Y→WS-Z, TS-Z→WS-Y
    n_y = vec3(n_y.x, n_y.z, n_y.y);
    // Z-facing surface: tangent space maps directly (X→X, Y→Y, Z→Z)
    // n_z stays as-is

    // Blend weights from absolute normal components, with sharpening
    vec3 w = pow(abs(geom_norm), vec3(4.0));
    w /= (w.x + w.y + w.z + 0.001);

    return n_x * w.x + n_y * w.y + n_z * w.z;
}
```

- [ ] **Step 3: Apply the normal map in `main()`**

Find the line in `main()` that sets `f_norm_n`:

```glsl
// BEFORE (around line 65–66):
vec3 face_norm = normalize(f_norm);
vec3 f_norm_n  = face_norm;
```

Change to:

```glsl
vec3 face_norm = normalize(f_norm);

// Triplanar normal map perturbation
const float NORMAL_MAP_SCALE    = 1.0 / 4.0; // tile every 4 world units
const float NORMAL_MAP_STRENGTH = 0.4;        // blend factor (0 = pure geometry, 1 = full detail)
vec3 detail = triplanar_normal(f_pos, face_norm, float(f_block_kind), NORMAL_MAP_SCALE);
vec3 f_norm_n = normalize(face_norm + detail * NORMAL_MAP_STRENGTH);
```

- [ ] **Step 4: Run the game and verify normal maps appear**

```bash
cargo run --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose" \
  2>&1 | grep -i "error\|warn\|normal" | head -20
```

Walk around terrain — the procedural normals from Task 5 are already distinct per material, so you should immediately see subtle surface roughness variations: rock looks faceted, grass looks soft, sand looks rippled.

- [ ] **Step 5: Commit**

```bash
git add assets/voxygen/shaders/smooth-terrain-frag.glsl
git commit -m "feat(phase3): triplanar normal mapping in smooth-terrain-frag"
```

---

## Task 14: Tune procedural noise parameters

**Files:**
- Modify: `voxygen/src/render/renderer/mod.rs` (adjust constants in `MATERIAL_NOISE`)

The `MATERIAL_NOISE` constants in Task 5 are starting points. After seeing the game running (Task 13), tune them by changing constants and restarting — no recompilation of shaders needed, just re-run.

- [ ] **Step 1: Run the game and evaluate each material visually**

```bash
cargo run --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose"
```

Stand on each terrain type and note if the surface detail feels right:
- Rock: should look rough and irregular — if too smooth, increase `octaves` or `amplitude`
- Grass: should feel organic and gentle — if too harsh, reduce `amplitude`
- Sand: should show ripple/wave pattern — the directional warp in `generate_normal_map_layer` handles this
- Snow: should feel soft, low bumps — keep `amplitude` low
- Wood: grain lines should be visible when looking at wooden structures

- [ ] **Step 2: Tune `MATERIAL_NOISE` constants**

Adjust in `voxygen/src/render/renderer/mod.rs`. Reference:

| Parameter | Effect |
|-----------|--------|
| `octaves` | More octaves = finer detail layered on top of coarse bumps. 3–5 is typical. |
| `frequency` | Higher = smaller, tighter features. Lower = larger, smoother shapes. |
| `amplitude` | Direct multiplier on bumpiness. Doubles the `NORMAL_MAP_STRENGTH` impact. |

Also tune `NORMAL_MAP_SCALE` and `NORMAL_MAP_STRENGTH` in `smooth-terrain-frag.glsl` if the overall effect is too strong or too weak across all materials.

- [ ] **Step 3: Commit tuned parameters**

```bash
git add voxygen/src/render/renderer/mod.rs \
        assets/voxygen/shaders/smooth-terrain-frag.glsl
git commit -m "tune(phase3): adjust procedural normal map noise parameters for each material"
```

---

## Task 15: Parallax mapping (Ultra only)

**Files:**
- Modify: `voxygen/src/render/mod.rs` (add `terrain_smoothing` to `PipelineModes`)
- Modify: `voxygen/src/render/renderer/pipeline_creation.rs` (inject `TERRAIN_SMOOTHING_ULTRA` define)
- Modify: `assets/voxygen/shaders/smooth-terrain-frag.glsl` (add `#ifdef TERRAIN_SMOOTHING_ULTRA` block)

> **Prerequisite:** Tasks 1–13 complete and normal maps visible in-game. Task 14 (tuning) can run in parallel.

- [ ] **Step 1: Add `terrain_smoothing` to `PipelineModes`**

In `voxygen/src/render/mod.rs`, update `PipelineModes`:

```rust
pub struct PipelineModes {
    // ... existing fields ...
    pub terrain_smoothing: TerrainSmoothingMode,
}
```

Update `RenderMode::split()` to include it:

```rust
PipelineModes {
    // ... existing ...
    terrain_smoothing: self.terrain_smoothing,
}
```

- [ ] **Step 2: Inject shader constant in pipeline_creation**

In the `constants` string building in `ShaderModules::new()`, add after the `SHADOW_MODE` define:

```rust
match pipeline_modes.terrain_smoothing {
    TerrainSmoothingMode::Ultra => constants += "#define TERRAIN_SMOOTHING_ULTRA\n",
    _ => {},
}
```

- [ ] **Step 3: Add parallax block to fragment shader**

In `smooth-terrain-frag.glsl`, update the `triplanar_normal` call in `main()`:

```glsl
// BEFORE:
vec3 detail = triplanar_normal(f_pos, face_norm, float(f_block_kind), NORMAL_MAP_SCALE);

// AFTER:
#ifdef TERRAIN_SMOOTHING_ULTRA
// Parallax offset: shift UV by view direction projected onto surface, scaled
// by height from normal map alpha. Only the Z-projection is parallaxed
// (dominant on flat terrain); X/Y projections use standard triplanar.
const float PARALLAX_SCALE = 0.04;
vec2 par_uv = fract(f_pos.xy * NORMAL_MAP_SCALE);
float height = textureLod(
    sampler2DArray(t_terrain_normals, s_terrain_normals),
    vec3(par_uv, float(f_block_kind)), 0.0
).a;
// Project view direction to tangent space (approximate for near-flat terrain)
vec2 view_ts = vec2(dot(cam_to_frag, vec3(1, 0, 0)),
                    dot(cam_to_frag, vec3(0, 1, 0)));
vec2 par_offset = view_ts * (height - 0.5) * PARALLAX_SCALE;
vec2 par_uv_z = fract((f_pos.xy + par_offset) * NORMAL_MAP_SCALE);
// Re-sample with parallax offset for the Z projection
vec3 tz_par = textureLod(
    sampler2DArray(t_terrain_normals, s_terrain_normals),
    vec3(par_uv_z, float(f_block_kind)), 0.0
).rgb;
vec3 n_z_par = tz_par * 2.0 - 1.0;
// Temporarily override the Z component for the parallax sample;
// use the same triplanar_normal function but replace n_z externally.
// Simplification: call triplanar_normal normally, then blend parallaxed Z in.
vec3 detail_no_par = triplanar_normal(f_pos, face_norm, float(f_block_kind), NORMAL_MAP_SCALE);
vec3 w_abs = pow(abs(face_norm), vec3(4.0));
float w_z = w_abs.z / (w_abs.x + w_abs.y + w_abs.z + 0.001);
vec3 detail = detail_no_par + (n_z_par - detail_no_par) * w_z;
#else
vec3 detail = triplanar_normal(f_pos, face_norm, float(f_block_kind), NORMAL_MAP_SCALE);
#endif
```

- [ ] **Step 4: Handle `PipelineModes` PartialEq — pipeline recreates on Ultra toggle**

The existing `#[derive(PartialEq)]` on `PipelineModes` means changing between `Smooth` and `Ultra` now triggers shader recompilation (same as changing cloud mode). This is correct behavior — accept the brief recompile when switching quality presets.

Verify by running the game, changing terrain smoothing from Smooth to Ultra in settings, and confirming shaders recompile (the game should briefly freeze, then show improved parallax on flat surfaces).

- [ ] **Step 5: Compile and run check**

```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

Expected: no errors.

```bash
cargo run --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose"
```

Set terrain smoothing to Ultra. Stand on flat rock terrain close to the surface, move sideways — parallax creates subtle depth impression in the rock texture.

- [ ] **Step 6: Commit**

```bash
git add voxygen/src/render/mod.rs \
        voxygen/src/render/renderer/pipeline_creation.rs \
        assets/voxygen/shaders/smooth-terrain-frag.glsl
git commit -m "feat(phase3): parallax mapping for Ultra terrain smoothing tier"
```

---

## Task 16: Lint, test, and final verification

- [ ] **Step 1: Run full test suite**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-voxygen 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings 2>&1 | grep "^error" | head -20
```

Fix any warnings. Common ones to watch for:
- `unused import` if `NormalMapBindGroup` re-exports are incomplete
- `dead_code` on the `kinds` field if tests don't exercise it at crate level

- [ ] **Step 3: Visual verification checklist**

Run the game:
```bash
cargo run --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose"
```

Check:
- [ ] Disabled mode: terrain looks identical to before (greedy mesher, no smooth pipeline)
- [ ] Soft/Smooth mode: normal maps visible as subtle surface detail
- [ ] Ultra mode: parallax adds depth to flat surfaces at close range
- [ ] No artifacts at chunk seams (smooth chunks don't have wrong normal map alignment)
- [ ] FPS within expected range for each tier on reference hardware

- [ ] **Step 4: Update spec tracking table**

In `docs/superpowers/specs/2026-06-04-terrain-resolution-design.md`, update the progress table:

```markdown
| Fase 3 — Normal maps | ✅ Completa | Triplanar + parallax Ultra — HEAD: `<git hash>` |
```

- [ ] **Step 5: Final commit**

```bash
git add docs/superpowers/specs/2026-06-04-terrain-resolution-design.md
git commit -m "docs: mark Phase 3 complete in terrain resolution spec"
```

---

## Self-Review Notes

1. **DensityField size**: Adding `kinds: Vec<u8>` doubles the memory per density field. A 34×34×514-voxel padded chunk field (Fase 2 hires) = ~592 KB → 1.18 MB. This is acceptable for in-flight meshing (fields are transient, not stored per chunk).

2. **Texture format Rgba8Unorm vs Rgba8UnormSrgb**: Normal maps MUST use `Rgba8Unorm` (not sRGB). The existing `Texture::new()` forces sRGB for Rgba8 images — that's why Task 6 uses `Texture::new_raw()` with explicit `Rgba8Unorm`. If this is accidentally changed to sRGB, normals will appear washed out / incorrect.

3. **set 3 binding requirement**: All draw calls using `SmoothTerrainPipeline` MUST bind set 3. If `render_smooth()` calls `draw_smooth_terrain()` without the bind group, wgpu will panic. Task 11 stores the bind group in `Terrain` to guarantee it's always available.

4. **Procedural normals are immediately visible**: The FBM noise from Task 5 produces distinct non-flat normals for each material from day one. No external assets required. Task 14 is purely about tuning the feel.

5. **Vertex size increase**: `SmoothTerrainVertex` grows from 20 to 24 bytes. For a large chunk (~10k triangles) that's 240 KB vs 200 KB — 20% more GPU vertex buffer memory per smooth chunk. Acceptable.
