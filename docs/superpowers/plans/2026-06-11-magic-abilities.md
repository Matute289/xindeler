# Magic Abilities v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The magic layer from the design spec: per-spell cooldowns on top of Energy casting, a weaponless innate-ability pool (`AuxiliaryAbility::Innate`), a ground-targeted AoE state (`GroundAoe`), three new BuffKinds (Terrified/Charmed/Hollowtouched), three caster implements (Tome/HolySymbol/Focus), and 15 of the 16 v1 spells as RON assets (Wildshape #12 is deferred to the spec's Phase-4 polish pass — `Transform` player edge cases are their own milestone).

**Architecture:** Spells stay 100% data-driven: each is a RON file deserializing into a `CharacterAbility` variant, registered in `ability_set_manifest.ron`. New Rust is limited to genuinely new mechanics: an `AbilityCooldowns` component (decay-on-check, no tick system) gated in `handle_ability` (`common/src/states/utils.rs:1440`), an `AbilityPool` component resolved through `AbilityMap` `Custom(...)` keys for equipment-independent abilities, the `ground_aoe` character state modeled on `states/explosion.rs` (target locked from `input_attr.select_pos`, the same client-aim channel `Blink` already uses; strike FX come free from the existing `ExplosionEvent` → `Outcome::Explosion` path), and three BuffKinds.

**Tech Stack:** Rust nightly (2024 edition), specs ECS, RON assets. Design spec: `docs/superpowers/specs/2026-06-10-magic-abilities-design.md`.

**Depends on:** Phases 1–3 have **NO external dependency** — cooldowns, Innate plumbing, GroundAoe, buffs, and caster tools are self-contained (spells ship skill-ungated, usable by anyone holding the implement). Only Task 15 (class skill gating) depends on **classes-races plan Task 1** (`Skill::Class` / ClassKind); it is grep-gated and skippable until that lands.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/magic-v1` off `development` before Task 1.
- Invoke the `veloren-abilities` skill for pipeline context and `superpowers:test-driven-development` before writing code.
- Never silence a non-exhaustive match with `_ =>` — exhaustive matching is the safety net for every enum this plan extends.

---

## Phase 1 — Cooldowns + innate-ability foundations (no dependencies)

### Task 1: `SpellSchool`, `AbilityMeta.cooldown`, and the `AbilityCooldowns` component

**Files:**
- Modify: `common/src/comp/ability.rs` (component after `ActiveAbilities` impls ~line 78; `SpellSchool` + meta fields at `AbilityMeta`, line 3525; tests at end of file)
- Modify: `common/state/src/state.rs:247` (registration), `common/net/src/synced_components.rs:74,303` (sync), `server/src/state_ext.rs:746` area (insertion), `common/src/comp/mod.rs` (re-export)

- [ ] **Step 1: Write the failing tests**

At the very end of `common/src/comp/ability.rs`:

```rust
#[cfg(test)]
mod ability_cooldown_tests {
    use super::*;
    use crate::resources::Time;

    #[test]
    fn fresh_component_is_ready() {
        let cds = AbilityCooldowns::default();
        assert!(cds.is_ready("common.abilities.spells.ruin.shatterburst", Time(0.0)));
    }

    #[test]
    fn set_blocks_until_ready_time() {
        let mut cds = AbilityCooldowns::default();
        cds.set("a", Time(10.0), 30.0);
        assert!(!cds.is_ready("a", Time(10.0)));
        assert!(!cds.is_ready("a", Time(39.9)));
        assert!(cds.is_ready("a", Time(40.0)));
        assert!(cds.is_ready("b", Time(10.0)));
    }

    #[test]
    fn set_prunes_expired_entries() {
        let mut cds = AbilityCooldowns::default();
        cds.set("a", Time(0.0), 5.0);
        // "a" became ready at t=5; setting "b" at t=100 prunes it
        cds.set("b", Time(100.0), 5.0);
        assert_eq!(cds.0.len(), 1);
        assert!(cds.ready_at("b").is_some());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common ability_cooldown -- --nocapture`
Expected: FAIL to compile with "cannot find type `AbilityCooldowns`".

- [ ] **Step 3: Implement component + meta fields**

In `common/src/comp/ability.rs`, extend the resources import (`use crate::{... resources::Secs ...}`) to `resources::{Secs, Time}`. Directly after the `impl Default for ActiveAbilities` block (line ~78), add:

```rust
/// Per-ability cooldowns, keyed by ability id (the RON asset path, or the
/// pool key for innate abilities). Stores the absolute game `Time` at which
/// the ability is ready again; expired entries are pruned opportunistically
/// on `set`, so no tick system is needed (magic-abilities spec §8). Not
/// persisted across logout (accepted v1 exploit surface, spec Open Q #3).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AbilityCooldowns(pub HashMap<String, Time>);

impl AbilityCooldowns {
    pub fn is_ready(&self, ability_id: &str, now: Time) -> bool {
        self.0
            .get(ability_id)
            .is_none_or(|ready_at| now.0 >= ready_at.0)
    }

    pub fn ready_at(&self, ability_id: &str) -> Option<Time> {
        self.0.get(ability_id).copied()
    }

    pub fn set(&mut self, ability_id: &str, now: Time, cooldown_secs: f32) {
        self.0.retain(|_, ready_at| ready_at.0 > now.0);
        self.0
            .insert(ability_id.to_string(), Time(now.0 + f64::from(cooldown_secs)));
    }
}

impl Component for AbilityCooldowns {
    type Storage = DerefFlaggedStorage<Self, specs::DenseVecStorage<Self>>;
}
```

In the same file, before `pub struct AbilityMeta` (line ~3525), add:

```rust
/// Spell school taxonomy (magic-abilities spec §1). Working names; the
/// lore-cosmology spec owns final names. Carried in `AbilityMeta` for UI
/// grouping, class gating, and future resistances.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpellSchool {
    Ruin,
    Wardcraft,
    Threshold,
    Flux,
    Dawnfire,
    Gravesong,
    Verdance,
    Pactbinding,
    Hollow,
}
```

And inside `AbilityMeta` (which has `#[serde(deny_unknown_fields)]` and derives `Default`/`Copy` — both new fields are `Copy` and defaulted), after `precision_power_mult`:

```rust
    /// School this ability belongs to, if it is a spell.
    #[serde(default)]
    pub school: Option<SpellSchool>,
    /// Per-ability cooldown in seconds, gated in `handle_ability`.
    #[serde(default)]
    pub cooldown: Option<f32>,
```

- [ ] **Step 4: Register, sync, insert, re-export**

- `common/src/comp/mod.rs`: add `AbilityCooldowns` (and `SpellSchool`) to the existing `ability::{...}` re-export list (grep `ActiveAbilities` in that file to find it).
- `common/state/src/state.rs`: next to `ecs.register::<comp::ActiveAbilities>();` (line 247), add `ecs.register::<comp::AbilityCooldowns>();`.
- `common/net/src/synced_components.rs`: in the "Synced to the client only for its own entity" block (line ~74, next to `active_abilities: ActiveAbilities,`), add `ability_cooldowns: AbilityCooldowns,`; then next to `impl NetSync for ActiveAbilities` (line 303) add:
  ```rust
  impl NetSync for AbilityCooldowns {
      const SYNC_FROM: SyncFrom = SyncFrom::ClientEntity;
  }
  ```
- `server/src/state_ext.rs`, in `initialize_character_data` (line 610) next to the `write_component_ignore_entity_dead(entity, active_abilities)` call at ~line 746, add:
  ```rust
  self.write_component_ignore_entity_dead(entity, comp::AbilityCooldowns::default());
  ```

- [ ] **Step 5: Verify**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common ability_cooldown` — Expected: 3 PASS.
Run: `cargo check --workspace --all-targets` — Expected: clean (new meta fields are `#[serde(default)]`, so all existing ability RONs still deserialize).

- [ ] **Step 6: Commit**

```bash
git add common/src common/state/src common/net/src server/src
git commit -m "feat: AbilityCooldowns component, SpellSchool, AbilityMeta.cooldown"
```

---

### Task 2: Cooldown gate in `handle_ability` + `SetAbilityCooldownEvent`

**Files:**
- Modify: `common/src/states/behavior.rs:136` (`JoinData`) and `:176` (`JoinStruct`)
- Modify: `common/systems/src/character_behavior.rs` (system join — compiler-driven)
- Modify: `common/src/states/utils.rs:1440` (`handle_ability`)
- Modify: `common/src/event.rs:497` area (new event), `server/src/events/event_types.rs:15,74`, `server/src/events/entity_manipulation.rs:102,2745` area (dispatch + handler)

- [ ] **Step 1: Plumb the component into `JoinData`**

In `common/src/states/behavior.rs`, add to `JoinData` (after `active_abilities` at line 161) and the matching field in `JoinStruct`:

```rust
    pub ability_cooldowns: Option<&'a AbilityCooldowns>,
```

Run `cargo check -p veloren-common -p veloren-common-systems 2>&1 | head -40` and fix every construction site the compiler reports (the `JoinData::new` body in `behavior.rs` and the `JoinStruct` literal in `common/systems/src/character_behavior.rs`, which needs `ReadStorage<'a, AbilityCooldowns>` added to its `SystemData` and `.maybe()` in the join — copy exactly how `active_abilities` flows through that system).

- [ ] **Step 2: Define the event**

In `common/src/event.rs`, next to `TeleportToEvent` (line 497):

```rust
pub struct SetAbilityCooldownEvent {
    pub entity: EcsEntity,
    pub ability_id: String,
    pub cooldown_secs: f32,
}
```

Register it exactly like `TeleportToEvent`: add to the import + macro list in `server/src/events/event_types.rs` (lines 15 and 74), add `event_dispatch::<SetAbilityCooldownEvent>(builder, &[]);` next to line 102 of `server/src/events/entity_manipulation.rs`, and implement the handler next to `impl ServerEvent for TeleportToEvent` (line 2745) — the trait shape must match its neighbors (compiler-checked):

```rust
impl ServerEvent for SetAbilityCooldownEvent {
    type SystemData<'a> = (
        Read<'a, Time>,
        WriteStorage<'a, comp::AbilityCooldowns>,
    );

    fn handle(
        events: impl ExactSizeIterator<Item = Self>,
        (time, mut ability_cooldowns): Self::SystemData<'_>,
    ) {
        for ev in events {
            if let Some(cooldowns) = ability_cooldowns.get_mut(ev.entity) {
                cooldowns.set(&ev.ability_id, *time, ev.cooldown_secs);
            }
        }
    }
}
```

- [ ] **Step 3: Gate and write in `handle_ability`**

In `common/src/states/utils.rs`, `handle_ability` (line 1440). The cooldown check goes **first** in the existing `.filter(...)` so energy is never paid for an ability on cooldown (`requirements_paid` mutates `update`):

```rust
            .filter(|(ability, _, spec_ability)| {
                cooldown_ready(data, ability, spec_ability)
                    && ability.requirements_paid(data, update)
            })
```

Then, inside the `Ok(character_state)` arm (after `update.character = character_state;`), start the cooldown:

```rust
                if let Some(cooldown_secs) = ability_meta.cooldown
                    && let Some(id) =
                        spec_ability.ability_id(Some(data.character), data.inventory)
                {
                    output_events.emit_server(SetAbilityCooldownEvent {
                        entity: data.entity,
                        ability_id: id.to_string(),
                        cooldown_secs,
                    });
                }
```

And add the helper below `handle_ability`:

```rust
/// An ability with `meta.cooldown` may only fire when `AbilityCooldowns` says
/// it is ready. Runs on client and server; the server-side check is
/// authoritative (the event above is server-only), the client check uses the
/// synced component for prediction.
fn cooldown_ready(
    data: &JoinData<'_>,
    ability: &CharacterAbility,
    spec_ability: &SpecifiedAbility,
) -> bool {
    ability.ability_meta().cooldown.is_none_or(|_| {
        spec_ability
            .ability_id(Some(data.character), data.inventory)
            .is_none_or(|id| {
                data.ability_cooldowns
                    .is_none_or(|cds| cds.is_ready(id, *data.time))
            })
    })
}
```

Import `SetAbilityCooldownEvent` in the `crate::event::{...}` use block at the top of `utils.rs` (it already imports `ChangeStanceEvent`, `BuffEvent`, …).

- [ ] **Step 4: Verify**

Run: `cargo check --workspace --all-targets` — Expected: clean.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` — Expected: PASS, no regressions.
Manual (after Phase 4 lands content): cast a cooldown spell twice fast; second cast must do nothing.

- [ ] **Step 5: Commit**

```bash
git add common/src common/systems/src server/src
git commit -m "feat: gate abilities on per-spell cooldowns in handle_ability"
```

---

### Task 3: `AuxiliaryAbility::Innate` + `AbilityPool` component

**Files:**
- Modify: `common/src/comp/ability.rs` — `AuxiliaryAbility` (line 645), `Ability` (line 386), `try_ability_set_key` (404), `ability_id` (419), `is_from_wielded` (499), `SpecifiedAbility::ability_id` (534), `activate_ability` (192), `all_available_abilities` (315), new component
- Modify: registration/sync/insertion (same four files as Task 1 Step 4)

- [ ] **Step 1: Add the component and variants**

After the `AbilityCooldowns` impls from Task 1, add:

```rust
/// Ability-set keys (manifest `Custom(...)` entries) granted to a character
/// independent of equipment: racial innates and class signature abilities
/// (magic-abilities spec §3 Path B). Indexed by `AuxiliaryAbility::Innate(i)`.
/// Each key's set `primary` is the granted ability; the key itself doubles as
/// the frontend ability id (icon/i18n key), like Contextualized pseudo_ids.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AbilityPool {
    pub abilities: Vec<String>,
}

impl Component for AbilityPool {
    type Storage = DerefFlaggedStorage<Self, specs::DenseVecStorage<Self>>;
}
```

In `AuxiliaryAbility` (line 645) add `Innate(usize),` before `Empty`; in `Ability` (line 386) add `InnateAux(usize),` before `Empty` (this is the extension the `ArmorAbility` placeholder comment anticipated). Extend the `From<AuxiliaryAbility> for Ability` impl (line 652) with `AuxiliaryAbility::Innate(i) => Ability::InnateAux(i),`.

- [ ] **Step 2: Resolution paths (compiler-driven, real arms)**

Run `cargo check -p veloren-common 2>&1 | grep -B2 "non-exhaustive\|InnateAux"` and fill every arm:

- `try_ability_set_key` (line 404): fold into the aux arm — `Self::GliderAux(idx) | Self::OffWeaponAux(idx) | Self::MainWeaponAux(idx) | Self::InnateAux(idx) => AbilityInput::Auxiliary(*idx),`
- `is_from_wielded` (line 499): `Ability::InnateAux(_)` joins the `false` arm with `SpeciesMovement | Empty`.
- `Ability::ability_id` (line 419): change the signature to take `ability_pool: Option<&AbilityPool>` as a new parameter (after `skill_set`), and in **both** source match blocks add:
  ```rust
                Ability::InnateAux(index) => ability_pool
                    .and_then(|pool| pool.abilities.get(index))
                    .map(|key| key.as_str()),
  ```
- `SpecifiedAbility::ability_id` (line 534): same new parameter, same two arms.
- `ActiveAbilities::activate_ability` (line 192): add parameters `ability_pool: Option<&AbilityPool>` and `ability_map: &AbilityMap` (the `JoinData` already carries `ability_map`, behavior.rs:162). Add the match arm next to `Ability::GliderAux`:
  ```rust
            Ability::InnateAux(index) => ability_pool
                .and_then(|pool| pool.abilities.get(index))
                .and_then(|key| {
                    ability_map
                        .get_ability_set(&AbilitySpec::Custom(key.clone()))
                        .and_then(|set| set.primary(Some(skill_set), context))
                        .map(|(item, i)| {
                            (
                                item.ability.clone().adjusted_by_skills(skill_set, None),
                                false,
                                spec_ability(i),
                            )
                        })
                }),
  ```
  (`AbilitySpec` and `AbilityMap` are already imported from `tool` in this file's use block — extend it if not.)
- `all_available_abilities` (line 315): add `ability_pool: Option<&AbilityPool>` parameter and, before `ability_buff` is returned:
  ```rust
        // Push innate (class/racial) abilities
        if let Some(pool) = ability_pool {
            (0..pool.abilities.len())
                .map(AuxiliaryAbility::Innate)
                .for_each(|a| ability_buff.push(a));
        }
  ```

- [ ] **Step 3: Fix call sites workspace-wide**

Run `cargo check --workspace --all-targets 2>&1 | grep -A3 "arguments\|InnateAux"` repeatedly. Known call sites: `common/src/states/utils.rs:1448` (`activate_ability` — pass `data.ability_pool, data.ability_map`; add `pub ability_pool: Option<&'a AbilityPool>` to `JoinData`/`JoinStruct` exactly as in Task 2 Step 1), `voxygen/src/hud/skillbar.rs` (~1119, ~1263, ~1294 — `ability_id` calls; the HUD reads the synced `AbilityPool` from the client ECS, pass it where available or `None` where the entity is not the player), `voxygen/src/hud/diary.rs` and `voxygen/src/hud/mod.rs` (`all_available_abilities`). Never pass a wildcard; thread the real component through each site's existing storage fetches.

- [ ] **Step 4: Register, sync, insert**

Repeat Task 1 Step 4 for `AbilityPool`: re-export in `comp/mod.rs`, `ecs.register::<comp::AbilityPool>()` in `common/state/src/state.rs:247` area, `ability_pool: AbilityPool,` + `impl NetSync for AbilityPool { const SYNC_FROM: SyncFrom = SyncFrom::ClientEntity; }` in `synced_components.rs`, and `self.write_component_ignore_entity_dead(entity, comp::AbilityPool::default());` in `initialize_character_data` (populated per-species in Task 14).

- [ ] **Step 5: Verify and commit**

Run: `cargo check --workspace --all-targets` then `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` — Expected: clean / PASS.

```bash
git add common voxygen/src server/src
git commit -m "feat: AbilityPool component and AuxiliaryAbility::Innate resolution"
```

---

### Task 4: Persistence arms for `Innate` + round-trip test

**Files:**
- Modify: `server/src/persistence/json_models.rs:126` (`aux_ability_to_string`), `:136` (`aux_ability_from_string`), tests at end of file

- [ ] **Step 1: Write the failing test**

At the end of `server/src/persistence/json_models.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn innate_aux_ability_round_trips() {
        use common::comp::ability::AuxiliaryAbility;
        for ability in [
            AuxiliaryAbility::Innate(0),
            AuxiliaryAbility::Innate(3),
            AuxiliaryAbility::MainWeapon(1),
            AuxiliaryAbility::Empty,
        ] {
            let s = aux_ability_to_string(ability);
            assert_eq!(aux_ability_from_string(&s), ability);
        }
        assert_eq!(
            aux_ability_to_string(AuxiliaryAbility::Innate(3)),
            "Innate:index:3"
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server innate_aux` — Expected: FAIL (non-exhaustive match in `aux_ability_to_string` reported by the compiler — that error IS the to-do list).

- [ ] **Step 3: Implement both arms**

In `aux_ability_to_string` (line 126): `AuxiliaryAbility::Innate(index) => format!("Innate:index:{}", index),`. In `aux_ability_from_string` (line 136), a new arm modeled byte-for-byte on the `Some("Glider")` arm (lines 182–202), with `Some("Innate")` and `AuxiliaryAbility::Innate(index)` in the `Ok` case and the same two `dev_panic!` fallbacks to `AuxiliaryAbility::Empty`.

- [ ] **Step 4: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server innate_aux` — Expected: PASS.
Dry-run migration safety: copy a dev character DB, start the server against the copy, log in, bind nothing — old `ability_sets` strings must load unchanged (the new arm only adds a parse case).

```bash
git add server/src/persistence/json_models.rs
git commit -m "feat: persist AuxiliaryAbility::Innate in ability_sets"
```

---

## Phase 2 — New machinery: GroundAoe + BuffKinds (no dependencies)

### Task 5: `CharacterAbility::GroundAoe` variant

**Files:**
- Modify: `common/src/comp/ability.rs` — enum variant (after `Explosion`, line 1088), `adjusted_by_stats` (after the Explosion arm ending line 1904), `energy_cost`/`ability_meta`/`try_from` arms (compiler-driven)

- [ ] **Step 1: Add the variant**

Directly after the `Explosion { ... }` variant (line 1062–1088):

```rust
    GroundAoe {
        energy_cost: f32,
        buildup_duration: f32,
        /// Telegraph time between target lock and the strike
        delay: f32,
        recover_duration: f32,
        max_range: f32,
        radius: f32,
        min_falloff: f32,
        damage: f32,
        poise: f32,
        knockback: Knockback,
        #[serde(default)]
        dodgeable: Dodgeable,
        #[serde(default)]
        reagent: Option<Reagent>,
        /// If true the caster cannot move during the telegraph
        #[serde(default)]
        rooted_cast: bool,
        #[serde(default)]
        meta: AbilityMeta,
    },
```

- [ ] **Step 2: Compiler-driven arms**

Run `cargo check -p veloren-common 2>&1 | grep -B5 "non-exhaustive"`. Every big `match self` over `CharacterAbility` needs a `GroundAoe` arm — known sites: the `energy_cost` accessor (the match containing `CharacterAbility::Explosion { energy_cost, .. }` at line ~1433: add `| CharacterAbility::GroundAoe { energy_cost, .. }` to that arm), `ability_meta()` (add `| GroundAoe { meta, .. }` to the arm listing `Explosion`), and `adjusted_by_stats` — model on the Explosion arm (lines 1877–1904):

```rust
            GroundAoe {
                ref mut energy_cost,
                ref mut buildup_duration,
                delay: _,
                ref mut recover_duration,
                ref mut max_range,
                ref mut radius,
                min_falloff: _,
                ref mut damage,
                poise: ref mut poise_damage,
                ref mut knockback,
                dodgeable: _,
                reagent: _,
                rooted_cast: _,
                meta: _,
            } => {
                *energy_cost /= stats.energy_efficiency;
                *buildup_duration /= stats.speed;
                *recover_duration /= stats.speed;
                *damage *= stats.power;
                *poise_damage *= stats.effect_power;
                knockback.strength *= stats.effect_power;
                *radius *= stats.range;
                *max_range *= stats.range;
            },
```

The `try_from` arm is written in Task 6 (it needs the state types); until then a `todo!()` arm is acceptable **only between Steps 2 and Task 6 Step 2 of the same working session** — do not commit it.

- [ ] **Step 3: Commit (together with Task 6)** — see Task 6 Step 5.

---

### Task 6: `states/ground_aoe.rs` + dispatch

**Files:**
- Create: `common/src/states/ground_aoe.rs`
- Modify: `common/src/states/mod.rs` (alphabetical insert after `glide_wield`), `common/src/comp/character_state.rs` (enum variant + `behavior()` match ~line 783 + `handle_event()` ~line 848), `common/src/comp/ability.rs` (`try_from` arm)

- [ ] **Step 1: The state (full file)**

`common/src/states/ground_aoe.rs` — same skeleton as `states/explosion.rs` (verified: Buildup → Action → strike-on-transition → Recover), with the target locked at the end of Buildup the way `states/blink.rs` reads `input_attr.select_pos`:

```rust
use crate::{
    Damage, DamageKind, Explosion, GroupTarget, Knockback, RadiusEffect,
    combat::{Attack, AttackDamage, AttackEffect, CombatEffect, CombatRequirement},
    comp::{
        CharacterState, StateUpdate, ability::Dodgeable, character_state::OutputEvents,
        item::Reagent,
    },
    event::ExplosionEvent,
    states::{
        behavior::{CharacterBehavior, JoinData},
        utils::*,
    },
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use vek::Vec3;

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StaticData {
    pub buildup_duration: Duration,
    /// Telegraph duration between target lock and strike
    pub delay: Duration,
    pub recover_duration: Duration,
    pub max_range: f32,
    pub radius: f32,
    pub min_falloff: f32,
    pub damage: f32,
    pub poise: f32,
    pub knockback: Knockback,
    pub dodgeable: Dodgeable,
    pub reagent: Option<Reagent>,
    pub rooted_cast: bool,
    pub ability_info: AbilityInfo,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Data {
    pub static_data: StaticData,
    pub timer: Duration,
    pub stage_section: StageSection,
    /// Locked at the Buildup -> Action transition; the telegraph and strike
    /// happen here even if the caster moves away.
    pub target_pos: Option<Vec3<f32>>,
}

impl CharacterBehavior for Data {
    fn behavior(&self, data: &JoinData, output_events: &mut OutputEvents) -> StateUpdate {
        let mut update = StateUpdate::from(data);

        handle_orientation(data, &mut update, 1.0, None);
        let move_efficiency = if self.static_data.rooted_cast { 0.0 } else { 0.7 };
        handle_move(data, &mut update, move_efficiency);

        match self.stage_section {
            StageSection::Buildup => {
                if self.timer < self.static_data.buildup_duration {
                    update.character = CharacterState::GroundAoe(Data {
                        timer: tick_attack_or_default(data, self.timer, None),
                        ..*self
                    });
                } else {
                    // Lock the target: client-selected ground pos (same trust
                    // model as Blink), clamped to max_range; fallback to a
                    // point max_range ahead along the look dir.
                    let aim = self
                        .static_data
                        .ability_info
                        .input_attr
                        .and_then(|attr| attr.select_pos)
                        .unwrap_or_else(|| {
                            data.pos.0 + *data.inputs.look_dir * self.static_data.max_range
                        });
                    let offset = aim - data.pos.0;
                    let clamped = if offset.magnitude_squared()
                        > self.static_data.max_range.powi(2)
                    {
                        data.pos.0 + offset.normalized() * self.static_data.max_range
                    } else {
                        aim
                    };
                    update.character = CharacterState::GroundAoe(Data {
                        timer: Duration::default(),
                        stage_section: StageSection::Action,
                        target_pos: Some(clamped),
                        ..*self
                    });
                }
            },
            StageSection::Action => {
                if self.timer < self.static_data.delay {
                    update.character = CharacterState::GroundAoe(Data {
                        timer: tick_attack_or_default(data, self.timer, None),
                        ..*self
                    });
                } else {
                    // Strike: server resolves an explosion at the locked pos.
                    if let Some(pos) = self.target_pos {
                        output_events.emit_server(ExplosionEvent {
                            pos,
                            explosion: Explosion {
                                effects: vec![RadiusEffect::Attack {
                                    attack: Attack::new(Some(self.static_data.ability_info))
                                        .with_damage(AttackDamage::new(
                                            Damage {
                                                kind: DamageKind::Energy,
                                                value: self.static_data.damage,
                                            },
                                            Some(GroupTarget::OutOfGroup),
                                            rand::random(),
                                        ))
                                        .with_effect(
                                            AttackEffect::new(
                                                Some(GroupTarget::OutOfGroup),
                                                CombatEffect::Poise(self.static_data.poise),
                                            )
                                            .with_requirement(CombatRequirement::AnyDamage),
                                        )
                                        .with_effect(
                                            AttackEffect::new(
                                                Some(GroupTarget::OutOfGroup),
                                                CombatEffect::Knockback(
                                                    self.static_data.knockback,
                                                ),
                                            )
                                            .with_requirement(CombatRequirement::AnyDamage),
                                        ),
                                    dodgeable: self.static_data.dodgeable,
                                }],
                                radius: self.static_data.radius,
                                reagent: self.static_data.reagent,
                                min_falloff: self.static_data.min_falloff,
                            },
                            owner: Some(*data.uid),
                        });
                    }
                    update.character = CharacterState::GroundAoe(Data {
                        timer: Duration::default(),
                        stage_section: StageSection::Recover,
                        ..*self
                    });
                }
            },
            StageSection::Recover => {
                if self.timer < self.static_data.recover_duration {
                    update.character = CharacterState::GroundAoe(Data {
                        timer: tick_attack_or_default(
                            data,
                            self.timer,
                            Some(data.stats.recovery_speed_modifier),
                        ),
                        ..*self
                    });
                } else {
                    end_ability(data, &mut update);
                }
            },
            _ => {
                end_ability(data, &mut update);
            },
        }

        // At end of state logic so an interrupt isn't overwritten —
        // poise breaks during Buildup cancel the cast (spec §8 counterplay).
        handle_interrupts(data, &mut update, output_events);

        update
    }
}
```

Add `pub mod ground_aoe;` to `common/src/states/mod.rs` (alphabetical, after `glide_wield`).

- [ ] **Step 2: `try_from` + dispatch**

In `common/src/comp/ability.rs`, replace the Task 5 placeholder with the real `try_from` arm (place after the `Explosion` arm; field-by-field `Duration::from_secs_f32(*x)` conversion exactly like the `Explosion` arm earlier in the same match, with `timer: Duration::default(), stage_section: StageSection::Buildup, target_pos: None`).

In `common/src/comp/character_state.rs`: add `GroundAoe(ground_aoe::Data),` to the `CharacterState` enum, then `CharacterState::GroundAoe(data) => data.behavior(j, output_events),` in `behavior()` (~line 783) and the matching `handle_event` arm (~line 848). Run `cargo check -p veloren-common 2>&1 | grep -B3 non-exhaustive` and fill every remaining accessor match (`ability_info()`, `stage_section()`, `durations()`, `timer()`, etc. in the same file) following what `Explosion(data)` does in each — never `_ =>`.

- [ ] **Step 3: State-transition unit test**

At the end of `common/src/comp/ability.rs` tests (or a new `#[cfg(test)]` module in `ground_aoe.rs` if `JoinData` proves too heavy to construct — it is; test the pure part):

```rust
#[cfg(test)]
mod ground_aoe_tests {
    // JoinData is impractical to construct in isolation; behaviour is covered
    // by the deserialization test below and the Task 7 in-game check. Here we
    // pin the RON contract.
    use crate::{assets::AssetExt, comp::CharacterAbility};

    #[test]
    fn shatterburst_deserializes_as_ground_aoe() {
        let ability = crate::assets::Ron::<CharacterAbility>::load_expect(
            "common.abilities.spells.ruin.shatterburst",
        )
        .read()
        .0
        .clone();
        assert!(matches!(ability, CharacterAbility::GroundAoe { .. }));
    }
}
```

(The asset lands in Step 4 — run the test before it to see the load failure, after it to see PASS.)

- [ ] **Step 4: Shatterburst RON (spell #4)**

Create `assets/common/abilities/spells/ruin/shatterburst.ron`:

```ron
GroundAoe(
    energy_cost: 120.0,
    buildup_duration: 1.2,
    delay: 1.0,
    recover_duration: 0.6,
    max_range: 30.0,
    radius: 6.0,
    min_falloff: 0.4,
    damage: 50,
    poise: 25,
    knockback: (strength: 15, direction: Away),
    dodgeable: Roll,
    reagent: Some(Red),
    rooted_cast: false,
    meta: (cooldown: Some(20.0), school: Some(Ruin)),
)
```

(Buildup 1.2 s > 1.0 s and no `POISE_RESISTANT` capability — interruptible per spec §8. Manifest registration happens in Task 11 under `Tool(Tome)`.)

- [ ] **Step 5: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common ground_aoe shatterburst` — Expected: PASS.
Run: `cargo check --workspace --all-targets` — Expected: clean.

```bash
git add common/src assets/common/abilities/spells
git commit -m "feat: GroundAoe character state and ability variant"
```

---

### Task 7: GroundAoe telegraph FX

**Files:**
- Modify: `voxygen/src/scene/particle.rs:1557` (`maintain_char_state_particles` — new arm next to `CharacterState::Blink` at line 1951)

- [ ] **Step 1: Telegraph particles**

Strike FX and SFX come free: the `ExplosionEvent` handler already emits `Outcome::Explosion` with our `reagent: Some(Red)` (this is why the state carries a reagent — documented decision: no new `Outcome` variant needed for v1). What's missing is the warning circle during `Action`. In `maintain_char_state_particles` add, next to the `CharacterState::Blink(c)` arm (line 1951):

```rust
                CharacterState::GroundAoe(c) => {
                    if matches!(c.stage_section, StageSection::Action)
                        && let Some(target) = c.target_pos
                    {
                        let radius = c.static_data.radius;
                        self.particles.resize_with(
                            self.particles.len()
                                + usize::from(
                                    self.scheduler.heartbeats(Duration::from_millis(5)),
                                ),
                            || {
                                let theta = rng.random::<f32>() * std::f32::consts::TAU;
                                let edge = target
                                    + Vec3::new(theta.cos(), theta.sin(), 0.0) * radius;
                                // TODO(magic-v1 polish): dedicated decal/ParticleMode;
                                // CultistFlame is a readable placeholder ring.
                                Particle::new_directed(
                                    Duration::from_secs_f32(0.4),
                                    time,
                                    ParticleMode::CultistFlame,
                                    edge,
                                    edge + Vec3::unit_z() * 1.5,
                                    scene_data,
                                )
                            },
                        );
                    }
                },
```

- [ ] **Step 2: Verify in game**

Run: `cargo check -p veloren-voxygen` — Expected: clean.
Use the `veloren-run` skill; give yourself a test binding via the Task 11 Tome (or temporarily set the Staff primary to the shatterburst id, reverting before commit). Confirm: aim point locks where the crosshair hit terrain, a particle ring marks the area for ~1 s, the explosion lands on the ring, and getting poise-stunned during the 1.2 s buildup cancels the cast. Milestone check (spec Phase 3): telegraph readable in 3rd person at 30 m.

- [ ] **Step 3: Commit**

```bash
git add voxygen/src/scene/particle.rs
git commit -m "feat: GroundAoe telegraph particle ring"
```

---

### Task 8: BuffKinds `Terrified`, `Charmed`, `Hollowtouched`

**Files:**
- Modify: `common/src/comp/buff.rs` — enum (after `ArdentHunted`, line 259), `differentiate` (284), `stacks` (374), `effects` (376)
- Modify: `voxygen/src/hud/util.rs:229` (i18n keys), `voxygen/src/hud/mod.rs:5632` (icons), compiler-driven remainder
- Modify: `assets/voxygen/i18n/en/buff.ftl` (before the `## Util` block)

- [ ] **Step 1: Enum + metadata**

In `common/src/comp/buff.rs`, after `ArdentHunted` (line 259):

```rust
    /// Dread of death. Heavy movement slow (players); NPCs additionally rout
    /// (server/agent). Strength scales the slow non-linearly like Crippled.
    /// v1 implements the spec's sanctioned fallback (slow, not forced
    /// movement) to avoid prediction artifacts; see magic spec §5 risk note.
    Terrified,
    /// Cannot bring itself to harm the charmer. No stat effects; consumed by
    /// agent targeting (NPCs only in v1, spec §5).
    Charmed,
    /// The Hollow's surcharge: stacking multiplicative max-health reduction
    /// applied by every Hollow-school cast via AbilityMeta.init_event.
    Hollowtouched,
```

`differentiate()` (line 284): add all three to the `SimpleNegative` arm (after `ArdentHunted`).
`stacks()` (line 374): `matches!(self, BuffKind::PotionSickness | BuffKind::Resilience | BuffKind::Hollowtouched)`.
`effects()` (line 376), next to the `Rooted` arm:

```rust
            BuffKind::Terrified => vec![BuffEffect::MovementSpeed(
                1.0 - nn_scaling(data.strength),
            )],
            BuffKind::Charmed => vec![],
            BuffKind::Hollowtouched => vec![BuffEffect::MaxHealthModifier {
                value: 1.0 - (0.08 * data.strength).min(0.4),
                kind: ModifierKind::Multiplicative,
            }],
```

- [ ] **Step 2: Compiler-driven match completion**

Run `cargo check --workspace --all-targets 2>&1 | grep -B3 "non-exhaustive"` and fill every arm. Known sites: `buff.rs` has further matches (duration default near line 757, etc. — give Terrified/Charmed the same treatment as `Rooted`, Hollowtouched the same as `Cursed`); `voxygen/src/hud/util.rs:229`:

```rust
        BuffKind::Terrified => "buff-terrified",
        BuffKind::Charmed => "buff-charmed",
        BuffKind::Hollowtouched => "buff-hollowtouched",
```

plus the strength-formatting match at `util.rs:369` (group with `Rooted`); `voxygen/src/hud/mod.rs:5632` icons — placeholders reusing existing art, with a polish TODO:

```rust
        // TODO(magic-v1 polish): dedicated icons
        BuffKind::Terrified => imgs.debuff_rooted_0,
        BuffKind::Charmed => imgs.debuff_amnesia_0,
        BuffKind::Hollowtouched => imgs.debuff_cursed_0,
```

(If `debuff_amnesia_0`/`debuff_cursed_0` are named differently in `voxygen/src/hud/img_ids.rs:959` area, use whatever the `Amnesia`/`Cursed` arms in this same match already use.)

- [ ] **Step 3: i18n**

In `assets/voxygen/i18n/en/buff.ftl`, before `## Util`:

```ftl
## Terrified
buff-terrified = Terrified
    .desc = Dread floods your limbs, slowing every step.
## Charmed
buff-charmed = Charmed
    .desc = You cannot bring yourself to harm whoever did this to you.
## Hollowtouched
buff-hollowtouched = Hollowtouched
    .desc = The Hollow has tasted you. Maximum health reduced.
```

- [ ] **Step 4: Verify and commit**

Run: `cargo check --workspace --all-targets` then `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-voxygen` — Expected: clean / PASS (i18n tests validate the ftl syntax).

```bash
git add common/src/comp/buff.rs voxygen/src assets/voxygen/i18n
git commit -m "feat: Terrified, Charmed, Hollowtouched buff kinds"
```

---

### Task 9: Agent reactions + spells #15 Dread Whisper, #8 Censure

**Files:**
- Modify: `server/agent/src/action_nodes.rs` (helpers near `below_flee_health`, line 2226; flee hook in the hostile path that already calls `self.flee` at line 2097)
- Create: `assets/common/abilities/spells/hollow/dread_whisper.ron`, `assets/common/abilities/spells/gravesong/censure.ron`

- [ ] **Step 1: Agent helpers (hot-reloadable dev loop — `server-agent` is a dylib)**

In `server/agent/src/action_nodes.rs`, next to `below_flee_health` (line 2226):

```rust
    /// Magic spec §5: Terrified entities rout instead of fighting.
    pub fn is_terrified(&self, read_data: &ReadData) -> bool {
        read_data
            .buffs
            .get(*self.entity)
            .is_some_and(|buffs| buffs.contains(BuffKind::Terrified))
    }

    /// Magic spec §5: Charmed entities will not attack their charmer.
    pub fn is_charmed_by(&self, target_uid: Uid, read_data: &ReadData) -> bool {
        read_data.buffs.get(*self.entity).is_some_and(|buffs| {
            buffs.iter_kind(BuffKind::Charmed).any(|buff| {
                matches!(buff.source, BuffSource::Character { by, .. } if by == target_uid)
            })
        })
    }
```

(Imports: `BuffKind`/`BuffSource` follow the existing buff usage at `server/agent/src/attack.rs:810` and `:2868` — copy that module path. If `iter_kind` yields a different item shape, adapt to exactly what `attack.rs:2868` destructures; the compiler arbitrates.)

- [ ] **Step 2: Wire into the hostile decision path**

Find the hostile-target pursuit entry: `grep -n "fn handle_attack\|fn hostile" server/agent/src/action_nodes.rs server/agent/src/lib.rs`. At the top of the function that decides to engage a `target` (the one that, lower down, calls `self.flee(agent, controller, read_data, &sound_pos)` at line ~2097 for threatening sounds), add before engaging:

```rust
        if self.is_terrified(read_data) {
            self.flee(agent, controller, read_data, &tgt_pos);
            return;
        }
        if self.is_charmed_by(target_uid, read_data) {
            // Stand down: charmed entities refuse to engage the charmer.
            agent.target = None;
            return;
        }
```

(`tgt_pos`/`target_uid` use whatever bindings the surrounding function already has for the target's `Pos`/`Uid` — match the locals used by the existing `flee` call. Boss immunity to Charmed is deferred to the spec's CC-resist tuning.)

- [ ] **Step 3: The two CC spells**

`assets/common/abilities/spells/hollow/dread_whisper.ron` (spell #15 — Hollow surcharge via `init_event`, spec §8):

```ron
BasicRanged(
    energy_cost: 90.0,
    buildup_duration: 0.8,
    recover_duration: 0.5,
    projectile: (
        kind: Explosive(radius: 2.0, min_falloff: 0.5, reagent: Some(Purple), terrain: None),
        attack: Some((
            damage: 18,
            buff: Some((kind: Terrified, dur_secs: 4, strength: Value(0.8), chance: 1.0)),
        )),
    ),
    projectile_body: Object(BoltFire),
    projectile_speed: 60.0,
    num_projectiles: Value(1),
    meta: (
        init_event: Some(GainBuff(kind: Hollowtouched, strength: 1.0, duration: Some(60))),
        cooldown: Some(25.0),
        school: Some(Hollow),
    ),
)
```

Before writing, confirm the `init_event` RON syntax against an existing user: `grep -rn "init_event" assets/common/abilities | head -3` and mirror it exactly (the enum is `AbilityInitEvent::GainBuff { kind, strength, duration }`).

`assets/common/abilities/spells/gravesong/censure.ron` (spell #8 — silence via the existing `Amnesia` buff, content-only):

```ron
BasicRanged(
    energy_cost: 70.0,
    buildup_duration: 0.6,
    recover_duration: 0.4,
    projectile: (
        kind: Explosive(radius: 1.5, min_falloff: 0.6, reagent: Some(Purple), terrain: None),
        attack: Some((
            damage: 14,
            buff: Some((kind: Amnesia, dur_secs: 5, strength: Value(1.0), chance: 1.0)),
        )),
    ),
    projectile_body: Object(BoltFire),
    projectile_speed: 70.0,
    num_projectiles: Value(1),
    meta: (cooldown: Some(18.0), school: Some(Gravesong)),
)
```

- [ ] **Step 4: Verify and commit**

Run: `cargo check --workspace --all-targets` — Expected: clean.
In game (after Task 11 binds these to the Focus/HolySymbol): Dread Whisper visibly routs a gnarling pack (spec Phase 3 milestone) and applies Hollowtouched to the caster; Censure blocks a staff NPC's auxiliary casts.

```bash
git add server/agent/src assets/common/abilities/spells
git commit -m "feat: agent fear/charm reactions plus Dread Whisper and Censure"
```

---

## Phase 3 — Caster implements (no dependencies)

### Task 10: `ToolKind::{Tome, HolySymbol, Focus}`

**Files:**
- Modify: `common/src/comp/inventory/item/tool.rs:26` (enum), `:57` (`identifier_name`), `:107` (`block_priority`)
- Modify: `server/src/persistence/json_models.rs:214` (`tool_kind_to_string`), `:241` (`tool_kind_from_string`)
- Modify (compiler-driven): every other exhaustive `ToolKind` match the workspace reports

- [ ] **Step 1: Central arms**

In the `ToolKind` enum (line 26), after `Sceptre` in the weapons block:

```rust
    // caster implements (magic-abilities spec §3 Path A)
    Tome,
    HolySymbol,
    Focus,
```

`identifier_name` (line 57): `ToolKind::Tome => "tome", ToolKind::HolySymbol => "holy_symbol", ToolKind::Focus => "focus",`.
`gains_combat_xp` (line 80): **no change** — the `matches!` list omits the new kinds, so they return `false`. This is deliberate for v1: it keeps `SkillGroupKind::Weapon(Tome)` out of `skill_group_to_db_string` (`json_models.rs:71`), avoiding a DB enum migration; caster progression comes from class trees (classes-races spec).
`can_block` (line 96): no change (returns `false`).
`block_priority` (line 107): `ToolKind::Tome => 3, ToolKind::HolySymbol => 4, ToolKind::Focus => 3,` (same tier as Staff/Sceptre — casters fall back to weapon guard last; duplicate values are fine, the match returns plain `i32`).

- [ ] **Step 2: Persistence strings (required — `AuxiliaryKey` serializes equipped tool kinds)**

`tool_kind_to_string` (line 214): `Some(Tome) => "Tome", Some(HolySymbol) => "HolySymbol", Some(Focus) => "Focus",` after `Some(Sceptre)`. `tool_kind_from_string` (line 241): the three mirror arms after `"Sceptre"`.

- [ ] **Step 3: Compiler-driven sweep**

Run `cargo check --workspace --all-targets 2>&1 | grep -B5 "non-exhaustive"` repeatedly until clean. Expect hits in voxygen animation/hud and item code: in each, add the three kinds to whichever arm `Staff`/`Sceptre` already share **at that site** (e.g. wield poses: group with `Staff`). No wildcard arms.

- [ ] **Step 4: Verify and commit**

Run: `cargo check --workspace --all-targets` and `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` — Expected: clean / PASS.

```bash
git add common/src voxygen server/src
git commit -m "feat: Tome, HolySymbol, and Focus caster tool kinds"
```

---

### Task 11: Starter implements + manifest ability sets

**Files:**
- Create: `assets/common/items/weapons/tome/apprentice_tome.ron`, `assets/common/items/weapons/holy_symbol/initiate_symbol.ron`, `assets/common/items/weapons/focus/wanderer_focus.ron`
- Modify: `assets/voxygen/voxel/biped_weapon_manifest.ron:1424` area, `assets/voxygen/item_image_manifest.ron:1358` area, `assets/common/abilities/ability_set_manifest.ron:305` area (after `Tool(Sceptre)`)

- [ ] **Step 1: Item definitions (modeled on the verified `starter_staff.ron`)**

`assets/common/items/weapons/tome/apprentice_tome.ron`:

```ron
ItemDef(
    legacy_name: "Apprentice Tome",
    legacy_description: "Margins crowded with a студент's frantic leyline notes.",
    kind: Tool((
        kind: Tome,
        hands: Two,
        stats: (
            equip_time_secs: 0.4,
            power: 0.55,
            effect_power: 1.0,
            speed: 0.75,
            range: 1.0,
            energy_efficiency: 0.75,
            buff_strength: 0.75,
        ),
    )),
    quality: Low,
    tags: [],
    ability_spec: None,
)
```

(Fix the description typo when writing — keep it original-IP per `veloren-lore`.) The other two are identical apart from `legacy_name`/`legacy_description` and `kind: HolySymbol` / `kind: Focus`.

- [ ] **Step 2: Voxel + image manifests (placeholder art, polish TODO)**

In `assets/voxygen/voxel/biped_weapon_manifest.ron`, after the `starter_staff` entry (line 1424), one per item:

```ron
    // TODO(magic-v1 polish): dedicated implement models
    Tool("common.items.weapons.tome.apprentice_tome"): (
        vox_spec: ("weapon.staff.firestaff_starter", (-2.5, -3.0, -3.0)),
        color: None
    ),
```

In `assets/voxygen/item_image_manifest.ron`, after the `starter_staff` entry (line 1358), one per item:

```ron
    Simple("common.items.weapons.tome.apprentice_tome"): VoxTrans(
        "voxel.weapon.staff.firestaff_starter",
        (1.0, 0.0, 0.0), (-130., 90.0, 0.0), 1.2,
    ),
```

- [ ] **Step 3: Manifest ability sets**

In `assets/common/abilities/ability_set_manifest.ron`, after the `Tool(Sceptre)` entry (line ~305) — skill gates are `None` until Task 15:

```ron
    Tool(Tome): AbilitySet((
        primary: Simple(None, "common.abilities.spells.ruin.emberlance"),
        secondary: Simple(None, "common.abilities.spells.wardcraft.wardshell"),
        abilities: [
            Simple(None, "common.abilities.spells.threshold.veilrend"),
            Simple(None, "common.abilities.spells.ruin.shatterburst"),
        ],
    )),
    Tool(HolySymbol): AbilitySet((
        primary: Simple(None, "common.abilities.spells.dawnfire.radiant_verdict"),
        secondary: Simple(None, "common.abilities.spells.dawnfire.dawnlight_mend"),
        abilities: [
            Simple(None, "common.abilities.spells.dawnfire.aegis_of_dawn"),
            Simple(None, "common.abilities.spells.gravesong.censure"),
        ],
    )),
    Tool(Focus): AbilitySet((
        primary: Simple(None, "common.abilities.spells.verdance.thornlash"),
        secondary: Simple(None, "common.abilities.spells.verdance.verdant_mending"),
        abilities: [
            Simple(None, "common.abilities.spells.verdance.galeburst"),
            Simple(None, "common.abilities.spells.pactbinding.umbral_bolt"),
            Simple(None, "common.abilities.spells.pactbinding.soul_siphon"),
            Simple(None, "common.abilities.spells.hollow.dread_whisper"),
            Simple(None, "common.abilities.spells.hollow.hollow_gate"),
        ],
    )),
```

Note: until Tasks 12–13 create these RONs, missing ids fall back to `CharacterAbility::default()` with a load warning (`tool.rs:719-723`) — the manifest still parses. The Task 13 test closes the gap.

- [ ] **Step 4: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` — Expected: PASS (item asset tests load the three new items).
In game: `/give_item common.items.weapons.tome.apprentice_tome`, equip — Shatterburst and the placeholder primaries appear in the Diary; Shatterburst cooldown greys re-cast for 20 s (server-side; HUD radial sweep is a polish follow-up).

```bash
git add assets/common/items assets/voxygen assets/common/abilities/ability_set_manifest.ron
git commit -m "feat: starter caster implements with manifest ability sets"
```

---

## Phase 4 — Content: spells, innates, gating

### Task 12: Template spell — Emberlance (#1), worked end-to-end

**Files:**
- Create: `assets/common/abilities/spells/ruin/emberlance.ron`
- Modify: `assets/voxygen/i18n/en/hud/ability.ftl`, `voxygen/src/hud/util.rs:687` (icon), `common/src/comp/ability.rs` (test module from Task 6)

- [ ] **Step 1: The RON (modeled on the verified `staff/firebomb.ron`)**

```ron
BasicRanged(
    energy_cost: 0,
    buildup_duration: 0.5,
    recover_duration: 0.35,
    projectile: (
        kind: Explosive(
            radius: 1.5,
            min_falloff: 0.5,
            reagent: Some(Red),
            terrain: None
        ),
        attack: Some((
            damage: 16,
            energy: Some(6),
            buff: Some((
                kind: Burning,
                dur_secs: 4,
                strength: DamageFraction(0.1),
                chance: 0.2,
            )),
        )),
    ),
    projectile_body: Object(BoltFire),
    projectile_speed: 65.0,
    num_projectiles: Value(1),
    movement_modifier: (
        buildup: Some(0.3),
        recover: Some(0.3),
    ),
    meta: (school: Some(Ruin)),
)
```

(Cantrip tier: zero energy cost, no cooldown — spec §8 baseline.)

- [ ] **Step 2: Frontend registration**

i18n, in `assets/voxygen/i18n/en/hud/ability.ftl` (key = ability id with dots→dashes, format verified at line 21):

```ftl
common-abilities-spells-ruin-emberlance = Emberlance
    .desc = Hurls a lance of leyline fire that bursts on impact.
```

Icon, in the ability-id match at `voxygen/src/hud/util.rs:687`:

```rust
        // TODO(magic-v1 polish): dedicated spell icons
        "common.abilities.spells.ruin.emberlance" => imgs.fire_aoe,
```

- [ ] **Step 3: Asset-load test**

Add to the `ground_aoe_tests`-style module in `common/src/comp/ability.rs` (rename the module `spell_asset_tests` and keep the shatterburst case):

```rust
    #[test]
    fn emberlance_deserializes_as_basic_ranged() {
        let ability = crate::assets::Ron::<CharacterAbility>::load_expect(
            "common.abilities.spells.ruin.emberlance",
        )
        .read()
        .0
        .clone();
        assert!(matches!(ability, CharacterAbility::BasicRanged { .. }));
    }
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common spell_asset` — Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add assets common/src voxygen/src
git commit -m "feat: Emberlance, the worked template spell"
```

---

### Task 13: Remaining 10 content-only spells (table-driven)

**Files:**
- Create: 10 RONs under `assets/common/abilities/spells/` (paths below)
- Modify: `assets/voxygen/i18n/en/hud/ability.ftl`, `voxygen/src/hud/util.rs:687`, the test list in `common/src/comp/ability.rs`

Each spell follows an already-verified donor RON exactly, changing only the listed numbers, buff kinds, reagents, and `meta`. Donors in tree: `staff/firebomb.ron` (BasicRanged — full text in Task 12), `sword/heavy_fortitude.ron` (SelfBuff), `custom/cursekeeper/teleport.ron` (Blink), `sceptre/healingaura.ron` (BasicAura), `staff/fireshockwave.ron` (Shockwave), `sceptre/lifestealbeam.ron` (BasicBeam). For `StaticAura` (#7, #10) start from `sceptre/wardingaura.ron`. For `BasicSummon` (#16) start from any `BasicSummon(` hit under `assets/common/abilities/custom/` (`grep -rln "BasicSummon(" assets/common/abilities/custom | head -3`) and point `summon_info` at an existing low-tier entity config, with a TODO for the dedicated servitor entity (spec Phase 2 item).

| # | Asset path (`assets/common/abilities/spells/…`) | Donor / variant | Key parameters (energy / buildup / cooldown) | Meta |
|---|---|---|---|---|
| 2 | `wardcraft/wardshell.ron` | SelfBuff (`heavy_fortitude`) | 60 / 0.4 / 30 s; `BuffDesc(kind: ProtectingWard, data: BuffData(strength: 0.4, duration: Some(10.0)))`; no stance requirement | `(cooldown: Some(30.0), school: Some(Wardcraft))` |
| 3 | `threshold/veilrend.ron` | Blink (`cursekeeper/teleport`) | 80 / 0.5 / 15 s; `max_range: 24.0`, `frontend_specifier: Some(CultistFlame)`; add `energy_cost: 80.0` if the donor lacks it (check `CharacterAbility::Blink` fields in ability.rs first) | `(cooldown: Some(15.0), school: Some(Threshold))` |
| 5 | `dawnfire/dawnlight_mend.ron` | BasicAura (`healingaura` near-clone) | 20 / 0.2 / none; `kind: Regeneration, strength: 0.4`, `range: 25.0`, `targets: InGroup` | `(school: Some(Dawnfire))` |
| 6 | `dawnfire/radiant_verdict.ron` | BasicRanged (emberlance clone) | 0 / 0.5 / none; damage 14, `reagent: Some(Yellow)`, smite buff `(kind: Burning, dur_secs: 3, strength: DamageFraction(0.15), chance: 1.0)` | `(school: Some(Dawnfire))` |
| 7 | `dawnfire/aegis_of_dawn.ron` | StaticAura (`wardingaura`) | 90 / 0.6 / 45 s; `kind: ProtectingWard`, party-targeted as donor | `(cooldown: Some(45.0), school: Some(Dawnfire))` |
| 9 | `verdance/thornlash.ron` | BasicBeam (`lifestealbeam`) | drain 12 / 0.2 / none; damage 5, `range: 20.0`, `damage_effect: Some(Buff((kind: Crippled, dur_secs: 2, strength: Value(0.3), chance: 1.0)))` — verify the beam-buff syntax with `grep -rn "damage_effect: Some(Buff" assets/common/abilities | head -2` and copy it | `(school: Some(Verdance))` |
| 10 | `verdance/verdant_mending.ron` | StaticAura (`wardingaura` shape, Regeneration payload) | 70 / 0.5 / 25 s; `kind: Regeneration, strength: 0.7` | `(cooldown: Some(25.0), school: Some(Verdance))` |
| 11 | `verdance/galeburst.ron` | Shockwave (`fireshockwave`) | 60 / 0.4 / 12 s; damage 8, `knockback: (strength: 40, direction: Away)`, `damage_kind: Energy`, `specifier: Fire` (TODO dedicated specifier) | `(cooldown: Some(12.0), school: Some(Verdance))` |
| 13 | `pactbinding/umbral_bolt.ron` | BasicRanged (emberlance clone) | 0 / 0.5 / none; damage 12, `reagent: Some(Purple)`, buff `(kind: Cursed, dur_secs: 4, strength: Value(0.5), chance: 1.0)` | `(school: Some(Pactbinding))` |
| 14 | `pactbinding/soul_siphon.ron` | BasicBeam (`lifestealbeam` near-clone) | as donor / 0.2 / none; `damage_effect: Some(Lifesteal(0.08))`, damage 5 | `(school: Some(Pactbinding))` |
| 16 | `hollow/hollow_gate.ron` | BasicSummon (custom donor) | 250 / 1.5 / 90 s; 1 summon, 30 s lifetime; **plus** `init_event: Some(GainBuff(kind: Hollowtouched, strength: 1.0, duration: Some(60)))` | `(init_event: …, cooldown: Some(90.0), school: Some(Hollow))` |

- [ ] **Step 1:** Write the 10 RONs per the table, copying each donor's full field list verbatim before editing values. All buildups ≥ 1.0 s must not set `POISE_RESISTANT` (spec §8) — none of these do.
- [ ] **Step 2:** Extend `spell_asset_tests` with a loop test:

```rust
    #[test]
    fn all_v1_spell_rons_deserialize_to_expected_variants() {
        for (id, is_default_forbidden) in [
            ("common.abilities.spells.wardcraft.wardshell", true),
            ("common.abilities.spells.threshold.veilrend", true),
            ("common.abilities.spells.dawnfire.dawnlight_mend", true),
            ("common.abilities.spells.dawnfire.radiant_verdict", true),
            ("common.abilities.spells.dawnfire.aegis_of_dawn", true),
            ("common.abilities.spells.gravesong.censure", true),
            ("common.abilities.spells.verdance.thornlash", true),
            ("common.abilities.spells.verdance.verdant_mending", true),
            ("common.abilities.spells.verdance.galeburst", true),
            ("common.abilities.spells.pactbinding.umbral_bolt", true),
            ("common.abilities.spells.pactbinding.soul_siphon", true),
            ("common.abilities.spells.hollow.dread_whisper", true),
            ("common.abilities.spells.hollow.hollow_gate", true),
        ] {
            // load_expect panics (with the id in the message) on malformed RON,
            // which is the failure mode tool.rs:719 would otherwise swallow.
            let _ = crate::assets::Ron::<CharacterAbility>::load_expect(id);
            assert!(is_default_forbidden);
        }
    }
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common spell_asset` — Expected: PASS only when every file parses.
- [ ] **Step 3:** Add the 10 i18n entries (`common-abilities-spells-…` keys, Task 12 format) and 10 icon arms at `util.rs:687` (placeholders: `imgs.heal_aoe` for heals/auras, `imgs.fire_aoe` for nukes, or whatever the neighboring staff/sceptre arms use).
- [ ] **Step 4:** In-game smoke test per implement (veloren-run): each archetype playable with its spells from Task 11's sets — the spec Phase-2 milestone.
- [ ] **Step 5: Commit**

```bash
git add assets common/src voxygen/src
git commit -m "feat: v1 content-wave spells for all four caster archetypes"
```

---

### Task 14: Racial innates — 6 RONs + creation grant

**Files:**
- Create: `assets/common/abilities/innate/{human,elf,dwarf,orc,danari,draugr}.ron`
- Modify: `assets/common/abilities/ability_set_manifest.ron` (6 `Custom` sets), `server/src/state_ext.rs` (`initialize_character_data`, replacing Task 3's default insert), i18n + icons as in Task 13

- [ ] **Step 1: Worked innate — Human "Second Wind"** (`innate/human.ron`, SelfBuff donor `heavy_fortitude`):

```ron
SelfBuff(
    buildup_duration: 0.3,
    cast_duration: 0.2,
    recover_duration: 0.4,
    buffs: [
        BuffDesc(kind: EnergyRegen, data: BuffData(strength: 2.0, duration: Some(8.0))),
        BuffDesc(kind: Regeneration, data: BuffData(strength: 1.5, duration: Some(8.0))),
    ],
    energy_cost: 0,
    meta: (cooldown: Some(120.0)),
)
```

Table for the other five (same donor unless noted; all `energy_cost: 0`, racial identity comes from the long cooldown):

| Species | File | Variant / donor | Payload | Cooldown |
|---|---|---|---|---|
| Elf | `elf.ron` | SelfBuff | `Hastened` 0.3 + `Agility` 0.3, 6 s | 90 s |
| Dwarf | `dwarf.ron` | SelfBuff | `Fortitude` 1.0, 10 s | 90 s |
| Orc | `orc.ron` | SelfBuff | `Reckless` 0.5, 8 s | 90 s |
| Danari | `danari.ron` | Blink (`cursekeeper/teleport` donor) | `max_range: 8.0` (short hop vs Veilrend's 24) | 45 s |
| Draugr | `draugr.ron` | Shockwave (`fireshockwave` donor) | damage 6, `Chilled`-applying via `damage_effect: Some(Buff(...))` (same grep-verified syntax as Task 13 #9), `specifier: Ice` if it exists else `Fire` + TODO | 60 s |

- [ ] **Step 2: Manifest sets** — for each species, after the Task 11 entries:

```ron
    Custom("innate.human"): AbilitySet((
        primary: Simple(None, "common.abilities.innate.human"),
        secondary: Simple(None, "common.abilities.innate.human"),
        abilities: [],
    )),
```

(The set key doubles as the frontend ability id — Task 3 design — so i18n keys are `common-abilities-innate-human` etc. and icon arms match on `"innate.human"`.)

- [ ] **Step 3: Grant at character init**

In `server/src/state_ext.rs` `initialize_character_data`, replace Task 3's `AbilityPool::default()` insert:

```rust
        let species_innate = match body {
            comp::Body::Humanoid(humanoid) => {
                use comp::humanoid::Species;
                Some(match humanoid.species {
                    Species::Human => "innate.human",
                    Species::Elf => "innate.elf",
                    Species::Dwarf => "innate.dwarf",
                    Species::Orc => "innate.orc",
                    Species::Danari => "innate.danari",
                    Species::Draugr => "innate.draugr",
                })
            },
            _ => None,
        };
        self.write_component_ignore_entity_dead(entity, comp::AbilityPool {
            abilities: species_innate
                .into_iter()
                .map(String::from)
                .collect(),
        });
```

(`body` is among the persisted components destructured at the top of that function — line ~704 region; adjust the binding name to the local. The species list is verified against `common/src/comp/body/humanoid.rs:116`.)

- [ ] **Step 4: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` — Expected: PASS.
In game: create a Danari, open Diary → the innate appears (Task 3's `all_available_abilities`); bind it to a hotbar slot, blink 8 m bare-handed, relog → binding survives (Task 4 persistence). That is the full Path-B pipeline proven.

```bash
git add assets server/src
git commit -m "feat: racial innate abilities granted through AbilityPool"
```

---

### Task 15: Class skill gating (**Depends on: classes-races plan Task 1**)

**Files:**
- Modify: `assets/common/abilities/ability_set_manifest.ron` (Task 11 entries)

- [ ] **Step 1: Gate check — do not proceed if it fails**

Run: `grep -n "Class" common/src/comp/skillset/skills.rs`
Expected if the dependency landed: a `Skill::Class(...)` (or equivalent ClassKind-gated) variant. **If the grep finds nothing, STOP this task** — leave the manifest gates as `None` (spells remain implement-gated only, which is the shipped v1 fallback) and note the deferral in the final summary. Everything else in this plan is unaffected.

- [ ] **Step 2: Flip the gates**

For each Task 11 `abilities:` entry, replace `Simple(None, …)` with the class-tree skill the classes-races plan defines, e.g. `Simple(Some(Class(Arcanist(Shatterburst))), "common.abilities.spells.ruin.shatterburst"),` — exact variant path per that plan's skill enum. Primaries/secondaries stay ungated (cantrip feel, spec §3).

- [ ] **Step 3: Verify and commit**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` — Expected: PASS.
In game: without the skill the ability no longer lists in the Diary (`AbilityKind::ability()` skill check, tool.rs:367-396); after `/skill_point` grants, it does.

```bash
git add assets/common/abilities/ability_set_manifest.ron
git commit -m "feat: class-tree skill gates on caster implement spells"
```

---

### Task 16: Lint, format, changelog, branch finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. The new enum variants make every missed match fail here — fix each properly, no `#[allow]` without a justifying comment.

- [ ] **Step 2: Voxygen publish-profile clippy + format**

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
cargo fmt --all -- --check
```
Expected: both clean (run `cargo fmt --all` if the check fails, then re-check).

- [ ] **Step 3: Full test pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server -p veloren-voxygen`
Expected: PASS, including `ability_cooldown` (3), `innate_aux` (1), `spell_asset` (3) suites.

- [ ] **Step 4: Changelog**

Add under the unreleased section of `CHANGELOG.md`:

```markdown
- Magic v1: per-spell cooldowns, ground-targeted AoE casting, Terrified/Charmed/Hollowtouched debuffs, Tome/Holy Symbol/Focus caster implements with 15 spells, and one innate racial ability per species.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for magic abilities v1"
```

- [ ] **Step 5: Balance pass + finish the branch**

Run a live `asset_tweak` tuning session against the spec §8 baselines (energy 0/50–150/250+, cooldown matrix) with the `veloren-telemetry` skill recording per-spell casts. Then invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). Deferred follow-ups tracked in the design spec: Wildshape #12 (Phase 4), dedicated icons/voxel models/particle modes (TODO markers), hotbar radial sweep, cooldown persistence (Open Q #3), boss Charm immunity.
