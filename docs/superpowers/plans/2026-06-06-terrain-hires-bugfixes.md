# terrain-hires Bug Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three confirmed bugs found in the terrain-hires code review: incorrect horizontal waypoint scaling, wrong view-distance presets, and an unused dead constant.

**Architecture:** The waypoint migration incorrectly scales X/Y coordinates — only Z (vertical height) should scale because world gen only doubled vertical quantities while horizontal block layout is unchanged. View-distance presets were incorrectly doubled because chunks did not shrink horizontally. `BLOCK_SIZE` is defined but never referenced outside its own declaration.

**Tech Stack:** Rust nightly, `serde_json`, `common::consts`, `server/src/persistence/`, `voxygen/src/settings/`.

---

## File Map

| File | Change |
|------|--------|
| `server/src/persistence/character/conversions.rs` | Fix `convert_waypoint_from_database_json`: only scale `pos.z`, leave `pos.x`/`pos.y` and `map_marker` unchanged |
| `server/src/persistence/character/conversions.rs` | Fix 4 unit tests to assert correct Z-only scaling |
| `voxygen/src/settings/graphics.rs` | Revert 5 `terrain_view_distance` presets: remove `* HIRES_SCALE` |
| `server/src/settings/mod.rs` | Revert `max_view_distance` default: remove `* HIRES_SCALE` |
| `common/src/consts.rs` | Remove unused `BLOCK_SIZE` constant |

---

## Background: why only Z scales

Block coordinates in Veloren are 3D. The horizontal layout of the world (biome placement, site positions, chunk grid) is determined by the world seed at the same X/Y block coordinates in both standard and hires mode. Only the **vertical** world-gen parameters were doubled (`sea_level`, `mountain_scale`, cave depths, scatter heights). Therefore:

- A mountain peak at X=1000, Y=500 in standard is at **the same X=1000, Y=500** in hires (horizontal layout unchanged)
- That mountain's top at Z=800 in standard is at **Z=1600** in hires (vertical doubled)
- A player's map marker at (1000, 500) is a horizontal world position — **unchanged**

---

## Task 1: Fix waypoint Z-only scaling + rewrite tests

**Files:**
- Modify: `server/src/persistence/character/conversions.rs:308-316` (load function)
- Modify: `server/src/persistence/character/conversions.rs:964-1031` (test module)

### Step 1: Read current load function

```bash
sed -n '308,316p' server/src/persistence/character/conversions.rs
```

Expected output:
```rust
    Ok((
        character_position
            .waypoint
            .map(|pos| Waypoint::new(pos * scale_factor, Time(0.0))),
        character_position
            .map_marker
            .map(|pos| MapMarker(pos.map(|v| (v as f32 * scale_factor) as i32))),
    ))
```

### Step 2: Fix the load function — only scale Z

Replace that block with:

```rust
    Ok((
        character_position.waypoint.map(|pos| {
            Waypoint::new(
                vek::Vec3::new(pos.x, pos.y, pos.z * scale_factor),
                Time(0.0),
            )
        }),
        character_position.map_marker.map(MapMarker),
    ))
```

`pos.x` and `pos.y` are horizontal — unchanged. Only `pos.z` (altitude) is scaled. `map_marker` is a 2D horizontal world position — not scaled at all.

### Step 3: Verify it compiles

```bash
source "$HOME/.cargo/env"
cargo build -p veloren-server 2>&1 | tail -3
cargo build -p veloren-server --features veloren-server/terrain-hires 2>&1 | tail -3
```

Both: `Finished dev profile`

### Step 4: Rewrite the tests

The current tests at lines 964–1031 assert incorrect behavior (X/Y/map_marker scaling). Replace the entire `waypoint_scale_tests` module with:

```rust
#[cfg(test)]
mod waypoint_scale_tests {
    use super::convert_waypoint_from_database_json;

    // Standard build: old save (no hires field) — no change to any coord
    #[cfg(not(feature = "terrain-hires"))]
    #[test]
    fn old_save_no_change_in_standard_build() {
        let json = r#"{"waypoint":[100.0,200.0,50.0],"map_marker":[100,200]}"#;
        let (waypoint, map_marker) =
            convert_waypoint_from_database_json(json).expect("must parse");
        let pos = waypoint.unwrap().get_pos();
        assert!((pos.x - 100.0).abs() < 0.01);
        assert!((pos.y - 200.0).abs() < 0.01);
        assert!((pos.z - 50.0).abs() < 0.01);
        let mm = map_marker.unwrap();
        assert_eq!(mm.0.x, 100);
        assert_eq!(mm.0.y, 200);
    }

    // Standard build: hires save → only Z halved, X/Y/map_marker unchanged
    #[cfg(not(feature = "terrain-hires"))]
    #[test]
    fn hires_save_only_z_scaled_down_in_standard_build() {
        let json = r#"{"waypoint":[200.0,400.0,100.0],"map_marker":[200,400],"hires":true}"#;
        let (waypoint, map_marker) =
            convert_waypoint_from_database_json(json).expect("must parse");
        let pos = waypoint.unwrap().get_pos();
        // X and Y are horizontal — unchanged
        assert!(
            (pos.x - 200.0).abs() < 0.01,
            "X must not change, got {}",
            pos.x
        );
        assert!(
            (pos.y - 400.0).abs() < 0.01,
            "Y must not change, got {}",
            pos.y
        );
        // Only Z (altitude) is halved
        assert!(
            (pos.z - 50.0).abs() < 0.01,
            "expected Z=50.0, got {}",
            pos.z
        );
        // map_marker is horizontal — unchanged
        let mm = map_marker.unwrap();
        assert_eq!(mm.0.x, 200, "map_marker X must not change");
        assert_eq!(mm.0.y, 400, "map_marker Y must not change");
    }

    // Hires build: old save → only Z doubled, X/Y/map_marker unchanged
    #[cfg(feature = "terrain-hires")]
    #[test]
    fn old_save_only_z_scaled_up_in_hires_build() {
        let json = r#"{"waypoint":[100.0,200.0,50.0],"map_marker":[100,200]}"#;
        let (waypoint, map_marker) =
            convert_waypoint_from_database_json(json).expect("must parse");
        let pos = waypoint.unwrap().get_pos();
        // X and Y are horizontal — unchanged
        assert!(
            (pos.x - 100.0).abs() < 0.01,
            "X must not change, got {}",
            pos.x
        );
        assert!(
            (pos.y - 200.0).abs() < 0.01,
            "Y must not change, got {}",
            pos.y
        );
        // Only Z (altitude) is doubled
        assert!(
            (pos.z - 100.0).abs() < 0.01,
            "expected Z=100.0, got {}",
            pos.z
        );
        // map_marker is horizontal — unchanged
        let mm = map_marker.unwrap();
        assert_eq!(mm.0.x, 100, "map_marker X must not change");
        assert_eq!(mm.0.y, 200, "map_marker Y must not change");
    }

    // Hires build: hires save — no change to any coord
    #[cfg(feature = "terrain-hires")]
    #[test]
    fn hires_save_no_change_in_hires_build() {
        let json = r#"{"waypoint":[200.0,400.0,100.0],"map_marker":[200,400],"hires":true}"#;
        let (waypoint, map_marker) =
            convert_waypoint_from_database_json(json).expect("must parse");
        let pos = waypoint.unwrap().get_pos();
        assert!((pos.x - 200.0).abs() < 0.01);
        assert!((pos.y - 400.0).abs() < 0.01);
        assert!((pos.z - 100.0).abs() < 0.01);
        let mm = map_marker.unwrap();
        assert_eq!(mm.0.x, 200);
        assert_eq!(mm.0.y, 400);
    }
}
```

Note the renamed test functions: `hires_save_scaled_down` → `hires_save_only_z_scaled_down` and `old_save_scaled_up` → `old_save_only_z_scaled_up`. This makes the intent clear.

### Step 5: Run tests — standard build

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server -- waypoint 2>&1 | tail -12
```

Expected:
```
test persistence::character::conversions::waypoint_scale_tests::old_save_no_change_in_standard_build ... ok
test persistence::character::conversions::waypoint_scale_tests::hires_save_only_z_scaled_down_in_standard_build ... ok
test persistence::json_models::waypoint_migration_tests::old_save_defaults_hires_to_false ... ok
test persistence::json_models::waypoint_migration_tests::hires_save_parses_hires_true ... ok
test result: ok. 4 passed; 0 failed
```

### Step 6: Run tests — hires build

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server --features veloren-server/terrain-hires -- waypoint 2>&1 | tail -12
```

Expected:
```
test persistence::character::conversions::waypoint_scale_tests::old_save_only_z_scaled_up_in_hires_build ... ok
test persistence::character::conversions::waypoint_scale_tests::hires_save_no_change_in_hires_build ... ok
test persistence::json_models::waypoint_migration_tests::old_save_defaults_hires_to_false ... ok
test persistence::json_models::waypoint_migration_tests::hires_save_parses_hires_true ... ok
test result: ok. 4 passed; 0 failed
```

### Step 7: Commit

```bash
git add server/src/persistence/character/conversions.rs
git commit -m "fix(terrain-hires): waypoint migration: only scale Z (altitude), not X/Y or map_marker"
```

---

## Task 2: Revert view-distance presets

**Why this is a bug:** Veloren's view distance is measured in **chunks**. A chunk is always 32×32 blocks (`TerrainChunkSize::RECT_SIZE`). Terrain-hires did not change the horizontal chunk grid — only vertical world-gen parameters were doubled. Doubling view-distance causes the client to load ~4× as many chunks (area scales as VD²) for zero additional horizontal coverage, wasting network bandwidth and GPU memory.

**Files:**
- Modify: `voxygen/src/settings/graphics.rs:59,85,112,139,169,199`
- Modify: `server/src/settings/mod.rs:229`

### Step 8: Read the current preset values

```bash
grep -n "terrain_view_distance\|max_view_distance.*HIRES" \
  voxygen/src/settings/graphics.rs server/src/settings/mod.rs
```

Expected lines to change (graphics.rs):
```
59:  terrain_view_distance: (10.0 * HIRES_SCALE) as u32,   // default
85:  terrain_view_distance: (4.0 * HIRES_SCALE) as u32,    // minimal
112: terrain_view_distance: (7.0 * HIRES_SCALE) as u32,    // low
139: terrain_view_distance: (10.0 * HIRES_SCALE) as u32,   // medium
169: terrain_view_distance: (16.0 * HIRES_SCALE) as u32,   // high
199: terrain_view_distance: (16.0 * HIRES_SCALE) as u32,   // ultra
```

Expected line to change (mod.rs):
```
229: max_view_distance: Some((65.0 * common::consts::HIRES_SCALE) as u32),
```

### Step 9: Fix all view-distance presets in `voxygen/src/settings/graphics.rs`

Replace each `(N.0 * HIRES_SCALE) as u32` with the plain integer `N`:

| Line | Before | After |
|------|--------|-------|
| ~59 (default) | `(10.0 * HIRES_SCALE) as u32` | `10` |
| ~85 (minimal) | `(4.0 * HIRES_SCALE) as u32` | `4` |
| ~112 (low) | `(7.0 * HIRES_SCALE) as u32` | `7` |
| ~139 (medium) | `(10.0 * HIRES_SCALE) as u32` | `10` |
| ~169 (high) | `(16.0 * HIRES_SCALE) as u32` | `16` |
| ~199 (ultra) | `(16.0 * HIRES_SCALE) as u32` | `16` |

Also check if `HIRES_SCALE` is imported/used anywhere else in this file. If `terrain_view_distance` was the only use, remove the import too.

### Step 10: Fix server max_view_distance in `server/src/settings/mod.rs`

Find line ~229:
```rust
max_view_distance: Some((65.0 * common::consts::HIRES_SCALE) as u32),
```

Change to:
```rust
max_view_distance: Some(65),
```

### Step 11: Compile check

```bash
source "$HOME/.cargo/env"
cargo build -p veloren-voxygen 2>&1 | tail -3
cargo build -p veloren-voxygen --features veloren-voxygen/terrain-hires 2>&1 | tail -3
cargo build -p veloren-server 2>&1 | tail -3
cargo build -p veloren-server --features veloren-server/terrain-hires 2>&1 | tail -3
```

All four: `Finished dev profile`

### Step 12: Commit

```bash
git add voxygen/src/settings/graphics.rs server/src/settings/mod.rs
git commit -m "fix(terrain-hires): revert view_distance presets — horizontal chunk grid is unchanged"
```

---

## Task 3: Remove unused BLOCK_SIZE constant

**Files:**
- Modify: `common/src/consts.rs:1-5`

### Step 13: Verify BLOCK_SIZE is unused

```bash
grep -rn "BLOCK_SIZE" --include="*.rs" . | grep -v "target\|\.git\|consts.rs"
```

Expected: no output (zero external references).

### Step 14: Read current consts.rs top

```bash
head -15 common/src/consts.rs
```

Expected:
```rust
#[cfg(feature = "terrain-hires")]
pub const BLOCK_SIZE: f32 = 0.15;
#[cfg(not(feature = "terrain-hires"))]
pub const BLOCK_SIZE: f32 = 0.30;

/// Multiplier applied to all block-unit constants...
#[cfg(feature = "terrain-hires")]
pub const HIRES_SCALE: f32 = 2.0;
#[cfg(not(feature = "terrain-hires"))]
pub const HIRES_SCALE: f32 = 1.0;
```

### Step 15: Remove BLOCK_SIZE (lines 1–5)

Delete the four lines defining `BLOCK_SIZE` (both cfg variants + the blank line after them). The file should start directly with the `HIRES_SCALE` constants after the deletion.

### Step 16: Compile check

```bash
source "$HOME/.cargo/env"
cargo build -p veloren-common 2>&1 | tail -3
cargo build -p veloren-common --features veloren-common/terrain-hires 2>&1 | tail -3
```

Both: `Finished dev profile`

### Step 17: Commit

```bash
git add common/src/consts.rs
git commit -m "chore(terrain-hires): remove unused BLOCK_SIZE constant"
```

---

## Task 4: Clippy + rustfmt

### Step 18: Clippy — standard build

```bash
source "$HOME/.cargo/env"
cargo clippy -p veloren-server -p veloren-voxygen -p veloren-common --locked -- -D warnings 2>&1 | grep "^error" | head -20
```

Expected: no output.

### Step 19: Clippy — hires build

```bash
cargo clippy -p veloren-server -p veloren-common --locked \
  --features "veloren-server/terrain-hires,veloren-common/terrain-hires" -- -D warnings 2>&1 | grep "^error" | head -20
```

Expected: no output.

### Step 20: rustfmt

```bash
cargo fmt --all -- --check 2>&1 | head -10
```

If output: run `cargo fmt --all` and commit:
```bash
cargo fmt --all
git add -u
git commit -m "style: cargo fmt after terrain-hires bugfixes"
```

If no output: no commit needed.

### Step 21: Push

```bash
git push origin main
```

---

## Known Architectural Issues (out of scope — future tasks)

The following issues were found in the code review but require deeper architectural changes. They are documented here for future planning sessions:

| ID | Issue | Scope |
|----|-------|-------|
| H1 | Physics floor threshold (127) doesn't match visual mesh threshold (64/94/101) — player floats/sinks | Requires plumbing the visual threshold into the physics system |
| H2 | Smooth physics runs client-only → server desync / rubber-banding | Requires either server-side setting propagation or making smooth floor a client-only visual offset |
| H3 | Water/lava not rendered on smooth-terrain chunks (`fluid_mesh` always empty) | Requires running the fluid mesher in the Transvoxel code path |
| M7 | `phys_smooth.rs` is dead code (497 lines, duplicated LUT) | Safe to delete; requires removing the `pub mod` declaration in `lib.rs` |
| M9 | Per-tick heap allocation in smooth physics hot path (`DensityField` clone per entity per tick) | Requires scratch buffer / arena allocation |
| M10 | Smooth color atlas silently truncates at 32×32 — colors corrupt on wide meshes | Requires sizing atlas to actual range |
| M11 | Smooth-terrain chunks have stub lighting (`|_| 1.0`) | Requires integrating light grid into the smooth mesher |
