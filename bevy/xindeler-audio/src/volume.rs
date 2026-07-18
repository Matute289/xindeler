//! Per-track + master volume, applied to the real Kira tracks on change.

use bevy::prelude::*;
use kira::Tween;

use crate::{manager::AudioBackend, to_decibels};

/// Linear (`0.0..=1.0`) volume knobs for the master output and each of the 4
/// named mixer sub-tracks. This is the seam EM-5.12 (Settings) will read from
/// and write to (a "Sound" settings tab); T56.34 only wires the write side.
///
/// All default to `1.0` (unmixed/full) — unlike `voxygen`'s own
/// `Volumes::default()` (which defaults every channel to silent `0.0` and
/// relies on a settings load to raise them before anything is audible), this
/// crate has no settings-load step wired yet (that is EM-5.12's job), so
/// defaulting to silence would make every later phase's SFX/music verify
/// step silently hear nothing until settings wiring lands. Full volume is the
/// more useful default for a headless foundation phase.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct AudioVolumes {
    pub master: f32,
    pub music: f32,
    pub ui: f32,
    pub sfx: f32,
    pub ambience: f32,
}

impl Default for AudioVolumes {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            ui: 1.0,
            sfx: 1.0,
            ambience: 1.0,
        }
    }
}

/// Pushes [`AudioVolumes`] onto the real Kira tracks. Gated by the caller
/// (`lib.rs`) on `resource_changed::<AudioVolumes>` — this body itself does
/// no additional change-detection, so it must never be registered
/// unconditionally.
pub(crate) fn apply_audio_volumes(volumes: Res<AudioVolumes>, mut backend: ResMut<AudioBackend>) {
    let Some(tracks) = backend.tracks_mut() else {
        return;
    };
    let tween = Tween::default();
    tracks.music.set_volume(to_decibels(volumes.music), tween);
    tracks.ui.set_volume(to_decibels(volumes.ui), tween);
    tracks.sfx.set_volume(to_decibels(volumes.sfx), tween);
    tracks
        .ambience
        .set_volume(to_decibels(volumes.ambience), tween);
    // The `tracks` borrow above ends here (last use); `manager_mut()` takes
    // its own fresh `&mut` on `backend` for the master track (which Kira only
    // exposes via `AudioManager::main_track()`, not a `TrackHandle` we could
    // have stored in `AudioTracks` alongside the other 4).
    if let Some(manager) = backend.manager_mut() {
        manager
            .main_track()
            .set_volume(to_decibels(volumes.master), tween);
    }
}
