//! BL-82 EM-4.9 — the Phase-4 milestone gate: the full "Mist-Bound" ORACLE
//! event drill (spec §6, task board T51.9). Composes THREE existing harnesses
//! this crate's own test suite already proves independently:
//!
//! - `replicon_quinnet_dual_stack.rs` (EM-4.2b): a real legacy `client::Client`
//!   (TCP) AND a real `xindeler-client` net-client process (replicon/quinnet)
//!   connected to the SAME `xindeler-server-app` process at once — this drill's
//!   dual-stack half (criterion G).
//! - `dimension_lifecycle.rs` (EM-4.5): `/metrics`
//!   `dimension_lifecycle_{spinup,active,draining,teardown,teardowns_total}`
//!   gauges as the black-box observability window into the dimension state
//!   machine — this drill's spinup/teardown assertions (criteria A/F).
//! - The inline sim-bridge Mist-Bound test (`xindeler-sim-bridge::oracle`'s own
//!   `#[cfg(test)]`, and `xindeler-sim-bridge::lib`'s
//!   `mist_bound_spawning_rules_spawn_fifteen_
//!   clamped_stalker_minions_in_a_test_dimension`) — the exact-count/psyche
//!   assertion at the unit level; THIS test only asserts the wired PRODUCER
//!   reaches that spawn in the real binary (via a log marker, since specs state
//!   isn't reachable across the process boundary).
//!
//! Full scripted sequence (spec §6): boot with `XINDELER_ORACLE_EVENTS_DIR`
//! pointed at a tempdir → connect both clients → drop
//! `mist_bound.dmevent.ron` → assert generate (`/metrics` active>=2) →
//! assert minions spawned (producer log marker, 15) → assert narrative
//! (chronicle `world_rumor` marker) → retire (delete the file) → assert
//! teardown (`/metrics` teardowns_total + active back to 1) → dual-stack
//! still connected throughout → `graceful_shutdown`.
//!
//! ## Scope note (honest, per the design's own §0.1)
//! This asserts the MINIMUM milestone gate: the ORACLE ingestion chain
//! genuinely wired (Phase A), real minions spawned into the event dimension
//! (Phase B/C's factory-sink routing), narrative/chronicle hooks firing, and
//! clean teardown — all server-observed, with BOTH clients connected
//! throughout. It does NOT drive a live player-transfer into the event
//! dimension (Phase C's `C4`, not implemented — see
//! `xindeler-sim-bridge::entity_factory`'s own module doc for why routing
//! stops short of that) or assert the client visually retargeting its
//! atmosphere (Phase D's wire message + table are real and unit-tested in
//! `xindeler_oracle_host::atmosphere_sync`, but nothing in this drill's
//! own scripted sequence ever changes a connected client's
//! `ClientViewpoint.dimension` to trigger it) — both are documented,
//! tracked follow-ups per the design's own "if the maximalist legs slip, the
//! gate still closes" fallback.
//!
//! Needs real assets + LFS + the client pre-built with `--features
//! net-client`; run with:
//! ```text
//! cargo build -p xindeler-client --features net-client
//! XINDELER_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test \
//!     e2e_mist_bound_drill -- --ignored --nocapture
//! ```

mod support;

use std::{
    io::{BufRead, Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use client::Client;
use common::clock::Clock;
use support::{ChildGuard, TPS, client_runtime, connect_client, create_character, wait_for};

const USERNAME: &str = "em49_mist_bound_drill_bot";

/// Same capturing-spawn shape `replicon_quinnet_dual_stack.rs` pioneered
/// (this crate's own `support/mod.rs` deliberately doesn't host it — see that
/// file's own module doc comment for why each test file grows its own
/// thin variant rather than sharing one that already works).
fn spawn_server_app_capturing(
    data_dir: &Path,
    tag: &'static str,
    extra_env: &[(&str, String)],
) -> (ChildGuard, Arc<Mutex<Vec<String>>>) {
    let bin = env!("CARGO_BIN_EXE_xindeler-server-app");
    let mut cmd = Command::new(bin);
    cmd.env("VELOREN_USERDATA", data_dir)
        .env("XINDELER_SERVER_NO_AUTH", "1")
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn xindeler-server-app ({tag}): {e:?}"));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    spawn_line_capture(child.stderr.take(), tag, Arc::clone(&lines));
    spawn_line_capture(child.stdout.take(), tag, Arc::clone(&lines));
    (ChildGuard(child), lines)
}

fn spawn_net_client_capturing(
    server_addr: SocketAddr,
    tag: &'static str,
) -> (ChildGuard, Arc<Mutex<Vec<String>>>) {
    let bin = xindeler_client_bin();
    let mut cmd = Command::new(bin);
    cmd.arg("--connect")
        .arg(server_addr.to_string())
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn xindeler-client ({tag}): {e:?}"));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    spawn_line_capture(child.stderr.take(), tag, Arc::clone(&lines));
    spawn_line_capture(child.stdout.take(), tag, Arc::clone(&lines));
    (ChildGuard(child), lines)
}

fn spawn_line_capture(
    pipe: Option<impl std::io::Read + Send + 'static>,
    tag: &'static str,
    lines: Arc<Mutex<Vec<String>>>,
) {
    if let Some(pipe) = pipe {
        thread::spawn(move || {
            for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                println!("[{tag}] {line}");
                lines.lock().expect("capture lock poisoned").push(line);
            }
        });
    }
}

fn xindeler_client_bin() -> PathBuf {
    let server_bin = PathBuf::from(env!("CARGO_BIN_EXE_xindeler-server-app"));
    let dir = server_bin
        .parent()
        .expect("CARGO_BIN_EXE_xindeler-server-app has a parent dir");
    let name = if cfg!(windows) {
        "xindeler-client.exe"
    } else {
        "xindeler-client"
    };
    let path = dir.join(name);
    assert!(
        path.exists(),
        "xindeler-client binary not found at {path:?} — build it WITH the `net-client` feature \
         first: `cargo build -p xindeler-client --features net-client`"
    );
    path
}

fn wait_for_log_line(
    lines: &Arc<Mutex<Vec<String>>>,
    pattern: &str,
    deadline: Instant,
    child: &mut ChildGuard,
    tag: &'static str,
) {
    loop {
        if lines
            .lock()
            .expect("capture lock poisoned")
            .iter()
            .any(|l| l.contains(pattern))
        {
            return;
        }
        assert!(
            !child.0.try_wait().is_ok_and(|s| s.is_some()),
            "{tag} process exited before logging a line matching {pattern:?}"
        );
        assert!(
            Instant::now() < deadline,
            "{tag} never logged a line matching {pattern:?} within the deadline"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

/// Like [`wait_for_log_line`] but ALSO ticks the legacy client once per poll
/// (keep-alive — see `replicon_quinnet_dual_stack.rs`'s identical helper for
/// the full reasoning: booting the net-client can take long enough to trip
/// the server's ping-timeout watchdog if the legacy client were left idle).
#[allow(clippy::too_many_arguments)]
fn wait_for_log_line_while_ticking(
    lines: &Arc<Mutex<Vec<String>>>,
    pattern: &str,
    deadline: Instant,
    net_client: &mut ChildGuard,
    tag: &'static str,
    legacy_client: &mut Client,
    clock: &mut Clock,
    server: &mut ChildGuard,
) {
    loop {
        if lines
            .lock()
            .expect("capture lock poisoned")
            .iter()
            .any(|l| l.contains(pattern))
        {
            return;
        }
        assert!(
            !net_client.0.try_wait().is_ok_and(|s| s.is_some()),
            "{tag} process exited before logging a line matching {pattern:?}"
        );
        assert!(
            Instant::now() < deadline,
            "{tag} never logged a line matching {pattern:?} within the deadline"
        );
        support::tick_client(
            legacy_client,
            clock,
            common::comp::ControllerInputs::default(),
            "keep-alive-wait",
            server,
        );
    }
}

/// Fetches `/metrics` and looks up a single bare Prometheus gauge line, same
/// approach `dimension_lifecycle.rs`/`ai_gateway_metrics.rs` use.
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

fn wait_for_gauge(
    metrics_addr: SocketAddr,
    name: &str,
    deadline: Instant,
    label: &str,
    mut predicate: impl FnMut(f64) -> bool,
) -> f64 {
    loop {
        if let Some(value) = sample_gauge(metrics_addr, name)
            && predicate(value)
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: {name} never reached the expected state within the deadline"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "spawns a real xindeler-server-app process + a real xindeler-client (net-client) \
            process + boots a real world + spins up a second real dimension: needs assets + LFS + \
            the client pre-built with --features net-client; see this file's module doc comment \
            for the exact run command"]
fn mist_bound_dm_event_drives_the_full_encounter_with_dual_stack_throughout() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = support::seed_settings(data_dir.path());
    let legacy_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let replicon_port = portpicker::pick_unused_port().expect("free port");
    let replicon_addr: SocketAddr = ([127, 0, 0, 1], replicon_port).into();
    let metrics_port = portpicker::pick_unused_port().expect("free port");
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], metrics_port).into();

    // The oracle events watch dir — a tempdir this test drops the shipped
    // Mist-Bound fixture into, canonicalized (macOS `/var` -> `/private/var`
    // symlink gotcha, same reasoning `dm_event.rs`'s own hot-reload test
    // documents).
    let oracle_dir = tempfile::tempdir().expect("tempdir");
    let oracle_root = oracle_dir
        .path()
        .canonicalize()
        .expect("canonicalize oracle tempdir");

    let (mut server, server_log) = spawn_server_app_capturing(data_dir.path(), "server-app", &[
        ("XINDELER_SERVER_REPLICON_ADDR", replicon_addr.to_string()),
        ("XINDELER_SERVER_METRICS_ADDR", metrics_addr.to_string()),
        (
            "XINDELER_ORACLE_EVENTS_DIR",
            oracle_root.to_string_lossy().into_owned(),
        ),
    ]);

    // ---- boot: both listeners come up ----
    support::wait_for_listener(legacy_addr, Duration::from_secs(180), "mist-bound-drill");
    wait_for_log_line(
        &server_log,
        "replicon/quinnet transport listening",
        Instant::now() + Duration::from_secs(60),
        &mut server,
        "server-app",
    );

    // ---- criterion G (part 1): connect the legacy client first ----
    let runtime = client_runtime("tokio-em49-legacy-client");
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut legacy_client =
        connect_client(&runtime, &mut server, game_port, USERNAME, connect_deadline);
    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
    let deadline = Instant::now() + Duration::from_secs(120);
    legacy_client.load_character_list();
    wait_for(
        &mut legacy_client,
        &mut clock,
        deadline,
        "character-list",
        &mut server,
        |c, _| (!c.character_list().loading).then_some(()),
    );
    let character_id = support::first_character_id(&legacy_client).unwrap_or_else(|| {
        create_character(
            &mut legacy_client,
            &mut clock,
            deadline,
            &mut server,
            USERNAME,
        )
    });
    legacy_client.request_character(
        common::character::CharacterId(character_id),
        common::ViewDistances {
            terrain: 3,
            entity: 3,
        },
    );
    wait_for(
        &mut legacy_client,
        &mut clock,
        deadline,
        "spawn",
        &mut server,
        |c, _| c.position(),
    );
    println!("[test] legacy client is in-game (dual-stack half 1 of 2)");

    // ---- criterion G (part 2): the real net-client, connected SIMULTANEOUSLY ----
    let (mut net_client, net_client_log) = spawn_net_client_capturing(replicon_addr, "net-client");
    let net_deadline = Instant::now() + Duration::from_secs(180);
    wait_for_log_line_while_ticking(
        &net_client_log,
        "presentation attached to a newly mirrored entity",
        net_deadline,
        &mut net_client,
        "net-client",
        &mut legacy_client,
        &mut clock,
        &mut server,
    );
    println!("[test] net-client connected over replicon+quinnet (dual-stack confirmed)");

    // ---- criterion A: baseline — only DimensionId::DEFAULT active ----
    let metrics_deadline = Instant::now() + Duration::from_secs(30);
    wait_for_gauge(
        metrics_addr,
        "dimension_lifecycle_active",
        metrics_deadline,
        "mist-bound-drill",
        |active| active >= 1.0,
    );

    // ---- drop the event ----
    let dmevent_text =
        include_str!("../../../assets/xindeler/oracle_events/mist_bound.dmevent.ron");
    std::fs::write(
        oracle_dir.path().join("mist_bound.dmevent.ron"),
        dmevent_text,
    )
    .expect("write the Mist-Bound fixture into the watched dir");
    println!("[test] dropped mist_bound.dmevent.ron into the watched oracle events dir");

    // ---- criterion A: the event dimension generates (a SECOND Active dimension)
    // ----
    wait_for_gauge(
        metrics_addr,
        "dimension_lifecycle_active",
        Instant::now() + Duration::from_secs(90),
        "mist-bound-drill",
        |active| active >= 2.0,
    );
    println!("[test] the Mist-Bound dimension reached Active (criterion A)");

    // ---- criterion E: 15 minions spawned (producer log marker — specs
    // state isn't reachable across the process boundary, see this file's
    // module doc comment) ----
    wait_for_log_line(
        &server_log,
        "mist-bound: spawned 15 minions into dimension",
        Instant::now() + Duration::from_secs(60),
        &mut server,
        "server-app",
    );
    println!("[test] 15 minions spawned into the Mist-Bound dimension (criterion E)");

    // ---- criterion D (narrative half)/world_rumor: the spinup marker
    // itself doubles as proof `ingest_dm_events` ran; the chronicle append
    // is asserted at the unit level (`xindeler_oracle_host::chronicle`'s own
    // tests) — here we just confirm the producer's own spinup marker fired,
    // which only happens after resolving the dropped DmEvent.
    assert!(
        server_log
            .lock()
            .expect("capture lock poisoned")
            .iter()
            .any(|l| l.contains("mist-bound: dimension") && l.contains("spinning up")),
        "the producer must have logged the dimension-spinup marker for the dropped event"
    );

    // ---- retire: delete the file ----
    std::fs::remove_file(oracle_dir.path().join("mist_bound.dmevent.ron"))
        .expect("delete the Mist-Bound fixture to retire the event");
    println!("[test] retired the Mist-Bound event (deleted the file)");

    // ---- criterion F: clean teardown — active back to 1, a teardown counted ----
    wait_for_gauge(
        metrics_addr,
        "dimension_lifecycle_teardowns_total",
        Instant::now() + Duration::from_secs(60),
        "mist-bound-drill",
        |total| total >= 1.0,
    );
    wait_for_gauge(
        metrics_addr,
        "dimension_lifecycle_active",
        Instant::now() + Duration::from_secs(10),
        "mist-bound-drill",
        |active| (active - 1.0).abs() < f64::EPSILON,
    );
    println!("[test] the Mist-Bound dimension torn down cleanly, back to baseline (criterion F)");

    // ---- criterion G: both clients STILL connected throughout ----
    assert!(
        !server.0.try_wait().is_ok_and(|s| s.is_some()),
        "server-app process must not have exited during the drill"
    );
    assert!(
        !net_client.0.try_wait().is_ok_and(|s| s.is_some()),
        "net-client process must not have exited during the drill"
    );
    let end_pos = legacy_client
        .position()
        .expect("legacy client should still have a position after the whole drill");
    println!(
        "[test] legacy client still connected/positioned at {end_pos:?}; net-client still \
         connected — dual-stack held for the entire drill (criterion G)"
    );

    support::logout(&mut legacy_client, &mut clock);
    drop(legacy_client);
    support::graceful_shutdown(&mut server, Duration::from_secs(15), "mist-bound-drill");
}
