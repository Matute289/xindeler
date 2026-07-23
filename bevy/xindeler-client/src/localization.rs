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

    /// Every key translated in the CORE-MENU batch (main.ftl, common.ftl,
    /// esc_menu.ftl, hud/char_window.ftl) resolves to real, non-English es-419
    /// text. Catches both a missing key falling through to `en` AND a lazy
    /// copy-paste of the English source.
    #[test]
    fn es419_core_menu_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };

        const KEYS: &[&str] = &[
            // main.ftl (45 keys)
            "main-xindeler_disclaimer_body",
            "main-xindeler_disclaimer_accept",
            "main-xindeler_tagline",
            "main-xindeler_login_body",
            "main-connect",
            "main-login-username_placeholder",
            "main-login-password_placeholder",
            "main-login-server_placeholder",
            "main-mode_offline",
            "main-mode_online",
            "main-options_notice",
            "main-xindeler_server_browser_body",
            "main-server_browser-add_server_heading",
            "main-server_browser-address_label",
            "main-server_browser-nickname_label",
            "main-server_browser-address_placeholder",
            "main-server_browser-nickname_placeholder",
            "main-server_browser-refresh",
            "main-server_browser-connect_selected",
            "main-server_browser-delete_selected",
            "main-server_browser-no_saved_servers",
            "main-server_browser-enter_address",
            "main-server_browser-already_in_list",
            "main-server_browser-added",
            "main-server_browser-refreshing",
            "main-server_browser-select_server_first",
            "main-server_browser-removed",
            "main-server_browser-select_to_delete",
            "main-server_browser-connecting_to",
            "main-server_browser-querying",
            "main-server_browser-unreachable",
            "main-server_browser-version_label",
            "main-server_browser-players_label",
            "main-connect_stage-starting",
            "main-connect_stage-generating_world",
            "main-connect_stage-establishing_connection",
            "main-connect_stage-checking_version",
            "main-connect_stage-authenticating",
            "main-connect_stage-loading_world_data",
            "main-connect_stage-preparing_client",
            "main-connect_stage-entering_world",
            "main-online_missing_server",
            "main-online_deferred_notice",
            "main-boot_crashed",
            "main-boot_failed",
            // common.ftl (20 keys)
            "common-on",
            "common-off",
            "common-class-warrior",
            "common-class-mage",
            "common-class-cleric",
            "common-class-rogue",
            "common-class-adventurer",
            "common-class-barbarian",
            "common-class-sorcerer",
            "common-class-warlock",
            "common-class-bard",
            "common-class-paladin",
            "common-class-druid",
            "common-class-ranger",
            "common-class-monk",
            "common-class-artificer",
            "common-class-blood_slayer",
            "common-weapons-tome",
            "common-weapons-holy_symbol",
            "common-weapons-focus",
            // esc_menu.ftl (3 keys)
            "esc_menu-title",
            "esc_menu-servers_soon",
            "esc_menu-logout_soon",
            // hud/char_window.ftl (6 keys)
            "character_window-character_level",
            "character_window-character_xp",
            "character_window-character_health",
            "character_window-character_energy",
            "character_window-character_poise",
            "character_window-character_combo",
        ];

        // Keys whose correct es-419 translation is legitimately identical to en
        // (real cognates only).
        const COGNATE_ALLOWLIST: &[&str] = &[
            "character_window-character_xp",     // "XP"
            "character_window-character_combo",  // "Combo"
            "main-server_browser-version_label", // "v:"
        ];

        let es = Localization::load(&parse_locale("es-419"), DEFAULT_HUD_FTL_FILES);
        let en = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);

        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{}: es-419 does not resolve (fell through to bare key)",
                k
            );
            assert!(!v.trim().is_empty(), "{}: es-419 resolves to empty", k);
            if !COGNATE_ALLOWLIST.contains(&k) {
                assert_ne!(
                    v,
                    en.tr(k),
                    "{}: es-419 is byte-identical to en (untranslated?)",
                    k
                );
            }
        }
    }

    /// Verify every character-creation UI key for es-419 resolves to real,
    /// non-English text — no untranslated fallthrough or empty strings.
    #[test]
    fn es419_chargen_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };
        const KEYS: &[&str] = &[
            "char_selection-select_character",
            "char_selection-no_characters_yet",
            "char_selection-delete",
            "char_selection-body_female",
            "char_selection-body_male",
            "char_selection-weapon_for_class",
            "char_selection-height_scale",
            "char_selection-class",
            "char_selection-class_warrior",
            "char_selection-class_mage",
            "char_selection-class_cleric",
            "char_selection-class_rogue",
            "char_selection-step_body",
            "char_selection-step_appearance",
            "char_selection-step_class",
            "char_selection-step_alignment",
            "char_selection-step_background",
            "char_selection-step_finish",
            "char_selection-wizard_next",
            "char_selection-wizard_back",
            "char_selection-alignment",
            "char_selection-ethos_good",
            "char_selection-ethos_neutral",
            "char_selection-ethos_evil",
            "char_selection-ethos_lawful",
            "char_selection-ethos_chaotic",
            "char_selection-ethos_true_neutral",
            "char_selection-wizard_step",
            "char_selection-summary_title",
            "char_selection-summary_label_name",
            "char_selection-summary_label_race",
            "char_selection-summary_label_class",
            "char_selection-summary_label_alignment",
            "char_selection-summary_label_background",
            "char_selection-sex",
            "char_selection-background",
            "char_selection-background_uncommitted",
            "char_selection-background_detail_name",
            "char_selection-background_detail_lore",
            "char_selection-background_detail_social",
            "char_selection-background_detail_affinity",
            "char_selection-background_detail_skills",
            "char_selection-background_detail_kit",
            "char_selection-background_detail_pending",
            "char_selection-background_social_pending",
            "char_selection-background_affinity_pending",
            "char_selection-background_society_label",
            "char_selection-background_detalle_acolyte",
            "char_selection-background_detalle_hermit",
            "char_selection-background_detalle_inquisitor",
            "char_selection-background_detalle_sage",
            "char_selection-background_detalle_archaeologist",
            "char_selection-background_detalle_scribe",
            "char_selection-background_detalle_investigator",
            "char_selection-background_detalle_soldier",
            "char_selection-background_detalle_guard",
            "char_selection-background_detalle_criminal",
            "char_selection-background_detalle_charlatan",
            "char_selection-background_detalle_bounty_hunter",
            "char_selection-background_detalle_noble",
            "char_selection-background_detalle_entertainer",
            "char_selection-background_detalle_folk_hero",
            "char_selection-background_detalle_merchant",
            "char_selection-background_detalle_artisan",
            "char_selection-background_detalle_farmer",
            "char_selection-background_detalle_fisher",
            "char_selection-background_detalle_miner",
            "char_selection-background_detalle_outlander",
            "char_selection-background_detalle_guide",
            "char_selection-background_detalle_sailor",
            "char_selection-background_detalle_urchin",
        ];
        const COGNATE_ALLOWLIST: &[&str] = &[
            "char_selection-ethos_neutral",
            "char_selection-background_detail_name", // already es in en source
            "char_selection-background_detail_lore", // already es in en source
            "char_selection-background_detail_social", // already es in en source
            "char_selection-background_detail_affinity", // already es in en source
            "char_selection-background_detail_skills", // already es in en source
            "char_selection-background_detail_kit",  // already es in en source
            "char_selection-background_detail_pending", // already es in en source
            "char_selection-background_social_pending", // already es in en source
            "char_selection-background_affinity_pending", // already es in en source
            "char_selection-background_society_label", // already es in en source
        ];
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
}
