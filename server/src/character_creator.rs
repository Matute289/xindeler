use crate::persistence::{PersistedComponents, character_updater::CharacterUpdater};
use common::{
    character::CharacterId,
    comp::{
        BASE_ABILITY_LIMIT, Body, CharacterClass, Content, Inventory, Item, SkillSet, Stats,
        Waypoint, class::ClassKind, inventory::loadout_builder::LoadoutBuilder,
        skillset::SkillGroupKind,
    },
};
use specs::{Entity, WriteExpect};

/// Per-class starter weapon whitelist (spec §3/§5). `[None, None]` is always
/// accepted separately for unmodified clients.
fn valid_starter_items(class: ClassKind) -> &'static [[Option<&'static str>; 2]] {
    match class {
        ClassKind::Adventurer => &[],
        ClassKind::Warrior => &[
            [Some("common.items.weapons.sword.starter"), None],
            [Some("common.items.weapons.axe.starter_axe"), None],
            [Some("common.items.weapons.hammer.starter_hammer"), None],
        ],
        ClassKind::Mage => &[[Some("common.items.weapons.staff.starter_staff"), None]],
        ClassKind::Cleric => &[[Some("common.items.weapons.sceptre.starter_sceptre"), None]],
        ClassKind::Rogue => &[
            [
                Some("common.items.weapons.sword_1h.starter"),
                Some("common.items.weapons.sword_1h.starter"),
            ],
            [Some("common.items.weapons.bow.starter"), None],
        ],
        // Classes-wave (BL-04): valid existing starters by archetype; thematic
        // implements (tome/instrument/quarterstaff) come with BL-06.
        ClassKind::Barbarian => &[[Some("common.items.weapons.axe.starter_axe"), None], [
            Some("common.items.weapons.hammer.starter_hammer"),
            None,
        ]],
        ClassKind::Sorcerer
        | ClassKind::Warlock
        | ClassKind::Bard
        | ClassKind::Druid
        | ClassKind::Artificer => &[[Some("common.items.weapons.staff.starter_staff"), None]],
        ClassKind::Paladin | ClassKind::BloodSlayer => {
            &[[Some("common.items.weapons.sword.starter"), None]]
        },
        ClassKind::Ranger => &[[Some("common.items.weapons.bow.starter"), None]],
        ClassKind::Monk => &[[Some("common.items.weapons.sword_1h.starter"), None]],
    }
}

/// One flavorful consumable per class (all verified under
/// assets/common/items/consumable/).
fn class_kit_item(class: ClassKind) -> &'static str {
    match class {
        ClassKind::Adventurer | ClassKind::Warrior => "common.items.consumable.potion_minor",
        ClassKind::Mage | ClassKind::Rogue => "common.items.consumable.potion_agility",
        ClassKind::Cleric => "common.items.consumable.potion_med",
        // Classes-wave (BL-04).
        ClassKind::Sorcerer
        | ClassKind::Warlock
        | ClassKind::Bard
        | ClassKind::Druid
        | ClassKind::Artificer => "common.items.consumable.potion_minor",
        ClassKind::Ranger | ClassKind::Monk => "common.items.consumable.potion_agility",
        ClassKind::Barbarian | ClassKind::Paladin | ClassKind::BloodSlayer => {
            "common.items.consumable.potion_med"
        },
    }
}

// Upstream names the variants InvalidWeapon/InvalidBody; keeping the prefix
// for the added InvalidClass minimizes the upstream-merge surface (renaming
// would touch every call site). Three same-prefix variants trip the lint.
#[expect(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum CreationError {
    InvalidWeapon,
    InvalidBody,
    InvalidClass,
}

pub fn create_character(
    entity: Entity,
    player_uuid: String,
    character_alias: String,
    character_mainhand: Option<String>,
    character_offhand: Option<String>,
    body: Body,
    character_class: ClassKind,
    ethos: common::comp::Ethos,
    hardcore: bool,
    character_updater: &mut WriteExpect<'_, CharacterUpdater>,
    waypoint: Option<Waypoint>,
) -> Result<(), CreationError> {
    // quick fix whitelist validation for now; eventually replace the
    // `Option<String>` with an index into a server-provided list of starter
    // items, and replace `comp::body::Body` with `comp::body::humanoid::Body`
    // throughout the messages involved
    if !matches!(body, Body::Humanoid(_)) {
        return Err(CreationError::InvalidBody);
    }
    if !character_class.is_playable() {
        return Err(CreationError::InvalidClass);
    }
    // [None, None] (no weapons) bypasses the class whitelist on purpose — stock
    // clients may create without weapons (zesterer); guard structure preserves it.
    if !(character_mainhand.is_none() && character_offhand.is_none())
        && !valid_starter_items(character_class)
            .contains(&[character_mainhand.as_deref(), character_offhand.as_deref()])
    {
        return Err(CreationError::InvalidWeapon);
    };
    // The client sends None if a weapon hand is empty
    let mut rng = rand::rng();
    let loadout = LoadoutBuilder::empty()
        .defaults()
        .with_asset_expect(
            &format!("common.loadout.class.{}", character_class.keyword()),
            &mut rng,
            None,
        )
        .active_mainhand(character_mainhand.map(|x| Item::new_from_asset_expect(&x)))
        .active_offhand(character_offhand.map(|x| Item::new_from_asset_expect(&x)))
        .build();
    let mut inventory = Inventory::with_loadout_humanoid(loadout);

    let stats = Stats::new(Content::Plain(character_alias.to_string()), body);
    let mut skill_set = SkillSet::default();
    skill_set.unlock_skill_group(SkillGroupKind::Class(character_class));
    // Default items for new characters
    inventory
        .push(Item::new_from_asset_expect(
            "common.items.consumable.potion_minor",
        ))
        .expect("Inventory has at least 2 slots left!");
    inventory
        .push(Item::new_from_asset_expect("common.items.food.cheese"))
        .expect("Inventory has at least 1 slot left!");
    inventory
        .push_recipe_group(Item::new_from_asset_expect("common.items.recipes.default"))
        .expect("New inventory should not already have default recipe group.");
    inventory
        .push(Item::new_from_asset_expect(class_kit_item(character_class)))
        .expect("Inventory has at least 1 slot left!");

    let map_marker = None;

    character_updater.create_character(entity, player_uuid, character_alias, PersistedComponents {
        body,
        hardcore: hardcore.then_some(common::comp::Hardcore),
        character_class: CharacterClass(character_class),
        stats,
        skill_set,
        inventory,
        waypoint,
        pets: Vec::new(),
        active_abilities: common::comp::ActiveAbilities::default_limited(BASE_ABILITY_LIMIT),
        map_marker,
        // BL-33: the alignment chosen at character creation (defaults to True
        // Neutral if the client sends it). Sanitised — never trust the wire
        // value. Deeds then drift it in-game (P3).
        ethos: ethos.clamped(),
    });
    Ok(())
}

pub fn edit_character(
    entity: Entity,
    player_uuid: String,
    id: CharacterId,
    character_alias: String,
    body: Body,
    character_updater: &mut WriteExpect<'_, CharacterUpdater>,
) -> Result<(), CreationError> {
    if !matches!(body, Body::Humanoid(_)) {
        return Err(CreationError::InvalidBody);
    }

    character_updater.edit_character(
        entity,
        player_uuid,
        id,
        Some(character_alias),
        (body,),
        None,
    );
    Ok(())
}

// Error handling
impl core::fmt::Display for CreationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CreationError::InvalidWeapon => write!(
                f,
                "Invalid weapon.\nServer and client might be partially incompatible."
            ),
            CreationError::InvalidBody => write!(
                f,
                "Invalid Body.\nServer and client might be partially incompatible"
            ),
            CreationError::InvalidClass => write!(
                f,
                "Invalid class.\nServer and client might be partially incompatible."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::comp::class::ClassKind;

    #[test]
    fn every_class_has_starter_weapons_and_they_load() {
        for class in ClassKind::PLAYABLE {
            let kits = valid_starter_items(class);
            assert!(!kits.is_empty(), "{class:?} has no starter weapons");
            for pair in kits {
                for item in pair.iter().flatten() {
                    Item::new_from_asset_expect(item);
                }
            }
        }
    }

    #[test]
    fn class_loadouts_and_kit_items_load() {
        let mut rng = rand::rng();
        for class in ClassKind::PLAYABLE {
            let _ = LoadoutBuilder::empty().defaults().with_asset_expect(
                &format!("common.loadout.class.{}", class.keyword()),
                &mut rng,
                None,
            );
            Item::new_from_asset_expect(class_kit_item(class));
        }
    }
}
