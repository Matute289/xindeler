//! BL-82 EM-4.9 follow-up (closing one of its two documented deferred gaps):
//! moves an already-mirrored, REAL connected player between dimensions —
//! the missing piece EM-4.9's own backlog row named verbatim: "no live
//! player-transfer trigger moving a connected player into the event
//! dimension". Everything else EM-4.9 shipped (DmEvent → dimension spinup →
//! NPC-spawn → atmosphere-sync) already works end to end; this module is
//! what makes a PLAYER (not just NPCs) actually experience one.
//!
//! ## Why this needed a NEW mechanism, not just reusing the NPC path
//! [`crate::SimEntityDimension`] (the per-mirrored-entity dimension cache
//! `crate::mirror_sim_entities` consults every tick) used to be decided ONCE,
//! the first tick an entity is seen, and never revisited — its own doc
//! comment said so verbatim ("no player/NPC dimension-TRANSFER path yet; a
//! mirror keeps the dimension it was created with for its whole lifetime").
//! That is fine for an NPC (it is created directly INTO its target dimension
//! by `crate::entity_factory`) but wrong for a player: a connected player
//! already exists, mirrored into [`xindeler_dimensions::DimensionId::DEFAULT`]
//! since the moment it first spawned, long before any `DmEvent` exists. This
//! module is the validated, single choke point that revisits that decision.
//!
//! ## Why a player can't just be "released" like an NPC on teardown
//! `crate::release_dimension_occupants_on_drain_request` clears an NPC's
//! dimension OCCUPANCY bookkeeping on drain so the dimension can reach
//! `Teardown` even though the NPC's mirror entity is still tagged to it —
//! that is correct for an NPC, whose mirror is MEANT to cascade-despawn with
//! the event ([`xindeler_dimensions::DimensionRoot`]'s
//! `#[relationship(linked_spawn)]`). A real player's mirror entity represents
//! a live, persistent session that must SURVIVE the event ending — so
//! [`eject_players_before_dimension_teardown`] instead TRANSFERS it back to
//! `DimensionId::DEFAULT` (retagging `DimensionId`/`DimensionRoot` + occupant
//! bookkeeping), so by the time the cascade-despawn actually runs, the player
//! is no longer a member of the doomed dimension at all.
//!
//! ## Trigger mechanism (v1): proximity, not an explicit "enter" command
//! `xindeler-sim-bridge::oracle`'s `detect_player_dimension_entry` is the
//! producer: it checks every tracked player's own sim position each tick
//! against every currently-`Active` event's trigger zone (the SAME origin
//! `spawn_event_minions` already scatters minions around, radius =
//! `spawning_rules.spawn_radius` — no new `DmEvent` schema field needed) and
//! emits [`TransferPlayerDimension`] the moment a `DEFAULT`-resident player
//! enters one. This was picked over an explicit narrative-hook-driven
//! trigger because it composes directly with machinery EM-4.9 already
//! shipped (the exact origin/radius the minions themselves spawn around) and
//! needs no new authored content — "walk into the mist and the shades are
//! around you" falls out of the SAME zone the minions already occupy. v1 is
//! deliberately a ONE-WAY crossing: a player already inside a dimension is
//! never re-evaluated for re-entry by proximity (see that function's own
//! doc comment) — only [`eject_players_before_dimension_teardown`] moves them
//! back out, when the event itself ends. This avoids boundary-flicker
//! thrashing and matches the narrative framing ("you have crossed into the
//! Mist-Bound") better than a system that could yank a player back and forth
//! near the zone's edge.
//!
//! ## Anti-chaos / isolation law
//! [`apply_player_dimension_transfers`] is the ONLY system that ever mutates
//! [`crate::SimEntityDimension`] after an entity's first sighting, and it
//! validates every request against [`DimensionRegistry`]'s own
//! `accepts_new_entrants` invariant — the identical gate a brand-new mirror
//! (or a factory-spawned NPC) already has to pass. A request naming an
//! unmirrored entity, or a dimension that doesn't exist / isn't accepting
//! entrants, is dropped with a `warn!`, never a panic. This module writes
//! into Bevy-side bookkeeping only (`DimensionRegistry`/`SimEntityDimension`/
//! component tags) — it never touches the sim's specs storages directly,
//! consistent with the bridge-is-read-mostly-into-the-sim isolation rule
//! (there is nothing FOR a dimension transfer to write into the sim itself:
//! the entity's real specs position/components are untouched, only its
//! Bevy-side dimension tag moves).

use std::collections::HashSet;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        component::Component,
        entity::Entity,
        message::{Message, MessageReader, MessageWriter},
        query::With,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res, ResMut},
    },
    log::{info, warn},
};
use xindeler_dimensions::{DimensionId, DimensionRegistry, DimensionRoot, DrainDimension};
use xindeler_protocol::{ClientViewpoint, NetLocalPlayer};

use crate::{SimEntity, SimEntityDimension, SimMirror};

/// Links a connected REPLICON client's own Bevy entity (the entity carrying
/// its [`ClientViewpoint`]) to the sim entity it controls (BL-82 EM-4.9
/// follow-up). Populated by `xindeler-server-app::login`'s
/// `handle_character_data`, the moment a login session reaches
/// `Presence::Character` — the SAME edge that already inserts into
/// `xindeler_protocol::ActiveReplicaSessions` (BL-82 EM-8.2 — relocated there
/// from a private map inside `xindeler-server-app::login` so
/// `xindeler-sim-bridge::social` could read it too), just expressed as a
/// component so [`apply_player_dimension_transfers`] (a DIFFERENT crate) can
/// look it up without a new cross-crate resource dependency in either
/// direction.
///
/// Absent for the listen-server's embedded local player: that path never
/// adds `bevy_replicon`'s CLIENT role at all (`xindeler-client::listen_server`
/// hosts SERVER-role-only, relying on replicon's local loopback message
/// re-emission — see that module's own doc comment), so there is no
/// `ConnectedClient`/`ClientViewpoint` to link there. A transfer of that
/// player's own sim entity still fully retags its mirror's
/// `DimensionId`/`DimensionRoot` and updates occupancy bookkeeping — only the
/// (dormant, for that path) `ClientViewpoint` sync half no-ops, which
/// [`apply_player_dimension_transfers`] treats as "nothing to sync on the
/// client side today", not an error.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerDimensionSession(pub specs::Entity);

/// Requests moving an already-mirrored sim entity into a different dimension
/// (BL-82 EM-4.9 follow-up) — the single validated choke point that revisits
/// [`SimEntityDimension`]'s per-entity decision. See the module doc comment
/// for the three producers that emit this today (the proximity trigger, the
/// teardown ejector, and a debug/admin lever).
#[derive(Message, Debug, Clone, Copy)]
pub struct TransferPlayerDimension {
    pub sim_entity: specs::Entity,
    pub target: DimensionId,
}

/// Applies every [`TransferPlayerDimension`] request queued this tick.
///
/// Atomic per request: the mirror's `DimensionId`/`DimensionRoot` retag, the
/// `SimEntityDimension` cache update, the `DimensionRegistry` occupant
/// bookkeeping (decrement the old dimension, increment the new one), and any
/// linked [`ClientViewpoint`]'s dimension all happen in this ONE system, ONE
/// tick — there is no intermediate tick where one half is updated and the
/// other stale (which would otherwise trip
/// `xindeler_dimensions::registry::sweep_isolation`'s own invariant check).
///
/// Rejections are logged, never panics (same anti-chaos posture every other
/// dimension-lifecycle entry point in this codebase documents):
/// - the sim entity isn't currently mirrored (e.g. it disconnected/died the
///   same tick the request was queued) — nothing to move;
/// - the target dimension doesn't exist, or isn't currently accepting new
///   entrants (`DimensionState::accepts_new_entrants` — the SAME invariant
///   `DimensionRegistry::try_add_occupant` already enforces for a brand-new
///   mirror; a transfer is "joining" the target exactly as much as a fresh
///   spawn is, INCLUDING a transfer back to `DimensionId::DEFAULT` itself);
/// - the request is a no-op (already in the target dimension).
///
/// ## Ordering — load-bearing, not cosmetic
/// Registered (see [`PlayerTransferPlugin`]) `.before(crate::
/// mirror_sim_entities)` so the very same tick's mirror pass sees the FRESH
/// `SimEntityDimension` entry (region key reflects the new dimension
/// immediately, not one tick late), and `.before(crate::
/// release_dimension_occupants_on_drain_request)` /
/// `.before(crate::delete_specs_entities_for_torn_down_dimensions)` /
/// `.before(xindeler_dimensions::teardown::teardown_completed_dimensions)` —
/// an ejected player's retag must be FLUSHED (Bevy auto-inserts the
/// `apply_deferred` sync point this explicit ordering chain requires) before
/// those downstream systems ever see the doomed dimension's tag still on
/// that entity, or they would destroy the player's own presence right along
/// with the event's minions in the very same tick (a dimension whose only
/// occupant was the just-ejected player advances `Draining -> Teardown`
/// immediately, per `DimensionRegistry::remove_occupant`'s own documented
/// auto-advance rule — so this is a real same-tick race, not a theoretical
/// one).
pub fn apply_player_dimension_transfers(
    mut requests: MessageReader<TransferPlayerDimension>,
    mirror: Res<SimMirror>,
    mut registry: ResMut<DimensionRegistry>,
    mut entity_dims: ResMut<SimEntityDimension>,
    sessions: Query<(Entity, &PlayerDimensionSession)>,
    mut viewpoints: Query<&mut ClientViewpoint>,
    mut commands: Commands,
) {
    for &TransferPlayerDimension { sim_entity, target } in requests.read() {
        let Some(&bevy_entity) = mirror.0.get(&sim_entity) else {
            warn!(
                ?sim_entity,
                target = target.0,
                "player-dimension-transfer requested for an unmirrored sim entity; ignoring"
            );
            continue;
        };

        let current = entity_dims
            .0
            .get(&sim_entity)
            .copied()
            .unwrap_or(DimensionId::DEFAULT);
        if current == target {
            continue; // already there — a harmless no-op, not an error
        }

        let Some(new_root) = registry
            .get(target)
            .filter(|state| state.accepts_new_entrants())
            .map(|state| state.root())
        else {
            warn!(
                ?sim_entity,
                target = target.0,
                "player-dimension-transfer target dimension doesn't exist or isn't accepting new \
                 entrants; ignoring"
            );
            continue;
        };

        let _ = registry.remove_occupant(current, bevy_entity);
        if let Err(err) = registry.try_add_occupant(target, bevy_entity) {
            warn!(
                ?err,
                ?sim_entity,
                target = target.0,
                "player-dimension-transfer target rejected the occupant (raced into Draining?); \
                 rolling back the occupancy decrement"
            );
            // BL-82 EM-4.9 follow-up (bevy-migration-reviewer MAJOR finding):
            // the rollback itself can fail too (e.g. `current` ALSO started
            // Draining this same tick) — if it does, `bevy_entity` stays
            // tagged `current` (its `DimensionId`/`DimensionRoot` components
            // are untouched, since we bail out before ever writing them) but
            // is no longer counted as `current`'s occupant, silently risking
            // a premature `Draining -> Teardown` auto-advance for a
            // dimension that still has a live, tagged entity. Logged (not
            // silently swallowed) so this dual-failure edge case is at least
            // observable, even though there is no further recovery action to
            // take here — the entity's tag is still consistent with
            // `current`, only the registry's occupant COUNT would be wrong.
            if let Err(rollback_err) = registry.try_add_occupant(current, bevy_entity) {
                warn!(
                    ?rollback_err,
                    ?sim_entity,
                    current = current.0,
                    "player-dimension-transfer rollback ALSO failed; the entity remains tagged to \
                     its original dimension but is no longer counted as one of its occupants — \
                     that dimension's occupant count may now undercount by one"
                );
            }
            continue;
        }

        entity_dims.0.insert(sim_entity, target);
        commands
            .entity(bevy_entity)
            .insert((target, DimensionRoot(new_root)));

        let client_entity = sessions
            .iter()
            .find(|(_, session)| session.0 == sim_entity)
            .map(|(entity, _)| entity);
        if let Some(client_entity) = client_entity
            && let Ok(mut viewpoint) = viewpoints.get_mut(client_entity)
        {
            viewpoint.dimension = target;
        }

        info!(
            ?sim_entity,
            from = current.0,
            to = target.0,
            "player transferred to a new dimension"
        );
    }
}

/// The player-analogue of `crate::release_dimension_occupants_on_drain_
/// request` (see the module doc comment's "why a player can't just be
/// released like an NPC" section for the full reasoning this complements,
/// not replaces).
///
/// Reads the SAME [`DrainDimension`] admin-command message
/// `release_dimension_occupants_on_drain_request` does, and emits a
/// [`TransferPlayerDimension`] back to `DimensionId::DEFAULT` for every
/// currently-mirrored occupant of the draining dimension that is a REAL
/// PLAYER — identified as either the embedded listen-server local player
/// ([`NetLocalPlayer`]) or a real replicon session ([`PlayerDimensionSession`]
/// linked by its sim entity). Everything else (event minions — nothing else
/// is ever mirrored into a non-default dimension today) is left alone:
/// `release_dimension_occupants_on_drain_request` clears its occupancy
/// bookkeeping and the eventual `teardown_completed_dimensions`
/// cascade-despawn removes it, exactly as designed before this task.
///
/// `DimensionId::DEFAULT` is never itself a source dimension here — a
/// `DrainDimension` targeting `DEFAULT` is already rejected upstream at
/// `xindeler_dimensions::spinup::handle_drain_requests`, so this is
/// defense-in-depth, not load-bearing, mirroring every sibling drain-adjacent
/// system's own exclusion of it.
///
/// `.after(handle_drain_requests)` (this tick's `Active -> Draining`
/// transition, if any, has already happened) and `.before(
/// apply_player_dimension_transfers)` (so the ejection this system requests
/// is fully APPLIED — mirror retag + occupant bookkeeping flushed — before
/// the sibling teardown systems downstream ever run this same tick; see that
/// function's own doc comment for the full ordering chain this depends on).
pub(crate) fn eject_players_before_dimension_teardown(
    mut requests: MessageReader<DrainDimension>,
    mirrors: Query<(&SimEntity, &DimensionId)>,
    local_player: Query<&SimEntity, With<NetLocalPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut transfer_writer: MessageWriter<TransferPlayerDimension>,
) {
    for &DrainDimension(dimension) in requests.read() {
        if dimension == DimensionId::DEFAULT {
            continue;
        }

        let session_sim_entities: HashSet<specs::Entity> =
            sessions.iter().map(|session| session.0).collect();
        let local_player_entity = local_player.single().ok().map(|sim_entity| sim_entity.0);

        for (sim_entity, tagged_dimension) in &mirrors {
            if *tagged_dimension != dimension {
                continue;
            }
            let is_player = Some(sim_entity.0) == local_player_entity
                || session_sim_entities.contains(&sim_entity.0);
            if !is_player {
                continue;
            }
            info!(
                ?sim_entity,
                dimension = dimension.0,
                "ejecting a real player back to DimensionId::DEFAULT before this dimension tears \
                 down"
            );
            transfer_writer.write(TransferPlayerDimension {
                sim_entity: sim_entity.0,
                target: DimensionId::DEFAULT,
            });
        }
    }
}

/// Registers the [`TransferPlayerDimension`] message and the two generic
/// (ORACLE-agnostic) systems in this module. The ORACLE-specific PROXIMITY
/// TRIGGER (`xindeler-sim-bridge::oracle::detect_player_dimension_entry`) is
/// registered separately by `ServerOraclePlugin`, which depends on this
/// plugin having already run (see that system's own ordering).
///
/// Add anywhere [`crate::SimEntityMirrorPlugin`] is added — both the
/// dedicated-server shell (`xindeler-server-app::plugin::SimServerPlugin`)
/// and the listen-server client (`xindeler-client::listen_server`) add this
/// alongside it, so a debug-triggered dimension (not just an ORACLE one) also
/// gets correct player-eject-on-teardown behavior.
pub struct PlayerTransferPlugin;

impl Plugin for PlayerTransferPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<TransferPlayerDimension>();
        // `init_resource` only INSERTS if missing (never overwrites) — this
        // plugin depends on both existing regardless of whether
        // `crate::SimBridgePlugin` happens to already be present (e.g. a
        // sim-less Style-B test that only needs THIS plugin's mechanism, like
        // `oracle::tests::ingest_spinup_spawn_and_hooks_chain_without_a_real_
        // sim`), so it is self-sufficient rather than assuming add-order.
        app.init_resource::<crate::SimMirror>();
        app.init_resource::<crate::SimEntityDimension>();
        app.add_systems(
            FixedUpdate,
            eject_players_before_dimension_teardown
                .after(xindeler_dimensions::spinup::handle_drain_requests)
                .before(apply_player_dimension_transfers),
        );
        app.add_systems(
            FixedUpdate,
            apply_player_dimension_transfers
                .after(crate::tick_sim)
                .after(xindeler_dimensions::spinup::handle_spinup_requests)
                .after(xindeler_dimensions::spinup::handle_drain_requests)
                .before(crate::mirror_sim_entities)
                .before(crate::release_dimension_occupants_on_drain_request)
                .before(crate::delete_specs_entities_for_torn_down_dimensions)
                .before(xindeler_dimensions::teardown::teardown_completed_dimensions),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{MinimalPlugins, app::PluginGroup};
    use specs::{Builder, WorldExt};
    use xindeler_dimensions::DimensionsPlugin;

    use super::*;
    use crate::{SimEntity, SimMirror};

    /// Boots a headless `App` with `DimensionsPlugin` +
    /// [`PlayerTransferPlugin`] — no real sim/world needed (the mechanism
    /// operates purely on Bevy-side bookkeeping + a fake `specs::Entity`,
    /// which `specs::World` happily mints without any component
    /// registration).
    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);
        app.add_plugins(PlayerTransferPlugin);
        // Normally initialized by `crate::SimBridgePlugin`/
        // `crate::SimEntityMirrorPlugin` — not added here (this test suite
        // deliberately avoids booting a real sim), so these tests init them
        // directly.
        app.init_resource::<SimMirror>();
        app.init_resource::<SimEntityDimension>();
        app
    }

    /// Registers a fresh `Active` dimension (via the same two-call sequence
    /// `xindeler_dimensions::registry`'s own tests use) and returns its id +
    /// root entity.
    fn spin_up_active_dimension(app: &mut App, id: u64) -> (DimensionId, Entity) {
        let dimension = DimensionId(id);
        let root = app.world_mut().spawn(dimension).id();
        {
            let mut registry = app
                .world_mut()
                .resource_mut::<xindeler_dimensions::DimensionRegistry>();
            registry.insert_spinning_up(dimension, root, 0).unwrap();
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(dimension, std::sync::Arc::new(world), index)
                .unwrap();
        }
        (dimension, root)
    }

    /// Spawns a mirror entity tagged `dimension`, registers it as an
    /// occupant, and returns its fake `specs::Entity` + Bevy `Entity`.
    fn spawn_mirror(
        app: &mut App,
        specs_world: &mut specs::World,
        dimension: DimensionId,
        root: Entity,
    ) -> (specs::Entity, Entity) {
        let sim_entity = specs::WorldExt::create_entity(specs_world).build();
        let bevy_entity = app
            .world_mut()
            .spawn((SimEntity(sim_entity), dimension, DimensionRoot(root)))
            .id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);
        app.world_mut()
            .resource_mut::<SimEntityDimension>()
            .0
            .insert(sim_entity, dimension);
        app.world_mut()
            .resource_mut::<xindeler_dimensions::DimensionRegistry>()
            .try_add_occupant(dimension, bevy_entity)
            .expect("Active dimension accepts the occupant");
        (sim_entity, bevy_entity)
    }

    #[test]
    fn transfer_moves_a_mirrored_entity_between_active_dimensions_and_updates_occupancy_and_tags() {
        let mut app = new_app();
        let (default_id, default_root) = spin_up_active_dimension(&mut app, 0);
        let (event_id, event_root) = spin_up_active_dimension(&mut app, 1);
        let mut specs_world = specs::World::new();
        let (sim_entity, bevy_entity) =
            spawn_mirror(&mut app, &mut specs_world, default_id, default_root);

        app.world_mut().write_message(TransferPlayerDimension {
            sim_entity,
            target: event_id,
        });
        app.world_mut().run_schedule(bevy::app::FixedUpdate);

        assert_eq!(
            app.world()
                .resource::<SimEntityDimension>()
                .0
                .get(&sim_entity)
                .copied(),
            Some(event_id)
        );
        assert_eq!(
            *app.world().get::<DimensionId>(bevy_entity).unwrap(),
            event_id
        );
        assert_eq!(
            app.world().get::<DimensionRoot>(bevy_entity).unwrap().0,
            event_root
        );
        assert_eq!(
            app.world()
                .resource::<xindeler_dimensions::DimensionRegistry>()
                .get(default_id)
                .unwrap()
                .occupant_count(),
            0,
            "the old dimension must lose the occupant"
        );
        assert_eq!(
            app.world()
                .resource::<xindeler_dimensions::DimensionRegistry>()
                .get(event_id)
                .unwrap()
                .occupant_count(),
            1,
            "the new dimension must gain the occupant"
        );
    }

    #[test]
    fn transfer_rejects_a_target_dimension_that_does_not_exist() {
        let mut app = new_app();
        let (default_id, default_root) = spin_up_active_dimension(&mut app, 0);
        let mut specs_world = specs::World::new();
        let (sim_entity, bevy_entity) =
            spawn_mirror(&mut app, &mut specs_world, default_id, default_root);

        app.world_mut().write_message(TransferPlayerDimension {
            sim_entity,
            target: DimensionId(999),
        });
        app.world_mut().run_schedule(bevy::app::FixedUpdate);

        assert_eq!(
            app.world()
                .resource::<SimEntityDimension>()
                .0
                .get(&sim_entity)
                .copied(),
            Some(default_id),
            "an unknown target dimension must leave the entity untouched"
        );
        assert_eq!(
            *app.world().get::<DimensionId>(bevy_entity).unwrap(),
            default_id
        );
        assert_eq!(
            app.world()
                .resource::<xindeler_dimensions::DimensionRegistry>()
                .get(default_id)
                .unwrap()
                .occupant_count(),
            1,
            "a rejected transfer must not touch occupancy either"
        );
    }

    #[test]
    fn transfer_rejects_a_target_dimension_that_is_draining() {
        let mut app = new_app();
        let (default_id, default_root) = spin_up_active_dimension(&mut app, 0);
        let (draining_id, _draining_root) = spin_up_active_dimension(&mut app, 1);
        {
            let mut registry = app
                .world_mut()
                .resource_mut::<xindeler_dimensions::DimensionRegistry>();
            // Give it a dummy occupant first so `begin_draining` doesn't
            // instant-teardown it (this test only cares that a Draining
            // dimension rejects a NEW entrant).
            let dummy = Entity::from_raw_u32(9001).unwrap();
            registry.try_add_occupant(draining_id, dummy).unwrap();
            registry.begin_draining(draining_id).unwrap();
        }
        let mut specs_world = specs::World::new();
        let (sim_entity, _bevy_entity) =
            spawn_mirror(&mut app, &mut specs_world, default_id, default_root);

        app.world_mut().write_message(TransferPlayerDimension {
            sim_entity,
            target: draining_id,
        });
        app.world_mut().run_schedule(bevy::app::FixedUpdate);

        assert_eq!(
            app.world()
                .resource::<SimEntityDimension>()
                .0
                .get(&sim_entity)
                .copied(),
            Some(default_id),
            "a Draining target must reject the transfer, same as a fresh mirror spawn would"
        );
    }

    #[test]
    fn transfer_is_a_no_op_when_already_in_the_target_dimension() {
        let mut app = new_app();
        let (default_id, default_root) = spin_up_active_dimension(&mut app, 0);
        let mut specs_world = specs::World::new();
        let (sim_entity, _bevy_entity) =
            spawn_mirror(&mut app, &mut specs_world, default_id, default_root);

        app.world_mut().write_message(TransferPlayerDimension {
            sim_entity,
            target: default_id,
        });
        app.world_mut().run_schedule(bevy::app::FixedUpdate);

        assert_eq!(
            app.world()
                .resource::<xindeler_dimensions::DimensionRegistry>()
                .get(default_id)
                .unwrap()
                .occupant_count(),
            1,
            "a same-dimension transfer must not double-count or otherwise disturb occupancy"
        );
    }

    #[test]
    fn transfer_syncs_a_linked_replicon_sessions_client_viewpoint_dimension() {
        let mut app = new_app();
        let (default_id, default_root) = spin_up_active_dimension(&mut app, 0);
        let (event_id, _event_root) = spin_up_active_dimension(&mut app, 1);
        let mut specs_world = specs::World::new();
        let (sim_entity, _bevy_entity) =
            spawn_mirror(&mut app, &mut specs_world, default_id, default_root);

        let client_entity = app
            .world_mut()
            .spawn((
                PlayerDimensionSession(sim_entity),
                ClientViewpoint::new(default_id, vek::Vec2::new(0.0, 0.0), 4),
            ))
            .id();

        app.world_mut().write_message(TransferPlayerDimension {
            sim_entity,
            target: event_id,
        });
        app.world_mut().run_schedule(bevy::app::FixedUpdate);

        assert_eq!(
            app.world()
                .get::<ClientViewpoint>(client_entity)
                .unwrap()
                .dimension,
            event_id,
            "the linked session's ClientViewpoint must follow the player's own transfer"
        );
    }

    #[test]
    fn eject_transfers_the_local_player_and_a_replicon_session_but_leaves_a_plain_npc_tagged() {
        let mut app = new_app();
        let (_default_id, _default_root) = spin_up_active_dimension(&mut app, 0);
        let (event_id, event_root) = spin_up_active_dimension(&mut app, 1);
        let mut specs_world = specs::World::new();

        let (local_player_sim, local_player_bevy) =
            spawn_mirror(&mut app, &mut specs_world, event_id, event_root);
        app.world_mut()
            .entity_mut(local_player_bevy)
            .insert(NetLocalPlayer);

        let (session_sim, _session_bevy) =
            spawn_mirror(&mut app, &mut specs_world, event_id, event_root);
        app.world_mut().spawn(PlayerDimensionSession(session_sim));

        let (npc_sim, npc_bevy) = spawn_mirror(&mut app, &mut specs_world, event_id, event_root);

        app.world_mut().write_message(DrainDimension(event_id));
        app.world_mut().run_schedule(bevy::app::FixedUpdate);

        assert_eq!(
            app.world()
                .resource::<SimEntityDimension>()
                .0
                .get(&local_player_sim)
                .copied(),
            Some(DimensionId::DEFAULT),
            "the embedded local player must be ejected back to DEFAULT"
        );
        assert_eq!(
            app.world()
                .resource::<SimEntityDimension>()
                .0
                .get(&session_sim)
                .copied(),
            Some(DimensionId::DEFAULT),
            "a real replicon session's player must be ejected back to DEFAULT too"
        );
        assert_eq!(
            *app.world().get::<DimensionId>(npc_bevy).unwrap(),
            event_id,
            "a plain NPC mirror must be left tagged to the draining dimension — it is meant to \
             cascade-despawn with it, unlike a real player"
        );
        let _ = npc_sim;
    }

    /// The REAL acceptance test (BL-82 EM-4.9 follow-up): boots a REAL sim +
    /// a REAL embedded local player (the same "real connected player" bar
    /// `crate::player`'s own `embedded_player_moves_with_input`/
    /// `jump_edge_raises_player_z` tests use), spins up a second real
    /// dimension (a fast `World::empty()` registration — this test is about
    /// the TRANSFER mechanism, not worldgen, which `oracle::tests`/
    /// `xindeler-server-app`'s E2E drills already cover separately), and
    /// drives the player through a full transfer-in → transfer-out cycle,
    /// proving the exact deliverable end to end:
    /// - the player's own mirror entity's `DimensionId`/`DimensionRoot`
    ///   genuinely flip when transferred;
    /// - `DimensionRegistry` occupancy correctly moves with it;
    /// - retiring the dimension while the player is still inside ejects them
    ///   BACK to `DimensionId::DEFAULT` instead of cascade-despawning their
    ///   mirror/specs entity along with the (fake, in this test) event content;
    /// - the player's own sim entity, and the whole app, keep ticking correctly
    ///   for many more ticks afterward — no corruption, no panic, no stuck
    ///   lifecycle.
    #[test]
    #[ignore = "boots a real world + embedded player: needs assets + LFS; run with XINDELER_ASSETS"]
    fn real_embedded_player_transfers_into_a_dimension_and_is_ejected_cleanly_on_teardown() {
        use std::time::Duration;

        use bevy::{
            app::PluginGroup as _,
            state::app::StatesPlugin,
            time::{Fixed, Time, TimeUpdateStrategy},
        };
        use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
        use xindeler_protocol::XindelerProtocolPlugin;

        use crate::{
            PlayerBridgePlugin, SimBridgePlugin, SimEntityMirrorPlugin, SimServer,
            boot_embedded_player, boot_test_server, player,
        };

        const MAX_SETTLE_TICKS: u32 = 6000;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut sim = boot_test_server(data_dir.path()).expect("failed to boot test server");
        let player = boot_embedded_player(&mut sim).expect("failed to boot embedded player");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
                PlayerBridgePlugin,
                PlayerTransferPlugin,
            ))
            .finish();
        app.insert_resource(Time::<Fixed>::from_hz(crate::SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / crate::SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);
        app.insert_non_send(player);

        // Settle: wait for the embedded player to reach in-game (a real
        // `Pos` on its sim entity).
        let mut sim_entity = None;
        for _ in 0..MAX_SETTLE_TICKS {
            app.update();
            let p = app.world().non_send::<crate::EmbeddedPlayer>();
            if let Some(uid) = p.uid() {
                let sim = app.world().non_send::<SimServer>();
                if let Some(e) = player::player_sim_entity(sim, uid) {
                    sim_entity = Some(e);
                    break;
                }
            }
        }
        let sim_entity = sim_entity.expect("embedded player never reached in-game");
        println!("[test] embedded player in-game, sim entity {sim_entity:?}");

        // Register a SECOND dimension directly (fast, no real worldgen — the
        // ORACLE ingestion chain that WOULD do this for real is already
        // covered by `oracle::tests`/`e2e_mist_bound_drill.rs`; this test is
        // about the transfer mechanism, not spinup).
        let event_id = DimensionId(1);
        {
            let root = app.world_mut().spawn(event_id).id();
            let mut registry = app
                .world_mut()
                .resource_mut::<xindeler_dimensions::DimensionRegistry>();
            registry
                .insert_spinning_up(event_id, root, 0)
                .expect("dimension 1 not already registered");
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(event_id, std::sync::Arc::new(world), index)
                .expect("Spinup -> Active");
        }

        // Find the player's OWN mirror entity (tagged NetLocalPlayer).
        let mirror_entity = *app
            .world_mut()
            .resource::<SimMirror>()
            .0
            .get(&sim_entity)
            .expect("the embedded player's sim entity must already be mirrored");

        // Note: this test does NOT assert DEFAULT's exact occupant count
        // before/after the transfer — a real booted world's background
        // population (wandering test NPCs, rtsim wildlife spawning/dying as
        // chunks load) genuinely fluctuates tick to tick, which made an
        // exact-delta assertion here flaky (verified empirically: it grew by
        // dozens over just ~90 ticks). The event dimension's own count below
        // IS exact (nothing else is ever routed into a brand-new, otherwise
        // empty dimension in this test) — that, plus the `DimensionId`
        // component checks, are what actually prove the mechanism; the fast,
        // population-free unit tests earlier in this module already pin the
        // EXACT DEFAULT-side occupant bookkeeping this system performs.
        //
        // ---- transfer in ----
        app.world_mut().write_message(TransferPlayerDimension {
            sim_entity,
            target: event_id,
        });
        for _ in 0..30 {
            app.update();
        }
        assert_eq!(
            *app.world().get::<DimensionId>(mirror_entity).unwrap(),
            event_id,
            "the real embedded player's mirror entity must have been retagged to the event \
             dimension"
        );
        assert_eq!(
            app.world()
                .resource::<xindeler_dimensions::DimensionRegistry>()
                .get(event_id)
                .unwrap()
                .occupant_count(),
            1,
            "the event dimension must count the transferred player as an occupant (and nothing \
             else — no NPC was ever routed there)"
        );
        println!("[test] player transferred into the event dimension");

        // ---- retire the event while the player is still inside it ----
        app.world_mut().write_message(DrainDimension(event_id));
        for _ in 0..60 {
            app.update();
        }

        assert_eq!(
            *app.world().get::<DimensionId>(mirror_entity).unwrap(),
            DimensionId::DEFAULT,
            "the player must have been ejected back to DEFAULT, not left tagged to (or destroyed \
             along with) the torn-down dimension"
        );
        assert!(
            app.world().get_entity(mirror_entity).is_ok(),
            "the player's own mirror entity must NOT have been cascade-despawned by the event \
             dimension's teardown"
        );
        assert!(
            !app.world()
                .resource::<xindeler_dimensions::DimensionRegistry>()
                .contains(event_id),
            "the event dimension must have actually reached Teardown and been GC'd"
        );
        println!("[test] player ejected back to DEFAULT; event dimension torn down cleanly");

        // ---- the app (and the player's own session) must keep working
        // fine afterward — no corruption, no panic, no stuck state ----
        for _ in 0..30 {
            app.update();
        }
        let position_after = app.world().non_send::<crate::EmbeddedPlayer>().position();
        assert!(
            position_after.is_some(),
            "the embedded player must still be alive and controllable after the whole transfer + \
             eject-on-teardown cycle"
        );
    }
}
