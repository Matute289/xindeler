//! Camera rig (EM-2.2): HDR `Camera3d` with TAA/SSAO/bloom/volumetric +
//! distance fog, driven by [`GraphicsSettings`], plus a simple fly-cam
//! (WASD + mouse-look, Shift = fast, click to grab cursor / Escape to
//! release).

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera::{Exposure, Hdr},
    core_pipeline::prepass::DepthPrepass,
    input::mouse::AccumulatedMouseMotion,
    pbr::{AtmosphereSettings, ContactShadows, ScreenSpaceAmbientOcclusion},
    post_process::bloom::Bloom,
    prelude::*,
    render::occlusion_culling::OcclusionCulling,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use xindeler_app::{GameplaySet, XindelerSettings};
use xindeler_oracle_host::AtmosphereProfile;

use crate::{atmosphere, post::VignettePost};

pub struct CameraRigPlugin;

/// GPU occlusion culling opt-in (EM-3.10b).
///
/// Bevy's own docs frame `OcclusionCulling` as a *measured* optimisation:
/// "Only enable it if you measure it to be a speedup on your scene" — it adds
/// a two-phase depth prepass + a per-frame HZB build, so it can cost more than
/// it saves on a scene with few/small occluders. Kept as a small local
/// resource (like `lod::CullingConfig`), not a `GraphicsSettings` toggle,
/// until it earns one (`xindeler-app` is out of scope for this crate's
/// changes — see the `lod::CullingConfig` TODO for the same deferral).
///
/// ## EM-3.10b measurement
/// Measured on the listen-server smoke scene (open highlands terrain +
/// streamed chunk/fluid/sprite meshes + ~8 wandering test-NPC figures — the
/// densest occluder set this branch could construct without altering
/// worldgen), dev-profile build, via the `XINDELER_PERF_LOG=1` rolling
/// frame-time log (`perf_log.rs`): two 100 s runs (`XINDELER_OCCLUSION_
/// CULLING=0` vs `=1`), averaging the last 20 steady-state samples (~40 s,
/// well past the terrain/figure/NPC warmup) of each:
/// - OFF: **34.35 ms/frame** (≈29.1 fps)
/// - ON:  **34.38 ms/frame** (≈29.1 fps)
///
/// No measurable difference (< 0.1%, inside run-to-run noise) — both runs
/// were also flat at ~30 fps throughout. **This measurement predates EM-3.11b**
/// (`xindeler-sim-bridge::tick_sim`'s doc): at the time, this windowed
/// listen-server App ran the embedded sim's `tick_sim`/`tick_player` in
/// `Update` (display rate, NOT the sim's intended 30 TPS — that was true only
/// of the *headless* `xindeler-server-app` shell), so a real, non-render CPU
/// cost (2–5× too much full-server-tick work) was baked into every one of
/// these frames alongside whatever the GPU was doing; the flat ~29 fps here
/// cannot be blamed on vsync/GPU-headroom alone. EM-3.11b moved the sim tick
/// to `FixedUpdate` at a real 30 Hz and separately confirmed via
/// `XINDELER_PERF_LOG` that the vsync/present-mode theory floated below was
/// only a partial explanation — see `docs/backlog/engine-migration.md`
/// EM-3.11b for the current numbers. Re-measuring occlusion culling itself on
/// top of the EM-3.11b fix is still future work; what's solid from THIS run:
/// occlusion culling made no measurable difference in this scene, matching
/// EM-3.10's prediction that the smoke world's few/small occluders wouldn't
/// earn back the two-phase depth prepass + HZB cost. Ships **opt-in, default
/// OFF** (`XINDELER_OCCLUSION_CULLING=1` to try it; `GraphicsTier` presets can
/// wire a real toggle once `xindeler-app` picks this up, and a denser scene —
/// a real town/dungeon site — is the honest way to re-measure this later).
///
/// (Original note, now superseded by the above: "the flat ~29 fps here points
/// to vsync/present-mode capping the frame, not necessarily 'no GPU
/// headroom'... re-measure with an uncapped present mode... before trusting
/// 'no headroom' as the reason" — re-measured in EM-3.11b; the sim-tick
/// overrun was the bigger factor.)
#[derive(Resource, Debug, Clone, Copy)]
pub struct OcclusionCullingConfig {
    pub enabled: bool,
}

impl Default for OcclusionCullingConfig {
    fn default() -> Self {
        // Env override so the A/B measurement above (and any future re-check
        // on a denser scene) doesn't need a code change.
        let enabled = std::env::var("XINDELER_OCCLUSION_CULLING")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .map(|v| v != 0)
            .unwrap_or(false);
        Self { enabled }
    }
}

/// System set covering the fly-cam controller (cursor grab + look + move). The
/// listen-server player rig (`player_input`) orders its follow-camera AFTER
/// this so it reads an up-to-date yaw and can override the transform (EM-3.7b).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlyCamSet;

/// Gates whether [`fly_cam_move`] actually translates the camera. Default on
/// (free fly-cam). The third-person player rig turns it OFF while following so
/// it can own the camera translation without the fly-cam fighting it; mouse
/// look ([`fly_cam_look`]) keeps running either way so its yaw drives the
/// orbit.
#[derive(Resource)]
pub struct FlyCamMovementEnabled(pub bool);

impl Default for FlyCamMovementEnabled {
    fn default() -> Self { Self(true) }
}

impl Plugin for CameraRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FlyCamMovementEnabled>()
            .init_resource::<OcclusionCullingConfig>()
            // BL-82 EM-5.17: the shared cursor-free signal — always present so
            // `cursor_grab` can read it in every mode (only the feature-gated
            // `cursor::update_cursor_free` ever writes it; see `CursorFree`).
            .init_resource::<CursorFree>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (cursor_grab, fly_cam_look, fly_cam_move)
                    .chain()
                    .in_set(FlyCamSet)
                    .in_set(GameplaySet)
                    // BL-82 EM-5.11: `fly_cam_look` reads `ActionState` (the
                    // gamepad look stick) — order after it resolves this
                    // frame's real gamepad state rather than reading a
                    // frame-stale value.
                    .after(xindeler_input::InputResolveSet),
            );
    }
}

/// Marks the primary player/fly camera (the HDR `Camera3d` this module spawns
/// in [`spawn_camera`]). A distinct marker because this crate spawns MORE than
/// one `Camera3d` (e.g. `far_terrain.rs`'s horizon camera), so a system that
/// must target ONLY the main view — e.g. the esc-menu graphics live-apply
/// (`crate::esc_menu`) toggling SSAO/TAA on the player camera, not the
/// far-terrain pass — queries `With<MainCamera>` rather than the ambiguous
/// `With<Camera3d>`.
///
/// `pub(crate)` — only this crate's own screens (`esc_menu`) query it; it is
/// not part of any cross-crate contract.
#[derive(Component, Debug, Default)]
pub(crate) struct MainCamera;

/// Whether the OS cursor should currently be FREE (visible + ungrabbed)
/// because some UI element needs pointer input — a HUD window is open, the
/// chat input box has keyboard focus, or the game is paused (BL-82 EM-5.17 —
/// the "cursor doesn't appear when a UI panel is open" fix).
///
/// This is the ONE shared cursor-free signal [`cursor_grab`] reads: while it
/// is `true` the cursor is forced free and a click can NOT re-grab it (so the
/// player can actually click a panel's controls); while it is `false` the
/// normal fly-cam controls apply (click grabs / Escape releases), and a
/// cursor that was auto-freed for a now-closed UI element re-grabs for
/// mouselook (legacy `voxygen`'s `want_grab` behaviour, `voxygen/src/hud/
/// mod.rs`).
///
/// It is aggregated each frame by `crate::cursor::update_cursor_free` (a
/// feature-gated client system) from [`xindeler_ui::hud_state::HudState::
/// any_window_open`] plus `crate::chat::text_input_focused`. In the pure
/// demo / fly-cam mode (no HUD, no chat — `cursor.rs` is not compiled) nothing
/// writes it, so it stays `false` and the fly-cam's click-to-grab / Escape-to-
/// release controls behave exactly as before. Always present:
/// [`CameraRigPlugin`] `init_resource`s it unconditionally so [`cursor_grab`]
/// can read it in every mode.
#[derive(Resource, Debug, Default)]
pub struct CursorFree(pub bool);

/// Simple free-fly camera controller state.
#[derive(Component)]
pub struct FlyCam {
    /// Base movement speed, m/s.
    pub speed: f32,
    /// Shift multiplier on `speed`.
    pub fast_multiplier: f32,
    /// Mouse-look sensitivity, rad/px.
    pub sensitivity: f32,
    /// Inverts the pitch axis (BL-82 EM-5.12+, ported from legacy
    /// `gameplay.invert_mouse_y`) — `true` flips [`fly_cam_look`]'s pitch
    /// contribution so pushing the mouse up looks DOWN. Baked from
    /// `CameraSettings::invert_pitch` at spawn time, same as
    /// `sensitivity`/`speed` above.
    pub invert_pitch: bool,
    pub yaw: f32,
    pub pitch: f32,
    /// Rotation left over from a frame whose desired yaw/pitch step exceeded
    /// [`MAX_LOOK_STEP_RAD`] (EM-3.11k) — applied on the following frame(s)
    /// instead of being dropped, so the full input still lands, just spread
    /// out instead of landing in one oversized step. See
    /// [`MAX_LOOK_STEP_RAD`]'s doc for why this exists.
    yaw_carry: f32,
    pitch_carry: f32,
}

// BL-82 EM-5.11: speed/sensitivity now come from `XindelerSettings::camera`
// (see `spawn_camera`) — this `Default` impl is the FALLBACK when no
// settings are threaded through (e.g. a hand-built `FlyCam` in a test), kept
// numerically identical to the values `CameraSettings::default()` also uses
// so nothing changes for a fresh install.
impl Default for FlyCam {
    fn default() -> Self {
        Self {
            speed: 12.0,
            fast_multiplier: 4.0,
            sensitivity: 0.002,
            invert_pitch: false,
            yaw: 0.0,
            pitch: 0.0,
            yaw_carry: 0.0,
            pitch_carry: 0.0,
        }
    }
}

/// Per-frame yaw/pitch rotation ceiling, radians (EM-3.11k — "brightness
/// flicker" investigation).
///
/// Root cause: Bevy's built-in `TemporalAntiAliasing` keeps a per-pixel
/// history-confidence counter baked into the taa shader (`bevy_anti_alias`,
/// not exposed as a tunable on the component — only `reset: bool` is public):
/// it resets to full weight-on-current-frame the instant a pixel's motion
/// vector exceeds ~0.01px, and otherwise climbs, biasing the frame toward
/// heavy (up to ~98.5%) history blending while the view stays still. Camera
/// ROTATION moves nearly every pixel's projected position at once (unlike
/// translation, where distant/background pixels barely move), so whenever
/// the player looks around, confidence broadly resets and the frame leans on
/// the crisper, unblended current sample; the instant rotation is small
/// again, confidence quickly climbs back up and the picture leans back on
/// several frames of jittered, softly-averaged history. Confirmed
/// empirically (`XINDELER_BRIGHTNESS_PROBE` debug harness, since reverted):
/// consecutive offscreen captures during scripted forward walking (no
/// rotation) never showed a discrete jump, but the SAME scene with the
/// camera yaw continuously sweeping showed repeated one-frame luminance
/// pops (+3 then -3, alternating) exactly matching Matías's "brillo/
/// contraste sube por un instante" report — a whole-frame, uniform
/// brightening (confirmed via a diff of consecutive frames), not a
/// localized geometry edge.
///
/// The trigger for the ALTERNATION (not just "rotation causes some popping",
/// but a sharp one-frame spike sandwiched between normal frames) is this
/// project's own already-tracked, still-open frame-pacing stutter
/// (EM-3.11c/d/e: the embedded sim tick can eat an uneven slice of a
/// render frame's budget). `AccumulatedMouseMotion::delta` naturally
/// accumulates however much real mouse motion happened since the last
/// `Update` tick — so when a tick is delayed by a slow sim step, more
/// motion piles into that ONE tick, immediately followed by a quick
/// catch-up tick with comparatively little residual motion. That backlog
/// spike rotates the WHOLE screen further in one frame than its neighbours,
/// tripping the confidence reset broadly for exactly one frame — the "pop".
///
/// We have no public API into the TAA shader's internal thresholds, and
/// this project's own frame-pacing stutter is a separate, already-tracked
/// epic this fix does not attempt to solve outright. What IS in scope and
/// fully in our control: preventing a single Update tick from ever applying
/// an outsized rotation step, by capping it and carrying any excess into
/// the next tick(s) (mirrors the `Time::<Virtual>::max_delta` clamp EM-3.11c
/// already applied to the FixedUpdate catch-up spiral — same shape of fix,
/// applied to camera rotation instead of the sim tick). Chosen generously —
/// ~172°/frame — so it only ever engages on a genuine multi-frame backlog
/// spike; a single ~16-33ms frame cannot get anywhere near this from actual
/// human mouse movement, so normal play (including fast, deliberate flicks)
/// is unaffected. Residual uncertainty: this reduces the frame-to-frame
/// motion-vector inconsistency that triggers the TAA confidence reset, but
/// since we can't tune the TAA shader itself, an underlying sim-tick stutter
/// severe enough to still cause single-frame spikes bigger than this ceiling
/// remains possible in principle — full confirmation needs Matías's own
/// in-game eyeball check under his actual mouse-look play.
const MAX_LOOK_STEP_RAD: f32 = 3.0;

/// Splits a desired rotation delta into what to apply THIS frame (bounded by
/// [`MAX_LOOK_STEP_RAD`]) and what to carry into the next. Pure + unit-tested
/// in isolation (see the `tests` module).
fn capped_look_step(desired: f32) -> (f32, f32) {
    if desired.abs() <= MAX_LOOK_STEP_RAD {
        (desired, 0.0)
    } else {
        let step = MAX_LOOK_STEP_RAD.copysign(desired);
        (step, desired - step)
    }
}

fn spawn_camera(
    mut commands: Commands,
    settings: Res<XindelerSettings>,
    occlusion: Res<OcclusionCullingConfig>,
) {
    let graphics = &settings.graphics;
    // Spawn-time fog matches the default profile so EM-2.4's first applied
    // AtmosphereController state is a visual no-op (no boot pop).
    let boot_profile = AtmosphereProfile::default();

    // Reframed for EM-3.5: the voxel demo is now a 5×5 chunk grid spanning
    // Bevy x ∈ [32, 192], z ∈ [-192, -32] (center ~(112, ~7, -112), heights
    // ≤ 15) — look at it diagonally from above the EM-2.2 scene corner so
    // the smoke screenshot shows the terrain continuous across chunks.
    let transform =
        Transform::from_xyz(44.0, 38.0, -44.0).looking_at(Vec3::new(124.0, 4.0, -124.0), Vec3::Y);
    let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);

    let mut camera = commands.spawn((
        MainCamera,
        Camera3d::default(),
        Hdr,
        // TAA requires Msaa::Off; MSAA also fights greedy meshing, so it
        // stays off regardless of the TAA toggle.
        Msaa::Off,
        transform,
        // The sun uses physical illuminance (RAW_SUNLIGHT), so compensate
        // exposure like the upstream atmosphere example.
        Exposure { ev100: 13.0 },
        // Picks up the standalone `Atmosphere` entity spawned by the light rig.
        AtmosphereSettings::default(),
        // Driven at runtime by the AtmosphereController (EM-2.4).
        atmosphere::distance_fog_from(&boot_profile),
        FlyCam {
            yaw,
            pitch,
            // BL-82 EM-5.11: sourced from the persisted `CameraSettings`
            // (closes the `TODO(EM-5.11)` this field used to carry) — a
            // fresh install's values are numerically identical to the old
            // hardcoded defaults, so this is a value-preserving move.
            speed: settings.camera.fly_speed,
            fast_multiplier: settings.camera.fly_fast_multiplier,
            sensitivity: settings.camera.mouse_sensitivity,
            invert_pitch: settings.camera.invert_pitch,
            ..Default::default()
        },
    ));

    if graphics.taa {
        camera.insert(TemporalAntiAliasing::default());
    }
    if graphics.ssao {
        camera.insert(ScreenSpaceAmbientOcclusion::default());
    }
    if graphics.bloom {
        camera.insert(Bloom::NATURAL);
    }
    if graphics.volumetric_fog {
        camera.insert(atmosphere::volumetric_fog_from(&boot_profile));
    }
    if graphics.contact_shadows {
        camera.insert(ContactShadows::default());
    }
    if graphics.vignette {
        // EM-2.6: custom post-process pass (vignette + gamma placeholder).
        camera.insert(VignettePost::default());
    }
    if occlusion.enabled {
        // `OcclusionCulling` requires a `DepthPrepass` on the view (Bevy
        // ignores it otherwise); TAA already requires one via `#[require]`
        // when enabled, but insert it explicitly so occlusion culling works
        // even with TAA off. Idempotent — Bevy no-ops a duplicate insert.
        camera.insert((DepthPrepass, OcclusionCulling));
    }
}

/// Owns the OS cursor's grab state (BL-82 EM-5.17).
///
/// While [`CursorFree`] is `true` (a HUD window is open, chat input has focus,
/// or the game is paused) the cursor is forced free (visible + ungrabbed) and
/// a click can NOT re-grab it — so the player can actually click a panel's
/// controls. This was the bug: before this, opening the Diary/Inventory/Map/
/// Chat left the cursor grabbed+hidden (or a stray click re-grabbed it), so
/// none of the new panels were clickable at all.
///
/// While [`CursorFree`] is `false` the normal fly-cam controls apply — click
/// grabs + hides, Escape releases — AND a cursor that we auto-freed for a UI
/// element that has since closed re-grabs for mouselook (legacy `voxygen`'s
/// `want_grab` re-grab on window close, `voxygen/src/hud/mod.rs`). The
/// `released_for_ui` latch makes that re-grab conditional: it only fires if WE
/// released a grabbed cursor for the UI, so a cursor the player had manually
/// freed (Escape) before opening a window is left free when it closes, rather
/// than being surprised by a force-grab.
fn cursor_grab(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    cursor_free: Res<CursorFree>,
    mut released_for_ui: Local<bool>,
    mut cursor_options: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut cursor) = cursor_options.single_mut() else {
        return;
    };

    if cursor_free.0 {
        // A UI element needs the pointer: ensure the cursor is free and never
        // let a click re-grab it while it is. Remember that WE released a
        // grabbed cursor so mouselook can be restored once the UI closes.
        if cursor.grab_mode != CursorGrabMode::None {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
            *released_for_ui = true;
        }
        return;
    }

    // No UI needs the pointer. If we auto-freed the cursor for a UI element
    // that has now closed, restore the mouselook grab and STOP — do NOT fall
    // through to the manual handlers below. The window that just closed was
    // almost always dismissed WITH Escape (closing the pause menu, the map,
    // etc.), so `keys.just_pressed(Escape)` is still true on this very frame;
    // without this early return the manual Escape-release handler at the
    // bottom would immediately undo the re-grab we just performed, dumping the
    // player back to a free cursor with no mouselook (bevy-migration-reviewer
    // finding). Skipping the manual handlers here is correct: this frame's
    // click/Escape belonged to the UI interaction, not to a fly-cam command.
    if *released_for_ui {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
        *released_for_ui = false;
        return;
    }
    if mouse.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) && cursor.grab_mode != CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// BL-82 EM-5.11 — gamepad right-stick look rate, radians/second at full
/// deflection. A held analog value (unlike a one-shot mouse delta) needs a
/// per-second rate scaled by `time.delta_secs()`, not a per-pixel
/// sensitivity — chosen as a moderate, controllable turn speed (a full
/// second at max deflection turns a bit less than a half-circle).
const GAMEPAD_LOOK_RATE_RAD_S: f32 = 2.2;

fn fly_cam_look(
    motion: Res<AccumulatedMouseMotion>,
    action_state: Res<xindeler_input::ActionState>,
    time: Res<Time>,
    cursor_options: Query<&CursorOptions, With<PrimaryWindow>>,
    mut cameras: Query<(&mut Transform, &mut FlyCam)>,
) {
    let grabbed = cursor_options
        .single()
        .is_ok_and(|cursor| cursor.grab_mode != CursorGrabMode::None);
    if !grabbed {
        // Not controlling the camera — drop any pending carry (EM-3.11k) so
        // an old backlog spike doesn't surface as a delayed turn on re-grab.
        for (_, mut cam) in &mut cameras {
            cam.yaw_carry = 0.0;
            cam.pitch_carry = 0.0;
        }
        return;
    }
    // BL-82 EM-5.11: the gamepad right stick contributes an ADDITIONAL
    // per-frame rotation on top of the mouse delta (never a replacement —
    // a player can nudge the stick while also moving the mouse and both
    // apply). Deadzone/inversion are already applied by `ActionState`
    // (`xindeler_input::gamepad::GamepadBindings::apply_axis`), so this is
    // just the rate-scaling step. Negated the same way the mouse delta is
    // (`-motion.delta.x`) so pushing the stick right/up turns the camera
    // right/up, matching mouse convention.
    let gamepad_look = action_state.look_axis * GAMEPAD_LOOK_RATE_RAD_S * time.delta_secs();
    // Still run with zero fresh motion this frame: a pending carry (EM-3.11k)
    // must keep draining even on a frame with no new mouse delta, rather than
    // waiting for the next real mouse event to resolve.
    for (mut transform, mut cam) in &mut cameras {
        if motion.delta == Vec2::ZERO
            && gamepad_look == Vec2::ZERO
            && cam.yaw_carry == 0.0
            && cam.pitch_carry == 0.0
        {
            continue;
        }
        let desired_yaw = cam.yaw_carry - motion.delta.x * cam.sensitivity - gamepad_look.x;
        let (yaw_step, yaw_carry) = capped_look_step(desired_yaw);
        cam.yaw += yaw_step;
        cam.yaw_carry = yaw_carry;

        // BL-82 EM-5.12+ (invert-Y port): flips ONLY the fresh mouse-delta
        // contribution, never the gamepad stick term or the carried-over
        // remainder — the stick already applies its own inversion upstream
        // (`xindeler_input::gamepad::GamepadBindings::apply_axis`, see this
        // function's own doc above), and the carry is just deferred motion
        // from a PREVIOUS frame that was already signed correctly when it
        // was computed.
        let pitch_mouse_delta = if cam.invert_pitch {
            -motion.delta.y
        } else {
            motion.delta.y
        };
        let desired_pitch = cam.pitch_carry - pitch_mouse_delta * cam.sensitivity - gamepad_look.y;
        let (pitch_step, pitch_carry) = capped_look_step(desired_pitch);
        cam.pitch_carry = pitch_carry;
        cam.pitch = (cam.pitch + pitch_step).clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );

        transform.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
    }
}

fn fly_cam_move(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    enabled: Res<FlyCamMovementEnabled>,
    mut cameras: Query<(&mut Transform, &FlyCam)>,
) {
    // Third-person follow (EM-3.7b) turns this off so it owns the translation.
    if !enabled.0 {
        return;
    }
    for (mut transform, cam) in &mut cameras {
        let mut wish = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            wish += *transform.forward();
        }
        if keys.pressed(KeyCode::KeyS) {
            wish += *transform.back();
        }
        if keys.pressed(KeyCode::KeyA) {
            wish += *transform.left();
        }
        if keys.pressed(KeyCode::KeyD) {
            wish += *transform.right();
        }
        if keys.pressed(KeyCode::Space) {
            wish += Vec3::Y;
        }
        if keys.pressed(KeyCode::ControlLeft) {
            wish -= Vec3::Y;
        }
        let speed = cam.speed
            * if keys.pressed(KeyCode::ShiftLeft) {
                cam.fast_multiplier
            } else {
                1.0
            };
        transform.translation += wish.normalize_or_zero() * speed * time.delta_secs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawns a primary-window entity carrying a [`CursorOptions`] in the
    /// given grab state — the minimal fixture [`cursor_grab`]'s query needs.
    fn spawn_window(app: &mut App, grab_mode: CursorGrabMode, visible: bool) -> Entity {
        app.world_mut()
            .spawn((PrimaryWindow, CursorOptions {
                grab_mode,
                visible,
                ..Default::default()
            }))
            .id()
    }

    /// BL-82 EM-5.17 (the bug): with [`CursorFree`] `true` (a HUD panel open /
    /// chat focused / paused) the cursor must be forced visible + ungrabbed so
    /// the panel is clickable, and a left click must NOT re-grab it (the exact
    /// failure Matías hit — opening the Diary left the cursor hidden/grabbed,
    /// or a click into the panel re-grabbed it, so nothing was clickable).
    #[test]
    fn ui_open_frees_the_cursor_and_a_click_cannot_regrab_it() {
        use bevy::input::mouse::MouseButton;

        let mut app = App::new();
        app.init_resource::<CursorFree>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        let window = spawn_window(&mut app, CursorGrabMode::Locked, false);
        app.add_systems(Update, cursor_grab);

        // A UI element opens -> the cursor must free up.
        app.world_mut().resource_mut::<CursorFree>().0 = true;
        app.update();
        {
            let cursor = app.world().get::<CursorOptions>(window).unwrap();
            assert_eq!(
                cursor.grab_mode,
                CursorGrabMode::None,
                "an open UI panel must ungrab the cursor"
            );
            assert!(
                cursor.visible,
                "an open UI panel must make the cursor visible"
            );
        }

        // A left click while the UI is open must NOT re-grab (so the player
        // can click the panel's controls).
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None,
            "clicking inside an open panel must not re-grab the cursor"
        );
    }

    /// Closing the UI restores mouselook: a cursor we auto-freed for an open
    /// panel re-grabs once [`CursorFree`] goes back to `false` (legacy
    /// `want_grab` re-grab on window close). Uses a real two-frame `App` run so
    /// the system's `released_for_ui` `Local` latch carries between frames.
    #[test]
    fn closing_the_ui_regrabs_for_mouselook() {
        use bevy::input::mouse::MouseButton;

        let mut app = App::new();
        app.init_resource::<CursorFree>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        let window = spawn_window(&mut app, CursorGrabMode::Locked, false);
        app.add_systems(Update, cursor_grab);

        // Frame 1: UI open -> cursor freed (and the latch remembers we did it).
        app.world_mut().resource_mut::<CursorFree>().0 = true;
        app.update();
        assert_eq!(
            app.world().get::<CursorOptions>(window).unwrap().grab_mode,
            CursorGrabMode::None
        );

        // Frame 2: UI closed -> the cursor we auto-freed re-grabs for mouselook.
        app.world_mut().resource_mut::<CursorFree>().0 = false;
        app.update();
        let cursor = app.world().get::<CursorOptions>(window).unwrap();
        assert_eq!(
            cursor.grab_mode,
            CursorGrabMode::Locked,
            "closing the last panel must re-grab the cursor for camera mouselook"
        );
        assert!(
            !cursor.visible,
            "a re-grabbed mouselook cursor must be hidden"
        );
    }

    /// The Escape-close path specifically: a window dismissed WITH Escape must
    /// STILL re-grab for mouselook — the `released_for_ui` branch's early
    /// return must beat the manual Escape-release handler on the same frame
    /// (bevy-migration-reviewer finding). Without the early return, the
    /// still-pressed Escape would immediately re-free the cursor we just
    /// re-grabbed, leaving the player cursor-free with no mouselook after
    /// closing the pause menu.
    #[test]
    fn closing_the_ui_with_escape_still_regrabs() {
        use bevy::input::mouse::MouseButton;

        let mut app = App::new();
        app.init_resource::<CursorFree>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        let window = spawn_window(&mut app, CursorGrabMode::Locked, false);
        app.add_systems(Update, cursor_grab);

        // Frame 1: UI open -> cursor freed, latch set.
        app.world_mut().resource_mut::<CursorFree>().0 = true;
        app.update();

        // Frame 2: UI closed by Escape (the key is still just-pressed this
        // frame) -> must re-grab, and the manual Escape-release must NOT undo it.
        app.world_mut().resource_mut::<CursorFree>().0 = false;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();

        let cursor = app.world().get::<CursorOptions>(window).unwrap();
        assert_eq!(
            cursor.grab_mode,
            CursorGrabMode::Locked,
            "closing a window WITH Escape must still re-grab — the manual Escape-release must not \
             undo the re-grab on the same frame"
        );
        assert!(!cursor.visible);
    }

    /// With no UI open, a cursor the player had already freed themselves
    /// (Escape) must NOT be surprise-grabbed just because a frame ticks: the
    /// re-grab only fires for a cursor WE auto-freed for a UI element. Here
    /// `CursorFree` is never set, so the fly-cam's manual controls own the
    /// cursor and a resting (ungrabbed) cursor stays free.
    #[test]
    fn no_ui_leaves_a_manually_freed_cursor_alone() {
        use bevy::input::mouse::MouseButton;

        let mut app = App::new();
        app.init_resource::<CursorFree>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        let window = spawn_window(&mut app, CursorGrabMode::None, true);
        app.add_systems(Update, cursor_grab);

        app.update();
        let cursor = app.world().get::<CursorOptions>(window).unwrap();
        assert_eq!(
            cursor.grab_mode,
            CursorGrabMode::None,
            "with no UI and no click, a free cursor must stay free (no surprise grab)"
        );
        assert!(cursor.visible);
    }

    /// A normal, in-band rotation step (well under the ceiling — every real
    /// single-frame mouse delta) applies in FULL with nothing carried over,
    /// so ordinary play is bit-for-bit unaffected by EM-3.11k's cap.
    #[test]
    fn in_band_step_applies_fully_no_carry() {
        let (step, carry) = capped_look_step(0.05);
        assert!((step - 0.05).abs() < f32::EPSILON);
        assert_eq!(carry, 0.0);

        let (step, carry) = capped_look_step(-0.05);
        assert!((step - (-0.05)).abs() < f32::EPSILON);
        assert_eq!(carry, 0.0);
    }

    /// A step at exactly the ceiling still applies in full (boundary case).
    #[test]
    fn step_at_ceiling_applies_fully() {
        let (step, carry) = capped_look_step(MAX_LOOK_STEP_RAD);
        assert!((step - MAX_LOOK_STEP_RAD).abs() < f32::EPSILON);
        assert_eq!(carry, 0.0);
    }

    /// An oversized step (a stutter-induced backlog spike, EM-3.11k) is
    /// clamped to the ceiling and the remainder is returned to carry into
    /// the next frame — no input is ever lost, only deferred.
    #[test]
    fn oversized_step_is_capped_and_remainder_carried() {
        let (step, carry) = capped_look_step(5.0);
        assert!((step - MAX_LOOK_STEP_RAD).abs() < f32::EPSILON);
        assert!((carry - 2.0).abs() < 1e-5);

        // Sign is preserved for a negative oversized step too.
        let (step, carry) = capped_look_step(-5.0);
        assert!((step - (-MAX_LOOK_STEP_RAD)).abs() < f32::EPSILON);
        assert!((carry - (-2.0)).abs() < 1e-5);
    }

    /// Feeding a carried remainder back in (as `fly_cam_look` does every
    /// frame) converges to the full originally-desired rotation over a few
    /// frames instead of a single oversized one — the fix SPREADS a spike,
    /// it doesn't discard part of it.
    #[test]
    fn carry_converges_to_full_rotation_over_frames() {
        // Simulate a single 10-radian backlog spike landing in one Update
        // tick, with zero fresh motion on every following tick.
        let mut carry = 0.0_f32;
        let mut total_applied = 0.0_f32;
        let (step, next_carry) = capped_look_step(10.0 + carry);
        total_applied += step;
        carry = next_carry;
        for _ in 0..20 {
            if carry == 0.0 {
                break;
            }
            let (step, next_carry) = capped_look_step(carry);
            total_applied += step;
            carry = next_carry;
        }
        assert_eq!(carry, 0.0, "carry must fully drain within a few frames");
        assert!(
            (total_applied - 10.0).abs() < 1e-4,
            "no rotation is lost, only spread out: {total_applied}"
        );
    }

    /// BL-82 (legacy `gameplay.invert_mouse_y` port): [`FlyCam::invert_pitch`]
    /// flips only the fresh mouse-delta term of [`fly_cam_look`]'s pitch
    /// integration — with a zero carry and zero gamepad look (both true
    /// here), a normal and an inverted cam fed the SAME mouse delta must end
    /// the frame with exactly opposite pitch, proving the sign flip is real
    /// and isolated to the mouse contribution (not, say, a global negation
    /// that would also flip yaw or the gamepad stick).
    #[test]
    fn invert_pitch_flips_only_the_mouse_pitch_contribution() {
        let mut app = App::new();
        app.insert_resource(AccumulatedMouseMotion {
            delta: Vec2::new(0.0, 10.0),
        });
        app.init_resource::<xindeler_input::ActionState>();
        app.init_resource::<Time>();
        app.world_mut().spawn((PrimaryWindow, CursorOptions {
            grab_mode: CursorGrabMode::Locked,
            visible: false,
            ..Default::default()
        }));
        let normal = app
            .world_mut()
            .spawn((Transform::IDENTITY, FlyCam {
                sensitivity: 0.01,
                ..Default::default()
            }))
            .id();
        let inverted = app
            .world_mut()
            .spawn((Transform::IDENTITY, FlyCam {
                sensitivity: 0.01,
                invert_pitch: true,
                ..Default::default()
            }))
            .id();
        app.add_systems(Update, fly_cam_look);
        app.update();

        let normal_pitch = app.world().get::<FlyCam>(normal).unwrap().pitch;
        let inverted_pitch = app.world().get::<FlyCam>(inverted).unwrap().pitch;
        assert_ne!(
            normal_pitch, 0.0,
            "sanity: the mouse delta must move the pitch at all"
        );
        assert!(
            (normal_pitch + inverted_pitch).abs() < 1e-6,
            "the same mouse delta must land at exactly opposite pitch when inverted: \
             normal={normal_pitch} inverted={inverted_pitch}"
        );
    }
}
