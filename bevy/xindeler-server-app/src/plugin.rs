//! Glues `sim` + `shutdown` + `metrics` into the one plugin the EM-4.1 task
//! board line asks for verbatim: "`SimServerPlugin` owning
//! `veloren_server::Server` + `.tick()` system, signal handling, metrics
//! passthrough."

use std::sync::Arc;

use bevy::{
    app::{App, Plugin, Update},
    ecs::schedule::IntoScheduleConfigs,
};
use tokio::sync::Notify;
use xindeler_oracle_host::{AiGatewayConfig, AiGatewayPlugin};

use crate::{
    metrics,
    shutdown::{self, ShutdownState},
    sim::{self, SimServerConfig},
};

/// Boots the embedded sim, installs the SIGINT/SIGTERM shutdown flag, starts
/// the metrics passthrough, and registers the per-tick systems — see the
/// module doc comment. Building this plugin boots a real (possibly slow,
/// asset-dependent) world, same as `server-cli`'s `main` calling `Server::new`
/// directly; unlike `xindeler-sim-bridge` (which defers booting to its
/// caller), this crate IS the shell, so there is no other place to do it.
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

        // EM-4.2e: AI-gateway config/metrics seam. Registers 2 zero-value
        // counters on the SAME registry `/metrics` above serves; makes no
        // real AI call (see `xindeler_oracle_host::ai_gateway`'s doc
        // comment).
        app.add_plugins(AiGatewayPlugin {
            config: AiGatewayConfig::default(),
            registry: metrics_registry,
        });

        app.insert_non_send(sim);
        app.insert_resource(ShutdownState {
            flag: shutdown_flag,
            metrics_shutdown,
        });
        // `check_shutdown` runs AFTER `tick_sim` so a shutdown request never
        // lands mid-tick — see shutdown.rs's doc comment.
        app.add_systems(Update, (sim::tick_sim, shutdown::check_shutdown).chain());
    }
}
