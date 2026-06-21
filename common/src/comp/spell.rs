//! Spell compendium metadata (magic-system-v2 spec §3). Pure UI/gating layer:
//! every `SpellDef` points at a `CharacterAbility` RON that actually executes.
//! Combat reads the ability; spellbook UI, class gating, and tooltips read
//! this.
use crate::{
    assets::{Asset, AssetCache, AssetExt, BoxedError, Ron, SharedString},
    comp::{
        ability::{MagicSource, School},
        class::ClassKind,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CastTime {
    Action,
    Bonus,
    Reaction,
    Minutes(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SpellDuration {
    Instant,
    Secs(f32),
    Concentration(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SpellRange {
    SelfOnly,
    Touch,
    Meters(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SpellAoe {
    Sphere(f32),
    Cone(f32),
    Line(f32),
    Cube(f32),
}

/// One catalogued spell. Metadata only; `ability` is the asset specifier of the
/// `CharacterAbility` RON that runs when cast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpellDef {
    pub id: String,
    pub name_i18n: String,
    /// 0 = cantrip … 9.
    pub level: u8,
    pub school: Option<School>,
    pub source: MagicSource,
    pub classes: Vec<ClassKind>,
    pub cast_time: CastTime,
    pub duration: SpellDuration,
    pub range: SpellRange,
    pub aoe: Option<SpellAoe>,
    pub description_i18n: String,
    /// Asset path of the executing `CharacterAbility` RON.
    pub ability: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SpellCompendium(pub Vec<SpellDef>);

impl Asset for SpellCompendium {
    fn load(cache: &AssetCache, specifier: &SharedString) -> Result<Self, BoxedError> {
        let inner = cache
            .load::<Ron<Vec<SpellDef>>>(specifier)?
            .read()
            .0
            .clone();
        Ok(SpellCompendium(inner))
    }
}

impl SpellCompendium {
    pub fn load_expect_cloned() -> Self { Self::load_expect("common.spells.compendium").cloned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assets::AssetExt, comp::ability::CharacterAbility};

    #[test]
    fn compendium_loads_and_abilities_resolve() {
        let book = SpellCompendium::load_expect_cloned();
        assert!(!book.0.is_empty(), "compendium is empty");
        for spell in &book.0 {
            Ron::<CharacterAbility>::load_expect(&spell.ability).read();
        }
    }
}
