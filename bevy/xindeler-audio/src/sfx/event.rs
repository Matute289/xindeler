//! BL-82 EM-5.10b (T56.35) — the SFX event vocabulary the frozen
//! `assets/voxygen/audio/sfx.ron` manifest is keyed by.
//!
//! Ported 1:1 from `voxygen/src/audio/sfx/mod.rs`'s `SfxEvent`/`VoiceKind`/
//! `SfxInventoryEvent`/`body_to_voice` (voxygen is no longer a workspace
//! member — CLAUDE.md — so this is a genuine re-implementation, not an
//! `use voxygen::...` reference) — EVERY variant is kept, even the ones no
//! system in this crate/`xindeler-client` constructs yet, because the frozen
//! manifest's RON keys use them and [`super::manifest::SfxManifestLoader`]
//! must successfully parse the WHOLE file (isolation law rule 3: the
//! manifest is frozen, never restructured to drop unused keys).
use common::{
    comp::{
        Body, CharacterAbilityType, biped_large, biped_small, bird_large, bird_medium,
        controller::UtteranceKind,
        humanoid,
        inventory::item::{
            item_key::ItemKey,
            tool::{AbilitySpec, ToolKind},
        },
        poise::PoiseState,
        quadruped_low, quadruped_medium, quadruped_small,
    },
    terrain::BlockKind,
};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize, Hash, Eq)]
pub enum SfxEvent {
    Campfire,
    Embers,
    Birdcall,
    Owl,
    Cricket1,
    Cricket2,
    Cricket3,
    Frog,
    Bees,
    RunningWaterSlow,
    Lavapool,
    Idle,
    Swim,
    SplashSmall,
    SplashMedium,
    SplashBig,
    Run(BlockKind),
    QuadRun(BlockKind),
    OctoRun(BlockKind),
    Roll,
    RollCancel,
    Sneak,
    Climb,
    GliderOpen,
    Glide,
    GliderClose,
    CatchAir,
    Jump,
    Fall,
    Attack(CharacterAbilityType, ToolKind),
    Wield(ToolKind),
    Unwield(ToolKind),
    Inventory(SfxInventoryEvent),
    Explosion,
    Damage,
    Death,
    Parry,
    Block,
    BreakBlock,
    PickaxeDamage,
    PickaxeDamageStrong,
    PickaxeBreakBlock,
    SceptreBeam,
    SkillPointGain,
    CharacterLevelUp,
    ArrowHit,
    ArrowMiss,
    ArrowShot,
    FireShot,
    NapalmShot,
    NapalmImpact,
    FireBreathShot,
    FireBreathCharge,
    PyroclasmCharge,
    PyroclasmBolt,
    FlameThrower,
    PoiseChange(PoiseState),
    GroundSlam,
    FlashFreeze,
    GigaRoar,
    IceSpikes,
    IceCrack,
    Utterance(UtteranceKind, VoiceKind),
    Lightning,
    CyclopsCharge,
    TerracottaStatueCharge,
    LaserBeam,
    Steam,
    FuseCharge,
    Music(ToolKind, AbilitySpec),
    Yeet,
    Hiss,
    LongHiss,
    Klonk,
    SmashKlonk,
    FireShockwave,
    DeepLaugh,
    Whoosh,
    Swoosh,
    GroundDig,
    PortalActivated,
    TeleportedByPortal,
    FromTheAshes,
    SurpriseEgg,
    Transformation,
    Bleep,
    Charge,
    StrigoiHead,
    BloodmoonHeiressSummon,
    TrainChugg,
    TrainChuggSteam,
    TrainAmbience,
    TrainClack,
    TrainSpeed,
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Hash, Eq)]
pub enum VoiceKind {
    HumanFemale,
    HumanMale,
    BipedLarge,
    Wendigo,
    Reptile,
    Bird,
    Critter,
    Sheep,
    Pig,
    Cow,
    Canine,
    Dagon,
    Lion,
    Mindflayer,
    Marlin,
    Maneater,
    Adlet,
    Antelope,
    Alligator,
    SeaCrocodile,
    Saurok,
    Cat,
    Goat,
    Mandragora,
    Asp,
    Fungome,
    Truffler,
    Wolf,
    Wyvern,
    Phoenix,
    VampireBat,
    Legoom,
}

/// Ported verbatim from `voxygen::audio::sfx::body_to_voice` — maps a body's
/// species onto the [`VoiceKind`] its utterance/hurt/death vocalizations use.
#[must_use]
pub fn body_to_voice(body: &Body) -> Option<VoiceKind> {
    Some(match body {
        Body::Humanoid(body) => match &body.body_type {
            humanoid::BodyType::Female => VoiceKind::HumanFemale,
            humanoid::BodyType::Male => VoiceKind::HumanMale,
        },
        Body::QuadrupedLow(body) => match body.species {
            quadruped_low::Species::Maneater => VoiceKind::Maneater,
            quadruped_low::Species::Alligator | quadruped_low::Species::Snaretongue => {
                VoiceKind::Alligator
            },
            quadruped_low::Species::SeaCrocodile => VoiceKind::SeaCrocodile,
            quadruped_low::Species::Dagon => VoiceKind::Dagon,
            quadruped_low::Species::Asp => VoiceKind::Asp,
            _ => return None,
        },
        Body::QuadrupedSmall(body) => match body.species {
            quadruped_small::Species::Truffler => VoiceKind::Truffler,
            quadruped_small::Species::Fungome => VoiceKind::Fungome,
            quadruped_small::Species::Sheep => VoiceKind::Sheep,
            quadruped_small::Species::Pig | quadruped_small::Species::Boar => VoiceKind::Pig,
            quadruped_small::Species::Cat => VoiceKind::Cat,
            quadruped_small::Species::Goat => VoiceKind::Goat,
            _ => VoiceKind::Critter,
        },
        Body::QuadrupedMedium(body) => match body.species {
            quadruped_medium::Species::Saber
            | quadruped_medium::Species::Tiger
            | quadruped_medium::Species::Lion
            | quadruped_medium::Species::Frostfang
            | quadruped_medium::Species::Snowleopard => VoiceKind::Lion,
            quadruped_medium::Species::Wolf => VoiceKind::Wolf,
            quadruped_medium::Species::Roshwalr
            | quadruped_medium::Species::Tarasque
            | quadruped_medium::Species::Darkhound
            | quadruped_medium::Species::Bonerattler
            | quadruped_medium::Species::Grolgar => VoiceKind::Canine,
            quadruped_medium::Species::Cattle
            | quadruped_medium::Species::Catoblepas
            | quadruped_medium::Species::Highland
            | quadruped_medium::Species::Yak
            | quadruped_medium::Species::Moose
            | quadruped_medium::Species::Dreadhorn => VoiceKind::Cow,
            quadruped_medium::Species::Antelope => VoiceKind::Antelope,
            _ => return None,
        },
        Body::BirdMedium(body) => match body.species {
            bird_medium::Species::BloodmoonBat | bird_medium::Species::VampireBat => {
                VoiceKind::VampireBat
            },
            _ => VoiceKind::Bird,
        },
        Body::BirdLarge(body) => match body.species {
            bird_large::Species::CloudWyvern
            | bird_large::Species::FlameWyvern
            | bird_large::Species::FrostWyvern
            | bird_large::Species::SeaWyvern
            | bird_large::Species::WealdWyvern => VoiceKind::Wyvern,
            bird_large::Species::Phoenix => VoiceKind::Phoenix,
            _ => VoiceKind::Bird,
        },
        Body::BipedSmall(body) => match body.species {
            biped_small::Species::Adlet => VoiceKind::Adlet,
            biped_small::Species::Mandragora => VoiceKind::Mandragora,
            biped_small::Species::Flamekeeper => VoiceKind::BipedLarge,
            biped_small::Species::GreenLegoom
            | biped_small::Species::OchreLegoom
            | biped_small::Species::PurpleLegoom
            | biped_small::Species::RedLegoom
            | biped_small::Species::UmberLegoom => VoiceKind::Legoom,
            _ => return None,
        },
        Body::BipedLarge(body) => match body.species {
            biped_large::Species::Wendigo => VoiceKind::Wendigo,
            biped_large::Species::Occultsaurok
            | biped_large::Species::Mightysaurok
            | biped_large::Species::Slysaurok => VoiceKind::Saurok,
            biped_large::Species::Mindflayer => VoiceKind::Mindflayer,
            _ => VoiceKind::BipedLarge,
        },
        Body::Theropod(_) | Body::Dragon(_) => VoiceKind::Reptile,
        Body::FishSmall(_) | Body::FishMedium(_) => VoiceKind::Marlin,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Deserialize, Hash, Eq)]
pub enum SfxInventoryEvent {
    Collected,
    CollectedTool(ToolKind),
    CollectedItem(String),
    CollectFailed,
    Consumed(ItemKey),
    Debug,
    Dropped,
    Given,
    Swapped,
    Craft,
}
