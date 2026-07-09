//! Shared boot/connect helpers for the EM-4.2 correctness tests
//! (`persistence_roundtrip.rs`, `rtsim_roundtrip.rs`). Deliberately NOT used
//! by `dual_stack.rs` (EM-4.1, already merged/working) to avoid touching a
//! test that already passes; this module only exists to avoid duplicating
//! the same "spawn a separate `xindeler-server-app` process, wait for its
//! listener, connect a real client" boilerplate `dual_stack.rs` pioneered
//! across two more test files.
//!
//! `tests/support/mod.rs` (a `mod.rs` under a subdirectory, not a top-level
//! `tests/*.rs` file) is the standard cargo convention for code shared
//! between integration test binaries without being compiled as its own test
//! crate.

#![allow(dead_code)] // not every test uses every helper

use std::{
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use client::{Client, ClientType, Event as ClientEvent, addr::ConnectionArgs};
use common::{clock::Clock, comp};

/// Matches `xindeler-server-app::sim::SIM_TICK_INTERVAL` / server-cli's TPS.
pub const TPS: f64 = 30.0;

/// Kills the child `xindeler-server-app` process on drop so a failing assert
/// mid-test doesn't leak an orphaned dedicated server (mirrors
/// `dual_stack.rs`'s `ChildGuard`).
pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Best-effort: if the process already exited cleanly (the intended
        // path in these tests, via `graceful_shutdown`), `kill` just fails
        // harmlessly on an already-dead pid.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Pre-seeds `<data_dir>/server/server_config/settings.ron` binding
/// `gameserver_protocols` AND `query_address` to FREE loopback/local ports —
/// the same `pick_unused_port` trick `dual_stack.rs` uses for the game port,
/// extended (review should-fix) to the query server too: `Settings::default`
/// hardcodes `query_address` to `0.0.0.0:14006`, which collided with an
/// unrelated concurrently-running `xindeler-server-app` instance during
/// review (benign — the query server is an ancillary ping/info service, its
/// bind failure doesn't affect any test assertion — but real log noise and a
/// real collision surface worth closing while touching this file). Returns
/// the picked game port.
pub fn seed_settings(data_dir: &Path) -> u16 {
    let game_port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let query_port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let settings = server::settings::Settings {
        gameserver_protocols: vec![server::settings::Protocol::Tcp {
            address: ([127, 0, 0, 1], game_port).into(),
        }],
        query_address: Some(([127, 0, 0, 1], query_port).into()),
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

/// Spawns the compiled `xindeler-server-app` binary as a genuinely separate
/// OS process rooted at `data_dir` (`VELOREN_USERDATA`), streaming its
/// stdout/stderr through `println!` with a `tag` prefix so `--nocapture`
/// interleaves the child's real log lines with the test's own.
pub fn spawn_server_app(data_dir: &Path, tag: &'static str) -> ChildGuard {
    spawn_server_app_with_env(data_dir, tag, std::iter::empty::<(&str, &str)>())
}

/// Like [`spawn_server_app`], plus arbitrary extra environment variables
/// (e.g. `XINDELER_SERVER_METRICS_ADDR` to pin the metrics port instead of
/// the production default, for tests that scrape `/metrics`).
pub fn spawn_server_app_with_env<K, V>(
    data_dir: &Path,
    tag: &'static str,
    extra_env: impl IntoIterator<Item = (K, V)>,
) -> ChildGuard
where
    K: AsRef<std::ffi::OsStr>,
    V: AsRef<std::ffi::OsStr>,
{
    let bin = env!("CARGO_BIN_EXE_xindeler-server-app");
    let mut cmd = Command::new(bin);
    cmd.env("VELOREN_USERDATA", data_dir)
        // Mirrors server-cli's `--no-auth` flag: this sandbox has no route to
        // the real auth server.
        .env("XINDELER_SERVER_NO_AUTH", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn xindeler-server-app ({tag}): {e:?}"));

    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                println!("[{tag}] {line}");
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                println!("[{tag}] {line}");
            }
        });
    }
    ChildGuard(child)
}

/// Blocks until `addr` accepts a real TCP connection (the sim's own
/// dual-stack listener coming up) or panics past `timeout`.
pub fn wait_for_listener(addr: SocketAddr, timeout: Duration, tag: &'static str) {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "xindeler-server-app ({tag}) never opened its game listener on {addr} within the boot \
             deadline"
        );
        thread::sleep(Duration::from_millis(500));
    }
}

/// Sends SIGTERM to the child process (the graceful-shutdown signal
/// `shutdown.rs` registers a handler for) and blocks until it exits,
/// asserting a clean (non-crash) exit. Uses the platform `kill` utility
/// rather than a new dependency — `Child::kill()` in `std` only sends
/// SIGKILL, which would skip `Drop for Server` entirely and defeat the
/// purpose of this helper.
#[cfg(unix)]
pub fn graceful_shutdown(child: &mut ChildGuard, timeout: Duration, tag: &'static str) {
    let pid = child.0.id();
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    assert!(
        status.is_ok_and(|s| s.success()),
        "failed to send SIGTERM to xindeler-server-app ({tag}, pid {pid})"
    );

    let deadline = Instant::now() + timeout;
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) => {
                assert!(
                    status.success(),
                    "xindeler-server-app ({tag}) did not exit cleanly after SIGTERM: {status:?}"
                );
                return;
            },
            Ok(None) => {
                assert!(
                    Instant::now() < deadline,
                    "xindeler-server-app ({tag}) did not exit within {timeout:?} of SIGTERM \
                     (graceful shutdown may be hanging)"
                );
                thread::sleep(Duration::from_millis(200));
            },
            Err(e) => panic!("failed to poll xindeler-server-app ({tag}) exit status: {e:?}"),
        }
    }
}

/// Builds a small dedicated tokio runtime for a test client (mirrors
/// `dual_stack.rs`'s client runtime sizing).
pub fn client_runtime(name: &'static str) -> std::sync::Arc<tokio::runtime::Runtime> {
    std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name(name)
            .build()
            .expect("failed to build the test client's tokio runtime"),
    )
}

/// Connects a real `xindeler-client-core::Client` over real loopback TCP,
/// retrying until `deadline` (the server may still be finishing worldgen).
pub fn connect_client(
    runtime: &std::sync::Arc<tokio::runtime::Runtime>,
    child: &mut ChildGuard,
    game_port: u16,
    username: &str,
    deadline: Instant,
) -> Client {
    loop {
        assert!(
            !child.0.try_wait().is_ok_and(|s| s.is_some()),
            "server-app process exited while the client was connecting"
        );
        let args = ConnectionArgs::Tcp {
            hostname: format!("127.0.0.1:{game_port}"),
            prefer_ipv6: false,
        };
        let attempt = runtime.block_on(Client::new(
            args,
            std::sync::Arc::clone(runtime),
            &mut None,
            username,
            "",
            None,
            |_| true,
            &|stage| println!("[test] client init: {stage:?}"),
            |_| {},
            PathBuf::default(),
            ClientType::Game,
        ));
        match attempt {
            Ok(client) => return client,
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "connect to the separate server-app process timed out: {e:?}"
                );
                println!("[test] connect attempt failed, retrying: {e:?}");
                thread::sleep(Duration::from_millis(500));
            },
        }
    }
}

pub fn tick_client(
    client: &mut Client,
    clock: &mut Clock,
    inputs: comp::ControllerInputs,
    phase: &'static str,
    child: &mut ChildGuard,
) -> Vec<ClientEvent> {
    assert!(
        !child.0.try_wait().is_ok_and(|s| s.is_some()),
        "server-app process exited during phase `{phase}`"
    );
    clock.tick();
    let events = client
        .tick(inputs, clock.game_dt())
        .unwrap_or_else(|e| panic!("client tick failed during phase `{phase}`: {e:?}"));
    client.cleanup();
    for event in &events {
        match event {
            ClientEvent::Disconnect => panic!("server disconnected the client during `{phase}`"),
            ClientEvent::CharacterError(msg) => {
                panic!("character error during `{phase}`: {msg}")
            },
            _ => {},
        }
    }
    events
}

pub fn wait_for<T>(
    client: &mut Client,
    clock: &mut Clock,
    deadline: Instant,
    phase: &'static str,
    child: &mut ChildGuard,
    mut check: impl FnMut(&mut Client, &[ClientEvent]) -> Option<T>,
) -> T {
    loop {
        assert!(Instant::now() < deadline, "phase `{phase}` timed out");
        let events = tick_client(
            client,
            clock,
            comp::ControllerInputs::default(),
            phase,
            child,
        );
        if let Some(value) = check(client, &events) {
            return value;
        }
    }
}

pub fn first_character_id(client: &Client) -> Option<i64> {
    client
        .character_list()
        .characters
        .first()
        .and_then(|c| c.character.id)
        .map(|id| id.0)
}

pub fn default_body() -> comp::body::humanoid::Body {
    comp::body::humanoid::Body {
        species: comp::body::humanoid::Species::Human,
        body_type: comp::body::humanoid::BodyType::Male,
        hair_style: 0,
        beard: 0,
        eyes: 0,
        accessory: 0,
        hair_color: 0,
        skin: 0,
        eye_color: 0,
    }
}

/// Creates a character (waiting for `ClientEvent::CharacterCreated`) and
/// returns its id.
pub fn create_character(
    client: &mut Client,
    clock: &mut Clock,
    deadline: Instant,
    child: &mut ChildGuard,
    alias: &str,
) -> i64 {
    client.create_character(
        alias.to_owned(),
        Some("common.items.weapons.sword.starter".to_owned()),
        None,
        default_body().into(),
        false,
        None,
        comp::class::ClassKind::Warrior,
        comp::Ethos::default(),
        comp::Background::default(),
    );
    wait_for(
        client,
        clock,
        deadline,
        "create-character",
        child,
        |c, events| {
            for event in events {
                if let ClientEvent::CharacterCreated(id) = event {
                    return Some(id.0);
                }
            }
            (!c.character_list().loading)
                .then(|| first_character_id(c))
                .flatten()
        },
    )
}

/// Cleanly logs a client out (drains until `Disconnect`, bounded to avoid
/// hanging if the server already went away).
pub fn logout(client: &mut Client, clock: &mut Clock) {
    client.logout();
    let flush_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < flush_deadline {
        clock.tick();
        match client.tick(comp::ControllerInputs::default(), clock.game_dt()) {
            Ok(events) => {
                client.cleanup();
                if events.iter().any(|e| matches!(e, ClientEvent::Disconnect)) {
                    break;
                }
            },
            Err(_) => break,
        }
    }
}
