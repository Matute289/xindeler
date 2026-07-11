//! EM-3.7b — local-player input + third-person camera (listen-server only).
//!
//! Two responsibilities, both pure Bevy (no specs, no server crate — the guard
//! greps this crate's `src`):
//!
//! 1. [`gather_input`] reads the SAME Bevy keyboard/mouse the fly-cam already
//!    reads ([`crate::camera`]) and writes a
//!    [`xindeler_protocol::LocalPlayerInput`] resource in SIM axes (x-east,
//!    y-north, z-up). The bridge's `tick_player` applies it to the embedded
//!    Client's `ControllerInputs`. We do NOT re-capture the mouse — the fly-cam
//!    owns cursor grab + yaw/pitch; we read the camera's transform to make the
//!    movement camera-relative.
//! 2. [`third_person_camera`] follows the player's mirrored entity (the one the
//!    bridge tagged with [`NetLocalPlayer`]) from behind + above, once such an
//!    entity exists. Mouse orbits (reusing the fly-cam yaw/pitch the existing
//!    look system already integrates). Pressing the toggle key (`F`) flips back
//!    to the free fly-cam for debugging, and again to re-follow.
//!
//! ## Axis conversion
//! The whole render side lives in Bevy's y-up frame; the sim is z-up. The voxel
//! converter maps sim `(x, y, z) → bevy (x, z, −y)`. Its inverse (used to turn
//! a camera-relative Bevy direction back into sim XY intent) is
//! `bevy (x, y, z) → sim (x, −z, y)`; restricted to the horizontal plane a Bevy
//! heading `(bx, 0, bz)` becomes sim `(bx, −bz)`.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use xindeler_app::GameplaySet;
use xindeler_protocol::{LocalPlayerInput, NetLocalPlayer};

use crate::{
    camera::{FlyCam, FlyCamMovementEnabled, FlyCamSet},
    entity_view::Interpolated,
};

/// Key that toggles between third-person follow and the free fly-cam (debug).
const CAMERA_TOGGLE_KEY: KeyCode = KeyCode::KeyF;

/// Third-person camera geometry (Bevy units). The camera sits `BACK` behind and
/// `UP` above the player along the current yaw, and looks at the player's chest
/// (`LOOK_UP` above its origin). A moderately high, pulled-back chase vantage.
///
/// NOTE (EM-3.7b): this is PLACEHOLDER framing. The follow logic is proven
/// (camera tracks the moving player — smoke `player_moved=true`), but the
/// placeholder capsule's on-screen clarity depends heavily on the spawn terrain
/// and lighting: the world-centre singleplayer spawn sits in a large shadowed
/// voxel formation, so the ~1 m capsule reads faintly against it. Crisp framing
/// arrives with the real figure models (EM-3.8) and the tunable, terrain-aware
/// in-game camera (EM-5.11). TODO(EM-5.11): eye-to-player raycast so the camera
/// never clips into or hides behind terrain.
const CAM_BACK: f32 = 9.0;
const CAM_LOOK_UP: f32 = 1.0;

/// BL-82 EM-4.11 follow-up ("slope-descent camera flicker"): per-frame
/// exponential-lerp rate for the camera's OWN follow-focus, decoupled from the
/// player entity's rendered position. `1.0 / THIRD_PERSON_INTERP_TIME` from the
/// old engine's camera (`/xindeler-old/voxygen/src/scene/camera.rs`,
/// `THIRD_PERSON_INTERP_TIME = 0.1`) — see [`smoothed_focus`]'s doc comment for
/// the full root-cause story this constant closes.
const CAMERA_FOCUS_LERP_RATE: f32 = 10.0;

/// Beyond this jump (metres) the camera focus SNAPS instead of easing —
/// teleports, the very first frame following a given player, and re-entering
/// third-person after free-flying elsewhere. Matches
/// `entity_view::SNAP_DISTANCE`.
const CAMERA_FOCUS_SNAP_DISTANCE: f32 = 64.0;

/// Whether the camera is currently following the player (vs. free fly-cam).
#[derive(Resource)]
pub struct ThirdPersonActive(pub bool);

impl Default for ThirdPersonActive {
    fn default() -> Self { Self(true) }
}

/// Installs the input-gather + third-person-follow systems (listen-server
/// only).
pub struct PlayerInputPlugin;

impl Plugin for PlayerInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ThirdPersonActive>()
            // `LocalPlayerInput` is inserted by the bridge's PlayerBridgePlugin;
            // init here too so the client compiles/runs even if that plugin's
            // order changes (init_resource is idempotent — first insert wins).
            .init_resource::<LocalPlayerInput>()
            .add_systems(
                Update,
                (
                    toggle_camera_mode,
                    sync_fly_cam_gate,
                    gather_input,
                    third_person_camera,
                )
                    .chain()
                    // After the fly-cam look/move so we read an up-to-date yaw
                    // and can override the transform when following.
                    .after(FlyCamSet)
                    .in_set(GameplaySet),
            );
    }
}

/// Disables fly-cam translation while third-person is following a real player
/// (so it doesn't fight the follow transform); re-enables it in fly-cam mode or
/// before the player entity exists (spectator terrain-exploration fallback).
fn sync_fly_cam_gate(
    mode: Res<ThirdPersonActive>,
    players: Query<(), With<NetLocalPlayer>>,
    mut fly_move: ResMut<FlyCamMovementEnabled>,
) {
    let following = mode.0 && players.iter().next().is_some();
    fly_move.0 = !following;
}

/// `F` flips between third-person follow and free fly-cam (debug).
fn toggle_camera_mode(keys: Res<ButtonInput<KeyCode>>, mut mode: ResMut<ThirdPersonActive>) {
    if keys.just_pressed(CAMERA_TOGGLE_KEY) {
        mode.0 = !mode.0;
        info!(
            "camera mode: {}",
            if mode.0 {
                "third-person follow"
            } else {
                "free fly-cam"
            }
        );
    }
}

/// Reads WASD / Space / Ctrl and the camera yaw, writes [`LocalPlayerInput`] in
/// SIM axes. Movement is camera-relative and only active while the cursor is
/// grabbed (same gate the fly-cam look uses) so typing/UI later won't drive the
/// player. Jump = Space; look = the camera's forward, converted to sim axes.
fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    cameras: Query<&Transform, With<FlyCam>>,
    mut input: ResMut<LocalPlayerInput>,
) {
    // Only drive the player while the cursor is grabbed (mouse-look engaged).
    let grabbed = cursor_options
        .single()
        .is_ok_and(|c| c.grab_mode != CursorGrabMode::None);
    let Ok(cam) = cameras.single() else {
        *input = LocalPlayerInput::default();
        return;
    };

    // Camera-relative horizontal basis in BEVY space, flattened to the ground.
    let fwd = flatten(*cam.forward());
    let right = flatten(*cam.right());

    let mut wish_bevy = Vec3::ZERO;
    if grabbed {
        if keys.pressed(KeyCode::KeyW) {
            wish_bevy += fwd;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish_bevy -= fwd;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish_bevy += right;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish_bevy -= right;
        }
    }
    let wish_bevy = wish_bevy.normalize_or_zero();

    // Bevy horizontal (bx, 0, bz) → sim (bx, −bz) [XY plane].
    let move_dir = Vec2::new(wish_bevy.x, -wish_bevy.z);
    // Look = camera forward in sim axes (full 3D so pitch carries): bevy
    // (x, y, z) → sim (x, −z, y).
    let look = bevy_to_sim(*cam.forward());

    *input = LocalPlayerInput {
        move_dir,
        jump: grabbed && keys.pressed(KeyCode::Space),
        look,
    };
}

/// When following is active AND a player entity exists, override the camera to
/// sit behind + above the player (yaw from the fly-cam look integration) and
/// look at its head. When following is off (fly-cam mode) or no player entity
/// is present yet, leaves the fly-cam alone.
///
/// ## BL-82 EM-4.11 follow-up — slope-descent camera flicker (root-caused,
/// fixed here)
/// Matías reported a flicker specifically "cuando avanza y hay un desnivel y
/// baja" (when advancing and there's an elevation drop, going down). Root
/// cause: EM-4.11 (PR #59) made the LOCAL player's rendered position (and this
/// camera, which followed it) a direct per-frame SNAP to
/// [`PredictedLocalTransform`] — correctly removing the 30 Hz motion
/// quantization, but on the (correct) assumption that the prediction is
/// "already smooth, frame-rate, zero-jitter" data with "nothing left to ease"
/// (`entity_view.rs` doc comment). That assumption holds horizontally, but
/// NOT vertically: the shared, unchanged `common/systems/src/phys` collision
/// code resolves ground contact per tick, and walking down a sloped/stepped
/// voxel surface produces small, genuine per-tick vertical noise (brief
/// ground/airborne toggling as the contact point steps down) — now ticking at
/// frame rate (100+ Hz) instead of the old 30 Hz, so there are MORE of these
/// small vertical corrections per second than before, all rendered completely
/// unfiltered.
///
/// The reference implementation already solves exactly this: old voxygen's
/// camera (`/xindeler-old/voxygen/src/scene/camera.rs::Camera::update`) NEVER
/// renders the raw entity position — it maintains its own `focus`/`tgt_focus`
/// and always lerps toward the target at a fixed `interp_time` (0.1 s for
/// `ThirdPerson`), decoupled from however jittery the underlying tracked
/// position is; `scene/mod.rs`'s first-person comment states the same
/// principle explicitly for its x/y-vs-z split ("z is controlled by camera
/// interpolation... because this produces visually smooth results in a larger
/// variety of cases"). [`smoothed_focus`] ports that same idea: the CAMERA
/// keeps its own eased focus point, entirely separate from the player
/// entity's own rendered `Transform` (which stays a direct snap, per EM-4.11 —
/// this fix does not touch or regress that quantization fix at all, since the
/// player's own mesh is a different consumer of the same
/// `PredictedLocalTransform`/`Interpolated` data).
fn third_person_camera(
    mode: Res<ThirdPersonActive>,
    time: Res<Time>,
    // The player's interpolated presentation transform (smooth) — the same one
    // the entity_view drives; falling back to NetLocalPlayer's Transform if the
    // interpolation buffer isn't attached yet.
    player: Query<(&Transform, Option<&Interpolated>), (With<NetLocalPlayer>, Without<FlyCam>)>,
    mut cameras: Query<(&mut Transform, &FlyCam), Without<NetLocalPlayer>>,
    mut focus: Local<Option<Vec3>>,
    mut perf_log: Local<Option<bool>>,
) {
    if !mode.0 {
        return;
    }
    let Ok((player_tf, interp)) = player.single() else {
        return; // no player entity mirrored yet — keep the spectator fly-cam
    };
    let player_pos = interp.map_or(player_tf.translation, |i| i.pos);
    let focus_pos = smoothed_focus(&mut focus, player_pos, mode.is_changed(), time.delta_secs());

    // BL-82 EM-4.11 follow-up (slope-descent camera flicker) — opt-in
    // diagnostic proving the smoothing actually engages live: logs the RAW
    // per-frame target vs the eased focus so a live session can confirm the
    // raw signal is the noisy one (reversals/hops while walking down a
    // slope) and the eased one damps it. Same "permanent, opt-in, gated by an
    // env var read once" convention as `XINDELER_FAR_MESH_PERF_LOG`
    // elsewhere in this crate (the EM-3.11r `XINDELER_LANDING_PERF_LOG` this
    // used to be worded against has since been retired, per EM-4.11).
    if *perf_log.get_or_insert_with(|| {
        std::env::var("XINDELER_CAMERA_FOCUS_PERF_LOG").is_ok_and(|v| v != "0")
    }) {
        debug!(
            raw_y = player_pos.y,
            eased_y = focus_pos.y,
            "EM-4.11 follow-up: third-person camera focus (raw target vs eased)"
        );
    }

    for (mut cam_tf, fly) in &mut cameras {
        // Full spherical orbit from BOTH yaw AND pitch (EM-3.11 smoke fix —
        // this previously only read `fly.yaw`, so vertical mouse motion did
        // nothing in third-person mode; only the horizontal orbit worked).
        // Derive the SAME forward direction the fly-cam itself looks along
        // (`fly_cam_look`'s `Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)`
        // convention) and orbit the eye around the look-at point along that
        // direction, so pitch swings the camera up/down exactly like it
        // swings the fly-cam's own look direction — no separate vertical
        // constant needed.
        let forward = Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.0) * Vec3::NEG_Z;
        let look_at = focus_pos + Vec3::Y * CAM_LOOK_UP;
        let eye = look_at - forward * CAM_BACK;
        *cam_tf = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    }
}

/// The camera's own follow-focus smoothing (BL-82 EM-4.11 follow-up — see
/// [`third_person_camera`]'s doc comment for the full root-cause writeup).
/// Eases `focus` toward `target` at [`CAMERA_FOCUS_LERP_RATE`], SNAPPING
/// instead when: this is the very first frame following any player (`focus`
/// is `None`), the mode just switched on (`mode_just_activated` — avoids a
/// visible glide-in from wherever the free fly-cam last was), or the target
/// jumped more than [`CAMERA_FOCUS_SNAP_DISTANCE`] (teleports). Factored out
/// of the system so the smoothing itself is unit-testable without a
/// live App/ECS.
fn smoothed_focus(
    focus: &mut Option<Vec3>,
    target: Vec3,
    mode_just_activated: bool,
    dt: f32,
) -> Vec3 {
    let snap_far = focus.is_some_and(|f| {
        f.distance_squared(target) >= CAMERA_FOCUS_SNAP_DISTANCE * CAMERA_FOCUS_SNAP_DISTANCE
    });
    let next = match *focus {
        Some(f) if !mode_just_activated && !snap_far => {
            f.lerp(target, (CAMERA_FOCUS_LERP_RATE * dt).min(1.0))
        },
        _ => target,
    };
    *focus = Some(next);
    next
}

// ---------------------------------------------------------------------------
// Smoke auto-move (scaffolding — smoke-screenshot only)
// ---------------------------------------------------------------------------

/// SCAFFOLDING for the autonomous smoke screenshot (EM-3.7b): the headless
/// smoke harness can't inject real keyboard input, so this plugin — added ONLY
/// under `--listen-server --smoke-screenshot` — overwrites [`LocalPlayerInput`]
/// with a constant forward walk once the player is in game, so the character
/// visibly moves under the third-person camera on the real terrain before the
/// capture. It runs AFTER [`gather_input`] so it wins, and is otherwise inert.
/// NOT part of the interactive path: no flag = this plugin is never added.
///
/// ## BL-82 EM-3.11n: selectable move pattern (straight vs. diagonal)
/// Also reused by `--smoke-perf-run` (`crate::smoke`) to drive a controlled
/// A/B frame-time comparison for the "diagonal movement feels choppier" report
/// (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md`, round 8):
/// `XINDELER_SMOKE_MOVE_PATTERN` selects the walk direction (sim XY plane),
/// read ONCE at plugin build time (not per-frame) since it never changes mid
/// run. `straight` (default, unset, or unrecognised) preserves the original
/// due-north walk so every existing screenshot smoke run is byte-identical;
/// `diagonal` walks north-east (`(1,1)` normalized) so the same real ground
/// speed crosses BOTH chunk-grid axes at once — the grid-crossing-rate
/// argument the round-8 hypothesis rests on (see the module docs on
/// `terrain_stream.rs`'s dirty-marking and `pipeline.rs`'s upload budget).
pub struct SmokeAutoMovePlugin;

/// The two walk patterns [`SmokeAutoMovePlugin`] can drive, selected via
/// `XINDELER_SMOKE_MOVE_PATTERN` (`straight` default / `diagonal`).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmokeMovePattern {
    /// Due sim-north (`(0,1)`) — the original EM-3.7b behaviour.
    Straight,
    /// Sim north-east (`(1,1)` normalized) — crosses both chunk-grid axes at
    /// once for the same ground speed (BL-82 EM-3.11n).
    Diagonal,
}

impl SmokeMovePattern {
    /// Reads `XINDELER_SMOKE_MOVE_PATTERN`; unset/unrecognised → `Straight` so
    /// every pre-existing smoke-screenshot invocation is unaffected.
    fn from_env() -> Self {
        match std::env::var("XINDELER_SMOKE_MOVE_PATTERN").as_deref() {
            Ok("diagonal") => Self::Diagonal,
            _ => Self::Straight,
        }
    }

    /// The sim-XY move vector + matching look vector for this pattern.
    fn move_dir(self) -> Vec2 {
        match self {
            Self::Straight => Vec2::new(0.0, 1.0),
            Self::Diagonal => Vec2::new(1.0, 1.0).normalize(),
        }
    }
}

impl Plugin for SmokeAutoMovePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SmokeMovePattern::from_env())
            .init_resource::<SmokeAutoMoveState>()
            .init_resource::<SmokeStuckDetour>()
            .add_systems(
                Update,
                (
                    smoke_auto_move.after(gather_input),
                    // BL-82 EM-3.11p round 11 (Wave-3 post-merge regression
                    // hunt): EVERY prior `--smoke-perf-run`/`--smoke-screenshot`
                    // trial (this round's and all earlier EM-3.11 rounds') left
                    // `FlyCam::yaw`/`pitch` frozen at their boot default the
                    // whole run — they only change from real
                    // `AccumulatedMouseMotion`, which a scripted bot never
                    // produces. A real player's session ALWAYS includes
                    // continuous mouse-look while walking; EM-3.11k already
                    // proved camera rotation has its own distinct render-cost
                    // profile (TAA history-confidence reset). `XINDELER_
                    // SMOKE_ROTATE=1` opts a run into a slow, continuous,
                    // scripted yaw sweep on top of the existing walk pattern,
                    // closing this blind spot for future rounds without
                    // requiring a human at the mouse.
                    smoke_rotate_camera.after(gather_input),
                )
                    .in_set(GameplaySet),
            );
    }
}

/// Radians/second the scripted camera sweeps when `XINDELER_SMOKE_ROTATE=1`
/// (see [`SmokeAutoMovePlugin`]'s doc comment). Slow enough to resemble a
/// human idly looking around while walking, not a disorienting spin.
const SMOKE_ROTATE_RATE_RAD_S: f32 = 0.6;

/// Continuously sweeps the fly-cam's yaw (never its pitch) at
/// [`SMOKE_ROTATE_RATE_RAD_S`] when `XINDELER_SMOKE_ROTATE` is set to
/// anything but `0`/unset — a no-op check (cached env read) otherwise, so
/// every pre-existing smoke run is unaffected by default.
fn smoke_rotate_camera(
    time: Res<Time>,
    mut cameras: Query<&mut FlyCam>,
    mut enabled: Local<Option<bool>>,
) {
    let enabled = *enabled
        .get_or_insert_with(|| std::env::var("XINDELER_SMOKE_ROTATE").is_ok_and(|v| v != "0"));
    if !enabled {
        return;
    }
    for mut fly in &mut cameras {
        fly.yaw += SMOKE_ROTATE_RATE_RAD_S * time.delta_secs();
    }
}

/// Distance (Bevy metres) the player must visibly travel before the smoke
/// capture is allowed to fire — proves the input actually moved the character.
const SMOKE_MOVE_THRESHOLD: f32 = 2.0;

#[derive(Resource, Default)]
struct SmokeAutoMoveState {
    /// The player's presentation position the first frame we saw it.
    start: Option<Vec3>,
}

/// BL-82 EM-3.11p round 12 (diagonal-stutter methodology fix): how often
/// [`stuck_detour_move_dir`] samples horizontal displacement to decide
/// whether the scripted walker is stuck (e.g. against a tree).
const STUCK_CHECK_INTERVAL_S: f32 = 1.0;
/// Minimum horizontal distance (Bevy metres) the character must cover in one
/// [`STUCK_CHECK_INTERVAL_S`] window to NOT be considered stuck. Well below a
/// normal walk speed's per-second distance, so only a genuine snag (near-zero
/// net movement) trips it, not ordinary speed variance.
const STUCK_DISTANCE_M: f32 = 0.5;
/// How long a detour lasts once engaged before the walker returns to trying
/// the original commanded heading again.
const DETOUR_DURATION_S: f32 = 2.5;

/// Per-run state for [`stuck_detour_move_dir`]'s obstacle-avoidance: this is
/// what makes `--smoke-perf-run`'s scripted diagonal/straight walk robust to
/// the world containing normal obstacles (trees, rocks) instead of silently
/// producing a confounded measurement.
///
/// ## Why this exists (BL-82 EM-3.11p, 6 rounds in)
/// Matías flagged that the diagonal-movement A/B harness's prior rounds were
/// likely confounded: "cuando lo probás lo que veo es que camina un par de
/// pasos y enseguida te trabás con el árbol" (when you test it, it walks a
/// couple steps and immediately gets stuck on a tree). A scripted walker with
/// a FIXED heading and no obstacle awareness will, in a real procedurally
/// generated world, eventually walk into a tree/rock and then spend the
/// REST of a 90-150s measurement window standing still against it —
/// collapsing the frame-time distribution to "whatever standing still costs"
/// for most of the window, not "whatever walking straight/diagonally costs".
/// That would explain why 6 rounds of investigation got noisy, inconsistent
/// results even after fixing every other methodology bug (fresh userdata,
/// camera rotation, etc. — round 10).
#[derive(Resource, Default)]
struct SmokeStuckDetour {
    /// Horizontal (xz) position + elapsed time at the last stuck-check.
    last_check: Option<(Vec3, f32)>,
    /// `(detour heading, elapsed time the detour ends)`, if currently
    /// detouring instead of following the commanded pattern.
    detour: Option<(Vec2, f32)>,
    /// How many detours have fired back-to-back (reset once a check finds
    /// the walker moving freely again) — escalates the turn angle so a
    /// walker cornered against two obstacles doesn't oscillate between the
    /// same two blocked headings forever.
    consecutive_detours: u32,
}

/// Rotates `dir` by 90° · `steps` (steps 1..=3 cycle through right/back/left
/// before repeating), escalating each time [`stuck_detour_move_dir`] detects
/// the walker is STILL stuck after a previous detour — a simple, deterministic
/// wall-follow-style escape that doesn't need real collision/raycast queries
/// (none are available to this pure-Bevy client; the sim is the only thing
/// that knows real terrain occupancy).
fn escalated_turn(dir: Vec2, steps: u32) -> Vec2 {
    // Cycles through 1/2/3 quarter-turns (right/back/left), never 0 — a
    // repeated stuck detection must never fall back to the ORIGINAL heading
    // that just got the walker stuck in the first place.
    let quarter_turns = ((steps.max(1) - 1) % 3) + 1;
    let mut d = dir;
    for _ in 0..quarter_turns {
        d = Vec2::new(-d.y, d.x); // rotate +90°
    }
    d
}

/// Pure decision function (unit-tested without a GPU/App) for
/// [`SmokeStuckDetour`]: given the current horizontal position, elapsed run
/// time, and the walker's originally-commanded heading, returns the heading
/// to ACTUALLY drive this frame — either the original pattern (normal case),
/// or a temporary detour heading if a stuck condition was just detected or is
/// still in effect. Mutates `detector` to track state across calls.
fn stuck_detour_move_dir(
    now: f32,
    horizontal_pos: Vec3,
    base_dir: Vec2,
    detector: &mut SmokeStuckDetour,
) -> Vec2 {
    // Currently detouring: keep the detour heading until it expires.
    if let Some((detour_dir, until)) = detector.detour {
        if now < until {
            return detour_dir;
        }
        detector.detour = None;
    }

    let Some((last_pos, last_time)) = detector.last_check else {
        detector.last_check = Some((horizontal_pos, now));
        return base_dir;
    };

    if now - last_time < STUCK_CHECK_INTERVAL_S {
        return base_dir;
    }

    let travelled = horizontal_pos.distance(last_pos);
    detector.last_check = Some((horizontal_pos, now));

    if travelled < STUCK_DISTANCE_M {
        detector.consecutive_detours += 1;
        let detour_dir = escalated_turn(base_dir, detector.consecutive_detours);
        detector.detour = Some((detour_dir, now + DETOUR_DURATION_S));
        detour_dir
    } else {
        detector.consecutive_detours = 0;
        base_dir
    }
}

/// How long (seconds) `XINDELER_SMOKE_JUMP_SPAM` holds jump pressed, then
/// released, then repeats — a square wave chosen to be much faster than a
/// human could sanely bunny-hop, to stress-test back-to-back jumps for the
/// BL-82 EM-3.11r "sometimes I don't reach the ground" investigation.
const JUMP_SPAM_HALF_PERIOD_S: f32 = 0.35;

/// Forces a steady walk (direction from [`SmokeMovePattern`]) + matching look
/// while the player exists, and raises [`crate::smoke::SmokePlayerMoved`] once
/// the character has actually travelled [`SMOKE_MOVE_THRESHOLD`], so the
/// capture/measurement lands on a frame that shows the walking player. Only
/// meaningful once a player entity is mirrored.
///
/// BL-82 EM-3.11r: when `XINDELER_SMOKE_JUMP_SPAM` is set to anything but
/// `0`/unset, also drives a repeated jump-press/release square wave on top of
/// the walk, so a scripted, reproducible "jump a lot" run is possible without
/// a human at the keyboard (Matías's report). Off by default — every
/// pre-existing smoke/perf run is unaffected.
///
/// BL-82 EM-3.11p round 12: ALWAYS runs [`stuck_detour_move_dir`] so a
/// scripted run can no longer silently collapse into "standing still against
/// a tree for the rest of the measurement window" (see
/// [`SmokeStuckDetour`]'s doc comment). This changes behavior only when the
/// walker is ACTUALLY stuck — the commanded heading (and every existing
/// straight-walk screenshot smoke test) is bit-for-bit unaffected in the
/// normal, unobstructed case.
fn smoke_auto_move(
    time: Res<Time>,
    player: Query<&Transform, With<NetLocalPlayer>>,
    pattern: Res<SmokeMovePattern>,
    mut input: ResMut<LocalPlayerInput>,
    mut state: ResMut<SmokeAutoMoveState>,
    mut stuck: ResMut<SmokeStuckDetour>,
    mut moved: ResMut<crate::smoke::SmokePlayerMoved>,
    mut jump_spam_enabled: Local<Option<bool>>,
) {
    let Ok(tf) = player.single() else {
        return;
    };
    let jump_spam_enabled = *jump_spam_enabled
        .get_or_insert_with(|| std::env::var("XINDELER_SMOKE_JUMP_SPAM").is_ok_and(|v| v != "0"));
    let jump = jump_spam_enabled
        && ((time.elapsed_secs() / JUMP_SPAM_HALF_PERIOD_S) as u64).is_multiple_of(2);
    let pos = tf.translation;
    let was_detouring = stuck.detour.is_some();
    let horizontal_pos = Vec3::new(pos.x, 0.0, pos.z);
    let move_dir = stuck_detour_move_dir(
        time.elapsed_secs(),
        horizontal_pos,
        pattern.move_dir(),
        &mut stuck,
    );
    if !was_detouring && stuck.detour.is_some() {
        info!(
            pos = ?pos,
            detour_dir = ?move_dir,
            consecutive = stuck.consecutive_detours,
            "BL-82 EM-3.11p round 12: scripted walker stuck, engaging detour"
        );
    }
    *input = LocalPlayerInput {
        move_dir,
        jump,
        // Sim (x, y) horizontal look, matching the walk direction (full 3D
        // look vector with z=0, same convention `gather_input` uses).
        look: Vec3::new(move_dir.x, move_dir.y, 0.0),
    };
    match state.start {
        None => state.start = Some(pos),
        Some(start) => {
            if pos.distance(start) >= SMOKE_MOVE_THRESHOLD {
                moved.0 = true;
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Smoke figure cam (scaffolding — smoke-screenshot only, EM-3.8)
// ---------------------------------------------------------------------------

/// SCAFFOLDING for the EM-3.8 smoke screenshot: the world-centre singleplayer
/// spawn buries the player (and its third-person camera) in a large shadowed
/// voxel pillar (the documented EM-3.7b framing caveat), so a follow shot can't
/// SHOW the new figures. Added ONLY under `--listen-server --smoke-screenshot`,
/// this plugin overrides the camera to orbit an elevated vantage onto the
/// NEAREST wandering NPC figure (they roam open, lit terrain around the
/// anchor), so the capture actually shows a real `.vox` model. It runs after
/// the third-person camera so it wins; interactive play never adds it.
pub struct SmokeFigureCamPlugin;

impl Plugin for SmokeFigureCamPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            smoke_figure_cam
                .after(third_person_camera)
                .in_set(GameplaySet),
        );
    }
}

/// Elevated 3/4 vantage offset (Bevy metres) from the framed NPC, and how close
/// an NPC must be to the anchor-ish player to be considered "the subject".
const FIG_CAM_BACK: f32 = 3.5;
const FIG_CAM_UP: f32 = 2.2;

/// Points the smoke camera at a non-player mirrored figure from a close,
/// elevated 3/4 angle, so the capture shows an assembled `.vox` NPC. PREFERS
/// the nearest HUMANOID (the richest figure — head recolour + clothing + the
/// sheathed weapon), falling back to the nearest figure of any body when no
/// humanoid is assembled yet. The wandering test critters spawn in a ring
/// around the anchor on open terrain.
fn smoke_figure_cam(
    players: Query<&Transform, With<NetLocalPlayer>>,
    humanoids: Query<
        (&Transform, Option<&Interpolated>),
        (
            With<crate::figure_view::HumanoidFigure>,
            Without<NetLocalPlayer>,
            Without<FlyCam>,
        ),
    >,
    figures: Query<
        (&Transform, Option<&Interpolated>),
        (With<Children>, Without<NetLocalPlayer>, Without<FlyCam>),
    >,
    mut cameras: Query<&mut Transform, (With<FlyCam>, Without<NetLocalPlayer>)>,
) {
    let focus = players.iter().next().map_or(Vec3::ZERO, |t| t.translation);
    let nearest = |it: &mut dyn Iterator<Item = Vec3>| {
        it.min_by(|a, b| {
            a.distance_squared(focus)
                .total_cmp(&b.distance_squared(focus))
        })
    };
    // Prefer a humanoid; else any assembled (child-bearing) figure.
    let subject = nearest(
        &mut humanoids
            .iter()
            .map(|(tf, interp)| interp.map_or(tf.translation, |i| i.pos)),
    )
    .or_else(|| {
        nearest(
            &mut figures
                .iter()
                .map(|(tf, interp)| interp.map_or(tf.translation, |i| i.pos)),
        )
    });
    let Some(subject) = subject else {
        return; // no figure spawned yet — leave the existing camera
    };
    // A fixed, clear 3/4 angle (looking roughly north-east-down at the NPC).
    let eye = subject + Vec3::new(FIG_CAM_BACK, FIG_CAM_UP, FIG_CAM_BACK);
    for mut cam in &mut cameras {
        *cam = Transform::from_translation(eye).looking_at(subject + Vec3::Y * 0.5, Vec3::Y);
    }
}

/// Drops the y (vertical) component and renormalizes — a ground-plane heading.
fn flatten(v: Vec3) -> Vec3 { Vec3::new(v.x, 0.0, v.z).normalize_or_zero() }

// ---------------------------------------------------------------------------
// Smoke sprite/water cam (scaffolding — smoke-screenshot only, EM-3.9)
// ---------------------------------------------------------------------------

/// SCAFFOLDING for the EM-3.9 smoke screenshot. The world-centre spawn sits in
/// a dark shadowed pillar structure (the EM-3.7b/3.8 framing caveat), so the
/// figure cam captures vegetation-free interior floor. This plugin instead
/// frames the DENSEST sprite patch (open, sunlit terrain where grass/flowers
/// grow) from a low, close 3/4 angle, so the capture actually shows the EM-3.9
/// sprites. Added ONLY under `--listen-server --smoke-screenshot`; runs after
/// the figure cam so it wins. Interactive play never adds it.
pub struct SmokeSpriteCamPlugin;

impl Plugin for SmokeSpriteCamPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            smoke_sprite_cam.after(smoke_figure_cam).in_set(GameplaySet),
        );
    }
}

/// Low, close 3/4 vantage offset (Bevy metres) from the framed vegetation
/// patch.
const SPR_CAM_BACK: f32 = 6.0;
const SPR_CAM_UP: f32 = 3.5;

/// Points the smoke camera at the densest built sprite patch from a close,
/// slightly-elevated angle, so the capture shows grass/flowers on lit terrain.
/// Falls back to leaving the camera as-is (figure framing) when no sprites have
/// been built yet.
fn smoke_sprite_cam(
    patches: Query<&crate::sprite_view::SpriteChunkParent>,
    mut cameras: Query<&mut Transform, With<FlyCam>>,
) {
    // The densest patch = the most visible vegetation.
    let Some(target) = patches
        .iter()
        .filter(|p| p.count > 0)
        .max_by_key(|p| p.count)
        .map(|p| p.centroid)
    else {
        return; // no sprites yet — keep the figure framing
    };
    // Stand back along +x/+z and above, looking down at the patch centroid.
    let eye = target + Vec3::new(SPR_CAM_BACK, SPR_CAM_UP, SPR_CAM_BACK);
    for mut cam in &mut cameras {
        *cam = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

/// SCAFFOLDING for the EM-3.9b smoke screenshot (same spirit as
/// [`SmokeSpriteCamPlugin`]): if the streamed window has meshed ANY fluid
/// (water) chunk, frame the one nearest the world anchor from a close,
/// elevated 3/4 angle, so the capture shows the animated water shader rather
/// than whatever the sprite/figure cam happened to land on. Added ONLY under
/// `--listen-server --smoke-screenshot`; runs after the sprite cam so it wins
/// when water exists, and no-ops (keeps the sprite/figure framing) otherwise
/// — not every world seed puts a river/lake inside the streamed window.
/// Interactive play never adds it.
pub struct SmokeWaterCamPlugin;

impl Plugin for SmokeWaterCamPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            smoke_water_cam.after(smoke_sprite_cam).in_set(GameplaySet),
        );
    }
}

/// Elevated 3/4 vantage offset (Bevy metres) from the framed water chunk —
/// further back than the sprite cam since a water surface reads better from
/// a bit of height/distance (shows the ripple pattern over an area, not one
/// quad close-up).
const WATER_CAM_BACK: f32 = 12.0;
const WATER_CAM_UP: f32 = 6.0;

/// Points the smoke camera at the fluid chunk nearest the world anchor, from
/// a close, elevated angle. Falls back to leaving the camera as-is (sprite or
/// figure framing) when no fluid chunk has been meshed yet.
fn smoke_water_cam(
    fluids: Query<&xindeler_render_voxel::pipeline::FluidChunkMesh>,
    anchor: Option<Res<crate::terrain_stream::TerrainCameraAnchor>>,
    mut cameras: Query<&mut Transform, With<FlyCam>>,
) {
    let focus = anchor.map_or(Vec3::ZERO, |a| a.bevy_pos);
    let half_edge = crate::terrain_stream::CHUNK_EDGE * 0.5;
    let Some(target) = fluids
        .iter()
        .map(|f| {
            xindeler_render_voxel::pipeline::chunk_transform(f.key).translation
                + Vec3::new(half_edge, 0.0, -half_edge)
        })
        .min_by(|a, b| {
            a.distance_squared(focus)
                .total_cmp(&b.distance_squared(focus))
        })
    else {
        return; // no water meshed yet — keep the sprite/figure framing
    };
    let eye = target + Vec3::new(WATER_CAM_BACK, WATER_CAM_UP, WATER_CAM_BACK);
    for mut cam in &mut cameras {
        *cam = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

/// Bevy y-up → sim z-up direction: inverse of the converter `(x,y,z)→(x,z,−y)`,
/// i.e. bevy `(x, y, z)` → sim `(x, −z, y)`.
fn bevy_to_sim(v: Vec3) -> Vec3 { Vec3::new(v.x, -v.z, v.y) }

#[cfg(test)]
mod tests {
    use super::*;

    /// The bevy→sim inverse round-trips the converter's forward map.
    #[test]
    fn bevy_sim_axis_inverse() {
        // sim (x, y, z) → bevy (x, z, −y) [converter]; back → sim.
        let sim = Vec3::new(3.0, 5.0, 7.0);
        let bevy = Vec3::new(sim.x, sim.z, -sim.y);
        let round = bevy_to_sim(bevy);
        assert!((round - sim).length() < 1e-5, "round-trip: {round:?}");
    }

    /// A Bevy heading due −z (yaw 0 forward) maps to sim +y (north).
    #[test]
    fn forward_maps_to_north() {
        let move_bevy = flatten(Vec3::new(0.0, 0.0, -1.0));
        let move_dir = Vec2::new(move_bevy.x, -move_bevy.z);
        assert!((move_dir - Vec2::new(0.0, 1.0)).length() < 1e-5);
    }

    /// A quarter-turn rotates a heading 90° (right-hand rotation in the xz
    /// plane, matching `Vec2::new(-d.y, d.x)`), and a full 4 steps returns to
    /// the original heading.
    #[test]
    fn escalated_turn_rotates_by_quarter_turns() {
        let base = Vec2::new(0.0, 1.0);
        let one = escalated_turn(base, 1);
        assert!((one - Vec2::new(-1.0, 0.0)).length() < 1e-5, "{one:?}");
        let two = escalated_turn(base, 2);
        assert!((two - Vec2::new(0.0, -1.0)).length() < 1e-5, "{two:?}");
        let three = escalated_turn(base, 3);
        assert!((three - Vec2::new(1.0, 0.0)).length() < 1e-5, "{three:?}");
        // Step 4 cycles back to a 1-quarter-turn (never the identity/original
        // heading — see the function's doc comment).
        let four = escalated_turn(base, 4);
        assert!(
            (four - one).length() < 1e-5,
            "cycles without ever returning to the original heading: {four:?}"
        );
    }

    /// BL-82 EM-3.11p round 12 regression: while the walker keeps making
    /// normal horizontal progress every check window, the commanded heading
    /// is returned UNCHANGED (no detour ever engages) — every existing
    /// straight-walk smoke run must be unaffected in the normal case.
    #[test]
    fn no_detour_while_moving_normally() {
        let mut detector = SmokeStuckDetour::default();
        let base_dir = Vec2::new(0.0, 1.0);
        let mut pos = Vec3::ZERO;
        let mut now = 0.0;
        for _ in 0..5 {
            now += STUCK_CHECK_INTERVAL_S;
            pos.z -= 5.0; // well beyond STUCK_DISTANCE_M every window
            let dir = stuck_detour_move_dir(now, pos, base_dir, &mut detector);
            assert_eq!(dir, base_dir, "must not detour while moving freely");
        }
        assert!(detector.detour.is_none());
        assert_eq!(detector.consecutive_detours, 0);
    }

    /// BL-82 EM-3.11p round 12 regression: a walker that stops making
    /// progress (e.g. snagged on a tree) gets diverted to a different
    /// heading instead of silently standing still for the rest of the run —
    /// this is the fix for Matías's "camina un par de pasos y enseguida te
    /// trabás con el árbol" methodology complaint.
    #[test]
    fn stuck_walker_gets_a_detour() {
        let mut detector = SmokeStuckDetour::default();
        let base_dir = Vec2::new(0.0, 1.0);
        let stuck_pos = Vec3::new(10.0, 0.0, 10.0); // never moves
        // First sample seeds the baseline (no decision made yet).
        let mut now = 0.0;
        let dir0 = stuck_detour_move_dir(now, stuck_pos, base_dir, &mut detector);
        assert_eq!(dir0, base_dir);
        // Past the check interval with zero displacement: must detour.
        now += STUCK_CHECK_INTERVAL_S;
        let dir1 = stuck_detour_move_dir(now, stuck_pos, base_dir, &mut detector);
        assert_ne!(dir1, base_dir, "must divert once stuck is detected");
        assert!(detector.detour.is_some());
        assert_eq!(detector.consecutive_detours, 1);
        // Immediately after, the SAME detour heading holds (still within
        // DETOUR_DURATION_S), not re-evaluated every frame.
        now += 0.1;
        let dir2 = stuck_detour_move_dir(now, stuck_pos, base_dir, &mut detector);
        assert_eq!(dir2, dir1, "detour heading holds for its full duration");
    }

    /// BL-82 EM-3.11p round 12: if the walker is STILL stuck after a detour
    /// expires, the next detour escalates to a different heading rather than
    /// repeating the same (possibly still-blocked) turn forever.
    #[test]
    fn repeated_stuck_escalates_the_turn() {
        let mut detector = SmokeStuckDetour::default();
        let base_dir = Vec2::new(0.0, 1.0);
        let stuck_pos = Vec3::new(1.0, 0.0, 1.0);
        let mut now = 0.0;
        let _ = stuck_detour_move_dir(now, stuck_pos, base_dir, &mut detector); // seed
        now += STUCK_CHECK_INTERVAL_S;
        let first_detour = stuck_detour_move_dir(now, stuck_pos, base_dir, &mut detector);
        assert_eq!(detector.consecutive_detours, 1);
        // Let the first detour fully expire, still stuck at the same spot.
        now += DETOUR_DURATION_S + STUCK_CHECK_INTERVAL_S;
        let second_detour = stuck_detour_move_dir(now, stuck_pos, base_dir, &mut detector);
        assert_eq!(detector.consecutive_detours, 2);
        assert_ne!(
            second_detour, first_detour,
            "escalation must try a different heading, not repeat the same blocked one"
        );
    }

    /// BL-82 EM-4.11 follow-up (slope-descent camera flicker): the very FIRST
    /// call for a given `focus` (still `None`) snaps directly to `target` —
    /// no glide-in from an arbitrary default, matching the "first frame
    /// following any player" case in [`third_person_camera`]'s doc comment.
    #[test]
    fn smoothed_focus_first_call_snaps() {
        let mut focus = None;
        let target = Vec3::new(3.0, 5.0, -2.0);
        let got = smoothed_focus(&mut focus, target, false, 1.0 / 60.0);
        assert_eq!(got, target);
        assert_eq!(focus, Some(target));
    }

    /// A small per-frame target delta (e.g. the per-tick vertical noise from
    /// walking down a sloped/stepped voxel surface — the reported bug) is
    /// EASED, not snapped: the new focus moves only partway toward the
    /// target, damping the noise instead of rendering it 1:1 — the core fix.
    #[test]
    fn smoothed_focus_small_delta_eases_not_snaps() {
        let mut focus = Some(Vec3::new(0.0, 10.0, 0.0));
        let target = Vec3::new(0.0, 9.8, 0.0); // 0.2 m vertical step (a plausible single-tick voxel-step delta)
        let got = smoothed_focus(&mut focus, target, false, 1.0 / 60.0);
        // rate/60 = 10/60 ≈ 0.167 → moves ~16.7% of the way, not all of it.
        assert!(
            got.y < 10.0 && got.y > target.y,
            "must ease partway, not snap: {got:?}"
        );
        let expected_t = (CAMERA_FOCUS_LERP_RATE / 60.0).min(1.0);
        let expected_y = 10.0 + (target.y - 10.0) * expected_t;
        assert!((got.y - expected_y).abs() < 1e-4, "got {got:?}");
    }

    /// A large jump (teleport) SNAPS immediately rather than gliding across
    /// the map over several frames.
    #[test]
    fn smoothed_focus_teleport_snaps() {
        let mut focus = Some(Vec3::ZERO);
        let target = Vec3::new(500.0, 0.0, 0.0);
        let got = smoothed_focus(&mut focus, target, false, 1.0 / 60.0);
        assert_eq!(got, target, "a teleport-sized jump must snap, not ease");
    }

    /// Re-activating third-person mode (toggling `F` back on after
    /// free-flying elsewhere) snaps the focus to the player immediately —
    /// the camera must not glide in from wherever the fly-cam last was.
    #[test]
    fn smoothed_focus_mode_reactivation_snaps() {
        let mut focus = Some(Vec3::new(1000.0, 50.0, 1000.0)); // stale, from free-fly
        let target = Vec3::new(0.0, 10.0, 0.0);
        let got = smoothed_focus(&mut focus, target, true, 1.0 / 60.0);
        assert_eq!(got, target, "mode reactivation must snap, not glide in");
    }

    /// Repeated small eases converge to a steady target over a handful of
    /// frames (not instantly, not never) — confirms the lerp is a genuine
    /// convergent low-pass filter, not a permanent offset.
    #[test]
    fn smoothed_focus_converges_to_a_steady_target() {
        let mut focus = Some(Vec3::ZERO);
        let target = Vec3::new(0.0, 5.0, 0.0);
        let mut last = 0.0;
        for _ in 0..120 {
            let got = smoothed_focus(&mut focus, target, false, 1.0 / 60.0);
            assert!(
                got.y >= last,
                "must move monotonically toward a steady target"
            );
            last = got.y;
        }
        assert!(
            (last - target.y).abs() < 0.05,
            "should have converged close to the target after 2s: {last}"
        );
    }
}
