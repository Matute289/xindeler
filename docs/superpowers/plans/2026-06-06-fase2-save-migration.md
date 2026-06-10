# Fase 2 Save Migration — terrain-hires Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate block-coordinate saves (`userdata/`) between standard (0.3m/block) and hires (0.15m/block) when loading with `terrain-hires` feature enabled or disabled, so characters always spawn at the correct world position.

**Architecture:** A `hires: bool` field is added to the `CharacterPosition` JSON struct. On load, the delta between `saved_in_hires` and `running_hires` determines a `scale_factor` (2.0 up, 0.5 down, 1.0 no-op). On write, the current build's `cfg!(feature = "terrain-hires")` value is stamped. No SQL migration required — it's handled entirely in the JSON deserialization layer using the serde `default` attribute for backwards compatibility.

**Tech Stack:** Rust nightly, `serde` / `serde_json`, `common::consts::HIRES_SCALE`, `server/src/persistence/`.

---

## File Map

| File | Change |
|------|--------|
| `server/src/persistence/json_models.rs` | Add `#[serde(default)] pub hires: bool` to `CharacterPosition` |
| `server/src/persistence/character/conversions.rs` | Scale coords on load in `convert_waypoint_from_database_json`; stamp `hires` on save in `convert_waypoint_to_database_json` |
| `server/src/persistence/json_models.rs` | Add `#[cfg(test)]` unit tests for round-trip and cross-scale deserialization |

---

## Task 1: Add `hires` version field to `CharacterPosition`

**Files:**
- Modify: `server/src/persistence/json_models.rs:65-69`

- [ ] **Step 1: Read current struct**

```bash
sed -n '65,70p' server/src/persistence/json_models.rs
```

Expected:
```rust
#[derive(Serialize, Deserialize)]
pub struct CharacterPosition {
    pub waypoint: Option<Vec3<f32>>,
    pub map_marker: Option<Vec2<i32>>,
}
```

- [ ] **Step 2: Add `hires` field**

In `server/src/persistence/json_models.rs`, change the struct to:

```rust
#[derive(Serialize, Deserialize)]
pub struct CharacterPosition {
    pub waypoint: Option<Vec3<f32>>,
    pub map_marker: Option<Vec2<i32>>,
    #[serde(default)]
    pub hires: bool,
}
```

`#[serde(default)]` makes `hires` deserialize as `false` when the field is absent — this is the backwards-compatible default for all existing saves.

- [ ] **Step 3: Compile check**

```bash
source "$HOME/.cargo/env" && cargo build -p veloren-server 2>&1 | tail -5
```

Expected: `Finished dev profile`

- [ ] **Step 4: Commit**

```bash
git add server/src/persistence/json_models.rs
git commit -m "feat(terrain-hires): add hires version field to CharacterPosition JSON"
```

---

## Task 2: Scale coordinates on load

**Files:**
- Modify: `server/src/persistence/character/conversions.rs:1,284-300`

- [ ] **Step 5: Read current imports and load function**

```bash
head -32 server/src/persistence/character/conversions.rs
sed -n '284,302p' server/src/persistence/character/conversions.rs
```

- [ ] **Step 6: Add `HIRES_SCALE` to imports**

In `server/src/persistence/character/conversions.rs`, find the `use common::{...}` block at the top and add `consts::HIRES_SCALE` to it.

The existing import block looks like:
```rust
use common::{
    character::CharacterId,
    comp::{
        ActiveAbilities, Body as CompBody, Content, Hardcore, Inventory, MapMarker, Stats,
        Waypoint, body,
        ...
    },
    resources::Time,
};
```

Change to:
```rust
use common::{
    character::CharacterId,
    comp::{
        ActiveAbilities, Body as CompBody, Content, Hardcore, Inventory, MapMarker, Stats,
        Waypoint, body,
        inventory::{
            item::{Item as VelorenItem, MaterialStatManifest, tool::AbilityMap},
            loadout::{Loadout, LoadoutError},
            loadout_builder::LoadoutBuilder,
            recipe_book::RecipeBook,
            slot::InvSlotId,
        },
        item,
        skillset::{self, SkillGroupKind, SkillSet, skills::Skill},
    },
    consts::HIRES_SCALE,
    resources::Time,
};
```

(Only the `consts::HIRES_SCALE,` line is new — add it before `resources::Time`.)

- [ ] **Step 7: Replace the load function body**

Find the current `convert_waypoint_from_database_json` function (lines ~284-300):

```rust
pub fn convert_waypoint_from_database_json(
    position: &str,
) -> Result<(Option<Waypoint>, Option<MapMarker>), PersistenceError> {
    let character_position =
        serde_json::de::from_str::<CharacterPosition>(position).map_err(|err| {
            PersistenceError::ConversionError(format!(
                "Error de-serializing waypoint: {} err: {}",
                position, err
            ))
        })?;
    Ok((
        character_position
            .waypoint
            .map(|pos| Waypoint::new(pos, Time(0.0))),
        character_position.map_marker.map(MapMarker),
    ))
}
```

Replace with:

```rust
pub fn convert_waypoint_from_database_json(
    position: &str,
) -> Result<(Option<Waypoint>, Option<MapMarker>), PersistenceError> {
    let character_position =
        serde_json::de::from_str::<CharacterPosition>(position).map_err(|err| {
            PersistenceError::ConversionError(format!(
                "Error de-serializing waypoint: {} err: {}",
                position, err
            ))
        })?;

    let saved_in_hires = character_position.hires;
    let running_hires = cfg!(feature = "terrain-hires");
    let scale_factor: f32 = match (saved_in_hires, running_hires) {
        (false, true) => HIRES_SCALE,       // old standard save, now hires → scale up
        (true, false) => 1.0 / HIRES_SCALE, // old hires save, now standard → scale down
        _ => 1.0,                            // same scale, no change
    };

    Ok((
        character_position.waypoint.map(|pos| {
            Waypoint::new(pos * scale_factor, Time(0.0))
        }),
        character_position.map_marker.map(|pos| {
            MapMarker(pos.map(|v| (v as f32 * scale_factor) as i32))
        }),
    ))
}
```

- [ ] **Step 8: Compile check both modes**

```bash
source "$HOME/.cargo/env"
cargo build -p veloren-server 2>&1 | tail -3
cargo build -p veloren-server --features veloren-server/terrain-hires 2>&1 | tail -3
```

Both: `Finished dev profile`

---

## Task 3: Stamp `hires` flag on save

**Files:**
- Modify: `server/src/persistence/character/conversions.rs:263-282`

- [ ] **Step 9: Read current save function**

```bash
sed -n '263,283p' server/src/persistence/character/conversions.rs
```

Expected:
```rust
pub fn convert_waypoint_to_database_json(
    waypoint: Option<Waypoint>,
    map_marker: Option<MapMarker>,
) -> Option<String> {
    if waypoint.is_some() || map_marker.is_some() {
        let charpos = CharacterPosition {
            waypoint: waypoint.map(|w| w.get_pos()),
            map_marker: map_marker.map(|m| m.0),
        };
        Some(
            serde_json::to_string(&charpos)
                .map_err(|err| {
                    PersistenceError::ConversionError(format!("Error encoding waypoint: {:?}", err))
                })
                .ok()?,
        )
    } else {
        None
    }
}
```

- [ ] **Step 10: Add `hires` field to the save**

Replace the function body with:

```rust
pub fn convert_waypoint_to_database_json(
    waypoint: Option<Waypoint>,
    map_marker: Option<MapMarker>,
) -> Option<String> {
    if waypoint.is_some() || map_marker.is_some() {
        let charpos = CharacterPosition {
            waypoint: waypoint.map(|w| w.get_pos()),
            map_marker: map_marker.map(|m| m.0),
            hires: cfg!(feature = "terrain-hires"),
        };
        Some(
            serde_json::to_string(&charpos)
                .map_err(|err| {
                    PersistenceError::ConversionError(format!("Error encoding waypoint: {:?}", err))
                })
                .ok()?,
        )
    } else {
        None
    }
}
```

- [ ] **Step 11: Compile check both modes**

```bash
source "$HOME/.cargo/env"
cargo build -p veloren-server 2>&1 | tail -3
cargo build -p veloren-server --features veloren-server/terrain-hires 2>&1 | tail -3
```

Both: `Finished dev profile`

- [ ] **Step 12: Commit**

```bash
git add server/src/persistence/character/conversions.rs
git commit -m "feat(terrain-hires): scale waypoint/map_marker coords on load/save by HIRES_SCALE"
```

---

## Task 4: Unit tests

**Files:**
- Modify: `server/src/persistence/json_models.rs` (add tests module)

- [ ] **Step 13: Add tests at the bottom of json_models.rs**

Add this block before the final `}` of the file (after the existing `#[cfg(test)] pub mod tests` block):

```rust
#[cfg(test)]
mod waypoint_migration_tests {
    use super::CharacterPosition;
    use vek::{Vec2, Vec3};

    fn old_save_json() -> &'static str {
        r#"{"waypoint":[100.0,200.0,50.0],"map_marker":[100,200]}"#
    }

    fn hires_save_json() -> &'static str {
        r#"{"waypoint":[200.0,400.0,100.0],"map_marker":[200,400],"hires":true}"#
    }

    #[test]
    fn old_save_defaults_hires_to_false() {
        let pos: CharacterPosition =
            serde_json::de::from_str(old_save_json()).expect("must parse");
        assert!(!pos.hires, "old saves without hires field must default to false");
    }

    #[test]
    fn hires_save_parses_hires_true() {
        let pos: CharacterPosition =
            serde_json::de::from_str(hires_save_json()).expect("must parse");
        assert!(pos.hires);
    }

    // Tests for actual scale conversion live in conversions.rs where HIRES_SCALE is accessible.
}
```

- [ ] **Step 14: Add conversion tests in conversions.rs**

At the bottom of `server/src/persistence/character/conversions.rs`, add:

```rust
#[cfg(test)]
mod waypoint_scale_tests {
    use super::convert_waypoint_from_database_json;

    // Non-hires build: old save (no hires field) → no scale change
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

    // Non-hires build: hires save → divide by 2
    #[cfg(not(feature = "terrain-hires"))]
    #[test]
    fn hires_save_scaled_down_in_standard_build() {
        let json = r#"{"waypoint":[200.0,400.0,100.0],"map_marker":[200,400],"hires":true}"#;
        let (waypoint, map_marker) =
            convert_waypoint_from_database_json(json).expect("must parse");
        let pos = waypoint.unwrap().get_pos();
        assert!((pos.x - 100.0).abs() < 0.01, "expected 100.0, got {}", pos.x);
        assert!((pos.y - 200.0).abs() < 0.01);
        assert!((pos.z - 50.0).abs() < 0.01);
        let mm = map_marker.unwrap();
        assert_eq!(mm.0.x, 100);
        assert_eq!(mm.0.y, 200);
    }

    // Hires build: old save (no hires field) → multiply by 2
    #[cfg(feature = "terrain-hires")]
    #[test]
    fn old_save_scaled_up_in_hires_build() {
        let json = r#"{"waypoint":[100.0,200.0,50.0],"map_marker":[100,200]}"#;
        let (waypoint, map_marker) =
            convert_waypoint_from_database_json(json).expect("must parse");
        let pos = waypoint.unwrap().get_pos();
        assert!((pos.x - 200.0).abs() < 0.01, "expected 200.0, got {}", pos.x);
        assert!((pos.y - 400.0).abs() < 0.01);
        assert!((pos.z - 100.0).abs() < 0.01);
        let mm = map_marker.unwrap();
        assert_eq!(mm.0.x, 200);
        assert_eq!(mm.0.y, 400);
    }

    // Hires build: hires save → no change
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

- [ ] **Step 15: Run tests (standard build)**

```bash
source "$HOME/.cargo/env"
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server -- waypoint 2>&1 | tail -15
```

Expected:
```
test waypoint_migration_tests::old_save_defaults_hires_to_false ... ok
test waypoint_migration_tests::hires_save_parses_hires_true ... ok
test waypoint_scale_tests::old_save_no_change_in_standard_build ... ok
test waypoint_scale_tests::hires_save_scaled_down_in_standard_build ... ok
test result: ok. 4 passed
```

- [ ] **Step 16: Run tests (hires build)**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server --features veloren-server/terrain-hires -- waypoint 2>&1 | tail -15
```

Expected:
```
test waypoint_migration_tests::old_save_defaults_hires_to_false ... ok
test waypoint_migration_tests::hires_save_parses_hires_true ... ok
test waypoint_scale_tests::old_save_scaled_up_in_hires_build ... ok
test waypoint_scale_tests::hires_save_no_change_in_hires_build ... ok
test result: ok. 4 passed
```

- [ ] **Step 17: Commit**

```bash
git add server/src/persistence/json_models.rs server/src/persistence/character/conversions.rs
git commit -m "test(terrain-hires): add waypoint scale migration unit tests"
```

---

## Task 5: Manual integration test

- [ ] **Step 18: Verify old save loads at correct position with terrain-hires**

Start a singleplayer session **without** terrain-hires, walk to a recognizable landmark (e.g. a town), and note the coordinates (`F3` or chat `/pos`). Close the game.

```bash
# Standard build — note your position, then quit
source "$HOME/.cargo/env"
cargo run --bin veloren-voxygen
```

- [ ] **Step 19: Re-launch with terrain-hires, verify correct spawn**

```bash
cargo run --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose"
```

Verify the character spawns near the **same landmark** (may be off by <1 chunk due to float precision, but should not be ×2 displaced or at sea level). The waypoint should be saved with `hires: true` after this session.

- [ ] **Step 20: Re-launch without terrain-hires, verify correct spawn**

```bash
cargo run --bin veloren-voxygen
```

Verify the character spawns at approximately the same real-world location (the `hires: true` save is divided by 2 on load, placing them back near the original position).

---

## Task 6: Clippy + format

- [ ] **Step 21: Run clippy**

```bash
source "$HOME/.cargo/env"
cargo clippy -p veloren-server --locked -- -D warnings 2>&1 | grep "^error" | head -20
cargo clippy -p veloren-server --locked --features veloren-server/terrain-hires -- -D warnings 2>&1 | grep "^error" | head -20
```

Expected: no output (no errors).

- [ ] **Step 22: Run rustfmt**

```bash
cargo fmt -p veloren-server
git diff --stat
```

If there are format changes, stage and amend:

```bash
git add -p
git commit --amend --no-edit
```

---

## Summary

After this plan:

- Old saves (no `hires` field) load correctly in both standard and terrain-hires builds.
- New saves written with terrain-hires are compatible with downgrading back to standard.
- The migration is transparent: no user action required, no SQL changes, fully backwards-compatible via serde `default`.
- Fase 2 is complete — save migration was the last pending item.

Next: begin **Fase 3 — Normal Maps + Micro-detalle** (see spec `docs/superpowers/specs/2026-06-04-terrain-resolution-design.md`, Fase 3 section).
