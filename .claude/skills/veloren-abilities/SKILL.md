---
name: veloren-abilities
description: Use when creating spells, combat abilities, buffs, auras, or new magic schools — guides the RON ability pipeline, CharacterState implementation, and the magic spec
---

# veloren-abilities

**REQUIRED:** Read `docs/superpowers/specs/2026-06-10-magic-abilities-design.md` first. Invoke `veloren-dev` and `superpowers:test-driven-development` before coding.

## The ability pipeline (end to end)

```
RON asset (assets/common/abilities/<school>/<spell>.ron)
  → ability_set_manifest.ron (maps AbilitySpec::Tool(ToolKind)/Custom(String) → AbilitySet)
  → AbilityMap resource (common/src/comp/inventory/item/tool.rs)
  → CharacterAbility variant (common/src/comp/ability.rs — 33 variants)
  → CharacterState (common/src/states/<state>.rs)
  → voxygen FX (scene/particle.rs, audio/sfx/mod.rs, outcomes)
```

A new *spell* using existing machinery = RON file + manifest entry + (optional) skill gate.
A new *mechanic* = new CharacterAbility variant + new state file + registration in
`character_state` handling + FX wiring. Always check the gap-analysis table in the magic
spec before writing Rust — most D&D-style effects are already expressible.

## Verified facts (2026-06-10)

- Existing variants cover: nukes, beams, AoE, auras (`BasicAura`/`StaticAura`), self-buffs,
  heals, `BasicSummon`, `Blink`, `Transform` (polymorph), `SpriteSummon` (walls/terrain),
  `Explosion`. Projectiles already apply status (`ProjectileConstructor.buff`,
  `common/src/comp/projectile.rs:141`).
- **No cooldown system exists.** `charge_duration` = charge-up attacks, not cooldowns. The
  cooldown design (`AbilityCooldowns` component, gate in `handle_ability` at
  `common/src/states/utils.rs:1440`) is specified in the magic spec — implement it there
  once, don't improvise per-spell timers.
- CC buffs: silence = `BuffKind::Amnesia`; slows = `Crippled`/`Chilled`/`Frozen`;
  `Charmed`/`Feared` do NOT exist yet (spec defines them).
- Ability slots: guard/primary/secondary/auxiliary + `movement` (`MovementAbility::Species`).
  Auxiliary sets key on equipped tools (`AuxiliaryKey = (Option<ToolKind>, Option<ToolKind>)`)
  — weaponless class/racial abilities need the spec's `Innate` variant, they cannot ride
  the existing keying.
- Skill-gating: `AbilityKind::Simple(Option<Skill>, T)` — gate spells on class-tree skills.
- `asset_tweak` feature exists (`common/assets/Cargo.toml:29`) — use it for live balance
  iteration instead of recompiling.

## Steps for a new spell (content-only)

1. Pick the school + class from the spec's spell table; confirm the `CharacterAbility`
   variant it maps to.
2. Copy the closest existing RON under `assets/common/abilities/` (e.g. staff fireball for
   a nuke), adjust numbers per the balance table.
3. Register in `assets/common/abilities/ability_set_manifest.ron` under the right
   `AbilitySpec`, with its `Skill` gate.
4. `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` — asset-loading tests
   (e.g. `AbilityMap` load) must pass; a typo'd RON fails here, not at runtime.
5. In-game check via `veloren-run` (hot-reloading picks up RON edits in dev).

## Steps for a new mechanic (Rust)

1. New variant in `common/src/comp/ability.rs` + state in `common/src/states/<name>.rs`
   (4-stage pattern: buildup → action → recover, see the spec's `GroundAoe` sketch).
2. Wire FX: outcome or particle mapping (shockwave pattern at
   `voxygen/src/scene/particle.rs:3604` is the reference).
3. Exhaustive matches: let `cargo check --workspace --all-targets` find every site; never
   add wildcard arms.
4. Unit-test state transitions; then content steps above.

## Rules

- Server authority on all effects; FX are client-side only.
- New BuffKinds need: variant, stacking/decay rules, icon asset, i18n string (`.ftl`).
- Balance numbers from `game-balance-designer` tables; spell names/flavor from
  `lore-writer` (canon check against `lore/`).
