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
//! ## Spatial positioning (EM-5.10d / T56.36b)
//! Sounds triggered through [`playback::trigger_sfx`] are now POSITIONAL:
//! [`SFX_DIST_LIMIT_SQR`] still hard-culls anything out of earshot, but within
//! that radius each sound is baked with real distance attenuation
//! ([`spatial::distance_attenuation`]) and stereo panning
//! ([`spatial::stereo_pan`]) relative to the [`spatial::AudioListener`], and
//! the whole `sfx` track is low-pass muffled while the listener is underwater
//! (see [`apply_sfx_muffle`] + `crate::manager::AudioBackend::set_sfx_muffle`).

pub mod event;
pub mod manifest;
pub mod playback;
pub mod spatial;

pub use event::{SfxEvent, SfxInventoryEvent, VoiceKind, body_to_voice};
pub use manifest::{SfxManifest, SfxManifestLoader, SfxTriggerItem};
pub use playback::{SfxAssetCache, dotted_key_to_ogg_path, trigger_sfx};
pub use spatial::{AudioListener, SFX_MIN_DISTANCE, distance_attenuation, stereo_pan};

use bevy::{
    app::{App, Plugin, Startup, Update},
    asset::{AssetApp, AssetServer, Handle},
    ecs::{
        resource::Resource,
        system::{Commands, Local, Res, ResMut},
    },
};

use crate::AudioBackend;

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

/// BL-82 EM-5.10d: drives the `sfx` track's low-pass filter from the listener's
/// underwater state. Only touches Kira when the state actually flips (tracked
/// in a `Local`), so the common above-water frame is a pure comparison — no
/// per-frame command write. The `None` initial value guarantees the correct
/// cutoff is applied on the very first frame regardless of the (default:
/// un-muffled) starting state.
fn apply_sfx_muffle(
    listener: Res<AudioListener>,
    mut backend: ResMut<AudioBackend>,
    mut last_underwater: Local<Option<bool>>,
) {
    if *last_underwater == Some(listener.underwater) {
        return;
    }
    *last_underwater = Some(listener.underwater);
    backend.set_sfx_muffle(listener.underwater);
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
            // BL-82 EM-5.10d: the listener state is always present (default =
            // origin, un-muffled) so `trigger_sfx` and `apply_sfx_muffle` can
            // read it every frame; the client overwrites it each frame from the
            // real camera + player state.
            .init_resource::<AudioListener>()
            .add_systems(Startup, load_sfx_manifest)
            .add_systems(Update, apply_sfx_muffle);
    }
}
