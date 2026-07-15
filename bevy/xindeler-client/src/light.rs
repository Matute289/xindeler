//! Light rig (EM-2.3): cascaded-shadow sun with volumetric + contact
//! shadows, a standalone `Atmosphere` entity (0.19 API), and a day/night
//! sun-angle stub driven by [`SunCycle`] until the real sim time-of-day
//! connects in EM-3.x.

use std::f32::consts::PI;

use bevy::{
    light::{
        Atmosphere, CascadeShadowConfigBuilder, VolumetricLight, atmosphere::ScatteringMedium,
    },
    prelude::*,
};
use xindeler_app::{GameplaySet, XindelerSettings};
use xindeler_oracle_host::AtmosphereProfile;

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

/// Minimum angle (radians) the sun must actually have moved since the last
/// COMMITTED `Transform::rotation` before [`day_night_stub`] writes a new one
/// (BL-82 EM-3.11 round 21 — see `docs/backlog/engine-migration.md`'s EM-3.11
/// row for the live-captured before/after evidence).
///
/// At the default day/night rate the un-paused sun's angle changes by a tiny
/// but NON-ZERO amount on literally every rendered frame. The old exact
/// (`!=`) equality check therefore committed a new rotation ~60×/s, and
/// Bevy's cascaded-shadow-map frusta are fully rebuilt from the light's
/// current rotation every time it changes (`bevy_light::cascade::
/// build_directional_light_cascades` has no dirty/throttle gate of its own).
/// Self-shadow acne is a DISCONTINUOUS function of the light angle (a
/// per-fragment depth-compare that flips a binary lit/shadowed decision), so
/// resampling it 60×/s for an input that is itself changing smoothly still
/// produces a genuinely different acne pattern almost every frame — which
/// reads as constant flicker rather than the intended gradual shadow drift,
/// on ANY greedy-meshed voxel geometry (large flat quads split into 2
/// triangles) lit near a grazing angle — indoors (walls/floors/furniture)
/// exactly as much as outdoors (terrain), since it is a property of the ONE
/// shared directional light, not of any specific material.
///
/// `0.0025` rad (~0.14°) lets a full day still visibly complete in the usual
/// ~20 real minutes while only actually committing (and therefore
/// regenerating shadows) a few times a second — long enough for a human eye
/// (and TAA's own temporal accumulation) to read the acne pattern as settled
/// between jumps. Confirmed empirically: an offscreen burst-capture A/B (sun
/// rotating vs. frozen, camera+NPCs otherwise identical) measured the
/// biggest per-frame pixel jumps (p99.9) drop ~5× once the sun stops
/// recomputing every single frame.
const MIN_SUN_ROTATION_STEP_RADIANS: f32 = 0.0025;

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
            // Physical sunlight (RAW_SUNLIGHT lux via the default atmosphere
            // profile — runtime-driven by EM-2.4's AtmosphereController); the
            // camera compensates with Exposure (EM-2.2).
            illuminance: AtmosphereProfile::default().sun_illuminance,
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

/// The sun's rotation for a given hour-of-day (pure function, unit-tested
/// below without needing a `Transform`/ECS world).
fn sun_rotation_for_hour(hour: f32) -> Quat {
    // hour 6 -> elevation 0 (sunrise), 12 -> PI/2 (zenith), 18 -> PI (sunset);
    // night hours put the sun below the horizon.
    let elevation = (hour - 6.0) / 12.0 * PI;
    Quat::from_rotation_y(SUN_AZIMUTH) * Quat::from_rotation_x(-elevation)
}

/// `true` iff `candidate` differs from `current` by at least
/// [`MIN_SUN_ROTATION_STEP_RADIANS`] — the throttle gate `day_night_stub`
/// uses to decide whether to commit a new `Transform::rotation` (and
/// therefore let Bevy regenerate the cascaded shadow map).
fn sun_rotation_step_is_due(current: Quat, candidate: Quat) -> bool {
    current.angle_between(candidate) >= MIN_SUN_ROTATION_STEP_RADIANS
}

fn day_night_stub(
    time: Res<Time>,
    mut cycle: ResMut<SunCycle>,
    mut suns: Query<&mut Transform, With<Sun>>,
) {
    if !cycle.paused {
        cycle.hour = (cycle.hour + time.delta_secs() * HOURS_PER_REAL_SECOND).rem_euclid(24.0);
    }
    let rotation = sun_rotation_for_hour(cycle.hour);
    for mut transform in &mut suns {
        // BL-82 EM-3.11 round 21: throttled on a real angular delta (not bare
        // `!=`) — see `MIN_SUN_ROTATION_STEP_RADIANS`'s doc for why an exact
        // equality check still committed (and forced a full CSM regen) on
        // ~every frame despite looking like a no-op guard.
        if sun_rotation_step_is_due(transform.rotation, rotation) {
            transform.rotation = rotation;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BL-82 EM-3.11 round 21 regression: at the default day/night rate, a
    /// SINGLE rendered frame's worth of elapsed time (~1/60s) must NOT clear
    /// the commit threshold — otherwise the throttle is a no-op and the
    /// pre-round-21 every-frame-CSM-regen flicker silently comes back.
    #[test]
    fn one_frame_of_default_sun_speed_does_not_clear_the_threshold() {
        let hour = 9.5;
        let dt = 1.0 / 60.0;
        let before = sun_rotation_for_hour(hour);
        let after = sun_rotation_for_hour(hour + dt * HOURS_PER_REAL_SECOND);
        assert!(
            !sun_rotation_step_is_due(before, after),
            "one frame at the default sun speed must stay below MIN_SUN_ROTATION_STEP_RADIANS, or \
             the throttle does nothing"
        );
    }

    /// Once ENOUGH real time has elapsed at the default rate for the angle to
    /// cross the threshold, the step must actually fire — the throttle must
    /// not silently freeze the sun forever.
    #[test]
    fn enough_elapsed_time_eventually_clears_the_threshold() {
        let hour = 9.5;
        let before = sun_rotation_for_hour(hour);
        // A few seconds at the default rate is comfortably past the ~0.14°
        // threshold (full day = 20 real minutes => several degrees/second is
        // nowhere near the rate; a few seconds' worth of drift already
        // exceeds 0.14°, see the module doc's rate derivation).
        let after = sun_rotation_for_hour((hour + 5.0 * HOURS_PER_REAL_SECOND).rem_euclid(24.0));
        assert!(
            sun_rotation_step_is_due(before, after),
            "several seconds of default-rate drift must clear the threshold, or the sun would \
             never visibly move"
        );
    }

    /// A fully paused sun (candidate == current every call) must never fire
    /// — matches the pre-round-21 `paused` guarantee (no cascade regen while
    /// frozen), just expressed via the angle check instead of bare `!=`.
    #[test]
    fn identical_rotation_never_clears_the_threshold() {
        let rotation = sun_rotation_for_hour(12.0);
        assert!(!sun_rotation_step_is_due(rotation, rotation));
    }

    /// The hour wrap (23.99.. -> 0.00..) must not look like a huge jump: the
    /// underlying rotation is periodic in the elevation angle, so a step
    /// straddling the wrap should read the same as any other same-sized step.
    #[test]
    fn hour_wraparound_is_continuous_not_a_jump() {
        let just_before_midnight = sun_rotation_for_hour(23.999);
        let just_after_midnight = sun_rotation_for_hour(0.001);
        assert!(
            !sun_rotation_step_is_due(just_before_midnight, just_after_midnight),
            "a tiny step straddling the 24h wrap must not read as a huge rotation jump"
        );
    }
}
