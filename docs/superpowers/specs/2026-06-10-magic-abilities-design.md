# Magic System & Class/Race Abilities: Design

**Date:** 2026-06-10
**Companion specs:** `2026-06-10-classes-races-design.md` (ClassKind, class skill trees), `2026-06-10-character-levels-design.md` (character Level), `2026-06-10-lore-cosmology-design.md` (pantheon, school lore alignment)

## Context

Veloren already ships a data-driven ability framework: ~33 `CharacterAbility` variants deserialized from RON files under `assets/common/abilities/`, mapped to weapons via `assets/common/abilities/ability_set_manifest.ron`, gated by skills, and executed as character states. Magic today is narrow: Staff (fire evocation in all but name) and Sceptre (heal/lifesteal). Our fork is adding D&D-inspired classes, races, and character levels (see companion specs). This spec designs the magic layer on top: a spell-school taxonomy, class spell lists, racial innate abilities, and the casting economy — maximizing reuse of the existing ability machinery and identifying exactly where new machinery is required.

## Goals

| Goal | Measure |
|---|---|
| D&D-flavored breadth: arcane schools, divine domains, primal and pact magic | 8 schools + 1 forbidden school defined, mapped to lore cosmology |
| Spells as data, not code | Every v1 spell is a RON asset; new Rust only for genuinely new mechanics |
| Class casters feel distinct | 4 caster archetypes with 4 spells each at launch (16 spells) |
| Racial identity | 1 innate ability per playable species (6 total) |
| Casting has cost and counterplay | Energy costs, buildup interruption via poise, per-spell cooldowns |

## Non-Goals (v1)

- **Full Vancian spell-slot economy** — we keep Energy-based casting with cooldowns (analysis below).
- Spell components/reagents, counterspelling, ritual casting, concentration checks.
- Resurrection of dead players (genuinely new server machinery; deferred, see Open Questions).
- NPC casters using the new schools (rtsim/agent integration is a follow-up).

### Spell Slots vs Energy: Analysis

| Criterion | Vancian slots | Energy + cooldowns (chosen) |
|---|---|---|
| Engine fit | No per-day resource exists; would need new component, persistence, rest mechanic | `Energy` component exists (`common/src/comp/energy.rs`, fixed-point u32, accelerating `regen_rate`); every ability already declares `energy_cost` |
| Pacing | Suits session-based tabletop, not a real-time MMO with no "long rest" concept | Matches Veloren's continuous combat loop; cooldowns add tactical rhythm without downtime |
| Balance surface | Slot counts per level — coarse | `energy_cost` + `cooldown` + buildup/recover durations per spell — fine-grained, tweakable via RON |
| Implementation | XL (new resource, UI, rest system, persistence) | S for costs (exists), M for cooldowns (new, designed below) |

**Decision:** Energy is the mana pool; big spells cost more energy and carry cooldowns. The D&D "you can only do this once in a while" feel comes from cooldowns, not slots.

## Current State (verified inventory)

| Machinery | Location | Notes |
|---|---|---|
| `CharacterAbility` enum | `common/src/comp/ability.rs` (3737 lines) | 33 variants: `BasicMelee`, `BasicRanged`, `RapidRanged`, `Boost`, `GlideBoost`, `DashMelee`, `BasicBlock`, `Roll`, `ComboMelee2`, `LeapExplosionShockwave`, `LeapMelee`, `LeapShockwave`, `ChargedMelee`, `ChargedRanged`, `Throw`, `Shockwave`, `Explosion`, `BasicBeam`, `BasicAura`, `StaticAura`, `Blink`, `BasicSummon`, `SelfBuff`, `SpriteSummon`, `Music`, `FinisherMelee`, `DiveMelee`, `RiposteMelee`, `RapidMelee`, `Transform`, `RegrowHead`, `LeapRanged`, `Simple` |
| Character states | `common/src/states/` | One module per variant (`shockwave.rs`, `blink.rs`, `basic_summon.rs`, `transform.rs`, `static_aura.rs`, …); `CharacterBehavior` trait in `states/behavior.rs`; dispatch match in `common/src/comp/character_state.rs` (`behavior()` ~line 783, `handle_event()` ~line 848) |
| Ability assets | `assets/common/abilities/**` | RON per ability; `ability_set_manifest.ron` maps `AbilitySpec::Tool(ToolKind)` or `AbilitySpec::Custom(String)` to `AbilitySet`s; `AbilitySetOverride` supports inheritance |
| `AbilitySet` / `AbilityKind` / `AbilityMap` | `common/src/comp/inventory/item/tool.rs` (lines 312–677) | `AbilitySet { guard, primary, secondary, abilities }`; `AbilityKind::Simple(Option<Skill>, T)` gates on skill; `Contextualized` switches on stance/dual-wield/combo |
| `ToolKind` | `common/src/comp/inventory/item/tool.rs:26` | Plain Rust enum (Sword … Empty), extensible; exhaustive matches in `identifier_name`, `gains_combat_xp`, `can_block`, `block_priority` must be updated when adding kinds |
| `ActiveAbilities` component | `common/src/comp/ability.rs:54` | Slots: `guard`, `primary`, `secondary`, `movement` + `auxiliary_sets: HashMap<AuxiliaryKey, Vec<AuxiliaryAbility>>`; `AuxiliaryKey = (Option<ToolKind>, Option<ToolKind>)` (line 47); `BASE_ABILITY_LIMIT = 5` (line 43) |
| `AuxiliaryAbility` | `common/src/comp/ability.rs:645` | `MainWeapon(usize)`, `OffWeapon(usize)`, `Glider(usize)`, `Empty` — **all auxiliary abilities currently come from equipped items**; `Ability` enum (line 386) has a commented-out `ArmorAbility(usize)` placeholder |
| Hotbar binding | `voxygen/src/hud/diary.rs` (ability selection), `voxygen/src/hud/hotbar.rs`, `voxygen/src/hud/mod.rs:739` `Event::ChangeAbility(usize, AuxiliaryAbility)` | Player drags unlocked abilities into aux slots in the Diary UI |
| Persistence | `server/src/persistence/models.rs:57` (`ability_sets: String`), `server/src/persistence/json_models.rs:126–136` | `AuxiliaryAbility` serialized as strings (`"Main Weapon:index:N"`) |
| Skills | `common/src/comp/skillset/skills.rs:13` (`Skill` enum: Sword/Axe/Hammer/Bow/Staff/Sceptre/Climb/Swim/Pick/UnlockGroup), `skillset/mod.rs:89` (`SkillGroupKind::{General, Weapon(ToolKind)}`) | Skill presets: `assets/common/skillset/preset/`, built by `common/src/skillset_builder.rs` |
| Buffs | `common/src/comp/buff.rs` | ~49 `BuffKind`s. Relevant: `Amnesia` ("prevents use of auxiliary abilities" — a Silence), `Crippled`/`Chilled` (slows), `Frozen`, `Rooted`, `Ensnared`, `Cursed`, `ProtectingWard`, `Invulnerability`. **No Charmed or Feared.** |
| Energy | `common/src/comp/energy.rs` | Stamina/mana hybrid; every `CharacterAbility` has `energy_cost` |
| Cooldowns | — | **Do not exist.** Abilities are limited only by energy, combo cost (`combo_cost()`, ability.rs:2259), and buildup/recover durations. `charge_duration` fields are charge-up mechanics, not cooldowns |
| `AbilityMeta` | `common/src/comp/ability.rs:3527` | `capabilities: Capability` (bitflags line 3647: PARRIES, BLOCKS, POISE_RESISTANT…), `init_event: Option<AbilityInitEvent>` (EnterStance / GainBuff), `requirements: AbilityRequirements` (stance, item) |
| Interruption | `common/src/comp/poise.rs:63` `PoiseState::{Normal, Interrupted, Stunned, Dazed, KnockedDown}` | Poise damage during buildup interrupts casts unless the ability sets `POISE_RESISTANT` |
| Projectiles with status | `common/src/comp/projectile.rs:141` | `ProjectileConstructor` carries `buff: Option<CombatBuff>` — projectile-applied debuffs already work |
| FX pipeline | `voxygen/src/scene/particle.rs` | Two paths: per-tick component-driven (`maintain_shockwave_particles`, line 3604, matches `shockwave.properties.specifier` `FrontendSpecifier` → `ParticleMode`) and one-shot `Outcome` events (`common/src/outcome.rs`, e.g. `FireShockwave` line 167, handled at particle.rs ~line 437). SFX keyed similarly in `voxygen/src/audio/sfx/` |
| Species | `common/src/comp/body/humanoid.rs:116` | Danari, Dwarf, Elf, Human, Orc, Draugr |
| Character creation | `server/src/character_creator.rs:61` | Currently `SkillSet::default()` — hook point for racial grants |
| Balance tweaking | `common/assets/Cargo.toml:29` | `asset_tweak = ["dep:serde", "hot-reloading"]` feature exists |

## Design

### 1. Spell School Taxonomy

Original names (no D&D/WotC terms in code or assets). Final lore names are owned by `2026-06-10-lore-cosmology-design.md`; the working names below are binding defaults until that spec overrides them. Code identifier: `SpellSchool` enum in `common/src/comp/ability.rs`, carried in `AbilityMeta` as `school: Option<SpellSchool>` for UI grouping, resistances, and class gating.

| School | Tradition | D&D analogue | Lore alignment (cosmology spec) | Signature effects |
|---|---|---|---|---|
| **Ruin** | Arcane | Evocation | Raw leyline force | Damage nukes, AoE bursts, beams |
| **Wardcraft** | Arcane | Abjuration | Order/structure principle | Shields, wards, dispels, parry buffs |
| **Threshold** | Arcane | Conjuration | Liminal space between planes | Blink, summons, conjured terrain |
| **Flux** | Arcane | Transmutation | Change principle | Haste/slow, polymorph, gravity tricks |
| **Dawnfire** | Divine domain | Life/Light | Sun deity of the pantheon | Heals, radiant smites, protective auras |
| **Gravesong** | Divine domain | Death/Twilight | Psychopomp deity | Decay damage, life drain, fear of death |
| **Verdance** | Primal | Druidic nature magic | The living world itself | Entangle, regrowth, beast shapes, storms |
| **Pactbinding** | Occult | Warlock pact magic | Bargains with lesser powers | Curses, soul siphons, summoned servitors |
| **The Hollow** | Forbidden | Far Realm / aberrant | The cosmic-horror outside (lore spec's "forbidden" tier) | Madness, unstable summons, reality tears — high power, self-harm costs |

Class access (caster archetypes from the classes-races spec; working names): **Arcanist** (Ruin, Wardcraft, Threshold, Flux), **Templar** (Dawnfire, Gravesong), **Warden** (Verdance), **Occultist** (Pactbinding, The Hollow). The Hollow is additionally locked behind late skill-tree nodes and applies a self-debuff on every cast (lore-enforced cost).

### 2. How a Spell Is Represented

A spell is exactly what an ability is today: **a RON file deserializing into a `CharacterAbility` variant**, registered in `ability_set_manifest.ron`, skill-gated via `AbilityKind::Simple(Some(skill), id)`.

New asset layout:

```
assets/common/abilities/spells/<school>/<spell>.ron     # one file per spell
assets/common/abilities/innate/<species>.ron            # racial innates
```

Manifest entries use both existing `AbilitySpec` arms:

- `Tool(Tome)`, `Tool(HolySymbol)`, `Tool(Focus)` — spells granted by caster implements.
- `Custom("class.arcanist")`, `Custom("innate.elf")` — weaponless class/racial ability pools (the `Custom(String)` arm is already exercised by ~60 NPC sets under `assets/common/abilities/custom/`).

Example — `assets/common/abilities/spells/ruin/emberlance.ron` (spell #1, pure reuse of `BasicRanged`, modeled on `staff/firebomb.ron`):

```ron
BasicRanged(
    energy_cost: 0,
    buildup_duration: 0.5,
    recover_duration: 0.35,
    projectile: (
        kind: Explosive(radius: 1.5, min_falloff: 0.5, reagent: Some(Red), terrain: None),
        attack: Some((damage: 16, energy: 6, buff: Some((kind: Burning, dur_secs: 4, strength: DamageFraction(0.1), chance: 0.2)))),
    ),
    projectile_body: Object(BoltFire),
    projectile_light: None,
    projectile_speed: 65.0,
    num_projectiles: Value(1),
    projectile_spread: None,
    meta: (school: Some(Ruin), cooldown: None),
)
```

The only non-vanilla token above is `meta.school`/`meta.cooldown` (Phase 1 additions to `AbilityMeta`). Everything else deserializes against today's `CharacterAbility::BasicRanged` (ability.rs:799).

### 3. Caster Implements vs Weaponless Class Abilities: Hybrid

**Recommendation: hybrid.** Both paths, complementary:

**Path A — new `ToolKind`s (implements).** Add `Tome` (Arcanist), `HolySymbol` (Templar), `Focus` (Warden/Occultist) to `ToolKind` in `common/src/comp/inventory/item/tool.rs:26`. Verified extensible: update the exhaustive matches at `identifier_name` (line 57), `gains_combat_xp` (line 80), `can_block` (line 96), `block_priority` (line 107), plus item RONs under `assets/common/items/weapons/`, voxel models, and `SkillGroupKind::Weapon(ToolKind)` persistence enums. Implements carry primary/secondary attacks (cantrip-feel: cheap bolt + utility) exactly like Staff/Sceptre today. Spells slot into their `abilities` vec.

**Path B — weaponless innate pool (new machinery).** Class and racial abilities that work regardless of equipment. Today `AuxiliaryAbility` can only reference equipped items (MainWeapon/OffWeapon/Glider). We add:

- `AuxiliaryAbility::Innate(usize)` and `Ability::Innate(usize)` variants in `common/src/comp/ability.rs` (the `ArmorAbility` placeholder comment at line 395 shows this extension was anticipated).
- A new `AbilityPool` component (`common/src/comp/ability.rs`, registered in `common-state`): an ordered `Vec<String>` of ability ids granted by class skill unlocks and species, resolved through `AbilityMap` with `AbilitySpec::Custom` keys.
- Resolution in `Ability::ability_id` (ability.rs:419) and `SpecifiedAbility::ability_id` (ability.rs:534): `Innate(i)` looks up `AbilityPool` instead of `inv.equipped(...)`.
- Persistence: new string arm `"Innate:index:N"` in `server/src/persistence/json_models.rs:126–145`.

Why hybrid: implements give casters an equipment progression track (loot, crafting) and reuse 100% of the existing pipeline; the innate pool covers racial abilities and class signatures that must survive disarming/weapon swaps, and is the smallest possible extension (one enum variant + one component).

### 4. Binding Path End-to-End

| Step | Mechanism | Files |
|---|---|---|
| 1. Skill unlock | Player spends points in class tree → `SkillSet` gains `Skill::Class(ClassSkill)` (new variant; tree defined in classes-races spec) | `common/src/comp/skillset/skills.rs`, `skillset/mod.rs` |
| 2. Ability becomes available | Manifest entry `AbilityKind::Simple(Some(Skill::Class(...)), "common.abilities.spells.ruin.shatterburst")` now passes the `has_skill` check in `AbilityKind::ability()` (tool.rs:367–396); innate-pool grants append to `AbilityPool` | `assets/common/abilities/ability_set_manifest.ron`, `common/src/comp/ability.rs` |
| 3. Hotbar binding | Diary UI lists unlocked abilities; drag to slot emits `Event::ChangeAbility(slot, AuxiliaryAbility::Innate(i))` → client → server `ActiveAbilities::change_ability` (ability.rs:123) keyed by `AuxiliaryKey` | `voxygen/src/hud/diary.rs`, `voxygen/src/hud/mod.rs:739` |
| 4. Input | Hotbar key → `Input::Ability(n)` → `handle_ability` (`common/src/states/utils.rs:1440`) → `ActiveAbilities::activate_ability` resolves slot → `AbilityItem` | `voxygen/src/hud/hotbar.rs`, `common/src/states/utils.rs` |
| 5. Character state | `CharacterState::try_from((&CharacterAbility, AbilityInfo, &JoinData))` (ability.rs:~2400+) instantiates the state; `character_behavior` system ticks it | `common/src/comp/character_state.rs:783`, `common/systems/src/character_behavior.rs` |
| 6. Server effect | State emits events (damage, aura, summon, explosion) handled by `common/systems/src/{shockwave,beam,aura,projectile,buff}.rs` and `server/src/events/` | per-effect |
| 7. FX | Component-driven particles (`maintain_*_particles`) or one-shot `Outcome`s → particles + SFX | `voxygen/src/scene/particle.rs`, `voxygen/src/audio/sfx/` |

### 5. Gap Analysis: Existing vs New Machinery

**Expressible today (content-only work):**

| D&D-style effect | Existing variant | Proof in tree |
|---|---|---|
| Damage nuke / bolt | `BasicRanged` (+ explosion on `ProjectileConstructor`) | `assets/common/abilities/staff/firebomb.ron` |
| Sustained beam | `BasicBeam` | `staff/flamethrower.ron`, `sceptre/lifestealbeam.ron` |
| Self-centered AoE | `Shockwave`, `Explosion` | `staff/fireshockwave.ron` |
| Party buff / enemy debuff aura | `BasicAura`, `StaticAura` | `sceptre/wardingaura.ron` |
| Heal | `BasicAura` (heal aura), heal beam | `sceptre/healingaura.ron` |
| Self buff / shield | `SelfBuff` (+ `ProtectingWard`), `AbilityMeta.capabilities` BLOCKS/PARRIES | `staff/flame_cloak.ron` |
| Summon creature | `BasicSummon` (`SummonInfo`) | used by NPC sets under `custom/` |
| Teleport | `Blink` | `states/blink.rs` |
| Projectile with status (slow/curse/DoT) | `BasicRanged` + `buff: Option<CombatBuff>` (projectile.rs:141) | bow/staff DoTs |
| Polymorph | `Transform` | `states/transform.rs` (NPC transforms today) |
| Conjured terrain (wall of thorns/ice) | `SpriteSummon` + new sprite kinds | `states/sprite_summon.rs` |
| Silence | projectile/aura applying `BuffKind::Amnesia` (blocks auxiliary abilities) | `buff.rs` Amnesia doc comment |

**New machinery required:**

The largest item, `GroundAoe`, follows the standard four-stage state shape used by `basic_summon.rs` and `shockwave.rs`:

```
GroundAoe state machine (common/src/states/ground_aoe.rs)

  Buildup ──────────► Action ─────────────► Strike ──────► Recover ──► Wielding
  (cast time;         (telegraph decal      (server emits   (tail;
   aim point =         at locked target      ExplosionEvent/  cancellable
   crosshair ray ∩     pos for `delay` s;    GroundAoeEvent   by roll)
   terrain, clamped    caster free to move   at pos; Outcome
   to `max_range`;     unless `rooted_cast`) for FX)
   poise-interruptible)
```

`StaticData`: `buildup_duration`, `delay`, `recover_duration`, `max_range`, `radius`, `damage`/`effects` (reuse `combat::AttackEffect`), `rooted_cast: bool`, `specifier` (frontend decal/particle selector), `ability_info`. Target position is resolved client-side during Buildup and validated server-side (range + line-of-sight) at Strike, same trust model as `BasicRanged` aim.

| Need | New variant/state | State machine sketch | Files to touch |
|---|---|---|---|
| Ground-targeted AoE template (Flame Strike/Blizzard) | `CharacterAbility::GroundAoe` + `common/src/states/ground_aoe.rs` | Buildup (aim reticle, `FrontendSpecifier` drives ground decal) → Action (telegraph circle at target pos for `delay` secs) → Strike (emit `ExplosionEvent`/new `GroundAoeEvent` at pos) → Recover | `common/src/comp/ability.rs` (variant + `try_from`), `common/src/states/{mod.rs,ground_aoe.rs}`, dispatch matches in `common/src/comp/character_state.rs` (~783, ~848), new `Outcome::GroundAoe` in `common/src/outcome.rs`, decal/particles in `voxygen/src/scene/particle.rs`, SFX map |
| Fear CC | `BuffKind::Terrified` + forced-flee | Buff effect forces movement away from source: players get inverted-control `MovementModifier`; NPCs get flee goal | `common/src/comp/buff.rs`, `common/systems/src/buff.rs`, movement override in `common/src/states/utils.rs`, agent reaction in `server/src/sys/agent/` (hot-reloadable `server-agent`) |
| Charm CC | `BuffKind::Charmed` | v1 scope: charmed NPCs switch `Alignment` toward caster for duration (reuse pet/loyalty machinery); no player-charm in v1 | `common/src/comp/buff.rs`, `server/src/sys/agent/`, `common/src/comp/agent.rs` |
| Per-spell cooldowns | `AbilityCooldowns` component | See §8 | `common/src/comp/ability.rs`, `common/src/states/utils.rs:1440`, `common-state` registration, HUD greying in `voxygen/src/hud/hotbar.rs` |
| Innate ability pool | `AbilityPool` + `AuxiliaryAbility::Innate` | See §3 Path B | ability.rs, json_models.rs, diary.rs |
| Resurrection | server `RevivalEvent` | Deferred — see Open Questions | `server/src/events/` |

The shockwave wiring pattern to copy for `GroundAoe` FX: `common/systems/src/shockwave.rs` ticks the server `Shockwave` component; `voxygen/src/scene/particle.rs:3604` (`maintain_shockwave_particles`) reads the same component client-side and matches `properties.specifier` to a `ParticleMode`; one-shot blasts instead emit an `Outcome` (e.g. `Outcome::FireShockwave`, outcome.rs:167) consumed at particle.rs:~437.

### 6. New BuffKinds

| Proposed | D&D analogue | Status today | Work |
|---|---|---|---|
| `Terrified` | Fear | Missing | New BuffKind + forced movement + agent flee (M) |
| `Charmed` | Charm Person/Monster | Missing | New BuffKind + NPC alignment swap (M) |
| Silence | Silence | **Exists as `Amnesia`** ("prevents use of auxiliary abilities") | Content-only: deliver via projectile/aura (S) |
| Slow | Ray of Frost / Slow | **Exists as `Crippled`, `Chilled`, `Frozen`** | Content-only (S) |
| Root | Entangle | **Exists as `Rooted`, `Ensnared`** | Content-only (S) |
| `Hollowtouched` | — (forbidden-school self-cost) | Missing | New BuffKind: stacking self-debuff applied by every Hollow cast via `AbilityMeta.init_event: GainBuff` (S — init_event already supports GainBuff, ability.rs:3726) |

### 7. Racial Abilities

Mechanism: at character creation (`server/src/character_creator.rs:61`), replace `SkillSet::default()` with `SkillSetBuilder` applying a per-species preset (`assets/common/skillset/preset/species/<species>.ron`, loaded via `with_asset_expect` like the existing `rank1.fullskill` presets), and push the species' innate ability id into the new `AbilityPool`. The innate is bound like any auxiliary ability (`AuxiliaryAbility::Innate(0)`); racial RONs live at `assets/common/abilities/innate/`.

| Species | Ability (working name) | Variant | Effect sketch |
|---|---|---|---|
| Human | Second Wind | `SelfBuff` | Burst of `EnergyRegen` + minor heal; long cooldown |
| Elf | Glimmerstride | `SelfBuff` | Short `Hastened` + `Agility` |
| Dwarf | Stoneblood | `SelfBuff` | `Fortitude` + poise resistance window |
| Orc | Blood Fury | `SelfBuff` | `Reckless` (damage up, defense down) |
| Danari | Veilstep | `Blink` | Short-range blink (8 m vs Arcanist spell's 24 m) |
| Draugr | Grave Chill | `Shockwave` | Small frost nova applying `Chilled` |

All six use existing variants — racial abilities are pure content plus the Path B plumbing.

### 8. Casting Rules

- **Costs:** `energy_cost` per spell RON. Baseline: cantrip-tier 0–10, standard 50–150, ultimate 250+ (full default pool ≈ player energy; exact curves tuned in balance pass with the levels spec's energy-per-level growth).
- **Cast times:** `buildup_duration` is the cast time (0.3 s instant-feel to 2.5 s for ultimates); `recover_duration` is the global-cooldown-feel tail.
- **Interruption:** verified — poise damage during buildup triggers `PoiseState::Interrupted/Stunned` (`common/src/comp/poise.rs:63`) and cancels the state. Rule: spells with buildup ≥ 1.0 s must NOT set `Capability::POISE_RESISTANT`, preserving counterplay.
- **Cooldowns (new):** add `cooldown: Option<f32>` to `AbilityMeta` (RON-tweakable). New `AbilityCooldowns` component (`HashMap<String, Time>` keyed by ability id) on players, registered in `common-state`. Check + write in `handle_ability` (`common/src/states/utils.rs:1440`) next to the existing energy/requirements checks; replicate to client for hotbar grey-out and radial sweep in `voxygen/src/hud/hotbar.rs`. Not persisted across logout (acceptable v1; documented exploit surface is ≤ 60 s ultimates).
- **The Hollow surcharge:** every Hollow spell's `init_event` applies `Hollowtouched` (stacking max-health reduction, 60 s), making the forbidden school a deliberate gamble.

## Content Plan: Spell List v1

16 spells, 4 per caster archetype. Level req = character level (levels spec); skill gate = class tree node (classes-races spec).

| # | Spell | School | Class | Lvl | Variant | Machinery |
|---|---|---|---|---|---|---|
| 1 | Emberlance | Ruin | Arcanist | 1 | `BasicRanged` | Existing |
| 2 | Wardshell | Wardcraft | Arcanist | 3 | `SelfBuff` (`ProtectingWard`) | Existing |
| 3 | Veilrend | Threshold | Arcanist | 5 | `Blink` | Existing |
| 4 | Shatterburst | Ruin | Arcanist | 8 | `GroundAoe` | **New state** |
| 5 | Dawnlight Mend | Dawnfire | Templar | 1 | `BasicAura` (heal) | Existing (clone `sceptre/healingaura.ron`) |
| 6 | Radiant Verdict | Dawnfire | Templar | 3 | `BasicRanged` + smite buff | Existing |
| 7 | Aegis of Dawn | Dawnfire | Templar | 5 | `StaticAura` (`ProtectingWard`) | Existing |
| 8 | Censure | Gravesong | Templar | 8 | `BasicRanged` + `Amnesia` | Existing (silence via projectile buff) |
| 9 | Thornlash | Verdance | Warden | 1 | `BasicBeam` + `Crippled` | Existing |
| 10 | Verdant Mending | Verdance | Warden | 3 | `StaticAura` (`Regeneration`) | Existing |
| 11 | Galeburst | Verdance | Warden | 5 | `Shockwave` (knockback) | Existing |
| 12 | Wildshape: Stalker | Verdance | Warden | 10 | `Transform` | Existing state, new player-facing config |
| 13 | Umbral Bolt | Pactbinding | Occultist | 1 | `BasicRanged` + `Cursed` | Existing |
| 14 | Soul Siphon | Pactbinding | Occultist | 3 | `BasicBeam` (lifesteal) | Existing (clone `sceptre/lifestealbeam.ron`) |
| 15 | Dread Whisper | The Hollow | Occultist | 8 | `BasicRanged` + `Terrified` | **New BuffKind** |
| 16 | Hollow Gate | The Hollow | Occultist | 12 | `BasicSummon` (aberrant servitor) | Existing state, new entity config + `Hollowtouched` cost |

Plus the 6 racial innates (§7). Total v1: 22 ability RONs, 12 reuse existing variants outright.

## Phases

### Phase 1 — Foundations (M, ~8 dev-days)

| Task | Files | Size |
|---|---|---|
| Add `Tome`/`HolySymbol`/`Focus` to `ToolKind` + exhaustive-match fixes + 3 starter implement items/voxel models | `common/src/comp/inventory/item/tool.rs`, `assets/common/items/weapons/`, `assets/voxygen/voxel/` | M |
| `AbilityPool` component + `AuxiliaryAbility::Innate`/`Ability::Innate` + resolution in `ability_id` paths + component registration | `common/src/comp/ability.rs`, `common-state/src/state.rs` | M |
| Persistence arms for `Innate` + DB migration dry-run on copied character DB | `server/src/persistence/json_models.rs` | S |
| Cooldown system: `AbilityMeta.cooldown`, `AbilityCooldowns` component, gate in `handle_ability`, hotbar radial sweep | `common/src/comp/ability.rs`, `common/src/states/utils.rs:1440`, `voxygen/src/hud/hotbar.rs` | M |
| `Skill::Class(...)` + `SkillGroupKind::Class(...)` plumbing (shared deliverable with classes-races spec) | `common/src/comp/skillset/` | S |
| `SpellSchool` enum in `AbilityMeta` | `common/src/comp/ability.rs` | S |

- **Milestone:** a test Tome with one skill-gated RON spell castable end-to-end; cooldown visible on hotbar.
- **Risk:** `ToolKind` additions touch DB enums for skill groups — migration tested against a copied character DB before merge.

### Phase 2 — Content Wave: Existing-Machinery Spells (M, ~9 dev-days)

| Task | Files | Size |
|---|---|---|
| 12 existing-variant spell RONs (#1–3, 5–7, 9–11, 13–14, 16) + manifest entries | `assets/common/abilities/spells/`, `ability_set_manifest.ron` | M |
| 6 racial innate RONs + species skillset presets + creation hook | `assets/common/abilities/innate/`, `assets/common/skillset/preset/species/`, `server/src/character_creator.rs:61` | S |
| `Hollowtouched` BuffKind wired through `init_event: GainBuff` | `common/src/comp/buff.rs`, Hollow spell RONs | S |
| Icons + i18n strings for all 18 abilities | `assets/voxygen/element/`, `assets/voxygen/i18n/` | M |
| Hollow Gate servitor entity config | `assets/common/entity/` | S |

- **Milestone:** all four archetypes playable with 3 spells each; every species has its innate at creation.
- **Risk:** balance drift — mitigated by `asset_tweak` live-tuning sessions and the Phase 4 pass.

### Phase 3 — New Machinery: GroundAoe + CC (L, ~12 dev-days)

| Task | Files | Size |
|---|---|---|
| `CharacterAbility::GroundAoe` variant + `try_from` instantiation + `adjusted_by_stats` arm | `common/src/comp/ability.rs` | M |
| `states/ground_aoe.rs` + module + dispatch matches | `common/src/states/{mod.rs,ground_aoe.rs}`, `common/src/comp/character_state.rs:783,848` | M |
| Telegraph decal + `Outcome::GroundAoe` + particles + SFX (copy shockwave wiring, §5) | `common/src/outcome.rs`, `voxygen/src/scene/particle.rs`, `voxygen/src/audio/sfx/` | M |
| `Terrified`: buff effect, player movement override, agent flee (hot-reloadable dev loop) | `common/src/comp/buff.rs`, `common/systems/src/buff.rs`, `common/src/states/utils.rs`, `server/src/sys/agent/` | M |
| `Charmed`: NPC alignment swap + boss immunity tags | `common/src/comp/buff.rs`, `common/src/comp/agent.rs`, `server/src/sys/agent/` | M |
| Spells #4, #15, #8 + CC-resist tuning | `assets/common/abilities/spells/` | S |

- **Milestone:** Shatterburst telegraph readable in 3rd-person at 30 m; fear visibly routs a gnarling pack.
- **Risk:** forced movement vs player agency/netcode prediction — fall back to "fear = Silenced + heavy slow" if prediction artifacts appear in playtests.

### Phase 4 — Wildshape, Balance & Polish (M, ~7 dev-days)

| Task | Files | Size |
|---|---|---|
| Wildshape: Stalker (#12): player-initiated `Transform`, stat mapping, revert input, edge-case handling | `common/src/states/transform.rs`, spell RON, `voxygen` input | M |
| Balance pass: energy curves vs level growth, cooldown matrix, Hollow surcharge — via `asset_tweak` | all spell RONs | M |
| Spell-school grouping in Diary UI (school icons, filter tabs) | `voxygen/src/hud/diary.rs` | S |
| FX/SFX polish for all 22 abilities | `voxygen/src/scene/particle.rs`, `voxygen/src/audio/sfx/` | M |

- **Milestone:** v1 ship gate — full caster session (level 1→12) playable without dev console.
- **Risk:** `Transform` was NPC-only; player edge cases (mounting, gliding, inventory access) enumerated and tested.

**Total:** ~36 dev-days, one senior dev + AI. Complexity rollup: Phase 1 M, Phase 2 M, Phase 3 L, Phase 4 M.

## Testing Strategy

- **Deserialization:** unit tests loading every RON under `assets/common/abilities/spells/` and `innate/` through `AbilityMap::load()` (`tool.rs:665`), run with `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`; fails CI on any malformed spell.
- **State transitions:** unit tests for `ground_aoe::Data::behavior` stage progression (Buildup→Action→Strike→Recover) and cooldown gating in `handle_ability` (energy present, cooldown active → no state change), following existing patterns in `common/src/states/`.
- **Buff semantics:** tests asserting `Terrified` movement override and `Charmed` alignment swap in `common/systems/src/buff.rs` and agent tests.
- **Persistence round-trip:** `json_models.rs` test: `AuxiliaryAbility::Innate(3)` → string → back.
- **Balance:** live sessions with the verified `asset_tweak` feature (`common/assets/Cargo.toml:29`) for energy/cooldown/damage tuning without recompiles; telemetry hooks (existing logging system, `common/systems/src/telemetry.rs`) record per-spell cast counts and kill participation.
- **Lint/CI:** the new `ToolKind`/`CharacterAbility` variants make exhaustive matches fail loudly — `cargo clippy --all-targets -- -D warnings` is the completeness check.

## Open Questions

1. **Resurrection** — needs a downed/defeated state and a server `RevivalEvent`; out of v1, revisit when group content (dungeons spec) lands.
2. **NPC casters** — should rtsim mages use the new schools? Requires agent ability selection awareness of cooldowns; Phase 3 of a future spec.
3. **Cooldown persistence** — v1 resets on logout; if relog-cycling ultimates becomes an exploit in playtests, persist `AbilityCooldowns` alongside `ability_sets`.
4. **Hollow school visuals** — cosmic-horror FX (screen-edge distortion?) may need a post-process hook beyond particles; coordinate with the rendering pipeline owners.
5. **Spell school resistances** — should armor gain per-school resist stats? Deferred to an itemization spec; `SpellSchool` in `AbilityMeta` keeps the door open.
