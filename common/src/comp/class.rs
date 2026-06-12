//! Character class identity. See
//! docs/superpowers/specs/2026-06-10-classes-races-design.md.
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use specs::{Component, DerefFlaggedStorage, VecStorage};

use crate::{
    assets::{AssetExt, Ron},
    comp::{Stats, body::humanoid::Species},
};

#[derive(
    Clone, Copy, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd,
)]
pub enum ClassKind {
    /// Legacy/default class for pre-class characters. Not selectable at
    /// creation, has no class skill tree, and is never listed in item class
    /// whitelists (equipment-restrictions Spec B).
    #[default]
    Adventurer,
    Warrior,
    Mage,
    Cleric,
    Rogue,
}

impl ClassKind {
    /// Every variant, including Adventurer. Single source of truth for
    /// enumeration — persistence round-trips and tests iterate this, so a
    /// new variant added here cannot silently fall out of any converter.
    pub const ALL: [ClassKind; 5] = [
        ClassKind::Adventurer,
        ClassKind::Warrior,
        ClassKind::Mage,
        ClassKind::Cleric,
        ClassKind::Rogue,
    ];
    /// Classes selectable at character creation (excludes Adventurer).
    pub const PLAYABLE: [ClassKind; 4] = [
        ClassKind::Warrior,
        ClassKind::Mage,
        ClassKind::Cleric,
        ClassKind::Rogue,
    ];

    pub fn is_playable(self) -> bool { !matches!(self, ClassKind::Adventurer) }

    /// Lowercase keyword used by chat commands and asset specifiers.
    pub fn keyword(self) -> &'static str {
        match self {
            ClassKind::Adventurer => "adventurer",
            ClassKind::Warrior => "warrior",
            ClassKind::Mage => "mage",
            ClassKind::Cleric => "cleric",
            ClassKind::Rogue => "rogue",
        }
    }

    /// Inverse of [`Self::keyword`] for the playable classes only.
    pub fn from_keyword(keyword: &str) -> Option<Self> {
        Self::PLAYABLE
            .iter()
            .copied()
            .find(|c| c.keyword() == keyword)
    }
}

/// The class a player character chose at creation (or via /set_class).
/// Synced to all clients; persisted in the `character` table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterClass(pub ClassKind);

impl Component for CharacterClass {
    type Storage = DerefFlaggedStorage<Self, VecStorage<Self>>;
}

/// Per-species passive stat modifiers (spec §6). All values small (≤10%).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct RacialTraits {
    pub move_speed_mult: f32,
    pub attack_damage_mult: f32,
    pub energy_reward_mult: f32,
    pub max_energy_mult: f32,
    pub damage_reduction_add: f32,
    pub crowd_control_resistance_add: f32,
}

impl Default for RacialTraits {
    fn default() -> Self {
        Self {
            move_speed_mult: 1.0,
            attack_damage_mult: 1.0,
            energy_reward_mult: 1.0,
            max_energy_mult: 1.0,
            damage_reduction_add: 0.0,
            crowd_control_resistance_add: 0.0,
        }
    }
}

/// Single asset-cache lookup. Do NOT call per-entity in tick systems — hoist
/// one lookup per system run (hot-reload still works per-run); see buff.rs.
pub fn racial_traits(species: Species) -> RacialTraits {
    racial_traits_manifest()
        .0
        .get(&species)
        .copied()
        .unwrap_or_default()
}

/// One manifest read for per-tick consumers: one cache access covers all
/// species. Hot-reloads between system runs.
pub fn racial_traits_manifest() -> crate::assets::AssetReadGuard<Ron<HashMap<Species, RacialTraits>>>
{
    Ron::<HashMap<Species, RacialTraits>>::load_expect("common.class.racial_traits").read()
}

impl RacialTraits {
    /// Applies these passives onto freshly-reset stats. Must run right after
    /// `Stats::reset_temp_modifiers` so traits stack with buffs (spec §6).
    pub fn apply(self, stats: &mut Stats) {
        stats.move_speed_modifier *= self.move_speed_mult;
        stats.attack_damage_modifier *= self.attack_damage_mult;
        stats.energy_reward_modifier *= self.energy_reward_mult;
        stats.max_energy_modifiers.mult_mod *= self.max_energy_mult;
        stats.damage_reduction.pos_mod += self.damage_reduction_add;
        stats.crowd_control_resistance += self.crowd_control_resistance_add;
    }
}

/// Applies racial passives onto freshly-reset stats. Must run right after
/// `Stats::reset_temp_modifiers` so traits stack with buffs (spec §6).
pub fn apply_racial_traits(stats: &mut Stats, species: Species) {
    racial_traits(species).apply(stats);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playable_is_all_minus_adventurer() {
        assert_eq!(ClassKind::ALL.len(), ClassKind::PLAYABLE.len() + 1);
        for class in ClassKind::PLAYABLE {
            assert!(ClassKind::ALL.contains(&class));
        }
        assert!(ClassKind::ALL.contains(&ClassKind::Adventurer));
    }

    #[test]
    fn default_class_is_adventurer() {
        assert_eq!(ClassKind::default(), ClassKind::Adventurer);
        assert_eq!(CharacterClass::default().0, ClassKind::Adventurer);
    }

    #[test]
    fn keyword_round_trips_for_playable_classes() {
        for class in ClassKind::PLAYABLE {
            assert!(class.is_playable());
            assert_eq!(ClassKind::from_keyword(class.keyword()), Some(class));
        }
        // Adventurer is deliberately not re-pickable by keyword
        assert_eq!(ClassKind::from_keyword("adventurer"), None);
        assert_eq!(ClassKind::from_keyword("paladin"), None);
    }

    #[test]
    fn racial_traits_manifest_loads_with_expected_values() {
        use crate::comp::body::humanoid::Species;
        // Spec §6 v1 values
        assert!(racial_traits(Species::Human).energy_reward_mult > 1.0);
        assert!(racial_traits(Species::Dwarf).damage_reduction_add > 0.0);
        assert!(racial_traits(Species::Elf).move_speed_mult > 1.0);
        assert!(racial_traits(Species::Orc).attack_damage_mult > 1.0);
        assert!(racial_traits(Species::Danari).max_energy_mult > 1.0);
        assert!(racial_traits(Species::Draugr).crowd_control_resistance_add > 0.0);
    }

    #[test]
    fn racial_traits_apply_to_stats() {
        use crate::comp::{Stats, body::humanoid::Species};
        let body = crate::comp::Body::Humanoid(crate::comp::humanoid::Body::random());
        let mut stats = Stats::empty(body);
        let before = stats.attack_damage_modifier;
        apply_racial_traits(&mut stats, Species::Orc);
        assert!(stats.attack_damage_modifier > before);
    }
}
