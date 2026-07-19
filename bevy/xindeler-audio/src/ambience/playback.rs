//! BL-82 EM-5.10c (T56.36) — per-tag ambience channel playback: lazily spawns
//! a looping instance the first time a tag's target volume goes above zero,
//! then fades its volume toward the target every frame — ported from
//! `voxygen::audio::ambience::AmbienceMgr::maintain`'s per-tag channel
//! spawn/fade loop, adapted the same way `crate::music::playback` adapts
//! music playback: this crate's [`crate::AudioTracks`] has ONE `ambience`
//! mixer track (not one Kira sub-track per tag like the old client's
//! `AmbienceChannel`), so each tag gets its own retained
//! [`kira::sound::static_sound::StaticSoundHandle`] instead of its own
//! sub-track, and "fade to volume" is `StaticSoundHandle::set_volume` with a
//! tween instead of `TrackHandle::set_volume`.

use std::{collections::HashMap, time::Duration};

use bevy::{
    asset::{AssetServer, Assets, Handle},
    ecs::resource::Resource,
};
use kira::{
    Tween,
    sound::{EndPosition, PlaybackPosition, Region, static_sound::StaticSoundHandle},
};

use super::manifest::{AmbienceChannelTag, AmbienceCollection, AmbienceItem};
use crate::{AudioBackend, XindelerAudioAsset, sfx::dotted_key_to_ogg_path, to_decibels};

/// Caches `.ogg` asset handles by their resolved path — same rationale as
/// [`crate::sfx::SfxAssetCache`]/[`crate::music::MusicAssetCache`], kept as
/// its own type for the same "don't cross-pollute bookkeeping" reason.
#[derive(Resource, Default)]
pub struct AmbienceAssetCache(HashMap<String, Handle<XindelerAudioAsset>>);

/// One tag's live playback state: the retained handle (once spawned) and the
/// volume it's currently fading toward, so [`maintain_ambience_channel`]
/// only issues a NEW fade tween when the target actually changes (mirrors
/// the old code's `channel.fade_to(target_volume, 1.0)` being called every
/// `maintain` tick regardless — Kira's own tween machinery makes repeat
/// calls to the same target cheap/harmless, so this crate doesn't bother
/// gating on change either, keeping the port simple).
struct AmbienceChannelState {
    handle: StaticSoundHandle,
}

/// Per-tag ambience playback state (BL-82 EM-5.10c). Starts with no channels
/// spawned; [`maintain_ambience_channel`] lazily spawns one per tag the first
/// time its target volume is `> 0.0`, matching the old client's own
/// "spawn a channel for each tag" `maintain` loop (module doc comment).
#[derive(Resource, Default)]
pub struct AmbiencePlayerState {
    channels: HashMap<AmbienceChannelTag, AmbienceChannelState>,
}

const AMBIENCE_FADE_SECS: f32 = 1.0;

/// Ensures `tag`'s channel is playing (spawning it, looped, at silence, the
/// first time `target_volume > 0.0`) and fades it toward `target_volume`
/// this frame. A no-op when: the manifest has no entry for `tag`; the audio
/// backend is [`AudioBackend::Unavailable`]; or (on the very first call for
/// a tag going audible) the asset hasn't finished loading yet — same
/// documented v1 trade-off `crate::sfx::playback::trigger_sfx` already
/// carries.
pub fn maintain_ambience_channel(
    tag: AmbienceChannelTag,
    target_volume: f32,
    manifest: &AmbienceCollection,
    asset_server: &AssetServer,
    audio_assets: &Assets<XindelerAudioAsset>,
    cache: &mut AmbienceAssetCache,
    state: &mut AmbiencePlayerState,
    backend: &mut AudioBackend,
) {
    let Some(item) = manifest.get(tag) else {
        return;
    };

    if let Some(existing) = state.channels.get_mut(&tag) {
        existing
            .handle
            .set_volume(to_decibels(target_volume.max(0.0)), Tween {
                duration: Duration::from_secs_f32(AMBIENCE_FADE_SECS),
                ..Default::default()
            });
        return;
    }

    // Not spawned yet: only bother once the tag actually wants to be
    // audible — matching the old code's OWN "spawn at target 0.0, let the
    // fade bring it up" shape, but this port additionally waits for a
    // nonzero target before spawning at all, avoiding a silent channel sunk
    // for tags that never turn on in a given session (e.g. `Cave` outdoors).
    if target_volume <= 0.0 {
        return;
    }

    let Some(handle) = spawn_looping(item, asset_server, audio_assets, cache, backend) else {
        return;
    };
    state.channels.insert(tag, AmbienceChannelState { handle });
    // Immediately fade the freshly-spawned (silent) instance up to the
    // target — the SAME tween path as the "already spawned" branch, so the
    // very first frame a tag turns on also starts its fade-in instead of
    // snapping straight to volume.
    if let Some(spawned) = state.channels.get_mut(&tag) {
        spawned
            .handle
            .set_volume(to_decibels(target_volume.max(0.0)), Tween {
                duration: Duration::from_secs_f32(AMBIENCE_FADE_SECS),
                ..Default::default()
            });
    }
}

fn spawn_looping(
    item: &AmbienceItem,
    asset_server: &AssetServer,
    audio_assets: &Assets<XindelerAudioAsset>,
    cache: &mut AmbienceAssetCache,
    backend: &mut AudioBackend,
) -> Option<StaticSoundHandle> {
    let path = dotted_key_to_ogg_path(&item.path);
    let handle = cache
        .0
        .entry(path.clone())
        .or_insert_with(|| asset_server.load(path))
        .clone();
    let asset = audio_assets.get(&handle)?;
    let tracks = backend.tracks_mut()?;

    // Start silent (`Decibels::SILENCE`) — the caller's very next
    // `set_volume` call fades it up, mirroring the old client's own
    // `new_ambience_channel(tag)` (constructed at `init_volume: 0.0`) +
    // `AmbienceMgr::maintain`'s subsequent `fade_to`.
    let data = asset
        .0
        .clone()
        .volume(to_decibels(0.0))
        .loop_region(Region {
            start: PlaybackPosition::Samples(item.start),
            end: EndPosition::Custom(PlaybackPosition::Samples(item.end)),
        });
    tracks.ambience.play(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_volume_table_matches_the_old_client() {
        assert_eq!(AmbienceChannelTag::Wind.max_volume(), 1.0);
        assert_eq!(AmbienceChannelTag::Rain.max_volume(), 0.95);
        assert_eq!(AmbienceChannelTag::RiverLoud.max_volume(), 1.2);
    }
}
