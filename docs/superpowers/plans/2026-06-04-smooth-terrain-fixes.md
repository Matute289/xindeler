# Smooth Terrain Fixes — Structures + Smoothing Quality

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two post-launch issues found in the SmoothTerrainVertex pipeline: (1) building walls become invisible when smooth terrain is enabled, and (2) the terrain surface looks faceted (triangle edges too visible) instead of truly smooth.

**Architecture:** Fix 1 adds a structure-detection heuristic to `generate_mesh` — if a chunk contains site structures (detected via `BlocksOfInterest`), it falls back to the greedy mesher instead of Transvoxel. Fix 2 replaces the single-pass box filter in `smooth_density_field` with an N-pass Gaussian-weighted kernel, producing softer density gradients and smoother normals.

**Tech Stack:** Rust, existing `DensityField` / `BlocksOfInterest` / `TerrainSmoothingMode` types.

---

## Context for implementers

**The issue (Fix 1):** When `TerrainSmoothingMode != Disabled`, every chunk uses Transvoxel meshing. The Transvoxel algorithm smooths the density field, which destroys flat vertical surfaces like building walls. Building geometry becomes invisible; only sprite-based doors/windows remain visible because they render through a separate sprite system.

**Why this approach works:** `BlocksOfInterest` is already computed and passed to `generate_mesh` as `_boi`. It contains `interactables` (crafting benches, forges — always placed by site generators), `smokers` (fireplaces in houses), and `one_way_walls` (building entrances). Any of these present → the chunk contains site structures → use greedy mesh instead.

**The issue (Fix 2):** The current `smooth_density_field` applies a single 3×3×3 box filter. All 27 neighbours get equal weight. This creates only ~1 voxel of transition at solid/air boundaries, resulting in sharp density gradients. The Transvoxel normals are gradient-based, so sharp density gradients produce harsh normals and visible triangle edges.

**Why Gaussian works:** A weighted kernel (center > faces > edges > corners) creates a wider, softer falloff zone. Multiple passes compound the effect. The surface normal varies more gradually across triangles, and adjacent triangles blend better.

---

## File Map

| File | Change |
|------|--------|
| `voxygen/src/mesh/terrain.rs` | Fix 1: rename `_boi` → `boi`, add `has_structures` check before Transvoxel |
| `common/src/terrain/density.rs` | Fix 2: replace box-filter with Gaussian-weighted, accept `passes` param |
| `voxygen/src/mesh/terrain.rs` | Fix 2: pass `passes` to `smooth_density_field` based on `TerrainSmoothingMode` |

---

## Fix 1: Greedy fallback for site/structure chunks

### Task 1.1: Detect site chunks in `generate_mesh`

**Files:**
- Modify: `voxygen/src/mesh/terrain.rs` (Transvoxel path entry, around line 264)

- [ ] **Step 1: Read the current function signature and Transvoxel entry point**

```bash
sed -n '235,275p' voxygen/src/mesh/terrain.rs
```

The current parameter destructure is:
```rust
(range, max_texture_size, _boi, smoothing): (
    Aabb<i32>,
    Vec2<u16>,
    &'a BlocksOfInterest,
    TerrainSmoothingMode,
),
```

- [ ] **Step 2: Rename `_boi` to `boi` in the parameter destructure**

Change:
```rust
(range, max_texture_size, _boi, smoothing): (
```
To:
```rust
(range, max_texture_size, boi, smoothing): (
```

- [ ] **Step 3: Add structure detection and early fallthrough**

Immediately after the `if smoothing != TerrainSmoothingMode::Disabled {` line (before building the density field), add a guard that falls through to greedy when structures are present:

```rust
if smoothing != TerrainSmoothingMode::Disabled {
    // If this chunk contains site structures (fireplaces, crafting stations,
    // building entrances), the Transvoxel algorithm would destroy their flat
    // vertical wall geometry. Fall back to the greedy mesher instead.
    let has_structures = !boi.interactables.is_empty()
        || !boi.smokers.is_empty()
        || !boi.one_way_walls.is_empty();

    if !has_structures {
        use crate::render::Tri;
        // ... rest of transvoxel path unchanged
```

And close the `if !has_structures` block with `}` before the existing `}` that closes the Transvoxel path.

The full structure becomes:
```rust
if smoothing != TerrainSmoothingMode::Disabled {
    let has_structures = !boi.interactables.is_empty()
        || !boi.smokers.is_empty()
        || !boi.one_way_walls.is_empty();

    if !has_structures {
        use crate::render::Tri;
        let s = range.size();
        let padded_size = Vec3::new((s.w + 2) as u32, (s.h + 2) as u32, (s.d + 2) as u32);
        let offset = range.min - Vec3::new(1, 1, 1);
        let mut density = convert_chunk_to_density_field(vol, offset, padded_size);
        smooth_density_field(&mut density);
        let tris = mesh_transvoxel(&density);

        // ... [all existing atlas + mesh emission code unchanged] ...

        return (
            Mesh::new(),
            Mesh::new(),
            smooth_opaque_mesh,
            ( bounds, atlas_data, atlas_size, Arc::new(|_| 1.0f32),
              Arc::new(|_| 0.0f32),
              AltIndices { deep_end: 0, underground_end: 0 },
              sun_occluder_z_bounds ),
        );
    } // end if !has_structures
} // end if smoothing != Disabled
```

When `has_structures == true`, the code falls through to the existing greedy mesher path at the bottom of `generate_mesh`.

- [ ] **Step 4: Compile check**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-voxygen 2>&1 | tail -5
```

Expected: `Finished dev profile`.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/mesh/terrain.rs
git commit -m "fix(mesh): fall back to greedy mesher for site/structure chunks"
```

---

### Task 1.2: Test in-game — structures visible again

- [ ] **Step 6: Launch game and verify**

```bash
source "$HOME/.cargo/env" && ./target/debug/veloren-voxygen > /tmp/veloren-fix1.log 2>&1 &
```

Navigate to a town/settlement with smooth terrain enabled. Expected:
- Building walls are visible again (not transparent)
- Natural terrain (hills, plains, cliffs) still uses smooth Transvoxel mesh
- The transition between smooth terrain and greedy structure chunks is abrupt (will improve in future)

Note: chunks with ONLY plain house walls (no interactables/smokers/one_way_walls) will NOT yet fall back to greedy. This is acceptable for a first pass. Track as a known limitation.

---

## Fix 2: Improved density field smoothing (Gaussian + multi-pass)

### Task 2.1: Add Gaussian-weighted smoothing to `density.rs`

**Files:**
- Modify: `common/src/terrain/density.rs`

The current `smooth_density_field` uses a single-pass uniform box filter where all 27 neighbours have equal weight. This creates sharp density gradients and visible triangle edges.

The new approach: a weighted kernel where central neighbours have more influence (distance-based falloff), applied N times. The weight scheme:
- Center (distance 0): weight 8 (×1)
- Face neighbours (distance 1, 6 voxels): weight 4 (×6)
- Edge neighbours (distance √2, 12 voxels): weight 2 (×12)
- Corner neighbours (distance √3, 8 voxels): weight 1 (×8)
- Total weight sum: 8 + 24 + 24 + 8 = **64**

This approximates a 3D Gaussian kernel. Multiple passes compound the smoothing effect.

- [ ] **Step 7: Replace `smooth_density_field` with the new implementation**

In `common/src/terrain/density.rs`, replace the existing function (lines 89-112):

```rust
/// Applies a 3×3×3 Gaussian-weighted blur to a `DensityField` in-place, `passes` times.
///
/// The weight kernel approximates a Gaussian falloff by distance:
/// - center: 8, face-adjacent (6): 4, edge-adjacent (12): 2, corner (8): 1
/// - total weight = 64
///
/// Multiple passes compound the smoothing for wider, softer transitions.
/// Typical values: `passes = 1` for Soft quality, `passes = 3` for Smooth/Ultra.
pub fn smooth_density_field(field: &mut DensityField, passes: u8) {
    let mut snap_data = field.data.clone();

    for _ in 0..passes {
        let snap = DensityField {
            data: snap_data.clone(),
            size: field.size,
        };

        for x in 0..field.size.x as i32 {
            for y in 0..field.size.y as i32 {
                for z in 0..field.size.z as i32 {
                    let center = snap.get_or_zero(Vec3::new(x, y, z)) as u32;

                    // Face neighbours (±1 on exactly one axis) — weight 4
                    let face_sum = [(1,0,0),(-1,0,0),(0,1,0),(0,-1,0),(0,0,1),(0,0,-1)]
                        .iter()
                        .map(|&(dx,dy,dz)| snap.get_or_zero(Vec3::new(x+dx, y+dy, z+dz)) as u32)
                        .sum::<u32>();

                    // Edge neighbours (±1 on exactly two axes) — weight 2
                    let edge_sum = [(1,1,0),(1,-1,0),(-1,1,0),(-1,-1,0),
                                    (1,0,1),(1,0,-1),(-1,0,1),(-1,0,-1),
                                    (0,1,1),(0,1,-1),(0,-1,1),(0,-1,-1)]
                        .iter()
                        .map(|&(dx,dy,dz)| snap.get_or_zero(Vec3::new(x+dx, y+dy, z+dz)) as u32)
                        .sum::<u32>();

                    // Corner neighbours (±1 on all three axes) — weight 1
                    let corner_sum = [(1,1,1),(1,1,-1),(1,-1,1),(1,-1,-1),
                                      (-1,1,1),(-1,1,-1),(-1,-1,1),(-1,-1,-1)]
                        .iter()
                        .map(|&(dx,dy,dz)| snap.get_or_zero(Vec3::new(x+dx, y+dy, z+dz)) as u32)
                        .sum::<u32>();

                    // Weighted sum / 64, rounded to nearest
                    let weighted = 8 * center + 4 * face_sum + 2 * edge_sum + corner_sum;
                    let blended = ((weighted + 32) / 64) as u8;
                    field.set(Vec3::new(x, y, z), blended);
                }
            }
        }
        snap_data = field.data.clone();
    }
}
```

- [ ] **Step 8: Update tests in `density.rs` to pass the new `passes` parameter**

Find `smooth_density_field(&mut field)` in the `#[cfg(test)]` section and replace with `smooth_density_field(&mut field, 1)`.

There are 3 occurrences in tests. All 3 change to `smooth_density_field(&mut field, 1)`.

- [ ] **Step 9: Compile check**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -5
```

Expected: `Finished dev profile`.

---

### Task 2.2: Wire `passes` into `generate_mesh` based on quality level

**Files:**
- Modify: `voxygen/src/mesh/terrain.rs`

The `smooth_density_field` call currently has no `passes` argument. Wire the number of passes to the `TerrainSmoothingMode` quality setting:

| Mode | Passes | Rationale |
|------|--------|-----------|
| `Soft` | 1 | Minimal smoothing, lowest cost |
| `Smooth` | 2 | Noticeably softer surface |
| `Ultra` | 3 | Maximum softness, wider transition zone |

- [ ] **Step 10: Replace the `smooth_density_field` call in `generate_mesh`**

Find:
```rust
smooth_density_field(&mut density);
```

Replace with:
```rust
let smooth_passes = match smoothing {
    TerrainSmoothingMode::Soft   => 1,
    TerrainSmoothingMode::Smooth => 2,
    TerrainSmoothingMode::Ultra  => 3,
    TerrainSmoothingMode::Disabled => unreachable!(),
};
smooth_density_field(&mut density, smooth_passes);
```

- [ ] **Step 11: Full compile + test**

```bash
source "$HOME/.cargo/env"
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common 2>&1 | tail -10
cargo build -p veloren-voxygen 2>&1 | tail -5
```

Expected: all tests pass, binary builds.

- [ ] **Step 12: Clippy clean**

```bash
cargo clippy -p veloren-common -p veloren-voxygen -- -D warnings 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 13: Commit**

```bash
git add common/src/terrain/density.rs voxygen/src/mesh/terrain.rs
git commit -m "fix(terrain): replace box filter with Gaussian smoothing; add multi-pass support"
```

---

### Task 2.3: Test in-game — smoother surface

- [ ] **Step 14: Launch game and verify with Smooth mode**

```bash
source "$HOME/.cargo/env" && ./target/debug/veloren-voxygen > /tmp/veloren-fix2.log 2>&1 &
```

Navigate to natural terrain (hills, cliffs). Compare `Soft` vs `Smooth` mode.

Expected:
- `Soft`: slightly fewer visible triangle edges than before Fix 2
- `Smooth`: noticeably smoother surface, wider transition zones between terrain types
- `Ultra`: near-continuous surface shading on gentle slopes
- No performance regression for `Soft` (single pass = same as before)

---

## Post-fix: PRs back to development and main

After both fixes are verified in-game:

- [ ] **Step 15: Push and create PR to development**

```bash
TOKEN=$(gh auth token)
git push "https://x-token:${TOKEN}@github.com/Matute289/veloren.git" development
gh pr create --repo Matute289/veloren --base main --head development \
  --title "fix: smooth terrain structure fallback + Gaussian smoothing" \
  --body "..."
gh pr merge --repo Matute289/veloren --merge
```

---

## Known limitations (not in scope for this plan)

- Plain building walls with no interactables/smokers/one_way_walls are not yet detected → still get Transvoxeled. Future fix: scan for `BlockKind::Wood` or `BlockKind::Misc` blocks in the range.
- The transition between smooth-terrain chunks and greedy-terrain chunks is abrupt. Future fix: LOD blending or a mixed-mode edge shader.
- Multi-pass smoothing increases mesh generation time linearly with `passes`. For `Ultra` with 3 passes, the 3×3×3 Gaussian loop runs 3× over the ~34³ field. Acceptable for background mesh workers but worth tracking.
