# World Difficulty Zones Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every world region gets a deterministic difficulty rating (1–10) computed at worldgen; every combat entity spawns with a `Level` that scales its HP/damage and skillset rank; XP scales with the level differential (gray-mob cutoff, anti-farm dampening); nameplates and the world map make zones visible.

**Architecture:** A `difficulty: u8` on `SimChunk` is computed after civ placement (distance-to-town + biome + altitude, 5×5 box-blur smoothed) — never persisted, rebuilt deterministically from the seed. A new synced `comp::Level(u16)` carries the spawn level; stat scaling is applied **once** at NPC construction (`SpawnEntityData::from_entity_info`): HP via a one-shot `Health::with_max_multiplier`, damage via a new `Stats::level_damage_multiplier` field explicitly *preserved* across the tick-reset `reset_temp_modifiers()`. Level flows `SimChunk.difficulty → EntityInfo.level → NpcData/NpcBuilder → comp::Level`. XP differential hooks the existing kill-award site in `entity_manipulation.rs`. Players keep their derived `SkillSet::character_level()`; NPCs get the component.

**Tech Stack:** Rust nightly (2024 edition), specs ECS, conrod HUD. Design spec: `docs/superpowers/specs/2026-06-10-world-difficulty-zones-design.md`.

**Depends on:**
- `2026-06-10-character-levels.md` (M1) — **already merged**: `SkillSet::character_level()`, `Outcome::CharacterLevelUp`, `MAX_CHARACTER_LEVEL = 60`, `LEVEL_XP_BASE = 250` (`common/src/comp/skillset/mod.rs`); nameplate `Name [N]` via `overhead::Info.level`.
- `2026-06-11-classes-races.md` — required **only for Task 9** (Profession→ClassKind), which starts with a gate step that greps for `ClassKind` and stops if absent. All other tasks are independent.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/world-difficulty-zones` off `development` before Task 1.
- Invoke the `veloren-worldgen` skill (Tasks 3, 6, 7), `veloren-progression` (Tasks 1, 4, 5, 10), and `superpowers:test-driven-development` before writing code.
- Line numbers were verified at commit `53c4466145`. If a hunk doesn't match, re-locate by the quoted code, not the number.

---

### Task 1: Entity level math and the `Level` component

**Files:**
- Create: `common/src/comp/level.rs`
- Modify: `common/src/comp/mod.rs` (`pub mod level;` between `mod last;` line 24 and `mod location;` line 25; re-export in the `pub use self::{...}` block after `last::Last,`)
- Modify: `common/state/src/state.rs:246` (register after `ecs.register::<comp::SkillSet>();`)

- [ ] **Step 1: Write the failing tests**

Create `common/src/comp/level.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_covers_one_to_cap_without_gaps() {
        assert_eq!(level_band(1), 1..=3);
        assert_eq!(level_band(10), 28..=30);
        for d in 1..10u8 {
            assert_eq!(*level_band(d + 1).start(), level_band(d).end() + 1);
        }
        // Out-of-range difficulties clamp instead of panicking
        assert_eq!(level_band(0), 1..=3);
        assert_eq!(level_band(11), 28..=30);
    }

    #[test]
    fn rank_and_role_levels_match_spec_table() {
        // rank = ceil(level / 6), clamped to the 5 preset ranks
        for (level, rank) in [(0, 1), (1, 1), (6, 1), (7, 2), (15, 3), (21, 4), (27, 5), (30, 5)] {
            assert_eq!(skillset_rank(level), rank, "level {level}");
        }
        assert_eq!(elite_level(1), 5); // band top 3 + 2
        assert_eq!(boss_level(5), 19); // band top 15 + 4
        assert_eq!(boss_level(10), 30); // clamped to cap
    }

    #[test]
    fn stat_multipliers_match_spec_endpoints() {
        assert!((hp_mult(1) - 1.0).abs() < 1e-6);
        assert!((hp_mult(30) - 4.48).abs() < 1e-6);
        assert!((dmg_mult(1) - 1.0).abs() < 1e-6);
        assert!((dmg_mult(30) - 2.74).abs() < 1e-6);
    }

    #[test]
    fn xp_mult_gray_cutoff_and_caps() {
        assert_eq!(xp_mult(-10), 0.0); // gray mob
        assert_eq!(xp_mult(-20), 0.0);
        assert!((xp_mult(-9) - 0.25).abs() < 1e-6); // floor
        assert!((xp_mult(0) - 1.0).abs() < 1e-6);
        assert!((xp_mult(10) - 2.0).abs() < 1e-6); // cap
        assert!((xp_mult(20) - 2.0).abs() < 1e-6);
    }
}
```

Add `pub mod level;` to `common/src/comp/mod.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common comp::level -- --nocapture`
Expected: FAIL to compile with "cannot find function `level_band`".

- [ ] **Step 3: Implement**

Above the test module in `common/src/comp/level.rs`:

```rust
//! Entity spawn levels for world-difficulty zones.
//! See docs/superpowers/specs/2026-06-10-world-difficulty-zones-design.md §2.

use serde::{Deserialize, Serialize};
use specs::{Component, DerefFlaggedStorage};
use std::ops::RangeInclusive;

/// Entity level cap for the initial release (player cap is 60).
pub const ENTITY_LEVEL_CAP: u16 = 30;

/// Spawn level of an entity, assigned once at spawn from its region's
/// difficulty. Immutable afterwards — unlike `Stats`, whose modifiers reset
/// every tick. Synced to clients for nameplates.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level(pub u16);

impl Default for Level {
    fn default() -> Self { Self(1) }
}

impl Component for Level {
    type Storage = DerefFlaggedStorage<Self, specs::VecStorage<Self>>;
}

/// Inclusive spawn-level band for region difficulty `d` (1..=10): `3d-2..=3d`.
pub fn level_band(difficulty: u8) -> RangeInclusive<u16> {
    let d = u16::from(difficulty.clamp(1, 10));
    (3 * d - 2)..=(3 * d).min(ENTITY_LEVEL_CAP)
}

/// Skillset preset rank (1..=5) for an entity level: `ceil(level / 6)`.
pub fn skillset_rank(level: u16) -> u8 { (level.max(1).div_ceil(6)).min(5) as u8 }

/// Max-health multiplier: L1 ×1.0 → L30 ×4.48 (steeper than damage so
/// high-zone fights are longer, not just lethal).
pub fn hp_mult(level: u16) -> f32 { 1.0 + 0.12 * f32::from(level.max(1) - 1) }

/// Outgoing-damage multiplier: L1 ×1.0 → L30 ×2.74.
pub fn dmg_mult(level: u16) -> f32 { 1.0 + 0.06 * f32::from(level.max(1) - 1) }

/// XP multiplier for `delta = victim_level - attacker_level`. Mobs 10+ levels
/// below the attacker are gray (0 XP); bonus caps at +100%.
pub fn xp_mult(delta: i32) -> f32 {
    if delta <= -10 {
        0.0
    } else {
        (1.0 + 0.10 * delta as f32).clamp(0.25, 2.0)
    }
}

/// Dungeon elite level: band top + 2 (clamped to cap).
pub fn elite_level(difficulty: u8) -> u16 {
    (3 * u16::from(difficulty.clamp(1, 10)) + 2).min(ENTITY_LEVEL_CAP)
}

/// Dungeon boss level: band top + 4 (clamped to cap).
pub fn boss_level(difficulty: u8) -> u16 {
    (3 * u16::from(difficulty.clamp(1, 10)) + 4).min(ENTITY_LEVEL_CAP)
}
```

In `common/src/comp/mod.rs`, inside `pub use self::{...}` (after `last::Last,`): add `level::Level,`.
In `common/state/src/state.rs` after line 246: `ecs.register::<comp::Level>();`

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common comp::level -- --nocapture`
Expected: 4 tests PASS. Then `cargo check -p veloren-common-state` — clean.

- [ ] **Step 5: Commit**

```bash
git add common/src/comp/level.rs common/src/comp/mod.rs common/state/src/state.rs
git commit -m "feat: Level component and difficulty-band math for world zones"
```

---

### Task 2: Sync `Level` to clients

**Files:**
- Modify: `common/net/src/synced_components.rs:25` (x-macro entry after `stats: Stats,`) and `:135-137` (NetSync impl after `impl NetSync for Stats`)

The `synced_components!` x-macro drives everything: `server/src/sys/sentinel.rs:318` and `common/net/src/msg/ecs_packet.rs:106` expand it, so one entry registers tracking, packets, and client application. `Level` is re-exported via the macro's `pub use common::comp::*;` after Task 1.

- [ ] **Step 1: Add the entry and impl**

After `stats: Stats,` (line 25): add `level: Level,`. After the `impl NetSync for Stats` block (lines 135–137):

```rust
impl NetSync for Level {
    const SYNC_FROM: SyncFrom = SyncFrom::AnyEntity;
}
```

- [ ] **Step 2: Compiler-driven verification**

Run: `cargo check -p veloren-common-net -p veloren-server -p veloren-client 2>&1 | tail -20`
Expected: clean. Any "no impl"/non-exhaustive error points at another x-macro expansion site — fix it following the `Stats` pattern there. Do NOT add wildcard arms.

- [ ] **Step 3: Commit**

```bash
git add common/net/src/synced_components.rs
git commit -m "feat: sync Level component to clients"
```

---

### Task 3: `SimChunk.difficulty` computed at worldgen

**Files:**
- Create: `world/src/sim/difficulty.rs`
- Modify: `world/src/sim/mod.rs` — module decl; `SimChunk` (line 2503); both `SimChunk` literals (placeholder chunk ~line 729, `SimChunk::generate` return ~line 2776); new `WorldSim::compute_difficulty`
- Modify: `world/src/lib.rs` — call after `civ::Civs::generate` (lines 151–154); accessor next to `pub fn sim()` (line 165); spawn-score consumer (lines 279–284)

- [ ] **Step 1: Write the module with failing-then-passing tests**

Create `world/src/sim/difficulty.rs`:

```rust
//! Region difficulty (1..=10), computed once at worldgen after civ placement.
//! Deterministic for a fixed seed, never persisted (SimChunk is rebuilt at load).

/// Raw, pre-smoothing difficulty for one chunk.
/// * `d_town` — distance to the nearest town in chunks.
/// * `biome_difficulty` — `BiomeKind::difficulty()`, 1..=5.
/// * `alt` — chunk altitude in blocks.
pub fn raw_difficulty(d_town: f32, biome_difficulty: i32, alt: f32) -> u8 {
    let base = (d_town / 64.0).powf(0.8); // +1 tier per ~64 chunks, sublinear
    let biome = (biome_difficulty - 1) as f32 * 0.75; // 0.0..=3.0
    let alt = ((alt - 1200.0).max(0.0) / 800.0).min(2.0); // high mountains +0..2
    (1.0 + base + biome + alt).round().clamp(1.0, 10.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn town_chunks_are_safe() {
        assert_eq!(raw_difficulty(0.0, 1, 100.0), 1);
        assert_eq!(raw_difficulty(0.0, 2, 0.0), 2); // hostile biome still bumps it
    }

    #[test]
    fn monotonic_in_distance_and_reaches_max() {
        let mut last = 0;
        for d in 0..2048 {
            let v = raw_difficulty(d as f32, 1, 0.0);
            assert!(v >= last, "difficulty decreased at distance {d}");
            last = v;
        }
        assert_eq!(last, 10);
    }

    #[test]
    fn clamped_and_deterministic() {
        for d in [0.0, 64.0, 100_000.0] {
            for b in 1..=5 {
                for alt in [0.0, 1200.0, 4000.0] {
                    assert!((1..=10).contains(&raw_difficulty(d, b, alt)));
                }
            }
        }
        assert_eq!(raw_difficulty(200.0, 3, 1500.0), raw_difficulty(200.0, 3, 1500.0));
    }
}
```

Add `pub mod difficulty;` next to the existing module declarations at the top of `world/src/sim/mod.rs` (match the `mod map;` style).

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world difficulty -- --nocapture`
Expected: 3 tests PASS (the module is self-contained; if monotonicity fails, fix the curve, not the test).

- [ ] **Step 2: Add the field and the compute pass**

In `pub struct SimChunk` (line 2503), after `pub surface_veg: f32,`:

```rust
    /// Region difficulty (1..=10) for world-difficulty zones. Computed by
    /// [`WorldSim::compute_difficulty`] after civ placement; 1 until then.
    pub difficulty: u8,
```

Add `difficulty: 1,` to **both** `SimChunk` literals (placeholder chunk ~line 729 beginning `chunks: vec![SimChunk { chaos: 0.0,`, and the `Self { ... }` return of `SimChunk::generate` ~line 2776). `cargo check -p veloren-world` missing-field errors are the authoritative list.

Add to `impl WorldSim` (after `pub fn get`, line 2072):

```rust
    /// Computes region difficulty for every chunk. Must run after civ
    /// generation (towns project difficulty-1 safe discs). A 5×5 box blur
    /// turns zone borders into gradients instead of single-chunk cliffs.
    pub fn compute_difficulty(&mut self, town_centers: &[Vec2<i32>]) {
        let map_size_lg = self.map_size_lg();
        let sz = map_size_lg.chunks().map(|e| e as i32);
        let max_dist = 2.0 * sz.x.max(sz.y) as f32;
        let raw = (0..self.chunks.len())
            .map(|posi| {
                let pos = uniform_idx_as_vec2(map_size_lg, posi);
                let chunk = &self.chunks[posi];
                let d_town = town_centers
                    .iter()
                    .map(|town| (town - pos).map(|e| e as f32).magnitude())
                    .fold(max_dist, f32::min);
                difficulty::raw_difficulty(d_town, chunk.get_biome().difficulty(), chunk.alt)
                    as f32
            })
            .collect::<Vec<_>>();
        for posi in 0..self.chunks.len() {
            let pos = uniform_idx_as_vec2(map_size_lg, posi);
            let mut sum = 0.0;
            let mut n = 0.0;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let p = pos + Vec2::new(dx, dy);
                    if p.x >= 0 && p.y >= 0 && p.x < sz.x && p.y < sz.y {
                        sum += raw[vec2_as_uniform_idx(map_size_lg, p)];
                        n += 1.0;
                    }
                }
            }
            self.chunks[posi].difficulty = (sum / n).round().clamp(1.0, 10.0) as u8;
        }
    }
```

(`uniform_idx_as_vec2`/`vec2_as_uniform_idx` are imported at `world/src/sim/mod.rs:55`; `SimChunk::get_biome` exists — used at `world/src/lib.rs:280`.)

- [ ] **Step 3: Wire into `World::generate`, replace the old heuristic**

In `world/src/lib.rs`, directly after the `civ::Civs::generate` call (lines 151–154):

```rust
            // Region difficulty depends on town placement, so it runs right
            // after civ generation (and before anything reads it).
            let town_centers = civs
                .sites
                .values()
                .filter(|site| {
                    matches!(
                        site.kind,
                        SiteKind::Refactor
                            | SiteKind::CliffTown
                            | SiteKind::SavannahTown
                            | SiteKind::CoastalTown
                            | SiteKind::DesertCity
                    )
                })
                .map(|site| site.center)
                .collect::<Vec<_>>();
            sim.compute_difficulty(&town_centers);
```

(`SiteKind` is already imported in lib.rs; `civ::Site` has `kind: SiteKind` and `center: Vec2<i32>` — `world/src/civ/mod.rs:1810-1816`.)

Next to `pub fn sim(&self)` (line 165):

```rust
    /// Region difficulty (1..=10) for a chunk; 1 off-map. Project Oracle and
    /// the server read zone difficulty exclusively through this accessor.
    pub fn get_chunk_difficulty(&self, chunk_pos: Vec2<i32>) -> u8 {
        self.sim.get(chunk_pos).map_or(1, |c| c.difficulty)
    }
```

Replace the starting-site scoring heuristic at lines 279–284:

```rust
                                let chunk_difficulty = 20.0
                                    / (20.0 + chunk.get_biome().difficulty().pow(4) as f32 / 5.0);
```

becomes (one source of truth — the computed field):

```rust
                                let chunk_difficulty =
                                    20.0 / (20.0 + ((chunk.difficulty - 1) as f32).powi(3));
```

Delete the commented-out `// let chunk_difficulty = 1.0 / ...` line below it.

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p veloren-world && VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world difficulty`
Expected: clean check; 3 tests PASS.

```bash
git add world/src/sim/difficulty.rs world/src/sim/mod.rs world/src/lib.rs
git commit -m "feat: per-chunk region difficulty computed at worldgen"
```

---

### Task 4: Permanent stat-scaling primitives (`Stats`, `Health`, combat hook)

**Files:**
- Modify: `common/src/comp/stats.rs:86` (field after `attack_damage_modifier`), `:124` (init in `Stats::new`), `:147-154` (`reset_temp_modifiers`), tests at end of file
- Modify: `common/src/comp/health.rs` (method after `pub fn new`; tests at end of file)
- Modify: `common/src/combat.rs:396-398` (damage modifier read)

- [ ] **Step 1: Write the failing tests**

End of `common/src/comp/stats.rs`:

```rust
#[cfg(test)]
mod level_scaling_tests {
    use super::*;

    #[test]
    fn level_damage_multiplier_survives_temp_reset() {
        let body = Body::Humanoid(crate::comp::humanoid::Body::random());
        let mut stats = Stats::new(Content::dummy(), body);
        stats.attack_damage_modifier = 1.5; // a buff-style temp modifier
        stats.level_damage_multiplier = 2.0; // spawn-level scaling
        stats.reset_temp_modifiers();
        assert_eq!(stats.attack_damage_modifier, 1.0, "temp modifier must reset");
        assert_eq!(stats.level_damage_multiplier, 2.0, "level scaling must persist");
    }
}
```

End of `common/src/comp/health.rs`:

```rust
#[cfg(test)]
mod level_scaling_tests {
    use super::*;
    use crate::comp;

    #[test]
    fn with_max_multiplier_scales_and_clamps() {
        let body = comp::Body::Humanoid(comp::humanoid::Body::random());
        let base = Health::new(body);
        let scaled = Health::new(body).with_max_multiplier(4.48);
        assert!((scaled.maximum() / base.maximum() - 4.48).abs() < 0.01);
        assert!((scaled.base_max() / base.base_max() - 4.48).abs() < 0.01);
        assert_eq!(scaled.current(), scaled.maximum(), "spawns at full health");
        let huge = Health::new(body).with_max_multiplier(1.0e9);
        assert!(huge.maximum() <= f32::from(u16::MAX - 1));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common level_scaling -- --nocapture`
Expected: FAIL to compile — "no field `level_damage_multiplier`", "no method named `with_max_multiplier`".

- [ ] **Step 3: Implement**

`common/src/comp/stats.rs`, after `pub attack_damage_modifier: f32,` (line 86):

```rust
    /// Permanent outgoing-damage multiplier from the entity's spawn
    /// [`Level`](crate::comp::Level) (world-difficulty zones). NOT a temp
    /// modifier: it survives `reset_temp_modifiers`.
    pub level_damage_multiplier: f32,
```

In `Stats::new` (after line 124): `level_damage_multiplier: 1.0,`. Replace `reset_temp_modifiers` (lines 147–154):

```rust
    /// Resets temporary modifiers to default values
    pub fn reset_temp_modifiers(&mut self) {
        // "consume" name and body and re-create from scratch
        let name = std::mem::replace(&mut self.name, Content::dummy());
        let body = self.original_body;
        // Spawn-level scaling is permanent, not a buff — carry it across.
        let level_damage_multiplier = self.level_damage_multiplier;

        *self = Self::new(name, body);
        self.level_damage_multiplier = level_damage_multiplier;
    }
```

`common/src/comp/health.rs`, in `impl Health` directly after `pub fn new`:

```rust
    /// Scales current, base-max, and max health by `mult`, clamped to the
    /// engine maximum. Called exactly once at spawn for level scaling
    /// (world-difficulty zones); do NOT call it on a live entity.
    #[must_use]
    pub fn with_max_multiplier(mut self, mult: f32) -> Self {
        let scale = |v: u32| (((v as f32) * mult) as u32).min(Self::MAX_SCALED_HEALTH);
        self.current = scale(self.current);
        self.base_max = scale(self.base_max);
        self.maximum = scale(self.maximum);
        self
    }
```

`common/src/combat.rs`, lines 396–398, change:

```rust
        let damage_modifier = attacker
            .and_then(|a| a.stats)
            .map_or(1.0, |s| s.attack_damage_modifier);
```

to:

```rust
        let damage_modifier = attacker
            .and_then(|a| a.stats)
            .map_or(1.0, |s| s.attack_damage_modifier * s.level_damage_multiplier);
```

- [ ] **Step 4: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common level_scaling -- --nocapture`
Expected: 2 tests PASS. (`Stats` is only built via `Stats::new`/`Stats::empty`, so no literal elsewhere breaks — confirm with `cargo check --workspace`.)

```bash
git add common/src/comp/stats.rs common/src/comp/health.rs common/src/combat.rs
git commit -m "feat: non-reset level damage multiplier and one-shot HP scaling"
```

---

### Task 5: `EntityInfo.level` → spawn-time scaling → `Level` component

**Files:**
- Modify: `common/src/comp/level.rs` (`preset_rank_override` + test)
- Modify: `common/src/generation.rs:238` (`EntityInfo` field), `:276` (init in `at()`), builder next to `with_skillset_asset` (line 548)
- Modify: `server/src/sys/terrain.rs:415-431` (`NpcData`), `:444-516` (`from_entity_info`), `:564` (literal), `:617-657` (`to_npc_builder`); test at end of file
- Modify: `common/src/event.rs:55-99` (`NpcBuilder` field + builder)
- Modify: `server/src/events/entity_creation.rs:114-143` (`handle_create_npc`)
- Modify (compiler-driven): `server/src/events/entity_manipulation.rs:3592` (`NpcData` literal) and anything else `cargo check` reports

- [ ] **Step 1: Write the failing tests**

Inside `mod tests` in `common/src/comp/level.rs`:

```rust
    #[test]
    fn preset_rank_override_repoints_rank_paths() {
        let over = |a: &str, l| preset_rank_override(a.to_string(), l);
        assert_eq!(
            over("common.skillset.preset.rank3.fullskill", Some(25)),
            "common.skillset.preset.rank5.fullskill"
        );
        assert_eq!(
            over("common.skillset.preset.rank5.sword", Some(2)),
            "common.skillset.preset.rank1.sword"
        );
        // Bespoke (boss) skillsets pass through untouched
        assert_eq!(
            over("common.skillset.dungeon.cultist.warlord", Some(25)),
            "common.skillset.dungeon.cultist.warlord"
        );
        // No level, no change
        assert_eq!(
            over("common.skillset.preset.rank3.fullskill", None),
            "common.skillset.preset.rank3.fullskill"
        );
    }
```

End of `server/src/sys/terrain.rs`:

```rust
#[cfg(test)]
mod spawn_scaling_tests {
    use super::*;
    use common::{comp::level, generation::EntityInfo};

    #[test]
    fn entity_level_scales_npc_data() {
        let mut rng = rand::rng();
        let template = |level: Option<u16>| {
            let mut info = EntityInfo::at(Vec3::zero()).with_asset_expect(
                "common.entity.template",
                &mut rng,
                None,
            );
            if let Some(level) = level {
                info = info.with_level(level);
            }
            SpawnEntityData::from_entity_info(info)
                .into_npc_data_inner()
                .expect("template is not special")
        };
        let unscaled = template(None);
        let scaled = template(Some(30));
        assert_eq!(unscaled.level, None);
        assert_eq!(scaled.level, Some(30));
        assert!((unscaled.stats.level_damage_multiplier - 1.0).abs() < 1e-6);
        assert!((scaled.stats.level_damage_multiplier - level::dmg_mult(30)).abs() < 1e-6);
        let (u, s) = (unscaled.health.unwrap(), scaled.health.unwrap());
        assert!((s.maximum() / u.maximum() - level::hp_mult(30)).abs() < 0.05);
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common preset_rank_override`
Expected: FAIL to compile — "cannot find function `preset_rank_override`".

- [ ] **Step 2: Implement `preset_rank_override`**

In `common/src/comp/level.rs` after `boss_level`:

```rust
/// Re-points a `common.skillset.preset.rank{N}.*` asset at the rank for the
/// given level, overriding any rank baked into entity RON `meta`. Bespoke
/// (non-preset) skillsets — boss kits — pass through unchanged. All five rank
/// directories ship the same weapon files
/// (`assets/common/skillset/preset/rank{1..5}/`), so the result always loads.
pub fn preset_rank_override(asset: String, level: Option<u16>) -> String {
    const PREFIX: &str = "common.skillset.preset.rank";
    match (level, asset.strip_prefix(PREFIX)) {
        (Some(level), Some(rest)) => match rest.split_once('.') {
            Some((_, weapon)) => format!("{PREFIX}{}.{}", skillset_rank(level), weapon),
            None => asset,
        },
        _ => asset,
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common preset_rank_override` — PASS.

- [ ] **Step 3: `EntityInfo.level` + builder**

In `common/src/generation.rs`, `pub struct EntityInfo` after `pub skillset_asset: Option<String>,` (line 238):

```rust
    /// Spawn level from the region difficulty band (world-difficulty zones).
    /// `None` = unleveled, no scaling. Not inherited by `pets`/`rider`
    /// configs (they spawn unleveled; see spec Open Question 2).
    pub level: Option<u16>,
```

In `EntityInfo::at` (after `skillset_asset: None,`, line 276): `level: None,`. After `pub fn with_skillset_asset` (line 548):

```rust
    #[must_use]
    pub fn with_level(mut self, level: u16) -> Self {
        self.level = Some(level);
        self
    }
```

- [ ] **Step 4: Scale at NPC construction**

In `server/src/sys/terrain.rs`:

(a) `pub struct NpcData` (line 415): add `pub level: Option<u16>,` after `pub scale: comp::Scale,`.

(b) In `SpawnEntityData::from_entity_info`: add `level,` to the `EntityInfo` destructuring (after `skillset_asset,`, line 462). Replace the stats/skillset construction (lines 477–487):

```rust
        let name = name.unwrap_or_else(Content::dummy);
        let mut stats = comp::Stats::new(name, body);
        if let Some(level) = level {
            stats.level_damage_multiplier = comp::level::dmg_mult(level);
        }

        let skill_set = {
            let skillset_builder = SkillSetBuilder::default();
            if let Some(skillset_asset) = skillset_asset {
                let skillset_asset = comp::level::preset_rank_override(skillset_asset, level);
                skillset_builder.with_asset_expect(&skillset_asset).build()
            } else {
                skillset_builder.build()
            }
        };
```

and the health line (515):

```rust
        let health = Some(
            comp::Health::new(body)
                .with_max_multiplier(level.map_or(1.0, comp::level::hp_mult)),
        );
```

(c) In the `SpawnEntityData::Npc(NpcData { ... })` literal (line 564): add `level,` after `scale: comp::Scale(scale),`.

(d) In `NpcData::to_npc_builder` (line 617): add `level,` to the destructuring and `.with_level(level)` to the `NpcBuilder::new(...)` chain (after `.with_loot(loot)`).

- [ ] **Step 5: `NpcBuilder` + component attachment**

In `common/src/event.rs`, `pub struct NpcBuilder` (line 55): add `pub level: Option<u16>,` after `pub scale: comp::Scale,`; add `level: None,` to the `NpcBuilder::new` literal; add next to `with_health`:

```rust
    pub fn with_level(mut self, level: impl Into<Option<u16>>) -> Self {
        self.level = level.into();
        self
    }
```

In `server/src/events/entity_creation.rs`, `handle_create_npc` (line 114): add `level,` to the `NpcBuilder` destructuring (after `scale,`), and append to the builder chain ending `.maybe_with(rider_effects)` (line 143):

```rust
        .maybe_with(level.map(comp::Level))
```

- [ ] **Step 6: Compiler-driven literal sweep**

Run: `cargo check -p veloren-server --all-targets 2>&1 | grep -B2 "missing field"`
Known site: `server/src/events/entity_manipulation.rs:3592` (`SpawnEntityData::Npc(NpcData { ... })`) — add `level: None,` with comment `// transformation keeps pre-existing scaling baked into components`. Repeat until clean.

- [ ] **Step 7: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server spawn_scaling -- --nocapture`
Expected: 1 test PASS.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`
Expected: PASS (generation.rs asset tests still load every entity config).

- [ ] **Step 8: Commit**

```bash
git add common/src/comp/level.rs common/src/generation.rs common/src/event.rs server/src
git commit -m "feat: spawn-time level scaling and rank override for NPCs"
```

---

### Task 6: Band levels for all worldgen spawns (wildlife + dungeon trash)

**Files:**
- Modify: `world/src/lib.rs` — import (`comp::Content,` → `comp::{self, Content},` in the `use common::{...}` block, lines 47–64) and a post-pass after the site-supplement loop (lines 564–567)

One central pass in `World::generate_chunk` levels everything flowing through `ChunkSupplement`: wildlife (`apply_wildlife_supplement`, lib.rs:553), dungeon `apply_supplement` spawns, and painter spawns from site rendering (collected into `canvas.entity_spawns`, lib.rs:516). Producers that want non-band levels (Task 7 bosses) call `.with_level(...)` and are skipped because the pass only fills `None`.

- [ ] **Step 1: Implement the post-pass**

Directly after the site-supplement loop (lines 564–567, ends `index.sites[*site].apply_supplement(&mut dynamic_rng, chunk_wpos2d, &mut supplement)`):

```rust
        // World-difficulty zones: every spawned entity that wasn't explicitly
        // leveled by its producer rolls this chunk's difficulty band.
        for entity_spawn in &mut supplement.entity_spawns {
            let mut assign = |entity: &mut EntityInfo| {
                if entity.level.is_none() && entity.special_entity.is_none() {
                    entity.level = Some(
                        dynamic_rng.random_range(comp::level::level_band(sim_chunk.difficulty)),
                    );
                }
            };
            match entity_spawn {
                EntitySpawn::Entity(entity) => assign(entity),
                EntitySpawn::Group(group) => group.iter_mut().for_each(assign),
            }
        }
```

(`EntityInfo`/`EntitySpawn` are imported at lib.rs:50; `supplement.entity_spawns` is `pub` — `common/src/generation.rs:601`. Group members roll independently — `Pack::generate` clones one `EntityInfo` per member, `world/src/layer/wildlife.rs:135`, so a wolf pack spans the band.)

- [ ] **Step 2: Verify**

Run: `cargo check -p veloren-world`
Expected: clean (if rustc complains about the closure borrowing `dynamic_rng` across the match, inline the closure body into both arms).
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world wildlife`
Expected: PASS — the existing manifest tests (`world/src/layer/wildlife.rs:690+`) re-instantiate every entity asset, catching `EntityInfo` breakage.

- [ ] **Step 3: Commit**

```bash
git add world/src/lib.rs
git commit -m "feat: band-level assignment for all worldgen entity spawns"
```

---

### Task 7: Dungeon plot role offsets (boss +4, elite +2)

**Files:**
- Modify: `world/src/lib.rs:566` (pass difficulty), `world/src/site/mod.rs:3269-3282` (`Site::apply_supplement`)
- Modify: `world/src/site/plot/gnarling.rs:324-330` (signature), `:369` (harvester boss), `:431/:488/:502` (wood golems), `:442-443` (chieftain)
- Modify: `world/src/site/plot/adlet.rs:408-414` (signature, body is empty), `:2094` (elder via `Land`)

- [ ] **Step 1: Thread difficulty through `apply_supplement`**

`world/src/lib.rs:566`:

```rust
            index.sites[*site].apply_supplement(
                &mut dynamic_rng,
                chunk_wpos2d,
                &mut supplement,
                sim_chunk.difficulty,
            )
```

`world/src/site/mod.rs:3269` — add the trailing parameter and forward it:

```rust
    pub fn apply_supplement(
        &self,
        dynamic_rng: &mut impl Rng,
        wpos2d: Vec2<i32>,
        supplement: &mut crate::ChunkSupplement,
        difficulty: u8,
    ) {
        for (_, plot) in self.plots.iter() {
            match &plot.kind {
                PlotKind::Gnarling(g) => {
                    g.apply_supplement(dynamic_rng, wpos2d, supplement, difficulty)
                },
                PlotKind::Adlet(a) => {
                    a.apply_supplement(dynamic_rng, wpos2d, supplement, difficulty)
                },
                _ => {},
            }
        }
    }
```

Both plot `apply_supplement` signatures gain `difficulty: u8` (gnarling line 324; adlet line 408, name it `_difficulty: u8` — its body is empty).

- [ ] **Step 2: Gnarling role offsets**

Add `use common::comp::level;` to gnarling.rs's `use common::{...}` block. Inside `apply_supplement`:

Line 369 (harvester boss):

```rust
            supplement.add_entity_spawn(EntitySpawn::Entity(Box::new(
                harvester_boss(
                    self.tunnels.end + boss_room_offset - 2 * Vec3::unit_z(),
                    dynamic_rng,
                )
                .with_level(level::boss_level(difficulty)),
            )));
```

Lines 442–443 (chieftain):

```rust
                        supplement.add_entity_spawn(EntitySpawn::Entity(Box::new(
                            gnarling_chieftain(pos, dynamic_rng)
                                .with_level(level::boss_level(difficulty)),
                        )));
```

Each `wood_golem(...)` spawn inside `apply_supplement` (lines 431, 488, 502) gets `.with_level(level::elite_level(difficulty))` appended to the call in the same style. Trash (`random_gnarling`, `mandragora`, `deadwood`, `gnarling_stalker`) stays untouched — Task 6's band pass levels it.

- [ ] **Step 3: Adlet elder (painter-spawned)**

The adlet boss spawns during rendering (`adlet.rs:2094`), and `render_inner` receives `land: &Land`; `Land::get_chunk_wpos` (`world/src/land.rs:67`) exposes the sim chunk. Replace line 2094:

```rust
                    let difficulty = land
                        .get_chunk_wpos(boss_spawn.xy())
                        .map_or(1, |c| c.difficulty);
                    painter.spawn(
                        adlet_elder(boss_spawn.as_(), &mut rng)
                            .with_level(common::comp::level::boss_level(difficulty)),
                    );
```

(adlet's `render_inner` binds `land: &Land` at line 428 — verified.)

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p veloren-world --all-targets` — fix any remaining `apply_supplement` arity errors it reports (the dummy test calls at gnarling.rs:2216 / adlet.rs:2420 only build `EntityInfo`s and are unaffected). Then `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world` — PASS.

```bash
git add world/src/lib.rs world/src/site/mod.rs world/src/site/plot/gnarling.rs world/src/site/plot/adlet.rs
git commit -m "feat: dungeon boss/elite level offsets from host-chunk difficulty"
```

---

### Task 8: rtsim NPC levels (persistence-safe) and Architect assignment

**Files:**
- Modify: `rtsim/Cargo.toml` (dev-dependency), `rtsim/src/data/npc.rs:281-430` (field, default, builder, test)
- Modify: `rtsim/src/rule/architect.rs` (`spawn_level` helper + five `Npc::new` sites: lines 313, 359, 444, 512, 573)
- Modify: `server/src/rtsim/tick.rs:361-443` (`get_npc_entity_info`)

- [ ] **Step 1: Write the failing save-compat test**

Add to `rtsim/Cargo.toml`:

```toml
[dev-dependencies]
serde_json = { workspace = true }
```

(If cargo errors "not found in workspace.dependencies", use `serde_json = "1.0"`.)

End of `rtsim/src/data/npc.rs`:

```rust
#[cfg(test)]
mod level_save_compat {
    use super::*;

    // rtsim persistence uses named-map encoding (rmp_serde::encode::write_named,
    // rtsim/src/data/mod.rs:112), so a missing `level` key is exactly what a
    // pre-difficulty-zones save looks like; serde_json has the same
    // named-field semantics.
    #[test]
    fn old_saves_default_to_level_one() {
        let npc = Npc::new(
            0,
            Vec3::zero(),
            comp::Body::Humanoid(comp::humanoid::Body::random()),
            Role::Wild,
        )
        .with_level(7);
        let mut value = serde_json::to_value(&npc).expect("npc serializes");
        let map = value.as_object_mut().expect("npc serializes as a named map");
        assert!(map.remove("level").is_some(), "level must be serialized");
        let restored: Npc = serde_json::from_value(value).expect("old save must load");
        assert_eq!(restored.level, 1);
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim level_save_compat -- --nocapture`
Expected: FAIL to compile — "no field `level`" / "no method named `with_level`".

- [ ] **Step 2: Implement `Npc.level`**

In `pub struct Npc` (line 281), after `pub faction: Option<FactionId>,`:

```rust
    /// Spawn level (world-difficulty zones). Defaults to 1 so rtsim saves
    /// from before this field load unchanged.
    #[serde(default = "default_npc_level")]
    pub level: u16,
```

Above the struct: `fn default_npc_level() -> u16 { 1 }`. In `Npc::new` (line 372) add `level: 1,` after `faction: None,`. Next to `with_home` (line 416):

```rust
    // TODO: have a dedicated `NpcBuilder` type for this.
    pub fn with_level(mut self, level: u16) -> Self {
        self.level = level;
        self
    }
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim level_save_compat` — PASS.

- [ ] **Step 3: Architect level assignment**

In `rtsim/src/rule/architect.rs`, add below `randomize_body` (~line 252; add `use vek::Vec2;` if missing — `CoordinateConversions` is already imported at line 5):

```rust
/// Spawn level for an rtsim NPC from its spawn chunk's band and its role
/// (spec §3): guards sit above the band midpoint, adventurer tiers map to
/// fixed levels, hostiles roll the band, civilians stay level 1.
fn spawn_level(world: &World, wpos: Vec2<i32>, role: &Role, rng: &mut impl RngExt) -> u16 {
    use common::comp::level::{ENTITY_LEVEL_CAP, level_band};
    let difficulty = world
        .sim()
        .get(wpos.wpos_to_cpos())
        .map_or(1, |c| c.difficulty);
    let band = level_band(difficulty);
    let band_mid = (band.start() + band.end()) / 2;
    let profession = match role {
        Role::Civilised(p) => p.as_ref(),
        _ => None,
    };
    match (role, profession) {
        (_, Some(Profession::Guard | Profession::Captain)) => {
            (band_mid + 2).min(ENTITY_LEVEL_CAP)
        },
        // tiers 0..=3 → levels 1/9/17/25
        (_, Some(Profession::Adventurer(tier))) => {
            (1 + (*tier).min(3) as u16 * 8).min(ENTITY_LEVEL_CAP)
        },
        (_, Some(Profession::Cultist)) => rng.random_range(band),
        (_, Some(Profession::Pirate(leader))) => {
            (rng.random_range(band) + if *leader { 3 } else { 0 }).min(ENTITY_LEVEL_CAP)
        },
        (Role::Civilised(_), _) => 1, // other professions are civilians
        (Role::Wild | Role::Monster, _) => rng.random_range(band),
        (Role::Vehicle, _) => 1,
    }
}
```

Append `.with_level(spawn_level(world, <wpos2d>, &death.role, rng))` to all five `Npc::new(...)` chains, using the 2D position already in scope:

| Site | `<wpos2d>` | Notes |
|---|---|---|
| `spawn_anywhere`, line 313 | `cpos.cpos_to_wpos_center()` | inside the `attempt` closure |
| `spawn_at_plot`, line 359 | `site.tile_center_wpos(plot.root_tile())` | bind as `let wpos2d = ...` *before* the existing `let wpos = wpos.as_()...` shadowing |
| faction respawn, line 444 | `site.wpos` | bind before the `.as_()` shadowing |
| wild-at-site, line 512 | `site.wpos` | same pattern |
| monster respawn, line 573 | `cpos.cpos_to_wpos_center()` | chunk already in scope |

Verify the sweep: `grep -n "Npc::new" rtsim/src/rule/architect.rs` — every hit's chain must carry `.with_level`.

- [ ] **Step 4: Propagate into the ECS spawn path**

In `server/src/rtsim/tick.rs`, `get_npc_entity_info` (line 361): append `.with_level(npc.level)` to **both** return expressions — the profession branch (chain ending `.with_agent_mark(...)`, line 392) and the wild/monster branch (`EntityInfo::at(pos.0).with_entity_config(...)`, line 441). The level then flows through Task 5's scaling; guards' `rank3.fullskill` (from `village/guard.ron`) is rank-overridden by their level automatically.

- [ ] **Step 5: Verify and commit**

Run: `cargo check -p veloren-rtsim -p veloren-server --all-targets && VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim`
Expected: clean; tests PASS.

```bash
git add rtsim/Cargo.toml rtsim/src/data/npc.rs rtsim/src/rule/architect.rs server/src/rtsim/tick.rs
git commit -m "feat: rtsim NPC levels with serde-default save compatibility"
```

---

### Task 9: Profession→ClassKind mapping [GATED on classes-races plan]

**Files:**
- Modify: `common/src/rtsim.rs:485-511` (impl after the `Profession` enum)

- [ ] **Step 0: GATE — verify ClassKind exists**

Run: `grep -rn "enum ClassKind" common/src/ || echo "CLASSKIND ABSENT"`
If `CLASSKIND ABSENT`: **STOP this task.** Leave it unchecked, continue with Task 10, and report that Task 9 awaits `2026-06-11-classes-races.md`. Otherwise note the module path and use it below.

- [ ] **Step 1: Write the failing test**

End of `common/src/rtsim.rs`:

```rust
#[cfg(test)]
mod class_mapping_tests {
    use super::*;
    use crate::comp::ClassKind; // adjust to the path found in Step 0

    #[test]
    fn combat_professions_have_classes_civilians_do_not() {
        use Profession::*;
        for (profession, class) in [
            (Guard, Some(ClassKind::Warrior)),
            (Captain, Some(ClassKind::Warrior)),
            (Hunter, Some(ClassKind::Ranger)),
            (Adventurer(0), Some(ClassKind::Rogue)),
            (Adventurer(1), Some(ClassKind::Rogue)),
            (Adventurer(2), Some(ClassKind::Warrior)),
            (Adventurer(3), Some(ClassKind::Warrior)),
            (Cultist, Some(ClassKind::Warlock)),
            (Pirate(false), Some(ClassKind::Rogue)),
            (Pirate(true), Some(ClassKind::Rogue)),
            (Herbalist, Some(ClassKind::Druid)),
            (Alchemist, Some(ClassKind::Druid)),
            (Farmer, None),
            (Merchant, None),
            (Blacksmith, None),
            (Chef, None),
        ] {
            assert_eq!(profession.class_kind(), class, "{profession:?}");
        }
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class_mapping -- --nocapture`
Expected: FAIL to compile — "no method named `class_kind`".

- [ ] **Step 2: Implement**

After the `Profession` enum (ends line ~511):

```rust
impl Profession {
    /// Class for combat-capable rtsim NPCs (world-difficulty spec §4).
    /// `None` = classless civilian. Kit/loadout consumption of this mapping
    /// is owned by the classes-races plan; this is the single source of truth.
    pub fn class_kind(&self) -> Option<crate::comp::ClassKind> {
        use crate::comp::ClassKind;
        match self {
            Profession::Guard | Profession::Captain => Some(ClassKind::Warrior),
            Profession::Hunter => Some(ClassKind::Ranger),
            // Mirrors gear progression: low tiers skirmish, high tiers front-line
            Profession::Adventurer(tier) if *tier <= 1 => Some(ClassKind::Rogue),
            Profession::Adventurer(_) => Some(ClassKind::Warrior),
            Profession::Cultist => Some(ClassKind::Warlock),
            Profession::Pirate(_) => Some(ClassKind::Rogue),
            // Flavor only in this phase — no combat kit change
            Profession::Herbalist | Profession::Alchemist => Some(ClassKind::Druid),
            Profession::Farmer
            | Profession::Merchant
            | Profession::Blacksmith
            | Profession::Chef => None,
        }
    }
}
```

(If `ClassKind` lacks `Warlock`/`Druid`/`Ranger` variants, STOP and reconcile with the classes-races plan — do not invent variants.)

- [ ] **Step 3: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class_mapping` — PASS.

```bash
git add common/src/rtsim.rs
git commit -m "feat: Profession to ClassKind mapping for combat NPCs"
```

---

### Task 10: XP level differential with gray-mob cutoff

**Files:**
- Modify: `server/src/events/entity_manipulation.rs` — `DestroyEventData` (line 567), victim level by the `exp_reward` computation (~line 1131), the award `for_each` (lines 1253–1268)

`xp_mult` was implemented and tested in Task 1; this is wiring.

- [ ] **Step 1: Storage + victim level**

In `pub struct DestroyEventData<'a>` (line 567), after `stats: ReadStorage<'a, Stats>,`: add `levels: ReadStorage<'a, comp::Level>,`.

Directly after the `exp_reward` computation (`combat::combat_rating(...) * 20.0`, ends line ~1138):

```rust
                // World-difficulty zones: levels for the differential. NPCs
                // carry comp::Level; players derive theirs from lifetime XP.
                let victim_level = data
                    .levels
                    .get(ev.entity)
                    .map(|l| l.0)
                    .unwrap_or_else(|| entity_skill_set.character_level());
```

- [ ] **Step 2: Differential per award (group uses its highest member)**

Directly **before** `exp_awards.iter().for_each(...)` (line 1253):

```rust
                // The differential uses the highest-level group member so
                // low-level mules can't inflate group rewards.
                let mut group_max_level = HashMap::<Group, u16>::new();
                let attacker_levels = exp_awards
                    .iter()
                    .map(|(attacker, _, group)| {
                        let level = data.levels.get(*attacker).map(|l| l.0).unwrap_or_else(|| {
                            data.skill_sets
                                .get(*attacker)
                                .map_or(1, |s| s.character_level())
                        });
                        if let Some(group) = group {
                            let entry = group_max_level.entry(*group).or_insert(level);
                            *entry = (*entry).max(level);
                        }
                        (*attacker, level)
                    })
                    .collect::<HashMap<_, _>>();
```

Change the `for_each` head from `|(attacker, exp_reward, _)|` to `|(attacker, exp_reward, group)|` and insert at the top of its body (before the existing `if let Some((mut attacker_skill_set, ...))`):

```rust
                    let attacker_level = group
                        .as_ref()
                        .and_then(|g| group_max_level.get(g))
                        .or_else(|| attacker_levels.get(attacker))
                        .copied()
                        .unwrap_or(1);
                    let delta = i32::from(victim_level) - i32::from(attacker_level);
                    let exp_reward = exp_reward * comp::level::xp_mult(delta);
                    if exp_reward <= f32::EPSILON {
                        return; // gray mob — no XP, no ExpChange spam
                    }
```

and change `handle_exp_gain`'s first argument from `*exp_reward` to the shadowed `exp_reward`. (`HashMap` is already imported — used at line ~1140.)

- [ ] **Step 3: Verify and commit**

Run: `cargo check -p veloren-server --all-targets`
Expected: clean. If the pre-pass `data.skill_sets.get(...)` errors on the `WriteStorage`, write `(&data.skill_sets).get(*attacker)` — immutable reads of a `WriteStorage` are valid.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server` — PASS.

```bash
git add server/src/events/entity_manipulation.rs
git commit -m "feat: XP scales with level differential, gray mobs give nothing"
```

---

### Task 11: Anti-farm kill ring buffer

**Files:**
- Modify: `server/src/events/entity_manipulation.rs` (`RecentKills` + tests; `DestroyEventData` field; award-loop hook)
- Modify: `server/src/events/mod.rs` (re-export), `server/src/lib.rs:369` (insert next to `RecentClientIPs`)

- [ ] **Step 1: Write the failing tests**

End of `server/src/events/entity_manipulation.rs`:

```rust
#[cfg(test)]
mod recent_kills_tests {
    use super::*;

    #[test]
    fn repeat_kills_decay_recover_and_floor() {
        let mut kills = RecentKills::default();
        let uid = Uid(7);
        let victim = Body::Humanoid(comp::humanoid::Body::random());
        assert_eq!(kills.register_kill(uid, victim, 0.0), 1.0);
        assert!((kills.register_kill(uid, victim, 10.0) - 0.9).abs() < 1e-6);
        assert!((kills.register_kill(uid, victim, 20.0) - 0.81).abs() < 1e-6);
        // A different species is undampened
        let other = Body::QuadrupedMedium(comp::quadruped_medium::Body::random());
        assert_eq!(kills.register_kill(uid, other, 21.0), 1.0);
        // Past the 10-minute window the counter has decayed away
        assert_eq!(kills.register_kill(uid, victim, 20.0 + 601.0), 1.0);
        // Hammering the same config floors at ×0.2
        let mut mult = 1.0;
        for i in 0..40 {
            mult = kills.register_kill(uid, victim, 1000.0 + i as f64);
        }
        assert!((mult - 0.2).abs() < 1e-6);
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server recent_kills -- --nocapture`
Expected: FAIL to compile — "cannot find type `RecentKills`".

- [ ] **Step 2: Implement the resource**

Above `DestroyEventData` (line 567):

```rust
/// Per-player ring buffer of recent kills for XP anti-farm dampening
/// (world-difficulty spec §6). In-memory only: a relog resets it, which is
/// acceptable friction. Keyed by victim `Body` since entities don't retain
/// their config-asset id after spawn.
#[derive(Default)]
pub struct RecentKills {
    per_player: HashMap<Uid, std::collections::VecDeque<(Body, f64)>>,
}

impl RecentKills {
    const FLOOR: f32 = 0.2;
    const MAX_ENTRIES: usize = 50;
    const WINDOW_SECS: f64 = 600.0;

    /// Records a kill at `now` (game `Time` seconds) and returns the XP
    /// multiplier: ×0.9 per repeat of the same body within the window.
    pub fn register_kill(&mut self, attacker: Uid, victim_body: Body, now: f64) -> f32 {
        let buf = self.per_player.entry(attacker).or_default();
        while buf
            .front()
            .is_some_and(|(_, t)| now - *t > Self::WINDOW_SECS)
        {
            buf.pop_front();
        }
        let repeats = buf.iter().filter(|(b, _)| *b == victim_body).count() as i32;
        if buf.len() >= Self::MAX_ENTRIES {
            buf.pop_front();
        }
        buf.push_back((victim_body, now));
        0.9f32.powi(repeats).max(Self::FLOOR)
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server recent_kills` — 1 test PASS.

- [ ] **Step 3: Wire into the award loop**

(a) `DestroyEventData`: add `recent_kills: Write<'a, RecentKills>,` after Task 10's `levels` field.
(b) Next to Task 10's pre-pass: `let victim_body = data.bodies.get(ev.entity).copied();`
(c) Inside the `for_each`'s `if let Some((mut attacker_skill_set, attacker_uid, attacker_inventory))` block, before `handle_exp_gain`:

```rust
                        // Anti-farm: only players accumulate kill history.
                        let farm_mult = match victim_body {
                            Some(victim_body) if data.players.contains(*attacker) => data
                                .recent_kills
                                .register_kill(*attacker_uid, victim_body, data.time.0),
                            _ => 1.0,
                        };
```

and pass `exp_reward * farm_mult` as `handle_exp_gain`'s first argument.

(d) Make the type reachable and insert it: in `server/src/events/mod.rs` add `pub use entity_manipulation::RecentKills;` (or make the module path public); in `server/src/lib.rs` next to line 369 (`state.ecs_mut().insert(RecentClientIPs::default());`):

```rust
        state.ecs_mut().insert(events::RecentKills::default());
```

- [ ] **Step 4: Verify and commit**

Run: `cargo check -p veloren-server --all-targets && VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server recent_kills`
Expected: clean; PASS.

```bash
git add server/src/events/entity_manipulation.rs server/src/events/mod.rs server/src/lib.rs
git commit -m "feat: anti-farm XP dampening via per-player kill ring buffer"
```

---

### Task 12: Nameplates read `Level`, skull at +8

**Files:**
- Modify: `voxygen/src/hud/mod.rs:1526` (storage), `:2390-2392` (join tuple), `:2415` (closure args), `:2449-2466` (`Info` literal)
- Modify: `voxygen/src/hud/overhead.rs:71-80` (`Info` struct), `:162-171` (destructure), `:415-417` (skull condition)

How mobs reach `Info.level` today (verified): the nameplate join in hud/mod.rs includes `&skill_sets` for **every** entity and sets `level: Some(skill_set.character_level())` (line 2451) — so NPCs all show `[1]` (their skillsets carry no lifetime XP). This task makes the synced `comp::Level` win when present.

- [ ] **Step 1: Prefer the `Level` component**

After `let stances = ecs.read_storage::<comp::Stance>();` (line 1526): `let levels = ecs.read_storage::<comp::Level>();`

In the overhead join (~line 2390), extend the last tuple element from `(is_mounts.maybe(), is_riders.maybe(), stances.maybe())` to:

```rust
                (is_mounts.maybe(), is_riders.maybe(), stances.maybe(), levels.maybe()),
```

and the matching closure pattern `(is_mount, is_rider, stance)` (~line 2415) to `(is_mount, is_rider, stance, entity_level)`.

In the `overhead::Info` literal (line 2449), replace `level: Some(skill_set.character_level()),` with:

```rust
                            // NPCs carry a synced spawn Level; players derive
                            // theirs from lifetime XP.
                            level: entity_level
                                .map(|l| l.0)
                                .or_else(|| Some(skill_set.character_level())),
                            own_level: skill_sets.get(me).map(|s| s.character_level()),
```

(`skill_sets` is bound at line 1508 and `me` is used in the same closure at line 2419 — both in scope.)

- [ ] **Step 2: Skull when target ≥ own + 8**

In `voxygen/src/hud/overhead.rs`: add `pub own_level: Option<u16>,` after `pub level: Option<u16>,` in `Info` (line 73), and `own_level,` after `level,` in the `if let Some(Info { ... })` destructure (lines 162–171).

Before the `if let Some(combat_rating) = combat_rating` block (~line 415):

```rust
                    // World-difficulty zones: outleveling danger marker.
                    let level_skull =
                        matches!((level, own_level), (Some(t), Some(o)) if t >= o + 8);
```

and change the skull condition (line ~416) from
`if combat_rating > artifact_diffculty && !self.in_group {` to:

```rust
                        if (combat_rating > artifact_diffculty || level_skull) && !self.in_group {
```

- [ ] **Step 3: Compiler-driven sweep**

Run: `cargo check -p veloren-voxygen 2>&1 | grep -B2 "missing field\|own_level"`
Any other `overhead::Info { ... }` literal gets `own_level: None,`. Repeat until clean.

- [ ] **Step 4: Visual verification**

Use the `veloren-run` skill (fresh map). Verify: starting-zone wolves show `[1..3]` with band variety; after `/tp` far from town (admin `Tp`/`RtsimTp` — `common/src/cmd.rs:459,440`) mobs show 20+ and a skull when ≥ your level + 8; a far-zone wolf takes visibly longer to kill and hits harder than a starting-zone one.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/hud/mod.rs voxygen/src/hud/overhead.rs
git commit -m "feat: nameplates show NPC spawn level with outlevel skull"
```

---

### Task 13: World-map difficulty overlay

**Files:**
- Modify: `common/net/src/msg/world_msg.rs:24-38` (`WorldMapMsg` field)
- Modify: `world/src/sim/mod.rs` (`get_map`'s `WorldMapMsg` literal — locate via `grep -n "fn get_map" world/src/sim/mod.rs`)
- Modify: `client/src/lib.rs:736-977` (overlay layer image)
- Modify: `voxygen/src/settings/interface.rs:38` (setting + `Default`), `voxygen/src/session/settings_change.rs:172/:753` (variant + arm), `voxygen/src/hud/map.rs` (toggle + layer gating), `assets/voxygen/i18n/en/hud/map.ftl` (label)

- [ ] **Step 1: Extend the message and populate it**

In `pub struct WorldMapMsg` (world_msg.rs:24), after `pub alt: Grid<u32>,`:

```rust
    /// Region difficulty (1..=10), downsampled to one byte per 4×4-chunk cell
    /// (~64 KiB at the default 1024² map). Static per map seed — sent once on
    /// connect, no streaming.
    pub difficulty: Grid<u8>,
```

`World::get_map_data` ends with `..self.sim.get_map(index, ...)` (world/src/lib.rs), so the base literal lives in `WorldSim::get_map`. Inside its `WorldMapMsg { ... }` literal add:

```rust
            difficulty: {
                let sz = self.map_size_lg().chunks().map(|e| e as i32);
                Grid::populate_from(sz.map(|e| (e / 4).max(1)), |cell| {
                    let mut max = 1u8;
                    for dy in 0..4 {
                        for dx in 0..4 {
                            if let Some(chunk) = self.get(cell * 4 + Vec2::new(dx, dy)) {
                                max = max.max(chunk.difficulty);
                            }
                        }
                    }
                    max
                })
            },
```

Run: `cargo check --workspace --all-targets 2>&1 | grep -B2 "missing field .difficulty"` — fix any other `WorldMapMsg` literal (test/bot fixtures) with `difficulty: Grid::populate_from(Vec2::one(), |_| 1),`.

- [ ] **Step 2: Build the overlay image client-side**

In `client/src/lib.rs`, near `let rgba = world_map.rgba;` (line 737) add `let difficulty_grid = world_map.difficulty;`. After `let world_map_topo_img = make_raw(&world_map_topo)?;` (line 976):

```rust
            // Difficulty heat tint: green 1-3, yellow 4-6, orange 7-8, red 9-10.
            let world_map_difficulty = (0..map_size.y as i32)
                .flat_map(|y| (0..map_size.x as i32).map(move |x| Vec2::new(x, y)))
                .map(|pos| {
                    let d = difficulty_grid.get(pos / 4).copied().unwrap_or(1);
                    let (r, g, b) = match d {
                        1..=3 => (60, 200, 80),
                        4..=6 => (230, 210, 60),
                        7..=8 => (240, 140, 40),
                        _ => (220, 50, 50),
                    };
                    u32::from_le_bytes([r, g, b, 160])
                })
                .collect::<Vec<_>>();
            let world_map_difficulty_img = make_raw(&world_map_difficulty)?;
```

and change line 977 to:

```rust
            let world_map_layers =
                vec![world_map_rgb_img, world_map_topo_img, world_map_difficulty_img];
```

- [ ] **Step 3: Voxygen toggle (compiler-driven UI wiring)**

(a) `voxygen/src/settings/interface.rs`: add `pub map_show_zone_difficulty: bool,` next to `map_show_topo_map` (line 38) and `map_show_zone_difficulty: false,` in the `Default` impl in the same file.

(b) `voxygen/src/session/settings_change.rs`: add `MapShowZoneDifficulty(bool),` after `MapShowTopoMap(bool)` (line 172) and after the `MapShowDifficulty` arm (~line 756):

```rust
                    Interface::MapShowZoneDifficulty(map_show_zone_difficulty) => {
                        settings.interface.map_show_zone_difficulty = map_show_zone_difficulty;
                    },
```

(c) `voxygen/src/hud/map.rs`:
- Bind `let show_zone_difficulty = self.global_state.settings.interface.map_show_zone_difficulty;` next to `show_topo_map` (line 264).
- Layer gating (lines 483–500): replace the `} else if show_topo_map {` branch condition with `} else if (index == 1 && show_topo_map) || (index == 2 && show_zone_difficulty) {`.
- Toggle button: copy the "Show topographic map" block (lines 1686–1708, `map_mode_btn`/`map_mode_overlay`) into a new block placed `left_from` the existing mode-button row, with new widget ids `map_mode_zone_btn`/`map_mode_zone_overlay` (add both to the `widget_ids!` block — `grep -n "map_mode_btn" voxygen/src/hud/map.rs`), tooltip key `hud-map-zone_difficulty`, and event `MapShowZoneDifficulty(!show_zone_difficulty)`. Reuse `self.imgs.map_mode_overlay` as the icon for now.

(d) i18n — `assets/voxygen/i18n/en/hud/map.ftl`:

```ftl
hud-map-zone_difficulty = Difficulty zones
```

Resolve errors by class: missing `widget_ids` entries → add to the `widget_ids!` block; unknown `Interface::` variant → step (b) incomplete; settings deserialization → `InterfaceSettings` already uses serde defaults, follow the surrounding pattern. Iterate `cargo check -p veloren-voxygen` until clean.

- [ ] **Step 4: Visual verification**

Use the `veloren-run` skill: open the map, click the new mode button — green at towns grading to red at map edges/mountains, matching the nameplate levels observed in Task 12.

- [ ] **Step 5: Commit**

```bash
git add common/net/src/msg/world_msg.rs world/src/sim/mod.rs client/src/lib.rs voxygen/src assets/voxygen/i18n/en/hud/map.ftl
git commit -m "feat: world map difficulty-zone overlay with toggle"
```

---

### Task 14: Lint, format, changelog, branch finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. Fix warnings; no `#[allow]` without a justifying comment.

- [ ] **Step 2: Voxygen publish-profile clippy**

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
```
Expected: clean (catches hot-reload-gated paths the first run misses).

- [ ] **Step 3: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check.

- [ ] **Step 4: Full test suite**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-world -p veloren-rtsim -p veloren-server
```
Expected: PASS.

- [ ] **Step 5: Changelog**

Add under the unreleased section of `CHANGELOG.md`:

```markdown
- World regions now have difficulty zones (1-10). Creatures, dungeon mobs, and NPCs spawn with levels that scale their health, damage, and skills; XP scales with the level gap (heavily outleveled mobs give none), and the map shows a difficulty overlay.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for world difficulty zones"
```

- [ ] **Step 6: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). If Task 9 was gated out, file it as a follow-up tied to the classes-races plan.

---

## Phase 4 (deferred — re-verify anchors when started)

Multi-plane worlds are XL and explicitly deferred. Descriptions below are file-level; **do not trust this plan's line anchors when starting Phase 4 — re-verify everything against HEAD first.**

### Task P4.1: `PlaneId` type and serde-compatible portal extension
- **Files:** `common/src/rtsim.rs` (or adjacent shared module) — `PlaneId(pub u16)` newtype, `Default` = plane 0; `common/src/comp/misc.rs` — `PortalData` (today `{ target: Vec3<f32>, requires_no_aggro, buildup_time }`, line 54) gains `target_plane: Option<PlaneId>` with `#[serde(default)]` so existing portals deserialize unchanged.
- **Tests:** serde round-trip of a `PortalData` without the field (mirror Task 8's save-compat pattern); `None` means same-plane.
- **Scope guard:** plumbed but *unused at runtime* — teleport handling (`common/src/comp/teleport.rs`, `server/src/sys/teleporter.rs`) keeps ignoring it.

### Task P4.2: Pocket-plane site kind (intra-world)
- **Files:** `world/src/site/mod.rs` (`SiteKind` variant + `meta()`), new plot module `world/src/site/plot/pocket_plane.rs` structured like `gnarling.rs` (`generate`, `render_inner`, `apply_supplement`); placement in a reserved margin band of the world grid via `world/src/civ/mod.rs`; exclude the band from rtsim nav at the pathing entry points in `rtsim/src/rule/npc_ai/mod.rs`.
- **Difficulty:** the plot's `apply_supplement` hardcodes difficulty 9–10 interiors via the Task 7 `with_level` machinery, independent of host-chunk difficulty.
- **Portals:** entrance/exit pairs reuse `SpecialEntity::Teleporter(PortalData)` (`common/src/generation.rs:202-209`) exactly as waypoints are emitted in `world/src/lib.rs` — fixed-coordinate, same-world teleports; no engine change.
- **Milestone:** enter an overworld portal, arrive in a themed pocket plane, fight L28–30 mobs, portal back.

### Task P4.3: Transfer-queue prototype behind a feature flag
- **Files:** new `server/src/plane_transfer.rs` behind a `multiplane-prototype` cargo feature in `server/Cargo.toml`; models cross-plane travel as serialize-persisted-components → despawn → enqueue-spawn, structurally identical to the login flow (`server/src/state_ext.rs` character loading, `server/src/sys/persistence.rs`).
- **Persistence groundwork:** `plane` column (default 0) migration under `server/src/persistence/migrations/` for character position; rtsim data moves to `data/rtsim/plane_{id}/` (today one file — load path `server/src/rtsim/mod.rs:58`).
- **Out of scope even here:** N concurrent `World`/`IndexOwned` instances, cross-plane chat/trade, multi-process sharding. The prototype proves the despawn/respawn round-trip on one world only.

### Task P4.4: Map enlargement decision point
- Not an engineering task: `GenOpts` (`world/src/sim/mod.rs:147`) already supports `x_lg/y_lg = 11`. Revisit only if Phase 3 playtests show band crowding; treat as an offline asset bake (generate once on a big box, ship the map file) and record the decision in the spec's §7 table.
