# Character Levels — Design

**Date:** 2026-06-10
**Part of:** [RPG Evolution Master Roadmap](2026-06-10-rpg-evolution-master-roadmap.md)
**Implementation plan:** [docs/superpowers/plans/2026-06-10-character-levels.md](../plans/2026-06-10-character-levels.md)

## Context

The fork's goal is WoW/Diablo-style character progression. Veloren today has *skill-tree*
progression but no character level: players earn XP into per-pool `SkillGroup`s and spend
skill points, but there is no single number that says "how far along is this character",
nothing for mobs/zones to compare against, and no level-up moment.

A character level is also a hard dependency for: classes (level-gated class features,
[classes-races-design](2026-06-10-classes-races-design.md)), equipment requirements
(`min_level`, [equipment-restrictions-design](2026-06-10-equipment-restrictions-design.md)),
and zone difficulty (mob level bands and XP differential rules,
[world-difficulty-zones-design](2026-06-10-world-difficulty-zones-design.md)).

## Current state (verified)

| Mechanism | Where |
|---|---|
| XP pools per skill group; `earned_exp`/`available_exp` per group; SP earned by spending XP | `common/src/comp/skillset/mod.rs:142-209` |
| `SkillSet { skill_groups, skills }` ECS component, persisted per group (incl. `earned_exp`) | `common/src/comp/skillset/mod.rs:216`, `server/src/persistence/` |
| XP award on kill: combat-rating-based reward, split across equipped-weapon pools + General | `server/src/events/entity_manipulation.rs:505-556` (`handle_exp_gain`), reward calc ~`:1122` |
| Client feedback: `Outcome::ExpChange` / `Outcome::SkillPointGain` | `common/src/outcome.rs:56-65` |
| Nameplate HUD: `overhead::Info` (name, health, combat_rating…); dormant `level`/`level_skull` widget ids currently render combat-rating icons | `voxygen/src/hud/overhead.rs:45-49,409-437`, built at `voxygen/src/hud/mod.rs:2449` (with `skill_set` already in the join) |
| Stats temp modifiers exist but are **reset every tick** (buff-driven) — unsuitable for permanent level scaling | `common/src/comp/stats.rs` (`reset_temp_modifiers`) |

## Design

### Level is *derived*, not stored

`character_level = f(total earned_exp across all skill groups)`. Because `earned_exp` is
already persisted per skill group, **no database migration is needed**, existing characters
get a correct level retroactively, and the value can never desync from XP. This also keeps
the diff surface near zero for upstream merges.

```
total_exp(L) = LEVEL_XP_BASE · (L−1)²        (quadratic curve, standard RPG feel)
level(xp)    = min(MAX_CHARACTER_LEVEL, floor(sqrt(xp / LEVEL_XP_BASE)) + 1)
```

- `MAX_CHARACTER_LEVEL = 60`, `LEVEL_XP_BASE = 250` (tunable constants; M3 balances them
  against real telemetry — at base 250, level 60 ≈ 870k lifetime XP).
- Monotonic, total at level 1 = 0, exact inverse pair → property-tested.

### Level-up moment

`handle_exp_gain` compares level before/after adding XP and emits a new
`Outcome::CharacterLevelUp { uid, new_level }`. v1 reuses existing no-op/SFX arms
client-side; M2 adds dedicated SFX/particles/toast.

### Display

- Nameplates: level shown as `Name [12]` via a new `level: Option<u16>` field on
  `overhead::Info` (the construction site already joins `SkillSet`).
- M2: character window, social/group list, character-select list.

### Explicit non-goals of v1 (owned by other specs)

- **Stat growth per level** → classes spec (needs a *permanent* modifier mechanism, not
  the tick-reset `StatsModifier`s; classes own HP/damage-per-level tables per class).
- **XP curve by mob level / gray-mob cutoff / rested XP** → world-difficulty spec.
- **Level-gated content** → equipment-restrictions + classes specs.

## Risks

| Risk | Mitigation |
|---|---|
| Curve mistuned (60 too fast/slow) | Constants + telemetry events (`telemetry!` on level-up) make retune a 1-line change; derived level means retunes apply retroactively and consistently |
| Upstream touches `handle_exp_gain` | Change is ~10 added lines inside one function; trivially re-mergeable |
| Players expect stat gain on level-up | Patch notes scope v1 as "display + milestone"; classes wave delivers power growth |

## Milestones

| Milestone | Content | Complexity |
|---|---|---|
| M1 (plan ready) | Curve + `SkillSet::character_level()` + level-up outcome + nameplate display | S — 2–3 dev-days |
| M2 | Dedicated level-up SFX/VFX/toast; level in char window, social list, char select; `telemetry!` event | S — 2–3 dev-days |
| M3 | Balance pass with `game-balance-designer` agent using telemetry distributions; final curve constants | S — 1–2 dev-days + playtest time |

## Testing

- Unit: curve monotonicity, inverse property, cap; `SkillSet::character_level()` on
  default and seeded skillsets (`VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`).
- Integration: kill mobs in a dev session (`veloren-run` skill), verify level-up outcome in
  logs/telemetry (`veloren-telemetry` skill) and nameplate rendering.

## Open questions

- Should pet/NPC nameplates show level too? (Default v1: yes — same code path; revisit if
  visual noise.) Resolved per world-difficulty spec when mobs get levels.
- Account-wide vs per-character max-level perks — defer to post-wave-3 design.
