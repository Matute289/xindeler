//! BL-82 EM-5.10b (T56.35) — resolves a manifest [`SfxTriggerItem`] to a real
//! loaded `.ogg` asset and plays it through the `sfx` mixer track, via the
//! EXACT [`AudioBackend`]/`TrackHandle::play` API PR #174 (T56.34)
//! established — not a parallel playback path.

use std::collections::HashMap;

use bevy::{
    asset::{AssetServer, Assets, Handle},
    ecs::resource::Resource,
};
use rand::seq::IndexedRandom;

use super::{event::SfxEvent, manifest::SfxManifest};
use crate::{AudioBackend, XindelerAudioAsset, to_decibels};

/// Converts an `sfx.ron` dotted asset key (no extension, e.g.
/// `"voxygen.audio.sfx.footsteps.stepgrass_1"`) into the real `.ogg` asset
/// path `AssetServer` resolves relative to the asset root — the SAME
/// `dotted.replace('.', "/")` + extension convention
/// `xindeler-client::figure_view::asset_path` already established for
/// figure/armour manifests (isolation law rule 3: only the separator/
/// extension are ever translated, never the name itself). Every sfx asset
/// under `assets/voxygen/audio/sfx/` is `.ogg` (verified: zero `.wav` files
/// in that tree), so the extension is hardcoded, unlike the generic
/// two-extension `XindelerAudioAssetLoader`.
#[must_use]
pub fn dotted_key_to_ogg_path(dotted: &str) -> String {
    format!("{}.ogg", dotted.replace('.', "/"))
}

/// Caches `.ogg` asset handles by their resolved path so repeated triggers of
/// the same sound don't re-issue an `AssetServer::load` (which would be
/// harmless — bevy's own asset server dedups identical-path loads — but this
/// avoids the redundant `HashMap`/path-format churn on a per-frame-frequent
/// hot path like footsteps).
#[derive(Resource, Default)]
pub struct SfxAssetCache(HashMap<String, Handle<XindelerAudioAsset>>);

impl SfxAssetCache {
    /// Seeds the cache with an already-obtained handle for `path` (the real
    /// `.ogg` asset path, e.g. from [`dotted_key_to_ogg_path`]) — lets a
    /// caller warm up a critical sound ahead of the first real trigger (a
    /// loading-screen prefetch, or a test that wants to avoid racing the
    /// async first load; see `xindeler-client::sfx`'s own integration test).
    /// A plain overwrite: re-seeding an already-cached path just replaces the
    /// handle (harmless — both point at the same underlying asset once
    /// loaded).
    pub fn insert_preloaded(
        &mut self,
        path: impl Into<String>,
        handle: Handle<XindelerAudioAsset>,
    ) {
        self.0.insert(path.into(), handle);
    }
}

/// Looks up `event` in `manifest`, resolves + (lazily loads) the chosen
/// audio asset, and plays it through the `sfx` track at `volume_scale` (a
/// linear amplitude multiplier — the SAME "extra volume" the old client's
/// `AudioFrontend::emit_sfx(trigger, pos, Some(volume))` took, e.g. `1.5`/
/// `2.0` for a louder one-off; `1.0` for "no adjustment"). Track-level/master
/// volume (`AudioVolumes`) is NOT re-applied here — it already lives on the
/// `sfx` `TrackHandle` itself via [`crate::volume::apply_audio_volumes`], so
/// double-applying it here would double-attenuate.
///
/// Returns `true` iff a sound actually started playing — the caller uses
/// this to update its own per-entity "last played" bookkeeping (mirrors the
/// old client's own `should_emit` → `emit_sfx` → `internal_state.time =
/// Instant::now()` sequencing: only a REAL play resets the cooldown clock).
/// Returns `false`, harmlessly, when: the manifest has no entry for `event`
/// (a normal "no sound authored for this yet" case, not an error); the
/// trigger item's `files` list is empty; the audio backend is
/// [`AudioBackend::Unavailable`] (no `cpal` device); or the chosen asset
/// hasn't finished its (async) first load yet — a real, documented v1
/// trade-off: the very FIRST time a given sound is triggered in a session it
/// may be silently skipped for the one/two frames the load takes, exactly
/// like any other lazily-`AssetServer::load`ed resource in this client.
pub fn trigger_sfx(
    manifest: &SfxManifest,
    event: &SfxEvent,
    volume_scale: f32,
    asset_server: &AssetServer,
    audio_assets: &Assets<XindelerAudioAsset>,
    cache: &mut SfxAssetCache,
    backend: &mut AudioBackend,
) -> bool {
    let Some(item) = manifest.get(event) else {
        return false;
    };
    let Some(dotted) = item.files.choose(&mut rand::rng()) else {
        return false;
    };
    let path = dotted_key_to_ogg_path(dotted);
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
    let sound = asset.0.clone().volume(to_decibels(volume_scale.max(0.0)));
    tracks.sfx.play(sound).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_key_converts_to_the_real_ogg_path() {
        assert_eq!(
            dotted_key_to_ogg_path("voxygen.audio.sfx.footsteps.stepgrass_1"),
            "voxygen/audio/sfx/footsteps/stepgrass_1.ogg"
        );
    }
}
