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
    ecs::{
        change_detection::{NonSend, NonSendMut},
        entity::Entity,
        query::With,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res},
    },
    math::Quat,
};
use client::{Client, ClientType, Event as ClientEvent, WorldData, addr::ConnectionArgs};
use common::{
    ViewDistances,
    clock::Clock,
    comp,
    uid::{IdMaps, Uid},
    util::Dir,
};
use specs::WorldExt;
use xindeler_protocol::{LocalPlayerInput, NetLocalPlayer, PredictedLocalTransform};

use crate::SimServer;

/// Registers the [`LocalPlayerInput`] resource, the [`tick_player`] system, and
/// [`mirror_local_player_prediction`] (main-thread non-send). Does NOT boot the
/// embedded player itself — the listen-server shell inserts an
/// [`EmbeddedPlayer`] via [`boot_embedded_player`] once the sim is up (booting
/// the Client blocks on a loopback handshake, so the shell owns the timing,
/// same as [`SimServer`]).
///
/// Add AFTER [`crate::SimBridgePlugin`].
pub struct PlayerBridgePlugin;

impl Plugin for PlayerBridgePlugin {
    fn build(&self, app: &mut App) {
        // BL-82 EM-4.11: `Update` (frame rate), NOT `FixedUpdate` — this
        // SUPERSEDES EM-3.11b's "FixedUpdate, matching `tick_sim`'s move"
        // rationale for THIS system. EM-3.11b was right that the
        // AUTHORITATIVE SERVER (`crate::tick_sim`) must stay `FixedUpdate`
        // (running its full heavy system graph at render rate was the
        // EM-3.11b regression itself: 2-5x too much work). But `tick_player`
        // is the LIGHT CLIENT PREDICTOR, not the server — old (pre-Bevy)
        // voxygen ran exactly this predictor once per rendered frame, and
        // EM-3.11b's blanket "match tick_sim's schedule" move dragged it
        // along for no reason tied to ITS OWN cost. Ticking it at frame rate
        // instead of 30 Hz is what closes the render-side tick-quantization/
        // landing-lag bug family (see `xindeler_protocol::
        // PredictedLocalTransform`'s doc comment and
        // `docs/design/specs/2026-07-11-bl82-frame-rate-prediction-design.md`).
        // `mirror_local_player_prediction` is chained directly after it (same
        // schedule now, so a plain `.chain()` orders them — no cross-schedule
        // `.after()` needed).
        app.init_resource::<LocalPlayerInput>().add_systems(
            Update,
            (tick_player, mirror_local_player_prediction).chain(),
        );
    }
}

/// Client + server tick rate for the embedded player (matches the sim's TPS and
/// the smoke bot) — used ONLY to seed [`boot_embedded_player`]'s temporary
/// background-thread pacer (real 30 Hz pacing is correct there: that thread has
/// nothing else to do while the loopback handshake completes). The
/// `EmbeddedPlayer`'s own [`Clock`] no longer paces itself to this rate (BL-82
/// EM-4.11: see [`boot_embedded_player`]'s doc for the `target_dt =
/// Duration::ZERO` change) — it now ticks once per rendered `Update` frame
/// instead.
const PLAYER_TPS: f64 = crate::SIM_TICK_HZ;

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
    /// the embedded Client. `None` before spawn. BL-82 EM-4.11: this is now
    /// the LOAD-BEARING source for the local player's rendered transform (via
    /// [`crate::mirror_local_player_prediction`] →
    /// `xindeler_protocol::PredictedLocalTransform`), not just a test/dev
    /// helper — the replicated `NetPos` remains the reconciliation truth and
    /// the source for every OTHER (remote) mirrored entity.
    pub fn position(&self) -> Option<vek::Vec3<f32>> { self.client.position() }

    /// The player entity's current velocity (sim axes), read the same way as
    /// [`Self::position`] — feeds the EM-4.11 frame-rate prediction mirror.
    /// `None` before spawn or on the rare tick the entity somehow lacks a
    /// `Vel` (defensive, mirrors `position()`'s own optionality).
    pub fn velocity(&self) -> Option<vek::Vec3<f32>> {
        self.client
            .state()
            .read_storage::<comp::Vel>()
            .get(self.client.entity())
            .map(|v| v.0)
    }

    /// The player entity's current orientation (sim axes, z-up quaternion),
    /// read the same way as [`Self::position`]. `None` before spawn / if the
    /// entity lacks an `Ori` this tick. Feeds the EM-4.11 frame-rate
    /// prediction mirror, converted to Bevy axes the same way
    /// `crate::mirror_sim_entities` converts every other mirrored entity's
    /// orientation (`crate::sim_ori_to_bevy`).
    pub fn orientation(&self) -> Option<vek::Quaternion<f32>> {
        self.client
            .state()
            .read_storage::<comp::Ori>()
            .get(self.client.entity())
            .map(|o| o.to_quat())
    }

    /// The world's coarse LOD data (`lod_alt`/`lod_horizon`/map images),
    /// downloaded during the embedded `Client`'s initial handshake — populated
    /// as soon as `Client::new` returns, i.e. available the moment an
    /// [`EmbeddedPlayer`] exists (well before `is_in_game`). EM-3.10b: source
    /// for the server → client far-terrain heightmap (`send_lod_alt_once` in
    /// `lib.rs`, which broadcasts `xindeler_protocol::NetLodAlt`).
    pub fn world_data(&self) -> &WorldData { self.client.world_data() }

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
        // BL-82 EM-4.11: `target_dt = Duration::ZERO`, NOT `1/PLAYER_TPS`.
        // `tick_player` now runs once per rendered `Update` frame (Bevy owns
        // frame pacing via present-mode/vsync), so this `Clock`'s own
        // `spin_sleep` pacing (`Clock::tick`, `common/src/clock.rs`) must be a
        // no-op — a zero `target_dt` makes `target_dt.checked_sub(busy_time)`
        // return `None` on every call (busy_time is never negative), so
        // `spin_sleep` never fires. This keeps every OTHER part of `Clock`'s
        // per-tick behaviour (the `average_dt`/`NUDGE_RATE`/`MAX_GAME_DT`
        // exponential-smoothing of the real per-frame `dt`) exactly as old
        // voxygen used it: a dt-SMOOTHER, not a tick-rate LIMITER. (A real,
        // if minor, trade-off: `Clock::new`/`set_target_dt` also seed
        // `average_dt`/`average_busy` from `target_dt`, so seeding at `ZERO`
        // means the very first ~20 post-spawn frames' smoothed dt ramps up
        // from 0 rather than a realistic guess — self-corrects within well
        // under a second and is bounded by the same reconciliation that
        // already smooths every other correction, so not worth a bespoke
        // seeding path for a one-time, sub-second startup transient.)
        clock: Clock::new(Duration::ZERO),
        stage: PlayerStage::LoadingCharacterList,
        character_id: None,
        uid: None,
        jumping: false,
    })
}

/// Advances the embedded player one step: ticks its network/sync and walks
/// the life-cycle state machine, applying [`LocalPlayerInput`] once in game.
///
/// ## BL-82 EM-4.11: `Update` (frame rate), not `FixedUpdate`
/// This used to run in `FixedUpdate` alongside [`crate::tick_sim`] (EM-3.11b).
/// That was right for the SERVER (`tick_sim`, the heavy authoritative system
/// graph — running it at render rate was the EM-3.11b regression itself) but
/// wrong for THIS system: it is the light CLIENT PREDICTOR — the SAME
/// `xindeler-client-core::Client` old (pre-Bevy) voxygen ticked once per
/// rendered frame. Now it runs in `Update`, so it advances every rendered
/// frame instead of at most once per 30 Hz sim step. `player.clock.tick()`
/// still computes a SMOOTHED `dt` (its `average_dt`/`NUDGE_RATE`/
/// `MAX_GAME_DT` machinery, unchanged) from the real per-frame time — the
/// `Clock`'s `target_dt` is `Duration::ZERO` now (see
/// [`boot_embedded_player`]'s doc), so its internal `spin_sleep` pacing is a
/// no-op: Bevy owns frame pacing (present-mode/vsync), the `Clock` here is
/// purely a dt-smoother, exactly old voxygen's per-frame `clock.game_dt()`
/// role, not a tick-rate limiter.
///
/// No-ops until the shell inserts an [`EmbeddedPlayer`] (listen-server only).
/// `Server::tick` (`crate::tick_sim`) stays in `FixedUpdate` at 30 Hz,
/// unchanged — this system reconciles against it over the loopback socket
/// INSIDE `client.tick()` below (the Client's own built-in reconciliation),
/// across frames, exactly as old singleplayer voxygen's background-thread
/// server + per-frame client reconciled; no explicit `.after(tick_sim)`
/// ordering is needed (or even meaningful — the two are different schedules
/// now) for this to work correctly.
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

/// BL-82 EM-4.11: writes the local player's frame-rate-predicted transform
/// onto its mirror entity every `Update` frame, `.after(tick_player)` (see
/// [`PlayerBridgePlugin::build`]) so it reads the freshest prediction. The
/// render (`xindeler-client::entity_view::interpolate_entities`) drives the
/// local player's `Transform` straight from
/// [`xindeler_protocol::PredictedLocalTransform`] (a snap, not an ease)
/// instead of interpolating the authoritative, 30 Hz-sampled `NetPos` the way
/// every remote entity still does — see that component's doc comment for the
/// full rationale.
///
/// No-ops until BOTH an [`EmbeddedPlayer`] is in-game AND its mirror entity
/// has already been tagged [`NetLocalPlayer`] by [`crate::mirror_sim_entities`]
/// (a `FixedUpdate` system, so right after the very first spawn it may lag
/// this `Update` system by up to one frame) — a pre-spawn/pre-mirror frame is
/// simply skipped, self-healing the next frame, exactly like every other
/// "wait for the sim to catch up" gate in this crate.
pub fn mirror_local_player_prediction(
    player: Option<NonSend<EmbeddedPlayer>>,
    // BL-82 EM-4.11 (bevy-migration-reviewer follow-up): `Option<&mut
    // PredictedLocalTransform>` so every frame AFTER the first mutates the
    // component IN PLACE (a plain World write, no deferred-command/
    // `ApplyDeferred` cost) — only the very first frame (component not yet
    // present) falls back to `Commands::insert`. This system runs every
    // rendered `Update` frame (up to 100-160+ Hz), so avoiding a
    // `Commands`-flush on the steady-state path (which would otherwise be
    // the ONLY reason `entity_view.rs`'s cross-plugin `.after()` ordering
    // needs Bevy's auto-inserted `ApplyDeferred` sync point every single
    // frame) is worth the small extra query complexity.
    mut local_player: Query<(Entity, Option<&mut PredictedLocalTransform>), With<NetLocalPlayer>>,
    mut commands: Commands,
) {
    let Some(player) = player else { return };
    if !player.is_in_game() {
        return;
    }
    let Some(pos) = player.position() else {
        return;
    };
    let Ok((entity, existing)) = local_player.single_mut() else {
        return;
    };
    let vel = player.velocity().unwrap_or_default();
    let ori = player.orientation();
    let predicted = predicted_local_transform(pos, vel, ori);
    match existing {
        Some(mut existing) => *existing = predicted,
        None => {
            commands.entity(entity).insert(predicted);
        },
    }
}

/// Sim-axis → Bevy-axis conversion for the EM-4.11 prediction mirror, factored
/// out of [`mirror_local_player_prediction`] so it (and the entity-resolution
/// logic around it) can be unit-tested without booting a real
/// [`EmbeddedPlayer`] (which needs a live sim + loopback `Client` — heavy,
/// `#[ignore]`d elsewhere in this file's tests). Uses the SAME
/// `sim_pos_to_bevy`/`sim_ori_to_bevy` helpers `crate::mirror_sim_entities`
/// converts every other mirrored entity's position/orientation with.
fn predicted_local_transform(
    pos: vek::Vec3<f32>,
    vel: vek::Vec3<f32>,
    ori: Option<vek::Quaternion<f32>>,
) -> PredictedLocalTransform {
    PredictedLocalTransform {
        pos: crate::sim_pos_to_bevy(pos),
        ori: ori.map_or(Quat::IDENTITY, crate::sim_ori_to_bevy),
        vel: crate::sim_pos_to_bevy(vel),
    }
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
    use bevy::{
        MinimalPlugins,
        app::PluginGroup,
        math::Vec2 as BVec2,
        state::app::StatesPlugin,
        time::{Fixed, Time, TimeUpdateStrategy},
    };
    use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
    use xindeler_protocol::XindelerProtocolPlugin;

    use super::*;
    use crate::{SimBridgePlugin, SimEntityMirrorPlugin, boot_test_server};

    /// BL-82 EM-4.11: `tick_player` now derives its `dt` from a REAL wall-clock
    /// `Clock` (`target_dt = Duration::ZERO`, purely a dt-smoother — see
    /// `boot_embedded_player`'s doc), not from Bevy's `Time` resource at all.
    /// A test loop that calls `app.update()` back-to-back with no real elapsed
    /// time between calls (as the pre-EM-4.11 `FixedUpdate` + `Time::<Fixed>`+
    /// `TimeUpdateStrategy::ManualDuration` pinning made possible) would starve
    /// `tick_player`'s smoothed `game_dt` toward ~0 — the SAME real-time-based
    /// smoothing that makes production frame-rate prediction work correctly
    /// requires a REAL per-frame interval in a test too. This sleeps a
    /// realistic per-tick duration (matching the sim's own 30 Hz budget, the
    /// same cadence the pre-EM-4.11 virtual-time pinning assumed) before each
    /// `app.update()`, so `Clock`'s wall-clock EMA sees a sane, repeatable dt
    /// instead of whatever the test loop's incidental CPU cost happens to be.
    fn update_with_real_frame_time(app: &mut App) {
        std::thread::sleep(Duration::from_secs_f64(1.0 / crate::SIM_TICK_HZ));
        app.update();
    }

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
        // EM-3.11b: `tick_sim`/`tick_player` now run in `FixedUpdate` — pin
        // the step to exactly `SIM_TICK_HZ` and feed a matching real-time
        // delta each `app.update()` (see `SimBridgePlugin`'s tests in
        // `lib.rs` for the full rationale) so this stays a per-tick loop.
        app.insert_resource(Time::<Fixed>::from_hz(crate::SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / crate::SIM_TICK_HZ,
        )));
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
            update_with_real_frame_time(&mut app);
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

    /// EM-3.11b regression test (BL-82 jump investigation): boots the REAL
    /// sim plus embedded player exactly as `--listen-server` does, drives a
    /// press-hold-release jump sequence through [`tick_player`] (the same
    /// `LocalPlayerInput` → `handle_input(InputKind::Jump, ..)` edge-detect
    /// path a real click uses), and asserts the sim-authoritative `Pos.z`
    /// actually rises — i.e. the impulse is applied end-to-end through the
    /// network/character-behavior/physics chain, not just queued.
    ///
    /// ## What this proved (and what it did NOT)
    /// This test PASSES against an unmodified `player.rs`/legacy
    /// `common::states::utils::handle_jump`: the player's `Vel.z` gets set to
    /// the expected impulse (`0.4 * GRAVITY`, confirmed via
    /// `Controller.queued_inputs`/`PhysicsState.on_ground` tracing while
    /// developing this test) the tick after the `StartInput(Jump)` message
    /// reaches the embedded loopback `Server`, and `Pos.z` rises well past
    /// the ground-clearance threshold before falling back and re-landing. In
    /// other words: **the sim-side jump plumbing this crate owns
    /// (`tick_player`'s edge-detect, the embedded `Client`→loopback
    /// `Server`→`Controller.queued_inputs`→`handle_jump`→`LocalEvent::Jump`→
    /// `Vel.z` chain) is correct.** It does NOT cover — and therefore does
    /// NOT rule out — a bug upstream of `LocalPlayerInput` (i.e.
    /// `xindeler-client`'s `gather_input`, which reads the real
    /// keyboard/cursor-grab state; this crate cannot depend on that crate,
    /// see the module isolation law) or a purely visual/perception issue
    /// (the jump is fast — well under a second — so it may just be hard to
    /// notice at low fps). If jump still looks broken in play after this
    /// test passes, look there next, not here.
    #[test]
    #[ignore = "boots a real world + embedded player: needs assets + LFS; run with VELOREN_ASSETS"]
    fn jump_edge_raises_player_z() {
        const MAX_SETTLE_TICKS: u32 = 6000;
        /// Extra ticks to run once in-game before starting the jump, so the
        /// character finishes falling onto the terrain and `on_ground` has
        /// gone `Some` (a fresh spawn starts slightly above the ground).
        const SETTLE_GRACE_TICKS: u32 = 90;
        const JUMP_HOLD_TICKS: u32 = 30;
        const POST_RELEASE_TICKS: u32 = 60;
        /// Minimum rise (metres) that counts as "the jump visibly happened".
        /// The observed rise while developing this test was ~1.9 m (a
        /// default-scale humanoid's `jump_impulse`); 0.3 m has ample margin
        /// over both jitter and any future balance retune.
        const MIN_RISE: f32 = 0.3;

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
        app.insert_resource(LocalPlayerInput {
            move_dir: BVec2::ZERO,
            jump: false,
            look: bevy::math::Vec3::new(0.0, 1.0, 0.0),
        });

        // Settle: wait for in-game, then a short fixed grace period. Breaks
        // out as soon as both are satisfied instead of always burning the
        // full `MAX_SETTLE_TICKS` budget (that budget is only a safety net
        // against never reaching in-game at all).
        let mut in_game_at: Option<u32> = None;
        let mut settled: Option<vek::Vec3<f32>> = None;
        for tick in 0..MAX_SETTLE_TICKS {
            update_with_real_frame_time(&mut app);
            let p = app.world().non_send::<EmbeddedPlayer>();
            if p.is_in_game()
                && let Some(pos) = p.position()
            {
                in_game_at.get_or_insert(tick);
                settled = Some(pos);
                if let Some(start_tick) = in_game_at
                    && tick >= start_tick + SETTLE_GRACE_TICKS
                {
                    break;
                }
            }
        }
        let start = settled.expect("embedded player never reached in-game");

        // Press jump (mirrors a real click: `LocalPlayerInput.jump` flips
        // `true`, `tick_player`'s edge-detect sends `StartInput(Jump)` once).
        app.insert_resource(LocalPlayerInput {
            move_dir: BVec2::ZERO,
            jump: true,
            look: bevy::math::Vec3::new(0.0, 1.0, 0.0),
        });
        let mut max_z = start.z;
        for _ in 0..JUMP_HOLD_TICKS {
            update_with_real_frame_time(&mut app);
            if let Some(pos) = app.world().non_send::<EmbeddedPlayer>().position() {
                max_z = max_z.max(pos.z);
            }
        }

        // Release jump (edge-detect sends `CancelInput(Jump)` once) and keep
        // sampling while the character falls back to the ground.
        app.insert_resource(LocalPlayerInput {
            move_dir: BVec2::ZERO,
            jump: false,
            look: bevy::math::Vec3::new(0.0, 1.0, 0.0),
        });
        for _ in 0..POST_RELEASE_TICKS {
            update_with_real_frame_time(&mut app);
            if let Some(pos) = app.world().non_send::<EmbeddedPlayer>().position() {
                max_z = max_z.max(pos.z);
            }
        }

        assert!(
            max_z > start.z + MIN_RISE,
            "expected the player to rise off the ground while jump was held: start.z={:.3} \
             max.z={:.3} (rose {:.3} m, needed >{MIN_RISE} m)",
            start.z,
            max_z,
            max_z - start.z
        );
    }

    /// BL-82 EM-4.11: [`predicted_local_transform`]'s sim→Bevy axis conversion
    /// matches `crate::sim_pos_to_bevy`/`crate::sim_ori_to_bevy` exactly (the
    /// SAME helpers `crate::mirror_sim_entities` uses for every other mirrored
    /// entity): `(x, y, z) -> (x, z, -y)`. No assets, no boot.
    #[test]
    fn predicted_local_transform_converts_sim_axes_to_bevy() {
        let sim_pos = vek::Vec3::new(10.0, 20.0, 5.0);
        let sim_vel = vek::Vec3::new(1.0, 2.0, 0.0);

        let predicted = predicted_local_transform(sim_pos, sim_vel, None);

        assert_eq!(predicted.pos, bevy::math::Vec3::new(10.0, 5.0, -20.0));
        assert_eq!(predicted.vel, bevy::math::Vec3::new(1.0, 0.0, -2.0));
        assert_eq!(
            predicted.ori,
            Quat::IDENTITY,
            "no orientation sample falls back to identity, same as position()'s None handling"
        );
    }

    /// BL-82 EM-4.11 regression: given a mirror entity already tagged
    /// [`NetLocalPlayer`], the write path [`mirror_local_player_prediction`]
    /// uses (entity resolution via `Query<Entity, With<NetLocalPlayer>>` +
    /// `Commands::insert`) lands a [`PredictedLocalTransform`] with the
    /// correct axis-converted value on exactly that entity. Exercises the
    /// SAME entity-resolution + insert call `mirror_local_player_prediction`
    /// makes, without needing a real (heavy, network-booted) [`EmbeddedPlayer`]
    /// to source the pose from — see that function's doc comment.
    #[test]
    fn writes_predicted_local_transform_onto_the_tagged_mirror_entity() {
        use bevy::{MinimalPlugins, app::App};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let entity = app.world_mut().spawn(NetLocalPlayer).id();

        // Mirrors `mirror_local_player_prediction`'s own first-frame-inserts/
        // subsequent-frames-mutate-in-place split (BL-82 EM-4.11 perf
        // follow-up), without needing a real `EmbeddedPlayer` — see that
        // function's doc comment for why a stub is used here.
        fn write_stub_prediction(
            mut local_player: Query<
                (Entity, Option<&mut PredictedLocalTransform>),
                With<NetLocalPlayer>,
            >,
            mut commands: Commands,
            mut sim_pos: bevy::ecs::system::Local<f32>,
        ) {
            let Ok((entity, existing)) = local_player.single_mut() else {
                return;
            };
            // A different position each call, so the mutate-in-place test
            // below can tell "inserted once" apart from "updated again".
            *sim_pos += 1.0;
            let stub = predicted_local_transform(
                vek::Vec3::new(*sim_pos, 20.0, 5.0),
                vek::Vec3::new(1.0, 2.0, 0.0),
                None,
            );
            match existing {
                Some(mut existing) => *existing = stub,
                None => {
                    commands.entity(entity).insert(stub);
                },
            }
        }
        app.add_systems(Update, write_stub_prediction);
        app.update();

        let first = app
            .world()
            .get::<PredictedLocalTransform>(entity)
            .expect("PredictedLocalTransform must be written onto the NetLocalPlayer entity")
            .pos;
        assert_eq!(first, bevy::math::Vec3::new(1.0, 5.0, -20.0));

        // A second frame must MUTATE the existing component in place (no
        // re-`insert`, no stale value left over) — the in-place path.
        app.update();
        let second = app
            .world()
            .get::<PredictedLocalTransform>(entity)
            .expect("PredictedLocalTransform must still be present")
            .pos;
        assert_eq!(
            second,
            bevy::math::Vec3::new(2.0, 5.0, -20.0),
            "the second frame must update the SAME component in place, not leave the first \
             frame's value behind"
        );
    }
}
