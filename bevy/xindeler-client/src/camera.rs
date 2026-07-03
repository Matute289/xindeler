//! Camera rig (EM-2.2): HDR `Camera3d` with TAA/SSAO/bloom/volumetric +
//! distance fog, driven by [`GraphicsSettings`], plus a simple fly-cam
//! (WASD + mouse-look, Shift = fast, click to grab cursor / Escape to
//! release).

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    camera::{Exposure, Hdr},
    input::mouse::AccumulatedMouseMotion,
    light::VolumetricFog,
    pbr::{AtmosphereSettings, ContactShadows, ScreenSpaceAmbientOcclusion},
    post_process::bloom::Bloom,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use xindeler_app::{GameplaySet, XindelerSettings};

pub struct CameraRigPlugin;

impl Plugin for CameraRigPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera).add_systems(
            Update,
            (cursor_grab, fly_cam_look, fly_cam_move)
                .chain()
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

fn spawn_camera(mut commands: Commands, settings: Res<XindelerSettings>) {
    let graphics = &settings.graphics;

    let transform =
        Transform::from_xyz(-22.0, 9.0, 26.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y);
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
        // Placeholder colors until EM-2.4's AtmosphereController drives this.
        DistanceFog {
            color: Color::srgb(0.55, 0.65, 0.75),
            directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.5),
            directional_light_exponent: 30.0,
            falloff: FogFalloff::from_visibility_colors(
                350.0,
                Color::srgb(0.35, 0.5, 0.66),
                Color::srgb(0.8, 0.844, 1.0),
            ),
        },
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
        camera.insert(VolumetricFog {
            ambient_intensity: 0.1,
            ..Default::default()
        });
    }
    if graphics.contact_shadows {
        camera.insert(ContactShadows::default());
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
    mut cameras: Query<(&mut Transform, &FlyCam)>,
) {
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
