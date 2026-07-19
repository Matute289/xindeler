//! BL-82 EM-5.10c (T56.36): the music vocabulary, manifest, state machine
//! and playback API this crate exposes. The client-state-reading
//! orchestrator (gathering real nearby-hostile/weather/day-period signals
//! and driving [`state::MusicMachine::advance`] every frame) lives in
//! `xindeler-client::music` (it needs `xindeler-protocol`'s `Net*` types and
//! `xindeler-oracle-host`'s `AtmosphereController`, which this crate
//! deliberately does not depend on), matching the exact split `crate::sfx`
//! already establishes between this crate's generic audio-domain vocabulary
//! and `xindeler-client`'s game-state-aware mapper.

pub mod manifest;
pub mod playback;
pub mod state;

pub use manifest::{
    CombatIntensity, DayPeriod, MusicActivity, MusicChannelTag, MusicState,
    MusicTransitionManifest, MusicTransitionManifestLoader, SoundtrackCollection, SoundtrackItem,
    SoundtrackManifestLoader,
};
pub use playback::{MusicAssetCache, MusicPlayerState, crossfade};
pub use state::{MusicAction, MusicInputs, MusicMachine, TrackToPlay, day_period_for_hour};

use bevy::{
    app::{App, Plugin, Startup},
    asset::{AssetApp, AssetServer, Handle},
    ecs::{
        resource::Resource,
        system::{Commands, Res},
    },
};

/// The loaded `soundtrack.ron` manifest handle — kept alive (a dropped
/// `Handle` unloads the asset) and read via
/// `Res<Assets<SoundtrackCollection>>::get(&handle.0)`, same pattern
/// `crate::sfx::SfxManifestHandle` establishes.
#[derive(Resource, Clone)]
pub struct SoundtrackManifestHandle(pub Handle<SoundtrackCollection>);

/// The loaded `music_transition_manifest.ron` handle.
#[derive(Resource, Clone)]
pub struct MusicTransitionManifestHandle(pub Handle<MusicTransitionManifest>);

fn load_music_manifests(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(SoundtrackManifestHandle(
        asset_server.load("voxygen/audio/soundtrack.ron"),
    ));
    commands.insert_resource(MusicTransitionManifestHandle(
        asset_server.load("voxygen/audio/music_transition_manifest.ron"),
    ));
}

/// Adds the `soundtrack.ron`/`music_transition_manifest.ron` `AssetLoader`s +
/// kicks off loading the real manifests + the playback state/asset cache.
/// Does NOT add any per-frame system (the state machine needs real mirrored
/// client state to drive it — that's `xindeler-client::music::MusicViewPlugin`,
/// added alongside this).
#[derive(Default)]
pub struct MusicManifestPlugin;

impl Plugin for MusicManifestPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SoundtrackCollection>()
            .init_asset_loader::<SoundtrackManifestLoader>()
            .init_asset::<MusicTransitionManifest>()
            .init_asset_loader::<MusicTransitionManifestLoader>()
            .init_resource::<MusicAssetCache>()
            .init_resource::<MusicPlayerState>()
            .add_systems(Startup, load_music_manifests);
    }
}
