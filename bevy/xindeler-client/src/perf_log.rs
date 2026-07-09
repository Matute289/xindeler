//! EM-3.10b — opt-in periodic frame-time log, used to A/B-measure the GPU
//! occlusion-culling toggle ([`crate::camera::OcclusionCullingConfig`]).
//!
//! `FrameTimeDiagnosticsPlugin` is already installed by `XindelerAppPlugin`
//! (`bevy/xindeler-app/src/lib.rs`) for the FPS overlay; this module just
//! reads its smoothed value and logs a rolling average every
//! [`LOG_INTERVAL_FRAMES`] frames, so a plain `cargo run … 2>&1 | grep
//! "perf_log"` over a fixed wall-clock window gives a steady-state ms/frame
//! comparison without a bespoke benchmark harness. Silent (zero overhead
//! beyond one resource read) unless `XINDELER_PERF_LOG` is set, so it never
//! affects normal runs or the smoke screenshot.
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

/// How often to emit a log line, in frames — frequent enough to see a trend
/// within a short measurement run, sparse enough to stay readable.
const LOG_INTERVAL_FRAMES: u32 = 60;

/// Whether [`log_frame_time`] is active this run (`XINDELER_PERF_LOG=1`),
/// cached once at startup rather than re-reading the environment every frame.
#[derive(Resource)]
struct PerfLogEnabled(bool);

/// Frame counter for the logging cadence.
#[derive(Resource, Default)]
struct PerfLogState {
    frame: u32,
}

pub struct PerfLogPlugin;

impl Plugin for PerfLogPlugin {
    fn build(&self, app: &mut App) {
        let enabled = std::env::var("XINDELER_PERF_LOG").is_ok_and(|v| v != "0");
        app.insert_resource(PerfLogEnabled(enabled))
            .init_resource::<PerfLogState>()
            .add_systems(Update, log_frame_time);
    }
}

fn log_frame_time(
    enabled: Res<PerfLogEnabled>,
    mut state: ResMut<PerfLogState>,
    diagnostics: Res<DiagnosticsStore>,
) {
    if !enabled.0 {
        return;
    }
    state.frame += 1;
    if !state.frame.is_multiple_of(LOG_INTERVAL_FRAMES) {
        return;
    }
    let frame_time_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(bevy::diagnostic::Diagnostic::smoothed);
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(bevy::diagnostic::Diagnostic::smoothed);
    info!(
        target: "perf_log",
        frame = state.frame,
        frame_time_ms = frame_time_ms.unwrap_or(f64::NAN),
        fps = fps.unwrap_or(f64::NAN),
        "perf_log"
    );
}
