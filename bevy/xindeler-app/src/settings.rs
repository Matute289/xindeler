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
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct XindelerSettings {
    pub graphics: GraphicsSettings,
    /// BL-82 EM-5.1 T56.6 — the UI-scale foundation (legacy `ui/scale.rs`
    /// analog): a global multiplier `xindeler-ui`'s `HudScale` resource
    /// applies to Bevy's own `UiScale`. Persisted here (not in `xindeler-ui`
    /// itself) because settings persistence is this crate's established
    /// seam (`save()`/`load_or_default()`) — the EM-5.12 settings UI reads/
    /// writes this field like every other toggle. Manual `Default` impl
    /// below (not `#[derive(Default)]`) because the correct default is
    /// `1.0`, not `f32`'s own `0.0` (which would render an invisible HUD).
    pub ui_scale: f32,
    /// BL-82 EM-5.11 (T56.12) — the `controls` section: keyboard/mouse
    /// keymap + gamepad bindings (buttons/chords/axes), persisted as a delta
    /// against each sub-map's own defaults (see `xindeler_input::KeyMap` and
    /// its `keybind`/`gamepad` submodules for the delta-serde shape).
    /// `#[serde(default)]` on this struct + `KeyMap: Default` together mean
    /// an old `settings.ron` missing this key entirely still loads, getting
    /// the full stock keymap — same backward-compat guarantee `ui_scale`
    /// established for itself.
    pub controls: xindeler_input::KeyMap,
    /// BL-82 EM-5.11 — closes the `camera.rs` `TODO(EM-5.11)`: fly-cam
    /// speed/fast-multiplier/mouse-sensitivity move out of a hardcoded
    /// `FlyCam::default()` and into user-facing, persisted settings.
    pub camera: CameraSettings,
    /// BL-82 EM-5.12 (T56.39) — the Interface settings tab: HUD-facing
    /// toggles (crosshair, …). `ui_scale` stays a top-level field (T56.6
    /// established it there and `hud_scale.rs` reads it there); this holds the
    /// remaining interface toggles the settings window exposes. Same
    /// `#[serde(default)]` backward-compat guarantee as every field above.
    pub interface: InterfaceSettings,
    /// BL-82 EM-5.12 (T56.39) — the Chat settings tab. Closes `chat.rs`'s own
    /// documented TODO (its `CHAT_BG` const doc: "legacy `chat_opacity` is a
    /// user-configurable field; this pass bakes the default as a const … true
    /// parity will need it to read from a settings resource"): the chat
    /// scrollback box's translucent-black opacity is now a persisted setting.
    pub chat: ChatSettings,
    /// BL-82 EM-5.12 (T56.39) — the Language settings tab: the selected UI
    /// locale (BCP-47-ish tag, e.g. `"en"`). v1 ships the `en` catalog only
    /// (the `xindeler-ui` i18n seam is `en`-only until EM-5.16 wires the full
    /// reactive Fluent pipeline + hot-swap), so this is persisted and
    /// selectable but currently only `"en"` resolves — see the settings
    /// window's Language tab note.
    pub language: String,
    /// BL-82 EM-5.9 (T56.29) — the main-menu / login section: the first-run
    /// pre-alpha disclaimer gate + the remembered login fields the login
    /// screen pre-fills. `#[serde(default)]` on this struct + [`Default`]
    /// keeps every older `settings.ron` (written before this key existed)
    /// loading unchanged, exactly the backward-compat guarantee `ui_scale`/
    /// `controls` already established.
    pub menu: MenuSettings,
    /// BL-82 EM-5.16 (T56.43) — the settings window's Accessibility tab:
    /// reduced-flashing + high-contrast-UI toggles. Same backward-compat
    /// guarantee every field above establishes for itself.
    pub accessibility: AccessibilitySettings,
    /// BL-82 EM-5.16 (T56.43) — the first-run tutorial overlay's persisted
    /// "seen" flag (`xindeler-client::tutorial_overlay`). Same backward-
    /// compat guarantee every field above establishes for itself.
    pub tutorial: TutorialSettings,
}

impl Default for XindelerSettings {
    fn default() -> Self {
        Self {
            graphics: GraphicsSettings::default(),
            ui_scale: 1.0,
            controls: xindeler_input::KeyMap::default(),
            camera: CameraSettings::default(),
            interface: InterfaceSettings::default(),
            chat: ChatSettings::default(),
            language: default_language(),
            menu: MenuSettings::default(),
            accessibility: AccessibilitySettings::default(),
            tutorial: TutorialSettings::default(),
        }
    }
}

/// The default (and, in v1, only fully-wired) UI locale.
fn default_language() -> String { "en".to_owned() }

/// BL-82 EM-5.12 — HUD-facing interface toggles (the settings window's
/// Interface tab). Kept as its own sub-struct (not loose fields on
/// [`XindelerSettings`]) so the tab maps to one cohesive section, mirroring
/// [`GraphicsSettings`]/[`CameraSettings`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterfaceSettings {
    /// Whether the centred combat-HUD crosshair reticle is shown
    /// (`combat_hud::Crosshair`'s live `Visibility` follows this).
    pub show_crosshair: bool,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            show_crosshair: true,
        }
    }
}

/// BL-82 EM-5.12 — chat-panel appearance (the settings window's Chat tab).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatSettings {
    /// Chat scrollback background opacity, `0.0` (fully transparent) … `1.0`
    /// (opaque). Legacy default `0.4` (`voxygen` `ChatSettings::chat_opacity`).
    pub opacity: f32,
    /// Whether a message's speaker alias is prefixed onto its line (legacy
    /// `chat_character_name`, ported from `voxygen`'s Chat tab). `true` by
    /// default. Read by `xindeler-client::chat::format_chat_line` at the
    /// moment a message is ingested, so toggling it only affects messages
    /// that arrive afterward — already-rendered rows keep whatever alias
    /// state they were spawned with (they're never rebuilt), the same
    /// "future rows only" honesty `chat.opacity`'s live-but-not-retroactive
    /// apply already has for colour.
    pub show_character_name: bool,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            opacity: 0.4,
            show_character_name: true,
        }
    }
}

/// BL-82 EM-5.16 (T56.43) — accessibility toggles (the settings window's
/// Accessibility tab). `#[derive(Default)]` is correct here (unlike
/// [`InterfaceSettings`]/[`ChatSettings`]): both fields' honest defaults ARE
/// their `bool`/derived zero value — "full flashing" (`reduce_flashing:
/// false`) and "the normal dark-fantasy palette" (`high_contrast_ui: false`)
/// are what a fresh install should ship with, so no hand-written [`Default`]
/// impl is needed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilitySettings {
    /// Dampens the low-health damage vignette (`combat_hud`'s
    /// `DamageVignette`, the only screen-covering flash effect this client
    /// currently has) down to fully transparent, regardless of health.
    /// `false` (full effect) by default. The death screen itself (a static,
    /// non-flashing full-stop) is unaffected — it always shows at 0 health.
    pub reduce_flashing: bool,
    /// Brightens HUD secondary/muted text to full-contrast and makes panel
    /// backgrounds fully opaque, for players who find the default dark-
    /// fantasy palette's translucency/soft-contrast hard to read. Takes full
    /// effect on next launch: the live [`xindeler_ui::theme::HudTheme`]
    /// resource IS reconciled immediately (see `settings_window::
    /// apply_accessibility_theme`), but most widgets already on screen bake
    /// their colour from it ONCE at spawn time, so an already-open window
    /// only picks up the new palette the next time it's (re)opened —
    /// honestly documented the same way the Video tab's shadow-cascade
    /// count already is ("applies on next launch").
    pub high_contrast_ui: bool,
    /// Shows a text overlay (`xindeler-client::subtitle_overlay`) for triggered
    /// SFX/dialogue cues. `false` (no overlay) by default, matching the other
    /// two accessibility toggles' honest derived default.
    pub subtitles: bool,
}

/// BL-82 EM-5.16 (T56.43) — the first-run tutorial overlay's persisted state
/// (`xindeler-client::tutorial_overlay`). `#[derive(Default)]` is correct:
/// `seen: false` IS the honest fresh-install default (show it once).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TutorialSettings {
    /// Whether the player has dismissed the tutorial overlay at least once
    /// (by any dismissal path — the "Got it" button or Escape). `false` (the
    /// derived default) shows it automatically the first time a real local
    /// player exists; dismissing it flips this to `true` and persists so it
    /// never shows automatically again. It stays re-openable at any time
    /// afterwards via the Accessibility tab's "Show tutorial again" button,
    /// independent of this flag.
    pub seen: bool,
}

/// BL-82 EM-5.9 (T56.29) — persisted main-menu / login state: the first-run
/// disclaimer acknowledgement + the last-used login fields the login screen
/// pre-fills, mirroring legacy `voxygen`'s `NetworkingSettings`
/// (`username`/`default_server`) + `show_disclaimer` gate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuSettings {
    /// Whether the player has accepted the one-time pre-alpha disclaimer.
    /// `false` (the derived default) shows the disclaimer once, before the main
    /// menu; accepting it flips this to `true` and persists so it never shows
    /// again.
    pub disclaimer_accepted: bool,
    /// Last username typed on the login screen (pre-filled next launch).
    pub username: String,
    /// Last server address typed on the login screen (pre-filled next launch).
    /// Empty = the legacy default host is offered.
    pub server_address: String,
    /// Whether the login screen's Online/Offline toggle last sat on Online.
    /// `false` (the derived default) = Offline (singleplayer embedded world).
    pub online: bool,
    /// BL-82 EM-5.9 (T56.31) — the player's saved multiplayer servers, shown
    /// (and live-queried for ping/version/player-count) in the in-game server
    /// browser. Mirrors legacy `voxygen`'s `NetworkingSettings::servers`
    /// (`Vec<String>`), one level richer: each entry also carries an optional
    /// nickname. `#[serde(default)]` on this struct keeps every older
    /// `settings.ron` (written before this key existed) loading unchanged —
    /// the same backward-compat guarantee the rest of `MenuSettings` gives.
    pub servers: Vec<SavedServer>,
}

/// BL-82 EM-5.9 (T56.31) — one saved multiplayer server in the browser's
/// persisted list: a dialable address plus an optional human-friendly
/// nickname. Kept deliberately small (just the persisted identity) — the live
/// ping/version/player-count/MOTD fields the browser paints are queried fresh
/// each session and never persisted (they'd be stale on next launch), so they
/// live only in the in-memory browser state, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedServer {
    /// The dialable address, `host` or `host:port` (an omitted port defaults
    /// to the game port when queried/connected).
    pub address: String,
    /// An optional display name; empty = fall back to showing [`Self::address`]
    /// itself in the list.
    pub nickname: String,
}

/// Camera-rig tunables a player expects to control (mouse sensitivity, debug
/// fly-cam speed). Kept here (not on `FlyCam` itself) so `spawn_camera`
/// reads them the same way it already reads `GraphicsSettings` — a single
/// persisted source, not a hardcoded struct default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    /// Free fly-cam base speed, m/s (debug camera only — the real gameplay
    /// camera follows the player, it doesn't use this).
    pub fly_speed: f32,
    /// Shift-held speed multiplier on [`Self::fly_speed`].
    pub fly_fast_multiplier: f32,
    /// Mouse-look sensitivity, radians/pixel — applies to BOTH the debug
    /// fly-cam and the real third-person orbit (both read the same `FlyCam`
    /// yaw/pitch integration, `player_input::third_person_camera`'s doc
    /// comment).
    pub mouse_sensitivity: f32,
    /// Inverts the mouse-look pitch axis (legacy `gameplay.invert_mouse_y`,
    /// ported from `voxygen`'s Gameplay tab). `false` (normal, "push mouse
    /// up to look up") by default. Baked into `FlyCam::invert_pitch` at
    /// spawn time — same apply-on-next-launch honesty `fly_speed`/
    /// `mouse_sensitivity` already have (see `camera::spawn_camera`'s doc).
    pub invert_pitch: bool,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            fly_speed: 12.0,
            fly_fast_multiplier: 4.0,
            mouse_sensitivity: 0.002,
            invert_pitch: false,
        }
    }
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

    /// BL-82 EM-5.1 T56.6: the default UI scale is `1.0` (identity), never
    /// `f32`'s own `0.0` default — an invisible HUD would be a silent, hard-
    /// to-diagnose regression on any fresh install.
    #[test]
    fn default_ui_scale_is_identity() {
        assert_eq!(XindelerSettings::default().ui_scale, 1.0);
    }

    /// A settings.ron predating the `ui_scale` field still loads and gets
    /// the `1.0` default, same guarantee `old_settings_files_still_load`
    /// already covers for the graphics sub-struct.
    #[test]
    fn old_settings_files_default_ui_scale_to_identity() {
        let text = "(graphics: (taa: false, shadow_cascades: 2))";
        let settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.ui_scale, 1.0);
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

    /// BL-82 EM-5.11 (T56.12): a settings.ron predating the `controls`
    /// section entirely still loads and gets the FULL stock keymap (not an
    /// empty/broken one) — same backward-compat guarantee `ui_scale` and
    /// `graphics` each already established for themselves.
    #[test]
    fn old_settings_files_default_controls_to_the_stock_keymap() {
        let text = "(graphics: (taa: false, shadow_cascades: 2))";
        let settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.controls, xindeler_input::KeyMap::default());
    }

    /// A settings.ron with real rebind customizations round-trips through
    /// save/load intact (the actual end-to-end persistence path EM-5.11
    /// needs: rebind → restart → the customization survives).
    #[test]
    fn rebound_controls_round_trip_through_ron() {
        use xindeler_input::{GameInput, KeyOrMouse};

        let mut settings = XindelerSettings::default();
        settings.controls.keyboard.modify_binding(
            GameInput::Jump,
            KeyOrMouse::Key(bevy::input::keyboard::KeyCode::KeyZ),
        );
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("settings serialize");
        let round_tripped: XindelerSettings = ron::from_str(&text).expect("settings deserialize");
        assert_eq!(
            round_tripped.controls.keyboard.get_binding(GameInput::Jump),
            Some(KeyOrMouse::Key(bevy::input::keyboard::KeyCode::KeyZ))
        );
    }

    /// BL-82 EM-5.11: a settings.ron predating `camera` entirely still loads
    /// and gets the same defaults `camera.rs`'s hardcoded `FlyCam::default()`
    /// used to hold — this is a value-preserving move, not a behavior change.
    #[test]
    fn old_settings_files_default_camera_settings() {
        let text = "(graphics: (taa: false, shadow_cascades: 2))";
        let settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.camera, CameraSettings::default());
        assert_eq!(settings.camera.mouse_sensitivity, 0.002);
    }

    /// BL-82 EM-5.12: a settings.ron predating the `interface`/`chat`/
    /// `language` sections (every file written before this epic) still loads
    /// and gets their defaults — same backward-compat guarantee every field
    /// above already established for itself.
    #[test]
    fn old_settings_files_default_the_em512_sections() {
        let text = "(graphics: (taa: false, shadow_cascades: 2))";
        let settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.interface, InterfaceSettings::default());
        assert!(settings.interface.show_crosshair, "crosshair on by default");
        assert_eq!(settings.chat, ChatSettings::default());
        assert_eq!(settings.chat.opacity, 0.4, "legacy chat_opacity default");
        assert_eq!(settings.language, "en");
    }

    /// The new EM-5.12 sub-structs round-trip through save/load intact (the
    /// persistence path the settings window relies on: change → restart →
    /// survives).
    #[test]
    fn em512_settings_round_trip_through_ron() {
        let mut settings = XindelerSettings::default();
        settings.interface.show_crosshair = false;
        settings.chat.opacity = 0.75;
        settings.language = "es-419".to_owned();
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("settings serialize");
        let round_tripped: XindelerSettings = ron::from_str(&text).expect("settings deserialize");
        assert!(!round_tripped.interface.show_crosshair);
        assert_eq!(round_tripped.chat.opacity, 0.75);
        assert_eq!(round_tripped.language, "es-419");
    }

    /// BL-82 EM-5.16 (T56.43): a settings.ron predating the `accessibility`/
    /// `tutorial` sections (every file written before this epic) still loads
    /// and gets their defaults — same backward-compat guarantee every field
    /// above already established for itself.
    #[test]
    fn old_settings_files_default_the_em516a_sections() {
        let text = "(graphics: (taa: false, shadow_cascades: 2))";
        let settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.accessibility, AccessibilitySettings::default());
        assert!(
            !settings.accessibility.reduce_flashing,
            "full flashing by default"
        );
        assert!(
            !settings.accessibility.high_contrast_ui,
            "the normal palette by default"
        );
        assert!(!settings.accessibility.subtitles, "no overlay by default");
        assert_eq!(settings.tutorial, TutorialSettings::default());
        assert!(!settings.tutorial.seen, "unseen on a fresh install");
    }

    /// The new EM-5.16a sub-structs round-trip through save/load intact (the
    /// persistence path the settings window / tutorial overlay rely on:
    /// change → restart → survives).
    #[test]
    fn em516a_settings_round_trip_through_ron() {
        let mut settings = XindelerSettings::default();
        settings.accessibility.reduce_flashing = true;
        settings.accessibility.high_contrast_ui = true;
        settings.accessibility.subtitles = true;
        settings.tutorial.seen = true;
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("settings serialize");
        let round_tripped: XindelerSettings = ron::from_str(&text).expect("settings deserialize");
        assert!(round_tripped.accessibility.reduce_flashing);
        assert!(round_tripped.accessibility.high_contrast_ui);
        assert!(round_tripped.accessibility.subtitles);
        assert!(round_tripped.tutorial.seen);
    }

    /// BL-82 EM-5.9 (T56.31): a settings.ron predating the server-browser
    /// `servers` list still loads and gets an empty list (not a parse error) —
    /// same backward-compat guarantee every other `MenuSettings` field gives.
    #[test]
    fn old_settings_files_default_to_no_saved_servers() {
        let text = "(graphics: (taa: false), menu: (username: \"mati\"))";
        let settings: XindelerSettings = ron::from_str(text).expect("old file parses");
        assert_eq!(settings.menu.username, "mati");
        assert!(settings.menu.servers.is_empty());
    }

    /// The server-browser saved list round-trips through save/load intact
    /// (address + optional nickname): the persistence path the browser needs
    /// (add a server → restart → it's still there).
    #[test]
    fn saved_servers_round_trip_through_ron() {
        let mut settings = XindelerSettings::default();
        settings.menu.servers = vec![
            SavedServer {
                address: "play.example.com:14004".to_owned(),
                nickname: "Example".to_owned(),
            },
            SavedServer {
                address: "127.0.0.1:14004".to_owned(),
                nickname: String::new(),
            },
        ];
        let text = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default())
            .expect("settings serialize");
        let round_tripped: XindelerSettings = ron::from_str(&text).expect("settings deserialize");
        assert_eq!(round_tripped.menu.servers, settings.menu.servers);
        assert_eq!(round_tripped.menu.servers[0].nickname, "Example");
        assert_eq!(round_tripped.menu.servers[1].address, "127.0.0.1:14004");
    }
}
