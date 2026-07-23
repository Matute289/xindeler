//! BL-82 EM-5.16 item D — the `Skill -> "<name key>"` i18n lookup the Diary
//! skill-tree node tooltip (`diary`) resolves display names through. The
//! Rust sibling of `buff_i18n::buff_i18n_key`; an exhaustive `match` (the
//! compiler guarantees every current `Skill` variant is mapped), so a node
//! name is never a raw `Debug` identifier.
//!
//! Four sources of truth, all pre-existing except the feats (spec §3):
//! - 89 weapon nodes (Sword/Axe/Hammer/Bow/Staff) -> `hud/ability.ftl` keys,
//!   derived from the legacy `voxygen` diary's per-node `ability_id` with
//!   `.`->`-` (6 Sword variants use the `veloren-core-pseudo_abilities-sword-*`
//!   prefix; `StaffSkill::FireShockwave` uses `...-staff-fireshockwave`, no
//!   underscore — a naming quirk, still resolves).
//! - 19 Sceptre/Climb/Swim/Mining + 6 weapon-unlock + 48 class nodes ->
//!   `hud/skills.ftl` `hud-skill-*_title` / `hud-skill-class-*_title` keys
//!   (from the legacy `skill_strings` family / BL-06).
//! - 72 feats -> `hud/skills.ftl` `hud-feat-*_title` (+ `.desc`) entries newly
//!   authored for this task.
//!
//! `Option` because the match must cover `UnlockGroup(SkillGroupKind)` for
//! non-weapon groups, which are never rendered as tree nodes (mirrors the
//! legacy `unlock_skill_strings` warn->Empty path) -> `None`.

use common::comp::{
    skillset::{
        SkillGroupKind,
        skills::{
            AxeSkill, BowSkill, ClericSkill, ClimbSkill, FeatSkill, HammerSkill, MageSkill,
            MiningSkill, RogueSkill, SceptreSkill, Skill, StaffSkill, SwimSkill, SwordSkill,
            WarriorSkill,
        },
    },
    tool::ToolKind,
};

/// The frozen `.ftl` key resolving `skill`'s display NAME (its `.desc`
/// attribute, where present — only feats have one — carries the tooltip
/// description). Exhaustive over `Skill`: adding a variant upstream is a
/// compile error here until it is mapped. `None` only for the non-weapon
/// `UnlockGroup` variants that never appear as a skill-tree node.
#[must_use]
pub fn skill_i18n_key(skill: Skill) -> Option<&'static str> {
    match skill {
        Skill::Sword(SwordSkill::CrescentSlash) => {
            Some("veloren-core-pseudo_abilities-sword-crescent_slash")
        },
        Skill::Sword(SwordSkill::FellStrike) => {
            Some("veloren-core-pseudo_abilities-sword-fell_strike")
        },
        Skill::Sword(SwordSkill::Skewer) => Some("veloren-core-pseudo_abilities-sword-skewer"),
        Skill::Sword(SwordSkill::Cascade) => Some("veloren-core-pseudo_abilities-sword-cascade"),
        Skill::Sword(SwordSkill::CrossCut) => Some("veloren-core-pseudo_abilities-sword-cross_cut"),
        Skill::Sword(SwordSkill::Finisher) => Some("veloren-core-pseudo_abilities-sword-finisher"),
        Skill::Sword(SwordSkill::HeavySweep) => Some("common-abilities-sword-heavy_sweep"),
        Skill::Sword(SwordSkill::HeavyPommelStrike) => {
            Some("common-abilities-sword-heavy_pommel_strike")
        },
        Skill::Sword(SwordSkill::AgileQuickDraw) => Some("common-abilities-sword-agile_quick_draw"),
        Skill::Sword(SwordSkill::AgileFeint) => Some("common-abilities-sword-agile_feint"),
        Skill::Sword(SwordSkill::DefensiveRiposte) => {
            Some("common-abilities-sword-defensive_riposte")
        },
        Skill::Sword(SwordSkill::DefensiveDisengage) => {
            Some("common-abilities-sword-defensive_disengage")
        },
        Skill::Sword(SwordSkill::CripplingGouge) => Some("common-abilities-sword-crippling_gouge"),
        Skill::Sword(SwordSkill::CripplingHamstring) => {
            Some("common-abilities-sword-crippling_hamstring")
        },
        Skill::Sword(SwordSkill::CleavingWhirlwindSlice) => {
            Some("common-abilities-sword-cleaving_whirlwind_slice")
        },
        Skill::Sword(SwordSkill::CleavingEarthSplitter) => {
            Some("common-abilities-sword-cleaving_earth_splitter")
        },
        Skill::Sword(SwordSkill::HeavyFortitude) => Some("common-abilities-sword-heavy_fortitude"),
        Skill::Sword(SwordSkill::HeavyPillarThrust) => {
            Some("common-abilities-sword-heavy_pillar_thrust")
        },
        Skill::Sword(SwordSkill::AgileDancingEdge) => {
            Some("common-abilities-sword-agile_dancing_edge")
        },
        Skill::Sword(SwordSkill::AgileFlurry) => Some("common-abilities-sword-agile_flurry"),
        Skill::Sword(SwordSkill::DefensiveStalwartSword) => {
            Some("common-abilities-sword-defensive_stalwart_sword")
        },
        Skill::Sword(SwordSkill::DefensiveDeflect) => {
            Some("common-abilities-sword-defensive_deflect")
        },
        Skill::Sword(SwordSkill::CripplingEviscerate) => {
            Some("common-abilities-sword-crippling_eviscerate")
        },
        Skill::Sword(SwordSkill::CripplingBloodyGash) => {
            Some("common-abilities-sword-crippling_bloody_gash")
        },
        Skill::Sword(SwordSkill::CleavingBladeFever) => {
            Some("common-abilities-sword-cleaving_blade_fever")
        },
        Skill::Sword(SwordSkill::CleavingSkySplitter) => {
            Some("common-abilities-sword-cleaving_sky_splitter")
        },
        Skill::Axe(AxeSkill::BrutalSwing) => Some("common-abilities-axe-brutal_swing"),
        Skill::Axe(AxeSkill::Berserk) => Some("common-abilities-axe-berserk"),
        Skill::Axe(AxeSkill::RisingTide) => Some("common-abilities-axe-rising_tide"),
        Skill::Axe(AxeSkill::SavageSense) => Some("common-abilities-axe-savage_sense"),
        Skill::Axe(AxeSkill::AdrenalineRush) => Some("common-abilities-axe-adrenaline_rush"),
        Skill::Axe(AxeSkill::Execute) => Some("common-abilities-axe-execute"),
        Skill::Axe(AxeSkill::Maelstrom) => Some("common-abilities-axe-maelstrom"),
        Skill::Axe(AxeSkill::Rake) => Some("common-abilities-axe-rake"),
        Skill::Axe(AxeSkill::Bloodfeast) => Some("common-abilities-axe-bloodfeast"),
        Skill::Axe(AxeSkill::FierceRaze) => Some("common-abilities-axe-fierce_raze"),
        Skill::Axe(AxeSkill::Furor) => Some("common-abilities-axe-furor"),
        Skill::Axe(AxeSkill::Fracture) => Some("common-abilities-axe-fracture"),
        Skill::Axe(AxeSkill::Lacerate) => Some("common-abilities-axe-lacerate"),
        Skill::Axe(AxeSkill::Riptide) => Some("common-abilities-axe-riptide"),
        Skill::Axe(AxeSkill::SkullBash) => Some("common-abilities-axe-skull_bash"),
        Skill::Axe(AxeSkill::Sunder) => Some("common-abilities-axe-sunder"),
        Skill::Axe(AxeSkill::Plunder) => Some("common-abilities-axe-plunder"),
        Skill::Axe(AxeSkill::Defiance) => Some("common-abilities-axe-defiance"),
        Skill::Axe(AxeSkill::Keelhaul) => Some("common-abilities-axe-keelhaul"),
        Skill::Axe(AxeSkill::Bulkhead) => Some("common-abilities-axe-bulkhead"),
        Skill::Axe(AxeSkill::Capsize) => Some("common-abilities-axe-capsize"),
        Skill::Hammer(HammerSkill::ScornfulSwipe) => Some("common-abilities-hammer-scornful_swipe"),
        Skill::Hammer(HammerSkill::Tremor) => Some("common-abilities-hammer-tremor"),
        Skill::Hammer(HammerSkill::VigorousBash) => Some("common-abilities-hammer-vigorous_bash"),
        Skill::Hammer(HammerSkill::Retaliate) => Some("common-abilities-hammer-retaliate"),
        Skill::Hammer(HammerSkill::SpineCracker) => Some("common-abilities-hammer-spine_cracker"),
        Skill::Hammer(HammerSkill::Breach) => Some("common-abilities-hammer-breach"),
        Skill::Hammer(HammerSkill::IronTempest) => Some("common-abilities-hammer-iron_tempest"),
        Skill::Hammer(HammerSkill::Upheaval) => Some("common-abilities-hammer-upheaval"),
        Skill::Hammer(HammerSkill::Thunderclap) => Some("common-abilities-hammer-thunderclap"),
        Skill::Hammer(HammerSkill::SeismicShock) => Some("common-abilities-hammer-seismic_shock"),
        Skill::Hammer(HammerSkill::HeavyWhorl) => Some("common-abilities-hammer-heavy_whorl"),
        Skill::Hammer(HammerSkill::Intercept) => Some("common-abilities-hammer-intercept"),
        Skill::Hammer(HammerSkill::PileDriver) => Some("common-abilities-hammer-pile_driver"),
        Skill::Hammer(HammerSkill::LungPummel) => Some("common-abilities-hammer-lung_pummel"),
        Skill::Hammer(HammerSkill::HelmCrusher) => Some("common-abilities-hammer-helm_crusher"),
        Skill::Hammer(HammerSkill::Rampart) => Some("common-abilities-hammer-rampart"),
        Skill::Hammer(HammerSkill::Tenacity) => Some("common-abilities-hammer-tenacity"),
        Skill::Hammer(HammerSkill::Earthshaker) => Some("common-abilities-hammer-earthshaker"),
        Skill::Hammer(HammerSkill::Judgement) => Some("common-abilities-hammer-judgement"),
        Skill::Bow(BowSkill::Foothold) => Some("common-abilities-bow-foothold"),
        Skill::Bow(BowSkill::HeavyNock) => Some("common-abilities-bow-heavy_nock"),
        Skill::Bow(BowSkill::ArdentHunt) => Some("common-abilities-bow-ardent_hunt"),
        Skill::Bow(BowSkill::StormChaser) => Some("common-abilities-bow-storm_chaser"),
        Skill::Bow(BowSkill::EagleEye) => Some("common-abilities-bow-eagle_eye"),
        Skill::Bow(BowSkill::Heartseeker) => Some("common-abilities-bow-heartseeker"),
        Skill::Bow(BowSkill::Hawkstrike) => Some("common-abilities-bow-hawkstrike"),
        Skill::Bow(BowSkill::SepticShot) => Some("common-abilities-bow-septic_shot"),
        Skill::Bow(BowSkill::IgniteArrow) => Some("common-abilities-bow-ignite_arrow"),
        Skill::Bow(BowSkill::DrenchArrow) => Some("common-abilities-bow-drench_arrow"),
        Skill::Bow(BowSkill::FreezeArrow) => Some("common-abilities-bow-freeze_arrow"),
        Skill::Bow(BowSkill::JoltArrow) => Some("common-abilities-bow-jolt_arrow"),
        Skill::Bow(BowSkill::Barrage) => Some("common-abilities-bow-barrage"),
        Skill::Bow(BowSkill::PiercingGale) => Some("common-abilities-bow-piercing_gale"),
        Skill::Bow(BowSkill::ThornStake) => Some("common-abilities-bow-thorn_stake"),
        Skill::Bow(BowSkill::Fusillade) => Some("common-abilities-bow-fusillade"),
        Skill::Bow(BowSkill::DeathVolley) => Some("common-abilities-bow-death_volley"),
        Skill::Staff(StaffSkill::FireShockwave) => Some("common-abilities-staff-fireshockwave"),
        Skill::Staff(StaffSkill::NapalmStrike) => Some("common-abilities-staff-napalm_strike"),
        Skill::Staff(StaffSkill::FlameCloak) => Some("common-abilities-staff-flame_cloak"),
        Skill::Staff(StaffSkill::FireDash) => Some("common-abilities-staff-fire_dash"),
        Skill::Staff(StaffSkill::FireBreath) => Some("common-abilities-staff-fire_breath"),
        Skill::Staff(StaffSkill::Pyroclasm) => Some("common-abilities-staff-pyroclasm"),
        Skill::Sceptre(SceptreSkill::LDamage) => Some("hud-skill-sc_lifesteal_damage_title"),
        Skill::Sceptre(SceptreSkill::LRange) => Some("hud-skill-sc_lifesteal_range_title"),
        Skill::Sceptre(SceptreSkill::LLifesteal) => Some("hud-skill-sc_lifesteal_lifesteal_title"),
        Skill::Sceptre(SceptreSkill::LRegen) => Some("hud-skill-sc_lifesteal_regen_title"),
        Skill::Sceptre(SceptreSkill::HHeal) => Some("hud-skill-sc_heal_heal_title"),
        Skill::Sceptre(SceptreSkill::HRange) => Some("hud-skill-sc_heal_range_title"),
        Skill::Sceptre(SceptreSkill::HDuration) => Some("hud-skill-sc_heal_duration_title"),
        Skill::Sceptre(SceptreSkill::HCost) => Some("hud-skill-sc_heal_cost_title"),
        Skill::Sceptre(SceptreSkill::UnlockAura) => Some("hud-skill-sc_wardaura_unlock_title"),
        Skill::Sceptre(SceptreSkill::AStrength) => Some("hud-skill-sc_wardaura_strength_title"),
        Skill::Sceptre(SceptreSkill::ADuration) => Some("hud-skill-sc_wardaura_duration_title"),
        Skill::Sceptre(SceptreSkill::ARange) => Some("hud-skill-sc_wardaura_range_title"),
        Skill::Sceptre(SceptreSkill::ACost) => Some("hud-skill-sc_wardaura_cost_title"),
        Skill::Climb(ClimbSkill::Cost) => Some("hud-skill-climbing_cost_title"),
        Skill::Climb(ClimbSkill::Speed) => Some("hud-skill-climbing_speed_title"),
        Skill::Swim(SwimSkill::Speed) => Some("hud-skill-swim_speed_title"),
        Skill::Pick(MiningSkill::Speed) => Some("hud-skill-pick_strike_speed_title"),
        Skill::Pick(MiningSkill::OreGain) => Some("hud-skill-pick_strike_oregain_title"),
        Skill::Pick(MiningSkill::GemGain) => Some("hud-skill-pick_strike_gemgain_title"),
        Skill::Warrior(WarriorSkill::HardenedBody) => {
            Some("hud-skill-class-warrior-hardened_body_title")
        },
        Skill::Warrior(WarriorSkill::PracticedStrikes) => {
            Some("hud-skill-class-warrior-practiced_strikes_title")
        },
        Skill::Warrior(WarriorSkill::Rally) => Some("hud-skill-class-warrior-rally_title"),
        Skill::Warrior(WarriorSkill::IronSkin) => Some("hud-skill-class-warrior-iron_skin_title"),
        Skill::Warrior(WarriorSkill::BrutalEdge) => {
            Some("hud-skill-class-warrior-brutal_edge_title")
        },
        Skill::Warrior(WarriorSkill::CrushingBlows) => {
            Some("hud-skill-class-warrior-crushing_blows_title")
        },
        Skill::Warrior(WarriorSkill::Stalwart) => Some("hud-skill-class-warrior-stalwart_title"),
        Skill::Warrior(WarriorSkill::SunderingForce) => {
            Some("hud-skill-class-warrior-sundering_force_title")
        },
        Skill::Warrior(WarriorSkill::Stagger) => Some("hud-skill-class-warrior-stagger_title"),
        Skill::Warrior(WarriorSkill::BattleMomentum) => {
            Some("hud-skill-class-warrior-battle_momentum_title")
        },
        Skill::Warrior(WarriorSkill::BulwarkStance) => {
            Some("hud-skill-class-warrior-bulwark_stance_title")
        },
        Skill::Warrior(WarriorSkill::Onslaught) => Some("hud-skill-class-warrior-onslaught_title"),
        Skill::Mage(MageSkill::FocusedMind) => Some("hud-skill-class-mage-focused_mind_title"),
        Skill::Mage(MageSkill::TrueAim) => Some("hud-skill-class-mage-true_aim_title"),
        Skill::Mage(MageSkill::ArcaneSurge) => Some("hud-skill-class-mage-arcane_surge_title"),
        Skill::Mage(MageSkill::SpellPotency) => Some("hud-skill-class-mage-spell_potency_title"),
        Skill::Mage(MageSkill::PyromanticAttunement) => {
            Some("hud-skill-class-mage-pyromantic_attunement_title")
        },
        Skill::Mage(MageSkill::CryomanticAttunement) => {
            Some("hud-skill-class-mage-cryomantic_attunement_title")
        },
        Skill::Mage(MageSkill::QuickCasting) => Some("hud-skill-class-mage-quick_casting_title"),
        Skill::Mage(MageSkill::PenetratingMagic) => {
            Some("hud-skill-class-mage-penetrating_magic_title")
        },
        Skill::Mage(MageSkill::WardedSkin) => Some("hud-skill-class-mage-warded_skin_title"),
        Skill::Mage(MageSkill::ManaEfficiency) => {
            Some("hud-skill-class-mage-mana_efficiency_title")
        },
        Skill::Mage(MageSkill::Overcharge) => Some("hud-skill-class-mage-overcharge_title"),
        Skill::Mage(MageSkill::ArcaneMastery) => Some("hud-skill-class-mage-arcane_mastery_title"),
        Skill::Cleric(ClericSkill::FaithfulVigor) => {
            Some("hud-skill-class-cleric-faithful_vigor_title")
        },
        Skill::Cleric(ClericSkill::DevoutFocus) => {
            Some("hud-skill-class-cleric-devout_focus_title")
        },
        Skill::Cleric(ClericSkill::MendingLight) => {
            Some("hud-skill-class-cleric-mending_light_title")
        },
        Skill::Cleric(ClericSkill::BlessedAim) => Some("hud-skill-class-cleric-blessed_aim_title"),
        Skill::Cleric(ClericSkill::SacredWards) => {
            Some("hud-skill-class-cleric-sacred_wards_title")
        },
        Skill::Cleric(ClericSkill::SteadfastFaith) => {
            Some("hud-skill-class-cleric-steadfast_faith_title")
        },
        Skill::Cleric(ClericSkill::PurifyingGrace) => {
            Some("hud-skill-class-cleric-purifying_grace_title")
        },
        Skill::Cleric(ClericSkill::DivineConduit) => {
            Some("hud-skill-class-cleric-divine_conduit_title")
        },
        Skill::Cleric(ClericSkill::SmitingStrikes) => {
            Some("hud-skill-class-cleric-smiting_strikes_title")
        },
        Skill::Cleric(ClericSkill::ArmorOfFaith) => {
            Some("hud-skill-class-cleric-armor_of_faith_title")
        },
        Skill::Cleric(ClericSkill::Aegis) => Some("hud-skill-class-cleric-aegis_title"),
        Skill::Cleric(ClericSkill::RadiantChannel) => {
            Some("hud-skill-class-cleric-radiant_channel_title")
        },
        Skill::Rogue(RogueSkill::Lithe) => Some("hud-skill-class-rogue-lithe_title"),
        Skill::Rogue(RogueSkill::KeenEdge) => Some("hud-skill-class-rogue-keen_edge_title"),
        Skill::Rogue(RogueSkill::Ambush) => Some("hud-skill-class-rogue-ambush_title"),
        Skill::Rogue(RogueSkill::DeadlyPrecision) => {
            Some("hud-skill-class-rogue-deadly_precision_title")
        },
        Skill::Rogue(RogueSkill::FleetFooted) => Some("hud-skill-class-rogue-fleet_footed_title"),
        Skill::Rogue(RogueSkill::SureStrike) => Some("hud-skill-class-rogue-sure_strike_title"),
        Skill::Rogue(RogueSkill::FindTheGap) => Some("hud-skill-class-rogue-find_the_gap_title"),
        Skill::Rogue(RogueSkill::QuickHands) => Some("hud-skill-class-rogue-quick_hands_title"),
        Skill::Rogue(RogueSkill::ToxinTolerance) => {
            Some("hud-skill-class-rogue-toxin_tolerance_title")
        },
        Skill::Rogue(RogueSkill::Opportunist) => Some("hud-skill-class-rogue-opportunist_title"),
        Skill::Rogue(RogueSkill::Shadowstep) => Some("hud-skill-class-rogue-shadowstep_title"),
        Skill::Rogue(RogueSkill::Vanish) => Some("hud-skill-class-rogue-vanish_title"),
        Skill::Feat(FeatSkill::Athlete) => Some("hud-feat-athlete_title"),
        Skill::Feat(FeatSkill::Charger) => Some("hud-feat-charger_title"),
        Skill::Feat(FeatSkill::Crusher) => Some("hud-feat-crusher_title"),
        Skill::Feat(FeatSkill::CrossbowExpert) => Some("hud-feat-crossbow_expert_title"),
        Skill::Feat(FeatSkill::DefensiveDuelist) => Some("hud-feat-defensive_duelist_title"),
        Skill::Feat(FeatSkill::DualWielder) => Some("hud-feat-dual_wielder_title"),
        Skill::Feat(FeatSkill::GreatWeaponMaster) => Some("hud-feat-great_weapon_master_title"),
        Skill::Feat(FeatSkill::HeavyArmorMaster) => Some("hud-feat-heavy_armor_master_title"),
        Skill::Feat(FeatSkill::MageSlayer) => Some("hud-feat-mage_slayer_title"),
        Skill::Feat(FeatSkill::Mobile) => Some("hud-feat-mobile_title"),
        Skill::Feat(FeatSkill::Piercer) => Some("hud-feat-piercer_title"),
        Skill::Feat(FeatSkill::PolearmMaster) => Some("hud-feat-polearm_master_title"),
        Skill::Feat(FeatSkill::SavageAttacker) => Some("hud-feat-savage_attacker_title"),
        Skill::Feat(FeatSkill::Sentinel) => Some("hud-feat-sentinel_title"),
        Skill::Feat(FeatSkill::Sharpshooter) => Some("hud-feat-sharpshooter_title"),
        Skill::Feat(FeatSkill::ShieldMaster) => Some("hud-feat-shield_master_title"),
        Skill::Feat(FeatSkill::Slasher) => Some("hud-feat-slasher_title"),
        Skill::Feat(FeatSkill::Speedy) => Some("hud-feat-speedy_title"),
        Skill::Feat(FeatSkill::TavernBrawler) => Some("hud-feat-tavern_brawler_title"),
        Skill::Feat(FeatSkill::AberrantBloodmark) => Some("hud-feat-aberrant_bloodmark_title"),
        Skill::Feat(FeatSkill::ArcaneCollegeInitiate) => {
            Some("hud-feat-arcane_college_initiate_title")
        },
        Skill::Feat(FeatSkill::ArtificerInitiate) => Some("hud-feat-artificer_initiate_title"),
        Skill::Feat(FeatSkill::ElementalAdept) => Some("hud-feat-elemental_adept_title"),
        Skill::Feat(FeatSkill::FrostCaster) => Some("hud-feat-frost_caster_title"),
        Skill::Feat(FeatSkill::GenieMagic) => Some("hud-feat-genie_magic_title"),
        Skill::Feat(FeatSkill::GiftOfTheChromaticDragon) => {
            Some("hud-feat-gift_of_the_chromatic_dragon_title")
        },
        Skill::Feat(FeatSkill::GiftOfTheGemDragon) => Some("hud-feat-gift_of_the_gem_dragon_title"),
        Skill::Feat(FeatSkill::GiftOfTheMetallicDragon) => {
            Some("hud-feat-gift_of_the_metallic_dragon_title")
        },
        Skill::Feat(FeatSkill::GreaterAberrantBloodmark) => {
            Some("hud-feat-greater_aberrant_bloodmark_title")
        },
        Skill::Feat(FeatSkill::MagicInitiate) => Some("hud-feat-magic_initiate_title"),
        Skill::Feat(FeatSkill::MythalTouched) => Some("hud-feat-mythal_touched_title"),
        Skill::Feat(FeatSkill::SpellSniper) => Some("hud-feat-spell_sniper_title"),
        Skill::Feat(FeatSkill::SpellfireAdept) => Some("hud-feat-spellfire_adept_title"),
        Skill::Feat(FeatSkill::SpellfireSpark) => Some("hud-feat-spellfire_spark_title"),
        Skill::Feat(FeatSkill::Telekinetic) => Some("hud-feat-telekinetic_title"),
        Skill::Feat(FeatSkill::Telepathic) => Some("hud-feat-telepathic_title"),
        Skill::Feat(FeatSkill::UmbraTouched) => Some("hud-feat-umbra_touched_title"),
        Skill::Feat(FeatSkill::VeilTouched) => Some("hud-feat-veil_touched_title"),
        Skill::Feat(FeatSkill::WarCaster) => Some("hud-feat-war_caster_title"),
        Skill::Feat(FeatSkill::FairyTrickster) => Some("hud-feat-fairy_trickster_title"),
        Skill::Feat(FeatSkill::InspiringLeader) => Some("hud-feat-inspiring_leader_title"),
        Skill::Feat(FeatSkill::LordlyResolve) => Some("hud-feat-lordly_resolve_title"),
        Skill::Feat(FeatSkill::TirelessReveler) => Some("hud-feat-tireless_reveler_title"),
        Skill::Feat(FeatSkill::Alert) => Some("hud-feat-alert_title"),
        Skill::Feat(FeatSkill::Chef) => Some("hud-feat-chef_title"),
        Skill::Feat(FeatSkill::ChildOfTheSun) => Some("hud-feat-child_of_the_sun_title"),
        Skill::Feat(FeatSkill::DungeonDelver) => Some("hud-feat-dungeon_delver_title"),
        Skill::Feat(FeatSkill::Healer) => Some("hud-feat-healer_title"),
        Skill::Feat(FeatSkill::Observant) => Some("hud-feat-observant_title"),
        Skill::Feat(FeatSkill::ShadowmoorHexer) => Some("hud-feat-shadowmoor_hexer_title"),
        Skill::Feat(FeatSkill::Bombardier) => Some("hud-feat-bombardier_title"),
        Skill::Feat(FeatSkill::DraconicCultInitiate) => {
            Some("hud-feat-draconic_cult_initiate_title")
        },
        Skill::Feat(FeatSkill::Dragonscarred) => Some("hud-feat-dragonscarred_title"),
        Skill::Feat(FeatSkill::OrdersResilience) => Some("hud-feat-orders_resilience_title"),
        Skill::Feat(FeatSkill::Poisoner) => Some("hud-feat-poisoner_title"),
        Skill::Feat(FeatSkill::Quicksmith) => Some("hud-feat-quicksmith_title"),
        Skill::Feat(FeatSkill::StrikeOfTheGiants) => Some("hud-feat-strike_of_the_giants_title"),
        Skill::Feat(FeatSkill::VampireHunter) => Some("hud-feat-vampire_hunter_title"),
        Skill::Feat(FeatSkill::Bloodlust) => Some("hud-feat-bloodlust_title"),
        Skill::Feat(FeatSkill::CloyingMists) => Some("hud-feat-cloying_mists_title"),
        Skill::Feat(FeatSkill::DeliciousPain) => Some("hud-feat-delicious_pain_title"),
        Skill::Feat(FeatSkill::Durable) => Some("hud-feat-durable_title"),
        Skill::Feat(FeatSkill::LightBringer) => Some("hud-feat-light_bringer_title"),
        Skill::Feat(FeatSkill::LoveBites) => Some("hud-feat-love_bites_title"),
        Skill::Feat(FeatSkill::Lucky) => Some("hud-feat-lucky_title"),
        Skill::Feat(FeatSkill::Putrefy) => Some("hud-feat-putrefy_title"),
        Skill::Feat(FeatSkill::Rebuke) => Some("hud-feat-rebuke_title"),
        Skill::Feat(FeatSkill::Resilient) => Some("hud-feat-resilient_title"),
        Skill::Feat(FeatSkill::Tough) => Some("hud-feat-tough_title"),
        Skill::Feat(FeatSkill::TreacherousAllure) => Some("hud-feat-treacherous_allure_title"),
        Skill::Feat(FeatSkill::VampireTouched) => Some("hud-feat-vampire_touched_title"),
        Skill::Feat(FeatSkill::VampiresPlaything) => Some("hud-feat-vampires_plaything_title"),
        Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Sword)) => {
            Some("hud-skill-unlck_sword_title")
        },
        Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Axe)) => {
            Some("hud-skill-unlck_axe_title")
        },
        Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Hammer)) => {
            Some("hud-skill-unlck_hammer_title")
        },
        Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Bow)) => {
            Some("hud-skill-unlck_bow_title")
        },
        Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Staff)) => {
            Some("hud-skill-unlck_staff_title")
        },
        Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Sceptre)) => {
            Some("hud-skill-unlck_sceptre_title")
        },
        // Non-weapon unlock groups are never rendered as tree nodes (mirrors
        // legacy `unlock_skill_strings` warn->Empty path).
        Skill::UnlockGroup(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use xindeler_ui::i18n::{DEFAULT_HUD_FTL_FILES, Localization, fallback_locale};

    use super::*;

    /// A representative spread across all NON-feat sources resolves to real
    /// text (mirrors `buff_i18n`'s representative test): the two Sword prefix
    /// families, the `fireshockwave` quirk, one node from each other weapon,
    /// Sceptre/Climb/Swim/Mining, a class node, and a weapon-unlock node.
    #[test]
    fn representative_non_feat_skills_resolve() {
        let l10n = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);
        for skill in [
            Skill::Sword(SwordSkill::FellStrike), // pseudo_abilities prefix
            Skill::Sword(SwordSkill::HeavySweep), // common.abilities prefix
            Skill::Staff(StaffSkill::FireShockwave), // fireshockwave quirk
            Skill::Axe(AxeSkill::BrutalSwing),
            Skill::Hammer(HammerSkill::ScornfulSwipe),
            Skill::Bow(BowSkill::ArdentHunt),
            Skill::Sceptre(SceptreSkill::LDamage),
            Skill::Climb(ClimbSkill::Speed),
            Skill::Swim(SwimSkill::Speed),
            Skill::Pick(MiningSkill::Speed),
            Skill::Warrior(WarriorSkill::Rally),
            Skill::Mage(MageSkill::ArcaneSurge),
            Skill::Cleric(ClericSkill::MendingLight),
            Skill::Rogue(RogueSkill::Ambush),
            Skill::UnlockGroup(SkillGroupKind::Weapon(ToolKind::Sword)),
        ] {
            let key = skill_i18n_key(skill).expect("real leaf must have a key");
            assert_ne!(l10n.tr(key), key, "{skill:?} -> {key} must resolve");
        }
    }

    /// EXHAUSTIVE over the 72 feats: every feat's title AND its `.desc`
    /// attribute must resolve to real prose in `hud/skills.ftl` (this is the
    /// content Task 2 authored — the most error-prone half, so it is checked
    /// exhaustively rather than by sample).
    #[test]
    fn every_feat_title_and_desc_resolve() {
        let l10n = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);
        for feat in ALL_FEATS {
            let key = skill_i18n_key(Skill::Feat(feat)).expect("feat has a key");
            assert_ne!(l10n.tr(key), key, "{feat:?} title -> {key} must resolve");
            let desc = l10n.tr_attr(key, "desc");
            assert_ne!(
                desc,
                format!("{key}.desc"),
                "{feat:?} -> {key}.desc must resolve to real prose"
            );
        }
    }

    /// Every `FeatSkill` variant (kept in the same order as the enum). If a
    /// new feat is added upstream, add it here AND author its `.ftl` entry.
    const ALL_FEATS: [FeatSkill; 72] = [
        FeatSkill::Athlete,
        FeatSkill::Charger,
        FeatSkill::Crusher,
        FeatSkill::CrossbowExpert,
        FeatSkill::DefensiveDuelist,
        FeatSkill::DualWielder,
        FeatSkill::GreatWeaponMaster,
        FeatSkill::HeavyArmorMaster,
        FeatSkill::MageSlayer,
        FeatSkill::Mobile,
        FeatSkill::Piercer,
        FeatSkill::PolearmMaster,
        FeatSkill::SavageAttacker,
        FeatSkill::Sentinel,
        FeatSkill::Sharpshooter,
        FeatSkill::ShieldMaster,
        FeatSkill::Slasher,
        FeatSkill::Speedy,
        FeatSkill::TavernBrawler,
        FeatSkill::AberrantBloodmark,
        FeatSkill::ArcaneCollegeInitiate,
        FeatSkill::ArtificerInitiate,
        FeatSkill::ElementalAdept,
        FeatSkill::FrostCaster,
        FeatSkill::GenieMagic,
        FeatSkill::GiftOfTheChromaticDragon,
        FeatSkill::GiftOfTheGemDragon,
        FeatSkill::GiftOfTheMetallicDragon,
        FeatSkill::GreaterAberrantBloodmark,
        FeatSkill::MagicInitiate,
        FeatSkill::MythalTouched,
        FeatSkill::SpellSniper,
        FeatSkill::SpellfireAdept,
        FeatSkill::SpellfireSpark,
        FeatSkill::Telekinetic,
        FeatSkill::Telepathic,
        FeatSkill::UmbraTouched,
        FeatSkill::VeilTouched,
        FeatSkill::WarCaster,
        FeatSkill::FairyTrickster,
        FeatSkill::InspiringLeader,
        FeatSkill::LordlyResolve,
        FeatSkill::TirelessReveler,
        FeatSkill::Alert,
        FeatSkill::Chef,
        FeatSkill::ChildOfTheSun,
        FeatSkill::DungeonDelver,
        FeatSkill::Healer,
        FeatSkill::Observant,
        FeatSkill::ShadowmoorHexer,
        FeatSkill::Bombardier,
        FeatSkill::DraconicCultInitiate,
        FeatSkill::Dragonscarred,
        FeatSkill::OrdersResilience,
        FeatSkill::Poisoner,
        FeatSkill::Quicksmith,
        FeatSkill::StrikeOfTheGiants,
        FeatSkill::VampireHunter,
        FeatSkill::Bloodlust,
        FeatSkill::CloyingMists,
        FeatSkill::DeliciousPain,
        FeatSkill::Durable,
        FeatSkill::LightBringer,
        FeatSkill::LoveBites,
        FeatSkill::Lucky,
        FeatSkill::Putrefy,
        FeatSkill::Rebuke,
        FeatSkill::Resilient,
        FeatSkill::Tough,
        FeatSkill::TreacherousAllure,
        FeatSkill::VampireTouched,
        FeatSkill::VampiresPlaything,
    ];
}
