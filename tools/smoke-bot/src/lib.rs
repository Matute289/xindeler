//! BL-82 (EM-1.6) — end-to-end smoke regression for the logic crates.
//!
//! Boots a real, headless `xindeler-server-core` [`Server`] in a throwaway
//! data dir (singleplayer settings: free loopback TCP port, auth disabled,
//! default asset map), ticks it at 30 TPS on a background thread, then drives
//! a real `xindeler-client-core` [`Client`] through the whole life cycle over
//! actual loopback networking:
//!
//! 1. connect + register (`smoke_bot`, no auth server → direct username),
//! 2. load the character list and create a default character,
//! 3. enter the game (`request_character`) and wait for the in-game spawn,
//! 4. run [`SmokeOptions::move_ticks`] client ticks holding `move_dir` forward
//!    and verify the position changed,
//! 5. log out cleanly and shut the server down.
//!
//! This proves the whole logic stack (server ECS tick, persistence, network
//! protocol, client sync) stays functional while the Bevy migration reshapes
//! everything around it. Pure logic: no bevy/wgpu/winit anywhere — the
//! engine-isolation guard scans this crate like any other logic crate.
//!
//! Needs the real assets (`VELOREN_ASSETS`/`XINDELER_ASSETS` + LFS map blobs),
//! so the in-crate test is `#[ignore]`d; run it locally with
//! `XINDELER_ASSETS="$(pwd)/assets" cargo test -p xindeler-smoke-bot --
//! --ignored`.

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use client::{Client, ClientType, Event as ClientEvent, addr::ConnectionArgs};
use common::{ViewDistances, clock::Clock, comp};
use server::{
    EditableSettings, Input, Server, Settings,
    persistence::{DatabaseSettings, SqlLogMode},
    settings::Protocol,
};
use vek::{Vec2, Vec3};

/// Server + client tick rate for the smoke run (matches server-cli's TPS).
const TPS: f64 = 30.0;

/// Knobs for [`run_smoke`]. [`Default`] is what CI/the `#[ignore]`d test use.
#[derive(Debug, Clone)]
pub struct SmokeOptions {
    /// Username to register with (auth is disabled → used directly). Also the
    /// character alias.
    pub username: String,
    /// How many in-game client ticks to spend holding `move_dir` forward.
    pub move_ticks: u32,
    /// Terrain/entity view distance to request (small keeps chunk load light).
    pub view_distance: u32,
    /// Total wall-clock budget for the whole run, server boot included.
    pub deadline: Duration,
}

impl Default for SmokeOptions {
    fn default() -> Self {
        Self {
            username: "smoke_bot".to_owned(),
            move_ticks: 60,
            view_distance: 3,
            deadline: Duration::from_secs(120),
        }
    }
}

/// What actually happened, for asserts and logs.
#[derive(Debug, Clone)]
pub struct SmokeReport {
    /// Wall-clock time `Server::new` took (world/asset load dominates).
    pub server_boot: Duration,
    /// Id of the character the bot created (or reused) and spawned with.
    pub character_id: i64,
    /// In-game movement ticks actually performed.
    pub ticks: u32,
    /// Position right after spawning in game.
    pub start_pos: Vec3<f32>,
    /// Position after the movement ticks.
    pub end_pos: Vec3<f32>,
    /// 3D distance between `start_pos` and `end_pos`.
    pub moved_distance: f32,
    /// Horizontal (xy) component of the move — the walk itself, gravity aside.
    pub moved_distance_xy: f32,
    /// Successful sim ticks the server thread ran over the whole session.
    pub server_ticks: u64,
    /// Total wall-clock duration of the run.
    pub total_elapsed: Duration,
}

/// Phase-tagged failure so CI logs say exactly where the stack broke.
#[derive(Debug)]
pub enum SmokeError {
    /// `Server::new` itself failed.
    ServerBoot(server::Error),
    /// A phase failed outright (connect refused, character error, tick error…).
    Phase { phase: &'static str, msg: String },
    /// A phase didn't reach its goal before [`SmokeOptions::deadline`].
    Timeout {
        phase: &'static str,
        waited: Duration,
    },
}

impl fmt::Display for SmokeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerBoot(e) => write!(f, "smoke: server boot failed: {e:?}"),
            Self::Phase { phase, msg } => write!(f, "smoke: phase `{phase}` failed: {msg}"),
            Self::Timeout { phase, waited } => {
                write!(f, "smoke: phase `{phase}` timed out after {waited:?}")
            },
        }
    }
}

impl std::error::Error for SmokeError {}

fn phase_err(phase: &'static str, msg: impl fmt::Display) -> SmokeError {
    SmokeError::Phase {
        phase,
        msg: msg.to_string(),
    }
}

/// Runs the whole smoke flow described in the crate docs.
///
/// Everything lives in a `tempfile` data dir (settings, sqlite saves) that is
/// deleted on return; the only external requirement is the assets path env
/// var (`VELOREN_ASSETS`/`XINDELER_ASSETS`) with the LFS map blobs present.
pub fn run_smoke(opts: SmokeOptions) -> Result<SmokeReport, SmokeError> {
    let started = Instant::now();
    let deadline = started + opts.deadline;

    // ---- Phase: boot ----
    // Same recipe as `bevy/xindeler-sim-bridge::boot_test_server` (EM-1.5),
    // minus Bevy: singleplayer settings pick a free loopback TCP port and
    // disable auth; sqlite lives under the tempdir.
    let data_dir = tempfile::tempdir().map_err(|e| phase_err("boot", e))?;
    let settings = Settings::singleplayer(data_dir.path());
    let server_addr = match settings.gameserver_protocols.first() {
        Some(Protocol::Tcp { address }) => *address,
        other => {
            return Err(phase_err(
                "boot",
                format!("expected a TCP gameserver protocol, got {other:?}"),
            ));
        },
    };
    let editable_settings = EditableSettings::singleplayer(data_dir.path());
    let database_settings = DatabaseSettings {
        db_dir: data_dir.path().join("saves"),
        sql_log_mode: SqlLogMode::Disabled,
    };
    let server_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tokio-smoke-server")
            .build()
            .map_err(|e| phase_err("boot", e))?,
    );
    tracing::info!(?server_addr, "smoke: booting server");
    let server = Server::new(
        settings,
        editable_settings,
        database_settings,
        data_dir.path(),
        &|stage| tracing::debug!(?stage, "smoke server init"),
        Arc::clone(&server_runtime),
    )
    .map_err(SmokeError::ServerBoot)?;
    let server_boot = started.elapsed();
    tracing::info!(?server_boot, "smoke: server up");

    // ---- Server tick thread (server-cli's loop shape, 30 TPS) ----
    let stop = Arc::new(AtomicBool::new(false));
    let server_thread = {
        let stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("smoke-server-tick".to_owned())
            .spawn(move || -> Result<u64, String> {
                let mut server = server;
                let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));
                let mut ticks = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    server
                        .tick(Input::default(), clock.game_dt())
                        .map_err(|e| format!("server tick failed: {e:?}"))?;
                    server.cleanup();
                    ticks += 1;
                    clock.tick();
                }
                Ok(ticks)
            })
            .map_err(|e| phase_err("boot", e))?
    };

    // Run the client side in a closure so the server thread is always stopped
    // and joined, pass or fail.
    let client_result = drive_client(&opts, server_addr.port(), deadline);

    stop.store(true, Ordering::Relaxed);
    let server_ticks = match server_thread.join() {
        Ok(Ok(ticks)) => ticks,
        Ok(Err(msg)) => {
            // A client-phase error (e.g. a connect timeout) is usually the
            // symptom; the server tick error is the cause — report it.
            return Err(client_result
                .err()
                .unwrap_or_else(|| phase_err("server-tick", msg)));
        },
        Err(_) => return Err(phase_err("server-tick", "server tick thread panicked")),
    };

    let (character_id, ticks, start_pos, end_pos) = client_result?;
    let moved = end_pos - start_pos;
    Ok(SmokeReport {
        server_boot,
        character_id,
        ticks,
        start_pos,
        end_pos,
        moved_distance: moved.magnitude(),
        moved_distance_xy: moved.xy().magnitude(),
        server_ticks,
        total_elapsed: started.elapsed(),
    })
}

/// Connect → register → create character → spawn → move → logout.
/// Returns `(character_id, movement_ticks, start_pos, end_pos)`.
fn drive_client(
    opts: &SmokeOptions,
    port: u16,
    deadline: Instant,
) -> Result<(i64, u32, Vec3<f32>, Vec3<f32>), SmokeError> {
    // ---- Phase: connect ----
    // `Client::new` performs the full handshake (TCP connect, version check,
    // registration — auth is disabled, so the username is accepted directly —
    // and init-data download); it returns an already-registered client.
    let client_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tokio-smoke-client")
            .build()
            .map_err(|e| phase_err("connect", e))?,
    );
    let mut client = loop {
        let addr = ConnectionArgs::Tcp {
            hostname: format!("127.0.0.1:{port}"),
            prefer_ipv6: false,
        };
        let attempt = client_runtime.block_on(Client::new(
            addr,
            Arc::clone(&client_runtime),
            &mut None,
            &opts.username,
            "",
            None,
            |_| true,
            &|stage| tracing::debug!(?stage, "smoke client init"),
            |_| {},
            PathBuf::default(),
            ClientType::Game,
        ));
        match attempt {
            Ok(client) => break client,
            Err(e) if Instant::now() < deadline => {
                tracing::warn!(?e, "smoke: connect attempt failed, retrying");
                thread::sleep(Duration::from_millis(500));
            },
            Err(e) => return Err(phase_err("connect", format!("{e:?}"))),
        }
    };
    tracing::info!("smoke: connected + registered");

    let mut clock = Clock::new(Duration::from_secs_f64(1.0 / TPS));

    // ---- Phase: character list ----
    client.load_character_list();
    wait_for(
        &mut client,
        &mut clock,
        deadline,
        "character-list",
        |c, _| (!c.character_list().loading).then_some(()),
    )?;

    // ---- Phase: create character (if the roster is empty) ----
    let existing = first_character_id(&client);
    let character_id = match existing {
        Some(id) => id,
        None => {
            client.create_character(
                opts.username.clone(),
                // Keep in sync with valid_starter_items(Warrior) — same combo
                // the bot bin uses (client/src/bin/bot/main.rs).
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
                |c, events| {
                    for event in events {
                        if let ClientEvent::CharacterCreated(id) = event {
                            return Some(id.0);
                        }
                    }
                    // Fallback: some paths only surface the refreshed list.
                    if !c.character_list().loading {
                        first_character_id(c)
                    } else {
                        None
                    }
                },
            )?
        },
    };
    tracing::info!(character_id, "smoke: character ready");

    // ---- Phase: spawn ----
    // The server answers with `CharacterSuccess` + entity sync once the spawn
    // chunks are loaded; the first synced `Pos` on our entity is the signal.
    client.request_character(
        common::character::CharacterId(character_id),
        ViewDistances {
            terrain: opts.view_distance,
            entity: opts.view_distance,
        },
    );
    let start_pos = wait_for(&mut client, &mut clock, deadline, "spawn", |c, _| {
        c.position()
    })?;
    tracing::info!(?start_pos, "smoke: spawned in game");

    // ---- Phase: move ----
    let mut ticks = 0u32;
    for _ in 0..opts.move_ticks {
        let inputs = comp::ControllerInputs {
            move_dir: Vec2::unit_y(),
            ..Default::default()
        };
        tick_client(&mut client, &mut clock, inputs, "move")?;
        ticks += 1;
    }
    let end_pos = client
        .position()
        .ok_or_else(|| phase_err("move", "client lost its position mid-run"))?;
    tracing::info!(?end_pos, ticks, "smoke: movement done");

    // ---- Phase: logout ----
    // `logout` sends `Terminate` and marks the client unregistered; give the
    // async network stack a beat to flush before dropping the client (whose
    // `Drop` closes the participant).
    client.logout();
    thread::sleep(Duration::from_millis(200));
    drop(client);
    tracing::info!("smoke: logged out cleanly");

    Ok((character_id, ticks, start_pos, end_pos))
}

/// One paced client tick with fatal-event screening.
fn tick_client(
    client: &mut Client,
    clock: &mut Clock,
    inputs: comp::ControllerInputs,
    phase: &'static str,
) -> Result<Vec<ClientEvent>, SmokeError> {
    clock.tick();
    let events = client
        .tick(inputs, clock.game_dt())
        .map_err(|e| phase_err(phase, format!("client tick failed: {e:?}")))?;
    client.cleanup();
    for event in &events {
        match event {
            ClientEvent::Disconnect => {
                return Err(phase_err(phase, "server disconnected the client"));
            },
            ClientEvent::CharacterError(msg) => {
                return Err(phase_err(phase, format!("character error: {msg}")));
            },
            _ => {},
        }
    }
    Ok(events)
}

/// Ticks the client (default inputs) until `check` yields a value or the
/// deadline passes.
fn wait_for<T>(
    client: &mut Client,
    clock: &mut Clock,
    deadline: Instant,
    phase: &'static str,
    mut check: impl FnMut(&mut Client, &[ClientEvent]) -> Option<T>,
) -> Result<T, SmokeError> {
    let waited = Instant::now();
    loop {
        if Instant::now() > deadline {
            return Err(SmokeError::Timeout {
                phase,
                waited: waited.elapsed(),
            });
        }
        let events = tick_client(client, clock, comp::ControllerInputs::default(), phase)?;
        if let Some(value) = check(client, &events) {
            return Ok(value);
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

/// Same default humanoid the bot bin creates.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// EM-1.6 acceptance: full boot → connect → character → move → logout.
    #[test]
    #[ignore = "boots server+client with real assets; run locally with XINDELER_ASSETS"]
    fn smoke_full_flow() {
        let opts = SmokeOptions::default();
        let move_ticks = opts.move_ticks;
        let report = run_smoke(opts).expect("smoke run failed");
        println!("smoke report: {report:#?}");
        assert_eq!(
            report.ticks, move_ticks,
            "all movement ticks should have run"
        );
        assert!(
            report.moved_distance > 0.5,
            "character should have moved while holding move_dir forward: {report:#?}"
        );
        assert!(report.server_ticks > 0, "server should have ticked");
    }
}
