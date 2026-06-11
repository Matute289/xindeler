# PROJECT ORACLE — Task Board

**Source plan:** [../plans/2026-06-11-project-oracle.md](../plans/2026-06-11-project-oracle.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> If acceptance fails twice, escalate one model tier and leave a note in the task file.

Conventions (every task): tests via `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim` (plus `-p veloren-server` / `-p veloren-common` where stated); invoke `veloren-oracle` + `superpowers:test-driven-development` before code; every field reachable from rtsim `Data` gets `#[serde(default)]` and joins the old-save fixture test (ORC-P1.2); every lifecycle transition emits `common::telemetry!("oracle_event", ...)`, vetoes `"oracle_veto"`, chronicle appends `"oracle_chronicle"` — never remove these (the Phase 8 soak harness and `veloren-telemetry` skill depend on them); no wildcard `_ =>` arms on `WorldFact`/`EventState`/`ChronicleKind` matches outside tests. Section IDs carry the plan's Task numbers (e.g. ORC-P3.9 = plan Task 9, Phase 3).

## Phase 1 — World State Engine (branch `feature/oracle-phase1` off `development`; Phases 1–2 ship on this branch)

## ORC-P1.1 — Task 1: `WorldFact` typed fact store

- **Model:** sonnet — TDD with full code in plan; pure data module, no `Data` field yet.
- **Depends on:** none (first ORACLE task; create `feature/oracle-phase1` off `development`).
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): typed WorldFact store with replace-on-assert semantics`.
- **Files:** Create: `rtsim/src/data/oracle/mod.rs` (skeleton: `pub mod facts;`), `rtsim/src/data/oracle/facts.rs`. Modify: `rtsim/src/data/mod.rs:1-9` (insert `pub mod oracle;` between `pub mod npc;` and `pub mod quest;`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–5 verbatim. Traps: replace-on-assert keys on (variant tag, subject a, subject b) — `AtWar(a,b)` is symmetric via min/max; never repurpose variants (rmp-serde names variants, renames break old saves); `BTreeMap` keeps iteration deterministic.
- **Acceptance:** `VELOREN_ASSETS=... cargo test -p veloren-rtsim oracle` → 2 tests PASS (monotonic ids + replace semantics + symmetric AtWar; retract + msgpack round-trip).
- **Size:** M

## ORC-P1.2 — Task 2: Chronicle + `OracleData` field on rtsim `Data` (old-save fixture)

- **Model:** opus — rtsim `Data`-field/save-compat work (policy) plus the causal-chain invariant (append-only, forward refs dropped).
- **Depends on:** ORC-P1.1.
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): chronicle + OracleData field on rtsim Data with old-save fixture`.
- **Files:** Create: `rtsim/src/data/oracle/chronicle.rs`. Modify: `rtsim/src/data/oracle/mod.rs` (`OracleData` struct), `rtsim/src/data/mod.rs:39-70` (`#[serde(default)] pub oracle` after `quests`; `oracle_serde_tests` fixture at end of file).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–6 verbatim. Traps: `append` drops causes referencing not-yet-existing entries (no forward references — makes `validate_causal_chain` a hard invariant); `get` is a binary search over push-only id order; the `LegacyData` fixture mirrors pre-oracle on-disk shape via `write_named`.
- **Acceptance:** `cargo test -p veloren-rtsim oracle` PASS (chronicle + fixture + Task 1 tests); `cargo check -p veloren-rtsim -p veloren-server` clean — `Data::generate` and the server load path need no changes because `OracleData: Default`.
- **Size:** M

## ORC-P1.3 — Task 3: `OracleWorldState` rule — change detection into the chronicle

- **Model:** sonnet — observation methods + rule wiring fully specified in plan; mirrors the Architect stride pattern.
- **Depends on:** ORC-P1.2.
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): OracleWorldState rule folds deaths/thefts into the chronicle`.
- **Files:** Create: `rtsim/src/rule/oracle/mod.rs` (`pub mod world_state;`), `rtsim/src/rule/oracle/world_state.rs`. Modify: `rtsim/src/rule/mod.rs:1-8` (`pub mod oracle;` after `pub mod npc_ai;`), `rtsim/src/lib.rs:199-209` (register in `start_default_rules`), `rtsim/src/data/oracle/mod.rs` (observation methods + tests).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–7 verbatim. Traps: only player-caused deaths are chronicled (ambient NPC churn lives in `Architect::deaths` and would grow the chronicle without bound); all thefts chronicled (rare, player-driven); `ORACLE_TICK_SKIP = 32` matching `ARCHITECT_TICK_SKIP`; this rule only records — mechanical effects belong to the event engine.
- **Acceptance:** `cargo test -p veloren-rtsim observe` PASS; `cargo check -p veloren-rtsim -p veloren-server` clean; full `cargo test -p veloren-rtsim` PASS (no regressions in `ai`/`sentiment`).
- **Size:** M

## Phase 2 — Event Engine (same branch `feature/oracle-phase1`)

## ORC-P2.4 — Task 4: Event taxonomy + lifecycle state machine as data

- **Model:** opus — event-engine core (policy): the lifecycle state machine is the transition-legality authority everything else relies on.
- **Depends on:** ORC-P1.2 (`OracleData`).
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): event taxonomy and lifecycle state machine as data`.
- **Files:** Create: `rtsim/src/data/oracle/events.rs`. Modify: `rtsim/src/data/oracle/mod.rs` (`pub mod events;`, re-exports, `#[serde(default)] pub events: WorldEvents` on `OracleData`; extend the ORC-P1.2 fixture test with `assert!(data.oracle.events.is_empty());`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 4' steps 1–5 verbatim. Traps: `TimeOfDay` has no `PartialEq` so `EventState` doesn't either — compare with `matches!`; failed transitions must not mutate state; Active stages escalate strictly by one; terminal states accept nothing; `active_visible()` is the quantity density caps bound.
- **Acceptance:** `cargo test -p veloren-rtsim events` 2 PASS; `cargo test -p veloren-rtsim oracle` — fixture still PASS.
- **Size:** M

## ORC-P2.5 — Task 5: Validation layer — density caps and class cooldowns

- **Model:** sonnet — anti-chaos parameters already fixed by the plan as code constants (1/region, 4 global, 30-day cooldown); implementation fully specified. Relaxing any cap = fable, not a tuning knob.
- **Depends on:** ORC-P2.4.
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): validation layer with density caps and class cooldowns`.
- **Files:** Create: `rtsim/src/data/oracle/validate.rs`. Modify: `rtsim/src/data/oracle/mod.rs` (`pub mod validate;`, re-export `validate::{Pacing, Veto}`, `#[serde(default)] pub pacing: Pacing`; extend fixture test with the `cooldown_active` assertion).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 5' steps 1–5 verbatim. Traps: invisible events bypass density caps but still respect class cooldowns; `validate` is called twice per event (Proposed→Validated and again at trigger time).
- **Acceptance:** `cargo test -p veloren-rtsim validate` 2 PASS; `cargo test -p veloren-rtsim oracle` fixture still PASS.
- **Size:** S

## ORC-P2.6 — Task 6: Event engine rule — templates, transitions, effects, telemetry

- **Model:** opus — event-engine core (policy): sole transition authority, effect/inverse bookkeeping, chronicle causality, telemetry contract.
- **Depends on:** ORC-P2.4, ORC-P2.5, ORC-P1.3 (rule module + registration order).
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): event engine rule with lifecycle transitions, effects, telemetry`.
- **Files:** Create: `rtsim/src/data/oracle/templates.rs` (8 built-ins, one per class), `rtsim/src/rule/oracle/event_engine.rs`. Modify: `rtsim/src/data/oracle/mod.rs` (`pub mod templates;`, `propose_from_template`), `rtsim/src/rule/oracle/mod.rs` (`pub mod event_engine;`), `rtsim/src/lib.rs` (register after `OracleWorldState`).
- **Assets:** none (templates are a built-in `const` registry; RON externalization is ORC-P6.14).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 6' steps 1–6 verbatim. Traps: `propose_from_template` is the ONLY write path into the event store (admin command and future LLM proposer both come through it); `step_events` is pure over `OracleData` and id-ordered; re-validate at trigger time — `Rejected` is the Proposed-stage verdict, `Expired` the trigger-time verdict (the plan's Step 5 note: if the density-cap test fails with `Expired` instead of `Rejected`, fix the engine, not the test); illegal transitions log `tracing::error!` and leave the event alone; activation asserts facts recorded in `asserted_facts`, resolution retracts them (effect-inverse bookkeeping).
- **Acceptance:** `cargo test -p veloren-rtsim event_engine` 3 PASS (full lifecycle + fact cleanup + causal chain ≥5 chronicle entries; density-cap rejection; 30-day class cooldown); full `cargo test -p veloren-rtsim` PASS.
- **Size:** L

## ORC-P2.7 — Task 7: `/oracle_event` admin command

- **Model:** sonnet — mechanical multi-crate wiring fully specified (enum/data/keyword/dispatch/handler/i18n) plus in-game smoke test.
- **Depends on:** ORC-P2.6 (`propose_from_template`, telemetry walk).
- **Branch / commit:** `feature/oracle-phase1`; commit `feat(oracle): /oracle_event admin command for manual event injection`.
- **Files:** Modify: `common/src/cmd.rs:422` (variant between `Object,` and `Outcome,`), `:834` area (`data()` arm), `:1217` area (`keyword()` arm); `server/src/cmd.rs:229` (dispatch) + handler after `handle_rtsim_purge` (`:2113`); `assets/voxygen/i18n/en/command.ftl:79`.
- **Assets:** i18n text — `command-oracle_event-desc` line in `assets/voxygen/i18n/en/command.ftl`; Claude creates inline (exact string in plan).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 7' steps 1–6 verbatim. Traps: `verify_cmd_list_sorted` (`common/src/cmd.rs:1597`) enforces keyword-sorted enum order ("object" < "oracle_event" < "outcome"); the command only *proposes* — injected events still pass validation; if voxygen reports a non-exhaustive `ServerChatCommand` match, add the arm explicitly — no wildcard.
- **Acceptance:** `cargo test -p veloren-common cmd` PASS incl. sorted invariant; `cargo check -p veloren-common -p veloren-server -p veloren-voxygen` clean. In-game smoke (`veloren-run`, admin): `/oracle_event harvest_festival 0` → proposed message; `/oracle_event bogus` → error listing the eight templates; telemetry shows `Proposed → Validated → Scheduled → Active` plus chronicle appends and the `oracle_tick` heartbeat; server restart shows no rtsim purge (persistence proof for all Phase 1–2 fields). Then run ORC-T17 for the phase-1-2 branch finish.
- **Size:** M

## Phases 3–8 — contract-level (each phase on its own branch `feature/oracle-phaseN` off `development` after the previous phase merges; re-verify anchors at phase start; every phase ends with ORC-T17)

## ORC-P3.8 — Task 8: AURORA interface contract — typed WorldFact read API + observation queue

- **Model:** opus — the AURORA↔ORACLE integration boundary (multi-system contract; reconciliation authority if the companion plan drifted).
- **Depends on:** ORC-P2.6/P2.7 merged (Phase 1–2). Cross-file: AURORA consumers of `WorldFact`s (file 08 — e.g. AUR-P5.5's integration contract) depend on THIS task; AURORA never gets `&mut` access.
- **Branch / commit:** `feature/oracle-phase3`; commit `feat(oracle): AURORA interface — typed fact read API + bounded observation queue`.
- **Files:** Modify: `rtsim/src/data/oracle/facts.rs` (query API), `rtsim/src/data/oracle/mod.rs` (`observations` field, `#[serde(skip)]` — transient between strides). Create: `rtsim/src/data/oracle/observations.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Specifically confirm the AURORA plan still expects read-only fact access from NPC think-ticks + a bounded observation submit queue (spec Section 2.3); if its contract drifted, reconcile **here first**. API: `at_war`, `controlling_faction`, `active_festival`, `bounty_on`, `region_facts`; `ObservationQueue` cap 1024, drop-oldest with `telemetry!("oracle_obs_dropped", ...)`; `OracleWorldState::on_tick` drains the queue into chronicle entries each stride.
- **Acceptance:** AURORA-side code compiles against `&data.oracle.facts` only (no `&mut` leaks); queue never exceeds cap under a 10k-submit unit test; all five accessors unit-tested; `at_war(a,b) == at_war(b,a)`; `cargo test -p veloren-rtsim oracle` PASS.
- **Size:** M

## ORC-P3.9 — Task 9 (Phase 3): Regions + ecosystem data model

- **Model:** opus — simulation-dynamics judgment (logistic + Lotka-Volterra + migration with hard clamps) on persisted `OracleData` state.
- **Depends on:** ORC-P3.8 (phase branch); ORC-P1.2 (fixture-test extension pattern).
- **Branch / commit:** `feature/oracle-phase3`; commit `feat(oracle): regional ecosystem model with logistic/L-V dynamics`.
- **Files:** Create: `rtsim/src/data/oracle/ecosystem.rs`, `assets/common/oracle/predation.ron`. Modify: `rtsim/src/data/oracle/mod.rs` (`ecosystem` field, `#[serde(default)]`, fixture-test extension).
- **Assets:** `assets/common/oracle/predation.ron` — sparse predator/prey matrix `[(predator: "Wolf", prey: "Deer", rate: 0.08), ...]`, loaded via `common::assets` like entity configs; Claude creates the RON inline (species names from `TrackedPopulation`).
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. `RegionMap::derive` partitions on weather-cell-aligned tiles (`common::weather::CELL_SIZE`) with per-region biome profile, tension, adjacency; `step_ecosystem` clamps `N in [0.0, 1.2 * K]`.
- **Acceptance:** Populations stay in `[0.2K, 1.2K]` over a 365-day simulated unit test; never negative; migration conserves total population; solver deterministic (no RNG); update O(regions × species).
- **Size:** L

## ORC-P3.10 — Task 10 (Phase 3): Ecosystem planner drives Architect spawns

- **Model:** opus — multi-system: ORACLE becomes planner, Architect stays executor; old-save convergence semantics.
- **Depends on:** ORC-P3.9; ORC-P2.6 (`propose_from_template` for the `migration_wave` template).
- **Branch / commit:** `feature/oracle-phase3`; commit `feat(oracle): ecosystem planner writes Architect wanted_population`.
- **Files:** Create: `rtsim/src/rule/oracle/ecosystem.rs`. Modify: `rtsim/src/rule/migrate.rs` (`wanted_population` recompute), `rtsim/src/generate/mod.rs` (startup seed).
- **Assets:** none (the new `migration_wave` invisible Ecological template joins the built-in registry in code).
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Legacy static computation remains the fallback for empty ecosystem state (old saves); migration flows > 20% of a region's population call `propose_from_template("migration_wave", ...)`.
- **Acceptance:** `architect.wanted_population.total()` within ±30% of the legacy value on a fresh world (regression test comparing both paths); old saves load and converge (fixture test); a forced drought (K halved) measurably reduces herbivore targets within 7 in-game days in a unit test.
- **Size:** L

## ORC-P3.11 — Task 11 (Phase 3): `VariantOverlay` spawn modifiers

- **Model:** opus — multi-system (`common` generation + architect spawn path + persisted legendary records) with balance constraints from another spec.
- **Depends on:** ORC-P3.9 (`DriftProfile`, ecosystem store); ORC-P3.10 (architect spawn path). External: stat multipliers must respect the level bands in `docs/superpowers/specs/2026-06-10-world-difficulty-zones-design.md` (verify it exists — plan anchor); affix ability ids land with the magic-abilities plan — `affixes` stays empty until then.
- **Branch / commit:** `feature/oracle-phase3`; commit `feat(oracle): variant overlay system (elite/regional/legendary)`. Phase 3 ends with ORC-T17.
- **Files:** Modify: `common/src/generation.rs` (`EntityConfig` overlay application), `rtsim/src/rule/architect.rs` (spawn path applies overlays), `rtsim/src/data/oracle/ecosystem.rs` (`DriftProfile`, legendary records).
- **Assets:** none (name suffixes are strings; no new voxel models — if distinct legendary models are ever wanted, that is a fable decision point).
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Elite chance 2–5% scaled by region tension; Regional drift biases capped ±15%, reset on population collapse; Legendary records persist keyed by name with kill/respawn history.
- **Acceptance:** Multipliers respect the difficulty-zones level bands; `roll_variant` deterministic under a seeded RNG; spawning a Legendary writes a chronicle entry and a rumor fact for AURORA.
- **Size:** L

## ORC-P4.12 — Task 12 (Phase 4): Climate states and anomaly events

- **Model:** opus — multi-system: rtsim state → server weather sim bridge → world economy shock inputs, with inverse bookkeeping.
- **Depends on:** Phase 3 merged; ORC-P2.6 (template/lifecycle machinery).
- **Branch / commit:** `feature/oracle-phase4`; commit `feat(oracle): climate anomalies as lifecycle events driving the weather sim`. Phase ends with ORC-T17.
- **Files:** Create: `rtsim/src/data/oracle/climate.rs`. Modify: `server/src/weather/sim.rs` (anomaly modifiers), `rtsim/src/data/oracle/templates.rs` (`flood`, `heatwave`, `harsh_winter` join `drought`), `world/src/site/economy/context.rs` (shock inputs), `server/src/rtsim/tick.rs` (bridge reads climate states each weather tick, drives `WeatherSim::add_zone`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Resolution clears state (inverse bookkeeping, as with facts). Acceptance-test the consequence chain: Active drought → `crop_yield_mod = 0.5` → `WorldFact::FoodShortage` asserted for the region's sites → economy context consumes the shock.
- **Acceptance:** No new network messages (weather grid already synced); anomaly lifecycle round-trips save/load; `/oracle_event drought <region>` visibly stops rain in a smoke test; economy shock input covered by a `veloren-world` unit test; `cargo test -p veloren-rtsim -p veloren-server` PASS.
- **Size:** L

## ORC-P5.13 — Task 13 (Phase 5): Seasons, moon phases, `CelestialState` sync

- **Model:** opus — multi-system across `common` time/calendar/resources, a new `common-net` sync message (protocol bump), and shader uniforms.
- **Depends on:** Phase 4 merged; ORC-P3.9/P3.10 (ecosystem planner consumes moon/season couplings).
- **Branch / commit:** `feature/oracle-phase5`; commit `feat(oracle): seasons, moon phases, and CelestialState sync`. Phase ends with ORC-T17.
- **Files:** Modify: `common/src/calendar.rs` (in-game `Season` alongside real-date `CalendarEvent`), `common/src/time.rs` (`MoonPhase` next to `MoonPeriod` at `:37`), `common/src/resources.rs` (`CelestialState` near `get_moon_dir` at `:27`), `common-net` (new sync message mirroring the Weather grid-sync message), `assets/voxygen/shaders/include/sky.glsl` + `voxygen/src/render/pipelines/skybox.rs` (uniforms).
- **Assets:** shader edits to the existing `sky.glsl` only — no new asset files.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Year = 96 in-game days; moon cycle 8 in-game days; eclipses/comets are ORACLE events (full lifecycle) whose Active stage sets `CelestialState` fields; full moon raises night-monster spawn weight in the ecosystem planner; season modulates carrying capacities and weather-sim humidity constants. S3 terrain visuals are **explicitly out** (spec Section 5.1).
- **Acceptance:** `season_at`/`moon_phase_at` pure, total, unit-tested at cycle boundaries; client renders moon phase (shader uniform) and season color grading; protocol bump documented in the changelog; tests `-p veloren-common -p veloren-rtsim` PASS.
- **Size:** L

## ORC-P6.14 — Task 14 (Phase 6): Narrative director + LLM proposer thread

- **Model:** opus — LLM-integration plumbing (policy): async worker off the tick path, schema validation, canon checker; narrative beat machine.
- **Depends on:** Phase 5 merged; ORC-P2.6 (`propose_from_template` is the LLM's only write path).
- **Branch / commit:** `feature/oracle-phase6`; commit `feat(oracle): narrative director with arcs, canon checker, async LLM proposer`. Phase ends with ORC-T17.
- **Files:** Create: `rtsim/src/data/oracle/narrative.rs`, `rtsim/src/rule/oracle/narrative.rs`, `server/src/oracle/mod.rs`, `server/src/oracle/llm.rs`, `server/src/oracle/validate.rs`, `assets/common/oracle/canon.ron`, `assets/common/oracle/events/*.ron` (externalize ORC-P2.6's built-ins; two templates per class).
- **Assets:** `assets/common/oracle/canon.ron` — deity names, dead characters, geographic invariants sourced from `docs/superpowers/specs/2026-06-10-lore-cosmology-design.md` (use the `veloren-lore` skill; Claude creates the RON inline). `assets/common/oracle/events/*.ron` — externalized event templates, Claude creates inline from the built-in registry + spec's two-per-class target.
- **Downloads/tools:** LLM endpoint exactly as the plan specifies: `trait LlmBackend { fn propose(&self, digest: WorldDigest) -> ... }` with an HTTP impl and a `Disabled` impl (template-text fallback). The plan defines no concrete provider, model, env var, or local-model download — implement and CI-soak with `Disabled`; standing up a live HTTP endpoint (URL in server settings) is a **fable decision point**; do not invent beyond the plan.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Worker thread + crossbeam channel **mirroring the rtsim save-thread pattern** — never on the tick path; `validate.rs` schema-validates proposal JSON then maps to `propose_from_template` — the LLM never gets a richer write path than the admin command; canon checker rejects pitches contradicting canon or chronicle facts.
- **Acceptance:** With `LlmBackend::Disabled` the full arc machinery runs on template text (CI-soakable, no network); malformed LLM output rejected with `telemetry!("oracle_llm_rejected", ...)`, never panics; canon checker unit-tested with deliberately contradictory pitches; beat advancement covered by pure-data tests like ORC-P2.6's.
- **Size:** L

## ORC-P7.15 — Task 15 (Phase 7): Player impact — deeds, fame/infamy, villains, legacy

- **Model:** opus — multi-system: server event hooks, observation queue, validator invariants (one nemesis per player), sanctioned terrain edit.
- **Depends on:** Phase 6 merged; ORC-P3.8 (observation queue feeds the ledger); ORC-P2.5 (cooldown machinery reused for the griefing-loop guard).
- **Branch / commit:** `feature/oracle-phase7`; commit `feat(oracle): player deed ledger, fame/infamy, villain pipeline, legacy monuments`. Phase ends with ORC-T17.
- **Files:** Create: `rtsim/src/data/oracle/players.rs`. Modify: `server/src/events/entity_manipulation.rs`, `server/src/events/trade.rs` (hooks beside existing telemetry call sites), monument placement via `server/src/terrain_persistence.rs` callers.
- **Assets:** none — monuments are placed as ≤ 200 blocks via `TerrainPersistence::set_block` (the one sanctioned runtime terrain edit), generated in code; if a bespoke voxel model is preferred instead, that is a **fable decision point**.
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. `PlayerLedger { deeds, fame, infamy }` keyed by `CharacterId`, fed through the ORC-P3.8 queue; fame/infamy decay per in-game day in the world-state rule; infamy > 0.5 → `WorldFact::BountyOn`, > 0.8 → regional `manhunt` template; **one active nemesis per player** (validator invariant, spec Section 10); dungeon invasions flip a dungeon site's spawn faction via Architect orders + `SiteControlled` facts — zero terrain work.
- **Acceptance:** Fame/infamy decay unit-tested; bounty cap test stacks three bounties and gets one; monument plan asserted ≤ 200 blocks in a unit test; griefing-loop check — infamy earned inside an active `manhunt` cannot re-trigger a second manhunt (class-cooldown machinery); tests `-p veloren-rtsim -p veloren-server` PASS.
- **Size:** L

## ORC-P8.16 — Task 16 (Phase 8): Catch-up sim, compaction, soak harness

- **Model:** opus — multi-system: boot catch-up, chronicle compaction under the causal-chain invariant, admin command, CI soak.
- **Depends on:** Phase 7 merged; ORC-P2.6 (`step_events`), ORC-P3.9 (ecosystem step), ORC-P4.12 (climate step), ORC-P2.7 (sorted-command invariant pattern). Deferred external: dormant dungeon sites stay **deferred** until the difficulty-zones map regen is scheduled (spec Section 13 risk).
- **Branch / commit:** `feature/oracle-phase8`; commit `feat(oracle): downtime catch-up, chronicle compaction, soak harness`. Phase ends with ORC-T17.
- **Files:** Modify: `server/src/rtsim/mod.rs` (boot catch-up after the `OnSetup` emit), `rtsim/src/data/oracle/chronicle.rs` (compaction), `common/src/cmd.rs` + `server/src/cmd.rs` (`/oracle_fastforward` — keyword sorts between `oracle_event` and `outcome`). Create: soak script for CI (headless `veloren-server-cli` driving `/oracle_fastforward 365` nightly).
- **Assets:** none (one new `command-oracle_fastforward-desc` i18n line, Claude creates inline).
- **Downloads/tools:** none.
- **Steps:** Contract task — run the anchor-verification greps in the plan first; if anchors moved, escalate to fable for re-planning before implementing. Catch-up: coarse ORACLE-only steps (`step_events`, ecosystem, climate — no NPC pathing) at 1-in-game-hour resolution, capped at 7 in-game days, entries flagged `ChronicleKind::SimulatedOffline`. Compaction: Resolved events older than 60 in-game days collapse to consequence facts + one summary entry; chronicle beyond 50k entries streams to JSONL via `common/frontend/src/bounded_writer.rs`; `validate_causal_chain` must still pass post-compaction (exactly the corruption case the ORC-P1.2 test guards).
- **Acceptance:** Catch-up of 30-day downtime completes < 10 s wall-clock and stops at the 7-day cap; fastforward deterministic for a fixed seed and start state; soak asserts zero invariant breaches, populations in `[0.2K, 1.2K]`, `validate_causal_chain() == Ok(())`, no panics; ORACLE stride p95 < 2 ms via tracy (`veloren-engine-perf` skill) at current world size.
- **Size:** L

## ORC-T17 — Task 17: Lint, format, changelog, branch finish (run at the END of every phase branch)

- **Model:** fable — phase-gate review and merge decision; owns cross-task fallout from the final sweep. Re-run this task once per phase branch (phases 1–2 combined, then 3, 4, 5, 6, 7, 8).
- **Depends on:** all tasks of the phase being closed (first run: ORC-P1.1–P2.7).
- **Branch / commit:** current phase branch; commit `docs: changelog entry for ORACLE world-director phase 1-2` (adjust wording per phase); merge into `development`.
- **Files:** Modify: `CHANGELOG.md`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 17' steps 1–5 verbatim: CI-identical clippy (full feature string) + voxygen publish-profile clippy; `cargo fmt --all -- --check`; full suite `VELOREN_ASSETS=... cargo test -p veloren-rtsim -p veloren-server -p veloren-common`; changelog; `veloren-review` then `superpowers:finishing-a-development-branch`. After merge, the next phase branches off `development` and re-verifies its contract anchors (the grep commands in each task's Step 1).
- **Acceptance:** Both clippy invocations clean (no `#[allow]` without justifying comment); fmt clean; full suite PASS; changelog entry committed; phase branch merged into `development`.
- **Size:** M
