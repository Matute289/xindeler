//! BL-82 EM-4.5 acceptance: the debug/admin dimension-spinup/drain triggers
//! (`XINDELER_DEBUG_SPINUP_DIMENSION`/`XINDELER_DEBUG_DRAIN_DIMENSION`) drive
//! a REAL second dimension through the lifecycle inside a REAL, separately
//! spawned `xindeler-server-app` process, observed the same black-box way
//! `ai_gateway_metrics.rs` observes the AI-gateway seam: scraping the real
//! `/metrics` HTTP endpoint (no internal introspection of the process).
//!
//! Two scenarios, split into two tests (rather than one script driving both
//! through env vars set at boot) specifically to avoid a race: this shell's
//! env-var triggers are read ONCE at boot (there is no live admin RPC
//! channel to send a *second*, differently-timed command into an already-
//! running process — see `dimensions.rs`'s module doc), so proving BOTH "a
//! brand-new dimension reaches `Active`" AND "then gets drained" in one
//! process risks sampling `/metrics` in the split-second window between the
//! two (the drain fires the same tick `Active` is first observed, since the
//! debug-triggered dimension never has any tracked occupant — see
//! `DimensionRegistry::begin_draining`'s doc comment for why an
//! occupant-less `Draining` skips straight to `Teardown`). Each test below
//! instead polls for a STABLE end state that nothing in this task un-does.
//!
//! Needs real assets + LFS and spawns a real child process, so — like the
//! other full-world tests in this crate — this is `#[ignore]`d; run locally
//! with: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app
//! --test dimension_lifecycle -- --ignored --nocapture`

mod support;

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use support::{graceful_shutdown, seed_settings, spawn_server_app_with_env, wait_for_listener};

/// Fetches `/metrics` over a raw HTTP/1.1 GET (same approach
/// `ai_gateway_metrics.rs`/`soak_readiness.rs` use) and looks up a single
/// bare Prometheus gauge line (`"<name> <value>"`, no labels).
fn sample_gauge(metrics_addr: SocketAddr, name: &str) -> Option<f64> {
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

/// Polls all 4 lifecycle-count gauges until `predicate` is satisfied or
/// `deadline` passes (panicking with the last-seen counts on timeout).
fn wait_for_dimension_counts(
    metrics_addr: SocketAddr,
    deadline: Instant,
    label: &str,
    mut predicate: impl FnMut(i64, i64, i64, i64) -> bool,
) -> (i64, i64, i64, i64) {
    loop {
        let counts = (
            sample_gauge(metrics_addr, "dimension_lifecycle_spinup"),
            sample_gauge(metrics_addr, "dimension_lifecycle_active"),
            sample_gauge(metrics_addr, "dimension_lifecycle_draining"),
            sample_gauge(metrics_addr, "dimension_lifecycle_teardown"),
        );
        if let (Some(spinup), Some(active), Some(draining), Some(teardown)) = counts {
            let (spinup, active, draining, teardown) = (
                spinup as i64,
                active as i64,
                draining as i64,
                teardown as i64,
            );
            if predicate(spinup, active, draining, teardown) {
                return (spinup, active, draining, teardown);
            }
            assert!(
                Instant::now() < deadline,
                "{label}: dimension_lifecycle_{{spinup,active,draining,teardown}} never reached \
                 the expected state within the boot deadline (last seen: spinup={spinup} \
                 active={active} draining={draining} teardown={teardown})"
            );
        } else {
            assert!(
                Instant::now() < deadline,
                "{label}: dimension_lifecycle_* gauges never appeared at http://{metrics_addr}/metrics \
                 within the boot deadline"
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `DimensionId::DEFAULT` always shows as `Active` with nothing else running
/// (EM-4.5's own "wrapping refactor, not a behavior change" acceptance bar,
/// made observable via `/metrics`).
#[test]
#[ignore = "spawns a real xindeler-server-app process: needs assets + LFS; run locally with \
            VELOREN_ASSETS"]
fn default_dimension_is_active_with_no_debug_commands_set() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let game_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let metrics_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port for metrics");
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], metrics_port).into();

    let mut child = spawn_server_app_with_env(data_dir.path(), "dim-default-only", [(
        "XINDELER_SERVER_METRICS_ADDR",
        metrics_addr.to_string(),
    )]);
    wait_for_listener(game_addr, Duration::from_secs(60), "dim-default-only");

    let deadline = Instant::now() + Duration::from_secs(30);
    let (spinup, active, draining, teardown) = wait_for_dimension_counts(
        metrics_addr,
        deadline,
        "dim-default-only",
        |_, active, _, _| active >= 1,
    );
    assert_eq!(
        (spinup, active, draining, teardown),
        (0, 1, 0, 0),
        "with no debug commands set, only DimensionId::DEFAULT should exist, and it should be \
         Active"
    );

    graceful_shutdown(&mut child, Duration::from_secs(15), "dim-default-only");
}

/// The full acceptance bar: `XINDELER_DEBUG_SPINUP_DIMENSION` spins up a
/// SECOND, real, independently-generated dimension (proving `Spinup ->
/// Active` via the async task pool in a real process); once it reaches
/// `Active`, `XINDELER_DEBUG_DRAIN_DIMENSION` drains it — and because this
/// debug-triggered dimension never has a tracked occupant, `Draining`
/// immediately advances to `Teardown` (a real, tested transition — see
/// `DimensionRegistry::begin_draining`'s doc comment).
///
/// ## Why this polls `teardowns_total`, not the `teardown` gauge (EM-4.6
/// follow-up, found verifying the phase-4 wave-3 integration merge)
/// `xindeler_dimensions::teardown::teardown_completed_dimensions` removes a
/// torn-down dimension's registry entry in the SAME `Update` pass it
/// discovers it in `Teardown` (that module's own documented "immediate GC"
/// posture) — so the transient `dimension_lifecycle_teardown` GAUGE can
/// legitimately read 0 on every single scrape here: nothing keeps dimension
/// 1 "currently in Teardown" for longer than that one internal system pass,
/// and this test's own 200ms poll interval can trivially straddle it. The
/// durable, scrape-any-time-after signal is the monotonic
/// `dimension_lifecycle_teardowns_total` COUNTER (see `dimensions.rs`'s
/// `DimensionMetrics` doc comment) — this test waits on THAT, then confirms
/// the gauges have settled back to EXACTLY the same baseline
/// `default_dimension_is_active_with_no_debug_commands_set` asserts
/// (dimension 1 has fully vanished from the registry, not merely "currently
/// Teardown").
#[test]
#[ignore = "spawns a real xindeler-server-app process: needs assets + LFS; run locally with \
            VELOREN_ASSETS"]
fn debug_triggered_dimension_spins_up_then_drains_to_teardown() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let game_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let metrics_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port for metrics");
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], metrics_port).into();

    let mut child = spawn_server_app_with_env(data_dir.path(), "dim-spinup-drain", [
        ("XINDELER_SERVER_METRICS_ADDR", metrics_addr.to_string()),
        ("XINDELER_DEBUG_SPINUP_DIMENSION", "1".to_string()),
        ("XINDELER_DEBUG_DRAIN_DIMENSION", "1".to_string()),
    ]);
    wait_for_listener(game_addr, Duration::from_secs(60), "dim-spinup-drain");

    // Dimension 1's (tiny, x_lg=5/y_lg=5) real world generation runs on the
    // async task pool — give it real wall-clock time, well beyond what the
    // xindeler-dimensions crate's own equivalent test needed.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(total) = sample_gauge(metrics_addr, "dimension_lifecycle_teardowns_total")
            && total >= 1.0
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "dim-spinup-drain: dimension_lifecycle_teardowns_total never reached >= 1 within the \
             boot deadline (the debug-triggered dimension never finished tearing down)"
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    // The teardown-completion pass that bumped the counter above ALSO
    // removed dimension 1's registry entry in that same pass, so the gauges
    // should already read the settled baseline by the time we sample them —
    // still poll (bounded, short) rather than assert on a single sample, to
    // stay robust to any future scheduling change.
    let (spinup, active, draining, teardown) = wait_for_dimension_counts(
        metrics_addr,
        Instant::now() + Duration::from_secs(5),
        "dim-spinup-drain",
        |spinup, active, draining, teardown| (spinup, active, draining, teardown) == (0, 1, 0, 0),
    );
    assert_eq!(
        (spinup, active, draining, teardown),
        (0, 1, 0, 0),
        "expected the debug-triggered dimension to have fully vanished from the registry after \
         teardown — the SAME baseline default_dimension_is_active_with_no_debug_commands_set \
         asserts (dimension 0 Active alone; Draining skipped straight to Teardown since dimension \
         1 had zero occupants, and Teardown removes the registry entry in that same pass)"
    );

    graceful_shutdown(&mut child, Duration::from_secs(15), "dim-spinup-drain");
}
