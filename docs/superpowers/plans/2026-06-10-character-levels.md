# Character Levels (M1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Characters get a visible RPG-style level (1–60) derived from lifetime XP, with a level-up outcome event and nameplate display.

**Architecture:** Level is *computed* from the sum of `earned_exp` across all persisted skill groups — no new component, no DB migration, retroactively correct for existing characters. A new `Outcome::CharacterLevelUp` is emitted server-side when XP crosses a level boundary. The HUD nameplate shows the level via a new field on `overhead::Info` (its construction site already joins `SkillSet`).

**Tech Stack:** Rust nightly (2024 edition), specs ECS, conrod HUD. Design spec: `docs/superpowers/specs/2026-06-10-character-levels-design.md`.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/character-levels` off `development` before Task 1.
- Invoke the `veloren-progression` skill for context and the `superpowers:test-driven-development` skill before writing code.

---

### Task 1: Level curve functions

**Files:**
- Modify: `common/src/comp/skillset/mod.rs` (add functions near the top, after the existing `use` block; add tests at end of file)

- [ ] **Step 1: Write the failing tests**

At the very end of `common/src/comp/skillset/mod.rs`, add:

```rust
#[cfg(test)]
mod character_level_tests {
    use super::*;

    #[test]
    fn level_curve_boundaries() {
        assert_eq!(level_from_total_exp(0), 1);
        assert_eq!(level_from_total_exp(LEVEL_XP_BASE - 1), 1);
        assert_eq!(level_from_total_exp(LEVEL_XP_BASE), 2);
        assert_eq!(level_from_total_exp(u32::MAX), MAX_CHARACTER_LEVEL);
    }

    #[test]
    fn level_curve_is_monotonic() {
        let mut last = 0;
        for xp in (0..2_000_000u32).step_by(1000) {
            let level = level_from_total_exp(xp);
            assert!(level >= last, "level decreased at xp={xp}");
            last = level;
        }
    }

    #[test]
    fn total_exp_for_level_inverts_level_from_total_exp() {
        for level in 1..=MAX_CHARACTER_LEVEL {
            let xp = total_exp_for_level(level);
            assert_eq!(
                level_from_total_exp(xp),
                level,
                "level_from_total_exp(total_exp_for_level({level})) mismatch"
            );
            if xp > 0 {
                assert_eq!(level_from_total_exp(xp - 1), level - 1);
            }
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common character_level -- --nocapture`
Expected: FAIL to compile with "cannot find function `level_from_total_exp`".

- [ ] **Step 3: Implement the curve**

In `common/src/comp/skillset/mod.rs`, after the existing imports/consts at the top of the file (before `pub struct SkillGroupDef`), add:

```rust
/// Maximum character level (WoW/Diablo-style derived level — see
/// docs/superpowers/specs/2026-06-10-character-levels-design.md).
pub const MAX_CHARACTER_LEVEL: u16 = 60;
/// Cumulative XP required for level L is LEVEL_XP_BASE * (L - 1)^2.
pub const LEVEL_XP_BASE: u32 = 250;

/// Character level for a given lifetime (earned) XP total. Level 1 at 0 XP,
/// capped at MAX_CHARACTER_LEVEL. Inverse of [`total_exp_for_level`].
pub fn level_from_total_exp(total_exp: u32) -> u16 {
    let raw = ((total_exp / LEVEL_XP_BASE) as f64).sqrt() as u16 + 1;
    raw.min(MAX_CHARACTER_LEVEL)
}

/// Cumulative lifetime XP at which the given level is reached.
pub fn total_exp_for_level(level: u16) -> u32 {
    let l = u32::from(level.clamp(1, MAX_CHARACTER_LEVEL)) - 1;
    LEVEL_XP_BASE.saturating_mul(l).saturating_mul(l)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common character_level -- --nocapture`
Expected: 3 tests PASS.

Note: if `level_curve_boundaries` fails on the `u32::MAX` case with an off-by-one from
float truncation, that is a real bug in your implementation — `(u32::MAX / 250) as f64`
is exact enough here; do not loosen the test.

- [ ] **Step 5: Commit**

```bash
git add common/src/comp/skillset/mod.rs
git commit -m "feat: character level curve derived from lifetime XP"
```

---

### Task 2: `SkillSet::total_earned_exp()` and `SkillSet::character_level()`

**Files:**
- Modify: `common/src/comp/skillset/mod.rs` (inside `impl SkillSet`, after `add_experience` which ends near line 360; tests added to the `character_level_tests` module from Task 1)

- [ ] **Step 1: Write the failing tests**

Add inside `mod character_level_tests`:

```rust
    #[test]
    fn default_skillset_is_level_one() {
        let skill_set = SkillSet::default();
        assert_eq!(skill_set.total_earned_exp(), 0);
        assert_eq!(skill_set.character_level(), 1);
    }

    #[test]
    fn earning_exp_raises_character_level() {
        let mut skill_set = SkillSet::default();
        // General pool exists on default skillsets
        skill_set.add_experience(SkillGroupKind::General, LEVEL_XP_BASE);
        assert_eq!(skill_set.total_earned_exp(), LEVEL_XP_BASE);
        assert_eq!(skill_set.character_level(), 2);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common character_level -- --nocapture`
Expected: FAIL to compile with "no method named `total_earned_exp`".

- [ ] **Step 3: Implement the methods**

Inside `impl SkillSet` in `common/src/comp/skillset/mod.rs` (place directly after the
existing `pub fn add_experience` method), add:

```rust
    /// Lifetime XP earned across all skill groups. Drives the derived
    /// character level; relies on `earned_exp` being monotonically
    /// non-decreasing and persisted per skill group.
    pub fn total_earned_exp(&self) -> u32 {
        self.skill_groups
            .values()
            .map(|sg| sg.earned_exp)
            .fold(0, u32::saturating_add)
    }

    /// Derived character level (1..=MAX_CHARACTER_LEVEL). Not persisted —
    /// always computed from lifetime XP so it can never desync.
    pub fn character_level(&self) -> u16 {
        level_from_total_exp(self.total_earned_exp())
    }
```

Note: `SkillSet::add_experience(SkillGroupKind, u32)` already exists (~line 350) — do not
confuse it with `SkillGroup::add_experience`. If the test fails because the General group
is missing, check `SkillSet::default()` (line ~229): it unlocks `SkillGroupKind::General`,
so `add_experience` on General must succeed.

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common character_level -- --nocapture`
Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add common/src/comp/skillset/mod.rs
git commit -m "feat: SkillSet::character_level() derived from lifetime XP"
```

---

### Task 3: `Outcome::CharacterLevelUp` emitted on level boundary

**Files:**
- Modify: `common/src/outcome.rs:61-65` (new variant after `SkillPointGain`) and `common/src/outcome.rs:262-266` (no-position match arm)
- Modify: `server/src/events/entity_manipulation.rs:505-556` (`handle_exp_gain`)
- Modify (compiler-driven): `voxygen/src/scene/particle.rs:676` area, `voxygen/src/audio/sfx/mod.rs:777` area — wherever `cargo check` reports non-exhaustive matches

- [ ] **Step 1: Add the variant**

In `common/src/outcome.rs`, directly after the `SkillPointGain` variant (lines 61–65), add:

```rust
    CharacterLevelUp {
        uid: Uid,
        new_level: u16,
    },
```

- [ ] **Step 2: Fix the position match in the same file**

In the `get_pos` match at `common/src/outcome.rs:262-266`, extend the `None` arm:

```rust
            Outcome::ExpChange { .. }
            | Outcome::ComboChange { .. }
            | Outcome::Lightning { .. }
            | Outcome::SkillPointGain { .. }
            | Outcome::CharacterLevelUp { .. }
            | Outcome::HeadLost { .. } => None,
```

- [ ] **Step 3: Emit on level-up in `handle_exp_gain`**

In `server/src/events/entity_manipulation.rs`, function `handle_exp_gain` (line 505):
capture the level before the XP loop and compare after it. The function body currently
ends with the `Outcome::ExpChange` emit (lines 551–555). Apply this change:

```rust
    // ... existing code up to `let num_pools = xp_pools.len() as f32;` ...
    let level_before = skill_set.character_level();
    for pool in xp_pools.iter() {
        if let Some(level_outcome) =
            skill_set.add_experience(*pool, (exp_reward / num_pools).ceil() as u32)
        {
            outcomes_emitter.emit(Outcome::SkillPointGain {
                uid: *uid,
                skill_tree: *pool,
                total_points: level_outcome,
            });
        }
    }
    let level_after = skill_set.character_level();
    if level_after > level_before {
        outcomes_emitter.emit(Outcome::CharacterLevelUp {
            uid: *uid,
            new_level: level_after,
        });
    }
    outcomes_emitter.emit(Outcome::ExpChange {
        uid: *uid,
        exp: exp_reward as u32,
        xp_pools,
    });
```

(Only the `level_before`/`level_after`/`CharacterLevelUp` lines are new; the loop and the
`ExpChange` emit are unchanged existing code shown for placement.)

- [ ] **Step 4: Compiler-driven exhaustive-match fixes**

Run: `cargo check --workspace --all-targets 2>&1 | grep -A3 "non-exhaustive\|CharacterLevelUp"`

For every non-exhaustive `match outcome` error reported, add
`| Outcome::CharacterLevelUp { .. }` to the arm that already contains
`Outcome::SkillPointGain { .. }` **when that arm is a no-op/None arm**, e.g. in
`voxygen/src/scene/particle.rs` (~line 676):

```rust
            | Outcome::SkillPointGain { .. }
            | Outcome::CharacterLevelUp { .. }
```

In `voxygen/src/audio/sfx/mod.rs` the `SkillPointGain` arm (~line 777) *plays a sound*;
for v1 give `CharacterLevelUp` its own arm that does nothing, with a note for M2:

```rust
            // M2 (character-levels spec): dedicated level-up SFX/VFX/toast
            Outcome::CharacterLevelUp { .. } => {},
```

Repeat `cargo check --workspace --all-targets` until clean. Do NOT silence with a
wildcard `_ =>` arm — exhaustive matching is what makes future outcome work safe.

- [ ] **Step 5: Verify build and existing tests**

Run: `cargo check --workspace --all-targets`
Expected: clean.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`
Expected: PASS (no regressions).

- [ ] **Step 6: Commit**

```bash
git add common/src/outcome.rs server/src/events/entity_manipulation.rs voxygen/src
git commit -m "feat: emit CharacterLevelUp outcome when lifetime XP crosses a level"
```

---

### Task 4: Nameplate shows level

**Files:**
- Modify: `voxygen/src/hud/overhead.rs:72` (Info struct), `:161-169` (destructure), `:279-300` (name text)
- Modify: `voxygen/src/hud/mod.rs:2449-2465` (Info construction — `skill_set` is already bound at `:2403`)

- [ ] **Step 1: Add the field to `overhead::Info`**

In `voxygen/src/hud/overhead.rs`, the struct at line ~72:

```rust
pub struct Info<'a> {
    pub name: Option<String>,
    pub level: Option<u16>,
    // ... existing fields unchanged (health, buffs, energy, combat_rating, hardcore, stance)
}
```

- [ ] **Step 2: Destructure and render it**

Same file, the `if let Some(Info { ... })` destructure at lines 161–169 — add `level`:

```rust
        if let Some(Info {
            ref name,
            level,
            health,
            buffs,
            energy,
            combat_rating,
            hardcore,
            stance,
        }) = self.info
```

Directly after the destructure block (before the `hp_percentage` calculation at ~line 172),
build the display string:

```rust
            let display_name = match level {
                Some(level) => format!("{} [{}]", name.as_deref().unwrap_or(""), level),
                None => name.as_deref().unwrap_or("").to_string(),
            };
```

Then replace **both** name `Text::new` calls (the shadow text at ~line 279 and the main
text at ~line 287): `Text::new(name.as_deref().unwrap_or(""))` becomes
`Text::new(&display_name)`.

- [ ] **Step 3: Populate it at the construction site**

In `voxygen/src/hud/mod.rs` at line ~2449 (`overhead::Info { ... }` literal — the closure
already binds `skill_set` from the join):

```rust
                        let info = display_overhead_info.then(|| overhead::Info {
                            name: Some(i18n.get_content(&stats.name)),
                            level: Some(skill_set.character_level()),
                            // ... existing fields unchanged
                        });
```

- [ ] **Step 4: Check for other `overhead::Info` constructors**

Run: `grep -rn "overhead::Info\|Info {" voxygen/src/hud/overhead.rs voxygen/src/hud/mod.rs | grep -v "DebugInfo"`
If any other construction site exists (e.g. in `overhead.rs` tests or map markers), add
`level: None` there. Then run: `cargo check -p veloren-voxygen`
Expected: clean.

- [ ] **Step 5: Visual verification**

Use the `veloren-run` skill to launch server + client. In game:
- Approach an NPC: nameplate shows `Name [N]`.
- Kill mobs until a level-up; confirm a `CharacterLevelUp` outcome appears in client logs
  (or via the `veloren-telemetry` skill if running the logging-verbose build).

- [ ] **Step 6: Commit**

```bash
git add voxygen/src/hud/overhead.rs voxygen/src/hud/mod.rs
git commit -m "feat: show character level on nameplates"
```

---

### Task 5: Lint, format, and branch finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. Fix any warnings (do not `#[allow]` without a comment justifying it).

- [ ] **Step 2: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check.

- [ ] **Step 3: Full test suite**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server`
Expected: PASS.

- [ ] **Step 4: Update CHANGELOG and commit**

Add under the unreleased section of `CHANGELOG.md`:
```markdown
- Characters now have a level (1–60) derived from lifetime experience, shown on nameplates.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for character levels"
```

- [ ] **Step 5: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging
into `development`). M2/M3 follow-ups are tracked in the design spec's milestone table.
