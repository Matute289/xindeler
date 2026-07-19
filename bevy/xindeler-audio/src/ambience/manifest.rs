//! BL-82 EM-5.10c (T56.36) — the ambience vocabulary + the Bevy
//! `AssetLoader` for the FROZEN `assets/voxygen/audio/ambience.ron` manifest
//! (isolation law rule 3: never renamed/restructured, read verbatim).
//!
//! Ported 1:1 from `voxygen::audio::channel::AmbienceChannelTag` and
//! `voxygen::audio::ambience::{AmbienceItem, AmbienceCollection}` (voxygen is
//! no longer a workspace member — CLAUDE.md — so this is a genuine
//! re-implementation).

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    ecs::error::BevyError,
    reflect::TypePath,
};
use serde::Deserialize;
use strum::EnumIter;

/// Ported verbatim from `voxygen::audio::channel::AmbienceChannelTag`.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash, Deserialize, EnumIter)]
pub enum AmbienceChannelTag {
    Wind,
    Rain,
    ThunderRumbling,
    Leaves,
    Cave,
    Thunder,
    RiverLoud,
    RiverQuiet,
}

impl AmbienceChannelTag {
    /// Ported verbatim from `voxygen::audio::ambience`'s
    /// `impl AmbienceChannelTag { pub fn get_max_volume(&self) -> f32 }`.
    #[must_use]
    pub fn max_volume(self) -> f32 {
        match self {
            Self::Wind => 1.0,
            Self::Rain => 0.95,
            Self::ThunderRumbling => 1.33,
            Self::Leaves => 1.33,
            Self::Cave => 1.0,
            Self::Thunder => 1.0,
            Self::RiverLoud => 1.2,
            Self::RiverQuiet => 1.0,
        }
    }
}

/// One `ambience.ron` entry. `looping` is deliberately NOT a field here
/// (ported verbatim from the old client's own `AmbienceItem`, which also
/// lacks it — every shipped entry sets `looping: true` in the RON, but serde
/// silently ignores unknown fields on a struct without
/// `#[serde(deny_unknown_fields)]`, exactly like the old code; this crate's
/// own playback layer always loops ambience anyway, matching that reality).
#[derive(Debug, Deserialize, Clone)]
pub struct AmbienceItem {
    pub path: String,
    pub tag: AmbienceChannelTag,
    pub start: usize,
    pub end: usize,
}

#[derive(Asset, TypePath, Debug, Default, Clone, Deserialize)]
pub struct AmbienceCollection {
    pub tracks: Vec<AmbienceItem>,
}

impl AmbienceCollection {
    /// Looks up the (first) entry for `tag`, if the manifest has one —
    /// mirrors the old client's own
    /// `ambience_sounds.0.tracks.iter().find(|track| track.tag == tag)`.
    #[must_use]
    pub fn get(&self, tag: AmbienceChannelTag) -> Option<&AmbienceItem> {
        self.tracks.iter().find(|track| track.tag == tag)
    }
}

/// Async [`AssetLoader`] for `ambience.ron`, same style as
/// [`crate::sfx::manifest::SfxManifestLoader`]/
/// [`crate::music::manifest::SoundtrackManifestLoader`].
#[derive(Default, TypePath)]
pub struct AmbienceManifestLoader;

impl AssetLoader for AmbienceManifestLoader {
    type Asset = AmbienceCollection;
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

    /// The REAL, shipped, frozen `ambience.ron` must parse in full.
    #[test]
    fn shipped_ambience_manifest_parses_in_full() {
        let text = include_str!("../../../../assets/voxygen/audio/ambience.ron");
        let collection: AmbienceCollection =
            ron::from_str(text).expect("ambience.ron must parse with the ported vocabulary");
        assert_eq!(collection.tracks.len(), 7, "the real manifest has 7 tags");
        assert!(collection.get(AmbienceChannelTag::Rain).is_some());
        assert!(collection.get(AmbienceChannelTag::Wind).is_some());
    }
}
