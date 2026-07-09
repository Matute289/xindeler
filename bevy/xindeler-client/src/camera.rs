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
/// were also flat at ~30 fps throughout. This windowed listen-server App
/// runs its own render loop at DISPLAY rate (not the embedded sim's 30 TPS,
/// which only paces the *headless* `xindeler-server-app` shell — see
/// `xindeler-sim-bridge::tick_sim`'s doc), so the flat ~29 fps here points to
/// vsync/present-mode capping the frame, not necessarily "no GPU headroom" —
/// this measurement does NOT rule out a genuine GPU-render-bound cost that
/// vsync happens to be masking; re-measure with an uncapped present mode (or
/// on a scene dense enough to blow past the vsync ceiling) before trusting
/// "no headroom" as the reason occlusion culling didn't help here. What IS
/// solid: occlusion culling made no measurable difference in THIS scene,
/// matching EM-3.10's prediction that the smoke world's few/small occluders
/// wouldn't earn back the two-phase depth prepass + HZB cost. Ships **opt-in,
/// default OFF** (`XINDELER_OCCLUSION_CULLING=1` to try it; `GraphicsTier`
/// presets can wire a real toggle once `xindeler-app` picks this up, and a
/// denser scene — a real town/dungeon site — is the honest way to re-measure
/// this later).
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
        }
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
    if !grabbed || motion.delta == Vec2::ZERO {
        return;
    }
    for (mut transform, mut cam) in &mut cameras {
        cam.yaw -= motion.delta.x * cam.sensitivity;
        cam.pitch = (cam.pitch - motion.delta.y * cam.sensitivity).clamp(
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
