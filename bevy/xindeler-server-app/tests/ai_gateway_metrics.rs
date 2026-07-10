//! BL-82 EM-4.2e acceptance: the AI-gateway seam's 2 zero-value Prometheus
//! counters (`ai_gateway_requests_total`, `ai_gateway_fallback_total`) are
//! visible at the SAME real `/metrics` endpoint EM-4.1 already exposes
//! (`soak_readiness.rs` scrapes `tick_time_hist` off this identical
//! passthrough; this test scrapes the 2 new counters instead), reading `0`
//! since nothing in this task ever increments them (no real AI call exists
//! yet — see `xindeler_oracle_host::ai_gateway`'s module doc comment).
//!
//! Needs real assets + LFS and spawns a real child process, so — like the
//! other full-world tests in this crate — this is `#[ignore]`d; run locally
//! with: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app
//! --test ai_gateway_metrics -- --ignored --nocapture`

mod support;

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use support::{graceful_shutdown, seed_settings, spawn_server_app_with_env, wait_for_listener};

/// Fetches `/metrics` over a raw HTTP/1.1 GET (same no-http-client-dependency
/// approach `soak_readiness.rs`'s `sample_tick_hist` uses) and looks up a
/// single bare Prometheus counter line (`"<name> <value>"`, no labels).
fn sample_counter(metrics_addr: SocketAddr, name: &str) -> Option<f64> {
    let mut stream = TcpStream::connect_timeout(&metrics_addr, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET /metrics HTTP/1.1\r\nHost: {metrics_addr}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut body = String::new();
    stream.read_to_string(&mut body).ok()?;

    let prefix = format!("{name} ");
    body.lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .and_then(|value| value.trim().parse::<f64>().ok())
}

#[test]
#[ignore = "spawns a real xindeler-server-app process: needs assets + LFS; run locally with \
            VELOREN_ASSETS"]
fn ai_gateway_counters_visible_and_zero() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let game_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let metrics_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port for metrics");
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], metrics_port).into();

    let mut child = spawn_server_app_with_env(data_dir.path(), "ai-gateway-metrics", [(
        "XINDELER_SERVER_METRICS_ADDR",
        metrics_addr.to_string(),
    )]);

    wait_for_listener(game_addr, Duration::from_secs(60), "ai-gateway-metrics");

    // Give the metrics HTTP server (spawned a moment after the game
    // listener) a short grace period, mirroring `soak_readiness.rs`.
    let deadline = Instant::now() + Duration::from_secs(30);
    let (requests_total, fallback_total) = loop {
        match (
            sample_counter(metrics_addr, "ai_gateway_requests_total"),
            sample_counter(metrics_addr, "ai_gateway_fallback_total"),
        ) {
            (Some(requests), Some(fallback)) => break (requests, fallback),
            _ => {
                assert!(
                    Instant::now() < deadline,
                    "ai_gateway_{{requests,fallback}}_total never appeared at \
                     http://{metrics_addr}/metrics within the boot deadline"
                );
                std::thread::sleep(Duration::from_millis(500));
            },
        }
    };

    assert_eq!(
        requests_total, 0.0,
        "ai_gateway_requests_total must read 0 — no real AI-gateway caller exists yet"
    );
    assert_eq!(
        fallback_total, 0.0,
        "ai_gateway_fallback_total must read 0 — no real AI-gateway caller exists yet"
    );

    graceful_shutdown(&mut child, Duration::from_secs(15), "ai-gateway-metrics");
}
