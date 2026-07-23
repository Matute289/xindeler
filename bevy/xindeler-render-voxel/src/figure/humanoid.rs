//! EM-3.8b — the HUMANOID figure path (the player + humanoid NPCs).
//!
//! This is the big visual jump over EM-3.8's quadruped-small pig: the local
//! player and every humanoid NPC stop being placeholder capsules and become
//! real assembled voxel characters on the streamed world.
//!
//! ## How voxygen assembles a humanoid (the reference we port)
//! `voxygen/src/scene/figure/load.rs`'s `HumSpec` reads a family of RON
//! manifests — a colour manifest (`humanoid_color_manifest`), a head manifest
//! (`humanoid_head_manifest`), and one manifest per armour slot
//! (`humanoid_armor_{chest,belt,pants,hand,foot,shoulder,…}_manifest`). For a
//! given `humanoid::Body` it:
//! 1. loads each part's `.vox` as a [`MatSegment`] — a voxel grid whose special
//!    palette indices (0..=6) are MATERIALS (`Skin`, `Hair`, `EyeDark`, …)
//!    rather than fixed colours;
//! 2. RECOLOURS those materials from the body's `(species, skin, hair, eye)`
//!    indices via the colour manifest (`HumColorSpec::color_segment`), turning
//!    the `MatSegment` into a concrete-colour [`Segment`]; greyscale armour
//!    voxels are additionally tinted by a per-item `recolor_grey`;
//! 3. places each finished part at its skeleton bone (the 16-bone
//!    `CharacterSkeleton`), each bone matrix already carrying the per-body
//!    model scale (`body.height() / 25`).
//!
//! ## What this module does (EM-3.8b + EM-3.8d)
//! - Ports the colour manifest + recolour EXACTLY (`HumColorSpec`,
//!   `recolor_grey`) — so skin/hair/eye colour is REAL, per-body.
//! - Ports the head manifest fully (bare head + eyes + hair + beard +
//!   accessory, unified like voxygen's `DynaUnionizer`).
//! - EM-3.8d: uses the character's **real equipped gear** (mirrored from the
//!   sim inventory as a [`FigureLoadout`]) — the equipped item's key selects
//!   its `.vox` from each armour manifest's `map`, falling back to `default`
//!   for empty slots (voxygen's `CharacterCacheKey`/`ArmorVoxSpecMap` mapping).
//!   The equipped weapon(s) ride the `main`/`second` bones (keyed by
//!   [`WeaponKey`] in `biped_weapon_manifest`), sheathed on the back with the
//!   real `ToolKind`/`Hands` pose; a carried lantern rides the `lantern` bone;
//!   back armour the `back` bone. We deserialize the frozen RON verbatim
//!   (isolation law rule 3 — the files are untouched).
//! - EM-3.8e: **head-slot helmets** — a species-keyed head-armour manifest
//!   ([`HumArmorHeadSpec`], ported from voxygen's `HumArmorHeadSpec`/
//!   `load_head`) resolves the equipped helmet's `.vox`, unioned into the SAME
//!   head assembly as the bare head/eyes/hair/beard/accessory pieces (voxygen's
//!   `mesh_head` — helmets are an EXTRA piece stacked on the bare head, not a
//!   swap, and they can hollow out hair/parts underneath via the `Cell`
//!   fill-override bits, ported verbatim as the unionizer's merge rule). **The
//!   glider** — its bone is always computed, but the `Idle`/`Run` humanoid
//!   animations force it to zero scale (ported unchanged from `xindeler-anim`'s
//!   `idle.rs`/`run.rs`, which already do this upstream); only
//!   [`HumAnim::Glide`] (driven by the mirror's new `NetLoadout::gliding` flag
//!   — the smallest addition that lets the client pick the right animation
//!   without replicating the whole `CharacterState`) scales it back to one.
//!   `GlideWield`, the more specific pre-jump pose, and the glider's live
//!   steering orientation (which needs physics data the mirror doesn't carry)
//!   are both approximated by the plain `Glide` animation with an identity
//!   orientation — a deliberate, documented simplification (see
//!   [`HumAnim::Glide`]), not a rendering bug.
//!
//! ## Recolour: honest scope
//! Skin, hair, eye and greyscale-armour recolour are all REAL here (ported from
//! `HumColorSpec` + `recolor_grey`).
//!
//! ## Purity
//! Same as the parent module: this is engine-shell code depending on the LOGIC
//! crates `common` (for `MatSegment`/`Material`/`DynaUnionizer`/the colour math
//! in `common::util`) + `xindeler-anim` (rest-pose + animation bone matrices).
//! It takes parsed `.vox` bytes from the caller; it never loads assets itself.

use std::collections::HashMap;

use bevy::transform::components::Transform;
use common::{
    comp::{
        humanoid::{Body, BodyType, EyeColor, Skin, Species},
        tool::{Hands, ToolKind},
    },
    figure::{Cell, DynaUnionizer, MatSegment, Material, Segment},
};
use dot_vox::DotVoxData;
use serde::Deserialize;
use vek::*;

use super::{mat_to_transform, segment_to_bevy};

/// The manifest ASSET PATHS the humanoid path reads (upstream names, frozen —
/// isolation law rule 3). The client resolves + loads these; this crate parses
/// the bytes into the structs below.
pub const HUM_COLOR_MANIFEST: &str = "voxygen.voxel.humanoid_color_manifest";
pub const HUM_HEAD_MANIFEST: &str = "voxygen.voxel.humanoid_head_manifest";
pub const HUM_ARMOR_CHEST_MANIFEST: &str = "voxygen.voxel.humanoid_armor_chest_manifest";
pub const HUM_ARMOR_BELT_MANIFEST: &str = "voxygen.voxel.humanoid_armor_belt_manifest";
pub const HUM_ARMOR_PANTS_MANIFEST: &str = "voxygen.voxel.humanoid_armor_pants_manifest";
pub const HUM_ARMOR_HAND_MANIFEST: &str = "voxygen.voxel.humanoid_armor_hand_manifest";
pub const HUM_ARMOR_FOOT_MANIFEST: &str = "voxygen.voxel.humanoid_armor_foot_manifest";
pub const HUM_ARMOR_SHOULDER_MANIFEST: &str = "voxygen.voxel.humanoid_armor_shoulder_manifest";
pub const HUM_ARMOR_BACK_MANIFEST: &str = "voxygen.voxel.humanoid_armor_back_manifest";
pub const HUM_MAIN_WEAPON_MANIFEST: &str = "voxygen.voxel.biped_weapon_manifest";
pub const HUM_LANTERN_MANIFEST: &str = "voxygen.voxel.humanoid_lantern_manifest";
/// The head-armour (helmet) manifest, keyed by `(species, body_type,
/// item-def-id)` (EM-3.8e).
pub const HUM_ARMOR_HEAD_MANIFEST: &str = "voxygen.voxel.humanoid_armor_head_manifest";
/// The glider manifest (EM-3.8e).
pub const HUM_GLIDER_MANIFEST: &str = "voxygen.voxel.humanoid_glider_manifest";

/// `.vox` files whose names are relative to the `voxygen.voxel` namespace
/// (`graceful_load_vox` prepends it in voxygen). The client prepends the same
/// prefix before resolving to a path.
pub const VOX_NAMESPACE: &str = "voxygen.voxel";

// ---------------------------------------------------------------------------
// Colour manifest (ported verbatim from `HumColorSpec` in load.rs)
// ---------------------------------------------------------------------------

/// Colour information not found in voxels, for humanoids (the recolour table).
/// Field-for-field identical to voxygen's `HumColorSpec` so the SAME frozen
/// `humanoid_color_manifest.ron` parses.
///
/// (No `Clone`/`Debug`: the `PureCases` case-elimination tables from
/// `make_case_elim!` implement neither — same as voxygen, which never clones
/// the colour spec.)
#[derive(Deserialize)]
pub struct HumColorSpec {
    hair_colors: common::comp::humanoid::species::PureCases<Vec<(u8, u8, u8)>>,
    eye_colors_light: common::comp::humanoid::eye_color::PureCases<(u8, u8, u8)>,
    eye_colors_dark: common::comp::humanoid::eye_color::PureCases<(u8, u8, u8)>,
    eye_white: (u8, u8, u8),
    skin_colors_plain: common::comp::humanoid::skin::PureCases<(u8, u8, u8)>,
    skin_colors_light: common::comp::humanoid::skin::PureCases<(u8, u8, u8)>,
    skin_colors_dark: common::comp::humanoid::skin::PureCases<(u8, u8, u8)>,
}

impl HumColorSpec {
    fn hair_color(&self, species: Species, val: u8) -> (u8, u8, u8) {
        species
            .elim_case_pure(&self.hair_colors)
            .get(val as usize)
            .copied()
            .unwrap_or((0, 0, 0))
    }

    /// Resolve a [`MatSegment`]'s materials to concrete colours for this body,
    /// yielding a renderable [`Segment`] (voxygen
    /// `HumColorSpec::color_segment`).
    fn color_segment(
        &self,
        mat_segment: MatSegment,
        skin: Skin,
        hair_color: (u8, u8, u8),
        eye_color: EyeColor,
    ) -> Segment {
        mat_segment.to_segment(|mat| {
            match mat {
                Material::Skin => *skin.elim_case_pure(&self.skin_colors_plain),
                Material::SkinDark => *skin.elim_case_pure(&self.skin_colors_dark),
                Material::SkinLight => *skin.elim_case_pure(&self.skin_colors_light),
                Material::Hair => hair_color,
                Material::EyeLight => *eye_color.elim_case_pure(&self.eye_colors_light),
                Material::EyeDark => *eye_color.elim_case_pure(&self.eye_colors_dark),
                Material::EyeWhite => self.eye_white,
            }
            .into()
        })
    }
}

/// Tint a greyscale voxel colour toward `color` (voxygen `load::recolor_grey`).
/// Only greys (`r == g == b`) are tinted; other colours pass through, so
/// pre-coloured detail on an armour piece survives.
fn recolor_grey(rgb: Rgb<u8>, color: Rgb<u8>) -> Rgb<u8> {
    use common::util::{linear_to_srgb, srgb_to_linear_fast};

    const BASE_GREY: f32 = 178.0;
    if rgb.r == rgb.g && rgb.g == rgb.b {
        let c1 = srgb_to_linear_fast(rgb.map(|e| e as f32 / BASE_GREY));
        let c2 = srgb_to_linear_fast(color.map(|e| e as f32 / 255.0));
        linear_to_srgb(c1 * c2).map(|e| (e.clamp(0.0, 1.0) * 255.0) as u8)
    } else {
        rgb
    }
}

// ---------------------------------------------------------------------------
// Head manifest (ported from `HumHeadSpec`)
// ---------------------------------------------------------------------------

/// A `.vox` reference with an offset + a model index: `("name", (x,y,z))` or
/// `("name", (x,y,z), idx)` (voxygen `VoxSpec<i32>`; the 3rd field defaults 0).
#[derive(Deserialize, Clone, Debug)]
pub struct VoxSpec(String, [i32; 3], #[serde(default)] u32);

/// One `(species, body_type)` head sub-spec (voxygen `HumHeadSubSpec`).
#[derive(Deserialize, Clone, Debug)]
pub struct HumHeadSubSpec {
    offset: [f32; 3],
    head: VoxSpec,
    eyes: Vec<Option<VoxSpec>>,
    hair: Vec<Option<VoxSpec>>,
    beard: Vec<Option<VoxSpec>>,
    accessory: Vec<Option<VoxSpec>>,
}

/// The whole head manifest (voxygen `HumHeadSpec`).
#[derive(Deserialize, Clone, Debug)]
pub struct HumHeadSpec(std::collections::HashMap<(Species, BodyType), HumHeadSubSpec>);

// ---------------------------------------------------------------------------
// Armour + weapon manifests — real equipped gear (EM-3.8d)
// ---------------------------------------------------------------------------

/// A `.vox` reference with a float offset (voxygen `VoxSpec<f32>`).
#[derive(Deserialize, Clone, Debug)]
pub struct VoxSpecF(String, [f32; 3], #[serde(default)] u32);

/// One armour piece spec (voxygen `ArmorVoxSpec`).
#[derive(Deserialize, Clone, Debug)]
pub struct ArmorVoxSpec {
    vox_spec: VoxSpecF,
    color: Option<[u8; 3]>,
}

/// A sided (left/right) armour spec (voxygen `SidedArmorVoxSpec`).
#[derive(Deserialize, Clone, Debug)]
pub struct SidedArmorVoxSpec {
    left: ArmorVoxSpec,
    right: ArmorVoxSpec,
}

/// An armour manifest's inner map: the `default` slot plus the per-item `map`
/// of equippable variants keyed by item-definition-id (voxygen
/// `ArmorVoxSpecMap<String, S>`). EM-3.8d reads BOTH — the equipped item's key
/// selects its variant, falling back to `default` when the slot is empty or the
/// item has no entry.
#[derive(Deserialize, Clone, Debug)]
pub struct ArmorSpecMap<S> {
    default: S,
    #[serde(default = "HashMap::new")]
    map: HashMap<String, S>,
}

/// An armour manifest as a NEWTYPE around its map (voxygen wraps each manifest
/// in a 1-tuple struct — e.g. `HumArmorChestSpec(ArmorVoxSpecMap)` — so the RON
/// begins `( ( default: … , map: { … } ) )`). We mirror that outer wrapper so
/// the SAME frozen RON parses.
#[derive(Deserialize, Clone, Debug)]
pub struct ArmorManifest<S>(ArmorSpecMap<S>);

impl<S> ArmorManifest<S> {
    /// Resolve the spec for an equipped item `key` (its item-definition-id),
    /// falling back to the `default` slot when the slot is empty or the item
    /// has no manifest entry (voxygen's `map.get(key).unwrap_or(&default)`
    /// + `not_found` fallback, minus the debug mesh).
    fn resolve(&self, key: Option<&str>) -> &S {
        key.and_then(|k| self.0.map.get(k))
            .unwrap_or(&self.0.default)
    }
}

/// The armour manifests (default + per-item map).
pub type HumArmorChestSpec = ArmorManifest<ArmorVoxSpec>;
pub type HumArmorBeltSpec = ArmorManifest<ArmorVoxSpec>;
pub type HumArmorPantsSpec = ArmorManifest<ArmorVoxSpec>;
pub type HumArmorFootSpec = ArmorManifest<ArmorVoxSpec>;
pub type HumArmorBackSpec = ArmorManifest<ArmorVoxSpec>;
pub type HumArmorHandSpec = ArmorManifest<SidedArmorVoxSpec>;
pub type HumArmorShoulderSpec = ArmorManifest<SidedArmorVoxSpec>;
pub type HumLanternSpec = ArmorManifest<ArmorVoxSpec>;
/// The glider manifest (EM-3.8e) — same shape as lantern/back: a plain
/// `String` item-def-id key, `default` = the empty (`armor.empty`) `.vox`
/// (voxygen `HumArmorGliderSpec`; `mesh_glider` material-recolours it exactly
/// like a body-slot armour piece, so it reuses [`HumVoxRole::Body`]).
pub type HumGliderSpec = ArmorManifest<ArmorVoxSpec>;

/// The head-armour (helmet) manifest inner map (voxygen's
/// `HumArmorHeadSpec(ArmorVoxSpecMap<(Species, BodyType, String),
/// ArmorVoxSpec>)`). Keyed on `(species, body_type, item-def-id)` because,
/// unlike every other slot, a helmet's shape depends on the wearer's head
/// (EM-3.8e).
#[derive(Deserialize, Clone, Debug)]
struct HumArmorHeadSpecMap {
    /// Present in the frozen RON (every `ArmorVoxSpecMap` has one) but never
    /// read — voxygen's `load_head` returns `None` for an unequipped head
    /// rather than falling back to a generic default (there is no "bare
    /// helmet" model; the bare head IS the head).
    #[allow(dead_code)]
    default: ArmorVoxSpec,
    #[serde(default = "HashMap::new")]
    map: HashMap<(Species, BodyType, String), ArmorVoxSpec>,
}

/// The head-armour (helmet) manifest (EM-3.8e). Deserializes the SAME frozen
/// `humanoid_armor_head_manifest.ron` voxygen reads (isolation law rule 3).
#[derive(Deserialize, Clone, Debug)]
pub struct HumArmorHeadSpec(HumArmorHeadSpecMap);

impl HumArmorHeadSpec {
    /// Resolve the equipped helmet's spec for this `(species, body_type)`, or
    /// `None` if no head item is equipped OR the manifest has no entry for
    /// this exact `(species, body_type, item)` combination (voxygen's
    /// `load_head`: an absent `head` short-circuits to no extra mesh at all —
    /// there is no `default` fallback here, unlike every other armour slot).
    fn resolve(
        &self,
        species: Species,
        body_type: BodyType,
        head: Option<&str>,
    ) -> Option<&ArmorVoxSpec> {
        let head = head?;
        self.0.map.get(&(species, body_type, head.to_owned()))
    }
}

/// The figure-manifest key of a tool (voxygen `ToolKey`): a simple item id or a
/// modular weapon's `(primary, secondary, hands)` key. Deserializes the frozen
/// `biped_weapon_manifest` map keys verbatim (`Tool("…")` / `Modular((…))`).
#[derive(Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum WeaponKey {
    /// A non-modular tool, keyed by its item-definition-id.
    Tool(String),
    /// A modular weapon, keyed by `(primary, secondary, hands)`.
    Modular((String, String, Hands)),
}

/// The main-weapon manifest (`biped_weapon_manifest`): each `.vox` + offset
/// keyed by [`WeaponKey`]. The RON is a 1-tuple around the map (`({ key: …
/// })`).
#[derive(Deserialize, Clone, Debug)]
pub struct HumMainWeaponSpec(HashMap<WeaponKey, ArmorVoxSpec>);

impl HumMainWeaponSpec {
    fn get(&self, key: &WeaponKey) -> Option<&ArmorVoxSpec> { self.0.get(key) }
}

/// The parsed humanoid manifests, bundled by BORROW so the caller hands
/// references straight from wherever it stored them (e.g. Bevy asset stores)
/// with no cloning — [`HumColorSpec`] is not `Clone`, so an owned bundle would
/// be awkward; a reference bundle is the natural fit.
pub struct HumManifests<'a> {
    pub color: &'a HumColorSpec,
    pub head: &'a HumHeadSpec,
    pub chest: &'a HumArmorChestSpec,
    pub belt: &'a HumArmorBeltSpec,
    pub pants: &'a HumArmorPantsSpec,
    pub foot: &'a HumArmorFootSpec,
    pub hand: &'a HumArmorHandSpec,
    pub shoulder: &'a HumArmorShoulderSpec,
    pub back: &'a HumArmorBackSpec,
    pub main_weapon: &'a HumMainWeaponSpec,
    pub lantern: &'a HumLanternSpec,
    /// The head-armour (helmet) manifest (EM-3.8e).
    pub armor_head: &'a HumArmorHeadSpec,
    /// The glider manifest (EM-3.8e).
    pub glider: &'a HumGliderSpec,
}

/// The figure-relevant equipped gear the humanoid assembly consumes (EM-3.8d) —
/// the render-crate-native mirror of the client's replicated loadout, so this
/// crate stays protocol-free. The client builds it from `NetLoadout`. Armour
/// slots carry the equipped item-definition-id (`None` = default/empty); tools
/// carry their [`WeaponKey`] + `ToolKind`/`Hands`.
#[derive(Clone, Debug, Default)]
pub struct FigureLoadout {
    pub active_tool: Option<FigureTool>,
    pub second_tool: Option<FigureTool>,
    pub chest: Option<String>,
    pub belt: Option<String>,
    pub back: Option<String>,
    pub pants: Option<String>,
    pub shoulder: Option<String>,
    pub hand: Option<String>,
    pub foot: Option<String>,
    pub lantern: Option<String>,
    /// Equipped helmet item-def-id (EM-3.8e). `None` = no extra head mesh.
    pub head: Option<String>,
    /// Equipped glider item-def-id (EM-3.8e). Only the ITEM — see
    /// [`FigureLoadout`]'s caller (`NetLoadout::gliding`) for the transient
    /// glide-visibility signal, which lives outside this protocol-free type.
    pub glider: Option<String>,
}

/// An equipped tool for figure assembly: its manifest key plus the
/// `ToolKind`/`Hands` the animation needs to sheathe it.
#[derive(Clone, Debug)]
pub struct FigureTool {
    pub key: WeaponKey,
    pub kind: ToolKind,
    pub hands: Hands,
}

/// The active/second tool KINDS (+ hands), extracted from a [`FigureLoadout`],
/// that drive the character animation's back-sheathe pose (`do_tools_on_back`).
/// Kept separate from the `.vox` refs so the per-frame animation needs only
/// this tiny `Copy` value (stored on the figure), not the whole loadout.
#[derive(Clone, Copy, Debug, Default)]
pub struct FigureToolKinds {
    pub active: Option<ToolKind>,
    pub second: Option<ToolKind>,
    /// `(main-hand hands, off-hand hands)` — the tuple the anim dep expects.
    pub hands: (Option<Hands>, Option<Hands>),
}

impl FigureLoadout {
    /// The tool kinds/hands this loadout implies (for the animation).
    #[must_use]
    pub fn tool_kinds(&self) -> FigureToolKinds {
        FigureToolKinds {
            active: self.active_tool.as_ref().map(|t| t.kind),
            second: self.second_tool.as_ref().map(|t| t.kind),
            hands: (
                self.active_tool.as_ref().map(|t| t.hands),
                self.second_tool.as_ref().map(|t| t.hands),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Which bone each humanoid part parents to (a subset of the 16-bone skeleton)
// ---------------------------------------------------------------------------

/// The humanoid bones v1 places parts on (matches `ComputedCharacterSkeleton`
/// field names / `voxygen/anim/src/character`). Pants ride the `shorts` bone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumBone {
    Head,
    Chest,
    Belt,
    Shorts,
    HandL,
    HandR,
    FootL,
    FootR,
    ShoulderL,
    ShoulderR,
    /// The main-hand weapon bone (EM-3.8c). Holds the active tool `.vox`.
    Main,
    /// The off-hand weapon bone (EM-3.8d). Holds the second/off-hand tool.
    Second,
    /// The back bone (EM-3.8d). Holds back armour (cape / pack).
    Back,
    /// The lantern bone (EM-3.8d). Holds a carried lantern at the hip.
    Lantern,
    /// The glider bone (EM-3.8e). Holds the equipped glider; its scale is
    /// zero outside [`HumAnim::Glide`] (see the module doc).
    Glider,
}

impl HumBone {
    fn label(self) -> &'static str {
        match self {
            HumBone::Head => "head",
            HumBone::Chest => "chest",
            HumBone::Belt => "belt",
            HumBone::Shorts => "shorts",
            HumBone::HandL => "hand_l",
            HumBone::HandR => "hand_r",
            HumBone::FootL => "foot_l",
            HumBone::FootR => "foot_r",
            HumBone::ShoulderL => "shoulder_l",
            HumBone::ShoulderR => "shoulder_r",
            HumBone::Main => "main",
            HumBone::Second => "second",
            HumBone::Back => "back",
            HumBone::Lantern => "lantern",
            HumBone::Glider => "glider",
        }
    }
}

// ---------------------------------------------------------------------------
// Part 1: which `.vox` files a humanoid needs (so the client can load them)
// ---------------------------------------------------------------------------

/// A `.vox` file the humanoid assembly needs, keyed so the caller can load the
/// bytes then hand them back paired with the right recolour rule.
#[derive(Clone, Debug)]
pub struct HumVoxRef {
    /// Which slot/role this `.vox` fills (drives the recolour rule at
    /// assembly).
    pub role: HumVoxRole,
    /// The `.vox` name RELATIVE to the `voxygen.voxel` namespace (the caller
    /// prepends it; identical to voxygen's `graceful_load_vox`).
    pub vox_name: String,
    /// Which model inside the `.vox` (voxygen `model_index`).
    pub model_index: u32,
}

/// The role a loaded `.vox` plays, so the assembler knows how to recolour +
/// where to place it. Each role now carries the resolved manifest offset/tint
/// (EM-3.8d), so assembly needs only the roles — not the manifests — for
/// placement.
#[derive(Clone, Debug, PartialEq)]
pub enum HumVoxRole {
    HeadBare {
        offset: [i32; 3],
    },
    HeadEyes {
        offset: [i32; 3],
    },
    HeadHair {
        offset: [i32; 3],
    },
    HeadBeard {
        offset: [i32; 3],
    },
    HeadAccessory {
        offset: [i32; 3],
    },
    /// An equipped helmet (EM-3.8e): a plain (non-material-recoloured)
    /// segment, optionally grey-tinted, unioned into the SAME head assembly
    /// as the bare head/eyes/hair/beard/accessory (voxygen `load_head` +
    /// `mesh_head`'s `maybe_add(helmet)`).
    HeadArmor {
        offset: [i32; 3],
        tint: Option<[u8; 3]>,
    },
    /// A material-recoloured body-slot armour (chest/belt/pants/back/lantern):
    /// skin/hair recolour + optional grey tint, on `bone` at `offset`.
    Body {
        bone: HumBone,
        offset: [f32; 3],
        tint: Option<[u8; 3]>,
    },
    /// A hand/foot/shoulder — sided (left parts are the right `.vox` mirrored).
    Sided {
        bone: HumBone,
        flipped: bool,
        tint: Option<[u8; 3]>,
        offset: [f32; 3],
    },
    /// A weapon on the `main`/`second` bone (plain colour, no recolour).
    /// `flipped` mirrors the off-hand tool (voxygen `mesh_main_weapon(_,
    /// true)`).
    Weapon {
        bone: HumBone,
        flipped: bool,
        offset: [f32; 3],
    },
}

/// The list of `.vox` files (+ their roles) a humanoid figure needs, in the
/// order the assembler expects. The caller loads each `vox_name`, then calls
/// [`assemble_humanoid`] with the parsed data in the SAME order. `loadout`
/// picks the REAL equipped `.vox` per slot (EM-3.8d), falling back to each
/// manifest's `default` for empty slots.
///
/// Returns `None` if the head manifest has no entry for this `(species,
/// body_type)` (voxygen's `not_found` fallback — the caller keeps its capsule).
#[must_use]
pub fn humanoid_vox_refs(
    manifests: &HumManifests<'_>,
    body: &Body,
    loadout: &FigureLoadout,
) -> Option<Vec<HumVoxRef>> {
    let head = manifests.head.0.get(&(body.species, body.body_type))?;
    let mut refs = Vec::new();

    // --- Head sub-parts (bare + eyes + hair + beard + accessory) ---
    refs.push(HumVoxRef {
        role: HumVoxRole::HeadBare {
            offset: head.head.1,
        },
        vox_name: head.head.0.clone(),
        model_index: head.head.2,
    });
    if let Some(Some(spec)) = head.eyes.get(body.eyes as usize) {
        refs.push(HumVoxRef {
            role: HumVoxRole::HeadEyes { offset: spec.1 },
            vox_name: spec.0.clone(),
            model_index: spec.2,
        });
    }
    if let Some(Some(spec)) = head.hair.get(body.hair_style as usize) {
        refs.push(HumVoxRef {
            role: HumVoxRole::HeadHair { offset: spec.1 },
            vox_name: spec.0.clone(),
            model_index: spec.2,
        });
    }
    if let Some(Some(spec)) = head.beard.get(body.beard as usize) {
        refs.push(HumVoxRef {
            role: HumVoxRole::HeadBeard { offset: spec.1 },
            vox_name: spec.0.clone(),
            model_index: spec.2,
        });
    }
    if let Some(Some(spec)) = head.accessory.get(body.accessory as usize) {
        refs.push(HumVoxRef {
            role: HumVoxRole::HeadAccessory { offset: spec.1 },
            vox_name: spec.0.clone(),
            model_index: spec.2,
        });
    }
    // --- Equipped helmet (EM-3.8e): an EXTRA piece unioned into the head, ---
    // not a bone of its own — see `HumVoxRole::HeadArmor`.
    if let Some(spec) =
        manifests
            .armor_head
            .resolve(body.species, body.body_type, loadout.head.as_deref())
    {
        refs.push(HumVoxRef {
            role: HumVoxRole::HeadArmor {
                offset: Vec3::<f32>::from(spec.vox_spec.1).as_::<i32>().into_array(),
                tint: spec.color,
            },
            vox_name: spec.vox_spec.0.clone(),
            model_index: spec.vox_spec.2,
        });
    }

    // --- Body slots: resolve the equipped item (or the manifest default). ---
    // chest/belt/pants always render (their `default` is the bare torso/legs);
    // back only when equipped (its `default` is the empty `armor.empty`).
    refs.push(body_ref(
        HumBone::Chest,
        manifests.chest.resolve(loadout.chest.as_deref()),
    ));
    refs.push(body_ref(
        HumBone::Belt,
        manifests.belt.resolve(loadout.belt.as_deref()),
    ));
    refs.push(body_ref(
        HumBone::Shorts,
        manifests.pants.resolve(loadout.pants.as_deref()),
    ));
    if loadout.back.is_some() {
        refs.push(body_ref(
            HumBone::Back,
            manifests.back.resolve(loadout.back.as_deref()),
        ));
    }

    // Sided: hands, feet, shoulders (left = right `.vox` mirrored for
    // hands/feet; shoulders have distinct left/right specs).
    let hand = manifests.hand.resolve(loadout.hand.as_deref());
    refs.push(sided_ref(HumBone::HandL, &hand.left, true));
    refs.push(sided_ref(HumBone::HandR, &hand.right, false));
    let foot = manifests.foot.resolve(loadout.foot.as_deref());
    // Feet share one spec, left mirrored (voxygen `mesh_foot(flipped)`).
    refs.push(sided_ref(HumBone::FootL, foot, true));
    refs.push(sided_ref(HumBone::FootR, foot, false));
    let shoulder = manifests.shoulder.resolve(loadout.shoulder.as_deref());
    refs.push(sided_ref(HumBone::ShoulderL, &shoulder.left, true));
    refs.push(sided_ref(HumBone::ShoulderR, &shoulder.right, false));

    // --- Lantern (EM-3.8d): only when carried; hangs on the `lantern` bone. ---
    if loadout.lantern.is_some() {
        refs.push(body_ref(
            HumBone::Lantern,
            manifests.lantern.resolve(loadout.lantern.as_deref()),
        ));
    }

    // --- Glider (EM-3.8e): only when equipped; hangs on the `glider` bone at
    // zero scale outside `HumAnim::Glide` (see the module doc) — same
    // material-recoloured `Body` role as lantern (voxygen `mesh_glider`).
    if loadout.glider.is_some() {
        refs.push(body_ref(
            HumBone::Glider,
            manifests.glider.resolve(loadout.glider.as_deref()),
        ));
    }

    // --- Equipped weapons (EM-3.8d): active on `main`, off-hand on `second`. ---
    if let Some(tool) = &loadout.active_tool {
        if let Some(spec) = manifests.main_weapon.get(&tool.key) {
            refs.push(weapon_ref(HumBone::Main, spec, false));
        } else {
            tracing::warn!(
                tool_key = ?tool.key,
                "no biped_weapon_manifest entry for equipped active tool; rendering unarmed"
            );
        }
    }
    if let Some(tool) = &loadout.second_tool {
        if let Some(spec) = manifests.main_weapon.get(&tool.key) {
            refs.push(weapon_ref(HumBone::Second, spec, true));
        } else {
            tracing::warn!(
                tool_key = ?tool.key,
                "no biped_weapon_manifest entry for equipped second tool; rendering unarmed"
            );
        }
    }

    Some(refs)
}

/// A material-recoloured body-slot `.vox` ref (chest/belt/pants/back/lantern).
fn body_ref(bone: HumBone, spec: &ArmorVoxSpec) -> HumVoxRef {
    HumVoxRef {
        role: HumVoxRole::Body {
            bone,
            offset: spec.vox_spec.1,
            tint: spec.color,
        },
        vox_name: spec.vox_spec.0.clone(),
        model_index: spec.vox_spec.2,
    }
}

fn sided_ref(bone: HumBone, spec: &ArmorVoxSpec, flipped: bool) -> HumVoxRef {
    HumVoxRef {
        role: HumVoxRole::Sided {
            bone,
            flipped,
            tint: spec.color,
            offset: spec.vox_spec.1,
        },
        vox_name: spec.vox_spec.0.clone(),
        model_index: spec.vox_spec.2,
    }
}

fn weapon_ref(bone: HumBone, spec: &ArmorVoxSpec, flipped: bool) -> HumVoxRef {
    HumVoxRef {
        role: HumVoxRole::Weapon {
            bone,
            flipped,
            offset: spec.vox_spec.1,
        },
        vox_name: spec.vox_spec.0.clone(),
        model_index: spec.vox_spec.2,
    }
}

// ---------------------------------------------------------------------------
// Part 2: assemble the loaded `.vox` data into placed, coloured parts
// ---------------------------------------------------------------------------

/// A loaded `.vox` for a humanoid part, paired back with its role.
pub struct LoadedHumPart<'a> {
    pub role: HumVoxRole,
    pub vox: &'a DotVoxData,
    pub model_index: u32,
}

/// One assembled humanoid part: a coloured `bevy::Mesh` + the bone it parents
/// to. The mesh is already in the part's LOCAL voxel frame (offset baked in);
/// the caller places it with the bone's [`Transform`] from
/// [`humanoid_bone_transforms`].
pub struct HumAssembledPart {
    pub mesh: bevy::mesh::Mesh,
    pub bone: HumBone,
    pub name: &'static str,
}

/// Assembles the loaded humanoid `.vox` parts into placed, recoloured meshes
/// (voxygen's per-slot `mesh_*` functions). Each part's placement/tint comes
/// from its role (resolved in [`humanoid_vox_refs`] from the real loadout —
/// EM-3.8d), so this needs only the colour spec, not the manifests.
///
/// The head is unified (bare + eyes + hair + beard + accessory) into ONE mesh
/// exactly like voxygen's `DynaUnionizer` in `mesh_head`; every other slot is
/// one mesh. Empty parts are dropped.
#[must_use]
pub fn assemble_humanoid(
    body: &Body,
    manifests: &HumManifests<'_>,
    parts: &[LoadedHumPart],
) -> Vec<HumAssembledPart> {
    let color = manifests.color;
    let skin = body.species.skin_color(body.skin);
    let hair_color = color.hair_color(body.species, body.hair_color);
    let hair_rgb: Rgb<u8> = hair_color.into();
    let eye = body.species.eye_color(body.eye_color);

    let mut out = Vec::new();

    // --- Head: unify all its sub-parts into one Segment (voxygen mesh_head) ---
    // The head sub-spec's own float `offset` recentres the whole head around the
    // bone; each piece's integer offset positions it within that frame (voxygen
    // `HumHeadSubSpec.offset` + each piece's Vec3 offset).
    let head_manifest_offset = head_spec_offset(manifests, body).unwrap_or_else(Vec3::zero);
    let mut unionizer = DynaUnionizer::new();
    let mut has_head = false;
    for p in parts {
        match p.role {
            HumVoxRole::HeadBare { offset } => {
                let seg = color.color_segment(
                    mat_seg(p.vox, false, p.model_index),
                    skin,
                    hair_color,
                    eye,
                );
                unionizer = unionizer.add(seg, Vec3::from(offset));
                has_head = true;
            },
            HumVoxRole::HeadEyes { offset } => {
                let seg = color.color_segment(
                    mat_seg(p.vox, false, p.model_index).map_rgb(|rgb| recolor_grey(rgb, hair_rgb)),
                    skin,
                    hair_color,
                    eye,
                );
                unionizer = unionizer.add(seg, Vec3::from(offset));
            },
            HumVoxRole::HeadHair { offset } | HumVoxRole::HeadBeard { offset } => {
                // Hair/beard are plain (non-material) segments recoloured to the
                // hair colour (voxygen `graceful_load_segment(..).map_rgb`).
                let seg = plain_seg(p.vox, false, p.model_index)
                    .map_rgb(|rgb| recolor_grey(rgb, hair_rgb));
                unionizer = unionizer.add(seg, Vec3::from(offset));
            },
            HumVoxRole::HeadAccessory { offset } => {
                let seg = plain_seg(p.vox, false, p.model_index);
                unionizer = unionizer.add(seg, Vec3::from(offset));
            },
            HumVoxRole::HeadArmor { offset, tint } => {
                // EM-3.8e: a helmet is a plain segment (no skin/hair/eye
                // material recolour — voxygen `load_head` uses
                // `graceful_load_segment`, not `graceful_load_mat_segment`),
                // optionally grey-tinted like a body-slot armour piece.
                let mut seg = plain_seg(p.vox, false, p.model_index);
                if let Some(c) = tint {
                    let tint_rgb = Rgb::from(Vec3::from(c));
                    seg = seg.map_rgb(|rgb| recolor_grey(rgb, tint_rgb));
                }
                unionizer = unionizer.add(seg, Vec3::from(offset));
            },
            _ => {},
        }
    }
    if has_head {
        // EM-3.8e: the same hollow/override merge rule voxygen's `mesh_head`
        // uses (`Cell`'s fill bits) so a helmet can correctly cut away hair/
        // head geometry poking through it, instead of z-fighting or clipping
        // (ported verbatim from `HumHeadSpec::mesh_head`'s `unify_with`).
        let (head_seg, origin_offset) = unionizer.unify_with(|v: Cell, old_v: Cell| {
            if old_v.is_override_hollow() {
                old_v
            } else if v.is_hollowing() && !old_v.is_override_hollow() {
                Cell::empty()
            } else if v.is_filled() {
                v
            } else {
                old_v
            }
        });
        // voxygen: final offset = spec.offset + (-origin_offset) (the unionizer
        // re-origins to the min corner; shift back so the bone origin lines up).
        let offset = head_manifest_offset + origin_offset.map(|e| -(e as f32));
        if let Some(mesh) = segment_to_bevy(&head_seg, offset) {
            out.push(HumAssembledPart {
                mesh,
                bone: HumBone::Head,
                name: HumBone::Head.label(),
            });
        }
    }

    // --- Body / sided / weapon slots ---
    // Body + sided armour are material-recoloured (skin/hair) + optionally
    // grey-tinted from the manifest `color`; weapons are plain colour. The
    // resolved `.vox` (default OR the real equipped item) already rode in via
    // the role (EM-3.8d).
    for p in parts {
        let (bone, seg, offset) = match &p.role {
            HumVoxRole::Body { bone, offset, tint } => {
                let seg = tinted_body_seg(color, p, false, skin, hair_color, eye, *tint);
                (*bone, seg, Vec3::from(*offset))
            },
            HumVoxRole::Sided {
                bone,
                flipped,
                tint,
                offset,
            } => {
                let seg = tinted_body_seg(color, p, *flipped, skin, hair_color, eye, *tint);
                (*bone, seg, Vec3::from(*offset))
            },
            HumVoxRole::Weapon {
                bone,
                flipped,
                offset,
            } => {
                // The weapon is a plain-colour `.vox` (no material recolour),
                // placed on the `main`/`second` bone (voxygen `mesh_main_weapon`).
                let seg = plain_seg(p.vox, *flipped, p.model_index);
                let mut off: Vec3<f32> = Vec3::from(*offset);
                if *flipped {
                    // voxygen: mirroring the off-hand `.vox` also mirrors its
                    // x-offset about the segment width.
                    off.x = -off.x - seg.sz.x as f32;
                }
                (*bone, seg, off)
            },
            _ => continue,
        };
        if let Some(mesh) = segment_to_bevy(&seg, offset) {
            out.push(HumAssembledPart {
                mesh,
                bone,
                name: bone.label(),
            });
        }
    }

    out
}

/// A material-recoloured body-slot armour segment (voxygen
/// chest/belt/pants/back tint): parse (optionally mirrored) → optional grey
/// tint → skin/hair recolour.
fn tinted_body_seg(
    color: &HumColorSpec,
    p: &LoadedHumPart,
    flipped: bool,
    skin: Skin,
    hair_color: (u8, u8, u8),
    eye: EyeColor,
    tint: Option<[u8; 3]>,
) -> Segment {
    let mut seg = mat_seg(p.vox, flipped, p.model_index);
    if let Some(c) = tint {
        let tint_rgb = Rgb::from(Vec3::from(c));
        seg = seg.map_rgb(|rgb| recolor_grey(rgb, tint_rgb));
    }
    color.color_segment(seg, skin, hair_color, eye)
}

/// Parse a `.vox` model into a [`MatSegment`] (materials preserved for
/// recolour).
fn mat_seg(vox: &DotVoxData, flipped: bool, model_index: u32) -> MatSegment {
    MatSegment::from_vox(vox, flipped, model_index as usize)
}

/// Parse a `.vox` model into a plain (colour) [`Segment`] (hair/beard/accessory
/// — no material recolour, voxygen `graceful_load_segment`).
fn plain_seg(vox: &DotVoxData, flipped: bool, model_index: u32) -> Segment {
    Segment::from_vox(vox, flipped, model_index as usize, None)
}

/// The head sub-spec's float offset for this body (voxygen
/// `HumHeadSubSpec.offset`).
fn head_spec_offset(manifests: &HumManifests<'_>, body: &Body) -> Option<Vec3<f32>> {
    manifests
        .head
        .0
        .get(&(body.species, body.body_type))
        .map(|s| Vec3::from(s.offset))
}

// ---------------------------------------------------------------------------
// Bones: rest pose + animated pose (Part B)
// ---------------------------------------------------------------------------

/// The full set of bone transforms a humanoid figure places its parts on (Bevy
/// space, model-scaled), for the bones v1 uses.
pub struct HumBoneTransforms {
    pub head: Transform,
    pub chest: Transform,
    pub belt: Transform,
    pub shorts: Transform,
    pub hand_l: Transform,
    pub hand_r: Transform,
    pub foot_l: Transform,
    pub foot_r: Transform,
    pub shoulder_l: Transform,
    pub shoulder_r: Transform,
    /// The main-hand weapon bone (EM-3.8c).
    pub main: Transform,
    /// The off-hand weapon bone (EM-3.8d).
    pub second: Transform,
    /// The back bone (EM-3.8d — back armour).
    pub back: Transform,
    /// The lantern bone (EM-3.8d — carried lantern at the hip).
    pub lantern: Transform,
    /// The glider bone (EM-3.8e — carried glider; zero-scaled outside
    /// [`HumAnim::Glide`]).
    pub glider: Transform,
}

impl HumBoneTransforms {
    #[must_use]
    pub fn get(&self, bone: HumBone) -> Transform {
        match bone {
            HumBone::Head => self.head,
            HumBone::Chest => self.chest,
            HumBone::Belt => self.belt,
            HumBone::Shorts => self.shorts,
            HumBone::HandL => self.hand_l,
            HumBone::HandR => self.hand_r,
            HumBone::FootL => self.foot_l,
            HumBone::FootR => self.foot_r,
            HumBone::ShoulderL => self.shoulder_l,
            HumBone::ShoulderR => self.shoulder_r,
            HumBone::Main => self.main,
            HumBone::Second => self.second,
            HumBone::Back => self.back,
            HumBone::Lantern => self.lantern,
            HumBone::Glider => self.glider,
        }
    }
}

/// Which locomotion state to animate the humanoid in (Part B). Picked
/// client-side from the entity's replicated velocity: near-zero → idle,
/// otherwise → run/walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumAnim {
    /// Standing idle (subtle breathing/sway).
    Idle,
    /// Moving on the ground (run cycle; also used for walk in v1).
    Run,
    /// Airborne under an open glider (EM-3.8e), driven by the mirror's
    /// `NetLoadout::gliding` flag. Dispatches to `xindeler-anim`'s
    /// `GlidingAnimation`, which scales the glider bone back to one (`Idle`/
    /// `Run` bake it to zero — see the module doc). Approximates BOTH
    /// voxygen's `CharacterState::Glide` and `GlideWield` (the more specific
    /// pre-jump pose isn't separately modelled) with an IDENTITY body/glider
    /// orientation (the mirror doesn't carry the physics-computed bank
    /// angle) — a deliberate simplification: the glider renders correctly
    /// placed and visible, just without live steering/banking motion.
    Glide,
}

/// Compute the humanoid bone transforms for `body` at animation `anim`, phase
/// accumulator `acc` and `time` seconds (Part B). `time` drives the cyclic
/// idle sway; `acc` (blocks travelled, advanced by the caller as `speed * dt`)
/// drives the foot cycle so it stays phase-continuous across speed changes
/// (EM-3.8c polish minor a). Passing `acc = 0`, `time = 0` with
/// [`HumAnim::Idle`] yields the deterministic rest pose (the EM-3.8 static
/// path).
///
/// `ground_speed` (blocks/s, from the entity's replicated velocity) scales the
/// run cycle so a slow walk animates slower than a sprint.
///
/// `tools` are the REAL equipped tool kinds/hands (EM-3.8d — from the mirrored
/// loadout), so the idle/run animation's `do_tools_on_back(hands,
/// active_tool_kind, ..)` step sheathes the actual weapon(s) on the back with
/// the right pose (a 2H sword lies flat high on the back, a 1H axe hangs at the
/// hip, …). An empty `tools` = an unarmed figure (bare hands, no back weapon).
#[must_use]
pub fn humanoid_bone_transforms(
    body: &Body,
    anim: HumAnim,
    acc: f32,
    time: f32,
    ground_speed: f32,
    tools: FigureToolKinds,
) -> HumBoneTransforms {
    use xindeler_anim::{
        Animation, Skeleton,
        character::{
            CharacterSkeleton, GlidingAnimation, IdleAnimation, RunAnimation, SkeletonAttr,
        },
    };

    let attr = SkeletonAttr::from(body);
    let mut rate = 0.0;
    // IMPORTANT: build the skeleton with `squash = 1.0` (no squash). The
    // `Default` impl leaves `squash = 0.0`, which the skeleton's `squash_chest`/
    // `squash_limb` closures treat as an EXTREME squash — rotating the chest
    // ~2 rad about x and zeroing the vertical offsets, which lays the whole
    // figure flat. Voxygen always constructs it via `CharacterSkeleton::new(..,
    // squash = 1.0)`; we must match that. (`holding_lantern = false` — a carried
    // lantern rides the hip, not the hand; `back_carry_offset = 0.0` — no
    // backpack detection yet.)
    let base = CharacterSkeleton::new(false, 0.0, 1.0);

    // EM-3.8d: drive the sheathe pose from the REAL equipped tools. The
    // idle/run animations call `do_tools_on_back(hands, active_tool_kind,
    // second_tool_kind, ..)` internally, which places the `main`/`second` bones
    // SHEATHED on the back with a per-`ToolKind` pose. Without the right kind the
    // long flat weapon `.vox` would render as a big vertical slab at the hand
    // (the EM-3.8c-review bug); with it the weapon sits correctly on the back.
    // We do NOT drive a wield/attack pose (no live CharacterState in the mirror)
    // — "weapon on the back" is the correct neutral pose for a non-attacking
    // character, exactly what voxygen shows for an idle figure.
    let active_tool = tools.active;
    let second_tool = tools.second;
    let hands = tools.hands;

    let skeleton = match anim {
        HumAnim::Idle => IdleAnimation::update_skeleton(
            &base,
            // (active_tool, second_tool, hands, global_time)
            (active_tool, second_tool, hands, time),
            time,
            &mut rate,
            &attr,
        ),
        HumAnim::Run => {
            // Feed the run animation a forward-moving velocity of the right
            // magnitude so the stride matches the entity's speed. Direction is
            // "forward" in the figure's own frame (the entity Transform already
            // rotates the whole figure to face its heading), so a +y velocity
            // (sim forward) is a good generic input.
            let speed = ground_speed.max(0.5);
            let vel = Vec3::new(0.0, speed, 0.0);
            let ori = Vec3::new(0.0, 1.0, 0.0);
            // acc_vel accumulates distance for the foot-cycle phase. Use the
            // caller-maintained `acc` (integrated `speed * dt`) so the cycle
            // stays continuous when the speed changes (polish minor a) rather
            // than the old `time * speed`, which jumps on any speed change.
            let acc_vel = acc;
            RunAnimation::update_skeleton(
                &base,
                (
                    active_tool,    // active_tool_kind (sheathes the main weapon)
                    second_tool,    // second_tool_kind (sheathes the off-hand)
                    hands,          // hands (drives the back-sheathe pose)
                    vel,            // velocity
                    ori,            // orientation
                    ori,            // last_ori
                    Vec3::unit_y(), // look_dir
                    time,           // global_time
                    vel,            // avg_vel
                    acc_vel,        // acc_vel
                    None,           // wall
                ),
                time,
                &mut rate,
                &attr,
            )
        },
        HumAnim::Glide => {
            // GlidingAnimation::Dependency = (velocity, orientation,
            // glider_orientation, global_time, acc_vel). Only `velocity`'s
            // MAGNITUDE feeds the visible pose (a speed-based lean); its
            // direction is irrelevant, so reuse the same forward-facing
            // convention as Run. `orientation`/`glider_orientation` are
            // identity — see `HumAnim::Glide`'s doc for why.
            let speed = ground_speed.max(0.0);
            let vel = Vec3::new(0.0, speed, 0.0);
            let identity = vek::Quaternion::<f32>::identity();
            GlidingAnimation::update_skeleton(
                &base,
                (vel, identity, identity, time, acc),
                time,
                &mut rate,
                &attr,
            )
        },
    };

    let mut buf = [xindeler_anim::FigureBoneData::default(); xindeler_anim::MAX_BONE_COUNT];
    let computed = skeleton.compute_matrices(vek::Mat4::identity(), &mut buf, *body);

    // With `squash = 1.0` the character skeleton is authored z-up exactly like
    // the quadruped one, so the SHARED `mat_to_transform` (basis `C: x,y,z→
    // x,z,−y`) stands it upright in Bevy (y-up) — no extra rotation needed, and
    // bones stay consistent with the identically-`C`-converted vertices.
    //
    // The glider bone is special-cased (EM-3.8e): `xindeler-anim`'s `Idle`/
    // `Run` (ported UNCHANGED from voxygen) bake its LOCAL scale to zero, so
    // `computed.glider`'s linear part is a singular (non-invertible) matrix —
    // decomposing that back into a `Transform` (`mat_to_transform` → glam's
    // `Mat4::to_scale_rotation_translation`, which divides by axis length)
    // yields a NaN rotation, even though the (harmless) scale comes out as
    // zero. We only decompose `computed.glider` when we KNOW the anim gave it
    // a real (non-singular) scale (`Glide`); otherwise we build a safe,
    // explicit zero-scale `Transform` directly, never running the risky
    // decomposition. This is belt-and-braces: it happens to also be exactly
    // the behaviour voxygen gets "for free" by never decomposing its bone
    // matrices at all.
    let glider = match anim {
        HumAnim::Glide => mat_to_transform(computed.glider),
        HumAnim::Idle | HumAnim::Run => Transform::default().with_scale(bevy::math::Vec3::ZERO),
    };
    HumBoneTransforms {
        head: mat_to_transform(computed.head),
        chest: mat_to_transform(computed.chest),
        belt: mat_to_transform(computed.belt),
        shorts: mat_to_transform(computed.shorts),
        hand_l: mat_to_transform(computed.hand_l),
        hand_r: mat_to_transform(computed.hand_r),
        foot_l: mat_to_transform(computed.foot_l),
        foot_r: mat_to_transform(computed.foot_r),
        shoulder_l: mat_to_transform(computed.shoulder_l),
        shoulder_r: mat_to_transform(computed.shoulder_r),
        main: mat_to_transform(computed.main),
        second: mat_to_transform(computed.second),
        back: mat_to_transform(computed.back),
        lantern: mat_to_transform(computed.lantern),
        glider,
    }
}

/// The static rest pose (EM-3.8-style): idle at `time = 0`, deterministic
/// (`sin(0) = 0`), with the given equipped `tools` (EM-3.8d). Convenience
/// wrapper over [`humanoid_bone_transforms`].
#[must_use]
pub fn humanoid_bone_rest(body: &Body, tools: FigureToolKinds) -> HumBoneTransforms {
    humanoid_bone_transforms(body, HumAnim::Idle, 0.0, 0.0, 0.0, tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default (Human male, no cosmetics) test body.
    fn test_body() -> Body {
        Body {
            species: Species::Human,
            body_type: BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
            height_scale: 0,
        }
    }

    /// A 2-handed sword tool kinds set (what the starter Warrior carries).
    fn sword_tools() -> FigureToolKinds {
        FigureToolKinds {
            active: Some(ToolKind::Sword),
            second: None,
            hands: (Some(Hands::Two), None),
        }
    }

    /// The rest pose is deterministic + sane: a standing human's head is above
    /// its feet, all bones finite, model-scaled (within a couple metres of the
    /// root). Pure `xindeler-anim` — no assets.
    #[test]
    fn human_rest_pose_is_sane() {
        let body = test_body();
        let rest = humanoid_bone_rest(&body, FigureToolKinds::default());
        for (name, t) in [
            ("head", rest.head),
            ("chest", rest.chest),
            ("foot_l", rest.foot_l),
            ("hand_r", rest.hand_r),
        ] {
            assert!(
                t.translation.is_finite(),
                "{name} rest translation must be finite: {:?}",
                t.translation
            );
        }
        assert!(
            rest.head.translation.y > rest.foot_l.translation.y,
            "head above feet: head.y={} foot.y={}",
            rest.head.translation.y,
            rest.foot_l.translation.y
        );
        assert!(
            rest.head.translation.length() < 4.0,
            "model scale applied: head near root, got {:?}",
            rest.head.translation
        );
    }

    /// Idle and run at the same time produce DIFFERENT poses (the animation
    /// actually does something), and run advances with time.
    #[test]
    fn run_differs_from_idle_and_advances() {
        let body = test_body();
        let t = sword_tools();
        let idle = humanoid_bone_transforms(&body, HumAnim::Idle, 0.0, 1.0, 0.0, t);
        let run_a = humanoid_bone_transforms(&body, HumAnim::Run, 4.0, 1.0, 4.0, t);
        // Advance the accumulator (not just `time`) to prove acc drives phase.
        let run_b = humanoid_bone_transforms(&body, HumAnim::Run, 5.2, 1.3, 4.0, t);
        // A running foot is placed differently than an idle one.
        assert_ne!(
            idle.foot_l.translation, run_a.foot_l.translation,
            "run should move the feet vs idle"
        );
        // The run cycle advances between two times.
        assert_ne!(
            run_a.foot_l.translation, run_b.foot_l.translation,
            "run cycle should advance with time"
        );
    }

    /// EM-3.8d: a weapon `.vox` meshes to a real (non-empty) `bevy::Mesh`, and
    /// the `main` bone — fed the REAL equipped tool kind/hands (Sword/Two) —
    /// sheathes it on the back (not at the raw skeleton default at the hand). A
    /// tiny synthetic `.vox` stands in for the real sword so no assets are
    /// needed.
    #[test]
    fn main_weapon_meshes_on_the_main_bone() {
        use super::super::LoadedPart;

        /// The starter sword's manifest offset (`biped_weapon_manifest.ron`).
        const STARTER_SWORD_OFFSET: [f32; 3] = [-2.5, -4.0, -4.0];

        // Reuse the parent module's 1-voxel `.vox` helper via a local build.
        let mut palette = vec![
            dot_vox::Color {
                r: 0,
                g: 0,
                b: 0,
                a: 0
            };
            256
        ];
        palette[1] = dot_vox::Color {
            r: 180,
            g: 180,
            b: 200,
            a: 255,
        };
        let vox = DotVoxData {
            version: 150,
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![dot_vox::Voxel {
                    x: 0,
                    y: 0,
                    z: 0,
                    i: 1,
                }],
            }],
            palette,
            materials: vec![],
            scenes: vec![],
            layers: vec![],
            index_map: Vec::new(),
        };
        // The weapon is a plain-colour segment (no recolour) placed at its
        // offset — exactly what `assemble_humanoid` does for a `Weapon` role.
        let part = LoadedPart {
            vox: &vox,
            model_index: 0,
            offset: Vec3::from(STARTER_SWORD_OFFSET),
            flipped: false,
            bone: super::super::FigureBoneName::Chest, // unused for direct mesh
        };
        let mesh = super::super::figure_part_to_bevy(&part).expect("weapon voxel meshes");
        assert_eq!(mesh.count_vertices(), 24, "a cube is 6 quads × 4 verts");

        // And the `main` bone transform exists + is finite in a full pose.
        let body = test_body();
        let bones = humanoid_bone_rest(&body, sword_tools());
        let main = bones.get(HumBone::Main).translation;
        assert!(
            main.is_finite(),
            "the main-weapon bone transform must be finite"
        );
        // EM-3.8c/d: the 2H sword must be SHEATHED ON THE BACK, not stuck
        // at the raw skeleton default. `do_tools_on_back` (fed the REAL
        // `ToolKind::Sword` + `Hands::Two`) moves `main` well BEHIND the chest
        // (negative sim-y = Bevy +z, i.e. behind), high up the back — clearly
        // separated from the hand. Assert it moved off the chest centre so the
        // "flat slab at the origin" regression can't come back silently.
        let chest = bones.get(HumBone::Chest).translation;
        assert!(
            (main - chest).length() > 0.05,
            "the sheathed weapon must be offset from the chest (on the back), got main={main:?} \
             chest={chest:?}"
        );
        // On the back = behind the chest in Bevy z (+z is behind, since sim −y →
        // Bevy +z) and above the belt.
        assert!(
            main.z > chest.z,
            "the sheathed 2H sword sits BEHIND the chest (on the back), got main.z={} chest.z={}",
            main.z,
            main.z,
        );
    }

    /// EM-3.8c minor (a): the run foot cycle is driven by the `acc`
    /// accumulator, NOT the wall clock — advancing `acc` while holding `time`
    /// fixed still moves the feet. This is what keeps the cycle continuous when
    /// the speed (and thus `d(acc)/dt`) changes.
    #[test]
    fn run_phase_follows_acc_not_time() {
        let body = test_body();
        let t = sword_tools();
        // Same `time`, different `acc` → different foot placement.
        let a = humanoid_bone_transforms(&body, HumAnim::Run, 2.0, 1.0, 4.0, t);
        let b = humanoid_bone_transforms(&body, HumAnim::Run, 6.0, 1.0, 4.0, t);
        assert_ne!(
            a.foot_l.translation, b.foot_l.translation,
            "acc must drive the foot cycle independently of the wall clock"
        );
    }

    /// EM-3.8e (pure, no assets): the glider bone is INVISIBLE (zero scale,
    /// finite) in every anim state except [`HumAnim::Glide`], where it's
    /// visible (scale ≈ one) — the core "no floating glider on a standing
    /// figure" invariant this task explicitly must not violate. Also proves
    /// the zero-scale `Transform` never leaks NaN (the risk flagged in
    /// [`humanoid_bone_transforms`]'s doc comment: decomposing the anim's own
    /// singular zero-scale matrix would).
    #[test]
    fn glider_scale_is_zero_outside_glide_and_one_in_glide() {
        let body = test_body();
        let t = sword_tools();
        for anim in [HumAnim::Idle, HumAnim::Run] {
            let bones = humanoid_bone_transforms(&body, anim, 1.0, 1.0, 3.0, t);
            assert_eq!(
                bones.glider.scale,
                bevy::math::Vec3::ZERO,
                "{anim:?}: the glider must be zero-scaled (invisible)"
            );
            assert!(
                bones.glider.translation.is_finite() && bones.glider.rotation.is_finite(),
                "{anim:?}: the glider transform must stay finite even at zero scale, got {:?}",
                bones.glider
            );
        }
        let gliding = humanoid_bone_transforms(&body, HumAnim::Glide, 1.0, 1.0, 3.0, t);
        // The glider's LOCAL scale is 1.0 (voxygen `next.glider.scale =
        // Vec3::one()`), but every bone matrix also carries the whole
        // figure's per-body MODEL scale (chest included), so the absolute
        // scale isn't exactly one — compare against another always-visible
        // bone (`chest`) instead of a literal `Vec3::ONE`.
        assert!(
            (gliding.glider.scale - gliding.chest.scale).length() < 1e-3,
            "Glide: the glider must be visible at the same model scale as the rest of the figure, \
             got glider={:?} chest={:?}",
            gliding.glider.scale,
            gliding.chest.scale
        );
        assert!(
            gliding.glider.translation.is_finite() && gliding.glider.rotation.is_finite(),
            "Glide: the glider transform must be finite, got {:?}",
            gliding.glider
        );
    }

    /// EM-3.8e (pure, no assets): the head-armour manifest resolves a helmet
    /// keyed on `(species, body_type, item)` — unlike every other slot, an
    /// unequipped OR unknown head resolves to `None` (no generic "bare
    /// helmet" fallback; voxygen's `load_head` behaviour).
    #[test]
    fn head_armor_manifest_resolves_species_body_and_item() {
        let ron = r#"((
            default: ( vox_spec: ("armor.empty", (0.0, 0.0, 0.0)), color: None ),
            map: {
                (Human, Male, "common.items.armor.mail.bronze.head"): (
                    vox_spec: ("armor.mail.bronze.head", (-12.0, -11.0, 18.0)),
                    color: None
                ),
            },
        ))"#;
        let head_armor: HumArmorHeadSpec = ron::de::from_str(ron).expect("head manifest parses");
        assert_eq!(
            head_armor
                .resolve(
                    Species::Human,
                    BodyType::Male,
                    Some("common.items.armor.mail.bronze.head")
                )
                .map(|s| s.vox_spec.0.as_str()),
            Some("armor.mail.bronze.head"),
            "an equipped, manifest-known helmet resolves"
        );
        assert!(
            head_armor
                .resolve(Species::Human, BodyType::Male, None)
                .is_none(),
            "no helmet equipped -> no extra head mesh (not a generic default)"
        );
        assert!(
            head_armor
                .resolve(
                    Species::Human,
                    BodyType::Female,
                    Some("common.items.armor.mail.bronze.head")
                )
                .is_none(),
            "the manifest is species/body_type-specific: a Female entry doesn't exist here"
        );
        assert!(
            head_armor
                .resolve(Species::Human, BodyType::Male, Some("nope"))
                .is_none(),
            "an unknown helmet id resolves to nothing (caller drops the mesh)"
        );
    }

    /// EM-3.8d (pure, no assets): the armour manifest parses `default` + the
    /// per-item `map`, and `resolve` picks the equipped item's spec, falling
    /// back to `default` for empty/unknown keys. Locks the map lookup that lets
    /// real equipped gear select its `.vox`.
    #[test]
    fn armor_manifest_resolves_equipped_item() {
        let ron = r#"((
            default: ( vox_spec: ("armor.misc.chest.none", (-7.0, -3.5, 2.0)), color: None ),
            map: {
                "common.items.armor.misc.chest.worker_purple_brown": (
                    vox_spec: ("armor.misc.chest.worker_purp_brown", (-7.0, -3.5, 2.0)),
                    color: None
                ),
            },
        ))"#;
        let chest: HumArmorChestSpec = ron::de::from_str(ron).expect("chest manifest parses");
        // Empty slot → default naked-torso model.
        assert_eq!(chest.resolve(None).vox_spec.0, "armor.misc.chest.none");
        // Equipped item → its variant .vox.
        assert_eq!(
            chest
                .resolve(Some("common.items.armor.misc.chest.worker_purple_brown"))
                .vox_spec
                .0,
            "armor.misc.chest.worker_purp_brown"
        );
        // Unknown item → default fallback (voxygen `not_found` behaviour).
        assert_eq!(
            chest.resolve(Some("nope")).vox_spec.0,
            "armor.misc.chest.none"
        );
    }

    /// EM-3.8d (pure, no assets): the weapon manifest parses its
    /// `ToolKey`-shaped keys (`Tool("…")` / `Modular((…))`) and looks a
    /// tool up by its [`WeaponKey`]. Locks the weapon `.vox` selection from
    /// the equipped tool.
    #[test]
    fn weapon_manifest_resolves_tool_key() {
        let ron = r#"({
            Tool("common.items.weapons.sword.starter"): (
                vox_spec: ("weapon.sword.starter", (-2.5, -4.0, -4.0)), color: None
            ),
            Modular(("common.items.modular.weapon.primary.sword.longsword", "common.items.mineral.ingot.bronze", Two)): (
                vox_spec: ("weapon.sword.longsword.bronze-2h", (-1.5, -3.5, -5.0)), color: None
            ),
        })"#;
        let weapons: HumMainWeaponSpec = ron::de::from_str(ron).expect("weapon manifest parses");
        let starter = weapons
            .get(&WeaponKey::Tool(
                "common.items.weapons.sword.starter".to_owned(),
            ))
            .expect("starter sword resolves");
        assert_eq!(starter.vox_spec.0, "weapon.sword.starter");
        let modular = weapons
            .get(&WeaponKey::Modular((
                "common.items.modular.weapon.primary.sword.longsword".to_owned(),
                "common.items.mineral.ingot.bronze".to_owned(),
                Hands::Two,
            )))
            .expect("modular longsword resolves");
        assert_eq!(modular.vox_spec.0, "weapon.sword.longsword.bronze-2h");
        // An unequipped/unknown key resolves to nothing (caller drops the mesh).
        assert!(
            weapons
                .get(&WeaponKey::Tool("missing".to_owned()))
                .is_none()
        );
    }

    /// END-TO-END with the REAL frozen manifests + `.vox` assets: parse the
    /// humanoid manifests, resolve + load every part's `.vox`, assemble a
    /// default Human male → assert we get a full set of non-empty parts
    /// (head + chest + belt + pants + 2 hands + 2 feet + 2 shoulders) with
    /// vertices. Needs the asset tree; run locally with `XINDELER_ASSETS`/
    /// `VELOREN_ASSETS`.
    #[test]
    #[ignore = "reads the real humanoid manifests + .vox assets: needs the asset tree"]
    fn real_human_assembles() {
        use dot_vox::load_bytes;

        let root = std::env::var("XINDELER_ASSETS")
            .or_else(|_| std::env::var("VELOREN_ASSETS"))
            .expect("set XINDELER_ASSETS or VELOREN_ASSETS to the assets dir");
        let read_ron = |dotted: &str| -> String {
            let path = format!("{root}/{}.ron", dotted.replace('.', "/"));
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
        };
        let color: HumColorSpec =
            ron::de::from_str(&read_ron(HUM_COLOR_MANIFEST)).expect("color manifest");
        let head: HumHeadSpec =
            ron::de::from_str(&read_ron(HUM_HEAD_MANIFEST)).expect("head manifest");
        let chest: HumArmorChestSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_CHEST_MANIFEST)).expect("chest manifest");
        let belt: HumArmorBeltSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_BELT_MANIFEST)).expect("belt manifest");
        let pants: HumArmorPantsSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_PANTS_MANIFEST)).expect("pants manifest");
        let foot: HumArmorFootSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_FOOT_MANIFEST)).expect("foot manifest");
        let hand: HumArmorHandSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_HAND_MANIFEST)).expect("hand manifest");
        let shoulder: HumArmorShoulderSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_SHOULDER_MANIFEST)).expect("shoulder manifest");
        let back: HumArmorBackSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_BACK_MANIFEST)).expect("back manifest");
        let main_weapon: HumMainWeaponSpec =
            ron::de::from_str(&read_ron(HUM_MAIN_WEAPON_MANIFEST)).expect("weapon manifest");
        let lantern: HumLanternSpec =
            ron::de::from_str(&read_ron(HUM_LANTERN_MANIFEST)).expect("lantern manifest");
        // EM-3.8e: the helmet + glider manifests.
        let armor_head: HumArmorHeadSpec =
            ron::de::from_str(&read_ron(HUM_ARMOR_HEAD_MANIFEST)).expect("head-armor manifest");
        let glider: HumGliderSpec =
            ron::de::from_str(&read_ron(HUM_GLIDER_MANIFEST)).expect("glider manifest");
        let manifests = HumManifests {
            color: &color,
            head: &head,
            chest: &chest,
            belt: &belt,
            pants: &pants,
            foot: &foot,
            hand: &hand,
            shoulder: &shoulder,
            back: &back,
            main_weapon: &main_weapon,
            lantern: &lantern,
            armor_head: &armor_head,
            glider: &glider,
        };
        let body = test_body();
        // EM-3.8d: the embedded Warrior's real starter kit — a starter sword +
        // worker chest/pants/sandals + a lantern. This exercises the REAL gear
        // resolution (per-item manifest map), not just the defaults.
        // EM-3.8e adds a bronze mail head cap (the Warrior's starter armour
        // set — see `warrior.ron`) + a basic glider, to exercise the new
        // helmet/glider resolution end-to-end with real assets.
        let loadout = FigureLoadout {
            active_tool: Some(FigureTool {
                key: WeaponKey::Tool("common.items.weapons.sword.starter".to_owned()),
                kind: ToolKind::Sword,
                hands: Hands::Two,
            }),
            chest: Some("common.items.armor.misc.chest.worker_purple_brown".to_owned()),
            pants: Some("common.items.armor.misc.pants.worker_brown".to_owned()),
            foot: Some("common.items.armor.misc.foot.sandals".to_owned()),
            lantern: Some("common.items.lantern.black_0".to_owned()),
            head: Some("common.items.armor.mail.bronze.head".to_owned()),
            glider: Some("common.items.glider.basic_white".to_owned()),
            ..Default::default()
        };
        let refs = humanoid_vox_refs(&manifests, &body, &loadout)
            .expect("human has a head-manifest entry");
        // The helmet resolves to a real (species, body_type)-keyed `.vox`,
        // unioned into the head (not its own bone).
        assert!(
            refs.iter()
                .any(|r| matches!(r.role, HumVoxRole::HeadArmor { .. })),
            "the equipped bronze-mail head cap must resolve to a HeadArmor ref, got: {:?}",
            refs.iter().map(|r| &r.vox_name).collect::<Vec<_>>()
        );
        // The glider resolves onto its own bone.
        assert!(
            refs.iter().any(|r| matches!(r.role, HumVoxRole::Body {
                bone: HumBone::Glider,
                ..
            })),
            "the equipped glider must resolve onto the glider bone, got: {:?}",
            refs.iter().map(|r| &r.vox_name).collect::<Vec<_>>()
        );
        // The resolved chest must be the EQUIPPED worker chest .vox, not the
        // default naked-torso model — proving real gear feeds the assembly.
        assert!(
            refs.iter()
                .any(|r| r.vox_name == "armor.misc.chest.worker_purp_brown"),
            "the equipped worker chest .vox must be selected, got: {:?}",
            refs.iter().map(|r| &r.vox_name).collect::<Vec<_>>()
        );
        // And the starter sword .vox rides the `main` weapon bone.
        assert!(
            refs.iter().any(|r| matches!(r.role, HumVoxRole::Weapon {
                bone: HumBone::Main,
                ..
            }) && r.vox_name == "weapon.sword.starter"),
            "the equipped starter sword must be on the main bone"
        );
        // Load every `.vox` (relative to voxygen.voxel).
        let voxes: Vec<DotVoxData> = refs
            .iter()
            .map(|r| {
                let path = format!(
                    "{root}/{}/{}.vox",
                    VOX_NAMESPACE.replace('.', "/"),
                    r.vox_name.replace('.', "/")
                );
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
                load_bytes(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"))
            })
            .collect();
        let loaded: Vec<LoadedHumPart> = refs
            .iter()
            .zip(&voxes)
            .map(|(r, vox)| LoadedHumPart {
                role: r.role.clone(),
                vox,
                model_index: r.model_index,
            })
            .collect();
        let assembled = assemble_humanoid(&body, &manifests, &loaded);
        // Head + chest + belt + shorts(pants) + 2 hands + 2 feet + 2 shoulders.
        assert!(
            assembled.len() >= 8,
            "expected a full humanoid part set, got {} parts: {:?}",
            assembled.len(),
            assembled.iter().map(|p| p.name).collect::<Vec<_>>()
        );
        for p in &assembled {
            assert!(
                p.mesh.count_vertices() > 0,
                "part {} should have vertices",
                p.name
            );
        }
        // The head part must be present (skin/hair recolour path exercised).
        assert!(
            assembled.iter().any(|p| p.bone == HumBone::Head),
            "head part assembled"
        );
        // The glider bone must have assembled a real, non-empty mesh too.
        assert!(
            assembled.iter().any(|p| p.bone == HumBone::Glider),
            "glider part assembled"
        );
        eprintln!(
            "assembled {} humanoid parts: {:?}",
            assembled.len(),
            assembled
                .iter()
                .map(|p| (p.name, p.mesh.count_vertices()))
                .collect::<Vec<_>>()
        );

        // EM-3.8e: prove the helmet actually contributes geometry to the head
        // union (not just resolved-but-silently-dropped) by re-assembling the
        // SAME head with `head: None` and asserting the bare head has FEWER
        // vertices than the helmeted one.
        let bare_loadout = FigureLoadout {
            head: None,
            ..loadout
        };
        let bare_refs = humanoid_vox_refs(&manifests, &body, &bare_loadout)
            .expect("human has a head-manifest entry");
        assert!(
            !bare_refs
                .iter()
                .any(|r| matches!(r.role, HumVoxRole::HeadArmor { .. })),
            "an unequipped head must not resolve a HeadArmor ref"
        );
        let bare_voxes: Vec<DotVoxData> = bare_refs
            .iter()
            .map(|r| {
                let path = format!(
                    "{root}/{}/{}.vox",
                    VOX_NAMESPACE.replace('.', "/"),
                    r.vox_name.replace('.', "/")
                );
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
                load_bytes(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"))
            })
            .collect();
        let bare_loaded: Vec<LoadedHumPart> = bare_refs
            .iter()
            .zip(&bare_voxes)
            .map(|(r, vox)| LoadedHumPart {
                role: r.role.clone(),
                vox,
                model_index: r.model_index,
            })
            .collect();
        let bare_assembled = assemble_humanoid(&body, &manifests, &bare_loaded);
        let helmeted_head_verts = assembled
            .iter()
            .find(|p| p.bone == HumBone::Head)
            .expect("helmeted head assembled")
            .mesh
            .count_vertices();
        let bare_head_verts = bare_assembled
            .iter()
            .find(|p| p.bone == HumBone::Head)
            .expect("bare head assembled")
            .mesh
            .count_vertices();
        assert!(
            helmeted_head_verts > bare_head_verts,
            "the helmet must add geometry to the head union: helmeted={helmeted_head_verts} \
             bare={bare_head_verts}"
        );
    }
}
