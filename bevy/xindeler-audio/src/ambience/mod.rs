//! BL-82 EM-5.10c (T56.36) — the ambience vocabulary + manifest + per-tag
//! volume model + playback API this crate exposes; the client-state-reading
//! orchestrator (real `is_indoors` terrain query + weather signal, one call
//! to [`playback::maintain_ambience_channel`] per tag every frame) lives in
//! `xindeler-client::ambience`, mirroring the exact split `crate::sfx`/
//! `crate::music` already establish.

pub mod manifest;
pub mod playback;
pub mod volume;

pub use manifest::{AmbienceChannelTag, AmbienceCollection, AmbienceItem, AmbienceManifestLoader};
pub use playback::{AmbienceAssetCache, AmbiencePlayerState, maintain_ambience_channel};
pub use volume::{AmbienceInputs, tag_volume};

use bevy::{
    app::{App, Plugin, Startup},
    asset::{AssetApp, AssetServer, Handle},
    ecs::{
        resource::Resource,
        system::{Commands, Res},
    },
};

/// The loaded `ambience.ron` manifest handle — kept alive and read via
/// `Res<Assets<AmbienceCollection>>::get(&handle.0)`, same pattern
/// `crate::sfx::SfxManifestHandle`/`crate::music::SoundtrackManifestHandle`
/// establish.
#[derive(Resource, Clone)]
pub struct AmbienceManifestHandle(pub Handle<AmbienceCollection>);

fn load_ambience_manifest(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(AmbienceManifestHandle(
        asset_server.load("voxygen/audio/ambience.ron"),
    ));
}

/// Adds the `ambience.ron` `AssetLoader` + kicks off loading the real
/// manifest + the playback state/asset cache. Does NOT add any per-frame
/// system — that's `xindeler-client::ambience::AmbienceViewPlugin`.
#[derive(Default)]
pub struct AmbienceManifestPlugin;

impl Plugin for AmbienceManifestPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<AmbienceCollection>()
            .init_asset_loader::<AmbienceManifestLoader>()
            .init_resource::<AmbienceAssetCache>()
            .init_resource::<AmbiencePlayerState>()
            .add_systems(Startup, load_ambience_manifest);
    }
}
