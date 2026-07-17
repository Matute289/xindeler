//! Light rig (EM-2.3): cascaded-shadow sun with volumetric + contact
//! shadows, a standalone `Atmosphere` entity (0.19 API), and a day/night
//! sun-angle + illuminance stub driven by [`SunCycle`] until the real sim
//! time-of-day connects in EM-3.x. [`day_night_stub`] both rotates the sun
//! below the horizon at night AND scales its `illuminance` toward `0.0` as it
//! goes (`day_night_illuminance_factor`) — the rotation alone used to leave a
//! full-brightness directional light casting hard shadows through the night
//! (bug report, 2026-07-16/17).

use std::f32::consts::PI;

use bevy::{
    light::{
        Atmosphere, CascadeShadowConfigBuilder, VolumetricLight, atmosphere::ScatteringMedium,
    },
    prelude::*,
};
use xindeler_app::{GameplaySet, XindelerSettings};
use xindeler_oracle_host::{AtmosphereController, AtmosphereProfile};

pub struct LightRigPlugin;

impl Plugin for LightRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SunCycle>()
            .add_systems(Startup, spawn_light_rig)
            .add_systems(
                Update,
                (force_hour_for_smoke_capture, day_night_stub)
                    .chain()
                    .in_set(GameplaySet),
            );
    }
}

/// `XINDELER_SMOKE_HOUR` (dev/test tooling only — same env-var-gated
/// convention as `XINDELER_SMOKE_FORCE_TARGET` in `boss_nameplate.rs` and
/// `force_open_diary_for_smoke_capture` in `diary.rs`): when set to a valid
/// `f32`, pins [`SunCycle::hour`] to that value and pauses the cycle every
/// frame, so a `--smoke-screenshot` capture (or any live manual repro) can
/// force a specific, repeatable time of day — e.g. deep night — instead of
/// waiting on `day_night_stub`'s real-time drift. A no-op unless the env var
/// is set to a value that parses as a finite `f32`; never touches gameplay
/// otherwise.
fn force_hour_for_smoke_capture(mut cycle: ResMut<SunCycle>) {
    if let Ok(hour) = std::env::var("XINDELER_SMOKE_HOUR")
        && let Ok(hour) = hour.parse::<f32>()
        && hour.is_finite()
    {
        cycle.hour = hour.rem_euclid(24.0);
        cycle.paused = true;
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

/// The sun's elevation angle (radians) for a given hour-of-day (pure
/// function, unit-tested below): 6h -> 0 (sunrise, on the horizon), 12h ->
/// `PI/2` (zenith), 18h -> `PI` (sunset, back on the horizon); the remaining
/// half of the sweep (`PI..2*PI`, i.e. hours 18-24 and 0-6) is night, with the
/// sun below the horizon.
fn sun_elevation_for_hour(hour: f32) -> f32 { (hour - 6.0) / 12.0 * PI }

/// The sun's rotation for a given hour-of-day (pure function, unit-tested
/// below without needing a `Transform`/ECS world).
fn sun_rotation_for_hour(hour: f32) -> Quat {
    let elevation = sun_elevation_for_hour(hour);
    Quat::from_rotation_y(SUN_AZIMUTH) * Quat::from_rotation_x(-elevation)
}

/// Day/night direct-sunlight factor for a given sun elevation (radians):
/// `1.0` at zenith, tapering smoothly to `0.0` at the horizon on both sides,
/// and pinned to `0.0` for the whole below-horizon half of the sweep (night).
///
/// This is the piece that was MISSING (BL-82 bug report, 2026-07-16/17,
/// Matías: flora/terrain show a hard directional shadow "even at night," when
/// there's no sun to cast one). `sun_rotation_for_hour` already rotates the
/// light below the horizon at night, but nothing ever scaled
/// `DirectionalLight::illuminance` down to match — a live `--smoke-screenshot`
/// capture at `XINDELER_SMOKE_HOUR=0.0` (deep midnight) confirmed a full-
/// contrast, sharp-edged directional shadow still raking across the terrain,
/// with the sky/fog correctly reading as black night. `elevation.sin()` is
/// exactly the sun's height above the horizon (0 at the horizon, 1 at
/// zenith), and it goes negative for the whole night half of the sweep
/// (`elevation` in `(PI, 2*PI)`), so clamping it at `0.0` turns the
/// directional light fully off there — no separate "is it night" branch
/// needed, and no moonlight/stars invented (that's the still-blocked EM-7.6/
/// EM-7.7 content epics, out of scope for this bug fix).
fn day_night_illuminance_factor(elevation: f32) -> f32 { elevation.sin().max(0.0) }

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
    controller: Res<AtmosphereController>,
    mut suns: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
) {
    if !cycle.paused {
        cycle.hour = (cycle.hour + time.delta_secs() * HOURS_PER_REAL_SECOND).rem_euclid(24.0);
    }
    let elevation = sun_elevation_for_hour(cycle.hour);
    let rotation = sun_rotation_for_hour(cycle.hour);
    // The atmosphere profile's `sun_illuminance` is the DAYTIME peak (spec
    // §5.4); `apply_atmosphere` (atmosphere.rs) no longer writes
    // `DirectionalLight::illuminance` directly — this is the single writer,
    // so the day/night factor below can never be raced/overwritten by a
    // profile transition (see this function's — and
    // `day_night_illuminance_factor`'s — doc comments).
    let target_illuminance =
        controller.current.sun_illuminance * day_night_illuminance_factor(elevation);
    for (mut transform, mut light) in &mut suns {
        // BL-82 EM-3.11 round 21: throttled on a real angular delta (not bare
        // `!=`) — see `MIN_SUN_ROTATION_STEP_RADIANS`'s doc for why an exact
        // equality check still committed (and forced a full CSM regen) on
        // ~every frame despite looking like a no-op guard.
        if sun_rotation_step_is_due(transform.rotation, rotation) {
            transform.rotation = rotation;
        }
        // Unlike the rotation above, writing `illuminance` does not trigger a
        // cascaded-shadow-map regen (CSM frusta depend on the light's
        // transform + the camera, not its brightness) — it's safe/cheap to
        // update every frame; only the epsilon guard (avoid needlessly
        // dirtying the component when nothing changed) matters here.
        if (light.illuminance - target_illuminance).abs() > f32::EPSILON {
            light.illuminance = target_illuminance;
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

    /// Regression for the bug this phase fixes (2026-07-16/17: Matías —
    /// flora/terrain show a hard directional shadow "even at night"). Every
    /// hour in the below-horizon half of the sweep must produce EXACTLY zero
    /// direct sunlight — not just "dimmer" — or a full-brightness directional
    /// light keeps casting hard shadows through the night, live-confirmed via
    /// `--smoke-screenshot` at `XINDELER_SMOKE_HOUR=0.0` before this fix.
    #[test]
    fn night_hours_have_zero_illuminance_factor() {
        for &hour in &[18.0_f32, 19.0, 21.0, 23.0, 0.0, 1.0, 3.0, 5.999] {
            let elevation = sun_elevation_for_hour(hour);
            let factor = day_night_illuminance_factor(elevation);
            assert_eq!(
                factor, 0.0,
                "hour {hour} is past sunset/before sunrise (elevation {elevation}) and must have \
                 a zero day/night illuminance factor, got {factor}"
            );
        }
    }

    /// Noon (hour 12, zenith) must be at full strength — the factor must not
    /// ALSO dim daytime brightness, only remove the incorrect night-time
    /// shadow.
    #[test]
    fn noon_has_full_illuminance_factor() {
        let elevation = sun_elevation_for_hour(12.0);
        let factor = day_night_illuminance_factor(elevation);
        assert!(
            (factor - 1.0).abs() < 1e-6,
            "zenith (noon) must be at full strength (factor 1.0), got {factor}"
        );
    }

    /// Sunrise/sunset (hours 6 and 18, right on the horizon) must taper to
    /// zero continuously rather than popping — both boundary hours evaluate
    /// to exactly 0.0 (elevation 0 or PI), matching a smooth `sin` taper.
    #[test]
    fn sunrise_and_sunset_are_the_zero_crossing() {
        for &hour in &[6.0_f32, 18.0] {
            let elevation = sun_elevation_for_hour(hour);
            let factor = day_night_illuminance_factor(elevation);
            assert!(
                factor.abs() < 1e-6,
                "hour {hour} is exactly on the horizon and must read as the (continuous) zero \
                 crossing, got {factor}"
            );
        }
    }

    /// The factor must never go negative (it multiplies an illuminance, so a
    /// negative value would be nonsensical) even deep in the night half of
    /// the sweep where `sin(elevation)` itself is negative.
    #[test]
    fn illuminance_factor_never_negative() {
        let mut hour = 0.0_f32;
        while hour < 24.0 {
            let factor = day_night_illuminance_factor(sun_elevation_for_hour(hour));
            assert!(
                factor >= 0.0,
                "hour {hour} produced a negative illuminance factor {factor}"
            );
            hour += 0.25;
        }
    }
}
