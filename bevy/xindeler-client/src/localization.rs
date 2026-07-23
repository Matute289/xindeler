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
}
