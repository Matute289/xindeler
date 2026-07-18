//! BL-82 EM-5.10b (T56.35) — the SFX vocabulary + manifest + playback API
//! this crate exposes; the event-MAPPER systems (movement/combat/campfire/
//! block/vehicle + `handle_outcome`) that decide WHEN to trigger which
//! [`SfxEvent`] off real mirrored client state live in
//! `xindeler-client::sfx` (they need `xindeler-protocol`'s `Net*` types,
//! which this crate deliberately does not depend on — this crate stays the
//! generic audio-domain vocabulary + playback primitive, matching the split
//! `xindeler-render-voxel` (generic voxel meshing) draws against
//! `xindeler-client` (the game-state-aware caller)).
//!
//! ## Scope boundary vs EM-5.10d (spatial + muffling)
//! Sounds triggered through [`playback::trigger_sfx`] play FLAT (no spatial
//! panning/attenuation) — only [`SFX_DIST_LIMIT_SQR`] gates whether a sound
//! plays AT ALL (a hard cull, matching the old client's own
//! `audio::channel::SFX_DIST_LIMIT`). Real 3D positioning (Kira spatial
//! tracks + listener + distance-based volume/pan) is EM-5.10d/T56.36b's job
//! — not fabricated here ahead of that task.

pub mod event;
pub mod manifest;
pub mod playback;

pub use event::{SfxEvent, SfxInventoryEvent, VoiceKind, body_to_voice};
pub use manifest::{SfxManifest, SfxManifestLoader, SfxTriggerItem};
pub use playback::{SfxAssetCache, dotted_key_to_ogg_path, trigger_sfx};

use bevy::{
    app::{App, Plugin, Startup},
    asset::{AssetApp, AssetServer, Handle},
    ecs::{
        resource::Resource,
        system::{Commands, Res},
    },
};

/// Sounds farther than this from the listener never play at all (a hard
/// cull, not an attenuation curve) — ported verbatim from the old client's
/// `voxygen::audio::channel::SFX_DIST_LIMIT`.
pub const SFX_DIST_LIMIT: f32 = 256.0;
/// Squared form, matching every distance-cull call site's own preference
/// for avoiding a `sqrt` (same convention the old client's mappers used).
pub const SFX_DIST_LIMIT_SQR: f32 = SFX_DIST_LIMIT * SFX_DIST_LIMIT;

/// The loaded `sfx.ron` manifest handle — kept alive (a dropped `Handle`
/// unloads the asset) and read by every event-mapper system via
/// `Res<Assets<SfxManifest>>::get(&handle.0)`.
#[derive(Resource, Clone)]
pub struct SfxManifestHandle(pub Handle<SfxManifest>);

fn load_sfx_manifest(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(SfxManifestHandle(
        asset_server.load("voxygen/audio/sfx.ron"),
    ));
}

/// Adds the `sfx.ron` `AssetLoader` + kicks off loading the real manifest +
/// the playback asset cache. Does NOT add any event-mapper systems (those
/// live in `xindeler-client::sfx::SfxViewPlugin`, added alongside this).
#[derive(Default)]
pub struct SfxManifestPlugin;

impl Plugin for SfxManifestPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SfxManifest>()
            .init_asset_loader::<SfxManifestLoader>()
            .init_resource::<SfxAssetCache>()
            .add_systems(Startup, load_sfx_manifest);
    }
}
