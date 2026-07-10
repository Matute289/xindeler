//! BL-82 EM-4.2b acceptance test — the FIRST time this codebase proves TWO
//! independent, REAL network transports serve the SAME `xindeler-server-app`
//! process at once:
//!
//! 1. A real legacy `xindeler-client-core::Client` connects over the UNTOUCHED
//!    `xindeler_network` TCP listener (`gameserver_protocols`) and reaches an
//!    in-game character — same rigor as `dual_stack.rs` (EM-4.1), proving the
//!    legacy path is unperturbed by this change.
//! 2. In the SAME test run, on the SAME server process, a real
//!    `xindeler-client` process (built with the `net-client` cargo feature —
//!    see that crate's `net_client.rs`) connects over the NEW real
//!    replicon+quinnet transport (`xindeler-transport`) and receives at least
//!    one replicated entity (the wandering test NPCs
//!    `xindeler_sim_bridge::spawn_test_npcs` spawns) and at least one terrain
//!    chunk — confirmed by grepping its stdout for the two marker log lines
//!    `terrain_stream.rs`/`entity_view.rs` emit for exactly this purpose
//!    (EM-4.2b).
//!
//! This mirrors `dual_stack.rs`'s rigor (separate OS processes, real sockets,
//! no in-process loopback) but for the NEW transport, while proving the OLD
//! one is untouched — the literal EM-4.2b acceptance bar (spec §1.1).
//!
//! ## Prerequisites to run this test locally
//! - Real assets (`VELOREN_ASSETS`/`XINDELER_ASSETS` + the LFS map blobs) —
//!   same as every other full-world test in this crate.
//! - The `xindeler-client` binary must be pre-built WITH the `net-client`
//!   feature: `cargo build -p xindeler-client --features net-client`. Cargo
//!   does not build a sibling package's binary automatically for an integration
//!   test that lives in a DIFFERENT package (`CARGO_BIN_EXE_xindeler-client` is
//!   only set for tests inside `xindeler-client`'s own `tests/`); this test
//!   locates the already-built binary next to `xindeler-server-app`'s own (both
//!   land in the same workspace `target/<profile>/` directory).
//! - A real window/GPU adapter: the net-client is the FULL graphical
//!   `xindeler-client` binary (same as `--listen-server`'s own smoke runs),
//!   just pointed at a remote server instead of an embedded one.
//!
//! Run with:
//! ```text
//! cargo build -p xindeler-client --features net-client
//! XINDELER_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test \
//!     replicon_quinnet_dual_stack -- --ignored --nocapture
//! ```

mod support;

use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use client::Client;
use common::{ViewDistances, character::CharacterId, clock::Clock, comp};
use support::{
    ChildGuard, TPS, client_runtime, connect_client, create_character, first_character_id,
    seed_settings, wait_for, wait_for_listener,
};
use vek::Vec2;

const USERNAME: &str = "em42b_dual_transport_bot";

/// Spawns `xindeler-server-app`, capturing stdout/stderr into a shared,
/// greppable buffer (in addition to `println!`-ing every line — same visible
/// behavior as `support::spawn_server_app_with_env`). Capturing is needed
/// here (and not in that shared helper) because this test must confirm the
/// NEW replicon+quinnet endpoint is actually listening (a log line — see
/// `xindeler_transport::quinnet::start_server_endpoint`) before dialing it:
/// unlike the legacy TCP listener, there is no "connect succeeds" probe for a
/// UDP/QUIC endpoint from a plain client-side connect attempt.
fn spawn_server_app_capturing(
    data_dir: &Path,
    tag: &'static str,
    extra_env: &[(&str, String)],
) -> (ChildGuard, Arc<Mutex<Vec<String>>>) {
    let bin = env!("CARGO_BIN_EXE_xindeler-server-app");
    let mut cmd = Command::new(bin);
    cmd.env("VELOREN_USERDATA", data_dir)
        // Mirrors server-cli's `--no-auth` flag: this sandbox has no route to
        // the real auth server (same reasoning as `support::spawn_server_app`).
        .env("XINDELER_SERVER_NO_AUTH", "1")
        // `xindeler-server-app::main` calls `tracing_subscriber::fmt::init()`
        // with no explicit filter, which (confirmed empirically — its
        // default without `RUST_LOG` set shows NOTHING, not even `info!`)
        // needs `RUST_LOG` to surface anything at all, including the
        // "replicon/quinnet transport listening" marker this test greps for
        // below. Harmless/additive for every other assertion in this test.
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

/// Locates the pre-built `xindeler-client` binary (see this file's own module
/// doc comment for why it must be built out-of-band) and spawns it in
/// net-client mode (`--connect <server_addr>`), capturing its output the same
/// way [`spawn_server_app_capturing`] does.
fn spawn_net_client_capturing(
    server_addr: SocketAddr,
    tag: &'static str,
) -> (ChildGuard, Arc<Mutex<Vec<String>>>) {
    let bin = xindeler_client_bin();
    let mut cmd = Command::new(bin);
    cmd.arg("--connect")
        .arg(server_addr.to_string())
        // Bevy's `LogPlugin` (via `DefaultPlugins`) defaults to `Level::INFO`
        // and honors `RUST_LOG` when set — pinned explicitly here for the
        // same reason as `spawn_server_app_capturing`'s own `RUST_LOG`: this
        // test's markers must surface regardless of either binary's default
        // filter behavior.
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

/// Streams `pipe` line-by-line onto stdout (tagged, so `--nocapture`
/// interleaves every process's real log lines) AND into `lines`, so the test
/// can later grep for a marker with [`wait_for_log_line`].
fn spawn_line_capture(
    pipe: Option<impl std::io::Read + Send + 'static>,
    tag: &'static str,
    lines: Arc<Mutex<Vec<String>>>,
) {
    if let Some(pipe) = pipe {
        thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                println!("[{tag}] {line}");
                lines.lock().expect("capture lock poisoned").push(line);
            }
        });
    }
}

/// The compiled `xindeler-client` binary's path, derived from
/// `CARGO_BIN_EXE_xindeler-server-app`'s directory (every workspace binary
/// lands in the same `target/<profile>/` directory) rather than
/// `CARGO_BIN_EXE_xindeler-client` (unset here — that env var is only
/// populated for integration tests living inside `xindeler-client`'s OWN
/// `tests/`, a different package from this one). Asserts it exists with an
/// actionable message rather than failing with a confusing `spawn` I/O error.
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
         first: `cargo build -p xindeler-client --features net-client` (Cargo does not auto-build \
         a sibling package's binary for a DIFFERENT package's integration test — see this file's \
         module doc comment)"
    );
    path
}

/// Blocks until `lines` contains an entry matching `pattern`, or panics past
/// `deadline` (or if `child` exited first).
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

/// Like [`wait_for_log_line`], but ALSO ticks the LEGACY client once per
/// poll iteration (with `inputs`) instead of sleeping. Two real, independent
/// reasons this matters, discovered empirically while writing this test:
///
/// **Keep-alive**: a real client must keep ticking/pinging or the server's
/// own ping-timeout watchdog (`server/src/sys/msg/ping.rs`) disconnects it as
/// unresponsive. Booting the net-client (asset loading + window/GPU init +
/// the QUIC handshake) can take 10-30s — long enough to trip that watchdog
/// if the legacy client were left idle (i.e. merely `thread::sleep`ing) the
/// whole time, which would silently break the "both transports coexist" half
/// of this test's own final assertions.
///
/// **Fresh terrain generation**: the server's terrain-anchor persister
/// streams its INITIAL batch of chunks once, immediately at boot — long
/// before the (much slower-booting) net-client can possibly have connected.
/// `CompressedChunk`/`RemoveChunk` are one-shot `make_message_independent`
/// broadcasts with NO late-join replay (a known, documented limitation of
/// the v0 wire shape — interest management/join-in-progress terrain sync is
/// EM-4.2d's job, not this task's). So the net-client can only ever observe
/// a terrain chunk that is generated/broadcast AFTER it is already connected
/// — which means driving the legacy player to WALK (streaming NEW chunks
/// around its updated position) is the only reliable way this test can
/// produce one.
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
    inputs: comp::ControllerInputs,
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
            inputs.clone(),
            "keep-alive-wait",
            server,
        );
    }
}

#[test]
#[ignore = "spawns a real xindeler-server-app process + a real xindeler-client (net-client) \
            process + boots a real world: needs assets + LFS + the client pre-built with \
            --features net-client; see this file's module doc comment"]
fn legacy_and_replicon_quinnet_clients_connect_simultaneously() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let legacy_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let replicon_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let replicon_addr: SocketAddr = ([127, 0, 0, 1], replicon_port).into();
    let metrics_port = portpicker::pick_unused_port().expect("failed to find a free loopback port");

    let (mut server, server_log) = spawn_server_app_capturing(data_dir.path(), "server-app", &[
        ("XINDELER_SERVER_REPLICON_ADDR", replicon_addr.to_string()),
        (
            "XINDELER_SERVER_METRICS_ADDR",
            format!("127.0.0.1:{metrics_port}"),
        ),
    ]);

    // ---- both listeners come up ----
    wait_for_listener(legacy_addr, Duration::from_secs(180), "legacy");
    wait_for_log_line(
        &server_log,
        "replicon/quinnet transport listening",
        Instant::now() + Duration::from_secs(60),
        &mut server,
        "server-app",
    );
    println!(
        "[test] both the legacy TCP listener ({legacy_addr}) and the new replicon+quinnet \
         listener ({replicon_addr}) are up on the SAME server process — dual-stack confirmed at \
         the transport level"
    );

    // ---- legacy client: connect + reach an in-game character (untouched path)
    // ----
    let runtime = client_runtime("tokio-em42b-legacy-client");
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut legacy_client =
        connect_client(&runtime, &mut server, game_port, USERNAME, connect_deadline);
    println!(
        "[test] legacy xindeler-client-core::Client connected over the untouched TCP listener"
    );

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
    let character_id = first_character_id(&legacy_client).unwrap_or_else(|| {
        create_character(
            &mut legacy_client,
            &mut clock,
            deadline,
            &mut server,
            USERNAME,
        )
    });
    legacy_client.request_character(CharacterId(character_id), ViewDistances {
        terrain: 3,
        entity: 3,
    });
    let start_pos = wait_for(
        &mut legacy_client,
        &mut clock,
        deadline,
        "spawn",
        &mut server,
        |c, _| c.position(),
    );
    println!(
        "[test] legacy client spawned in-game at {start_pos:?} — the untouched legacy stack works \
         exactly as `dual_stack.rs` (EM-4.1) already proves"
    );

    // ---- net-client (EM-4.2b): connect over the NEW transport, SIMULTANEOUSLY
    // ----
    let (mut net_client, net_client_log) = spawn_net_client_capturing(replicon_addr, "net-client");
    let net_deadline = Instant::now() + Duration::from_secs(180);

    // Entities are genuine REPLICATED STATE (not one-shot messages), so
    // `bevy_replicon` full-state-syncs them to the net-client whenever it
    // connects, regardless of timing — this alone proves it. Idle-tick the
    // legacy client meanwhile (see the helper's doc comment for why).
    wait_for_log_line_while_ticking(
        &net_client_log,
        "presentation attached to a newly mirrored entity",
        net_deadline,
        &mut net_client,
        "net-client",
        &mut legacy_client,
        &mut clock,
        &mut server,
        comp::ControllerInputs::default(),
    );
    println!(
        "[test] net-client (replicon+quinnet) received >=1 replicated entity over the NEW \
         transport, while the legacy client was ALSO connected"
    );

    // Now walk the legacy player (see the helper's doc comment: terrain
    // chunks are one-shot broadcasts with no late-join replay, so a FRESH
    // chunk generated after the net-client is confirmed connected — above —
    // is the only reliable way to observe one over the new transport).
    wait_for_log_line_while_ticking(
        &net_client_log,
        "first terrain chunk(s) received over the network",
        net_deadline,
        &mut net_client,
        "net-client",
        &mut legacy_client,
        &mut clock,
        &mut server,
        comp::ControllerInputs {
            move_dir: Vec2::unit_y(),
            ..Default::default()
        },
    );
    println!(
        "[test] net-client (replicon+quinnet) ALSO received >=1 terrain chunk over the NEW \
         transport — true dual-stack, both transports live on one server process at once, \
         confirmed for both entity and terrain replication"
    );

    // ---- everything is still alive / connected ----
    assert!(
        !server.0.try_wait().is_ok_and(|s| s.is_some()),
        "server-app process must not have exited during the test"
    );
    assert!(
        !net_client.0.try_wait().is_ok_and(|s| s.is_some()),
        "net-client process must not have exited during the test"
    );
    let end_pos = legacy_client
        .position()
        .expect("legacy client should still have a position after the net-client connected");
    println!(
        "[test] legacy client still connected/positioned at {end_pos:?} after the net-client \
         joined — both transports coexisted on one server process for the whole test"
    );

    support::logout(&mut legacy_client, &mut clock);
    drop(legacy_client);
}
