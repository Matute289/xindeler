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
use prometheus::{IntCounter, IntGauge, Opts, Registry};
use xindeler_dimensions::{
    DimensionId, DimensionLifecycle, DimensionRegistry, DimensionSpinupConfig, DimensionTornDown,
    DrainDimension, SpinupDimension, WorldGenThreadPool,
};
use xindeler_sim_bridge::{PlayerDimensionSession, TransferPlayerDimension};

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
    /// BL-82 EM-4.9 follow-up: if set, once this dimension id reaches
    /// `Active`, transfers the first currently-tracked real replicon session
    /// ([`PlayerDimensionSession`]) into it. A deterministic manual lever for
    /// the player-transfer mechanism (`xindeler_sim_bridge::player_transfer`)
    /// — mirrors [`Self::spinup`]/[`Self::drain`]'s own "no chat/console
    /// admin-command bridge exists yet" convention, and doubles as this
    /// codebase's fast, non-flaky way to drive a REAL logged-in player
    /// through the transfer end to end in an integration test (proximity
    /// requires the player to physically be standing inside an event's
    /// trigger zone, which a black-box subprocess test cannot arrange
    /// deterministically without this).
    pub transfer_player: Option<u64>,
}

impl DebugDimensionCommands {
    pub fn from_env() -> Self {
        let spinup = std::env::var("XINDELER_DEBUG_SPINUP_DIMENSION")
            .ok()
            .and_then(|v| v.parse().ok());
        let drain = std::env::var("XINDELER_DEBUG_DRAIN_DIMENSION")
            .ok()
            .and_then(|v| v.parse().ok());
        let transfer_player = std::env::var("XINDELER_DEBUG_TRANSFER_PLAYER_DIMENSION")
            .ok()
            .and_then(|v| v.parse().ok());
        Self {
            spinup,
            drain,
            transfer_player,
        }
    }
}

/// One-shot latches so [`apply_debug_dimension_commands`] fires each
/// requested command exactly once.
#[derive(Resource, Default)]
pub(crate) struct DebugDimensionState {
    spinup_sent: bool,
    drain_sent: bool,
    transfer_sent: bool,
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
    mut transfer_writer: bevy::ecs::message::MessageWriter<TransferPlayerDimension>,
    sessions: bevy::ecs::system::Query<&PlayerDimensionSession>,
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

    // BL-82 EM-4.9 follow-up: transfers the FIRST currently-tracked real
    // replicon session into `id` once it's `Active`. "First" is an
    // intentional v1 simplification (this debug lever is for a
    // single-session manual/test drive, not a multi-player admin tool) —
    // see `DebugDimensionCommands::transfer_player`'s own doc comment.
    if !state.transfer_sent
        && let Some(id) = commands.transfer_player
        && registry.lifecycle(DimensionId(id)) == Some(DimensionLifecycle::Active)
        && let Some(session) = sessions.iter().next()
    {
        transfer_writer.write(TransferPlayerDimension {
            sim_entity: session.0,
            target: DimensionId(id),
        });
        tracing::info!(
            dimension = id,
            "debug command: transferring the first tracked player session into dimension \
             (XINDELER_DEBUG_TRANSFER_PLAYER_DIMENSION)"
        );
        state.transfer_sent = true;
    }
}

/// The registry's lifecycle-count gauges (BL-82 EM-4.5), mirroring
/// EM-4.2e's `AiGatewayMetrics` pattern: registered once on the SAME
/// `prometheus::Registry` `/metrics` already serves.
///
/// ## `teardown` is a snapshot gauge; `teardowns_total` is the durable signal
/// (EM-4.6 follow-up, found while verifying the phase-4 wave-3 integration)
/// `xindeler_dimensions::teardown::teardown_completed_dimensions` removes a
/// dimension's registry entry in the SAME `FixedUpdate` tick it observes it
/// in `Teardown` (its own documented "immediate GC" posture — see that
/// module's doc comment; EM-4.10 Finding B moved this chain off `Update`
/// render cadence onto `FixedUpdate` sim cadence, this invariant is
/// unaffected by that move) — so `teardown` (an `IntGauge` reflecting only
/// the CURRENT registry snapshot) can be, and in the common
/// occupant-less-drain case reliably IS, zero on every single `/metrics`
/// scrape: an external poller can never durably catch a dimension
/// "currently in Teardown" because nothing keeps it there past the tick
/// that discovers it. A dimension actually being torn down is still a real,
/// meaningful event operators/tests want to observe — so `teardowns_total`
/// is a genuinely monotonic `IntCounter`, bumped once per
/// [`xindeler_dimensions::DimensionTornDown`] message
/// [`update_dimension_metrics`] drains — fired unconditionally, exactly
/// once, at the removal site itself (`teardown_completed_dimensions`), so
/// unlike a between-tick registry-id diff (this module's first cut, caught
/// by an `ecs-design-reviewer` pass) it can never silently miss a dimension
/// whose entire `Spinup -> Active -> Draining -> Teardown` lifecycle happens
/// to complete within a single `FixedUpdate` tick. Unlike the gauge, this
/// survives being scraped any time after the event, same as
/// `xindeler_oracle_host::ai_gateway`'s own `requests_total`/
/// `fallback_total` counters.
#[derive(Resource, Clone)]
pub struct DimensionMetrics {
    pub spinup: IntGauge,
    pub active: IntGauge,
    pub draining: IntGauge,
    pub teardown: IntGauge,
    pub teardowns_total: IntCounter,
}

fn make_gauge(registry: &Registry, name: &str, help: &str) -> IntGauge {
    let gauge =
        IntGauge::with_opts(Opts::new(name, help)).expect("static metric options are always valid");
    registry
        .register(Box::new(gauge.clone()))
        .unwrap_or_else(|_| panic!("{name} must not already be registered"));
    gauge
}

fn make_counter(registry: &Registry, name: &str, help: &str) -> IntCounter {
    let counter = IntCounter::with_opts(Opts::new(name, help))
        .expect("static metric options are always valid");
    registry
        .register(Box::new(counter.clone()))
        .unwrap_or_else(|_| panic!("{name} must not already be registered"));
    counter
}

/// Registers the 4 lifecycle-count gauges + the `teardowns_total` counter on
/// `registry`. Manual `Opts`/`with_opts`/`registry.register` pattern,
/// matching every other metrics module in this workspace (same convention
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
        teardowns_total: make_counter(
            registry,
            "dimension_lifecycle_teardowns_total",
            "cumulative count of dimensions torn down (GC-completed) since boot",
        ),
    }
}

/// Refreshes the 4 gauges from the live [`DimensionRegistry`] every tick —
/// cheap (O(live dimensions), a handful at most) and the only way an
/// external process (a test scraping `/metrics`, or an operator) observes
/// this shell's dimension state without a live admin RPC channel. Also bumps
/// `teardowns_total` once per [`DimensionTornDown`] message drained this tick
/// — see [`DimensionMetrics`]'s doc comment for why a real message, fired at
/// the removal site, is the durable signal instead of anything derived from
/// this system's own registry snapshot.
pub fn update_dimension_metrics(
    registry: Res<DimensionRegistry>,
    metrics: Res<DimensionMetrics>,
    mut torn_down_reader: bevy::ecs::message::MessageReader<DimensionTornDown>,
) {
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

    let torn_down_this_tick = torn_down_reader.read().count() as u64;
    if torn_down_this_tick > 0 {
        metrics.teardowns_total.inc_by(torn_down_this_tick);
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
