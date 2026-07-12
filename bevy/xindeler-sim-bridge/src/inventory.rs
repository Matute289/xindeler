//! BL-82 EM-5.6 — the inventory/bag mirror + the inventory-mutation intent
//! applicator (spec §3.2/§6, tasks T56.18/T56.19).
//!
//! Follows [`crate::combat_hud::mirror_combat_hud_state`]'s exact shape: its
//! own module doc comment explains WHY this lives as its own, separate,
//! additive system rather than folded into [`crate::mirror_sim_entities`] —
//! the same reasoning applies here verbatim (a distinct concern, reading
//! storages that function doesn't even open).
//!
//! ## The write half (isolation-law rule 4: sim writes via public APIs only)
//! [`apply_inventory_action_requests`] is the ONLY system in this module that
//! writes toward the sim, and it does so by re-emitting the client's
//! [`InventoryActionRequest`] payload as a `common::event::InventoryManipEvent`
//! through the sim's own public event bus (`common_state::State::
//! emit_event_now`) — the EXACT event `server::events::inventory_manip`
//! already exists to consume (swap/equip/drop/sort/craft/...). Nothing here
//! mutates a `comp::Inventory` storage directly.

use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        change_detection::NonSendMut,
        message::MessageReader,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res, ResMut},
    },
};
use bevy_replicon::prelude::FromClient;
use common::{comp, event::InventoryManipEvent, uid::Uid};
use specs::WorldExt;
use xindeler_protocol::{
    InventoryActionRequest, NetEquippedSlot, NetInventory, NetInventorySlot, NetOwnerOnly,
};

use crate::{SimMirror, SimServer, mirror_sim_entities, tick_sim};

/// Last-mirrored [`NetInventory`] per sim entity — the same dedup shape
/// [`crate::SimLoadoutCache`]/[`crate::combat_hud::CombatHudMirrorCache`]
/// already use: an inventory is `Vec`-shaped, so re-inserting an UNCHANGED
/// value every tick would still force replicon to treat it as mutated.
#[derive(Resource, Default, Debug)]
pub struct InventoryMirrorCache(HashMap<specs::Entity, NetInventory>);

/// Converts one sim `Item` into its wire projection. `#[allow(deprecated)]`:
/// `ItemDesc::legacy_name` is the one raw, plain-`&str` display name an item
/// carries pre-i18n-resolution — real i18n-driven naming is EM-5.16's job
/// (the `.ftl` catalog port), matching the SAME "themed placeholder,
/// deferred to the epic that owns real i18n depth" posture EM-5.2's
/// buff-strip colour swatches already established as reviewer-approved v1
/// scope.
#[allow(deprecated)]
pub(crate) fn item_name(item: &comp::Item) -> String { item.legacy_name().into_owned() }

/// Reads the sim's `comp::Inventory` for every currently-mirrored entity and
/// UPSERTs [`NetInventory`] (+ tags [`NetOwnerOnly`] with the entity's own
/// `Uid`, for the per-owner visibility scoping — see
/// `xindeler_protocol::owner_visibility`'s module doc comment). Mirrors
/// [`crate::combat_hud::mirror_combat_hud_state`]'s `Some(..) => insert /
/// None => remove` shape.
///
/// `NetOwnerOnly` is deliberately NEVER removed once written (mirrors how
/// [`xindeler_protocol::NetUid`] is a permanent identity, never revoked) —
/// see this module's own doc comment for why sharing it with
/// [`crate::trade::mirror_trade_state`] this way is safe.
pub fn mirror_inventory_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<InventoryMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    cache.0.retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let inventories = ecs.read_storage::<comp::Inventory>();
    let uids = ecs.read_storage::<Uid>();

    for (&sim_entity, &bevy_entity) in mirror.0.iter() {
        let mut ec = commands.entity(bevy_entity);

        if let Some(uid) = uids.get(sim_entity) {
            ec.insert(NetOwnerOnly(uid.0.get()));
        }

        match inventories.get(sim_entity) {
            Some(inventory) => {
                // Every PHYSICAL bag slot (occupied or not) — see
                // `NetInventorySlot`'s doc comment for why an empty slot
                // still needs a real address (a valid drop target).
                let slots = inventory
                    .slots_with_id()
                    .map(|(slot, item)| NetInventorySlot {
                        slot,
                        item: item.as_ref().map(|item| xindeler_protocol::NetItemStack {
                            item_id: item.item_definition_id().to_owned(),
                            name: item_name(item),
                            amount: item.amount(),
                            quality: item.quality(),
                        }),
                    })
                    .collect::<Vec<_>>();
                // Every POSSIBLE equip slot (see `xindeler_protocol::inventory::
                // ALL_EQUIP_SLOTS`'s doc comment) — `Inventory::equipped`
                // returns `None` for one that's currently empty.
                let equipped = xindeler_protocol::inventory::ALL_EQUIP_SLOTS
                    .into_iter()
                    .map(|slot| NetEquippedSlot {
                        slot,
                        item: inventory.equipped(slot).map(|item| {
                            xindeler_protocol::NetItemStack {
                                item_id: item.item_definition_id().to_owned(),
                                name: item_name(item),
                                amount: item.amount(),
                                quality: item.quality(),
                            }
                        }),
                    })
                    .collect::<Vec<_>>();
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a bag never holds u32::MAX slots"
                )]
                let capacity = inventory.capacity() as u32;
                let net_inventory = NetInventory {
                    slots,
                    equipped,
                    capacity,
                };
                if cache.0.get(&sim_entity) != Some(&net_inventory) {
                    ec.insert(net_inventory.clone());
                    cache.0.insert(sim_entity, net_inventory);
                }
            },
            None => {
                ec.remove::<NetInventory>();
                cache.0.remove(&sim_entity);
            },
        }
    }
}

/// Drains [`InventoryActionRequest`]s and re-emits each one as a
/// `common::event::InventoryManipEvent` through the sim's public event bus —
/// see the module doc comment. Resolves the acting entity via the embedded
/// local player's own `Uid` (`crate::player::player_sim_entity`, the SAME
/// resolution [`crate::mirror_sim_entities`] itself uses to tag the player's
/// mirror) — v1 has exactly one controllable entity per bridge instance
/// (the embedded local player), matching every other single-controllable-
/// entity assumption `xindeler-sim-bridge::player` already makes; resolving
/// a REMOTE client's own entity from its `ClientId` (multiple simultaneous
/// controllable entities) is EM-4.2b/EM-4.2c territory
/// (`xindeler-server-app::login`'s `ActiveReplicaSessions`), not duplicated
/// here.
pub fn apply_inventory_action_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<bevy::ecs::change_detection::NonSend<crate::EmbeddedPlayer>>,
    mut requests: MessageReader<FromClient<InventoryActionRequest>>,
) {
    let Some(sim) = sim else { return };
    let Some(entity) = player
        .and_then(|p| p.uid())
        .and_then(|uid| crate::player::player_sim_entity(&sim, uid))
    else {
        // No controllable entity resolved yet (still connecting, or this
        // bridge instance is a spectator-only fallback) — drop pending
        // requests rather than buffering them forever (degrade clean, spec
        // §3.2).
        requests.clear();
        return;
    };

    for FromClient { message, .. } in requests.read() {
        sim.server
            .state()
            .emit_event_now(InventoryManipEvent(entity, message.0.clone()));
    }
}

/// Registers [`mirror_inventory_state`] + [`apply_inventory_action_requests`]
/// in `FixedUpdate`, after the same systems [`crate::combat_hud::
/// CombatHudMirrorPlugin`] orders after (same reasoning: [`SimMirror`] must
/// be this tick's fresh map). Add alongside
/// [`crate::SimEntityMirrorPlugin`]/
/// [`crate::combat_hud::CombatHudMirrorPlugin`] in whichever shell hosts the
/// bridge.
pub struct InventoryMirrorPlugin;

impl Plugin for InventoryMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InventoryMirrorCache>().add_systems(
            FixedUpdate,
            (mirror_inventory_state, apply_inventory_action_requests)
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
    use xindeler_protocol::NetInventory;

    use super::*;
    use crate::{SimServer, boot_test_server};

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.init_resource::<InventoryMirrorCache>();
        app.insert_non_send(sim);
        app
    }

    /// Degrade-clean (spec §3.2): no mirrored entities is a harmless no-op.
    #[test]
    fn no_mirrored_entities_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(mirror_inventory_state)
            .expect("system runs without a mirrored entity");
    }

    /// A sim entity carrying a real `comp::Inventory` with an item in it
    /// mirrors to a `NetInventory` whose slot list contains that item's
    /// resolved id/name/amount/quality — the core T56.18 acceptance bar.
    #[test]
    fn mirrors_a_real_inventory_with_an_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut inventory = Inventory::with_empty();
            inventory
                .push(common::comp::Item::new_from_asset_expect(
                    "common.items.consumable.potion_minor",
                ))
                .expect("space for one potion");
            ecs.create_entity().with(inventory).build()
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_inventory_state)
            .expect("system runs");
        app.update();

        let net_inventory = app
            .world()
            .get::<NetInventory>(bevy_entity)
            .expect("NetInventory must be mirrored");
        assert_eq!(
            net_inventory.slots.len(),
            net_inventory.capacity as usize,
            "every PHYSICAL slot must have an entry, occupied or not"
        );
        let occupied: Vec<_> = net_inventory
            .slots
            .iter()
            .filter_map(|slot| slot.item.as_ref())
            .collect();
        assert_eq!(occupied.len(), 1, "exactly one slot is occupied");
        assert_eq!(occupied[0].amount, 1);
        assert!(!occupied[0].name.is_empty());
        assert!(
            net_inventory.equipped.len() == xindeler_protocol::inventory::ALL_EQUIP_SLOTS.len(),
            "every POSSIBLE equip slot must have an entry too"
        );
        assert!(
            net_inventory
                .equipped
                .iter()
                .all(|slot| slot.item.is_none()),
            "nothing was equipped in this fixture"
        );
    }

    /// An entity that loses its `comp::Inventory` outright has its
    /// `NetInventory` mirror removed too.
    #[test]
    fn removing_inventory_removes_the_net_mirror() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity().with(Inventory::with_empty()).build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_inventory_state)
            .expect("first run mirrors NetInventory");
        app.update();
        assert!(app.world().get::<NetInventory>(bevy_entity).is_some());

        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<Inventory>().remove(sim_entity);
        }

        app.world_mut()
            .run_system_once(mirror_inventory_state)
            .expect("second run removes the stale mirror");
        app.update();
        assert!(app.world().get::<NetInventory>(bevy_entity).is_none());
    }
}
