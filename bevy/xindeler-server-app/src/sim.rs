//! The embedded specs simulation (EM-4.1): boots the SAME `Server::new` /
//! `.tick()` / `.cleanup()` recipe server-cli's `server_loop` and
//! `xindeler-sim-bridge::boot_test_server` use, but reads the REAL production
//! settings (`server::Settings::load` / `EditableSettings::load` — not the
//! `singleplayer()` shortcut those two use), so `gameserver_protocols` comes
//! straight from `<userdata>/server/server_config/settings.ron` exactly like
//! server-cli. Dual-stack falls out of this for free: whatever protocols
//! (TCP/QUIC) that file lists come up unmodified — this shell never touches
//! `gameserver_protocols` beyond the same `no_auth` override server-cli's
//! `--no-auth` flag applies.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bevy::{
    ecs::{change_detection::NonSendMut, system::Res},
    time::Time,
};
use server::{
    EditableSettings, Event, Input, Server, Settings,
    persistence::{DatabaseSettings, SqlLogMode},
    settings::Protocol,
};
use tokio::runtime::Runtime;

use crate::metrics::DEFAULT_METRICS_ADDR;

/// Server tick rate (30 TPS — matches server-cli's `TPS` const and the sim's
/// own expectations). `ScheduleRunnerPlugin::run_loop(SIM_TICK_INTERVAL)`
/// paces the whole headless `App` at this cadence.
pub const SIM_TICK_INTERVAL: Duration = Duration::from_nanos(33_333_333); // exactly 1/30 s

/// Knobs [`crate::plugin::SimServerPlugin`] needs beyond what's read off
/// `settings.ron`.
#[derive(Debug, Clone)]
pub struct SimServerConfig {
    /// Mirrors server-cli's `--no-auth` flag: clears
    /// `Settings::auth_server_address` so clients register directly instead
    /// of going through the (possibly unreachable, e.g. in CI/dev sandboxes)
    /// auth server.
    pub no_auth: bool,
    /// Bind address for the metrics passthrough (EM-4.1). Defaults to the
    /// same port server-cli's own `web_address` setting serves `/metrics` on.
    pub metrics_addr: SocketAddr,
}

impl Default for SimServerConfig {
    fn default() -> Self {
        Self {
            no_auth: false,
            metrics_addr: DEFAULT_METRICS_ADDR
                .parse()
                .expect("DEFAULT_METRICS_ADDR is a valid SocketAddr literal"),
        }
    }
}

/// The embedded authoritative simulation, owned by
/// [`crate::plugin::SimServerPlugin`] as a Bevy **non-send** resource (`Server`
/// is `Send` but not `Sync` — its specs `SendDispatcher` boxes `dyn RunNow +
/// Send` stages without a `Sync` bound; same reasoning
/// `xindeler-sim-bridge::SimServer` documents). Non-send storage also pins
/// [`tick_sim`] to the main thread, matching how server-cli ticks the sim from
/// its own main loop.
pub struct SimServer {
    /// The authoritative veloren/xindeler simulation.
    pub server: Server,
    /// Runtime backing the sim's async work (networking, persistence). Kept
    /// alive here for the sim's lifetime; also used to spawn the metrics
    /// passthrough server (EM-4.1).
    pub runtime: Arc<Runtime>,
    /// Number of successful [`tick_sim`] passes since boot.
    pub ticks: u64,
}

/// Boots a real dedicated-server `Server` rooted at `<userdata>/server` (the
/// SAME data dir server-cli uses — see `server::DEFAULT_DATA_DIR_NAME`'s doc
/// comment: "Used so that different server frontends can share the same
/// server saves, etc."). Panics only where server-cli itself would treat the
/// failure as fatal (tokio runtime build); a `Server::new` failure is
/// returned so the caller decides how to report it.
pub fn boot_dedicated_server(config: &SimServerConfig) -> Result<SimServer, server::Error> {
    let data_dir: PathBuf = {
        let mut path = common_base::userdata_dir();
        path.push(server::DEFAULT_DATA_DIR_NAME);
        path
    };
    tracing::info!(path = %data_dir.display(), "using userdata folder");

    let mut server_settings = Settings::load(&data_dir);
    let editable_settings = EditableSettings::load(&data_dir);
    if config.no_auth {
        server_settings.auth_server_address = None;
    }

    let database_settings = DatabaseSettings {
        db_dir: data_dir.join("saves"),
        sql_log_mode: SqlLogMode::Disabled,
    };

    // Same sizing formula + thread-name-per-worker scheme as server-cli's
    // runtime (server-cli/src/main.rs): a small pool is enough — the sim's
    // heavy lifting runs on its own rayon/slow-job pools, this runtime only
    // backs networking + persistence + (EM-4.1) the metrics server.
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(
                (num_cpus::get() / 4).max(common::consts::MIN_RECOMMENDED_TOKIO_THREADS),
            )
            .thread_name_fn(|| {
                static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
                let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
                format!("tokio-server-app-{id}")
            })
            .build()
            .expect("failed to build tokio runtime for the xindeler dedicated server"),
    );

    // Log the listener addresses BEFORE `Server::new` consumes `settings` by
    // value, so the dual-stack acceptance criterion (old client can still
    // reach the sim's own listener) is directly visible in the process log.
    let gameserver_addresses: Vec<(&'static str, std::net::SocketAddr)> = server_settings
        .gameserver_protocols
        .iter()
        .map(|protocol| match protocol {
            Protocol::Tcp { address } => ("TCP", *address),
            Protocol::Quic { address, .. } => ("QUIC", *address),
        })
        .collect();

    let server = Server::new(
        server_settings,
        editable_settings,
        database_settings,
        &data_dir,
        &|stage| tracing::debug!(?stage, "sim server init"),
        Arc::clone(&runtime),
    )?;

    tracing::info!(
        ?gameserver_addresses,
        "xindeler dedicated server ready to accept connections (dual-stack: the sim's own \
         listener is unmodified — old/wire-protocol-identical clients connect exactly as they do \
         against server-cli)"
    );

    Ok(SimServer {
        server,
        runtime,
        ticks: 0,
    })
}

/// Advances the embedded sim by one tick using Bevy's frame `dt`, then drains
/// the sim's frontend events into `tracing` — the same shape as server-cli's
/// `server_loop` body and `xindeler-sim-bridge::tick_sim`.
///
/// No-ops until [`crate::plugin::SimServerPlugin`] has inserted a
/// [`SimServer`] (there is no `resource_exists` equivalent for non-send data,
/// so the gate is the `Option` param).
pub fn tick_sim(time: Res<Time>, sim: Option<NonSendMut<SimServer>>) {
    let Some(mut sim) = sim else { return };
    let dt = time.delta();
    let events = match sim.server.tick(Input::default(), dt) {
        Ok(events) => events,
        Err(err) => {
            tracing::error!(?err, "sim server tick failed");
            return;
        },
    };
    for event in events {
        match event {
            Event::ClientConnected { .. } => tracing::info!("client connected"),
            Event::ClientDisconnected { .. } => tracing::info!("client disconnected"),
            Event::Chat { msg, .. } => tracing::info!("chat: {msg}"),
        }
    }

    // Like server-cli's loop: clean up after every tick (clears TerrainChanges
    // and other per-tick bookkeeping the sim expects a frontend to drain).
    sim.server.cleanup();
    sim.ticks += 1;
    if sim.ticks.rem_euclid(1000) == 0 {
        tracing::trace!(ticks = sim.ticks, "keepalive");
    }
}
