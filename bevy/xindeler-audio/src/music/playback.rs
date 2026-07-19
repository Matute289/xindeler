//! BL-82 EM-5.10c (T56.36) — resolves a [`super::state::TrackToPlay`] to a
//! real loaded `.ogg` asset and crossfades it in through the `music` mixer
//! track, via the EXACT [`crate::AudioBackend`]/`TrackHandle::play` API PR
//! #174 (T56.34) established (no parallel playback path) — mirroring
//! `crate::sfx::playback::trigger_sfx`'s shape, but keeping a RETAINED
//! handle (music isn't fire-and-forget: the old client's own
//! `MusicChannel` kept a handle so it could fade/stop/loop it later; a
//! one-shot SFX play never needed that).
//!
//! Crossfade mechanics (ported from `voxygen::audio::mod.rs::play_music`,
//! adapted from "3 named Kira sub-tracks, one per `MusicChannelTag`" to
//! "one `music` mixer track, per-SOUND fade tweens" — this crate's
//! [`crate::AudioTracks`] only exposes a single `music` track, so instead of
//! stopping/starting sub-tracks this fades the individual sound INSTANCES):
//! the previously-playing handle (if any) gets `.stop(Tween{duration:
//! fade_out})` (mirrors `MusicChannel::stop`'s fade-out-then-stop), and the
//! new sound is built with `.fade_in_tween(Tween{duration: fade_in})`
//! (mirrors `MusicChannel::play`'s `fade_in` param) before being played on
//! the SAME `music` track — both audible simultaneously during the overlap,
//! which is exactly what a crossfade is.

use bevy::{
    asset::{AssetServer, Assets, Handle},
    ecs::resource::Resource,
};
use kira::{
    Tween,
    sound::{EndPosition, PlaybackPosition, Region, static_sound::StaticSoundHandle},
};
use std::{collections::HashMap, time::Duration};

use super::{manifest::MusicChannelTag, state::TrackToPlay};
use crate::{AudioBackend, XindelerAudioAsset, sfx::dotted_key_to_ogg_path};

/// Caches `.ogg` asset handles by their resolved path, same rationale as
/// [`crate::sfx::SfxAssetCache`] — kept as its own type (rather than sharing
/// that one) so the music and sfx asset caches can't accidentally cross-pollute
/// bookkeeping despite both wrapping the same `HashMap<String, Handle<..>>`
/// shape.
#[derive(Resource, Default)]
pub struct MusicAssetCache(HashMap<String, Handle<XindelerAudioAsset>>);

impl MusicAssetCache {
    /// Seeds the cache with an already-obtained handle for `path` (the real
    /// `.ogg` asset path, e.g. from [`dotted_key_to_ogg_path`]) — same
    /// rationale as [`crate::sfx::SfxAssetCache::insert_preloaded`]: lets a
    /// caller (or a test) warm up a track ahead of the first real crossfade,
    /// avoiding a race against the async first `AssetServer::load`.
    pub fn insert_preloaded(
        &mut self,
        path: impl Into<String>,
        handle: Handle<XindelerAudioAsset>,
    ) {
        self.0.insert(path.into(), handle);
    }
}

/// What's currently playing on the `music` track, if anything — the
/// crossfade's "previous" side. `None` at boot (nothing playing yet).
#[derive(Resource, Default)]
pub struct MusicPlayerState {
    current: Option<(StaticSoundHandle, MusicChannelTag)>,
}

impl MusicPlayerState {
    /// The [`MusicChannelTag`] currently playing, if any — what the caller
    /// (`xindeler-client::music`) looks up in
    /// `MusicTransitionManifest::fade_timings` as the `from` side of the
    /// `(from, to)` key.
    #[must_use]
    pub fn current_tag(&self) -> Option<MusicChannelTag> {
        self.current.as_ref().map(|(_, tag)| tag).copied()
    }
}

/// Crossfades to `track`: fades out (then stops) whatever was previously
/// playing using `fade_out` seconds, and plays `track`'s resolved `.ogg`
/// asset with a `fade_in`-second fade-in on the SAME `music` track, looping
/// [`TrackToPlay::loop_points`] when tagged [`MusicChannelTag::Combat`]
/// (ported from the old client's own "combat tracks loop, exploration
/// tracks don't" — `play_music`'s `tag == MusicChannelTag::Combat` branch).
///
/// A no-op (returns `false`, changing nothing) when: the audio backend is
/// [`AudioBackend::Unavailable`] (no `cpal` device); or the chosen asset
/// hasn't finished its (async) first load yet — same documented v1 trade-off
/// `trigger_sfx` already carries for SFX. On success, updates
/// [`MusicPlayerState`] so the NEXT crossfade fades out THIS track.
pub fn crossfade(
    state: &mut MusicPlayerState,
    track: &TrackToPlay,
    fade_out: f32,
    fade_in: f32,
    asset_server: &AssetServer,
    audio_assets: &Assets<XindelerAudioAsset>,
    cache: &mut MusicAssetCache,
    backend: &mut AudioBackend,
) -> bool {
    let path = dotted_key_to_ogg_path(&track.path);
    let handle = cache
        .0
        .entry(path.clone())
        .or_insert_with(|| asset_server.load(path))
        .clone();
    let Some(asset) = audio_assets.get(&handle) else {
        return false;
    };
    let Some(tracks) = backend.tracks_mut() else {
        return false;
    };

    let fade_out_tween = Tween {
        duration: Duration::from_secs_f32(fade_out.max(0.0)),
        ..Default::default()
    };
    if let Some((mut previous, _)) = state.current.take() {
        previous.stop(fade_out_tween);
    }

    let mut data = asset.0.clone().fade_in_tween(Some(Tween {
        duration: Duration::from_secs_f32(fade_in.max(0.0)),
        ..Default::default()
    }));
    if track.tag == MusicChannelTag::Combat
        && let Some((start, end)) = track.loop_points
    {
        data = data.loop_region(Region {
            start: PlaybackPosition::Seconds(start as f64),
            end: EndPosition::Custom(PlaybackPosition::Seconds(end as f64)),
        });
    }

    match tracks.music.play(data) {
        Ok(new_handle) => {
            state.current = Some((new_handle, track.tag));
            true
        },
        Err(_) => false,
    }
}
