# Equipment Restrictions — Task Board

**Source plan:** [../plans/2026-06-11-equipment-restrictions.md](../plans/2026-06-11-equipment-restrictions.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> Escalation rule: If acceptance fails twice, escalate one model tier and leave a note in the task file.

> Branch setup (before EQ-A1): create `feature/equipment-restrictions` off `development`. Phase A (EQ-A1…EQ-A6) has no dependencies and is implementable now. Phase B (EQ-B7, EQ-B8) is gated on `ClassKind` existing (classes plan Task 1 = CLS-1) — every Phase B task starts with a grep verification gate; if the gate fails, SKIP to EQ-9 and ship Phase A alone (Phase B lands later on a follow-up branch).

## Phase A — Level + race gates

## EQ-A1 — `ItemRequirements` type, `ItemDef`/`RawItemDef` field, RON wiring

- **Model:** sonnet — TDD task; full code is in the plan but the field must be threaded through five locations in one large file (`RawItemDef` destructure, two test constructors) with backward-compat stakes for ~thousands of item RONs.
- **Depends on:** none.
- **Branch / commit:** `feature/equipment-restrictions` — `feat: ItemRequirements (min_level, races) on ItemDef, RON-optional`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/inventory/item/mod.rs` (types before `ItemDef` line 786; field on `ItemDef` ~801 and `RawItemDef` ~996; `impl Asset for ItemDef` destructure 957–982; `requirements: None` in `new_test` ~894 and `create_test_itemdef_from_kind` ~914; tests in `mod tests` ~2151)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 1' steps 1–5 verbatim. TDD order: write `item_requirements_ron_roundtrip` first and confirm the compile failure ("no field `requirements` on type `RawItemDef`") before implementing. WARNING: RON files deserialize through the private `RawItemDef`, so the `#[serde(default)]` field must be added BOTH there and on `ItemDef`, plus both struct literals in `impl Asset`.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common item_requirements -- --nocapture` → FAIL to compile after Step 1, 1 test PASSES after Step 3.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_assets_items` → PASS (all existing RONs still deserialize — the asset-walk CI guard).
- **Size:** M

## EQ-A2 — Shared predicate `Item::meets_requirements`

- **Model:** sonnet — TDD plus compiler-driven implementation of a new `ItemDesc` trait method across five impls; workspace-wide check.
- **Depends on:** EQ-A1.
- **Branch / commit:** `feature/equipment-restrictions` — `feat: shared meets_requirements predicate on Item/ItemDesc`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/inventory/item/mod.rs` (methods in `impl Item` after `ability_spec` ~1440; new method on `trait ItemDesc` ~1773 and its five impls at ~1862/1893/1924/1957/2008; tests in `mod tests`)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–5 verbatim. After adding the trait method, run `cargo check -p veloren-common` and implement it in every impl the compiler reports (mirror each impl's `tags()`); the plan gives the exact five one-liners.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements -- --nocapture` → FAIL to compile after Step 1 ("no method named `meets_requirements`"), 1 test PASSES after Step 3.
  - `cargo check --workspace --all-targets` → clean.
- **Size:** M

## EQ-A3 — Restricted test item asset

- **Model:** haiku — RON asset, i18n manifest entry, .ftl block, and the test are all given verbatim; pure copy-in plus running the listed tests.
- **Depends on:** EQ-A2 (the test calls `item.requirements()` from EQ-A2).
- **Branch / commit:** `feature/equipment-restrictions` — `feat: gated test item asset (level 10, Draugr-only)`
- **Files:**
  - Create: `assets/common/items/testing/test_draugr_blade.ron`
  - Modify: `assets/common/item_i18n_manifest.ron` (after `common.items.testing.test_boots`), `assets/voxygen/i18n/en/item/items/internal.ftl` (after the `test_boots` block ~line 91), `common/src/comp/inventory/item/mod.rs` (test in `mod tests`)
  - Delete: none
- **Assets:** `assets/common/items/testing/test_draugr_blade.ron` — Claude creates: RON file inline (full content in the plan, structure copied from `assets/common/items/weapons/sword/caladbolg.ron`). Manifest + .ftl entries — Claude creates: inline text from the plan. No new audio/voxel assets, no downloads.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–5 verbatim. TDD order: run the test first to confirm the asset-not-found panic before creating the RON.
- **Acceptance:**
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_draugr_blade -- --nocapture` → FAIL (asset missing) after Step 1, then:
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_draugr_blade ensure_item_localization test_assets_items` → all PASS.
- **Size:** S

## EQ-A4 — Server-side enforcement in Use/Swap with localized rejection

- **Model:** sonnet — multi-site server wiring (imports, SystemData, two manip arms with re-indentation of an existing block) plus an in-game manual check; code is in the plan but surgical placement matters.
- **Depends on:** EQ-A2 (predicate), EQ-A3 (test item for the manual check).
- **Branch / commit:** `feature/equipment-restrictions` — `feat: server rejects gated equips in InventoryManip Use/Swap`
- **Files:**
  - Create: none
  - Modify: `server/src/events/inventory_manip.rs` (import line 38, helpers after `snuff_lantern` ~62, `InventoryManipData` after `stats` ~112, `Use` arm ~550–574, `Swap` arm before line 815), `assets/voxygen/i18n/en/hud/bag.ftl`
  - Delete: none
- **Assets:** `hud-bag-requirements_not_met` key in `assets/voxygen/i18n/en/hud/bag.ftl` — Claude creates: .ftl text inline from the plan.
- **Downloads/tools:** none (manual check uses the `veloren-run` skill).
- **Steps:** Follow plan section '### Task 4' steps 1–6 verbatim. WARNING: in the `Use` arm the existing equip body (lines 551–574) is wrapped and re-indented, not replaced — keep every existing line. The `Swap` gate's `continue` targets the `for InventoryManipEvent(...)` loop (same pattern as the `SplitSwap` arm at line 856).
- **Acceptance:**
  - `cargo check -p veloren-server` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` → PASS (no regressions).
  - Manual (veloren-run, non-Draugr level-1 character): `/give_item common.items.testing.test_draugr_blade`, attempt equip → slot unchanged, chat shows "You don't meet the requirements to equip this item."; unrestricted weapon equips normally.
- **Size:** M

## EQ-A5 — Bag UI gray-out for unusable items

- **Model:** sonnet — small diff but the plan explicitly calls for compiler-driven borrow-ordering resolution (storage guards vs `&mut self.slot_manager`) plus visual verification.
- **Depends on:** EQ-A2 (predicate), EQ-A3 (test item for visual check).
- **Branch / commit:** `feature/equipment-restrictions` — `feat: gray out items failing equip requirements in bag UI`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/hud/bag.rs` (`InventoryScroller::scrollbar_and_slots` ~243; context before the slot loop ~411; tint after the overflow-red block ~447–450)
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none (visual check via `veloren-run`).
- **Steps:** Follow plan section '### Task 5' steps 1–5 verbatim. If the storage read guards conflict with the loop's `&mut` uses, the plan's prescribed fallback is cloning into locals (`SkillSet` is `Clone`).
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean.
  - Visual (veloren-run, level-1 non-Draugr): `/give_item common.items.testing.test_draugr_blade` → blade slot renders gray-tinted; normal items keep their quality background.
- **Size:** S

## EQ-A6 — Tooltip requirements block (unmet lines in red)

- **Model:** sonnet — multi-file wiring across the HUD helper and the conrod tooltip widget, including layout-height math and anchor-chain edits; code is given but placement is intricate.
- **Depends on:** EQ-A2 (predicate/`requirements()` on `ItemDesc`), EQ-A3 (test item for visual check).
- **Branch / commit:** `feature/equipment-restrictions` — `feat: tooltip requirements block with unmet lines in red`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/hud/util.rs` (helper near `item_text` ~66), `voxygen/src/ui/widgets/item_tooltip.rs` (imports ~9, `widget_ids!` ~319, `update()` after the Description block ~1251–1269, price `down_from` chain ~1281–1288, `default_y_dimension` ~1325/1384), `assets/voxygen/i18n/en/hud/bag.ftl`
  - Delete: none
- **Assets:** `hud-bag-requirement_level` and `hud-bag-requirement_race` keys in `assets/voxygen/i18n/en/hud/bag.ftl` — Claude creates: .ftl text inline from the plan (species names reuse existing `common-species-*` keys in `assets/voxygen/i18n/en/common.ftl`).
- **Downloads/tools:** none (visual check via `veloren-run`).
- **Steps:** Follow plan section '### Task 6' steps 1–5 verbatim. Do not forget step 3.4 (prepend two branches to the price `down_from` selector) and 3.5 (add `req_h` to the tooltip height sum) — both are layout-only and will not fail compilation if skipped.
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean.
  - Visual (veloren-run): hovering the test blade as level-1 Human → "Requires Level 10" and "Requires Race: Draugr" in red; as a qualifying viewer the lines render grey; unrestricted items show no requirements block and no extra empty space.
- **Size:** M

## Phase B — Class gates (gated on CLS-1)

## EQ-B7 — `classes` field on `ItemRequirements` + predicate extension

- **Model:** sonnet — deliberate delete-and-migrate of the old predicate methods with compiler-driven updates across server/bag/util/tooltip call sites; the plan flags the tempting-but-wrong shortcut explicitly.
- **Depends on:** CLS-1 (from 03-classes-races-tasks.md — `ClassKind` + `CharacterClass` in `common/src/comp/class.rs`; verified by the Step 0 grep gate), EQ-A2, EQ-A4, EQ-A5, EQ-A6 (all call sites it migrates).
- **Branch / commit:** `feature/equipment-restrictions` — `feat: class whitelist gate on ItemRequirements`
- **Files:**
  - Create: none
  - Modify: `common/src/comp/inventory/item/mod.rs` (type, predicate, tests), `server/src/events/inventory_manip.rs`, `voxygen/src/hud/bag.rs`, `voxygen/src/hud/util.rs`, `voxygen/src/ui/widgets/item_tooltip.rs` (call-site migration, per the commit's `git add` list)
  - Delete: none
- **Assets:** none (also add `classes: None,` to the EQ-A1 RON test string per the plan).
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 7' steps 0–4 verbatim. STEP 0 IS A HARD GATE: `grep -rn "enum ClassKind" common/src/comp/` must hit `common/src/comp/class.rs`; if empty, STOP — do not start, go to EQ-9 (Phase A ships alone). WARNING: do NOT keep `meets_requirements(skill_set, body)` as a wrapper passing `class: None` (it would silently fail class-gated items for everyone) — DELETE the two old methods and let `cargo check --workspace --all-targets` list every caller, updating each to fetch `CharacterClass`.
- **Acceptance:**
  - Gate: `grep -rn "enum ClassKind" common/src/comp/` → exactly one hit in `common/src/comp/class.rs`.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements` → FAIL to compile ("no field `classes`") after Step 1, then `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements item_requirements test_draugr_blade` → PASS.
  - `cargo check --workspace --all-targets` → clean (all call sites migrated).
- **Size:** M

## EQ-B8 — Class line in tooltip + class i18n

- **Model:** sonnet — partially specified ("mirroring the race block"), requires a grep-driven decision on whether class i18n keys already shipped and updating two callers.
- **Depends on:** EQ-B7; CLS-1 (Step 0 gate). Cross-file note: if the classes plan already shipped `common-class-*`/`hud-class*` keys, reuse them (Step 1 grep decides).
- **Branch / commit:** `feature/equipment-restrictions` — `feat: class requirement line in item tooltips`
- **Files:**
  - Create: none
  - Modify: `voxygen/src/hud/util.rs`, `voxygen/src/ui/widgets/item_tooltip.rs`, `assets/voxygen/i18n/en/hud/bag.ftl`, and (only if the Step 1 grep is empty) `assets/voxygen/i18n/en/common.ftl`
  - Delete: none
- **Assets:** `hud-bag-requirement_class` key in `bag.ftl` — Claude creates: .ftl text inline. Class-name keys (`common-class-warrior` … `common-class-adventurer`) — Claude creates inline in `common.ftl` ONLY if `grep -rn "common-class\|hud-class" assets/voxygen/i18n/en/` finds nothing.
- **Downloads/tools:** none (visual check via `veloren-run`).
- **Steps:** Follow plan section '### Task 8' steps 0–4 verbatim. Step 0 gate identical to EQ-B7. The visual check temporarily adds `classes: Some([Cleric])` to `test_draugr_blade.ron` — revert that RON edit afterwards (or keep it and update the EQ-A3 test expectations in the same commit).
- **Acceptance:**
  - `cargo check -p veloren-voxygen` → clean.
  - Visual (veloren-run, Warrior holding the temporarily class-gated blade): tooltip shows "Requires Class: Cleric" in red AND the server rejects the equip.
- **Size:** M

## EQ-9 — Lint, format, changelog, and branch finish

- **Model:** haiku — prescribed check commands plus a one-line changelog; escalate only if clippy/test failures need real fixes.
- **Depends on:** EQ-A6 (if the Phase B gate failed) or EQ-B8 (if Phase B ran).
- **Branch / commit:** `feature/equipment-restrictions` — `docs: changelog entry for equipment restrictions`; then finish via `superpowers:finishing-a-development-branch` (run `veloren-review` before merging into `development`).
- **Files:**
  - Create: none
  - Modify: `CHANGELOG.md`
  - Delete: none
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '## Task 9' steps 1–6 verbatim. Design-spec open questions 1–3 (force-unequip on later illegality, loot-roll badges, consumable `Use` gating) stay out of scope for this branch.
- **Acceptance:**
  - `cargo clippy --all-targets --locked --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" -- -D warnings` → clean.
  - `cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings` → clean.
  - `cargo fmt --all -- --check` → clean.
  - `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server` → PASS.
- **Size:** S
