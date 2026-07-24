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

// BL-82 EM-3.11 round 21 introduced an angular throttle here (`0.0025` rad,
// ~0.14°) gating `day_night_stub`'s `Transform::rotation` writes,
// hypothesizing that committing a new sun rotation ~60×/s forced an
// expensive full CSM regen and that resampling self-shadow acne that often
// read as constant flicker. Round 25 removed it (see `sun_rotation_changed`
// below) after establishing both halves of that premise didn't hold up, and
// that the throttle itself caused a real, confirmed regression: the sun
// (and, since it drives the SAME `Transform` the CSM system reads, the
// shadows it casts) visibly moved in discrete ~0.14° steps every ~0.48s
// instead of sweeping smoothly — exactly what Matías reported.
//
// - `bevy_light::cascade::build_directional_light_cascades` (0.19.0) has NO
//   `Changed<Transform>` filter at all — it recomputes every directional
//   light's cascade frusta from the current `GlobalTransform` on literally
//   every `PostUpdate`, whether or not the light moved since last frame.
//   Throttling the *write* therefore never saved that computation; it only
//   changed how often the resulting numbers differed.
// - Round 24 later found the actual "constant flicker" symptom this gate was
//   introduced to fix was an unrelated bug (LOD-proxy z-fighting near the
//   camera, fixed via a near-band fragment discard), not per-frame CSM
//   recomputation or shadow acne.
//
// An angle-based epsilon (round 25's first attempt) turned out to be its own
// footgun: `Quat::angle_between` derives the angle via `acos`, which is
// ill-conditioned exactly where a no-op guard needs precision — near
// identical rotations (`dot ≈ 1`, where `acos`'s slope is near-vertical).
// f32 dot-product rounding noise on the order of `1e-7` gets amplified by
// that slope into an apparent angle on the order of `1e-3` rad — i.e. bigger
// than a whole frame's worth of real motion — so a small epsilon threshold
// spuriously reads bit-identical (paused) rotations as "changed". Comparing
// the `Quat`s for exact equality instead sidesteps `acos` entirely: two
// calls to the same pure `sun_rotation_for_hour` with the same `hour`
// produce bit-identical output, so `!=` correctly (and cheaply) detects only
// genuine motion — matching the pre-round-21 `paused` guarantee without the
// numerical landmine.

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
            // 500m across 4 cascades in Bevy's default 2048px shadow-map
            // texture (no DirectionalLightShadowMap override exists
            // anywhere in this codebase) put the far cascade's texels
            // several METRES wide — read as large, blocky, hard-edged dark
            // rectangles, not fine shadow-map acne. xindeler-old capped
            // shadow-casting distance at 96m with a single non-cascaded map
            // — it never needed to resolve shadows over hundreds of
            // metres. 150m keeps meaningful near-camera texel density
            // across all 4 cascades; distant terrain simply stops
            // receiving dynamic shadows past that range.
            maximum_distance: 150.0,
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

/// `true` iff `candidate` is not bit-identical to `current` — the no-op
/// guard `day_night_stub` uses to decide whether to commit a new
/// `Transform::rotation`. Only a genuinely unchanged rotation (the sun
/// paused) should ever read `false`; see this module's doc comment above
/// [`day_night_stub`]'s throttle constant for why exact equality (not an
/// angle-based epsilon) is the numerically sound choice here.
fn sun_rotation_changed(current: Quat, candidate: Quat) -> bool { current != candidate }

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
        // BL-82 EM-3.11 round 25: commit every frame the rotation actually
        // changed (no angular throttle — see the doc comment above this
        // function's guard for why round 21's throttle caused visible
        // stepping and didn't save what it was meant to save). Only skips
        // the write when bit-identical (the sun paused).
        if sun_rotation_changed(transform.rotation, rotation) {
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

    /// BL-82 EM-3.11 round 25 regression (the bug this round fixes): at the
    /// default day/night rate, a SINGLE rendered frame's worth of elapsed
    /// time (~1/60s) MUST register as changed — otherwise `day_night_stub`
    /// skips the write and the sun (and its shadows) visibly steps instead
    /// of sweeping smoothly, which is exactly what Matías reported. The
    /// pre-round-25 version of this test asserted the opposite (that one
    /// frame must NOT clear a much coarser 0.0025 rad throttle); that was
    /// the throttle causing the bug, not a guarantee worth preserving.
    #[test]
    fn one_frame_of_default_sun_speed_registers_as_changed() {
        let hour = 9.5;
        let dt = 1.0 / 60.0;
        let before = sun_rotation_for_hour(hour);
        let after = sun_rotation_for_hour(hour + dt * HOURS_PER_REAL_SECOND);
        assert!(
            sun_rotation_changed(before, after),
            "one frame at the default sun speed must register as changed, or the sun/shadows will \
             visibly step instead of sweeping smoothly"
        );
    }

    /// Even a very low framerate (~1 FPS — a full second between frames)
    /// must still register as changed: there is no minimum-delta floor left
    /// to clear, only exact equality, so this holds at any framerate.
    #[test]
    fn one_low_framerate_frame_still_registers_as_changed() {
        let hour = 9.5;
        let dt = 1.0; // ~1 FPS
        let before = sun_rotation_for_hour(hour);
        let after = sun_rotation_for_hour(hour + dt * HOURS_PER_REAL_SECOND);
        assert!(
            sun_rotation_changed(before, after),
            "even a 1 FPS frame must register as changed, or slow machines would still see \
             stepped sun/shadow motion"
        );
    }

    /// A fully paused sun (candidate == current every call, bit-identical
    /// since [`sun_rotation_for_hour`] is a pure function of `hour`) must
    /// never register as changed — the ONE case this guard exists to catch:
    /// skip a literal no-op `Transform` write (and the `GlobalTransform`
    /// rebuild `bevy_transform`'s `Changed<Transform>`-gated propagation
    /// would otherwise redo for nothing) while the sun is frozen.
    #[test]
    fn identical_rotation_never_registers_as_changed() {
        let rotation = sun_rotation_for_hour(12.0);
        assert!(!sun_rotation_changed(rotation, rotation));
    }

    /// The hour wrap (23.99.. -> 0.00..) must not look like a huge jump: the
    /// underlying rotation is periodic in the elevation angle, so a step
    /// straddling the wrap must read as the same tiny angle as any other
    /// same-sized step (checked directly against the angle, not the gate —
    /// with a near-zero epsilon the gate itself will correctly say "due" for
    /// any nonzero step, wrap or not).
    #[test]
    fn hour_wraparound_is_continuous_not_a_jump() {
        let just_before_midnight = sun_rotation_for_hour(23.999);
        let just_after_midnight = sun_rotation_for_hour(0.001);
        let angle = just_before_midnight.angle_between(just_after_midnight);
        assert!(
            angle < 0.01,
            "a tiny step straddling the 24h wrap must not read as a huge rotation jump, got \
             {angle} rad"
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
