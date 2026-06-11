# Magic Abilities v1 — Task Board

**Source plan:** [../plans/2026-06-11-magic-abilities.md](../plans/2026-06-11-magic-abilities.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> Escalation rule: If acceptance fails twice, escalate one model tier and leave a note in the task file.

> Branch setup (before MAG-P1.1): create `feature/magic-v1` off `development`. All tasks commit to this branch. Invoke the `veloren-abilities` skill for pipeline context and `superpowers:test-driven-development` before writing code. Never silence a non-exhaustive match with `_ =>`.

> Plan IDs are `MAG-P<phase>.<plan task number>`. Phases 1–3 have NO external dependency; only MAG-P4.15 depends on the classes-races plan (CLS tasks in `03-classes-races-tasks.md`).

## MAG-P1.1 — `SpellSchool`, `AbilityMeta.cooldown`, and the `AbilityCooldowns` component

- **Model:** opus — adds a new client-synced component (extends `synced_components.rs`, i.e. changes netcode) plus `AbilityMeta` fields every existing ability RON deserializes through; a mistake here breaks sync or asset loading globally.
- **Depends on:** none.
- **Branch / commit:** `feature/magic-v1` — `feat: AbilityCooldowns component, SpellSchool, AbilityMeta.cooldown`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/ability.rs`, `common/src/comp/mod.rs`, `common/state/src/state.rs`, `common/net/src/synced_components.rs`, `server/src/state_ext.rs`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–6 verbatim. TDD: write the three `ability_cooldown_tests` first and confirm compile failure before implementing. Both new `AbilityMeta` fields MUST be `#[serde(default)]` and `Copy` (the struct has `deny_unknown_fields` and derives `Default`/`Copy`) — otherwise every existing ability RON stops deserializing.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common ability_cooldown` → 3 PASS.
  - `cargo check --workspace --all-targets` → clean.
- **Size:** M

## MAG-P1.2 — Cooldown gate in `handle_ability` + `SetAbilityCooldownEvent`

- **Model:** sonnet — real code given for the event/handler/gate, but plumbing `ability_cooldowns` through `JoinData`/`JoinStruct` and the character_behavior system join is compiler-driven multi-file wiring.
- **Depends on:** MAG-P1.1 (component + meta field).
- **Branch / commit:** `feature/magic-v1` — `feat: gate abilities on per-spell cooldowns in handle_ability`
- **Files:**
  - Create: none
  - Modify: `common/src/states/behavior.rs`, `common/systems/src/character_behavior.rs`, `common/src/states/utils.rs`, `common/src/event.rs`, `server/src/events/event_types.rs`, `server/src/events/entity_manipulation.rs`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–5 verbatim. ORDER TRAP: the cooldown check must go FIRST in the existing `.filter(...)` in `handle_ability` — `requirements_paid` mutates `update`, so energy would be paid for an on-cooldown ability otherwise. Register the event exactly like `TeleportToEvent` (import + macro list at `event_types.rs:15,74`, `event_dispatch` at `entity_manipulation.rs:102`, handler next to line 2745). Copy how `active_abilities` flows through `character_behavior.rs` for the new `ReadStorage` + `.maybe()`.
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` → PASS, no regressions.
  - (Manual double-cast check deferred until Phase 4 content lands — noted in plan.)
- **Size:** M

## MAG-P1.3 — `AuxiliaryAbility::Innate` + `AbilityPool` component

- **Model:** opus — extends the `Ability`/`AuxiliaryAbility` enums with workspace-wide signature changes (`ability_id`, `activate_ability`, `all_available_abilities` gain parameters), threads real components through HUD call sites, and adds another synced component (netcode).
- **Depends on:** MAG-P1.1 (registration/sync/insertion pattern repeated for `AbilityPool`); MAG-P1.2 (`JoinData` plumbing pattern reused).
- **Branch / commit:** `feature/magic-v1` — `feat: AbilityPool component and AuxiliaryAbility::Innate resolution`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/ability.rs`, `common/src/comp/mod.rs`, `common/state/src/state.rs`, `common/net/src/synced_components.rs`, `server/src/state_ext.rs`, `common/src/states/behavior.rs`, `common/src/states/utils.rs`, `voxygen/src/hud/skillbar.rs`, `voxygen/src/hud/diary.rs`, `voxygen/src/hud/mod.rs`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–5 verbatim. Compiler-driven: iterate `cargo check` greps for `non-exhaustive|InnateAux|arguments` and fill EVERY arm/call site listed in the plan (`try_ability_set_key`, `is_from_wielded`, both `ability_id`s, `activate_ability`, `all_available_abilities`). NEVER pass a wildcard; in HUD sites thread the real synced `AbilityPool` (or `None` where the entity is not the player).
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` → PASS.
- **Size:** L

## MAG-P1.4 — Persistence arms for `Innate` + round-trip test

- **Model:** opus — touches character DB persistence (`ability_sets` string round-trip); per routing policy anything that can corrupt saves goes to opus, even though both arms are small and modeled byte-for-byte on the `Glider` arm.
- **Depends on:** MAG-P1.3 (`AuxiliaryAbility::Innate` exists).
- **Branch / commit:** `feature/magic-v1` — `feat: persist AuxiliaryAbility::Innate in ability_sets`
- **Files:**
  - Create: none
  - Modify: `server/src/persistence/json_models.rs`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** a dev character DB copy for the dry-run migration check (copy, start server against it, log in — old `ability_sets` strings must load unchanged).
- **Steps:** Follow plan section '### Task 4' steps 1–4 verbatim. The compile error from the non-exhaustive match in `aux_ability_to_string` IS the to-do list. The `from_string` arm must keep the same two `dev_panic!` fallbacks to `AuxiliaryAbility::Empty` as the `Some("Glider")` arm (lines 182–202). Serialized format is fixed: `"Innate:index:3"`.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server innate_aux` → PASS.
  - Dry-run DB migration check: old saves load unchanged.
- **Size:** S

## MAG-P2.5 — `CharacterAbility::GroundAoe` variant

- **Model:** opus — new `CharacterAbility` variant (explicit opus per routing policy); exhaustive-match fan-out across `energy_cost`/`ability_meta`/`adjusted_by_stats`/`try_from`.
- **Depends on:** MAG-P1.1 (`AbilityMeta.cooldown`/`school` referenced by the variant's `meta`). MUST be executed in the same working session as MAG-P2.6 — the `try_from` arm is allowed to be `todo!()` only between this task's Step 2 and Task 6 Step 2; it must NOT be committed.
- **Branch / commit:** `feature/magic-v1` — committed together with MAG-P2.6 as `feat: GroundAoe character state and ability variant`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/ability.rs`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 5' steps 1–3 verbatim. Model the `adjusted_by_stats` arm on the Explosion arm (lines 1877–1904); fold `GroundAoe { energy_cost, .. }` into the existing Explosion `energy_cost` arm and `ability_meta()` arm. Do not commit separately — see Task 6 Step 5.
- **Acceptance:** covered by MAG-P2.6's acceptance (`cargo check -p veloren-common` shows no non-exhaustive matches once arms are filled; commit happens in Task 6).
- **Size:** S

## MAG-P2.6 — `states/ground_aoe.rs` + dispatch + Shatterburst RON

- **Model:** opus — brand-new character state (explicit opus per routing policy: new CharacterAbility states); full state machine with client-aim trust model, `CharacterState` enum dispatch, and accessor-match fan-out.
- **Depends on:** MAG-P2.5 (same session, shared commit); MAG-P1.1 (meta fields used in the RON).
- **Branch / commit:** `feature/magic-v1` — `feat: GroundAoe character state and ability variant`
- **Files:**
  - Create: `common/src/states/ground_aoe.rs`, `assets/common/abilities/spells/ruin/shatterburst.ron`
  - Modify: `common/src/states/mod.rs` (alphabetical insert after `glide_wield`), `common/src/comp/character_state.rs`, `common/src/comp/ability.rs` (`try_from` arm + `ground_aoe_tests` module)
  - Delete: none
- **Assets:**
  - `assets/common/abilities/spells/ruin/shatterburst.ron` — RON config, Claude creates inline (full text in plan; buildup 1.2 s, no `POISE_RESISTANT` — interruptible per spec §8). Strike FX/SFX reuse the existing `ExplosionEvent` → `Outcome::Explosion` path via `reagent: Some(Red)` — no new audio/visual asset.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 6' steps 1–5 verbatim. The state file is given in full — same skeleton as `states/explosion.rs`, target locked at Buildup→Action from `input_attr.select_pos` (Blink's channel). `handle_interrupts` goes at the END of `behavior()` so poise breaks cancel the cast. Fill every `CharacterState` accessor match (`ability_info()`, `stage_section()`, `durations()`, `timer()`, …) following `Explosion(data)` — never `_ =>`. Run the test BEFORE creating the RON (expect load failure) and after (PASS).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common ground_aoe shatterburst` → PASS.
  - `cargo check --workspace --all-targets` → clean.
- **Size:** L

## MAG-P2.7 — GroundAoe telegraph FX

- **Model:** sonnet — particle code is given verbatim but the acceptance is an in-game visual judgment call (telegraph readable at 30 m, interrupt behavior), which haiku can't evaluate.
- **Depends on:** MAG-P2.6 (state + Shatterburst exist).
- **Branch / commit:** `feature/magic-v1` — `feat: GroundAoe telegraph particle ring`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/scene/particle.rs` (new arm next to `CharacterState::Blink`, line 1951)
  - Delete: none
- **Assets:** reuse existing asset: `ParticleMode::CultistFlame` as the warning-ring placeholder (documented `TODO(magic-v1 polish): dedicated decal/ParticleMode`). No new particle/decal asset in v1.
- **Downloads/tools:** `veloren-run` skill for the in-game check. Temporary test binding: the Task 11 Tome, or temporarily set the Staff primary to the shatterburst id and REVERT before commit.
- **Steps:** Follow plan section '### Task 7' steps 1–3 verbatim. In-game verify: aim point locks where the crosshair hit terrain, particle ring marks the area ~1 s, explosion lands on the ring, poise-stun during the 1.2 s buildup cancels the cast; telegraph readable in 3rd person at 30 m (spec Phase-3 milestone).
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean.
  - In-game checklist above passes.
- **Size:** S

## MAG-P2.8 — BuffKinds `Terrified`, `Charmed`, `Hollowtouched`

- **Model:** sonnet — enum extension with compiler-driven match completion across common + voxygen; effect code given but several sites ("treat like `Rooted`"/"like `Cursed`") require pattern judgment.
- **Depends on:** none (independent of P1/P2 cooldown work; plan order on shared branch).
- **Branch / commit:** `feature/magic-v1` — `feat: Terrified, Charmed, Hollowtouched buff kinds`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/buff.rs`, `voxygen/src/hud/util.rs`, `voxygen/src/hud/mod.rs`, `assets/voxygen/i18n/en/buff.ftl`
  - Delete: none
- **Assets:**
  - Buff icons — reuse existing assets via `voxygen/src/hud/img_ids.rs`: `imgs.debuff_rooted_0` (Terrified), `imgs.debuff_amnesia_0` (Charmed), `imgs.debuff_cursed_0` (Hollowtouched), with `TODO(magic-v1 polish): dedicated icons`. If names differ at `img_ids.rs:959` area, use whatever the `Amnesia`/`Cursed` arms in the same match use.
  - `.ftl` strings (`buff-terrified`, `buff-charmed`, `buff-hollowtouched`) — Claude creates inline (full text in plan, insert before `## Util`).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 8' steps 1–4 verbatim. Iterate `cargo check --workspace --all-targets | grep non-exhaustive` until clean: `differentiate()` → `SimpleNegative`; `stacks()` adds `Hollowtouched`; duration-default match (~buff.rs:757) — Terrified/Charmed like `Rooted`, Hollowtouched like `Cursed`; `util.rs:369` strength formatting groups with `Rooted`. No wildcard arms.
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-voxygen` → PASS (i18n tests validate the ftl).
- **Size:** M

## MAG-P2.9 — Agent reactions + spells #15 Dread Whisper, #8 Censure

- **Model:** sonnet — agent helpers given but the hostile-decision-path hook must be located by grep and adapted to surrounding locals; RON content follows existing patterns with a grep-verify step for `init_event` syntax.
- **Depends on:** MAG-P2.8 (`Terrified`/`Charmed`/`Hollowtouched` BuffKinds); MAG-P1.1 (`meta.cooldown`/`school` in the RONs).
- **Branch / commit:** `feature/magic-v1` — `feat: agent fear/charm reactions plus Dread Whisper and Censure`
- **Files:**
  - Create: `assets/common/abilities/spells/hollow/dread_whisper.ron`, `assets/common/abilities/spells/gravesong/censure.ron`
  - Modify: `server/agent/src/action_nodes.rs`
  - Delete: none
- **Assets:** both RONs — Claude creates inline (full text in plan). Projectile visuals reuse `Object(BoltFire)` + `Reagent::Purple` — no new assets.
- **Downloads/tools:** none. Note: `server-agent` is a hot-reloadable dylib in dev — fast iteration loop.
- **Steps:** Follow plan section '### Task 9' steps 1–4 verbatim. TRAPS: (1) before writing `dread_whisper.ron`, grep `init_event` in `assets/common/abilities` and mirror the existing RON syntax exactly; (2) buff-import paths follow `server/agent/src/attack.rs:810` / `:2868` — if `iter_kind` yields a different item shape, adapt to what `attack.rs:2868` destructures (compiler arbitrates); (3) `tgt_pos`/`target_uid` in the hook must reuse the surrounding function's existing bindings. Boss Charm immunity is deliberately deferred.
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - In-game (after MAG-P3.11 binds the spells): Dread Whisper routs a gnarling pack and applies Hollowtouched to the caster; Censure blocks a staff NPC's auxiliary casts.
- **Size:** M

## MAG-P3.10 — `ToolKind::{Tome, HolySymbol, Focus}`

- **Model:** sonnet — central arms + persistence strings are given verbatim, but the workspace-wide compiler-driven sweep (voxygen animation/hud wield poses) requires per-site judgment ("group with `Staff` at that site").
- **Depends on:** none (Phase 3 is self-contained; plan order on shared branch).
- **Branch / commit:** `feature/magic-v1` — `feat: Tome, HolySymbol, and Focus caster tool kinds`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/inventory/item/tool.rs`, `server/src/persistence/json_models.rs`, plus every exhaustive `ToolKind` match the compiler reports (voxygen animation/hud, item code)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 10' steps 1–4 verbatim. DELIBERATE NON-CHANGES: `gains_combat_xp` stays untouched (keeps `SkillGroupKind::Weapon(Tome)` out of the DB string mapping — avoids a DB enum migration); `can_block` stays untouched. `tool_kind_to_string`/`from_string` arms ARE required (`AuxiliaryKey` serializes equipped tool kinds). Sweep `cargo check --workspace --all-targets | grep non-exhaustive` until clean; in each site add the three kinds to whichever arm `Staff`/`Sceptre` share there. No wildcard arms.
- **Acceptance:**
  - `cargo check --workspace --all-targets` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` → PASS.
- **Size:** M

## MAG-P3.11 — Starter implements + manifest ability sets

- **Model:** haiku — pure RON/manifest additions with full text in the plan, modeled on verified `starter_staff.ron`; missing spell ids fall back gracefully until MAG-P4.12/13 land.
- **Depends on:** MAG-P3.10 (the three ToolKinds must exist for the items to deserialize); MAG-P2.6 (shatterburst id referenced in the Tome set).
- **Branch / commit:** `feature/magic-v1` — `feat: starter caster implements with manifest ability sets`
- **Files:**
  - Create: `assets/common/items/weapons/tome/apprentice_tome.ron`, `assets/common/items/weapons/holy_symbol/initiate_symbol.ron`, `assets/common/items/weapons/focus/wanderer_focus.ron`
  - Modify: `assets/voxygen/voxel/biped_weapon_manifest.ron` (~line 1424, after `starter_staff`), `assets/voxygen/item_image_manifest.ron` (~line 1358), `assets/common/abilities/ability_set_manifest.ron` (~line 305, after `Tool(Sceptre)`)
  - Delete: none
- **Assets:**
  - 3 item RONs — Claude creates inline (full Tome text in plan; HolySymbol/Focus differ only in `legacy_name`/`legacy_description`/`kind`). FIX the plan's Cyrillic typo in the Tome description ("студент's" → "student's") and keep descriptions original-IP per `veloren-lore`.
  - Voxel models — reuse existing asset: `weapon.staff.firestaff_starter` (`assets/voxygen/voxel/weapon/staff/`) for all three, with `TODO(magic-v1 polish): dedicated implement models`. Dedicated .vox models for Tome/HolySymbol/Focus are a **fable decision point** deferred to the polish pass — do NOT attempt new voxel art here.
  - Item images — reuse via `VoxTrans("voxel.weapon.staff.firestaff_starter", ...)` per plan.
  - Manifest ability-set entries (skill gates `None` until MAG-P4.15) — Claude creates inline (full text in plan).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 11' steps 1–4 verbatim. NOTE: until MAG-P4.12/13 create the referenced spell RONs, missing ids fall back to `CharacterAbility::default()` with a load warning (`tool.rs:719-723`) — the manifest still parses; the MAG-P4.13 test closes the gap.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` → PASS (item asset tests load the three items).
  - In-game (can be deferred to MAG-P4.13's smoke test): `/give_item common.items.weapons.tome.apprentice_tome` → Shatterburst appears in the Diary; cooldown blocks re-cast for 20 s.
- **Size:** M

## MAG-P4.12 — Template spell Emberlance (#1), worked end-to-end

- **Model:** haiku — RON, .ftl entry, icon arm, and asset-load test all given verbatim; pure mechanical addition.
- **Depends on:** MAG-P1.1 (`meta.school`), MAG-P3.11 (manifest references the id).
- **Branch / commit:** `feature/magic-v1` — `feat: Emberlance, the worked template spell`
- **Files:**
  - Create: `assets/common/abilities/spells/ruin/emberlance.ron`
  - Modify: `assets/voxygen/i18n/en/hud/ability.ftl`, `voxygen/src/hud/util.rs` (icon match at :687), `common/src/comp/ability.rs` (rename Task 6 test module to `spell_asset_tests`, keep shatterburst case, add emberlance test)
  - Delete: none
- **Assets:**
  - `emberlance.ron` — Claude creates inline (full text in plan; cantrip tier — zero energy, no cooldown).
  - i18n key `common-abilities-spells-ruin-emberlance` — Claude creates inline (key = ability id with dots→dashes).
  - Icon — reuse existing asset: `imgs.fire_aoe`, with `TODO(magic-v1 polish): dedicated spell icons`.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 12' steps 1–4 verbatim.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common spell_asset` → PASS.
- **Size:** S

## MAG-P4.13 — Remaining 10 content-only spells (table-driven)

- **Model:** sonnet — new RON spell content following existing donor patterns (explicit sonnet per routing policy): each file requires reading its donor verbatim, grep-verifying buff/beam syntax, and an in-game smoke test per implement.
- **Depends on:** MAG-P4.12 (template + `spell_asset_tests` module), MAG-P2.8 (Hollowtouched for hollow_gate's `init_event`), MAG-P3.11 (manifest ids these files satisfy).
- **Branch / commit:** `feature/magic-v1` — `feat: v1 content-wave spells for all four caster archetypes`
- **Files:**
  - Create: `assets/common/abilities/spells/wardcraft/wardshell.ron`, `threshold/veilrend.ron`, `dawnfire/dawnlight_mend.ron`, `dawnfire/radiant_verdict.ron`, `dawnfire/aegis_of_dawn.ron`, `verdance/thornlash.ron`, `verdance/verdant_mending.ron`, `verdance/galeburst.ron`, `pactbinding/umbral_bolt.ron`, `pactbinding/soul_siphon.ron`, `hollow/hollow_gate.ron` (all under `assets/common/abilities/spells/`)
  - Modify: `assets/voxygen/i18n/en/hud/ability.ftl`, `voxygen/src/hud/util.rs` (10 icon arms), `common/src/comp/ability.rs` (loop test `all_v1_spell_rons_deserialize_to_expected_variants`)
  - Delete: none
- **Assets:**
  - 10 spell RONs — Claude creates inline per the plan's parameter table, copying each donor's FULL field list verbatim before editing values. Donors: `staff/firebomb.ron` (BasicRanged), `sword/heavy_fortitude.ron` (SelfBuff), `custom/cursekeeper/teleport.ron` (Blink), `sceptre/healingaura.ron` (BasicAura), `staff/fireshockwave.ron` (Shockwave), `sceptre/lifestealbeam.ron` (BasicBeam), `sceptre/wardingaura.ron` (StaticAura), and a `BasicSummon` donor found via `grep -rln "BasicSummon(" assets/common/abilities/custom | head -3`. hollow_gate points `summon_info` at an existing low-tier entity config with a TODO for the dedicated servitor entity (spec Phase-2 item).
  - 10 i18n entries — Claude creates inline (Task 12 key format; write original spell names/descs per `veloren-lore`).
  - Icons — reuse existing assets: `imgs.heal_aoe` for heals/auras, `imgs.fire_aoe` for nukes, or whatever neighboring staff/sceptre arms use.
- **Downloads/tools:** `veloren-run` skill for the per-implement smoke test.
- **Steps:** Follow plan section '### Task 13' steps 1–5 verbatim. TRAPS: grep-verify `damage_effect: Some(Buff(...))` beam syntax before writing thornlash/draugr-style effects; check `CharacterAbility::Blink` fields before adding `energy_cost` to veilrend; no buildup ≥ 1.0 s may set `POISE_RESISTANT` (none in the table do). The loop test must list all 13 ids (incl. censure + dread_whisper from MAG-P2.9).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common spell_asset` → PASS only when every file parses.
  - In-game smoke test: each of Tome/HolySymbol/Focus playable with its Task 11 set (spec Phase-2 milestone).
- **Size:** L

## MAG-P4.14 — Racial innates: 6 RONs + creation grant

- **Model:** sonnet — RON table work plus a real edit inside `initialize_character_data` (replacing the MAG-P1.3 default insert, adapting to the local `body` binding) and an end-to-end in-game pipeline proof.
- **Depends on:** MAG-P1.3 (`AbilityPool` + Innate resolution), MAG-P1.4 (binding persistence), MAG-P4.13 (donor patterns + i18n/icon conventions).
- **Branch / commit:** `feature/magic-v1` — `feat: racial innate abilities granted through AbilityPool`
- **Files:**
  - Create: `assets/common/abilities/innate/human.ron`, `elf.ron`, `dwarf.ron`, `orc.ron`, `danari.ron`, `draugr.ron`
  - Modify: `assets/common/abilities/ability_set_manifest.ron` (6 `Custom("innate.<species>")` sets), `server/src/state_ext.rs` (`initialize_character_data` — replace the Task 3 `AbilityPool::default()` insert with the species match), i18n (`common-abilities-innate-<species>` keys) + icon arms as in Task 13
  - Delete: none
- **Assets:**
  - 6 innate RONs — Claude creates inline (human worked in full; table for elf/dwarf/orc/danari/draugr; all `energy_cost: 0`, identity from long cooldowns). draugr: `specifier: Ice` if it exists else `Fire` + TODO; verify `damage_effect: Some(Buff(...))` syntax by grep as in Task 13.
  - i18n + icons — Claude inline / reuse existing icons (set key doubles as frontend ability id).
- **Downloads/tools:** `veloren-run` skill for the Danari pipeline proof.
- **Steps:** Follow plan section '### Task 14' steps 1–4 verbatim. The species list is verified against `common/src/comp/humanoid.rs` `Species` (`body/humanoid.rs:116`); adjust the `body` binding name to the local in `initialize_character_data` (~line 704 region).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` → PASS.
  - In-game: create a Danari → innate in Diary; bind, blink 8 m bare-handed; relog → binding survives (full Path-B pipeline).
- **Size:** M

## MAG-P4.15 — Class skill gating [GATED on classes-races plan]

- **Model:** sonnet — manifest-only edit, but requires reconciling against the classes-races skill enum (exact variant paths from another plan) rather than copying given text.
- **Depends on:** **CLS Task 1 (`Skill::Class`/ClassKind) from `03-classes-races-tasks.md`** — HARD GATE; also MAG-P3.11 (the manifest entries being gated). If the Step 1 grep finds nothing, STOP: leave gates `None` (implement-gated v1 fallback ships) and note the deferral.
- **Branch / commit:** `feature/magic-v1` — `feat: class-tree skill gates on caster implement spells`
- **Files:**
  - Create: none
  - Modify: `assets/common/abilities/ability_set_manifest.ron`
  - Delete: none
- **Assets:** none (manifest edits only).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 15' steps 1–3 verbatim. Gate check FIRST: `grep -n "Class" common/src/comp/skillset/skills.rs` — proceed only on a hit. Replace `Simple(None, …)` with `Simple(Some(Class(...)), …)` for `abilities:` entries ONLY; primaries/secondaries stay ungated (cantrip feel, spec §3).
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` → PASS.
  - In-game: ungated character no longer sees the ability in the Diary; after `/skill_point` grants it appears.
- **Size:** S

## MAG-P4.16 — Lint, format, changelog, balance pass, branch finish

- **Model:** fable — the live `asset_tweak` balance-tuning session against the spec §8 energy/cooldown matrix is balance-table work (fable per routing policy); the lint/format/changelog steps are mechanical but ride along.
- **Depends on:** all MAG tasks above (MAG-P4.15 may be gated out — if so, record the deferral in the summary).
- **Branch / commit:** `feature/magic-v1` — `docs: changelog entry for magic abilities v1` (+ any fix commits from lint)
- **Files:**
  - Create: none
  - Modify: `CHANGELOG.md` (+ whatever clippy/fmt fixes touch)
  - Delete: none
- **Assets:** none (asset_tweak adjusts values in place; balance changes commit as RON edits).
- **Downloads/tools:** `asset_tweak` feature for live tuning; `veloren-telemetry` skill recording per-spell casts; `superpowers:finishing-a-development-branch` + `veloren-review` before merging into `development`.
- **Steps:** Follow plan section '### Task 16' steps 1–5 verbatim. No `#[allow]` without a justifying comment. Deferred follow-ups to record: Wildshape #12, dedicated icons/voxel models/particle modes (TODO markers), hotbar radial sweep, cooldown persistence (Open Q #3), boss Charm immunity.
- **Acceptance:**
  - `cargo clippy --all-targets --locked --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" -- -D warnings` → clean.
  - `cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings` → clean.
  - `cargo fmt --all -- --check` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server -p veloren-voxygen` → PASS, including `ability_cooldown` (3), `innate_aux` (1), `spell_asset` (3) suites.
- **Size:** M
