//! BL-82 EM-5.7 — the character diary / skill-tree mirror + SP-spend
//! applicator (spec §3.2/§6, tasks T56.22-.24).
//!
//! Follows [`crate::hotbar::mirror_hotbar_state`]'s exact shape (which itself
//! follows [`crate::combat_hud::mirror_combat_hud_state`]'s): a separate,
//! additive `FixedUpdate` system reading storages neither of those touch. See
//! `xindeler_protocol::skillset`'s own doc comment for why the STATIC
//! skill-tree shape (prereqs/costs/tiers) is NOT projected here — only the
//! sim's dynamic `SkillSet`/`AbilityPool` state is.
//!
//! ## The write half (isolation-law rule 4: sim writes via public APIs only)
//! [`apply_local_skill_unlock_requests`] is the ONLY system in this module
//! that writes toward the sim, and it does so through
//! [`crate::EmbeddedPlayer::unlock_skill`] — a real client->server network
//! send, never a direct `SkillSet` mutation. Mirrors
//! [`crate::hotbar::apply_local_hotbar_assignment`] precisely.

use std::collections::HashMap;

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
use common::{comp, comp::inventory::item::tool::AbilityContext, uid::Uid};
use specs::WorldExt;
use xindeler_protocol::{
    LocalUnlockSkillRequest, NetAbilityPool, NetHotbarSlot, NetOwnerOnly, NetSkillGroup,
    NetSkillSet,
};

use crate::{
    EmbeddedPlayer, SimMirror, SimServer, hotbar::to_net_aux, mirror_sim_entities, tick_sim,
};

/// Last-mirrored [`NetSkillSet`]/[`NetAbilityPool`] per sim entity — the same
/// dedup shape [`crate::hotbar::HotbarMirrorCache`]/
/// [`crate::inventory::InventoryMirrorCache`] already use (both are
/// `Vec`-shaped, so re-inserting an UNCHANGED value every tick would still
/// force replicon to treat it as mutated). `owner` mirrors
/// `InventoryMirrorCache::owner`'s exact dedup discipline for
/// [`NetOwnerOnly`] (skillset is self-only HUD data, spec §3.2's own
/// example list names it explicitly).
#[derive(Resource, Default, Debug)]
pub struct SkillSetMirrorCache {
    skillset: HashMap<specs::Entity, NetSkillSet>,
    abilities: HashMap<specs::Entity, NetAbilityPool>,
    owner: HashMap<specs::Entity, u64>,
}

/// Reads the sim's `comp::SkillSet`/`comp::ability::AbilityPool` for every
/// currently-mirrored entity and UPSERTs `NetSkillSet`/`NetAbilityPool` (+
/// tags [`NetOwnerOnly`] with the entity's own `Uid`) — mirrors
/// [`crate::hotbar::mirror_hotbar_state`]'s `Some(..) => insert / None =>
/// remove` shape. An entity whose `Uid` lookup fails this tick `continue`s
/// past everything for that entity (matches
/// [`crate::inventory::mirror_inventory_state`]'s own stricter shape — see
/// that function's doc comment for why: leaving a `NetSkillSet` without a
/// matching `NetOwnerOnly` would make it visible to every client, not just
/// its owner).
///
/// A no-op if no [`SimServer`] is booted yet — same early-out every sibling
/// mirror system uses.
pub fn mirror_skillset_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut cache: ResMut<SkillSetMirrorCache>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    cache
        .skillset
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .abilities
        .retain(|entity, _| mirror.0.contains_key(entity));
    cache
        .owner
        .retain(|entity, _| mirror.0.contains_key(entity));

    let ecs = sim.server.state().ecs();
    let skill_sets = ecs.read_storage::<comp::SkillSet>();
    let inventories = ecs.read_storage::<comp::Inventory>();
    let ability_pools = ecs.read_storage::<comp::ability::AbilityPool>();
    let char_states = ecs.read_storage::<comp::CharacterState>();
    let stances = ecs.read_storage::<comp::Stance>();
    let combos = ecs.read_storage::<comp::Combo>();
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

        let Some(skill_set) = skill_sets.get(sim_entity) else {
            ec.remove::<NetSkillSet>();
            ec.remove::<NetAbilityPool>();
            cache.skillset.remove(&sim_entity);
            cache.abilities.remove(&sim_entity);
            continue;
        };

        let groups: Vec<NetSkillGroup> = skill_set
            .skill_groups()
            .map(|sg| NetSkillGroup {
                kind: sg.skill_group_kind,
                available_sp: sg.available_sp,
                earned_sp: sg.earned_sp,
            })
            .collect();
        let skills: Vec<(comp::skillset::skills::Skill, u16)> =
            skill_set.unlocked_skills().collect();
        let net_skillset = NetSkillSet { groups, skills };
        if cache.skillset.get(&sim_entity) != Some(&net_skillset) {
            ec.insert(net_skillset.clone());
            cache.skillset.insert(sim_entity, net_skillset);
        }

        let inv = inventories.get(sim_entity);
        let ability_pool = ability_pools.get(sim_entity);
        let char_state = char_states.get(sim_entity);
        let stance = stances.get(sim_entity);
        let combo = combos.get(sim_entity);
        let context = AbilityContext::from(stance, inv, combo);

        let resolve = |ability: comp::ability::Ability| -> Option<String> {
            ability
                .ability_id(char_state, inv, Some(skill_set), ability_pool, &context)
                .map(str::to_owned)
        };

        let available: Vec<NetHotbarSlot> =
            comp::ActiveAbilities::all_available_abilities(inv, Some(skill_set), ability_pool)
                .into_iter()
                .map(|aux| NetHotbarSlot {
                    aux: to_net_aux(aux),
                    ability_id: resolve(comp::ability::Ability::from(aux)),
                })
                .collect();
        let net_pool = NetAbilityPool(available);
        if cache.abilities.get(&sim_entity) != Some(&net_pool) {
            ec.insert(net_pool.clone());
            cache.abilities.insert(sim_entity, net_pool);
        }
    }
}

/// Drains [`LocalUnlockSkillRequest`] (the client diary's own SP-spend
/// action) and applies it through the embedded player's real
/// [`crate::EmbeddedPlayer::unlock_skill`] — a genuine client->server network
/// send, never a direct ECS write (isolation-law rule 4). A no-op if no
/// embedded player exists yet (degrade clean).
pub fn apply_local_skill_unlock_requests(
    mut events: MessageReader<LocalUnlockSkillRequest>,
    player: Option<NonSendMut<EmbeddedPlayer>>,
) {
    let Some(mut player) = player else { return };
    for event in events.read() {
        player.unlock_skill(event.0);
    }
}

/// Registers [`SkillSetMirrorCache`] + [`mirror_skillset_state`] in
/// `FixedUpdate` (after `tick_sim`/`mirror_sim_entities` — exactly
/// [`crate::hotbar::HotbarMirrorPlugin`]'s own ordering) and
/// [`apply_local_skill_unlock_requests`] in `Update` (the embedded player's
/// own pass-through methods are driven from `Update`, matching every sibling
/// applicator's cadence).
pub struct SkillSetMirrorPlugin;

impl Plugin for SkillSetMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillSetMirrorCache>()
            .add_systems(
                FixedUpdate,
                mirror_skillset_state
                    .after(tick_sim)
                    .after(mirror_sim_entities),
            )
            .add_systems(Update, apply_local_skill_unlock_requests);
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use common::comp::skillset::{SkillGroupKind, skills::Skill};
    use specs::{Builder, WorldExt};
    use xindeler_protocol::{NetAbilityPool, NetSkillSet};

    use super::*;
    use crate::{SimServer, boot_test_server};

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.init_resource::<SkillSetMirrorCache>();
        app.insert_non_send(sim);
        app
    }

    /// `mirror_skillset_state` is a documented no-op when [`SimMirror`] is
    /// empty — the "degrade clean" rule (spec §3.2).
    #[test]
    fn no_mirrored_entities_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(mirror_skillset_state)
            .expect("system runs without a mirrored entity");
    }

    /// A sim entity carrying a real `SkillSet::default()` (General + Pick +
    /// Feats groups unlocked, per that type's own `Default` impl) gets
    /// `NetSkillSet` UPSERTed with the matching group/skill state — the core
    /// T56.22 acceptance bar.
    #[test]
    fn mirrors_default_skillset_groups_and_skills() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let (sim_entity, uid) = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let uid = Uid(std::num::NonZeroU64::new(42).unwrap());
            let entity = ecs
                .create_entity()
                .with(comp::SkillSet::default())
                .with(uid)
                .build();
            (entity, uid)
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_skillset_state)
            .expect("system runs");
        app.update();

        let net = app
            .world()
            .get::<NetSkillSet>(bevy_entity)
            .expect("NetSkillSet must be mirrored");
        assert_eq!(
            net.groups.len(),
            3,
            "General + Pick + Feats, per SkillSet::default"
        );
        assert!(
            net.skills
                .iter()
                .any(|(s, l)| *s == Skill::UnlockGroup(SkillGroupKind::General) && *l == 1)
        );
        assert_eq!(
            app.world().get::<NetOwnerOnly>(bevy_entity).map(|o| o.0),
            Some(uid.0.get()),
            "skillset is self-only HUD data — NetOwnerOnly must be tagged"
        );
    }

    /// An entity that loses its sim-side `SkillSet` has `NetSkillSet`/
    /// `NetAbilityPool` removed too — the `None => ec.remove::<NetX>()` arm.
    #[test]
    fn removing_skillset_removes_the_net_mirrors_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity()
                .with(comp::SkillSet::default())
                .with(Uid(std::num::NonZeroU64::new(7).unwrap()))
                .build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_skillset_state)
            .expect("first run mirrors NetSkillSet");
        app.update();
        assert!(app.world().get::<NetSkillSet>(bevy_entity).is_some());

        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<comp::SkillSet>().remove(sim_entity);
        }

        app.world_mut()
            .run_system_once(mirror_skillset_state)
            .expect("second run must remove the now-stale mirror");
        app.update();
        assert!(
            app.world().get::<NetSkillSet>(bevy_entity).is_none(),
            "NetSkillSet must be removed once the sim-side SkillSet is gone"
        );
        assert!(
            app.world().get::<NetAbilityPool>(bevy_entity).is_none(),
            "NetAbilityPool must be removed alongside it"
        );
    }
}
