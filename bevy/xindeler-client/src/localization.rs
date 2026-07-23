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
}
