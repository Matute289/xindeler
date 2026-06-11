# Character Levels (M2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The level-up moment becomes *felt*: a dedicated SFX, a chat toast ("Level up! You reached level N"), the level visible in the Diary's Character page, and a server-side `telemetry!` event for the M3 balance pass.

**Architecture:** M1 is merged — `Outcome::CharacterLevelUp { uid, new_level }` already flows server→client (`common/src/outcome.rs:66`, emitted in `handle_exp_gain` at `server/src/events/entity_manipulation.rs:552-558`). M2 only adds *consumers*: a new `SfxEvent::CharacterLevelUp` (reusing the existing skill-point sound file — acceptable per spec v1), a new arm in `Hud::handle_outcome` pushing a localized chat message, a new `CharacterStat::Level` row in the diary, and one `telemetry!` call next to the existing emit. The sfx outcome match (`voxygen/src/audio/sfx/mod.rs:994`) and the HUD outcome match (`voxygen/src/hud/mod.rs` end of `handle_outcome`) both end in `_ => {}`, so the new arms are purely additive — place them deliberately, the compiler will *not* remind you. Social list / character-select display from the spec's M2 row is explicitly deferred.

**Tech Stack:** Rust nightly (2024 edition), specs ECS, conrod HUD, fluent i18n (`.ftl`), RON assets. Design spec: `docs/superpowers/specs/2026-06-10-character-levels-design.md`.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/character-levels-m2` off `development` before Task 1.
- Invoke the `veloren-progression` skill for context and the `superpowers:test-driven-development` skill before writing code.

---

### Task 1: Dedicated level-up SFX

**Files:**
- Modify: `assets/voxygen/audio/sfx.ron` (new entry after the `SkillPointGain` block at lines 1693–1699)
- Modify: `voxygen/src/audio/sfx/mod.rs:158` (new `SfxEvent` variant) and `:777-784` (new outcome arm after the `SkillPointGain` arm)
- Modify: `assets/voxygen/i18n/en/hud/subtitles.ftl:111` area (new subtitle key)

- [ ] **Step 1: Write the failing asset change first**

In `assets/voxygen/audio/sfx.ron`, directly after the `SkillPointGain` entry (lines 1693–1699, which map to `voxygen.audio.sfx.character.level_up_sound_-_shorter_wind_up`), add:

```ron
    CharacterLevelUp: (
        files: [
            "voxygen.audio.sfx.character.level_up_sound_-_shorter_wind_up",
        ],
        threshold: 0.2,
        subtitle: "subtitle-character_level_up",
    ),
```

(Reusing the existing sound file is the spec's v1 choice — no new audio asset, so the `new_sfx_credited` test is unaffected.)

- [ ] **Step 2: Run the existing RON-load test to verify it fails**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen test_load_sfx_triggers`
Expected: FAIL — `SfxTriggers::load_expect` panics with a RON deserialize error naming the unknown enum variant `CharacterLevelUp` (the map key type is `SfxEvent`, see `pub type SfxTriggers = Ron<HashMap<SfxEvent, SfxTriggerItem>>` at `voxygen/src/audio/sfx/mod.rs:398`).

- [ ] **Step 3: Add the enum variant**

In `voxygen/src/audio/sfx/mod.rs`, in `pub enum SfxEvent` directly after `SkillPointGain,` (line 158), add:

```rust
    CharacterLevelUp,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen test_load_sfx_triggers`
Expected: PASS (1 test).

- [ ] **Step 5: Play it from the outcome handler**

In `voxygen/src/audio/sfx/mod.rs`, inside `pub fn handle_outcome` (`match outcome` starts at line 490; `uids` is already bound at line 486), directly after the `Outcome::SkillPointGain` arm (lines 777–784) and before `Outcome::Beam`, add:

```rust
            Outcome::CharacterLevelUp { uid, .. } => {
                if let Some(client_uid) = uids.get(client.entity())
                    && uid == client_uid
                {
                    let sfx_trigger_item = triggers.0.get_key_value(&SfxEvent::CharacterLevelUp);
                    audio.emit_ui_sfx(sfx_trigger_item, Some(0.4), Some(UiChannelTag::LevelUp));
                }
            },
```

This match ends in `_ => {}` (line 994), so without this arm the outcome is silently dropped — do not rely on the compiler here. Note: `emit_ui_sfx` skips playback when a UI channel already plays with the same tag (`voxygen/src/audio/mod.rs:683-688`), so a level-up landing on the same tick as a skill-point gain plays exactly one sound — desired.

- [ ] **Step 6: Add the subtitle string**

In `assets/voxygen/i18n/en/hud/subtitles.ftl`, next to `subtitle-skill_point = Skill Point gained` (line 111), add:

```ftl
subtitle-character_level_up = Level up
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n`
Expected: PASS (the i18n tests load and validate every `.ftl` file).

- [ ] **Step 7: Verify build**

Run: `cargo check -p veloren-voxygen`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add assets/voxygen/audio/sfx.ron assets/voxygen/i18n/en/hud/subtitles.ftl voxygen/src/audio/sfx/mod.rs
git commit -m "feat: dedicated SFX for character level-up"
```

---

### Task 2: Chat toast on own level-up

**Files:**
- Modify: `assets/voxygen/i18n/en/hud/misc.ftl` (new message key)
- Modify: `voxygen/src/hud/mod.rs` (`handle_outcome`, new arm after the `SkillPointGain` arm at lines 5400–5417)

- [ ] **Step 1: Add the i18n string first and verify it loads**

In `assets/voxygen/i18n/en/hud/misc.ftl` (keys there use the `hud-` prefix, e.g. `hud-waypoint_saved = Waypoint Saved`), add near the top:

```ftl
hud-level_up_msg = Level up! You reached level { $level }.
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n`
Expected: PASS.

- [ ] **Step 2: Push the chat message from `Hud::handle_outcome`**

In `voxygen/src/hud/mod.rs`, inside `pub fn handle_outcome` (line 5363), directly after the `Outcome::SkillPointGain { .. }` arm (ends line 5417) and before `Outcome::ComboChange`, add:

```rust
            Outcome::CharacterLevelUp { uid, new_level } => {
                let ecs = client.state().ecs();
                let uids = ecs.read_storage::<Uid>();
                let me = scene_data.viewpoint_entity;

                if uids.get(me).is_some_and(|me| *me == *uid) {
                    self.new_messages.push_back(comp::ChatType::Meta.into_msg(
                        Content::localized_with_args("hud-level_up_msg", [(
                            "level",
                            u64::from(*new_level),
                        )]),
                    ));
                }
            },
```

Anchors verified: this match also ends in `_ => {}` (additive arm, no compiler help); `Content` is already imported (`voxygen/src/hud/mod.rs:100`); `comp::ChatType::CommandInfo.into_plain_msg(...)` is used the same way at line 4057 and `ChatType::Meta.into_msg(Content::localized(...))` at `voxygen/src/session/mod.rs:337`; `LocalizationArg` has `impl From<u64>` (`common/i18n/src/lib.rs:87`); `ChatType::Meta` has no source `Uid`, so it shows in the chatbox (`chat::show_in_chatbox` only filters `ChatType::Npc`) and produces no speech bubble.

- [ ] **Step 3: Verify build**

Run: `cargo check -p veloren-voxygen`
Expected: clean. If you get `mismatched types` on the args array, the fix is the explicit `u64::from(*new_level)` shown above — `new_level` is `u16` and `LocalizationArg` only implements `From<u64>` for integers.

- [ ] **Step 4: Commit**

```bash
git add assets/voxygen/i18n/en/hud/misc.ftl voxygen/src/hud/mod.rs
git commit -m "feat: chat toast on own character level-up"
```

---

### Task 3: Level in the Diary character page

**Files:**
- Modify: `voxygen/src/hud/diary.rs:3189-3233` (`STAT_COUNT`, `CharacterStat` enum, `localized_str`) and `:1222-1235` (value match)
- Modify: `assets/voxygen/i18n/en/hud/char_window.ftl` (new label key)

- [ ] **Step 1: Add the enum variant and let the compiler find every match**

In `voxygen/src/hud/diary.rs`, the stats page iterates `CharacterStat::iter()` (line 1171) and renders one row per variant. Add `Level` right after `Name` (line 3194) so it renders as the second row:

```rust
#[derive(EnumIter)]
enum CharacterStat {
    Name,
    Level,
    BattleMode,
    // ... rest unchanged
}
```

- [ ] **Step 2: Compiler-driven match fixes**

Run: `cargo check -p veloren-voxygen 2>&1 | grep -B2 -A6 "E0004\|non-exhaustive"`
Expected: exactly two `E0004` (non-exhaustive patterns) errors, both in `diary.rs`:

1. The value match at `diary.rs:1223` (`let value = match stat {`). Add after the `CharacterStat::Name => name,` arm — `self.skill_set: &SkillSet` is already a field (line 190) and `character_level()` exists from M1:

```rust
                        CharacterStat::Level => {
                            format!("{}", self.skill_set.character_level())
                        },
```

2. The label match in `localized_str` at `diary.rs:3215`. Add after the `Name =>` arm:

```rust
            Level => i18n.get_msg("character_window-character_level"),
```

- [ ] **Step 3: Bump the widget-id count**

Still in `diary.rs`, line 3190 — the conrod id lists `stat_names`/`stat_values` are resized to this constant (lines 1160–1167), and the render loop indexes them per enum variant, so forgetting this panics at runtime, not compile time:

```rust
/// The number of variants of the [`CharacterStat`] enum.
const STAT_COUNT: usize = 16;
```

- [ ] **Step 4: Add the label string**

In `assets/voxygen/i18n/en/hud/char_window.ftl`, after `character_window-character_name = Character name` (line 1), add:

```ftl
character_window-character_level = Level
```

- [ ] **Step 5: Verify build and i18n**

Run: `cargo check -p veloren-voxygen`
Expected: clean (zero `E0004`).
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-client-i18n`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add voxygen/src/hud/diary.rs assets/voxygen/i18n/en/hud/char_window.ftl
git commit -m "feat: show character level in diary character page"
```

---

### Task 4: Server-side telemetry on level-up

**Files:**
- Modify: `server/src/events/entity_manipulation.rs:552-558` (inside the level-up branch of `handle_exp_gain`)
- Modify: `.claude/skills/veloren-telemetry/SKILL.md` (event-code table, line ~251)

- [ ] **Step 1: Emit the event**

In `server/src/events/entity_manipulation.rs`, function `handle_exp_gain` (line 505): the level-up branch currently reads (lines 552–558):

```rust
    let level_after = skill_set.character_level();
    if level_after > level_before {
        outcomes_emitter.emit(Outcome::CharacterLevelUp {
            uid: *uid,
            new_level: level_after,
        });
    }
```

Extend the `if` body so it becomes:

```rust
    let level_after = skill_set.character_level();
    if level_after > level_before {
        outcomes_emitter.emit(Outcome::CharacterLevelUp {
            uid: *uid,
            new_level: level_after,
        });
        common::telemetry!(
            "lvl",
            event = "character_level_up",
            uid = ?uid,
            new_level = level_after
        );
    }
```

Pattern verified against existing calls in the same file: `common::telemetry!("ch", dst = ?uid.copied(), dmg, ...)` at line 330 and `common::telemetry!("pd", uid = ?uid, ...)` at line 1092 (the `event =` field follows the `telemetry!("pd_conn", event = "disconnect", ...)` precedent in `server/src/events/player.rs:227`). The macro is defined at `common/src/lib.rs:24` and needs no import — the fully qualified `common::telemetry!` path is the house style.

- [ ] **Step 2: Verify build**

Run: `cargo check -p veloren-server`
Expected: clean.

- [ ] **Step 3: Document the event code**

In `.claude/skills/veloren-telemetry/SKILL.md`, add a row to the event-code table (the `| Code | Meaning | Key fields |` table at line ~251), after the `pd` row:

```markdown
| `lvl` | Character level-up (server) | `event`, `uid`, `new_level` |
```

- [ ] **Step 4: Commit**

```bash
git add server/src/events/entity_manipulation.rs .claude/skills/veloren-telemetry/SKILL.md
git commit -m "feat: telemetry event on character level-up"
```

---

### Task 5: In-game verification

- [ ] **Step 1: Launch and trigger a level-up**

Use the `veloren-run` skill to launch server + client (logging-verbose build if available). On a *fresh* character, kill weak mobs near spawn until lifetime XP crosses `LEVEL_XP_BASE = 250` (`common/src/comp/skillset/mod.rs`) — a handful of critters reaches level 2. Mob kills are the only trigger: `handle_exp_gain` is the sole emitter of `Outcome::CharacterLevelUp`, and the admin command `/skill_point` grants SP directly without touching `earned_exp`, so it will NOT fire the outcome (there is no `/give_exp` command).

- [ ] **Step 2: Confirm all four surfaces**

- Chat shows "Level up! You reached level 2." (Task 2).
- The level-up sound plays once (Task 1) — if the same kill also crosses a skill-point boundary, still exactly one sound is correct (shared `UiChannelTag::LevelUp` dedupes).
- Diary (default key `L`) → Character section shows a `Level` row with value `2` directly under the character name (Task 3).
- Telemetry log contains the event (Task 4) — with the logging-verbose build, use the `veloren-telemetry` skill:
  `grep '"t":"lvl"' userdata/voxygen/logs/*telemetry*.jsonl` → one line with `"event":"character_level_up"` and `"new_level":2`.

No commit for this task; fix anything found before moving on.

---

### Task 6: Lint, format, changelog, and branch finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. Fix any warnings (do not `#[allow]` without a comment justifying it).

- [ ] **Step 2: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check.

- [ ] **Step 3: Test suite**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-client-i18n`
Expected: PASS (includes the M1 `character_level` tests and the `.ftl` validation).
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen test_load_sfx_triggers`
Expected: PASS.

- [ ] **Step 4: Update CHANGELOG and commit**

Add under `### Added` in the `## [Unreleased]` section of `CHANGELOG.md`, after the existing M1 line ("Characters now have a level (1–60)…"):

```markdown
- Level-ups now play a sound, announce the new level in chat, and the character level is shown in the Diary's character page.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for character levels M2"
```

- [ ] **Step 5: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). The remaining spec M2 items (level in social list and character-select) and the M3 balance pass stay tracked in the design spec's milestone table.
