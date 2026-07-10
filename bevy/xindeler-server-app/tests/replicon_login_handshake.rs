//! BL-82 EM-4.2c acceptance test (T47.5, spec §1.2) — the login/session
//! handshake over the NEW replicon+quinnet transport (EM-4.2b).
//!
//! A real, headless `bevy_replicon` CLIENT app (no window/GPU — this is a
//! plain `MinimalPlugins` App driving [`xindeler_protocol::LoginRequest`]/
//! [`xindeler_protocol::LoginResult`] over a REAL loopback UDP socket, not the
//! in-process test loopback `bevy_replicon::test_app` uses) connects to a
//! genuinely separate `xindeler-server-app` OS process and:
//!
//! 1. [`offline_login_reaches_persisted_character`] — logs in OFFLINE (derived-
//!    UUID, `--no-auth`) as an account that already has a REAL persisted
//!    character (created via the LEGACY TCP path first, mirroring
//!    `persistence_roundtrip.rs`'s own fixture pattern — the spec explicitly
//!    asks to reuse that shape), and confirms the reply is the SAME persisted
//!    character, auto-loaded, with the sim-side entity having reached
//!    `Presence::Character(id)` (the ONLY way `LoginResult::Ok`'s `selected`
//!    field is ever populated — see `login.rs`'s `handle_character_data`). It
//!    then logs in a SECOND replicon client for the SAME account and confirms
//!    the FIRST client gets disconnected (duplicate-login kick — both the
//!    sim-side entity AND the old replicon/quinnet connection, per `login.rs`'s
//!    `ActiveReplicaSessions`).
//! 2. [`online_mode_auth_error_round_trips`] — boots the server in ONLINE mode
//!    (a configured, but unreachable, `auth_server_address` — this sandbox has
//!    no route to a real `veloren/auth` instance) and confirms the ONLINE-mode
//!    branch of `LoginProvider::verify`/`login` (a REAL `authc::AuthClient`
//!    HTTP dispatch — a genuinely different code path from the offline mode's
//!    instant, synchronous resolution) is reached and its failure correctly
//!    round-trips as a typed `LoginResult::Err` over the real transport. This
//!    does NOT prove a successful online-mode LOGIN (no live auth service is
//!    available in this environment) — see that test's own doc comment for
//!    exactly what it does and doesn't prove.
//!
//! Needs real assets (`VELOREN_ASSETS`/`XINDELER_ASSETS` + the LFS map blobs)
//! and spawns real child processes, so — like this crate's other full-world
//! tests — both are `#[ignore]`d; run locally with:
//! ```text
//! VELOREN_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test \
//!     replicon_login_handshake -- --ignored --nocapture
//! ```

mod support;

use std::{
    net::SocketAddr,
    path::Path,
    process::{Command, Stdio},
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
use support::{
    ChildGuard, TPS, client_runtime, connect_client, create_character, logout, seed_settings,
    spawn_server_app_with_env, wait_for_listener,
};
use xindeler_protocol::{LoginRequest, LoginResult, XindelerProtocolPlugin};
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

const OFFLINE_USERNAME: &str = "em42c_login_bot";
const OFFLINE_ALIAS: &str = "Em42cLoginHero";

/// Boots a headless (no window/GPU) `bevy_replicon` CLIENT app wired EXACTLY
/// like `xindeler-client::net_client::NetClientPlugin` (client role +
/// `XindelerProtocolPlugin` + the real `QuinnetTransport`), minus the
/// rendering/presentation plugins that module also adds — this test only
/// needs the wire protocol, not a picture.
fn login_client_app(server_addr: SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins,
        XindelerProtocolPlugin,
    ));
    app.add_plugins(QuinnetTransport.client_plugins(&TransportConfig::client(server_addr)));
    // Mirrors `xindeler-protocol`'s own test harness (`new_app`, `lib.rs`):
    // `.finish()` must run before the first `.update()` drives `Startup`
    // (which is what actually dials the real UDP socket, via
    // `xindeler_transport::quinnet::OpenClientConnection`).
    app.finish();
    app
}

/// Loops `.update()` (with a small real-time sleep — this is a REAL socket,
/// not the in-process test loopback, so the handshake genuinely needs wall
/// time) until `app`'s `ClientState` reaches `Connected`, or panics past
/// `deadline`.
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

/// Loops `.update()` until a [`LoginResult`] message has arrived, or panics
/// past `deadline`.
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

#[test]
#[ignore = "spawns real xindeler-server-app + boots a real world: needs assets + LFS; run locally \
            with VELOREN_ASSETS (see this file's module doc comment)"]
fn offline_login_reaches_persisted_character() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let game_port = seed_settings(data_dir.path());
    let legacy_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let replicon_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let replicon_addr: SocketAddr = ([127, 0, 0, 1], replicon_port).into();

    let mut server = spawn_server_app_with_env(data_dir.path(), "server-app", [(
        "XINDELER_SERVER_REPLICON_ADDR",
        replicon_addr.to_string(),
    )]);
    wait_for_listener(legacy_addr, Duration::from_secs(180), "server-app");
    println!(
        "[test] server-app up: legacy TCP on {legacy_addr}, replicon+quinnet on {replicon_addr}"
    );

    // ---- fixture: a REAL persisted character via the LEGACY path, exactly
    // `persistence_roundtrip.rs`'s own pattern (spec §1.2: "reuse the EM-4.2
    // persistence-roundtrip test's fixture") ----
    let runtime = client_runtime("tokio-em42c-legacy-client");
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut legacy_client = connect_client(
        &runtime,
        &mut server,
        game_port,
        OFFLINE_USERNAME,
        connect_deadline,
    );
    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
    let deadline = Instant::now() + Duration::from_secs(60);
    let character_id = create_character(
        &mut legacy_client,
        &mut clock,
        deadline,
        &mut server,
        OFFLINE_ALIAS,
    );
    logout(&mut legacy_client, &mut clock);
    drop(legacy_client);
    println!(
        "[test] fixture ready: persisted character {character_id} (\"{OFFLINE_ALIAS}\") via the \
         legacy path, then logged out"
    );

    // ---- the actual EM-4.2c handshake: a real replicon client logs in over
    // the NEW transport, offline (derived-UUID) mode — the settings.ron
    // `seed_settings` writes has no `auth_server_address`, and
    // `spawn_server_app_with_env` always sets `XINDELER_SERVER_NO_AUTH=1` on
    // top of that, so `LoginProvider::verify` takes its `None` branch
    // (`derive_uuid`) ----
    let mut client = login_client_app(replicon_addr);
    let client_deadline = Instant::now() + Duration::from_secs(60);
    wait_for_connected(&mut client, client_deadline, "replicon client");
    println!("[test] replicon test client connected over the new transport");

    client.world_mut().write_message(LoginRequest {
        token_or_username: OFFLINE_USERNAME.to_owned(),
        locale: "en-US".to_owned(),
    });
    let result = wait_for_login_result(&mut client, client_deadline, "replicon client");
    let success = result
        .outcome
        .unwrap_or_else(|e| panic!("expected a successful offline-mode login, got {e:?}"));
    assert_eq!(
        success.characters.len(),
        1,
        "expected exactly the one persisted character, got: {:?}",
        success.characters
    );
    assert_eq!(success.characters[0].id, CharacterId(character_id));
    assert_eq!(success.characters[0].alias, OFFLINE_ALIAS);
    assert_eq!(
        success.selected,
        Some(CharacterId(character_id)),
        "the account's only character should have been auto-selected and loaded — this field is \
         ONLY populated after `update_character_data` succeeds server-side, i.e. \
         Presence::Character(id) was genuinely reached (see login.rs's handle_character_data)"
    );
    println!(
        "[test] replicon login succeeded: character {character_id} (\"{OFFLINE_ALIAS}\") \
         auto-loaded, Presence::Character reached server-side"
    );

    // ---- duplicate-login: a SECOND replicon client for the SAME account
    // should eventually succeed, and the FIRST client should get kicked (both
    // the sim-side entity AND the actual replicon/quinnet connection — see
    // `login.rs`'s `ActiveReplicaSessions`).
    //
    // "Eventually", not "immediately": kicking the FIRST client queues its
    // character for persistence (`handle_client_disconnect` →
    // `persist_entity` → `CharacterUpdater::add_pending_logout_update`) —
    // the SAME `has_pending_database_action` guard `login.rs`'s
    // `handle_character_list` checks before loading (mirroring
    // `character_screen.rs`'s own guard) can legitimately reject an
    // immediate retry with "a character update was already pending" until
    // that logout write completes. This is the EXACT legacy safety
    // mechanism `persist_entity`'s own doc comment describes ("the user
    // will be temporarily unable to log in during this period to avoid the
    // race condition of their login fetching their old data and
    // overwriting the data saved here") — a real client would retry after
    // seeing this, which is what this loop does. ----
    let mut second_client = login_client_app(replicon_addr);
    wait_for_connected(
        &mut second_client,
        client_deadline,
        "second replicon client",
    );
    let second_success = loop {
        second_client.world_mut().write_message(LoginRequest {
            token_or_username: OFFLINE_USERNAME.to_owned(),
            locale: "en-US".to_owned(),
        });
        let second_result = wait_for_login_result(
            &mut second_client,
            client_deadline,
            "second replicon client",
        );
        match second_result.outcome {
            Ok(success) => break success,
            Err(xindeler_protocol::LoginError::CharacterDataFailed(msg))
                if msg.contains("already pending") =>
            {
                println!(
                    "[test] second (duplicate) login attempt transiently rejected (\"{msg}\") — \
                     the first client's kick-triggered persistence hasn't completed yet; retrying"
                );
                assert!(
                    Instant::now() < client_deadline,
                    "second (duplicate) login never succeeded within the deadline"
                );
                thread::sleep(Duration::from_millis(200));
            },
            Err(e) => {
                panic!("expected the second (duplicate) login to eventually succeed, got {e:?}")
            },
        }
    };
    assert_eq!(
        second_success.selected,
        Some(CharacterId(character_id)),
        "the second login for the same account should reach the same persisted character"
    );
    println!("[test] second (duplicate) login for the same account succeeded");

    // Drive the FIRST client's own App a bit more so it can observe the
    // server's `DisconnectRequest` (a real, separate network round trip —
    // needs a few more ticks after the second login completed).
    let kick_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        client.update();
        if *client.world().resource::<State<ClientState>>().get() == ClientState::Disconnected {
            break;
        }
        assert!(
            Instant::now() < kick_deadline,
            "the FIRST replicon client was never disconnected after a duplicate login for the \
             same account logged in — duplicate-login kick did not reach the transport layer"
        );
        thread::sleep(Duration::from_millis(50));
    }
    println!(
        "[test] first replicon client was disconnected after the duplicate login — kick confirmed \
         at the transport layer, not just the sim layer"
    );

    assert!(
        !server.0.try_wait().is_ok_and(|s| s.is_some()),
        "server-app process must not have exited during the test"
    );
}

/// Boots `xindeler-server-app` in ONLINE mode: a configured
/// `auth_server_address` pointing at a loopback port with NO listener (this
/// sandbox has no route to a real `veloren/auth` instance), so
/// `LoginProvider::verify`'s `Some(auth_server)` branch runs for real — a
/// genuine `authc::AuthClient` HTTP dispatch over TLS/HTTP, spawned onto the
/// server's tokio runtime and polled across MULTIPLE ticks (a materially
/// different code path from the offline mode's synchronous, same-tick
/// resolution `offline_login_reaches_persisted_character` exercises).
///
/// ## What this proves, and what it does NOT
/// It proves: the online-mode branch is genuinely reached (not just
/// compiled), a real `authc::AuthClient` attempts a real connection, the
/// resulting failure is polled correctly by `login.rs`'s `advance_auth`
/// across ticks, and it round-trips as a typed `LoginResult::Err(LoginError::
/// Auth(_))` over the real replicon transport — end to end.
///
/// It does NOT prove a successful online-mode LOGIN: that needs a live
/// `veloren/auth` service issuing real tokens, which isn't available in this
/// environment. That gap is inherent to the external dependency, not this
/// task's implementation — `LoginProvider::verify`'s online branch itself is
/// untouched by BL-82 EM-4.2c (see `login_provider.rs`'s `login` doc comment:
/// only its ban-check `Client` parameter was widened).
#[test]
#[ignore = "spawns real xindeler-server-app + boots a real world: needs assets + LFS; run locally \
            with VELOREN_ASSETS (see this file's module doc comment)"]
fn online_mode_auth_error_round_trips() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let unreachable_auth_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let game_port = seed_online_mode_settings(
        data_dir.path(),
        &format!("http://127.0.0.1:{unreachable_auth_port}"),
    );
    let legacy_addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let replicon_port =
        portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let replicon_addr: SocketAddr = ([127, 0, 0, 1], replicon_port).into();

    let mut server =
        spawn_server_app_online_mode(data_dir.path(), "server-app-online", replicon_addr);
    wait_for_listener(legacy_addr, Duration::from_secs(180), "server-app-online");
    println!("[test] server-app (online mode) up on {legacy_addr}/{replicon_addr}");

    let mut client = login_client_app(replicon_addr);
    let deadline = Instant::now() + Duration::from_secs(60);
    wait_for_connected(&mut client, deadline, "replicon client");

    client.world_mut().write_message(LoginRequest {
        // `authc::AuthToken::from_str` just parses a `u64` — any digit
        // string is syntactically valid, so this actually reaches a real
        // HTTP connection attempt (to the unreachable port above) instead of
        // failing at local parsing.
        token_or_username: "123456789".to_owned(),
        locale: "en-US".to_owned(),
    });
    let result = wait_for_login_result(&mut client, deadline, "replicon client");
    match result.outcome {
        Err(xindeler_protocol::LoginError::Auth(msg)) => {
            println!(
                "[test] online-mode auth path exercised for real: LoginProvider::verify's \
                 Some(auth_server) branch dispatched a real authc::AuthClient HTTP request, which \
                 failed as expected (no live auth service in this sandbox) and round-tripped as a \
                 typed LoginResult::Err(LoginError::Auth(\"{msg}\"))"
            );
        },
        other => panic!(
            "expected LoginResult::Err(LoginError::Auth(_)) from the unreachable auth server, \
             got: {other:?}"
        ),
    }

    assert!(
        !server.0.try_wait().is_ok_and(|s| s.is_some()),
        "server-app process must not have exited during the test"
    );
}

/// Like `support::seed_settings`, but also sets `auth_server_address` —
/// local to this test file (not promoted to `support/mod.rs`) since it's the
/// only test exercising online mode; every other test in this crate wants
/// `--no-auth`.
fn seed_online_mode_settings(data_dir: &Path, auth_server_address: &str) -> u16 {
    let game_port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let query_port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let settings = server::settings::Settings {
        gameserver_protocols: vec![server::settings::Protocol::Tcp {
            address: ([127, 0, 0, 1], game_port).into(),
        }],
        query_address: Some(([127, 0, 0, 1], query_port).into()),
        auth_server_address: Some(auth_server_address.to_owned()),
        ..server::settings::Settings::default()
    };
    let settings_dir = data_dir.join("server").join("server_config");
    std::fs::create_dir_all(&settings_dir).expect("failed to create server_config dir");
    std::fs::write(
        settings_dir.join("settings.ron"),
        ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("failed to serialize test settings"),
    )
    .expect("failed to write settings.ron");
    game_port
}

/// Like `support::spawn_server_app_with_env`, but deliberately does NOT set
/// `XINDELER_SERVER_NO_AUTH` — this test exercises the ONLINE-mode branch,
/// which needs `Settings::auth_server_address` (seeded by
/// [`seed_online_mode_settings`]) to actually take effect (`--no-auth`/
/// `XINDELER_SERVER_NO_AUTH` clears that field unconditionally at boot, per
/// `sim::boot_dedicated_server`).
fn spawn_server_app_online_mode(
    data_dir: &Path,
    tag: &'static str,
    replicon_addr: SocketAddr,
) -> ChildGuard {
    let bin = env!("CARGO_BIN_EXE_xindeler-server-app");
    let mut cmd = Command::new(bin);
    cmd.env("VELOREN_USERDATA", data_dir)
        .env("XINDELER_SERVER_REPLICON_ADDR", replicon_addr.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn xindeler-server-app ({tag}): {e:?}"));
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in
                std::io::BufRead::lines(std::io::BufReader::new(stderr)).map_while(Result::ok)
            {
                println!("[{tag}] {line}");
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            for line in
                std::io::BufRead::lines(std::io::BufReader::new(stdout)).map_while(Result::ok)
            {
                println!("[{tag}] {line}");
            }
        });
    }
    ChildGuard(child)
}
