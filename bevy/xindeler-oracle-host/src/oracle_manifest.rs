//! BL-82 EM-4.9 follow-up (comprehensive-review Finding 1, data-driven-content
//! cleanup): which ORACLE `.dmevent.ron` files the host proactively requests
//! by name is now DATA
//! (`assets/xindeler/oracle_events/manifest.oracle_manifest.ron`),
//! not a compiled-in Rust list.
//!
//! ## Why this exists at all — the "outstanding handle" requirement
//! `xindeler_sim_bridge::oracle`'s own doc comments
//! (`request_well_known_events_from_manifest`) explain the underlying
//! constraint this manifest serves: `bevy_asset`'s file watcher only reloads
//! paths that already have an OUTSTANDING handle — a brand-new,
//! never-requested path dropped into the watched `oracle://` directory is
//! NOT auto-discovered merely by watching the directory. So the host must
//! know, ahead of time, the names of every canonical event it should
//! pre-request a handle for. Before this task that list
//! (`WELL_KNOWN_EVENT_FILENAMES`) was a compiled-in `&[&str]` constant —
//! shipping a second canonical event required a Rust code change + recompile,
//! against this project's established data-driven-content convention (see
//! `xindeler_dimensions::predictive_gc`'s `PredictiveGcAsset`/T48.6 for the
//! precedent this module mirrors almost exactly, minus the interpolation —
//! a manifest reload just adds newly-named entries, there is nothing to
//! animate).
//!
//! A general "scan the whole `oracle://` directory for any file" watcher
//! remains the nicer v2 (out of scope here, same deferred-scope note
//! `xindeler_sim_bridge::oracle`'s own module doc already carries for
//! `WELL_KNOWN_EVENT_FILENAMES`) — this manifest only removes the
//! code-vs-data mismatch for the EXPLICIT-list v1 approach, it does not
//! replace it with directory scanning.
//!
//! Split of responsibilities (mirrors every other asset type in this crate):
//! this module owns the [`OracleEventManifest`] asset type + its
//! [`AssetLoader`]; `xindeler_sim_bridge::oracle` (the crate that already
//! depends on both `xindeler-oracle-host` and Bevy's asset system) owns the
//! Bevy systems that request the manifest, react to it loading/reloading, and
//! turn its `event_filenames` into individual `DmEvent` handle requests.

use bevy::{
    app::{App, Plugin},
    asset::{Asset, AssetApp, AssetLoader, LoadContext, io::Reader},
    ecs::error::BevyError,
    reflect::TypePath,
};
use serde::{Deserialize, Serialize};

/// Canonical asset path (relative to the `assets/` source root) of the
/// shipped manifest naming every canonical ORACLE event the host proactively
/// requests a handle for at boot. Mirrors
/// `xindeler_dimensions::predictive_gc::DEFAULT_CONFIG_ASSET_PATH`'s own
/// "shipped default config asset path" convention exactly.
pub const DEFAULT_MANIFEST_ASSET_PATH: &str = "xindeler/oracle_events/manifest.oracle_manifest.ron";

/// The set of canonical `.dmevent.ron`/`.dmevent.json` filenames (bare, not
/// `oracle://`-prefixed — the caller builds the asset-server load path and
/// the on-disk retirement-poll path from each entry, exactly like
/// `xindeler_sim_bridge::oracle`'s own retired `WELL_KNOWN_EVENT_FILENAMES`
/// constant did) the host knows about by name.
///
/// `#[serde(default)]` (empty list) so a missing/partial manifest file degrades
/// to "no well-known events pre-requested" rather than failing to load —
/// same anti-chaos posture as every other RON asset in this crate.
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OracleEventManifest {
    pub event_filenames: Vec<String>,
}

/// Async [`AssetLoader`] for `*.oracle_manifest.ron` (mirrors
/// `AtmosphereProfileLoader`/`PredictiveGcLoader`'s shape exactly — this
/// manifest has no `sanitize()` pass of its own: a filename is just an
/// opaque string handed straight to `AssetServer::load`/`Path::join`, nothing
/// numeric to clamp).
#[derive(Default, TypePath)]
pub struct OracleEventManifestLoader;

impl AssetLoader for OracleEventManifestLoader {
    type Asset = OracleEventManifest;
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
        let manifest: OracleEventManifest = ron::de::from_bytes(&bytes)?;
        Ok(manifest)
    }

    fn extensions(&self) -> &[&str] { &["oracle_manifest.ron"] }
}

/// Registers the [`OracleEventManifest`] asset + [`OracleEventManifestLoader`].
/// Requires `AssetPlugin` (part of `DefaultPlugins`/`MinimalPlugins`) to
/// already be present — same contract every other `init_asset` caller in this
/// crate documents (e.g. [`crate::dm_event::DmEventPlugin`]).
pub struct OracleEventManifestPlugin;

impl Plugin for OracleEventManifestPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<OracleEventManifest>()
            .init_asset_loader::<OracleEventManifestLoader>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `manifest.oracle_manifest.ron` parses and names EXACTLY
    /// the one canonical event this repo ships today (`mist_bound.dmevent.ron`)
    /// — mirrors `dm_event::tests::shipped_mist_bound_dmevent_parses_and_is_already_sane`'s
    /// "the checked-in fixture, not just the type default" rigor.
    #[test]
    fn shipped_manifest_parses_and_names_the_mist_bound_event() {
        let text =
            include_str!("../../../assets/xindeler/oracle_events/manifest.oracle_manifest.ron");
        let parsed: OracleEventManifest =
            ron::from_str(text).expect("manifest.oracle_manifest.ron parses");
        assert_eq!(parsed.event_filenames, vec![
            "mist_bound.dmevent.ron".to_owned()
        ]);
    }

    #[test]
    fn missing_fields_default_to_an_empty_list() {
        let parsed: OracleEventManifest = ron::from_str("()").expect("empty RON parses");
        assert_eq!(parsed, OracleEventManifest::default());
        assert!(parsed.event_filenames.is_empty());
    }
}
