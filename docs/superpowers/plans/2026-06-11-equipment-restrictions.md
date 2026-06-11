# Equipment Restrictions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Items can declare equip gates (min character level, race whitelist; class whitelist in Phase B) in their RON `ItemDef`. The server rejects gated equips authoritatively with a localized message; the client grays out unusable items in the bag and lists requirements in the tooltip with unmet lines in red.

**Architecture:** A new `ItemRequirements` struct lives next to `ItemDef` in `common/src/comp/inventory/item/mod.rs` and rides into every item via a `#[serde(default)]` optional field — zero changes to existing RONs (note: RON files deserialize through the private `RawItemDef` at `mod.rs:988`, so the field is added there *and* on `ItemDef`). One shared predicate (`ItemRequirements::unmet` → `Item::meets_requirements`) feeds both the server's `InventoryManip::Use`/`Swap` enforcement (`server/src/events/inventory_manip.rs:535`/`:798`) and the client tooltip, so they agree byte-for-byte. Equip-only gating: pickup, trade, and `LoadoutBuilder` (NPC) paths are untouched.

**Tech Stack:** Rust nightly (2024 edition), specs ECS, conrod HUD. Design spec: `docs/superpowers/specs/2026-06-10-equipment-restrictions-design.md`. Uses `SkillSet::character_level()` (merged, `common/src/comp/skillset/mod.rs:390`).

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/equipment-restrictions` off `development` before Task 1.
- Invoke the `veloren-progression` skill for context and the `superpowers:test-driven-development` skill before writing code.
- **Phase A (Tasks 1–6) has no dependencies and is implementable now. Phase B (Tasks 7–8) is gated on `ClassKind` existing — every Phase B task starts with a verification gate. If the gate fails, skip to Task 9 and ship Phase A alone.**

---

## Phase A — Level + race gates (no dependencies)

### Task 1: `ItemRequirements` type, `ItemDef`/`RawItemDef` field, RON wiring

**Files:**
- Modify: `common/src/comp/inventory/item/mod.rs` — new types before `pub struct ItemDef` (line 786); field on `ItemDef` (after `ability_spec`, line 801) and on `RawItemDef` (after `ability_spec`, line 996); destructure in `impl Asset for ItemDef` (lines 957–982); `requirements: None` in the two `#[cfg(test)]` constructors (`new_test` line 894, `create_test_itemdef_from_kind` line 914); tests in `mod tests` (line 2151)

- [ ] **Step 1: Write the failing tests**

Inside `mod tests` at the end of `common/src/comp/inventory/item/mod.rs` (line 2151), add:

```rust
    #[test]
    fn item_requirements_ron_roundtrip() {
        // Every RON under assets/common/items deserializes through RawItemDef.
        let with_requirements = r#"
            ItemDef(
                legacy_name: "Test Blade",
                legacy_description: "",
                kind: Tool((
                    kind: Sword,
                    hands: Two,
                    stats: (
                        equip_time_secs: 0.25,
                        power: 1.0,
                        effect_power: 1.0,
                        speed: 1.0,
                        range: 1.0,
                        energy_efficiency: 1.0,
                        buff_strength: 1.0,
                    ),
                )),
                quality: Low,
                tags: [],
                ability_spec: None,
                requirements: Some((
                    min_level: Some(10),
                    races: Some([Draugr]),
                )),
            )
        "#;
        let raw: RawItemDef =
            ron::de::from_str(with_requirements).expect("requirements field must parse");
        let requirements = raw.requirements.expect("requirements present");
        assert_eq!(requirements.min_level, Some(10));
        assert_eq!(
            requirements.races,
            Some(vec![crate::comp::body::humanoid::Species::Draugr])
        );

        // Absent field must keep deserializing (backward compatibility with
        // the ~thousands of existing item RONs).
        let without_requirements = r#"
            ItemDef(
                legacy_name: "Test Blade",
                legacy_description: "",
                kind: Tool((
                    kind: Sword,
                    hands: Two,
                    stats: (
                        equip_time_secs: 0.25,
                        power: 1.0,
                        effect_power: 1.0,
                        speed: 1.0,
                        range: 1.0,
                        energy_efficiency: 1.0,
                        buff_strength: 1.0,
                    ),
                )),
                quality: Low,
                tags: [],
                ability_spec: None,
            )
        "#;
        let raw: RawItemDef =
            ron::de::from_str(without_requirements).expect("absent field must parse");
        assert_eq!(raw.requirements, None);
    }
```

(`ron` is a direct dependency of `veloren-common`, and `mod tests` sees the private `RawItemDef` via `use super::*`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common item_requirements -- --nocapture`
Expected: FAIL to compile with "no field `requirements` on type `RawItemDef`".

- [ ] **Step 3: Implement the type and wire it through**

In `common/src/comp/inventory/item/mod.rs`, extend the existing `crate::` use block (line 10) so the `comp::` entry reads `comp::{body::humanoid, inventory::InvSlot},`. Then, directly above `pub struct ItemDef` (line 786), add:

```rust
/// Optional gates restricting who may *equip* an item. Declared per item in
/// RON; an absent field (or any `None` sub-field) means unrestricted. Pickup,
/// carrying, trading, and NPC loadouts (`LoadoutBuilder`) are never gated.
/// See docs/superpowers/specs/2026-06-10-equipment-restrictions-design.md.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ItemRequirements {
    /// Minimum derived character level (see `SkillSet::character_level`).
    pub min_level: Option<u16>,
    /// Whitelist of humanoid species that may equip this item.
    pub races: Option<Vec<humanoid::Species>>,
}

/// A single requirement the equipping entity fails. Feeds both the server
/// rejection and the client tooltip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmetRequirement {
    Level { needed: u16 },
    Race,
}

impl ItemRequirements {
    /// Requirements unmet at the given character level/species. `species` is
    /// `None` for non-humanoid bodies, which therefore fail any race gate.
    pub fn unmet(&self, level: u16, species: Option<humanoid::Species>) -> Vec<UnmetRequirement> {
        let mut unmet = Vec::new();
        if let Some(needed) = self.min_level
            && level < needed
        {
            unmet.push(UnmetRequirement::Level { needed });
        }
        if let Some(races) = &self.races
            && !species.is_some_and(|s| races.contains(&s))
        {
            unmet.push(UnmetRequirement::Race);
        }
        unmet
    }
}
```

Then add the field in **three** places:

1. `ItemDef` (after `pub ability_spec: Option<AbilitySpec>,` line 801):
```rust
    /// Equip gates (min level / race whitelist; class in Phase B).
    /// None = unrestricted.
    #[serde(default)]
    pub requirements: Option<ItemRequirements>,
```
2. `RawItemDef` (after `ability_spec: Option<AbilitySpec>,` line 996), same two lines minus the doc comment but keeping `#[serde(default)]`.
3. `impl Asset for ItemDef` (lines 957–982): add `requirements,` to both the `RawItemDef { ... }` destructure and the `Ok(ItemDef { ... })` literal.

Finally add `requirements: None,` to the struct literals in `ItemDef::new_test` (line 894) and `ItemDef::create_test_itemdef_from_kind` (line 914).

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common item_requirements -- --nocapture`
Expected: 1 test PASSES.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_assets_items`
Expected: PASS — all existing RONs still deserialize (this is the asset-walk CI guard from the spec's testing strategy).

- [ ] **Step 5: Commit**

```bash
git add common/src/comp/inventory/item/mod.rs
git commit -m "feat: ItemRequirements (min_level, races) on ItemDef, RON-optional"
```

---

### Task 2: Shared predicate `Item::meets_requirements`

**Files:**
- Modify: `common/src/comp/inventory/item/mod.rs` — methods inside `impl Item` (place after `pub fn ability_spec`, which ends near line 1440); new method on `trait ItemDesc` (line 1773) and its five impls (`Item` :1862, `FrontendItem` :1893, `ItemDef` :1924, `PickupItem` :1957, blanket `&T` :2008); tests in `mod tests`

- [ ] **Step 1: Write the failing tests**

Inside `mod tests`, add:

```rust
    #[test]
    fn meets_requirements_matrix() {
        use crate::comp::{
            Body,
            skillset::{SkillGroupKind, SkillSet, total_exp_for_level},
        };
        use crate::comp::body::humanoid;

        fn body_of(species: humanoid::Species) -> Body {
            Body::Humanoid(humanoid::Body::random_with(&mut rand::rng(), &species))
        }

        fn skill_set_at_level(level: u16) -> SkillSet {
            let mut skill_set = SkillSet::default();
            skill_set.add_experience(SkillGroupKind::General, total_exp_for_level(level));
            assert_eq!(skill_set.character_level(), level);
            skill_set
        }

        fn gated_test_item(requirements: Option<ItemRequirements>) -> Item {
            let mut item_def = ItemDef::create_test_itemdef_from_kind(ItemKind::Armor(
                armor::Armor::test_armor(
                    armor::ArmorKind::Chest,
                    armor::Protection::Normal(0.0),
                    armor::Protection::Normal(0.0),
                ),
            ));
            item_def.requirements = requirements;
            Item::new_from_item_base(
                ItemBase::Simple(Arc::new(item_def)),
                Vec::new(),
                &AbilityMap::load().read(),
                &MaterialStatManifest::load().read(),
            )
        }

        let draugr = body_of(humanoid::Species::Draugr);
        let human = body_of(humanoid::Species::Human);

        // No requirements -> everyone passes.
        let open = gated_test_item(None);
        assert!(open.meets_requirements(&skill_set_at_level(1), &human));

        // Empty requirements block -> everyone passes.
        let empty = gated_test_item(Some(ItemRequirements::default()));
        assert!(empty.meets_requirements(&skill_set_at_level(1), &human));

        // Level gate alone, boundary inclusive (level == min_level passes).
        let lvl10 = gated_test_item(Some(ItemRequirements {
            min_level: Some(10),
            races: None,
        }));
        assert!(!lvl10.meets_requirements(&skill_set_at_level(9), &human));
        assert!(lvl10.meets_requirements(&skill_set_at_level(10), &human));
        assert_eq!(
            lvl10.unmet_requirements(&skill_set_at_level(9), &human),
            vec![UnmetRequirement::Level { needed: 10 }]
        );

        // Race gate alone.
        let draugr_only = gated_test_item(Some(ItemRequirements {
            min_level: None,
            races: Some(vec![humanoid::Species::Draugr]),
        }));
        assert!(draugr_only.meets_requirements(&skill_set_at_level(1), &draugr));
        assert!(!draugr_only.meets_requirements(&skill_set_at_level(1), &human));

        // Combined gates: both must hold.
        let both = gated_test_item(Some(ItemRequirements {
            min_level: Some(10),
            races: Some(vec![humanoid::Species::Draugr]),
        }));
        assert!(both.meets_requirements(&skill_set_at_level(10), &draugr));
        assert!(!both.meets_requirements(&skill_set_at_level(9), &draugr));
        assert!(!both.meets_requirements(&skill_set_at_level(10), &human));
        assert_eq!(
            both.unmet_requirements(&skill_set_at_level(9), &human).len(),
            2
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements -- --nocapture`
Expected: FAIL to compile with "no method named `meets_requirements` found for struct `Item`".

- [ ] **Step 3: Implement**

Extend the `comp::` entry of the use block once more to `comp::{Body, body::humanoid, inventory::InvSlot, skillset::SkillSet},`. Inside `impl Item` (after `ability_spec`, ~line 1440), add:

```rust
    /// Equip gates declared on this item's definition. Modular (crafted)
    /// weapons carry no gates.
    pub fn requirements(&self) -> Option<&ItemRequirements> {
        match &self.item_base {
            ItemBase::Simple(item_def) => item_def.requirements.as_ref(),
            ItemBase::Modular(_) => None,
        }
    }

    /// Requirements this entity fails for equipping this item. Shared by
    /// server enforcement and the client tooltip so they always agree.
    pub fn unmet_requirements(&self, skill_set: &SkillSet, body: &Body) -> Vec<UnmetRequirement> {
        self.requirements().map_or_else(Vec::new, |requirements| {
            let species = match body {
                Body::Humanoid(humanoid_body) => Some(humanoid_body.species),
                _ => None,
            };
            requirements.unmet(skill_set.character_level(), species)
        })
    }

    pub fn meets_requirements(&self, skill_set: &SkillSet, body: &Body) -> bool {
        self.unmet_requirements(skill_set, body).is_empty()
    }
```

Add to `trait ItemDesc` (line 1773):

```rust
    fn requirements(&self) -> Option<&ItemRequirements>;
```

Run `cargo check -p veloren-common` and implement it in every impl the compiler reports (the five at lines 1862/1893/1924/1957/2008), mirroring how each one implements `tags()`:

```rust
    // impl ItemDesc for Item
    fn requirements(&self) -> Option<&ItemRequirements> { Item::requirements(self) }
    // impl ItemDesc for FrontendItem
    fn requirements(&self) -> Option<&ItemRequirements> { self.0.requirements() }
    // impl ItemDesc for ItemDef
    fn requirements(&self) -> Option<&ItemRequirements> { self.requirements.as_ref() }
    // impl ItemDesc for PickupItem
    fn requirements(&self) -> Option<&ItemRequirements> { self.item().requirements() }
    // impl<T: ItemDesc + ?Sized> ItemDesc for &T
    fn requirements(&self) -> Option<&ItemRequirements> { (*self).requirements() }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements -- --nocapture`
Expected: 1 test PASSES.
Run: `cargo check --workspace --all-targets`
Expected: clean (the new trait method is implemented everywhere).

- [ ] **Step 5: Commit**

```bash
git add common/src/comp/inventory/item/mod.rs
git commit -m "feat: shared meets_requirements predicate on Item/ItemDesc"
```

---

### Task 3: Restricted test item asset

**Files:**
- Create: `assets/common/items/testing/test_draugr_blade.ron`
- Modify: `assets/common/item_i18n_manifest.ron` (testing entries are at the top, after `common.items.testing.test_boots`), `assets/voxygen/i18n/en/item/items/internal.ftl` (after the `test_boots` entry, line ~91)
- Modify: `common/src/comp/inventory/item/mod.rs` (test in `mod tests`)

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn test_draugr_blade_requirements_load() {
        let item = Item::new_from_asset_expect("common.items.testing.test_draugr_blade");
        let requirements = item.requirements().expect("test item declares requirements");
        assert_eq!(requirements.min_level, Some(10));
        assert_eq!(
            requirements.races,
            Some(vec![crate::comp::body::humanoid::Species::Draugr])
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_draugr_blade -- --nocapture`
Expected: FAIL (panic loading `common.items.testing.test_draugr_blade` — asset does not exist).

- [ ] **Step 3: Create the asset (structure copied from `assets/common/items/weapons/sword/caladbolg.ron`)**

`assets/common/items/testing/test_draugr_blade.ron`:

```ron
ItemDef(
    legacy_name: "Draugr Test Blade",
    legacy_description: "Used for equipment-restriction tests, do not delete.",
    kind: Tool((
        kind: Sword,
        hands: Two,
        stats: (
            equip_time_secs: 0.25,
            power: 1.0,
            effect_power: 1.0,
            speed: 1.0,
            range: 1.0,
            energy_efficiency: 1.0,
            buff_strength: 1.0,
        ),
    )),
    quality: Low,
    tags: [],
    ability_spec: None,
    requirements: Some((
        min_level: Some(10),
        races: Some([Draugr]),
    )),
)
```

In `assets/common/item_i18n_manifest.ron`, after the `test_boots` entry:

```ron
        Simple(
            "common.items.testing.test_draugr_blade",
        ): "common-items-testing-test_draugr_blade",
```

In `assets/voxygen/i18n/en/item/items/internal.ftl`, after the `test_boots` block:

```ftl
common-items-testing-test_draugr_blade = Draugr Test Blade
    .desc = Used for equipment-restriction tests, do not delete.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_draugr_blade ensure_item_localization test_assets_items`
Expected: all PASS (`ensure_item_localization` proves the i18n manifest entry is correct).

- [ ] **Step 5: Commit**

```bash
git add assets/common/items/testing/test_draugr_blade.ron assets/common/item_i18n_manifest.ron assets/voxygen/i18n/en/item/items/internal.ftl common/src/comp/inventory/item/mod.rs
git commit -m "feat: gated test item asset (level 10, Draugr-only)"
```

---

### Task 4: Server-side enforcement in Use/Swap with localized rejection

**Files:**
- Modify: `server/src/events/inventory_manip.rs` — import (line 38), helpers (after `snuff_lantern`, line 62), SystemData (`InventoryManipData`, add after `stats` line 112), `InventoryManip::Use` arm (`if is_equippable` block, lines 550–574), `InventoryManip::Swap` arm (before `if let Some(pos)` at line 815)
- Modify: `assets/voxygen/i18n/en/hud/bag.ftl` (rejection message key)

The error mechanism (verified pattern, used in `server/src/events/invite.rs:83`): `client.send_fallible(ServerGeneral::server_msg(ChatType::..., Content::...))` where `clients: ReadStorage<'a, Client>` is already in `InventoryManipData` (line 113) and `ServerGeneral` is already imported (line 39).

- [ ] **Step 1: Storages, imports, helpers**

Change line 38 to:

```rust
use common::comp::{
    Alignment, ChatType, CollectFailedReason, Content, Group, InventoryUpdateEvent,
    pet::is_tameable,
};
```

Add to `InventoryManipData` after `stats: ReadStorage<'a, comp::Stats>,` (line 112):

```rust
    skill_sets: ReadStorage<'a, comp::SkillSet>,
```

After `snuff_lantern` (line 62), add:

```rust
/// Equip gates only apply to entities that have both a `SkillSet` and a
/// `Body` (players). NPC loadouts are built by `LoadoutBuilder` and never
/// pass through `InventoryManip`, so they bypass this by design.
fn entity_meets_item_requirements(
    item: &comp::Item,
    skill_set: Option<&comp::SkillSet>,
    body: Option<&comp::Body>,
) -> bool {
    match (skill_set, body) {
        (Some(skill_set), Some(body)) => item.meets_requirements(skill_set, body),
        _ => true,
    }
}

fn notify_requirements_not_met(clients: &ReadStorage<'_, Client>, entity: EcsEntity) {
    if let Some(client) = clients.get(entity) {
        client.send_fallible(ServerGeneral::server_msg(
            ChatType::CommandError,
            Content::localized("hud-bag-requirements_not_met"),
        ));
    }
}
```

- [ ] **Step 2: Gate the `Use` arm**

In the `Slot::Inventory(slot)` branch of `comp::InventoryManip::Use(slot)` (line 550), wrap the existing equip body (lines 551–574, from `if let Some(lantern_info)` through `Some(InventoryUpdateEvent::Used)`) so the arm reads:

```rust
                            if is_equippable {
                                let requirements_ok = inventory.get(slot).is_none_or(|item| {
                                    entity_meets_item_requirements(
                                        item,
                                        data.skill_sets.get(entity),
                                        data.bodies.get(entity),
                                    )
                                });
                                if !requirements_ok {
                                    notify_requirements_not_met(&data.clients, entity);
                                    None
                                } else {
                                    // ... existing lines 551-573 unchanged, re-indented ...
                                    Some(InventoryUpdateEvent::Used)
                                }
                            } else if let Some(item) =
```

- [ ] **Step 3: Gate the `Swap` arm**

In `comp::InventoryManip::Swap(a, b)` (line 798), directly before `if let Some(pos) = data.positions.get(entity) {` (line 815), insert (uses `Inventory::get_slot`, verified at `common/src/comp/inventory/mod.rs:507`):

```rust
                    // Equip gate: reject when either side of the swap would
                    // mount a gated item into a loadout slot.
                    let violates_requirements = [(a, b), (b, a)].into_iter().any(|(src, dst)| {
                        matches!(dst, Slot::Equip(_))
                            && inventory.get_slot(src).is_some_and(|item| {
                                !entity_meets_item_requirements(
                                    item,
                                    data.skill_sets.get(entity),
                                    data.bodies.get(entity),
                                )
                            })
                    });
                    if violates_requirements {
                        notify_requirements_not_met(&data.clients, entity);
                        continue;
                    }
```

(`continue` targets the `for InventoryManipEvent(entity, manip) in events` loop — same pattern as the `SplitSwap` arm at line 856.)

- [ ] **Step 4: Add the i18n key**

Append to `assets/voxygen/i18n/en/hud/bag.ftl`:

```ftl
hud-bag-requirements_not_met = You don't meet the requirements to equip this item.
```

- [ ] **Step 5: Verify build and behavior**

Run: `cargo check -p veloren-server`
Expected: clean.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server`
Expected: PASS (no regressions).

Manual check with the `veloren-run` skill (server + client, non-Draugr level-1 character):
- `/give_item common.items.testing.test_draugr_blade`
- Try to equip it (right-click / drag onto mainhand): the loadout slot stays unchanged and the chat shows "You don't meet the requirements to equip this item."
- Equip any unrestricted weapon: works as before.

- [ ] **Step 6: Commit**

```bash
git add server/src/events/inventory_manip.rs assets/voxygen/i18n/en/hud/bag.ftl
git commit -m "feat: server rejects gated equips in InventoryManip Use/Swap"
```

---

### Task 5: Bag UI gray-out for unusable items

**Files:**
- Modify: `voxygen/src/hud/bag.rs` — `InventoryScroller::scrollbar_and_slots` (line 243; the slot loop starts at line 411, the existing tint blocks are at lines 439–450). `SkillSet` and `Body` are already imported at line 30; `self.client` is available (used at line 464).

- [ ] **Step 1: Compute the local player's context once, before the slot loop (line 411)**

```rust
        // Local player's context for equipment-requirement gray-out. Gates are
        // always evaluated against the viewer, even when inspecting someone
        // else's inventory.
        let ecs = self.client.state().ecs();
        let player_entity = self.client.entity();
        let skill_sets = ecs.read_storage::<SkillSet>();
        let bodies = ecs.read_storage::<Body>();
        let requirement_ctx = skill_sets.get(player_entity).zip(bodies.get(player_entity));
```

- [ ] **Step 2: Tint failing slots**

After the overflow-red block (lines 447–450), add — mirroring the `item.as_ref().is_some_and(...)` pattern from the salvage block at line 443:

```rust
            // Gray out items the player can't equip due to requirements
            if let Some((skill_set, body)) = requirement_ctx
                && item
                    .as_ref()
                    .is_some_and(|item| !item.meets_requirements(skill_set, body))
            {
                slot_widget = slot_widget.with_background_color(Color::Rgba(0.45, 0.45, 0.45, 1.0));
            }
```

- [ ] **Step 3: Compiler-driven cleanup**

Run: `cargo check -p veloren-voxygen`
Fix any borrow ordering the compiler reports (the storage reads must not outlive the loop's `&mut` uses of `self.slot_manager` — if they conflict, drop the guards into locals `let viewer_skill_set = ...cloned()` instead; `SkillSet` is `Clone`).
Expected: clean.

- [ ] **Step 4: Visual verification**

Use the `veloren-run` skill: with a level-1 non-Draugr character, `/give_item common.items.testing.test_draugr_blade`. The blade's bag slot renders with the gray tint; normal items keep their quality background.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/hud/bag.rs
git commit -m "feat: gray out items failing equip requirements in bag UI"
```

---

### Task 6: Tooltip requirements block (unmet lines in red)

**Files:**
- Modify: `voxygen/src/hud/util.rs` (new helper near `item_text`, line 66)
- Modify: `voxygen/src/ui/widgets/item_tooltip.rs` — imports (line 9), `widget_ids!` (line 319, after `desc,`), `update()` (after the Description block, lines 1251–1269), price `down_from` chain (lines 1281–1288), `default_y_dimension` (line 1325, height sum at line 1384)
- Modify: `assets/voxygen/i18n/en/hud/bag.ftl`

- [ ] **Step 1: i18n keys**

Append to `assets/voxygen/i18n/en/hud/bag.ftl` (species names reuse the existing `common-species-*` keys in `assets/voxygen/i18n/en/common.ftl:68`):

```ftl
hud-bag-requirement_level = Requires Level { $level }
hud-bag-requirement_race = Requires Race: { $races }
```

- [ ] **Step 2: Shared text helper in `voxygen/src/hud/util.rs`**

Add (extend the file's `common::comp` imports with `body::humanoid::Species` and `item::{ItemRequirements, UnmetRequirement}` as the compiler demands):

```rust
/// Requirement lines for an item tooltip, split into (met, unmet) for the
/// given viewer `(character_level, species)`. `viewer: None` (no SkillSet,
/// e.g. spectators) renders everything as met.
pub fn requirements_text(
    item: &dyn ItemDesc,
    viewer: Option<(u16, Option<Species>)>,
    i18n: &Localization,
) -> (Vec<String>, Vec<String>) {
    let (mut met, mut unmet) = (Vec::new(), Vec::new());
    let Some(requirements) = item.requirements() else {
        return (met, unmet);
    };
    // Reuse the shared predicate so the tooltip can never disagree with the
    // server's enforcement.
    let unmet_kinds = viewer
        .map(|(level, species)| requirements.unmet(level, species))
        .unwrap_or_default();

    if let Some(needed) = requirements.min_level {
        let line = i18n
            .get_msg_ctx("hud-bag-requirement_level", &i18n::fluent_args! {
                "level" => u32::from(needed),
            })
            .into_owned();
        if unmet_kinds.iter().any(|u| matches!(u, UnmetRequirement::Level { .. })) {
            unmet.push(line);
        } else {
            met.push(line);
        }
    }
    if let Some(races) = &requirements.races {
        let names = races
            .iter()
            .map(|species| i18n.get_msg(species_i18n_key(*species)).into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let line = i18n
            .get_msg_ctx("hud-bag-requirement_race", &i18n::fluent_args! {
                "races" => names,
            })
            .into_owned();
        if unmet_kinds.contains(&UnmetRequirement::Race) {
            unmet.push(line);
        } else {
            met.push(line);
        }
    }
    (met, unmet)
}

fn species_i18n_key(species: Species) -> &'static str {
    match species {
        Species::Danari => "common-species-danari",
        Species::Dwarf => "common-species-dwarf",
        Species::Elf => "common-species-elf",
        Species::Human => "common-species-human",
        Species::Orc => "common-species-orc",
        Species::Draugr => "common-species-draugr",
    }
}
```

- [ ] **Step 3: Render in `item_tooltip.rs`**

1. Imports: change the `common::comp` block (line 10) to also bring `Body, SkillSet` (alongside `Energy, Inventory`).
2. `widget_ids!` (line 319): add `requirements_met,` and `requirements_unmet,` after `desc,`.
3. In `update()`, directly after the Description block (line 1269), add:

```rust
        // Equipment requirements (level/race gates)
        let viewer = {
            let ecs = self.client.state().ecs();
            let entity = self.info.viewpoint_entity;
            let level = ecs
                .read_storage::<SkillSet>()
                .get(entity)
                .map(|skill_set| skill_set.character_level());
            let species = ecs
                .read_storage::<Body>()
                .get(entity)
                .and_then(|body| match_some!(*body, Body::Humanoid(b) => b.species));
            level.map(|level| (level, species))
        };
        let (req_met, req_unmet) = util::requirements_text(item, viewer, i18n);
        let req_anchor = if !desc.is_empty() {
            state.ids.desc
        } else if stats_count > 0 {
            state.ids.stats[state.ids.stats.len() - 1]
        } else {
            state.ids.item_frame
        };
        let mut last_req_id = None;
        if !req_met.is_empty() {
            widget::Text::new(&req_met.join("\n"))
                .x_align_to(state.ids.item_frame, conrod_core::position::Align::Start)
                .graphics_for(id)
                .parent(id)
                .with_style(self.style.desc)
                .color(conrod_core::color::GREY)
                .down_from(req_anchor, V_PAD)
                .w(text_w)
                .set(state.ids.requirements_met, ui);
            last_req_id = Some(state.ids.requirements_met);
        }
        if !req_unmet.is_empty() {
            widget::Text::new(&req_unmet.join("\n"))
                .x_align_to(state.ids.item_frame, conrod_core::position::Align::Start)
                .graphics_for(id)
                .parent(id)
                .with_style(self.style.desc)
                .color(Color::Rgba(1.0, 0.0, 0.0, 1.0))
                .down_from(last_req_id.unwrap_or(req_anchor), V_PAD_STATS)
                .w(text_w)
                .set(state.ids.requirements_unmet, ui);
            last_req_id = Some(state.ids.requirements_unmet);
        }
```

4. Price chain (lines 1281–1288): prepend two branches to the `down_from` selector so it reads `if !req_unmet.is_empty() { state.ids.requirements_unmet } else if !req_met.is_empty() { state.ids.requirements_met } else if !desc.is_empty() { ... }` (rest unchanged).
5. `default_y_dimension` (line 1325): before the final sum (line 1384) add

```rust
        // Equipment requirements
        let req_line_count = self
            .item
            .requirements()
            .map_or(0, |r| r.min_level.is_some() as usize + r.races.is_some() as usize);
        let req_h = if req_line_count > 0 {
            widget::Text::new("placeholder")
                .with_style(self.style.desc)
                .get_h(ui)
                .unwrap_or(0.0)
                * req_line_count as f64
                + V_PAD
        } else {
            0.0
        };
```

and change the sum to `let height = frame_h + stat_h + desc_h + req_h + price_h + V_PAD + 5.0;`.

- [ ] **Step 4: Verify**

Run: `cargo check -p veloren-voxygen`
Expected: clean.
Visual check via `veloren-run`: hovering `common.items.testing.test_draugr_blade` as a level-1 Human shows "Requires Level 10" and "Requires Race: Draugr" in red; as a level-10+ Draugr (level up via `/skill_preset` or mob grinding, or test with `min_level` only) the lines render grey; unrestricted items show no requirements block and no extra empty space.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/hud/util.rs voxygen/src/ui/widgets/item_tooltip.rs assets/voxygen/i18n/en/hud/bag.ftl
git commit -m "feat: tooltip requirements block with unmet lines in red"
```

---

## Phase B — Class gates

> **Depends on:** `2026-06-11-classes-races.md` Task 1 (`ClassKind` + `CharacterClass` component in `common/src/comp/class.rs`, per `docs/superpowers/specs/2026-06-10-classes-races-design.md` §1). That plan is not yet written/merged — the grep gate below is the source of truth. **If the gate fails, STOP Phase B and jump to Task 9.**

### Task 7: `classes` field on `ItemRequirements` + predicate extension

**Files:**
- Modify: `common/src/comp/inventory/item/mod.rs` (type, predicate, tests)

- [ ] **Step 0: VERIFICATION GATE**

Run: `grep -rn "enum ClassKind" common/src/comp/`
Expected: exactly one hit in `common/src/comp/class.rs` (e.g. `common/src/comp/class.rs:N:pub enum ClassKind {`).
**If there is no output, STOP: do not start this task. Phase A is complete and shippable — go to Task 9.**

- [ ] **Step 1: Failing tests** — extend `meets_requirements_matrix` (Task 2):

```rust
        use crate::comp::class::ClassKind;

        // Class gate alone. Adventurer (legacy) is never listed on items, so
        // class-gated items exclude legacy characters until /select_class.
        let cleric_only = gated_test_item(Some(ItemRequirements {
            classes: Some(vec![ClassKind::Cleric]),
            min_level: None,
            races: None,
        }));
        assert!(!cleric_only.meets_requirements_with_class(
            Some(ClassKind::Adventurer),
            &skill_set_at_level(1),
            &human,
        ));
        assert!(cleric_only.meets_requirements_with_class(
            Some(ClassKind::Cleric),
            &skill_set_at_level(1),
            &human,
        ));
```

and add `classes: None,` to every `ItemRequirements` literal in the existing tests and in the Task 3 RON test (also add `classes: None,` to the RON string).

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements`
Expected: FAIL to compile ("no field `classes`").

- [ ] **Step 2: Implement**

On `ItemRequirements`, add as the **first** field (matching the spec's order):

```rust
    /// Whitelist of classes that may equip this item. None = any class.
    pub classes: Option<Vec<crate::comp::class::ClassKind>>,
```

Add `UnmetRequirement::Class`, and in `ItemRequirements::unmet` change the signature to `pub fn unmet(&self, class: Option<ClassKind>, level: u16, species: Option<humanoid::Species>)` with, before the level check:

```rust
        if let Some(classes) = &self.classes
            && !class.is_some_and(|c| classes.contains(&c))
        {
            unmet.push(UnmetRequirement::Class);
        }
```

On `Item`, rename the entity-context methods to thread the class through: `unmet_requirements_with_class(&self, class: Option<ClassKind>, skill_set, body)` and `meets_requirements_with_class(...)`, and keep `meets_requirements(skill_set, body)` as a forwarding wrapper passing `class: None` is **wrong** (it would fail class-gated items for everyone in old call sites silently) — instead delete the two old methods and let `cargo check --workspace --all-targets` list every caller (Task 4 server helper, Task 5 bag.rs, Task 6 util.rs); update each to fetch `CharacterClass` (server: add `character_classes: ReadStorage<'a, comp::CharacterClass>` to `InventoryManipData` and pass `data.character_classes.get(entity).map(|c| c.0)`; voxygen: read `CharacterClass` storage next to the existing `SkillSet`/`Body` reads and pass `.map(|c| c.0)`).

- [ ] **Step 3: Verify**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common meets_requirements item_requirements test_draugr_blade`
Expected: PASS.
Run: `cargo check --workspace --all-targets`
Expected: clean (all call sites migrated).

- [ ] **Step 4: Commit**

```bash
git add common/src/comp/inventory/item/mod.rs server/src/events/inventory_manip.rs voxygen/src/hud/bag.rs voxygen/src/hud/util.rs voxygen/src/ui/widgets/item_tooltip.rs
git commit -m "feat: class whitelist gate on ItemRequirements"
```

---

### Task 8: Class line in tooltip + class i18n

- [ ] **Step 0: VERIFICATION GATE** — same as Task 7 Step 0; STOP if `grep -rn "enum ClassKind" common/src/comp/` is empty.

- [ ] **Step 1: Find the class-name i18n keys the classes plan shipped**

Run: `grep -rn "common-class\|hud-class" assets/voxygen/i18n/en/`
If keys exist, use them; if the grep is empty, add to `assets/voxygen/i18n/en/common.ftl` (next to the `common-species-*` block at line 68): `common-class-warrior = Warrior`, `common-class-mage = Mage`, `common-class-cleric = Cleric`, `common-class-rogue = Rogue`, `common-class-adventurer = Adventurer`.

- [ ] **Step 2: Extend `requirements_text` in `voxygen/src/hud/util.rs`**

Append to `bag.ftl`: `hud-bag-requirement_class = Requires Class: { $classes }`. In `requirements_text`, change `viewer` to `Option<(Option<ClassKind>, u16, Option<Species>)>`, pass the class into `requirements.unmet(...)`, and add a class block (before the level block, mirroring the race block exactly: join localized class names with `", "`, classify via `unmet_kinds.contains(&UnmetRequirement::Class)`), with a `class_i18n_key(class: ClassKind) -> &'static str` match mirroring `species_i18n_key`. Update the two callers (`item_tooltip.rs` viewer construction reads the `CharacterClass` storage; Task 7 already wired it).

- [ ] **Step 3: Verify**

Run: `cargo check -p veloren-voxygen` — expected clean.
Visual check via `veloren-run`: create a class-gated test variant (temporarily add `classes: Some([Cleric])` to `test_draugr_blade.ron` requirements, give it to a Warrior): tooltip shows "Requires Class: Cleric" in red and the server rejects the equip. Revert the temporary RON edit afterwards (or keep it and update the Task 3 test expectations in the same commit).

- [ ] **Step 4: Commit**

```bash
git add voxygen/src/hud/util.rs voxygen/src/ui/widgets/item_tooltip.rs assets/voxygen/i18n/en/
git commit -m "feat: class requirement line in item tooltips"
```

---

## Task 9: Lint, format, changelog, and branch finish

(Run after Task 8 — or directly after Task 6 if the Phase B gate failed; Phase B then lands later on a follow-up branch.)

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. Fix any warnings (no bare `#[allow]` without a justifying comment).

- [ ] **Step 2: Voxygen publish-profile clippy**

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
```
Expected: clean.

- [ ] **Step 3: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check.

- [ ] **Step 4: Full test suite**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server`
Expected: PASS.

- [ ] **Step 5: Changelog and commit**

Add under the unreleased section of `CHANGELOG.md`:
```markdown
- Items can now require a minimum character level or race (and class, once classes land) to equip; gated items are grayed out in the bag with requirements shown in the tooltip.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for equipment restrictions"
```

- [ ] **Step 6: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). Open questions 1–3 in the design spec (force-unequip on later illegality, loot-roll badges, consumable `Use` gating) remain explicitly out of scope for this branch.
