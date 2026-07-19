//! BL-82 EM-5.10c (T56.36) — the music vocabulary + the Bevy `AssetLoader`s
//! for the two FROZEN manifests (isolation law rule 3: never renamed/
//! restructured, read verbatim): `assets/voxygen/audio/soundtrack.ron` and
//! `assets/voxygen/audio/music_transition_manifest.ron`.
//!
//! Ported 1:1 from `voxygen/src/audio/music.rs`'s `SoundtrackItem`/
//! `RawSoundtrackItem`/`MusicState`/`MusicActivity`/`CombatIntensity`/
//! `DayPeriod`/`MusicTransitionManifest` (voxygen is no longer a workspace
//! member — CLAUDE.md — so this is a genuine re-implementation, not a `use
//! voxygen::...` reference), and `voxygen/src/audio/channel.rs`'s
//! `MusicChannelTag`. Every variant/field is kept even where this phase's
//! client-side inputs can't drive it yet (e.g. `WeatherKind::Cloudy` is never
//! produced by [`crate::music::state`]'s `WeatherEffect` mapping today — see
//! that module's doc comment) — the frozen manifest must still parse in
//! full.
//!
//! ## What's NOT here (documented, deliberate)
//! The old client's calendar-event soundtrack overrides
//! (`assets/voxygen/audio/calendar/{halloween,christmas}/soundtrack.ron`,
//! gated on `common::calendar::Calendar`) are NOT loaded here. Verified: no
//! `Calendar`/`CalendarEvent` mirror exists anywhere in `xindeler-protocol`/
//! `xindeler-sim-bridge` today (this Bevy client has no calendar/event system
//! at all yet) — this is an honest, explicitly out-of-scope gap, not a
//! silent narrowing. The base `voxygen.audio.soundtrack` manifest (used
//! whenever no calendar event is active — i.e. always, for this client) is
//! the ONLY soundtrack loaded.

use std::collections::HashMap;

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    ecs::error::BevyError,
    reflect::TypePath,
};
use common::{
    terrain::{BiomeKind, SiteKindMeta},
    weather::WeatherKind,
};
use serde::Deserialize;

/// Ported verbatim from `voxygen::audio::channel::MusicChannelTag` — the
/// three Kira sub-channels the old client routed exploration/combat/menu
/// music through. This phase's client-side state machine only ever selects
/// [`Self::Exploration`]/[`Self::Combat`] (there is no title-menu music state
/// in gameplay); `TitleMusic` is kept because `music_transition_manifest.ron`
/// names it in `fade_timings` keys and the manifest must parse in full.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
pub enum MusicChannelTag {
    TitleMusic,
    Exploration,
    Combat,
}

/// Ported verbatim from `voxygen::audio::music::DayPeriod`. Unlike the RON's
/// `Option<DayPeriod>` (`None` = "either period"), this type itself has only
/// the two real periods — see [`crate::music::state::day_period_for_hour`]
/// for how the client resolves an actual period from the (client-local, not
/// yet sim-mirrored — see that function's own doc comment) day/night clock.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum DayPeriod {
    /// 8:00 AM to 7:30 PM (old client's comment; the ported
    /// `day_period_for_hour` approximation uses 6:00-18:00, matching the sun
    /// rig's own day/night boundary — see that function's doc comment).
    Day,
    /// 7:31 PM to 6:59 AM.
    Night,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum CombatIntensity {
    Low,
    High,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum MusicActivity {
    Explore,
    Combat(CombatIntensity),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum MusicState {
    Activity(MusicActivity),
    Transition(MusicActivity, MusicActivity),
}

/// One `soundtrack.ron` track (after `Segmented` entries have been expanded
/// into individual segments — see [`RawSoundtrackItem`]). Ported verbatim
/// from `voxygen::audio::music::SoundtrackItem` (field-for-field, same
/// names) minus nothing — every field is kept even though [`Self::biomes`]/
/// [`Self::sites`] filtering can't be driven by a real signal yet (see
/// `crate::music::state`'s module doc comment for the honest gap).
#[derive(Clone, Debug, Deserialize)]
pub struct SoundtrackItem {
    pub title: String,
    pub path: String,
    pub length: f32,
    pub loop_points: Option<(f32, f32)>,
    pub timing: Option<DayPeriod>,
    pub weather: Option<WeatherKind>,
    pub biomes: Vec<(BiomeKind, u8)>,
    pub sites: Vec<SiteKindMeta>,
    pub music_state: MusicState,
    #[serde(default)]
    pub activity_override: Option<MusicActivity>,
    pub artist: (String, Option<String>),
}

/// The RON-level shape (`Individual`/`Segmented`) — ported verbatim from
/// `voxygen::audio::music::RawSoundtrackItem`. [`SoundtrackManifestLoader`]
/// expands every `Segmented` entry into its constituent segments at load
/// time, exactly like the old client's own `impl Asset for
/// SoundtrackCollection<SoundtrackItem>::load`.
#[derive(Clone, Debug, Deserialize)]
enum RawSoundtrackItem {
    Individual(SoundtrackItem),
    Segmented {
        title: String,
        timing: Option<DayPeriod>,
        weather: Option<WeatherKind>,
        biomes: Vec<(BiomeKind, u8)>,
        sites: Vec<SiteKindMeta>,
        segments: Vec<(String, f32, MusicState, Option<MusicActivity>)>,
        loop_points: (f32, f32),
        artist: (String, Option<String>),
    },
}

#[derive(Deserialize)]
struct RawSoundtrackCollection {
    tracks: Vec<RawSoundtrackItem>,
}

/// The parsed, fully-expanded `soundtrack.ron` (every `Segmented` entry
/// flattened into its segments — see [`RawSoundtrackItem`]).
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct SoundtrackCollection(pub Vec<SoundtrackItem>);

/// Async [`AssetLoader`] for `soundtrack.ron`, mirroring
/// [`crate::sfx::manifest::SfxManifestLoader`]'s exact style (async read to
/// bytes, `ron::de::from_bytes`, then a synchronous expansion pass).
#[derive(Default, TypePath)]
pub struct SoundtrackManifestLoader;

impl AssetLoader for SoundtrackManifestLoader {
    type Asset = SoundtrackCollection;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let raw: RawSoundtrackCollection = ron::de::from_bytes(&bytes)?;
        let mut tracks = Vec::new();
        for item in raw.tracks {
            match item {
                RawSoundtrackItem::Individual(track) => tracks.push(track),
                RawSoundtrackItem::Segmented {
                    title,
                    timing,
                    weather,
                    biomes,
                    sites,
                    segments,
                    loop_points,
                    artist,
                } => {
                    for (path, length, music_state, activity_override) in segments {
                        tracks.push(SoundtrackItem {
                            title: title.clone(),
                            path,
                            length,
                            loop_points: Some(loop_points),
                            timing,
                            weather,
                            biomes: biomes.clone(),
                            sites: sites.clone(),
                            music_state,
                            activity_override,
                            artist: artist.clone(),
                        });
                    }
                },
            }
        }
        Ok(SoundtrackCollection(tracks))
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

/// Ported verbatim from `voxygen::audio::music::MusicTransitionManifest`
/// (field-for-field) — `assets/voxygen/audio/music_transition_manifest.ron`.
/// Unlike the old client (which fell back to
/// `MusicTransitionManifest::default()` on a load error via
/// `Ron::load_or_insert_with`), this is a plain Bevy `Asset`: callers treat
/// "not loaded yet" as "do nothing this frame", the same `Option<Res<_>>`
/// pattern `crate::sfx`'s manifest handle already establishes — no `Default`
/// impl is needed for that degrade-clean path.
#[derive(Asset, TypePath, Debug, Clone, Deserialize)]
pub struct MusicTransitionManifest {
    /// Within what radius do enemies count towards combat music?
    pub combat_nearby_radius: f32,
    /// Each multiple of this factor that an enemy has health counts as an
    /// extra enemy.
    pub combat_health_factor: f32,
    /// How many nearby enemies (by weight) trigger High combat music.
    pub combat_nearby_high_thresh: u32,
    /// How many nearby enemies (by weight) trigger Low combat music.
    pub combat_nearby_low_thresh: u32,
    /// Fade in and fade out timings (seconds) for transitions between
    /// channels — `(from_tag, to_tag) -> (fade_out, fade_in)`.
    pub fade_timings: HashMap<(MusicChannelTag, MusicChannelTag), (f32, f32)>,
    /// How many seconds between interrupt checks.
    pub interrupt_delay: f32,
}

/// Async [`AssetLoader`] for `music_transition_manifest.ron`.
#[derive(Default, TypePath)]
pub struct MusicTransitionManifestLoader;

impl AssetLoader for MusicTransitionManifestLoader {
    type Asset = MusicTransitionManifest;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ron::de::from_bytes(&bytes)?)
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The REAL, shipped, frozen `soundtrack.ron` must parse in full — the
    /// strongest available guard that this ported vocabulary matches what
    /// the manifest actually uses, mirroring `sfx::manifest`'s own
    /// `shipped_sfx_manifest_parses_in_full` test.
    #[test]
    fn shipped_soundtrack_parses_in_full() {
        let text = include_str!("../../../../assets/voxygen/audio/soundtrack.ron");
        let raw: RawSoundtrackCollection =
            ron::from_str(text).expect("soundtrack.ron must parse with the ported vocabulary");
        assert!(
            raw.tracks.len() > 50,
            "sanity: the real manifest has well over 50 entries, got {}",
            raw.tracks.len()
        );
    }

    /// The REAL, shipped `music_transition_manifest.ron` must parse in full.
    #[test]
    fn shipped_music_transition_manifest_parses_in_full() {
        let text = include_str!("../../../../assets/voxygen/audio/music_transition_manifest.ron");
        let mtm: MusicTransitionManifest =
            ron::from_str(text).expect("music_transition_manifest.ron must parse");
        assert!(mtm.combat_nearby_radius > 0.0);
        assert!(
            mtm.fade_timings
                .contains_key(&(MusicChannelTag::Exploration, MusicChannelTag::Combat))
        );
    }
}
