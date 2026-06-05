# Smooth Terrain Physics — Floor Correction + Structure Detection

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the "walking in air" issue caused by the Transvoxel visual mesh rendering a smooth slope while physics resolves collision against the original block staircase. Also: extend structure detection to catch more building types, and investigate the unrelated input spinning bug.

**Architecture:**
- **Physics fix:** Add a `SmoothTerrainSettings` resource to `common-state` (passes count, enabled flag). Voxygen writes this resource when user changes graphics. The physics system reads it and, after block collision resolves the entity Z, samples the smooth isosurface height at the entity's XY using a tiny density field and snaps the entity to the smooth floor.
- **Structure detection:** Scan a sample of blocks in the chunk range for `BlockKind::Wood`, `BlockKind::Rock`, and other structural kinds — in addition to the existing `boi` check.
- **Spinning bug:** Investigate Veloren's input system on macOS when unpausing. Likely a controller/gamepad drift or a stuck mouse-delta event. Not related to terrain code.

**Tech Stack:** Rust, specs ECS, `common/src/terrain/density.rs`, `common/systems/src/phys/collision.rs`, `common-state/src/lib.rs`.

---

## Root Cause Analysis

**Why players float above smooth terrain:**
- Block physics resolves collision at integer block boundaries (staircase topology).
- Transvoxel visual mesh renders a smooth slope between block levels.
- On a downhill slope: the player walks on block tops (correct physics), but the smooth visual surface cuts BELOW those block tops → player appears to float above the rendered terrain.
- The mismatch is at most ~1 block height on a 45° slope.

**Why some walls are still invisible:**
- `has_structures` checks `boi.interactables`, `boi.smokers`, `boi.one_way_walls`.
- Many buildings only have wood walls — no crafting stations, fireplaces, or entrances in `boi`.
- Those chunks still use Transvoxel → walls invisible.

**Spinning bug:**
- Completely unrelated to terrain code. Likely:
  1. macOS: connected gamepad/controller with stick drift.
  2. Stuck mouse-delta event from opening the pause menu (mouse capture released then re-acquired incorrectly).
  - To confirm: check if it happens with smooth terrain **disabled**. If yes → pre-existing bug, file as separate issue.

---

## File Map

| File | Change |
|------|--------|
| `common/src/terrain/density.rs` | Add `sample_isosurface_z` function |
| `common/src/lib.rs` | Re-export `SmoothTerrainSettings` if needed |
| `common-state/src/lib.rs` | Register `SmoothTerrainSettings` resource |
| `common/systems/src/phys_smooth.rs` | Fix stale `smooth_density_field()` call (missing passes arg) |
| `common/systems/src/phys/mod.rs` | Read `SmoothTerrainSettings` in SystemData |
| `common/systems/src/phys/collision.rs` | After on-ground snap, apply smooth floor correction |
| `voxygen/src/scene/mod.rs` | Sync `TerrainSmoothingMode` → `SmoothTerrainSettings` resource |
| `voxygen/src/mesh/terrain.rs` | Extend `has_structures` to scan for structural block kinds |

---

## Task 1: Fix stale `smooth_density_field` call in phys_smooth.rs

**File:** `common/systems/src/phys_smooth.rs:492`

- [ ] **Step 1: Replace the stale call**

Find:
```rust
smooth_density_field(&mut field);
```
Replace with:
```rust
smooth_density_field(&mut field, 1);
```

- [ ] **Step 2: Compile check**
```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common-systems 2>&1 | tail -5
```
Expected: `Finished dev profile`.

- [ ] **Step 3: Commit**
```bash
git add common/systems/src/phys_smooth.rs
git commit -m "fix(phys_smooth): pass missing `passes` arg to smooth_density_field"
```

---

## Task 2: Add `sample_isosurface_z` to density.rs

This function finds the exact Z coordinate (as a float) where the density field crosses 127 at a given (x, y). Used by physics to snap entities to the smooth floor.

**File:** `common/src/terrain/density.rs`

- [ ] **Step 4: Add the function after `smooth_density_field`**

```rust
/// Returns the interpolated Z where density crosses 127 when descending from `z_start`
/// downward in the field at column (x, y). Returns `None` if no crossing found.
///
/// Used by physics to find the smooth isosurface floor height.
pub fn sample_isosurface_z(field: &DensityField, x: i32, y: i32, z_start: i32) -> Option<f32> {
    let z_min = 0i32;
    let z_max = (field.size.z as i32).min(z_start + 1);
    for z in (z_min..z_max).rev() {
        let d_above = field.get_or_zero(Vec3::new(x, y, z + 1)) as i32;
        let d_here  = field.get_or_zero(Vec3::new(x, y, z)) as i32;
        // Crossing: d_above < 127 and d_here >= 127
        if d_above < 127 && d_here >= 127 {
            // Linear interpolation between z and z+1
            let t = (127 - d_here) as f32 / (d_above - d_here) as f32;
            return Some(z as f32 + t.clamp(0.0, 1.0));
        }
    }
    None
}
```

- [ ] **Step 5: Compile check**
```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -5
```
Expected: `Finished dev profile`.

---

## Task 3: Add `SmoothTerrainSettings` resource to common-state

The physics system (common-systems) can't access `voxygen`'s `TerrainSmoothingMode` directly (different crate). We create a lightweight resource in `common-state` that voxygen writes and physics reads.

**Files:**
- Create: `common-state/src/smooth_settings.rs`
- Modify: `common-state/src/lib.rs`

- [ ] **Step 6: Create `common-state/src/smooth_settings.rs`**

```rust
/// Number of Gaussian smoothing passes applied to terrain density fields.
/// 0 = smooth terrain disabled (use block physics only).
/// Set by the graphics system from TerrainSmoothingMode; read by physics.
#[derive(Clone, Copy, Debug, Default)]
pub struct SmoothTerrainSettings {
    pub passes: u8,
}
```

- [ ] **Step 7: Register in `common-state/src/lib.rs`**

Find the block where resources are registered with `.insert(...)` or similar. Add:
```rust
pub mod smooth_settings;
pub use smooth_settings::SmoothTerrainSettings;
```

And in the ECS world setup (wherever `State::new` or similar initializes resources):
```rust
ecs.insert(SmoothTerrainSettings::default());
```

To find the right location:
```bash
grep -n "insert\|register\|add_resource" common-state/src/lib.rs | head -30
```

- [ ] **Step 8: Compile check**
```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common-state 2>&1 | tail -5
```
Expected: `Finished dev profile`.

---

## Task 4: Sync `TerrainSmoothingMode` → `SmoothTerrainSettings` in voxygen

**File:** `voxygen/src/scene/mod.rs`

- [ ] **Step 9: Write the resource whenever terrain smoothing mode is read**

In `voxygen/src/scene/mod.rs`, find where `terrain_smoothing` is read from settings (it's in the `Scene` or the tick function that updates the scene). After reading it to pass to terrain meshing, also write to the ECS resource.

```bash
grep -n "terrain_smoothing\|smoothing" voxygen/src/scene/mod.rs | head -20
```

The update should look like:
```rust
// After reading terrain_smoothing from settings:
let passes = match settings.graphics.terrain_smoothing {
    TerrainSmoothingMode::Disabled => 0,
    TerrainSmoothingMode::Soft     => 1,
    TerrainSmoothingMode::Smooth   => 2,
    TerrainSmoothingMode::Ultra    => 3,
};
ecs.write_resource::<veloren_common_state::SmoothTerrainSettings>()
    .passes = passes;
```

Find the correct place by checking where `settings.graphics.terrain_smoothing` is currently read in the scene tick:
```bash
grep -n "terrain_smoothing\|smoothing" voxygen/src/scene/mod.rs
```

- [ ] **Step 10: Add import**

Add to the use block in `voxygen/src/scene/mod.rs`:
```rust
use veloren_common_state::SmoothTerrainSettings;
```

- [ ] **Step 11: Compile check**
```bash
source "$HOME/.cargo/env" && cargo build -p veloren-voxygen 2>&1 | tail -5
```
Expected: `Finished dev profile`.

---

## Task 5: Apply smooth floor correction in physics

**File:** `common/systems/src/phys/collision.rs`

This is the core physics fix. After block collision resolves the entity Z position and sets `on_ground`, if smooth terrain is enabled, sample the smooth isosurface at the entity's XY and snap Z to the smooth floor if it's within 1 block.

- [ ] **Step 12: Add imports to `collision.rs`**

```rust
use common::terrain::density::{
    convert_chunk_to_density_field, sample_isosurface_z, smooth_density_field,
};
```

- [ ] **Step 13: Add `smooth_passes` parameter to `box_voxel_collision`**

The function signature is:
```rust
pub(super) fn box_voxel_collision<T: BaseVol<Vox = Block> + ReadVol>(
    ...
    terrain: &T,
    ...
```

Add `smooth_passes: u8` as the last parameter:
```rust
pub(super) fn box_voxel_collision<T: BaseVol<Vox = Block> + ReadVol>(
    ...
    terrain: &T,
    ...
    smooth_passes: u8,
```

- [ ] **Step 14: Find the call site in `mod.rs` and pass the value**

```bash
grep -n "box_voxel_collision" common/systems/src/phys/mod.rs | head -5
```

The call site passes `smooth_passes: u8` from `read.smooth_terrain.passes` (the resource we added).

In `PhysicsRead` struct:
```rust
smooth_terrain: Read<'a, veloren_common_state::SmoothTerrainSettings>,
```

Pass to `box_voxel_collision`:
```rust
box_voxel_collision(
    ...,
    &read.terrain,
    ...,
    read.smooth_terrain.passes,
);
```

- [ ] **Step 15: Add smooth floor snap inside `box_voxel_collision`**

After line ~288 where `on_ground` is checked and entity Z is snapped to block surface (the block at `pos.0.z - 0.1`), add:

```rust
// Smooth floor correction: if smooth terrain is enabled and the entity is on the
// ground, snap Z to the smooth isosurface to match the visual mesh.
if smooth_passes > 0 && physics_state.on_ground.is_some() {
    let foot = pos.0;
    // Build a tiny density field: 3×3×5 voxels around the entity's feet.
    let sample_offset = Vec3::new(
        (foot.x - 1.0).floor() as i32,
        (foot.y - 1.0).floor() as i32,
        (foot.z - 3.0).floor() as i32,
    );
    let sample_size = Vec3::new(3u32, 3, 5);
    let mut density = convert_chunk_to_density_field(terrain, sample_offset, sample_size);
    smooth_density_field(&mut density, smooth_passes);

    // Entity's local position within the sample field.
    let fx = (foot.x - sample_offset.x as f32).round() as i32;
    let fy = (foot.y - sample_offset.y as f32).round() as i32;
    let fz = (foot.z - sample_offset.z as f32).ceil() as i32;

    if let Some(smooth_z_local) = sample_isosurface_z(&density, fx, fy, fz) {
        let smooth_z_world = smooth_z_local + sample_offset.z as f32;
        // Only snap if the smooth floor is within 1 block of the block floor.
        let block_floor = pos.0.z;
        if (smooth_z_world - block_floor).abs() < 1.0 {
            pos.0.z = smooth_z_world;
        }
    }
}
```

- [ ] **Step 16: Full compile check**
```bash
source "$HOME/.cargo/env" && cargo build -p veloren-voxygen 2>&1 | tail -5
```
Expected: `Finished dev profile`.

- [ ] **Step 17: Run tests**
```bash
source "$HOME/.cargo/env" && VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-common-systems 2>&1 | tail -10
```
Expected: all tests pass.

- [ ] **Step 18: Commit**
```bash
git add \
  common/src/terrain/density.rs \
  common-state/src/smooth_settings.rs \
  common-state/src/lib.rs \
  common/systems/src/phys_smooth.rs \
  common/systems/src/phys/mod.rs \
  common/systems/src/phys/collision.rs \
  voxygen/src/scene/mod.rs
git commit -m "feat(physics): smooth floor correction — snap entity Z to Transvoxel isosurface"
```

---

## Task 6: Test smooth physics in-game

- [ ] **Step 19: Build and run**
```bash
source "$HOME/.cargo/env" && cargo run --bin veloren-voxygen > /tmp/veloren-smooth-phys.log 2>&1 &
```

Navigate to terrain with slopes/hills with smooth mode enabled.

**Expected:**
- Player walks smoothly on slopes without floating above the mesh
- Player does NOT clip through terrain (smooth floor should never be below block floor)
- Buildings still use greedy mesh (Fix 1 is intact)
- `Soft` mode: barely noticeable correction
- `Smooth`/`Ultra`: more pronounced smooth walking on slopes

**If player clips through terrain (Z drops below block floor):**
The `(smooth_z_world - block_floor).abs() < 1.0` guard should prevent this. If it still happens, tighten the guard:
```rust
if smooth_z_world > block_floor - 0.3 && smooth_z_world < block_floor + 1.0 {
    pos.0.z = smooth_z_world;
}
```

---

## Task 7: Extend structure detection for wood-only buildings

**File:** `voxygen/src/mesh/terrain.rs`

Some buildings have walls but no crafting stations, fireplaces, or entrances in `boi`. We scan a sample of blocks in the chunk range for structural block kinds.

- [ ] **Step 20: Extend `has_structures` before the Transvoxel path**

Find the current `has_structures` check (around line 268):
```rust
let has_structures = !boi.interactables.is_empty()
    || !boi.smokers.is_empty()
    || !boi.one_way_walls.is_empty();
```

Replace with:
```rust
let has_structures = !boi.interactables.is_empty()
    || !boi.smokers.is_empty()
    || !boi.one_way_walls.is_empty()
    || {
        // Scan a grid of sample points for structural blocks (walls, roofs).
        // Check every 3rd block in XY to balance accuracy vs cost.
        let s = range.size();
        let mut found = false;
        'scan: for sx in (0..s.w).step_by(3) {
            for sy in (0..s.h).step_by(3) {
                for sz in (0..s.d).rev() {
                    let wpos = range.min + Vec3::new(sx as i32, sy as i32, sz as i32);
                    if let Ok(b) = vol.get(wpos) {
                        if b.is_filled() {
                            if matches!(
                                b.kind(),
                                BlockKind::Wood
                                    | BlockKind::Rock
                                    | BlockKind::WeakRock
                                    | BlockKind::Misc
                            ) {
                                found = true;
                                break 'scan;
                            }
                            break; // stop descending this column once we hit the first solid
                        }
                    }
                }
            }
        }
        found
    };
```

Check which `BlockKind` variants exist:
```bash
grep -n "^[[:space:]]*[A-Z][a-zA-Z]*," common/src/terrain/block/mod.rs | head -40
```

Only include structural kinds that are always placed by site generators, not naturally generated (don't include `Earth`, `Snow`, etc.).

- [ ] **Step 21: Compile check**
```bash
source "$HOME/.cargo/env" && cargo build -p veloren-voxygen 2>&1 | tail -5
```
Expected: `Finished dev profile`.

- [ ] **Step 22: Commit**
```bash
git add voxygen/src/mesh/terrain.rs
git commit -m "fix(mesh): extend structure detection to scan for wood/rock block kinds"
```

---

## Task 8: Investigate spinning bug

The spinning-after-unpause bug is not terrain-related. Before spending time on it:

- [ ] **Step 23: Reproduce with smooth terrain DISABLED**

In settings, set terrain smoothing to Disabled. Enter pause menu and unpause. If spinning still occurs → pre-existing Veloren bug, not our code.

- [ ] **Step 24: Check for connected gamepad/controller**

On macOS, a connected gamepad or controller with stick drift can cause character rotation. Disconnect all controllers and test.

- [ ] **Step 25: Check Veloren input handling on macOS**

```bash
grep -n "mouse.*capture\|grab\|cursor_confined\|focus" voxygen/src/window.rs | head -20
```

If the bug is confirmed in unmodified Veloren (with smooth disabled), open a separate issue and do not attempt to fix it in this branch.

---

## Post-fix: PRs back to development and main

After all tasks verified in-game:

- [ ] **Step 26: Push and PR**
```bash
TOKEN=$(gh auth token)
git push "https://x-token:${TOKEN}@github.com/Matute289/veloren.git" development
gh pr create --repo Matute289/veloren --base main --head development \
  --title "feat: smooth terrain physics floor correction + extended structure detection" \
  --body "..."
gh pr merge --repo Matute289/veloren 5 --merge
```

---

## Known Limitations (not in scope)

- Smooth physics only corrects vertical (Z) position. Horizontal collision still uses block AABB → entity can still "feel" block edges on slopes when moving laterally. Full triangle mesh collision would fix this.
- Buildings scanned at step_by(3) — may miss buildings smaller than 3×3 columns. Good enough for first pass.
- The spinning bug may be a pre-existing Veloren input issue on macOS. Tracked separately.
- `sample_isosurface_z` builds a tiny 3×3×5 density field per entity per physics tick (when on ground). This is ~45 block lookups + Gaussian kernel — acceptable for ≤10 entities but may be noticeable at large player counts. If performance is an issue, add a per-chunk cache keyed by chunk coordinate.
