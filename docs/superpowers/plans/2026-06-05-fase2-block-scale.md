# Fase 2 — Block Scale 0.3m → 0.15m Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Halve the world-space size of each block (0.3m → 0.15m) behind a `terrain-hires` feature flag, making the terrain twice as detailed while keeping real-world distances and character proportions identical.

**Architecture:** A single `HIRES_SCALE: f32` constant in `common/src/consts.rs` (= 2.0 when `terrain-hires` is enabled, = 1.0 otherwise) acts as the multiplier. All block-unit constants — physics acceleration/gravity, entity collider sizes, world-gen heights, view distance — multiply by `HIRES_SCALE`. No changes to the Transvoxel pipeline or greedy mesher (they already work in block units). The feature flag lives in `common/Cargo.toml` and propagates through the dependency tree.

**Tech Stack:** Rust nightly, Cargo features, `common` crate (depended on by all others), `world` crate (world gen), `common-systems` (physics), `voxygen` (view distance settings).

**Scale math:** 1 block = 0.3m currently → 0.15m with flag. To maintain the same real-world value for any constant expressed in "blocks":
- Velocities (blocks/s): ×2 so same m/s
- Accelerations (blocks/s²): ×2 so same m/s²
- Heights/distances (blocks): ×2 so same metres
- Friction (dimensionless): no change
- Masses (kg): no change

---

## File Map

| File | Change |
|------|--------|
| `common/Cargo.toml` | Add `terrain-hires = []` feature |
| `world/Cargo.toml` | Propagate feature: `terrain-hires = ["veloren-common/terrain-hires"]` |
| `common/systems/Cargo.toml` | Propagate feature |
| `voxygen/Cargo.toml` | Propagate feature |
| `common/src/consts.rs` | Add `HIRES_SCALE`, `BLOCK_SIZE` constants |
| `common/src/consts.rs` | Scale `GRAVITY`, range constants that are in blocks |
| `common/src/states/utils.rs` | Scale `MOVEMENT_THRESHOLD_VEL` |
| `common/src/comp/body/humanoid.rs` | Scale `height()` return value |
| `common/src/comp/body/mod.rs` | Scale collider dimensions for all body types |
| `world/src/config.rs` | Scale `sea_level`, `mountain_scale` |
| `voxygen/src/settings/graphics.rs` | Scale `terrain_view_distance` default |

---

## Task 1: Feature flag + HIRES_SCALE constant

**Files:**
- Modify: `common/Cargo.toml`
- Modify: `world/Cargo.toml`
- Modify: `common/systems/Cargo.toml`
- Modify: `voxygen/Cargo.toml`
- Modify: `common/src/consts.rs`

- [ ] **Step 1: Find features sections in each Cargo.toml**

```bash
grep -n "^\[features\]" common/Cargo.toml world/Cargo.toml common/systems/Cargo.toml voxygen/Cargo.toml
```

If a `[features]` section doesn't exist in a file, add it after `[dependencies]`.

- [ ] **Step 2: Add `terrain-hires` to `common/Cargo.toml`**

In `common/Cargo.toml`, find `[features]` and add:
```toml
terrain-hires = []
```

- [ ] **Step 3: Propagate feature to dependent crates**

In `world/Cargo.toml`, `[features]` section:
```toml
terrain-hires = ["veloren-common/terrain-hires"]
```

In `common/systems/Cargo.toml`, `[features]` section:
```toml
terrain-hires = ["veloren-common/terrain-hires"]
```

In `voxygen/Cargo.toml`, `[features]` section:
```toml
terrain-hires = ["veloren-common/terrain-hires", "veloren-world/terrain-hires", "veloren-common-systems/terrain-hires"]
```

- [ ] **Step 4: Add HIRES_SCALE and BLOCK_SIZE to `common/src/consts.rs`**

Add these two constants at the top of the file (after any existing doc comment):

```rust
/// World-space size of one block in metres.
#[cfg(feature = "terrain-hires")]
pub const BLOCK_SIZE: f32 = 0.15;
#[cfg(not(feature = "terrain-hires"))]
pub const BLOCK_SIZE: f32 = 0.30;

/// Multiplier applied to all block-unit constants to maintain real-world scale
/// when the block size is halved. 2.0 with `terrain-hires`, 1.0 otherwise.
#[cfg(feature = "terrain-hires")]
pub const HIRES_SCALE: f32 = 2.0;
#[cfg(not(feature = "terrain-hires"))]
pub const HIRES_SCALE: f32 = 1.0;
```

- [ ] **Step 5: Compile check (flag OFF — must behave identically)**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -5
```
Expected: `Finished dev profile`

- [ ] **Step 6: Compile check (flag ON)**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common --features veloren-common/terrain-hires 2>&1 | tail -5
```
Expected: `Finished dev profile`

- [ ] **Step 7: Commit**

```bash
git add common/Cargo.toml world/Cargo.toml common/systems/Cargo.toml voxygen/Cargo.toml common/src/consts.rs
git commit -m "feat(terrain-hires): add feature flag and HIRES_SCALE/BLOCK_SIZE constants"
```

---

## Task 2: Scale physics constants

Physics in Veloren operates in block units (positions, velocities, accelerations are all in blocks/s, blocks/s²). When blocks are half the real-world size, all these values must double to maintain the same real feel.

**Files:**
- Modify: `common/src/consts.rs` (GRAVITY)
- Modify: `common/src/states/utils.rs` (MOVEMENT_THRESHOLD_VEL)

- [ ] **Step 8: Scale GRAVITY in `common/src/consts.rs`**

Find:
```rust
pub const GRAVITY: f32 = 25.0;
```
Change to:
```rust
pub const GRAVITY: f32 = 25.0 * HIRES_SCALE;
```

**Why:** GRAVITY is in blocks/s². At 0.3m/block, 25.0 blocks/s² = 7.5 m/s² (game feel). At 0.15m/block we need 50.0 blocks/s² to maintain 7.5 m/s².

- [ ] **Step 9: Scale MOVEMENT_THRESHOLD_VEL in `common/src/states/utils.rs`**

Find (line ~50):
```rust
pub const MOVEMENT_THRESHOLD_VEL: f32 = 3.0;
```
Change to:
```rust
pub const MOVEMENT_THRESHOLD_VEL: f32 = 3.0 * common::consts::HIRES_SCALE;
```

Add import at top if not present: `use common::consts::HIRES_SCALE;`

Note: `base_accel()` values (100.0 for humanoid, etc.) are also in blocks/s² and should be scaled. However, scaling hundreds of species values is a scope of its own — see Task 7 (ongoing audit). Start with GRAVITY and the movement threshold.

- [ ] **Step 10: Compile check both modes**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -3
cargo build -p veloren-common --features veloren-common/terrain-hires 2>&1 | tail -3
```
Both: `Finished dev profile`

- [ ] **Step 11: Commit**

```bash
git add common/src/consts.rs common/src/states/utils.rs
git commit -m "feat(terrain-hires): scale GRAVITY and movement threshold by HIRES_SCALE"
```

---

## Task 3: Scale entity collider sizes

Entity colliders are in block units. A humanoid at ~1.73-2.5m real height needs to be proportionally taller when blocks are smaller.

**Files:**
- Modify: `common/src/comp/body/humanoid.rs` (height function)
- Modify: `common/src/comp/body/mod.rs` (dimensions for non-humanoid bodies)

- [ ] **Step 12: Read humanoid height function**

```bash
grep -n "fn height\|fn scaler\|fn dimensions" common/src/comp/body/humanoid.rs | head -10
sed -n '83,95p' common/src/comp/body/humanoid.rs
```

The current function should look like:
```rust
pub fn height(&self) -> f32 { (20.0 / 9.0) * self.scaler() }
```

- [ ] **Step 13: Scale humanoid height**

Change to:
```rust
pub fn height(&self) -> f32 { (20.0 / 9.0) * self.scaler() * common::consts::HIRES_SCALE }
```

**Why:** `height()` returns blocks. At 0.3m/block, a humanoid is ~1.73-2.5m = 5.77-8.33 blocks. At 0.15m/block, the same 1.73-2.5m character needs 11.5-16.7 blocks.

- [ ] **Step 14: Verify collider uses height()**

```bash
grep -n "humanoid\|height()\|dimensions()" common/src/comp/body/mod.rs | grep -i "humanoid" | head -10
```

The dimensions for Humanoid should be `Vec3::new(height/1.7, 0.8, height)` or similar. Verify the width `0.8` is also scaled:

Find in `mod.rs`:
```rust
Body::Humanoid(humanoid) => {
    let height = humanoid.height();
    Vec3::new(height / 1.7, 0.8, height)
},
```
Change `0.8` to `0.8 * common::consts::HIRES_SCALE` so width scales too:
```rust
Body::Humanoid(humanoid) => {
    let height = humanoid.height();
    Vec3::new(height / 1.7, 0.8 * common::consts::HIRES_SCALE, height)
},
```

(The `/ 1.7` ratio stays: it's a proportion of height, which already scales.)

- [ ] **Step 15: Note remaining body dimensions as known-gap**

The `dimensions()` match for all other Body variants (QuadrupedSmall, QuadrupedMedium, BipedLarge, Golem, Dragon, etc.) contains hundreds of hardcoded Vec3 values in blocks. These are tracked in **Task 7** (ongoing audit). For now, only humanoid (the player character) is scaled.

- [ ] **Step 16: Compile check**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-common 2>&1 | tail -3
cargo build -p veloren-common --features veloren-common/terrain-hires 2>&1 | tail -3
```

- [ ] **Step 17: Commit**

```bash
git add common/src/comp/body/humanoid.rs common/src/comp/body/mod.rs
git commit -m "feat(terrain-hires): scale humanoid collider height and width by HIRES_SCALE"
```

---

## Task 4: Scale world gen primary constants

World gen heights are in block units. Sea level and mountain scale must double to maintain the same real-world terrain elevations.

**Files:**
- Modify: `world/src/config.rs`

- [ ] **Step 18: Read config.rs constant definitions**

```bash
sed -n '55,75p' world/src/config.rs
```

Expected output should show hardcoded values:
```rust
sea_level: 140.0,
mountain_scale: 2048.0,
```

- [ ] **Step 19: Scale sea_level and mountain_scale**

Find the struct literal (likely in a `Default` impl or `lazy_static`/`once_cell` init):

```rust
sea_level: 140.0,
mountain_scale: 2048.0,
```

Change to (add import `use common::consts::HIRES_SCALE;` at top of file):
```rust
sea_level: 140.0 * HIRES_SCALE,
mountain_scale: 2048.0 * HIRES_SCALE,
```

- [ ] **Step 20: Compile check with terrain-hires**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-world 2>&1 | tail -3
cargo build -p veloren-world --features veloren-world/terrain-hires 2>&1 | tail -3
```

- [ ] **Step 21: Commit**

```bash
git add world/src/config.rs
git commit -m "feat(terrain-hires): scale world gen sea_level and mountain_scale by HIRES_SCALE"
```

---

## Task 5: Scale view distance default

With half-sized blocks, chunks cover half the real-world area. Default view distance must double so players see the same real-world distance.

**Files:**
- Modify: `voxygen/src/settings/graphics.rs`

- [ ] **Step 22: Find view distance defaults**

```bash
grep -n "terrain_view_distance\|view_distance" voxygen/src/settings/graphics.rs | head -20
```

Note: there are multiple preset levels (low, medium, high, ultra). All need to be adjusted.

- [ ] **Step 23: Scale terrain_view_distance in all presets**

For each preset function (e.g., `into_low()`, `into_medium()`, `into_high()`, `into_ultra()`, `default()`):

Find patterns like:
```rust
terrain_view_distance: 10,
```
Change to:
```rust
terrain_view_distance: (10.0 * common::consts::HIRES_SCALE) as u32,
```

For all view distance values: multiply by HIRES_SCALE.

Read the actual values first:
```bash
grep -n "terrain_view_distance" voxygen/src/settings/graphics.rs
```

Then update each one to `(original_value as f32 * common::consts::HIRES_SCALE) as u32`.

- [ ] **Step 24: Compile check voxygen**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-voxygen 2>&1 | tail -3
cargo build -p veloren-voxygen --features veloren-voxygen/terrain-hires 2>&1 | tail -3
```

- [ ] **Step 25: Commit**

```bash
git add voxygen/src/settings/graphics.rs
git commit -m "feat(terrain-hires): scale terrain_view_distance defaults by HIRES_SCALE"
```

---

## Task 6: Integration test — run game with terrain-hires

- [ ] **Step 26: Build and run with flag enabled**

```bash
source "$HOME/.cargo/env" && cargo run --bin veloren-voxygen --features veloren-voxygen/terrain-hires > /tmp/veloren-hires.log 2>&1 &
```

Log should show normal startup. Check for panics:
```bash
sleep 30 && grep -i "panic\|error\|FATAL" /tmp/veloren-hires.log | grep -v winit | head -5
```

- [ ] **Step 27: Visual verification checklist**

Enter single-player and verify:
- [ ] Character height feels the same in real-world terms (not larger/smaller relative to doorways)
- [ ] Jump height feels the same (same physical height, but twice as many blocks)
- [ ] Gravity feels the same (fall speed similar to before)
- [ ] World terrain generates with mountains of similar visual scale
- [ ] No crashes or panics

Take screenshots for comparison.

- [ ] **Step 28: Run without flag (regression check)**

```bash
source "$HOME/.cargo/env" && cargo run --bin veloren-voxygen > /tmp/veloren-nohires.log 2>&1 &
```

Verify behavior is identical to pre-Fase-2 baseline (no regressions).

---

## Task 7: Ongoing audit — remaining block-unit constants

This task tracks all block-unit constants NOT yet scaled. It is a **living checklist** — check off items as they are implemented in follow-up commits.

### Physics (common/src/states/utils.rs)

All species `base_accel()` values are in blocks/s² and need ×2:
- [ ] `Body::Humanoid(_) => 100.0` → `100.0 * HIRES_SCALE`
- [ ] `Body::QuadrupedSmall` species table (~15 entries)
- [ ] `Body::QuadrupedMedium` species table (~20 entries)
- [ ] `Body::QuadrupedLow` species table
- [ ] `Body::BipedLarge` species table
- [ ] `Body::BipedSmall` species table
- [ ] All other body types

Strategy: global search-replace `=> (\d+\.0),` with `=> $1 * HIRES_SCALE,` in base_accel() match arms.

### Entity collider dimensions (common/src/comp/body/mod.rs)

All `Vec3::new(x, y, z)` entries in `dimensions()` match arms need ×HIRES_SCALE:
- [ ] QuadrupedSmall (~30 species)
- [ ] QuadrupedMedium (~25 species)
- [ ] QuadrupedLow (~20 species)
- [ ] BipedLarge (~20 species)
- [ ] BipedSmall
- [ ] Golem (~8 species)
- [ ] Dragon (~4 species)
- [ ] Arthropod, Crustacean, Theropod, etc.

Strategy: For each variant, multiply all three Vec3 components by `HIRES_SCALE`.

### World gen (world/src/)

Many constants throughout `world/src/sim/`, `world/src/layer/`, `world/src/site/` are in block units:
- [ ] `world/src/sim/mod.rs` — erosion heights, continent parameters
- [ ] `world/src/layer/cave.rs` — cave sizes and depths
- [ ] `world/src/layer/scatter.rs` — tree/shrub placement heights
- [ ] `world/src/site/` — site floor heights, building sizes
- [ ] `world/src/util/` — any distance/height utilities

Strategy: Use `grep -rn "[0-9]\+\.0" world/src/ | grep -v "//\|test\|probability\|temperature\|humidity"` to find candidate numeric literals, then audit each one.

### Networking / Server

- [ ] `server/src/settings.rs` — max_view_distance, default view distance
- [ ] `client/src/lib.rs` — view distance requests

### Save migration

Block coordinates in existing saves (`userdata/`) are in old block units. Loading old saves with terrain-hires enabled will place characters at half the correct real-world height.

Strategy: Version the save format. When terrain-hires is detected, multiply all stored block coordinates by 2 on load. This is a one-time migration.

---

## Activate the flag for production

When all items in Task 7 are complete and tested:

```bash
# Build and test full game
source "$HOME/.cargo/env" && cargo run --bin veloren-voxygen --features veloren-voxygen/terrain-hires

# When satisfied, add to default features in voxygen/Cargo.toml:
# default = [..., "terrain-hires"]
```

Once added to `default`, all builds will use terrain-hires without needing to pass the flag explicitly.

---

## Known limitations (not in scope for this plan)

- **Network protocol**: chunk wire format doesn't change (still 32×32 blocks), but with smaller blocks, same view distance requires 4× more chunks network traffic. `terrain-hires` should only be enabled in single-player until network optimization is addressed.
- **Performance**: 4× more terrain chunks to render + mesh at same view distance. Test FPS before enabling by default.
- **Asset rescaling**: `.vox` models and their `.ron` manifest scale values may need adjustment to look proportional in the hires world.
