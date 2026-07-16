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
        system::{Commands, Query, Res, ResMut},
    },
};
use bevy_replicon::prelude::{ClientId, FromClient};
use common::{comp, event::InventoryManipEvent, uid::Uid};
use specs::WorldExt;
use xindeler_protocol::{
    InventoryActionRequest, NetEquippedSlot, NetInventory, NetInventorySlot, NetOwnerOnly,
};

use crate::{PlayerDimensionSession, SimMirror, SimServer, mirror_sim_entities, tick_sim};

/// Last-mirrored [`NetInventory`] per sim entity — the same dedup shape
/// [`crate::SimLoadoutCache`]/[`crate::combat_hud::CombatHudMirrorCache`]
/// already use: an inventory is `Vec`-shaped, so re-inserting an UNCHANGED
/// value every tick would still force replicon to treat it as mutated.
///
/// `owner` is the SAME dedup discipline applied to [`NetOwnerOnly`]
/// (bevy-migration-reviewer MAJOR, BL-82 EM-5.6 follow-up): re-inserting an
/// UNCHANGED `NetOwnerOnly` every tick would retrigger `bevy_replicon`'s
/// O(clients) `VisibilityFilter` recomputation for no reason, exactly the
/// cost [`crate::SimRegionCache`] already exists to avoid for `RegionKey`.
#[derive(Resource, Default, Debug)]
pub struct InventoryMirrorCache {
    inventory: HashMap<specs::Entity, NetInventory>,
    owner: HashMap<specs::Entity, u64>,
}

/// Converts one sim `Item` into its wire projection. `#[allow(deprecated)]`:
/// `ItemDesc::legacy_name` is the one raw, plain-`&str` display name an item
/// carries pre-i18n-resolution — real i18n-driven naming is EM-5.16's job
/// (the `.ftl` catalog port), matching the SAME "themed placeholder,
/// deferred to the epic that owns real i18n depth" posture EM-5.2's
/// buff-strip colour swatches already established as reviewer-approved v1
/// scope.
#[allow(deprecated)]
pub(crate) fn item_name(item: &comp::Item) -> String { item.legacy_name().into_owned() }

/// BL-82 EM-5.17 T57.15 — whether `item` is a two-handed weapon
/// (`ItemKind::Tool` with `tool.hands == Hands::Two`). Populates
/// [`xindeler_protocol::NetItemStack::is_two_handed`] — see that field's own
/// doc comment for why this is the chosen resolution of T57.15's data gap
/// (a small additive mirror field, not a client-side item-definition lookup).
/// `pub(crate)` (not private): `crate::trade`'s own `NetItemStack`
/// construction site (`resolve_offer`) reuses this exact helper, the same
/// way it already reuses [`item_name`].
pub(crate) fn item_is_two_handed(item: &comp::Item) -> bool {
    matches!(
        &*item.kind(),
        comp::inventory::item::ItemKind::Tool(tool)
            if tool.hands == comp::inventory::item::tool::Hands::Two
    )
}

/// BL-82 EM-5.18 T58.7 — every [`comp::inventory::slot::EquipSlot`] `item` is
/// compatible with, populating [`xindeler_protocol::NetItemStack::
/// equippable_slots`] — see that field's own doc comment for why this
/// projects rather than re-implements slot-compatibility matching. Calls the
/// REAL authority, `EquipSlot::can_hold`, once per entry in
/// [`xindeler_protocol::inventory::ALL_EQUIP_SLOTS`] — this crate never
/// duplicates that match. `pub(crate)` (not private): `crate::trade`'s own
/// `NetItemStack` construction site (`resolve_offer`) reuses this exact
/// helper, the same way it already reuses [`item_is_two_handed`]/[`item_name`].
pub(crate) fn item_equippable_slots(item: &comp::Item) -> Vec<comp::inventory::slot::EquipSlot> {
    let kind = item.kind();
    xindeler_protocol::inventory::ALL_EQUIP_SLOTS
        .into_iter()
        .filter(|slot| slot.can_hold(&kind))
        .collect()
}

/// Reads the sim's `comp::Inventory` for every currently-mirrored entity and
/// UPSERTs [`NetInventory`] (+ tags [`NetOwnerOnly`] with the entity's own
/// `Uid`, for the per-owner visibility scoping — see
/// `xindeler_protocol::owner_visibility`'s module doc comment). Mirrors
/// [`crate::combat_hud::mirror_combat_hud_state`]'s `Some(..) => insert /
/// None => remove` shape.
///
/// BL-82 EM-5.6 follow-up (ecs-design-reviewer MAJOR): an entity whose `Uid`
/// lookup fails this tick now `continue`s past EVERYTHING for that entity
/// (matching [`crate::trade::mirror_trade_state`]'s own stricter shape)
/// rather than the previous inconsistent behaviour of skipping only the
/// `NetOwnerOnly` tag while still upserting `NetInventory` — the latter
/// would (transiently) leave a real `NetInventory` on an entity with NO
/// `NetOwnerOnly` at all, which per `bevy_replicon`'s own documented
/// behaviour (see `xindeler_protocol::visibility`'s module doc comment: "an
/// entity that never carries \[the filter component\] at all... keeps
/// replicon's ordinary DEFAULT-VISIBLE behavior") would make that bag
/// visible to every client, not just its owner — the exact leak this whole
/// filter exists to prevent. `NetOwnerOnly` itself is deliberately NEVER
/// REMOVED once written (mirrors how [`xindeler_protocol::NetUid`] is a
/// permanent identity, never revoked) but IS now dedup-cached (mirroring
/// [`crate::SimRegionCache`]'s own reasoning for `RegionKey`) so it's only
/// actually re-inserted the first tick it's known — see this module's own
/// doc comment for why sharing it with [`crate::trade::mirror_trade_state`]
/// this way is safe.
pub fn mirror_inventory_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<InventoryMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    cache
        .inventory
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .owner
        .retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let inventories = ecs.read_storage::<comp::Inventory>();
    let uids = ecs.read_storage::<Uid>();

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
                            is_two_handed: item_is_two_handed(item),
                            equippable_slots: item_equippable_slots(item),
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
                                is_two_handed: item_is_two_handed(item),
                                equippable_slots: item_equippable_slots(item),
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
                if cache.inventory.get(&sim_entity) != Some(&net_inventory) {
                    ec.insert(net_inventory.clone());
                    cache.inventory.insert(sim_entity, net_inventory);
                }
            },
            None => {
                ec.remove::<NetInventory>();
                cache.inventory.remove(&sim_entity);
            },
        }
    }
}

/// Resolves the SIM ENTITY a `FromClient<_>` request should be attributed to
/// — real client identity FIRST, via [`PlayerDimensionSession`] (the SAME
/// connection-entity ↔ sim-entity correlation `xindeler-server-app::login`
/// establishes for every REAL remote client, at the exact call site that
/// also inserts [`xindeler_protocol::ClientOwnedUid`] — see that module's
/// doc comment), falling back to the embedded local player ONLY when the
/// client has NO connection entity at all (`ClientId::entity()` returns
/// `None` for `ClientId::Server`, which is exactly what a listen-server's
/// OWN local write drains as — see `bevy_replicon`'s `ClientMessageAppExt::
/// add_client_message` doc comment: "drained... and written locally as
/// `FromClient<M>`... with `client_id` equal to `ClientId::Server`").
///
/// BL-82 EM-5.6 follow-up (bevy-migration-reviewer + ecs-design-reviewer,
/// both BLOCKER): every applicator in this module and `trade.rs` used to
/// resolve the acting entity via the embedded-player shortcut
/// UNCONDITIONALLY, ignoring `client_id` entirely. On `xindeler-server-app`
/// (the real dedicated server) there is no `EmbeddedPlayer` at all, so
/// EVERY real remote client's inventory/trade action was silently dropped
/// (a direct contradiction of this epic's locked "real replication
/// end-to-end, not a stub" scope, §9 Q7=A) — and even on a listen-server
/// with a SECOND real connection, that client's actions would have been
/// misattributed to the embedded host. This function is the shared fix,
/// reused by every applicator in both this module and `crate::trade`.
pub(crate) fn resolve_client_entity(
    client_id: ClientId,
    sim: &SimServer,
    player: Option<&crate::EmbeddedPlayer>,
    sessions: &Query<&PlayerDimensionSession>,
) -> Option<specs::Entity> {
    match client_id.entity() {
        Some(connection_entity) => sessions
            .get(connection_entity)
            .ok()
            .map(|session| session.0),
        None => player
            .and_then(|p| p.uid())
            .and_then(|uid| crate::player::player_sim_entity(sim, uid)),
    }
}

/// Drains [`InventoryActionRequest`]s and re-emits each one as a
/// `common::event::InventoryManipEvent` through the sim's public event bus —
/// see the module doc comment. Resolves the acting entity PER MESSAGE via
/// [`resolve_client_entity`] (a real dedicated server can have many
/// simultaneously-connected clients, each needing its OWN resolution, not a
/// single bridge-wide fallback).
pub fn apply_inventory_action_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<bevy::ecs::change_detection::NonSend<crate::EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<InventoryActionRequest>>,
) {
    let Some(sim) = sim else {
        // No sim booted yet — drop pending requests rather than buffering
        // them forever (degrade clean, spec §3.2).
        requests.clear();
        return;
    };

    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            // This specific client's identity didn't resolve (still
            // connecting, or a stale/forged client id) — skip just this
            // request, not the whole batch (other clients' requests in the
            // same batch are unrelated and must still be processed).
            continue;
        };
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
            // A real mirrored entity always carries a `Uid` (every entity
            // `mirror_sim_entities` mirrors is created via
            // `create_entity_synced`) — `mirror_inventory_state` now
            // requires one too (ecs-design-reviewer follow-up: skip the
            // WHOLE entity, not just the `NetOwnerOnly` tag, when it's
            // missing — see that fix's own doc comment).
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

    /// BL-82 EM-5.17 T57.15 — a two-handed weapon (`common.items.weapons.
    /// sword.starter`, `hands: Two` per its own RON) mirrors with
    /// `is_two_handed: true`; a one-handed weapon (`common.items.weapons.
    /// dagger.starter_dagger`, `hands: One`) mirrors `false`. This is the
    /// data T57.15's paired-Offhand-disable visual reads client-side.
    #[test]
    fn mirrors_is_two_handed_from_the_real_item_definition() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut inventory = Inventory::with_empty();
            inventory
                .push(common::comp::Item::new_from_asset_expect(
                    "common.items.weapons.sword.starter",
                ))
                .expect("space for the two-handed sword");
            inventory
                .push(common::comp::Item::new_from_asset_expect(
                    "common.items.weapons.dagger.starter_dagger",
                ))
                .expect("space for the one-handed dagger");
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
            .run_system_once(mirror_inventory_state)
            .expect("system runs");
        app.update();

        let net_inventory = app
            .world()
            .get::<NetInventory>(bevy_entity)
            .expect("NetInventory must be mirrored");
        let occupied: Vec<_> = net_inventory
            .slots
            .iter()
            .filter_map(|slot| slot.item.as_ref())
            .collect();
        assert_eq!(occupied.len(), 2, "both items are in the bag");
        assert!(
            occupied
                .iter()
                .any(|item| item.name.contains("Greatsword") && item.is_two_handed),
            "the two-handed sword must mirror is_two_handed: true"
        );
        assert!(
            occupied
                .iter()
                .any(|item| item.name.contains("Dagger") && !item.is_two_handed),
            "the one-handed dagger must mirror is_two_handed: false"
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
            let entity = ecs.create_entity().with(Inventory::with_empty()).build();
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

    /// BL-82 EM-5.6 follow-up (bevy-migration-reviewer + ecs-design-reviewer,
    /// both BLOCKER): [`resolve_client_entity`] resolves a REAL client
    /// connection (`ClientId::Client`) via its [`PlayerDimensionSession`] —
    /// the core fix for "every remote client's action was silently dropped
    /// on a dedicated server" — without needing a real embedded player at
    /// all (this is exactly the path a genuine `xindeler-server-app` remote
    /// client takes; `EmbeddedPlayer` is the OTHER, listen-server-only path,
    /// covered by the sibling test below).
    #[test]
    fn resolve_client_entity_uses_the_real_connection_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let target_sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity().build()
        };
        let connection_entity = app
            .world_mut()
            .spawn(PlayerDimensionSession(target_sim_entity))
            .id();
        let client_id = ClientId::Client(connection_entity);

        let resolved = app
            .world_mut()
            .run_system_once(
                move |sim: bevy::ecs::change_detection::NonSend<SimServer>,
                      sessions: Query<&PlayerDimensionSession>| {
                    resolve_client_entity(client_id, &sim, None, &sessions)
                },
            )
            .expect("system runs");

        assert_eq!(resolved, Some(target_sim_entity));
    }

    /// A real client connection with NO [`PlayerDimensionSession`] yet
    /// (still logging in, or a stale/forged connection entity) resolves to
    /// `None` rather than falling back to the embedded player — a real
    /// client's action must never get misattributed to the host.
    #[test]
    fn resolve_client_entity_returns_none_for_an_unresolved_real_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        let connection_entity = app.world_mut().spawn_empty().id();
        let client_id = ClientId::Client(connection_entity);

        let resolved = app
            .world_mut()
            .run_system_once(
                move |sim: bevy::ecs::change_detection::NonSend<SimServer>,
                      sessions: Query<&PlayerDimensionSession>| {
                    resolve_client_entity(client_id, &sim, None, &sessions)
                },
            )
            .expect("system runs");

        assert_eq!(resolved, None);
    }

    /// `ClientId::Server` (no connection entity at all — the listen-server's
    /// own local loopback write) with no `EmbeddedPlayer` resolves to `None`
    /// rather than panicking — degrade clean.
    #[test]
    fn resolve_client_entity_returns_none_for_server_id_without_an_embedded_player() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let resolved = app
            .world_mut()
            .run_system_once(
                |sim: bevy::ecs::change_detection::NonSend<SimServer>,
                 sessions: Query<&PlayerDimensionSession>| {
                    resolve_client_entity(ClientId::Server, &sim, None, &sessions)
                },
            )
            .expect("system runs");

        assert_eq!(resolved, None);
    }

    /// BL-82 EM-5.18 T58.8 — [`item_equippable_slots`] on a REAL armor item
    /// (`Armor{kind: Foot, ..}`) resolves to exactly the one matching
    /// [`EquipSlot`] — the core "projects the real authority" acceptance bar
    /// for the equip-picker's data gap.
    #[test]
    fn item_equippable_slots_resolves_armor_to_its_one_matching_slot() {
        use common::comp::inventory::slot::{ArmorSlot, EquipSlot};

        let boots = comp::Item::new_from_asset_expect("common.items.testing.test_boots");
        assert_eq!(item_equippable_slots(&boots), vec![EquipSlot::Armor(
            ArmorSlot::Feet
        )]);
    }

    /// A two-handed tool resolves to BOTH mainhands and NEITHER offhand —
    /// `EquipSlot::can_hold` rejects `Hands::Two` on `*Offhand` (see that
    /// function's own doc comment) — this is the same real greatsword asset
    /// [`mirrors_is_two_handed_from_the_real_item_definition`] already uses.
    #[test]
    fn item_equippable_slots_resolves_a_two_handed_tool_to_both_mainhands_only() {
        use common::comp::inventory::slot::EquipSlot;

        let greatsword = comp::Item::new_from_asset_expect("common.items.weapons.sword.starter");
        assert_eq!(item_equippable_slots(&greatsword), vec![
            EquipSlot::ActiveMainhand,
            EquipSlot::InactiveMainhand,
        ]);
    }

    /// A one-handed tool resolves to all FOUR weapon slots (both mainhands
    /// AND both offhands) — the same real starter-dagger asset
    /// [`mirrors_is_two_handed_from_the_real_item_definition`] already uses.
    #[test]
    fn item_equippable_slots_resolves_a_one_handed_tool_to_all_four_weapon_slots() {
        use common::comp::inventory::slot::EquipSlot;

        let dagger =
            comp::Item::new_from_asset_expect("common.items.weapons.dagger.starter_dagger");
        assert_eq!(item_equippable_slots(&dagger), vec![
            EquipSlot::ActiveMainhand,
            EquipSlot::ActiveOffhand,
            EquipSlot::InactiveMainhand,
            EquipSlot::InactiveOffhand,
        ]);
    }

    /// A non-equippable item (currency) resolves to an empty list — matching
    /// [`xindeler_protocol::NetItemStack::equippable_slots`]'s own documented
    /// `[]` default for this case.
    #[test]
    fn item_equippable_slots_resolves_a_non_equippable_item_to_empty() {
        let coins = comp::Item::new_from_asset_expect("common.items.utility.coins");
        assert!(item_equippable_slots(&coins).is_empty());
    }
}
