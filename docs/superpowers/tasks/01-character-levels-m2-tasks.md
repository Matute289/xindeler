# Character Levels (M2) — Task Board

**Source plan:** [../plans/2026-06-11-character-levels-m2.md](../plans/2026-06-11-character-levels-m2.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> Escalation rule: If acceptance fails twice, escalate one model tier and leave a note in the task file.

> Branch setup (before LV2-1): create `feature/character-levels-m2` off `development`. All tasks commit to this branch.

## LV2-1 — Dedicated level-up SFX

- **Model:** haiku — RON entry, enum variant, match arm, and .ftl line are all given verbatim with exact line anchors.
- **Depends on:** none (M1 is merged on `development`).
- **Branch / commit:** `feature/character-levels-m2` — `feat: dedicated SFX for character level-up`
- **Files:**
  - Create: none
  - Modify: `assets/voxygen/audio/sfx.ron`, `voxygen/src/audio/sfx/mod.rs`, `assets/voxygen/i18n/en/hud/subtitles.ftl`
  - Delete: none
- **Assets:** No new audio file — the spec's v1 choice reuses `voxygen.audio.sfx.character.level_up_sound_-_shorter_wind_up` (already credited; `new_sfx_credited` test unaffected). The `sfx.ron` `CharacterLevelUp` entry and `subtitle-character_level_up` .ftl line — Claude creates: inline text copied from the plan.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–8 verbatim. WARNING: the `handle_outcome` match in `voxygen/src/audio/sfx/mod.rs` ends in `_ => {}` (line 994) — the new arm is purely additive and the compiler will NOT remind you; place it exactly after the `SkillPointGain` arm (lines 777–784) and before `Outcome::Beam`. TDD order matters: add the RON entry first and confirm the test FAILS before adding the enum variant.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen test_load_sfx_triggers` → FAIL after Step 1 (RON deserialize error naming unknown variant `CharacterLevelUp`), PASS (1 test) after Step 3.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n` → PASS.
  - `cargo check -p veloren-voxygen` → clean.
- **Size:** S

## LV2-2 — Chat toast on own level-up

- **Model:** haiku — two-file edit with the exact arm code (including the `u64::from(*new_level)` fix) and the .ftl line given verbatim; all anchors pre-verified in the plan.
- **Depends on:** none (independent of LV2-1; runs after it in plan order on the shared branch).
- **Branch / commit:** `feature/character-levels-m2` — `feat: chat toast on own character level-up`
- **Files:**
  - Create: none
  - Modify: `assets/voxygen/i18n/en/hud/misc.ftl`, `voxygen/src/hud/mod.rs`
  - Delete: none
- **Assets:** `hud-level_up_msg` key in `assets/voxygen/i18n/en/hud/misc.ftl` — Claude creates: .ftl text inline from the plan.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–4 verbatim. WARNING: `Hud::handle_outcome` also ends in `_ => {}` — additive arm, no compiler help; place after the `Outcome::SkillPointGain` arm (ends line 5417), before `Outcome::ComboChange`. If `mismatched types` appears on the args array, the plan's prescribed fix is `u64::from(*new_level)` (LocalizationArg only implements `From<u64>` for integers).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n` → PASS.
  - `cargo check -p veloren-voxygen` → clean.
- **Size:** S

## LV2-3 — Level in the Diary character page

- **Model:** sonnet — compiler-driven resolution of two `E0004` non-exhaustive matches plus a runtime-only trap (widget-id count) that panics instead of failing compilation.
- **Depends on:** none (independent feature; plan order on shared branch).
- **Branch / commit:** `feature/character-levels-m2` — `feat: show character level in diary character page`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/hud/diary.rs`, `assets/voxygen/i18n/en/hud/char_window.ftl`
  - Delete: none
- **Assets:** `character_window-character_level = Level` key in `assets/voxygen/i18n/en/hud/char_window.ftl` — Claude creates: .ftl text inline from the plan.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–6 verbatim. Sequencing: add the `Level` enum variant first, then let `cargo check` surface exactly two `E0004` errors (value match at diary.rs:1223, label match at :3215) and fix both with the plan's arms. WARNING: Step 3's `STAT_COUNT` bump to 16 is NOT compiler-enforced — forgetting it panics at runtime when the conrod id lists are indexed; do not skip it.
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean (zero `E0004`).
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n` → PASS.
- **Size:** S

## LV2-4 — Server-side telemetry on level-up

- **Model:** haiku — single-site code extension with the exact `common::telemetry!` call given, plus one markdown table row.
- **Depends on:** none (M1's level-up branch already exists at `server/src/events/entity_manipulation.rs:552-558`).
- **Branch / commit:** `feature/character-levels-m2` — `feat: telemetry event on character level-up`
- **Files:**
  - Create: none
  - Modify: `server/src/events/entity_manipulation.rs`, `.claude/skills/veloren-telemetry/SKILL.md`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 4' steps 1–4 verbatim. Use the fully-qualified `common::telemetry!` path (house style, no import needed); the `event =` field follows the `telemetry!("pd_conn", event = "disconnect", ...)` precedent.
- **Acceptance:**
  - `cargo check -p veloren-server` → clean.
- **Size:** S

## LV2-5 — In-game verification

- **Model:** sonnet — requires launching server+client via the `veloren-run` skill, triggering a level-up by gameplay, observing four surfaces, and debugging anything that mismatches.
- **Depends on:** LV2-1, LV2-2, LV2-3, LV2-4.
- **Branch / commit:** `feature/character-levels-m2` — no commit for this task (fix anything found before moving on; fixes amend/extend the relevant earlier commit scope).
- **Files:**
  - Create: none
  - Modify: none (unless fixes are needed)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none (uses the `veloren-run` skill; logging-verbose build if available, plus the `veloren-telemetry` skill for log inspection).
- **Steps:** Follow plan section '### Task 5' steps 1–2 verbatim. WARNING: use a FRESH character and kill weak mobs until lifetime XP crosses `LEVEL_XP_BASE = 250` — mob kills are the ONLY trigger; `/skill_point` does NOT fire the outcome and there is no `/give_exp` command.
- **Acceptance:** All four surfaces confirmed on reaching level 2:
  - Chat shows "Level up! You reached level 2."
  - Level-up sound plays exactly once (shared `UiChannelTag::LevelUp` dedupes against a simultaneous skill-point gain).
  - Diary (key `L`) → Character section shows `Level` row with value `2` under the name.
  - `grep '"t":"lvl"' userdata/voxygen/logs/*telemetry*.jsonl` → one line with `"event":"character_level_up"` and `"new_level":2`.
- **Size:** M

## LV2-6 — Lint, format, changelog, and branch finish

- **Model:** haiku — running prescribed check commands and a one-line changelog entry; escalate only if clippy/test failures need real fixes.
- **Depends on:** LV2-5.
- **Branch / commit:** `feature/character-levels-m2` — `docs: changelog entry for character levels M2`; then finish the branch via `superpowers:finishing-a-development-branch` (run `veloren-review` before merging into `development`).
- **Files:**
  - Create: none
  - Modify: `CHANGELOG.md`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 6' steps 1–5 verbatim. Do not add `#[allow]` without a justifying comment. Social-list / character-select display and the M3 balance pass stay deferred per the design spec.
- **Acceptance:**
  - `cargo clippy --all-targets --locked --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" -- -D warnings` → clean.
  - `cargo fmt --all -- --check` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-client-i18n` → PASS.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen test_load_sfx_triggers` → PASS.
- **Size:** S
