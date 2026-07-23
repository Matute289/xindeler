//! BL-82 EM-5.16 (T56.44) — the ONE settings-aware piece of the reactive i18n
//! pipeline.
//!
//! `xindeler-ui::i18n` owns the entire generic machinery (the `Localization`
//! catalog, `CurrentLocale`, the `LocalizedText`/`LocalizedLabel` tags, and
//! the reload/relocalize systems, all wired into `XindelerUiPlugin`) with NO
//! dependency on `xindeler-app`/`XindelerSettings` — see that module's own
//! doc comment. This module is the thin bridge that closes the loop: it
//! reads the persisted `XindelerSettings::language` (which the settings
//! window's Language tab already writes + saves) and, when it differs from
//! `CurrentLocale`, writes it in — which is ALL it takes to trigger the whole
//! reload + relocalize chain (`xindeler_ui::i18n::LocaleSyncSet`) the same
//! frame, since that chain is entirely driven by `CurrentLocale` change
//! detection.
//!
//! Ordered `.before(LocaleSyncSet)` so the change and the reactive chain it
//! triggers land in the SAME frame, not one frame late.
//!
//! Compiled only under `listen-server`/`net-client`, matching every other
//! `xindeler_ui`-consuming screen module in this crate — `Localization`/
//! `CurrentLocale` only exist once `xindeler_ui::XindelerUiPlugin` has been
//! added (via `combat_hud::CombatHudViewPlugin`), which only happens under
//! those features.

use bevy::prelude::*;
use xindeler_app::XindelerSettings;
use xindeler_ui::i18n::{CurrentLocale, LocaleSyncSet};

/// Installs [`sync_locale_from_settings`], the settings-bridge half of the
/// T56.44 reactive i18n pipeline.
pub struct ClientLocalizationPlugin;

impl Plugin for ClientLocalizationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_locale_from_settings.before(LocaleSyncSet));
    }
}

/// Mirrors `XindelerSettings::language` onto `CurrentLocale` whenever they
/// differ — the settings window's Language tab (`crate::settings_window`)
/// already persists the former; this is the only code that turns "the user
/// picked a different language" into an actual locale reload.
///
/// Deliberately checks equality before writing (`ResMut::set_if_neq` would
/// also work, but this reads slightly clearer given `CurrentLocale`'s field is
/// public): an unconditional `current_locale.0 = ...` on every frame would
/// mark `CurrentLocale` "changed" every single frame regardless of whether
/// the value actually differs, defeating the `resource_changed` gate the
/// whole reload/relocalize chain relies on.
fn sync_locale_from_settings(
    settings: Res<XindelerSettings>,
    mut current_locale: ResMut<CurrentLocale>,
) {
    if current_locale.0 != settings.language {
        current_locale.0 = settings.language.clone();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        app.init_resource::<CurrentLocale>();
        app
    }

    /// The default settings language (`"en"`) matches `CurrentLocale`'s own
    /// default — no spurious reload on a fresh boot.
    #[test]
    fn matching_defaults_leave_current_locale_untouched() {
        let mut app = new_app();
        app.world_mut()
            .run_system_once(sync_locale_from_settings)
            .expect("system runs");
        assert_eq!(app.world().resource::<CurrentLocale>().0, "en");
    }

    /// Changing `XindelerSettings::language` propagates onto `CurrentLocale`
    /// the next time the bridge system runs.
    #[test]
    fn language_change_propagates_to_current_locale() {
        let mut app = new_app();
        app.world_mut().resource_mut::<XindelerSettings>().language = "es".to_owned();
        app.world_mut()
            .run_system_once(sync_locale_from_settings)
            .expect("system runs");
        assert_eq!(app.world().resource::<CurrentLocale>().0, "es");
    }

    /// A no-op run (settings unchanged since the last sync) must NOT mark
    /// `CurrentLocale` changed AGAIN — otherwise the `resource_changed` gate
    /// the whole reload/relocalize chain relies on
    /// (`xindeler_ui::i18n::LocaleSyncSet`) would spuriously reload the
    /// entire catalog every single frame forever. Same
    /// run-once-then-`clear_trackers`-then-run-again idiom
    /// `combat_hud.rs`'s own `_does_not_rewrite_unchanged_*` tests use.
    #[test]
    fn unchanged_settings_do_not_keep_marking_current_locale_changed() {
        let mut app = new_app();
        app.world_mut()
            .run_system_once(sync_locale_from_settings)
            .expect("first run succeeds");
        app.world_mut().clear_trackers();

        app.world_mut()
            .run_system_once(sync_locale_from_settings)
            .expect("second run succeeds");

        assert!(
            !app.world().resource_ref::<CurrentLocale>().is_changed(),
            "CurrentLocale must not be re-marked changed when settings.language is unchanged"
        );
    }

    /// Every key translated in this batch resolves to real, non-English
    /// es-419 text, validating that the translation completeness.
    #[test]
    fn es419_settings_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };

        const KEYS: &[&str] = &[
            "hud-settings-cloud_rendering_mode-flat",
            "hud-settings-instrument_volume",
            "hud-settings-indoor_ambience",
            "hud-settings-keyboard-binding",
            "hud-settings-quality_preset",
            "hud-settings-ssao",
            "hud-settings-taa",
            "hud-settings-volumetric_fog",
            "hud-settings-contact_shadows",
            "hud-settings-vignette",
            "hud-settings-reduce_flashing",
            "hud-settings-high_contrast_ui",
            "hud-settings-shadow_cascades",
            "hud-settings-mouse_sensitivity",
            "hud-settings-fly_speed",
            "hud-settings-fly_fast_multiplier",
            "hud-settings-language",
            "hud-settings-custom_graphics",
            "hud-settings-open_controls",
            "hud-settings-note_interface",
            "hud-settings-note_video",
            "hud-settings-note_controls",
            "hud-settings-note_gameplay",
            "hud-settings-note_chat",
            "hud-settings-note_language",
            "hud-settings-note_networking",
            "hud-settings-note_sound",
            "hud-settings-note_accessibility",
        ];

        const COGNATE_ALLOWLIST: &[&str] =
            &["hud-settings-language", "hud-settings-custom_graphics"];

        let es = Localization::load(&parse_locale("es-419"), DEFAULT_HUD_FTL_FILES);
        let en = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);

        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            if !COGNATE_ALLOWLIST.contains(&k) {
                assert_ne!(
                    v,
                    en.tr(k),
                    "{k}: es-419 is byte-identical to en (untranslated?)"
                );
            }
        }
    }

    /// Every key translated in the HUD-CHROME batch resolves to real,
    /// non-English es-419 text, validating translation completeness of ~50
    /// keys across 15 files.
    #[test]
    fn es419_hud_chrome_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};

        const FILES: &[&str] = &[
            "hud/bag.ftl",
            "hud/crafting.ftl",
            "hud/trade.ftl",
            "hud/map.ftl",
            "hud/misc.ftl",
            "hud/chat.ftl",
            "hud/social.ftl",
            "hud/sct.ftl",
            "hud/quest.ftl",
            "hud/controls.ftl",
            "hud/combat_hud.ftl",
            "hud/subtitles.ftl",
            "item/armor/armor.ftl",
            "item/weapon/weapon.ftl",
            "item/items/quest.ftl",
        ];

        const KEYS: &[&str] = &[
            "hud-bag-tab_items",
            "hud-bag-tab_stats",
            "hud-bag-title",
            "hud-bag-unequip",
            "hud-bag-requirements_not_met",
            "hud-bag-requirement_level",
            "hud-bag-requirement_race",
            "hud-bag-requirement_class",
            "hud-bag-requires_attunement",
            "hud-context-menu-use",
            "hud-context-menu-drop",
            "hud-context-menu-cancel",
            "hud-crafting-tabs-repair",
            "hud-crafting-tabs-modular",
            "hud-crafting-salvage_desc",
            "hud-crafting-repair_tab_desc",
            "hud-crafting-modular_tab_desc",
            "hud-crafting-select_a_recipe",
            "hud-crafting-no_salvageable_items",
            "hud-crafting-no_damaged_items",
            "hud-crafting-no_primary_components",
            "hud-crafting-no_secondary_components",
            "hud-crafting-primary",
            "hud-crafting-secondary",
            "hud-crafting-salvage_selected",
            "hud-crafting-repair_selected",
            "hud-crafting-forge_weapon",
            "hud-trade-invite_from_player",
            "hud-trade-trading_with_player",
            "hud-trade-phase_mutate",
            "hud-trade-phase_review",
            "hud-trade-phase_complete",
            "hud-map-objectives",
            "hud-map-full_map_instructions",
            "hud-level_up_msg",
            "hud-chat-tab_whisper",
            "hud-social-online_players",
            "hud-sct-miss",
            "hud-dialogue-continue",
            "hud-controls-conflicts_with",
            "hud-combat_hud-level_abbr",
            "hud-combat_hud-combo_label",
            "subtitle-character_level_up",
            "armor-misc-ring-test_attunement_ring",
            "armor-misc-ring-test_attunement_ring_2",
            "weapon-tome-apprentice_tome",
            "weapon-holy_symbol-initiate_symbol",
            "weapon-focus-wanderer_focus",
            "sprite-quest-legoom_leaf",
            "sprite-quest-gnarling_carving",
        ];

        const COGNATE_ALLOWLIST: &[&str] =
            &["hud-crafting-tabs-modular", "hud-combat_hud-combo_label"];

        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);

        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            if !COGNATE_ALLOWLIST.contains(&k) {
                assert_ne!(
                    v,
                    en.tr(k),
                    "{k}: es-419 is byte-identical to en (untranslated?)"
                );
            }
        }
    }
}
