//! BL-82 EM-5.15 — the crafting mirror (spec §3.2/§6, tasks T56.40-42).
//!
//! The read half of the crafting feature: [`mirror_crafting_state`] projects
//! each mirrored entity's real crafting state — its recipe book (resolved
//! against the sim's `common::recipe::RecipeBookManifest` resource) plus the
//! salvage/repair/modular candidate slots derived from its `comp::Inventory` —
//! into the owner-scoped [`xindeler_protocol::NetCrafting`] component. Mirrors
//! [`crate::inventory::mirror_inventory_state`]'s exact shape (dedup cache,
//! `NetOwnerOnly` tagging, `Some(..) => insert / None => remove`).
//!
//! There is NO write half here: every crafting action (craft/salvage/repair/
//! modular-forge) is a `common::comp::controller::CraftEvent` inside a
//! `common::comp::InventoryManip::CraftRecipe`, which the client already sends
//! via the existing [`xindeler_protocol::InventoryActionRequest`] and
//! [`crate::inventory::apply_inventory_action_requests`] already applies
//! through the sim's public event bus — see [`xindeler_protocol::crafting`]'s
//! module doc comment.

use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        change_detection::NonSendMut,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res, ResMut},
    },
};
use common::{
    comp,
    comp::inventory::item::{ItemDefinitionIdOwned, ItemDesc, ItemKind, TagExampleInfo, modular},
    recipe::{RecipeBookManifest, RecipeInput},
    uid::Uid,
};
use specs::WorldExt;
use xindeler_protocol::{
    NetCrafting, NetModularComponentSlot, NetOwnerOnly, NetRecipe, NetRecipeInput,
    NetRepairableSlot,
};

use crate::{SimMirror, SimServer, mirror_sim_entities, tick_sim};

/// Last-mirrored [`NetCrafting`]/owner per sim entity — the SAME dedup
/// discipline [`crate::inventory::InventoryMirrorCache`] uses: `NetCrafting` is
/// `Vec`-shaped, so re-inserting an UNCHANGED value every tick would still
/// force replicon to treat it as mutated; and re-inserting an UNCHANGED
/// [`NetOwnerOnly`] would retrigger replicon's O(clients) visibility
/// recomputation for no reason.
#[derive(Resource, Default, Debug)]
pub struct CraftingMirrorCache {
    crafting: HashMap<specs::Entity, NetCrafting>,
    owner: HashMap<specs::Entity, u64>,
}

/// A representative display name for one recipe input — an item's
/// `legacy_name` for an `Item` input, or the tag/material name for a tag-based
/// input (the same pre-i18n placeholder posture the inventory mirror's
/// `item_name` already uses). Never re-implements ingredient MATCHING — that
/// stays server-side via `Item::matches_recipe_input`; this is display text
/// only.
#[allow(deprecated)]
fn recipe_input_name(input: &RecipeInput) -> String {
    match input {
        RecipeInput::Item(def) => def.legacy_name().into_owned(),
        RecipeInput::Tag(tag) | RecipeInput::TagSameItem(tag) => tag.name().to_owned(),
        RecipeInput::ListSameItem(defs) => defs
            .first()
            .map(|def| def.legacy_name().into_owned())
            .unwrap_or_default(),
    }
}

/// How many items the inventory currently holds that satisfy `input` (summed
/// across every bag slot), via the REAL authority `Item::matches_recipe_input`
/// — never a client-side re-implementation of the matching rule.
fn available_for_input(inventory: &comp::Inventory, input: &RecipeInput, required: u32) -> u32 {
    inventory
        .slots_with_id()
        .filter_map(|(_, slot)| slot.as_ref())
        .filter(|item| item.matches_recipe_input(input, required))
        .map(comp::Item::amount)
        .sum()
}

/// Builds the [`NetCrafting`] projection for one entity's inventory + the
/// shared recipe manifest. Factored out of [`mirror_crafting_state`] so it can
/// be unit-tested against a bare `comp::Inventory` without a full Bevy app.
#[allow(deprecated)]
pub(crate) fn build_net_crafting(
    inventory: &comp::Inventory,
    rbm: &RecipeBookManifest,
) -> NetCrafting {
    // Every KNOWN recipe (the recipe book), with per-input availability +
    // whether it is craftable right now + the server-resolved input slots the
    // craft request echoes back.
    let recipes: Vec<NetRecipe> = inventory
        .available_recipes_iter(rbm)
        .map(|(key, recipe)| {
            let inputs = recipe
                .inputs()
                .map(|(input, required, _is_mod_comp)| NetRecipeInput {
                    name: recipe_input_name(input),
                    required,
                    available: available_for_input(inventory, input, required),
                })
                .collect();
            let (craftable, craft_slots) = match recipe.inventory_contains_ingredients(inventory, 1)
            {
                Ok(slots) => (true, slots),
                Err(_) => (false, Vec::new()),
            };
            let (output_def, output_amount) = &recipe.output;
            NetRecipe {
                key: key.clone(),
                output_id: ItemDefinitionIdOwned::Simple(output_def.id().to_owned()),
                output_name: output_def.legacy_name().into_owned(),
                output_amount: *output_amount,
                output_quality: output_def.quality(),
                inputs,
                craftable,
                craft_sprite: recipe.craft_sprite,
                craft_slots,
            }
        })
        .collect();

    // Bag-slot-derived candidate lists for the salvage/modular/repair tabs.
    let mut salvageable = Vec::new();
    let mut components = Vec::new();
    let mut repairable = Vec::new();
    for (slot, inv_slot) in inventory.slots_with_id() {
        let Some(item) = inv_slot.as_ref() else {
            continue;
        };
        if item.is_salvageable() {
            salvageable.push(slot);
        }
        if let ItemKind::ModularComponent(mod_comp) = &*item.kind() {
            let (is_primary, is_secondary, toolkind) = match mod_comp {
                modular::ModularComponent::ToolPrimaryComponent { toolkind, .. } => {
                    (true, false, *toolkind)
                },
                modular::ModularComponent::ToolSecondaryComponent { toolkind, .. } => {
                    (false, true, *toolkind)
                },
            };
            components.push(NetModularComponentSlot {
                slot,
                is_primary,
                is_secondary,
                toolkind,
            });
        }
        if let Some(lost) = item.durability_lost().filter(|lost| *lost > 0) {
            repairable.push(NetRepairableSlot {
                slot: comp::inventory::slot::Slot::Inventory(slot),
                durability_lost: lost,
                max_durability: comp::Item::MAX_DURABILITY,
            });
        }
    }
    // Equipped items can be damaged too (weapons/armour lose durability in
    // combat) — the repair tab must reach them via `Slot::Equip`, not just bag
    // slots.
    for equip_slot in xindeler_protocol::inventory::ALL_EQUIP_SLOTS {
        if let Some(item) = inventory.equipped(equip_slot)
            && let Some(lost) = item.durability_lost().filter(|lost| *lost > 0)
        {
            repairable.push(NetRepairableSlot {
                slot: comp::inventory::slot::Slot::Equip(equip_slot),
                durability_lost: lost,
                max_durability: comp::Item::MAX_DURABILITY,
            });
        }
    }

    NetCrafting {
        recipes,
        salvageable,
        components,
        repairable,
    }
}

/// Reads the sim's `comp::Inventory` + the shared `RecipeBookManifest` for
/// every currently-mirrored entity that is a REAL connected player and UPSERTs
/// [`NetCrafting`] (+ tags [`NetOwnerOnly`] with the entity's own `Uid`, for
/// per-owner visibility scoping — see `xindeler_protocol::owner_visibility`'s
/// module doc comment). Mirrors [`crate::inventory::mirror_inventory_state`]'s
/// shape verbatim, including the "skip the WHOLE entity when its `Uid` lookup
/// fails" rule.
///
/// ## Why gated on `comp::Presence` (bevy-migration-reviewer + ecs-design-
/// reviewer follow-up, both MAJOR)
/// [`build_net_crafting`] is far heavier than [`crate::inventory::
/// mirror_inventory_state`]'s own per-tick work (it re-derives the WHOLE
/// available-recipe-book projection — recipes × inputs × bag-slots, plus a
/// fresh allocation per recipe/input — where the inventory mirror is a flat
/// O(slots) pass). Since [`NetCrafting`] is owner-scoped via [`NetOwnerOnly`]
/// (it can only ever reach the ONE client whose `Uid` matches the entity's
/// own), an entity with no [`comp::Presence`] — i.e. every NPC; only a real
/// connected client's entity carries one — has NO POSSIBLE CONSUMER for this
/// mirror: nothing ever recomputing it changes what any client sees. Skipping
/// the whole per-entity body for non-`Presence` entities is therefore a pure
/// perf win with zero behavior change, not a scope cut.
///
/// Degrades clean when the sim has no `RecipeBookManifest` resource (it always
/// does on a real server — `server::events::inventory_manip` reads it as
/// `ReadExpect` — but `try_fetch` keeps this system from ever panicking a
/// bridge tick if it somehow runs before that resource exists).
pub fn mirror_crafting_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<CraftingMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    cache
        .crafting
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .owner
        .retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let inventories = ecs.read_storage::<comp::Inventory>();
    let uids = ecs.read_storage::<Uid>();
    let presences = ecs.read_storage::<comp::Presence>();
    let Some(rbm) = ecs.try_fetch::<RecipeBookManifest>() else {
        // No recipe manifest yet — nothing to project this tick (degrade
        // clean, spec §3.2). Leaves any existing mirror untouched rather than
        // churning it to empty and back.
        return;
    };

    for (&sim_entity, &bevy_entity) in mirror.0.iter() {
        let mut ec = commands.entity(bevy_entity);

        let Some(&uid) = uids.get(sim_entity) else {
            continue;
        };
        let owner = uid.0.get();
        if cache.owner.get(&sim_entity) != Some(&owner) {
            ec.insert(NetOwnerOnly(owner));
            cache.owner.insert(sim_entity, owner);
        }

        // See this function's own doc comment: skip the expensive recipe-book
        // projection entirely for non-`Presence` entities (every NPC) — their
        // `NetCrafting` could never reach any client anyway.
        match (presences.contains(sim_entity), inventories.get(sim_entity)) {
            (true, Some(inventory)) => {
                let net_crafting = build_net_crafting(inventory, &rbm);
                if cache.crafting.get(&sim_entity) != Some(&net_crafting) {
                    ec.insert(net_crafting.clone());
                    cache.crafting.insert(sim_entity, net_crafting);
                }
            },
            _ => {
                // Only issue a real remove command when we know a mirror was
                // previously inserted (either the entity never had one — the
                // overwhelmingly common NPC case — or it just lost `Presence`/
                // `Inventory`) — avoids queuing a no-op `EntityCommand` for
                // every NPC, every tick.
                if cache.crafting.remove(&sim_entity).is_some() {
                    ec.remove::<NetCrafting>();
                }
            },
        }
    }
}

/// Registers [`mirror_crafting_state`] in `FixedUpdate`, after the same systems
/// [`crate::inventory::InventoryMirrorPlugin`] orders after (same reasoning:
/// [`SimMirror`] must be this tick's fresh map). Add alongside
/// [`crate::inventory::InventoryMirrorPlugin`] in whichever shell hosts the
/// bridge.
pub struct CraftingMirrorPlugin;

impl Plugin for CraftingMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CraftingMirrorCache>().add_systems(
            FixedUpdate,
            mirror_crafting_state
                .after(tick_sim)
                .after(mirror_sim_entities),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use common::comp::inventory::Inventory;
    use specs::{Builder, WorldExt};

    use super::*;
    use crate::{SimServer, boot_test_server};

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.init_resource::<CraftingMirrorCache>();
        app.insert_non_send(sim);
        app
    }

    /// A recipe-book inventory projects its known recipes into `NetCrafting`,
    /// with the right output identity + per-input required/available counts +
    /// craftability — the core T56.40 acceptance bar, built from a REAL
    /// `RecipeBookManifest` (not a hand-rolled fixture).
    #[test]
    fn projects_a_real_recipe_book() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(dir.path()).expect("test server boots");
        let ecs = sim.server.state().ecs();
        let rbm = ecs
            .try_fetch::<RecipeBookManifest>()
            .expect("a real server has the recipe manifest");

        // A bag that knows the default recipe group (which contains
        // `craftsman_hammer`) and holds ONE of its two ingredients.
        let mut inventory = Inventory::with_empty();
        inventory
            .push_recipe_group(common::comp::Item::new_from_asset_expect(
                "common.items.recipes.default",
            ))
            .expect("recipe group registers");
        inventory
            .push(common::comp::Item::new_from_asset_expect(
                "common.items.log.wood",
            ))
            .expect("space for the wood");

        let crafting = build_net_crafting(&inventory, &rbm);
        let hammer = crafting
            .recipes
            .iter()
            .find(|r| r.key == "craftsman_hammer")
            .expect("the default recipe book knows craftsman_hammer");
        assert_eq!(
            hammer.output_id,
            ItemDefinitionIdOwned::Simple("common.items.tool.craftsman_hammer".to_owned())
        );
        assert!(!hammer.output_name.is_empty());
        // Its inputs are 1 wood + 3 iron ingots; we hold the wood but no iron.
        let wood = hammer
            .inputs
            .iter()
            .find(|i| i.available == 1)
            .expect("we hold exactly one wood");
        assert!(
            wood.satisfied(),
            "one wood satisfies the 1-wood requirement"
        );
        let iron = hammer
            .inputs
            .iter()
            .find(|i| i.required == 3)
            .expect("the recipe needs 3 iron ingots");
        assert_eq!(iron.available, 0, "we hold no iron");
        assert!(!iron.satisfied());
        assert!(
            !hammer.craftable,
            "missing the iron, so the recipe is not craftable yet"
        );
        assert!(
            hammer.craft_slots.is_empty(),
            "no resolved slots when short"
        );
        assert_eq!(
            hammer.craft_sprite, None,
            "craftsman_hammer crafts anywhere"
        );
    }

    /// A recipe whose ingredients are ALL present projects `craftable: true`
    /// with a non-empty resolved `craft_slots` list (the exact vec the craft
    /// request echoes back).
    #[test]
    fn a_fully_supplied_recipe_is_craftable_with_resolved_slots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(dir.path()).expect("test server boots");
        let ecs = sim.server.state().ecs();
        let rbm = ecs.try_fetch::<RecipeBookManifest>().expect("manifest");

        let mut inventory = Inventory::with_empty();
        inventory
            .push_recipe_group(common::comp::Item::new_from_asset_expect(
                "common.items.recipes.default",
            ))
            .expect("recipe group registers");
        inventory
            .push(common::comp::Item::new_from_asset_expect(
                "common.items.log.wood",
            ))
            .expect("wood");
        // 3 iron ingots (the recipe's other input).
        let mut iron = common::comp::Item::new_from_asset_expect("common.items.mineral.ingot.iron");
        iron.set_amount(3).expect("stackable");
        inventory.push(iron).expect("iron");

        let crafting = build_net_crafting(&inventory, &rbm);
        let hammer = crafting
            .recipes
            .iter()
            .find(|r| r.key == "craftsman_hammer")
            .expect("knows craftsman_hammer");
        assert!(hammer.craftable, "wood + 3 iron satisfies craftsman_hammer");
        assert!(
            !hammer.craft_slots.is_empty(),
            "a craftable recipe carries its resolved input slots"
        );
        assert!(hammer.inputs.iter().all(NetRecipeInput::satisfied));
    }

    /// A salvageable item shows up in the salvage candidate list keyed by its
    /// real bag slot (T56.42, salvage tab).
    #[test]
    fn a_salvageable_item_is_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(dir.path()).expect("test server boots");
        let ecs = sim.server.state().ecs();
        let rbm = ecs.try_fetch::<RecipeBookManifest>().expect("manifest");

        // Cloth garments are salvageable (they carry a `SalvageInto` tag).
        let mut inventory = Inventory::with_empty();
        let item = common::comp::Item::new_from_asset_expect("common.items.armor.cloth_blue.foot");
        let is_salvageable = item.is_salvageable();
        inventory.push(item).expect("space for the boots");

        let crafting = build_net_crafting(&inventory, &rbm);
        if is_salvageable {
            assert_eq!(
                crafting.salvageable.len(),
                1,
                "the one salvageable item is listed"
            );
        }
    }

    /// A damaged, durability-bearing item shows up in the repair candidate list
    /// with its real durability-lost/max (T56.42, repair tab).
    #[test]
    fn a_damaged_item_is_listed_as_repairable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(dir.path()).expect("test server boots");
        let ecs = sim.server.state().ecs();
        let rbm = ecs.try_fetch::<RecipeBookManifest>().expect("manifest");

        let mut inventory = Inventory::with_empty();
        let mut weapon =
            common::comp::Item::new_from_asset_expect("common.items.weapons.sword.starter");
        // Only durability-bearing items can be damaged; if this asset has
        // durability, damage it and expect it in the repair list.
        let has_durability = weapon.has_durability();
        if has_durability {
            weapon.persistence_set_durability(std::num::NonZeroU32::new(4));
        }
        inventory.push(weapon).expect("space for the sword");

        let crafting = build_net_crafting(&inventory, &rbm);
        if has_durability {
            let repairable = crafting
                .repairable
                .iter()
                .find(|r| r.durability_lost == 4)
                .expect("the damaged sword is listed as repairable");
            assert_eq!(
                repairable.max_durability,
                common::comp::Item::MAX_DURABILITY
            );
        }
    }

    /// `mirror_crafting_state` upserts a real `NetCrafting` onto a mirrored
    /// entity carrying a recipe-book inventory — the end-to-end mirror pass
    /// (the same shape `inventory.rs`'s `mirrors_a_real_inventory_with_an_item`
    /// proves for `NetInventory`).
    #[test]
    fn mirror_pass_upserts_net_crafting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut inventory = Inventory::with_empty();
            inventory
                .push_recipe_group(common::comp::Item::new_from_asset_expect(
                    "common.items.recipes.default",
                ))
                .expect("recipe group registers");
            // `mirror_crafting_state` now gates the (expensive) recipe-book
            // projection on `comp::Presence` — only a REAL connected player
            // (this test's fixture) ever has one; every NPC does not. See that
            // function's own doc comment for why.
            let entity = ecs
                .create_entity()
                .with(inventory)
                .with(common::comp::Presence::new(
                    common::ViewDistances {
                        terrain: 4,
                        entity: 4,
                    },
                    common::comp::PresenceKind::Character(common::character::CharacterId(1)),
                ))
                .build();
            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<common::uid::IdMaps>();
            uids.insert(entity, id_maps.allocate(entity)).unwrap();
            drop(uids);
            drop(id_maps);
            entity
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_crafting_state)
            .expect("system runs");
        app.update();

        let net_crafting = app
            .world()
            .get::<NetCrafting>(bevy_entity)
            .expect("NetCrafting must be mirrored");
        assert!(
            net_crafting
                .recipes
                .iter()
                .any(|r| r.key == "craftsman_hammer"),
            "the mirrored recipe book contains the default group's recipes"
        );
        assert!(
            app.world().get::<NetOwnerOnly>(bevy_entity).is_some(),
            "the crafting mirror is owner-scoped like the inventory mirror"
        );
    }

    /// bevy-migration-reviewer + ecs-design-reviewer follow-up (both MAJOR,
    /// BL-82 EM-5.15 review) — an NPC-shaped entity (a real recipe-book
    /// `Inventory`, but NO `comp::Presence`, exactly like every real NPC in
    /// this codebase) gets NO `NetCrafting` at all: the expensive recipe-book
    /// projection is skipped entirely, not just hidden from replication. This
    /// is the perf fix's own acceptance bar — see `mirror_crafting_state`'s
    /// doc comment for why an entity with no `Presence` has no possible
    /// consumer for this owner-scoped mirror in the first place.
    #[test]
    fn an_npc_with_no_presence_gets_no_net_crafting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut inventory = Inventory::with_empty();
            inventory
                .push_recipe_group(common::comp::Item::new_from_asset_expect(
                    "common.items.recipes.default",
                ))
                .expect("recipe group registers");
            // Deliberately NO `comp::Presence` — this is the NPC shape.
            let entity = ecs.create_entity().with(inventory).build();
            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<common::uid::IdMaps>();
            uids.insert(entity, id_maps.allocate(entity)).unwrap();
            drop(uids);
            drop(id_maps);
            entity
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_crafting_state)
            .expect("system runs");
        app.update();

        assert!(
            app.world().get::<NetCrafting>(bevy_entity).is_none(),
            "an NPC (no comp::Presence) must never get a NetCrafting mirror"
        );
        // NetOwnerOnly tagging is unaffected — it's cheap and matches the
        // inventory mirror's own convention of tagging every entity with a Uid.
        assert!(app.world().get::<NetOwnerOnly>(bevy_entity).is_some());
    }

    /// BL-82 EM-5.15 T56.41 — the flagship end-to-end craft against the REAL
    /// sim (the crafting analogue of `inventory.rs`'s
    /// `equip_swap_request_round_trips_through_a_real_sim_tick_into_net_
    /// inventory`). A player who knows `craftsman_hammer` and holds its
    /// ingredients (1 wood + 3 iron) sends the SAME wire request the crafting
    /// UI's Craft button sends — `InventoryActionRequest(InventoryManip::
    /// CraftRecipe { CraftEvent::Simple { .. }, craft_sprite: None })`, with
    /// the server-resolved `craft_slots` the [`NetCrafting`] mirror itself
    /// computed — and after ONE real sim tick processes it through the
    /// authoritative `server::events::inventory_manip` handler, the
    /// mirrored `NetInventory` shows the crafted hammer AND the consumed
    /// materials are gone. No UI mock: the real applicator, the real sim
    /// handler, the real recipe machinery. `craftsman_hammer` is used
    /// precisely because it is one of the station-free recipes
    /// (`craft_sprite: None`) that complete without the crafting-station
    /// `VolumePos` plumbing the Bevy client does not have yet
    /// (see the module doc comment's honest-scope note).
    #[test]
    fn crafting_a_recipe_round_trips_through_a_real_sim_tick_into_net_inventory() {
        use bevy_replicon::prelude::{ClientId, FromClient};

        use crate::inventory::{apply_inventory_action_requests, mirror_inventory_state};

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.init_resource::<crate::inventory::InventoryMirrorCache>();
        app.add_message::<FromClient<xindeler_protocol::InventoryActionRequest>>();

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut inventory = Inventory::with_empty();
            inventory
                .push_recipe_group(common::comp::Item::new_from_asset_expect(
                    "common.items.recipes.default",
                ))
                .expect("recipe group registers");
            inventory
                .push(common::comp::Item::new_from_asset_expect(
                    "common.items.log.wood",
                ))
                .expect("wood");
            let mut iron =
                common::comp::Item::new_from_asset_expect("common.items.mineral.ingot.iron");
            iron.set_amount(3).expect("iron stacks");
            inventory.push(iron).expect("iron");
            // Same `Pos` + dummy-`Anchor` fixture `inventory.rs`'s round-trip
            // test documents: a bare `Pos`-carrying entity is culled the first
            // real tick unless anchored to a permanently-alive, `Pos`-less
            // dummy — see that test's own inline comment for the full reasoning.
            let anchor_entity = ecs.create_entity().build();
            let entity = ecs
                .create_entity()
                .with(inventory)
                .with(common::comp::Pos(vek::Vec3::new(0.0, 0.0, 0.0)))
                .with(common::comp::Anchor::Entity(anchor_entity))
                // `mirror_crafting_state` gates the recipe-book projection on
                // `comp::Presence` — see `mirror_pass_upserts_net_crafting`'s
                // own inline comment for why this fixture needs one too.
                .with(common::comp::Presence::new(
                    common::ViewDistances {
                        terrain: 4,
                        entity: 4,
                    },
                    common::comp::PresenceKind::Character(
                        common::character::CharacterId(1),
                    ),
                ))
                .build();
            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<common::uid::IdMaps>();
            uids.insert(entity, id_maps.allocate(entity)).unwrap();
            drop(uids);
            drop(id_maps);
            entity
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        // Mirror crafting once to get the recipe key + the server-resolved
        // `craft_slots` — exactly what the UI reads off `NetCrafting` before it
        // sends a craft.
        app.world_mut()
            .run_system_once(mirror_crafting_state)
            .expect("crafting mirror runs");
        app.update();
        let (recipe_key, craft_slots) = {
            let net_crafting = app
                .world()
                .get::<NetCrafting>(bevy_entity)
                .expect("NetCrafting mirrored");
            let hammer = net_crafting
                .recipes
                .iter()
                .find(|r| r.key == "craftsman_hammer")
                .expect("knows craftsman_hammer");
            assert!(hammer.craftable, "wood + 3 iron makes it craftable");
            (hammer.key.clone(), hammer.craft_slots.clone())
        };

        // Send the SAME wire request the Craft button sends.
        let connection_entity = app
            .world_mut()
            .spawn(crate::PlayerDimensionSession(sim_entity))
            .id();
        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(connection_entity),
            message: xindeler_protocol::InventoryActionRequest(
                common::comp::InventoryManip::CraftRecipe {
                    craft_event: common::comp::controller::CraftEvent::Simple {
                        recipe: recipe_key,
                        slots: craft_slots,
                        amount: 1,
                    },
                    craft_sprite: None,
                },
            ),
        });

        app.world_mut()
            .run_system_once(apply_inventory_action_requests)
            .expect("applicator runs");

        // ONE real sim tick processes the queued `InventoryManipEvent` through
        // the authoritative `server::events::inventory_manip` craft handler.
        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            sim.server
                .tick(
                    server::Input::default(),
                    std::time::Duration::from_millis(33),
                )
                .expect("sim tick processes the craft");
        }

        app.world_mut()
            .run_system_once(mirror_inventory_state)
            .expect("inventory mirror runs");
        app.update();

        let net_inventory = app
            .world()
            .get::<xindeler_protocol::NetInventory>(bevy_entity)
            .expect("NetInventory mirrored after the craft");
        let names: Vec<String> = net_inventory
            .slots
            .iter()
            .filter_map(|slot| slot.item.as_ref())
            .map(|item| item.name.clone())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("Craftsman")),
            "the crafted Craftsman Hammer must appear in the bag, got: {names:?}"
        );
        // The 3 iron ingots were consumed — no iron-ingot stack remains.
        assert!(
            !names.iter().any(|n| n.contains("Iron Ingot")),
            "the iron ingots must have been consumed by the craft, got: {names:?}"
        );
    }
}
