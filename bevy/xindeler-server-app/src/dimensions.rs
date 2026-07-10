//! EM-4.5 wiring for the dedicated-server shell: wraps the sim's
//! already-generated `Arc<World>`/`IndexOwned` as `DimensionId::DEFAULT` at
//! boot, hosts the debug/admin spinup+drain env-var triggers (this task's
//! "debug/admin command" — a full `DmEvent`-triggered spinup is EM-4.9's
//! job), and registers the registry's lifecycle-count gauges on the same
//! `/metrics` passthrough `AiGatewayPlugin` already uses.
//!
//! Per `xindeler-oracle-host::dm_event`'s own doc comment (written just
//! before this task landed): "when EM-4.5 does the real wiring, it must land
//! in `xindeler-server-app` only, never `xindeler-client`" — this module is
//! that landing.

use std::sync::Arc;

use bevy::{
    app::App,
    ecs::{
        resource::Resource,
        system::{Res, ResMut},
    },
};
use prometheus::{IntGauge, Opts, Registry};
use xindeler_dimensions::{
    DimensionId, DimensionLifecycle, DimensionRegistry, DimensionSpinupConfig, DrainDimension,
    SpinupDimension, WorldGenThreadPool,
};

use crate::sim::SimServer;

/// Debug/admin one-shot triggers, read once at boot from env vars — mirrors
/// `xindeler-sim-bridge`'s own `XINDELER_TEST_NPC_COUNT` env-var-toggle
/// convention (the closest existing "trigger something at runtime" pattern
/// in this codebase; there is no chat/console admin-command bridge from a
/// Bevy shell into the sim yet).
#[derive(Debug, Clone, Default, Resource)]
pub struct DebugDimensionCommands {
    /// If set, spin up this dimension id on the first tick.
    pub spinup: Option<u64>,
    /// If set, once the `spinup` dimension reaches `Active`, drain it —
    /// exercises the FULL 4-state machine
    /// (Spinup -> Active -> Draining -> Teardown; the last transition is
    /// immediate here since this shell never registers a Bevy-side occupant
    /// for its own dimensions) in one boot, observable via `/metrics`.
    pub drain: Option<u64>,
}

impl DebugDimensionCommands {
    pub fn from_env() -> Self {
        let spinup = std::env::var("XINDELER_DEBUG_SPINUP_DIMENSION")
            .ok()
            .and_then(|v| v.parse().ok());
        let drain = std::env::var("XINDELER_DEBUG_DRAIN_DIMENSION")
            .ok()
            .and_then(|v| v.parse().ok());
        Self { spinup, drain }
    }
}

/// One-shot latches so [`apply_debug_dimension_commands`] fires each
/// requested command exactly once.
#[derive(Resource, Default)]
pub(crate) struct DebugDimensionState {
    spinup_sent: bool,
    drain_sent: bool,
}

/// Wraps `DimensionId::DEFAULT` around the sim's ALREADY-generated
/// `Arc<World>`/`IndexOwned` (spec §1.8's "wrapping refactor, not a behavior
/// change") and installs the [`WorldGenThreadPool`] (the sim's own reused
/// rayon pool) so a subsequent debug-triggered spinup has somewhere to run.
/// Call AFTER `app.add_plugins(xindeler_dimensions::DimensionsPlugin)` (which
/// `init_resource`s an EMPTY [`DimensionRegistry`] this function then
/// populates in place) and BEFORE the sim is moved into
/// `app.insert_non_send(sim)`.
///
/// The actual "register + complete spinup" sequence is
/// [`xindeler_dimensions::wrap_default_dimension`] — shared with
/// `xindeler-sim-bridge`'s own `ensure_default_dimension` so the two shells'
/// wrap logic can't silently drift; this function only supplies the
/// build-time-specific parts (spawning the root entity via `app.world_mut()`
/// rather than `Commands`, and installing the thread pool).
pub fn install_default_dimension(app: &mut App, sim: &SimServer) {
    let root = app.world_mut().spawn(DimensionId::DEFAULT).id();
    {
        let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
        xindeler_dimensions::wrap_default_dimension(&mut registry, root, &sim.server)
            .expect("DimensionId::DEFAULT is registered exactly once, at boot");
    }
    app.insert_resource(WorldGenThreadPool(Arc::clone(
        sim.server.state().thread_pool(),
    )));
    tracing::info!("dimension 0 (default) wrapped: Spinup -> Active");
}

/// Applies [`DebugDimensionCommands`] once each: sends [`SpinupDimension`] on
/// the very first tick if `spinup` is set, then — once that dimension
/// reaches `Active` — sends [`DrainDimension`] if `drain` is also set.
pub fn apply_debug_dimension_commands(
    commands: Res<DebugDimensionCommands>,
    mut state: ResMut<DebugDimensionState>,
    registry: Res<DimensionRegistry>,
    mut spinup_writer: bevy::ecs::message::MessageWriter<SpinupDimension>,
    mut drain_writer: bevy::ecs::message::MessageWriter<DrainDimension>,
) {
    if !state.spinup_sent
        && let Some(id) = commands.spinup
    {
        spinup_writer.write(SpinupDimension {
            id: DimensionId(id),
            // Arbitrary — real per-dimension seed derivation is a future
            // (EM-4.9) concern; what THIS task proves is that the resulting
            // dimension is independently generated and isolated, not a
            // specific seed-selection policy.
            base_seed: id as u32,
            config: DimensionSpinupConfig::default(),
        });
        tracing::info!(
            dimension = id,
            "debug command: spinning up dimension (XINDELER_DEBUG_SPINUP_DIMENSION)"
        );
        state.spinup_sent = true;
    }

    if !state.drain_sent
        && let Some(id) = commands.drain
        && registry.lifecycle(DimensionId(id)) == Some(DimensionLifecycle::Active)
    {
        drain_writer.write(DrainDimension(DimensionId(id)));
        tracing::info!(
            dimension = id,
            "debug command: draining dimension (XINDELER_DEBUG_DRAIN_DIMENSION)"
        );
        state.drain_sent = true;
    }
}

/// The registry's lifecycle-count gauges (BL-82 EM-4.5), mirroring
/// EM-4.2e's `AiGatewayMetrics` pattern: registered once on the SAME
/// `prometheus::Registry` `/metrics` already serves.
#[derive(Resource, Clone)]
pub struct DimensionMetrics {
    pub spinup: IntGauge,
    pub active: IntGauge,
    pub draining: IntGauge,
    pub teardown: IntGauge,
}

fn make_gauge(registry: &Registry, name: &str, help: &str) -> IntGauge {
    let gauge =
        IntGauge::with_opts(Opts::new(name, help)).expect("static metric options are always valid");
    registry
        .register(Box::new(gauge.clone()))
        .unwrap_or_else(|_| panic!("{name} must not already be registered"));
    gauge
}

/// Registers the 4 lifecycle-count gauges on `registry`. Manual
/// `Opts`/`with_opts`/`registry.register` pattern, matching every other
/// metrics module in this workspace (same convention
/// `xindeler_oracle_host::ai_gateway::register_metrics` documents).
pub fn register_metrics(registry: &Registry) -> DimensionMetrics {
    DimensionMetrics {
        spinup: make_gauge(
            registry,
            "dimension_lifecycle_spinup",
            "number of dimensions currently in Spinup",
        ),
        active: make_gauge(
            registry,
            "dimension_lifecycle_active",
            "number of dimensions currently Active",
        ),
        draining: make_gauge(
            registry,
            "dimension_lifecycle_draining",
            "number of dimensions currently Draining",
        ),
        teardown: make_gauge(
            registry,
            "dimension_lifecycle_teardown",
            "number of dimensions currently in Teardown",
        ),
    }
}

/// Refreshes the 4 gauges from the live [`DimensionRegistry`] every tick —
/// cheap (O(live dimensions), a handful at most) and the only way an
/// external process (a test scraping `/metrics`, or an operator) observes
/// this shell's dimension state without a live admin RPC channel.
pub fn update_dimension_metrics(registry: Res<DimensionRegistry>, metrics: Res<DimensionMetrics>) {
    let (mut spinup, mut active, mut draining, mut teardown) = (0i64, 0i64, 0i64, 0i64);
    for id in registry.ids() {
        match registry.lifecycle(id) {
            Some(DimensionLifecycle::Spinup) => spinup += 1,
            Some(DimensionLifecycle::Active) => active += 1,
            Some(DimensionLifecycle::Draining) => draining += 1,
            Some(DimensionLifecycle::Teardown) => teardown += 1,
            None => {},
        }
    }
    metrics.spinup.set(spinup);
    metrics.active.set(active);
    metrics.draining.set(draining);
    metrics.teardown.set(teardown);
}

/// Registers [`DebugDimensionState`] (the latch resource
/// [`apply_debug_dimension_commands`] needs — [`DebugDimensionCommands`]
/// itself is inserted by the caller with the env-derived config).
pub fn init_debug_state(app: &mut App) { app.init_resource::<DebugDimensionState>(); }
