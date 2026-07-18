//! BL-82 EM-5.15 — the crafting mirror (spec §3.2/§6, tasks T56.40-42).
//!
//! Follows the [`crate::inventory::NetInventory`] pattern exactly:
//! [`NetCrafting`] is a small, read-only, self-scoped projection of the sim's
//! authoritative crafting state — the player's known recipe book
//! (`common::comp::inventory::recipe_book::RecipeBook` resolved against the
//! `common::recipe::RecipeBookManifest`) plus the server-computed candidate
//! slot lists the four crafting tabs need (salvageable items, modular tool
//! components, repairable/damaged items). It carries NONE of the sim's
//! item-construction machinery (`AbilityMap`/`MaterialStatManifest`) —
//! "project, don't dump", the same discipline `NetInventory`'s own module doc
//! comment establishes.
//!
//! ## Why one crafting mirror, not fields on [`crate::inventory::NetItemStack`]
//! "Is this item salvageable / a modular component / damaged" is CRAFTING
//! state, not the item-identity every screen needs — folding it into the shared
//! `NetItemStack` (which `NetInventory` AND `NetTrade` both carry) would smear
//! a crafting concern across the whole item pipeline. Instead the candidate
//! lists here carry only the `InvSlotId`/`Slot` ADDRESS of each candidate; the
//! client cross-references that address against the
//! [`crate::inventory::NetInventory`] it already mirrors (same owner, same
//! tick) to render the item's name/quality/ rarity, reusing the EXACT
//! slot-render conventions `inventory_ui.rs` established. `NetItemStack` stays
//! untouched.
//!
//! ## Client → sim intent
//! There is NO new client→sim message: every crafting action (craft a recipe,
//! salvage, repair, forge a modular weapon) is already expressible as a
//! `common::comp::InventoryManip::CraftRecipe { craft_event, craft_sprite }`,
//! and [`crate::inventory::InventoryActionRequest`] already wraps
//! `InventoryManip` VERBATIM and is already drained by
//! `xindeler-sim-bridge::inventory::apply_inventory_action_requests` through
//! the sim's public event bus. The crafting UI just constructs the right
//! `common::comp::controller::CraftEvent` and sends it via the existing
//! request — no new wire type, no new applicator.
//!
//! ## The station gate (honest scope, spec §Q4=A)
//! The sim's `server::events::inventory_manip` handler gates salvage on a
//! `DismantlingBench` sprite, modular-weapon forging on a `CraftingBench`,
//! repair on a `RepairBench`, and every station-tagged recipe on that station's
//! own `SpriteKind` — supplied as the `craft_sprite: Option<VolumePos>` of the
//! request, which a client can only fill once it knows it is standing at that
//! station. The Bevy client has no crafting-station interaction / `VolumePos`
//! plumbing yet (a distinct engine-migration task), so [`NetRecipe::
//! craft_sprite`] is mirrored purely so the UI can DISPLAY the requirement; the
//! station-free recipes (`craft_sprite: None`, e.g. `craftsman_hammer`) are the
//! ones that complete end-to-end today. See this task's PR description for the
//! per-tab reality.

use bevy::ecs::component::Component;
use common::{
    comp::inventory::{
        item::{ItemDefinitionIdOwned, Quality, tool::ToolKind},
        slot::{InvSlotId, Slot},
    },
    terrain::SpriteKind,
};
use serde::{Deserialize, Serialize};

/// One ingredient of a recipe, projected for requirement-highlighting: the
/// display name, how many the recipe needs, and how many the player currently
/// has (summed SERVER-SIDE across every bag slot whose item satisfies the
/// input, via `Item::matches_recipe_input` — the real authority, never
/// re-implemented client-side). [`Self::satisfied`] drives the missing-
/// ingredient red flag (T56.41).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NetRecipeInput {
    /// Unlocalized display name of the ingredient (an item's `legacy_name`, or
    /// a tag/material name for tag-based inputs) — the same pre-i18n
    /// "themed placeholder" posture [`crate::inventory::NetItemStack::name`]
    /// already established for v1.
    pub name: String,
    /// How many of this ingredient one craft of the recipe consumes.
    pub required: u32,
    /// How many matching items the player currently holds (across all bag
    /// slots).
    pub available: u32,
}

impl NetRecipeInput {
    /// Whether the player holds enough of this ingredient for one craft — a
    /// DISPLAY heuristic (the missing-ingredient red flag, T56.41), not the
    /// authoritative craftability check. [`Self::available`] sums `amount()`
    /// across every matching slot, but `Item::matches_recipe_input`'s
    /// `TagSameItem`/`ListSameItem` variants additionally require a SINGLE
    /// stack to meet the requirement — so a requirement split across two
    /// sub-threshold stacks can report `available: 0` here while the
    /// authoritative per-recipe [`NetRecipe::craftable`] (computed by the real
    /// `Recipe::inventory_contains_ingredients`) disagrees in either
    /// direction. Harmless: the Craft button gates on `craftable`, never on
    /// this method (ecs-design-reviewer, BL-82 EM-5.15 review).
    #[must_use]
    pub fn satisfied(&self) -> bool { self.available >= self.required }
}

/// One KNOWN recipe (in the player's recipe book), projected for the recipes
/// tab. Carries the output display + every input's requirement/availability +
/// whether it is craftable right now + the server-resolved input slots the
/// craft request echoes back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetRecipe {
    /// The recipe-book key (e.g. `"craftsman_hammer"`) — echoed VERBATIM into
    /// `CraftEvent::Simple { recipe, .. }` when the player clicks Craft.
    pub key: String,
    /// Full identity of the output item (for a future `.vox`-icon lookup —
    /// same role as [`crate::inventory::NetItemStack::item_id`]).
    pub output_id: ItemDefinitionIdOwned,
    /// Unlocalized display name of the output item.
    pub output_name: String,
    /// How many output items one craft produces.
    pub output_amount: u32,
    /// Output item's rarity (drives the same rarity border the bag grid uses).
    pub output_quality: Quality,
    /// Every ingredient, with its required/available counts.
    pub inputs: Vec<NetRecipeInput>,
    /// Whether the player currently holds every ingredient in sufficient
    /// quantity (server-computed `Inventory::can_craft_recipe`). Independent of
    /// the [`Self::craft_sprite`] station requirement, which the sim checks
    /// separately at craft time.
    pub craftable: bool,
    /// The crafting-station sprite this recipe requires (`None` = craftable
    /// anywhere). Mirrored for DISPLAY only — see the module doc comment's
    /// "station gate" note.
    pub craft_sprite: Option<SpriteKind>,
    /// The server-resolved `(input-index, bag-slot)` pairs that satisfy this
    /// recipe's ingredients (the output of `Recipe::inventory_contains_
    /// ingredients`), or empty when the recipe is not currently craftable. The
    /// craft request echoes this straight back into `CraftEvent::Simple {
    /// slots, .. }`, so the client never re-implements recipe-input↔slot
    /// matching (which needs the real `Item`, not the projected mirror).
    pub craft_slots: Vec<(u32, InvSlotId)>,
}

/// A bag slot holding a modular tool component, for the modular-weapon tab
/// (T56.42). `is_primary`/`is_secondary` say which half of a modular weapon it
/// can form; `toolkind` lets the UI only offer to pair components of the SAME
/// tool kind (the sim's `recipe::modular_weapon` rejects a mismatch anyway —
/// this is a client-side convenience, not a re-implementation of the rule).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetModularComponentSlot {
    pub slot: InvSlotId,
    pub is_primary: bool,
    pub is_secondary: bool,
    pub toolkind: ToolKind,
}

/// A slot (bag or equipped) holding a damaged, durability-bearing item, for the
/// repair tab (T56.42). `durability_lost`/`max_durability` drive the damage bar
/// (both from the real `Item::durability_lost`/`Item::MAX_DURABILITY`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetRepairableSlot {
    /// Which slot the damaged item is in — echoed straight into
    /// `CraftEvent::Repair(slot)`.
    pub slot: Slot,
    /// Durability points lost (0 = pristine; `max_durability` = fully broken).
    pub durability_lost: u32,
    /// The maximum durability an item can lose (`Item::MAX_DURABILITY`).
    pub max_durability: u32,
}

/// The FULL crafting projection for the entity that owns it — recipe book +
/// the three candidate lists the salvage/repair/modular tabs need. A single
/// small `Net*` component (like [`crate::inventory::NetInventory`]), scoped via
/// [`crate::owner_visibility::NetOwnerOnly`] so it only replicates to the ONE
/// client that owns it.
///
/// Bounded in size the same way `NetInventory` is: a player knows a finite
/// recipe book and holds a finite bag, so this stays a "small `Net*`
/// component", not the unbounded-collection case spec §3.2's "bulk data =
/// messages" rule targets.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetCrafting {
    /// Every recipe in the player's recipe book (known recipes), for the
    /// recipes tab.
    pub recipes: Vec<NetRecipe>,
    /// Bag slots holding salvageable items, for the salvage tab.
    pub salvageable: Vec<InvSlotId>,
    /// Bag slots holding modular tool components, for the modular-weapon tab.
    pub components: Vec<NetModularComponentSlot>,
    /// Slots (bag or equipped) holding damaged items, for the repair tab.
    pub repairable: Vec<NetRepairableSlot>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_recipe_input_satisfied() {
        let short = NetRecipeInput {
            name: "Iron Ingot".to_owned(),
            required: 3,
            available: 1,
        };
        assert!(!short.satisfied());
        let enough = NetRecipeInput {
            name: "Iron Ingot".to_owned(),
            required: 3,
            available: 3,
        };
        assert!(enough.satisfied());
        let surplus = NetRecipeInput {
            name: "Wood".to_owned(),
            required: 1,
            available: 5,
        };
        assert!(surplus.satisfied());
    }

    /// The whole crafting mirror round-trips through bincode the same way every
    /// other `Net*` payload in this crate is exercised — a cheap guard that the
    /// derives (including the nested `SpriteKind`/`ToolKind`/`Slot` common
    /// types) actually hold together on the wire.
    #[test]
    fn net_crafting_round_trips_through_bincode() {
        let crafting = NetCrafting {
            recipes: vec![NetRecipe {
                key: "craftsman_hammer".to_owned(),
                output_id: ItemDefinitionIdOwned::Simple(
                    "common.items.tool.craftsman_hammer".to_owned(),
                ),
                output_name: "Craftsman Hammer".to_owned(),
                output_amount: 1,
                output_quality: Quality::Common,
                inputs: vec![
                    NetRecipeInput {
                        name: "Wood".to_owned(),
                        required: 1,
                        available: 2,
                    },
                    NetRecipeInput {
                        name: "Iron Ingot".to_owned(),
                        required: 3,
                        available: 0,
                    },
                ],
                craftable: false,
                craft_sprite: None,
                craft_slots: vec![],
            }],
            salvageable: vec![InvSlotId::new(0, 4)],
            components: vec![NetModularComponentSlot {
                slot: InvSlotId::new(0, 5),
                is_primary: true,
                is_secondary: false,
                toolkind: ToolKind::Sword,
            }],
            repairable: vec![NetRepairableSlot {
                slot: Slot::Inventory(InvSlotId::new(0, 6)),
                durability_lost: 4,
                max_durability: 12,
            }],
        };
        let bytes = bincode::serde::encode_to_vec(&crafting, bincode::config::legacy())
            .expect("serializes");
        let (decoded, _): (NetCrafting, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::legacy())
                .expect("deserializes");
        assert_eq!(decoded, crafting);
    }
}
