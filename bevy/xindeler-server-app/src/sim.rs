//! The embedded specs simulation (EM-4.1): boots the SAME `Server::new` /
//! `.tick()` / `.cleanup()` recipe server-cli's `server_loop` uses, but reads
//! the REAL production settings (`server::Settings::load` /
//! `EditableSettings::load` — not the `singleplayer()` shortcut
//! `xindeler-sim-bridge::boot_test_server` uses for the listen-server path),
//! so `gameserver_protocols` comes straight from
//! `<userdata>/server/server_config/settings.ron` exactly like server-cli.
//! Dual-stack falls out of this for free: whatever protocols (TCP/QUIC) that
//! file lists come up unmodified — this shell never touches
//! `gameserver_protocols` beyond the same `no_auth` override server-cli's
//! `--no-auth` flag applies.
//!
//! ## EM-4.2b: boot/tick now flow through `xindeler-sim-bridge`
//! [`SimServer`] used to be a SEPARATE, near-duplicate copy of
//! `xindeler-sim-bridge`'s own type (same fields, same tick recipe, just
//! reading production settings instead of the singleplayer shortcut and
//! without the bridge's `pending_terrain` snapshot). That duplication is now
//! gone: [`boot_dedicated_server`] loads the real settings this module has
//! always read, then hands them to `xindeler_sim_bridge::boot_with_settings`
//! (extracted from `boot_test_server` for exactly this reuse), producing an
//! `xindeler_sim_bridge::SimServer` — re-exported here as [`SimServer`] — with
//! its `pending_terrain` snapshot intact. [`crate::plugin::SimServerPlugin`]
//! then adds `xindeler_sim_bridge::{SimBridgePlugin, SimTerrainStreamPlugin,
//! SimEntityMirrorPlugin}` directly (imported there, not re-exported through
//! this module) instead of registering a second, local `tick_sim` system —
//! those plugins' `FixedUpdate` `tick_sim` drives the sim, streams terrain,
//! and mirrors entities into `Replicated` Bevy entities for the new
//! replicon+quinnet transport (`xindeler-transport`) to actually send. This
//! is a wrapping refactor, not a behavior change: the settings source, the
//! `no_auth` override, and the dual-stack legacy listener are all untouched —
//! including the tokio runtime's own CPU-scaled sizing (below), which an
//! earlier draft of this extraction accidentally silently downgraded to the
//! singleplayer path's small fixed size; [`boot_with_settings`] now takes
//! that sizing as a parameter instead of picking one on every caller's
//! behalf, so this shell passes its own formula back through exactly as
//! before.

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use server::{
    EditableSettings, Settings,
    persistence::{DatabaseSettings, SqlLogMode},
    settings::Protocol,
};
use xindeler_oracle_host::AiGatewayConfig;
pub use xindeler_sim_bridge::SimServer;

use crate::{dimensions::DebugDimensionCommands, metrics::DEFAULT_METRICS_ADDR};

/// Server tick rate (30 TPS — matches server-cli's `TPS` const,
/// `xindeler_sim_bridge::SIM_TICK_HZ`, and the sim's own expectations).
/// `ScheduleRunnerPlugin::run_loop(SIM_TICK_INTERVAL)` paces the whole
/// headless `App` at this cadence; [`crate::plugin::SimServerPlugin`] ALSO
/// installs `Time::<Fixed>::from_hz(SIM_TICK_HZ)` so `tick_sim`'s
/// `FixedUpdate` schedule (owned by `xindeler_sim_bridge::SimBridgePlugin`)
/// ticks at the same rate.
pub const SIM_TICK_INTERVAL: Duration = Duration::from_nanos(33_333_333); // exactly 1/30 s

/// Default bind address for the new replicon+quinnet transport (BL-82
/// EM-4.2b) — deliberately a DIFFERENT port from both the legacy
/// `gameserver_protocols` default (14004) and the metrics passthrough default
/// (`DEFAULT_METRICS_ADDR`, 14005), so the two transports never collide when
/// both run dual-stack on one process. Overridable via
/// `XINDELER_SERVER_REPLICON_ADDR` (same env-var pattern as
/// `XINDELER_SERVER_METRICS_ADDR`/`XINDELER_SERVER_NO_AUTH` in `main.rs`) —
/// a full settings-file field is Phase 5 polish, out of this task's scope.
pub const DEFAULT_REPLICON_ADDR: &str = "127.0.0.1:14006";

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
    /// AI-gateway config seam (EM-4.2e). Defaults to
    /// `AiGatewayConfig::default()` (`mode: Offline`, zero AI activity);
    /// `main.rs` overrides this from `XINDELER_SERVER_AI_GATEWAY_CONFIG` (a
    /// RON file path) when set, mirroring `metrics_addr`'s
    /// env-var-overrides-a-sane-default pattern.
    pub ai_gateway: AiGatewayConfig,
    /// Bind address for the new replicon+quinnet transport (EM-4.2b). See
    /// [`DEFAULT_REPLICON_ADDR`].
    pub replicon_addr: SocketAddr,
    /// EM-4.5 debug/admin dimension-spinup/drain triggers. Defaults to
    /// `DebugDimensionCommands::default()` (neither set — no second
    /// dimension ever spins up unless explicitly requested); `main.rs`
    /// overrides this from `XINDELER_DEBUG_SPINUP_DIMENSION`/
    /// `XINDELER_DEBUG_DRAIN_DIMENSION` when set.
    pub debug_dimension_commands: DebugDimensionCommands,
}

impl Default for SimServerConfig {
    fn default() -> Self {
        Self {
            no_auth: false,
            metrics_addr: DEFAULT_METRICS_ADDR
                .parse()
                .expect("DEFAULT_METRICS_ADDR is a valid SocketAddr literal"),
            ai_gateway: AiGatewayConfig::default(),
            replicon_addr: DEFAULT_REPLICON_ADDR
                .parse()
                .expect("DEFAULT_REPLICON_ADDR is a valid SocketAddr literal"),
            debug_dimension_commands: DebugDimensionCommands::default(),
        }
    }
}

/// The SAME `<userdata>/server` data dir server-cli uses (see
/// `server::DEFAULT_DATA_DIR_NAME`'s doc comment: "Used so that different
/// server frontends can share the same server saves, etc."). Factored out of
/// [`boot_dedicated_server`] (BL-82 EM-4.2c) so `login.rs` can point its OWN
/// dedicated `CharacterLoader` instance (see that module's doc comment for
/// why it needs one) at the exact same `saves/` sqlite path without
/// duplicating this computation.
pub fn server_data_dir() -> PathBuf {
    let mut path = common_base::userdata_dir();
    path.push(server::DEFAULT_DATA_DIR_NAME);
    path
}

/// Boots a real dedicated-server [`SimServer`] rooted at `<userdata>/server`
/// (the SAME data dir server-cli uses — see `server::DEFAULT_DATA_DIR_NAME`'s
/// doc comment: "Used so that different server frontends can share the same
/// server saves, etc."). Panics only where server-cli itself would treat the
/// failure as fatal (tokio runtime build, inside
/// `xindeler_sim_bridge::boot_with_settings`); a `Server::new` failure is
/// returned so the caller decides how to report it.
pub fn boot_dedicated_server(config: &SimServerConfig) -> Result<SimServer, server::Error> {
    let data_dir: PathBuf = server_data_dir();
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

    // Log the listener addresses BEFORE handing `settings` off by value, so
    // the dual-stack acceptance criterion (old client can still reach the
    // sim's own listener, alongside the new replicon+quinnet transport) is
    // directly visible in the process log.
    let gameserver_addresses: Vec<(&'static str, std::net::SocketAddr)> = server_settings
        .gameserver_protocols
        .iter()
        .map(|protocol| match protocol {
            Protocol::Tcp { address } => ("TCP", *address),
            Protocol::Quic { address, .. } => ("QUIC", *address),
        })
        .collect();

    // Same sizing formula server-cli's own production runtime uses
    // (server-cli/src/main.rs) — a small pool is enough since the sim's heavy
    // lifting runs on its own rayon/slow-job pools, this runtime only backs
    // networking + persistence + the EM-4.1 metrics server, but it still
    // needs to scale with host core count, unlike the singleplayer/dev-test
    // path's small fixed size (see `boot_with_settings`'s doc comment for why
    // this is passed explicitly rather than inherited from that path).
    let worker_threads = (num_cpus::get() / 4).max(common::consts::MIN_RECOMMENDED_TOKIO_THREADS);
    let sim = xindeler_sim_bridge::boot_with_settings(
        server_settings,
        editable_settings,
        database_settings,
        &data_dir,
        worker_threads,
        "tokio-server-app",
    )?;

    tracing::info!(
        ?gameserver_addresses,
        replicon_addr = %config.replicon_addr,
        "xindeler dedicated server ready to accept connections (dual-stack: the sim's own \
         legacy listener is unmodified — old/wire-protocol-identical clients connect exactly as \
         they do against server-cli — AND the new replicon+quinnet transport listens separately, \
         EM-4.2b)"
    );

    Ok(sim)
}
