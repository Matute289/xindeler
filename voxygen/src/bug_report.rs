use serde_json;
use std::{
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
};
use tracing::{error, info};

/// Result returned by send_bug_report — used to update the UI notification.
#[derive(Debug, Clone)]
pub enum BugReportResult {
    Sent,
    Skipped(String),
    Failed(String),
}

const MAX_TELEMETRY_LINES: usize = 500;
const MAX_ERR_LINES: usize = 200;

fn find_latest(dir: &Path, contains: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(contains))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().next().map(|(_, p)| p)
}

fn read_tail(path: &Path, max_lines: usize) -> Vec<String> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            error!(?e, ?path, "Cannot open log file for bug report");
            return Vec::new();
        },
    };
    let reader = io::BufReader::new(file);
    let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
    let skip = lines.len().saturating_sub(max_lines);
    lines[skip..].to_vec()
}

/// Send a bug report to the configured URL.
/// This function is synchronous — call from a background thread.
pub fn send_bug_report(url: &str, logs_dir: &Path) -> BugReportResult {
    if url.trim().is_empty() {
        return BugReportResult::Skipped("bug_report_url is empty".to_string());
    }

    let telemetry_lines = find_latest(logs_dir, "telemetry")
        .map(|p| read_tail(&p, MAX_TELEMETRY_LINES))
        .unwrap_or_default();

    let err_lines = find_latest(logs_dir, "client_err")
        .or_else(|| find_latest(logs_dir, "_err"))
        .map(|p| read_tail(&p, MAX_ERR_LINES))
        .unwrap_or_default();

    let payload = serde_json::json!({
        "client_version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "telemetry": telemetry_lines,
        "errors": err_lines,
    });

    info!(
        url,
        telemetry_lines = telemetry_lines.len(),
        err_lines = err_lines.len(),
        "Sending bug report"
    );

    match ureq::post(url).send_json(payload) {
        Ok(resp) => {
            info!(status = resp.status(), "Bug report sent");
            BugReportResult::Sent
        },
        Err(ureq::Error::Status(code, _)) => {
            let msg = format!("Server returned HTTP {code}");
            error!(code, "Bug report server error");
            BugReportResult::Failed(msg)
        },
        Err(e) => {
            let msg = format!("Network error: {e}");
            error!(?e, "Bug report network error");
            BugReportResult::Failed(msg)
        },
    }
}
