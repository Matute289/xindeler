# PROJECT AURORA — Task Board

**Source plan:** [../plans/2026-06-11-project-aurora.md](../plans/2026-06-11-project-aurora.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> If acceptance fails twice, escalate one model tier and leave a note in the task file.

Conventions (every task): tests via `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim`; invoke `veloren-aurora` + `superpowers:test-driven-development` before code; determinism — no RNG or `ChaChaRng` seeded from `npc.seed`/world state, never `rand::rng()`; every persisted field gets `#[serde(default)]`, a fixture assertion, and a stated byte budget; `Npc` has a **manual `Clone` impl** (`rtsim/src/data/npc.rs:341-366`) — every new `Npc` field must extend it. Cross-file: any AURORA code reading ORACLE `WorldFact`s must go through the read API landed by ORC-P3.8 (file 09) — read-only, never `&mut`.

## Phase 1 — Foundations (branch `feature/aurora-phase1` off `development`)

## AUR-P1.1 — Branch + pre-AURORA save-compatibility fixture

- **Model:** opus — rtsim save-compat work (policy: all save-compat/Data-field work goes to opus); this fixture is the safety net for every later persisted field.
- **Depends on:** none (first AURORA task).
- **Branch / commit:** `feature/aurora-phase1` off `development`; commit `test: pre-AURORA Npc save-compat fixture harness`.
- **Files:** Create: `rtsim/tests/save_compat.rs`, `rtsim/tests/fixtures/npc_pre_aurora.dat` (generated binary, committed).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–4 verbatim. Trap: the `generate_pre_aurora_fixture` test is `#[ignore]`d and must NEVER be re-run after AURORA fields land — the point is the bytes predate them.
- **Acceptance:** `VELOREN_ASSETS=... cargo test -p veloren-rtsim --test save_compat -- --ignored generate_pre_aurora_fixture` PASS and fixture exists; then `... --test save_compat pre_aurora_npc_still_loads` PASS.
- **Size:** S

## AUR-P1.2 — `Mind` component: types, seeded generation, `Npc` wiring, migrate re-seed

- **Model:** opus — persisted `Npc` field + migrate-rule re-seed (save-compat critical); deterministic seeding semantics.
- **Depends on:** AUR-P1.1 (fixture harness).
- **Branch / commit:** `feature/aurora-phase1`; commit `feat(aurora): persisted Mind component with seeded generation and migrate re-seed`.
- **Files:** Create: `rtsim/src/data/mind.rs`. Modify: `rtsim/src/data/mod.rs:1-19`, `rtsim/src/data/npc.rs:299-305` (field), `:341-366` (Clone), `:372-395` (`Npc::new`), `:400-403` (`with_personality`), `rtsim/src/rule/migrate.rs` (re-seed), `rtsim/tests/save_compat.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–6 verbatim (TDD). Traps: `Personality` fields are private — bias only via `Personality::is(PersonalityTrait)`; budget ≤128 B/NPC enforced by test — if it fails, shorten rename keys, do not raise the budget; `is_unseeded()` works because `seeded` value range starts at 16.
- **Acceptance:** `cargo test -p veloren-rtsim mind` 3 PASS; `--test save_compat` PASS (proves `serde(default)` against real pre-change bytes); `cargo check -p veloren-rtsim` clean.
- **Size:** M

## AUR-P1.3 — Short-term memory + perception/mood wiring at event sources

- **Model:** sonnet — TDD with code in plan; STM is `#[serde(skip)]` (no save-compat surface); wiring is compile-verified.
- **Depends on:** AUR-P1.2 (`Mind`/`Mood` exist).
- **Branch / commit:** `feature/aurora-phase1`; commit `feat(aurora): short-term perception memory + mood wiring from witnessed events`.
- **Files:** Create: `rtsim/src/data/memory.rs` (STM half only; LTM is AUR-P1.5). Modify: `rtsim/src/data/mod.rs`, `rtsim/src/data/npc.rs` (`#[serde(skip)] pub stm` + Clone), `rtsim/src/rule/report.rs:19-48,50-76`, `rtsim/src/rule/cleanup.rs:5-9,25-30`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–6 verbatim. Traps: do NOT feed STM at the npc_ai inbox take (`rule/npc_ai/mod.rs:135`) — borrow conflict verified; perceptions are pushed at the source in `rule/report.rs`. Mood decay in cleanup is deterministic (seed+tick stagger), neurotic NPCs at half rate.
- **Acceptance:** `cargo test -p veloren-rtsim memory` PASS; `cargo check -p veloren-rtsim` clean; `--test save_compat` PASS; `grep -n "stm.push\|on_witnessed\|MOOD_DECAY_TICK_SKIP" rtsim/src/rule/report.rs rtsim/src/rule/cleanup.rs` ≥ 5 hits.
- **Size:** M

## AUR-P1.4 — Persisted NPC names

- **Model:** opus — persisted `Npc` field (save-compat policy), though small.
- **Depends on:** AUR-P1.2 (field ordering after `mind`; fixture test pattern).
- **Branch / commit:** `feature/aurora-phase1`; commit `feat(aurora): persisted NPC names with seed-generated fallback`.
- **Files:** Modify: `rtsim/src/data/npc.rs` (field + Clone + `Npc::new` + `with_name` builder; `get_name` at `:431-439`), `rtsim/tests/save_compat.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 4' steps 1–5 verbatim. Budget: 0 B for existing NPCs (`None` ≈ 1 B); ≤ 24 B when set. Resolves the TODO at `npc.rs:431`.
- **Acceptance:** `cargo test -p veloren-rtsim --test save_compat` PASS (incl. `assert_eq!(npc.name, None)` on the old fixture and the override test); `cargo check -p veloren-rtsim` clean.
- **Size:** S

## AUR-P1.5 — Long-term episodic memory with salience consolidation

- **Model:** opus — persisted `Npc` field with cap/budget enforcement and deterministic forgetting semantics (persisted `last_decay`).
- **Depends on:** AUR-P1.3 (STM + `Perception`), AUR-P1.4 (field order).
- **Branch / commit:** `feature/aurora-phase1`; commit `feat(aurora): long-term episodic memory with salience consolidation and forgetting`.
- **Files:** Modify: `rtsim/src/data/memory.rs` (LTM + `consolidate`), `rtsim/src/data/npc.rs` (field + Clone + `Npc::cleanup` at `:456-467`), `rtsim/src/rule/cleanup.rs:56-60` (pass `time_of_day`), `rtsim/tests/save_compat.rs`, `rtsim/src/data/mod.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 5' steps 1–5 verbatim. Budget ≤ 1024 B/NPC at the 24-episode cap (the spec's 672 B assumed tighter `Actor` encoding — plan's deliberate divergence). No RNG; forgetting gates on the persisted `last_decay` timestamp so it is save/load- and cadence-independent. Same-(kind, actors) episodes refresh, not duplicate; eviction only if strictly weaker.
- **Acceptance:** `cargo test -p veloren-rtsim memory` 4 PASS (Tasks 3+5 combined); `--test save_compat` PASS; `cargo check -p veloren-rtsim` clean.
- **Size:** M

## AUR-P1.6 — Phase 1 gate: budget enforcement, sentiment re-cap, lint, changelog

- **Model:** fable — phase-gate review; contains a hard decision point (byte-ceiling breach ⇒ stop for review, never raise the ceiling) and the phase-2 branching decision.
- **Depends on:** AUR-P1.1–P1.5 (all of Phase 1).
- **Branch / commit:** `feature/aurora-phase1`; commit `feat(aurora): phase 1 budget gate — sentiment re-cap and NPC byte ceiling`.
- **Files:** Modify: `rtsim/src/data/sentiment.rs:13` (`NPC_MAX_SENTIMENTS` 128 → 64), `rtsim/tests/save_compat.rs` (whole-NPC budget test), `CHANGELOG.md`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 6' steps 1–4 verbatim. Trap: if `fully_loaded_npc_fits_byte_ceiling` exceeds 4096 B, print the size and STOP for review — do not raise the ceiling (2 KB p95 is a runtime target tracked in soak telemetry, the 4 KB ceiling is the hard guard). Run CI-identical clippy (full feature string from CLAUDE.md) + `cargo fmt --all -- --check`. Then invoke `veloren-review` and `superpowers:finishing-a-development-branch`; record in the merge/PR description whether Phase 2 continues on this branch or on `feature/aurora-phase2`.
- **Acceptance:** Budget test PASS; full `cargo test -p veloren-rtsim` PASS after the re-cap; clippy + fmt clean; changelog entry added; branch decision recorded.
- **Size:** M

## Phase 2 — Social Graph (same branch if kept open per AUR-P1.6 decision, else `feature/aurora-phase2` off `development` after Phase 1 merges)

## AUR-P2.7 — Typed relationship edges on `Npc`

- **Model:** opus — persisted `Npc` field with cap/eviction/structural-protection invariants and byte budget.
- **Depends on:** AUR-P1.6 (after Phase 1 gate / per branch decision).
- **Branch / commit:** per AUR-P1.6 decision; commit `feat(aurora): typed durable relationship edges with capped ego-adjacency`.
- **Files:** Create: `rtsim/src/data/relationship.rs`. Modify: `rtsim/src/data/mod.rs`, `rtsim/src/data/npc.rs` (field + Clone + `Npc::new`), `rtsim/tests/save_compat.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 7' steps 1–5 verbatim. Budget ≤ 768 B/NPC at the 16-edge cap. Edges are durable — **no stochastic decay**; structural edges (Kinship, Marriage) are exempt from cap-pressure eviction and floor at strength 1.
- **Acceptance:** `cargo test -p veloren-rtsim relationship` 2 PASS; `--test save_compat` PASS (`npc.relationships.len() == 0` on old fixture).
- **Size:** M

## AUR-P2.8 — Sentiment introspection + reputation query

- **Model:** sonnet — TDD with full code in plan; no persistence (reputation is always derived, never stored).
- **Depends on:** AUR-P2.7 (plan order; no hard code dependency).
- **Branch / commit:** Phase 2 branch; commit `feat(aurora): derived reputation queries over sentiments`.
- **Files:** Modify: `rtsim/src/data/sentiment.rs` (`Sentiment::value` private→pub at `:155`; `Sentiments::iter`; `reputation_of` free fn + tests), `rtsim/src/data/mod.rs` (`Data::reputation_at_site`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 8' steps 1–5 verbatim. Trap: `reputation_at_site` sorts residents by NPC uid before sampling — `population` is a HashSet, sorting keeps the sample deterministic.
- **Acceptance:** `cargo test -p veloren-rtsim sentiment` existing + new PASS; `cargo check -p veloren-rtsim` clean.
- **Size:** S

## AUR-P2.9 — `social` rule: edge consolidation, symmetry, integrity

- **Model:** sonnet — full rule code in plan; deterministic, RNG-free; borrow-order note provided.
- **Depends on:** AUR-P2.7 (edges), AUR-P2.8 (`Sentiments::iter`), AUR-P1.5 (LTM `involving`).
- **Branch / commit:** Phase 2 branch; commit `feat(aurora): social rule consolidating sentiments+episodes into symmetric durable edges`.
- **Files:** Create: `rtsim/src/rule/social.rs`. Modify: `rtsim/src/rule/mod.rs:1-8`, `rtsim/src/lib.rs:201-208` (register between `SimulateNpcs` and `NpcAi`, so brains see fresh edges).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 9' steps 1–5 verbatim. Spec mapping: +8 strength per qualifying pass, durable at 32 after 4 passes; young unsustained edges fade −4/pass; rivalry requires a negative-valence grievance episode. Borrow trap (pass 3): if collecting `live` trips the borrow checker, collect it at the top of the closure. LOD: runs for all NPCs at stagger regardless of `SimulationMode`.
- **Acceptance:** `cargo test -p veloren-rtsim social` 3 PASS; `cargo check -p veloren-rtsim` clean (registration compiles).
- **Size:** M

## AUR-P2.10 — Memory-aware dialogue: NPCs reference shared episodes

- **Model:** sonnet — dialogue wiring with code in plan + i18n asset + in-game verification.
- **Depends on:** AUR-P1.5 (LTM), AUR-P2.9 (consolidation cadence used in the in-game check).
- **Branch / commit:** Phase 2 branch; commit `feat(aurora): NPCs reference remembered episodes in dialogue`.
- **Files:** Modify: `rtsim/src/rule/npc_ai/dialogue.rs:5-36` (reminisce response in `general`) and near `:446` (`reminisce` fn), `assets/voxygen/i18n/en/dialogue.ftl`.
- **Assets:** i18n text — 7 Fluent keys (`dialogue-question-reminisce`, `npc-dialogue-reminisce_{helped,death,theft,good,bad,nothing}`) in `assets/voxygen/i18n/en/dialogue.ftl`; Claude creates inline, exact strings given in the plan; match the file's section style.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 10' steps 1–5 verbatim. Read-only in dialogue: `ctx.npc` is `&Npc`; rehearsal-refresh happens in consolidation, not here.
- **Acceptance:** `cargo test -p veloren-rtsim reminisce` PASS; `cargo check` clean. In-game (`veloren-run` skill): witnessed death near an NPC → after ≥ ~4 s the "Do you remember…" option appears with an episode-matching reply; a fresh NPC does NOT show the option (logging-verbose + `veloren-telemetry` confirms the branch).
- **Size:** M

## AUR-P2.11 — Phase 2 gate: full suite, lint, changelog, finish

- **Model:** fable — phase-gate review and merge (first shippable milestone: NPCs visibly remember players).
- **Depends on:** AUR-P2.7–P2.10.
- **Branch / commit:** Phase 2 branch; commit `docs: changelog entry for AURORA phase 2 social graph`; merge into `development`.
- **Files:** Modify: `CHANGELOG.md`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 11' steps 1–3 verbatim: `cargo test -p veloren-rtsim -p veloren-common`; CI-identical clippy + voxygen publish-profile clippy + fmt check; changelog; `veloren-review` then `superpowers:finishing-a-development-branch`.
- **Acceptance:** All tests PASS incl. save-compat fixture with all phase-1/2 assertions; lints/format clean; branch merged.
- **Size:** S

## Phase 3 — Families (branch `feature/aurora-phase3` off `development` after Phase 2 merges; contract-level tasks)

Universal constraints for Phases 3–8: every persisted field `#[serde(default)]` + `save_compat.rs` fixture assertion (generate per-phase fixtures for `Site`/`Data` exactly as AUR-P1.1 did for `Npc`); every store states and tests a byte budget; new rules use seeded `ChaChaRng` or no RNG; new behavior defines loaded-vs-simulated semantics.

## AUR-P3.1 — Birth time, age, life stages

- **Model:** opus — persisted `Npc` field + `WorldSettings` change in `common` (save-compat/Data-field policy).
- **Depends on:** AUR-P2.11 merged (phase boundary).
- **Branch / commit:** `feature/aurora-phase3`; commit message not fixed by plan — use `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/data/npc.rs` (`#[serde(default)] pub birth_tod: Option<TimeOfDay>` — `None` = pre-AURORA adult), `common/src/rtsim.rs` (`WorldSettings` gains `pub year_secs: f64`, default `18.0 * 3600.0`, server-tunable), `rtsim/tests/save_compat.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Then TDD the interfaces: `Npc::age_years`, `Npc::life_stage`, `enum LifeStage { Child, Adult, Elder }` (Child < 16y, Elder ≥ 60y; `None` ⇒ Adult).
- **Acceptance:** Stage-boundary unit tests PASS; fixture asserts `birth_tod == None`; budget ≤ 10 B/NPC; existing saves load; all NPCs report Adult until births occur.
- **Size:** M

## AUR-P3.2 — `lifecycle` rule: births and old-age death

- **Model:** opus — multi-system (new rtsim event, architect interplay, population invariants, soak verification).
- **Depends on:** AUR-P3.1; AUR-P2.7 (Marriage edges gate births).
- **Branch / commit:** `feature/aurora-phase3`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/rule/lifecycle.rs` (register after `social`, before `npc_ai`). Modify: `rtsim/src/event.rs` (`pub struct OnBirth { child, parents: [NpcId; 2], site }`, `type SystemData<'a> = ();`), `rtsim/src/lib.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. `try_birth` gated on Marriage edge + home-site population below `wanted_population` ceiling; old-age death emits the **existing** `OnDeath` (no second death path); RNG `ChaChaRng::seed_from_u64(npc.seed as u64 ^ day_index)`. LOD: identical simulated/loaded; loaded NPCs additionally `NpcAction::Say` (presentation only).
- **Acceptance:** Deterministic decisions for fixed seed+day; population ≤ ceiling over 1000 simulated days on a synthetic site; soak (`veloren-telemetry` `"life"` channel) shows stable population, no architect double-spawn.
- **Size:** L

## AUR-P3.3 — Genetics and kinship edges

- **Model:** opus — touches private `Personality` internals in `common` plus NPC creation and structural-edge guarantees.
- **Depends on:** AUR-P3.2 (births), AUR-P1.4 (persisted names for surnames), AUR-P1.2 (`Mind::seeded`).
- **Branch / commit:** `feature/aurora-phase3`; `feat(aurora): ...` convention.
- **Files:** Modify: `common/src/rtsim.rs` (`Personality::blend` — lives in `common` because fields are private), `rtsim/src/rule/lifecycle.rs` (`child_of`: body-param blend + jitter, `Mind::seeded` then value-inheritance at half strength, surname from a parent, `birth_tod = Some(now)`; kinship `EdgeKind::Kinship` upserts on child/parents/siblings).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Kinship edges are structural — never evicted (AUR-P2.7 guarantee).
- **Acceptance:** Same seed+parents ⇒ identical child; kinship symmetric; structural exemption holds at the 16-edge cap; 3-generation families by soak day 60 (spec metric).
- **Size:** M

## AUR-P3.4 — Architect ceiling refactor

- **Model:** opus — changes the core population-replenishment semantics of the architect (regression risk across rtsim).
- **Depends on:** AUR-P3.2 (births are the new replenishment path).
- **Branch / commit:** `feature/aurora-phase3`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/rule/architect.rs` — for `Role::Civilised`, respawn becomes a **floor** repair (force-spawn only below 50% of wanted); `Wild`/`Monster` unchanged.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Spawn-decision function tests: no spawn at 100%/75%, spawn at 40%; soak: no unrelated replacement NPCs for civilised deaths; recovery from a simulated 60% cull.
- **Size:** M

## AUR-P3.5 — Coins, deeds, inheritance

- **Model:** opus — persisted fields on both `Npc` and `Site` + plot-id re-link/orphan cleanup in `migrate` (save-compat heavy).
- **Depends on:** AUR-P3.3 (kinship edges drive inheritance order), AUR-P3.1 (`birth_tod` for eldest-child ordering).
- **Branch / commit:** `feature/aurora-phase3`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/data/npc.rs` (`#[serde(default)] pub coins: u32`, ≤ 5 B), `rtsim/src/data/site.rs` (`deeds: Vec<Deed>`; `Deed { plot, owner, kind }`, `DeedKind { Home, Shop, Farm }`), `rtsim/src/rule/lifecycle.rs` (binds `OnDeath`: spouse → eldest child → sibling → site treasury), `rtsim/src/rule/migrate.rs` (orphan cleanup), `rtsim/tests/save_compat.rs` + new `rtsim/tests/fixtures/site_pre_phase3.dat`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Plot ids re-linked at load like `world_site`. Budget: deeds on `Site` (≈ 30 B/deed, ≤ 64/site), not on NPCs.
- **Acceptance:** Inheritance conserves total coins (property test over random family shapes); deed orphan cleanup tested; `site_pre_phase3.dat` fixture loads with `deeds == []`.
- **Size:** L

## Phase 4 — Economy (branch `feature/aurora-phase4` after Phase 3 merges)

## AUR-P4.1 — `SiteEconomy` persisted state

- **Model:** opus — persisted `Site` field with sparse serializers + migrate seeding (save-compat policy).
- **Depends on:** AUR-P3.5 merged (phase boundary; site fixture pattern exists).
- **Branch / commit:** `feature/aurora-phase4`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/data/economy.rs` (`SiteEconomy { stock, demand, price_mult }` as `EnumMap<Good, f32>` using the sparse `rugged_ser_enum_map` serializers at `rtsim/src/data/mod.rs:122-166`). Modify: `rtsim/src/data/site.rs` (`#[serde(default)] pub economy`), `rtsim/src/rule/migrate.rs` (seed from worldgen snapshot when all-default).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Budget ≤ 600 B/site (3 maps × ~16 active goods; ~10² sites ⇒ ≤ 60 KB).
- **Acceptance:** Sparse serde round-trip; seeding from a synthetic snapshot; site fixture loads default economy.
- **Size:** M

## AUR-P4.2 — `economy` rule: production, consumption, pricing

- **Model:** sonnet — self-contained deterministic rule; formulas and clamps fully specified in the plan (no parameter choices left open).
- **Depends on:** AUR-P4.1.
- **Branch / commit:** `feature/aurora-phase4`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/rule/economy.rs` (register after `npc_ai`). Modify: `rtsim/src/lib.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. In-game-daily per site staggered by `site.seed`; production from profession census; `price_mult = clamp((demand/supply).powf(0.5), 0.25, 4.0)` smoothed `0.9*old + 0.1*new`. No RNG. LOD: site-level aggregates ARE the far-ring economy — identical for all sites.
- **Acceptance:** Price rises under shortage and recovers; bounded after 10k random shocks (proptest); two identical runs ⇒ identical state.
- **Size:** M

## AUR-P4.3 — Live prices into player trade

- **Model:** opus — multi-system: server tick merchant stocking + `common/src/trade.rs` pricing + anti-exploit server const.
- **Depends on:** AUR-P4.2.
- **Branch / commit:** `feature/aurora-phase4`; `feat(aurora): ...` convention.
- **Files:** Modify: `server/src/rtsim/tick.rs` (merchant stocking consumes `SiteEconomy.stock`/`price_mult` instead of frozen `SiteInformation` — the `// economy isn't economying sometimes` hack site; thread `price_mult` into `SitePrices` used by `common/src/trade.rs::balance`). No voxygen change.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Stocked-merchant `SitePrices` reflect `price_mult`; manual `veloren-run` check: bulk-buying raises the price within one in-game day (spec metric); anti-exploit per-player daily trade-volume cap per site (server const) documented and tested.
- **Size:** L

## AUR-P4.4 — Merchant cargo and utility routes

- **Model:** opus — persisted `Npc` field + npc_ai routing + economy transfers + death-drop hook (multi-system).
- **Depends on:** AUR-P4.2 (price spreads), AUR-P3.5 (`npc.coins`).
- **Branch / commit:** `feature/aurora-phase4`; `feat(aurora): ...` convention; phase ends with a P1.6/P2.11-style gate (full suite, CI lint, changelog, `veloren-review`, finishing-a-development-branch).
- **Files:** Modify: `rtsim/src/data/npc.rs` (`#[serde(default)] pub cargo: Option<(Good, f32)>`, ≤ 16 B), `rtsim/src/rule/npc_ai/mod.rs` (merchant branch of `adventure()` picks destination by `max((price_mult_dst − price_mult_src) / distance)` over `nearby_sites_by_size`, deterministic site-id tiebreak; buy-on-departure / sell-on-arrival; death drops cargo via existing `OnDeath`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Route choice on synthetic spreads; goods conservation across a completed route; killed merchant loses cargo without duplication.
- **Size:** L

## Phase 5 — Organizations (branch `feature/aurora-phase5` after Phase 4 merges)

## AUR-P5.1 — `Organization` entity + `Data.organizations`

- **Model:** opus — new persisted top-level `Data` map + new slotmap key in `common` (Data-field policy).
- **Depends on:** AUR-P4.4 merged (phase boundary).
- **Branch / commit:** `feature/aurora-phase5`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/data/organization.rs`. Modify: `common/src/rtsim.rs` (slotmap key `OrgId` beside `FactionId`), `rtsim/src/data/mod.rs` (`#[serde(default)] pub organizations: Organizations`), `rtsim/tests/save_compat.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Struct per plan: `OrgKind` (spec's 8 variants), `Governance { Autocratic, Council, Elective }`, `Membership { rank, standing, joined }`, goals cap 4, roster capped at 128 listed members (larger orgs: count + sampled roster). Budget ≤ 4 KB/org; ≤ 10³ orgs expected.
- **Acceptance:** Serde round-trip; member-cap enforcement; old fixtures unaffected (new top-level map defaults empty).
- **Size:** L

## AUR-P5.2 — `organizations` rule: founding, ranks, dissolution

- **Model:** opus — multi-system (new rule + additive `ReportKind` variant feeding gossip).
- **Depends on:** AUR-P5.1; AUR-P1.2 (`Mind.values`/`Goal` gate founding).
- **Branch / commit:** `feature/aurora-phase5`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/rule/organizations.rs` (in-game daily per org, staggered by org-id hash; seeded `ChaChaRng`). Modify: `rtsim/src/data/report.rs` (extend `ReportKind` with `OrgEvent { org, kind }` — additive variant), `rtsim/src/lib.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Founding scan per the spec table (≥ N same-profession NPCs in one site + founder with matching `Goal`/`Mind.values` + 2 willing co-founders); dissolution on treasury ≤ 0, leader death without succession, or members < 3 for an in-game month.
- **Acceptance:** Founding fires for a synthetic 6-blacksmith site, not for 2; each dissolution path tested; determinism.
- **Size:** L

## AUR-P5.3 — GOAP planner for org goals only

- **Model:** sonnet — self-contained pure-algorithm module (~300 lines, zero new deps, A* over action indices) with crisp tests.
- **Depends on:** AUR-P5.2 (sole consumer).
- **Branch / commit:** `feature/aurora-phase5`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/ai/goap.rs` (`WorldFacts` bitset+numeric, `trait OrgAction { preconditions/apply/cost }`, `fn plan(start, goal, actions, max_depth) -> Option<Vec<usize>>`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Used **only** by the organizations rule — per-NPC GOAP explicitly rejected (spec §AI(c)); do not wire it anywhere else.
- **Acceptance:** Plans a 3-step `Monopolize(Iron)` toy domain; `None` when unsatisfiable; depth cap respected.
- **Size:** M

## AUR-P5.4 — Faction → Organization migration (staged behind alias)

- **Model:** opus — save-data migration of a load-bearing field (count call sites first; no data deletion).
- **Depends on:** AUR-P5.1.
- **Branch / commit:** `feature/aurora-phase5`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/rule/migrate.rs` (per `Faction`, create a `PoliticalFaction` org mirroring leader/sentiments; record `faction_id → org_id`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first (`grep -rn "\.faction" rtsim/src/rule/architect.rs rtsim/src/rule/npc_ai/ | wc -l` — faction is load-bearing); if anchors moved, escalate to fable for re-planning before implementing. `Npc::faction`/`Site::faction` STAY as deprecated aliases for one release; only new AURORA systems read orgs; architect and existing AI keep reading `faction`.
- **Acceptance:** Migrated org count == faction count; idempotent on re-run (second setup creates nothing).
- **Size:** M

## AUR-P5.5 — Governance dynamics: succession and elections

- **Model:** opus — multi-system (organizations + social hooks + kinship-driven succession) and the ORACLE integration boundary.
- **Depends on:** AUR-P5.2, AUR-P3.3 (kinship edges for primogeniture). Cross-file: coup *sanctioning* is ORACLE's job via the integration contract — AURORA only publishes tension telemetry here; any fact reads go through ORC-P3.8's API (file 09). Do not implement coup initiation.
- **Branch / commit:** `feature/aurora-phase5`; `feat(aurora): ...` convention; phase ends with a gate (full suite, CI lint, changelog, review, finish).
- **Files:** Modify: `rtsim/src/rule/organizations.rs` (+ hooks in `rtsim/src/rule/social.rs`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Leader `OnDeath` → primogeniture via kinship edges (noble house) or top rank; `Elective` term expiry → candidacy (members with `Value::Power` ≥ threshold), vote weight `standing × sentiment`, seeded-RNG tiebreak.
- **Acceptance:** Deterministic succession on a fixture family/org; election vote counting tested; contested succession (two similar claimants) emits the quest-seed report consumed by Phase 6.
- **Size:** L

## Phase 6 — Dynamic Quests (branch `feature/aurora-phase6` after Phase 5 merges)

## AUR-P6.1 — New `QuestKind` variants + payload generalization

- **Model:** opus — persisted quest data with serde compatibility and the arbiter monotonic-resolution invariant.
- **Depends on:** AUR-P5.5 merged (phase boundary; contested-succession seeds feed quests).
- **Branch / commit:** `feature/aurora-phase6`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/data/quest.rs` (add `Find`, `Procure`, `Mediate`, `Investigate`; generalize the hardcoded courier `Payload` to carry an `ItemResource`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Additive enum variants only — serde-compatible with old saves.
- **Acceptance:** Serde round-trip per variant; existing quest tests unaffected; arbiter monotonic-resolution property (`AtomicU8` compare-exchange) holds for new kinds.
- **Size:** M

## AUR-P6.2 — `quest_gen` rule: needs → seeds → validation → rewards

- **Model:** opus — multi-system (Mind goals, moods, reports, `SiteEconomy.demand`, `world::civ::Track` reachability, XP rewards).
- **Depends on:** AUR-P6.1; AUR-P4.2 (`SiteEconomy.demand`); external hard dependency: the character-levels spec (XP band) — **already merged** on `development`; verify with the plan's `ls docs/superpowers/specs/ | grep character-levels` anchor.
- **Branch / commit:** `feature/aurora-phase6`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/rule/quest_gen.rs` (in-game daily per site). Modify: `rtsim/src/lib.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Validation: target alive + reachable, items obtainable, arbiter alive and home; **unsolvable seeds are dropped, never patched**; reward = base(danger × distance) × site wealth.
- **Acceptance:** Validation rejects unreachable/dead targets; urgency ordering deterministic; reward monotonic in distance and danger.
- **Size:** L

## AUR-P6.3 — Ten templates wired to dialogue offers

- **Model:** sonnet — template constructors + dialogue/i18n wiring following the existing quest path and the AUR-P2.10 test pattern.
- **Depends on:** AUR-P6.2.
- **Branch / commit:** `feature/aurora-phase6`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/rule/quest_gen.rs` (constructors for the spec's 10-template taxonomy), `rtsim/src/rule/npc_ai/quest.rs` (offer through existing dialogue path + escrow deposits), `assets/voxygen/i18n/en/dialogue.ftl`.
- **Assets:** i18n text — Fluent keys for the 10 quest templates in `assets/voxygen/i18n/en/dialogue.ftl`; Claude writes the strings inline (plan fixes keys/pattern, not prose — keep canon via `veloren-lore` if naming anything).
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Each template yields a valid `Quest` from a synthetic world state; per-template i18n key test mirroring AUR-P2.10's pattern.
- **Size:** M

## AUR-P6.4 — Anti-exploit guards

- **Model:** opus — persisted per-site cooldown map (save-compat) + exploit-surface reasoning; parameters fixed by plan.
- **Depends on:** AUR-P6.3.
- **Branch / commit:** `feature/aurora-phase6`; `feat(aurora): ...` convention; phase ends with a gate.
- **Files:** Modify: `rtsim/src/rule/quest_gen.rs`, `rtsim/src/data/quest.rs` (≤ 3 active generated quests per player; per-template-per-site cooldown — persisted `#[serde(default)]` map on the site, ≤ 200 B/site; expiry via existing `timeout`; abandonment ⇒ arbiter sentiment penalty).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Rate-limit property test; cooldown survives save/load; penalty applied on abandon; reward duplication impossible by construction (existing escrow + monotonic resolution).
- **Size:** M

## Phase 7 — LLM Integration (branch `feature/aurora-phase7` after Phase 6 merges)

## AUR-P7.1 — `TextOracle` trait + `NullOracle`

- **Model:** opus — LLM-integration plumbing (policy) and the resource boundary into rtsim.
- **Depends on:** AUR-P6.4 merged (phase boundary).
- **Branch / commit:** `feature/aurora-phase7`; `feat(aurora): ...` convention.
- **Files:** Create: `rtsim/src/llm.rs` (`TextRequest { template_id, personality_bucket, mood_bucket, facts }`, `TextTicket`, `trait TextOracle { request/poll }`, `NullOracle` — `poll` always `None` ⇒ template fallback). Oracle handed to rtsim as an `RtState` resource via `with_resource`.
- **Assets:** none.
- **Downloads/tools:** none — `NullOracle` requires no endpoint.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Also build the test-only `MockOracle` for AUR-P7.3.
- **Acceptance:** `NullOracle` contract test; **the game is 100% playable with `NullOracle`** (phase acceptance).
- **Size:** M

## AUR-P7.2 — Server bridge with cache and budget

- **Model:** opus — LLM-integration plumbing: worker thread, bounded queue, LRU cache, never-block-the-tick guarantee.
- **Depends on:** AUR-P7.1.
- **Branch / commit:** `feature/aurora-phase7`; `feat(aurora): ...` convention.
- **Files:** Create: `server/src/rtsim/llm_bridge.rs`. Modify: `server/src/settings/mod.rs` (backend enum).
- **Assets:** none.
- **Downloads/tools:** LLM endpoint config exactly as the plan specifies: backend enum `{ Disabled, Local { url }, Remote { model } }` read from `server/src/settings/mod.rs`. The plan defines no concrete provider, model name, env var, or model download — implement and test against `Disabled` + the `MockOracle`; choosing/standing up a live local or remote endpoint is a **fable decision point** (do not invent beyond the plan).
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Bounded queue depth 64 drop-oldest; LRU keyed `(template_id, personality_bucket, mood_bucket, fact_hash)`; 2 s timeout; `poll` is a map lookup, `request` enqueue-only — **no blocking call in any tick path** (debug-assert on elapsed time); hit/miss/drop counters exposed to metrics.
- **Acceptance:** Overflow drops oldest; timeout yields `None`; bucketing collapses similar NPCs to few cache keys (hit-rate test over a synthetic population).
- **Size:** L

## AUR-P7.3 — Dialogue color + org charters consumption

- **Model:** opus — LLM plumbing consumption with an explicit LOD split and a sim-isolation invariant.
- **Depends on:** AUR-P7.2; AUR-P5.2 (org founding requests charters).
- **Branch / commit:** `feature/aurora-phase7`; `feat(aurora): ...` convention; phase ends with a gate.
- **Files:** Modify: `rtsim/src/rule/npc_ai/dialogue.rs` (**LOD split: loaded NPCs only** — simulated NPCs always use templates), `rtsim/src/rule/organizations.rs` (founding requests a one-shot charter cached in `Organization.charter`).
- **Assets:** none.
- **Downloads/tools:** as AUR-P7.2 (MockOracle/Disabled path suffices for all tests).
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** With `MockOracle`: colored line used when ready, template ships verbatim on `None`; charter generated exactly once per org; outputs length-capped (240 chars); LLM text never feeds back into simulation state besides the `charter` string.
- **Size:** M

## Phase 8 — Optimization (branch `feature/aurora-phase8` after Phase 7 merges)

## AUR-P8.1 — Statistical far ring

- **Model:** opus — divergent-LOD demotion/promotion across lifecycle/social/economy rules with quest-safety invariants.
- **Depends on:** AUR-P7.3 merged (phase boundary); touches AUR-P3.2/P2.9/P4.2 rules.
- **Branch / commit:** `feature/aurora-phase8`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/src/rule/simulate_npcs.rs` + the `lifecycle`/`social`/`economy` rules.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Sites without player presence > 1 in-game day demote to aggregate updates; promotion lazily reconciles individuals. Off-screen whitelist (spec Open Question 6): marriages/births/prices **yes**; deaths of arbiters with active quests **no** (`Quests::related_to`).
- **Acceptance:** Demote → 30 days → promote yields population/edge counts within ±10% of a fully-simulated reference; no active-quest arbiter dies off-screen (property test).
- **Size:** L

## AUR-P8.2 — Criterion benches + tick budgets

- **Model:** sonnet — bench-harness additions over synthetic 10k-NPC `Data` with thresholds fixed by the plan.
- **Depends on:** AUR-P8.1 (benches measure the post-far-ring rules).
- **Branch / commit:** `feature/aurora-phase8`; `feat(aurora): ...` convention.
- **Files:** Modify: `rtsim/Cargo.toml` (`[dev-dependencies] criterion` + `[[bench]]` entries). Create: `rtsim/benches/social_tick_10k.rs`, `rtsim/benches/consolidation_10k.rs`, `rtsim/benches/economy_50_sites.rs`, `rtsim/benches/data_clone_serialize_10k.rs`, `rtsim/benches/quest_gen_validation.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first (incl. `grep -n "save" server/src/rtsim/tick.rs | head` for the 60 s background-save guard); if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Thresholds (dev profile, spec §Scale): social ≤ 0.3 ms/tick-slice; consolidation ≤ 0.2 ms; economy ≤ 0.3 ms amortized; full `Data` clone+serialize ≤ 250 ms; CI manual-dispatch job alerts at +20%.
- **Size:** M

## AUR-P8.3 — Save-clone optimization (conditional)

- **Model:** opus — `Arc`-wrap copy-on-write over `Data` sub-maps; memory/perf judgment + byte-identical-serialization invariant. **Only run if AUR-P8.2's `data_clone_serialize_10k` exceeds its 250 ms budget** — otherwise mark N/A and close.
- **Depends on:** AUR-P8.2 (its measurement is the trigger).
- **Branch / commit:** `feature/aurora-phase8`; `feat(aurora): ...` convention; phase (and program) ends with a gate.
- **Files:** Modify: rtsim `Data` sub-map types behind an unchanged `Data::write_to`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing.
- **Acceptance:** Serialization byte-identical pre/post; save-thread behavior unchanged; `data.dat` ≤ 30 MB at 10k NPCs (spec metric), measured in soak.
- **Size:** L
