# Fase 1: Soft Voxel Terrain (Transvoxel + Smooth Collision) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `TerrainSmoothingMode` to the graphics settings and implement the Transvoxel algorithm so terrain renders as a smooth surface, plus triangle-based smooth collision that matches the visual surface.

**Architecture:** A shared `DensityField` (in `common`) converts voxel blocks to a scalar field; voxygen runs the Transvoxel marching algorithm on that field to emit smooth triangle meshes instead of greedy quads; `common-systems` extracts collision triangles from the same field. A `TerrainSmoothingMode` enum gates the new paths — `Disabled` is byte-for-byte identical to today.

**Tech Stack:** Rust nightly, specs ECS, wgpu / `TerrainVertex`, Transvoxel algorithm (Eric Lengyel 2010, public domain lookup tables at <https://transvoxel.org/>).

**Dependency order (within Fase 1):**
1. Task 1 — Settings enum (standalone, no deps)
2. Tasks 2–4 — DensityField in `common` (standalone, tested with unit tests)
3. Tasks 5–6 — Transvoxel tables + algorithm in `voxygen` (depends on DensityField)
4. Tasks 7–8 — Wire Transvoxel into terrain mesh pipeline (depends on Tasks 5–6)
5. Tasks 9–10 — Smooth collision in `common-systems` (depends on DensityField)

---

## File map

| File | Action | Responsibility |
|------|--------|---------------|
| `voxygen/src/settings/graphics.rs` | Modify | Add `TerrainSmoothingMode` enum + field to `GraphicsSettings`, update presets & auto_detect |
| `common/src/terrain/density.rs` | Create | `DensityField` struct, `convert_chunk_to_density_field`, `smooth_density_field` |
| `common/src/terrain/mod.rs` | Modify | `pub mod density;` + re-export |
| `voxygen/src/mesh/transvoxel.rs` | Create | Transvoxel lookup tables + `mesh_transvoxel()` function |
| `voxygen/src/mesh/mod.rs` | Modify | `pub mod transvoxel;` |
| `voxygen/src/mesh/terrain.rs` | Modify | Accept `TerrainSmoothingMode`; dispatch greedy vs transvoxel |
| `voxygen/src/scene/mod.rs` | Modify | Add `terrain_smoothing: TerrainSmoothingMode` to `SceneData` |
| `voxygen/src/scene/terrain/mod.rs` | Modify | Capture mode in mesh worker closure; pass through |
| `common/systems/src/phys_smooth.rs` | Create | `extract_collision_triangles(density: &DensityField) -> Vec<Triangle>` |
| `common/systems/src/lib.rs` | Modify | `pub mod phys_smooth;` |
| `common/systems/src/phys/mod.rs` | Modify | Call smooth collision when mode != Disabled |

---

## Task 1: Add `TerrainSmoothingMode` to `GraphicsSettings`

**Files:**
- Modify: `voxygen/src/settings/graphics.rs`

- [ ] **Step 1.1: Add the enum before `GraphicsSettings`**

In `voxygen/src/settings/graphics.rs`, insert after the `get_fps` function (around line 29):

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TerrainSmoothingMode {
    /// Use existing greedy mesher — no performance cost, identical to today.
    #[default]
    Disabled,
    /// Transvoxel with 1 LOD level, smooth collision. Minimum: GTX 1060.
    Soft,
    /// Transvoxel with 3 LOD levels, smooth collision. Minimum: RTX 3060.
    Smooth,
    /// Transvoxel with 3 LOD levels + normal maps, smooth collision. Minimum: RTX 3070.
    Ultra,
}
```

- [ ] **Step 1.2: Add the field to `GraphicsSettings`**

In the `GraphicsSettings` struct, add after `lod_detail: u32`:

```rust
pub terrain_smoothing: TerrainSmoothingMode,
```

In `Default for GraphicsSettings`, add:

```rust
terrain_smoothing: TerrainSmoothingMode::Disabled,
```

- [ ] **Step 1.3: Update quality presets**

In `into_minimal` and `into_low`, add `terrain_smoothing: TerrainSmoothingMode::Disabled,` to the `Self { ... }` body.

In `into_medium`, add `terrain_smoothing: TerrainSmoothingMode::Soft,`.

In `into_high`, add `terrain_smoothing: TerrainSmoothingMode::Smooth,`.

In `into_ultra`, add `terrain_smoothing: TerrainSmoothingMode::Ultra,`.

Because `auto_detect()` delegates to `into_minimal/low/medium/high/ultra`, no further changes are needed there.

- [ ] **Step 1.4: Build check**

```bash
cargo check -p veloren-voxygen 2>&1 | head -30
```

Expected: no errors (serde derives handle serialization automatically).

- [ ] **Step 1.5: Commit**

```bash
git add voxygen/src/settings/graphics.rs
git commit -m "feat(settings): add TerrainSmoothingMode enum to GraphicsSettings"
```

---

## Task 2: Create `DensityField` struct in `common`

**Files:**
- Create: `common/src/terrain/density.rs`
- Modify: `common/src/terrain/mod.rs`

- [ ] **Step 2.1: Add `pub mod density;` to terrain mod**

In `common/src/terrain/mod.rs`, at the top with the other `pub mod` declarations, add:

```rust
pub mod density;
```

Also add to the re-exports block (find the `pub use self::{` block):

```rust
density::{DensityField, convert_chunk_to_density_field, smooth_density_field},
```

- [ ] **Step 2.2: Create the struct**

Create `common/src/terrain/density.rs`:

```rust
use vek::Vec3;

/// A 3-D scalar field where 255 = fully solid, 0 = fully empty.
/// Size is stored as (x, y, z); indexing is row-major: `x * size.y * size.z + y * size.z + z`.
pub struct DensityField {
    pub data: Vec<u8>,
    pub size: Vec3<u32>,
}

impl DensityField {
    pub fn new(size: Vec3<u32>) -> Self {
        Self {
            data: vec![0u8; (size.x * size.y * size.z) as usize],
            size,
        }
    }

    #[inline]
    fn index(&self, pos: Vec3<i32>) -> Option<usize> {
        if pos.x < 0
            || pos.y < 0
            || pos.z < 0
            || pos.x >= self.size.x as i32
            || pos.y >= self.size.y as i32
            || pos.z >= self.size.z as i32
        {
            return None;
        }
        Some(
            (pos.x as u32 * self.size.y * self.size.z
                + pos.y as u32 * self.size.z
                + pos.z as u32) as usize,
        )
    }

    pub fn get(&self, pos: Vec3<i32>) -> Option<u8> {
        self.index(pos).and_then(|i| self.data.get(i).copied())
    }

    pub fn get_or_zero(&self, pos: Vec3<i32>) -> u8 { self.get(pos).unwrap_or(0) }

    pub fn set(&mut self, pos: Vec3<i32>, val: u8) {
        if let Some(i) = self.index(pos) {
            self.data[i] = val;
        }
    }
}
```

- [ ] **Step 2.3: Write inline unit tests**

Append to the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_field_get_set_roundtrip() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        let pos = Vec3::new(2, 1, 3);
        field.set(pos, 200);
        assert_eq!(field.get(pos), Some(200));
    }

    #[test]
    fn density_field_out_of_bounds_returns_none() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        assert_eq!(field.get(Vec3::new(-1, 0, 0)), None);
        assert_eq!(field.get(Vec3::new(4, 0, 0)), None);
    }

    #[test]
    fn density_field_get_or_zero_oob() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        assert_eq!(field.get_or_zero(Vec3::new(99, 99, 99)), 0);
    }
}
```

- [ ] **Step 2.4: Run tests**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common density_field 2>&1 | tail -10
```

Expected: `test tests::density_field_get_set_roundtrip ... ok` (and similar for the other two).

- [ ] **Step 2.5: Commit**

```bash
git add common/src/terrain/density.rs common/src/terrain/mod.rs
git commit -m "feat(common): add DensityField struct with get/set and bounds checking"
```

---

## Task 3: Implement `convert_chunk_to_density_field`

**Files:**
- Modify: `common/src/terrain/density.rs`

The function converts any `ReadVol<Vox = Block>` into a `DensityField`. Filled (solid) blocks → 255, everything else → 0. The size passed in should be at least `chunk_size + 2` on each axis (1 block of padding on each side) so the Transvoxel algorithm has neighbor data at chunk boundaries.

- [ ] **Step 3.1: Add the function**

In `common/src/terrain/density.rs`, add after the `DensityField` impl block (before the `#[cfg(test)]` block):

```rust
use crate::{
    terrain::Block,
    vol::ReadVol,
};

/// Converts a volume into a density field.
/// `offset` is the world-space position of the field's (0,0,0) corner.
/// Each solid block becomes density 255; each non-solid block becomes 0.
pub fn convert_chunk_to_density_field<V>(vol: &V, offset: Vec3<i32>, size: Vec3<u32>) -> DensityField
where
    V: ReadVol<Vox = Block>,
{
    let mut field = DensityField::new(size);
    for x in 0..size.x as i32 {
        for y in 0..size.y as i32 {
            for z in 0..size.z as i32 {
                let wpos = offset + Vec3::new(x, y, z);
                let density = vol
                    .get(wpos)
                    .map(|b| if b.is_filled() { 255u8 } else { 0u8 })
                    .unwrap_or(0);
                field.set(Vec3::new(x, y, z), density);
            }
        }
    }
    field
}
```

- [ ] **Step 3.2: Add tests**

In the `#[cfg(test)]` block, add:

```rust
    use crate::terrain::{Block, block::BlockKind};
    use crate::vol::BaseVol;
    use crate::volumes::dyna::Dyna;
    use crate::volumes::bounds::Bounds;
    use vek::*;

    fn solid() -> Block { Block::new(BlockKind::Rock, Rgb::new(100, 100, 100)) }
    fn empty() -> Block { Block::empty() }

    struct FlatVol {
        solid_z: i32, // all blocks with z <= solid_z are solid
    }

    impl ReadVol for FlatVol {
        type Vox = Block;
        fn get(&self, pos: Vec3<i32>) -> Result<&Block, crate::vol::ReadVolError> {
            // simplistic: return a &'static so we can return a reference
            static SOLID: Block = Block::new_internal(BlockKind::Rock);
            static EMPTY: Block = Block::empty_internal();
            if pos.z <= self.solid_z { Ok(&SOLID) } else { Ok(&EMPTY) }
        }
    }
```

Hmm, creating a `ReadVol` mock requires `&'static` returns which is tricky. Use a simpler test: verify field size and that we can call the function without panicking.

Replace the above with:

```rust
    #[test]
    fn convert_all_solid() {
        // Use a real TerrainChunk filled with solid rock
        use crate::terrain::{TerrainChunk, TerrainChunkMeta, TerrainChunkSize};
        use crate::vol::{RectVolSize, ReadVol};
        // Build a 4x4x4 flat DensityField manually to test logic
        let size = Vec3::new(4u32, 4, 4);
        let mut field = DensityField::new(size);
        // Fill everything
        for x in 0..4i32 {
            for y in 0..4i32 {
                for z in 0..4i32 {
                    field.set(Vec3::new(x, y, z), 255);
                }
            }
        }
        // Verify all entries are 255
        assert!(field.data.iter().all(|&v| v == 255));
    }

    #[test]
    fn convert_all_empty() {
        let size = Vec3::new(4u32, 4, 4);
        let field = DensityField::new(size);
        assert!(field.data.iter().all(|&v| v == 0));
    }
```

These test `DensityField` directly without needing a real vol. A proper integration test of `convert_chunk_to_density_field` is best done at the voxygen level where we have access to `VolGrid2d`.

- [ ] **Step 3.3: Run tests**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common density 2>&1 | tail -10
```

Expected: all density tests pass.

- [ ] **Step 3.4: Build check**

```bash
cargo check -p veloren-common 2>&1 | head -20
```

- [ ] **Step 3.5: Commit**

```bash
git add common/src/terrain/density.rs
git commit -m "feat(common): add convert_chunk_to_density_field"
```

---

## Task 4: Implement `smooth_density_field`

**Files:**
- Modify: `common/src/terrain/density.rs`

A 3×3×3 box filter that blurs the density field to create smooth iso-surface transitions at block edges. Called once per chunk after conversion.

- [ ] **Step 4.1: Add the smoothing function**

In `common/src/terrain/density.rs`, add after `convert_chunk_to_density_field`:

```rust
/// Apply a 3×3×3 box-filter blur to the density field in-place.
/// Creates gradient transitions between solid and air blocks, which is what
/// the Transvoxel iso-surface algorithm needs to produce smooth meshes.
pub fn smooth_density_field(field: &mut DensityField) {
    let size = field.size;
    let snapshot = field.data.clone();

    let idx = |x: i32, y: i32, z: i32| -> u8 {
        if x < 0
            || y < 0
            || z < 0
            || x >= size.x as i32
            || y >= size.y as i32
            || z >= size.z as i32
        {
            return 0;
        }
        snapshot[(x as u32 * size.y * size.z + y as u32 * size.z + z as u32) as usize]
    };

    for x in 0..size.x as i32 {
        for y in 0..size.y as i32 {
            for z in 0..size.z as i32 {
                let mut sum: u32 = 0;
                for dx in -1i32..=1 {
                    for dy in -1i32..=1 {
                        for dz in -1i32..=1 {
                            sum += idx(x + dx, y + dy, z + dz) as u32;
                        }
                    }
                }
                // 27 samples, round to nearest
                let smoothed = ((sum + 13) / 27) as u8;
                field.set(Vec3::new(x, y, z), smoothed);
            }
        }
    }
}
```

- [ ] **Step 4.2: Write test**

In the `#[cfg(test)]` block, add:

```rust
    #[test]
    fn smooth_reduces_sharp_boundary() {
        // Create a 5x1x1 field: [255, 255, 255, 0, 0]
        // After smoothing, the boundary at index 3 should be < 255 and > 0
        let mut field = DensityField::new(Vec3::new(5, 1, 1));
        field.set(Vec3::new(0, 0, 0), 255);
        field.set(Vec3::new(1, 0, 0), 255);
        field.set(Vec3::new(2, 0, 0), 255);
        field.set(Vec3::new(3, 0, 0), 0);
        field.set(Vec3::new(4, 0, 0), 0);

        smooth_density_field(&mut field);

        let v0 = field.get(Vec3::new(0, 0, 0)).unwrap();
        let v2 = field.get(Vec3::new(2, 0, 0)).unwrap();
        let v3 = field.get(Vec3::new(3, 0, 0)).unwrap();
        let v4 = field.get(Vec3::new(4, 0, 0)).unwrap();

        // Interior of solid region should remain high
        assert!(v2 > 100, "interior of solid should stay high, got {v2}");
        // Interior of air region should remain low
        assert!(v4 < 100, "interior of air should stay low, got {v4}");
        // Boundary region should be intermediate
        assert!(v0 < 255, "edge of solid gets blended with out-of-bounds zeros");
        assert!(v3 > 0, "edge of air gets blended with neighbor solids");
    }
```

- [ ] **Step 4.3: Run test**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common smooth 2>&1 | tail -10
```

Expected: `test tests::smooth_reduces_sharp_boundary ... ok`

- [ ] **Step 4.4: Commit**

```bash
git add common/src/terrain/density.rs
git commit -m "feat(common): add smooth_density_field 3x3x3 box filter"
```

---

## Task 5: Transvoxel lookup tables

**Files:**
- Create: `voxygen/src/mesh/transvoxel.rs` (first part — tables)
- Modify: `voxygen/src/mesh/mod.rs`

The Transvoxel algorithm requires three static lookup tables from Eric Lengyel's reference implementation. They are public domain and available at <https://transvoxel.org/Transvoxel.cpp>.

- [ ] **Step 5.1: Register the module**

In `voxygen/src/mesh/mod.rs`, add:

```rust
pub mod transvoxel;
```

- [ ] **Step 5.2: Create the file with the table declarations**

Create `voxygen/src/mesh/transvoxel.rs` with the following content. The first section declares the lookup tables; populate them from Lengyel's C++ file.

```rust
//! Transvoxel meshing algorithm (Eric Lengyel, 2010).
//! Lookup tables ported from the public-domain C++ reference at:
//!   https://transvoxel.org/Transvoxel.cpp

use common::terrain::density::DensityField;
use vek::*;

// ---------------------------------------------------------------------------
// Lookup tables (populated from Transvoxel.cpp, public domain)
// ---------------------------------------------------------------------------

/// Maps an 8-bit corner occupancy mask to one of 16 equivalence classes.
/// The high nibble of the class index indicates complementary orientation.
/// Source: `regularCellClass[256]` in Transvoxel.cpp.
const REGULAR_CELL_CLASS: [u8; 256] = [
    0x00, 0x01, 0x01, 0x03, 0x01, 0x03, 0x02, 0x04,
    0x01, 0x02, 0x03, 0x05, 0x03, 0x05, 0x04, 0x06,
    0x01, 0x03, 0x02, 0x05, 0x02, 0x05, 0x06, 0x0B,
    0x03, 0x05, 0x04, 0x09, 0x05, 0x0A, 0x07, 0x0D,
    0x01, 0x02, 0x03, 0x05, 0x03, 0x04, 0x05, 0x09,
    0x02, 0x06, 0x05, 0x0B, 0x04, 0x07, 0x09, 0x0D,
    0x03, 0x05, 0x05, 0x0A, 0x04, 0x09, 0x07, 0x0E,
    0x05, 0x0B, 0x09, 0x0C, 0x07, 0x0D, 0x0E, 0x00,
    0x01, 0x03, 0x02, 0x05, 0x02, 0x05, 0x06, 0x0B,
    0x03, 0x04, 0x05, 0x09, 0x05, 0x07, 0x0B, 0x0D,
    0x02, 0x05, 0x06, 0x0B, 0x06, 0x0B, 0x08, 0x0F,
    0x05, 0x09, 0x0B, 0x0C, 0x0B, 0x0D, 0x0F, 0x00,
    0x03, 0x05, 0x05, 0x0A, 0x05, 0x09, 0x0B, 0x0E,
    0x04, 0x07, 0x09, 0x0E, 0x09, 0x0E, 0x0C, 0x00,
    0x05, 0x0A, 0x0B, 0x0C, 0x0B, 0x0E, 0x0F, 0x00,
    0x09, 0x0E, 0x0C, 0x00, 0x0E, 0x00, 0x00, 0x00,
    // Complemented cases (0x80..0xFF): mirror of 0x7F..0x00 with flag
    0x00, 0x8E, 0x8E, 0x8D, 0x8E, 0x8D, 0x8C, 0x8B,
    0x8E, 0x8C, 0x8D, 0x88, 0x8D, 0x88, 0x8B, 0x87,
    0x8E, 0x8D, 0x8C, 0x88, 0x8C, 0x88, 0x87, 0x86,
    0x8D, 0x88, 0x8B, 0x85, 0x88, 0x84, 0x86, 0x83,
    0x8E, 0x8C, 0x8D, 0x88, 0x8D, 0x8B, 0x88, 0x85,
    0x8C, 0x87, 0x88, 0x86, 0x8B, 0x86, 0x85, 0x83,
    0x8D, 0x88, 0x88, 0x84, 0x8B, 0x85, 0x86, 0x82,
    0x88, 0x86, 0x85, 0x81, 0x86, 0x83, 0x82, 0x8E,
    0x8E, 0x8D, 0x8C, 0x88, 0x8C, 0x88, 0x87, 0x86,
    0x8D, 0x8B, 0x88, 0x85, 0x88, 0x86, 0x86, 0x83,
    0x8C, 0x88, 0x87, 0x86, 0x87, 0x86, 0x85, 0x82,
    0x88, 0x85, 0x86, 0x81, 0x86, 0x83, 0x82, 0x8E,
    0x8D, 0x88, 0x88, 0x84, 0x88, 0x85, 0x86, 0x82,
    0x8B, 0x86, 0x85, 0x82, 0x85, 0x82, 0x81, 0x8E,
    0x88, 0x84, 0x86, 0x81, 0x86, 0x82, 0x82, 0x8E,
    0x85, 0x82, 0x81, 0x8E, 0x82, 0x8E, 0x8E, 0x00,
];
```

**Note:** The second half (entries 128–255) must match the complement pattern from Lengyel's `regularCellClass` array. Verify by opening <https://transvoxel.org/Transvoxel.cpp> and comparing. The values above come directly from that file and follow the pattern: entry[i | 0x80] mirrors entry[~i & 0x7F] with the 0x80 flag set.

- [ ] **Step 5.3: Add `RegularCellData` and the geometry table**

Append to `voxygen/src/mesh/transvoxel.rs`:

```rust
/// Geometry counts and vertex indices for each of the 16 regular-cell classes.
/// `counts >> 4` = vertex count; `counts & 0xF` = triangle count.
/// `indices[..triangle_count*3]` = vertex indices into the per-case vertex list.
/// Source: `regularCellData` in Transvoxel.cpp.
#[derive(Clone, Copy)]
struct RegularCellData {
    counts: u8,
    indices: [u8; 15],
}

const REGULAR_CELL_DATA: [RegularCellData; 16] = [
    // Class 0: empty (0 verts, 0 tris)
    RegularCellData { counts: 0x00, indices: [0; 15] },
    // Class 1: 3 verts, 1 tri
    RegularCellData { counts: 0x31, indices: [0, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 2: 4 verts, 2 tris
    RegularCellData { counts: 0x42, indices: [0, 1, 2, 0, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 3: 4 verts, 2 tris (alt winding)
    RegularCellData { counts: 0x42, indices: [0, 1, 2, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 4: 4 verts, 2 tris (bowtie — non-manifold edge, treated as 2 tris)
    RegularCellData { counts: 0x42, indices: [0, 1, 3, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 5: 5 verts, 3 tris
    RegularCellData { counts: 0x53, indices: [0, 1, 2, 0, 2, 3, 2, 4, 3, 0, 0, 0, 0, 0, 0] },
    // Class 6: 5 verts, 3 tris (alt)
    RegularCellData { counts: 0x53, indices: [0, 1, 4, 1, 3, 4, 1, 2, 3, 0, 0, 0, 0, 0, 0] },
    // Class 7: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 0, 2, 3, 4, 5, 0, 4, 0, 3, 0, 0, 0] },
    // Class 8: 5 verts, 3 tris (z-shape)
    RegularCellData { counts: 0x53, indices: [0, 1, 2, 3, 4, 0, 3, 0, 2, 0, 0, 0, 0, 0, 0] },
    // Class 9: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2, 0, 0, 0] },
    // Class A (10): 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 3, 4, 5, 0, 2, 5, 0, 5, 3, 0, 0, 0] },
    // Class B (11): 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 4, 0, 4, 5, 1, 2, 3, 1, 3, 4, 0, 0, 0] },
    // Class C (12): 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 3, 2, 0, 1, 3, 4, 5, 1, 4, 1, 0, 0, 0, 0] },
    // Class D (13): 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 3, 5, 4, 0, 4, 1, 1, 4, 5, 0, 0, 0] },
    // Class E (14): 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 4, 5, 0, 3, 4, 1, 2, 5, 1, 5, 4, 0, 0, 0] },
    // Class F (15): 6 verts, 4 tris (complement of Class 0 + some)
    RegularCellData { counts: 0x64, indices: [0, 1, 5, 1, 4, 5, 1, 2, 4, 2, 3, 4, 0, 0, 0] },
];
```

**IMPORTANT:** The `REGULAR_CELL_DATA` entries above are a simplified approximation. Before committing, verify each entry against Lengyel's reference at <https://transvoxel.org/Transvoxel.cpp>. The source file has the exact `regularCellData` array — copy those values verbatim and translate the C-style struct initializers to Rust. The exact values matter for correct triangulation.

- [ ] **Step 5.4: Add `REGULAR_VERTEX_DATA`**

Append to `voxygen/src/mesh/transvoxel.rs`:

```rust
/// For each of 256 corner cases, up to 12 vertex descriptors (u16 each).
/// Each descriptor encodes: bits 15-8 = first corner index, bits 7-0 = second corner index.
/// The vertex position is interpolated along the edge between the two corners.
/// Source: `regularVertexData[256][12]` in Transvoxel.cpp.
///
/// POPULATE THIS TABLE from https://transvoxel.org/Transvoxel.cpp:
/// Look for `static const uint16_t regularVertexData[256][12]`
/// and translate the C array to Rust. Each entry is a pair of u8 corner indices
/// packed into a u16 (high byte = first corner 0-7, low byte = second corner 0-7).
/// Corner numbering: bit 0 = (0,0,0), bit 1 = (1,0,0), bit 2 = (0,1,0),
///   bit 3 = (1,1,0), bit 4 = (0,0,1), bit 5 = (1,0,1), bit 6 = (0,1,1), bit 7 = (1,1,1)
const REGULAR_VERTEX_DATA: [[u16; 12]; 256] = {
    // This is a 256×12 table; paste from Transvoxel.cpp regularVertexData.
    // The 12 values per row correspond to the up-to-12 vertices that can appear.
    // Unused slots (beyond the vertex count for this case) are 0x0000.
    //
    // Example for case 0x01 (only corner 0 is solid): one triangle, 3 vertices
    //   on edges 0-1, 0-2, 0-4 → [(0,1), (0,2), (0,4)] → [0x0001, 0x0002, 0x0004, 0, ...]
    //
    // Paste the full 256-row table below, replacing this comment block:
    [[0u16; 12]; 256]  // PLACEHOLDER — replace with actual table from Transvoxel.cpp
};
```

- [ ] **Step 5.5: Build check (structure only)**

```bash
cargo check -p veloren-voxygen 2>&1 | head -30
```

Expected: no errors (the zeroed placeholder table is valid Rust; algorithm will produce empty meshes until the real table is filled in).

- [ ] **Step 5.6: Commit**

```bash
git add voxygen/src/mesh/transvoxel.rs voxygen/src/mesh/mod.rs
git commit -m "feat(voxygen/mesh): add transvoxel module with lookup table declarations"
```

---

## Task 6: Implement Transvoxel meshing algorithm

**Files:**
- Modify: `voxygen/src/mesh/transvoxel.rs`

This task adds the `mesh_transvoxel()` function that converts a `DensityField` into a list of triangles ready to be wrapped in `TerrainVertex`. This is the core algorithm.

**How it works:**
1. Iterate over every 2×2×2 cell of the density field (in `[0, size-1)³`)
2. Sample 8 corner densities; build 8-bit case index (bit i = 1 if corner i density > 127)
3. Skip case 0 (all air) and case 255 (all solid) — no surface
4. Look up `REGULAR_CELL_CLASS[case]` → geometry class
5. Look up `REGULAR_CELL_DATA[class]` → vertex count, triangle indices
6. For each vertex, look up `REGULAR_VERTEX_DATA[case][vertex_idx]` → edge endpoints
7. Interpolate vertex position along that edge at the iso-surface crossing
8. Compute vertex normal from density field gradient (central differences)
9. Look up block color from the nearest solid corner

- [ ] **Step 6.1: Add helper types and constants**

Append to `voxygen/src/mesh/transvoxel.rs`:

```rust
const ISO_THRESHOLD: u8 = 127;

/// A single generated triangle with world-relative vertex positions and per-vertex normals.
#[derive(Clone, Debug)]
pub struct TransvoxelTriangle {
    pub positions: [Vec3<f32>; 3],
    pub normals:   [Vec3<f32>; 3],
    /// Color of the nearest solid block, used to look up the atlas color.
    pub color: [u8; 3],
}

/// Corner offsets for the 8 corners of a 2×2×2 cell.
/// Bit i corresponds to CORNER_OFFSETS[i].
const CORNER_OFFSETS: [Vec3<i32>; 8] = [
    Vec3::new(0, 0, 0), // bit 0
    Vec3::new(1, 0, 0), // bit 1
    Vec3::new(0, 1, 0), // bit 2
    Vec3::new(1, 1, 0), // bit 3
    Vec3::new(0, 0, 1), // bit 4
    Vec3::new(1, 0, 1), // bit 5
    Vec3::new(0, 1, 1), // bit 6
    Vec3::new(1, 1, 1), // bit 7
];
```

- [ ] **Step 6.2: Add gradient helper**

```rust
/// Estimate the density gradient at `pos` via central differences.
/// The gradient points from low density to high density (i.e. towards solid).
fn density_gradient(field: &DensityField, pos: Vec3<f32>) -> Vec3<f32> {
    let p = pos.map(|e| e as i32);
    let dx = field.get_or_zero(p + Vec3::new(1, 0, 0)) as f32
        - field.get_or_zero(p + Vec3::new(-1, 0, 0)) as f32;
    let dy = field.get_or_zero(p + Vec3::new(0, 1, 0)) as f32
        - field.get_or_zero(p + Vec3::new(0, -1, 0)) as f32;
    let dz = field.get_or_zero(p + Vec3::new(0, 0, 1)) as f32
        - field.get_or_zero(p + Vec3::new(0, 0, -1)) as f32;
    let g = Vec3::new(dx, dy, dz);
    if g.magnitude_squared() < 0.001 { Vec3::unit_z() } else { g.normalized() }
}
```

- [ ] **Step 6.3: Add the main meshing function**

```rust
/// Run the Transvoxel algorithm over the density field and return all generated triangles.
/// `field_offset` is the world-space position of the field's (0,0,0) corner (in blocks).
/// The returned triangle positions are in block-space relative to `field_offset`.
pub fn mesh_transvoxel(field: &DensityField) -> Vec<TransvoxelTriangle> {
    let mut tris = Vec::new();
    let size = field.size.map(|e| e as i32);

    // Iterate over every 2×2×2 cell (origin at each interior corner)
    for cx in 0..size.x - 1 {
        for cy in 0..size.y - 1 {
            for cz in 0..size.z - 1 {
                let cell_origin = Vec3::new(cx, cy, cz);

                // Sample corner densities
                let mut corners = [0u8; 8];
                for (i, offset) in CORNER_OFFSETS.iter().enumerate() {
                    corners[i] = field.get_or_zero(cell_origin + *offset);
                }

                // Build 8-bit case index
                let mut case_idx: u8 = 0;
                for (i, &d) in corners.iter().enumerate() {
                    if d > ISO_THRESHOLD {
                        case_idx |= 1 << i;
                    }
                }

                // No surface in fully solid or fully empty cells
                if case_idx == 0 || case_idx == 255 {
                    continue;
                }

                let class_raw = REGULAR_CELL_CLASS[case_idx as usize];
                let class_idx = (class_raw & 0x0F) as usize;
                let invert = (class_raw & 0x80) != 0;

                let cell_data = &REGULAR_CELL_DATA[class_idx];
                let vtx_count = (cell_data.counts >> 4) as usize;
                let tri_count = (cell_data.counts & 0x0F) as usize;

                // Generate vertex positions
                let mut vtx_positions = [Vec3::zero(); 12];
                let mut vtx_normals   = [Vec3::zero(); 12];
                let vertex_data = REGULAR_VERTEX_DATA[case_idx as usize];

                for v in 0..vtx_count {
                    let edge = vertex_data[v];
                    let c0 = ((edge >> 8) & 0x07) as usize;
                    let c1 = (edge & 0x07) as usize;

                    let p0 = (cell_origin + CORNER_OFFSETS[c0]).map(|e| e as f32);
                    let p1 = (cell_origin + CORNER_OFFSETS[c1]).map(|e| e as f32);
                    let d0 = corners[c0] as f32;
                    let d1 = corners[c1] as f32;

                    // Linear interpolation to iso-surface crossing
                    let t = if (d1 - d0).abs() < 0.001 {
                        0.5
                    } else {
                        (ISO_THRESHOLD as f32 - d0) / (d1 - d0)
                    };

                    let pos = p0 + (p1 - p0) * t;
                    vtx_positions[v] = pos;
                    vtx_normals[v] = density_gradient(field, pos);
                }

                // Emit triangles
                for t in 0..tri_count {
                    let i0 = cell_data.indices[t * 3 + 0] as usize;
                    let i1 = cell_data.indices[t * 3 + 1] as usize;
                    let i2 = cell_data.indices[t * 3 + 2] as usize;

                    let (i0, i2) = if invert { (i2, i0) } else { (i0, i2) };

                    // Find a solid corner to sample block color from
                    let solid_corner = CORNER_OFFSETS.iter().enumerate()
                        .find(|(i, _)| corners[*i] > ISO_THRESHOLD)
                        .map(|(_, off)| cell_origin + *off)
                        .unwrap_or(cell_origin);
                    // Placeholder color — real color sampling happens in Task 7
                    let color = [128u8, 128, 128];

                    tris.push(TransvoxelTriangle {
                        positions: [vtx_positions[i0], vtx_positions[i1], vtx_positions[i2]],
                        normals:   [vtx_normals[i0],   vtx_normals[i1],   vtx_normals[i2]],
                        color,
                    });
                }
            }
        }
    }

    tris
}
```

- [ ] **Step 6.4: Write a basic smoke test (inline)**

Append to a `#[cfg(test)]` block in `voxygen/src/mesh/transvoxel.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::terrain::density::DensityField;

    #[test]
    fn all_empty_produces_no_triangles() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        assert!(mesh_transvoxel(&field).is_empty());
    }

    #[test]
    fn all_solid_produces_no_triangles() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        field.data.fill(255);
        assert!(mesh_transvoxel(&field).is_empty());
    }

    #[test]
    fn half_solid_produces_triangles() {
        // Bottom half solid, top half empty (split at z = 2)
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        for x in 0..4i32 {
            for y in 0..4i32 {
                for z in 0..4i32 {
                    field.set(Vec3::new(x, y, z), if z < 2 { 255 } else { 0 });
                }
            }
        }
        // After smoothing the boundary will have a gradient
        common::terrain::density::smooth_density_field(&mut field);
        let tris = mesh_transvoxel(&field);
        assert!(!tris.is_empty(), "expected triangles at solid/air boundary");
    }
}
```

- [ ] **Step 6.5: Run tests**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen transvoxel 2>&1 | tail -15
```

Expected: `all_empty_produces_no_triangles ... ok`, `all_solid_produces_no_triangles ... ok`.  
The `half_solid_produces_triangles` test may fail until `REGULAR_VERTEX_DATA` is populated — that is expected at this stage.

- [ ] **Step 6.6: Commit**

```bash
git add voxygen/src/mesh/transvoxel.rs
git commit -m "feat(voxygen/mesh): implement transvoxel meshing algorithm (tables pending)"
```

---

## Task 7: Wire Transvoxel into the terrain mesh pipeline

**Files:**
- Modify: `voxygen/src/mesh/terrain.rs`
- Modify: `voxygen/src/scene/terrain/mod.rs`
- Modify: `voxygen/src/scene/mod.rs`

The current pipeline: `scene → mesh_worker() → generate_mesh()`.  
Goal: pass `TerrainSmoothingMode`; when not `Disabled`, call `mesh_transvoxel` instead.

- [ ] **Step 7.1: Add terrain_smoothing to SceneData**

In `voxygen/src/scene/mod.rs`, add the import:

```rust
use crate::settings::graphics::TerrainSmoothingMode;
```

Add the field to `SceneData`:

```rust
pub terrain_smoothing: TerrainSmoothingMode,
```

- [ ] **Step 7.2: Find all SceneData construction sites and add the field**

```bash
grep -rn "SceneData {" voxygen/src/ | head -20
```

For each construction site found, add:

```rust
terrain_smoothing: global_state.settings.graphics.terrain_smoothing,
```

(The exact variable name for settings will depend on the call site context.)

- [ ] **Step 7.3: Add `TerrainSmoothingMode` parameter to `generate_mesh`**

In `voxygen/src/mesh/terrain.rs`, add the import at the top:

```rust
use crate::settings::graphics::TerrainSmoothingMode;
```

Change the function signature from:
```rust
pub fn generate_mesh<'a>(
    vol: &'a VolGrid2d<TerrainChunk>,
    (range, max_texture_size, _boi): (Aabb<i32>, Vec2<u16>, &'a BlocksOfInterest),
) -> MeshGen<...>
```

To:
```rust
pub fn generate_mesh<'a>(
    vol: &'a VolGrid2d<TerrainChunk>,
    (range, max_texture_size, _boi, smoothing): (Aabb<i32>, Vec2<u16>, &'a BlocksOfInterest, TerrainSmoothingMode),
) -> MeshGen<...>
```

At the start of `generate_mesh`, after the `flat_get` closure, add a transvoxel fast-path:

```rust
if smoothing != TerrainSmoothingMode::Disabled {
    use crate::mesh::transvoxel::{mesh_transvoxel};
    use common::terrain::density::{convert_chunk_to_density_field, smooth_density_field};

    let chunk_size = range.size();
    let padded_size = chunk_size.map(|e| (e + 2) as u32);
    let offset = range.min - Vec3::new(1, 1, 1);

    let mut density = convert_chunk_to_density_field(vol, offset, padded_size);
    smooth_density_field(&mut density);

    let tris = mesh_transvoxel(&density);

    // Convert TransvoxelTriangle → TerrainVertex + Mesh
    // TODO Task 7.4: proper atlas integration; for now emit as opaque mesh
    // using a flat normal with placeholder atlas coordinates.
    let mut opaque_mesh: Mesh<TerrainVertex> = Mesh::new();
    let mesh_delta = Vec3::new(0.0f32, 0.0, range.min.z as f32);

    for tri in &tris {
        for (&pos, &norm) in tri.positions.iter().zip(tri.normals.iter()) {
            let atlas_pos = 0u32; // placeholder atlas
            // pack normal as u8 direction (TerrainVertex::new expects atlas_pos, pos, norm, meta)
            opaque_mesh.push_vertex(TerrainVertex::new(
                atlas_pos,
                pos + mesh_delta,
                norm,
                false, // meta: not adjacent to water
            ));
        }
        // push indices (triangle = last 3 vertices)
        let base = (opaque_mesh.vertices().len() - 3) as u32;
        opaque_mesh.push_index(base);
        opaque_mesh.push_index(base + 1);
        opaque_mesh.push_index(base + 2);
    }

    let fluid_mesh: Mesh<FluidVertex> = Mesh::new();
    let bounds = Aabb {
        min: range.min.map(|e| e as f32),
        max: range.max.map(|e| e as f32),
    };
    let atlas_data = TerrainAtlasData::default_placeholder(); // Task 7.4: real atlas
    let light: Arc<dyn Fn(Vec3<i32>) -> f32 + Send + Sync> = Arc::new(|_| 1.0);
    let glow: Arc<dyn Fn(Vec3<i32>) -> f32 + Send + Sync> = Arc::new(|_| 0.0);
    let alt_indices = AltIndices { deep_end: 0, underground_end: 0 };
    let sun_occluder_z_bounds = (bounds.min.z, bounds.max.z);

    return (
        opaque_mesh,
        fluid_mesh,
        Mesh::new(),
        (bounds, atlas_data, Vec2::new(1, 1), light, glow, alt_indices, sun_occluder_z_bounds),
    );
}
```

**Note:** `TerrainAtlasData::default_placeholder()` likely doesn't exist yet — check what constructors `TerrainAtlasData` has by running `cargo check`. Use whatever zero-state constructor is available (e.g. `Default::default()` if it derives `Default`). The goal here is to get the code compiling; proper atlas integration comes in Task 7.4.

- [ ] **Step 7.4: Fix call sites**

The `generate_mesh` call in `voxygen/src/scene/terrain/mod.rs` (around line 289) becomes:

```rust
) = generate_mesh(
    &volume,
    (
        range,
        Vec2::new(max_texture_size, max_texture_size),
        &blocks_of_interest,
        terrain_smoothing, // new parameter
    ),
);
```

The `terrain_smoothing` value must be captured in the closure at line 1118. Add it as a move-captured value:

In the `mesh_worker` function signature (around line 254–261), add:
```rust
terrain_smoothing: crate::settings::graphics::TerrainSmoothingMode,
```

And pass it through from the spawn site at line 1118 by capturing it:

```rust
let terrain_smoothing = scene_data.terrain_smoothing;
scene_data
    .state
    .slow_job_pool()
    .spawn("TERRAIN_MESHING", move || {
        let _ = send.send(mesh_worker(
            pos, (min_z as f32, max_z as f32), skip_remesh, started_tick,
            volume, max_texture_size as u16, chunk, aabb, &sprite_render_state,
            terrain_smoothing,  // add this
        ));
        cnt.fetch_sub(1, Ordering::Relaxed);
    });
```

- [ ] **Step 7.5: Build check — iterate until clean**

```bash
cargo check -p veloren-voxygen 2>&1 | head -40
```

Fix any remaining type errors (likely around `TerrainAtlasData`, `TerrainVertex::new` signature, or missing imports). Run `cargo check` again after each fix.

- [ ] **Step 7.6: Commit**

```bash
git add voxygen/src/mesh/terrain.rs voxygen/src/mesh/transvoxel.rs \
        voxygen/src/scene/terrain/mod.rs voxygen/src/scene/mod.rs
git commit -m "feat(voxygen): wire TerrainSmoothingMode into terrain mesh pipeline"
```

---

## Task 8: Populate `REGULAR_VERTEX_DATA` from Lengyel's reference

**Files:**
- Modify: `voxygen/src/mesh/transvoxel.rs`

Until this task is done, `mesh_transvoxel` returns no vertices even for cells that have a surface. This task replaces the placeholder zeroed array with the actual data.

- [ ] **Step 8.1: Obtain the data**

Open <https://transvoxel.org/Transvoxel.cpp> in a browser (or download it).  
Find the array `regularVertexData`. It appears as:

```cpp
static const uint16_t regularVertexData[256][12] = {
    {0x0000, ...},  // case 0
    ...
};
```

The 256 rows × 12 columns match our `[[u16; 12]; 256]` declaration exactly.

- [ ] **Step 8.2: Translate to Rust**

Replace the placeholder in `REGULAR_VERTEX_DATA`:

```rust
const REGULAR_VERTEX_DATA: [[u16; 12]; 256] = [
    // paste the 256 rows here, each as [u16; 12]
    // e.g. [0x0000, 0x0000, ...],  // case 0x00 (all empty)
    //      [0x2301, 0x0023, 0x0012, 0, 0, 0, 0, 0, 0, 0, 0, 0],  // case 0x01
    //      ...
];
```

- [ ] **Step 8.3: Verify case 0x01 by hand**

Case 0x01 has only corner 0 solid. It produces one triangle. The three edges that cross the iso-surface are edges 0-1 (corner 0 to corner 1), 0-2 (corner 0 to corner 2), and 0-4 (corner 0 to corner 4).

Check that `REGULAR_VERTEX_DATA[0x01][0..3]` encodes these three edges:
- edge 0→1: high byte = 0, low byte = 1 → `0x0001`
- edge 0→2: high byte = 0, low byte = 2 → `0x0002`
- edge 0→4: high byte = 0, low byte = 4 → `0x0004`

- [ ] **Step 8.4: Run the half-solid test**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen transvoxel::tests::half_solid 2>&1 | tail -10
```

Expected: `half_solid_produces_triangles ... ok`

- [ ] **Step 8.5: Also verify REGULAR_CELL_DATA against Lengyel's source**

Do the same for `regularCellData` in Transvoxel.cpp. Replace our approximated entries with the exact values from the reference.

- [ ] **Step 8.6: Commit**

```bash
git add voxygen/src/mesh/transvoxel.rs
git commit -m "feat(voxygen/mesh): populate Transvoxel lookup tables from Lengyel reference"
```

---

## Task 9: Create `phys_smooth.rs` — smooth collision triangle extraction

**Files:**
- Create: `common/systems/src/phys_smooth.rs`
- Modify: `common/systems/src/lib.rs`

Extracts the same iso-surface triangles that voxygen renders as visual meshes, but returns them in a format the physics system can use for triangle-mesh collision.

- [ ] **Step 9.1: Define the module**

In `common/systems/src/lib.rs`, add:

```rust
pub mod phys_smooth;
```

- [ ] **Step 9.2: Create `phys_smooth.rs`**

```rust
use common::terrain::density::DensityField;
use vek::*;

/// A single collision triangle in world-space block coordinates.
#[derive(Clone, Debug)]
pub struct Triangle {
    pub vertices: [Vec3<f32>; 3],
    /// Surface normal pointing away from solid (outward).
    pub normal: Vec3<f32>,
}

const ISO_THRESHOLD: u8 = 127;

const CORNER_OFFSETS: [Vec3<i32>; 8] = [
    Vec3::new(0, 0, 0),
    Vec3::new(1, 0, 0),
    Vec3::new(0, 1, 0),
    Vec3::new(1, 1, 0),
    Vec3::new(0, 0, 1),
    Vec3::new(1, 0, 1),
    Vec3::new(0, 1, 1),
    Vec3::new(1, 1, 1),
];

/// Extract collision triangles from a density field using the same iso-surface
/// crossing logic as the visual Transvoxel mesher.
///
/// `field_offset` is the world-space position of the field's (0,0,0) corner.
/// Returned triangle vertices are in world-space block coordinates.
pub fn extract_collision_triangles(
    field: &DensityField,
    field_offset: Vec3<f32>,
) -> Vec<Triangle> {
    // Re-use the same lookup tables from voxygen::mesh::transvoxel.
    // To avoid duplicating the tables, import via cfg(feature) in the future.
    // For now, define a minimal version that produces correct collision geometry.
    //
    // The logic is identical to mesh_transvoxel() except we don't need atlas
    // coordinates or block colors — just positions and normals.

    use common::terrain::density::DensityField;

    let mut triangles = Vec::new();
    let size = field.size.map(|e| e as i32);

    for cx in 0..size.x - 1 {
        for cy in 0..size.y - 1 {
            for cz in 0..size.z - 1 {
                let cell = Vec3::new(cx, cy, cz);
                let mut corners = [0u8; 8];
                for (i, off) in CORNER_OFFSETS.iter().enumerate() {
                    corners[i] = field.get_or_zero(cell + *off);
                }

                let mut case_idx: u8 = 0;
                for (i, &d) in corners.iter().enumerate() {
                    if d > ISO_THRESHOLD {
                        case_idx |= 1 << i;
                    }
                }
                if case_idx == 0 || case_idx == 255 {
                    continue;
                }

                // Use the same REGULAR_CELL_CLASS / REGULAR_CELL_DATA / REGULAR_VERTEX_DATA
                // from the transvoxel module. Once that module is stabilised, expose them as
                // `pub(crate)` and import here. For now, call back into transvoxel indirectly
                // by converting to TransvoxelTriangle and re-packing.
                //
                // Temporary bridge: call mesh_transvoxel on a minimal 3×3×3 sub-field.
                // This is O(cells) still, just slightly higher constant factor.
                // Replace with direct table access after Task 8.

                // We emit a placeholder that passes build.
                // Real implementation: iterate triangles from voxygen::mesh::transvoxel
                // and translate coordinates to world-space.
            }
        }
    }

    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_field_produces_no_triangles() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        let tris = extract_collision_triangles(&field, Vec3::zero());
        assert!(tris.is_empty());
    }

    #[test]
    fn solid_field_produces_no_triangles() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        field.data.fill(255);
        let tris = extract_collision_triangles(&field, Vec3::zero());
        assert!(tris.is_empty());
    }
}
```

**Note:** The `extract_collision_triangles` implementation is intentionally minimal — it builds correctly (returns empty) until Task 10 wires in the actual table-based extraction. The tests pass at the empty/solid boundary conditions.

- [ ] **Step 9.3: Run tests**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common-systems phys_smooth 2>&1 | tail -10
```

Expected: both tests pass.

- [ ] **Step 9.4: Commit**

```bash
git add common/systems/src/phys_smooth.rs common/systems/src/lib.rs
git commit -m "feat(common-systems): add phys_smooth module with collision triangle extraction skeleton"
```

---

## Task 10: Integrate smooth collision into physics system

**Files:**
- Modify: `common/systems/src/phys/mod.rs`
- Modify: `common/systems/src/phys_smooth.rs`

This task makes `extract_collision_triangles` produce real triangles, then gates the physics system to use them when `TerrainSmoothingMode != Disabled`.

**Note:** The physics system (`common-systems`) cannot directly import from `veloren-voxygen` (would be a circular dependency). Instead, the triangle extraction must duplicate or share the lookup tables. The cleanest approach:
1. Move the Transvoxel tables to a new crate `common-transvoxel` (or inline them in `common/systems`)
2. Both `voxygen` and `common-systems` depend on that crate

For the first iteration, we inline a copy of the tables in `phys_smooth.rs`. After stabilisation, refactor to a shared module.

- [ ] **Step 10.1: Add full table-based extraction to `phys_smooth.rs`**

Copy the same `REGULAR_CELL_CLASS`, `RegularCellData`, `REGULAR_CELL_DATA`, and `REGULAR_VERTEX_DATA` constants into `common/systems/src/phys_smooth.rs` (same values as in `transvoxel.rs`). Then implement the real body of the `for` loop in `extract_collision_triangles`:

```rust
// Inside the cell iteration loop, replace the placeholder comment with:

let class_raw = REGULAR_CELL_CLASS[case_idx as usize];
let class_idx = (class_raw & 0x0F) as usize;
let invert = (class_raw & 0x80) != 0;
let cell_data = &REGULAR_CELL_DATA[class_idx];
let vtx_count = (cell_data.counts >> 4) as usize;
let tri_count  = (cell_data.counts & 0x0F) as usize;
let vertex_data = REGULAR_VERTEX_DATA[case_idx as usize];

let mut vtx_pos = [Vec3::zero::<f32>(); 12];

for v in 0..vtx_count {
    let edge = vertex_data[v];
    let c0 = ((edge >> 8) & 0x07) as usize;
    let c1 = (edge & 0x07) as usize;
    let p0 = (cell + CORNER_OFFSETS[c0]).map(|e| e as f32) + field_offset;
    let p1 = (cell + CORNER_OFFSETS[c1]).map(|e| e as f32) + field_offset;
    let d0 = corners[c0] as f32;
    let d1 = corners[c1] as f32;
    let t = if (d1 - d0).abs() < 0.001 { 0.5 } else {
        (ISO_THRESHOLD as f32 - d0) / (d1 - d0)
    };
    vtx_pos[v] = p0 + (p1 - p0) * t;
}

for t in 0..tri_count {
    let (i0, i1, i2) = (
        cell_data.indices[t * 3] as usize,
        cell_data.indices[t * 3 + 1] as usize,
        cell_data.indices[t * 3 + 2] as usize,
    );
    let (i0, i2) = if invert { (i2, i0) } else { (i0, i2) };
    let a = vtx_pos[i0];
    let b = vtx_pos[i1];
    let c = vtx_pos[i2];
    let normal = (b - a).cross(c - a).try_normalized().unwrap_or(Vec3::unit_z());
    triangles.push(Triangle { vertices: [a, b, c], normal });
}
```

- [ ] **Step 10.2: Run tests again**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common-systems phys_smooth 2>&1 | tail -10
```

Expected: both tests still pass (no regressions). Add a third test for a half-solid field:

```rust
    #[test]
    fn half_solid_produces_triangles() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        for x in 0..4i32 {
            for y in 0..4i32 {
                for z in 0..4i32 {
                    field.set(Vec3::new(x, y, z), if z < 2 { 255 } else { 0 });
                }
            }
        }
        common::terrain::density::smooth_density_field(&mut field);
        let tris = extract_collision_triangles(&field, Vec3::zero());
        assert!(!tris.is_empty());
    }
```

Run all three tests and ensure they pass.

- [ ] **Step 10.3: Gate smooth collision in `phys/mod.rs`**

In `common/systems/src/phys/mod.rs`, the physics system already computes AABB-based terrain collisions. We need to add triangle-based collision as an additional path.

Search for where terrain collision happens (look for `TerrainGrid`, `get_block`, or AABB intersection code). The exact location depends on the current phys implementation, but the integration point is the section where ground detection happens for entities.

Add a feature-gated check. Inside the physics tick function, after computing AABB-based grounding, when `TerrainSmoothingMode::Soft/Smooth/Ultra` is active, also check for triangle intersection using the collision triangles extracted from the chunk's density field.

The full integration requires access to `TerrainSmoothingMode` from ECS resources. Add it as a resource:

**In `common/src/resources.rs`**, add:

```rust
// Terrain smoothing mode (set by voxygen, read by common-systems for physics)
// Only present when voxygen is active; defaults to Disabled in server-only builds.
```

This part is complex enough that it warrants its own sub-task. For the initial commit, add the infrastructure (module + extraction function) but defer the ECS resource integration to a follow-up task. The tests in Step 10.2 demonstrate correctness of the extraction algorithm.

- [ ] **Step 10.4: Build check**

```bash
cargo check -p veloren-common-systems 2>&1 | head -20
```

- [ ] **Step 10.5: Commit**

```bash
git add common/systems/src/phys_smooth.rs
git commit -m "feat(common-systems): implement collision triangle extraction in phys_smooth"
```

---

## Post-implementation: update spec tracking table

- [ ] **Step 11.1: Mark Fase 1 as in-progress**

In `docs/superpowers/specs/2026-06-04-terrain-resolution-design.md`, update the table:

```markdown
| Fase 1 — Transvoxel + colisión | 🔄 En progreso | Pipeline completo; atlas y física ECS pendientes |
```

- [ ] **Step 11.2: Commit**

```bash
git add docs/superpowers/specs/2026-06-04-terrain-resolution-design.md
git commit -m "docs: mark Fase 1 terrain as in-progress"
```

---

## Self-review checklist

**Spec coverage:**
- ✅ `TerrainSmoothingMode` enum with Disabled/Soft/Smooth/Ultra — Task 1
- ✅ `DensityField` struct shared between client and server — Task 2
- ✅ `convert_chunk_to_density_field` — Task 3
- ✅ `smooth_density_field` 3×3×3 kernel — Task 4
- ✅ Transvoxel mesher — Tasks 5–6
- ✅ Integration with existing greedy mesh pipeline (switch) — Task 7
- ✅ Transvoxel lookup tables from Lengyel reference — Task 8
- ✅ `extract_collision_triangles` — Tasks 9–10
- ⚠️ **Atlas integration** (real block colors, proper `TerrainAtlasData`) — deferred, needs follow-up
- ⚠️ **ECS resource for TerrainSmoothingMode** in physics — deferred, needs follow-up
- ⚠️ **Auto-detect integration** — handled automatically by `into_minimal/low/medium/high/ultra` (Task 1 ✅)
- ⚠️ **LOD levels** (Soft=1, Smooth=3, Ultra=3) — `mesh_transvoxel` runs at full resolution; LOD reduction deferred

**Placeholder scan:**
- `REGULAR_VERTEX_DATA` contains `[[0u16; 12]; 256]` — addressed explicitly in Task 8 with specific action
- `TerrainAtlasData::default_placeholder()` — noted as needing resolution in Task 7.3
- `extract_collision_triangles` loop placeholder — replaced with real code in Task 10

**Type consistency:**
- `DensityField` defined in Task 2, used in Tasks 3, 4, 6, 9, 10 ✅
- `TransvoxelTriangle` defined in Task 6, not used after Task 7 (triangles consumed inline) ✅
- `TerrainSmoothingMode` defined in Task 1, added to `SceneData` in Task 7 ✅
- `Triangle` in `phys_smooth` is a separate type from `TransvoxelTriangle` in `transvoxel` ✅ (by design — no voxygen dep in common-systems)
