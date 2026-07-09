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

mod metrics;
mod plugin;
mod shutdown;
mod sim;

use bevy::{
    MinimalPlugins,
    app::{App, AppExit, PluginGroup, ScheduleRunnerPlugin},
};

use crate::{
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

    App::new()
        .add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(SIM_TICK_INTERVAL)))
        .add_plugins(SimServerPlugin {
            config: SimServerConfig {
                no_auth,
                metrics_addr,
            },
        })
        .run()
}
