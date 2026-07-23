//! The settings-aware piece of the reactive i18n pipeline.
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
/// reactive i18n pipeline.
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
        app.world_mut().resource_mut::<XindelerSettings>().language = "es-419".to_owned();
        app.world_mut()
            .run_system_once(sync_locale_from_settings)
            .expect("system runs");
        assert_eq!(app.world().resource::<CurrentLocale>().0, "es-419");
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

    /// Verify that all command.ftl keys in the es-419 batch translations are
    /// real, non-empty, and genuinely differ from their English counterparts
    /// (except for intentional cognates like command names and RON syntax).
    #[test]
    fn es419_commands_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };
        const KEYS: &[&str] = &[
            "command-adminify-already-has-no-role",
            "command-adminify-already-has-role",
            "command-adminify-assign-higher-than-own",
            "command-adminify-cannot-find-player",
            "command-adminify-reassign-to-above",
            "command-adminify-removed-role",
            "command-adminify-role-downgraded",
            "command-adminify-role-upgraded",
            "command-aura-invalid-buff-parameters",
            "command-aura-spawn",
            "command-aura-spawn-new-entity",
            "command-ban-added",
            "command-ban-already-added",
            "command-ban-ip-added",
            "command-ban-ip-queued",
            "command-battlemode-available-modes",
            "command-battlemode-cooldown",
            "command-battlemode-intown",
            "command-battlemode-same",
            "command-battlemode-updated",
            "command-buff-body-unknown",
            "command-buff-data",
            "command-buff-unknown",
            "command-cannot-send-message-hidden",
            "command-client-has-no-socketaddr",
            "command-death_effect-unknown",
            "command-destroyed-no-tethers",
            "command-destroyed-tethers",
            "command-disabled-by-settings",
            "command-disconnectall-confirm",
            "command-dismounted",
            "command-entity-has-no-client",
            "command-experimental-shaders-not-supported",
            "command-experimental-terrain-persistence-disabled",
            "command-explosion-power-too-high",
            "command-explosion-power-too-low",
            "command-faction-join",
            "command-give_item_quality-desc",
            "command-give_item_quality-success",
            "command-group_invite-invited-to-group",
            "command-group_invite-invited-to-your-group",
            "command-group-join",
            "command-into_npc-warning",
            "command-invalid-alignment",
            "command-invalid-skill-group",
            "command-inventory-cant-fit-item",
            "command-kick-higher-role",
            "command-kit-inventory-unavailable",
            "command-kit-not-enough-slots",
            "command-lantern-adjusted-strength",
            "command-lantern-adjusted-strength-color",
            "command-lantern-unequiped",
            "command-location-created",
            "command-location-deleted",
            "command-location-duplicate",
            "command-location-invalid",
            "command-location-not-found",
            "command-locations-empty",
            "command-locations-list",
            "command-make_party-desc",
            "command-make_test_char-desc",
            "command-message-group-missing",
            "command-no-buid-perms",
            "command-no-dismount",
            "command-outcome-expected_body_arg",
            "command-outcome-expected_entity_arg",
            "command-outcome-expected_frontent_specifier",
            "command-outcome-expected_integer",
            "command-outcome-expected_skill_group_kind",
            "command-outcome-expected_sprite_kind",
            "command-outcome-invalid_outcome",
            "command-outcome-variant_expected",
            "command-parse-duration-error",
            "command-permit-build-given",
            "command-permit-build-granted",
            "command-player-info-unavailable",
            "command-reloaded-chunks",
            "command-repaired-inventory_items",
            "command-repaired-items",
            "command-respawn-no-waypoint",
            "command-revoke-build",
            "command-revoke-build-all",
            "command-revoke-build-recv",
            "command-revoked-all-build",
            "command-scale-set",
            "command-server-no-experimental-terrain-persistence",
            "command-set_class-desc",
            "command-set_ethos-desc",
            "command-set_level-desc",
            "command-set_motd-message-added",
            "command-set_motd-message-not-set",
            "command-set_motd-message-removed",
            "command-set-build-mode-off",
            "command-set-build-mode-on-persistent",
            "command-set-build-mode-on-unpersistent",
            "command-set-waypoint-result",
            "command-site-not-found",
            "command-skillpreset-broken",
            "command-skillpreset-load-error",
            "command-skillpreset-missing",
            "command-spawned-campfire",
            "command-spawned-safezone",
            "command-spot-spot_not_found",
            "command-spot-world_feature",
            "command-sudo-higher-role",
            "command-sudo-no-permission-for-non-players",
            "command-tell-to-yourself",
            "command-time_scale-changed",
            "command-time_scale-current",
            "command-transform-invalid-presence",
            "command-unban-already-unbanned",
            "command-unban-ip-successful",
            "command-unban-successful",
            "command-unimplemented-spawn-special",
            "command-unknown",
            "command-version-current",
            "command-volume-created",
            "command-volume-size-incorrect",
            "command-waypoint-error",
            "command-waypoint-result",
            "command-weather-valid-values",
            "command-whitelist-added",
            "command-whitelist-already-added",
            "command-whitelist-permission-denied",
            "command-whitelist-removed",
            "command-whitelist-unlisted",
            "command-you-dont-exist",
        ];
        const COGNATE_ALLOWLIST: &[&str] = &[
            "command-battlemode-available-modes", // pvp, pve
            "command-disconnectall-confirm",      // confirm
            "command-faction-join",               // /join_faction
            "command-locations-list",             // RON identifier in braces
            "command-make_party-desc",            /* command syntax: class1 level1
                                                   * class2 level2 class3 level3 */
            "command-make_test_char-desc", // command syntax: level [class] [kit]
            "command-outcome-expected_frontent_specifier", /* typo in source: frontent (not
                                            * front-end) */
            "command-outcome-expected_skill_group_kind", // RON identifier
            "command-set_class-desc",                    /* command syntax: warrior, mage,
                                                          * cleric, rogue */
            "command-set_ethos-desc", /* command syntax: <good|neutral|evil>
                                       * <lawful|neutral|chaotic> */
            "command-spot-world_feature", // backticks for code: `worldgen`
            "command-weather-valid-values", // weather names: clear, rain, wind, storm
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

    /// Verifies that all recently-added ability and buff localization keys
    /// in es-419 resolve to real, non-English text (not untranslated).
    #[test]
    fn es419_abilities_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };
        const KEYS: &[&str] = &[
            "common-abilities-bow-ardent_hunt_clear",
            "common-abilities-bow-storm_chaser",
            "common-abilities-bow-heartseeker_shot",
            "common-abilities-bow-burning_heartseeker_shot",
            "common-abilities-bow-poison_heartseeker_shot",
            "common-abilities-bow-freezing_heartseeker_shot",
            "common-abilities-bow-lightning_heartseeker_shot",
            "common-abilities-bow-burning_hawkstrike_shot",
            "common-abilities-bow-poison_hawkstrike_shot",
            "common-abilities-bow-freezing_hawkstrike_shot",
            "common-abilities-bow-lightning_hawkstrike_shot",
            "common-abilities-spells-arcane-cinderbolt",
            "common-abilities-spells-divine-dawnmote",
            "common-abilities-spells-primal-thornspit",
            "innate-human",
            "innate-elf",
            "innate-dwarf",
            "innate-orc",
            "innate-danari",
            "innate-draugr",
            "class-warrior-rally",
            "class-warrior-onslaught",
            "class-mage-arcanesurge",
            "class-mage-arcanemastery",
            "class-cleric-mendinglight",
            "class-cleric-radiantchannel",
            "class-rogue-ambush",
            "class-rogue-vanish",
            "common-abilities-bow-thorn_stake",
            "common-abilities-bow-burning_thorn_stake",
            "common-abilities-bow-freezing_thorn_stake",
            "common-abilities-bow-poison_thorn_stake",
            "common-abilities-bow-lightning_thorn_stake",
            "buff-shielded",
            "buff-bleeding_mark",
            "buff-difficult_terrain",
            "buff-freedom_of_movement",
            "buff-antimagic",
            "buff-anchored",
            "buff-asleep",
            "buff-blinded",
            "buff-slowed",
            "buff-stormchaser",
            "buff-terrified",
            "buff-charmed",
            "buff-hollowtouched",
            "buff-ardenthunt",
            "buff-ignitearrow",
            "buff-freezearrow",
            "buff-drencharrow",
            "buff-joltarrow",
        ];
        const COGNATE_ALLOWLIST: &[&str] = &[];
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

    /// Skill tree UI header and node-status keys translated in the SKILLS-1
    /// batch: class/feats tabs and node state labels (passive, maxed,
    /// level, cost, locked). These are interface elements shown in the
    /// diary skill-tree UI.
    #[test]
    fn es419_skills_1_tree_ui_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };
        const KEYS: &[&str] = &[
            "hud-skill_tree-class",
            "hud-skill_tree-class-title",
            "hud-skill_tree-class-empty",
            "hud-skill_tree-feats",
            "hud-skill_tree-node_passive",
            "hud-skill_tree-node_maxed",
            "hud-skill_tree-node_level",
            "hud-skill_tree-node_cost",
            "hud-skill_tree-node_locked",
        ];
        const COGNATE_ALLOWLIST: &[&str] = &[];
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

    /// Class-skill keys translated: all 96 hud-skill-class-* entries covering
    /// Warrior, Mage, Cleric, and Rogue (12 skills each, 2 keys per skill).
    /// These are passive and active ability names and descriptions in the skill
    /// tree.
    #[test]
    fn es419_class_skills_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };
        const KEYS: &[&str] = &[
            "hud-skill-class-warrior-hardened_body_title",
            "hud-skill-class-warrior-hardened_body",
            "hud-skill-class-warrior-practiced_strikes_title",
            "hud-skill-class-warrior-practiced_strikes",
            "hud-skill-class-warrior-rally_title",
            "hud-skill-class-warrior-rally",
            "hud-skill-class-warrior-iron_skin_title",
            "hud-skill-class-warrior-iron_skin",
            "hud-skill-class-warrior-brutal_edge_title",
            "hud-skill-class-warrior-brutal_edge",
            "hud-skill-class-warrior-crushing_blows_title",
            "hud-skill-class-warrior-crushing_blows",
            "hud-skill-class-warrior-stalwart_title",
            "hud-skill-class-warrior-stalwart",
            "hud-skill-class-warrior-sundering_force_title",
            "hud-skill-class-warrior-sundering_force",
            "hud-skill-class-warrior-stagger_title",
            "hud-skill-class-warrior-stagger",
            "hud-skill-class-warrior-battle_momentum_title",
            "hud-skill-class-warrior-battle_momentum",
            "hud-skill-class-warrior-bulwark_stance_title",
            "hud-skill-class-warrior-bulwark_stance",
            "hud-skill-class-warrior-onslaught_title",
            "hud-skill-class-warrior-onslaught",
            "hud-skill-class-mage-focused_mind_title",
            "hud-skill-class-mage-focused_mind",
            "hud-skill-class-mage-true_aim_title",
            "hud-skill-class-mage-true_aim",
            "hud-skill-class-mage-arcane_surge_title",
            "hud-skill-class-mage-arcane_surge",
            "hud-skill-class-mage-spell_potency_title",
            "hud-skill-class-mage-spell_potency",
            "hud-skill-class-mage-pyromantic_attunement_title",
            "hud-skill-class-mage-pyromantic_attunement",
            "hud-skill-class-mage-cryomantic_attunement_title",
            "hud-skill-class-mage-cryomantic_attunement",
            "hud-skill-class-mage-quick_casting_title",
            "hud-skill-class-mage-quick_casting",
            "hud-skill-class-mage-penetrating_magic_title",
            "hud-skill-class-mage-penetrating_magic",
            "hud-skill-class-mage-warded_skin_title",
            "hud-skill-class-mage-warded_skin",
            "hud-skill-class-mage-mana_efficiency_title",
            "hud-skill-class-mage-mana_efficiency",
            "hud-skill-class-mage-overcharge_title",
            "hud-skill-class-mage-overcharge",
            "hud-skill-class-mage-arcane_mastery_title",
            "hud-skill-class-mage-arcane_mastery",
            "hud-skill-class-cleric-faithful_vigor_title",
            "hud-skill-class-cleric-faithful_vigor",
            "hud-skill-class-cleric-devout_focus_title",
            "hud-skill-class-cleric-devout_focus",
            "hud-skill-class-cleric-mending_light_title",
            "hud-skill-class-cleric-mending_light",
            "hud-skill-class-cleric-blessed_aim_title",
            "hud-skill-class-cleric-blessed_aim",
            "hud-skill-class-cleric-sacred_wards_title",
            "hud-skill-class-cleric-sacred_wards",
            "hud-skill-class-cleric-steadfast_faith_title",
            "hud-skill-class-cleric-steadfast_faith",
            "hud-skill-class-cleric-purifying_grace_title",
            "hud-skill-class-cleric-purifying_grace",
            "hud-skill-class-cleric-divine_conduit_title",
            "hud-skill-class-cleric-divine_conduit",
            "hud-skill-class-cleric-smiting_strikes_title",
            "hud-skill-class-cleric-smiting_strikes",
            "hud-skill-class-cleric-armor_of_faith_title",
            "hud-skill-class-cleric-armor_of_faith",
            "hud-skill-class-cleric-aegis_title",
            "hud-skill-class-cleric-aegis",
            "hud-skill-class-cleric-radiant_channel_title",
            "hud-skill-class-cleric-radiant_channel",
            "hud-skill-class-rogue-lithe_title",
            "hud-skill-class-rogue-lithe",
            "hud-skill-class-rogue-keen_edge_title",
            "hud-skill-class-rogue-keen_edge",
            "hud-skill-class-rogue-ambush_title",
            "hud-skill-class-rogue-ambush",
            "hud-skill-class-rogue-deadly_precision_title",
            "hud-skill-class-rogue-deadly_precision",
            "hud-skill-class-rogue-fleet_footed_title",
            "hud-skill-class-rogue-fleet_footed",
            "hud-skill-class-rogue-sure_strike_title",
            "hud-skill-class-rogue-sure_strike",
            "hud-skill-class-rogue-find_the_gap_title",
            "hud-skill-class-rogue-find_the_gap",
            "hud-skill-class-rogue-quick_hands_title",
            "hud-skill-class-rogue-quick_hands",
            "hud-skill-class-rogue-toxin_tolerance_title",
            "hud-skill-class-rogue-toxin_tolerance",
            "hud-skill-class-rogue-opportunist_title",
            "hud-skill-class-rogue-opportunist",
            "hud-skill-class-rogue-shadowstep_title",
            "hud-skill-class-rogue-shadowstep",
            "hud-skill-class-rogue-vanish_title",
            "hud-skill-class-rogue-vanish",
        ];
        const COGNATE_ALLOWLIST: &[&str] = &[];
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

    /// Every hud-feat-*_title key (72 total) and its .desc attribute resolve
    /// to real, non-English es-419 text. Verifies both the feat name and
    /// mechanical description are fully translated.
    #[test]
    fn es419_feats_keys_are_translated() {
        use xindeler_ui::i18n::{
            DEFAULT_HUD_FTL_FILES, Localization, fallback_locale, parse_locale,
        };
        const KEYS: &[&str] = &[
            "hud-feat-athlete_title",
            "hud-feat-charger_title",
            "hud-feat-crusher_title",
            "hud-feat-crossbow_expert_title",
            "hud-feat-defensive_duelist_title",
            "hud-feat-dual_wielder_title",
            "hud-feat-great_weapon_master_title",
            "hud-feat-heavy_armor_master_title",
            "hud-feat-mage_slayer_title",
            "hud-feat-mobile_title",
            "hud-feat-piercer_title",
            "hud-feat-polearm_master_title",
            "hud-feat-savage_attacker_title",
            "hud-feat-sentinel_title",
            "hud-feat-sharpshooter_title",
            "hud-feat-shield_master_title",
            "hud-feat-slasher_title",
            "hud-feat-speedy_title",
            "hud-feat-tavern_brawler_title",
            "hud-feat-aberrant_bloodmark_title",
            "hud-feat-arcane_college_initiate_title",
            "hud-feat-artificer_initiate_title",
            "hud-feat-elemental_adept_title",
            "hud-feat-frost_caster_title",
            "hud-feat-genie_magic_title",
            "hud-feat-gift_of_the_chromatic_dragon_title",
            "hud-feat-gift_of_the_gem_dragon_title",
            "hud-feat-gift_of_the_metallic_dragon_title",
            "hud-feat-greater_aberrant_bloodmark_title",
            "hud-feat-magic_initiate_title",
            "hud-feat-mythal_touched_title",
            "hud-feat-spell_sniper_title",
            "hud-feat-spellfire_adept_title",
            "hud-feat-spellfire_spark_title",
            "hud-feat-telekinetic_title",
            "hud-feat-telepathic_title",
            "hud-feat-umbra_touched_title",
            "hud-feat-veil_touched_title",
            "hud-feat-war_caster_title",
            "hud-feat-fairy_trickster_title",
            "hud-feat-inspiring_leader_title",
            "hud-feat-lordly_resolve_title",
            "hud-feat-tireless_reveler_title",
            "hud-feat-alert_title",
            "hud-feat-chef_title",
            "hud-feat-child_of_the_sun_title",
            "hud-feat-dungeon_delver_title",
            "hud-feat-healer_title",
            "hud-feat-observant_title",
            "hud-feat-shadowmoor_hexer_title",
            "hud-feat-bombardier_title",
            "hud-feat-draconic_cult_initiate_title",
            "hud-feat-dragonscarred_title",
            "hud-feat-orders_resilience_title",
            "hud-feat-poisoner_title",
            "hud-feat-quicksmith_title",
            "hud-feat-strike_of_the_giants_title",
            "hud-feat-vampire_hunter_title",
            "hud-feat-bloodlust_title",
            "hud-feat-cloying_mists_title",
            "hud-feat-delicious_pain_title",
            "hud-feat-durable_title",
            "hud-feat-light_bringer_title",
            "hud-feat-love_bites_title",
            "hud-feat-lucky_title",
            "hud-feat-putrefy_title",
            "hud-feat-rebuke_title",
            "hud-feat-resilient_title",
            "hud-feat-tough_title",
            "hud-feat-treacherous_allure_title",
            "hud-feat-vampire_touched_title",
            "hud-feat-vampires_plaything_title",
        ];
        const COGNATE_ALLOWLIST: &[&str] = &[];
        let es = Localization::load(&parse_locale("es-419"), DEFAULT_HUD_FTL_FILES);
        let en = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(v, k, "{k}: es-419 title does not resolve");
            assert!(!v.trim().is_empty(), "{k}: es-419 title resolves to empty");
            if !COGNATE_ALLOWLIST.contains(&k) {
                assert_ne!(
                    v,
                    en.tr(k),
                    "{k}: es-419 title is byte-identical to en (untranslated?)"
                );
            }
            let desc = es.tr_attr(k, "desc");
            assert_ne!(
                desc,
                format!("{k}.desc"),
                "{k}: es-419 .desc does not resolve"
            );
            assert_ne!(
                desc,
                en.tr_attr(k, "desc"),
                "{k}: es-419 .desc is byte-identical to en"
            );
        }
    }

    /// Verify that every tutorial.ftl key resolves to real, non-English es-419
    /// text — no fallthrough to bare key, no empty values, and distinct from
    /// EN.
    #[test]
    fn es419_tutorial_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["tutorial.ftl"];
        const KEYS: &[&str] = &[
            "tutorial-Move",
            "tutorial-Jump",
            "tutorial-OpenInventory",
            "tutorial-FallDamage",
            "tutorial-OpenGlider",
            "tutorial-Glider",
            "tutorial-StallGlider",
            "tutorial-Roll",
            "tutorial-Attacked",
            "tutorial-Unwield",
            "tutorial-Campfire",
            "tutorial-Waypoint",
            "tutorial-OpenDiary",
            "tutorial-FullInventory",
            "tutorial-RespawnDurability",
            "tutorial-RecipeAvailable",
            "tutorial-EnergyLow",
            "tutorial-Chat",
            "tutorial-Sneak",
            "tutorial-Lantern",
            "tutorial-Zoom",
            "tutorial-FirstPerson",
            "tutorial-Swim",
            "tutorial-OpenMap",
            "tutorial-UseItem",
            "tutorial-Crafting",
            "achievement-Moved",
            "achievement-Jumped",
            "achievement-OpenInventory",
            "achievement-OpenGlider",
            "achievement-StallGlider",
            "achievement-Rolled",
            "achievement-Wield",
            "achievement-Unwield",
            "achievement-FindCampfire",
            "achievement-SetWaypoint",
            "achievement-OpenDiary",
            "achievement-FullInventory",
            "achievement-Respawned",
            "achievement-RecipeAvailable",
            "achievement-OpenCrafting",
            "achievement-EnergyLow",
            "achievement-ReceivedChatMsg",
            "achievement-NearEnemies",
            "achievement-InDark",
            "achievement-UsedLantern",
            "achievement-Swim",
            "achievement-Zoom",
            "achievement-OpenMap",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// Verify that every spell.ftl key (axiomancy/hemomancy/abjuration, 74 keys
    /// incl. -desc) resolves to real, non-English es-419 text — no fallthrough
    /// to bare key, no empty values, and distinct from EN.
    #[test]
    fn es419_spells_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &[
            "spell/axiomancy.ftl",
            "spell/hemomancy.ftl",
            "spell/abjuration.ftl",
        ];
        const KEYS: &[&str] = &[
            // Abjuration (1 spell × 2 keys)
            "spell-antimagic_field",
            "spell-antimagic_field-desc",
            // Axiomancy (15 spells × 2 keys)
            "spell-sapping_sting",
            "spell-sapping_sting-desc",
            "spell-magnify_gravity",
            "spell-magnify_gravity-desc",
            "spell-gift_of_alacrity",
            "spell-gift_of_alacrity-desc",
            "spell-fortunes_favor",
            "spell-fortunes_favor-desc",
            "spell-immovable_object",
            "spell-immovable_object-desc",
            "spell-wristpocket",
            "spell-wristpocket-desc",
            "spell-pulse_wave",
            "spell-pulse_wave-desc",
            "spell-gravity_sinkhole",
            "spell-gravity_sinkhole-desc",
            "spell-temporal_shunt",
            "spell-temporal_shunt-desc",
            "spell-gravity_fissure",
            "spell-gravity_fissure-desc",
            "spell-tether_essence",
            "spell-tether_essence-desc",
            "spell-dark_star",
            "spell-dark_star-desc",
            "spell-reality_break",
            "spell-reality_break-desc",
            "spell-ravenous_void",
            "spell-ravenous_void-desc",
            "spell-time_ravage",
            "spell-time_ravage-desc",
            // Hemomancy (21 spells × 2 keys)
            "spell-bloodlet",
            "spell-bloodlet-desc",
            "spell-clot",
            "spell-clot-desc",
            "spell-crimson_brand",
            "spell-crimson_brand-desc",
            "spell-hemal_spike",
            "spell-hemal_spike-desc",
            "spell-sanguine_ward",
            "spell-sanguine_ward-desc",
            "spell-leeching_grasp",
            "spell-leeching_grasp-desc",
            "spell-bloodboil",
            "spell-bloodboil-desc",
            "spell-scarlet_tether",
            "spell-scarlet_tether-desc",
            "spell-bloodbound_edge",
            "spell-bloodbound_edge-desc",
            "spell-hemorrhage",
            "spell-hemorrhage-desc",
            "spell-blood_mirror",
            "spell-blood_mirror-desc",
            "spell-gory_eruption",
            "spell-gory_eruption-desc",
            "spell-sanguine_surge",
            "spell-sanguine_surge-desc",
            "spell-crimson_lash",
            "spell-crimson_lash-desc",
            "spell-exsanguinate",
            "spell-exsanguinate-desc",
            "spell-vitalic_font",
            "spell-vitalic_font-desc",
            "spell-crimson_tide",
            "spell-crimson_tide-desc",
            "spell-hearts_seizure",
            "spell-hearts_seizure-desc",
            "spell-hemoclysm",
            "spell-hemoclysm-desc",
            "spell-crimson_apotheosis",
            "spell-crimson_apotheosis-desc",
            "spell-the_last_vein",
            "spell-the_last_vein-desc",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// Verify that every courier_quests.ftl key (30 keys for
    /// courier/fetch/messenger dialogue; excludes
    /// npc-response-quest-courier-start_3 which only has attributes)
    /// resolves to real, non-English es-419 text — no fallthrough to bare key,
    /// no empty values, and distinct from EN.
    #[test]
    fn es419_courier_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["quest/courier_quests.ftl"];
        const KEYS: &[&str] = &[
            "npc-response-quest-courier-start",
            "npc-response-quest-courier-start_2",
            "npc-response-quest-courier-where",
            "npc-response-quest-courier-thanks",
            "npc-response-quest-courier-generic-insufficient-items",
            "dialogue-question-quest-courier-claim",
            "dialogue-question-quest-courier-where",
            "hud-map-spot-unspecified",
            "spot-name-gnarling-totem",
            "spot-name-unspecified",
            "npc-response-quest-courier-ask",
            "npc-response-quest-spot-courier-ask",
            "dialogue-question-quest-courier-what",
            "dialogue-question-quest-courier-what-target",
            "npc-response-quest-fetch-ask",
            "npc-response-quest-spot-fetch-ask",
            "dialogue-question-quest-fetch-what",
            "npc-response-quest-messenger-ask",
            "npc-response-quest-messenger-what-is-needed",
            "npc-response-quest-messenger-what-is-needed-target",
            "dialogue-question-quest-messenger-what",
            "dialogue-question-quest-messenger-what-target",
            "npc-response-quest-courier-gnarling-carving",
            "npc-response-quest-courier-gnarling-carving-insufficient-items",
            "npc-response-quest-courier-gnarling-carving-what-is-needed",
            "hud-map-spot-gnarling-carving-label",
            "npc-response-quest-courier-legoom-leaf",
            "npc-response-quest-courier-legoom-leaf-insufficient-items",
            "npc-response-quest-courier-legoom-leaf-what-is-needed",
            "npc-response-quest-messenger-send-word",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// Sample NPC armor names resolve to es-419 text (not falling through to
    /// bare keys). Creature-name-only entries (e.g. "Gnarling", "Kappa") are
    /// excluded as they legitimately have no translatable content.
    #[test]
    fn es419_npc_armor_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["item/armor/npc.ftl"];
        const KEYS: &[&str] = &[
            "common-items-npc_armor-pants-leather_blue",
            "common-items-npc_armor-pants-plate_red",
            "common-items-npc_armor-bird_large-phoenix",
            "common-items-npc_armor-bird_large-wyvern",
            "common-items-npc_armor-bird_medium-bloodmoon_bat",
            "common-items-npc_armor-golem-claygolem",
            "common-items-npc_armor-golem-woodgolem",
            "common-items-npc_armor-golem-irongolem",
            "common-items-npc_armor-golem-ancienteffigy",
            "common-items-npc_armor-golem-gravewarden",
            "common-items-npc_armor-biped_small-myrmidon-foot-hoplite",
            "common-items-npc_armor-biped_small-myrmidon-foot-marksman",
            "common-items-npc_armor-biped_small-myrmidon-foot-strategian",
            "common-items-npc_armor-biped_small-sahagin-foot-sniper",
            "common-items-npc_armor-biped_small-sahagin-foot-sorcerer",
            "common-items-npc_armor-biped_small-sahagin-foot-spearman",
            "common-items-npc_armor-biped_small-adlet-foot-hunter",
            "common-items-npc_armor-biped_small-adlet-foot-icepicker",
            "common-items-npc_armor-biped_small-adlet-foot-tracker",
            "common-items-npc_armor-biped_small-gnarling-foot-chieftain",
            "common-items-npc_armor-biped_small-boreal-foot-warrior",
            "common-items-npc_armor-biped_small-boreal-head-warrior",
            "common-items-npc_armor-biped_small-boreal-pants-warrior",
            "common-items-npc_armor-biped_small-boreal-chest-warrior",
            "common-items-npc_armor-biped_small-boreal-hand-warrior",
            "common-items-npc_armor-biped_small-ashen-foot-warrior",
            "common-items-npc_armor-biped_small-ashen-head-warrior",
            "common-items-npc_armor-biped_small-ashen-pants-warrior",
            "common-items-npc_armor-biped_small-ashen-chest-warrior",
            "common-items-npc_armor-biped_small-ashen-hand-warrior",
            "common-items-npc_armor-biped_small-treasure_egg-chest-treasure_egg",
            "common-items-npc_armor-biped_small-treasure_egg-foot-treasure_egg",
            "common-items-npc_armor-biped_small-iron_dwarf-foot-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-head-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-pants-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-chest-iron_dwarf",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let _en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
        }
    }

    /// The second batch of proc-generated NPC armor names (keys #121-240 of
    /// 357) resolve to real, non-English es-419 text (not falling through
    /// to bare keys or matching en). Excludes creature-name-only entries
    /// (e.g. "Irrwurz", "Jiangshi") that legitimately remain identical
    /// across languages.
    #[test]
    fn es419_npc_armor_2_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["item/armor/npc.ftl"];
        const KEYS: &[&str] = &[
            "common-items-npc_armor-biped_small-iron_dwarf-hand-iron_dwarf",
            "common-items-npc_armor-biped_small-haniwa-foot-archer",
            "common-items-npc_armor-biped_small-haniwa-foot-guard",
            "common-items-npc_armor-biped_small-haniwa-foot-soldier",
            "common-items-npc_armor-biped_small-haniwa-head-archer",
            "common-items-npc_armor-biped_small-haniwa-head-guard",
            "common-items-npc_armor-biped_small-haniwa-head-soldier",
            "common-items-npc_armor-biped_small-haniwa-pants-archer",
            "common-items-npc_armor-biped_small-haniwa-pants-guard",
            "common-items-npc_armor-biped_small-haniwa-pants-soldier",
            "common-items-npc_armor-biped_small-haniwa-chest-archer",
            "common-items-npc_armor-biped_small-haniwa-chest-guard",
            "common-items-npc_armor-biped_small-haniwa-chest-soldier",
            "common-items-npc_armor-biped_small-haniwa-hand-archer",
            "common-items-npc_armor-biped_small-haniwa-hand-guard",
            "common-items-npc_armor-biped_small-haniwa-hand-soldier",
            "common-items-npc_armor-biped_small-husk-foot-husk",
            "common-items-npc_armor-biped_small-husk-head-husk",
            "common-items-npc_armor-biped_small-husk-pants-husk",
            "common-items-npc_armor-biped_small-husk-chest-husk",
            "common-items-npc_armor-biped_small-husk-hand-husk",
            "common-items-npc_armor-biped_small-husk-tail-husk",
            "common-items-npc_armor-biped_small-flamekeeper-foot-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-head-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-pants-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-chest-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-hand-flamekeeper",
            "common-items-npc_armor-biped_small-gnoll-foot-rogue",
            "common-items-npc_armor-biped_small-gnoll-foot-shaman",
            "common-items-npc_armor-biped_small-gnoll-foot-trapper",
            "common-items-npc_armor-biped_small-gnoll-head-rogue",
            "common-items-npc_armor-biped_small-gnoll-head-shaman",
            "common-items-npc_armor-biped_small-gnoll-head-trapper",
            "common-items-npc_armor-biped_small-gnoll-pants-rogue",
            "common-items-npc_armor-biped_small-gnoll-pants-shaman",
            "common-items-npc_armor-biped_small-gnoll-pants-trapper",
            "common-items-npc_armor-biped_small-gnoll-chest-rogue",
            "common-items-npc_armor-biped_small-gnoll-chest-shaman",
            "common-items-npc_armor-biped_small-gnoll-chest-trapper",
            "common-items-npc_armor-biped_small-gnoll-hand-rogue",
            "common-items-npc_armor-biped_small-gnoll-hand-shaman",
            "common-items-npc_armor-biped_small-gnoll-hand-trapper",
            "common-items-npc_armor-biped_small-gnoll-tail-rogue",
            "common-items-npc_armor-biped_small-gnoll-tail-shaman",
            "common-items-npc_armor-biped_small-gnoll-tail-trapper",
            "common-items-npc_armor-biped_small-shamanic_spirit-head-shamanic_spirit",
            "common-items-npc_armor-biped_small-shamanic_spirit-chest-shamanic_spirit",
            "common-items-npc_armor-biped_small-shamanic_spirit-pants-shamanic_spirit",
            "common-items-npc_armor-biped_small-shamanic_spirit-hand-shamanic_spirit",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-head-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-chest-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-pants-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-hand-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-foot-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodservant-head-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-chest-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-pants-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-hand-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-foot-bloodservant",
            "common-items-npc_armor-biped_small-harlequin-head-harlequin",
            "common-items-npc_armor-biped_small-harlequin-chest-harlequin",
            "common-items-npc_armor-biped_small-harlequin-pants-harlequin",
            "common-items-npc_armor-biped_small-harlequin-hand-harlequin",
            "common-items-npc_armor-biped_small-harlequin-foot-harlequin",
            "common-items-npc_armor-biped_small-goblin_thug-head-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-chest-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-pants-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-hand-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-foot-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_chucker-head-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-chest-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-pants-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-hand-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-foot-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_ruffian-head-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-chest-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-pants-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-hand-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-foot-goblin_ruffian",
            "common-items-npc_armor-biped_small-green_legoom-head-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-chest-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-pants-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-hand-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-foot-green_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-head-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-chest-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-pants-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-hand-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-foot-ochre_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-head-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-chest-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-pants-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-hand-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-foot-purple_legoom",
            "common-items-npc_armor-biped_small-red_legoom-head-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-chest-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-pants-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-hand-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-foot-red_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-head-umber_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-chest-umber_legoom",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// Verify that the first batch of proc-generated NPC weapon names resolve
    /// to real, non-English es-419 text (not falling through to bare keys or
    /// matching en).
    #[test]
    fn es419_npc_weapon_1_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["item/weapon/npc.ftl"];
        const KEYS: &[&str] = &[
            "common-items-npc_weapons-biped_small-mandragora",
            "common-items-npc_weapons-biped_small-myrmidon-hoplite",
            "common-items-npc_weapons-biped_small-myrmidon-marksman",
            "common-items-npc_weapons-biped_small-myrmidon-strategian",
            "common-items-npc_weapons-biped_small-sahagin-sniper",
            "common-items-npc_weapons-biped_small-sahagin-sorcerer",
            "common-items-npc_weapons-biped_small-sahagin-spearman",
            "common-items-npc_weapons-biped_small-adlet-hunter",
            "common-items-npc_weapons-biped_small-adlet-icepicker",
            "common-items-npc_weapons-biped_small-adlet-tracker",
            "common-items-npc_weapons-biped_small-gnarling-chieftain",
            "common-items-npc_weapons-biped_small-gnarling-greentotem",
            "common-items-npc_weapons-biped_small-gnarling-logger",
            "common-items-npc_weapons-biped_small-gnarling-mugger",
            "common-items-npc_weapons-biped_small-gnarling-redtotem",
            "common-items-npc_weapons-biped_small-gnarling-stalker",
            "common-items-npc_weapons-biped_small-gnarling-whitetotem",
            "common-items-npc_weapons-biped_small-boreal-bow",
            "common-items-npc_weapons-biped_small-boreal-hammer",
            "common-items-npc_weapons-biped_small-ashen-axe",
            "common-items-npc_weapons-biped_small-ashen-staff",
            "common-items-npc_weapons-biped_small-haniwa-archer",
            "common-items-npc_weapons-biped_small-haniwa-guard",
            "common-items-npc_weapons-biped_small-haniwa-soldier",
            "common-items-npc_weapons-biped_small-vampire-harlequin_dagger",
            "common-items-npc_weapons-biped_small-vampire-bloodservant_axe",
            "common-items-npc_weapons-biped_small-vampire-bloodmoon_heiress_sword",
            "common-items-npc_weapons-unique-goblin_thug_club",
            "common-items-npc_weapons-unique-goblin_chucker",
            "common-items-npc_weapons-unique-goblin_ruffian_knife",
            "common-items-npc_weapons-unique-green_legoom_rake",
            "common-items-npc_weapons-unique-ochre_legoom_spade",
            "common-items-npc_weapons-unique-purple_legoom_pitchfork",
            "common-items-npc_weapons-unique-red_legoom_hoe",
            "common-items-npc_weapons-unique-umber_legoom_hook",
            "common-items-npc_weapons-bow-bipedlarge-velorite",
            "common-items-npc_weapons-bow-saurok_bow",
            "common-items-npc_weapons-bow-terracotta_besieger_bow",
            "common-items-npc_weapons-axe-gigas_frost_axe",
            "common-items-npc_weapons-sword-gigas_fire_sword",
            "common-items-npc_weapons-axe-executioner_axe",
            "common-items-npc_weapons-axe-minotaur_axe",
            "common-items-npc_weapons-axe-oni_blue_axe",
            "common-items-npc_weapons-staff-bipedlarge-cultist",
            "common-items-npc_weapons-staff-mindflayer_staff",
            "common-items-npc_weapons-staff-ogre_staff",
            "common-items-npc_weapons-staff-saurok_staff",
            "common-items-npc_weapons-sword-adlet_elder_sword",
            "common-items-npc_weapons-sword-bipedlarge-cultist",
            "common-items-npc_weapons-sword-dullahan_sword",
            "common-items-npc_weapons-sword-pickaxe_velorite_sword",
            "common-items-npc_weapons-sword-saurok_sword",
            "common-items-npc_weapons-sword-haniwa_general_sword",
            "common-items-npc_weapons-sword-terracotta_pursuer_sword",
            "common-items-npc_weapons-unique-akhlut",
            "common-items-npc_weapons-unique-beast_claws",
            "common-items-npc_weapons-unique-birdlargebasic",
            "common-items-npc_weapons-unique-birdlargebreathe",
            "common-items-npc_weapons-unique-birdlargefire",
            "common-items-npc_weapons-unique-birdmediumbasic",
            "common-items-npc_weapons-unique-bushly",
            "common-items-npc_weapons-unique-cactid",
            "common-items-npc_weapons-unique-cardinal",
            "common-items-npc_weapons-unique-clay_golem_fist",
            "common-items-npc_weapons-unique-iron_dwarf",
            "common-items-npc_weapons-unique-cloudwyvern",
            "common-items-npc_weapons-unique-coral_golem_fist",
            "common-items-npc_weapons-unique-crab_pincer",
            "common-items-npc_weapons-unique-karkatha_pincer",
            "common-items-npc_weapons-unique-dagon",
            "common-items-npc_weapons-unique-driggle",
            "common-items-npc_weapons-unique-emberfly",
            "common-items-npc_weapons-unique-fiery_tornado",
            "common-items-npc_weapons-unique-flamekeeper_staff",
            "common-items-npc_weapons-unique-cursekeeper_sceptre",
            "common-items-npc_weapons-unique-cursekeeper_sceptre_fake",
            "common-items-npc_weapons-unique-jiangshi",
            "common-items-npc_weapons-unique-mogwai",
            "common-items-npc_weapons-unique-shamanic_spirit",
            "common-items-npc_weapons-unique-terracotta_demolisher_fist",
            "common-items-npc_weapons-unique-terracotta_statue",
            "common-items-npc_weapons-unique-iron_golem_fist",
            "common-items-npc_weapons-hammer-forgemaster_hammer",
            "common-items-npc_weapons-unique-snaretongue",
            "common-items-npc_weapons-unique-flamethrower",
            "common-items-npc_weapons-unique-flamewyvern",
            "common-items-npc_weapons-unique-frostfang",
            "common-items-npc_weapons-unique-frostwyvern",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// Verify that the third batch of proc-generated NPC armor names resolve
    /// to real, non-English es-419 text (not falling through to bare keys or
    /// matching en). This batch completes the translation coverage (keys
    /// #241-357 of 357), closing the gap.
    #[test]
    fn es419_npc_armor_3_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["item/armor/npc.ftl"];
        const KEYS: &[&str] = &[
            "common-items-npc_armor-biped_small-umber_legoom-pants-umber_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-hand-umber_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-foot-umber_legoom",
            "common-items-npc_armor-crustacean-karkatha",
            "common-items-npc_armor-chest-plate_red",
            "common-items-npc_armor-quadruped_low-basilisk",
            "common-items-npc_armor-quadruped_low-dagon",
            "common-items-npc_armor-quadruped_low-drake",
            "common-items-npc_armor-quadruped_low-shell",
            "common-items-npc_armor-quadruped_low-snapper",
            "common-items-npc_armor-quadruped_medium-tarasque",
            "common-items-npc_armor-quadruped_medium-claysteed",
            "common-items-npc_armor-generic",
            "common-items-npc_armor-generic_high",
            "common-items-npc_armor-theropod-rugged",
            "common-items-npc_armor-biped_large-cyclops",
            "common-items-npc_armor-biped_large-dullahan",
            "common-items-npc_armor-biped_large-generic",
            "common-items-npc_armor-biped_large-gigas_frost",
            "common-items-npc_armor-biped_large-gigas_fire",
            "common-items-npc_armor-biped_large-harvester",
            "common-items-npc_armor-biped_large-mindflayer",
            "common-items-npc_armor-biped_large-minotaur",
            "common-items-npc_armor-biped_large-tidal_warrior",
            "common-items-npc_armor-biped_large-tursus",
            "common-items-npc_armor-biped_large-warlock",
            "common-items-npc_armor-biped_large-warlord",
            "common-items-npc_armor-biped_large-yeti",
            "common-items-npc_armor-biped_large-forgemaster",
            "common-items-npc_armor-biped_large-haniwageneral",
            "common-items-npc_armor-biped_large-terracotta",
            "armor-leather_blue-pants",
            "armor-leather_blue-back",
            "armor-velorite_battlemage-back",
            "armor-velorite_battlemage-belt",
            "armor-velorite_battlemage-chest",
            "armor-velorite_battlemage-foot",
            "armor-velorite_battlemage-hand",
            "armor-velorite_battlemage-pants",
            "armor-velorite_battlemage-shoulder",
            "armor-cardinal-belt",
            "armor-cardinal-chest",
            "armor-cardinal-foot",
            "armor-cardinal-hand",
            "armor-cardinal-mitre",
            "armor-cardinal-pants",
            "armor-cardinal-shoulder",
            "armor-merchant-back",
            "armor-merchant-belt",
            "armor-merchant-chest",
            "armor-merchant-foot",
            "armor-merchant-hand",
            "armor-merchant-pants",
            "armor-merchant-shoulder_l",
            "common-items-armor-alchemist-belt",
            "common-items-armor-alchemist-chest",
            "common-items-armor-alchemist-hat",
            "common-items-armor-alchemist-pants",
            "common-items-armor-misc-head-headband",
            "common-items-armor-witch-back",
            "common-items-armor-witch-belt",
            "common-items-armor-witch-chest",
            "common-items-armor-witch-foot",
            "common-items-armor-witch-hand",
            "common-items-armor-witch-pants",
            "common-items-armor-witch-shoulder",
            "common-items-armor-pirate-belt",
            "common-items-armor-pirate-chest",
            "common-items-armor-pirate-foot",
            "common-items-armor-pirate-hand",
            "common-items-armor-pirate-pants",
            "common-items-armor-pirate-shoulder",
            "common-items-armor-miner-back",
            "common-items-armor-miner-belt",
            "common-items-armor-miner-chest",
            "common-items-armor-miner-foot",
            "common-items-armor-miner-hand",
            "common-items-armor-miner-pants",
            "common-items-armor-miner-shoulder",
            "common-items-armor-miner-shoulder_captain",
            "common-items-armor-miner-shoulder_flame",
            "common-items-armor-miner-shoulder_overseer",
            "common-items-armor-chef-belt",
            "common-items-armor-chef-chest",
            "common-items-armor-chef-hat",
            "common-items-armor-chef-pants",
            "common-items-armor-blacksmith-belt",
            "common-items-armor-blacksmith-chest",
            "common-items-armor-blacksmith-hand",
            "common-items-armor-blacksmith-hat",
            "common-items-armor-blacksmith-pants",
            "common-items-armor-leather_plate-helmet",
            "armor-assassin-belt",
            "armor-assassin-chest",
            "armor-assassin-foot",
            "armor-assassin-hand",
            "armor-misc-head-assa_mask-0",
            "armor-assassin-pants",
            "armor-assassin-shoulder",
            "armor-ferocious-back",
            "armor-ferocious-belt",
            "armor-ferocious-chest",
            "armor-ferocious-foot",
            "armor-ferocious-hand",
            "armor-ferocious-pants",
            "armor-ferocious-shoulder",
            "armor-bonerattler-belt",
            "armor-bonerattler-chest",
            "armor-bonerattler-foot",
            "armor-bonerattler-hand",
            "armor-bonerattler-pants",
            "armor-bonerattler-shoulder",
            "armor-misc-head-exclamation",
            "armor-misc-foot-iceskate",
            "armor-misc-foot-jackalope",
            "armor-misc-foot-ski",
            "armor-miner-helmet",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// Verify that the second batch of proc-generated NPC weapon names resolve
    /// to real, non-English es-419 text (not falling through to bare keys or
    /// matching en). Excludes creature names that are identical in Spanish
    /// (proper nouns and universal terms).
    #[test]
    fn es419_npc_weapon_2_keys_are_translated() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};
        const FILES: &[&str] = &["item/weapon/npc.ftl"];
        const KEYS: &[&str] = &[
            "common-items-npc_weapons-unique-haniwa_sentry",
            "common-items-npc_weapons-unique-hermit_alligator",
            "common-items-npc_weapons-unique-husk",
            "common-items-npc_weapons-unique-husk_brute",
            "common-items-npc_weapons-unique-irrwurz",
            "common-items-npc_weapons-unique-mossysnail",
            "common-items-npc_weapons-unique-organ",
            "common-items-npc_weapons-unique-quadlowbasic",
            "common-items-npc_weapons-unique-quadlowbeam",
            "common-items-npc_weapons-unique-quadlowbreathe",
            "common-items-npc_weapons-unique-quadlowquick",
            "common-items-npc_weapons-unique-quadlowtail",
            "common-items-npc_weapons-unique-quadmedbasic",
            "common-items-npc_weapons-unique-quadmedbasicgentle",
            "common-items-npc_weapons-unique-quadmedcharge",
            "common-items-npc_weapons-unique-quadmedhoof",
            "common-items-npc_weapons-unique-quadmedjump",
            "common-items-npc_weapons-unique-quadmedquick",
            "common-items-npc_weapons-unique-quadsmallbasic",
            "common-items-npc_weapons-unique-quadsmall_long_range",
            "common-items-npc_weapons-unique-darkhound",
            "common-items-npc_weapons-unique-rocksnapper",
            "common-items-npc_weapons-unique-sea_bishop_sceptre",
            "common-items-npc_weapons-unique-seawyvern",
            "common-items-npc_weapons-unique-simpleflyingbasic",
            "common-items-npc_weapons-unique-stone_golems_fist",
            "common-items-npc_weapons-unique-theropodbasic",
            "common-items-npc_weapons-unique-theropodbird",
            "common-items-npc_weapons-unique-theropodcharge",
            "common-items-npc_weapons-unique-theropodsmall",
            "common-items-npc_weapons-unique-tidal_spear",
            "common-items-npc_weapons-unique-tidal_totem",
            "common-items-npc_weapons-unique-treantsapling",
            "common-items-npc_weapons-unique-turret",
            "common-items-npc_weapons-unique-tursus_claws",
            "common-items-npc_weapons-unique-wealdwyvern",
            "common-items-npc_weapons-unique-wendigo_magic",
            "common-items-npc_weapons-unique-wood_golem_fist",
            "common-items-npc_weapons-unique-ancient_effigy_eyes",
            "common-items-npc_weapons-unique-claysteed",
            "common-items-npc_weapons-unique-gravewarden_fist",
            "common-items-npc_weapons-unique-arthropods-antlion",
            "common-items-npc_weapons-unique-arthropods-blackwidow",
            "common-items-npc_weapons-unique-arthropods-cavespider",
            "common-items-npc_weapons-unique-arthropods-dagonite",
            "common-items-npc_weapons-unique-arthropods-hornbeetle",
            "common-items-npc_weapons-unique-arthropods-leafbeetle",
            "common-items-npc_weapons-unique-arthropods-moltencrawler",
            "common-items-npc_weapons-unique-arthropods-mosscrawler",
            "common-items-npc_weapons-unique-arthropods-tarantula",
            "common-items-npc_weapons-unique-arthropods-weevil",
            "common-items-npc_weapons-unique-quadruped_low-asp",
            "common-items-npc_weapons-unique-quadruped_low-basilisk",
            "common-items-npc_weapons-unique-quadruped_low-deadwood",
            "common-items-npc_weapons-unique-quadruped_low-icedrake",
            "common-items-npc_weapons-unique-quadruped_low-lavadrake",
            "common-items-npc_weapons-unique-quadruped_low-maneater",
            "common-items-npc_weapons-unique-quadruped_low-tortoise",
            "common-items-npc_weapons-unique-quadruped_low-hydra",
            "common-items-npc_weapons-unique-quadruped_medium-elephant",
            "common-items-npc_weapons-unique-quadruped_medium-antelope",
            "common-items-npc_weapons-unique-quadruped_medium-donkey",
            "common-items-npc_weapons-unique-quadruped_medium-highland",
            "common-items-npc_weapons-unique-quadruped_medium-horse",
            "common-items-npc_weapons-unique-quadruped_medium-moose",
            "common-items-npc_weapons-unique-quadruped_medium-mouflon",
            "common-items-npc_weapons-unique-quadruped_medium-wolf",
            "common-items-npc_weapons-unique-quadruped_small-boar",
            "common-items-npc_weapons-unique-quadruped_small-hyena",
            "common-items-npc_weapons-unique-quadruped_small-rodent",
            "common-items-npc_weapons-hammer-bipedlarge-cultist",
            "common-items-npc_weapons-hammer-cyclops_hammer",
            "common-items-npc_weapons-hammer-harvester_scythe",
            "common-items-npc_weapons-hammer-ogre_hammer",
            "common-items-npc_weapons-hammer-oni_red_hammer",
            "common-items-npc_weapons-hammer-troll_hammer",
            "common-items-npc_weapons-hammer-wendigo_hammer",
            "common-items-npc_weapons-hammer-yeti_hammer",
            "common-items-npc_weapons-hammer-terracotta_punisher_club",
            "common-items-npc_weapons-unique-bloodmoon_bat",
            "common-items-npc_weapons-unique-vampire_bat",
            "common-items-npc_weapons-unique-strigoi_claws",
        ];
        let es = Localization::load(&parse_locale("es-419"), FILES);
        let en = Localization::load(&fallback_locale(), FILES);
        for &k in KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            assert_ne!(
                v,
                en.tr(k),
                "{k}: es-419 is byte-identical to en (untranslated?)"
            );
        }
    }

    /// EM-5.20 drift guard: es-419 stays at parity with en for every batched
    /// player-facing catalog. A new en key added without its es-419
    /// translation (or a copy-pasted English value) fails here. Union of
    /// every per-batch `KEYS`/`FILES`/`COGNATE_ALLOWLIST` array above,
    /// deduplicated (the `item/items/internal.ftl` INTERNAL batch is
    /// intentionally excluded — non-player-facing, not translated).
    #[test]
    fn es419_reaches_en_parity_for_batched_catalogs() {
        use xindeler_ui::i18n::{Localization, fallback_locale, parse_locale};

        const ALL_EM520_FILES: &[&str] = &[
            "buff.ftl",
            "hud/misc.ftl",
            "hud/sct.ftl",
            "common.ftl",
            "esc_menu.ftl",
            "hud/settings.ftl",
            "main.ftl",
            "gameinput.ftl",
            "hud/controls.ftl",
            "hud/bag.ftl",
            "hud/char_window.ftl",
            "hud/chat.ftl",
            "hud/crafting.ftl",
            "hud/map.ftl",
            "hud/quest.ftl",
            "hud/skills.ftl",
            "hud/ability.ftl",
            "hud/social.ftl",
            "hud/group.ftl",
            "hud/trade.ftl",
            "hud/combat_hud.ftl",
            "char_selection.ftl",
            "hud/subtitles.ftl",
            "command.ftl",
            "item/armor/armor.ftl",
            "item/weapon/weapon.ftl",
            "item/items/quest.ftl",
            "tutorial.ftl",
            "spell/axiomancy.ftl",
            "spell/hemomancy.ftl",
            "spell/abjuration.ftl",
            "quest/courier_quests.ftl",
            "item/armor/npc.ftl",
            "item/weapon/npc.ftl",
        ];

        const ALL_EM520_KEYS: &[&str] = &[
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
            "esc_menu-title",
            "esc_menu-servers_soon",
            "esc_menu-logout_soon",
            "character_window-character_level",
            "character_window-character_xp",
            "character_window-character_health",
            "character_window-character_energy",
            "character_window-character_poise",
            "character_window-character_combo",
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
            "command-adminify-already-has-no-role",
            "command-adminify-already-has-role",
            "command-adminify-assign-higher-than-own",
            "command-adminify-cannot-find-player",
            "command-adminify-reassign-to-above",
            "command-adminify-removed-role",
            "command-adminify-role-downgraded",
            "command-adminify-role-upgraded",
            "command-aura-invalid-buff-parameters",
            "command-aura-spawn",
            "command-aura-spawn-new-entity",
            "command-ban-added",
            "command-ban-already-added",
            "command-ban-ip-added",
            "command-ban-ip-queued",
            "command-battlemode-available-modes",
            "command-battlemode-cooldown",
            "command-battlemode-intown",
            "command-battlemode-same",
            "command-battlemode-updated",
            "command-buff-body-unknown",
            "command-buff-data",
            "command-buff-unknown",
            "command-cannot-send-message-hidden",
            "command-client-has-no-socketaddr",
            "command-death_effect-unknown",
            "command-destroyed-no-tethers",
            "command-destroyed-tethers",
            "command-disabled-by-settings",
            "command-disconnectall-confirm",
            "command-dismounted",
            "command-entity-has-no-client",
            "command-experimental-shaders-not-supported",
            "command-experimental-terrain-persistence-disabled",
            "command-explosion-power-too-high",
            "command-explosion-power-too-low",
            "command-faction-join",
            "command-give_item_quality-desc",
            "command-give_item_quality-success",
            "command-group_invite-invited-to-group",
            "command-group_invite-invited-to-your-group",
            "command-group-join",
            "command-into_npc-warning",
            "command-invalid-alignment",
            "command-invalid-skill-group",
            "command-inventory-cant-fit-item",
            "command-kick-higher-role",
            "command-kit-inventory-unavailable",
            "command-kit-not-enough-slots",
            "command-lantern-adjusted-strength",
            "command-lantern-adjusted-strength-color",
            "command-lantern-unequiped",
            "command-location-created",
            "command-location-deleted",
            "command-location-duplicate",
            "command-location-invalid",
            "command-location-not-found",
            "command-locations-empty",
            "command-locations-list",
            "command-make_party-desc",
            "command-make_test_char-desc",
            "command-message-group-missing",
            "command-no-buid-perms",
            "command-no-dismount",
            "command-outcome-expected_body_arg",
            "command-outcome-expected_entity_arg",
            "command-outcome-expected_frontent_specifier",
            "command-outcome-expected_integer",
            "command-outcome-expected_skill_group_kind",
            "command-outcome-expected_sprite_kind",
            "command-outcome-invalid_outcome",
            "command-outcome-variant_expected",
            "command-parse-duration-error",
            "command-permit-build-given",
            "command-permit-build-granted",
            "command-player-info-unavailable",
            "command-reloaded-chunks",
            "command-repaired-inventory_items",
            "command-repaired-items",
            "command-respawn-no-waypoint",
            "command-revoke-build",
            "command-revoke-build-all",
            "command-revoke-build-recv",
            "command-revoked-all-build",
            "command-scale-set",
            "command-server-no-experimental-terrain-persistence",
            "command-set_class-desc",
            "command-set_ethos-desc",
            "command-set_level-desc",
            "command-set_motd-message-added",
            "command-set_motd-message-not-set",
            "command-set_motd-message-removed",
            "command-set-build-mode-off",
            "command-set-build-mode-on-persistent",
            "command-set-build-mode-on-unpersistent",
            "command-set-waypoint-result",
            "command-site-not-found",
            "command-skillpreset-broken",
            "command-skillpreset-load-error",
            "command-skillpreset-missing",
            "command-spawned-campfire",
            "command-spawned-safezone",
            "command-spot-spot_not_found",
            "command-spot-world_feature",
            "command-sudo-higher-role",
            "command-sudo-no-permission-for-non-players",
            "command-tell-to-yourself",
            "command-time_scale-changed",
            "command-time_scale-current",
            "command-transform-invalid-presence",
            "command-unban-already-unbanned",
            "command-unban-ip-successful",
            "command-unban-successful",
            "command-unimplemented-spawn-special",
            "command-unknown",
            "command-version-current",
            "command-volume-created",
            "command-volume-size-incorrect",
            "command-waypoint-error",
            "command-waypoint-result",
            "command-weather-valid-values",
            "command-whitelist-added",
            "command-whitelist-already-added",
            "command-whitelist-permission-denied",
            "command-whitelist-removed",
            "command-whitelist-unlisted",
            "command-you-dont-exist",
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
            "common-abilities-bow-ardent_hunt_clear",
            "common-abilities-bow-storm_chaser",
            "common-abilities-bow-heartseeker_shot",
            "common-abilities-bow-burning_heartseeker_shot",
            "common-abilities-bow-poison_heartseeker_shot",
            "common-abilities-bow-freezing_heartseeker_shot",
            "common-abilities-bow-lightning_heartseeker_shot",
            "common-abilities-bow-burning_hawkstrike_shot",
            "common-abilities-bow-poison_hawkstrike_shot",
            "common-abilities-bow-freezing_hawkstrike_shot",
            "common-abilities-bow-lightning_hawkstrike_shot",
            "common-abilities-spells-arcane-cinderbolt",
            "common-abilities-spells-divine-dawnmote",
            "common-abilities-spells-primal-thornspit",
            "innate-human",
            "innate-elf",
            "innate-dwarf",
            "innate-orc",
            "innate-danari",
            "innate-draugr",
            "class-warrior-rally",
            "class-warrior-onslaught",
            "class-mage-arcanesurge",
            "class-mage-arcanemastery",
            "class-cleric-mendinglight",
            "class-cleric-radiantchannel",
            "class-rogue-ambush",
            "class-rogue-vanish",
            "common-abilities-bow-thorn_stake",
            "common-abilities-bow-burning_thorn_stake",
            "common-abilities-bow-freezing_thorn_stake",
            "common-abilities-bow-poison_thorn_stake",
            "common-abilities-bow-lightning_thorn_stake",
            "buff-shielded",
            "buff-bleeding_mark",
            "buff-difficult_terrain",
            "buff-freedom_of_movement",
            "buff-antimagic",
            "buff-anchored",
            "buff-asleep",
            "buff-blinded",
            "buff-slowed",
            "buff-stormchaser",
            "buff-terrified",
            "buff-charmed",
            "buff-hollowtouched",
            "buff-ardenthunt",
            "buff-ignitearrow",
            "buff-freezearrow",
            "buff-drencharrow",
            "buff-joltarrow",
            "hud-skill_tree-class",
            "hud-skill_tree-class-title",
            "hud-skill_tree-class-empty",
            "hud-skill_tree-feats",
            "hud-skill_tree-node_passive",
            "hud-skill_tree-node_maxed",
            "hud-skill_tree-node_level",
            "hud-skill_tree-node_cost",
            "hud-skill_tree-node_locked",
            "hud-skill-class-warrior-hardened_body_title",
            "hud-skill-class-warrior-hardened_body",
            "hud-skill-class-warrior-practiced_strikes_title",
            "hud-skill-class-warrior-practiced_strikes",
            "hud-skill-class-warrior-rally_title",
            "hud-skill-class-warrior-rally",
            "hud-skill-class-warrior-iron_skin_title",
            "hud-skill-class-warrior-iron_skin",
            "hud-skill-class-warrior-brutal_edge_title",
            "hud-skill-class-warrior-brutal_edge",
            "hud-skill-class-warrior-crushing_blows_title",
            "hud-skill-class-warrior-crushing_blows",
            "hud-skill-class-warrior-stalwart_title",
            "hud-skill-class-warrior-stalwart",
            "hud-skill-class-warrior-sundering_force_title",
            "hud-skill-class-warrior-sundering_force",
            "hud-skill-class-warrior-stagger_title",
            "hud-skill-class-warrior-stagger",
            "hud-skill-class-warrior-battle_momentum_title",
            "hud-skill-class-warrior-battle_momentum",
            "hud-skill-class-warrior-bulwark_stance_title",
            "hud-skill-class-warrior-bulwark_stance",
            "hud-skill-class-warrior-onslaught_title",
            "hud-skill-class-warrior-onslaught",
            "hud-skill-class-mage-focused_mind_title",
            "hud-skill-class-mage-focused_mind",
            "hud-skill-class-mage-true_aim_title",
            "hud-skill-class-mage-true_aim",
            "hud-skill-class-mage-arcane_surge_title",
            "hud-skill-class-mage-arcane_surge",
            "hud-skill-class-mage-spell_potency_title",
            "hud-skill-class-mage-spell_potency",
            "hud-skill-class-mage-pyromantic_attunement_title",
            "hud-skill-class-mage-pyromantic_attunement",
            "hud-skill-class-mage-cryomantic_attunement_title",
            "hud-skill-class-mage-cryomantic_attunement",
            "hud-skill-class-mage-quick_casting_title",
            "hud-skill-class-mage-quick_casting",
            "hud-skill-class-mage-penetrating_magic_title",
            "hud-skill-class-mage-penetrating_magic",
            "hud-skill-class-mage-warded_skin_title",
            "hud-skill-class-mage-warded_skin",
            "hud-skill-class-mage-mana_efficiency_title",
            "hud-skill-class-mage-mana_efficiency",
            "hud-skill-class-mage-overcharge_title",
            "hud-skill-class-mage-overcharge",
            "hud-skill-class-mage-arcane_mastery_title",
            "hud-skill-class-mage-arcane_mastery",
            "hud-skill-class-cleric-faithful_vigor_title",
            "hud-skill-class-cleric-faithful_vigor",
            "hud-skill-class-cleric-devout_focus_title",
            "hud-skill-class-cleric-devout_focus",
            "hud-skill-class-cleric-mending_light_title",
            "hud-skill-class-cleric-mending_light",
            "hud-skill-class-cleric-blessed_aim_title",
            "hud-skill-class-cleric-blessed_aim",
            "hud-skill-class-cleric-sacred_wards_title",
            "hud-skill-class-cleric-sacred_wards",
            "hud-skill-class-cleric-steadfast_faith_title",
            "hud-skill-class-cleric-steadfast_faith",
            "hud-skill-class-cleric-purifying_grace_title",
            "hud-skill-class-cleric-purifying_grace",
            "hud-skill-class-cleric-divine_conduit_title",
            "hud-skill-class-cleric-divine_conduit",
            "hud-skill-class-cleric-smiting_strikes_title",
            "hud-skill-class-cleric-smiting_strikes",
            "hud-skill-class-cleric-armor_of_faith_title",
            "hud-skill-class-cleric-armor_of_faith",
            "hud-skill-class-cleric-aegis_title",
            "hud-skill-class-cleric-aegis",
            "hud-skill-class-cleric-radiant_channel_title",
            "hud-skill-class-cleric-radiant_channel",
            "hud-skill-class-rogue-lithe_title",
            "hud-skill-class-rogue-lithe",
            "hud-skill-class-rogue-keen_edge_title",
            "hud-skill-class-rogue-keen_edge",
            "hud-skill-class-rogue-ambush_title",
            "hud-skill-class-rogue-ambush",
            "hud-skill-class-rogue-deadly_precision_title",
            "hud-skill-class-rogue-deadly_precision",
            "hud-skill-class-rogue-fleet_footed_title",
            "hud-skill-class-rogue-fleet_footed",
            "hud-skill-class-rogue-sure_strike_title",
            "hud-skill-class-rogue-sure_strike",
            "hud-skill-class-rogue-find_the_gap_title",
            "hud-skill-class-rogue-find_the_gap",
            "hud-skill-class-rogue-quick_hands_title",
            "hud-skill-class-rogue-quick_hands",
            "hud-skill-class-rogue-toxin_tolerance_title",
            "hud-skill-class-rogue-toxin_tolerance",
            "hud-skill-class-rogue-opportunist_title",
            "hud-skill-class-rogue-opportunist",
            "hud-skill-class-rogue-shadowstep_title",
            "hud-skill-class-rogue-shadowstep",
            "hud-skill-class-rogue-vanish_title",
            "hud-skill-class-rogue-vanish",
            "hud-feat-athlete_title",
            "hud-feat-charger_title",
            "hud-feat-crusher_title",
            "hud-feat-crossbow_expert_title",
            "hud-feat-defensive_duelist_title",
            "hud-feat-dual_wielder_title",
            "hud-feat-great_weapon_master_title",
            "hud-feat-heavy_armor_master_title",
            "hud-feat-mage_slayer_title",
            "hud-feat-mobile_title",
            "hud-feat-piercer_title",
            "hud-feat-polearm_master_title",
            "hud-feat-savage_attacker_title",
            "hud-feat-sentinel_title",
            "hud-feat-sharpshooter_title",
            "hud-feat-shield_master_title",
            "hud-feat-slasher_title",
            "hud-feat-speedy_title",
            "hud-feat-tavern_brawler_title",
            "hud-feat-aberrant_bloodmark_title",
            "hud-feat-arcane_college_initiate_title",
            "hud-feat-artificer_initiate_title",
            "hud-feat-elemental_adept_title",
            "hud-feat-frost_caster_title",
            "hud-feat-genie_magic_title",
            "hud-feat-gift_of_the_chromatic_dragon_title",
            "hud-feat-gift_of_the_gem_dragon_title",
            "hud-feat-gift_of_the_metallic_dragon_title",
            "hud-feat-greater_aberrant_bloodmark_title",
            "hud-feat-magic_initiate_title",
            "hud-feat-mythal_touched_title",
            "hud-feat-spell_sniper_title",
            "hud-feat-spellfire_adept_title",
            "hud-feat-spellfire_spark_title",
            "hud-feat-telekinetic_title",
            "hud-feat-telepathic_title",
            "hud-feat-umbra_touched_title",
            "hud-feat-veil_touched_title",
            "hud-feat-war_caster_title",
            "hud-feat-fairy_trickster_title",
            "hud-feat-inspiring_leader_title",
            "hud-feat-lordly_resolve_title",
            "hud-feat-tireless_reveler_title",
            "hud-feat-alert_title",
            "hud-feat-chef_title",
            "hud-feat-child_of_the_sun_title",
            "hud-feat-dungeon_delver_title",
            "hud-feat-healer_title",
            "hud-feat-observant_title",
            "hud-feat-shadowmoor_hexer_title",
            "hud-feat-bombardier_title",
            "hud-feat-draconic_cult_initiate_title",
            "hud-feat-dragonscarred_title",
            "hud-feat-orders_resilience_title",
            "hud-feat-poisoner_title",
            "hud-feat-quicksmith_title",
            "hud-feat-strike_of_the_giants_title",
            "hud-feat-vampire_hunter_title",
            "hud-feat-bloodlust_title",
            "hud-feat-cloying_mists_title",
            "hud-feat-delicious_pain_title",
            "hud-feat-durable_title",
            "hud-feat-light_bringer_title",
            "hud-feat-love_bites_title",
            "hud-feat-lucky_title",
            "hud-feat-putrefy_title",
            "hud-feat-rebuke_title",
            "hud-feat-resilient_title",
            "hud-feat-tough_title",
            "hud-feat-treacherous_allure_title",
            "hud-feat-vampire_touched_title",
            "hud-feat-vampires_plaything_title",
            "tutorial-Move",
            "tutorial-Jump",
            "tutorial-OpenInventory",
            "tutorial-FallDamage",
            "tutorial-OpenGlider",
            "tutorial-Glider",
            "tutorial-StallGlider",
            "tutorial-Roll",
            "tutorial-Attacked",
            "tutorial-Unwield",
            "tutorial-Campfire",
            "tutorial-Waypoint",
            "tutorial-OpenDiary",
            "tutorial-FullInventory",
            "tutorial-RespawnDurability",
            "tutorial-RecipeAvailable",
            "tutorial-EnergyLow",
            "tutorial-Chat",
            "tutorial-Sneak",
            "tutorial-Lantern",
            "tutorial-Zoom",
            "tutorial-FirstPerson",
            "tutorial-Swim",
            "tutorial-OpenMap",
            "tutorial-UseItem",
            "tutorial-Crafting",
            "achievement-Moved",
            "achievement-Jumped",
            "achievement-OpenInventory",
            "achievement-OpenGlider",
            "achievement-StallGlider",
            "achievement-Rolled",
            "achievement-Wield",
            "achievement-Unwield",
            "achievement-FindCampfire",
            "achievement-SetWaypoint",
            "achievement-OpenDiary",
            "achievement-FullInventory",
            "achievement-Respawned",
            "achievement-RecipeAvailable",
            "achievement-OpenCrafting",
            "achievement-EnergyLow",
            "achievement-ReceivedChatMsg",
            "achievement-NearEnemies",
            "achievement-InDark",
            "achievement-UsedLantern",
            "achievement-Swim",
            "achievement-Zoom",
            "achievement-OpenMap",
            "spell-antimagic_field",
            "spell-antimagic_field-desc",
            "spell-sapping_sting",
            "spell-sapping_sting-desc",
            "spell-magnify_gravity",
            "spell-magnify_gravity-desc",
            "spell-gift_of_alacrity",
            "spell-gift_of_alacrity-desc",
            "spell-fortunes_favor",
            "spell-fortunes_favor-desc",
            "spell-immovable_object",
            "spell-immovable_object-desc",
            "spell-wristpocket",
            "spell-wristpocket-desc",
            "spell-pulse_wave",
            "spell-pulse_wave-desc",
            "spell-gravity_sinkhole",
            "spell-gravity_sinkhole-desc",
            "spell-temporal_shunt",
            "spell-temporal_shunt-desc",
            "spell-gravity_fissure",
            "spell-gravity_fissure-desc",
            "spell-tether_essence",
            "spell-tether_essence-desc",
            "spell-dark_star",
            "spell-dark_star-desc",
            "spell-reality_break",
            "spell-reality_break-desc",
            "spell-ravenous_void",
            "spell-ravenous_void-desc",
            "spell-time_ravage",
            "spell-time_ravage-desc",
            "spell-bloodlet",
            "spell-bloodlet-desc",
            "spell-clot",
            "spell-clot-desc",
            "spell-crimson_brand",
            "spell-crimson_brand-desc",
            "spell-hemal_spike",
            "spell-hemal_spike-desc",
            "spell-sanguine_ward",
            "spell-sanguine_ward-desc",
            "spell-leeching_grasp",
            "spell-leeching_grasp-desc",
            "spell-bloodboil",
            "spell-bloodboil-desc",
            "spell-scarlet_tether",
            "spell-scarlet_tether-desc",
            "spell-bloodbound_edge",
            "spell-bloodbound_edge-desc",
            "spell-hemorrhage",
            "spell-hemorrhage-desc",
            "spell-blood_mirror",
            "spell-blood_mirror-desc",
            "spell-gory_eruption",
            "spell-gory_eruption-desc",
            "spell-sanguine_surge",
            "spell-sanguine_surge-desc",
            "spell-crimson_lash",
            "spell-crimson_lash-desc",
            "spell-exsanguinate",
            "spell-exsanguinate-desc",
            "spell-vitalic_font",
            "spell-vitalic_font-desc",
            "spell-crimson_tide",
            "spell-crimson_tide-desc",
            "spell-hearts_seizure",
            "spell-hearts_seizure-desc",
            "spell-hemoclysm",
            "spell-hemoclysm-desc",
            "spell-crimson_apotheosis",
            "spell-crimson_apotheosis-desc",
            "spell-the_last_vein",
            "spell-the_last_vein-desc",
            "npc-response-quest-courier-start",
            "npc-response-quest-courier-start_2",
            "npc-response-quest-courier-where",
            "npc-response-quest-courier-thanks",
            "npc-response-quest-courier-generic-insufficient-items",
            "dialogue-question-quest-courier-claim",
            "dialogue-question-quest-courier-where",
            "hud-map-spot-unspecified",
            "spot-name-gnarling-totem",
            "spot-name-unspecified",
            "npc-response-quest-courier-ask",
            "npc-response-quest-spot-courier-ask",
            "dialogue-question-quest-courier-what",
            "dialogue-question-quest-courier-what-target",
            "npc-response-quest-fetch-ask",
            "npc-response-quest-spot-fetch-ask",
            "dialogue-question-quest-fetch-what",
            "npc-response-quest-messenger-ask",
            "npc-response-quest-messenger-what-is-needed",
            "npc-response-quest-messenger-what-is-needed-target",
            "dialogue-question-quest-messenger-what",
            "dialogue-question-quest-messenger-what-target",
            "npc-response-quest-courier-gnarling-carving",
            "npc-response-quest-courier-gnarling-carving-insufficient-items",
            "npc-response-quest-courier-gnarling-carving-what-is-needed",
            "hud-map-spot-gnarling-carving-label",
            "npc-response-quest-courier-legoom-leaf",
            "npc-response-quest-courier-legoom-leaf-insufficient-items",
            "npc-response-quest-courier-legoom-leaf-what-is-needed",
            "npc-response-quest-messenger-send-word",
            "common-items-npc_armor-pants-leather_blue",
            "common-items-npc_armor-pants-plate_red",
            "common-items-npc_armor-bird_large-phoenix",
            "common-items-npc_armor-bird_large-wyvern",
            "common-items-npc_armor-bird_medium-bloodmoon_bat",
            "common-items-npc_armor-golem-claygolem",
            "common-items-npc_armor-golem-woodgolem",
            "common-items-npc_armor-golem-irongolem",
            "common-items-npc_armor-golem-ancienteffigy",
            "common-items-npc_armor-golem-gravewarden",
            "common-items-npc_armor-biped_small-myrmidon-foot-hoplite",
            "common-items-npc_armor-biped_small-myrmidon-foot-marksman",
            "common-items-npc_armor-biped_small-myrmidon-foot-strategian",
            "common-items-npc_armor-biped_small-sahagin-foot-sniper",
            "common-items-npc_armor-biped_small-sahagin-foot-sorcerer",
            "common-items-npc_armor-biped_small-sahagin-foot-spearman",
            "common-items-npc_armor-biped_small-adlet-foot-hunter",
            "common-items-npc_armor-biped_small-adlet-foot-icepicker",
            "common-items-npc_armor-biped_small-adlet-foot-tracker",
            "common-items-npc_armor-biped_small-gnarling-foot-chieftain",
            "common-items-npc_armor-biped_small-boreal-foot-warrior",
            "common-items-npc_armor-biped_small-boreal-head-warrior",
            "common-items-npc_armor-biped_small-boreal-pants-warrior",
            "common-items-npc_armor-biped_small-boreal-chest-warrior",
            "common-items-npc_armor-biped_small-boreal-hand-warrior",
            "common-items-npc_armor-biped_small-ashen-foot-warrior",
            "common-items-npc_armor-biped_small-ashen-head-warrior",
            "common-items-npc_armor-biped_small-ashen-pants-warrior",
            "common-items-npc_armor-biped_small-ashen-chest-warrior",
            "common-items-npc_armor-biped_small-ashen-hand-warrior",
            "common-items-npc_armor-biped_small-treasure_egg-chest-treasure_egg",
            "common-items-npc_armor-biped_small-treasure_egg-foot-treasure_egg",
            "common-items-npc_armor-biped_small-iron_dwarf-foot-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-head-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-pants-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-chest-iron_dwarf",
            "common-items-npc_armor-biped_small-iron_dwarf-hand-iron_dwarf",
            "common-items-npc_armor-biped_small-haniwa-foot-archer",
            "common-items-npc_armor-biped_small-haniwa-foot-guard",
            "common-items-npc_armor-biped_small-haniwa-foot-soldier",
            "common-items-npc_armor-biped_small-haniwa-head-archer",
            "common-items-npc_armor-biped_small-haniwa-head-guard",
            "common-items-npc_armor-biped_small-haniwa-head-soldier",
            "common-items-npc_armor-biped_small-haniwa-pants-archer",
            "common-items-npc_armor-biped_small-haniwa-pants-guard",
            "common-items-npc_armor-biped_small-haniwa-pants-soldier",
            "common-items-npc_armor-biped_small-haniwa-chest-archer",
            "common-items-npc_armor-biped_small-haniwa-chest-guard",
            "common-items-npc_armor-biped_small-haniwa-chest-soldier",
            "common-items-npc_armor-biped_small-haniwa-hand-archer",
            "common-items-npc_armor-biped_small-haniwa-hand-guard",
            "common-items-npc_armor-biped_small-haniwa-hand-soldier",
            "common-items-npc_armor-biped_small-husk-foot-husk",
            "common-items-npc_armor-biped_small-husk-head-husk",
            "common-items-npc_armor-biped_small-husk-pants-husk",
            "common-items-npc_armor-biped_small-husk-chest-husk",
            "common-items-npc_armor-biped_small-husk-hand-husk",
            "common-items-npc_armor-biped_small-husk-tail-husk",
            "common-items-npc_armor-biped_small-flamekeeper-foot-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-head-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-pants-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-chest-flamekeeper",
            "common-items-npc_armor-biped_small-flamekeeper-hand-flamekeeper",
            "common-items-npc_armor-biped_small-gnoll-foot-rogue",
            "common-items-npc_armor-biped_small-gnoll-foot-shaman",
            "common-items-npc_armor-biped_small-gnoll-foot-trapper",
            "common-items-npc_armor-biped_small-gnoll-head-rogue",
            "common-items-npc_armor-biped_small-gnoll-head-shaman",
            "common-items-npc_armor-biped_small-gnoll-head-trapper",
            "common-items-npc_armor-biped_small-gnoll-pants-rogue",
            "common-items-npc_armor-biped_small-gnoll-pants-shaman",
            "common-items-npc_armor-biped_small-gnoll-pants-trapper",
            "common-items-npc_armor-biped_small-gnoll-chest-rogue",
            "common-items-npc_armor-biped_small-gnoll-chest-shaman",
            "common-items-npc_armor-biped_small-gnoll-chest-trapper",
            "common-items-npc_armor-biped_small-gnoll-hand-rogue",
            "common-items-npc_armor-biped_small-gnoll-hand-shaman",
            "common-items-npc_armor-biped_small-gnoll-hand-trapper",
            "common-items-npc_armor-biped_small-gnoll-tail-rogue",
            "common-items-npc_armor-biped_small-gnoll-tail-shaman",
            "common-items-npc_armor-biped_small-gnoll-tail-trapper",
            "common-items-npc_armor-biped_small-shamanic_spirit-head-shamanic_spirit",
            "common-items-npc_armor-biped_small-shamanic_spirit-chest-shamanic_spirit",
            "common-items-npc_armor-biped_small-shamanic_spirit-pants-shamanic_spirit",
            "common-items-npc_armor-biped_small-shamanic_spirit-hand-shamanic_spirit",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-head-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-chest-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-pants-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-hand-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodmoon_heiress-foot-bloodmoon_heiress",
            "common-items-npc_armor-biped_small-bloodservant-head-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-chest-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-pants-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-hand-bloodservant",
            "common-items-npc_armor-biped_small-bloodservant-foot-bloodservant",
            "common-items-npc_armor-biped_small-harlequin-head-harlequin",
            "common-items-npc_armor-biped_small-harlequin-chest-harlequin",
            "common-items-npc_armor-biped_small-harlequin-pants-harlequin",
            "common-items-npc_armor-biped_small-harlequin-hand-harlequin",
            "common-items-npc_armor-biped_small-harlequin-foot-harlequin",
            "common-items-npc_armor-biped_small-goblin_thug-head-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-chest-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-pants-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-hand-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_thug-foot-goblin_thug",
            "common-items-npc_armor-biped_small-goblin_chucker-head-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-chest-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-pants-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-hand-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_chucker-foot-goblin_chucker",
            "common-items-npc_armor-biped_small-goblin_ruffian-head-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-chest-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-pants-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-hand-goblin_ruffian",
            "common-items-npc_armor-biped_small-goblin_ruffian-foot-goblin_ruffian",
            "common-items-npc_armor-biped_small-green_legoom-head-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-chest-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-pants-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-hand-green_legoom",
            "common-items-npc_armor-biped_small-green_legoom-foot-green_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-head-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-chest-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-pants-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-hand-ochre_legoom",
            "common-items-npc_armor-biped_small-ochre_legoom-foot-ochre_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-head-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-chest-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-pants-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-hand-purple_legoom",
            "common-items-npc_armor-biped_small-purple_legoom-foot-purple_legoom",
            "common-items-npc_armor-biped_small-red_legoom-head-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-chest-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-pants-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-hand-red_legoom",
            "common-items-npc_armor-biped_small-red_legoom-foot-red_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-head-umber_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-chest-umber_legoom",
            "common-items-npc_weapons-biped_small-mandragora",
            "common-items-npc_weapons-biped_small-myrmidon-hoplite",
            "common-items-npc_weapons-biped_small-myrmidon-marksman",
            "common-items-npc_weapons-biped_small-myrmidon-strategian",
            "common-items-npc_weapons-biped_small-sahagin-sniper",
            "common-items-npc_weapons-biped_small-sahagin-sorcerer",
            "common-items-npc_weapons-biped_small-sahagin-spearman",
            "common-items-npc_weapons-biped_small-adlet-hunter",
            "common-items-npc_weapons-biped_small-adlet-icepicker",
            "common-items-npc_weapons-biped_small-adlet-tracker",
            "common-items-npc_weapons-biped_small-gnarling-chieftain",
            "common-items-npc_weapons-biped_small-gnarling-greentotem",
            "common-items-npc_weapons-biped_small-gnarling-logger",
            "common-items-npc_weapons-biped_small-gnarling-mugger",
            "common-items-npc_weapons-biped_small-gnarling-redtotem",
            "common-items-npc_weapons-biped_small-gnarling-stalker",
            "common-items-npc_weapons-biped_small-gnarling-whitetotem",
            "common-items-npc_weapons-biped_small-boreal-bow",
            "common-items-npc_weapons-biped_small-boreal-hammer",
            "common-items-npc_weapons-biped_small-ashen-axe",
            "common-items-npc_weapons-biped_small-ashen-staff",
            "common-items-npc_weapons-biped_small-haniwa-archer",
            "common-items-npc_weapons-biped_small-haniwa-guard",
            "common-items-npc_weapons-biped_small-haniwa-soldier",
            "common-items-npc_weapons-biped_small-vampire-harlequin_dagger",
            "common-items-npc_weapons-biped_small-vampire-bloodservant_axe",
            "common-items-npc_weapons-biped_small-vampire-bloodmoon_heiress_sword",
            "common-items-npc_weapons-unique-goblin_thug_club",
            "common-items-npc_weapons-unique-goblin_chucker",
            "common-items-npc_weapons-unique-goblin_ruffian_knife",
            "common-items-npc_weapons-unique-green_legoom_rake",
            "common-items-npc_weapons-unique-ochre_legoom_spade",
            "common-items-npc_weapons-unique-purple_legoom_pitchfork",
            "common-items-npc_weapons-unique-red_legoom_hoe",
            "common-items-npc_weapons-unique-umber_legoom_hook",
            "common-items-npc_weapons-bow-bipedlarge-velorite",
            "common-items-npc_weapons-bow-saurok_bow",
            "common-items-npc_weapons-bow-terracotta_besieger_bow",
            "common-items-npc_weapons-axe-gigas_frost_axe",
            "common-items-npc_weapons-sword-gigas_fire_sword",
            "common-items-npc_weapons-axe-executioner_axe",
            "common-items-npc_weapons-axe-minotaur_axe",
            "common-items-npc_weapons-axe-oni_blue_axe",
            "common-items-npc_weapons-staff-bipedlarge-cultist",
            "common-items-npc_weapons-staff-mindflayer_staff",
            "common-items-npc_weapons-staff-ogre_staff",
            "common-items-npc_weapons-staff-saurok_staff",
            "common-items-npc_weapons-sword-adlet_elder_sword",
            "common-items-npc_weapons-sword-bipedlarge-cultist",
            "common-items-npc_weapons-sword-dullahan_sword",
            "common-items-npc_weapons-sword-pickaxe_velorite_sword",
            "common-items-npc_weapons-sword-saurok_sword",
            "common-items-npc_weapons-sword-haniwa_general_sword",
            "common-items-npc_weapons-sword-terracotta_pursuer_sword",
            "common-items-npc_weapons-unique-akhlut",
            "common-items-npc_weapons-unique-beast_claws",
            "common-items-npc_weapons-unique-birdlargebasic",
            "common-items-npc_weapons-unique-birdlargebreathe",
            "common-items-npc_weapons-unique-birdlargefire",
            "common-items-npc_weapons-unique-birdmediumbasic",
            "common-items-npc_weapons-unique-bushly",
            "common-items-npc_weapons-unique-cactid",
            "common-items-npc_weapons-unique-cardinal",
            "common-items-npc_weapons-unique-clay_golem_fist",
            "common-items-npc_weapons-unique-iron_dwarf",
            "common-items-npc_weapons-unique-cloudwyvern",
            "common-items-npc_weapons-unique-coral_golem_fist",
            "common-items-npc_weapons-unique-crab_pincer",
            "common-items-npc_weapons-unique-karkatha_pincer",
            "common-items-npc_weapons-unique-dagon",
            "common-items-npc_weapons-unique-driggle",
            "common-items-npc_weapons-unique-emberfly",
            "common-items-npc_weapons-unique-fiery_tornado",
            "common-items-npc_weapons-unique-flamekeeper_staff",
            "common-items-npc_weapons-unique-cursekeeper_sceptre",
            "common-items-npc_weapons-unique-cursekeeper_sceptre_fake",
            "common-items-npc_weapons-unique-jiangshi",
            "common-items-npc_weapons-unique-mogwai",
            "common-items-npc_weapons-unique-shamanic_spirit",
            "common-items-npc_weapons-unique-terracotta_demolisher_fist",
            "common-items-npc_weapons-unique-terracotta_statue",
            "common-items-npc_weapons-unique-iron_golem_fist",
            "common-items-npc_weapons-hammer-forgemaster_hammer",
            "common-items-npc_weapons-unique-snaretongue",
            "common-items-npc_weapons-unique-flamethrower",
            "common-items-npc_weapons-unique-flamewyvern",
            "common-items-npc_weapons-unique-frostfang",
            "common-items-npc_weapons-unique-frostwyvern",
            "common-items-npc_armor-biped_small-umber_legoom-pants-umber_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-hand-umber_legoom",
            "common-items-npc_armor-biped_small-umber_legoom-foot-umber_legoom",
            "common-items-npc_armor-crustacean-karkatha",
            "common-items-npc_armor-chest-plate_red",
            "common-items-npc_armor-quadruped_low-basilisk",
            "common-items-npc_armor-quadruped_low-dagon",
            "common-items-npc_armor-quadruped_low-drake",
            "common-items-npc_armor-quadruped_low-shell",
            "common-items-npc_armor-quadruped_low-snapper",
            "common-items-npc_armor-quadruped_medium-tarasque",
            "common-items-npc_armor-quadruped_medium-claysteed",
            "common-items-npc_armor-generic",
            "common-items-npc_armor-generic_high",
            "common-items-npc_armor-theropod-rugged",
            "common-items-npc_armor-biped_large-cyclops",
            "common-items-npc_armor-biped_large-dullahan",
            "common-items-npc_armor-biped_large-generic",
            "common-items-npc_armor-biped_large-gigas_frost",
            "common-items-npc_armor-biped_large-gigas_fire",
            "common-items-npc_armor-biped_large-harvester",
            "common-items-npc_armor-biped_large-mindflayer",
            "common-items-npc_armor-biped_large-minotaur",
            "common-items-npc_armor-biped_large-tidal_warrior",
            "common-items-npc_armor-biped_large-tursus",
            "common-items-npc_armor-biped_large-warlock",
            "common-items-npc_armor-biped_large-warlord",
            "common-items-npc_armor-biped_large-yeti",
            "common-items-npc_armor-biped_large-forgemaster",
            "common-items-npc_armor-biped_large-haniwageneral",
            "common-items-npc_armor-biped_large-terracotta",
            "armor-leather_blue-pants",
            "armor-leather_blue-back",
            "armor-velorite_battlemage-back",
            "armor-velorite_battlemage-belt",
            "armor-velorite_battlemage-chest",
            "armor-velorite_battlemage-foot",
            "armor-velorite_battlemage-hand",
            "armor-velorite_battlemage-pants",
            "armor-velorite_battlemage-shoulder",
            "armor-cardinal-belt",
            "armor-cardinal-chest",
            "armor-cardinal-foot",
            "armor-cardinal-hand",
            "armor-cardinal-mitre",
            "armor-cardinal-pants",
            "armor-cardinal-shoulder",
            "armor-merchant-back",
            "armor-merchant-belt",
            "armor-merchant-chest",
            "armor-merchant-foot",
            "armor-merchant-hand",
            "armor-merchant-pants",
            "armor-merchant-shoulder_l",
            "common-items-armor-alchemist-belt",
            "common-items-armor-alchemist-chest",
            "common-items-armor-alchemist-hat",
            "common-items-armor-alchemist-pants",
            "common-items-armor-misc-head-headband",
            "common-items-armor-witch-back",
            "common-items-armor-witch-belt",
            "common-items-armor-witch-chest",
            "common-items-armor-witch-foot",
            "common-items-armor-witch-hand",
            "common-items-armor-witch-pants",
            "common-items-armor-witch-shoulder",
            "common-items-armor-pirate-belt",
            "common-items-armor-pirate-chest",
            "common-items-armor-pirate-foot",
            "common-items-armor-pirate-hand",
            "common-items-armor-pirate-pants",
            "common-items-armor-pirate-shoulder",
            "common-items-armor-miner-back",
            "common-items-armor-miner-belt",
            "common-items-armor-miner-chest",
            "common-items-armor-miner-foot",
            "common-items-armor-miner-hand",
            "common-items-armor-miner-pants",
            "common-items-armor-miner-shoulder",
            "common-items-armor-miner-shoulder_captain",
            "common-items-armor-miner-shoulder_flame",
            "common-items-armor-miner-shoulder_overseer",
            "common-items-armor-chef-belt",
            "common-items-armor-chef-chest",
            "common-items-armor-chef-hat",
            "common-items-armor-chef-pants",
            "common-items-armor-blacksmith-belt",
            "common-items-armor-blacksmith-chest",
            "common-items-armor-blacksmith-hand",
            "common-items-armor-blacksmith-hat",
            "common-items-armor-blacksmith-pants",
            "common-items-armor-leather_plate-helmet",
            "armor-assassin-belt",
            "armor-assassin-chest",
            "armor-assassin-foot",
            "armor-assassin-hand",
            "armor-misc-head-assa_mask-0",
            "armor-assassin-pants",
            "armor-assassin-shoulder",
            "armor-ferocious-back",
            "armor-ferocious-belt",
            "armor-ferocious-chest",
            "armor-ferocious-foot",
            "armor-ferocious-hand",
            "armor-ferocious-pants",
            "armor-ferocious-shoulder",
            "armor-bonerattler-belt",
            "armor-bonerattler-chest",
            "armor-bonerattler-foot",
            "armor-bonerattler-hand",
            "armor-bonerattler-pants",
            "armor-bonerattler-shoulder",
            "armor-misc-head-exclamation",
            "armor-misc-foot-iceskate",
            "armor-misc-foot-jackalope",
            "armor-misc-foot-ski",
            "armor-miner-helmet",
            "common-items-npc_weapons-unique-haniwa_sentry",
            "common-items-npc_weapons-unique-hermit_alligator",
            "common-items-npc_weapons-unique-husk",
            "common-items-npc_weapons-unique-husk_brute",
            "common-items-npc_weapons-unique-irrwurz",
            "common-items-npc_weapons-unique-mossysnail",
            "common-items-npc_weapons-unique-organ",
            "common-items-npc_weapons-unique-quadlowbasic",
            "common-items-npc_weapons-unique-quadlowbeam",
            "common-items-npc_weapons-unique-quadlowbreathe",
            "common-items-npc_weapons-unique-quadlowquick",
            "common-items-npc_weapons-unique-quadlowtail",
            "common-items-npc_weapons-unique-quadmedbasic",
            "common-items-npc_weapons-unique-quadmedbasicgentle",
            "common-items-npc_weapons-unique-quadmedcharge",
            "common-items-npc_weapons-unique-quadmedhoof",
            "common-items-npc_weapons-unique-quadmedjump",
            "common-items-npc_weapons-unique-quadmedquick",
            "common-items-npc_weapons-unique-quadsmallbasic",
            "common-items-npc_weapons-unique-quadsmall_long_range",
            "common-items-npc_weapons-unique-darkhound",
            "common-items-npc_weapons-unique-rocksnapper",
            "common-items-npc_weapons-unique-sea_bishop_sceptre",
            "common-items-npc_weapons-unique-seawyvern",
            "common-items-npc_weapons-unique-simpleflyingbasic",
            "common-items-npc_weapons-unique-stone_golems_fist",
            "common-items-npc_weapons-unique-theropodbasic",
            "common-items-npc_weapons-unique-theropodbird",
            "common-items-npc_weapons-unique-theropodcharge",
            "common-items-npc_weapons-unique-theropodsmall",
            "common-items-npc_weapons-unique-tidal_spear",
            "common-items-npc_weapons-unique-tidal_totem",
            "common-items-npc_weapons-unique-treantsapling",
            "common-items-npc_weapons-unique-turret",
            "common-items-npc_weapons-unique-tursus_claws",
            "common-items-npc_weapons-unique-wealdwyvern",
            "common-items-npc_weapons-unique-wendigo_magic",
            "common-items-npc_weapons-unique-wood_golem_fist",
            "common-items-npc_weapons-unique-ancient_effigy_eyes",
            "common-items-npc_weapons-unique-claysteed",
            "common-items-npc_weapons-unique-gravewarden_fist",
            "common-items-npc_weapons-unique-arthropods-antlion",
            "common-items-npc_weapons-unique-arthropods-blackwidow",
            "common-items-npc_weapons-unique-arthropods-cavespider",
            "common-items-npc_weapons-unique-arthropods-dagonite",
            "common-items-npc_weapons-unique-arthropods-hornbeetle",
            "common-items-npc_weapons-unique-arthropods-leafbeetle",
            "common-items-npc_weapons-unique-arthropods-moltencrawler",
            "common-items-npc_weapons-unique-arthropods-mosscrawler",
            "common-items-npc_weapons-unique-arthropods-tarantula",
            "common-items-npc_weapons-unique-arthropods-weevil",
            "common-items-npc_weapons-unique-quadruped_low-asp",
            "common-items-npc_weapons-unique-quadruped_low-basilisk",
            "common-items-npc_weapons-unique-quadruped_low-deadwood",
            "common-items-npc_weapons-unique-quadruped_low-icedrake",
            "common-items-npc_weapons-unique-quadruped_low-lavadrake",
            "common-items-npc_weapons-unique-quadruped_low-maneater",
            "common-items-npc_weapons-unique-quadruped_low-tortoise",
            "common-items-npc_weapons-unique-quadruped_low-hydra",
            "common-items-npc_weapons-unique-quadruped_medium-elephant",
            "common-items-npc_weapons-unique-quadruped_medium-antelope",
            "common-items-npc_weapons-unique-quadruped_medium-donkey",
            "common-items-npc_weapons-unique-quadruped_medium-highland",
            "common-items-npc_weapons-unique-quadruped_medium-horse",
            "common-items-npc_weapons-unique-quadruped_medium-moose",
            "common-items-npc_weapons-unique-quadruped_medium-mouflon",
            "common-items-npc_weapons-unique-quadruped_medium-wolf",
            "common-items-npc_weapons-unique-quadruped_small-boar",
            "common-items-npc_weapons-unique-quadruped_small-hyena",
            "common-items-npc_weapons-unique-quadruped_small-rodent",
            "common-items-npc_weapons-hammer-bipedlarge-cultist",
            "common-items-npc_weapons-hammer-cyclops_hammer",
            "common-items-npc_weapons-hammer-harvester_scythe",
            "common-items-npc_weapons-hammer-ogre_hammer",
            "common-items-npc_weapons-hammer-oni_red_hammer",
            "common-items-npc_weapons-hammer-troll_hammer",
            "common-items-npc_weapons-hammer-wendigo_hammer",
            "common-items-npc_weapons-hammer-yeti_hammer",
            "common-items-npc_weapons-hammer-terracotta_punisher_club",
            "common-items-npc_weapons-unique-bloodmoon_bat",
            "common-items-npc_weapons-unique-vampire_bat",
            "common-items-npc_weapons-unique-strigoi_claws",
        ];

        const ALL_EM520_COGNATE_ALLOWLIST: &[&str] = &[
            "character_window-character_xp",
            "character_window-character_combo",
            "main-server_browser-version_label",
            "char_selection-ethos_neutral",
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
            "command-battlemode-available-modes",
            "command-disconnectall-confirm",
            "command-faction-join",
            "command-locations-list",
            "command-make_party-desc",
            "command-make_test_char-desc",
            "command-outcome-expected_frontent_specifier",
            "command-outcome-expected_skill_group_kind",
            "command-set_class-desc",
            "command-set_ethos-desc",
            "command-spot-world_feature",
            "command-weather-valid-values",
            "hud-settings-language",
            "hud-settings-custom_graphics",
            "hud-crafting-tabs-modular",
            "hud-combat_hud-combo_label",
        ];

        let es = Localization::load(&parse_locale("es-419"), ALL_EM520_FILES);
        let en = Localization::load(&fallback_locale(), ALL_EM520_FILES);

        for &k in ALL_EM520_KEYS {
            let v = es.tr(k);
            assert_ne!(
                v, k,
                "{k}: es-419 does not resolve (fell through to bare key)"
            );
            assert!(!v.trim().is_empty(), "{k}: es-419 resolves to empty");
            if !ALL_EM520_COGNATE_ALLOWLIST.contains(&k) {
                assert_ne!(
                    v,
                    en.tr(k),
                    "{k}: es-419 is byte-identical to en (untranslated?)"
                );
            }
        }
    }
}
