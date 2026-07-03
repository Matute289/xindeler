//! BL-82 (EM-1.6) — smoke-bot binary: boots a local server, runs a real
//! client through connect → character → move → logout, prints the report.
//! Exit code 0 = the logic stack is alive. See the lib docs for details.

use xindeler_smoke_bot::{SmokeOptions, run_smoke};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match run_smoke(SmokeOptions::default()) {
        Ok(report) => {
            println!("smoke OK: {report:#?}");
        },
        Err(e) => {
            eprintln!("smoke FAILED: {e}");
            std::process::exit(1);
        },
    }
}
