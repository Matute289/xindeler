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

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use bevy::{
    app::{App, Plugin, Update},
    ecs::{
        change_detection::{NonSend, NonSendMut},
        entity::Entity,
        query::With,
        schedule::IntoScheduleConfigs,
        system::{Commands, Local, Query, Res},
    },
    math::Quat,
};
use client::{Client, ClientType, Event as ClientEvent, WorldData, addr::ConnectionArgs};
use common::{
    ViewDistances,
    clock::Clock,
    comp,
    comp::invite::InviteKind,
    rtsim,
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

/// Default upper safety ceiling on [`tick_player`]'s dispatch rate, in Hz.
///
/// ## Why this exists (BL-82 EM-4.11 follow-up, rust-perf-reviewer MAJOR)
/// EM-4.11 (PR #59) deliberately moved `tick_player` from `FixedUpdate` (30 Hz)
/// to `Update` (render-frame rate) — that move is CORRECT and must stay: it is
/// what closes the tick-quantization/landing-lag bug family (see
/// `docs/design/specs/2026-07-11-bl82-frame-rate-prediction-design.md`).
/// Running the embedded predictor at a normal monitor's refresh rate
/// (60/120/144 Hz) is the intended behaviour, not a bug. The spec's own §3
/// risk list even names this exact possibility ("if a machine can't afford
/// it, the client tick can be clamped to a max Hz") but the clamp itself was
/// never implemented — a genuine gap: with `XINDELER_PRESENT_MODE=novsync`
/// (uncapped) or on a 240 Hz+ display, `Update` (and therefore this system)
/// can run far more often than any perceptible prediction benefit justifies,
/// spending CPU on a full `client.tick()` dispatch (interpolation, tether,
/// mount, controller, character_behavior, buff, stats, phys(+events),
/// projectile, shockwave, arcing, beam, pool, aura, telemetry, a terrain
/// scan/prune, and two network sends — see `common_systems::add_local_systems`
/// and this module's doc comment) with no upside.
///
/// 240 Hz comfortably covers every normal monitor (60/120/144 Hz) with
/// headroom — the clamp never engages at those refresh rates, so this is
/// purely a ceiling against the uncapped/very-high-refresh case, not a cap
/// back toward 30 Hz (which would resurrect the EM-4.11 bug). See
/// [`max_player_tick_interval`] for the env override.
const DEFAULT_MAX_PLAYER_TICK_HZ: f64 = 240.0;

/// Resolves the minimum wall-clock interval [`tick_player`] must wait between
/// successive real `client.tick()` dispatches, from
/// `XINDELER_MAX_PLAYER_TICK_HZ` (Hz) — falls back to
/// [`DEFAULT_MAX_PLAYER_TICK_HZ`] if unset or unparseable (same "bogus value
/// never panics, just uses the safe default" convention as
/// `xindeler_client::present_mode_from_env`). A value `<= 0` is an EXPLICIT
/// opt-out (returns `Duration::ZERO`, meaning "no ceiling") so the clamp can be
/// disabled for testing/profiling without a rebuild — `tick_player` treats a
/// zero interval as "always eligible".
///
/// ## Setting this well below the default is a manual, lossy escape hatch
/// The 240 Hz default never engages at any normal refresh rate, so this only
/// matters if you deliberately override it low for testing (as the empirical
/// verification for this ceiling did, e.g. `=20`). Two things are worth
/// knowing before doing that (rust-perf-reviewer / bevy-migration-reviewer,
/// BL-82 EM-4.11 follow-up review):
/// - `player.clock`'s `MAX_GAME_DT` clamp (`common/src/clock.rs`, 0.2 s) caps
///   the simulated step on the next real dispatch well below the actual elapsed
///   gap at very low Hz (e.g. 1-5 Hz) — the embedded predictor effectively runs
///   in slow motion rather than "catching up", which is expected for a manual
///   override but easy to mistake for a bug.
/// - `LocalPlayerInput` (`xindeler-client`'s `gather_input`) is written as
///   continuous level-state every `Update` frame, with no edge-buffering across
///   skipped frames — a very brief input (e.g. a fast jump tap) shorter than a
///   deliberately-low override's gate interval can be missed entirely. This is
///   a non-issue at the shipped default (which never skips at normal input
///   timescales) and an acceptable tradeoff for an opt-in low-Hz test knob, but
///   not for a production ceiling value.
fn max_player_tick_interval() -> Duration {
    match std::env::var("XINDELER_MAX_PLAYER_TICK_HZ")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        Some(hz) if hz > 0.0 => Duration::from_secs_f64(1.0 / hz),
        Some(_) => Duration::ZERO,
        None => Duration::from_secs_f64(1.0 / DEFAULT_MAX_PLAYER_TICK_HZ),
    }
}

/// Pure predicate for [`tick_player`]'s Hz ceiling: given the wall-clock
/// instant of the last REAL dispatch (`None` if there has never been one),
/// the current instant, and the minimum interval required between
/// dispatches, returns whether a dispatch is due now. `max_interval ==
/// Duration::ZERO` means "no ceiling" (always due — the
/// `XINDELER_MAX_PLAYER_TICK_HZ<=0` opt-out). Factored out of `tick_player`
/// so the gating logic is unit-testable with plain `Instant`/`Duration` math,
/// without booting a real `EmbeddedPlayer` (needs a live sim + loopback
/// `Client` — heavy, `#[ignore]`d elsewhere in this file's tests).
fn tick_is_due(last: Option<Instant>, now: Instant, max_interval: Duration) -> bool {
    if max_interval.is_zero() {
        return true;
    }
    match last {
        Some(last) => now.duration_since(last) >= max_interval,
        None => true,
    }
}

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
    /// Wall-clock instant of the last REAL `client.tick()` dispatch, used by
    /// [`tick_player`]'s Hz safety ceiling (BL-82 EM-4.11 follow-up — see
    /// [`max_player_tick_interval`]). `None` until the first dispatch ever
    /// runs, so the ceiling never blocks the initial tick.
    last_tick_wall: Option<Instant>,
    /// BL-82 EM-5.8: NPC-initiated dialogue turns (`ClientEvent::Dialogue`)
    /// captured by [`capture_social_events`] each [`tick_player`] dispatch —
    /// `xindeler_sim_bridge::social::mirror_dialogue` drains this via
    /// [`Self::take_pending_dialogue`] to project `xindeler_protocol::
    /// NetDialogue`. A `Vec` (not a single `Option`) so a burst of multiple
    /// dialogue turns arriving within one skipped-frame gap (see
    /// [`tick_player`]'s Hz-ceiling doc comment) is never silently dropped.
    pending_dialogue: Vec<(Uid, rtsim::Dialogue<true>)>,
    /// Chat lines the embedded Client received THIS tick (BL-82 EM-5.4),
    /// captured from `client.tick()`'s returned frontend events by
    /// [`tick_player`] (the only place those events surface) and drained by
    /// `xindeler-sim-bridge::chat::broadcast_embedded_chat` right after, so
    /// this never grows unbounded even across the same-frame ordering.
    pending_chat: Vec<comp::ChatMsg>,
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

    /// The world's coarse LOD data (`lod_base`/`lod_alt`/`lod_horizon`/map
    /// images), downloaded during the embedded `Client`'s initial handshake —
    /// populated as soon as `Client::new` returns, i.e. available the moment
    /// an [`EmbeddedPlayer`] exists (well before `is_in_game`). EM-3.10b
    /// (+ BL-82 EM-3.11 Phase A colour): source for the server → client
    /// far-terrain grid (`send_far_terrain_once` in `lib.rs`, which
    /// broadcasts `xindeler_protocol::NetFarTerrain`).
    pub fn world_data(&self) -> &WorldData { self.client.world_data() }

    /// Drains every NPC-initiated dialogue turn captured since the last call
    /// (BL-82 EM-5.8) — see [`Self::pending_dialogue`]'s own doc comment.
    pub fn take_pending_dialogue(&mut self) -> Vec<(Uid, rtsim::Dialogue<true>)> {
        std::mem::take(&mut self.pending_dialogue)
    }

    /// Sends a real group invite to `invitee` over the embedded `Client`'s
    /// network connection (BL-82 EM-5.8) — `client::Client::send_invite`
    /// itself pushes a `ControlEvent::InitiateInvite` that the sim's
    /// `InitiateInviteEvent` handler processes, exactly the same path a
    /// genuinely remote client's invite takes. A no-op before the player is
    /// in-game (mirrors every other action method below).
    pub fn send_group_invite(&mut self, invitee: Uid, kind: InviteKind) {
        if self.is_in_game() {
            self.client.send_invite(invitee, kind);
        }
    }

    /// Accepts the local player's currently outstanding incoming invite, if
    /// any (`client::Client::invite()`'s own target) — a no-op if there is
    /// none (the sim/client itself already guards this; this is just a
    /// defensive early-out matching this module's "in-game only" posture).
    pub fn accept_invite(&mut self) {
        if self.is_in_game() {
            self.client.accept_invite();
        }
    }

    /// Declines the local player's currently outstanding incoming invite, if
    /// any.
    pub fn decline_invite(&mut self) {
        if self.is_in_game() {
            self.client.decline_invite();
        }
    }

    /// Leaves the local player's current group, if any.
    pub fn leave_group(&mut self) {
        if self.is_in_game() {
            self.client.leave_group();
        }
    }

    /// Requests kicking `member` from the local player's group — the sim
    /// itself enforces the leader-only permission check
    /// (`server::events::group_manip`); a non-leader's request is simply
    /// rejected server-side, never trusted client-side.
    pub fn kick_from_group(&mut self, member: Uid) {
        if self.is_in_game() {
            self.client.kick_from_group(member);
        }
    }

    /// Requests handing group leadership to `member` — same server-side
    /// permission enforcement note as [`Self::kick_from_group`].
    pub fn assign_group_leader(&mut self, member: Uid) {
        if self.is_in_game() {
            self.client.assign_group_leader(member);
        }
    }

    /// Sends a dialogue turn (an Ack/Response, or a fresh `Start`) to
    /// `target` (BL-82 EM-5.8), resolving `target`'s sim `Entity` via the
    /// embedded `Client`'s own `IdMaps` (the SAME sim `target` lives in —
    /// the embedded player is a loopback client to this exact `SimServer`).
    /// Returns `false` (a no-op, logged) if `target` doesn't resolve to a
    /// live entity — e.g. the NPC despawned/moved out of view between the UI
    /// rendering the prompt and the player answering it.
    pub fn perform_dialogue(&mut self, target: Uid, dialogue: rtsim::Dialogue) -> bool {
        if !self.is_in_game() {
            return false;
        }
        let Some(entity) = self
            .client
            .state()
            .ecs()
            .read_resource::<IdMaps>()
            .uid_entity(target)
        else {
            tracing::warn!(
                ?target,
                "perform_dialogue: target Uid no longer resolves to a live sim entity"
            );
            return false;
        };
        self.client.perform_dialogue(entity, dialogue);
        true
    }

    /// Every currently-known site + extra marker (BL-82 EM-5.5) — kind/wpos/
    /// label/quest-flag, verbatim [`client::Client::markers`]. Populated the
    /// same moment [`Self::world_data`] is (the initial handshake), so it's
    /// available well before [`Self::is_in_game`]. Source for the one-shot
    /// `NetMapData` broadcast (`xindeler-sim-bridge::map::send_map_data_once`).
    pub fn markers(&self) -> impl Iterator<Item = &common::map::Marker> { self.client.markers() }

    /// Named terrain features (peaks/lakes) — verbatim [`client::Client::
    /// pois`]. Same availability as [`Self::markers`].
    pub fn pois(&self) -> &[common_net::msg::world_msg::PoiInfo] { self.client.pois() }

    fn character_jumping(&self) -> bool { self.jumping }

    fn set_character_jumping(&mut self, jumping: bool) { self.jumping = jumping; }

    /// Takes every chat line captured this tick (BL-82 EM-5.4), leaving the
    /// internal queue empty — called once per frame by
    /// `xindeler-sim-bridge::chat::broadcast_embedded_chat`, right after
    /// [`tick_player`] populates it (see [`Self::pending_chat`]'s doc).
    pub(crate) fn drain_pending_chat(&mut self) -> Vec<comp::ChatMsg> {
        std::mem::take(&mut self.pending_chat)
    }

    /// Applies a client → server chat/command send request (BL-82 EM-5.4) to
    /// the embedded Client's REAL network connection — the send-side
    /// counterpart to [`Self::drain_pending_chat`]. Exactly mirrors how
    /// movement (`controller_inputs_from` → `client.tick`) and jump
    /// (`client.handle_input`) already reach the sim: a genuine call through
    /// `client::Client`'s own public API (`send_command`), which sends a real
    /// `ClientGeneral` message over the loopback socket to the embedded
    /// `Server` — never a direct sim-state write (isolation law). All the
    /// actual branching/validation lives in [`resolve_chat_send`] (a pure
    /// function, unit-tested without a live `Client` — see its own doc
    /// comment); this method is a thin `send_command` applicator.
    pub(crate) fn send_chat_request(&mut self, request: &xindeler_protocol::ChatSendRequest) {
        if let Some((name, args)) = resolve_chat_send(self.stage == PlayerStage::InGame, request) {
            self.client.send_command(name, args);
        }
    }
}

/// Pure decision logic for [`EmbeddedPlayer::send_chat_request`] (BL-82
/// EM-5.4 follow-up, reviewer-flagged testability gap): given whether the
/// embedded player is currently in-game and the request to apply, returns
/// the `(name, args)` to hand to `client::Client::send_command`, or `None`
/// if nothing should be sent. Factored out so this branching is
/// unit-testable directly, matching this file's own established convention
/// for the same reason (`controller_inputs_from`/`tick_is_due`/
/// `predicted_local_transform` are all pure functions carved out of a
/// method/system that otherwise needs a live `Client`/`EmbeddedPlayer`).
///
/// `None` cases: not yet [`PlayerStage::InGame`] (nothing sane to attribute
/// the message to yet); a [`ChatSendRequest::Channel`] whose text is empty/
/// whitespace-only, or whose channel is one of the three receive-only kinds
/// ([`NetChatChannel::Tell`]/[`NetChatChannel::Npc`]/
/// [`NetChatChannel::System`] — `channel.send_command_name()` already
/// returns `None` for exactly these, so no separate match arm is needed); a
/// [`ChatSendRequest::Command`] whose name is empty/whitespace-only.
fn resolve_chat_send(
    in_game: bool,
    request: &xindeler_protocol::ChatSendRequest,
) -> Option<(String, Vec<String>)> {
    use xindeler_protocol::ChatSendRequest;

    if !in_game {
        return None;
    }

    match request {
        ChatSendRequest::Channel { channel, text } => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            let name = channel.send_command_name()?;
            Some((name.to_owned(), vec![text.to_owned()]))
        },
        ChatSendRequest::Command { name, args } => {
            if name.trim().is_empty() {
                return None;
            }
            Some((name.clone(), args.clone()))
        },
    }
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
        last_tick_wall: None,
        pending_dialogue: Vec::new(),
        pending_chat: Vec::new(),
    })
}

/// Opt-in real-dispatch-rate diagnostic for [`tick_player`]'s Hz ceiling
/// (BL-82 EM-4.11 follow-up), gated on `XINDELER_PLAYER_TICK_PERF_LOG=1` (same
/// "cached bool, off by default" convention as `XINDELER_CULL_PERF_LOG` /
/// `XINDELER_SPRITE_PERF_LOG` elsewhere in this codebase). Counts real
/// dispatches vs. ceiling-skipped `Update` frames over a rolling ~1 s window
/// and logs the measured Hz — `target: "player_tick_perf"` so a plain `grep
/// player_tick_perf` on any run's log (e.g. a `--smoke-perf-run` with
/// `XINDELER_PRESENT_MODE=novsync`) gives the empirical dispatch rate
/// directly, without a debugger or a code change.
///
/// `pub(crate)`: it appears in `tick_player`'s signature as a `Local<Option<
/// TickPerfLog>>` system parameter, so it must be at least as reachable as
/// that function within the crate (`private_interfaces` lint) — `tick_player`
/// itself is `pub(crate)` too (bevy-migration-reviewer audit: no external
/// crate actually calls it, only doc-comment prose mentions it; tightening
/// both together avoids growing the crate's real public API surface for a
/// pure diagnostic type). Its fields stay private; nothing outside this
/// module constructs or reads one directly.
pub(crate) struct TickPerfLog {
    enabled: bool,
    window_start: Instant,
    ticked: u32,
    skipped: u32,
}

/// Rolling window length for [`TickPerfLog`]'s Hz summary — long enough to
/// average out single-frame jitter, short enough to react quickly in a
/// manual smoke run.
const TICK_PERF_LOG_WINDOW: Duration = Duration::from_secs(1);

/// Updates (and, on the `XINDELER_PLAYER_TICK_PERF_LOG=1` path, logs) the
/// rolling real-tick-rate window. `state` starts `None` (first call per
/// `tick_player` instance) and is initialized here; a disabled log still pays
/// only the one-time env read (cached via the `enabled` flag) plus a no-op
/// early return, matching every other `*_PERF_LOG` in this codebase.
fn log_tick_perf(state: &mut Option<TickPerfLog>, ticked: bool, now: Instant) {
    let state = state.get_or_insert_with(|| TickPerfLog {
        enabled: std::env::var("XINDELER_PLAYER_TICK_PERF_LOG").is_ok_and(|v| v != "0"),
        window_start: now,
        ticked: 0,
        skipped: 0,
    });
    if !state.enabled {
        return;
    }
    if ticked {
        state.ticked += 1;
    } else {
        state.skipped += 1;
    }
    let elapsed = now.duration_since(state.window_start);
    if elapsed >= TICK_PERF_LOG_WINDOW {
        let secs = elapsed.as_secs_f64();
        tracing::info!(
            target: "player_tick_perf",
            real_ticks = state.ticked,
            skipped_frames = state.skipped,
            measured_tick_hz = state.ticked as f64 / secs,
            window_secs = secs,
            "tick_player Hz-ceiling window"
        );
        state.window_start = now;
        state.ticked = 0;
        state.skipped = 0;
    }
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
///
/// ## BL-82 EM-4.11 follow-up: Hz safety ceiling
/// This does NOT cap back toward 30 Hz and does NOT revert the `Update` move
/// above — both remain correct and intentional. It adds an upper-bound-only
/// guard (default [`DEFAULT_MAX_PLAYER_TICK_HZ`] = 240 Hz, comfortably above
/// any normal monitor's refresh rate) so an uncapped render loop
/// (`XINDELER_PRESENT_MODE=novsync`) or a 240 Hz+ display can't run this
/// system's full dispatch — and its `client.tick()` system graph — more
/// often than any perceptible benefit justifies. See
/// [`max_player_tick_interval`] for the `XINDELER_MAX_PLAYER_TICK_HZ`
/// override.
///
/// ### Verifying the ceiling empirically
/// Set `XINDELER_PLAYER_TICK_PERF_LOG=1` (see [`TickPerfLog`]) to log the
/// measured real-dispatch Hz once per second — e.g. pair it with
/// `--smoke-perf-run` and `XINDELER_PRESENT_MODE=novsync` and `grep
/// player_tick_perf` the output to confirm the dispatch rate stays bounded
/// near the ceiling even when the render loop itself runs much faster.
///
/// `pub(crate)` (tightened from a pre-existing `pub` + crate-root re-export
/// during this follow-up, bevy-migration-reviewer audit): no crate outside
/// `xindeler-sim-bridge` actually calls this — `PlayerBridgePlugin::build`
/// registers it in this same module, and every other mention across
/// `xindeler-client` is plain doc-comment prose, not a resolved cross-crate
/// reference. Keeping it crate-internal avoids growing the public API
/// surface further (this follow-up's new [`TickPerfLog`] system parameter
/// would otherwise have needed the same wider `pub` + re-export treatment).
pub(crate) fn tick_player(
    player: Option<NonSendMut<EmbeddedPlayer>>,
    input: Res<LocalPlayerInput>,
    mut max_tick_interval: Local<Option<Duration>>,
    mut tick_perf_log: Local<Option<TickPerfLog>>,
) {
    let Some(mut player) = player else { return };

    // BL-82 EM-4.11 follow-up (rust-perf-reviewer MAJOR): Hz safety ceiling.
    // `Update` can run far faster than any perceptible prediction benefit
    // justifies (uncapped `XINDELER_PRESENT_MODE=novsync`, or a 240 Hz+
    // display) — see [`DEFAULT_MAX_PLAYER_TICK_HZ`]'s doc for the full
    // rationale. Read (and cache) the env override once via `Local`, same
    // "resolve once, reuse every frame" shape `far_terrain.rs`/`lod.rs` use
    // for their own per-frame env-gated flags.
    let max_interval = *max_tick_interval.get_or_insert_with(max_player_tick_interval);
    let now = Instant::now();
    let due = tick_is_due(player.last_tick_wall, now, max_interval);
    log_tick_perf(&mut tick_perf_log, due, now);
    if !due {
        // Under the ceiling: skip this frame's dispatch entirely. Bevy still
        // renders the frame — we simply don't run the redundant
        // interpolation/tether/mount/controller/character_behavior/buff/
        // stats/phys(+events)/projectile/shockwave/arcing/beam/pool/aura/
        // telemetry system graph, the terrain scan/prune, or the two network
        // sends `client.tick` performs (see this module's doc comment).
        // `mirror_local_player_prediction` (chained right after this system)
        // still runs and simply re-reads the unchanged
        // `EmbeddedPlayer::position()`/velocity/orientation from the last
        // real tick — a cheap no-op write, not a correctness gap.
        return;
    }
    player.last_tick_wall = Some(now);

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
    // BL-82 EM-5.8: stash any NPC-initiated dialogue turns this tick's
    // events carried — see `capture_social_events`'s own doc comment.
    capture_social_events(&mut player, &events);
    // BL-82 EM-5.4: stash any `Event::Chat` this tick's dispatch produced —
    // `client.tick()`'s returned events are the ONLY place they surface, so
    // this capture must live here; `xindeler-sim-bridge::chat::
    // broadcast_embedded_chat` drains + broadcasts them right after (same
    // `Update` frame, chained after this system).
    collect_chat_events(&mut player, &events);
}

/// Appends every `ClientEvent::Chat` this frame's dispatch produced onto
/// [`EmbeddedPlayer::pending_chat`] — a small, pure-ish helper factored out
/// of [`tick_player`] so the capture step reads as one line there.
fn collect_chat_events(player: &mut EmbeddedPlayer, events: &[ClientEvent]) {
    player
        .pending_chat
        .extend(events.iter().filter_map(|event| match event {
            ClientEvent::Chat(msg) => Some(msg.clone()),
            _ => None,
        }));
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

/// BL-82 EM-5.8: scans this frame's `client.tick()` events for
/// `ClientEvent::Dialogue` (an NPC addressing the local player — quest/
/// dialogue, v1-minimal) and stashes each one in
/// [`EmbeddedPlayer::pending_dialogue`] for `xindeler_sim_bridge::social::
/// mirror_dialogue` to drain and project as `xindeler_protocol::NetDialogue`.
/// Called right after [`advance_stage`] in [`tick_player`] — a small,
/// additive scan over the same `events` slice that function already
/// iterates, not a second `client.tick()` dispatch.
fn capture_social_events(player: &mut EmbeddedPlayer, events: &[ClientEvent]) {
    for event in events {
        if let ClientEvent::Dialogue(sender, dialogue) = event {
            player.pending_dialogue.push((*sender, dialogue.clone()));
        }
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

    /// BL-82 EM-4.11 follow-up (rust-perf-reviewer MAJOR): pins the pure
    /// `XINDELER_MAX_PLAYER_TICK_HZ` → `Duration` mapping so the default Hz
    /// ceiling never silently drifts, and the disable/override paths keep
    /// working. Runs single-threaded within this one test (env vars are
    /// process-wide state), same convention as
    /// `xindeler_client::present_mode_tests`.
    #[test]
    fn max_player_tick_interval_defaults_and_respects_overrides() {
        // SAFETY: this test is the sole reader/writer of
        // `XINDELER_MAX_PLAYER_TICK_HZ` in this crate's test suite, and every
        // set/assert/remove step below runs sequentially within this one test
        // function, so there is no cross-thread data race on the var.
        unsafe {
            std::env::remove_var("XINDELER_MAX_PLAYER_TICK_HZ");
        }
        assert_eq!(
            max_player_tick_interval(),
            Duration::from_secs_f64(1.0 / DEFAULT_MAX_PLAYER_TICK_HZ),
            "unset XINDELER_MAX_PLAYER_TICK_HZ must resolve to the default ceiling"
        );

        // SAFETY: see justification above.
        unsafe {
            std::env::set_var("XINDELER_MAX_PLAYER_TICK_HZ", "not-a-number");
        }
        assert_eq!(
            max_player_tick_interval(),
            Duration::from_secs_f64(1.0 / DEFAULT_MAX_PLAYER_TICK_HZ),
            "an unparseable value must fall back to the safe default, not panic"
        );

        // SAFETY: see justification above.
        unsafe {
            std::env::set_var("XINDELER_MAX_PLAYER_TICK_HZ", "120");
        }
        assert_eq!(
            max_player_tick_interval(),
            Duration::from_secs_f64(1.0 / 120.0),
            "a valid override must be honoured exactly"
        );

        for opt_out in ["0", "-1"] {
            // SAFETY: see justification above.
            unsafe {
                std::env::set_var("XINDELER_MAX_PLAYER_TICK_HZ", opt_out);
            }
            assert_eq!(
                max_player_tick_interval(),
                Duration::ZERO,
                "{opt_out:?} must be an explicit opt-out (no ceiling), not clamped to 0 Hz"
            );
        }

        // SAFETY: see justification above; leave the environment clean.
        unsafe {
            std::env::remove_var("XINDELER_MAX_PLAYER_TICK_HZ");
        }
    }

    /// BL-82 EM-4.11 follow-up: [`tick_is_due`]'s pure gating logic — the
    /// piece that actually decides whether `tick_player` runs its full
    /// dispatch or skips a frame. Verifies all four branches with real
    /// `Instant`/`Duration` math (no mock clock needed): first-ever call is
    /// always due; a call too soon after the last real tick is NOT due;
    /// waiting out the interval makes it due again; `Duration::ZERO` (the
    /// `XINDELER_MAX_PLAYER_TICK_HZ<=0` opt-out) is always due regardless of
    /// timing.
    #[test]
    fn tick_is_due_gates_on_elapsed_wall_clock_time() {
        let max_interval = Duration::from_millis(20);
        let last = Instant::now();

        assert!(
            tick_is_due(None, Instant::now(), max_interval),
            "the very first dispatch (no prior tick) must never be blocked by the ceiling"
        );
        assert!(
            !tick_is_due(Some(last), Instant::now(), max_interval),
            "called again immediately, well under the interval, must NOT be due yet"
        );

        std::thread::sleep(max_interval + Duration::from_millis(15));
        assert!(
            tick_is_due(Some(last), Instant::now(), max_interval),
            "after the interval has elapsed, a dispatch must be due again"
        );

        assert!(
            !tick_is_due(Some(Instant::now()), Instant::now(), Duration::from_secs(1)),
            "sanity: a huge interval right after a tick must gate (proves the assertion above \
             isn't vacuously true)"
        );
        assert!(
            tick_is_due(Some(last), Instant::now(), Duration::ZERO),
            "Duration::ZERO must always be due — the explicit opt-out disables the ceiling \
             entirely, regardless of elapsed time"
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

    /// BL-82 EM-5.4 follow-up (bevy-migration-reviewer / ecs-design-reviewer
    /// MINOR: `send_chat_request`'s branching had no direct test). Not
    /// in-game yet: nothing is sent regardless of the request's shape.
    #[test]
    fn resolve_chat_send_is_none_before_in_game() {
        use xindeler_protocol::{ChatSendRequest, NetChatChannel};

        assert_eq!(
            resolve_chat_send(false, &ChatSendRequest::Channel {
                channel: NetChatChannel::Say,
                text: "hello".to_owned(),
            }),
            None
        );
        assert_eq!(
            resolve_chat_send(false, &ChatSendRequest::Command {
                name: "say".to_owned(),
                args: vec!["hello".to_owned()],
            }),
            None
        );
    }

    /// A [`ChatSendRequest::Channel`] with a sendable channel and non-empty
    /// text resolves to `(keyword, [trimmed text])`; whitespace-only text
    /// resolves to `None` (nothing sent, not an empty command).
    #[test]
    fn resolve_chat_send_channel_resolves_to_the_command_keyword() {
        use xindeler_protocol::{ChatSendRequest, NetChatChannel};

        assert_eq!(
            resolve_chat_send(true, &ChatSendRequest::Channel {
                channel: NetChatChannel::Region,
                text: "  hello there  ".to_owned(),
            }),
            Some(("region".to_owned(), vec!["hello there".to_owned()]))
        );
        assert_eq!(
            resolve_chat_send(true, &ChatSendRequest::Channel {
                channel: NetChatChannel::Say,
                text: "   ".to_owned(),
            }),
            None,
            "whitespace-only text must resolve to None, not an empty send"
        );
    }

    /// A [`ChatSendRequest::Channel`] naming a RECEIVE-ONLY channel
    /// (`Tell`/`Npc`/`System`, none of which have a `send_command_name`)
    /// resolves to `None` — the chat UI must never be able to send garbage
    /// through one of these, even if it somehow constructed such a request.
    #[test]
    fn resolve_chat_send_channel_rejects_receive_only_channels() {
        use xindeler_protocol::{ChatSendRequest, NetChatChannel};

        for channel in [
            NetChatChannel::Tell,
            NetChatChannel::Npc,
            NetChatChannel::System,
        ] {
            assert_eq!(
                resolve_chat_send(true, &ChatSendRequest::Channel {
                    channel,
                    text: "hello".to_owned(),
                }),
                None,
                "{channel:?} must never resolve to a send"
            );
        }
    }

    /// A [`ChatSendRequest::Command`] with a non-empty name passes its name
    /// and args through verbatim; an empty/whitespace-only name resolves to
    /// `None`.
    #[test]
    fn resolve_chat_send_command_passes_name_and_args_through() {
        use xindeler_protocol::ChatSendRequest;

        assert_eq!(
            resolve_chat_send(true, &ChatSendRequest::Command {
                name: "tell".to_owned(),
                args: vec!["Bob".to_owned(), "hi".to_owned()],
            }),
            Some(("tell".to_owned(), vec!["Bob".to_owned(), "hi".to_owned()]))
        );
        assert_eq!(
            resolve_chat_send(true, &ChatSendRequest::Command {
                name: "  ".to_owned(),
                args: vec![],
            }),
            None,
            "a whitespace-only command name must resolve to None"
        );
    }
}
