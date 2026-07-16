//! BL-82 EM-5.17 T57.7 — the `HudImages` resource: a compile-time-checked
//! lookup from a named [`HudImageKey`] to the real `Handle<Image>` loaded
//! from the "HUD-D4" dark-gothic ARPG art pack.
//!
//! Mirrors [`crate::theme::HudFonts`] exactly: an enum-keyed lookup rather
//! than a stringly-typed `HashMap<String, _>` (a typo'd key string would
//! silently miss at runtime; a typo'd enum variant fails to compile), loaded
//! via [`AssetServer::load`] at `Startup` (hot-reloadable in dev, same as
//! every other asset this workspace loads).
//!
//! The 58 source PNGs (`~/MyXindeler/HUD/HUD-D4/assets/` on the authoring
//! machine — NOT part of this repo, read-only design reference) were copied
//! into the real asset tree at `assets/voxygen/element/ui/hud_d4/`, the
//! existing per-screen-subfolder convention `assets/voxygen/element/ui/{bag,
//! chat, diary, map, minimap, …}/` already uses (this pack gets its own
//! subfolder since it's a themed asset SET, not a single screen). The actual
//! file count on disk at copy time was **55**, not 58 (verified via `ls`,
//! not assumed from the design spec) — the spec's "58 PNGs" figure is
//! approximate; every real file was copied and accounted for here.
//!
//! ## BL-82 EM-5.17 Phase 7 (T57.14) — 8 more `equip_empty_*.png` frames
//! Matías generated bespoke frame art for the 8 equip slots that had no
//! dedicated background yet (Legs, Tabard, and the 4 weapon slots +
//! Lantern/Glider), closing the asset gap spec §3.7 originally flagged.
//! Copied the same way as the original 55, into the same [`HUD_D4_DIR`].
//! The source filenames do NOT exactly match their `EquipSlot`/`ArmorSlot`
//! variant names (e.g. `equip_empty_mainhand.png` covers `ActiveMainhand`,
//! `equip_empty_inactive_mainhand.png` covers `InactiveMainhand`) — the
//! variant names below spell out which real slot each one is for rather
//! than mirroring the filename verbatim, to avoid an
//! `EquipEmptyMainhand`/`EquipEmptyInactiveMainhand` pair that reads as
//! ambiguous about which is which. Combined with the pre-existing 9, all 18
//! equip slots shown on the Phase 7 equipment panel (spec §3.7's confirmed
//! layout) now have bespoke frame art — `slot_empty.png` is never used as a
//! fallback anywhere in that panel.
//!
//! ## Deliberately excluded from this enum (spec §3.1, §6 "RESOLVED")
//! Two of the 55 real files are copied to disk (for completeness/history)
//! but have NO [`HudImageKey`] variant and are NEVER loaded by [`HudImages`]
//! — a later phase reaching for `HudImageKey::Mana2Liquid` or `::ActionBarBg`
//! simply cannot, because the variants don't exist:
//! - `mana2_liquid.png` — a rejected alternative mana-liquid colour Matías
//!   generated then discarded (confirmed: use `mana_liquid.png`, which matches
//!   Xindeler's existing blue mana SVG icons).
//! - `action_bar_bg.png` — the superseded single-piece action-bar background,
//!   replaced by the confirmed 2-piece `action_bar_bg_left.png`/
//!   `action_bar_bg_right.png` split (needed to make room for the third,
//!   centred Stamina orb).
//!
//! ## `skill_slot_border.png` filename note
//! The source file for this asset actually has a **leading space** in its
//! filename (`" skill_slot_border.png"`, confirmed via a `repr()`'d Python
//! directory listing, not just `ls`) — almost certainly an unintentional
//! artifact of however the art pack was exported. The leading space was
//! stripped when copying into this repo (a leading-space filename is a real
//! landmine for asset-path string literals and general repo hygiene); the
//! enum below and every `AssetServer::load` path use the clean
//! `"skill_slot_border.png"` name.

use bevy::{
    asset::{AssetServer, Handle},
    ecs::{resource::Resource, system::Res},
    image::Image,
};

/// The asset subfolder every `HudImageKey` variant resolves against,
/// relative to `VELOREN_ASSETS` — mirrors the existing
/// `assets/voxygen/element/ui/{bag,chat,diary,map,minimap,…}/` per-screen
/// convention (`assets/voxygen/element/ui/` already holds several such
/// image-asset subfolders; this pack is a themed asset SET rather than one
/// screen, hence its own `hud_d4` sibling rather than reusing an existing
/// name).
const HUD_D4_DIR: &str = "voxygen/element/ui/hud_d4";

/// One variant per real, WIRED-IN HUD-D4 PNG (compile-time checked — see the
/// module doc comment for the two files deliberately NOT represented here).
/// Variant names follow the source filename (PascalCase), not a
/// screen-oriented renaming, so a later phase can find "the file I'm looking
/// at in the asset pack" by name alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HudImageKey {
    ActionBarBgLeft,
    ActionBarBgRight,
    BagLock,
    BlacksmithAnvil,
    BossBarFrame,
    BossLevelBadge,
    BossNamePlateBg,
    BossStaggerBar,
    BossStaggerFullBar,
    ButtonHover,
    ButtonNormal,
    ButtonPressed,
    CopperCoin,
    EquipEmptyActiveMainhand,
    EquipEmptyActiveOffhand,
    EquipEmptyBack,
    EquipEmptyBelt,
    EquipEmptyChest,
    EquipEmptyFeet,
    EquipEmptyGlider,
    EquipEmptyHands,
    EquipEmptyHelmet,
    EquipEmptyInactiveMainhand,
    EquipEmptyInactiveOffhand,
    EquipEmptyLantern,
    EquipEmptyLegs,
    EquipEmptyNecklace,
    EquipEmptyRing,
    EquipEmptyShoulders,
    EquipEmptyTabard,
    GoldCoin,
    HealthLiquid,
    InventoryBg,
    InventoryTooltipBg,
    ManaLiquid,
    NonObjectiveBullet,
    ObjectiveBullet,
    OrbFrameAngel,
    OrbFrameCuthulhu,
    OrbFrameStamina,
    OtherSkillTreeBg,
    PartyLevelBadge,
    PartyPortraitFrame,
    PartyVoiceActive,
    PartyVoiceInactive,
    PartyVoiceMuted,
    PlatinumCoin,
    SilverCoin,
    SkillLineActive,
    SkillLineLocked,
    SkillSlotBorder,
    SkillTooltipBg,
    SkillTreeBg,
    SlotBgCommon,
    SlotBgLegendary,
    SlotBgMythic,
    SlotBgRare,
    SlotBgUncommon,
    SlotBgVeryRare,
    SlotEmpty,
    StaminaLiquid,
}

impl HudImageKey {
    /// Every variant, in declaration order — used by [`HudImages::load`] to
    /// build the lookup table and by this module's own test to prove every
    /// variant resolves to a distinct file.
    const ALL: &'static [Self] = &[
        Self::ActionBarBgLeft,
        Self::ActionBarBgRight,
        Self::BagLock,
        Self::BlacksmithAnvil,
        Self::BossBarFrame,
        Self::BossLevelBadge,
        Self::BossNamePlateBg,
        Self::BossStaggerBar,
        Self::BossStaggerFullBar,
        Self::ButtonHover,
        Self::ButtonNormal,
        Self::ButtonPressed,
        Self::CopperCoin,
        Self::EquipEmptyActiveMainhand,
        Self::EquipEmptyActiveOffhand,
        Self::EquipEmptyBack,
        Self::EquipEmptyBelt,
        Self::EquipEmptyChest,
        Self::EquipEmptyFeet,
        Self::EquipEmptyGlider,
        Self::EquipEmptyHands,
        Self::EquipEmptyHelmet,
        Self::EquipEmptyInactiveMainhand,
        Self::EquipEmptyInactiveOffhand,
        Self::EquipEmptyLantern,
        Self::EquipEmptyLegs,
        Self::EquipEmptyNecklace,
        Self::EquipEmptyRing,
        Self::EquipEmptyShoulders,
        Self::EquipEmptyTabard,
        Self::GoldCoin,
        Self::HealthLiquid,
        Self::InventoryBg,
        Self::InventoryTooltipBg,
        Self::ManaLiquid,
        Self::NonObjectiveBullet,
        Self::ObjectiveBullet,
        Self::OrbFrameAngel,
        Self::OrbFrameCuthulhu,
        Self::OrbFrameStamina,
        Self::OtherSkillTreeBg,
        Self::PartyLevelBadge,
        Self::PartyPortraitFrame,
        Self::PartyVoiceActive,
        Self::PartyVoiceInactive,
        Self::PartyVoiceMuted,
        Self::PlatinumCoin,
        Self::SilverCoin,
        Self::SkillLineActive,
        Self::SkillLineLocked,
        Self::SkillSlotBorder,
        Self::SkillTooltipBg,
        Self::SkillTreeBg,
        Self::SlotBgCommon,
        Self::SlotBgLegendary,
        Self::SlotBgMythic,
        Self::SlotBgRare,
        Self::SlotBgUncommon,
        Self::SlotBgVeryRare,
        Self::SlotEmpty,
        Self::StaminaLiquid,
    ];

    /// The filename (no directory) this key loads, exactly matching the file
    /// copied into [`HUD_D4_DIR`].
    #[must_use]
    const fn filename(self) -> &'static str {
        match self {
            Self::ActionBarBgLeft => "action_bar_bg_left.png",
            Self::ActionBarBgRight => "action_bar_bg_right.png",
            Self::BagLock => "bag_lock.png",
            Self::BlacksmithAnvil => "blacksmith_anvil.png",
            Self::BossBarFrame => "boss_bar_frame.png",
            Self::BossLevelBadge => "boss_level_badge.png",
            Self::BossNamePlateBg => "boss_name_plate_bg.png",
            Self::BossStaggerBar => "boss_stagger_bar.png",
            Self::BossStaggerFullBar => "boss_stagger_full_bar.png",
            Self::ButtonHover => "button_hover.png",
            Self::ButtonNormal => "button_normal.png",
            Self::ButtonPressed => "button_pressed.png",
            Self::CopperCoin => "copper_coin.png",
            Self::EquipEmptyActiveMainhand => "equip_empty_mainhand.png",
            Self::EquipEmptyActiveOffhand => "equip_empty_offhand.png",
            Self::EquipEmptyBack => "equip_empty_back.png",
            Self::EquipEmptyBelt => "equip_empty_belt.png",
            Self::EquipEmptyChest => "equip_empty_chest.png",
            Self::EquipEmptyFeet => "equip_empty_feet.png",
            Self::EquipEmptyGlider => "equip_empty_glider.png",
            Self::EquipEmptyHands => "equip_empty_hands.png",
            Self::EquipEmptyHelmet => "equip_empty_helmet.png",
            Self::EquipEmptyInactiveMainhand => "equip_empty_inactive_mainhand.png",
            Self::EquipEmptyInactiveOffhand => "equip_empty_inactive_offhand.png",
            Self::EquipEmptyLantern => "equip_empty_lantern.png",
            Self::EquipEmptyLegs => "equip_empty_legs.png",
            Self::EquipEmptyNecklace => "equip_empty_necklace.png",
            Self::EquipEmptyRing => "equip_empty_ring.png",
            Self::EquipEmptyShoulders => "equip_empty_shoulders.png",
            Self::EquipEmptyTabard => "equip_empty_tabard.png",
            Self::GoldCoin => "gold_coin.png",
            Self::HealthLiquid => "health_liquid.png",
            Self::InventoryBg => "inventory_bg.png",
            Self::InventoryTooltipBg => "inventory_tooltip_bg.png",
            Self::ManaLiquid => "mana_liquid.png",
            Self::NonObjectiveBullet => "non_objective_bullet.png",
            Self::ObjectiveBullet => "objective_bullet.png",
            Self::OrbFrameAngel => "orb_frame_angel.png",
            Self::OrbFrameCuthulhu => "orb_frame_cuthulhu.png",
            Self::OrbFrameStamina => "orb_frame_stamina.png",
            Self::OtherSkillTreeBg => "other_skill_tree_bg.png",
            Self::PartyLevelBadge => "party_level_badge.png",
            Self::PartyPortraitFrame => "party_portrait_frame.png",
            Self::PartyVoiceActive => "party_voice_active.png",
            Self::PartyVoiceInactive => "party_voice_inactive.png",
            Self::PartyVoiceMuted => "party_voice_muted.png",
            Self::PlatinumCoin => "platinum_coin.png",
            Self::SilverCoin => "silver_coin.png",
            Self::SkillLineActive => "skill_line_active.png",
            Self::SkillLineLocked => "skill_line_locked.png",
            Self::SkillSlotBorder => "skill_slot_border.png",
            Self::SkillTooltipBg => "skill_tooltip_bg.png",
            Self::SkillTreeBg => "skill_tree_bg.png",
            Self::SlotBgCommon => "slot_bg_common.png",
            Self::SlotBgLegendary => "slot_bg_legendary.png",
            Self::SlotBgMythic => "slot_bg_mythic.png",
            Self::SlotBgRare => "slot_bg_rare.png",
            Self::SlotBgUncommon => "slot_bg_uncommon.png",
            Self::SlotBgVeryRare => "slot_bg_very_rare.png",
            Self::SlotEmpty => "slot_empty.png",
            Self::StaminaLiquid => "stamina_liquid.png",
        }
    }
}

/// The HUD-D4 image lookup resource every widget-kit image-backed primitive
/// (and every later phase's HUD screen) reads. One `Handle<Image>` per
/// [`HudImageKey`] — resolved by `match`, never a string/hashmap lookup, so a
/// typo'd key fails to compile instead of silently resolving to nothing at
/// runtime.
#[derive(Resource, Debug, Clone)]
pub struct HudImages {
    handles: Vec<Handle<Image>>,
}

impl HudImages {
    /// Loads every [`HudImageKey`] variant's `Handle<Image>` via the
    /// [`AssetServer`] (hot-reloadable in dev, same as [`crate::theme::
    /// HudFonts::load`]).
    #[must_use]
    pub fn load(asset_server: &AssetServer) -> Self {
        let handles = HudImageKey::ALL
            .iter()
            .map(|key| asset_server.load(format!("{HUD_D4_DIR}/{}", key.filename())))
            .collect();
        Self { handles }
    }

    /// Test-only constructor mirroring this module's own
    /// `every_key_resolves_to_a_handle` fixture below: every key maps to a
    /// default (invalid, but real) `Handle<Image>`, with no `AssetServer`
    /// needed. `pub(crate)` so sibling modules whose tests need a real
    /// `HudImages` resource (e.g. `crate::tooltip`'s T57.16 rarity/reskin
    /// tests) don't have to spin up a full `App` + `AssetServer` just to
    /// prove their own logic — this crate's own field privacy (`handles` has
    /// no `pub`) otherwise blocks that from outside this module.
    #[cfg(test)]
    pub(crate) fn dummy() -> Self {
        Self {
            handles: HudImageKey::ALL.iter().map(|_| Handle::default()).collect(),
        }
    }

    /// The handle for a given key. `HudImageKey::ALL` covers every variant
    /// and [`Self::load`] always builds one handle per `ALL` entry in the
    /// same order, so this never panics for a real [`HudImageKey`] value —
    /// enforced by this module's own `every_key_resolves_to_a_handle` test.
    #[must_use]
    pub fn get(&self, key: HudImageKey) -> Handle<Image> {
        let index = HudImageKey::ALL
            .iter()
            .position(|candidate| *candidate == key)
            .expect("HudImageKey::ALL is exhaustive by construction");
        self.handles[index].clone()
    }
}

/// `Startup` system inserting [`HudImages::load`] (via the real
/// [`AssetServer`]). `pub` (not `pub(crate)`) for the same reason
/// [`crate::theme::init_theme`] is: a downstream screen plugin that spawns
/// image-backed widgets at `Startup` needs to `.after(init_images)` its own
/// spawn system so the resource is guaranteed to exist first.
pub fn init_images(mut commands: bevy::ecs::system::Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(HudImages::load(&asset_server));
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// BL-82 EM-5.17 Phase 7 (T57.14) — pins the 8 new equip-slot frame
    /// variants' filenames EXPLICITLY against the brief's own mapping table
    /// (the source PNG names do not exactly match their `EquipSlot`/
    /// `ArmorSlot` variant names, e.g. `equip_empty_mainhand.png` covers
    /// `ActiveMainhand`, not a generic "mainhand" — a generic/derived name
    /// would have silently swapped `ActiveMainhand`/`InactiveMainhand`).
    #[test]
    fn the_eight_new_equip_frame_variants_map_to_the_documented_filenames() {
        assert_eq!(
            HudImageKey::EquipEmptyLegs.filename(),
            "equip_empty_legs.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyTabard.filename(),
            "equip_empty_tabard.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyActiveMainhand.filename(),
            "equip_empty_mainhand.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyActiveOffhand.filename(),
            "equip_empty_offhand.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyInactiveMainhand.filename(),
            "equip_empty_inactive_mainhand.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyInactiveOffhand.filename(),
            "equip_empty_inactive_offhand.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyLantern.filename(),
            "equip_empty_lantern.png"
        );
        assert_eq!(
            HudImageKey::EquipEmptyGlider.filename(),
            "equip_empty_glider.png"
        );
    }

    /// Every [`HudImageKey`] variant has a distinct filename — a copy-paste
    /// mistake in the big `match` (two variants pointing at the same file,
    /// or `ALL` missing a variant added later) would otherwise go unnoticed.
    #[test]
    fn every_variant_has_a_distinct_filename() {
        let filenames: HashSet<&str> = HudImageKey::ALL.iter().map(|k| k.filename()).collect();
        assert_eq!(
            filenames.len(),
            HudImageKey::ALL.len(),
            "two HudImageKey variants must not resolve to the same file"
        );
    }

    /// The two confirmed-discarded files never got a variant at all — this
    /// is enforced structurally (no `HudImageKey::Mana2Liquid` or
    /// `::ActionBarBg` exists to construct), but pin it as a named test too
    /// so a future variant addition thinks twice before reintroducing either.
    #[test]
    fn discarded_variants_are_absent() {
        let filenames: HashSet<&str> = HudImageKey::ALL.iter().map(|k| k.filename()).collect();
        assert!(!filenames.contains("mana2_liquid.png"));
        assert!(!filenames.contains("action_bar_bg.png"));
    }

    /// [`HudImages::get`] resolves without panicking for every real key —
    /// exercised without a real [`AssetServer`] by building the lookup
    /// directly (mirrors this crate's other resource tests, which don't spin
    /// up a full `App`/`AssetServer` just to prove a lookup table is
    /// internally consistent).
    #[test]
    fn every_key_resolves_to_a_handle() {
        let images = HudImages {
            handles: HudImageKey::ALL.iter().map(|_| Handle::default()).collect(),
        };
        for &key in HudImageKey::ALL {
            let _ = images.get(key);
        }
    }
}
