//! BL-82 EM-5.3 — the skillbar/hotbar mirror slice (spec §3.2): projects the
//! sim's `ActiveAbilities`/`AbilityPool`/`AbilityCooldowns` onto
//! `xindeler_protocol::{NetAbilities, NetCooldowns}`, following the exact
//! `NetHealth`/`NetLoadout` pattern `crate::combat_hud` already established
//! for EM-5.2 — a separate, additive system rather than folded into
//! `mirror_sim_entities`/`mirror_combat_hud_state`.
//!
//! Also owns the write half: [`apply_hotbar_assignment_requests`] drains
//! `FromClient<xindeler_protocol::AssignHotbarSlot>` (the real replicon
//! client message — see that type's own doc comment) and re-emits it as a
//! `common::event::ChangeAbilityEvent` through the sim's own public event
//! bus (`common_state::State::emit_event_now`), following
//! `crate::inventory::apply_inventory_action_requests`'s exact shape —
//! never a direct ECS mutation from this bridge (isolation-law rule 4).
//!
//! BL-82 EM-5.3 follow-up (bevy-migration-reviewer + ecs-design-reviewer):
//! this replaces a previous `apply_local_hotbar_assignment`, which drained a
//! separate `LocalAssignHotbarSlot` plain-Bevy-message (modeled after
//! EM-5.8's original `LocalGroupAction`, itself later removed in BL-82 EM-8.3's
//! unification) and resolved the acting client PURELY via the embedded-player
//! shortcut, ignoring which real client actually sent
//! the request — the exact anti-pattern EM-5.6 was blocked on by two
//! independent reviewers and fixed (see `crate::inventory::
//! resolve_client_entity`'s own doc comment). Harmless while
//! `HotbarMirrorPlugin` only ever ran on a listen-server with exactly one
//! real player, but silently dropped every real remote client's rebind
//! request on `xindeler-server-app` (the real dedicated, multi-client
//! server) the moment this plugin was wired in there (which it already is —
//! `bevy/xindeler-server-app/src/plugin.rs`).

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        change_detection::{NonSend, NonSendMut},
        message::MessageReader,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res, ResMut},
    },
};
use bevy_replicon::prelude::FromClient;
use common::{comp, event::ChangeAbilityEvent, resources::Time as SimTime};
use specs::WorldExt;
use xindeler_protocol::{
    AssignHotbarSlot, NetAbilities, NetAuxiliaryAbility, NetCooldownEntry, NetCooldowns,
    NetHotbarSlot,
};

use crate::{
    EmbeddedPlayer, PlayerDimensionSession, SimMirror, SimServer, inventory::resolve_client_entity,
    mirror_sim_entities, tick_sim,
};

/// Last-mirrored [`NetAbilities`] per sim entity — same dedup shape
/// `CombatHudMirrorCache` uses for `NetCombo`/`NetXp` (a slot binding only
/// changes on an equip-swap or an explicit rebind, so re-inserting an
/// unchanged `NetAbilities` every tick would be a real, avoidable bandwidth
/// cost for a `Vec<NetHotbarSlot>`-shaped component). `NetCooldowns` stays on
/// the always-overwrite path (like `NetBuffs`) — it legitimately changes
/// most ticks while anything is cooling down; entries are pruned here
/// alongside the cache, mirroring `CombatHudMirrorCache`'s own per-tick
/// prune shape.
#[derive(Resource, Default, Debug)]
pub struct HotbarMirrorCache(std::collections::HashMap<specs::Entity, NetAbilities>);

/// `common::comp::ability::AuxiliaryAbility` -> the replicable
/// [`NetAuxiliaryAbility`] (see that type's own doc comment for why both
/// exist). `pub(crate)`: `crate::skillset::mirror_skillset_state` (BL-82
/// EM-5.7) reuses this exact conversion for `NetAbilityPool`'s entries —
/// the SAME resolution `mirror_hotbar_state` does, just over
/// `AbilityPool::all_available_abilities` instead of the current bound-slot
/// set, so it must not diverge into a second copy.
pub(crate) fn to_net_aux(ability: comp::ability::AuxiliaryAbility) -> NetAuxiliaryAbility {
    use comp::ability::AuxiliaryAbility as A;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "ability-set indices are tiny (a handful of abilities per weapon/pool), never \
                  near u32::MAX"
    )]
    match ability {
        A::MainWeapon(i) => NetAuxiliaryAbility::MainWeapon(i as u32),
        A::OffWeapon(i) => NetAuxiliaryAbility::OffWeapon(i as u32),
        A::Glider(i) => NetAuxiliaryAbility::Glider(i as u32),
        A::Innate(i) => NetAuxiliaryAbility::Innate(i as u32),
        A::Empty => NetAuxiliaryAbility::Empty,
    }
}

/// The inverse of [`to_net_aux`] — used by [`apply_hotbar_assignment_requests`]
/// to turn a client's rebind request back into the sim's own
/// `AuxiliaryAbility` before emitting a `ChangeAbilityEvent`.
fn from_net_aux(ability: NetAuxiliaryAbility) -> comp::ability::AuxiliaryAbility {
    use comp::ability::AuxiliaryAbility as A;
    match ability {
        NetAuxiliaryAbility::MainWeapon(i) => A::MainWeapon(i as usize),
        NetAuxiliaryAbility::OffWeapon(i) => A::OffWeapon(i as usize),
        NetAuxiliaryAbility::Glider(i) => A::Glider(i as usize),
        NetAuxiliaryAbility::Innate(i) => A::Innate(i as usize),
        NetAuxiliaryAbility::Empty => A::Empty,
    }
}

/// Reads the sim's `ActiveAbilities`/`AbilityPool`/`AbilityCooldowns` for
/// every currently-mirrored entity ([`SimMirror`]) and UPSERTs
/// `NetAbilities`/`NetCooldowns` — mirrors `mirror_combat_hud_state`'s own
/// shape exactly (`Some(..) => insert` / `None => remove::<NetX>()`).
///
/// A no-op if no [`SimServer`] is booted yet — same early-out every sibling
/// mirror system uses.
pub fn mirror_hotbar_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<HotbarMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    cache.0.retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let now = *ecs.read_resource::<SimTime>();

    let active_abilities_storage = ecs.read_storage::<comp::ActiveAbilities>();
    let ability_pools = ecs.read_storage::<comp::ability::AbilityPool>();
    let cooldowns_storage = ecs.read_storage::<comp::ability::AbilityCooldowns>();
    let inventories = ecs.read_storage::<comp::Inventory>();
    let skill_sets = ecs.read_storage::<comp::SkillSet>();
    let char_states = ecs.read_storage::<comp::CharacterState>();
    let stances = ecs.read_storage::<comp::Stance>();
    let combos = ecs.read_storage::<comp::Combo>();
    let buffs_storage = ecs.read_storage::<comp::Buffs>();

    for (&sim_entity, &bevy_entity) in mirror.0.iter() {
        let mut ec = commands.entity(bevy_entity);

        match active_abilities_storage.get(sim_entity) {
            Some(active) => {
                let inv = inventories.get(sim_entity);
                let skill_set = skill_sets.get(sim_entity);
                let ability_pool = ability_pools.get(sim_entity);
                let char_state = char_states.get(sim_entity);
                let stance = stances.get(sim_entity);
                let combo = combos.get(sim_entity);
                let buffs = buffs_storage.get(sim_entity);

                let resolve = |ability: comp::ability::Ability| -> Option<String> {
                    ability
                        .ability_id(
                            char_state,
                            inv,
                            skill_set,
                            ability_pool,
                            stance,
                            combo,
                            buffs,
                        )
                        .map(str::to_owned)
                };

                let primary = resolve(comp::ability::Ability::from(active.primary));
                let secondary = resolve(comp::ability::Ability::from(active.secondary));
                let slots: Vec<NetHotbarSlot> = active
                    .auxiliary_set(inv, skill_set)
                    .iter()
                    .map(|&aux| NetHotbarSlot {
                        aux: to_net_aux(aux),
                        ability_id: resolve(comp::ability::Ability::from(aux)),
                    })
                    .collect();

                let net = NetAbilities {
                    primary,
                    secondary,
                    slots,
                };
                if cache.0.get(&sim_entity) != Some(&net) {
                    ec.insert(net.clone());
                    cache.0.insert(sim_entity, net);
                }
            },
            None => {
                ec.remove::<NetAbilities>();
                cache.0.remove(&sim_entity);
            },
        }

        match cooldowns_storage.get(sim_entity) {
            Some(cooldowns) if !cooldowns.0.is_empty() => {
                let mut entries: Vec<NetCooldownEntry> = cooldowns
                    .0
                    .iter()
                    .filter_map(|(ability_id, ready_at)| {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "a remaining cooldown is at most a few hundred seconds, well \
                                      within f32 precision for display purposes"
                        )]
                        let remaining = (ready_at.0 - now.0) as f32;
                        (remaining > 0.0).then(|| NetCooldownEntry {
                            ability_id: ability_id.clone(),
                            remaining_secs: remaining,
                        })
                    })
                    .collect();
                entries.sort_by(|a, b| a.ability_id.cmp(&b.ability_id));
                ec.insert(NetCooldowns(entries));
            },
            _ => {
                ec.remove::<NetCooldowns>();
            },
        }
    }
}

/// Drains [`FromClient<AssignHotbarSlot>`] and re-emits each one as a
/// `common::event::ChangeAbilityEvent` through the sim's public event bus —
/// see the module doc comment. Resolves the acting entity PER MESSAGE via
/// [`resolve_client_entity`] (a real dedicated server can have many
/// simultaneously-connected clients, each needing its OWN resolution, not a
/// single bridge-wide fallback), following
/// `crate::inventory::apply_inventory_action_requests`'s exact shape.
///
/// `auxiliary_key` is computed HERE from the resolved entity's own
/// `Inventory` (`comp::ActiveAbilities::active_auxiliary_key`) — the same
/// pure logic `client::Client::change_ability` used to compute inline before
/// sending; it is never carried over the wire since each entity must use its
/// OWN equipped tools, not whatever the requesting client happened to
/// compute.
pub fn apply_hotbar_assignment_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSend<EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<AssignHotbarSlot>>,
) {
    let Some(sim) = sim else {
        // No sim booted yet — drop pending requests rather than buffering
        // them forever (degrade clean, spec §3.2).
        requests.clear();
        return;
    };

    let ecs = sim.server.state().ecs();
    let inventories = ecs.read_storage::<comp::Inventory>();

    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            // This specific client's identity didn't resolve (still
            // connecting, or a stale/forged client id) — skip just this
            // request, not the whole batch (other clients' requests in the
            // same batch are unrelated and must still be processed).
            continue;
        };
        let auxiliary_key = comp::ActiveAbilities::active_auxiliary_key(inventories.get(entity));
        // `u32 as usize` is a widening (never-truncating) conversion on
        // every supported target — no `cast_possible_truncation` risk.
        sim.server.state().emit_event_now(ChangeAbilityEvent {
            entity,
            slot: message.slot as usize,
            auxiliary_key,
            new_ability: from_net_aux(message.ability),
        });
    }
}

/// Registers [`HotbarMirrorCache`] + [`mirror_hotbar_state`] +
/// [`apply_hotbar_assignment_requests`] in `FixedUpdate`, after `tick_sim`/
/// `mirror_sim_entities` — exactly `InventoryMirrorPlugin`'s own ordering
/// (this tick's fresh sim state, this tick's up-to-date `SimMirror` identity
/// map).
pub struct HotbarMirrorPlugin;

impl Plugin for HotbarMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HotbarMirrorCache>().add_systems(
            FixedUpdate,
            (mirror_hotbar_state, apply_hotbar_assignment_requests)
                .after(tick_sim)
                .after(mirror_sim_entities),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use bevy_replicon::prelude::ClientId;
    use common::{event::ChangeAbilityEvent, resources::Time};
    use specs::{Builder, WorldExt};
    use xindeler_protocol::{NetAbilities, NetAuxiliaryAbility, NetCooldowns};

    use super::*;
    use crate::{SimServer, boot_test_server};

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.init_resource::<HotbarMirrorCache>();
        app.insert_non_send(sim);
        app
    }

    /// `mirror_hotbar_state` is a documented no-op when [`SimMirror`] is
    /// empty (nothing mirrored yet) — the "degrade clean" rule (spec §3.2).
    #[test]
    fn no_mirrored_entities_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(mirror_hotbar_state)
            .expect("system runs without a mirrored entity");
    }

    /// A sim entity carrying a real `ActiveAbilities`/`Inventory`/`SkillSet`,
    /// once registered in `SimMirror`, gets `NetAbilities` UPSERTed with a
    /// slot count matching `ActiveAbilities::auxiliary_set`'s real length —
    /// the core EM-5.3 acceptance bar (spec §6-style: "prove the pattern
    /// end-to-end").
    #[test]
    fn mirrors_ability_pool_and_slot_bindings_for_a_real_sim_entity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity()
                .with(comp::ActiveAbilities::default_limited(
                    comp::ability::BASE_ABILITY_LIMIT,
                ))
                .with(comp::SkillSet::default())
                .build()
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_hotbar_state)
            .expect("system runs");
        app.update();

        let abilities = app
            .world()
            .get::<NetAbilities>(bevy_entity)
            .expect("NetAbilities must be mirrored");
        assert_eq!(
            abilities.slots.len(),
            comp::ability::BASE_ABILITY_LIMIT,
            "the mirrored slot count must match ActiveAbilities' real limit, not a hardcoded 10"
        );
        // No weapon equipped, no inventory at all -> every slot resolves to
        // Empty with no display id (degrade clean, not a panic).
        assert!(
            abilities
                .slots
                .iter()
                .all(|s| s.aux == NetAuxiliaryAbility::Empty && s.ability_id.is_none())
        );
    }

    /// An entity that loses its sim-side `ActiveAbilities` (e.g. turned into
    /// a pure prop) has `NetAbilities` removed too — the `None =>
    /// ec.remove::<NetX>()` arm.
    #[test]
    fn removing_active_abilities_removes_the_net_mirror_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity()
                .with(comp::ActiveAbilities::default())
                .build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_hotbar_state)
            .expect("first run mirrors NetAbilities");
        app.update();
        assert!(app.world().get::<NetAbilities>(bevy_entity).is_some());

        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<comp::ActiveAbilities>()
                .remove(sim_entity);
        }

        app.world_mut()
            .run_system_once(mirror_hotbar_state)
            .expect("second run must remove the now-stale mirror");
        app.update();
        assert!(
            app.world().get::<NetAbilities>(bevy_entity).is_none(),
            "NetAbilities must be removed once the sim-side ActiveAbilities is gone"
        );
    }

    /// A real `AbilityCooldowns` entry with a future ready-at time mirrors as
    /// a `NetCooldownEntry` with a positive `remaining_secs`; an EXPIRED one
    /// (ready-at in the past) is excluded — "an absent entry means ready"
    /// (this module's own doc comment).
    #[test]
    fn mirrors_only_still_cooling_down_abilities() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let now = *ecs.read_resource::<Time>();

            let mut cooldowns = comp::ability::AbilityCooldowns::default();
            cooldowns.set("class.warrior.rally", now, 8.0);
            cooldowns
                .0
                .insert("already.expired".to_owned(), Time(now.0 - 5.0));

            ecs.create_entity().with(cooldowns).build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_hotbar_state)
            .expect("system runs");
        app.update();

        let cooldowns = app
            .world()
            .get::<NetCooldowns>(bevy_entity)
            .expect("NetCooldowns must be mirrored");
        assert_eq!(cooldowns.0.len(), 1, "only the still-cooling entry mirrors");
        assert_eq!(cooldowns.0[0].ability_id, "class.warrior.rally");
        assert!(cooldowns.0[0].remaining_secs > 0.0);
    }

    /// [`to_net_aux`]/[`from_net_aux`] round-trip every variant — the
    /// conversion [`apply_hotbar_assignment_requests`] relies on to turn a
    /// client rebind request back into the sim's own enum.
    #[test]
    fn net_aux_conversion_round_trips_every_variant() {
        use comp::ability::AuxiliaryAbility as A;

        for aux in [
            A::MainWeapon(2),
            A::OffWeapon(0),
            A::Glider(1),
            A::Innate(3),
            A::Empty,
        ] {
            assert_eq!(from_net_aux(to_net_aux(aux)), aux);
        }
    }

    /// Drains every queued [`ChangeAbilityEvent`] straight from the sim's own
    /// `EventBus` (`common::event::EventBus::recv_all`) — the same public
    /// read API the sim's own dispatcher uses, letting these tests verify
    /// EXACTLY what [`apply_hotbar_assignment_requests`] resolved and queued
    /// without needing a full sim tick (out of scope here: these tests are
    /// about client-identity resolution, not the downstream
    /// `ActiveAbilities::change_ability` mutation, which is already covered
    /// by `common::comp::ability`'s own unit tests).
    fn drain_change_ability_events(app: &mut App) -> Vec<ChangeAbilityEvent> {
        let sim = app.world_mut().non_send_mut::<SimServer>();
        sim.server
            .state()
            .ecs()
            .read_resource::<common::event::EventBus<ChangeAbilityEvent>>()
            .recv_all()
            .collect()
    }

    /// BL-82 EM-5.3 follow-up (bevy-migration-reviewer + ecs-design-reviewer,
    /// both BLOCKER — the exact anti-pattern EM-5.6 was blocked on and
    /// fixed): [`apply_hotbar_assignment_requests`] resolves a REAL client
    /// connection (`ClientId::Client`) via its [`PlayerDimensionSession`] and
    /// re-emits the request as a `ChangeAbilityEvent` targeting THAT
    /// connection's own sim entity — the core fix for "every remote client's
    /// hotbar rebind was silently dropped on a dedicated server" — without
    /// needing an `EmbeddedPlayer` at all (exactly the path a genuine
    /// `xindeler-server-app` remote client takes; `EmbeddedPlayer` is the
    /// OTHER, listen-server-only path, covered by the sibling test below).
    #[test]
    fn apply_hotbar_assignment_requests_changes_the_real_connections_own_entity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<AssignHotbarSlot>>();

        let target_sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity()
                .with(comp::ActiveAbilities::default_limited(
                    comp::ability::BASE_ABILITY_LIMIT,
                ))
                .build()
        };
        let connection_entity = app
            .world_mut()
            .spawn(PlayerDimensionSession(target_sim_entity))
            .id();

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(connection_entity),
            message: AssignHotbarSlot {
                slot: 0,
                ability: NetAuxiliaryAbility::MainWeapon(2),
            },
        });

        app.world_mut()
            .run_system_once(apply_hotbar_assignment_requests)
            .expect("system runs");

        let events = drain_change_ability_events(&mut app);
        assert_eq!(
            events.len(),
            1,
            "exactly one ChangeAbilityEvent must be queued for the resolved entity"
        );
        assert_eq!(events[0].entity, target_sim_entity);
        assert_eq!(events[0].slot, 0);
        assert_eq!(
            events[0].new_ability,
            comp::ability::AuxiliaryAbility::MainWeapon(2)
        );
    }

    /// A real client connection with NO [`PlayerDimensionSession`] yet
    /// (still logging in, or a stale/forged connection entity) must NOT
    /// change any entity's ability — a real client's action must never get
    /// misattributed (here: to nothing changing at all, rather than falling
    /// back to whatever entity happens to be resolved next).
    #[test]
    fn apply_hotbar_assignment_requests_ignores_an_unresolved_real_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<AssignHotbarSlot>>();

        let connection_entity = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(connection_entity),
            message: AssignHotbarSlot {
                slot: 0,
                ability: NetAuxiliaryAbility::MainWeapon(1),
            },
        });

        app.world_mut()
            .run_system_once(apply_hotbar_assignment_requests)
            .expect("system runs without panicking");

        assert!(
            drain_change_ability_events(&mut app).is_empty(),
            "an unresolved real connection must not queue a ChangeAbilityEvent"
        );
    }

    /// `ClientId::Server` (the listen-server's own local loopback echo) with
    /// NO `EmbeddedPlayer` present degrades clean — no event, no panic —
    /// matching `resolve_client_entity`'s own equivalent test in
    /// `crate::inventory`.
    #[test]
    fn apply_hotbar_assignment_requests_degrades_clean_for_server_id_without_embedded_player() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<AssignHotbarSlot>>();

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Server,
            message: AssignHotbarSlot {
                slot: 0,
                ability: NetAuxiliaryAbility::MainWeapon(1),
            },
        });

        app.world_mut()
            .run_system_once(apply_hotbar_assignment_requests)
            .expect("system runs without panicking despite no EmbeddedPlayer");

        assert!(drain_change_ability_events(&mut app).is_empty());
    }
}
