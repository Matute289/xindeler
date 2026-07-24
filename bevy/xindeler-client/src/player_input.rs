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
    input::mouse::AccumulatedMouseScroll,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use common::{terrain::Block, vol::ReadVol};
use vek::Vec3 as VVec3;
use xindeler_app::GameplaySet;
use xindeler_protocol::{LocalPlayerInput, NetLocalPlayer};
use xindeler_render_voxel::pipeline::ChunkMeshIndex;

use crate::{
    camera::{FlyCam, FlyCamMovementEnabled, FlyCamSet},
    entity_view::Interpolated,
    terrain_stream::SharedTerrain,
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
/// in-game camera. DONE (BL-82 EM-3.12): the stale `TODO(EM-5.11)` that used to
/// sit here ("eye-to-player raycast so the camera never clips into or hides
/// behind terrain") is now closed by [`collide_boom`] + [`smoothed_boom`] — see
/// their doc comments and `docs/design/specs/2026-07-11-bl82-camera-collision-
/// design.md`. (`EM-5.11` itself was reassigned to "Input rebinding" upstream
/// of this fix — that reference was already stale before this landed.)
const CAM_BACK: f32 = 9.0;
const CAM_LOOK_UP: f32 = 1.0;

/// Mouse-wheel-adjustable third-person camera boom length — the desired
/// (uncollided) distance fed into [`collide_boom`]/[`smoothed_boom`] in
/// place of the old fixed [`CAM_BACK`] constant. Clamped to
/// `[CAMERA_ZOOM_MIN, CAMERA_ZOOM_MAX]` by [`handle_camera_zoom_input`] —
/// keeps the camera from clipping into the character at the near end or
/// drifting absurdly far at the far end.
#[derive(Resource)]
pub struct CameraZoom(pub f32);

impl Default for CameraZoom {
    fn default() -> Self { Self(CAM_BACK) }
}

const CAMERA_ZOOM_MIN: f32 = 2.0;
const CAMERA_ZOOM_MAX: f32 = 20.0;
/// Boom-length change (metres) per full wheel "line" of scroll — tuned so a
/// couple of notches noticeably pulls the camera in/out without a single
/// notch overshooting the clamp range in one step.
const CAMERA_ZOOM_SCROLL_SENSITIVITY: f32 = 0.6;

/// BL-82 EM-3.12 — camera-collision spring-arm geometry (code consts, not
/// game-balance content — matches this file's `CAM_BACK`/`CAM_LOOK_UP`
/// convention; see the design doc §6 for the reasoning behind each value).
///
/// Subtracted from a hit's raw distance so the near clip plane clears the
/// solid surface instead of sitting flush on it (tune in smoke). `pub(crate)`
/// so `terrain_stream.rs`'s real-embedded-world integration test can assert
/// against the exact same constant rather than a duplicated literal.
pub(crate) const CAM_NEAR_PAD: f32 = 0.2;
/// The boom must never collapse fully onto the pivot (would put the eye
/// inside the character mesh) — a small floor, not the old engine's much
/// larger zoom-out minimum, since the reported bug wants the camera to come
/// right in under the floor/wall.
const CAM_MIN_DIST: f32 = 0.5;
/// DDA step budget for the boom ray. A `CAM_BACK`-length (9 m) boom crosses
/// at most ~9 voxel boundaries; 64 is ample explicit headroom (`ReadVol::
/// ray`'s own default of 100 would also do).
const CAM_RAY_MAX_ITER: usize = 64;

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
            .init_resource::<CameraZoom>()
            // `LocalPlayerInput` is inserted by the bridge's PlayerBridgePlugin;
            // init here too so the client compiles/runs even if that plugin's
            // order changes (init_resource is idempotent — first insert wins).
            .init_resource::<LocalPlayerInput>()
            .add_systems(
                Update,
                (
                    toggle_camera_mode,
                    handle_camera_zoom_input,
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

/// Mouse-wheel adjusts the third-person camera boom length live, clamped to
/// `[CAMERA_ZOOM_MIN, CAMERA_ZOOM_MAX]`. Scrolling up (positive `y`, the same
/// convention `map_view.rs`'s own scroll-zoom already uses) pulls the camera
/// IN; scrolling down pushes it back out.
fn handle_camera_zoom_input(mut zoom: ResMut<CameraZoom>, scroll: Res<AccumulatedMouseScroll>) {
    if scroll.delta.y.abs() > f32::EPSILON {
        zoom.0 = (zoom.0 - scroll.delta.y * CAMERA_ZOOM_SCROLL_SENSITIVITY)
            .clamp(CAMERA_ZOOM_MIN, CAMERA_ZOOM_MAX);
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

/// Reads the [`GameInput::MoveForward`]/`MoveBack`/`MoveLeft`/`MoveRight`/
/// `Jump` actions (BL-82 EM-5.11: via [`xindeler_input::ActionState`] —
/// keyboard/mouse AND gamepad both drive these the same way, so rebinding
/// either device changes the in-game effect immediately) and the camera
/// yaw, writes [`LocalPlayerInput`] in SIM axes. Movement is camera-relative
/// and only active while the cursor is grabbed (same gate the fly-cam look
/// uses) so typing/UI later won't drive the player. Look = the camera's
/// forward, converted to sim axes.
///
/// `pub(crate)`: BL-82 EM-5.19 Phase 2's `crate::targeting::
/// apply_hard_lock_facing` orders `.after(gather_input)` (so a hard lock
/// overrides `LocalPlayerInput.look` AFTER this system sets it from the
/// camera, rather than being clobbered by it) and reuses [`bevy_to_sim`] for
/// the exact same axis conversion — see that module's doc comment.
pub(crate) fn gather_input(
    action_state: Res<xindeler_input::ActionState>,
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
        if action_state.pressed(xindeler_input::GameInput::MoveForward) {
            wish_bevy += fwd;
        }
        if action_state.pressed(xindeler_input::GameInput::MoveBack) {
            wish_bevy -= fwd;
        }
        if action_state.pressed(xindeler_input::GameInput::MoveRight) {
            wish_bevy += right;
        }
        if action_state.pressed(xindeler_input::GameInput::MoveLeft) {
            wish_bevy -= right;
        }
        // BL-82 EM-5.11: the gamepad left stick contributes an analog
        // direction on top of any digital keys held — same "full-speed once
        // past deadzone" feel as WASD below (the subsequent
        // `normalize_or_zero()` collapses any non-zero combined vector to a
        // unit direction either way); partial-speed analog throttling
        // (walk vs. run off stick magnitude) is a documented v1 follow-up,
        // not silently dropped.
        wish_bevy += fwd * action_state.move_axis.y + right * action_state.move_axis.x;
    }
    let wish_bevy = wish_bevy.normalize_or_zero();

    // Bevy horizontal (bx, 0, bz) → sim (bx, −bz) [XY plane].
    let move_dir = Vec2::new(wish_bevy.x, -wish_bevy.z);
    // Look = camera forward in sim axes (full 3D so pitch carries): bevy
    // (x, y, z) → sim (x, −z, y).
    let look = bevy_to_sim(*cam.forward());

    *input = LocalPlayerInput {
        move_dir,
        jump: grabbed && action_state.pressed(xindeler_input::GameInput::Jump),
        roll: grabbed && action_state.pressed(xindeler_input::GameInput::Roll),
        glide_toggle: grabbed && action_state.pressed(xindeler_input::GameInput::Glide),
        toggle_lantern: grabbed && action_state.pressed(xindeler_input::GameInput::ToggleLantern),
        swap_loadout: grabbed && action_state.pressed(xindeler_input::GameInput::SwapLoadout),
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
    terrain: Res<SharedTerrain>,
    // BL-82 EM-3.11 round 19: gates `terrain.boom_cast` on the SAME "does this
    // chunk have its real render mesh yet" signal `xindeler-render-voxel`
    // tracks — see `terrain_stream::TerrainStore::boom_cast`'s doc comment
    // for the "colliding with something invisible" bug this closes.
    mesh_index: Res<ChunkMeshIndex>,
    zoom: Res<CameraZoom>,
    mut focus: Local<Option<Vec3>>,
    mut cam_dist: Local<Option<f32>>,
    mut perf_log: Local<Option<bool>>,
    mut collision_enabled: Local<Option<bool>>,
) {
    if !mode.0 {
        return;
    }
    let Ok((player_tf, interp)) = player.single() else {
        return; // no player entity mirrored yet — keep the spectator fly-cam
    };
    let player_pos = interp.map_or(player_tf.translation, |i| i.pos);
    // Snapshot the PRE-update eased focus so we can detect, right after the
    // call, whether `smoothed_focus` itself just snapped (a teleport/mode
    // reactivation) — see the boom-snap comment below.
    let prev_eased_focus = *focus;
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

    // BL-82 EM-3.12 kill-switch: `XINDELER_CAMERA_COLLISION=0` disables the
    // spring-arm clamp entirely (today's pre-fix fixed-`CAM_BACK` behaviour),
    // so Matías can A/B live. Default ON (unset/anything-but-`0`) — the
    // INVERSE of this file's other opt-in debug flags
    // (`XINDELER_CAMERA_FOCUS_PERF_LOG`, `XINDELER_SMOKE_ROTATE`, which
    // default OFF): those gate optional diagnostics/scripting, this gates a
    // shipped bug fix, so unset must mean "fix enabled", not "fix disabled".
    // See [`camera_collision_enabled_from_env`]'s doc comment for a review
    // finding this exact default direction caught.
    let collision_enabled = *collision_enabled.get_or_insert_with(|| {
        camera_collision_enabled_from_env(std::env::var("XINDELER_CAMERA_COLLISION"))
    });

    // BL-82 EM-3.12: the boom's own snap conditions mirror `smoothed_focus`'s
    // exactly (first frame / mode reactivation / focus teleport), so the two
    // smoothers stay in lock step — a teleport or re-entering third-person
    // never leaves a stale boom length glide-fighting a freshly-snapped
    // focus. `focus_teleported` re-derives `smoothed_focus`'s own internal
    // `snap_far` test from the OUTSIDE (comparing the focus before vs. after
    // this frame's call) without touching that function at all: when it
    // snaps, `focus_pos` jumps straight to `target`, so the pre/post delta
    // IS the same distance `smoothed_focus` itself just compared against
    // `CAMERA_FOCUS_SNAP_DISTANCE`.
    let focus_teleported = prev_eased_focus.is_some_and(|f| {
        f.distance_squared(focus_pos) >= CAMERA_FOCUS_SNAP_DISTANCE * CAMERA_FOCUS_SNAP_DISTANCE
    });
    let snap_boom = cam_dist.is_none() || mode.is_changed() || focus_teleported;

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
        // BL-82 EM-3.12: the collision clamp is the FINAL step producing the
        // eye, layered AFTER the (untouched) eased focus above — see the
        // design doc §5. Cast from the eased `look_at` toward the desired
        // eye (`-forward`, sim/z-up), against the client's OWN streamed
        // terrain snapshot (`SharedTerrain::boom_cast`; missing chunks pass
        // through, matching the reference engine).
        let dist = if collision_enabled {
            let pivot_sim = to_vek(bevy_to_sim(look_at));
            let dir_sim = to_vek(bevy_to_sim(-forward));
            let clamped = terrain.boom_cast(pivot_sim, dir_sim, zoom.0, &mesh_index);
            smoothed_boom(&mut cam_dist, clamped, snap_boom, time.delta_secs())
        } else {
            zoom.0
        };
        let eye = look_at - forward * dist;
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

/// BL-82 EM-3.12 — the third-person camera's spring-arm collision clamp
/// (design doc §6). Casts the SAME voxel DDA ray the physics uses
/// (`common/systems/src/phys/collision.rs:548`) from `pivot_sim` toward the
/// desired eye (`pivot_sim + dir_sim * desired`, sim/z-up coords), stopping at
/// the first solid block. On a hit, the boom is clamped to just short of the
/// hit surface (never below [`CAM_MIN_DIST`], never past `desired`); with no
/// hit — including an unloaded/out-of-bounds cell along the ray, via
/// `.ignore_error()` — the full `desired` distance is returned (no clip).
///
/// Generic over the volume (any `V: ReadVol<Vox = Block>`) so unit tests can
/// pass a hand-built `VolGrid2d` instead of the client's live terrain
/// snapshot; production call sites are [`crate::terrain_stream`]'s streamed
/// `VolGrid2d<TerrainChunk>` (via `SharedTerrain::boom_cast`).
pub(crate) fn collide_boom<V: ReadVol<Vox = Block>>(
    vol: &V,
    pivot_sim: VVec3<f32>,
    dir_sim: VVec3<f32>,
    desired: f32,
) -> f32 {
    let to = pivot_sim + dir_sim * desired;
    match vol
        .ray(pivot_sim, to)
        .until(|b: &Block| b.is_solid())
        .ignore_error()
        .max_iter(CAM_RAY_MAX_ITER)
        .cast()
    {
        (d, Ok(Some(_))) => (d - CAM_NEAR_PAD).clamp(CAM_MIN_DIST, desired),
        _ => desired,
    }
}

/// BL-82 EM-3.12 — the boom's own snap-in/ease-out smoothing, a separate,
/// small state machine from [`smoothed_focus`] (design doc §5: the two ease
/// ORTHOGONAL quantities — the pivot vs. the radial arm length from it — and
/// must not fight each other). Mirrors old voxygen's `Camera::update`/
/// `compute_dependents` split: collision pull-in is always INSTANT (never
/// lerp into a wall — a wall that's already closer than the current boom
/// snaps straight to it), while growing back out toward `clamped` (which
/// equals `desired`/`CAM_BACK` once the obstruction clears) EASES at
/// [`CAMERA_FOCUS_LERP_RATE`] (the ported `THIRD_PERSON_INTERP_TIME`).
///
/// `snap` forces a direct set instead of easing even when growing outward —
/// the same first-frame / mode-reactivation / focus-teleport conditions
/// [`smoothed_focus`] snaps on (call sites keep the two in lock step so
/// re-entering third-person or a teleport never leaves a stale boom length
/// glide-fighting a freshly-snapped focus).
fn smoothed_boom(cam_dist: &mut Option<f32>, clamped: f32, snap: bool, dt: f32) -> f32 {
    let next = match *cam_dist {
        Some(d) if !snap => {
            if clamped < d {
                clamped // snap IN instantly — never lerp into a wall
            } else {
                d + (clamped - d) * (CAMERA_FOCUS_LERP_RATE * dt).min(1.0) // ease OUT
            }
        },
        _ => clamped, // first frame / snap conditions
    };
    *cam_dist = Some(next);
    next
}

/// BL-82 EM-3.12 — the `XINDELER_CAMERA_COLLISION` kill-switch's env-var →
/// bool mapping, pulled out to a pure fn (rather than inlined at the call
/// site) so its DEFAULT DIRECTION is pinned by a fast unit test. This is a
/// kill-switch (default ON, opt OUT via `=0`), the inverse of this file's
/// other debug flags (`XINDELER_CAMERA_FOCUS_PERF_LOG`, `XINDELER_SMOKE_
/// ROTATE`, default OFF, opt IN via any non-`"0"` value).
///
/// ## A real bug this exact test shape caught in review
/// An earlier version of this function read
/// `raw.is_ok_and(|v| v != "0")` — copy-pasted from this file's OPT-IN flags.
/// `Result::is_ok_and` returns `false` on `Err`, so with the env var UNSET
/// (`Err(NotPresent)`, the case for every player who never sets it) that
/// expression evaluated to `false` — silently defaulting the fix OFF for
/// everyone, exactly backwards from the doc comment's own claimed "default
/// ON" behaviour. Caught by `bevy-migration-reviewer`, not by the smoke
/// screenshots (both manual smoke runs happened to always pass the var
/// explicitly, `=0` or `=1`, so the broken UNSET default was never
/// exercised). Correct logic: negate the WHOLE check — unset or any
/// unrecognised value means "keep the fix on"; only an explicit `"0"` turns
/// it off.
fn camera_collision_enabled_from_env(raw: Result<String, std::env::VarError>) -> bool {
    !raw.is_ok_and(|v| v == "0")
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
        roll: false,
        glide_toggle: false,
        toggle_lantern: false,
        swap_loadout: false,
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

// ---------------------------------------------------------------------------
// Smoke camera-collision repro (scaffolding — smoke-screenshot only, BL-82
// EM-3.12)
// ---------------------------------------------------------------------------

/// SCAFFOLDING for the BL-82 EM-3.12 visual smoke: reproduces Matías's exact
/// reported framing ("miro al personaje desde abajo, la cámara ... traspasa
/// el piso") headlessly, by forcing the fly-cam's pitch steeply UP every
/// frame instead of relying on real mouse input the harness can't inject.
/// Added ONLY under `--listen-server --smoke-screenshot` +
/// `XINDELER_SMOKE_CAMERA_COLLISION=1`, and — unlike the other `Smoke*Cam`
/// plugins — deliberately does NOT set the camera `Transform` itself: it only
/// nudges [`FlyCam::pitch`], so the NEXT frame's [`third_person_camera`]
/// computes `forward`/`eye` (and, when the `XINDELER_CAMERA_COLLISION`
/// kill-switch is on, casts+clamps the boom) from this forced angle exactly
/// like a real player's mouse-look would — it does not bypass the collision
/// clamp, which is the entire point of the capture. Mutually exclusive with
/// `SmokeFigureCamPlugin`/`SmokeSpriteCamPlugin`/`SmokeWaterCamPlugin` (see
/// `main.rs`'s registration): those override the `Transform` directly after
/// `third_person_camera` runs and would otherwise clobber this framing.
pub struct SmokeCameraCollisionPlugin;

/// Steep upward pitch (radians) forced onto the fly-cam. With this sign
/// convention (`camera.rs`'s mouse-look: pitching up increases `pitch`, and
/// `Quat::from_euler(YXZ, yaw, pitch, 0) * NEG_Z` then carries a positive-Y
/// (upward) component), `third_person_camera`'s `eye = look_at - forward *
/// dist` puts the eye BELOW `look_at` — i.e. the camera ends up under the
/// player looking up, the exact reported framing. Kept a little short of the
/// hard ±π/2-ish pitch clamp (`camera.rs`) so the forced value survives
/// clamping unchanged.
const SMOKE_LOOK_UP_PITCH: f32 = 1.3;

impl Plugin for SmokeCameraCollisionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            smoke_force_look_up_from_below
                .after(third_person_camera)
                .in_set(GameplaySet),
        );
    }
}

/// Forces [`SMOKE_LOOK_UP_PITCH`] onto every fly-cam every frame. See
/// [`SmokeCameraCollisionPlugin`]'s doc comment for why this only touches
/// `FlyCam::pitch`, not the camera `Transform`.
fn smoke_force_look_up_from_below(mut cameras: Query<&mut FlyCam>) {
    for mut fly in &mut cameras {
        fly.pitch = SMOKE_LOOK_UP_PITCH;
    }
}

/// Bevy y-up → sim z-up direction: inverse of the converter `(x,y,z)→(x,z,−y)`,
/// i.e. bevy `(x, y, z)` → sim `(x, −z, y)`.
pub(crate) fn bevy_to_sim(v: Vec3) -> Vec3 { Vec3::new(v.x, -v.z, v.y) }

/// Bevy `Vec3` (already holding sim-axis values, post-[`bevy_to_sim`]) → the
/// `vek::Vec3<f32>` `common`'s `ReadVol::ray`/[`collide_boom`] require. A
/// plain component copy — [`bevy_to_sim`] already did the axis permutation;
/// this only changes the Rust type the same three floats are carried in.
fn to_vek(v: Vec3) -> VVec3<f32> { VVec3::new(v.x, v.y, v.z) }

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

    /// BL-82 EM-3.12 regression: Bevy "up" maps to sim +z (sim is z-up) — this
    /// pins the DIRECTION semantics [`collide_boom`]'s "downward"/"upward"
    /// reasoning (and the [`SmokeCameraCollisionPlugin`] doc comment) depend
    /// on, as a fast unit test independent of the slow, `#[ignore]`d
    /// real-world integration test.
    #[test]
    fn bevy_up_maps_to_sim_up() {
        let sim = bevy_to_sim(Vec3::Y);
        assert!((sim - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-5, "{sim:?}");
    }

    /// `gather_input` reads `GameInput::Roll`/`Glide`/`ToggleLantern`/
    /// `SwapLoadout` into their matching `LocalPlayerInput` fields. Drives
    /// `ActionState` through the real key-resolution pipeline
    /// (`xindeler_input::action_state::update_action_state`), so this also
    /// proves the DEFAULT keybinds actually reach `ActionState`, not just
    /// that `gather_input` reads whatever's already set.
    #[test]
    fn gather_input_reads_roll_glide_lantern_and_loadout_swap() {
        use bevy::input::mouse::AccumulatedMouseMotion;
        use xindeler_input::{
            ActionState, GameInput, KeyMap, KeyOrMouse, action_state::update_action_state,
        };

        let mut app = App::new();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<AccumulatedMouseMotion>();
        app.init_resource::<LocalPlayerInput>();
        app.world_mut().spawn((PrimaryWindow, CursorOptions {
            grab_mode: CursorGrabMode::Locked,
            ..Default::default()
        }));
        app.world_mut()
            .spawn((FlyCam::default(), Transform::IDENTITY));

        let key_map = app.world().resource::<KeyMap>().clone();
        for input in [
            GameInput::Roll,
            GameInput::Glide,
            GameInput::ToggleLantern,
            GameInput::SwapLoadout,
        ] {
            match key_map.keyboard.get_binding(input) {
                Some(KeyOrMouse::Key(key)) => {
                    app.world_mut()
                        .resource_mut::<ButtonInput<KeyCode>>()
                        .press(key);
                },
                Some(KeyOrMouse::Mouse(button)) => {
                    app.world_mut()
                        .resource_mut::<ButtonInput<MouseButton>>()
                        .press(button);
                },
                None => panic!("{input:?} has no default binding"),
            }
        }

        app.add_systems(Update, (update_action_state, gather_input).chain());
        app.update();

        let input = *app.world().resource::<LocalPlayerInput>();
        assert!(input.roll, "Roll must reach LocalPlayerInput");
        assert!(input.glide_toggle, "Glide must reach LocalPlayerInput");
        assert!(
            input.toggle_lantern,
            "ToggleLantern must reach LocalPlayerInput"
        );
        assert!(
            input.swap_loadout,
            "SwapLoadout must reach LocalPlayerInput"
        );
    }

    /// A Bevy heading due −z (yaw 0 forward) maps to sim +y (north).
    #[test]
    fn forward_maps_to_north() {
        let move_bevy = flatten(Vec3::new(0.0, 0.0, -1.0));
        let move_dir = Vec2::new(move_bevy.x, -move_bevy.z);
        assert!((move_dir - Vec2::new(0.0, 1.0)).length() < 1e-5);
    }

    /// Scrolling up (positive `y`, `map_view.rs`'s own scroll-zoom
    /// convention) pulls the camera IN toward [`CAMERA_ZOOM_MIN`]; scrolling
    /// down pushes it back OUT — and both directions clamp rather than
    /// overshoot the configured range.
    #[test]
    fn camera_zoom_clamps_to_the_configured_range() {
        let mut app = App::new();
        app.init_resource::<CameraZoom>();
        app.insert_resource(AccumulatedMouseScroll {
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            delta: Vec2::new(0.0, 1000.0), // scroll far past the near clamp
        });
        app.add_systems(Update, handle_camera_zoom_input);
        app.update();
        assert_eq!(app.world().resource::<CameraZoom>().0, CAMERA_ZOOM_MIN);

        app.insert_resource(AccumulatedMouseScroll {
            unit: bevy::input::mouse::MouseScrollUnit::Line,
            delta: Vec2::new(0.0, -1000.0), // scroll far past the far clamp
        });
        app.update();
        assert_eq!(app.world().resource::<CameraZoom>().0, CAMERA_ZOOM_MAX);
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

    // -----------------------------------------------------------------------
    // BL-82 EM-3.12 — collide_boom (hand-built VolGrid2d, no App/GPU)
    // -----------------------------------------------------------------------

    use common::{
        terrain::{BlockKind, MapSizeLg, TerrainChunk, TerrainChunkMeta},
        vol::{RectRasterableVol, WriteVol},
        volumes::vol_grid_2d::VolGrid2d,
    };
    use vek::{Rgb, Vec2 as VVec2};

    /// Builds a single-chunk `VolGrid2d<TerrainChunk>` (mirrors
    /// `terrain_stream.rs`'s test `solid_chunk`/`TerrainStore::new` shape)
    /// with one solid `Rock` slab spanning the whole chunk footprint from
    /// `solid_from_z` (inclusive) up `SLAB_HEIGHT` blocks, everything else
    /// air. `None` (`solid_from_z = None`) inserts no chunk at all, so a cast
    /// through it exercises the unloaded/`NoSuchChunk` → `.ignore_error()`
    /// pass-through path.
    const SLAB_HEIGHT: i32 = 8;

    fn grid_with_solid_slab(solid_from_z: Option<i32>) -> VolGrid2d<TerrainChunk> {
        let map_size_lg = MapSizeLg::new(VVec2::new(6, 6)).expect("valid map size");
        let default = std::sync::Arc::new(TerrainChunk::new(
            0,
            Block::empty(),
            Block::empty(),
            TerrainChunkMeta::void(),
        ));
        let mut grid = VolGrid2d::new(map_size_lg, default).expect("chunk size is a power of two");
        if let Some(solid_from_z) = solid_from_z {
            let mut chunk =
                TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
            let edge = TerrainChunk::RECT_SIZE.x as i32;
            for lx in 0..edge {
                for ly in 0..edge {
                    for z in solid_from_z..(solid_from_z + SLAB_HEIGHT) {
                        chunk
                            .set(
                                VVec3::new(lx, ly, z),
                                Block::new(BlockKind::Rock, Rgb::new(120, 120, 120)),
                            )
                            .expect("in-bounds write");
                    }
                }
            }
            grid.insert(VVec2::new(0, 0), std::sync::Arc::new(chunk));
        }
        grid
    }

    /// A vertical cast (pivot at z=0, straight up) into a solid slab starting
    /// at z=4 hits at distance 4 (unit voxels, DDA steps land exactly on
    /// integer boundaries) — returns `4 - CAM_NEAR_PAD`.
    #[test]
    fn collide_boom_hits_wall_at_expected_distance() {
        let grid = grid_with_solid_slab(Some(4));
        let pivot = VVec3::new(16.0, 16.0, 0.0);
        let dir = VVec3::new(0.0, 0.0, 1.0);
        let got = collide_boom(&grid, pivot, dir, 9.0);
        assert!((got - (4.0 - CAM_NEAR_PAD)).abs() < 1e-3, "got {got}");
    }

    /// No solid block within `desired` (the slab starts far above the cast
    /// range) → the full `desired` distance, unclamped.
    #[test]
    fn collide_boom_clear_air_returns_desired() {
        let grid = grid_with_solid_slab(Some(100));
        let pivot = VVec3::new(16.0, 16.0, 0.0);
        let dir = VVec3::new(0.0, 0.0, 1.0);
        let got = collide_boom(&grid, pivot, dir, 9.0);
        assert_eq!(got, 9.0);
    }

    /// A solid block starting immediately at the pivot clamps to
    /// `CAM_MIN_DIST`, never below it (never collapses the boom onto the
    /// pivot/character).
    #[test]
    fn collide_boom_clamps_to_min_dist() {
        let grid = grid_with_solid_slab(Some(0));
        let pivot = VVec3::new(16.0, 16.0, 0.0);
        let dir = VVec3::new(0.0, 0.0, 1.0);
        let got = collide_boom(&grid, pivot, dir, 9.0);
        assert_eq!(got, CAM_MIN_DIST);
    }

    /// A ray through a chunk that was never inserted (within map bounds, but
    /// no chunk stored — the real "not-yet-streamed" case) errors
    /// `NoSuchChunk` at every step; `.ignore_error()` treats that as
    /// pass-through, so the camera never clips on unstreamed terrain.
    #[test]
    fn collide_boom_unloaded_chunk_passes_through() {
        let grid = grid_with_solid_slab(None);
        let pivot = VVec3::new(16.0, 16.0, 0.0);
        let dir = VVec3::new(0.0, 0.0, 1.0);
        let got = collide_boom(&grid, pivot, dir, 9.0);
        assert_eq!(got, 9.0);
    }

    // -----------------------------------------------------------------------
    // BL-82 EM-3.12 — smoothed_boom (mirrors the smoothed_focus_* test shape)
    // -----------------------------------------------------------------------

    /// The very first call (`cam_dist` still `None`) sets directly — no
    /// glide-in from an arbitrary default.
    #[test]
    fn smoothed_boom_first_call_sets_directly() {
        let mut cam_dist = None;
        let got = smoothed_boom(&mut cam_dist, 4.0, false, 1.0 / 60.0);
        assert_eq!(got, 4.0);
        assert_eq!(cam_dist, Some(4.0));
    }

    /// Collision pull-in is INSTANT: when the clamped distance is closer than
    /// the current boom, the next value snaps straight to it, never eases.
    #[test]
    fn smoothed_boom_snaps_in_on_a_closer_hit() {
        let mut cam_dist = Some(9.0);
        let got = smoothed_boom(&mut cam_dist, 3.0, false, 1.0 / 60.0);
        assert_eq!(got, 3.0, "must snap in instantly, never lerp into a wall");
    }

    /// Growing back out toward a farther (clear) distance EASES — the new
    /// value moves only partway, not immediately to the target.
    #[test]
    fn smoothed_boom_eases_out_when_clearing() {
        let mut cam_dist = Some(3.0);
        let got = smoothed_boom(&mut cam_dist, 9.0, false, 1.0 / 60.0);
        let expected_t = (CAMERA_FOCUS_LERP_RATE / 60.0).min(1.0);
        let expected = 3.0 + (9.0 - 3.0) * expected_t;
        assert!(
            got > 3.0 && got < 9.0,
            "must ease partway out, not snap: {got}"
        );
        assert!(
            (got - expected).abs() < 1e-4,
            "got {got}, expected {expected}"
        );
    }

    /// `snap = true` (first frame / mode reactivation / focus teleport) sets
    /// directly even when growing OUTWARD — no glide-in artifact.
    #[test]
    fn smoothed_boom_snap_flag_forces_direct_set_even_when_growing() {
        let mut cam_dist = Some(1.0);
        let got = smoothed_boom(&mut cam_dist, 9.0, true, 1.0 / 60.0);
        assert_eq!(got, 9.0, "snap=true must set directly, not ease");
    }

    // -----------------------------------------------------------------------
    // BL-82 EM-3.12 — camera_collision_enabled_from_env kill-switch default
    // (regression test for the inverted-default bug bevy-migration-reviewer
    // caught: see the function's own doc comment for the full story)
    // -----------------------------------------------------------------------

    /// The env var UNSET (`Err(NotPresent)`, every player who never touches
    /// it) must default the fix ON — the whole point of a kill-switch vs. an
    /// opt-in debug flag.
    #[test]
    fn camera_collision_defaults_enabled_when_env_unset() {
        assert!(camera_collision_enabled_from_env(Err(
            std::env::VarError::NotPresent
        )));
    }

    /// Only an explicit `"0"` disables it; any other value (including
    /// nonsense) leaves it on.
    #[test]
    fn camera_collision_disabled_only_by_explicit_zero() {
        assert!(!camera_collision_enabled_from_env(Ok("0".to_owned())));
        assert!(camera_collision_enabled_from_env(Ok("1".to_owned())));
        assert!(camera_collision_enabled_from_env(Ok("bogus".to_owned())));
        assert!(camera_collision_enabled_from_env(Ok(String::new())));
    }
}
