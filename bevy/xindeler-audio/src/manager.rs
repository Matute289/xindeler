//! [`AudioBackend`] — the Kira [`AudioManager`] + 4 named mixer sub-tracks,
//! wrapped as a single Bevy [`Resource`].
//!
//! Kept as ONE enum resource (rather than two independently-`Option`al
//! resources for "the manager" and "the tracks") so the two can never
//! desynchronize — there is exactly one way to observe "audio is
//! unavailable", and every consumer destructures the same match.

use bevy::prelude::*;
use kira::{
    AudioManager, AudioManagerSettings,
    backend::cpal::CpalBackend,
    track::{TrackBuilder, TrackHandle},
};
use tracing::warn;

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
    let sfx = manager.add_sub_track(TrackBuilder::new()).ok()?;
    let ambience = manager.add_sub_track(TrackBuilder::new()).ok()?;
    Some(AudioTracks {
        music,
        ui,
        sfx,
        ambience,
    })
}
