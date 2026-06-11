# World Difficulty Zones — Task Board

**Source plan:** [../plans/2026-06-11-world-difficulty-zones.md](../plans/2026-06-11-world-difficulty-zones.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> Escalation rule: If acceptance fails twice, escalate one model tier and leave a note in the task file.

> Branch setup (before WDZ-T1): create `feature/world-difficulty-zones` off `development`. All tasks commit to this branch. Character-levels M1 is **already merged** (`SkillSet::character_level()`, `Outcome::CharacterLevelUp`, nameplate `Info.level`) — no dependency edges needed for it. Plan line anchors verified at commit `53c4466145`; re-locate by quoted code if a hunk drifted. Skills: `veloren-worldgen` (T3/T6/T7), `veloren-progression` (T1/T4/T5/T10), `superpowers:test-driven-development` everywhere.

## WDZ-T1 — Entity level math and the `Level` component

- **Model:** haiku — every line (tests, component, all six math functions, registration) is given verbatim with exact anchors; purely mechanical TDD transcription.
- **Depends on:** none.
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: Level component and difficulty-band math for world zones`
- **Files:**
  - Create: `common/src/comp/level.rs`
  - Modify: `common/src/comp/mod.rs` (`pub mod level;` between `mod last;` and `mod location;`; re-export `level::Level,` after `last::Last,`), `common/state/src/state.rs` (register after line 246)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–5 verbatim. TDD: tests-only file first, confirm "cannot find function `level_band`", then implement.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common comp::level -- --nocapture` → 4 PASS.
  - `cargo check -p veloren-common-state` → clean.
- **Size:** M

## WDZ-T2 — Sync `Level` to clients

- **Model:** opus — two-line edit, but it extends the `synced_components!` x-macro (wire format/netcode change touching sentinel tracking, packets, and client application); routing policy sends netcode changes to opus.
- **Depends on:** WDZ-T1 (`comp::Level` exists and is re-exported).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: sync Level component to clients`
- **Files:**
  - Create: none
  - Modify: `common/net/src/synced_components.rs` (entry after `stats: Stats,` line 25; `NetSync` impl with `SyncFrom::AnyEntity` after the `Stats` impl, lines 135–137)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–3 verbatim. Any "no impl"/non-exhaustive error points at another x-macro expansion site (`server/src/sys/sentinel.rs:318`, `common/net/src/msg/ecs_packet.rs:106`) — fix following the `Stats` pattern there. Do NOT add wildcard arms.
- **Acceptance:**
  - `cargo check -p veloren-common-net -p veloren-server -p veloren-client` → clean.
- **Size:** S

## WDZ-T3 — `SimChunk.difficulty` computed at worldgen

- **Model:** opus — worldgen change touching `SimChunk` and `World::generate` (explicit opus per routing policy); replaces the live starting-site scoring heuristic.
- **Depends on:** none (independent of T1/T2; plan order on shared branch).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: per-chunk region difficulty computed at worldgen`
- **Files:**
  - Create: `world/src/sim/difficulty.rs`
  - Modify: `world/src/sim/mod.rs` (module decl; `SimChunk` field at line 2503; BOTH `SimChunk` literals ~line 729 and ~line 2776; new `WorldSim::compute_difficulty` after `pub fn get`), `world/src/lib.rs` (compute call right after `civ::Civs::generate`; `get_chunk_difficulty` accessor next to `pub fn sim()`; replace the spawn-score heuristic at lines 279–284 and delete the commented-out line below it)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `veloren-worldgen` skill for pipeline context.
- **Steps:** Follow plan section '### Task 3' steps 1–4 verbatim. ORDER TRAP: `compute_difficulty` must run right after civ generation (towns project difficulty-1 safe discs) and before anything reads the field. `cargo check -p veloren-world` missing-field errors are the authoritative list of `SimChunk` literals. If monotonicity tests fail, fix the curve, not the test.
- **Acceptance:**
  - `cargo check -p veloren-world` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world difficulty` → 3 PASS.
- **Size:** M

## WDZ-T4 — Permanent stat-scaling primitives (`Stats`, `Health`, combat hook)

- **Model:** sonnet — code given verbatim, but it rewrites `reset_temp_modifiers` and the global damage-modifier read in `combat.rs` — core combat path where a subtle slip silently mis-scales all damage.
- **Depends on:** none (referenced by T5; plan order).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: non-reset level damage multiplier and one-shot HP scaling`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/stats.rs` (field after `attack_damage_modifier` line 86; init in `Stats::new`; replace `reset_temp_modifiers` lines 147–154; tests), `common/src/comp/health.rs` (`with_max_multiplier` after `pub fn new`; tests), `common/src/combat.rs` (lines 396–398: multiply by `level_damage_multiplier`)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `veloren-progression` skill.
- **Steps:** Follow plan section '### Task 4' steps 1–4 verbatim. KEY INVARIANT: `level_damage_multiplier` must survive `reset_temp_modifiers` (carried across the `*self = Self::new(...)` re-creation) — that's the whole point and the test pins it. `with_max_multiplier` is called exactly once at spawn; never on a live entity.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common level_scaling -- --nocapture` → 2 PASS.
  - `cargo check --workspace` → clean (Stats is only built via `Stats::new`/`Stats::empty`).
- **Size:** S

## WDZ-T5 — `EntityInfo.level` → spawn-time scaling → `Level` component

- **Model:** opus — the central spawn-pipeline change (explicit opus per routing policy): threads level through `EntityInfo` → `NpcData` → `NpcBuilder` → component attachment across common/server with a compiler-driven literal sweep.
- **Depends on:** WDZ-T1 (math + component), WDZ-T4 (`level_damage_multiplier`, `with_max_multiplier`).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: spawn-time level scaling and rank override for NPCs`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/level.rs` (`preset_rank_override` + test), `common/src/generation.rs` (`EntityInfo.level` field, init, `with_level` builder), `server/src/sys/terrain.rs` (`NpcData.level`, `from_entity_info` scaling, literal, `to_npc_builder`; `spawn_scaling_tests`), `common/src/event.rs` (`NpcBuilder.level` + `with_level`), `server/src/events/entity_creation.rs` (`.maybe_with(level.map(comp::Level))`), `server/src/events/entity_manipulation.rs:3592` (`level: None,` in the transformation literal) and anything else `cargo check` reports
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `veloren-progression` skill.
- **Steps:** Follow plan section '### Task 5' steps 1–8 verbatim. NOTES: `level: None` = unleveled, no scaling; pets/riders deliberately spawn unleveled (spec Open Question 2); the entity_manipulation literal gets `level: None,` with the comment about transformation keeping pre-baked scaling. Run the compiler sweep (`grep -B2 "missing field"`) until clean.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common preset_rank_override` → PASS.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server spawn_scaling -- --nocapture` → 1 PASS.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` → PASS (entity-config asset tests).
- **Size:** L

## WDZ-T6 — Band levels for all worldgen spawns (wildlife + dungeon trash)

- **Model:** sonnet — a single verbatim block, but with a documented compiler fallback (inline the closure if rustc complains about the borrow across the match) and worldgen-context judgment.
- **Depends on:** WDZ-T3 (`sim_chunk.difficulty`), WDZ-T5 (`EntityInfo.level` + `level_band` consumption).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: band-level assignment for all worldgen entity spawns`
- **Files:**
  - Create: none
  - Modify: `world/src/lib.rs` (import `comp::{self, Content}` in the `use common::{...}` block lines 47–64; post-pass after the site-supplement loop lines 564–567)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `veloren-worldgen` skill.
- **Steps:** Follow plan section '### Task 6' steps 1–3 verbatim. The pass only fills `level == None` and skips `special_entity` spawns — producers that pre-level (WDZ-T7 bosses) are untouched. Group members roll independently (a wolf pack spans the band).
- **Acceptance:**
  - `cargo check -p veloren-world` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world wildlife` → PASS (manifest tests re-instantiate every entity asset).
- **Size:** S

## WDZ-T7 — Dungeon plot role offsets (boss +4, elite +2)

- **Model:** sonnet — parameter threading through `apply_supplement` signatures plus per-site `.with_level` appends; mostly given, compiler-driven arity cleanup.
- **Depends on:** WDZ-T6 (band pass that levels the trash these offsets sit above), WDZ-T3, WDZ-T5.
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: dungeon boss/elite level offsets from host-chunk difficulty`
- **Files:**
  - Create: none
  - Modify: `world/src/lib.rs:566` (pass `sim_chunk.difficulty`), `world/src/site/mod.rs:3269-3282` (`Site::apply_supplement` signature + forwarding), `world/src/site/plot/gnarling.rs` (signature :324; harvester boss :369; chieftain :442; wood golems :431/:488/:502), `world/src/site/plot/adlet.rs` (signature :408 as `_difficulty`; elder via `Land` at :2094)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `veloren-worldgen` skill.
- **Steps:** Follow plan section '### Task 7' steps 1–4 verbatim. The adlet elder is painter-spawned during rendering — read difficulty via `land.get_chunk_wpos(boss_spawn.xy())`. Trash (`random_gnarling`, `mandragora`, `deadwood`, `gnarling_stalker`) stays untouched — T6 levels it. The dummy test calls at gnarling.rs:2216 / adlet.rs:2420 only build `EntityInfo`s and are unaffected.
- **Acceptance:**
  - `cargo check -p veloren-world --all-targets` → clean (fix remaining arity errors).
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world` → PASS.
- **Size:** M

## WDZ-T8 — rtsim NPC levels (persistence-safe) and Architect assignment

- **Model:** opus — rtsim save-compat field (explicit opus per routing policy): a wrong serde default corrupts or rejects existing rtsim saves; also five `Npc::new` sites with shadowing traps.
- **Depends on:** WDZ-T3 (chunk difficulty read), WDZ-T5 (`with_level` flow into `get_npc_entity_info` scaling).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: rtsim NPC levels with serde-default save compatibility`
- **Files:**
  - Create: none
  - Modify: `rtsim/Cargo.toml` (`[dev-dependencies] serde_json = { workspace = true }` — fall back to `"1.0"` if not in workspace deps), `rtsim/src/data/npc.rs` (field with `#[serde(default = "default_npc_level")]`, `Npc::new` init, `with_level`, save-compat test), `rtsim/src/rule/architect.rs` (`spawn_level` helper + `.with_level(...)` on all five `Npc::new` chains: lines 313, 359, 444, 512, 573), `server/src/rtsim/tick.rs` (`get_npc_entity_info` — `.with_level(npc.level)` on BOTH return expressions, lines ~392 and ~441)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 8' steps 1–5 verbatim. TDD: the save-compat test (remove `level` key from serialized map → must load as level 1) comes first. SHADOWING TRAP: at architect sites 359/444/512, bind the 2D wpos BEFORE the existing `let wpos = wpos.as_()...` shadowing (table in plan gives the exact expression per site). Verify sweep: `grep -n "Npc::new" rtsim/src/rule/architect.rs` — every hit carries `.with_level`. Guards' `rank3.fullskill` is rank-overridden automatically by T5's `preset_rank_override`.
- **Acceptance:**
  - `cargo check -p veloren-rtsim -p veloren-server --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim` → PASS (incl. `level_save_compat`).
- **Size:** M

## WDZ-T9 — Profession→ClassKind mapping [GATED on classes-races plan]

- **Model:** sonnet — small TDD task with code given, but it must reconcile against the classes-races `ClassKind` enum from another plan and STOP rather than invent variants.
- **Depends on:** **CLS Task 1 (`ClassKind`) from `03-classes-races-tasks.md`** — HARD GATE (Step 0 grep; if `CLASSKIND ABSENT`, stop, leave unchecked, continue with WDZ-T10 and report). No WDZ-internal dependencies.
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: Profession to ClassKind mapping for combat NPCs`
- **Files:**
  - Create: none
  - Modify: `common/src/rtsim.rs` (impl after the `Profession` enum, lines 485–511, + `class_mapping_tests`)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 9' steps 0–3 verbatim. Gate first: `grep -rn "enum ClassKind" common/src/ || echo "CLASSKIND ABSENT"`. Adjust the test's import to the module path found. If `ClassKind` lacks `Warlock`/`Druid`/`Ranger`, STOP and reconcile with the classes-races plan — do not invent variants. Kit/loadout consumption of the mapping is owned by the classes-races plan; this is the single source of truth only.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class_mapping` → PASS.
- **Size:** S

## WDZ-T10 — XP level differential with gray-mob cutoff

- **Model:** sonnet — wiring into the live kill-award loop in `entity_manipulation.rs` with group-max bookkeeping and a documented `WriteStorage` immutable-read fallback; math already tested in T1.
- **Depends on:** WDZ-T1 (`xp_mult`), WDZ-T2 (synced `Level` read server-side via storage), WDZ-T5 (NPCs actually carry `comp::Level`).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: XP scales with level differential, gray mobs give nothing`
- **Files:**
  - Create: none
  - Modify: `server/src/events/entity_manipulation.rs` (`DestroyEventData` field line 567; victim level after the `exp_reward` computation ~line 1131; group-max pre-pass + `for_each` head/body changes at lines 1253–1268; `handle_exp_gain` first arg becomes the shadowed `exp_reward`)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `veloren-progression` skill.
- **Steps:** Follow plan section '### Task 10' steps 1–3 verbatim. Differential uses the highest-level group member (anti-mule). Gray mobs (`delta <= -10`): `return` before any ExpChange spam. If the pre-pass `data.skill_sets.get(...)` errors on the `WriteStorage`, use `(&data.skill_sets).get(*attacker)`.
- **Acceptance:**
  - `cargo check -p veloren-server --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server` → PASS.
- **Size:** M

## WDZ-T11 — Anti-farm kill ring buffer

- **Model:** sonnet — TDD with real code given (`RecentKills` resource), plus ECS-resource insertion and award-loop integration around T10's edits.
- **Depends on:** WDZ-T10 (hooks into the same award loop, multiplies into `handle_exp_gain`'s first argument).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: anti-farm XP dampening via per-player kill ring buffer`
- **Files:**
  - Create: none
  - Modify: `server/src/events/entity_manipulation.rs` (`RecentKills` + `recent_kills_tests`; `recent_kills: Write<'a, RecentKills>` in `DestroyEventData`; `victim_body` binding; `farm_mult` block before `handle_exp_gain`), `server/src/events/mod.rs` (`pub use entity_manipulation::RecentKills;`), `server/src/lib.rs` (insert next to `RecentClientIPs` at line 369)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 11' steps 1–4 verbatim. Only players accumulate kill history (`data.players.contains(*attacker)`); resource is in-memory only — relog resets it (accepted friction). Pass `exp_reward * farm_mult` to `handle_exp_gain`.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server recent_kills` → 1 PASS.
  - `cargo check -p veloren-server --all-targets` → clean.
- **Size:** M

## WDZ-T12 — Nameplates read `Level`, skull at +8

- **Model:** sonnet — HUD join-tuple surgery with a compiler sweep for other `overhead::Info` literals plus in-game visual verification.
- **Depends on:** WDZ-T2 (client receives `comp::Level`), WDZ-T6 (mobs actually have band levels to show). Nameplate `Info.level` itself is from merged levels-M1 — no task dependency.
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: nameplates show NPC spawn level with outlevel skull`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/hud/mod.rs` (storage :1526; join tuple ~:2390; closure args ~:2415; `Info` literal :2449 — component wins over `skill_set.character_level()`, plus `own_level`), `voxygen/src/hud/overhead.rs` (`Info.own_level` :73; destructure :162–171; `level_skull` + skull condition ~:415–417)
  - Delete: none
- **Assets:** none (reuses the existing skull icon and `Name [N]` nameplate from M1).
- **Downloads/tools:** `veloren-run` skill (fresh map) + admin `/tp` (`Tp`/`RtsimTp`, `common/src/cmd.rs:459,440`).
- **Steps:** Follow plan section '### Task 12' steps 1–5 verbatim. Sweep: any other `overhead::Info { ... }` literal gets `own_level: None,` — iterate `cargo check -p veloren-voxygen` until clean. Visual: starting-zone wolves `[1..3]` with band variety; far from town 20+ levels and skull when ≥ own + 8; far-zone wolf visibly tankier and harder-hitting.
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean.
  - In-game visual checklist above passes.
- **Size:** M

## WDZ-T13 — World-map difficulty overlay

- **Model:** opus — extends `WorldMapMsg` (the connect-time wire message — netcode change per routing policy) and spans world/client/voxygen with multi-class compiler-driven UI wiring.
- **Depends on:** WDZ-T3 (`SimChunk.difficulty` to downsample), WDZ-T12 (visual cross-check against nameplate levels).
- **Branch / commit:** `feature/world-difficulty-zones` — `feat: world map difficulty-zone overlay with toggle`
- **Files:**
  - Create: none
  - Modify: `common/net/src/msg/world_msg.rs` (`difficulty: Grid<u8>` after `alt`), `world/src/sim/mod.rs` (populate in `get_map`'s `WorldMapMsg` literal — locate via `grep -n "fn get_map"`), `client/src/lib.rs` (overlay image build, lines 736–977; `world_map_layers` gains a third entry), `voxygen/src/settings/interface.rs` (setting + Default), `voxygen/src/session/settings_change.rs` (`MapShowZoneDifficulty` variant + arm), `voxygen/src/hud/map.rs` (toggle button, widget ids, layer gating), `assets/voxygen/i18n/en/hud/map.ftl`
  - Delete: none
- **Assets:**
  - `hud-map-zone_difficulty = Difficulty zones` in `assets/voxygen/i18n/en/hud/map.ftl` — Claude creates inline.
  - Toggle-button icon — reuse existing asset: `self.imgs.map_mode_overlay` for now (per plan).
- **Downloads/tools:** `veloren-run` skill for the visual check.
- **Steps:** Follow plan section '### Task 13' steps 1–5 verbatim. Downsample is one byte per 4×4-chunk cell (max within the cell), sent once on connect. Fix any other `WorldMapMsg` literal (test/bot fixtures) with `difficulty: Grid::populate_from(Vec2::one(), |_| 1),`. Layer gating: `(index == 1 && show_topo_map) || (index == 2 && show_zone_difficulty)`. Resolve UI errors by class per plan (widget_ids / Interface variant / serde defaults).
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - In-game: map mode button shows green at towns grading to red at edges/mountains, matching WDZ-T12 nameplate levels.
- **Size:** L

## WDZ-T14 — Lint, format, changelog, branch finish

- **Model:** haiku — mechanical CI-identical commands and a verbatim changelog entry; escalate only if clippy surfaces real fixes.
- **Depends on:** WDZ-T1 … WDZ-T13 (WDZ-T9 may be gated out — if so, file it as a follow-up tied to classes-races).
- **Branch / commit:** `feature/world-difficulty-zones` — `docs: changelog entry for world difficulty zones` (+ any fix commits)
- **Files:**
  - Create: none
  - Modify: `CHANGELOG.md` (+ whatever clippy/fmt fixes touch)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** `superpowers:finishing-a-development-branch` + `veloren-review` before merging into `development`.
- **Steps:** Follow plan section '### Task 14' steps 1–6 verbatim. No `#[allow]` without a justifying comment.
- **Acceptance:**
  - `cargo clippy --all-targets --locked --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" -- -D warnings` → clean.
  - `cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings` → clean.
  - `cargo fmt --all -- --check` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-world -p veloren-rtsim -p veloren-server` → PASS.
- **Size:** M

---

## Phase 4 — DEFERRED (multi-plane worlds; do NOT schedule with T1–T14)

> Explicitly deferred by the plan ("XL"). When started, **re-verify every line anchor against HEAD first** — the descriptions below are file-level only.

## WDZ-P4.1 — `PlaneId` type and serde-compatible portal extension [DEFERRED]

- **Model:** opus — serde save-compat field on `PortalData` (existing portals must deserialize unchanged).
- **Depends on:** decision to start Phase 4 (after Phase-3 playtests); no T1–T14 code dependency.
- **Branch / commit:** new branch off `development` when scheduled (plan does not assign one — propose `feature/multiplane-p4`); commit message TBD at execution.
- **Files:**
  - Create: none
  - Modify: `common/src/rtsim.rs` (or adjacent shared module — `PlaneId(pub u16)`, `Default` = plane 0), `common/src/comp/misc.rs` (`PortalData` gains `target_plane: Option<PlaneId>` with `#[serde(default)]`)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task P4.1' (file-level). Tests: serde round-trip of a `PortalData` WITHOUT the field (mirror WDZ-T8's save-compat pattern); `None` = same-plane. Scope guard: plumbed but UNUSED at runtime — teleport handling keeps ignoring it.
- **Acceptance:** round-trip test PASS; `cargo check --workspace` clean; teleporter behavior unchanged.
- **Size:** S

## WDZ-P4.2 — Pocket-plane site kind (intra-world) [DEFERRED]

- **Model:** opus — new `SiteKind` + plot module + civ placement + rtsim nav exclusion; deep worldgen/spawn-pipeline work.
- **Depends on:** WDZ-P4.1; WDZ-T7 (`with_level` machinery for hardcoded difficulty 9–10 interiors).
- **Branch / commit:** Phase-4 branch; commit message TBD at execution.
- **Files:**
  - Create: `world/src/site/plot/pocket_plane.rs` (structured like `gnarling.rs`: `generate`, `render_inner`, `apply_supplement`)
  - Modify: `world/src/site/mod.rs` (`SiteKind` variant + `meta()`), `world/src/civ/mod.rs` (reserved margin-band placement), `rtsim/src/rule/npc_ai/mod.rs` (exclude band from nav)
  - Delete: none
- **Assets:** themed pocket-plane interior content — entity/loot RONs Claude creates inline at execution; any genuinely new voxel set is a **fable decision point**.
- **Downloads/tools:** `veloren-worldgen` skill.
- **Steps:** Follow plan section '### Task P4.2'. Portals reuse `SpecialEntity::Teleporter(PortalData)` exactly as waypoints are emitted in `world/src/lib.rs` — fixed-coordinate same-world teleports, no engine change.
- **Acceptance:** milestone — enter an overworld portal, arrive in a themed pocket plane, fight L28–30 mobs, portal back.
- **Size:** L

## WDZ-P4.3 — Transfer-queue prototype behind a feature flag [DEFERRED]

- **Model:** opus — persistence migration (`plane` column), rtsim data relocation, despawn/respawn round-trip; maximal save-corruption surface.
- **Depends on:** WDZ-P4.1, WDZ-P4.2.
- **Branch / commit:** Phase-4 branch; commit message TBD at execution.
- **Files:**
  - Create: `server/src/plane_transfer.rs` (behind a `multiplane-prototype` cargo feature), migration under `server/src/persistence/migrations/`
  - Modify: `server/Cargo.toml` (feature), `server/src/rtsim/mod.rs` area (rtsim data path → `data/rtsim/plane_{id}/`)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task P4.3'. Model on the login flow (`server/src/state_ext.rs` character loading, `server/src/sys/persistence.rs`). OUT of scope even here: N concurrent `World`/`IndexOwned` instances, cross-plane chat/trade, multi-process sharding.
- **Acceptance:** despawn/respawn round-trip proven on one world; migration applies cleanly to an existing DB copy.
- **Size:** L

## WDZ-P4.4 — Map enlargement decision point [DEFERRED]

- **Model:** fable — explicit decision point (not an engineering task) weighing band crowding from Phase-3 playtests.
- **Depends on:** Phase-3 playtest data.
- **Branch / commit:** none (decision recorded in the spec's §7 table).
- **Files:**
  - Create: none
  - Modify: spec §7 table (decision record)
  - Delete: none
- **Assets:** none (if enlarged: offline asset bake — generate once on a big box via `GenOpts` `x_lg/y_lg = 11`, ship the map file).
- **Downloads/tools:** big-box machine for the one-off bake, if approved.
- **Steps:** Follow plan section '### Task P4.4'. Revisit only if playtests show band crowding.
- **Acceptance:** decision documented with rationale.
- **Size:** S
