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
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (cursor_grab, fly_cam_look, fly_cam_move)
                    .chain()
                    .in_set(FlyCamSet)
                    .in_set(GameplaySet),
            );
    }
}

/// Simple free-fly camera controller state.
#[derive(Component)]
pub struct FlyCam {
    /// Base movement speed, m/s.
    pub speed: f32,
    /// Shift multiplier on `speed`.
    pub fast_multiplier: f32,
    /// Mouse-look sensitivity, rad/px.
    pub sensitivity: f32,
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

// TODO(EM-5.11): speed/sensitivity belong in XindelerSettings (user-facing
// input settings); hardcoded defaults are Phase-2 fly-cam scaffolding only.
impl Default for FlyCam {
    fn default() -> Self {
        Self {
            speed: 12.0,
            fast_multiplier: 4.0,
            sensitivity: 0.002,
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

/// Click grabs + hides the cursor; Escape releases it.
fn cursor_grab(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor_options: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut cursor) = cursor_options.single_mut() else {
        return;
    };
    if mouse.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) && cursor.grab_mode != CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn fly_cam_look(
    motion: Res<AccumulatedMouseMotion>,
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
    // Still run with zero fresh motion this frame: a pending carry (EM-3.11k)
    // must keep draining even on a frame with no new mouse delta, rather than
    // waiting for the next real mouse event to resolve.
    for (mut transform, mut cam) in &mut cameras {
        if motion.delta == Vec2::ZERO && cam.yaw_carry == 0.0 && cam.pitch_carry == 0.0 {
            continue;
        }
        let desired_yaw = cam.yaw_carry - motion.delta.x * cam.sensitivity;
        let (yaw_step, yaw_carry) = capped_look_step(desired_yaw);
        cam.yaw += yaw_step;
        cam.yaw_carry = yaw_carry;

        let desired_pitch = cam.pitch_carry - motion.delta.y * cam.sensitivity;
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
}
