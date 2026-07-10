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

/// Graphics quality tier (EM-2.5). Every tier except [`Custom`] is a preset
/// that overwrites the individual [`GraphicsSettings`] toggles at load time
/// (see [`GraphicsSettings::sanitize`]) — the settings UI (EM-5.12) will
/// offer the tiers first and flip to `Custom` when a toggle is hand-edited.
///
/// [`Custom`]: GraphicsTier::Custom
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GraphicsTier {
    Low,
    Medium,
    High,
    #[default]
    Ultra,
    /// The individual toggles in `settings.ron` are authoritative; no preset
    /// is applied.
    Custom,
}

impl GraphicsTier {
    /// Serde default for the `tier` FIELD: files predating EM-2.5 (no `tier`
    /// key) keep their hand-edited toggles authoritative.
    #[must_use]
    pub fn for_old_files() -> Self { Self::Custom }

    /// The toggle values this tier stands for (`None` for [`Self::Custom`]),
    /// as `(taa, ssao, bloom, volumetric_fog, contact_shadows,
    /// shadow_cascades)`.
    ///
    /// `contact_shadows` is `false` for EVERY preset (BL-82 EM-3.11q — see
    /// [`GraphicsSettings::contact_shadows`]'s doc for why); a settings.ron
    /// can still hand-enable it under `tier: Custom`.
    #[must_use]
    pub fn preset(self) -> Option<(bool, bool, bool, bool, bool, u8)> {
        match self {
            Self::Low => Some((false, false, false, false, false, 1)),
            Self::Medium => Some((true, false, true, false, false, 2)),
            Self::High => Some((true, true, true, true, false, 3)),
            Self::Ultra => Some((true, true, true, true, false, 4)),
            Self::Custom => None,
        }
    }
}

/// Experimental renderer features reserved for a future opt-in path
/// (spec §4.5). **Both flags are always `false` today**: Solari (real-time
/// ray tracing) and DLSS are experimental Bevy 0.19 features that require
/// Vulkan + an NVIDIA RTX GPU — they are never part of the baseline renderer
/// and nothing in the client reads them yet. [`GraphicsSettings::sanitize`]
/// forces them back to `false` (with a warning) if a settings.ron sets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentalGraphics {
    /// Bevy Solari ray-traced lighting. Vulkan + RTX only; not wired.
    pub solari: bool,
    /// NVIDIA DLSS upscaling. Vulkan + RTX only; not wired.
    pub dlss: bool,
}

/// Graphics toggles consumed by the client when building the camera and
/// light rigs (EM-2.2 / EM-2.3). Defaults = the [`GraphicsTier::Ultra`]
/// preset (4 shadow cascades, everything else on EXCEPT `contact_shadows` —
/// see its field doc, BL-82 EM-3.11q) + vignette.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    /// Quality preset; overwrites the toggles below unless `Custom` (EM-2.5).
    ///
    /// The DESERIALIZATION default is `Custom`, not the struct default
    /// `Ultra`: a pre-EM-2.5 settings.ron carries hand-picked toggles but no
    /// `tier` field, and defaulting it to `Ultra` would let `sanitize`
    /// clobber them. New files (written from `Default`) still say `Ultra`.
    #[serde(default = "GraphicsTier::for_old_files")]
    pub tier: GraphicsTier,
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
    ///
    /// **Defaults to `false` in EVERY tier, including Ultra** (BL-82
    /// EM-3.11q — `docs/design/specs/2026-07-09-bl82-em311-findings-log.md`).
    /// Matías reported distant flower/grass sprite shadows/edges appearing to
    /// flicker; an A/B offscreen-capture harness with the camera AND sun
    /// both frozen (so only per-frame rendering noise could differ between
    /// otherwise-identical captures) measured real, sizeable pixel variance
    /// concentrated on sprite silhouettes and lit grass, and toggling this
    /// flag off cut that variance ~5x (p99) — by far the largest single
    /// contributor found (SSAO's independent contribution was small and
    /// fully subsumed once this was off). Root cause: Bevy's
    /// `bevy_pbr::contact_shadows::ContactShadows` is a screen-space,
    /// per-pixel dithered ray march with a fixed, small `length` (world-space
    /// metres, default 0.3) meant to add FINE contact-point detail beyond
    /// what cascaded shadow maps resolve — it has no distance falloff/cutoff,
    /// so it keeps evaluating (and dithering, relying on TAA to average out
    /// over time) on tiny/thin sprite silhouettes at any range, where a
    /// 0.3 m ray is comparatively huge relative to the object's on-screen
    /// footprint and the dithered result never fully converges. The
    /// cascaded shadow maps (still enabled, still up to 4 cascades) keep
    /// providing real, correctly-scaled shadows; this only removes the
    /// small-scale screen-space ADD-ON, which was providing negligible
    /// visual benefit at range while causing a confirmed, reproducible
    /// flicker. Re-enable (`tier: Custom`, `contact_shadows: true`) once
    /// Bevy exposes a distance falloff/cutoff for it, or once Xindeler adds
    /// its own (e.g. gating the per-pixel effect by scene depth).
    pub contact_shadows: bool,
    /// Number of directional-light shadow cascades. Effective range 1..=4
    /// (clamped at the light rig).
    pub shadow_cascades: u8,
    /// Custom vignette + gamma post-process pass (EM-2.6). Independent of
    /// the tier presets.
    pub vignette: bool,
    /// Reserved experimental features — always forced off (see
    /// [`ExperimentalGraphics`]).
    pub experimental: ExperimentalGraphics,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            tier: GraphicsTier::Ultra,
            taa: true,
            ssao: true,
            bloom: true,
            volumetric_fog: true,
            // BL-82 EM-3.11q: false in every tier, including Ultra — see the
            // field doc above.
            contact_shadows: false,
            shadow_cascades: 4,
            vignette: true,
            experimental: ExperimentalGraphics::default(),
        }
    }
}

impl GraphicsSettings {
    /// Applies the tier preset (non-`Custom` tiers overwrite the individual
    /// toggles) and forces the experimental flags off. Called on every load;
    /// idempotent.
    pub fn sanitize(&mut self) {
        if let Some((taa, ssao, bloom, volumetric_fog, contact_shadows, shadow_cascades)) =
            self.tier.preset()
        {
            self.taa = taa;
            self.ssao = ssao;
            self.bloom = bloom;
            self.volumetric_fog = volumetric_fog;
            self.contact_shadows = contact_shadows;
            self.shadow_cascades = shadow_cascades;
        }
        if self.experimental != ExperimentalGraphics::default() {
            warn!(
                "settings.ron enables experimental graphics (solari/dlss) — these are Vulkan+RTX \
                 experimental Bevy features, not wired into Xindeler yet; forcing them off"
            );
            self.experimental = ExperimentalGraphics::default();
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
            Ok(text) => match ron::from_str::<Self>(&text) {
                Ok(mut settings) => {
                    info!("Loaded settings from {}", path.display());
                    settings.graphics.sanitize();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_the_ultra_preset() {
        // Defaults must already be sanitized (no surprise rewrite on load).
        let mut settings = GraphicsSettings::default();
        let before = settings.clone();
        settings.sanitize();
        assert_eq!(settings, before);
        assert_eq!(settings.tier, GraphicsTier::Ultra);
    }

    /// BL-82 EM-3.11q regression: `contact_shadows` must stay `false` in
    /// EVERY tier preset (including Ultra), so the confirmed distant-sprite
    /// flicker never silently comes back via a tier change. See
    /// `GraphicsSettings::contact_shadows`'s doc for the full investigation.
    #[test]
    fn no_tier_preset_enables_contact_shadows() {
        for tier in [
            GraphicsTier::Low,
            GraphicsTier::Medium,
            GraphicsTier::High,
            GraphicsTier::Ultra,
        ] {
            let (_, _, _, _, contact_shadows, _) =
                tier.preset().expect("non-Custom tiers have a preset");
            assert!(
                !contact_shadows,
                "{tier:?} must not enable contact_shadows (BL-82 EM-3.11q)"
            );
        }
        // The struct default (fresh install, tier: Ultra) must match.
        assert!(!GraphicsSettings::default().contact_shadows);
    }

    #[test]
    fn tier_preset_overwrites_toggles() {
        let mut settings = GraphicsSettings {
            tier: GraphicsTier::Low,
            ..Default::default()
        };
        settings.sanitize();
        assert!(!settings.taa && !settings.ssao && !settings.bloom);
        assert!(!settings.volumetric_fog && !settings.contact_shadows);
        assert_eq!(settings.shadow_cascades, 1);
        // Vignette is independent of the tier presets.
        assert!(settings.vignette);
    }

    #[test]
    fn custom_tier_preserves_hand_edited_toggles() {
        let mut settings = GraphicsSettings {
            tier: GraphicsTier::Custom,
            taa: false,
            shadow_cascades: 2,
            ..Default::default()
        };
        settings.sanitize();
        assert!(!settings.taa);
        assert_eq!(settings.shadow_cascades, 2);
    }

    #[test]
    fn experimental_flags_are_forced_off() {
        let mut settings = GraphicsSettings {
            experimental: ExperimentalGraphics {
                solari: true,
                dlss: true,
            },
            ..Default::default()
        };
        settings.sanitize();
        assert_eq!(settings.experimental, ExperimentalGraphics::default());
    }

    #[test]
    fn old_settings_files_still_load() {
        // A pre-EM-2.5 file (no tier/vignette/experimental fields) must
        // parse, deserialize its missing tier as Custom, and keep the user's
        // hand-picked toggles through sanitize (NOT get clobbered by Ultra).
        let text = "(graphics: (taa: false, shadow_cascades: 2))";
        let mut settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.graphics.tier, GraphicsTier::Custom);
        settings.graphics.sanitize();
        assert!(!settings.graphics.taa);
        assert_eq!(settings.graphics.shadow_cascades, 2);
        // New fields still get their defaults.
        assert!(settings.graphics.vignette);
        assert_eq!(
            settings.graphics.experimental,
            ExperimentalGraphics::default()
        );
    }
}
