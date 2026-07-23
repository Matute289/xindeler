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
//! ## The write half (BL-82 EM-8.3 — unified `FromClient`, dedicated-server-safe)
//! [`apply_skill_unlock_requests`] is the ONLY system in this module that
//! writes toward the sim. It reads [`xindeler_protocol::UnlockSkillRequest`] as
//! `FromClient<_>` (a genuinely-remote dedicated-server client's real send, OR
//! the listen-server's own local write echoed back with `ClientId::Server`),
//! resolves the acting sim entity per message via
//! [`crate::inventory::resolve_client_entity`] (the SAME real-connection-first,
//! embedded-player-fallback resolution `InventoryMirrorPlugin`/
//! `TradeMirrorPlugin` use), and applies the unlock to that entity's own
//! `comp::SkillSet` — mirroring `server/src/sys/msg/in_game.rs`'s
//! `ClientGeneral::UnlockSkill` handler exactly. Before EM-8.3 this went
//! through an `EmbeddedPlayer::unlock_skill` shortcut that silently did nothing
//! on the real dedicated server (no `EmbeddedPlayer` there) — the ledger's A1
//! parity gap.

use std::collections::HashMap;

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
use common::{comp, uid::Uid};
use specs::WorldExt;
use xindeler_protocol::{
    NetAbilityPool, NetHotbarSlot, NetOwnerOnly, NetSkillGroup, NetSkillSet, UnlockSkillRequest,
};

use crate::{
    EmbeddedPlayer, PlayerDimensionSession, SimMirror, SimServer, hotbar::to_net_aux,
    inventory::resolve_client_entity, mirror_sim_entities, tick_sim,
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
    let buffs_storage = ecs.read_storage::<comp::Buffs>();
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
        let buffs = buffs_storage.get(sim_entity);

        let resolve = |ability: comp::ability::Ability| -> Option<String> {
            ability
                .ability_id(
                    char_state,
                    inv,
                    Some(skill_set),
                    ability_pool,
                    stance,
                    combo,
                    buffs,
                )
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

/// Drains [`UnlockSkillRequest`]s (the client diary's SP-spend action) and
/// applies each to the RESOLVED acting player's own `comp::SkillSet` — see the
/// module doc comment for the full unified-write rationale (BL-82 EM-8.3).
/// Resolves the acting entity PER MESSAGE via [`resolve_client_entity`] (a real
/// dedicated server can have many simultaneously-connected clients, each
/// needing its OWN resolution, not a single bridge-wide fallback), exactly like
/// [`crate::inventory::apply_inventory_action_requests`].
///
/// ## Why this reads via `read_storage` first, not a speculative `write_storage`
/// (bevy-migration-reviewer finding)
/// `comp::SkillSet`'s `Component::Storage` is `DerefFlaggedStorage` — ANY
/// `WriteStorage::get_mut(entity)` call unconditionally marks that entity
/// Modified in the flagged-storage change channel (persistence/replication
/// dirty-tracking), regardless of whether the caller actually mutates the
/// value afterward. The legacy `server/src/sys/msg/in_game.rs`'s
/// `ClientGeneral::UnlockSkill` handler deliberately avoids this: it operates
/// on a `Cow<'_, SkillSet>` built from a `ReadStorage` borrow
/// (`skill_set: &mut Option<Cow<'_, SkillSet>>`, never a `WriteStorage`
/// fetch), and only writes the mutated result back into the REAL ECS storage
/// (`skill_sets.get_mut(entity)`, at the very end of that whole system) for
/// entities whose `Cow` actually became `Cow::Owned` — i.e., only on a
/// genuinely SUCCESSFUL unlock. A rejected attempt (insufficient SP, missing
/// prerequisites, already unlocked, …) never touches `WriteStorage` at all,
/// so it never flags Modified.
///
/// This function reproduces that same two-phase discipline without the
/// legacy system's batching machinery (unnecessary here — one message,
/// one entity, applied immediately): clone the CURRENT `SkillSet` off a
/// `read_storage` borrow (no flagging), attempt `unlock_skill` on the OWNED
/// clone (a plain struct mutation, still no ECS storage involved), and ONLY
/// on `Ok(())` write the mutated clone back via `write_storage().get_mut()`
/// (correctly flags Modified — a real change happened). A stale/illegal
/// spend's `Err` is intentionally swallowed (matching the legacy handler's
/// own `// FIXME: How do we want to handle the error?` posture) and, as of
/// this fix, never touches write storage at all — no spurious
/// replication/persistence-dirty traffic on a rejected request.
pub fn apply_skill_unlock_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSend<EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<UnlockSkillRequest>>,
) {
    let Some(sim) = sim else {
        // No sim booted yet — drop pending requests rather than buffering them
        // forever (degrade clean, spec §3.2), same as every sibling applicator.
        requests.clear();
        return;
    };
    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            // This specific client's identity didn't resolve (still connecting,
            // or a stale/forged id) — skip just this request, not the batch.
            continue;
        };
        let ecs = sim.server.state().ecs();

        // Phase 1: read-only clone, no flagged-storage touch at all.
        let Some(mut candidate) = ecs.read_storage::<comp::SkillSet>().get(entity).cloned() else {
            continue;
        };
        // Phase 2: attempt the unlock on the OWNED clone — still a plain
        // struct mutation, no ECS storage involved.
        if candidate.unlock_skill(message.0).is_err() {
            // Rejected (insufficient SP / missing prereqs / already unlocked /
            // unavailable group) — deliberately swallowed (see this
            // function's doc comment), and critically: no write-storage touch
            // follows, so no Modified flag fires for a no-op request.
            continue;
        }
        // Phase 3: a genuine change happened — write it back, which correctly
        // flags Modified now.
        if let Some(mut skill_set) = ecs.write_storage::<comp::SkillSet>().get_mut(entity) {
            *skill_set = candidate;
        }
    }
}

/// Registers [`SkillSetMirrorCache`] + [`mirror_skillset_state`] +
/// [`apply_skill_unlock_requests`] in `FixedUpdate` (after
/// `tick_sim`/`mirror_sim_entities` — exactly [`crate::inventory::
/// InventoryMirrorPlugin`]'s own ordering, since the applicator now resolves
/// via `PlayerDimensionSession` + writes the sim like that plugin's own
/// applicator, no longer an `Update`-cadence `EmbeddedPlayer` pass-through).
pub struct SkillSetMirrorPlugin;

impl Plugin for SkillSetMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillSetMirrorCache>().add_systems(
            FixedUpdate,
            (mirror_skillset_state, apply_skill_unlock_requests)
                .after(tick_sim)
                .after(mirror_sim_entities),
        );
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

    /// Builds a sim player entity carrying a real `SkillSet` pre-loaded with
    /// exactly the SP `skill` costs (so a subsequent unlock genuinely
    /// succeeds), plus a `Uid` registered in `IdMaps` the same way a real
    /// player-creation path does — the shared fixture both applicator tests
    /// below use.
    fn spawn_player_ready_to_unlock(app: &mut App, skill: Skill) -> specs::Entity {
        use common::uid::IdMaps;

        let mut sim = app.world_mut().non_send_mut::<SimServer>();
        let ecs = sim.server.state_mut().ecs_mut();
        let mut skill_set = comp::SkillSet::default();
        let group = skill
            .skill_group_kind()
            .expect("the chosen skill has a spend group");
        skill_set.add_skill_points(group, skill_set.skill_cost(skill));
        let entity = ecs.create_entity().with(skill_set).build();
        let mut uids = ecs.write_storage::<Uid>();
        let mut id_maps = ecs.write_resource::<IdMaps>();
        uids.insert(entity, id_maps.allocate(entity)).unwrap();
        drop(uids);
        drop(id_maps);
        entity
    }

    /// A skill unlockable from the default skillset once SP is granted:
    /// `Feat(Athlete)` — a `Feats`-group node (the `Feats` group is unlocked in
    /// `SkillSet::default()`) with no prerequisite (only a handful of Feats
    /// carry one, per `assets/common/skill_trees/skill_prerequisites.ron`), so
    /// granting its cost and unlocking it always succeeds.
    fn unlockable_skill() -> Skill {
        use common::comp::skillset::skills::FeatSkill;
        Skill::Feat(FeatSkill::Athlete)
    }

    /// BL-82 EM-8.3 acceptance: a real dedicated-server client's
    /// [`UnlockSkillRequest`] (`FromClient<_>` carrying the sender's own
    /// `ClientId::Client`) unlocks THAT client's own sim `SkillSet`, resolved
    /// via [`PlayerDimensionSession`] — the same real-connection path
    /// `apply_inventory_action_requests` uses. This is the core parity fix: the
    /// old `EmbeddedPlayer::unlock_skill` shortcut silently did nothing on a
    /// dedicated server (no embedded player there).
    #[test]
    fn skill_unlock_request_unlocks_the_resolved_clients_own_skillset() {
        use bevy_replicon::prelude::ClientId;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<UnlockSkillRequest>>();

        let skill = unlockable_skill();
        let sim_entity = spawn_player_ready_to_unlock(&mut app, skill);
        let connection_entity = app
            .world_mut()
            .spawn(PlayerDimensionSession(sim_entity))
            .id();

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(connection_entity),
            message: UnlockSkillRequest(skill),
        });
        app.world_mut()
            .run_system_once(apply_skill_unlock_requests)
            .expect("applicator runs");

        let sim = app.world().non_send::<SimServer>();
        let ecs = sim.server.state().ecs();
        let skill_sets = ecs.read_storage::<comp::SkillSet>();
        assert!(
            skill_sets
                .get(sim_entity)
                .expect("entity still has a SkillSet")
                .has_skill(skill),
            "the sender's own SkillSet must have the skill unlocked after the request"
        );
    }

    /// BL-82 EM-8.3 owner-scoping guard: client A's SP-spend mutates A's OWN
    /// `SkillSet`, never a different connected client B's — the skillset
    /// analogue of `inventory`'s
    /// `resolve_client_entity_uses_the_real_connection_when_present`, proving a
    /// real dedicated server's per-connection resolution keeps each player's
    /// progression private to itself.
    #[test]
    fn skill_unlock_request_is_scoped_to_the_sender_not_another_client() {
        use bevy_replicon::prelude::ClientId;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<UnlockSkillRequest>>();

        let skill = unlockable_skill();
        let entity_a = spawn_player_ready_to_unlock(&mut app, skill);
        let entity_b = spawn_player_ready_to_unlock(&mut app, skill);
        let conn_a = app.world_mut().spawn(PlayerDimensionSession(entity_a)).id();
        // B has its own connection but never sends anything.
        let _conn_b = app.world_mut().spawn(PlayerDimensionSession(entity_b)).id();

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(conn_a),
            message: UnlockSkillRequest(skill),
        });
        app.world_mut()
            .run_system_once(apply_skill_unlock_requests)
            .expect("applicator runs");

        let sim = app.world().non_send::<SimServer>();
        let ecs = sim.server.state().ecs();
        let skill_sets = ecs.read_storage::<comp::SkillSet>();
        assert!(
            skill_sets.get(entity_a).unwrap().has_skill(skill),
            "A (the sender) must have unlocked the skill"
        );
        assert!(
            !skill_sets.get(entity_b).unwrap().has_skill(skill),
            "B (a different client that sent nothing) must be untouched — progression is \
             per-connection, never leaked/misattributed"
        );
    }

    /// BL-82 EM-8.3 follow-up (bevy-migration-reviewer finding): a REJECTED
    /// unlock request (insufficient SP — no SP was granted for this fixture)
    /// must fire ZERO `ComponentEvent::Modified` events on `comp::SkillSet`'s
    /// `DerefFlaggedStorage`-backed change channel. This directly verifies the
    /// bug the reviewer flagged and this function's own doc comment describes:
    /// a pre-fix `WriteStorage::get_mut(entity)` immediately followed by a
    /// `&mut self` method call forces a `DerefMut` on the
    /// `DerefFlaggedStorage`'s `FlaggedAccessMut` wrapper — which
    /// unconditionally fires `Modified` (see `specs`'
    /// `storage::deref_flagged::FlaggedAccessMut::deref_mut`) BEFORE
    /// `unlock_skill_cow`'s internal validation ever runs, regardless of
    /// whether the spend actually succeeds. That would mean every rejected
    /// spend attempt (a client spamming an unaffordable unlock, or a stale/
    /// racy request) spuriously marks the entity's `SkillSet` dirty —
    /// unnecessary replication/persistence-dirty traffic on every no-op. This
    /// test registers a real `ReaderId` on the storage's own event channel
    /// BEFORE running the applicator and asserts the channel is still empty
    /// after a request that cannot possibly succeed.
    #[test]
    fn rejected_unlock_request_does_not_flag_the_skillset_storage_modified() {
        use bevy_replicon::prelude::ClientId;
        use common::uid::IdMaps;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<UnlockSkillRequest>>();

        // A player with a DEFAULT (zero-SP) SkillSet — any real skill unlock
        // request against it is guaranteed to be rejected
        // (`SkillUnlockError::InsufficientSP`).
        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let entity = ecs.create_entity().with(comp::SkillSet::default()).build();
            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<IdMaps>();
            uids.insert(entity, id_maps.allocate(entity)).unwrap();
            entity
        };
        let connection_entity = app
            .world_mut()
            .spawn(PlayerDimensionSession(sim_entity))
            .id();

        let mut reader = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<comp::SkillSet>().register_reader()
        };

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(connection_entity),
            message: UnlockSkillRequest(unlockable_skill()),
        });
        app.world_mut()
            .run_system_once(apply_skill_unlock_requests)
            .expect("applicator runs");

        let sim = app.world().non_send::<SimServer>();
        let ecs = sim.server.state().ecs();
        let skill_sets = ecs.read_storage::<comp::SkillSet>();
        let events: Vec<_> = skill_sets.channel().read(&mut reader).copied().collect();

        assert!(
            !skill_sets
                .get(sim_entity)
                .unwrap()
                .has_skill(unlockable_skill()),
            "sanity: the request must genuinely have been rejected (no SP was granted)"
        );
        assert!(
            events.is_empty(),
            "a rejected unlock request must fire ZERO ComponentEvent::Modified/Inserted/Removed \
             events — got {events:?} (the bug: a speculative write_storage().get_mut() \
             dereference fires Modified even when nothing changes)"
        );
    }

    /// The successful-unlock counterpart: a real, affordable unlock request
    /// DOES fire exactly one `ComponentEvent::Modified` — confirming the fix
    /// doesn't just suppress ALL flagging (which would silently break
    /// replication/persistence for genuine changes), only the spurious
    /// no-op case above.
    #[test]
    fn accepted_unlock_request_flags_the_skillset_storage_modified_exactly_once() {
        use bevy_replicon::prelude::ClientId;
        use specs::storage::ComponentEvent;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<UnlockSkillRequest>>();

        let skill = unlockable_skill();
        let sim_entity = spawn_player_ready_to_unlock(&mut app, skill);
        let connection_entity = app
            .world_mut()
            .spawn(PlayerDimensionSession(sim_entity))
            .id();

        let mut reader = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<comp::SkillSet>().register_reader()
        };

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(connection_entity),
            message: UnlockSkillRequest(skill),
        });
        app.world_mut()
            .run_system_once(apply_skill_unlock_requests)
            .expect("applicator runs");

        let sim = app.world().non_send::<SimServer>();
        let ecs = sim.server.state().ecs();
        let skill_sets = ecs.read_storage::<comp::SkillSet>();
        let events: Vec<_> = skill_sets.channel().read(&mut reader).copied().collect();

        assert_eq!(
            events,
            vec![ComponentEvent::Modified(sim_entity.id())],
            "a genuinely successful unlock must flag Modified exactly once, for the resolved \
             entity — the fix must not suppress flagging for REAL changes"
        );
    }
}
