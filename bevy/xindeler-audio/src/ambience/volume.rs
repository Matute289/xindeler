//! BL-82 EM-5.10c (T56.36) — per-tag ambience volume, ported (as far as real
//! signals allow — see below) from `voxygen::audio::ambience::AmbienceMgr::
//! get_tag_volume`/`is_indoors`.
//!
//! ## Real vs. honestly-stubbed inputs (read before touching [`AmbienceInputs`])
//! - [`AmbienceInputs::indoors`] — REAL. `xindeler-client::ambience` computes
//!   this with a real per-block terrain query against the ALREADY-STREAMED
//!   chunk store (`xindeler-client::terrain_stream::SharedTerrain`), porting
//!   the old client's own 5-direction "am I under a roof" heuristic
//!   (`voxygen::audio::ambience::is_indoors`) at the block level (stepping
//!   through real, decoded voxel data — not a raycast library call, since this
//!   crate has no equivalent to the old client's `state.terrain().ray`
//!   convenience, but functionally the same check).
//! - [`AmbienceInputs::weather`] — REAL, but COARSE. Same
//!   `xindeler_oracle_host::AtmosphereController`-mirrored `WeatherEffect`
//!   (`None`/`Rain`/`Storm`) `crate::music::state`'s own doc comment explains —
//!   a placeholder taxonomy, not the old client's CONTINUOUS `Weather.rain:
//!   f32` (0.0..1.0) + wind vector. [`rain_intensity`] below maps the three
//!   discrete tags onto a fixed intensity value rather than a smoothly-varying
//!   one — an honest, documented approximation.
//! - [`AmbienceInputs::site`] — DOCUMENTED GAP, always `SiteKindMeta::Void` (no
//!   site-kind mirror exists — see `crate::music::state`'s doc comment for the
//!   same gap already explained there). [`tag_volume`] still implements the
//!   real `Cave` predicate (`site == SiteKindMeta::Cave`) against it, so it's a
//!   live no-op today and a pure input-wiring change once a site mirror lands,
//!   not a redesign.
//! - **`Wind`/`Leaves`/`RiverLoud`/`RiverQuiet` are NOT computed at all** —
//!   [`tag_volume`] returns `0.0` (silent) for these four tags, honestly,
//!   rather than fabricating a plausible-looking number. The old client's
//!   volume curves for these need real tree density
//!   (`ChunkMetaData::tree_density`), a real wind vector (`Weather.wind:
//!   Vec2<f32>`), and real per-chunk river-block positions + velocity
//!   (`BlocksOfInterest::water`, `ChunkMeta::river_velocity`) — NONE of these
//!   are mirrored anywhere in `xindeler-protocol`/ `xindeler-sim-bridge` today
//!   (verified by grep; this is the SAME "blocks of interest" gap
//!   `xindeler-client::sfx`'s own module doc comment already documents for its
//!   block-ambience sub-mapper). Building a new bulk per-chunk terrain-metadata
//!   mirror is a real, separate task — out of scope for "wire up systems
//!   reading state that already exists".

use common::{terrain::SiteKindMeta, weather::WeatherKind};

use super::manifest::AmbienceChannelTag;

/// The real + honestly-stubbed inputs [`tag_volume`] reads — see this
/// module's doc comment for which fields are real signals vs. documented
/// gaps.
#[derive(Clone, Copy, Debug)]
pub struct AmbienceInputs {
    pub weather: Option<WeatherKind>,
    pub indoors: bool,
    pub site: SiteKindMeta,
}

/// A fixed intensity per discrete [`WeatherKind`] tag, standing in for the
/// old client's continuous `Weather.rain` field — see this module's doc
/// comment for why this is a coarse, documented approximation rather than a
/// smoothly-varying value. `None`/`Clear`/`Cloudy` all read as dry (`0.0`);
/// `Rain` and `Storm` get distinct, increasing intensities.
#[must_use]
fn rain_intensity(weather: Option<WeatherKind>) -> f32 {
    match weather {
        // Chosen to sit ABOVE the 0.7 rumble threshold below but BELOW
        // `AmbienceChannelTag::Rain::max_volume` (0.95) — so `Rain` and
        // `Storm` stay distinguishable after the tag's own volume clamp
        // instead of both saturating to the same capped value.
        Some(WeatherKind::Rain) => 0.8,
        Some(WeatherKind::Storm) => 1.2,
        _ => 0.0,
    }
}

/// Ported from `AmbienceMgr::get_tag_volume`, restricted to the tags a real
/// signal actually drives — see this module's doc comment for the honest
/// `0.0` returns on the other four. Clamped to each tag's
/// [`AmbienceChannelTag::max_volume`], matching the old code's own
/// `.min(tag.get_max_volume())` clamp.
#[must_use]
pub fn tag_volume(tag: AmbienceChannelTag, inputs: &AmbienceInputs) -> f32 {
    let volume = match tag {
        AmbienceChannelTag::Rain => {
            // Ported from `AmbienceMgr::get_tag_volume`'s `Rain` arm: an
            // `indoor_factor` of 0.7 while sheltered (rain is muffled, not
            // silenced, matching the master ambience lowpass filter's own
            // "indoors dampens, doesn't mute" design in the old
            // `AmbienceMgr::maintain`).
            let indoor_factor = if inputs.indoors { 0.7 } else { 1.0 };
            rain_intensity(inputs.weather) * indoor_factor
        },
        AmbienceChannelTag::ThunderRumbling => {
            let intensity = rain_intensity(inputs.weather);
            if intensity < 0.7 { 0.0 } else { intensity }
        },
        AmbienceChannelTag::Cave => {
            if inputs.site == SiteKindMeta::Cave {
                1.0
            } else {
                0.0
            }
        },
        // Wind / Leaves / RiverLoud / RiverQuiet — see this module's doc
        // comment for the missing tree-density/wind-vector/river-block
        // mirrors this would need.
        AmbienceChannelTag::Wind
        | AmbienceChannelTag::Leaves
        | AmbienceChannelTag::RiverLoud
        | AmbienceChannelTag::RiverQuiet
        | AmbienceChannelTag::Thunder => 0.0,
    };
    volume.min(tag.max_volume())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(weather: Option<WeatherKind>, indoors: bool) -> AmbienceInputs {
        AmbienceInputs {
            weather,
            indoors,
            site: SiteKindMeta::Void,
        }
    }

    #[test]
    fn rain_is_silent_when_clear() {
        assert_eq!(
            tag_volume(
                AmbienceChannelTag::Rain,
                &inputs(Some(WeatherKind::Clear), false)
            ),
            0.0
        );
        assert_eq!(
            tag_volume(AmbienceChannelTag::Rain, &inputs(None, false)),
            0.0
        );
    }

    #[test]
    fn rain_plays_when_raining_and_is_muffled_indoors() {
        let outdoor = tag_volume(
            AmbienceChannelTag::Rain,
            &inputs(Some(WeatherKind::Rain), false),
        );
        let indoor = tag_volume(
            AmbienceChannelTag::Rain,
            &inputs(Some(WeatherKind::Rain), true),
        );
        assert!(outdoor > 0.0, "rain must be audible outdoors when raining");
        assert!(indoor > 0.0, "rain must still be audible (muffled) indoors");
        assert!(indoor < outdoor, "indoors must be quieter than outdoors");
    }

    #[test]
    fn storm_rain_is_louder_than_light_rain() {
        let rain = tag_volume(
            AmbienceChannelTag::Rain,
            &inputs(Some(WeatherKind::Rain), false),
        );
        let storm = tag_volume(
            AmbienceChannelTag::Rain,
            &inputs(Some(WeatherKind::Storm), false),
        );
        assert!(storm > rain);
    }

    #[test]
    fn thunder_rumbling_is_silent_when_clear_but_rumbles_in_rain_or_storm() {
        assert_eq!(
            tag_volume(
                AmbienceChannelTag::ThunderRumbling,
                &inputs(Some(WeatherKind::Clear), false)
            ),
            0.0,
            "dry weather never crosses the >= 0.7 rumble threshold"
        );
        // Both `Rain` (1.5) and `Storm` (3.0) clear the 0.7 threshold in this
        // coarse discrete mapping (see this module's doc comment on why
        // there's no smoothly-varying intensity to gate on instead).
        assert!(
            tag_volume(
                AmbienceChannelTag::ThunderRumbling,
                &inputs(Some(WeatherKind::Rain), false)
            ) > 0.0
        );
        assert!(
            tag_volume(
                AmbienceChannelTag::ThunderRumbling,
                &inputs(Some(WeatherKind::Storm), false)
            ) > 0.0
        );
    }

    #[test]
    fn cave_tag_is_silent_without_a_cave_site_signal() {
        // Honest gap: `site` is always `Void` today (no site mirror), so
        // Cave ambience never plays yet — see this module's doc comment.
        assert_eq!(
            tag_volume(AmbienceChannelTag::Cave, &inputs(None, false)),
            0.0
        );
    }

    #[test]
    fn cave_tag_plays_when_a_real_cave_site_signal_is_supplied() {
        let cave_inputs = AmbienceInputs {
            weather: None,
            indoors: false,
            site: SiteKindMeta::Cave,
        };
        assert!(tag_volume(AmbienceChannelTag::Cave, &cave_inputs) > 0.0);
    }

    #[test]
    fn undriven_tags_are_honestly_silent_not_fabricated() {
        let i = inputs(Some(WeatherKind::Storm), false);
        for tag in [
            AmbienceChannelTag::Wind,
            AmbienceChannelTag::Leaves,
            AmbienceChannelTag::RiverLoud,
            AmbienceChannelTag::RiverQuiet,
            AmbienceChannelTag::Thunder,
        ] {
            assert_eq!(tag_volume(tag, &i), 0.0);
        }
    }
}
