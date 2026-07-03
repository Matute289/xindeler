//! User settings: RON file in the userdata dir, loaded at boot (EM-2.1).
//!
//! Resolution order for the userdata dir: `XINDELER_USERDATA` env var, else
//! `./userdata`.
//!
//! TODO(BL-82): once the dependency story is settled, resolve userdata via
//! `common-base` instead, so the old and new stacks agree on the location.
//! For now we deliberately do NOT depend on `common-base` — it would drag sim
//! crates into the pure-Bevy client (isolation law).

use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Env var overriding where settings (and future userdata) live.
pub const USERDATA_ENV: &str = "XINDELER_USERDATA";
const SETTINGS_FILE: &str = "settings.ron";

/// Root of all user-local data (settings, logs, screenshots, ...).
#[must_use]
pub fn userdata_dir() -> PathBuf {
    std::env::var_os(USERDATA_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./userdata"))
}

/// All persisted user settings. Serialized as RON at
/// `<userdata>/settings.ron`; unknown/missing fields fall back to defaults so
/// old files keep loading as the struct grows.
#[derive(Resource, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct XindelerSettings {
    pub graphics: GraphicsSettings,
}

/// Graphics toggles consumed by the client when building the camera and
/// light rigs (EM-2.2 / EM-2.3). Defaults = everything on, 4 shadow cascades.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    /// Temporal anti-aliasing (requires `Msaa::Off`; our fix for distant
    /// block flicker).
    pub taa: bool,
    /// Screen-space ambient occlusion.
    pub ssao: bool,
    /// HDR bloom (NATURAL preset).
    pub bloom: bool,
    /// Volumetric fog / light shafts on the camera.
    pub volumetric_fog: bool,
    /// Screen-space contact shadows (camera + per-light flag).
    pub contact_shadows: bool,
    /// Number of directional-light shadow cascades (clamped to >= 1).
    /// Number of shadow cascades. Effective range 1..=4 (clamped at the light
    /// rig).
    pub shadow_cascades: u8,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            taa: true,
            ssao: true,
            bloom: true,
            volumetric_fog: true,
            contact_shadows: true,
            shadow_cascades: 4,
        }
    }
}

impl XindelerSettings {
    /// Path of the settings file inside [`userdata_dir`].
    #[must_use]
    pub fn path() -> PathBuf { userdata_dir().join(SETTINGS_FILE) }

    /// Loads settings from disk, falling back to defaults (and writing them
    /// out, best-effort) when the file is missing or unparseable.
    #[must_use]
    pub fn load_or_default() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match ron::from_str(&text) {
                Ok(settings) => {
                    info!("Loaded settings from {}", path.display());
                    settings
                },
                Err(err) => {
                    warn!(
                        "Failed to parse {} ({err}); using default settings",
                        path.display()
                    );
                    Self::default()
                },
            },
            Err(_) => {
                let settings = Self::default();
                match settings.save() {
                    Ok(()) => info!("Wrote default settings to {}", path.display()),
                    Err(err) => warn!("Could not write default settings ({err})"),
                }
                settings
            },
        }
    }

    /// Serializes the settings to `<userdata>/settings.ron`, creating the
    /// directory if needed. (Save-on-change is deliberately not wired yet —
    /// v0 saves only when creating the default file; a settings UI will call
    /// this explicitly.)
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        std::fs::write(&path, text)
    }
}
