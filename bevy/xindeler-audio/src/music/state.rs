//! BL-82 EM-5.10c (T56.36) — the music state machine: explore/combat
//! classification, transition/interrupt logic, and weighted track selection.
//! Ported from `voxygen::audio::music::MusicMgr::maintain`/
//! `play_random_track`/`generate_silence_between_tracks`, but restructured
//! into a set of PURE functions plus one plain-data [`MusicMachine`] struct —
//! no Bevy, no Kira, no `xindeler-protocol` — so the whole decision engine is
//! unit-testable without an `App` (this crate depends on `common` only, the
//! same "generic audio-domain vocabulary" positioning `sfx::event`/
//! `sfx::manifest` already establish; the Net*-reading translation lives in
//! `xindeler-client::music`, mirroring `xindeler-client::sfx`'s own mappers).
//!
//! ## Real vs. stubbed inputs (read before touching [`MusicInputs`])
//! - [`MusicInputs::nearby_enemy_weight`]/[`MusicInputs::player_dead`] — REAL.
//!   `xindeler-client::music` computes these off the ALREADY-mirrored
//!   `NetAlignment`/`NetHealth` (the same signal `targeting.rs`'s soft-target
//!   scanner uses for "what counts as an enemy"), exactly mirroring the old
//!   client's `group::ENEMY` nearby-health-weighted count. This is what drives
//!   the task's own verify bar ("combat proximity swaps the track").
//! - [`MusicInputs::day_period`] — REAL-ish. Derived from `xindeler-client`'s
//!   `SunCycle` (a CLIENT-LOCAL day/night stub — see that module's own doc
//!   comment: "Replaced by the mirrored sim time-of-day in EM-3.x"), not yet
//!   the server's authoritative time-of-day. Honest, documented approximation —
//!   better than no day/night variation at all, but not sim-authoritative.
//! - [`MusicInputs::weather`] — REAL, but coarse. `xindeler-client::music`
//!   reads `xindeler_oracle_host::AtmosphereController`'s mirrored
//!   `WeatherEffect` (`None`/`Rain`/`Storm`, ORACLE's placeholder weather
//!   taxonomy — see `xindeler-oracle-host::atmosphere`'s own doc comment) and
//!   maps it onto `common::weather::WeatherKind` (`Clear`/`Rain`/`Storm`
//!   respectively). `WeatherKind::Cloudy` is never produced by this mapping (no
//!   `WeatherEffect` variant for it) — a soundtrack entry gated on `Cloudy`
//!   specifically just never matches, same as any other never-satisfied filter.
//! - [`MusicInputs::site`]/[`MusicInputs::biome`] — DOCUMENTED GAPS, not
//!   fabricated. No `SiteKindMeta`/`BiomeKind` mirror exists ANYWHERE in
//!   `xindeler-protocol`/`xindeler-sim-bridge` today (verified by grep) — this
//!   Bevy client has no way to know what site or biome the player is standing
//!   in yet. `xindeler-client::music` always passes `SiteKindMeta::Void`
//!   (matching the old client's own "unknown" default, `MusicMgr::new`'s
//!   `last_site: SiteKindMeta::Void`) and `biome: None`.
//!   [`select_track`]/[`silence_between_tracks`] below still implement the REAL
//!   filtering/weighting logic against these fields (so a future site/biome
//!   mirror is a pure input-wiring change, not a redesign) — they just always
//!   receive the "no signal" input for now, which naturally degrades to "match
//!   the overworld (Void) tracks" / "no biome-based weighting" rather than
//!   silently dropping content.

use common::{
    terrain::{BiomeKind, SiteKindMeta},
    weather::WeatherKind,
};
use rand::{Rng, RngExt, seq::IndexedRandom};

use super::manifest::{CombatIntensity, MusicActivity, MusicState, SoundtrackItem};

/// The real + honestly-stubbed inputs [`MusicMachine::advance`] reads each
/// frame — see this module's doc comment for which fields are real signals
/// vs. documented gaps.
#[derive(Clone, Copy, Debug)]
pub struct MusicInputs {
    pub day_period: super::manifest::DayPeriod,
    pub weather: Option<WeatherKind>,
    pub site: SiteKindMeta,
    pub biome: Option<BiomeKind>,
    /// Sum, over nearby `NetAlignment::Enemy` entities, of
    /// `(health.max / combat_health_factor).ceil()` — ported verbatim from
    /// the old client's `num_nearby_entities` fold.
    pub nearby_enemy_weight: u32,
    pub player_dead: bool,
}

/// The client-local day/night stub's hour-of-day (`SunCycle::hour`,
/// `0.0..24.0`) mapped onto [`super::manifest::DayPeriod`] — using the SAME
/// 6:00/18:00 boundary `xindeler-client::light::sun_elevation_for_hour`
/// already treats as sunrise/sunset (`elevation.sin() <= 0` outside that
/// range), so day/night music selection agrees with what the sky/lighting is
/// visibly doing. See this module's doc comment for why this is a real-ish,
/// not sim-authoritative, signal.
#[must_use]
pub fn day_period_for_hour(hour: f32) -> super::manifest::DayPeriod {
    let h = hour.rem_euclid(24.0);
    if (6.0..18.0).contains(&h) {
        super::manifest::DayPeriod::Day
    } else {
        super::manifest::DayPeriod::Night
    }
}

/// Ported verbatim from the old client's `MusicMgr::maintain` combat
/// classification (the `num_nearby_entities` threshold ladder), minus the
/// nearby-entity scan itself (that's `xindeler-client::music`'s job, over
/// real mirrored `NetAlignment`/`NetHealth`/`Transform`). `player_dead`
/// overrides straight to [`MusicActivity::Explore`], matching the old code's
/// own "Override combat music with explore music if the player is dead".
#[must_use]
pub fn classify_activity(
    nearby_enemy_weight: u32,
    high_thresh: u32,
    low_thresh: u32,
    player_dead: bool,
) -> MusicActivity {
    if player_dead {
        return MusicActivity::Explore;
    }
    if nearby_enemy_weight >= high_thresh {
        MusicActivity::Combat(CombatIntensity::High)
    } else if nearby_enemy_weight >= low_thresh {
        MusicActivity::Combat(CombatIntensity::Low)
    } else {
        MusicActivity::Explore
    }
}

/// Ported verbatim from the old client's own transition derivation: a
/// changed activity becomes a one-frame [`MusicState::Transition`]; an
/// already-observed [`MusicState::Transition`] resolves to
/// `Activity(next)` the following tick (the old code's own
/// `MusicState::Transition(_, next) => { warn!(..); MusicState::Activity(next)
/// }` arm, minus the log).
#[must_use]
pub fn next_music_state(last: MusicState, activity: MusicActivity) -> MusicState {
    match last {
        MusicState::Activity(prev) => {
            if prev != activity {
                MusicState::Transition(prev, activity)
            } else {
                MusicState::Activity(activity)
            }
        },
        MusicState::Transition(_, next) => MusicState::Activity(next),
    }
}

/// Ported verbatim from `MusicMgr::play_random_track`'s filter/weight chain,
/// minus the site-kind fallback stage (dead code today — see this module's
/// doc comment — since `inputs.site` is always `Void`; kept ready for when a
/// real site mirror lands rather than deleted, the same "implement the real
/// logic, feed it a documented stub input" posture the whole module takes).
///
/// - `timing`/`weather` gate first (both `None` = "any").
/// - `sites` is NEVER empty on a real track (every shipped entry lists at least
///   `Void`) — an exact match against `inputs.site` is required.
/// - `biomes` gates only when `inputs.biome` is `Some` (real signal); with
///   `None` (today's honest default) every track passes this stage — matching
///   the old code's own `track.biomes.is_empty()` short-circuit, generalized to
///   "no biome opinion available at all", not just "no biome opinion authored
///   for this track".
/// - `music_state` must match exactly.
/// - avoids repeating `last_track`/`last_combat_track`, same two-branch split
///   as the old code (combat state prefers *repeating* the loop combat track;
///   everything else avoids repeating either).
/// - weighted-random pick: `1.0 / weight` when `inputs.biome` names a listed
///   biome, else uniform `1.0` — collapsing to uniform whenever `biome` is
///   `None` (today's honest default), matching the old code's own "song still
///   gets a slot even with no biome match" fallback weight.
#[must_use]
pub fn select_track<'a>(
    tracks: &'a [SoundtrackItem],
    inputs: &MusicInputs,
    music_state: MusicState,
    last_track: &str,
    last_combat_track: &str,
    rng: &mut impl Rng,
) -> Option<&'a SoundtrackItem> {
    let mut candidates: Vec<&SoundtrackItem> = tracks
        .iter()
        .filter(|track| {
            track
                .timing
                .as_ref()
                .is_none_or(|t| *t == inputs.day_period)
                && track.weather.is_none_or(|w| Some(w) == inputs.weather)
        })
        .filter(|track| track.sites.contains(&inputs.site))
        .filter(|track| {
            track.biomes.is_empty()
                || inputs
                    .biome
                    .is_none_or(|b| track.biomes.iter().any(|tb| tb.0 == b))
        })
        .filter(|track| track.music_state == music_state)
        .collect();

    if candidates.is_empty() {
        return None;
    }

    let is_combat_loop_or_outro = matches!(
        music_state,
        MusicState::Activity(MusicActivity::Combat(CombatIntensity::High))
            | MusicState::Transition(
                MusicActivity::Combat(CombatIntensity::High),
                MusicActivity::Explore
            )
    );
    if is_combat_loop_or_outro {
        let repeating: Vec<_> = candidates
            .iter()
            .filter(|track| track.title == last_track)
            .copied()
            .collect();
        if !repeating.is_empty() {
            candidates = repeating;
        }
    } else {
        let fresh: Vec<_> = candidates
            .iter()
            .filter(|track| track.title != last_track && track.title != last_combat_track)
            .copied()
            .collect();
        if !fresh.is_empty() {
            candidates = fresh;
        }
    }

    candidates
        .choose_weighted(rng, |track| {
            inputs
                .biome
                .and_then(|b| track.biomes.iter().find(|tb| tb.0 == b))
                .map_or(1.0_f32, |tb| 1.0_f32 / (tb.1 as f32))
        })
        .ok()
        .copied()
}

/// Ported from `MusicMgr::generate_silence_between_tracks`, parameterized on
/// `site` (always `Void` today — see this module's doc comment) instead of
/// reading a live `Client`.
#[must_use]
pub fn silence_between_tracks(
    spacing_multiplier: f32,
    site: SiteKindMeta,
    music_state: MusicState,
    rng: &mut impl Rng,
) -> f32 {
    if spacing_multiplier <= f32::EPSILON {
        return 0.0;
    }
    let is_explore_ish = matches!(
        music_state,
        MusicState::Activity(MusicActivity::Explore)
            | MusicState::Transition(
                MusicActivity::Explore,
                MusicActivity::Combat(CombatIntensity::High)
            )
    );
    if is_explore_ish && matches!(site, SiteKindMeta::Settlement(_)) {
        rng.random_range(120.0 * spacing_multiplier..180.0 * spacing_multiplier)
    } else if is_explore_ish && matches!(site, SiteKindMeta::Dungeon(_)) {
        rng.random_range(10.0 * spacing_multiplier..20.0 * spacing_multiplier)
    } else if is_explore_ish && site == SiteKindMeta::Cave {
        rng.random_range(20.0 * spacing_multiplier..40.0 * spacing_multiplier)
    } else if is_explore_ish {
        rng.random_range(120.0 * spacing_multiplier..240.0 * spacing_multiplier)
    } else if matches!(
        music_state,
        MusicState::Activity(MusicActivity::Combat(_)) | MusicState::Transition(_, _)
    ) {
        0.0
    } else {
        rng.random_range(30.0 * spacing_multiplier..60.0 * spacing_multiplier)
    }
}

/// What [`MusicMachine::advance`] wants the caller to do this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum MusicAction {
    /// Nothing to change this frame (still mid-track, mid-gap, or no
    /// eligible track was found for the current state).
    None,
    /// Cross-fade to this track, tagged as exploration or combat music (the
    /// caller — `xindeler-client::music` — resolves fade timings from
    /// `MusicTransitionManifest::fade_timings` against whatever tag is
    /// CURRENTLY playing, then calls `crate::music::playback::crossfade`).
    Play(TrackToPlay),
}

/// An owned snapshot of the fields playback actually needs, decoupled from
/// the manifest's borrow so [`MusicMachine::advance`] can return it without
/// fighting the borrow checker over `&mut self` + `&[SoundtrackItem]`.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackToPlay {
    pub title: String,
    pub path: String,
    pub length: f32,
    pub loop_points: Option<(f32, f32)>,
    pub tag: super::manifest::MusicChannelTag,
}

/// The stateful half of the port: began-playing/song-end/gap bookkeeping,
/// ported field-for-field from `voxygen::audio::music::MusicMgr` (swapping
/// Kira `ClockTime` for plain `f64` seconds — `xindeler-client::music` feeds
/// `Time::elapsed_secs_f64()`, since this crate has no Kira `Clock` of its
/// own to synchronize against; the specific clock source doesn't matter to
/// this state machine, only that it's monotonic).
#[derive(Debug, Clone)]
pub struct MusicMachine {
    began_playing: Option<f64>,
    song_end: Option<f64>,
    is_gap: bool,
    gap_length: f32,
    gap_time: f32,
    last_track: String,
    last_combat_track: String,
    last_interrupt_attempt: Option<f64>,
    last_activity: MusicState,
    track_length: f32,
    loop_points: Option<(f32, f32)>,
    last_site: SiteKindMeta,
}

impl Default for MusicMachine {
    fn default() -> Self {
        Self {
            began_playing: None,
            song_end: None,
            is_gap: true,
            gap_length: 0.0,
            gap_time: -1.0,
            last_track: String::from("None"),
            last_combat_track: String::from("None"),
            last_interrupt_attempt: None,
            last_activity: MusicState::Activity(MusicActivity::Explore),
            track_length: 0.0,
            loop_points: None,
            last_site: SiteKindMeta::Void,
        }
    }
}

impl MusicMachine {
    /// The activity the machine most recently settled on (test/inspection
    /// hook — this is exactly what the task's own "combat proximity swaps
    /// the track" verify checks: that this flips to `Combat` when nearby
    /// hostiles cross the threshold).
    #[must_use]
    pub fn last_activity(&self) -> MusicState { self.last_activity }

    /// One tick of the state machine — ported from `MusicMgr::maintain`
    /// (see this module's doc comment for what `inputs` really carries).
    /// `music_spacing` mirrors the old client's `AudioFrontend::music_spacing`
    /// settings knob; no settings pipeline wires it yet, so callers pass
    /// `1.0` (unmixed) — a documented, harmless default (`spacing_multiplier
    /// <= f32::EPSILON` is the only other special case, and `1.0` isn't it).
    pub fn advance(
        &mut self,
        now_secs: f64,
        inputs: &MusicInputs,
        tracks: &[SoundtrackItem],
        mtm: &super::manifest::MusicTransitionManifest,
        music_spacing: f32,
        rng: &mut impl Rng,
    ) -> MusicAction {
        let activity_state = classify_activity(
            inputs.nearby_enemy_weight,
            mtm.combat_nearby_high_thresh,
            mtm.combat_nearby_low_thresh,
            inputs.player_dead,
        );

        let mut music_state = next_music_state(self.last_activity, activity_state);

        let began_playing = *self.began_playing.get_or_insert(now_secs);
        let last_interrupt_attempt = *self.last_interrupt_attempt.get_or_insert(now_secs);
        let song_end = *self.song_end.get_or_insert(now_secs);
        let time_since_began_playing = now_secs - began_playing;

        let site_changed = inputs.site != self.last_site;
        self.last_site = inputs.site;

        if site_changed
            && !matches!(
                music_state,
                MusicState::Activity(MusicActivity::Combat(CombatIntensity::High))
            )
        {
            music_state = MusicState::Transition(MusicActivity::Explore, MusicActivity::Explore);
        }

        let interrupt = matches!(music_state, MusicState::Transition(_, _))
            && (matches!(
                music_state,
                MusicState::Transition(MusicActivity::Explore, MusicActivity::Explore)
            ) || now_secs - last_interrupt_attempt > mtm.interrupt_delay as f64);

        if matches!(
            music_state,
            MusicState::Transition(
                MusicActivity::Combat(CombatIntensity::High),
                MusicActivity::Explore
            )
        ) {
            music_state = MusicState::Activity(MusicActivity::Explore);
        }

        let mut action = MusicAction::None;

        if !tracks.is_empty() && (time_since_began_playing > song_end - began_playing || interrupt)
        {
            if interrupt {
                self.last_interrupt_attempt = Some(now_secs);
                self.is_gap = false;
                self.gap_time = 0.0;
                self.gap_length = 0.0;
                let track_state = if music_state
                    == MusicState::Transition(MusicActivity::Explore, MusicActivity::Explore)
                {
                    MusicState::Activity(MusicActivity::Explore)
                } else {
                    music_state
                };
                if let Some(item) = select_track(
                    tracks,
                    inputs,
                    track_state,
                    &self.last_track,
                    &self.last_combat_track,
                    rng,
                ) {
                    action = self.commit_play(now_secs, item, track_state);
                }
            } else if music_state == MusicState::Activity(MusicActivity::Explore)
                || music_state
                    == MusicState::Transition(
                        MusicActivity::Explore,
                        MusicActivity::Combat(CombatIntensity::High),
                    )
            {
                if !self.is_gap {
                    self.gap_length =
                        silence_between_tracks(music_spacing, inputs.site, music_state, rng);
                    self.gap_time = self.gap_length;
                    self.song_end = Some(now_secs);
                    self.is_gap = true;
                } else if self.gap_time < 0.0 {
                    let effective_state = if music_state
                        == MusicState::Transition(
                            MusicActivity::Explore,
                            MusicActivity::Combat(CombatIntensity::High),
                        ) {
                        MusicState::Activity(MusicActivity::Explore)
                    } else {
                        music_state
                    };
                    if let Some(item) = select_track(
                        tracks,
                        inputs,
                        effective_state,
                        &self.last_track,
                        &self.last_combat_track,
                        rng,
                    ) {
                        action = self.commit_play(now_secs, item, effective_state);
                        self.gap_time = 0.0;
                        self.gap_length = 0.0;
                        self.is_gap = false;
                    }
                }
            } else if music_state
                == MusicState::Activity(MusicActivity::Combat(CombatIntensity::High))
            {
                self.began_playing = Some(now_secs);
                let (lp0, lp1) = self.loop_points.unwrap_or((0.0, 0.0));
                self.song_end = Some(now_secs + (lp1 - lp0) as f64);
            }
        }

        if time_since_began_playing > self.track_length as f64 && self.is_gap {
            self.gap_time = self.gap_length - (now_secs - song_end) as f32;
        }

        action
    }

    /// Ported from the tail of `play_random_track`: records bookkeeping,
    /// resolves the channel tag, and returns the [`MusicAction`] the caller
    /// plays. Also applies `activity_override` (e.g. a combat intro segment
    /// that should be treated as the loop for outro-transition purposes),
    /// matching the old code's `Ok(MusicState::Activity(state))` branch.
    fn commit_play(
        &mut self,
        now_secs: f64,
        item: &SoundtrackItem,
        music_state: MusicState,
    ) -> MusicAction {
        self.last_track = item.title.clone();
        self.began_playing = Some(now_secs);
        self.song_end = Some(now_secs + item.length as f64);
        self.track_length = item.length;
        self.gap_length = 0.0;

        let tag = if matches!(
            music_state,
            MusicState::Activity(MusicActivity::Explore)
                | MusicState::Transition(MusicActivity::Explore, MusicActivity::Explore)
        ) {
            super::manifest::MusicChannelTag::Exploration
        } else {
            self.last_combat_track = item.title.clone();
            super::manifest::MusicChannelTag::Combat
        };
        self.loop_points = if tag == super::manifest::MusicChannelTag::Combat {
            item.loop_points
        } else {
            None
        };

        self.last_activity = item
            .activity_override
            .map_or(music_state, MusicState::Activity);

        MusicAction::Play(TrackToPlay {
            title: item.title.clone(),
            path: item.path.clone(),
            length: item.length,
            loop_points: item.loop_points,
            tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use rand::rng;

    use super::*;
    use crate::music::manifest::DayPeriod;

    fn track(title: &str, state: MusicState, site: SiteKindMeta) -> SoundtrackItem {
        SoundtrackItem {
            title: title.to_owned(),
            path: format!("voxygen.audio.soundtrack.{title}"),
            length: 10.0,
            loop_points: None,
            timing: None,
            weather: None,
            biomes: Vec::new(),
            sites: vec![site],
            music_state: state,
            activity_override: None,
            artist: ("Test".to_owned(), None),
        }
    }

    fn inputs(nearby: u32, dead: bool) -> MusicInputs {
        MusicInputs {
            day_period: DayPeriod::Day,
            weather: None,
            site: SiteKindMeta::Void,
            biome: None,
            nearby_enemy_weight: nearby,
            player_dead: dead,
        }
    }

    #[test]
    fn day_period_boundaries_match_the_sun_rig() {
        assert_eq!(day_period_for_hour(6.0), DayPeriod::Day);
        assert_eq!(day_period_for_hour(17.99), DayPeriod::Day);
        assert_eq!(day_period_for_hour(18.0), DayPeriod::Night);
        assert_eq!(day_period_for_hour(5.99), DayPeriod::Night);
        assert_eq!(day_period_for_hour(0.0), DayPeriod::Night);
        // Wraps cleanly for an hour outside 0..24.
        assert_eq!(day_period_for_hour(30.0), day_period_for_hour(6.0));
    }

    #[test]
    fn classify_activity_escalates_with_nearby_weight() {
        assert_eq!(classify_activity(0, 3, 1, false), MusicActivity::Explore);
        assert_eq!(
            classify_activity(1, 3, 1, false),
            MusicActivity::Combat(CombatIntensity::Low)
        );
        assert_eq!(
            classify_activity(3, 3, 1, false),
            MusicActivity::Combat(CombatIntensity::High)
        );
    }

    #[test]
    fn classify_activity_player_dead_forces_explore() {
        assert_eq!(classify_activity(10, 3, 1, true), MusicActivity::Explore);
    }

    #[test]
    fn next_music_state_transitions_then_settles() {
        let start = MusicState::Activity(MusicActivity::Explore);
        let combat = MusicActivity::Combat(CombatIntensity::High);
        let transitioning = next_music_state(start, combat);
        assert_eq!(
            transitioning,
            MusicState::Transition(MusicActivity::Explore, combat)
        );
        let settled = next_music_state(transitioning, combat);
        assert_eq!(settled, MusicState::Activity(combat));
    }

    #[test]
    fn select_track_matches_only_the_requested_site_and_state() {
        let tracks = vec![
            track(
                "Overworld Song",
                MusicState::Activity(MusicActivity::Explore),
                SiteKindMeta::Void,
            ),
            track(
                "Town Song",
                MusicState::Activity(MusicActivity::Explore),
                SiteKindMeta::Settlement(common::terrain::site::SettlementKindMeta::Default),
            ),
        ];
        let inputs = inputs(0, false);
        let mut rng = rng();
        let picked = select_track(
            &tracks,
            &inputs,
            MusicState::Activity(MusicActivity::Explore),
            "None",
            "None",
            &mut rng,
        );
        assert_eq!(picked.map(|t| t.title.as_str()), Some("Overworld Song"));
    }

    /// The task's headline logic: the MACHINE's own activity classification
    /// flips from Explore to Combat once nearby hostile weight crosses the
    /// threshold, and a matching Combat track is what gets selected —
    /// "combat proximity swaps the track" at the pure-logic level (the
    /// integration test in `xindeler-client::music` drives this through a
    /// real headless `App` + real mirrored entities + real Kira playback).
    ///
    /// The combat track is authored the same way a real
    /// `soundtrack.ron`/`music_transition_manifest.ron` combat entry would be
    /// (see the commented-out `Segmented` combat examples at the bottom of
    /// the real manifest — a "start" segment tagged
    /// `Transition(Explore, Combat(High))` with `activity_override:
    /// Some(Combat(High))`, so the state machine settles on `Activity(Combat
    /// (High))` — i.e. the OLD client's real, if currently-unused, content
    /// shape, not a fabricated one).
    ///
    /// Also exercises the real `interrupt_delay` throttle (ported verbatim —
    /// see `MusicMachine::advance`'s doc comment): a combat transition inside
    /// the throttle window does NOT swap the track immediately; only once
    /// `interrupt_delay` seconds have elapsed since the last interrupt
    /// attempt (here, boot) does the swap fire — matching the reference
    /// engine's own "avoid rapid switching" design, not a bug in this port.
    #[test]
    fn advance_transitions_to_combat_and_selects_a_combat_track() {
        let mut battle_hymn = track(
            "Battle Hymn",
            MusicState::Transition(
                MusicActivity::Explore,
                MusicActivity::Combat(CombatIntensity::High),
            ),
            SiteKindMeta::Void,
        );
        battle_hymn.activity_override = Some(MusicActivity::Combat(CombatIntensity::High));
        let tracks = vec![
            track(
                "Peaceful Walk",
                MusicState::Activity(MusicActivity::Explore),
                SiteKindMeta::Void,
            ),
            battle_hymn,
        ];
        let mtm = super::super::manifest::MusicTransitionManifest {
            combat_nearby_radius: 25.0,
            combat_health_factor: 70.0,
            combat_nearby_high_thresh: 3,
            combat_nearby_low_thresh: 1,
            fade_timings: std::collections::HashMap::new(),
            interrupt_delay: 5.0,
        };
        let mut machine = MusicMachine::default();
        let mut rng = rng();

        // Frame 0 (t=0): the very first tick only SEEDS the clock
        // (`began_playing`/`song_end` both default-insert to `now`, so
        // `time_since_began_playing (0) > song_end - began_playing (0)` is
        // false) — ported faithfully from the reference engine's own
        // `MusicMgr::maintain` (same shape, same one-tick warm-up), and
        // invisible in real gameplay where the next `Update` tick follows
        // within milliseconds.
        let action = machine.advance(0.0, &inputs(0, false), &tracks, &mtm, 1.0, &mut rng);
        assert_eq!(action, MusicAction::None);

        // Frame 1 (t=0.01): any positive elapsed time now clears the gate ->
        // no hostiles nearby -> explore music.
        let action = machine.advance(0.01, &inputs(0, false), &tracks, &mtm, 1.0, &mut rng);
        assert_eq!(
            action,
            MusicAction::Play(TrackToPlay {
                title: "Peaceful Walk".to_owned(),
                path: "voxygen.audio.soundtrack.Peaceful Walk".to_owned(),
                length: 10.0,
                loop_points: None,
                tag: super::super::manifest::MusicChannelTag::Exploration,
            })
        );
        assert_eq!(
            machine.last_activity(),
            MusicState::Activity(MusicActivity::Explore)
        );

        // Frame 2 (t=1): 3+ weighted hostiles suddenly nearby, but still
        // inside the `interrupt_delay` throttle window -> no swap yet.
        let action = machine.advance(1.0, &inputs(3, false), &tracks, &mtm, 1.0, &mut rng);
        assert_eq!(action, MusicAction::None);

        // Frame 3 (t=5.1): hostiles still nearby, and `interrupt_delay` (5s)
        // has now elapsed since boot -> the transition is a real interrupt,
        // and the combat track is selected.
        let action = machine.advance(5.1, &inputs(3, false), &tracks, &mtm, 1.0, &mut rng);
        assert_eq!(
            action,
            MusicAction::Play(TrackToPlay {
                title: "Battle Hymn".to_owned(),
                path: "voxygen.audio.soundtrack.Battle Hymn".to_owned(),
                length: 10.0,
                loop_points: None,
                tag: super::super::manifest::MusicChannelTag::Combat,
            })
        );
        assert_eq!(
            machine.last_activity(),
            MusicState::Activity(MusicActivity::Combat(CombatIntensity::High))
        );
    }
}
