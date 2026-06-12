//! Character class identity. See
//! docs/superpowers/specs/2026-06-10-classes-races-design.md.
use serde::{Deserialize, Serialize};
use specs::{Component, DerefFlaggedStorage, VecStorage};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
