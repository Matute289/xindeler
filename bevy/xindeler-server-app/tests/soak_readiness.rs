//! EM-4.2: **soak-readiness sanity check** (NOT the real 24h soak — that's a
//! separate, much longer-running task; this is a short, bounded smoke test
//! that the shell doesn't show an OBVIOUS leak or tick-time blowup over a
//! run long enough to leave the initial-boot warm-up behind).
//!
//! Samples two signals at a fixed interval while `xindeler-server-app` runs
//! unattended (no client connects — this measures the shell's own
//! steady-state cost, not per-player load):
//!  - **RSS** via `ps -o rss=` (an external, black-box measurement — doesn't
//!    require instrumenting the binary).
//!  - **Tick time** via the REAL `/metrics` Prometheus endpoint EM-4.1 already
//!    exposes (`Server::metrics_registry()`'s `tick_time_hist`, a `Histogram`
//!    over the whole-tick duration in seconds — see `server/src/lib.rs`,
//!    `tick_time_hist.observe(end_of_server_tick.
//!    duration_since(before_state_tick)...)`). Consecutive
//!    `tick_time_hist_sum`/`_count` deltas give the mean tick time for just the
//!    window since the last sample, so a drift shows up as a rising per-window
//!    average rather than being smoothed by the whole-run cumulative average.
//!
//! Assertions here are DELIBERATELY loose (generous multiplicative bounds,
//! not tight regression gates) — the point of this test is to catch an
//! obviously-broken shell (a real leak or runaway tick time would show up
//! even in a short window), not to certify long-run stability, which is the
//! real 24h soak's job.
//!
//! Duration is controlled by `XINDELER_SOAK_SECONDS` (default 600 = 10
//! minutes, within the task's own "10-20 minutes is plenty" guidance);
//! sample interval by `XINDELER_SOAK_SAMPLE_SECONDS` (default 15s).
//!
//! Needs real assets + LFS and spawns a real child process, so — like the
//! other full-world tests in this crate — this is `#[ignore]`d; run locally
//! with: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app
//! --test soak_readiness -- --ignored --nocapture`

mod support;

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use support::{graceful_shutdown, seed_settings, spawn_server_app_with_env, wait_for_listener};

const DEFAULT_SOAK_SECONDS: u64 = 600;
const DEFAULT_SAMPLE_SECONDS: u64 = 15;

struct Sample {
    elapsed: Duration,
    rss_kb: u64,
    tick_hist_sum_secs: f64,
    tick_hist_count: u64,
}

/// Reads `rss=` in KB for `pid` via the platform `ps` utility (works
/// identically on macOS and Linux — the two platforms these binaries
/// actually run on; no new dependency needed for a black-box sample).
fn sample_rss_kb(pid: u32) -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Fetches `/metrics` over a raw HTTP/1.1 GET (no HTTP client dependency
/// needed for a single plaintext prometheus scrape) and extracts
/// `tick_time_hist_sum` / `tick_time_hist_count`.
fn sample_tick_hist(metrics_addr: SocketAddr) -> Option<(f64, u64)> {
    let mut stream = TcpStream::connect_timeout(&metrics_addr, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET /metrics HTTP/1.1\r\nHost: {metrics_addr}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut body = String::new();
    stream.read_to_string(&mut body).ok()?;

    let mut sum = None;
    let mut count = None;
    for line in body.lines() {
        if line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("tick_time_hist_sum ") {
            sum = value.trim().parse::<f64>().ok();
        } else if let Some(value) = line.strip_prefix("tick_time_hist_count ") {
            count = value.trim().parse::<u64>().ok();
        }
    }
    Some((sum?, count?))
}

#[test]
#[ignore = "spawns a real xindeler-server-app process and samples it for several minutes: needs \
            assets + LFS; run locally with VELOREN_ASSETS. Duration overridable via \
            XINDELER_SOAK_SECONDS (default 600s)."]
fn soak_readiness_short_window() {
    let soak_duration = Duration::from_secs(
        std::env::var("XINDELER_SOAK_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_SOAK_SECONDS),
    );
    let sample_interval = Duration::from_secs(
        std::env::var("XINDELER_SOAK_SAMPLE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_SAMPLE_SECONDS),
    );

    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let game_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let metrics_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port for metrics");
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], metrics_port).into();

    let mut child = spawn_server_app_with_env(data_dir.path(), "soak", [(
        "XINDELER_SERVER_METRICS_ADDR",
        metrics_addr.to_string(),
    )]);
    let pid = child.0.id();

    wait_for_listener(game_addr, Duration::from_secs(60), "soak");
    // The metrics HTTP server is spawned onto the sim's own tokio runtime a
    // moment after the game listener comes up; give it a short grace period
    // rather than treating a slow first scrape as fatal.
    let metrics_deadline = Instant::now() + Duration::from_secs(30);
    while sample_tick_hist(metrics_addr).is_none() {
        assert!(
            Instant::now() < metrics_deadline,
            "the /metrics endpoint never came up on {metrics_addr}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }

    println!(
        "[test] soak-readiness sanity check: pid={pid}, duration={soak_duration:?}, \
         sample_interval={sample_interval:?}"
    );

    let start = Instant::now();
    let mut samples = Vec::new();
    while start.elapsed() < soak_duration {
        assert!(
            !child.0.try_wait().is_ok_and(|s| s.is_some()),
            "xindeler-server-app exited unexpectedly during the soak-readiness window"
        );
        let rss_kb = sample_rss_kb(pid);
        let hist = sample_tick_hist(metrics_addr);
        if let (Some(rss_kb), Some((sum, count))) = (rss_kb, hist) {
            let sample = Sample {
                elapsed: start.elapsed(),
                rss_kb,
                tick_hist_sum_secs: sum,
                tick_hist_count: count,
            };
            println!(
                "[test] t={:>4.0}s rss={:>7}KB tick_hist_count={:>7} tick_hist_sum={:.3}s",
                sample.elapsed.as_secs_f64(),
                sample.rss_kb,
                sample.tick_hist_count,
                sample.tick_hist_sum_secs
            );
            samples.push(sample);
        } else {
            println!("[test] sample skipped (rss or metrics scrape failed transiently)");
        }
        std::thread::sleep(sample_interval);
    }

    graceful_shutdown(&mut child, Duration::from_secs(30), "soak");
    println!("[test] process exited cleanly (graceful SIGTERM shutdown confirmed)");

    assert!(
        samples.len() >= 3,
        "expected at least 3 successful samples over the soak-readiness window, got {}",
        samples.len()
    );

    // ---- RSS: loose leak sanity check ----
    // Skip the first sample (still warming up: chunk generation, asset
    // caches) and compare the rest.
    let warm = &samples[1..];
    let first_rss = warm.first().unwrap().rss_kb;
    let last_rss = warm.last().unwrap().rss_kb;
    let max_rss = warm.iter().map(|s| s.rss_kb).max().unwrap();
    println!(
        "[test] RSS: first(post-warmup)={first_rss}KB last={last_rss}KB max={max_rss}KB over {:?}",
        soak_duration
    );
    assert!(
        last_rss < first_rss.saturating_mul(3).max(first_rss + 200_000),
        "RSS grew from {first_rss}KB to {last_rss}KB over {soak_duration:?} — more than 3x (or \
         +200MB) growth in this short a window suggests an obvious leak, not just steady-state \
         noise. NOTE: this is a coarse sanity bound, not a real leak detector — the real 24h soak \
         is authoritative for that."
    );

    // ---- tick time: per-window mean, checked for gross drift ----
    let mut window_means_ms = Vec::new();
    for pair in samples.windows(2) {
        let [a, b] = pair else { unreachable!() };
        let delta_count = b.tick_hist_count.saturating_sub(a.tick_hist_count);
        let delta_sum = b.tick_hist_sum_secs - a.tick_hist_sum_secs;
        if delta_count > 0 && delta_sum >= 0.0 {
            window_means_ms.push(delta_sum / delta_count as f64 * 1000.0);
        }
    }
    assert!(
        !window_means_ms.is_empty(),
        "no tick-time windows could be computed from the sampled histogram deltas"
    );
    let first_mean = window_means_ms[0];
    let last_mean = *window_means_ms.last().unwrap();
    let max_mean = window_means_ms.iter().cloned().fold(f64::MIN, f64::max);
    println!(
        "[test] tick time (mean ms/tick per sample window): first={first_mean:.2}ms \
         last={last_mean:.2}ms max={max_mean:.2}ms across {} windows (ideal @30Hz = 33.3ms)",
        window_means_ms.len()
    );
    assert!(
        max_mean < 200.0,
        "worst per-window mean tick time was {max_mean:.2}ms — over 6x the ideal 33.3ms @30Hz \
         budget, suggesting the shell is not soak-ready at a basic level"
    );
    assert!(
        last_mean < first_mean * 4.0 + 20.0,
        "per-window mean tick time drifted from {first_mean:.2}ms to {last_mean:.2}ms over \
         {soak_duration:?} — more than a 4x-plus-20ms rise in this short a window suggests active \
         drift, not just scheduling noise. NOTE: this is a coarse sanity bound; the real 24h soak \
         is authoritative for genuine drift detection."
    );

    println!(
        "[test] soak-readiness sanity check passed over {soak_duration:?}: RSS \
         {first_rss}KB->{last_rss}KB, tick time {first_mean:.2}ms->{last_mean:.2}ms"
    );
}
