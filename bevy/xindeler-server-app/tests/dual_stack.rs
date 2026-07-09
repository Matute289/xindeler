//! EM-4.1 acceptance: **dual-stack**. A REAL `xindeler-client-core::Client`
//! connects over REAL loopback TCP networking to `xindeler-server-app`
//! running as a genuinely SEPARATE OS PROCESS (not in-process, unlike
//! `tools/smoke-bot`), then walks through connect → character → spawn → move
//! → logout.
//!
//! This is the strongest wire-protocol-level proxy available in this
//! environment for the task board's literal acceptance line ("OLD client
//! connects & plays against the shell"): `xindeler-client-core::Client` is a
//! real client speaking the exact same wire protocol the graphical old
//! client (voxygen / `xindeler-old`) speaks — that repo just isn't checked
//! out here. A manual check with the real `xindeler-old` client is a
//! reasonable extra-assurance follow-up, not required by this test.
//!
//! Needs real assets (`VELOREN_ASSETS`/`XINDELER_ASSETS` + the LFS map blobs)
//! and spawns a real child process, so — like the other full-world tests in
//! this repo (`xindeler-sim-bridge`, `xindeler-smoke-bot`) — this is
//! `#[ignore]`d; run locally with:
//! `XINDELER_ASSETS="$(pwd)/assets" cargo test -p xindeler-server-app --test
//! dual_stack -- --ignored --nocapture`

use std::{
    io::{BufRead, BufReader},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use client::{Client, ClientType, Event as ClientEvent, addr::ConnectionArgs};
use common::{ViewDistances, character::CharacterId, clock::Clock, comp};
use vek::Vec2;

/// Matches `xindeler-server-app::sim::SIM_TICK_INTERVAL` / server-cli's TPS.
const TPS: f64 = 30.0;
const USERNAME: &str = "em41_dual_stack_bot";
const MOVE_TICKS: u32 = 60;
const MIN_MOVED_XY: f32 = 0.5;

/// Kills the child `xindeler-server-app` process on drop so a failing assert
/// mid-test doesn't leak an orphaned dedicated server.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "spawns a real xindeler-server-app process + boots a real world: needs assets + LFS; \
            run locally with XINDELER_ASSETS"]
fn dual_stack_real_process_real_client() {
    let data_dir = tempfile::tempdir().expect("tempdir");

    // Pre-seed `settings.ron` binding `gameserver_protocols` to a FREE
    // loopback port (the same `pick_unused_port` trick
    // `server::settings::Settings::singleplayer` uses) instead of the
    // production default 14004: a hardcoded port could silently TCP-connect
    // this test to an unrelated already-running server-cli/server-app on the
    // dev machine and false-pass against the WRONG process. This still
    // proves the dual-stack claim (the sim's own unmodified
    // `gameserver_protocols` listener comes up and accepts a real client) —
    // it just doesn't have to be on the exact default port to do so.
    let game_port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    let settings = server::settings::Settings {
        gameserver_protocols: vec![server::settings::Protocol::Tcp {
            address: ([127, 0, 0, 1], game_port).into(),
        }],
        ..server::settings::Settings::default()
    };
    let settings_dir = data_dir.path().join("server").join("server_config");
    std::fs::create_dir_all(&settings_dir).expect("failed to create server_config dir");
    std::fs::write(
        settings_dir.join("settings.ron"),
        ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("failed to serialize test settings"),
    )
    .expect("failed to write settings.ron");

    // `CARGO_BIN_EXE_<bin-name>` is set by cargo for integration tests that
    // live in the SAME package as the binary — the compiled
    // `xindeler-server-app` binary under test, launched as a genuinely
    // separate process (own PID, own address space, real OS-level TCP).
    let bin = env!("CARGO_BIN_EXE_xindeler-server-app");
    let mut child = Command::new(bin)
        .env("VELOREN_USERDATA", data_dir.path())
        // Mirrors server-cli's `--no-auth` flag: this sandbox has no route to
        // the real auth server.
        .env("XINDELER_SERVER_NO_AUTH", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn xindeler-server-app");

    // tracing's default writer is stderr; stream both so `--nocapture` shows
    // the SEPARATE process's own real log lines inline with the test's.
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                println!("[server-app] {line}");
            }
        });
    }
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                println!("[server-app] {line}");
            }
        });
    }
    let mut child = ChildGuard(child);

    // ---- wait for the sim's own dual-stack listener to come up ----
    let addr: SocketAddr = ([127, 0, 0, 1], game_port).into();
    let boot_deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if TcpStream::connect(addr).is_ok() {
            break;
        }
        assert!(
            Instant::now() < boot_deadline,
            "xindeler-server-app never opened its game listener on {addr} within the boot deadline"
        );
        thread::sleep(Duration::from_millis(500));
    }
    println!(
        "[test] xindeler-server-app's OWN listener (unmodified `gameserver_protocols`) is \
         accepting real TCP connections on {addr} — dual-stack confirmed at the transport level"
    );

    // ---- connect a REAL client over REAL loopback networking ----
    let client_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tokio-em41-client")
            .build()
            .expect("failed to build the test client's tokio runtime"),
    );
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let mut client = loop {
        assert!(
            !child.0.try_wait().is_ok_and(|s| s.is_some()),
            "server-app process exited while the client was connecting"
        );
        let args = ConnectionArgs::Tcp {
            hostname: format!("127.0.0.1:{game_port}"),
            prefer_ipv6: false,
        };
        let attempt = client_runtime.block_on(Client::new(
            args,
            Arc::clone(&client_runtime),
            &mut None,
            USERNAME,
            "",
            None,
            |_| true,
            &|stage| println!("[test] client init: {stage:?}"),
            |_| {},
            PathBuf::default(),
            ClientType::Game,
        ));
        match attempt {
            Ok(client) => break client,
            Err(e) => {
                assert!(
                    Instant::now() < connect_deadline,
                    "connect to the separate server-app process timed out: {e:?}"
                );
                println!("[test] connect attempt failed, retrying: {e:?}");
                thread::sleep(Duration::from_millis(500));
            },
        }
    };
    println!("[test] connected + registered against the SEPARATE xindeler-server-app process");

    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
    let deadline = Instant::now() + Duration::from_secs(120);

    // ---- character list + create ----
    client.load_character_list();
    wait_for(
        &mut client,
        &mut clock,
        deadline,
        "character-list",
        &mut child,
        |c, _| (!c.character_list().loading).then_some(()),
    );

    let character_id = first_character_id(&client).unwrap_or_else(|| {
        client.create_character(
            USERNAME.to_owned(),
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
            &mut client,
            &mut clock,
            deadline,
            "create-character",
            &mut child,
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
    });
    println!("[test] character ready: {character_id}");

    // ---- spawn ----
    client.request_character(CharacterId(character_id), ViewDistances {
        terrain: 3,
        entity: 3,
    });
    let start_pos = wait_for(
        &mut client,
        &mut clock,
        deadline,
        "spawn",
        &mut child,
        |c, _| c.position(),
    );
    println!("[test] spawned in game at {start_pos:?}");

    // ---- move ----
    for _ in 0..MOVE_TICKS {
        tick_client(
            &mut client,
            &mut clock,
            comp::ControllerInputs {
                move_dir: Vec2::unit_y(),
                ..Default::default()
            },
            "move",
            &mut child,
        );
    }
    let end_pos = client
        .position()
        .expect("client should still have a position after moving");
    let moved_xy = (end_pos - start_pos).xy().magnitude();
    println!("[test] moved from {start_pos:?} to {end_pos:?} ({moved_xy:.3} m horizontally)");
    assert!(
        moved_xy > MIN_MOVED_XY,
        "character barely moved horizontally ({moved_xy:.3} < {MIN_MOVED_XY}) over {MOVE_TICKS} \
         ticks against the separate server-app process"
    );

    // ---- logout ----
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
    drop(client);
    println!("[test] logged out cleanly from the separate xindeler-server-app process");
}

fn tick_client(
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

fn wait_for<T>(
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

fn first_character_id(client: &Client) -> Option<i64> {
    client
        .character_list()
        .characters
        .first()
        .and_then(|c| c.character.id)
        .map(|id| id.0)
}

fn default_body() -> comp::body::humanoid::Body {
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
