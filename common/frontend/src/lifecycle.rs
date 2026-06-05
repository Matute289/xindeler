use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

// ── ErrorDetectorLayer ─────────────────────────────────────────────────────

/// Sets `has_errors` to true whenever a WARN or ERROR event is emitted.
pub struct ErrorDetectorLayer {
    pub has_errors: Arc<AtomicBool>,
}

impl ErrorDetectorLayer {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (Self { has_errors: Arc::clone(&flag) }, flag)
    }
}

impl<S: Subscriber> Layer<S> for ErrorDetectorLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() <= Level::WARN {
            self.has_errors.store(true, Ordering::Relaxed);
        }
    }
}

// ── LogLifecycleManager ────────────────────────────────────────────────────

pub struct LogLifecycleManager {
    logs_dir: PathBuf,
}

impl LogLifecycleManager {
    pub fn new(logs_dir: PathBuf) -> Self { Self { logs_dir } }

    /// Run on process start: delete files older than their retention window.
    pub fn cleanup_on_startup(&self, info_retention: Duration, err_retention: Duration) {
        let now = SystemTime::now();
        let Ok(entries) = fs::read_dir(&self.logs_dir) else { return };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            let age = now.duration_since(modified).unwrap_or_default();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let expired = if name.contains("_info") || name.contains("_telemetry") {
                age > info_retention
            } else if name.contains("_err") {
                age > err_retention
            } else {
                false
            };
            if expired {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    /// Run on clean exit: if no errors occurred during the session, delete
    /// info and telemetry log files (they add no diagnostic value).
    pub fn cleanup_on_exit(&self, has_errors: &AtomicBool) {
        if has_errors.load(Ordering::Relaxed) {
            return; // errors occurred — keep all logs
        }
        let Ok(entries) = fs::read_dir(&self.logs_dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("_info") || name.contains("_telemetry") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}
