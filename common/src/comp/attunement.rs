//! Attunement (ENG-D2 / I1): some magic items grant their effects only while
//! the wearer is *attuned* to them. A character may attune a limited number of
//! items at once (the cap grows with level); attuning takes time scaled by the
//! item's rarity, and un-attuning is instant.
//!
//! This module is the **data model + the (tunable) rules**. The
//! attune/un-attune actions and the effect-gating (an attuned-required item is
//! inert until attuned) are wired in follow-ups (ENG-D2b / D2c).

use super::inventory::{item::Quality, slot::EquipSlot};
use serde::{Deserialize, Serialize};
use specs::{Component, DerefFlaggedStorage, VecStorage};

/// The equipment slots a character currently has attuned. Session-only for now
/// (persistence is a follow-up). Its length is bounded by `max_attuned_items`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttunedItems(pub Vec<EquipSlot>);

impl AttunedItems {
    /// Whether the item in `slot` is currently attuned.
    pub fn is_attuned(&self, slot: EquipSlot) -> bool { self.0.contains(&slot) }

    /// How many items are currently attuned.
    pub fn count(&self) -> usize { self.0.len() }

    /// Attune the item in `slot` if it is not already attuned and the wearer is
    /// under the `max` cap. Returns whether the slot is attuned afterwards.
    pub fn attune(&mut self, slot: EquipSlot, max: u32) -> bool {
        if self.is_attuned(slot) {
            return true;
        }
        if self.0.len() as u32 >= max {
            return false;
        }
        self.0.push(slot);
        true
    }

    /// Un-attune the item in `slot` (instant). Returns whether it had been
    /// attuned.
    pub fn unattune(&mut self, slot: EquipSlot) -> bool {
        let before = self.0.len();
        self.0.retain(|s| *s != slot);
        self.0.len() != before
    }
}

impl Component for AttunedItems {
    type Storage = DerefFlaggedStorage<Self, VecStorage<Self>>;
}

/// How many items a character of `level` may keep attuned at once.
///
/// Matias §I1: **L1-3 → 1**, **L4-7 → 2**, then **+1 per further 4 levels**
/// (8-11 → 3, 12-15 → 4, …). Tunable; the exact bands await the final level
/// cap.
pub fn max_attuned_items(level: u16) -> u32 {
    if level <= 3 {
        1
    } else {
        2 + u32::from((level - 4) / 4)
    }
}

/// Seconds to attune an item of the given rarity (`Quality`). Un-attuning is
/// instant, so it has no equivalent.
///
/// Matias §I1 (tunable — "los tiempos los podemos ir charlando"): común ~3 ·
/// raro ~6 · muy raro ~12 · legendario ~21 · mítico ~35. The engine `Quality`
/// ladder maps onto those tiers; the in-between `Moderate` ("uncommon") is
/// interpolated, and `Low`/`Debug` cost nothing (never attunement items).
pub fn attune_time(quality: Quality) -> f32 {
    match quality {
        Quality::Low | Quality::Debug => 0.0,
        Quality::Common => 3.0,     // común
        Quality::Moderate => 4.5,   // "uncommon" — interpolated, not specified
        Quality::High => 6.0,       // raro
        Quality::Epic => 12.0,      // muy raro
        Quality::Legendary => 21.0, // legendario
        Quality::Artifact => 35.0,  // mítico
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comp::inventory::slot::ArmorSlot;

    #[test]
    fn cap_grows_with_level() {
        assert_eq!(max_attuned_items(1), 1);
        assert_eq!(max_attuned_items(3), 1);
        assert_eq!(max_attuned_items(4), 2);
        assert_eq!(max_attuned_items(7), 2);
        assert_eq!(max_attuned_items(8), 3);
        assert_eq!(max_attuned_items(11), 3);
        assert_eq!(max_attuned_items(12), 4);
    }

    #[test]
    fn attune_time_scales_with_rarity() {
        assert!(attune_time(Quality::Common) < attune_time(Quality::High));
        assert!(attune_time(Quality::High) < attune_time(Quality::Epic));
        assert!(attune_time(Quality::Epic) < attune_time(Quality::Legendary));
        assert!(attune_time(Quality::Legendary) < attune_time(Quality::Artifact));
        assert_eq!(attune_time(Quality::Common), 3.0);
        assert_eq!(attune_time(Quality::Artifact), 35.0);
        assert_eq!(attune_time(Quality::Low), 0.0);
    }

    #[test]
    fn attune_respects_cap() {
        let mut a = AttunedItems::default();
        let r1 = EquipSlot::Armor(ArmorSlot::Ring1);
        let r2 = EquipSlot::Armor(ArmorSlot::Ring2);
        let neck = EquipSlot::Armor(ArmorSlot::Neck);
        assert!(a.attune(r1, 2));
        assert!(a.attune(r2, 2));
        assert_eq!(a.count(), 2);
        assert!(!a.attune(neck, 2)); // over the cap
        assert!(!a.is_attuned(neck));
        assert!(a.is_attuned(r1));
    }

    #[test]
    fn attune_is_idempotent_and_unattune_is_instant() {
        let mut a = AttunedItems::default();
        let r1 = EquipSlot::Armor(ArmorSlot::Ring1);
        assert!(a.attune(r1, 1));
        assert!(a.attune(r1, 1)); // already attuned → still true, not double-counted
        assert_eq!(a.count(), 1);
        assert!(a.unattune(r1));
        assert!(!a.is_attuned(r1));
        assert!(!a.unattune(r1)); // not attuned → false
    }
}
