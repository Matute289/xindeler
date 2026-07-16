//! BL-82 EM-5.6 — the inventory/bag mirror (spec §3.2/§6, tasks T56.18).
//!
//! Follows the `NetHealth`/`NetLoadout`/EM-5.2 `NetEnergy` pattern exactly:
//! [`NetInventory`] is a small, read-only, self-scoped projection of the sim's
//! `common::comp::inventory::Inventory` — NOT the whole component (`Inventory`
//! carries `AbilityMap`/`MaterialStatManifest`-dependent item construction
//! machinery the client has no business touching; "project, don't dump").
//!
//! ## Wire shapes reuse `common` types verbatim where they are ALREADY
//! `Serialize`/`Deserialize` plain data — the same discipline `NetLoadout`
//! established for `ToolKind`/`Hands`: [`common::comp::inventory::slot::
//! InvSlotId`]/[`common::comp::inventory::slot::EquipSlot`]/
//! [`common::comp::inventory::item::ItemDefinitionIdOwned`]/
//! [`common::comp::inventory::item::Quality`] all derive
//! `Serialize + Deserialize` already, so they travel on the wire unchanged
//! instead of this crate inventing parallel copies.
//!
//! ## Client → sim intent
//! [`InventoryActionRequest`] wraps `common::comp::InventoryManip` (itself
//! already `Serialize + Deserialize` — the exact payload
//! `server::events::inventory_manip` already knows how to apply via the sim's
//! public `InventoryManipEvent` bus) VERBATIM rather than inventing a new
//! enum — this is "the existing item-move/trade event type", per the
//! isolation-law brief, not a new mirror mechanism. `xindeler-sim-bridge`'s
//! `inventory::apply_inventory_action_requests` is the one system that reads
//! it and re-emits it as `InventoryManipEvent` through the sim's own public
//! event bus (`common_state::State::emit_event_now`) — never mutating
//! `Inventory` storages directly.
//!
//! ## Visibility (spec §3.2 "interest-managed")
//! [`NetInventory`] is scoped with [`crate::owner_visibility::NetOwnerOnly`]
//! (see that module's doc comment) so a bag's contents only ever replicate to
//! the ONE connected client that owns them — never to any other nearby
//! client, even one that can already see the entity's `NetPos`/`NetHealth`/etc.

use common::comp::inventory::{
    item::{ItemDefinitionIdOwned, Quality},
    slot::{ArmorSlot, EquipSlot, InvSlotId},
};
use serde::{Deserialize, Serialize};

use bevy::ecs::{component::Component, message::Message};

/// The canonical, STABLE list of every possible [`EquipSlot`] (16
/// [`ArmorSlot`] variants + 6 weapon/lantern/glider slots — `common`'s enum
/// itself has no `EnumIter`/`Sequence` derive, so this crate names them
/// explicitly once, here, rather than duplicating the list in both
/// `xindeler-sim-bridge` (which emits one [`NetEquippedSlot`] per entry, EVEN
/// when empty — see that struct's doc comment) AND `xindeler-client` (which
/// needs the SAME order to address a paper-doll grid position back to a real
/// `EquipSlot`).
pub const ALL_EQUIP_SLOTS: [EquipSlot; 22] = [
    EquipSlot::Armor(ArmorSlot::Head),
    EquipSlot::Armor(ArmorSlot::Neck),
    EquipSlot::Armor(ArmorSlot::Shoulders),
    EquipSlot::Armor(ArmorSlot::Chest),
    EquipSlot::Armor(ArmorSlot::Hands),
    EquipSlot::Armor(ArmorSlot::Ring1),
    EquipSlot::Armor(ArmorSlot::Ring2),
    EquipSlot::Armor(ArmorSlot::Back),
    EquipSlot::Armor(ArmorSlot::Belt),
    EquipSlot::Armor(ArmorSlot::Legs),
    EquipSlot::Armor(ArmorSlot::Feet),
    EquipSlot::Armor(ArmorSlot::Tabard),
    EquipSlot::Armor(ArmorSlot::Bag1),
    EquipSlot::Armor(ArmorSlot::Bag2),
    EquipSlot::Armor(ArmorSlot::Bag3),
    EquipSlot::Armor(ArmorSlot::Bag4),
    EquipSlot::ActiveMainhand,
    EquipSlot::ActiveOffhand,
    EquipSlot::InactiveMainhand,
    EquipSlot::InactiveOffhand,
    EquipSlot::Lantern,
    EquipSlot::Glider,
];

/// One item's identity/display data — shared shape between
/// [`NetInventorySlot`] and [`NetEquippedSlot`] (and reused verbatim by
/// `xindeler_protocol::trade::NetTradeOfferEntry`, which embeds the same
/// fields inline rather than nesting this type — see that struct's own doc
/// comment for why: a trade offer additionally needs `offered`/`owned`
/// counts alongside identity, so nesting would just add an indirection with
/// no shared-mutation benefit).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetItemStack {
    /// Full item identity (needed to distinguish a modular weapon from a
    /// simple one, and as the future `.vox`-icon lookup key — EM-5.1's own
    /// doc comment defers that primitive to whichever screen needs it first;
    /// this is that screen).
    pub item_id: ItemDefinitionIdOwned,
    /// Raw, unlocalized display name (`ItemDesc::legacy_name()`) — the v1
    /// tooltip/label text. Full i18n resolution is EM-5.16's job (the
    /// `.ftl` catalog port); this is the same "themed placeholder,
    /// reviewer-approved for v1" posture EM-5.2's buff-strip colour swatches
    /// already established.
    pub name: String,
    pub amount: u32,
    pub quality: Quality,
    /// BL-82 EM-5.17 T57.15 — whether this item is a two-handed weapon
    /// (`ItemKind::Tool` with `tool.hands == Hands::Two`). A small, additive
    /// mirror field (the SAME kind of addition `quality` itself already was,
    /// EM-5.6) rather than new architecture: the Phase 7 equipment panel
    /// needs to know, client-side, whether a Mainhand's item should visually
    /// disable its paired Offhand slot (`common::comp::inventory::slot::
    /// EquipSlot::can_hold` already ENFORCES this server/client-logic-side —
    /// this field only lets the HUD REFLECT that existing state, per that
    /// function's own doc comment: "Phase 7 must NOT re-implement this
    /// enforcement, only read the paired Mainhand's current item's `Hands`").
    /// Chosen over a client-side item-definition lookup because no such
    /// lookup crosses the logic/shell isolation boundary today (the client
    /// only ever sees already-projected `Net*` mirrors, never raw
    /// `ItemDefinitionId` → `ItemDef` resolution machinery — that machinery
    /// lives in `common`/`common-assets` behind `Inventory`'s own
    /// `AbilityMap`/`MaterialStatManifest` dependencies, which is exactly
    /// what this module's own doc comment says `NetInventory` deliberately
    /// does NOT expose to the client: "project, don't dump"). `false` for
    /// every non-`Tool` item (armor, consumables, …), matching
    /// `EquipSlot::can_hold`'s own `Hands::One` default posture.
    pub is_two_handed: bool,
    /// BL-82 EM-5.18 Phase 2 (T58.7, spec §3.4) — every [`EquipSlot`] this
    /// item is compatible with, computed SERVER-SIDE by calling the real
    /// authority ([`common::comp::inventory::slot::EquipSlot::can_hold`])
    /// across every entry in [`crate::inventory::ALL_EQUIP_SLOTS`]. The
    /// client NEVER re-implements slot-compatibility matching — it only ever
    /// membership-tests this list (the same "project, don't dump" posture
    /// this struct's own module doc comment already establishes for
    /// `is_two_handed`). Empty for non-equippable items (consumables,
    /// currency, quest items, etc.).
    pub equippable_slots: Vec<EquipSlot>,
}

/// One bag slot, projected for the bag-grid UI (EM-5.6 T56.19) — EVERY
/// physical slot up to [`NetInventory::capacity`], not just occupied ones:
/// an EMPTY slot still needs a real [`InvSlotId`] address so the client can
/// drag an item INTO it (a drop target with `item: None`), not just drag
/// occupied ones around.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetInventorySlot {
    /// Which bag slot this is — round-trips verbatim into
    /// [`InventoryActionRequest`] when the player drags to/from it.
    pub slot: InvSlotId,
    /// `None` = this physical slot is currently empty (a valid drop
    /// target, nothing to display/drag).
    pub item: Option<NetItemStack>,
}

/// One equipped (paper-doll) slot, projected the same way as
/// [`NetInventorySlot`] but keyed by [`EquipSlot`] instead of [`InvSlotId`].
/// Like [`NetInventorySlot`], one entry per POSSIBLE equip slot (not just
/// occupied ones) so the paper-doll can show every slot as a drop target.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetEquippedSlot {
    pub slot: EquipSlot,
    pub item: Option<NetItemStack>,
}

/// The FULL bag + paper-doll projection for the entity that owns it — a
/// single small `Net*` component (like [`crate::NetBuffs`]), not a bulk
/// message: a real inventory tops out in the low hundreds of slots, the same
/// size class `NetBuffs`/`NetLoadout` already replicate as components, not
/// the unbounded-collection case spec §3.2's "bulk data = messages" rule
/// targets (map/recipe-book/chat, which ARE unbounded or streamed).
///
/// Self-scoped via [`crate::owner_visibility::NetOwnerOnly`] — see the module
/// doc comment.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetInventory {
    /// One entry per PHYSICAL bag slot (`0..capacity`, see [`Self::capacity`]).
    pub slots: Vec<NetInventorySlot>,
    /// One entry per POSSIBLE equip slot — see [`NetEquippedSlot`].
    pub equipped: Vec<NetEquippedSlot>,
    /// Total bag capacity (`Inventory::capacity()`) — matches
    /// `slots.len()`; kept as its own field so the UI can sanity-check
    /// without recounting.
    pub capacity: u32,
}

/// Client → sim inventory-mutation intent (EM-5.6 T56.19): swap/equip/
/// unequip/drop/sort. Wraps `common::comp::InventoryManip` VERBATIM — see the
/// module doc comment for why this is not a new mirror mechanism.
///
/// Registered via `add_client_message` (real replicon wire message,
/// `XindelerProtocolPlugin`). In listen-server mode (`ClientState::
/// Disconnected`) `bevy_replicon` drains this locally as
/// `FromClient<InventoryActionRequest>` with `client_id ==
/// ClientId::Server` (`ClientMessageAppExt::add_client_message`'s own doc
/// comment) — so the SAME wire type and the SAME bridge system serve both
/// the embedded local player and a genuine remote client, with no separate
/// "local-only" shadow path (unlike `PlayerInput`/`LocalPlayerInput`, whose
/// split predates/avoids this drain-locally behavior for a much
/// higher-frequency per-frame sample; a discrete, infrequent action like an
/// inventory move has no such cost to avoid).
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InventoryActionRequest(pub common::comp::InventoryManip);

#[cfg(test)]
mod tests {
    use common::comp::{
        InventoryManip,
        inventory::{item::Quality, slot::Slot},
    };

    use super::*;

    #[test]
    fn net_inventory_slot_is_plain_serde_data() {
        let slot = NetInventorySlot {
            slot: InvSlotId::new(0, 3),
            item: Some(NetItemStack {
                item_id: ItemDefinitionIdOwned::Simple(
                    "common.items.weapons.sword.starter".to_owned(),
                ),
                name: "Starter Sword".to_owned(),
                amount: 1,
                quality: Quality::Common,
                is_two_handed: false,
                equippable_slots: vec![EquipSlot::ActiveMainhand],
            }),
        };
        // Round-trips through bincode the same way every other Net* payload
        // in this crate is exercised (see `lib.rs`'s own component tests) —
        // a cheap guard that the derive actually holds together.
        let bytes =
            bincode::serde::encode_to_vec(&slot, bincode::config::legacy()).expect("serializes");
        let (decoded, _): (NetInventorySlot, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::legacy())
                .expect("deserializes");
        assert_eq!(decoded, slot);
    }

    #[test]
    fn an_empty_slot_round_trips_as_none() {
        let slot = NetInventorySlot {
            slot: InvSlotId::new(0, 5),
            item: None,
        };
        let bytes =
            bincode::serde::encode_to_vec(&slot, bincode::config::legacy()).expect("serializes");
        let (decoded, _): (NetInventorySlot, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::legacy())
                .expect("deserializes");
        assert_eq!(decoded, slot);
        assert!(decoded.item.is_none());
    }

    /// BL-82 EM-5.17 T57.15 — `is_two_handed: true` round-trips through
    /// bincode too, not just the `false` default this file's other fixture
    /// happens to use.
    #[test]
    fn a_two_handed_item_stack_round_trips_true() {
        let slot = NetInventorySlot {
            slot: InvSlotId::new(0, 4),
            item: Some(NetItemStack {
                item_id: ItemDefinitionIdOwned::Simple(
                    "common.items.weapons.greatsword.starter".to_owned(),
                ),
                name: "Starter Greatsword".to_owned(),
                amount: 1,
                quality: Quality::Common,
                is_two_handed: true,
                equippable_slots: vec![EquipSlot::ActiveMainhand, EquipSlot::InactiveMainhand],
            }),
        };
        let bytes =
            bincode::serde::encode_to_vec(&slot, bincode::config::legacy()).expect("serializes");
        let (decoded, _): (NetInventorySlot, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::legacy())
                .expect("deserializes");
        assert_eq!(decoded, slot);
        assert!(decoded.item.expect("item present").is_two_handed);
    }

    #[test]
    fn inventory_action_request_wraps_inventory_manip_verbatim() {
        let manip = InventoryManip::Swap(
            Slot::Inventory(InvSlotId::new(0, 1)),
            Slot::Inventory(InvSlotId::new(0, 2)),
        );
        let request = InventoryActionRequest(manip.clone());
        assert_eq!(request.0, manip);
    }
}
