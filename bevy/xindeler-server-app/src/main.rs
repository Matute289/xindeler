//! Xindeler dedicated server — Bevy headless shell (`MinimalPlugins`) that
//! embeds the authoritative Veloren sim (BL-82 EM-4.1).
//!
//! **Dual-stack from day one**: [`sim::boot_dedicated_server`] reads the SAME
//! production `server::Settings` server-cli reads (`<userdata>/server`'s
//! `settings.ron`), so the sim's own quinn/TCP listener
//! (`gameserver_protocols`) comes up completely unmodified — an OLD
//! (wire-protocol-identical) client connects to this binary exactly as it does
//! to `server-cli` today. `server-cli` itself is untouched and still builds;
//! this binary is the shell that eventually replaces it (EM-4.x), not a fork of
//! it.
//!
//! [`plugin::SimServerPlugin`] owns the embedded `Server` (as a Bevy
//! non-send resource — see `sim.rs`), the per-tick `Server::tick` system,
//! SIGINT/SIGTERM graceful-shutdown handling (`shutdown.rs`), and the
//! Prometheus metrics passthrough (`metrics.rs`).
//!
//! **BL-82 EM-4.2b**: this is also now the FIRST place in the codebase a real
//! `bevy_replicon` connection crosses an actual network socket — a second,
//! genuinely dual-stack listener alongside the untouched legacy one. See
//! `plugin.rs`'s doc comment and `sim::DEFAULT_REPLICON_ADDR` for the config
//! surface, and `xindeler-transport`'s crate doc comment for the transport
//! abstraction this binary calls through.

mod dimensions;
mod metrics;
mod plugin;
mod shutdown;
mod sim;

use bevy::{
    MinimalPlugins,
    app::{App, AppExit, PluginGroup, ScheduleRunnerPlugin},
};

use xindeler_oracle_host::AiGatewayConfig;

use crate::{
    dimensions::DebugDimensionCommands,
    plugin::SimServerPlugin,
    sim::{SIM_TICK_INTERVAL, SimServerConfig},
};

fn main() -> AppExit {
    tracing_subscriber::fmt::init();

    // EM-4.2: mirrors server-cli's own `#[cfg(feature = "hot-agent")]
    // agent::init()` call (`server-cli/src/main.rs`) — eagerly starts the
    // `server-agent` dylib compile+load+file-watcher BEFORE `Server::new`
    // boots the sim, so NPC AI is already hot-reloadable from the first
    // tick rather than lazily on the first AI decision. No-op (compiled out
    // entirely) unless built with `--features hot-agent`, matching
    // server-cli's own opt-in (not default) posture for this feature.
    #[cfg(feature = "hot-agent")]
    {
        agent::init();
    }

    // Mirrors server-cli's `--no-auth` CLI flag as an env var: this shell has
    // no CLI parser yet (EM-4.1 scope is the shell + dual-stack + signal
    // handling + metrics, not a `clap` port of server-cli's `ArgvApp`).
    let no_auth = std::env::var_os("XINDELER_SERVER_NO_AUTH").is_some();
    let metrics_addr = match std::env::var("XINDELER_SERVER_METRICS_ADDR") {
        Ok(addr) => match addr.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    addr,
                    "XINDELER_SERVER_METRICS_ADDR is set but not a valid socket address; falling \
                     back to the default"
                );
                SimServerConfig::default().metrics_addr
            },
        },
        Err(_) => SimServerConfig::default().metrics_addr,
    };
    // EM-4.2e: AI-gateway config seam. No caller dials out from this crate
    // regardless of the loaded `mode` — see `xindeler_oracle_host::
    // ai_gateway`'s module doc. Optional so this shell keeps booting with the
    // safe `Offline` default when unset (the common case today: BL-83/BL-85
    // don't exist yet).
    let ai_gateway = match std::env::var("XINDELER_SERVER_AI_GATEWAY_CONFIG") {
        Ok(path) => match std::fs::read_to_string(&path) {
            Ok(text) => match AiGatewayConfig::from_ron_str(&text) {
                Ok(config) => config,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        path,
                        "XINDELER_SERVER_AI_GATEWAY_CONFIG points at a file that failed to parse \
                         as AiGatewayConfig RON; falling back to the default (Offline)"
                    );
                    AiGatewayConfig::default()
                },
            },
            Err(err) => {
                tracing::warn!(
                    ?err,
                    path,
                    "XINDELER_SERVER_AI_GATEWAY_CONFIG is set but the file could not be read; \
                     falling back to the default (Offline)"
                );
                AiGatewayConfig::default()
            },
        },
        Err(_) => AiGatewayConfig::default(),
    };
    // EM-4.5: debug/admin dimension-spinup/drain triggers — see
    // `dimensions.rs`'s doc comment for why env vars (not a live RPC/console)
    // are this task's "debug/admin command" mechanism.
    let debug_dimension_commands = DebugDimensionCommands::from_env();

    // BL-82 EM-4.2b: same env-var-for-v1 pattern as the two vars above — a
    // full settings-file field is Phase 5 polish. Must never collide with
    // `metrics_addr`/the legacy `gameserver_protocols` port(s); see
    // `sim::DEFAULT_REPLICON_ADDR`'s doc comment.
    let replicon_addr = match std::env::var("XINDELER_SERVER_REPLICON_ADDR") {
        Ok(addr) => match addr.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    addr,
                    "XINDELER_SERVER_REPLICON_ADDR is set but not a valid socket address; falling \
                     back to the default"
                );
                SimServerConfig::default().replicon_addr
            },
        },
        Err(_) => SimServerConfig::default().replicon_addr,
    };

    App::new()
        .add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(SIM_TICK_INTERVAL)))
        .add_plugins(SimServerPlugin {
            config: SimServerConfig {
                no_auth,
                metrics_addr,
                ai_gateway,
                replicon_addr,
                debug_dimension_commands,
            },
        })
        .run()
}
