use crate::assets::{self, AssetExt, Ron};
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use super::item_key::ItemKey;

/// How to render a single item's icon: a flat PNG, an untransformed voxel
/// model, or a voxel model with a hand-tuned camera offset/rotation/zoom.
/// One entry per catalogue item, keyed by [`ItemKey`] in
/// `voxygen/item_image_manifest.ron`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImageSpec {
    Png(String),
    Vox(
        String,
        #[serde(default)] u32,
        #[serde(default)] Option<[u8; 3]>,
    ),
    // (specifier, offset, (x_rot, y_rot, z_rot) in degrees, zoom, model_index, color)
    VoxTrans(
        String,
        [f32; 3],
        [f32; 3],
        f32,
        #[serde(default)] u32,
        #[serde(default)] Option<[u8; 3]>,
    ),
}

impl ImageSpec {
    /// The underlying asset specifier this variant renders from — a `.png`
    /// path for [`ImageSpec::Png`], a `.vox` path for the other two. Manifest
    /// entries are written relative to `assets/voxygen/` (e.g.
    /// `"voxel.sprite.crafting_station.anvil"`
    /// → `assets/voxygen/voxel/sprite/crafting_station/anvil.vox`), so
    /// callers load the `"voxygen."`-prefixed form, not this raw value.
    pub fn specifier(&self) -> &str {
        match self {
            ImageSpec::Png(specifier)
            | ImageSpec::Vox(specifier, ..)
            | ImageSpec::VoxTrans(specifier, ..) => specifier,
        }
    }

    /// [`Self::specifier`] with the `voxygen.` asset-root prefix applied —
    /// the actual string to pass to `Image::load`/`DotVox::load`.
    pub fn full_specifier(&self) -> String { ["voxygen.", self.specifier()].concat() }
}

/// The whole `voxygen/item_image_manifest.ron` catalogue, ~1400 hand-tuned
/// entries mapping every item to its icon render spec.
pub type ItemImageManifest = HashMap<ItemKey, ImageSpec>;

/// Load the manifest asset.
pub fn load_manifest() -> Result<ItemImageManifest, assets::Error> {
    Ron::<ItemImageManifest>::load_owned("voxygen.item_image_manifest").map(|ron| ron.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{DotVox, Image};

    /// Every manifest entry must deserialize, and its underlying asset
    /// specifier must resolve to a real file — a `.png`/`.jpg` for
    /// [`ImageSpec::Png`], a `.vox` for [`ImageSpec::Vox`]/
    /// [`ImageSpec::VoxTrans`]. Catches drift between the manifest and the
    /// (frozen) asset tree before any rendering work depends on it.
    #[test]
    fn every_manifest_entry_resolves_to_a_real_asset() {
        let manifest = load_manifest().expect("voxygen.item_image_manifest.ron must load");
        assert!(
            manifest.len() > 1000,
            "expected the full ~1400-entry catalogue, got {}",
            manifest.len()
        );

        let mut missing = Vec::new();
        for (key, spec) in &manifest {
            let full_specifier = spec.full_specifier();
            let resolves = match spec {
                ImageSpec::Png(..) => Image::load(&full_specifier).is_ok(),
                ImageSpec::Vox(..) | ImageSpec::VoxTrans(..) => {
                    DotVox::load(&full_specifier).is_ok()
                },
            };
            if !resolves {
                missing.push(format!("{key:?} -> {full_specifier}"));
            }
        }

        assert!(
            missing.is_empty(),
            "{} manifest entries reference a missing asset:\n{}",
            missing.len(),
            missing.join("\n")
        );
    }
}
