//! Metrics passthrough (EM-4.1): serves `Server::metrics_registry()` at
//! `/metrics` over real HTTP — the exact route server-cli's own axum `web`
//! module exposes (`server-cli/src/web/mod.rs`'s `.nest("/metrics", ...)`),
//! via the already-pinned workspace `prometheus-hyper` crate (used the same
//! way in `network/examples/network_speed.rs`) instead of porting
//! server-cli's whole `web` module — the chat relay + admin `ui_api` + TUI
//! wiring are console-integrated features with no equivalent in this headless
//! shell, and are out of EM-4.1 scope (only the metrics passthrough itself is
//! asked for).

use std::{net::SocketAddr, sync::Arc};

use prometheus::Registry;
use tokio::{runtime::Runtime, sync::Notify};

/// Default bind address for the metrics passthrough — the SAME default port
/// server-cli's own `web_address` setting uses (`server-cli/src/settings.rs`),
/// so ops tooling already pointed at `:14005/metrics` keeps working once this
/// shell replaces server-cli. Overridable via `XINDELER_SERVER_METRICS_ADDR`.
pub const DEFAULT_METRICS_ADDR: &str = "127.0.0.1:14005";

/// Spawns the `/metrics` HTTP server onto `runtime`, serving `registry` until
/// `shutdown` is notified (mirrors server-cli's `metrics_shutdown: Arc<Notify>`
/// pattern — `main.rs` calls `.notify_one()` once `server_loop` returns).
pub fn spawn(runtime: &Runtime, registry: Arc<Registry>, addr: SocketAddr, shutdown: Arc<Notify>) {
    runtime.spawn(async move {
        if let Err(err) = prometheus_hyper::Server::run(registry, addr, shutdown.notified()).await {
            tracing::error!(?err, "metrics passthrough server error");
        }
    });
    tracing::info!(%addr, "metrics passthrough listening (prometheus text format at /metrics)");
}
