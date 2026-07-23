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
}
