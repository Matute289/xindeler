//! BL-82 EM-5.6 — the two-party trade mirror + intent applicators (spec
//! §3.2/§6, §9 Q7=A "full parity, not sub-scoped"; task T56.21).
//!
//! Mirrors [`crate::combat_hud::mirror_combat_hud_state`]'s shape once more:
//! reads the sim's OWN trade/invite state (`common::comp::invite::Invite`,
//! `common::trade::Trades`) and projects it onto
//! [`xindeler_protocol::NetIncomingTradeInvite`]/[`xindeler_protocol::
//! NetTrade`] — see those types' own module doc comment
//! (`xindeler_protocol::trade`) for the full wire-shape reasoning (reusing
//! `TradeId`/`TradeAction`/`TradePhase` verbatim, why the counterparty's raw
//! inventory is never needed).
//!
//! ## The write half (isolation-law rule 4)
//! Three systems drain client requests and re-emit them through the sim's
//! public event bus (`common_state::State::emit_event_now`) — never
//! mutating `Trades`/`Invite`/`Inventory` storages directly:
//! - [`apply_trade_invite_requests`] → `common::event::InitiateInviteEvent`
//!   (`InviteKind::Trade`) — the SAME event `server::events::invite` already
//!   handles (range/alive checks, `Trades::begin_trade`, cancelling any prior
//!   trade).
//! - [`apply_trade_invite_response_requests`] → `common::event::
//!   InviteResponseEvent`.
//! - [`apply_trade_action_requests`] → `common::event::
//!   ProcessTradeActionEvent` (add/remove item, accept, decline — the SAME
//!   event `server::events::trade::handle_process_trade_action` already
//!   commits/rejects).

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
use bevy_replicon::prelude::FromClient;
use common::{
    comp,
    comp::{
        inventory::item::item_key::ItemKey,
        invite::{Invite, InviteKind, InviteResponse},
    },
    event::{InitiateInviteEvent, InviteResponseEvent, ProcessTradeActionEvent},
    trade::Trades,
    uid::{IdMaps, Uid},
};
use hashbrown::HashMap as SimHashMap;
use specs::WorldExt;
use xindeler_protocol::{
    NetIncomingTradeInvite, NetOwnerOnly, NetTrade, NetTradeOfferEntry, TradeActionRequest,
    TradeInviteRequest, TradeInviteResponseRequest,
};

use crate::{
    PlayerDimensionSession, SimMirror, SimServer,
    inventory::{item_equippable_slots, item_is_two_handed, item_name, resolve_client_entity},
    mirror_sim_entities, tick_sim,
};

/// Last-mirrored [`NetTrade`]/[`NetIncomingTradeInvite`] per sim entity —
/// same dedup shape as [`crate::inventory::InventoryMirrorCache`], INCLUDING
/// the `owner` dedup guard (bevy-migration-reviewer MAJOR, BL-82 EM-5.6
/// follow-up — see that cache's own doc comment for why re-inserting an
/// unchanged `NetOwnerOnly` every tick is a real, avoidable
/// `VisibilityFilter`-recomputation cost, not a hypothetical one).
#[derive(Resource, Default, Debug)]
pub struct TradeMirrorCache {
    trade: HashMap<specs::Entity, NetTrade>,
    invite: HashMap<specs::Entity, NetIncomingTradeInvite>,
    owner: HashMap<specs::Entity, u64>,
}

/// Resolves one side of a [`common::trade::PendingTrade`]'s offer
/// (`HashMap<InvSlotId, u32>`) into the wire-ready [`NetTradeOfferEntry`]
/// list, reading the offering party's own `comp::Inventory` for item
/// identity/name/quality/owned-amount (see `xindeler_protocol::trade`'s
/// module doc comment for why the OTHER party never needs this data).
fn resolve_offer(
    offer: &SimHashMap<common::comp::inventory::slot::InvSlotId, u32>,
    inventory: Option<&comp::Inventory>,
) -> Vec<NetTradeOfferEntry> {
    let Some(inventory) = inventory else {
        return Vec::new();
    };
    offer
        .iter()
        .filter_map(|(&slot, &offered)| {
            inventory.get(slot).map(|item| NetTradeOfferEntry {
                slot,
                item: xindeler_protocol::NetItemStack {
                    item_id: item.item_definition_id().to_owned(),
                    name: item_name(item),
                    amount: item.amount(),
                    quality: item.quality(),
                    is_two_handed: item_is_two_handed(item),
                    equippable_slots: item_equippable_slots(item),
                    icon_key: ItemKey::from(item),
                },
                offered,
                owned: item.amount(),
            })
        })
        .collect()
}

/// Reads the sim's `Invite`/`Trades` state for every currently-mirrored
/// entity and UPSERTs [`NetIncomingTradeInvite`]/[`NetTrade`] (+
/// [`NetOwnerOnly`], harmless-redundant with
/// [`crate::inventory::mirror_inventory_state`]'s own write — see that
/// component's doc comment for why two mirrors agreeing on the same value is
/// safe). Mirrors the `Some(..) => insert / None => remove` shape every
/// sibling mirror system uses.
pub fn mirror_trade_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<TradeMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    cache
        .trade
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .invite
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .owner
        .retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let uids = ecs.read_storage::<Uid>();
    let invites = ecs.read_storage::<Invite>();
    let inventories = ecs.read_storage::<comp::Inventory>();
    let id_maps = ecs.read_resource::<IdMaps>();
    let trades = ecs.read_resource::<Trades>();

    for (&sim_entity, &bevy_entity) in mirror.0.iter() {
        let mut ec = commands.entity(bevy_entity);
        let Some(&my_uid) = uids.get(sim_entity) else {
            continue;
        };
        let owner = my_uid.0.get();
        if cache.owner.get(&sim_entity) != Some(&owner) {
            ec.insert(NetOwnerOnly(owner));
            cache.owner.insert(sim_entity, owner);
        }

        // Incoming, not-yet-accepted invite (only ever meaningful for Trade
        // — a Group invite is EM-5.8's own concern).
        match invites.get(sim_entity) {
            Some(invite) if invite.kind == InviteKind::Trade => {
                let Some(&inviter_uid) = uids.get(invite.inviter) else {
                    ec.remove::<NetIncomingTradeInvite>();
                    cache.invite.remove(&sim_entity);
                    continue;
                };
                let net_invite = NetIncomingTradeInvite {
                    from_uid: inviter_uid.0.get(),
                };
                if cache.invite.get(&sim_entity) != Some(&net_invite) {
                    ec.insert(net_invite.clone());
                    cache.invite.insert(sim_entity, net_invite);
                }
            },
            _ => {
                ec.remove::<NetIncomingTradeInvite>();
                cache.invite.remove(&sim_entity);
            },
        }

        // An already-begun trade this entity is a party to.
        let active = trades
            .entity_trades
            .get(&my_uid)
            .and_then(|trade_id| trades.trades.get(trade_id).map(|trade| (*trade_id, trade)));
        match active {
            Some((trade_id, trade)) => {
                let Some(my_index) = trade.which_party(my_uid) else {
                    // Invariant violation (we're keyed by our own uid but
                    // aren't a party) — degrade clean rather than panic.
                    ec.remove::<NetTrade>();
                    cache.trade.remove(&sim_entity);
                    continue;
                };
                let their_index = 1 - my_index;
                let counterparty_uid = trade.parties[their_index];
                let counterparty_entity = id_maps.uid_entity(counterparty_uid);

                let my_offer = resolve_offer(&trade.offers[my_index], inventories.get(sim_entity));
                let their_offer = resolve_offer(
                    &trade.offers[their_index],
                    counterparty_entity.and_then(|e| inventories.get(e)),
                );

                let net_trade = NetTrade {
                    trade_id,
                    counterparty_uid: counterparty_uid.0.get(),
                    phase: trade.phase(),
                    my_offer,
                    their_offer,
                    my_accepted: trade.accept_flags[my_index],
                    their_accepted: trade.accept_flags[their_index],
                };
                if cache.trade.get(&sim_entity) != Some(&net_trade) {
                    ec.insert(net_trade.clone());
                    cache.trade.insert(sim_entity, net_trade);
                }
            },
            None => {
                ec.remove::<NetTrade>();
                cache.trade.remove(&sim_entity);
            },
        }
    }
}

/// Drains [`TradeInviteRequest`]s and re-emits each as `common::event::
/// InitiateInviteEvent` (`InviteKind::Trade`) — the sim resolves
/// `target_uid` → entity itself (`server::events::invite`), so an invalid/
/// out-of-range target is rejected sim-side exactly like the legacy client.
/// Resolves the acting entity PER MESSAGE via
/// [`crate::inventory::resolve_client_entity`] — see that function's doc
/// comment for why (BL-82 EM-5.6 follow-up, BLOCKER: a bridge-wide
/// embedded-player-only fallback silently dropped every real remote
/// client's trade actions on a dedicated server).
pub fn apply_trade_invite_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<bevy::ecs::change_detection::NonSend<crate::EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<TradeInviteRequest>>,
) {
    let Some(sim) = sim else {
        requests.clear();
        return;
    };
    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            continue;
        };
        // `Uid` wraps a `NonZeroU64` — a `target_uid` of `0` is never a real
        // sim identity (see `common::uid::Uid`), so it's simply skipped
        // rather than panicking on a malformed/stale request.
        let Some(target_uid) = std::num::NonZeroU64::new(message.target_uid).map(Uid) else {
            continue;
        };
        sim.server.state().emit_event_now(InitiateInviteEvent(
            entity,
            target_uid,
            InviteKind::Trade,
        ));
    }
}

/// Drains [`TradeInviteResponseRequest`]s and re-emits each as
/// `common::event::InviteResponseEvent`. Resolves the acting entity PER
/// MESSAGE — see [`apply_trade_invite_requests`]'s doc comment.
pub fn apply_trade_invite_response_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<bevy::ecs::change_detection::NonSend<crate::EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<TradeInviteResponseRequest>>,
) {
    let Some(sim) = sim else {
        requests.clear();
        return;
    };
    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            continue;
        };
        let response = if message.accept {
            InviteResponse::Accept
        } else {
            InviteResponse::Decline
        };
        sim.server
            .state()
            .emit_event_now(InviteResponseEvent(entity, response));
    }
}

/// Drains [`TradeActionRequest`]s and re-emits each as `common::event::
/// ProcessTradeActionEvent` — the sim itself validates that `entity` is
/// actually a party to `trade_id` (`Trades::process_trade_action`'s
/// `which_party` check), so a stale/forged trade id is rejected sim-side.
/// Resolves the acting entity PER MESSAGE — see
/// [`apply_trade_invite_requests`]'s doc comment.
pub fn apply_trade_action_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<bevy::ecs::change_detection::NonSend<crate::EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<TradeActionRequest>>,
) {
    let Some(sim) = sim else {
        requests.clear();
        return;
    };
    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            continue;
        };
        sim.server.state().emit_event_now(ProcessTradeActionEvent(
            entity,
            message.trade_id,
            message.action.clone(),
        ));
    }
}

/// Registers the trade mirror + the three request applicators in
/// `FixedUpdate`, ordered the same way every sibling mirror system is.
pub struct TradeMirrorPlugin;

impl Plugin for TradeMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TradeMirrorCache>().add_systems(
            FixedUpdate,
            (
                mirror_trade_state,
                apply_trade_invite_requests,
                apply_trade_invite_response_requests,
                apply_trade_action_requests,
            )
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
    use xindeler_protocol::NetIncomingTradeInvite;

    use super::*;
    use crate::{SimServer, boot_test_server};

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.init_resource::<TradeMirrorCache>();
        app.insert_non_send(sim);
        app
    }

    /// Degrade-clean: nothing mirrored is a harmless no-op.
    #[test]
    fn no_mirrored_entities_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.world_mut()
            .run_system_once(mirror_trade_state)
            .expect("system runs");
    }

    /// An entity carrying a real sim `Invite{kind: Trade, inviter}` mirrors
    /// to a `NetIncomingTradeInvite` carrying the inviter's `Uid` — the T56.21
    /// "incoming invite" acceptance bar.
    #[test]
    fn mirrors_an_incoming_trade_invite() {
        use common::uid::IdMaps;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let (inviter_entity, invitee_entity) = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let inviter = ecs.create_entity().build();
            let invitee = ecs
                .create_entity()
                .with(Invite {
                    inviter,
                    kind: InviteKind::Trade,
                })
                .build();
            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<IdMaps>();
            uids.insert(inviter, id_maps.allocate(inviter)).unwrap();
            uids.insert(invitee, id_maps.allocate(invitee)).unwrap();
            drop(uids);
            drop(id_maps);
            (inviter, invitee)
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(invitee_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_trade_state)
            .expect("system runs");
        app.update();

        let got_from_uid = app
            .world()
            .get::<NetIncomingTradeInvite>(bevy_entity)
            .expect("NetIncomingTradeInvite must be mirrored")
            .from_uid;
        let expected_uid = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.read_storage::<Uid>()
                .get(inviter_entity)
                .unwrap()
                .0
                .get()
        };
        assert_eq!(got_from_uid, expected_uid);
    }

    /// A real begun trade (via `Trades::begin_trade`) between two entities,
    /// each with an offered item in their own inventory, mirrors on BOTH
    /// entities' own copy — each seeing their own + the counterparty's
    /// offer resolved to real item data. The core T56.21 acceptance bar.
    #[test]
    fn mirrors_a_real_two_party_trade_with_resolved_offers() {
        use common::uid::IdMaps;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let (a_entity, b_entity, a_uid, b_uid, a_slot) = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();

            let mut a_inv = Inventory::with_empty();
            a_inv
                .push(common::comp::Item::new_from_asset_expect(
                    "common.items.consumable.potion_minor",
                ))
                .expect("space");
            let a_slot = a_inv
                .slots_with_id()
                .find_map(|(slot, item)| item.is_some().then_some(slot))
                .expect("the pushed item has a slot");

            let a_entity = ecs.create_entity().with(a_inv).build();
            let b_entity = ecs.create_entity().with(Inventory::with_empty()).build();

            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<IdMaps>();
            uids.insert(a_entity, id_maps.allocate(a_entity)).unwrap();
            uids.insert(b_entity, id_maps.allocate(b_entity)).unwrap();
            let a_uid = *uids.get(a_entity).unwrap();
            let b_uid = *uids.get(b_entity).unwrap();
            drop(uids);
            drop(id_maps);

            let mut trades = ecs.write_resource::<Trades>();
            let trade_id = trades.begin_trade(a_uid, b_uid);
            trades.trades.get_mut(&trade_id).unwrap().offers[0].insert(a_slot, 1);
            drop(trades);

            (a_entity, b_entity, a_uid, b_uid, a_slot)
        };

        let a_bevy = app.world_mut().spawn_empty().id();
        let b_bevy = app.world_mut().spawn_empty().id();
        {
            let mut mirror = app.world_mut().resource_mut::<SimMirror>();
            mirror.0.insert(a_entity, a_bevy);
            mirror.0.insert(b_entity, b_bevy);
        }

        app.world_mut()
            .run_system_once(mirror_trade_state)
            .expect("system runs");
        app.update();

        let a_trade = app
            .world()
            .get::<NetTrade>(a_bevy)
            .expect("party A's own NetTrade");
        assert_eq!(a_trade.counterparty_uid, b_uid.0.get());
        assert_eq!(a_trade.my_offer.len(), 1);
        assert_eq!(a_trade.my_offer[0].slot, a_slot);
        assert_eq!(a_trade.my_offer[0].offered, 1);
        assert!(a_trade.their_offer.is_empty());

        let b_trade = app
            .world()
            .get::<NetTrade>(b_bevy)
            .expect("party B's own NetTrade");
        assert_eq!(b_trade.counterparty_uid, a_uid.0.get());
        assert!(b_trade.my_offer.is_empty());
        assert_eq!(
            b_trade.their_offer.len(),
            1,
            "party B must see party A's offer resolved from A's OWN inventory"
        );
        assert_eq!(b_trade.their_offer[0].offered, 1);
    }
}
