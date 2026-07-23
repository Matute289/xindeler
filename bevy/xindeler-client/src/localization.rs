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
