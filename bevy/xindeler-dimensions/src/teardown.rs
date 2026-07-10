//! BL-82 EM-4.6 (T47.8): the `Teardown` lifecycle state's actual GC PAYLOAD —
//! EM-4.5 (previous task) built the state machine and got a dimension
//! correctly INTO `Teardown`; this module is what happens once it SITS
//! there (migration spec §5.3, detailed in
//! `2026-07-10-bl82-phase4-remaining-plan.md` §1.9, task board
//! `47-bl82-phase4-remaining-tasks.md` T47.8).
//!
//! ## The mechanism (RAM + VRAM)
//! [`teardown_completed_dimensions`] despawns the dimension's `DimensionRoot`
//! entity. Bevy relationships (`#[relationship_target(linked_spawn)]`, see
//! `crate::component`) cascade that despawn through every entity still
//! holding a `DimensionRoot` pointing at it — no hand-rolled traversal.
//! Whatever components those descendants carry (mesh/material/texture
//! `Handle<T>`s, for a CLIENT's render-side dimension members) drop with
//! them; `Assets<T>`'s own ref-counting frees the underlying asset once its
//! last strong `Handle` drops (proven generically, without needing a real
//! render app, in `tests/teardown_asset_gc.rs`). The dimension's own
//! `DimensionState` (chunk store + generated `world`/`index`) is removed from
//! [`crate::registry::DimensionRegistry`] via
//! [`crate::registry::DimensionRegistry::remove_torn_down`] and dropped
//! whole — ordinary `Drop`, no manual cleanup.
//!
//! ## Sim-side (specs entities)
//! This module does NOT touch specs at all — `xindeler-dimensions` has no
//! entity-level view into the sim's mirrored occupants (that's
//! `xindeler-sim-bridge`'s `SimEntity`/`SimMirror` bookkeeping, a different
//! crate). The bridge's own teardown-adjacent system
//! (`xindeler_sim_bridge::delete_specs_entities_for_torn_down_dimensions`)
//! is ordered `.before(teardown_completed_dimensions)` specifically so it can
//! still read the about-to-be-cascade-despawned mirror entities'
//! `SimEntity`/`DimensionId` tags and delete the corresponding specs entities
//! through the sim's OWN normal delete path
//! (`server::StateExt::delete_entity_recorded` — never a raw storage poke,
//! isolation law rule 4) before this module's despawn removes the tags it
//! needs to find them.
//!
//! ## The always-on default dimension is NEVER actually torn down
//! [`DimensionId::DEFAULT`] is the persistent, always-present single game
//! world — despawning ITS root would mean cascade-despawning literally every
//! currently-mirrored entity in the live game. Nothing in EM-4.5's state
//! machine forbids `DimensionId::DEFAULT` from reaching `Draining`/`Teardown`
//! (a `DrainDimension(DimensionId::DEFAULT)` admin command issued before any
//! player has connected — e.g. right at boot, when occupant bookkeeping is
//! still empty — would legally transition it straight through per
//! `DimensionRegistry::begin_draining`'s own "already-empty dimension tears
//! down immediately" rule). Since EM-4.5 only ever LABELLED `Teardown` and
//! this task is the first code that actually ACTS on that label, refusing to
//! run the real GC payload against `DimensionId::DEFAULT` is a necessary
//! safety backstop THIS task must add — not a change to EM-4.5's own state
//! machine (which still, correctly, allows the transition; this module just
//! never carries out the destructive half of it for that one id). Flagged in
//! the PR description as a pre-existing latent gap in EM-4.5's own admin-
//! command path, not fixed here (out of this task's scope to touch
//! `begin_draining` itself).

use bevy::prelude::*;
use tracing::{error, info};

use crate::{
    component::DimensionId,
    registry::{DimensionError, DimensionRegistry, DimensionState},
};

/// Fired exactly once, synchronously, at the moment
/// [`teardown_completed_dimensions`] actually removes a dimension's registry
/// entry (right after the BL-16 chronicle hook runs, right before the root
/// despawn is queued) — the durable "a dimension was genuinely torn down"
/// signal.
///
/// ## Why this exists (found verifying the BL-82 phase-4 wave-3 integration)
/// A `/metrics` consumer (`xindeler-server-app`'s `update_dimension_metrics`)
/// cannot reliably observe "currently in `Teardown`" as a snapshot gauge:
/// this very function removes that entry in the SAME `Update` pass it
/// discovers it (see this module's own doc comment on the "immediate GC"
/// posture), so an external scrape landing between two ticks can never
/// durably catch it — and, a subtler gap an `ecs-design-reviewer` pass caught
/// in an earlier fix here that instead diffed `DimensionRegistry::ids()`
/// between ticks, a dimension whose ENTIRE `Spinup -> Active -> Draining ->
/// Teardown` lifecycle happens to complete within a single `Update` pass
/// would never appear in a "previously known" snapshot at all, so a
/// diff-based detector could silently miss it. A message fired
/// unconditionally at the removal site has no such window: it exists exactly
/// once per real teardown, independent of how many frames the lifecycle
/// actually took. Any interested system (a metrics counter today; BL-16's
/// chronicle system, tomorrow) drains it with a plain `MessageReader`.
#[derive(Message, Debug, Clone, Copy)]
pub struct DimensionTornDown {
    pub id: DimensionId,
    pub occupant_count: usize,
}

/// BL-16 chronicle-system hook point (documented no-op — this task's scope
/// boundary explicitly excludes real chronicle LOGIC, only the seam). Called
/// exactly once per dimension, synchronously, AFTER it's confirmed
/// `Teardown` and BEFORE [`teardown_completed_dimensions`] despawns its root
/// / drops its `DimensionState` — the "extract persistent side effects
/// (loot kept by players, chronicle entries) before teardown" step spec
/// §1.9 calls for, per the ORACLE chronicle contract.
///
/// Today this is a genuine no-op: it reads nothing and writes nothing. When
/// BL-16 lands, this is the call site it replaces/extends (exactly the same
/// seam pattern `xindeler-oracle-host`'s AI-gateway config used for
/// BL-83/BL-85 — see that crate's own doc comment) — the CALLER
/// ([`teardown_completed_dimensions`]) does not need to change, only this
/// function's body does.
pub fn extract_persistent_side_effects_before_teardown(id: DimensionId, state: &DimensionState) {
    // Deliberately does nothing yet. BL-16's chronicle system will read
    // `state` here (occupants, loot, notable events) and persist whatever
    // needs to survive the dimension's destruction. `state` is still fully
    // intact at this point (not yet removed/dropped by the caller).
    let _ = (id, state);
}

/// Runs every `Update`, finds every dimension currently sitting in
/// [`crate::lifecycle::DimensionLifecycle::Teardown`], and actually tears it
/// down: calls the BL-16 hook, despawns the `DimensionRoot` (cascading to
/// every descendant via Bevy relationships), and removes the dimension's
/// entry from the registry (dropping its chunk store/world/index whole).
///
/// Ordered LAST in [`crate::plugin::DimensionsPlugin`]'s chain, after the
/// isolation `debug_assert` sweep has already validated the PREVIOUS frame's
/// fully-settled state (see that plugin's own doc comment for why the sweep
/// was moved to the FRONT of the chain, not because this system is unsafe to
/// run near it, but so the sweep never observes the brief same-frame window
/// between "registry entry removed" and "despawn commands flushed").
pub fn teardown_completed_dimensions(
    mut commands: Commands,
    mut registry: ResMut<DimensionRegistry>,
    mut torn_down_writer: MessageWriter<DimensionTornDown>,
) {
    // Collect ids first: `remove_torn_down` takes `&mut self`, so we can't
    // hold a borrow from `registry.ids()` while calling it in the same loop.
    let ready: Vec<DimensionId> = registry
        .ids()
        .filter(|&id| {
            registry.lifecycle(id) == Some(crate::lifecycle::DimensionLifecycle::Teardown)
        })
        .collect();

    for id in ready {
        // See this module's own doc comment: the always-on default
        // dimension must never actually be destroyed, even if it somehow
        // reached `Teardown` (a pre-existing EM-4.5 admin-command gap, not
        // introduced here).
        if id == DimensionId::DEFAULT {
            error!(
                ?id,
                "refusing to tear down the default dimension (would destroy the live game) — this \
                 dimension will remain stuck in Teardown; see teardown.rs's module doc"
            );
            continue;
        }

        match registry.remove_torn_down(id) {
            Ok(state) => {
                let root = state.root();
                let occupant_count = state.occupant_count();
                extract_persistent_side_effects_before_teardown(id, &state);
                // `state` (chunk store, generated world/index) is dropped
                // here, whole, before the despawn even executes — ordinary
                // `Drop`, no manual cleanup.
                drop(state);
                commands.entity(root).despawn();
                torn_down_writer.write(DimensionTornDown { id, occupant_count });
                info!(?id, occupant_count, "dimension torn down (GC complete)");
            },
            Err(err @ DimensionError::NotFound(_) | err @ DimensionError::WrongLifecycle(..)) => {
                // Can't happen in practice (we just filtered on `Teardown`
                // above, and nothing else removes entries mid-loop), but
                // handled rather than `.unwrap()`ed — same defensive posture
                // `xindeler-sim-bridge`'s own registry calls use.
                error!(?err, ?id, "failed to remove a Teardown dimension");
            },
            Err(err) => error!(?err, ?id, "unexpected error tearing down dimension"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bevy::{MinimalPlugins, app::PluginGroup};
    use server::World;

    use super::*;
    use crate::{
        component::DimensionRoot, lifecycle::DimensionLifecycle, plugin::DimensionsPlugin,
    };

    /// Spec §1.9's own acceptance bar, made concrete: despawning cascades
    /// through descendants, the registry entry is gone, and NO entity
    /// anywhere still carries the dead `DimensionId` afterwards.
    #[test]
    fn teardown_despawns_root_cascades_members_and_frees_the_registry_entry() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);

        let root = app.world_mut().spawn(DimensionId(1)).id();
        let member = app
            .world_mut()
            .spawn((DimensionId(1), DimensionRoot(root)))
            .id();
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry
                .insert_spinning_up(DimensionId(1), root, 0)
                .unwrap();
            let (world, index) = World::empty();
            registry
                .complete_spinup(DimensionId(1), Arc::new(world), index)
                .unwrap();
            // Zero occupants recorded -> begin_draining tears it down
            // immediately (EM-4.5's own documented behavior).
            registry
                .begin_draining(DimensionId(1))
                .expect("Active -> Draining is legal");
            assert_eq!(
                registry.lifecycle(DimensionId(1)),
                Some(DimensionLifecycle::Teardown)
            );
        }

        app.update(); // teardown_completed_dimensions removes the registry
        // entry + queues the despawn; a second update flushes the deferred
        // command and runs the cascade.
        app.update();

        assert!(
            app.world().get_entity(root).is_err(),
            "root should have been despawned"
        );
        assert!(
            app.world().get_entity(member).is_err(),
            "member should have cascade-despawned with its root"
        );
        assert!(
            app.world()
                .resource::<DimensionRegistry>()
                .get(DimensionId(1))
                .is_none()
        );

        // No leftover entity anywhere still tagged with the dead id.
        let mut query = app.world_mut().query::<&DimensionId>();
        assert!(
            query.iter(app.world()).all(|id| *id != DimensionId(1)),
            "no entity should carry the dead DimensionId after teardown"
        );
    }

    /// The default dimension is protected even if it somehow reaches
    /// `Teardown` — the destructive payload refuses to run against it (see
    /// the module doc comment for why).
    #[test]
    fn teardown_never_despawns_the_default_dimension() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);

        let root = app.world_mut().spawn(DimensionId::DEFAULT).id();
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry
                .insert_spinning_up(DimensionId::DEFAULT, root, 0)
                .unwrap();
            let (world, index) = World::empty();
            registry
                .complete_spinup(DimensionId::DEFAULT, Arc::new(world), index)
                .unwrap();
            // No occupants ever registered here (mirrors the EXACT boot-time
            // gap this module's doc comment describes) -> begin_draining
            // tears it "down" per the label, immediately.
            registry.begin_draining(DimensionId::DEFAULT).unwrap();
            assert_eq!(
                registry.lifecycle(DimensionId::DEFAULT),
                Some(DimensionLifecycle::Teardown)
            );
        }

        app.update();
        app.update();

        // Still there — the payload refused to run.
        assert!(app.world().get_entity(root).is_ok());
        assert!(
            app.world()
                .resource::<DimensionRegistry>()
                .contains(DimensionId::DEFAULT)
        );
    }

    /// A dimension that is `Active`/`Draining` (not yet `Teardown`) is left
    /// completely alone by this system.
    #[test]
    fn teardown_ignores_dimensions_not_yet_in_teardown() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);

        let root = app.world_mut().spawn(DimensionId(2)).id();
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry
                .insert_spinning_up(DimensionId(2), root, 0)
                .unwrap();
            let (world, index) = World::empty();
            registry
                .complete_spinup(DimensionId(2), Arc::new(world), index)
                .unwrap();
        }

        app.update();

        assert!(app.world().get_entity(root).is_ok());
        assert!(
            app.world()
                .resource::<DimensionRegistry>()
                .contains(DimensionId(2))
        );
    }
}
