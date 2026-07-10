//! Graceful shutdown (EM-4.1): SIGINT/SIGTERM → shared atomic flag, the same
//! signal-hook mechanism server-cli's `shutdown_coordinator` uses for its
//! configurable signal list (server-cli defaults to SIGUSR1 only, chosen so
//! Watchtower-triggered restarts don't look like a plain kill; this shell
//! hardcodes SIGINT+SIGTERM per the EM-4.1 spec — the two signals a operator/
//! orchestrator (systemd, Docker, Ctrl+C) actually sends for a graceful stop).
//!
//! ## Simplified vs server-cli's `ShutdownCoordinator`
//! server-cli's coordinator supports a configurable **grace period**: on
//! signal it broadcasts a countdown chat warning to connected players, waits
//! out the grace period (repeating the warning, faster near the end), and
//! only then stops the tick loop. This v1 skips the countdown and stops on
//! the NEXT completed tick — still graceful (the in-flight tick finishes,
//! `cleanup()` runs, then `Server`'s `Drop` impl notifies players of the
//! disconnect + flushes terrain/rtsim persistence — see
//! `server/src/lib.rs`'s `impl Drop for Server`), just without the
//! player-facing countdown warnings. A configurable grace period is a
//! reasonable EM-4.2+ follow-up if operators want it; not required for the
//! EM-4.1 acceptance (dual-stack connect + clean exit).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bevy::{
    app::AppExit,
    ecs::{message::MessageWriter, resource::Resource, system::Res},
};
use tokio::sync::Notify;

/// Shared shutdown-request flag + the metrics server's own shutdown signal,
/// both driven from [`check_shutdown`].
#[derive(Resource)]
pub struct ShutdownState {
    /// Set by the SIGINT/SIGTERM handlers registered in [`register_signals`].
    pub flag: Arc<AtomicBool>,
    /// Notified once, right before we write `AppExit`, so the metrics server
    /// (EM-4.1's `metrics::spawn`) stops accepting connections in step with
    /// the sim shutting down rather than lingering after the process exits.
    pub metrics_shutdown: Arc<Notify>,
}

/// Registers OS signal handlers for SIGINT + SIGTERM that flip a shared
/// atomic flag (signal-hook's `flag::register`, same primitive server-cli
/// uses — safe to call from a signal handler, unlike most other work).
/// Windows has no equivalent via `signal-hook`; mirrors server-cli's own
/// platform gate (warn + no graceful path, matching upstream's precedent).
pub fn register_signals() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));

    #[cfg(not(target_os = "windows"))]
    {
        // SIGTERM is the signal a production orchestrator (systemd, Docker,
        // k8s) actually sends for a graceful stop; if it fails to register,
        // the OS default disposition kills the process immediately with NO
        // `Drop for Server` flush — the exact "graceful stop that silently
        // isn't" failure this module exists to prevent, so this is `error!`
        // rather than `warn!`. SIGINT (Ctrl+C) is the interactive/dev path;
        // a failure there is lower-stakes but still surfaced.
        if let Err(err) =
            signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag))
        {
            tracing::error!(
                ?err,
                "failed to register SIGTERM handler — an orchestrator-triggered stop will now \
                 hard-kill this process with NO persistence flush"
            );
        }
        if let Err(err) =
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&flag))
        {
            tracing::warn!(?err, "failed to register SIGINT (Ctrl+C) handler");
        }
    }
    #[cfg(target_os = "windows")]
    tracing::warn!(
        "SIGINT/SIGTERM handling is not supported on this platform; the process relies on the OS \
         terminating it directly (no graceful persistence flush)."
    );

    flag
}

/// Polls [`ShutdownState::flag`] once per `Update` — see
/// [`crate::plugin::SimServerPlugin`]'s doc comment: Bevy's
/// `MainScheduleOrder` always runs `Update` strictly AFTER the
/// `RunFixedMainLoop` schedule (which drives `xindeler_sim_bridge::tick_sim`'s
/// `FixedUpdate` step, EM-4.2b), so a shutdown request always lands after a
/// full tick+cleanup, never mid-tick. On a set flag: notifies the metrics
/// server to stop, then writes `AppExit::Success`. `ScheduleRunnerPlugin`'s
/// runner drops the whole `App` (and with it the non-send `SimServer` →
/// `Server`'s `Drop` impl) as soon as it observes the exit, before `App::run()`
/// returns to `main`.
pub fn check_shutdown(state: Res<ShutdownState>, mut exit: MessageWriter<AppExit>) {
    if state.flag.load(Ordering::Relaxed) {
        tracing::info!("shutdown signal received; finishing gracefully");
        state.metrics_shutdown.notify_one();
        exit.write(AppExit::Success);
    }
}
