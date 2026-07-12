//! BL-82 EM-5.3 — the skillbar/hotbar mirror slice (spec §3.2): projects the
//! sim's `ActiveAbilities`/`AbilityPool`/`AbilityCooldowns` onto
//! `xindeler_protocol::{NetAbilities, NetCooldowns}`, following the exact
//! `NetHealth`/`NetLoadout` pattern `crate::combat_hud` already established
//! for EM-5.2 — a separate, additive system rather than folded into
//! `mirror_sim_entities`/`mirror_combat_hud_state`.
//!
//! Also owns the write half: [`apply_local_hotbar_assignment`] drains
//! `xindeler_protocol::LocalAssignHotbarSlot` (the listen-server in-process
//! counterpart of the real `AssignHotbarSlot` client message — see that
//! type's own doc comment) and calls straight into the embedded player's
//! `client::Client::change_ability` (via [`crate::player::EmbeddedPlayer::
//! assign_hotbar_slot`]) — a REAL client->server network send over the
//! loopback socket, never a direct ECS mutation from this bridge
//! (isolation-law rule 4). Mirrors EM-5.8's `LocalGroupAction`/
//! `apply_local_group_actions` precedent exactly.

use bevy::{
    app::{App, FixedUpdate, Plugin, Update},
    ecs::{
        change_detection::NonSendMut,
        message::MessageReader,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res, ResMut},
    },
};
use common::{comp, comp::inventory::item::tool::AbilityContext, resources::Time as SimTime};
use specs::WorldExt;
use xindeler_protocol::{
    LocalAssignHotbarSlot, NetAbilities, NetAuxiliaryAbility, NetCooldownEntry, NetCooldowns,
    NetHotbarSlot,
};

use crate::{EmbeddedPlayer, SimMirror, SimServer, mirror_sim_entities, tick_sim};

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
/// exist).
fn to_net_aux(ability: comp::ability::AuxiliaryAbility) -> NetAuxiliaryAbility {
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

/// The inverse of [`to_net_aux`] — used by [`apply_local_hotbar_assignment`]
/// to turn a client's rebind request back into the sim's own enum before
/// calling `Client::change_ability`.
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
                let context = AbilityContext::from(stance, inv, combo);

                let resolve = |ability: comp::ability::Ability| -> Option<String> {
                    ability
                        .ability_id(char_state, inv, skill_set, ability_pool, &context)
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

/// Drains [`LocalAssignHotbarSlot`] (the client hotbar's own drag-drop
/// resolution) and applies it through the embedded player's real
/// `client::Client::change_ability` — a genuine client->server network send,
/// never a direct ECS write (isolation-law rule 4). A no-op if no embedded
/// player exists yet (degrade clean — same posture as every other
/// `Option<NonSendMut<EmbeddedPlayer>>` consumer in this crate).
pub fn apply_local_hotbar_assignment(
    mut events: MessageReader<LocalAssignHotbarSlot>,
    player: Option<NonSendMut<EmbeddedPlayer>>,
) {
    let Some(mut player) = player else { return };
    for event in events.read() {
        // `u32 as usize` is a widening (never-truncating) conversion on
        // every supported target — no `cast_possible_truncation` risk.
        player.assign_hotbar_slot(event.slot as usize, from_net_aux(event.ability));
    }
}

/// Registers [`HotbarMirrorCache`] + [`mirror_hotbar_state`] in
/// `FixedUpdate` (after `tick_sim`/`mirror_sim_entities` — exactly
/// `CombatHudMirrorPlugin`'s own ordering, for the same reason: this tick's
/// fresh sim state, this tick's up-to-date `SimMirror` identity map) and
/// [`apply_local_hotbar_assignment`] in `Update` (the embedded player's own
/// pass-through methods are driven from `Update`, matching
/// `PlayerBridgePlugin`'s `tick_player`/chat's `broadcast_embedded_chat`
/// cadence, not the 30 Hz sim schedule).
pub struct HotbarMirrorPlugin;

impl Plugin for HotbarMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HotbarMirrorCache>()
            .add_systems(
                FixedUpdate,
                mirror_hotbar_state
                    .after(tick_sim)
                    .after(mirror_sim_entities),
            )
            .add_systems(Update, apply_local_hotbar_assignment);
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use common::resources::Time;
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
    /// conversion [`apply_local_hotbar_assignment`] relies on to turn a
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
}
