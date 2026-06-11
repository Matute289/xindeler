# RPG Evolution — Master Roadmap

**Date:** 2026-06-10
**Status:** Approved program plan — individual specs linked below
**Scope:** Full evolution of this Veloren fork into a class-based, leveled, lore-rich RPG with AI-driven social simulation (AURORA) and an autonomous world director (ORACLE).

## Context

This fork already carries: a custom terrain pipeline (Transvoxel, block scaling, normal maps), a production logging/telemetry system, bug reporting, and upstream-merge automation. The base game provides a far stronger foundation for this program than expected:

| Foundation | State | Where |
|---|---|---|
| XP + skill points + skill trees | ✅ Exists (per weapon + general pools) | `common/src/comp/skillset/` |
| Data-driven abilities (RON) | ✅ Exists (~30 ability archetypes, states machine) | `common/src/comp/ability.rs`, `assets/common/abilities/` |
| Buffs/auras/debuffs | ✅ Exists (~30 BuffKinds) | `common/src/comp/buff.rs` |
| NPC personality (Big Five) + sentiments | ✅ Exists | `common/src/rtsim.rs`, `rtsim/src/data/` |
| NPC dialogue + quests (Escort/Slay/Courier) | ✅ Exists in this fork | `rtsim/src/data/quest.rs`, `rtsim/src/rule/npc_ai/dialogue.rs` |
| Population director ("Architect") | ✅ Exists | `rtsim/src/rule/architect.rs` |
| Biome difficulty (1–5) | ⚠️ Exists, only used for spawn weighting | `common/src/terrain/biome.rs` |
| Portals/teleporters | ⚠️ Minimal (one-way, same world) | `common/src/comp/misc.rs` |
| Character level, classes, races-as-mechanics, equip restrictions | ❌ Missing | — |
| Seasons/astronomy, dynamic dungeons, multi-plane worlds | ❌ Missing | — |

**Verdict: every requested feature is feasible.** None requires fighting the engine's architecture; the hard ones (AURORA, ORACLE, multi-plane) are large but build on existing rtsim scaffolding.

## Program structure

Nine workstreams, each with its own design spec:

| # | Workstream | Spec | Size | Est. (1 senior dev + AI) |
|---|---|---|---|---|
| 0 | Engine improvements (perf, memory, review process) | [engine-improvements-design](2026-06-10-engine-improvements-design.md) | M, continuous | 3–6 wks first pass |
| 1 | Character levels (WoW/Diablo-style) | [character-levels-design](2026-06-10-character-levels-design.md) | M | 1–2 wks |
| 2 | Magic + class/race abilities (D&D-style) | [magic-abilities-design](2026-06-10-magic-abilities-design.md) | L | 4–8 wks |
| 3 | Classes + races system | [classes-races-design](2026-06-10-classes-races-design.md) | L | 3–5 wks |
| 3b | Class/level/race-gated equipment | [equipment-restrictions-design](2026-06-10-equipment-restrictions-design.md) | S | ~1 wk |
| 4 | World difficulty zones, NPC levels, map, planes | [world-difficulty-zones-design](2026-06-10-world-difficulty-zones-design.md) | L (planes: XL) | 4–8 wks (+2–3 mo planes) |
| 5 | Lore & cosmology (Exandria-style + cosmic horror) | [lore-cosmology-design](2026-06-10-lore-cosmology-design.md) | M, writing-heavy | 2–4 wks core + ongoing |
| 6a | PROJECT AURORA — social simulation layer | [project-aurora-design](2026-06-10-project-aurora-design.md) | XL | 4–7 mo |
| 6b | PROJECT ORACLE — world director AI | [project-oracle-design](2026-06-10-project-oracle-design.md) | XL | 4–7 mo |

Implementation plans live in `docs/superpowers/plans/`. The first executable plan is
[2026-06-10-character-levels.md](../plans/2026-06-10-character-levels.md). Each subsequent
milestone gets its own plan authored just-in-time (designs drift; code-level plans written
months ahead rot).

## Dependency graph

```
            ┌──────────────────────────────┐
            │ 0 Engine improvements        │  (continuous, parallel to all)
            └──────────────────────────────┘

  1 Levels ──→ 3 Classes/Races ──→ 3b Equip restrictions
      │              │
      │              └──→ 2 Magic & class abilities
      │
      └──→ 4 World difficulty zones ──→ 4x Multi-plane worlds
                     ↑                            ↑
  5 Lore bible ──────┴────────────────────────────┘
      │
      ├──→ 6a AURORA (religions/orgs consume pantheon; quests consume levels)
      └──→ 6b ORACLE (events consume lore arcs, ecosystem consumes difficulty zones)

  6b ORACLE ──(world facts)──→ 6a AURORA ──(NPC reactions)──→ players
```

Key contract: **ORACLE decides WHAT happens in the world; AURORA decides how its
inhabitants react.** Both read/write rtsim `Data`; the integration interface is defined in
both specs and must stay symmetrical.

## Recommended sequencing (waves)

| Wave | Content | Duration | Playable outcome |
|---|---|---|---|
| 1 | Levels (plan ready) + equip-restriction scaffolding + engine pass 1 (profiling baseline, meshing/physics quick wins) | 3–4 wks | Characters show levels, level-up feedback, perf baseline dashboards |
| 2 | Classes + races v1 (4 classes) + magic v1 (12–20 spells) + lore bible core | 6–9 wks | Class selection at creation, class trees, first spell schools, canon pantheon |
| 3 | World difficulty zones + NPC levels/classes + magic v2 + lore in-game (books, naming, dialogue) | 6–9 wks | Leveled regions, leveled mobs/NPCs, zone-based progression loop |
| 4 | AURORA phases 1–4 (foundations, social graph, families, economy) | 3–4 mo | NPCs with relationships, families, living economy |
| 5 | ORACLE phases 1–4 (world state, events, ecosystem, climate) + AURORA 5–6 (orgs, dynamic quests) | 3–4 mo | World events, monster ecology, NPC organizations, organic quests |
| 6 | ORACLE 5–8 (astronomy, narrative, player impact, optimization) + AURORA 7–8 (LLM, optimization) + multi-plane worlds | 3–4 mo | Living narrated world, planes via portals |

**Total program: ~12–18 months** for one senior developer working with AI assistance,
delivering playable value every wave. Waves 4–6 parallelize well if a second contributor
joins (AURORA and ORACLE are deliberately decoupled).

## Estimation assumptions

- One senior Rust developer, full-time-ish, heavily AI-assisted (this tooling).
- Upstream merges continue monthly (GitlabMasterMerger skill); every workstream must
  minimize diff surface against upstream files to keep merges cheap (prefer new files/
  crates and additive RON fields).
- Estimates are 50th percentile; multiply by 1.5 for planning commitments.
- LLM-at-runtime features (AURORA phase 7, ORACLE narrative) assume an external or local
  model endpoint; cost/latency engineering is inside those specs.

## Tooling created for this program

Skills (in `.claude/skills/`):

| Skill | Use for |
|---|---|
| `veloren-progression` | Levels, classes, races, XP, equipment requirements |
| `veloren-abilities` | New spells/abilities/buffs/magic schools (RON pipeline) |
| `veloren-lore` | Writing canon lore content, naming, in-game delivery |
| `veloren-aurora` | AURORA social-simulation implementation |
| `veloren-oracle` | ORACLE world-director implementation |
| `veloren-engine-perf` | Profiling, optimization, memory-safety work |

Subagents (in `.claude/agents/`):

| Agent | Role |
|---|---|
| `rust-perf-reviewer` | Detailed perf/memory review of diffs (hot loops, allocs, contention) |
| `ecs-design-reviewer` | Reviews new components/systems against specs-ECS patterns |
| `game-balance-designer` | Numeric design: curves, costs, scaling tables |
| `lore-writer` | Original, canon-consistent lore content |
| `sim-systems-engineer` | rtsim/AURORA/ORACLE systems implementation |

Existing skills that remain mandatory: `veloren-dev` (any gameplay code),
`veloren-debug`, `veloren-review` (before merging), `veloren-run`, `veloren-telemetry`,
`veloren-worldgen`, plus the superpowers process skills (TDD, writing-plans,
subagent-driven development).

## Working process per milestone

1. Pick the next milestone from the wave table; read its design spec.
2. Author the implementation plan via `superpowers:writing-plans` into
   `docs/superpowers/plans/` (the character-levels plan is the template).
3. Execute via `superpowers:subagent-driven-development`, using the domain skill
   (`veloren-progression`, etc.) and TDD.
4. Review: `veloren-review` + dispatch `rust-perf-reviewer` and `ecs-design-reviewer`
   on the diff.
5. Verify in-game via `veloren-run`; analyze sessions via `veloren-telemetry`.
6. Merge to `development`; keep upstream merge cadence.

## Program-level risks

| Risk | Impact | Mitigation |
|---|---|---|
| Upstream divergence makes merges painful | High | Additive changes, new files/crates, monthly merges, GitlabMasterMerger |
| Balance debt (levels × classes × zones interact) | Medium | `game-balance-designer` agent owns one shared spreadsheet of curves; balance pass per wave |
| AURORA/ORACLE scope creep | High | Phase gates: each phase ships observable behavior or it doesn't merge |
| LLM runtime cost/latency | Medium | LLM never in tick path; template fallbacks; batch + cache (see AURORA §AI architecture) |
| Solo-dev burnout on XL items | High | Waves end in playable outcomes; planes & LLM phases are explicitly deferrable |
| rtsim save-format breakage | Medium | Versioned rtsim Data migrations; soak tests on copies of real saves |

## Success metrics

- Wave 1: level visible and correct for all existing characters (no migration), zero
  upstream-file conflicts on next merge, tracy baseline captured.
- Wave 2–3: new character funnel completes class pick → first spell → first leveled zone
  kill without dev intervention; clippy/CI stays green.
- Wave 4–6: headless server soak (72 h) with 5k+ rtsim NPCs at <10 ms median rtsim tick;
  one organic quest chain and one world event observed end-to-end in telemetry.
