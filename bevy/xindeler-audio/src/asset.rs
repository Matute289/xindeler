//! The `.ogg`/`.wav` [`AssetLoader`], decoding straight into Kira's own
//! [`StaticSoundData`] wrapped as a Bevy [`Asset`].

use std::io::Cursor;

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    ecs::error::BevyError,
    reflect::TypePath,
};
use kira::sound::static_sound::StaticSoundData;

/// A decoded audio clip, ready to `track.play(asset.0.clone())`.
///
/// Wraps Kira's [`StaticSoundData`], which is cheap to clone (an `Arc<[Frame]>`
/// of already-decoded samples shared across clones) — the same "decode once,
/// play many times" model `voxygen`'s own `soundcache.rs` implemented by hand
/// (an `AssetHandle`-keyed cache), now expressed as an ordinary Bevy `Asset`
/// handle instead of a bespoke cache.
#[derive(Asset, TypePath, Clone)]
pub struct XindelerAudioAsset(pub StaticSoundData);

/// Loads `.ogg` and `.wav` files into a [`XindelerAudioAsset`].
///
/// `.wav` needs `kira`'s `wav` feature (the Symphonia demuxer) AND `pcm`
/// (the codec) — verified empirically: `wav` alone still fails decode of a
/// real WAV fixture with a `symphonia` "unsupported audio codec" error. See
/// `Cargo.toml`'s dependency comment for the full reasoning; the crate root
/// doc comment notes this as a deliberate deviation from `voxygen`'s original
/// (`.ogg`-only) `kira` feature pin, since this task explicitly asks for both
/// extensions.
#[derive(Default, TypePath)]
pub struct XindelerAudioAssetLoader;

impl AssetLoader for XindelerAudioAssetLoader {
    type Asset = XindelerAudioAsset;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let data = StaticSoundData::from_cursor(Cursor::new(bytes))
            .map_err(|err| BevyError::from(std::io::Error::other(err.to_string())))?;
        Ok(XindelerAudioAsset(data))
    }

    fn extensions(&self) -> &[&str] { &["ogg", "wav"] }
}
