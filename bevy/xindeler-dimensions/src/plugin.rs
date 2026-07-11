//! [`DimensionsPlugin`] — wires [`DimensionRegistry`] + the spinup/drain
//! admin-command machinery + the isolation sweep into a Bevy `App`. Does
//! NOT itself register [`DimensionId::DEFAULT`] (wrapping the sim's already-
//! generated world/index needs a live `SimServer`, which this crate doesn't
//! own — see `xindeler-server-app`'s own wiring, which calls
//! [`DimensionRegistry::insert_spinning_up`] +
//! [`DimensionRegistry::complete_spinup`] once its sim exists, mirroring the
//! same "plugin doesn't boot the sim, the shell does" pattern
//! `xindeler-sim-bridge::SimBridgePlugin` already documents for `SimServer`).

use bevy::prelude::*;

use crate::{
    component::{DimensionId, DimensionRoot},
    predictive_gc::{PredictiveGc, PredictiveGcTrackers, predictive_gc_system},
    registry::DimensionRegistry,
    spinup::{
        DimensionActivated, DrainDimension, SpinupDimension, SpinupTasks, handle_drain_requests,
        handle_spinup_requests, poll_spinup_tasks,
    },
    teardown::{DimensionTornDown, teardown_completed_dimensions},
};

/// Registers [`DimensionRegistry`] + the spinup/drain message types +
/// systems, the EM-4.6 predictive-GC heuristic + `Teardown` GC payload, and
/// (debug builds only) the isolation `debug_assert` sweep (spec §5.3's own
/// acceptance bar).
///
/// ## System ordering (EM-4.6 update — read before touching this chain)
/// The isolation sweep now runs FIRST in the `FixedUpdate` chain, BEFORE
/// `handle_spinup_requests`/`handle_drain_requests`/`predictive_gc_system`/
/// `teardown_completed_dimensions` — a deliberate reordering from EM-4.5's
/// original `.after(handle_drain_requests)` placement, required now that
/// `teardown_completed_dimensions` performs a REAL despawn for the first
/// time. `teardown_completed_dimensions` removes a dimension's registry
/// entry synchronously but only QUEUES its root's despawn (`Commands`,
/// applied at the end of `FixedUpdate`); if the sweep ran later in the SAME
/// tick, it would transiently see entities still tagged with a `DimensionId`
/// the registry no longer knows about (a false-positive
/// `IsolationViolation::UnknownDimension` panic). Running the sweep FIRST
/// instead validates the fully-settled state left over from the END of the
/// PREVIOUS tick (all of that tick's deferred despawns are guaranteed
/// flushed by the schedule boundary before `FixedUpdate` runs again) —
/// equally rigorous, just shifted by one tick, and immune to this same-tick
/// race.
///
/// ## EM-4.10 Finding B: `FixedUpdate`, not `Update`
/// This whole chain — plus `debug_assert_dimension_isolation` below AND
/// `xindeler_sim_bridge::delete_specs_entities_for_torn_down_dimensions`
/// (which orders itself `.after(handle_drain_requests).after(
/// predictive_gc_system).before(teardown_completed_dimensions)` against this
/// SAME chain, so it must live in the SAME schedule) — used to run in
/// `Update`, i.e. once per RENDERED frame (60-160 Hz in a windowed
/// listen-server session). These are sim-bookkeeping systems with no reason
/// to run at render cadence; running them there multiplied every per-system
/// cost — and, worse, the Finding-A `EntityHashSet` fix's remaining
/// per-insert bookkeeping cost inside `teardown`/GC — by up to ~5x per
/// second, a direct contributor to the "FPS oscillating wildly, worsening
/// over the session" regression diagnosed 2026-07-10.
/// `debug_assert_dimension_isolation` was the worst single offender: a full
/// `Query` scan + a fresh `HashMap` build, `#[cfg(debug_assertions)]`-gated
/// but ON by default in the `dev` profile used for all gameplay testing, so
/// it ran at render rate too. This repeats the EM-3.11b fix pattern exactly
/// (`tick_sim`/`mirror_sim_entities`/`tick_aurora_overlay` already moved to
/// `FixedUpdate` for the identical reason) — this Wave-3 chain simply hadn't
/// gotten the same treatment yet. The `.chain()` and every `.before`/`.after`
/// edge documented above are preserved verbatim across the move; only the
/// schedule changed.
pub struct DimensionsPlugin;

impl Plugin for DimensionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DimensionRegistry>()
            .init_resource::<SpinupTasks>()
            .init_resource::<PredictiveGc>()
            .init_resource::<PredictiveGcTrackers>()
            .add_message::<SpinupDimension>()
            .add_message::<DrainDimension>()
            .add_message::<DimensionTornDown>()
            // BL-82 EM-4.9: the "Active edge" `DmEvent`-triggered producers
            // consume (see `spinup::DimensionActivated`'s doc comment).
            .add_message::<DimensionActivated>()
            .add_systems(
                FixedUpdate,
                (
                    handle_spinup_requests,
                    poll_spinup_tasks,
                    handle_drain_requests,
                    predictive_gc_system,
                    teardown_completed_dimensions,
                )
                    .chain(),
            );

        // Cheap (O(tagged entities)) and only meaningful in dev/test builds,
        // matching the "debug_assert sweep" framing spec §5.3 uses — never
        // shipped as production-tick overhead. `.before(handle_spinup_requests)`
        // (not `.after(handle_drain_requests)` — see this struct's own doc
        // comment for why EM-4.6 moved it to the front of the chain.
        // EM-4.10 Finding B: `FixedUpdate`, alongside the rest of the chain
        // above — see this struct's doc comment.
        #[cfg(debug_assertions)]
        app.add_systems(
            FixedUpdate,
            debug_assert_dimension_isolation.before(handle_spinup_requests),
        );
    }
}

/// Live Bevy-system wrapper around [`crate::registry::sweep_isolation`]:
/// queries every `(DimensionId, DimensionRoot)`-tagged entity and panics if
/// the isolation invariant is violated. `debug_assertions`-only (see
/// [`DimensionsPlugin`]).
#[cfg(debug_assertions)]
fn debug_assert_dimension_isolation(
    registry: Res<DimensionRegistry>,
    query: Query<(Entity, &DimensionId, &DimensionRoot)>,
) {
    let tagged = query.iter().map(|(entity, id, root)| (entity, *id, root.0));
    if let Err(violation) = crate::registry::sweep_isolation(&registry, tagged) {
        panic!("dimension isolation violated: {violation:?}");
    }
}

#[cfg(test)]
mod tests {
    use bevy::{MinimalPlugins, app::PluginGroup};

    use super::*;

    /// The plugin builds cleanly and the registry starts empty (no default
    /// dimension — that's the SHELL's job, see the module doc).
    #[test]
    fn plugin_builds_with_an_empty_registry() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);
        app.update();

        let registry = app.world().resource::<DimensionRegistry>();
        assert_eq!(registry.ids().count(), 0);
    }

    /// EM-4.10 Finding B: the lifecycle chain must live in `FixedUpdate`, not
    /// `Update` — proven by running each schedule in ISOLATION
    /// (`World::run_schedule`, which executes a schedule directly regardless
    /// of `Time::<Fixed>`'s own real-time accumulator, so this test is not
    /// timing-dependent) and observing that only running `FixedUpdate`
    /// actually processes a pending `DrainDimension` request.
    #[test]
    fn lifecycle_chain_runs_in_fixed_update_not_update() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);

        let id = DimensionId(11);
        let root = app.world_mut().spawn_empty().id();
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry.insert_spinning_up(id, root, 0).unwrap();
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(id, std::sync::Arc::new(world), index)
                .unwrap();
        }
        app.world_mut().write_message(DrainDimension(id));

        // Running JUST `Update` must NOT process the drain request — the
        // chain (and `handle_drain_requests` within it) is not registered
        // there. `try_run_schedule` (rather than `run_schedule`, which
        // PANICS if the label is unknown) lets this test assert the
        // strongest possible version of "not in Update": the schedule was
        // never even created, because nothing was ever registered into it.
        assert!(
            app.world_mut().try_run_schedule(bevy::app::Update).is_err(),
            "the `Update` schedule must not even exist — nothing in the lifecycle chain registers \
             into it anymore"
        );
        assert_eq!(
            app.world().resource::<DimensionRegistry>().lifecycle(id),
            Some(crate::lifecycle::DimensionLifecycle::Active),
            "the lifecycle chain must NOT be registered in Update"
        );

        // Running `FixedUpdate` DOES process it — zero registry-tracked
        // occupants means `begin_draining` tears it down immediately
        // (`handle_drain_requests`), and — since `teardown_completed_
        // dimensions` is chained right after in the SAME schedule run —
        // the dimension is torn all the way down to fully REMOVED from the
        // registry within this single `run_schedule` call, not merely
        // parked at `Teardown`. That end-to-end result is exactly what
        // proves the WHOLE chain (not just one system) is registered in
        // `FixedUpdate`.
        app.world_mut().run_schedule(bevy::app::FixedUpdate);
        assert_eq!(
            app.world().resource::<DimensionRegistry>().lifecycle(id),
            None,
            "the lifecycle chain must be registered in FixedUpdate and actually run there, all \
             the way through to removal"
        );
    }

    /// A correctly-tagged entity graph passes the live isolation sweep
    /// without panicking (the panic-on-violation path is exercised
    /// separately via `registry::sweep_isolation`'s own unit tests, which
    /// assert on the `Result` directly rather than relying on
    /// panic-catching a live system).
    #[test]
    fn isolation_sweep_system_passes_for_correctly_tagged_entities() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);

        let root = app.world_mut().spawn(DimensionId(0)).id();
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry
                .insert_spinning_up(DimensionId(0), root, 0)
                .unwrap();
        }
        app.world_mut().spawn((DimensionId(0), DimensionRoot(root)));

        // EM-4.10 Finding B: the sweep now lives in `FixedUpdate`, not
        // `Update` — run that schedule directly (see
        // `lifecycle_chain_runs_in_fixed_update_not_update`'s doc comment)
        // so this genuinely exercises the sweep rather than depending on
        // `Time::<Fixed>`'s real-time accumulator. Would panic if the sweep
        // found a violation.
        app.world_mut().run_schedule(bevy::app::FixedUpdate);
    }
}
