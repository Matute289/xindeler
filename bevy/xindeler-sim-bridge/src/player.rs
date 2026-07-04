//! EM-3.7b — the embedded LOCAL-PLAYER client (server-side).
//!
//! In listen-server mode the process hosts the sim ([`SimServer`]) AND a real
//! `xindeler-client-core::Client` connected to that sim over TCP loopback. That
//! embedded Client IS the local player's presence: it registers, creates (or
//! reuses) a default character, requests spawn, and — once in game — keeps the
//! chunks around itself loaded and holds a controllable entity in the sim. The
//! Bevy keyboard/mouse (read by the pure client) feeds
//! [`xindeler_protocol::LocalPlayerInput`], which [`tick_player`] translates
//! into the Client's [`comp::ControllerInputs`] and applies via `client.tick`.
//!
//! This reuses the EXACT life-cycle the EM-1.6 smoke bot proved
//! (`tools/smoke-bot`): `Client::new` over loopback → `load_character_list` →
//! `create_character`/reuse → `request_character` → tick with inputs. The only
//! differences are that it is driven from the Bevy schedule one frame at a time
//! (a small state machine instead of blocking loops) and that the input comes
//! from Bevy rather than a fixed `move_dir`.
//!
//! ## Why the Client, and not (only) `create_centered_persister`
//! EM-3.6's anchor was a `create_centered_persister` spectator — enough to keep
//! terrain streaming, but it has no controllable entity and no `Uid` the client
//! can follow. The embedded Client gives us a real player entity that moves
//! with input; it *also* keeps chunks loaded (it holds a `Presence`), so it
//! subsumes the persister's job. The persister stays as a FALLBACK: if the
//! Client never reaches in-game (missing assets, connect failure) the bridge
//! still spawns the persister so terrain streams and the world is visible
//! (see `ensure_terrain_anchor` in `lib.rs`, now gated on player readiness).
//!
//! ## Threading / storage
//! Like [`SimServer`], the `Client` is `Send` but `!Sync` (its `State` boxes a
//! specs `SendDispatcher`), so it is stored as Bevy **non-send** data and
//! [`tick_player`] runs on the main thread — the same thread the sim ticks on,
//! which is also required because both share nothing but the loopback socket.
//!
//! ## Isolation
//! This module lives in the server-side bridge (the only crate under `bevy/`
//! that may link `specs`/`client`/`server`). The pure Bevy client never sees
//! it. Purity of `bevy/xindeler-client/src` is unaffected.

use std::{path::PathBuf, sync::Arc, time::Duration};

use bevy::{
    app::{App, Plugin, Update},
    ecs::{change_detection::NonSendMut, schedule::IntoScheduleConfigs, system::Res},
};
use client::{Client, ClientType, Event as ClientEvent, addr::ConnectionArgs};
use common::{
    ViewDistances,
    clock::Clock,
    comp,
    uid::{IdMaps, Uid},
    util::Dir,
};
use specs::WorldExt;
use xindeler_protocol::LocalPlayerInput;

use crate::{SimServer, tick_sim};

/// Registers the [`LocalPlayerInput`] resource and the [`tick_player`] system
/// (runs after [`tick_sim`], main-thread non-send). Does NOT boot the embedded
/// player itself — the listen-server shell inserts an [`EmbeddedPlayer`] via
/// [`boot_embedded_player`] once the sim is up (booting the Client blocks on a
/// loopback handshake, so the shell owns the timing, same as [`SimServer`]).
///
/// Add AFTER [`crate::SimBridgePlugin`].
pub struct PlayerBridgePlugin;

impl Plugin for PlayerBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayerInput>()
            .add_systems(Update, tick_player.after(tick_sim));
    }
}

/// Client + server tick rate for the embedded player (matches the sim's TPS and
/// the smoke bot). The Client is ticked once per Bevy frame; its `Clock`
/// provides the `dt` its own sync loop expects.
const PLAYER_TPS: f64 = 30.0;

/// Username the embedded player registers with (auth is disabled on the
/// singleplayer sim, so it is used directly) and the character alias. Distinct
/// from the smoke bot's name so a shared save dir never collides.
const PLAYER_USERNAME: &str = "listen_host";

/// Terrain/entity view distance the embedded player requests. Kept modest
/// (matches the sim minimum) for the listen-server proof — a wide radius floods
/// the single local client with chunks until interest management lands
/// (EM-4.2d). Mirrors [`crate::ANCHOR_VIEW_DISTANCE`].
const PLAYER_VIEW_DISTANCE: u32 = server_min_vd();

const fn server_min_vd() -> u32 { server::MIN_VD }

/// The embedded local-player [`Client`], its runtime, tick clock, and the tiny
/// life-cycle state machine that walks it from "just connected" to "in game".
///
/// Non-send (see module docs). Inserted by [`boot_embedded_player`] once the
/// loopback connect + registration succeed; [`tick_player`] advances it every
/// frame thereafter.
pub struct EmbeddedPlayer {
    client: Client,
    /// Kept alive for the Client's async networking.
    _runtime: Arc<tokio::runtime::Runtime>,
    clock: Clock,
    stage: PlayerStage,
    /// The character id we created/selected, once known.
    character_id: Option<i64>,
    /// The player entity's server `Uid`, resolved once in game. Stable across
    /// the session; the mirror uses it to tag the player's replicated entity.
    uid: Option<Uid>,
    /// Last jump state we sent to the sim, so [`tick_player`] only emits jump
    /// press/release edges (the Client has no "is jump held" query).
    jumping: bool,
}

/// Non-blocking life-cycle stages, advanced one per frame by [`tick_player`].
/// Mirrors the smoke bot's blocking phases (`drive_client`), but each step
/// yields back to the Bevy schedule instead of spinning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayerStage {
    /// Waiting for the character list to finish loading.
    LoadingCharacterList,
    /// A `create_character` request is in flight (roster was empty).
    CreatingCharacter,
    /// `request_character` sent; waiting for the in-game spawn (first `Pos`).
    Spawning,
    /// In game: `tick_player` now applies input and the player is controllable.
    InGame,
    /// The Client failed terminally (logged once). It is left ticking network
    /// only so it doesn't wedge the sim; the persister fallback covers terrain.
    Failed,
}

impl EmbeddedPlayer {
    /// The player entity's server `Uid`, once in game (`None` before spawn).
    pub fn uid(&self) -> Option<Uid> { self.uid }

    /// Whether the player has reached the in-game stage (used to gate the
    /// terrain-anchor fallback in `lib.rs`).
    pub fn is_in_game(&self) -> bool { self.stage == PlayerStage::InGame }

    /// Whether we've reached a terminal failure (connect/tick died); the
    /// persister fallback should then cover terrain streaming.
    pub fn is_failed(&self) -> bool { self.stage == PlayerStage::Failed }

    /// The player entity's current world position (sim axes, z-up), read from
    /// the embedded Client. `None` before spawn. Test/dev helper — the
    /// authoritative view for the render side is the replicated `NetPos`.
    pub fn position(&self) -> Option<vek::Vec3<f32>> { self.client.position() }

    fn character_jumping(&self) -> bool { self.jumping }

    fn set_character_jumping(&mut self, jumping: bool) { self.jumping = jumping; }
}

/// Connects a fresh embedded [`Client`] to the already-running embedded sim
/// over TCP loopback and returns it in its initial [`PlayerStage`].
///
/// ## Why the sim must be pumped DURING the handshake
/// `Client::new` performs a multi-round handshake (connect, version check,
/// registration, init-data download) that only completes as the SERVER
/// PROCESSES TICKS — the server answers each round in `Server::tick`. The
/// EM-1.6 smoke bot ticks the server on a background thread precisely for this
/// reason. In our architecture the sim lives on the main thread (non-send) and
/// is NOT ticking yet when the listen-server shell calls this in `build()`, so
/// a plain blocking `Client::new` would deadlock/time out (observed).
///
/// So this takes `&mut SimServer` and, for the duration of the (blocking)
/// `Client::new`, moves the `Server` onto a scoped background thread that ticks
/// it at the sim TPS (`Server` is `Send`, just `!Sync`, so this is sound); when
/// the handshake returns, the thread stops and the `Server` is reclaimed onto
/// the main thread where [`tick_sim`] takes over. Net effect: exactly the smoke
/// bot's "server ticking while the client connects", bounded to boot.
///
/// Returns `Err` if the port can't be read or the handshake fails — the caller
/// logs and falls back to the persister anchor.
pub fn boot_embedded_player(sim: &mut SimServer) -> Result<EmbeddedPlayer, String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Read the loopback TCP port the sim is listening on (Settings live in the
    // sim ECS; `Server::settings()` derefs to them). Singleplayer settings only
    // configure TCP — the smoke bot relies on the same.
    let port = {
        let settings = sim.server.settings();
        settings
            .gameserver_protocols
            .iter()
            .find_map(|p| match p {
                server::settings::Protocol::Tcp { address } => Some(address.port()),
                server::settings::Protocol::Quic { .. } => None,
            })
            .ok_or_else(|| "sim has no TCP gameserver protocol to connect to".to_owned())?
    };

    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tokio-embedded-player")
            .build()
            .map_err(|e| format!("failed to build embedded-player runtime: {e}"))?,
    );

    let addr = ConnectionArgs::Tcp {
        hostname: format!("127.0.0.1:{port}"),
        prefer_ipv6: false,
    };

    // Pump the sim on a scoped background thread while `Client::new` blocks, so
    // the handshake's server-side rounds actually get processed. The scope joins
    // the thread before returning, so the `&mut Server` borrow is released and
    // the sim is safely back on the main thread afterwards.
    let stop = AtomicBool::new(false);
    let server = &mut sim.server;
    let connect = std::thread::scope(|scope| {
        let ticker = scope.spawn(|| {
            let mut clock = Clock::new(Duration::from_secs_f64(1.0 / PLAYER_TPS));
            while !stop.load(Ordering::Relaxed) {
                // Ignore tick errors here; a real failure surfaces once the
                // main-thread `tick_sim` runs (and the connect will fail too).
                if server
                    .tick(server::Input::default(), clock.game_dt())
                    .is_ok()
                {
                    server.cleanup();
                }
                clock.tick();
            }
        });

        // `timeout` must be created inside the runtime (its timer needs the
        // reactor), hence the async block — same shape as the smoke bot.
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(30),
                Client::new(
                    addr,
                    Arc::clone(&runtime),
                    &mut None,
                    PLAYER_USERNAME,
                    "",
                    None,
                    |_| true,
                    &|stage| tracing::debug!(?stage, "embedded player init"),
                    |_| {},
                    PathBuf::default(),
                    ClientType::Game,
                ),
            )
            .await
        });

        stop.store(true, Ordering::Relaxed);
        let _ = ticker.join();
        result
    });

    let mut client = match connect {
        Ok(Ok(client)) => client,
        Ok(Err(e)) => return Err(format!("embedded player connect failed: {e:?}")),
        Err(_) => return Err("embedded player connect timed out".to_owned()),
    };

    // Kick off the character-list load; the state machine takes it from here.
    client.load_character_list();

    Ok(EmbeddedPlayer {
        client,
        _runtime: runtime,
        clock: Clock::new(Duration::from_secs_f64(1.0 / PLAYER_TPS)),
        stage: PlayerStage::LoadingCharacterList,
        character_id: None,
        uid: None,
        jumping: false,
    })
}

/// Advances the embedded player one frame: ticks its network/sync and walks the
/// life-cycle state machine, applying [`LocalPlayerInput`] once in game.
///
/// Runs on the main thread (non-send) every `Update`, after [`crate::tick_sim`]
/// so the sim has already processed the previous frame's input. No-ops until
/// the shell inserts an [`EmbeddedPlayer`] (listen-server only).
pub fn tick_player(player: Option<NonSendMut<EmbeddedPlayer>>, input: Res<LocalPlayerInput>) {
    let Some(mut player) = player else { return };
    player.clock.tick();
    let dt = player.clock.game_dt();

    // Build the ControllerInputs for THIS frame from the shared Bevy input
    // sample (only meaningful in game; harmless otherwise).
    let inputs = if player.stage == PlayerStage::InGame {
        controller_inputs_from(&input)
    } else {
        comp::ControllerInputs::default()
    };

    // Jump is a discrete control action (InputKind::Jump), not a ControllerInputs
    // field — push it before the tick so it rides the same message batch. We
    // track the previous jump state to send press/release edges (StartInput /
    // CancelInput), matching how a real client drives jump.
    let jump_now = player.stage == PlayerStage::InGame && input.jump;
    let was_jumping = player.character_jumping();
    if jump_now != was_jumping {
        player
            .client
            .handle_input(comp::InputKind::Jump, jump_now, None, None);
        player.set_character_jumping(jump_now);
    }

    let events = match player.client.tick(inputs, dt) {
        Ok(events) => events,
        Err(err) => {
            if player.stage != PlayerStage::Failed {
                tracing::error!(
                    ?err,
                    "embedded player tick failed; disabling player control"
                );
                player.stage = PlayerStage::Failed;
            }
            return;
        },
    };
    player.client.cleanup();

    advance_stage(&mut player, &events);
}

/// Resolves the player's server `Uid` from its in-game entity, once. The
/// Client's own `State` holds the same server-authoritative `Uid` the sim
/// assigned (it is network-synced), so `client.uid()` is exactly the key the
/// mirror uses. Cached in [`EmbeddedPlayer::uid`].
fn advance_stage(player: &mut EmbeddedPlayer, events: &[ClientEvent]) {
    match player.stage {
        PlayerStage::LoadingCharacterList => {
            if player.client.character_list().loading {
                return;
            }
            match first_character_id(&player.client) {
                Some(id) => {
                    player.character_id = Some(id);
                    request_spawn(player, id);
                },
                None => {
                    create_default_character(player);
                    player.stage = PlayerStage::CreatingCharacter;
                },
            }
        },
        PlayerStage::CreatingCharacter => {
            // The created id surfaces either as a CharacterCreated event or via
            // the refreshed roster (same fallback the smoke bot uses).
            let created = events.iter().find_map(|e| match e {
                ClientEvent::CharacterCreated(id) => Some(id.0),
                _ => None,
            });
            let id = created.or_else(|| {
                (!player.client.character_list().loading)
                    .then(|| first_character_id(&player.client))
                    .flatten()
            });
            if let Some(id) = id {
                player.character_id = Some(id);
                request_spawn(player, id);
            }
        },
        PlayerStage::Spawning => {
            // The first synced `Pos` on our entity is the in-game signal.
            if player.client.position().is_some() {
                player.uid = player.client.uid();
                tracing::info!(
                    uid = ?player.uid,
                    character_id = ?player.character_id,
                    "embedded player spawned in game (controllable)"
                );
                player.stage = PlayerStage::InGame;
            }
        },
        PlayerStage::InGame => {
            // Keep the Uid fresh in the unlikely event it wasn't ready at the
            // spawn frame.
            if player.uid.is_none() {
                player.uid = player.client.uid();
            }
        },
        PlayerStage::Failed => {},
    }
}

/// Sends `request_character` for `id` and moves to the spawning stage.
fn request_spawn(player: &mut EmbeddedPlayer, id: i64) {
    player
        .client
        .request_character(common::character::CharacterId(id), ViewDistances {
            terrain: PLAYER_VIEW_DISTANCE,
            entity: PLAYER_VIEW_DISTANCE,
        });
    player.stage = PlayerStage::Spawning;
    tracing::info!(character_id = id, "embedded player requesting spawn");
}

/// Creates the same default humanoid Warrior the smoke bot / bot bin create.
fn create_default_character(player: &mut EmbeddedPlayer) {
    player.client.create_character(
        PLAYER_USERNAME.to_owned(),
        // Keep in sync with the smoke bot (valid_starter_items(Warrior)).
        Some("common.items.weapons.sword.starter".to_owned()),
        None,
        default_body().into(),
        false,
        None,
        comp::class::ClassKind::Warrior,
        comp::Ethos::default(),
        comp::Background::default(),
    );
    tracing::info!("embedded player roster empty; creating default character");
}

/// Same default humanoid the smoke bot / bot bin create.
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

fn first_character_id(client: &Client) -> Option<i64> {
    client
        .character_list()
        .characters
        .first()
        .and_then(|c| c.character.id)
        .map(|id| id.0)
}

/// Builds the sim's [`comp::ControllerInputs`] from the shared Bevy input
/// sample. `move_dir`/`look` are already in sim axes (x-east, y-north, z-up),
/// resolved camera-relative by the client input system — so this is a direct
/// copy with a `Dir` normalization on `look`.
fn controller_inputs_from(input: &LocalPlayerInput) -> comp::ControllerInputs {
    let move_dir = vek::Vec2::new(input.move_dir.x, input.move_dir.y);
    // Cap magnitude at 1 (diagonal keyboard input can exceed it); the sim
    // interprets |move_dir| as walk/run intent.
    let move_dir = if move_dir.magnitude() > 1.0 {
        move_dir.normalized()
    } else {
        move_dir
    };
    let look = vek::Vec3::new(input.look.x, input.look.y, input.look.z);
    let look_dir = Dir::from_unnormalized(look).unwrap_or_else(Dir::forward);
    comp::ControllerInputs {
        move_dir,
        move_z: 0.0,
        look_dir,
        break_block_pos: None,
        strafing: false,
    }
}

/// Resolves a player `Uid` to its sim `specs::Entity` inside the given sim,
/// using the sim's authoritative `IdMaps` resource. Used by the mirror to tag
/// the player's replicated entity ([`crate::mirror_sim_entities`]).
pub(crate) fn player_sim_entity(sim: &SimServer, uid: Uid) -> Option<specs::Entity> {
    sim.server
        .state()
        .ecs()
        .read_resource::<IdMaps>()
        .uid_entity(uid)
}

#[cfg(test)]
mod tests {
    use bevy::{MinimalPlugins, app::PluginGroup, math::Vec2 as BVec2, state::app::StatesPlugin};
    use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
    use xindeler_protocol::XindelerProtocolPlugin;

    use super::*;
    use crate::{SimBridgePlugin, SimEntityMirrorPlugin, boot_test_server};

    /// Pure axis-conversion check: forward keyboard intent (sim +y) survives
    /// [`controller_inputs_from`] with unit magnitude. No assets.
    #[test]
    fn forward_input_maps_to_move_dir() {
        let inputs = controller_inputs_from(&LocalPlayerInput {
            move_dir: BVec2::new(0.0, 1.0),
            jump: false,
            look: bevy::math::Vec3::new(0.0, 1.0, 0.0),
        });
        assert!((inputs.move_dir - vek::Vec2::new(0.0, 1.0)).magnitude() < 1e-5);
    }

    /// Diagonal input is clamped to unit magnitude (no faster-than-forward
    /// diagonal walk). No assets.
    #[test]
    fn diagonal_input_is_normalized() {
        let inputs = controller_inputs_from(&LocalPlayerInput {
            move_dir: BVec2::new(1.0, 1.0),
            jump: false,
            look: bevy::math::Vec3::new(0.0, 1.0, 0.0),
        });
        assert!((inputs.move_dir.magnitude() - 1.0).abs() < 1e-5);
    }

    /// EM-3.7b acceptance: boot the REAL sim + the embedded player, drive
    /// forward `ControllerInputs` programmatically for N ticks, and assert the
    /// player's `Pos` (read off the embedded Client) CHANGED horizontally —
    /// i.e. input actually moves the controllable character. `#[ignore]`
    /// (needs assets + LFS); run locally with XINDELER_ASSETS.
    #[test]
    #[ignore = "boots a real world + embedded player: needs assets + LFS; run with XINDELER_ASSETS"]
    fn embedded_player_moves_with_input() {
        const MAX_TICKS: u32 = 6000;
        const MIN_MOVED_XY: f32 = 0.5;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut sim = boot_test_server(data_dir.path()).expect("failed to boot test server");
        let player = boot_embedded_player(&mut sim).expect("failed to boot embedded player");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
                PlayerBridgePlugin,
            ))
            .finish();
        app.insert_non_send(sim);
        app.insert_non_send(player);
        // Constant forward (sim +y / north) walk.
        app.insert_resource(LocalPlayerInput {
            move_dir: BVec2::new(0.0, 1.0),
            jump: false,
            look: bevy::math::Vec3::new(0.0, 1.0, 0.0),
        });

        // Tick until the player is in game, capture the start position.
        let mut start: Option<vek::Vec3<f32>> = None;
        let mut end: Option<vek::Vec3<f32>> = None;
        for tick in 0..MAX_TICKS {
            app.update();
            let p = app.world().non_send::<EmbeddedPlayer>();
            if p.is_in_game()
                && let Some(pos) = p.position()
            {
                match start {
                    None => {
                        start = Some(pos);
                        eprintln!("player in game at {pos:?} (tick {tick})");
                    },
                    Some(s) => {
                        end = Some(pos);
                        let moved_xy = (pos.xy() - s.xy()).magnitude();
                        if moved_xy >= MIN_MOVED_XY {
                            eprintln!("player walked {moved_xy:.3} XY units by tick {tick}");
                            break;
                        }
                    },
                }
            }
        }

        let start = start.expect("embedded player never reached in-game");
        let end = end.expect("no post-spawn position sampled");
        let moved_xy = (end.xy() - start.xy()).magnitude();
        assert!(
            moved_xy >= MIN_MOVED_XY,
            "the controllable player must move with input: start {start:?} → end {end:?} \
             ({moved_xy:.3} XY units)"
        );
    }
}
