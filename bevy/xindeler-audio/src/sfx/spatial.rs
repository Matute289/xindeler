//! BL-82 EM-5.10d (T56.36b) — the spatial-audio math + listener state that
//! turns a flat, binary-culled SFX trigger into a positional one: real
//! distance attenuation (so a sound gets quieter as you walk away instead of
//! blaring at constant full volume — the reported "campfire" bug) and stereo
//! panning (so a sound off to your right is heard on your right).
//!
//! ## Why bake volume/pan into the sound instead of a Kira spatial track
//! Kira 0.12 *does* offer native `SpatialTrackBuilder` tracks + a listener that
//! compute attenuation/panning on the audio thread (the old client used them).
//! We instead compute both HERE and bake them into the played
//! [`kira::sound::static_sound::StaticSoundData`] via its own `.volume()` /
//! `.panning()` builders, playing on the ONE shared `sfx` track. Two reasons:
//! 1. **Testability.** Kira's spatial math is `pub(crate)` — neither the
//!    computed per-sound volume nor the rendered output frames are observable
//!    through any public handle (the `MockBackend` renders into a private
//!    buffer with no getter), so a native spatial track's attenuation/pan can
//!    only be asserted as opaque plumbing. Baking it makes both a pure,
//!    deterministic function of `(listener, emitter)` — genuinely in the signal
//!    path AND unit-testable (see this module's tests + the audio-crate/client
//!    integration tests).
//! 2. **Cost.** A native spatial track must be allocated per emitted sound (the
//!    emitter position is fixed at construction), which is exactly why the old
//!    client kept a bounded *pool* of pre-built channels. Baking needs no
//!    per-sound sub-track — a plain `sfx.play(sound)` on the existing track.
//!
//! The one thing that genuinely needs a Kira effect on the audio thread — the
//! underwater low-pass muffle — stays a `FilterHandle` on the `sfx` track
//! (`crate::manager`), since a per-sample filter cannot be baked at emit time.

use bevy::{ecs::resource::Resource, math::Vec3};

use super::SFX_DIST_LIMIT;

/// The distance (m) within which a positional sound plays at full volume — the
/// near end of the `(1.0, SFX_DIST_LIMIT)` range the old client handed Kira's
/// `SpatialTrackBuilder::distances`.
pub const SFX_MIN_DISTANCE: f32 = 1.0;

/// The current audio listener: the local player's camera world position, the
/// camera's right-ear direction (for left/right panning), and whether the
/// listener is underwater (for the low-pass muffle). Written every frame by the
/// client from the `MainCamera` transform + the local player's in-liquid state
/// (`xindeler_client::sfx::update_audio_listener`); read by
/// [`super::trigger_sfx`] to bake attenuation + panning into every positional
/// sound, and by the muffle system to drive the sfx-track filter.
///
/// Held as plain glam data so this crate stays free of any camera/client
/// dependency (the engine-isolation law): the CLIENT owns where the camera is;
/// this crate only consumes the resulting geometry.
#[derive(Resource, Debug, Clone, Copy)]
pub struct AudioListener {
    /// Listener position in Bevy world space (y-up).
    pub pos: Vec3,
    /// The listener's unit "right" direction in Bevy world space — the camera's
    /// local +X. Used to decide how far left/right of the listener an emitter
    /// sits. Defaults to [`Vec3::X`].
    pub right: Vec3,
    /// Whether the listener is currently underwater (drives the sfx low-pass
    /// muffle). Defaults to `false`.
    pub underwater: bool,
}

impl Default for AudioListener {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            right: Vec3::X,
            underwater: false,
        }
    }
}

/// Distance attenuation multiplier in `0.0..=1.0` for an emitter `distance`
/// metres from the listener.
///
/// Reproduces the EXACT curve the old client configured on its Kira spatial
/// track — `Easing::OutPowf(0.66)` over `distances((SFX_MIN_DISTANCE,
/// SFX_DIST_LIMIT))`. Kira's `OutPowf(p)` applied to `(1 - relative_distance)`
/// expands to the closed form `1 - relative_distance^p` (see `kira::Easing`),
/// where `relative_distance` linearly maps the clamped distance onto `0..1`
/// across the `[SFX_MIN_DISTANCE, SFX_DIST_LIMIT]` span. Result:
/// - full volume (`1.0`) within [`SFX_MIN_DISTANCE`],
/// - a smooth taper across the audible range,
/// - true silence (`0.0`) at/beyond [`SFX_DIST_LIMIT`].
///
/// This is the function that fixes the "campfire SFX blares at constant full
/// volume no matter how far I walk away" report: the pre-EM-5.10d path only
/// hard-culled at [`SFX_DIST_LIMIT`] and otherwise played every sound flat, so
/// a still-in-range campfire was as loud at 200 m as at 2 m.
#[must_use]
pub fn distance_attenuation(distance: f32) -> f32 {
    if distance <= SFX_MIN_DISTANCE {
        return 1.0;
    }
    if distance >= SFX_DIST_LIMIT {
        return 0.0;
    }
    let relative = (distance - SFX_MIN_DISTANCE) / (SFX_DIST_LIMIT - SFX_MIN_DISTANCE);
    1.0 - relative.powf(0.66)
}

/// Stereo pan in `-1.0..=1.0` (Kira's `Panning`: `-1.0` = hard left, `0.0` =
/// centre, `+1.0` = hard right) for an emitter at `emitter_pos` heard by a
/// listener at `listener_pos` whose right-ear direction is `listener_right`.
///
/// Projects the (normalized) listener→emitter direction onto the listener's
/// right axis — the same geometry Kira's own spatial-track ear model uses: an
/// emitter off to the right pans right, off to the left pans left, and one dead
/// ahead or behind sits centre. A coincident emitter (zero direction) is
/// centred rather than producing a NaN.
#[must_use]
pub fn stereo_pan(listener_pos: Vec3, listener_right: Vec3, emitter_pos: Vec3) -> f32 {
    let to_emitter = (emitter_pos - listener_pos).normalize_or_zero();
    listener_right
        .normalize_or_zero()
        .dot(to_emitter)
        .clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attenuation_is_full_at_the_listener_and_within_min_distance() {
        assert_eq!(distance_attenuation(0.0), 1.0);
        assert_eq!(distance_attenuation(SFX_MIN_DISTANCE), 1.0);
        assert_eq!(distance_attenuation(SFX_MIN_DISTANCE * 0.5), 1.0);
    }

    #[test]
    fn attenuation_is_silent_at_and_beyond_the_limit() {
        assert_eq!(distance_attenuation(SFX_DIST_LIMIT), 0.0);
        assert_eq!(distance_attenuation(SFX_DIST_LIMIT + 100.0), 0.0);
    }

    #[test]
    fn attenuation_falls_off_monotonically_with_distance() {
        // Closer is strictly louder than farther, all the way across the range.
        let mut prev = distance_attenuation(SFX_MIN_DISTANCE);
        for d in 2..=256 {
            let cur = distance_attenuation(d as f32);
            assert!(
                cur <= prev,
                "attenuation must not increase with distance: d={d} cur={cur} prev={prev}"
            );
            prev = cur;
        }
    }

    /// The reported bug, as a regression: a campfire heard from far away (but
    /// still inside the cull radius) must be *noticeably* quieter than one
    /// heard right next to you — not the identical full volume it used to be.
    #[test]
    fn far_but_in_range_campfire_is_much_quieter_than_a_near_one() {
        let near = distance_attenuation(2.0);
        let far = distance_attenuation(200.0);
        assert!(
            near > 0.9,
            "a campfire 2 m away should be near full volume: {near}"
        );
        assert!(
            far < near * 0.5,
            "a campfire 200 m away must be at most half as loud as one 2 m away (was equal, \
             pre-fix): near={near} far={far}"
        );
    }

    #[test]
    fn pan_is_positive_to_the_right_and_negative_to_the_left() {
        let listener = Vec3::ZERO;
        let right = Vec3::X;
        // Emitter directly to the listener's right (+X) pans hard right.
        assert!(stereo_pan(listener, right, Vec3::new(10.0, 0.0, 0.0)) > 0.9);
        // Emitter to the listener's left (-X) pans hard left.
        assert!(stereo_pan(listener, right, Vec3::new(-10.0, 0.0, 0.0)) < -0.9);
    }

    #[test]
    fn pan_is_centred_for_an_emitter_dead_ahead_or_coincident() {
        let right = Vec3::X;
        // Straight ahead (-Z, orthogonal to the right axis) → centred.
        assert!(stereo_pan(Vec3::ZERO, right, Vec3::new(0.0, 0.0, -10.0)).abs() < 1e-6);
        // Exactly on top of the listener → centred (no NaN from a zero dir).
        assert_eq!(stereo_pan(Vec3::ZERO, right, Vec3::ZERO), 0.0);
    }

    #[test]
    fn pan_tracks_the_listeners_own_right_axis() {
        // With the listener turned so its right axis points down +Z, an emitter
        // at +Z now pans right (not the world +X emitter).
        let right = Vec3::Z;
        assert!(stereo_pan(Vec3::ZERO, right, Vec3::new(0.0, 0.0, 10.0)) > 0.9);
        assert!(stereo_pan(Vec3::ZERO, right, Vec3::new(10.0, 0.0, 0.0)).abs() < 1e-6);
    }
}
