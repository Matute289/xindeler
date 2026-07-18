//! BL-82 EM-5.10b (T56.35) — the Bevy `AssetLoader` for the frozen
//! `assets/voxygen/audio/sfx.ron` manifest (isolation law rule 3: this asset
//! is frozen, never renamed/restructured — the loader reads it verbatim).
//!
//! Follows the exact async-`AssetLoader` house style
//! `xindeler-render-voxel::palette::BlockPaletteLoader` already established
//! (`ron::de::from_bytes` over the raw bytes, wrapped as a Bevy [`Asset`]) —
//! see that module for the precedent this task's brief pointed at.

use std::collections::HashMap;

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    ecs::error::BevyError,
    reflect::TypePath,
};
use serde::Deserialize;

use super::event::SfxEvent;

/// One `sfx.ron` entry: which file(s) to play for an [`SfxEvent`] and the
/// re-trigger threshold. Ported verbatim from the old client's own
/// `voxygen::audio::sfx::SfxTriggerItem`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct SfxTriggerItem {
    /// Asset paths (dotted, no extension — e.g.
    /// `"voxygen.audio.sfx.footsteps.stepgrass_1"`), minus the file
    /// extension. One is chosen at random when more than one is listed.
    pub files: Vec<String>,
    /// Seconds (or, for a handful of movement events, metres travelled) to
    /// wait before this event may re-trigger.
    pub threshold: f32,
    /// Accessibility subtitle key (EM-5.16's job to render; parsed here so
    /// the manifest round-trips, unused by playback itself).
    #[serde(default)]
    pub subtitle: Option<String>,
}

/// The parsed `sfx.ron` manifest: every [`SfxEvent`] this build knows a
/// sound for, keyed exactly as the RON file names them.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct SfxManifest(pub HashMap<SfxEvent, SfxTriggerItem>);

impl SfxManifest {
    /// Looks up the trigger item for `event`, if the manifest has one —
    /// mirrors the old client's own `triggers.0.get_key_value(&event)`
    /// lookup shape (a manifest with no entry for an event is a normal,
    /// silent no-op, never an error).
    #[must_use]
    pub fn get(&self, event: &SfxEvent) -> Option<&SfxTriggerItem> { self.0.get(event) }
}

/// Async [`AssetLoader`] for `sfx.ron`, registered for the plain `ron`
/// extension (typed load required, same disambiguation note
/// `BlockPaletteLoader` documents: bevy 0.19 picks the loader by the
/// REQUESTED asset type, not just the extension).
#[derive(Default, TypePath)]
pub struct SfxManifestLoader;

impl AssetLoader for SfxManifestLoader {
    type Asset = SfxManifest;
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
        let map: HashMap<SfxEvent, SfxTriggerItem> = ron::de::from_bytes(&bytes)?;
        Ok(SfxManifest(map))
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The REAL, shipped, frozen manifest must parse in full — the strongest
    /// available guard that every `SfxEvent`/`VoiceKind`/`SfxInventoryEvent`
    /// variant this crate ported actually matches what the RON file uses.
    #[test]
    fn shipped_sfx_manifest_parses_in_full() {
        let text = include_str!("../../../../assets/voxygen/audio/sfx.ron");
        let map: HashMap<SfxEvent, SfxTriggerItem> =
            ron::from_str(text).expect("sfx.ron must parse with the ported SfxEvent vocabulary");
        assert!(
            map.len() > 100,
            "sanity: the real manifest has well over 100 entries, got {}",
            map.len()
        );

        // Spot-check a few entries the client-side mappers actually depend
        // on (movement footsteps + a melee attack + campfire ambience).
        assert!(map.contains_key(&SfxEvent::Run(common::terrain::BlockKind::Grass)));
        assert!(map.contains_key(&SfxEvent::Campfire));
        let sword_attack = SfxEvent::Attack(
            common::comp::CharacterAbilityType::BasicMelee(
                common::states::utils::StageSection::Action,
            ),
            common::comp::inventory::item::tool::ToolKind::Sword,
        );
        assert!(
            map.contains_key(&sword_attack),
            "the manifest must carry a BasicMelee/Sword attack trigger"
        );
    }
}
