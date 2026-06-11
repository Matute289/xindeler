# Classes and Races (Phase 1+2 core) — Task Board

**Source plan:** [../plans/2026-06-11-classes-races.md](../plans/2026-06-11-classes-races.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> Escalation rule: If acceptance fails twice, escalate one model tier and leave a note in the task file.

> Branch setup (before CLS-1): create `feature/classes-races` off `development`. Depends on character levels M1 (merged). Depended on by: equipment-restrictions Phase B (EQ-B7/EQ-B8) and the magic-abilities plan — both can start as soon as CLS-1 lands. Protocol note: CLS-5 and CLS-10 change `ClientGeneral`/persistence tuples — fine on this private fork (client+server ship together), but do NOT cherry-pick them onto a branch that talks to old clients.

## CLS-1 — `ClassKind` enum + `CharacterClass` component

- **Model:** haiku — new file and mod.rs export given 100% verbatim, including tests; no judgment required.
- **Depends on:** none. (Unblocks EQ-B7/EQ-B8 in 02-equipment-restrictions-tasks.md once landed on `development`.)
- **Branch / commit:** `feature/classes-races` — `feat: ClassKind enum and CharacterClass component`
- **Files:**
  - Create: `common/src/comp/class.rs`
  - Modify: `common/src/comp/mod.rs` (module list after `pub mod chat;` line 11; `class::{CharacterClass, ClassKind}` in the `pub use self::{` block after the `chat::{...}` entry)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–4 verbatim.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class -- --nocapture` → 2 tests PASS (`default_class_is_adventurer`, `keyword_round_trips_for_playable_classes`).
- **Size:** S

## CLS-2 — ECS registration + net sync

- **Model:** haiku — three exact insertions (register line, x-macro entry, NetSync impl) with the plan's troubleshooting note for the only failure mode.
- **Depends on:** CLS-1.
- **Branch / commit:** `feature/classes-races` — `feat: register and net-sync CharacterClass component`
- **Files:**
  - Create: none
  - Modify: `common/state/src/state.rs` (after `ecs.register::<comp::Hardcore>();` line 242), `common/net/src/synced_components.rs` (x-macro list after `hardcore: Hardcore,` line 24; NetSync impl after the `Hardcore` impl ~131–133)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–4 verbatim. WARNING: if `sentinel.rs` errors mention `CharacterClass`, the x-macro entry doesn't match the `lowercase_name: TypeName,` pattern — fix the entry, do NOT edit `server/src/sys/sentinel.rs` (it is generated from the same x-macro).
- **Acceptance:**
  - `cargo check -p veloren-common-state -p veloren-common-net -p veloren-server -p veloren-client` → clean.
- **Size:** S

## CLS-3 — `SkillGroupKind::Class(..)` + DB string converters (both directions)

- **Model:** opus — persistence converters in `json_models.rs` in both directions; both converters panic on unknown kinds today, so a mistake here can panic the server on save or brick loads. Matches the opus routing rule exactly.
- **Depends on:** CLS-1.
- **Branch / commit:** `feature/classes-races` — `feat: SkillGroupKind::Class variant with persistence string mappings`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/skillset/mod.rs` (enum ~107–111, `skill_point_cost` ~117–140, imports ~1–12), `server/src/persistence/json_models.rs` (`skill_group_to_db_string` ~71–98, `db_string_to_skill_group` ~100–117, tests mod ~339), compiler-driven: `voxygen/src/hud/diary.rs` (~2937) and `voxygen/src/hud/skillbar.rs` (~854) plus any other non-exhaustive matches `cargo check` reports
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–6 verbatim. TDD: the round-trip test must fail to compile first. The `Class(_)` SP-cost arm REPLACES the head of the existing `Self::Weapon(...)` arm (body unchanged). `Class(ClassKind::Adventurer)` deliberately panics in `skill_group_to_db_string` (consistent with the unsupported-weapon arm). WARNING: in the compiler-driven match fixes, reuse the `General` arm's value and do NOT add wildcard `_ =>` arms; repeat `cargo check --workspace --all-targets` until clean.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server skill_group_db_string` → FAIL to compile after Step 1 (no variant `Class`), 1 test PASS after Step 3/4.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` → PASS (no regressions).
- **Size:** M

## CLS-4 — Migration V71 + character-table class column + load/store plumbing

- **Model:** opus — schema migration plus SELECT/INSERT row-index plumbing across the persistence layer; a column-index slip silently corrupts character loads. Save-integrity work per the routing rule.
- **Depends on:** CLS-1, CLS-2 (component must be registered before `state_ext.rs` inserts it).
- **Branch / commit:** `feature/classes-races` — `feat: persist character class (migration V71, default Adventurer)`
- **Files:**
  - Create: `server/src/migrations/V71__character_class.sql`
  - Modify: `server/src/persistence/json_models.rs` (class string converters + test), `server/src/persistence/models.rs` (~1–8, `Character` struct), `server/src/persistence/mod.rs` (~35–45, `PersistedComponents`), `server/src/persistence/character/conversions.rs` (new converters near `convert_hardcore_from_database` ~771), `server/src/persistence/character/mod.rs` (load SELECT ~143–176, list SELECT ~326–348, create destructure + INSERT ~418–428 / ~523–540), `server/src/character_creator.rs` (~77–87 placeholder field), `server/src/state_ext.rs` (~696–706 destructure, ~745 component insert)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 4' steps 1–9 verbatim. BEFORE creating the migration, re-verify the latest is V70: `ls server/src/migrations | sort -V | tail -1` → `V70__merge_remaining_unique_recipes.sql` (if not, this anchor moved — STOP and re-plan / escalate to fable). WARNING (plan's own emphasis): the load SELECT gains `c.class` between `c.hardcore` and `b.variant`, shifting the body columns by one — copy the plan's row indices exactly; `db_string_to_class` must NEVER panic (unknown → Adventurer with a warning).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server class_db_string` → FAIL to compile after Step 2, then PASS (`class_db_string_round_trips_and_tolerates_unknown`).
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server` → PASS.
  - Migration smoke test: back up the dev DB, boot `cargo run --bin veloren-server-cli` once → log shows `applying migration` … `V71__character_class`, clean startup, and a legacy character lists and loads.
- **Size:** L

## CLS-5 — `CreateCharacter` message field + server validation

- **Model:** opus — character-creation protocol change (breaking `ClientGeneral` change) threaded through client API, char-selection UI plumbing, server handler, and validation; per routing rule this is opus territory.
- **Depends on:** CLS-1, CLS-4 (replaces the Task 4 placeholder in `character_creator.rs`).
- **Branch / commit:** `feature/classes-races` — `feat: class field on CreateCharacter with server-side validation`
- **Files:**
  - Create: none
  - Modify: `common/net/src/msg/client.rs` (~77–85), `client/src/lib.rs` (~1325–1343 `create_character`), `voxygen/src/menu/char_selection/ui.rs` (Event ~146–153, Mode fields ~180–206, constructors ~225–293, emit ~1850–1874), `voxygen/src/menu/char_selection/mod.rs` (~133–144), `server/src/sys/msg/character_screen.rs` (~174–243), `server/src/character_creator.rs` (~24–90, ~115–128 validation + Display)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 5' steps 1–6 verbatim. Data plumbing only — the picker UI is CLS-9. WARNING: let the compiler walk you to every remaining `CreateCharacter`/`create_character` call site; fix each by passing `class`, NEVER by defaulting to a magic value on the server side. Breaking protocol change — do not cherry-pick onto branches talking to old clients.
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` → PASS.
- **Size:** M

## CLS-6 — Class starting kits (loadout assets, starter whitelists, class skill group)

- **Model:** sonnet — TDD with full code in the plan, but multi-file wiring (visibility change in common, creator rewrite, four new assets) and a documented architectural trap about `SkillSetBuilder`.
- **Depends on:** CLS-3 (`SkillGroupKind::Class`), CLS-5 (`character_class` parameter in `create_character`).
- **Branch / commit:** `feature/classes-races` — `feat: per-class starting kits, starter whitelists and class skill group unlock`
- **Files:**
  - Create: `assets/common/loadout/class/warrior.ron`, `assets/common/loadout/class/mage.ron`, `assets/common/loadout/class/cleric.ron`, `assets/common/loadout/class/rogue.ron`
  - Modify: `common/src/comp/skillset/mod.rs` (~337, `unlock_skill_group` → `pub fn` with doc comment), `server/src/character_creator.rs` (whitelist fn replacing `VALID_STARTER_ITEMS` ~11–22, loadout/skillset block ~53–73, kit push, tests)
  - Delete: none (the `VALID_STARTER_ITEMS` const is replaced in-file)
- **Assets:** Four loadout RONs — Claude creates: RON files inline (full warrior.ron content in the plan; mage/cleric/rogue swap the chest to `worker_purple_0`/`worker_yellow_0`/`worker_green_0`; format copied from `assets/common/loadout/default.ron`). All referenced armor/weapon/consumable specifiers already exist in `assets/common/items/` — no downloads.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 6' steps 1–6 verbatim. WARNING (plan note): do NOT route class groups through the `SkillSetBuilder` preset path — `SKILL_GROUP_LOOKUP` would make every class tree purchasable with General SP; class groups are unlocked directly in code via `unlock_skill_group`.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server character_creator` → FAIL to compile after Step 1, then 2 tests PASS.
  - `cargo check --workspace --all-targets` → clean.
- **Size:** M

## CLS-7 — Class skill-tree stubs in the manifest

- **Model:** haiku — four manifest lines plus a verbatim test; the only rule is an explicit do-not-touch.
- **Depends on:** CLS-3 (`SkillGroupKind::Class` must parse as a manifest key).
- **Branch / commit:** `feature/classes-races` — `feat: stub class skill trees in the skill-group manifest`
- **Files:**
  - Create: none
  - Modify: `assets/common/skill_trees/skills_skill-groups_manifest.ron` (append entries before the closing `}`), `common/src/comp/skillset/mod.rs` (test at end of file)
  - Delete: none
- **Assets:** Four manifest entries (`Class(Warrior): [],` etc.) — Claude creates: RON text inline from the plan.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 7' steps 1–4 verbatim. WARNING: do NOT touch the `General` list — a `General` hash change would force a respec for every existing character.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class_tree` → FAIL (missing manifest entries) after Step 1, 1 test PASS after Step 2.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` → PASS (no skillset regressions from the new manifest keys).
- **Size:** S

## CLS-8 — Racial traits manifest + per-tick application

- **Model:** sonnet — TDD across three crates (loader in common, manifest asset, buff-system hook); code given but the stat-stacking placement (`reset_temp_modifiers`) must land exactly. Balance numbers are already fixed by spec §6 — no fable needed.
- **Depends on:** CLS-1 (`class.rs` exists to host the loader).
- **Branch / commit:** `feature/classes-races` — `feat: data-driven racial trait passives applied in the stat rebuild`
- **Files:**
  - Create: `assets/common/class/racial_traits.ron`
  - Modify: `common/src/comp/class.rs` (trait struct + loader + tests), `common/systems/src/buff.rs` (~512–513, directly after `stat.reset_temp_modifiers();`)
  - Delete: none
- **Assets:** `assets/common/class/racial_traits.ron` — Claude creates: RON file inline (full content in the plan; spec §6 v1 values, partial structs via serde defaults, hot-reloads in dev).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 8' steps 1–6 verbatim. WARNING: the application must run RIGHT AFTER `stat.reset_temp_modifiers()` so traits stack with buffs and need no persistence; `body: &Body` is already bound in that loop (tuple destructure at buff.rs:163).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common racial_traits` → FAIL to compile after Step 1, 2 tests PASS after Steps 2–3.
  - `cargo check -p veloren-common-systems` → clean.
- **Size:** M

## CLS-9 — Class picker in character creation (UI)

- **Model:** sonnet — iced UI work with exact state/message code but compiler-driven layout (mirror the species-button block, gate tool buttons per class) and visual verification.
- **Depends on:** CLS-5 (Mode/Event class plumbing exists), CLS-4 (persistence for the relog check).
- **Branch / commit:** `feature/classes-races` — `feat: class picker in character creation UI`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/menu/char_selection/ui.rs` (Message enum ~326, constants ~59–64, `Mode::CreateOrEdit` buttons, update fn ~1896, layout near species buttons ~1066–1121, tool-button gating ~1143–1187), `assets/voxygen/i18n/en/char_selection.ftl`
  - Delete: none
- **Assets:** `char_selection-class*` keys in `assets/voxygen/i18n/en/char_selection.ftl` — Claude creates: .ftl text inline from the plan.
- **Downloads/tools:** none (visual check via `veloren-run`).
- **Steps:** Follow plan section '### Task 9' steps 1–5 verbatim. The `Message::Class` arm must rebuild the preview loadout exactly like `Message::Tool` does (copy the inventory-update body from the Tool arm at ~:1902). Layout is compiler-driven: run `cargo check -p veloren-voxygen` after each iteration until clean. Also gate the existing tool buttons so only the class's whitelisted weapons render enabled.
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean.
  - Visual (veloren-run): four class buttons render; picking Cleric switches the preview weapon to the sceptre; creating a Mage works and relog loads it (`SELECT alias, class FROM character;` shows `Mage`); a mismatched weapon is impossible from the UI.
- **Size:** L

## CLS-10 — Persist class changes on autosave

- **Model:** opus — extends the live autosave/logout persistence tuple and rewrites the waypoint UPDATE into a combined statement; an arity or parameter-order slip corrupts saved waypoints/classes. Save-integrity work per the routing rule.
- **Depends on:** CLS-4 (`convert_class_to_database`, class column), CLS-2 (component exists to read).
- **Branch / commit:** `feature/classes-races` — `feat: persist character class on autosave and logout`
- **Files:**
  - Create: none
  - Modify: `server/src/persistence/character_updater.rs` (tuple ~24–32, destructure ~426–442), `server/src/sys/persistence.rs` (SystemData, join, tuple ~57–66 / ~102–110), `server/src/events/player.rs` (logout tuple ~403–411), `server/src/persistence/character/mod.rs` (`update()` signature ~1064–1073, UPDATE ~1206–1224)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 10' steps 1–6 verbatim. WARNING: the new UPDATE replaces the waypoint-only UPDATE — keep the `?1`/`?2`/`?3` parameter order exactly as in the plan (`waypoint`, `class`, `character_id`); `cargo check --workspace --all-targets` finds any remaining tuple-arity mismatch. Breaking persistence-tuple change — do not cherry-pick (see file-top protocol note).
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server` → PASS.
- **Size:** M

## CLS-11 — `/set_class` for legacy characters

- **Model:** sonnet — full handler code given, but it spans command spec, dispatch registration, live ECS writes plus skill-group unlock, and a multi-step manual persistence check.
- **Depends on:** CLS-10 (the component and skill group persist via the autosave path), CLS-6 (`unlock_skill_group` is `pub`), CLS-3 (`SkillGroupKind::Class`).
- **Branch / commit:** `feature/classes-races` — `feat: /set_class one-time class pick for legacy characters`
- **Files:**
  - Create: none
  - Modify: `common/src/cmd.rs` (variant near `SetBodyType`/`SetMotd`, `data()` entry, `keyword()` entry), `server/src/cmd.rs` (dispatch registration ~146 area + handler near `handle_battlemode`), `assets/voxygen/i18n/en/command.ftl`
  - Delete: none
- **Assets:** `command-set_class-desc` key in `assets/voxygen/i18n/en/command.ftl` — Claude creates: .ftl text inline from the plan.
- **Downloads/tools:** none (manual check via `veloren-run`).
- **Steps:** Follow plan section '### Task 11' steps 1–4 verbatim. Pattern copied from the `BattleMode` command pair (`common/src/cmd.rs:549-557`/`1188`, `server/src/cmd.rs:156`/`5573`); keep alphabetical placement in both enums and the dispatch match.
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - Manual (veloren-run, pre-V71 character loading as Adventurer): `/set_class mage` → "Class set to Mage."; `/set_class rogue` again → rejection; relog → class survives and `SELECT alias, class FROM character;` shows `Mage`.
- **Size:** M

## CLS-12 — Lint, format, changelog, branch finish

- **Model:** haiku — prescribed check commands plus two changelog lines; escalate only if clippy/test failures need real fixes.
- **Depends on:** CLS-11 (and all prior CLS tasks).
- **Branch / commit:** `feature/classes-races` — `docs: changelog entry for classes and races`; then finish via `superpowers:finishing-a-development-branch` (run `veloren-review` before merging into `development`).
- **Files:**
  - Create: none
  - Modify: `CHANGELOG.md`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 12' steps 1–6 verbatim. The publish-profile clippy specifically guards that the char-selection UI changes don't depend on hot-reloading-only code. Phase 2/3 follow-ups (class skill content, diary tab, ability gating, equipment Spec B) stay tracked in the design spec.
- **Acceptance:**
  - `cargo clippy --all-targets --locked --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" -- -D warnings` → clean.
  - `cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings` → clean.
  - `cargo fmt --all -- --check` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server -p veloren-common-net` → PASS.
- **Size:** S
