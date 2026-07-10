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
    registry::DimensionRegistry,
    spinup::{
        DrainDimension, SpinupDimension, SpinupTasks, handle_drain_requests,
        handle_spinup_requests, poll_spinup_tasks,
    },
};

/// Registers [`DimensionRegistry`] + the spinup/drain message types +
/// systems, and (debug builds only) the isolation `debug_assert` sweep (spec
/// §5.3's own acceptance bar).
pub struct DimensionsPlugin;

impl Plugin for DimensionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DimensionRegistry>()
            .init_resource::<SpinupTasks>()
            .add_message::<SpinupDimension>()
            .add_message::<DrainDimension>()
            .add_systems(
                Update,
                (
                    handle_spinup_requests,
                    poll_spinup_tasks,
                    handle_drain_requests,
                )
                    .chain(),
            );

        // Cheap (O(tagged entities)) and only meaningful in dev/test builds,
        // matching the "debug_assert sweep" framing spec §5.3 uses — never
        // shipped as production-tick overhead.
        #[cfg(debug_assertions)]
        app.add_systems(
            Update,
            debug_assert_dimension_isolation.after(handle_drain_requests),
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

        // Would panic if the sweep found a violation.
        app.update();
    }
}
