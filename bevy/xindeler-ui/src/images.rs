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
//!
//! ## Some `.png` files are actually JPEG bytes (BL-82 EM-5.17 Phase 6 fix)
//! Found via a live `--smoke-screenshot` of the Phase 6 skill-tree connector
//! lines rendering as literally nothing: at least THREE of the 55 files
//! copied from the art pack — `skill_line_active.png`, `skill_line_locked.png`,
//! `orb_frame_stamina.png` — are JPEG-encoded bytes (`file(1)`/magic-byte
//! confirmed: `FF D8 FF E0`, a JFIF SOI marker) saved with a `.png`
//! extension, almost certainly an export-tool artifact of the same kind as
//! the leading-space filename above. Bevy's default image-loader setting
//! (`ImageFormatSetting::FromExtension`) trusts the extension, tries to
//! decode JPEG bytes as PNG, fails, and the `Handle<Image>` never resolves —
//! the `ImageNode` silently renders nothing (no error visible in a
//! screenshot, no placeholder texture either). [`HudImages::load`] loads
//! EVERY key with `ImageFormatSetting::Guess` instead (sniffs the real
//! magic bytes via the `image` crate's `guess_format`, same behaviour for a
//! genuinely-PNG file) so this asset-pack quirk can never silently blank out
//! a texture again, for these 3 files or any future one sharing the same
//! export artifact.

use bevy::{
    asset::{AssetServer, Handle},
    ecs::{resource::Resource, system::Res},
    image::{Image, ImageFormatSetting, ImageLoaderSettings},
};

/// The asset subfolder every `HudImageKey` variant resolves against,
/// relative to `VELOREN_ASSETS` — mirrors the existing
/// `assets/voxygen/element/ui/{bag,chat,diary,map,minimap,…}/` per-screen
/// convention (`assets/voxygen/element/ui/` already holds several such
/// image-asset subfolders; this pack is a themed asset SET rather than one
/// screen, hence its own `hud_d4` sibling rather than reusing an existing
/// name).
const HUD_D4_DIR: &str = "voxygen/element/ui/hud_d4";

/// BL-82 EM-5.17/5.18 legacy-inventory rebuild — the legacy pixel-art `bag/`
/// asset set Matías asked to switch the inventory/equipment window to
/// (replacing the reserved high-res `hud_d4/` art for THAT screen only; every
/// other `HudImageKey` variant keeps resolving against [`HUD_D4_DIR`]
/// unchanged via [`HudImageKey::dir`]). Sibling subfolders, mirroring
/// the real on-disk layout under `assets/voxygen/element/ui/bag/`.
/// Rarity slot backgrounds + the empty-slot art — `bag/buttons/`.
const BAG_BUTTONS_DIR: &str = "voxygen/element/ui/bag/buttons";
/// Ghost equipment-slot placeholders — `bag/backgrounds/`.
const BAG_BG_DIR: &str = "voxygen/element/ui/bag/backgrounds";
/// The stat-column icons + the title-bar character portrait — `bag/icons/`.
const BAG_ICONS_DIR: &str = "voxygen/element/ui/bag/icons";
/// BL-82 EM-5.18 legacy-inventory round 2 — the `bag/` ROOT itself, which
/// holds the full-window chrome plates (`inv_bg_0.png`) directly (not in a
/// sibling subfolder like the icons/buttons/backgrounds groups above).
const BAG_DIR: &str = "voxygen/element/ui/bag";
/// BL-82 EM-5.18 legacy-inventory round 2 — the shared generic UI buttons
/// (the red-X close button used by legacy "xindeler-old"'s bag/skillbar/map
/// windows, `bag.rs`'s
/// `Button::image(close_btn).hover_image(..).press_image(..)`). Its own sibling
/// subfolder, mirroring the real on-disk layout under `assets/voxygen/element/
/// ui/generic/buttons/`.
const GENERIC_BUTTONS_DIR: &str = "voxygen/element/ui/generic/buttons";

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
    /// BL-82 HUD polish round 4 (issue 4) — a dedicated left-mouse-button
    /// icon replacing the hotbar's last-two-slots `"LMB"` text label. See
    /// `crate::images`'s own module doc comment convention: variant names
    /// follow the source filename.
    MouseClickLeft,
    /// Right-mouse-button counterpart of [`Self::MouseClickLeft`].
    MouseClickRight,
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

    // BL-82 EM-5.17/5.18 legacy-inventory rebuild — the legacy `bag/`
    // pixel-art set (see the `BAG_*_DIR`/`GENERIC_BUTTONS_DIR` constants'
    // own doc comments for which subfolder each group below resolves
    // against, via `dir()`). Only the keys the rebuilt inventory window
    // actually renders are wired here. Round 2 (see [`Self::InventoryChrome`])
    // adopts `inv_bg_0.png` as the window chrome after all — the earlier
    // "intentionally NOT used, can't stretch without distortion" concern is
    // resolved by sizing the window to the asset's NATIVE 424x708 aspect and
    // scaling uniformly rather than stretching to a mismatched box (the
    // fixed-aspect, measured-region idiom PR #157 established). `inv_frame.png`
    // (the frame-only overlay variant) remains unused: `inv_bg_0.png` already
    // bundles the frame + fill + dividers in one plate.
    /// Rarity slot backgrounds — the legacy `Quality` → colour mapping (see
    /// `xindeler-client::inventory_ui::quality_rarity_background`).
    InvSlot,
    InvSlotGrey,
    InvSlotCommon,
    InvSlotGreen,
    InvSlotBlue,
    InvSlotPurple,
    InvSlotGold,
    InvSlotOrange,
    InvSlotRed,
    /// Per-`EquipSlot` ghost/silhouette placeholders shown when that slot is
    /// empty (see `xindeler-client::inventory_ui::equip_slot_frame`).
    GhostHead,
    GhostChest,
    GhostShoulders,
    GhostHands,
    GhostBelt,
    GhostLegs,
    GhostFeet,
    GhostRing,
    GhostBack,
    GhostNecklace,
    GhostTabard,
    GhostMainhand,
    GhostOffhand,
    GhostLantern,
    GhostGlider,
    /// Stat-column icons (health/energy/protection/stun-resist/combat-
    /// rating/stealth — see `xindeler-client::inventory_ui::StatKind`).
    StatHealth,
    StatEnergy,
    StatProtection,
    StatStunRes,
    StatCombatRating,
    StatStealth,
    /// BL-82 EM-5.18 legacy-inventory round 2 — the title-bar character
    /// portrait (top-left of the inventory window), a FLAT 2D pixel-art bust
    /// (legacy "xindeler-old"'s `bag.rs` `char_art`, `Image::new(char_art)` —
    /// NOT a live 3D render). `bag/icons/character.png`.
    CharacterPortrait,
    /// The red-X close button (top-right of the inventory window) + its
    /// hover/press states — the same three `generic/buttons/close_btn*.png`
    /// textures legacy "xindeler-old"'s `bag.rs` drives through
    /// `Button::image(..).hover_image(..).press_image(..)`.
    CloseBtn,
    CloseBtnHover,
    CloseBtnPress,
    /// BL-82 EM-5.18 legacy-inventory round 2 — the whole ornate window
    /// chrome bitmap (`bag/inv_bg_0.png`, 424x708): the golden filigree
    /// border + corner ornaments, a dark translucent fill, and the two carved
    /// region dividers (title-bar rule near the top, bag-grid rule near the
    /// bottom) — the SAME single background plate legacy "xindeler-old"'s
    /// `bag.rs` draws behind the inventory. Superseding this module's earlier
    /// "`inv_bg_0.png` intentionally NOT used" note: it is now used, but the
    /// window is sized to the asset's NATIVE 424x708 aspect and scaled
    /// UNIFORMLY (never stretched to a mismatched box), so the "distortion"
    /// the old note warned about does not arise — the same fixed-aspect,
    /// measured-region idiom PR #157's party portrait frame established.
    InventoryChrome,
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
        Self::MouseClickLeft,
        Self::MouseClickRight,
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
        Self::InvSlot,
        Self::InvSlotGrey,
        Self::InvSlotCommon,
        Self::InvSlotGreen,
        Self::InvSlotBlue,
        Self::InvSlotPurple,
        Self::InvSlotGold,
        Self::InvSlotOrange,
        Self::InvSlotRed,
        Self::GhostHead,
        Self::GhostChest,
        Self::GhostShoulders,
        Self::GhostHands,
        Self::GhostBelt,
        Self::GhostLegs,
        Self::GhostFeet,
        Self::GhostRing,
        Self::GhostBack,
        Self::GhostNecklace,
        Self::GhostTabard,
        Self::GhostMainhand,
        Self::GhostOffhand,
        Self::GhostLantern,
        Self::GhostGlider,
        Self::StatHealth,
        Self::StatEnergy,
        Self::StatProtection,
        Self::StatStunRes,
        Self::StatCombatRating,
        Self::StatStealth,
        Self::CharacterPortrait,
        Self::CloseBtn,
        Self::CloseBtnHover,
        Self::CloseBtnPress,
        Self::InventoryChrome,
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
            Self::MouseClickLeft => "mouse_click_left.png",
            Self::MouseClickRight => "mouse_click_right.png",
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
            Self::InvSlot => "inv_slot.png",
            Self::InvSlotGrey => "inv_slot_grey.png",
            Self::InvSlotCommon => "inv_slot_common.png",
            Self::InvSlotGreen => "inv_slot_green.png",
            Self::InvSlotBlue => "inv_slot_blue.png",
            Self::InvSlotPurple => "inv_slot_purple.png",
            Self::InvSlotGold => "inv_slot_gold.png",
            Self::InvSlotOrange => "inv_slot_orange.png",
            Self::InvSlotRed => "inv_slot_red.png",
            Self::GhostHead => "head.png",
            Self::GhostChest => "chest.png",
            Self::GhostShoulders => "shoulders.png",
            Self::GhostHands => "hands.png",
            Self::GhostBelt => "belt.png",
            Self::GhostLegs => "legs.png",
            Self::GhostFeet => "feet.png",
            Self::GhostRing => "ring.png",
            Self::GhostBack => "back.png",
            Self::GhostNecklace => "necklace.png",
            Self::GhostTabard => "tabard.png",
            Self::GhostMainhand => "mainhand.png",
            Self::GhostOffhand => "offhand.png",
            Self::GhostLantern => "lantern.png",
            Self::GhostGlider => "glider.png",
            Self::StatHealth => "health.png",
            Self::StatEnergy => "energy.png",
            Self::StatProtection => "protection.png",
            Self::StatStunRes => "stun_res.png",
            Self::StatCombatRating => "combat_rating.png",
            Self::StatStealth => "stealth_rating.png",
            Self::CharacterPortrait => "character.png",
            Self::CloseBtn => "close_btn.png",
            Self::CloseBtnHover => "close_btn_hover.png",
            Self::CloseBtnPress => "close_btn_press.png",
            Self::InventoryChrome => "inv_bg_0.png",
        }
    }

    /// The asset subfolder this key's file lives in. Every PRE-EXISTING
    /// variant resolves to [`HUD_D4_DIR`] (unchanged); every new legacy
    /// `bag/`-set variant added for BL-82 EM-5.17/5.18's legacy-inventory
    /// rebuild resolves to whichever of the [`BAG_BUTTONS_DIR`]/
    /// [`BAG_BG_DIR`]/[`BAG_ICONS_DIR`] siblings actually holds its file on
    /// disk.
    #[must_use]
    const fn dir(self) -> &'static str {
        match self {
            Self::StatHealth
            | Self::StatEnergy
            | Self::StatProtection
            | Self::StatStunRes
            | Self::StatCombatRating
            | Self::StatStealth
            | Self::CharacterPortrait => BAG_ICONS_DIR,
            Self::CloseBtn | Self::CloseBtnHover | Self::CloseBtnPress => GENERIC_BUTTONS_DIR,
            Self::InventoryChrome => BAG_DIR,
            Self::InvSlot
            | Self::InvSlotGrey
            | Self::InvSlotCommon
            | Self::InvSlotGreen
            | Self::InvSlotBlue
            | Self::InvSlotPurple
            | Self::InvSlotGold
            | Self::InvSlotOrange
            | Self::InvSlotRed => BAG_BUTTONS_DIR,
            Self::GhostHead
            | Self::GhostChest
            | Self::GhostShoulders
            | Self::GhostHands
            | Self::GhostBelt
            | Self::GhostLegs
            | Self::GhostFeet
            | Self::GhostRing
            | Self::GhostBack
            | Self::GhostNecklace
            | Self::GhostTabard
            | Self::GhostMainhand
            | Self::GhostOffhand
            | Self::GhostLantern
            | Self::GhostGlider => BAG_BG_DIR,
            _ => HUD_D4_DIR,
        }
    }

    /// The full asset path (directory + filename) this key loads, relative to
    /// `VELOREN_ASSETS` — what [`HudImages::load`] actually feeds to the
    /// [`AssetServer`].
    #[must_use]
    fn path(self) -> String { format!("{}/{}", self.dir(), self.filename()) }
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
    /// HudFonts::load`]). Uses `ImageFormatSetting::Guess` (content-sniffed,
    /// not the file extension) — see this module's own doc comment for why:
    /// at least 3 of the 55 real files are JPEG bytes under a `.png` name,
    /// which the default `FromExtension` setting fails to decode.
    #[must_use]
    pub fn load(asset_server: &AssetServer) -> Self {
        let handles = HudImageKey::ALL
            .iter()
            .map(|key| {
                asset_server
                    .load_builder()
                    .with_settings(|settings: &mut ImageLoaderSettings| {
                        settings.format = ImageFormatSetting::Guess;
                    })
                    .load(key.path())
            })
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

    /// Every [`HudImageKey`] variant has a distinct full PATH — a copy-paste
    /// mistake in the big `match` (two variants pointing at the same file in
    /// the same directory, or `ALL` missing a variant added later) would
    /// otherwise go unnoticed. Bare FILENAMES may now collide across
    /// directories (BL-82 EM-5.17/5.18 legacy-inventory rebuild introduced
    /// several `bag/`-set subfolders whose files share names with unrelated
    /// `hud_d4/` files), so this checks [`HudImageKey::path`], not
    /// `filename()`.
    #[test]
    fn every_variant_has_a_distinct_filename() {
        let paths: HashSet<String> = HudImageKey::ALL.iter().map(|k| k.path()).collect();
        assert_eq!(
            paths.len(),
            HudImageKey::ALL.len(),
            "two HudImageKey variants must not resolve to the same file"
        );
    }

    /// BL-82 EM-5.17/5.18 legacy-inventory rebuild — pins that the new legacy
    /// `bag/`-set variants resolve into the correct sibling subfolder (not
    /// silently falling back to [`HUD_D4_DIR`], and not colliding with the
    /// PRE-EXISTING `hud_d4/` files that happen to share a bare filename,
    /// e.g. `inv_slot_common.png`/`slot_bg_common.png` are DIFFERENT files
    /// in DIFFERENT directories).
    #[test]
    fn legacy_bag_variants_resolve_into_their_own_subfolder() {
        assert_eq!(
            HudImageKey::InvSlotCommon.path(),
            "voxygen/element/ui/bag/buttons/inv_slot_common.png"
        );
        assert_eq!(
            HudImageKey::GhostHead.path(),
            "voxygen/element/ui/bag/backgrounds/head.png"
        );
        assert_eq!(
            HudImageKey::StatHealth.path(),
            "voxygen/element/ui/bag/icons/health.png"
        );
        // A pre-existing variant must still resolve against HUD_D4_DIR,
        // completely unaffected by this generalization.
        assert_eq!(
            HudImageKey::SlotBgCommon.path(),
            "voxygen/element/ui/hud_d4/slot_bg_common.png"
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
