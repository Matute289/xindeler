//! BL-82 EM-4.9 follow-up — the real acceptance test for the player-transfer
//! gap EM-4.9's own backlog row documented as deferred: "no live
//! player-transfer trigger moving a connected player into the event
//! dimension". Composes the SAME harnesses `e2e_mist_bound_drill.rs` (the
//! ORACLE ingestion chain) and `replicon_login_handshake.rs` (a REAL,
//! persisted, logged-in replicon session) already prove independently, plus
//! this follow-up's own `XINDELER_DEBUG_TRANSFER_PLAYER_DIMENSION` debug
//! lever (`xindeler-server-app::dimensions`) for a DETERMINISTIC trigger —
//! see that field's own doc comment for why a black-box subprocess test can't
//! reliably arrange "the player is physically standing inside the event's
//! proximity zone" without it (the mist-bound dimension's own origin/spawn
//! point coincidence is plausible but not guaranteed in every generated test
//! world, and this drill needs to be non-flaky).
//!
//! What this proves, end to end, with a REAL logged-in connected player (not
//! just NPCs, which `e2e_mist_bound_drill.rs` already covers):
//! 1. A real, persisted, replicon-authenticated character (created via the
//!    legacy path, logged in over the new transport — the EM-4.2c handshake) is
//!    genuinely transferred into the Mist-Bound event's dimension once it goes
//!    `Active` (`xindeler_sim_bridge::player_transfer::
//!    apply_player_dimension_transfers`, triggered here via the debug lever
//!    rather than proximity — the MECHANISM under test is identical either way,
//!    only the trigger differs).
//! 2. The event is retired (file deleted) while the player is still inside it —
//!    the player is ejected back to `DimensionId::DEFAULT`
//!    (`eject_players_before_dimension_teardown`) BEFORE the dimension actually
//!    tears down, so the cascade-despawn that destroys the event's minions does
//!    NOT destroy the player's own session.
//! 3. The replicon client's connection survives the WHOLE drill — proving the
//!    transfer + eject-on-teardown sequence never corrupts or disconnects a
//!    live session, unlike a naive "just remove the occupant" approach would
//!    (see `player_transfer`'s own module doc comment).
//!
//! Needs real assets + LFS; run locally with:
//! ```text
//! VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test \
//!     player_dimension_transfer_drill -- --ignored --nocapture
//! ```

mod support;

use std::{
    io::{BufRead, Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use bevy::{
    MinimalPlugins,
    app::App,
    ecs::message::Messages,
    state::{app::StatesPlugin, state::State},
};
use bevy_replicon::prelude::{ClientState, RepliconPlugins};
use common::{character::CharacterId, clock::Clock};
use support::{ChildGuard, TPS, client_runtime, connect_client, create_character, logout};
use xindeler_protocol::{LoginRequest, LoginResult, XindelerProtocolPlugin};
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

const USERNAME: &str = "em49_transfer_drill_bot";
const ALIAS: &str = "Em49TransferHero";

/// Same capturing-spawn shape `e2e_mist_bound_drill.rs` pioneered (that
/// file's own module doc comment explains why each test file grows its own
/// thin variant rather than sharing one from `support/mod.rs`).
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

/// Boots a headless (no window/GPU) `bevy_replicon` CLIENT app wired exactly
/// like `xindeler-client::net_client::NetClientPlugin`'s wire-protocol half
/// (client role + `XindelerProtocolPlugin` + the real `QuinnetTransport`) —
/// mirrors `replicon_login_handshake.rs`'s own `login_client_app` (duplicated
/// rather than shared, per this crate's "each test file grows its own thin
/// variant" convention for local test-only App builders).
fn login_client_app(server_addr: SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins,
        XindelerProtocolPlugin,
    ));
    app.add_plugins(QuinnetTransport.client_plugins(&TransportConfig::client(server_addr)));
    app.finish();
    app
}

fn wait_for_connected(app: &mut App, deadline: Instant, tag: &'static str) {
    loop {
        app.update();
        if *app.world().resource::<State<ClientState>>().get() == ClientState::Connected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{tag} never reached ClientState::Connected within the deadline"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_login_result(app: &mut App, deadline: Instant, tag: &'static str) -> LoginResult {
    loop {
        app.update();
        if let Some(result) = app
            .world_mut()
            .resource_mut::<Messages<LoginResult>>()
            .drain()
            .next()
        {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "{tag} never received a LoginResult within the deadline"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// Fetches `/metrics` and looks up a single bare Prometheus gauge line, same
/// approach `e2e_mist_bound_drill.rs`/`dimension_lifecycle.rs` use.
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
#[ignore = "spawns a real xindeler-server-app process + a real headless replicon login client + \
            boots a real world + spins up a second real dimension: needs assets + LFS; see this \
            file's module doc comment for the exact run command"]
fn a_real_logged_in_player_transfers_into_the_mist_bound_dimension_and_is_ejected_cleanly_on_retire()
 {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = support::seed_settings(data_dir.path());
    let legacy_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let replicon_port = portpicker::pick_unused_port().expect("free port");
    let replicon_addr: SocketAddr = ([127, 0, 0, 1], replicon_port).into();
    let metrics_port = portpicker::pick_unused_port().expect("free port");
    let metrics_addr: SocketAddr = ([127, 0, 0, 1], metrics_port).into();

    let oracle_dir = tempfile::tempdir().expect("tempdir");
    let oracle_root = oracle_dir
        .path()
        .canonicalize()
        .expect("canonicalize oracle tempdir (macOS /var symlink gotcha)");

    // `XINDELER_DEBUG_TRANSFER_PLAYER_DIMENSION=1`: deterministic, since
    // `xindeler_sim_bridge::oracle`'s `NextDimensionId` allocator always
    // starts at 1 and this drill only ever ingests ONE event — see
    // `DebugDimensionCommands::transfer_player`'s own doc comment for why a
    // debug lever (rather than relying on proximity) is what makes this
    // drill non-flaky.
    let (mut server, server_log) = spawn_server_app_capturing(data_dir.path(), "server-app", &[
        ("XINDELER_SERVER_REPLICON_ADDR", replicon_addr.to_string()),
        ("XINDELER_SERVER_METRICS_ADDR", metrics_addr.to_string()),
        (
            "XINDELER_ORACLE_EVENTS_DIR",
            oracle_root.to_string_lossy().into_owned(),
        ),
        ("XINDELER_DEBUG_TRANSFER_PLAYER_DIMENSION", "1".to_owned()),
    ]);

    support::wait_for_listener(legacy_addr, Duration::from_secs(180), "transfer-drill");
    wait_for_log_line(
        &server_log,
        "replicon/quinnet transport listening",
        Instant::now() + Duration::from_secs(60),
        &mut server,
        "server-app",
    );

    // ---- fixture: a REAL persisted character via the legacy path (same
    // pattern `replicon_login_handshake.rs` uses) ----
    let runtime = client_runtime("tokio-em49-transfer-legacy-client");
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut legacy_client =
        connect_client(&runtime, &mut server, game_port, USERNAME, connect_deadline);
    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
    let deadline = Instant::now() + Duration::from_secs(60);
    let character_id =
        create_character(&mut legacy_client, &mut clock, deadline, &mut server, ALIAS);
    logout(&mut legacy_client, &mut clock);
    drop(legacy_client);
    println!("[test] fixture ready: persisted character {character_id} (\"{ALIAS}\")");

    // ---- the real EM-4.2c handshake: log in over replicon+quinnet ----
    let mut client = login_client_app(replicon_addr);
    let client_deadline = Instant::now() + Duration::from_secs(60);
    wait_for_connected(&mut client, client_deadline, "replicon client");
    client.world_mut().write_message(LoginRequest {
        token_or_username: USERNAME.to_owned(),
        locale: "en-US".to_owned(),
    });
    let result = wait_for_login_result(&mut client, client_deadline, "replicon client");
    let success = result
        .outcome
        .unwrap_or_else(|e| panic!("expected a successful login, got {e:?}"));
    assert_eq!(
        success.selected,
        Some(CharacterId(character_id)),
        "the persisted character must have been auto-selected and loaded"
    );
    println!(
        "[test] replicon client logged in as character {character_id}; PlayerDimensionSession \
         should now be linked server-side"
    );

    // ---- drop the Mist-Bound event ----
    let dmevent_text =
        include_str!("../../../assets/xindeler/oracle_events/mist_bound.dmevent.ron");
    std::fs::write(
        oracle_dir.path().join("mist_bound.dmevent.ron"),
        dmevent_text,
    )
    .expect("write the fixture into the watched dir");
    println!("[test] dropped mist_bound.dmevent.ron");

    // ---- criterion: the event dimension activates and its minions spawn
    // (proves the SAME ORACLE chain `e2e_mist_bound_drill.rs` already
    // covers is unaffected by this follow-up) ----
    wait_for_log_line(
        &server_log,
        "mist-bound: spawned 15 minions into dimension",
        Instant::now() + Duration::from_secs(90),
        &mut server,
        "server-app",
    );
    println!("[test] the Mist-Bound dimension is Active and its minions spawned");

    // ---- criterion (the whole point of this drill): the debug lever fires
    // once dimension 1 is Active, and the REAL logged-in player transfers ----
    wait_for_log_line(
        &server_log,
        "player transferred to a new dimension",
        Instant::now() + Duration::from_secs(30),
        &mut server,
        "server-app",
    );
    println!("[test] the real logged-in player was transferred into the event dimension");

    // The replicon client must still be alive and connected — a transfer
    // must never disconnect/desync a live session.
    for _ in 0..5 {
        client.update();
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        *client.world().resource::<State<ClientState>>().get(),
        ClientState::Connected,
        "the replicon client must stay connected after being transferred"
    );

    // ---- retire the event while the player is still inside it ----
    std::fs::remove_file(oracle_dir.path().join("mist_bound.dmevent.ron"))
        .expect("delete the Mist-Bound fixture to retire the event");
    println!(
        "[test] retired the Mist-Bound event (deleted the file) while the player is inside it"
    );

    // ---- criterion: the player is ejected BACK to DimensionId::DEFAULT
    // before the dimension actually tears down ----
    wait_for_log_line(
        &server_log,
        "ejecting a real player back to DimensionId::DEFAULT",
        Instant::now() + Duration::from_secs(30),
        &mut server,
        "server-app",
    );
    println!("[test] the player was ejected back to DimensionId::DEFAULT ahead of teardown");

    // ---- criterion: clean teardown — the event dimension is genuinely
    // garbage-collected (not stuck, not double-despawning the player) ----
    wait_for_log_line(
        &server_log,
        "dimension torn down (GC complete)",
        Instant::now() + Duration::from_secs(30),
        &mut server,
        "server-app",
    );
    wait_for_gauge(
        metrics_addr,
        "dimension_lifecycle_active",
        Instant::now() + Duration::from_secs(10),
        "transfer-drill",
        |active| (active - 1.0).abs() < f64::EPSILON,
    );
    println!("[test] the Mist-Bound dimension torn down cleanly, back to baseline");

    // ---- criterion: the player's own session survived the whole thing —
    // still connected, server still up, no corruption/kick ----
    for _ in 0..5 {
        client.update();
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        *client.world().resource::<State<ClientState>>().get(),
        ClientState::Connected,
        "the player's replicon session must survive the event ending — the eject-before-teardown \
         ordering is what this whole drill exists to prove"
    );
    assert!(
        !server.0.try_wait().is_ok_and(|s| s.is_some()),
        "server-app process must not have exited during the drill"
    );

    support::graceful_shutdown(&mut server, Duration::from_secs(15), "transfer-drill");
}
