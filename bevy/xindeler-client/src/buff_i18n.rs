//! BL-82 EM-5.16 close-out — the `BuffKind -> "buff-<key>"` i18n lookup the
//! buff strip (`combat_hud`) and the diary Stats tab (`diary`) resolve
//! display names through. Ported from legacy `voxygen::hud::util::buff_key`.
//! An exhaustive `match` (the compiler guarantees every current `BuffKind`
//! variant is mapped), so a display name is never a raw `Debug` identifier.
//!
//! Reconciled against the CURRENT `BuffKind` enum (post BL-82 EM-6.1 upstream
//! merge, 2026-07-23): legacy `xindeler-old` still names the pre-merge
//! `ArdentHunter`/`ArdentHunted`/`SepticShot`/`OwlTalon`/`HeavyNock`/
//! `Heartseeker` variants, all since renamed/removed upstream (replaced by
//! `ArdentHunt`/`IgniteArrow`/`FreezeArrow`/`DrenchArrow`/`JoltArrow`, and by
//! `StormChaser`/`EagleEye`) — those five new keys map to the exact
//! `buff-*` entries `buff.ftl` already carries from that same merge.
//! `BleedingMark` (BL-05 bleed-detonate) is Xindeler-only, no legacy source.

use common::comp::buff::BuffKind;

/// The frozen `assets/voxygen/i18n/en/buff.ftl` key for `kind`'s title/desc.
/// Exhaustive — adding a `BuffKind` variant upstream is a compile error here
/// until it is mapped (and given a `buff-*` entry in `buff.ftl`).
#[must_use]
pub fn buff_i18n_key(kind: BuffKind) -> &'static str {
    match kind {
        BuffKind::Regeneration => "buff-heal",
        BuffKind::Saturation => "buff-saturation",
        BuffKind::Potion => "buff-potion",
        BuffKind::Agility => "buff-agility",
        BuffKind::RestingHeal => "buff-resting_heal",
        BuffKind::EnergyRegen => "buff-energy_regen",
        BuffKind::ComboGeneration => "buff-combo_generation",
        BuffKind::IncreaseMaxHealth => "buff-increase_max_health",
        BuffKind::IncreaseMaxEnergy => "buff-increase_max_energy",
        BuffKind::Shielded => "buff-shielded",
        BuffKind::Invulnerability => "buff-invulnerability",
        BuffKind::ProtectingWard => "buff-protectingward",
        BuffKind::Frenzied => "buff-frenzied",
        BuffKind::Hastened => "buff-hastened",
        BuffKind::FreedomOfMovement => "buff-freedom_of_movement",
        BuffKind::Fortitude => "buff-fortitude",
        BuffKind::Reckless => "buff-reckless",
        BuffKind::Flame => "buff-burn",
        BuffKind::Frigid => "buff-frigid",
        BuffKind::Lifesteal => "buff-lifesteal",
        BuffKind::Bloodfeast => "buff-bloodfeast",
        BuffKind::ImminentCritical => "buff-imminentcritical",
        BuffKind::Fury => "buff-fury",
        BuffKind::Sunderer => "buff-sunderer",
        BuffKind::Defiance => "buff-defiance",
        BuffKind::Berserk => "buff-berserk",
        BuffKind::ScornfulTaunt => "buff-scornfultaunt",
        BuffKind::Tenacity => "buff-tenacity",
        BuffKind::Resilience => "buff-resilience",
        BuffKind::StormChaser => "buff-stormchaser",
        BuffKind::EagleEye => "buff-eagleeye",
        BuffKind::ArdentHunt => "buff-ardenthunt",
        BuffKind::IgniteArrow => "buff-ignitearrow",
        BuffKind::FreezeArrow => "buff-freezearrow",
        BuffKind::DrenchArrow => "buff-drencharrow",
        BuffKind::JoltArrow => "buff-joltarrow",
        BuffKind::Burning => "buff-burn",
        BuffKind::Bleeding => "buff-bleed",
        BuffKind::BleedingMark => "buff-bleeding_mark",
        BuffKind::Cursed => "buff-cursed",
        BuffKind::Crippled => "buff-crippled",
        BuffKind::Frozen => "buff-frozen",
        BuffKind::Wet => "buff-wet",
        BuffKind::Ensnared => "buff-ensnared",
        BuffKind::Poisoned => "buff-poisoned",
        BuffKind::Parried => "buff-parried",
        BuffKind::PotionSickness => "buff-potionsickness",
        BuffKind::Heatstroke => "buff-heatstroke",
        BuffKind::Rooted => "buff-rooted",
        BuffKind::Winded => "buff-winded",
        // Display names predate/diverge from the Rust variant names: Amnesia
        // shows as "Concussion" and OffBalance as "Staggered" (both existing,
        // thematically-matching `buff.ftl` entries — reused rather than
        // duplicated).
        BuffKind::Amnesia => "buff-concussion",
        BuffKind::OffBalance => "buff-staggered",
        BuffKind::Chilled => "buff-chilled",
        BuffKind::Terrified => "buff-terrified",
        BuffKind::Charmed => "buff-charmed",
        BuffKind::Hollowtouched => "buff-hollowtouched",
        BuffKind::DifficultTerrain => "buff-difficult_terrain",
        BuffKind::Antimagic => "buff-antimagic",
        BuffKind::Anchored => "buff-anchored",
        BuffKind::Asleep => "buff-asleep",
        BuffKind::Blinded => "buff-blinded",
        BuffKind::Slowed => "buff-slowed",
        BuffKind::Polymorphed => "buff-polymorphed",
    }
}

#[cfg(test)]
mod tests {
    use xindeler_ui::i18n::{DEFAULT_HUD_FTL_FILES, Localization, fallback_locale};

    use super::*;

    /// Every mapped key must resolve to REAL prose in the shipped `buff.ftl`
    /// (i.e. `tr(key) != key`), for a representative spread of current
    /// variants across buffs and debuffs — including the post-merge Ardent
    /// Hunt rework names, since those are the ones most likely to have been
    /// missed by a naive port of the legacy table.
    #[test]
    fn representative_buffkinds_resolve_to_real_text() {
        let l10n = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);
        for kind in [
            BuffKind::Regeneration,
            BuffKind::Potion,
            BuffKind::Hastened,
            BuffKind::Bleeding,
            BuffKind::BleedingMark,
            BuffKind::Frozen,
            BuffKind::Burning,
            BuffKind::Wet,
            BuffKind::Crippled,
            BuffKind::ArdentHunt,
            BuffKind::IgniteArrow,
            BuffKind::FreezeArrow,
            BuffKind::DrenchArrow,
            BuffKind::JoltArrow,
        ] {
            let key = buff_i18n_key(kind);
            assert_ne!(
                l10n.tr(key),
                key,
                "{kind:?} -> {key} must resolve to real text in buff.ftl"
            );
        }
    }
}
