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
const CAM_UP: f32 = 6.0;
const CAM_LOOK_UP: f32 = 1.0;

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
fn third_person_camera(
    mode: Res<ThirdPersonActive>,
    // The player's interpolated presentation transform (smooth) — the same one
    // the entity_view drives; falling back to NetLocalPlayer's Transform if the
    // interpolation buffer isn't attached yet.
    player: Query<(&Transform, Option<&Interpolated>), (With<NetLocalPlayer>, Without<FlyCam>)>,
    mut cameras: Query<(&mut Transform, &FlyCam), Without<NetLocalPlayer>>,
) {
    if !mode.0 {
        return;
    }
    let Ok((player_tf, interp)) = player.single() else {
        return; // no player entity mirrored yet — keep the spectator fly-cam
    };
    let player_pos = interp.map_or(player_tf.translation, |i| i.pos);

    for (mut cam_tf, fly) in &mut cameras {
        // Horizontal heading from the fly-cam yaw (mouse-orbit), pitch tilts the
        // eye up/down a little via CAM_UP scaling.
        let (sin_y, cos_y) = fly.yaw.sin_cos();
        // Bevy: yaw 0 looks toward −z; forward = (sin(yaw)? ...) — derive the
        // backward offset directly from the fly-cam yaw convention (yaw about
        // +Y, EulerRot::YXZ): forward_xz = (−sin yaw, −cos yaw).
        let back = Vec3::new(sin_y, 0.0, cos_y); // opposite of forward_xz
        let eye = player_pos + back * CAM_BACK + Vec3::Y * CAM_UP;
        let look_at = player_pos + Vec3::Y * CAM_LOOK_UP;
        *cam_tf = Transform::from_translation(eye).looking_at(look_at, Vec3::Y);
    }
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
pub struct SmokeAutoMovePlugin;

impl Plugin for SmokeAutoMovePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SmokeAutoMoveState>().add_systems(
            Update,
            smoke_auto_move.after(gather_input).in_set(GameplaySet),
        );
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

/// Forces a steady forward (sim +y / north) walk + look while the player
/// exists, and raises [`crate::smoke::SmokePlayerMoved`] once the character has
/// actually travelled [`SMOKE_MOVE_THRESHOLD`], so the capture lands on a frame
/// that shows the walking player. Only meaningful once a player entity is
/// mirrored.
fn smoke_auto_move(
    player: Query<&Transform, With<NetLocalPlayer>>,
    mut input: ResMut<LocalPlayerInput>,
    mut state: ResMut<SmokeAutoMoveState>,
    mut moved: ResMut<crate::smoke::SmokePlayerMoved>,
) {
    let Ok(tf) = player.single() else {
        return;
    };
    *input = LocalPlayerInput {
        move_dir: Vec2::new(0.0, 1.0),
        jump: false,
        look: Vec3::new(0.0, 1.0, 0.0),
    };
    let pos = tf.translation;
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

/// Points the smoke camera at the nearest non-player mirrored figure from a
/// close, elevated 3/4 angle, so the capture shows an assembled `.vox` NPC.
/// Picks the NPC closest to the player (or world centre) — the wandering test
/// critters spawn in a ring around the anchor on open terrain.
fn smoke_figure_cam(
    players: Query<&Transform, With<NetLocalPlayer>>,
    figures: Query<
        (&Transform, Option<&Interpolated>),
        (With<Children>, Without<NetLocalPlayer>, Without<FlyCam>),
    >,
    mut cameras: Query<&mut Transform, (With<FlyCam>, Without<NetLocalPlayer>)>,
) {
    let focus = players.iter().next().map_or(Vec3::ZERO, |t| t.translation);
    // Nearest child-bearing (= assembled figure) NPC to the player/centre.
    let subject = figures
        .iter()
        .map(|(tf, interp)| interp.map_or(tf.translation, |i| i.pos))
        .min_by(|a, b| {
            a.distance_squared(focus)
                .total_cmp(&b.distance_squared(focus))
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
}
