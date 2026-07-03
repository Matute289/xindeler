//! Light rig (EM-2.3): cascaded-shadow sun with volumetric + contact
//! shadows, a standalone `Atmosphere` entity (0.19 API), and a day/night
//! sun-angle stub driven by [`SunCycle`] until the real sim time-of-day
//! connects in EM-3.x.

use std::f32::consts::PI;

use bevy::{
    light::{
        Atmosphere, CascadeShadowConfigBuilder, VolumetricLight, atmosphere::ScatteringMedium,
        light_consts::lux,
    },
    prelude::*,
};
use xindeler_app::{GameplaySet, XindelerSettings};

pub struct LightRigPlugin;

impl Plugin for LightRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SunCycle>()
            .add_systems(Startup, spawn_light_rig)
            .add_systems(Update, day_night_stub.in_set(GameplaySet));
    }
}

/// Stub day/night clock (EM-2.3). Replaced by the mirrored sim time-of-day
/// in EM-3.x; until then it advances slowly on real time.
#[derive(Resource, Debug, Clone)]
pub struct SunCycle {
    /// Freeze the sun (useful for screenshots / debugging).
    pub paused: bool,
    /// Time of day in hours, `0.0..24.0`. 6.0 = sunrise, 12.0 = noon.
    pub hour: f32,
}

impl Default for SunCycle {
    fn default() -> Self {
        Self {
            paused: false,
            // Mid-morning: long-ish shadows, sun well above the horizon.
            hour: 9.5,
        }
    }
}

/// Game-hours advanced per real second (full day ~20 real minutes).
const HOURS_PER_REAL_SECOND: f32 = 24.0 / (20.0 * 60.0);
/// Fixed sun azimuth, radians (aesthetic pick for the demo scene).
const SUN_AZIMUTH: f32 = 0.7;

/// Marker for the sun light so [`day_night_stub`] can find it.
#[derive(Component)]
pub struct Sun;

fn spawn_light_rig(
    mut commands: Commands,
    settings: Res<XindelerSettings>,
    mut scattering_mediums: ResMut<Assets<ScatteringMedium>>,
) {
    let graphics = &settings.graphics;

    commands.spawn((
        Sun,
        DirectionalLight {
            // Physical sunlight; the camera compensates with Exposure (EM-2.2).
            illuminance: lux::RAW_SUNLIGHT,
            shadow_maps_enabled: true,
            // Per-light half of contact shadows; the camera carries the
            // `ContactShadows` component (bevy_pbr::contact_shadows, 0.19).
            contact_shadows_enabled: graphics.contact_shadows,
            ..Default::default()
        },
        CascadeShadowConfigBuilder {
            // Clamp to the range the renderer meaningfully supports — a user-edited
            // settings.ron with e.g. 255 would allocate 255 cascade frusta (reviewer m2).
            num_cascades: usize::from(graphics.shadow_cascades.clamp(1, 4)),
            maximum_distance: 500.0,
            ..Default::default()
        }
        .build(),
        // Lets the fog raymarch this light -> god rays through the FogVolume.
        VolumetricLight,
        // Rotation is driven every frame by `day_night_stub`.
        Transform::IDENTITY,
    ));

    // 0.19: `Atmosphere` is a standalone entity (planet), not a camera
    // component; cameras opt in via `AtmosphereSettings` (see camera.rs).
    let earth_medium = scattering_mediums.add(ScatteringMedium::earth(256, 256));
    commands.spawn(Atmosphere::earth(earth_medium));
}

fn day_night_stub(
    time: Res<Time>,
    mut cycle: ResMut<SunCycle>,
    mut suns: Query<&mut Transform, With<Sun>>,
) {
    if !cycle.paused {
        cycle.hour = (cycle.hour + time.delta_secs() * HOURS_PER_REAL_SECOND).rem_euclid(24.0);
    }
    // hour 6 -> elevation 0 (sunrise), 12 -> PI/2 (zenith), 18 -> PI (sunset);
    // night hours put the sun below the horizon.
    let elevation = (cycle.hour - 6.0) / 12.0 * PI;
    let rotation = Quat::from_rotation_y(SUN_AZIMUTH) * Quat::from_rotation_x(-elevation);
    for mut transform in &mut suns {
        // set_if_neq semantics: writing through Mut every frame would dirty the
        // light's change tick and force cascade recompute even with a paused sun.
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}
