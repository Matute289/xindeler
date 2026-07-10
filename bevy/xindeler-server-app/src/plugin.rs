//! Glues `sim` + `shutdown` + `metrics` + (BL-82 EM-4.2b) the new
//! replicon+quinnet transport into the one plugin the EM-4.1 task board line
//! asks for verbatim: "`SimServerPlugin` owning `veloren_server::Server` +
//! `.tick()` system, signal handling, metrics passthrough" — now extended per
//! the EM-4.2b spec (§1.1) to also host `xindeler-sim-bridge`'s entity/terrain
//! mirror + a REAL remote `bevy_replicon` server role, dual-stack alongside
//! the untouched legacy listener.

use std::sync::Arc;

use bevy::{
    app::{App, Plugin, Update},
    ecs::schedule::IntoScheduleConfigs,
    state::app::StatesPlugin,
    time::{Fixed, Time},
};
use bevy_replicon::prelude::RepliconPlugins;
use tokio::sync::Notify;
use xindeler_oracle_host::AiGatewayPlugin;
use xindeler_protocol::XindelerProtocolPlugin;
use xindeler_sim_bridge::{
    SIM_TICK_HZ, SimBridgePlugin, SimEntityMirrorPlugin, SimTerrainStreamPlugin, tick_sim,
};
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

use crate::{
    login::{self, ActiveReplicaSessions, PendingLogins},
    metrics,
    shutdown::{self, ShutdownState},
    sim::{self, SimServerConfig},
};

/// Boots the embedded sim, installs the SIGINT/SIGTERM shutdown flag, starts
/// the metrics passthrough, wires the entity/terrain mirror + the new
/// replicon+quinnet transport (EM-4.2b), and registers the per-tick systems —
/// see the module doc comment. Building this plugin boots a real (possibly
/// slow, asset-dependent) world, same as `server-cli`'s `main` calling
/// `Server::new` directly; unlike `xindeler-sim-bridge` (which defers booting
/// to its caller), this crate IS the shell, so there is no other place to do
/// it.
#[derive(Default)]
pub struct SimServerPlugin {
    pub config: SimServerConfig,
}

impl Plugin for SimServerPlugin {
    fn build(&self, app: &mut App) {
        let sim = sim::boot_dedicated_server(&self.config)
            .expect("failed to create the xindeler dedicated server instance");

        let shutdown_flag = shutdown::register_signals();

        let metrics_registry = Arc::clone(sim.server.metrics_registry());
        let metrics_shutdown = Arc::new(Notify::new());
        metrics::spawn(
            &sim.runtime,
            Arc::clone(&metrics_registry),
            self.config.metrics_addr,
            Arc::clone(&metrics_shutdown),
        );

        // EM-4.2e: AI-gateway config/metrics seam. `self.config.ai_gateway`
        // defaults to `Offline`/no-op but is overridable via `main.rs`'s
        // `XINDELER_SERVER_AI_GATEWAY_CONFIG` env var, so this is a genuinely
        // exercised RON-load path, not just a tested capability. Registers 2
        // zero-value counters on the SAME registry `/metrics` above serves;
        // makes no real AI call (see `xindeler_oracle_host::ai_gateway`'s doc
        // comment).
        app.add_plugins(AiGatewayPlugin {
            config: self.config.ai_gateway.clone(),
            registry: metrics_registry,
        });

        app.insert_non_send(sim);
        app.insert_resource(ShutdownState {
            flag: shutdown_flag,
            metrics_shutdown,
        });
        // `check_shutdown` runs in `Update`, which Bevy's `MainScheduleOrder`
        // always runs strictly AFTER the `RunFixedMainLoop` schedule (which
        // drives every `FixedUpdate` step queued for that frame) —
        // structurally, for any tick rate, not merely because
        // `sim::SIM_TICK_INTERVAL` happens to match `SIM_TICK_HZ` below — so
        // a shutdown request still never lands mid-tick (see
        // shutdown.rs's doc comment). `tick_sim` itself is no longer
        // registered here — `SimBridgePlugin` below owns it (EM-4.2b: shared
        // with the listen-server path instead of a second local copy).
        app.add_systems(Update, shutdown::check_shutdown);

        // EM-4.2b: pace the bridge's `FixedUpdate` tick_sim at the sim's real
        // 30 TPS, same as the listen-server client does (see
        // `xindeler_sim_bridge::tick_sim`'s doc comment for why FixedUpdate,
        // not Update, is the right home for this).
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));

        // Real remote `bevy_replicon` SERVER role + the shared replication
        // contract + the sim↔Bevy bridge (entity mirror + terrain stream —
        // the SAME plugins the listen-server path uses, now hosted here
        // instead, per spec §0.1/§1.1). `RepliconPlugins` is added
        // UNCONFIGURED (library default `ServerPlugin::default()`, which
        // replicates on `FixedPostUpdate`) rather than overridden to
        // `PostUpdate`: `FixedPostUpdate` runs in the SAME `FixedMain`
        // iteration as `SimBridgePlugin`/`SimEntityMirrorPlugin`'s own
        // `FixedUpdate` mirror writes, so every fixed-step mirror update gets
        // replicated — no frame can coalesce/drop an intermediate step (which
        // a `PostUpdate` override, ticking once per RENDER frame regardless
        // of how many `FixedUpdate` steps ran that frame, could do). An
        // earlier draft copied `ServerPlugin::new(PostUpdate)` from
        // `xindeler-sim-bridge`'s OWN test harness (`src/lib.rs`'s
        // `new_test_app`), where that override exists only to satisfy a
        // manually-stepped `TimeUpdateStrategy::ManualDuration` test app that
        // may never run a real `FixedMain` iteration — an unrelated
        // constraint that doesn't apply to this real wall-clock,
        // `ScheduleRunnerPlugin`-paced production shell.
        //
        // No client role is ever activated in this process (`ClientState`
        // stays `Disconnected`), so `SimTerrainStreamPlugin`/
        // `SimEntityMirrorPlugin`'s
        // `run_if(in_state(ClientState::Disconnected))` gate holds
        // continuously — this shell is purely a replication SOURCE, never a
        // sink.
        //
        // `StatesPlugin` is NOT part of `MinimalPlugins` (this shell's own
        // plugin set, `main.rs`) — only `DefaultPlugins` bundles it. Every
        // other place `RepliconPlugins` is added in this codebase gets
        // `StatesPlugin` for free (the listen-server client via
        // `DefaultPlugins`; `xindeler-protocol`'s own tests add it
        // explicitly alongside `MinimalPlugins`). `bevy_replicon`'s
        // `ClientState`/`ServerState` are Bevy `States`, so without this the
        // very first `RepliconPlugins` system to touch `StateTransition`
        // panics at Startup ("the `StateTransition` schedule is missing").
        //
        // Note: this process also compiles/registers a dormant client-role
        // `ClientPlugin`/`ClientMessagePlugin` (Cargo feature unification —
        // see `xindeler-transport`'s crate doc comment, "Both client+server
        // bevy_replicon roles compile into EVERY shell", for the full
        // explanation and why it's harmless and not worth fighting here).
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            XindelerProtocolPlugin,
            SimBridgePlugin,
            SimTerrainStreamPlugin,
            SimEntityMirrorPlugin,
        ));

        // EM-4.2b: the transport seam — this crate names ONLY
        // `xindeler_transport::{ReplicaTransport, TransportConfig,
        // QuinnetTransport}`, never `bevy_replicon_quinnet`/`bevy_quinnet`
        // types (see `xindeler-transport`'s own doc comment for the grep bar
        // this maintains). Binds a port distinct from the legacy
        // `gameserver_protocols` listener(s) — see `sim::DEFAULT_REPLICON_ADDR`
        // and `XINDELER_SERVER_REPLICON_ADDR` in `main.rs`.
        app.add_plugins(
            QuinnetTransport.server_plugins(&TransportConfig::server(self.config.replicon_addr)),
        );

        // BL-82 EM-4.2c: the login/session handshake — see `login.rs`'s
        // module doc comment for the full design (why a SEPARATE
        // `CharacterLoader` instance, the v1 auto-select policy, the
        // documented IP-ban gap). `RepliconCharacterLoader` reads/opens the
        // SAME `saves/` sqlite path `boot_dedicated_server` (via
        // `sim::server_data_dir()`) already pointed the sim's OWN
        // `CharacterLoader` at.
        app.insert_resource(login::boot_replicon_character_loader(
            &sim::server_data_dir(),
        ));
        app.insert_resource(PendingLogins::default());
        app.insert_resource(ActiveReplicaSessions::default());
        // `.before(tick_sim)`: this system creates the sim entity + resolves
        // auth/character-loading BEFORE the sim's own `FixedUpdate` systems
        // (physics, subscription, persistence batching, …) run for this
        // frame, so a character that finishes loading this tick is already
        // fully set up by the time the rest of the sim ticks over it —
        // matching the natural "ingest input, then simulate" ordering the
        // rest of this `FixedUpdate` chain already follows (see `xindeler-
        // sim-bridge`'s own doc comment on `tick_sim`'s
        // `.after`/`.before` chain). Correctness does not actually depend on
        // this ordering (see `login.rs`'s module doc comment: the dedicated
        // `CharacterLoader` instance means there is no shared-channel race
        // either way), but it is the more intuitive ordering.
        app.add_systems(
            bevy::app::FixedUpdate,
            login::handle_replicon_logins.before(tick_sim),
        );
    }
}
