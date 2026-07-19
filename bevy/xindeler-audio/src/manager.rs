//! [`AudioBackend`] — the Kira [`AudioManager`] + 4 named mixer sub-tracks,
//! wrapped as a single Bevy [`Resource`].
//!
//! Kept as ONE enum resource (rather than two independently-`Option`al
//! resources for "the manager" and "the tracks") so the two can never
//! desynchronize — there is exactly one way to observe "audio is
//! unavailable", and every consumer destructures the same match.

use bevy::prelude::*;
use kira::{
    AudioManager, AudioManagerSettings, Tween,
    backend::cpal::CpalBackend,
    effect::filter::{FilterBuilder, FilterHandle},
    track::{TrackBuilder, TrackHandle},
};
use tracing::warn;

/// The sfx-track low-pass cutoff (Hz) in the normal, un-muffled case — high
/// enough (≈ the top of human hearing) that the filter is effectively
/// transparent. Dropped to [`SFX_FILTER_MUFFLED_HZ`] when the listener is
/// underwater. Ported verbatim from the old client's own
/// `set_sfx_master_filter` values (`voxygen::audio::sfx`).
pub const SFX_FILTER_OPEN_HZ: f64 = 20_000.0;
/// The sfx-track low-pass cutoff (Hz) applied while the listener is underwater
/// — a heavy muffle that removes the high end, the SAME 888 Hz the old client
/// used.
pub const SFX_FILTER_MUFFLED_HZ: f64 = 888.0;

/// The sfx low-pass cutoff (Hz) for a given underwater state — muffled when
/// underwater, transparent otherwise. Split out as a pure function so the
/// mapping is unit-testable without a real audio device.
#[must_use]
pub fn sfx_filter_cutoff(underwater: bool) -> f64 {
    if underwater {
        SFX_FILTER_MUFFLED_HZ
    } else {
        SFX_FILTER_OPEN_HZ
    }
}

/// The 4 named mixer sub-tracks routed under Kira's main (master) track.
///
/// The master track itself has no handle stored here — Kira exposes it via
/// `AudioManager::main_track()` (a method on the manager, not a separate
/// resource we could hold onto independently), so callers reach it through
/// [`AudioBackend::manager_mut`].
pub struct AudioTracks {
    pub music: TrackHandle,
    pub ui: TrackHandle,
    pub sfx: TrackHandle,
    pub ambience: TrackHandle,
    /// The low-pass filter effect on the `sfx` track (BL-82 EM-5.10d) — its
    /// cutoff is driven between [`SFX_FILTER_OPEN_HZ`] and
    /// [`SFX_FILTER_MUFFLED_HZ`] by [`AudioBackend::set_sfx_muffle`] to muffle
    /// every positional sound when the listener is underwater.
    pub sfx_filter: FilterHandle,
}

/// The audio backend: either a live Kira [`AudioManager`] + its tracks, or
/// [`Unavailable`](AudioBackend::Unavailable) when no `cpal` output device
/// could be opened (e.g. a headless CI runner with no sound card). Every
/// system in this crate treats the `Unavailable` case as a silent no-op —
/// see the crate root doc comment for the "log and continue" rationale.
///
/// `Ready`'s fields are individually `Box`ed (clippy `large_enum_variant`):
/// `AudioManager<CpalBackend>` + 4 `TrackHandle`s make `Ready` far larger than
/// the unit-like `Unavailable`, so every `AudioBackend` value (moved around
/// freely as a plain `Resource`) would otherwise pay that size even in the
/// common `Unavailable` case. The `Box`es are invisible to every caller —
/// [`manager_mut`](Self::manager_mut)/[`tracks_mut`](Self::tracks_mut) return
/// plain `&mut` references, same as before boxing.
#[derive(Resource)]
pub enum AudioBackend {
    Ready {
        manager: Box<AudioManager<CpalBackend>>,
        tracks: Box<AudioTracks>,
    },
    Unavailable,
}

impl AudioBackend {
    /// Returns `true` if a real audio device is backing this manager.
    pub fn is_available(&self) -> bool { matches!(self, Self::Ready { .. }) }

    /// Mutable access to the Kira manager (for `main_track()`, spatial
    /// sub-tracks in a later phase, etc.), if available.
    pub fn manager_mut(&mut self) -> Option<&mut AudioManager<CpalBackend>> {
        match self {
            Self::Ready { manager, .. } => Some(manager.as_mut()),
            Self::Unavailable => None,
        }
    }

    /// Mutable access to the 4 named sub-tracks, if available.
    pub fn tracks_mut(&mut self) -> Option<&mut AudioTracks> {
        match self {
            Self::Ready { tracks, .. } => Some(tracks.as_mut()),
            Self::Unavailable => None,
        }
    }

    /// BL-82 EM-5.10d: muffle (or un-muffle) every positional sound by driving
    /// the `sfx` track's low-pass filter between [`SFX_FILTER_MUFFLED_HZ`]
    /// (underwater) and [`SFX_FILTER_OPEN_HZ`] (transparent). A cheap no-op
    /// when audio is [`Unavailable`](Self::Unavailable). The `0.1 s` tween
    /// smooths the transition so surfacing/diving does not click.
    pub fn set_sfx_muffle(&mut self, underwater: bool) {
        if let Self::Ready { tracks, .. } = self {
            let cutoff = sfx_filter_cutoff(underwater);
            tracks.sfx_filter.set_cutoff(cutoff, Tween {
                duration: std::time::Duration::from_secs_f32(0.1),
                ..Default::default()
            });
        }
    }
}

/// Initializes the Kira [`AudioManager`] on the default `cpal` output device
/// and creates the 4 mixer sub-tracks, inserting the result as an
/// [`AudioBackend`] resource.
///
/// Deliberately uses [`AudioManagerSettings::default`] rather than porting
/// `voxygen`'s own >48kHz-samplerate device/config renegotiation dance
/// (`AudioFrontend::new` in the old client) — that is a device-compatibility
/// nicety, not core plumbing, and is left as a documented follow-up for
/// whichever later phase first hits a real device that needs it.
pub(crate) fn insert_audio_backend(app: &mut App) {
    let backend = match AudioManager::<CpalBackend>::new(AudioManagerSettings::default()) {
        Ok(mut manager) => match build_tracks(&mut manager) {
            Some(tracks) => AudioBackend::Ready {
                manager: Box::new(manager),
                tracks: Box::new(tracks),
            },
            None => {
                warn!(
                    "xindeler-audio: failed to create the 4 mixer sub-tracks; running with audio \
                     disabled"
                );
                AudioBackend::Unavailable
            },
        },
        Err(err) => {
            warn!(
                ?err,
                "xindeler-audio: no cpal output device available; running with audio disabled \
                 (this is expected on a headless CI runner)"
            );
            AudioBackend::Unavailable
        },
    };
    app.insert_resource(backend);
}

fn build_tracks(manager: &mut AudioManager<CpalBackend>) -> Option<AudioTracks> {
    let music = manager.add_sub_track(TrackBuilder::new()).ok()?;
    let ui = manager.add_sub_track(TrackBuilder::new()).ok()?;
    // BL-82 EM-5.10d: the sfx track carries a low-pass filter (transparent by
    // default) so the underwater muffle can be toggled on it via
    // `set_sfx_muffle`, exactly as the old client filtered its whole sfx track
    // (`voxygen::audio::mod::Effects::sfx`).
    let mut sfx_builder = TrackBuilder::new();
    let sfx_filter = sfx_builder.add_effect(FilterBuilder::new().cutoff(SFX_FILTER_OPEN_HZ));
    let sfx = manager.add_sub_track(sfx_builder).ok()?;
    let ambience = manager.add_sub_track(TrackBuilder::new()).ok()?;
    Some(AudioTracks {
        music,
        ui,
        sfx,
        ambience,
        sfx_filter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underwater_muffles_and_surfacing_reopens_the_sfx_filter() {
        assert_eq!(sfx_filter_cutoff(true), SFX_FILTER_MUFFLED_HZ);
        assert_eq!(sfx_filter_cutoff(false), SFX_FILTER_OPEN_HZ);
        // The muffled cutoff must actually cut (well below the transparent one),
        // or "underwater" would be inaudible as a change.
        const {
            assert!(SFX_FILTER_MUFFLED_HZ < SFX_FILTER_OPEN_HZ);
        }
    }

    #[test]
    fn set_sfx_muffle_is_a_silent_no_op_when_audio_is_unavailable() {
        // The resilient path: no device → the method must not panic.
        let mut backend = AudioBackend::Unavailable;
        backend.set_sfx_muffle(true);
        backend.set_sfx_muffle(false);
    }
}
