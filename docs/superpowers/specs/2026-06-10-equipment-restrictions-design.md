# Equipment Restrictions: Class/Level/Race-Gated Items

**Date:** 2026-06-10
**Depends on:** `2026-06-10-classes-races-design.md` (`ClassKind`, `CharacterClass` component), `2026-06-10-character-levels-design.md` (character Level from total earned XP)

## Context

All equippable items in Veloren are usable by anyone whose loadout has a matching slot — the only
equip check is structural (does this `EquipSlot` hold this `ItemKind`?). With classes (Spec A) and
character levels (companion spec) landing, this fork wants D&D-style item gates: a holy mace only
Clerics can wield, plate that requires level 20, ancestral blades only Draugr can attune. Gates
must be declared per item in RON, enforced by the server, and clearly surfaced in the client UI.

## Goals

| Goal | Detail |
|---|---|
| Declarative requirements | Items optionally declare class whitelist, min character level, race whitelist in their RON `ItemDef` |
| Server-side enforcement | Equip/swap attempts that fail requirements are rejected authoritatively |
| Client UX | Unusable items render grayed/red in bag and loadout-target slots; tooltip lists requirements, unmet ones in red |
| Backward compatible | Absent field = unrestricted; zero changes to the ~thousands of existing item RONs |
| Loot stays class-agnostic | Anyone can loot, carry, trade, and sell gated items — only *equipping* is gated |

## Non-Goals

- Gating item *pickup*, crafting, or trading.
- Attribute-style requirements (STR/DEX scores) — no attribute system exists in this fork.
- NPC enforcement: NPC loadouts are built by `LoadoutBuilder` presets and bypass requirements.

## Current state (verified)

### Item definitions

`common/src/comp/inventory/item/mod.rs:786` — `ItemDef`:

```rust
pub struct ItemDef {
    #[serde(default)]
    item_definition_id: String,
    legacy_name: String,
    pub kind: ItemKind,
    pub quality: Quality,
    pub tags: Vec<ItemTag>,
    #[serde(default)]
    pub slots: u16,
    pub ability_spec: Option<AbilitySpec>,
}
```

Optional/defaulted serde fields are an established pattern (`slots`, `ability_spec`), so adding an
optional field is non-breaking for every existing RON under `assets/common/items/`.

### Equip validation chain

No restriction concept exists. Equip legality is decided in shared `common` code, invoked from a
server event handler:

| Layer | Location | What it checks |
|---|---|---|
| Server entry | `server/src/events/inventory_manip.rs` — `InventoryManip::Use(slot)` arm (`:535`) and `InventoryManip::Swap(a, b)` arm (`:798`) of the `InventoryManipEvent` handler | Routes to shared inventory code |
| Equip | `common/src/comp/inventory/mod.rs:875` `Inventory::equip` → `loadout.get_slot_to_equip_into` | Finds a compatible slot |
| Swap | `common/src/comp/inventory/mod.rs:1018` `Inventory::swap` → `:1070` `swap_inventory_loadout` → `:1146` `can_swap` | Slot compatibility |
| Slot rule | `common/src/comp/inventory/loadout.rs:384` `slot_can_hold` → `common/src/comp/inventory/slot.rs:111` `EquipSlot::can_hold(&ItemKind)` | Structural only — **sees only `ItemKind`, no entity context** |

Key consequence: `slot_can_hold`/`can_hold` have no access to the equipping entity's class, level,
or body, and threading entity context through them would ripple through dozens of call sites
(loadout building, NPC spawning, UI previews). Enforcement therefore belongs one level up, where
the entity is in hand.

### Entity context available at the enforcement point

In `server/src/events/inventory_manip.rs`, the handler's `InventoryManipData` SystemData already
joins per-entity storages; adding `ReadStorage<CharacterClass>` (Spec A), the Level source
(companion spec; derived from `SkillSet` total earned exp), and `ReadStorage<Body>` (species) is
routine.

## Design

### 1. Data model

New type in `common/src/comp/inventory/item/mod.rs` (next to `ItemDef`):

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ItemRequirements {
    pub classes: Option<Vec<ClassKind>>,      // None = any class
    pub min_level: Option<u16>,               // None = no level gate
    pub races: Option<Vec<humanoid::Species>>,// None = any race
}
```

`ItemDef` gains:

```rust
#[serde(default)]
pub requirements: Option<ItemRequirements>,
```

RON example (`assets/common/items/weapons/sceptre/divine_gaze.ron` style):

```ron
ItemDef(
    kind: Tool(( kind: Sceptre, ... )),
    quality: High,
    tags: [],
    requirements: Some((
        classes: Some([Cleric]),
        min_level: Some(10),
        races: None,
    )),
)
```

`Adventurer` (legacy class, Spec A) passes only items with `classes: None` — class-gated items
list concrete classes and thus exclude legacy characters until they `/select_class`.

### 2. Shared predicate

One function, in `common` so server and client agree byte-for-byte:

```rust
// common/src/comp/inventory/item/mod.rs
impl Item {
    pub fn meets_requirements(
        &self,
        class: Option<ClassKind>,
        level: u16,
        species: Option<humanoid::Species>,
    ) -> Result<(), UnmetRequirement> { ... }
}
```

`UnmetRequirement { Class, Level { needed: u16 }, Race }` feeds both the server rejection and the
client tooltip. Absent `requirements` (or any `None` sub-field) short-circuits to `Ok(())`.

### 3. Server-side enforcement

In `server/src/events/inventory_manip.rs`:

- `InventoryManip::Use(Slot::Inventory(slot))` arm (`:535`): after the existing `is_equippable`
  check and before `inventory.equip(..)`, call `meets_requirements`; on failure, skip the equip and
  send the localized failure (chat/error message via the existing event/notification plumbing in
  that handler).
- `InventoryManip::Swap(a, b)` arm (`:798`): when either side of the swap moves an item *into* an
  `EquipSlot`, validate that item the same way before calling `inventory.swap(..)`.

These two arms are the only paths by which a player-initiated action mounts an item into a
loadout slot, so they are the complete enforcement surface. `LoadoutBuilder` (NPC/spawn/admin
paths) intentionally bypasses the check.

### 4. Client UI treatment

| Surface | Change | File |
|---|---|---|
| Bag grid | Items failing `meets_requirements` for the local player render with the existing gray/red desaturation treatment used for inactive slots | `voxygen/src/hud/bag.rs`, `voxygen/src/hud/slots.rs` |
| Tooltip | New "Requirements:" block listing class/level/race; unmet lines in red | `voxygen/src/hud/util.rs` (alongside `item_text`/`describe`), i18n in `assets/voxygen/i18n/en/hud/bag.ftl` |
| Slot rejection | Drag-drop onto an equip slot is refused client-side (no manip event sent) with the standard error flash, mirroring the server rule for responsiveness | `voxygen/src/hud/slots.rs` |

The client check is pure UX prediction — see §6.

### 5. Backward compatibility and loot tables

- `#[serde(default)] requirements: Option<..>` means every existing item RON deserializes
  unchanged as unrestricted; no asset migration, no DB migration (requirements live in the item
  *definition*, never in per-instance `properties`).
- Loot tables (`assets/common/loot_tables/`) need no edits: drops, pickup
  (`InventoryManip::Pickup`, `inventory_manip.rs:174`), stacking, trading, and merchant logic
  never consult requirements. A Warrior loots a Mage staff and sells or trades it.

### 6. Anti-cheat: server authority

The server re-validates every equip/swap inside the `InventoryManipEvent` handler regardless of
what the client predicted; a modified client that sends raw `InventoryManip::Use`/`Swap` messages
for a gated item gets a no-op plus an error message. Because class (`CharacterClass`), level
(derived from server-persisted `SkillSet`), and species (`Body`) are all server-owned components,
there is no client-supplied input in the predicate. Item requirements ship in assets, which the
server loads from its own `VELOREN_ASSETS` — client asset tampering only desyncs the client's own
preview, never the authoritative outcome.

## Phases

### Phase 1 — Data model + server enforcement (M, ~3 dev-days)

| Task | Files | Size |
|---|---|---|
| `ItemRequirements` + `ItemDef.requirements` + `meets_requirements`/`UnmetRequirement` | `common/src/comp/inventory/item/mod.rs` | S |
| Enforcement in Use/Swap arms + storages in `InventoryManipData` | `server/src/events/inventory_manip.rs` | M |
| Failure message to client (localized) | `server/src/events/inventory_manip.rs`, `assets/voxygen/i18n/en/hud/bag.ftl` | S |
| First gated items (one per class) as proof | `assets/common/items/...` (4 RONs) | S |

**Milestone:** server rejects a Warrior equipping the Cleric sceptre; unrestricted items unaffected.

### Phase 2 — Client UX (M, ~3 dev-days)

| Task | Files | Size |
|---|---|---|
| Tooltip requirements block with red unmet lines | `voxygen/src/hud/util.rs`, i18n | M |
| Gray/red rendering in bag + drag-drop rejection | `voxygen/src/hud/bag.rs`, `voxygen/src/hud/slots.rs` | M |
| Trade/merchant tooltip parity (requirements visible when buying) | `voxygen/src/hud/trade.rs` | S |

**Milestone:** gated item is visibly unusable before any server round-trip, tooltip explains why.

**Total estimate:** ~6 dev-days (one senior dev + AI assistance).

## Testing strategy

- **Unit (common):** `meets_requirements` matrix — each gate alone, combined gates, empty
  requirements, `Adventurer` vs. class lists, level boundary (`level == min_level`) — in
  `common/src/comp/inventory/item/` tests; equip/swap-with-requirements cases added to the
  existing inventory suite `common/src/comp/inventory/test.rs` (helpers in `test_helpers.rs`).
  Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`.
- **Asset validation:** extend the asset-load tests that walk all item RONs so a typo'd
  `requirements` field fails CI rather than panicking at runtime.
- **Integration:** headless-client test (`bin_bot` feature path) that creates a Warrior, injects a
  Cleric-gated item into inventory server-side, sends `InventoryManip::Use`, and asserts the
  loadout slot is unchanged and an error message arrives; positive twin test with a Cleric.
- **Manual:** drag-drop rejection, tooltip rendering in bag and trade windows, behavior after
  `/select_class` (item turns usable without relog, since `CharacterClass` is synced).

## Open questions

1. Should equipped items that *become* illegal (e.g. future class change or item RON edits) be
   force-unequipped on login, or grandfathered while equipped? Current design: validate only on
   equip/swap; revisit if class change beyond the one-time legacy pick ever ships.
2. Do gated items need a visible badge in loot rolls/trade offers beyond the tooltip (e.g. a class
   icon on the slot), or is the tooltip enough for v1?
3. Should `min_level` also gate *using* consumables/lanterns (the other `InventoryManip::Use`
   semantics), or strictly loadout equips? Current design: loadout equips only.
