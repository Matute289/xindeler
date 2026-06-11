# RPG Evolution — Master Task Board

**Date:** 2026-06-11
**Program:** [Master roadmap](../specs/2026-06-10-rpg-evolution-master-roadmap.md)
**Total:** 134 tasks across 9 task files (below). Source of truth for each task's steps/code is its implementation plan; task files add routing metadata only.

## How to execute a task (any model)

1. Open the task file, pick the lowest-ID unblocked task assigned to your tier (or the task you were given).
2. Read its **Source plan** section fully; run any anchor-verification greps first.
3. Invoke the domain skill named in the plan (`veloren-progression`, `veloren-abilities`, `veloren-lore`, `veloren-aurora`, `veloren-oracle`, `veloren-engine-perf`) plus `superpowers:test-driven-development`.
4. Follow the plan steps verbatim; verify with the task's **Acceptance** commands; commit with the task's message.
5. **Escalation rule:** if acceptance fails twice, escalate one model tier (haiku→sonnet→opus→fable) and leave a note in the task file. If a contract grep fails (anchors moved), stop and escalate to fable for re-planning.

## Model routing policy (summary)

| Tier | Use for |
|---|---|
| haiku | Mechanical fully-specified edits: RON/ftl content already written in the plan, changelog, match arms, running documented checks/benchmarks |
| sonnet | Standard TDD tasks with real code in the plan, multi-file wiring, compiler-driven resolution |
| opus | Save/DB migrations, persistence converters, netcode/synced components, new ability variants/states, worldgen spawn pipeline, all rtsim Data fields, perf work needing measurement judgment, LLM plumbing |
| fable | Decision points, balance tables, canon/lore prose (via `lore-writer` agent + human curation), phase-gate reviews, re-planning when anchors moved |

## Asset policy

- **Claude creates inline:** all texts (.ftl, lore markdown), all RON configs (abilities, skillsets, entity configs, loot tables, `canon.ron`, `predation.ron`).
- **Icons/particles/voxel models:** reuse/recolor existing assets (paths given per task); genuinely new .vox models are fable decision points (programmatic generation or human-made — decide per case).
- **Audio:** no downloads currently needed (existing SFX reused via Outcome paths). If a future task needs new audio: CC0/CC-BY-SA-compatible from freesound.org or opengameart.org, attribution documented in the repo's credits file.
- **Tools:** Tracy viewer for engine tasks (`brew install tracy`); LLM endpoint for AURORA P7 / ORACLE P6 is `Disabled/Local{url}/Remote{model}` config — live-endpoint choice is a fable decision point.

## Task files (execution order = wave order)

| # | File | Tasks | haiku/sonnet/opus/fable | Status / notes |
|---|---|---|---|---|
| 1 | [01-character-levels-m2-tasks.md](01-character-levels-m2-tasks.md) | 6 | 4/2/0/0 | Unblocked (M1 merged) |
| 2 | [02-equipment-restrictions-tasks.md](02-equipment-restrictions-tasks.md) | 9 | 2/7/0/0 | Phase A unblocked; B7–B8 gated on CLS-1 |
| 3 | [03-classes-races-tasks.md](03-classes-races-tasks.md) | 12 | 4/4/4/0 | Unblocked |
| 4 | [04-magic-abilities-tasks.md](04-magic-abilities-tasks.md) | 16 | 2/8/5/1 | P4.15 gated on CLS-1 |
| 5 | [05-world-difficulty-zones-tasks.md](05-world-difficulty-zones-tasks.md) | 18 | 2/7/8/1 | T9 gated on CLS-1; P4.x DEFERRED (planes) |
| 6 | [06-lore-cosmology-tasks.md](06-lore-cosmology-tasks.md) | 12 | 5/0/0/7 | Unblocked; prose = fable + human curation |
| 7 | [07-engine-improvements-tasks.md](07-engine-improvements-tasks.md) | 9 | 3/4/1/1 | Unblocked, parallel to all |
| 8 | [08-project-aurora-tasks.md](08-project-aurora-tasks.md) | 35 | 0/8/25/2 | Phases sequential; WorldFact reads via ORC-P3.8 API |
| 9 | [09-project-oracle-tasks.md](09-project-oracle-tasks.md) | 17 | 0/4/12/1 | Phases sequential; P3.8 reconciles AURORA contract |
| | **Total** | **134** | **22/44/55/13** | |

## Cross-file dependency edges

- EQ-B7, EQ-B8 → CLS-1 (`ClassKind` must exist; hard grep gate with skip-to-finish fallback).
- MAG-P4.15 → CLS-1 (class skill trees). WDZ-T9 → CLS-1 (NPC class mapping).
- AUR-P5.5 ↔ ORC-P3.8 (coup sanctioning reads WorldFacts via ORACLE's read-only API; ORC-P3.8 must reconcile the AURORA plan contract before implementing).
- ORC-P3.11 → WDZ (level bands) + MAG (affix abilities). ORC-P6.14 → LORE (canon index).
- AURORA/ORACLE phase N+1 branches only after phase N merges; every contract task re-greps its anchors first.

## Wave mapping (from the roadmap)

| Wave | Task files |
|---|---|
| 1 | 01, 07 (ENG-1..4 baseline), 02 Phase A |
| 2 | 03, 04 (P1–P2), 06 (T1–T6) |
| 3 | 05 (T1–T14), 04 (P3–P4), 06 (T7–T12), 02 Phase B |
| 4 | 08 (P1–P4) |
| 5 | 09 (P1–P4), 08 (P5–P6) |
| 6 | 09 (P5–P8), 08 (P7–P8), 05 P4.x (planes) |
