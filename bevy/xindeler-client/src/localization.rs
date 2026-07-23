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
}
