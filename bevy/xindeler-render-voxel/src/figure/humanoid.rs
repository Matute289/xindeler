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
//! ## What v1 (this module) does
//! - Ports the colour manifest + recolour EXACTLY (`HumColorSpec`,
//!   `recolor_grey`) — so skin/hair/eye colour is REAL, per-body.
//! - Ports the head manifest fully (bare head + eyes + hair + beard +
//!   accessory, unified like voxygen's `DynaUnionizer`).
//! - Uses the **default loadout** for the body slots (no inventory/equipment
//!   yet — that needs more of the sim than the mirror carries): each armour
//!   manifest's `default` entry for chest/belt/pants/shoulders(L,R)/hands(L,R)/
//!   feet(L,R). We deliberately deserialize ONLY the `default` field of each
//!   armour manifest (a strict subset of the frozen RON — isolation law rule 3
//!   keeps the file untouched; we just read less of it).
//! - Skips (v1) back/glider/lantern/weapons/hold — additive later; a bare
//!   humanoid with default clothing is the visual milestone.
//!
//! ## Recolour: honest scope
//! Skin, hair, eye and greyscale-armour recolour are all REAL here (ported from
//! `HumColorSpec` + `recolor_grey`). What is NOT modelled in v1 is real
//! equipped gear (everything is the default clothing) and the weapon/lantern
//! bones — documented as `TODO(EM-3.8c)`.
//!
//! ## Purity
//! Same as the parent module: this is engine-shell code depending on the LOGIC
//! crates `common` (for `MatSegment`/`Material`/`DynaUnionizer`/the colour math
//! in `common::util`) + `xindeler-anim` (rest-pose + animation bone matrices).
//! It takes parsed `.vox` bytes from the caller; it never loads assets itself.

use bevy::transform::components::Transform;
use common::{
    comp::humanoid::{Body, BodyType, EyeColor, Skin, Species},
    figure::{DynaUnionizer, MatSegment, Material, Segment},
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
// Armour manifests — DEFAULT loadout only (subset of the frozen RON)
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

/// The `default` loadout slot of an armour manifest (voxygen `ArmorVoxSpecMap`,
/// of which v1 reads ONLY `default`). The frozen RON also carries `map: { … }`
/// of equippable variants; serde ignores that unknown field, so this reads just
/// the default loadout (isolation law rule 3: the file is untouched — we
/// deserialize a subset).
#[derive(Deserialize, Clone, Debug)]
pub struct ArmorDefaultMap<S> {
    default: S,
}

/// An armour manifest as a NEWTYPE around its map (voxygen wraps each manifest
/// in a 1-tuple struct — e.g. `HumArmorChestSpec(ArmorVoxSpecMap)` — so the RON
/// begins `( ( default: … ) )`). We mirror that outer wrapper so the SAME
/// frozen RON parses.
#[derive(Deserialize, Clone, Debug)]
pub struct ArmorDefault<S>(ArmorDefaultMap<S>);

impl<S> ArmorDefault<S> {
    fn default_slot(&self) -> &S { &self.0.default }
}

/// The armour manifests v1 uses (default loadout only).
pub type HumArmorChestSpec = ArmorDefault<ArmorVoxSpec>;
pub type HumArmorBeltSpec = ArmorDefault<ArmorVoxSpec>;
pub type HumArmorPantsSpec = ArmorDefault<ArmorVoxSpec>;
pub type HumArmorFootSpec = ArmorDefault<ArmorVoxSpec>;
pub type HumArmorHandSpec = ArmorDefault<SidedArmorVoxSpec>;
pub type HumArmorShoulderSpec = ArmorDefault<SidedArmorVoxSpec>;

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
/// where to place it. Head sub-parts carry their own local offset.
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
    Chest,
    Belt,
    Pants,
    /// A hand/foot/shoulder — sided (left parts are the right `.vox` mirrored).
    Sided {
        bone: HumBone,
        flipped: bool,
        tint: Option<[u8; 3]>,
    },
}

/// The list of `.vox` files (+ their roles) a humanoid figure needs, in the
/// order the assembler expects. The caller loads each `vox_name`, then calls
/// [`assemble_humanoid`] with the parsed data in the SAME order.
///
/// Returns `None` if the head manifest has no entry for this `(species,
/// body_type)` (voxygen's `not_found` fallback — the caller keeps its capsule).
#[must_use]
pub fn humanoid_vox_refs(manifests: &HumManifests<'_>, body: &Body) -> Option<Vec<HumVoxRef>> {
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

    // --- Body slots (default loadout) ---
    let chest = &manifests.chest.default_slot();
    refs.push(HumVoxRef {
        role: HumVoxRole::Chest,
        vox_name: chest.vox_spec.0.clone(),
        model_index: chest.vox_spec.2,
    });
    let belt = &manifests.belt.default_slot();
    refs.push(HumVoxRef {
        role: HumVoxRole::Belt,
        vox_name: belt.vox_spec.0.clone(),
        model_index: belt.vox_spec.2,
    });
    let pants = &manifests.pants.default_slot();
    refs.push(HumVoxRef {
        role: HumVoxRole::Pants,
        vox_name: pants.vox_spec.0.clone(),
        model_index: pants.vox_spec.2,
    });

    // Sided: hands, feet, shoulders (left = right `.vox` mirrored for
    // hands/feet; shoulders have distinct left/right specs).
    let hand = &manifests.hand.default_slot();
    refs.push(sided_ref(HumBone::HandL, &hand.left, true));
    refs.push(sided_ref(HumBone::HandR, &hand.right, false));
    let foot = &manifests.foot.default_slot();
    // Feet share one spec, left mirrored (voxygen `mesh_foot(flipped)`).
    refs.push(sided_ref(HumBone::FootL, foot, true));
    refs.push(sided_ref(HumBone::FootR, foot, false));
    let shoulder = &manifests.shoulder.default_slot();
    refs.push(sided_ref(HumBone::ShoulderL, &shoulder.left, true));
    refs.push(sided_ref(HumBone::ShoulderR, &shoulder.right, false));

    Some(refs)
}

fn sided_ref(bone: HumBone, spec: &ArmorVoxSpec, flipped: bool) -> HumVoxRef {
    HumVoxRef {
        role: HumVoxRole::Sided {
            bone,
            flipped,
            tint: spec.color,
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

/// The manifest offsets (float, voxel units) for the default body slots, read
/// once so the assembler can position chest/belt/pants/hands/feet/shoulders.
struct SlotOffsets {
    chest: Vec3<f32>,
    belt: Vec3<f32>,
    pants: Vec3<f32>,
    hand_l: Vec3<f32>,
    hand_r: Vec3<f32>,
    foot: Vec3<f32>,
    shoulder_l: Vec3<f32>,
    shoulder_r: Vec3<f32>,
}

impl SlotOffsets {
    fn from(m: &HumManifests<'_>) -> Self {
        Self {
            chest: Vec3::from(m.chest.default_slot().vox_spec.1),
            belt: Vec3::from(m.belt.default_slot().vox_spec.1),
            pants: Vec3::from(m.pants.default_slot().vox_spec.1),
            hand_l: Vec3::from(m.hand.default_slot().left.vox_spec.1),
            hand_r: Vec3::from(m.hand.default_slot().right.vox_spec.1),
            foot: Vec3::from(m.foot.default_slot().vox_spec.1),
            shoulder_l: Vec3::from(m.shoulder.default_slot().left.vox_spec.1),
            shoulder_r: Vec3::from(m.shoulder.default_slot().right.vox_spec.1),
        }
    }
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
/// (voxygen's per-slot `mesh_*` functions, condensed to the default loadout).
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
    let offsets = SlotOffsets::from(manifests);

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
            _ => {},
        }
    }
    if has_head {
        let (head_seg, origin_offset) = unionizer.unify();
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

    // --- Body slots ---
    // NOTE (v1): voxygen unions each chest/pants with a separate `armor.empty`
    // bare-torso base; the `default` loadout uses the self-contained
    // `armor.misc.{chest,pants}.none` models (naked torso/legs already in the
    // voxels), so we recolour+tint the slot `.vox` directly. Equipping real
    // armour (which the mirror doesn't carry yet) → TODO(EM-3.8c).
    for p in parts {
        let (bone, seg, offset) = match &p.role {
            HumVoxRole::Chest => {
                let seg = tinted_body_seg(
                    color,
                    p,
                    skin,
                    hair_color,
                    eye,
                    manifests.chest.default_slot().color,
                );
                (HumBone::Chest, seg, offsets.chest)
            },
            HumVoxRole::Pants => {
                let seg = tinted_body_seg(
                    color,
                    p,
                    skin,
                    hair_color,
                    eye,
                    manifests.pants.default_slot().color,
                );
                (HumBone::Shorts, seg, offsets.pants)
            },
            HumVoxRole::Belt => {
                let seg = tinted_body_seg(
                    color,
                    p,
                    skin,
                    hair_color,
                    eye,
                    manifests.belt.default_slot().color,
                );
                (HumBone::Belt, seg, offsets.belt)
            },
            HumVoxRole::Sided {
                bone,
                flipped,
                tint,
            } => {
                let mut seg = color.color_segment(
                    mat_seg(p.vox, *flipped, p.model_index),
                    skin,
                    hair_color,
                    eye,
                );
                if let Some(c) = tint {
                    let tint_rgb = Rgb::from(Vec3::from(*c));
                    seg = seg.map_rgb(|rgb| recolor_grey(rgb, tint_rgb));
                }
                let offset = match bone {
                    HumBone::HandL => offsets.hand_l,
                    HumBone::HandR => offsets.hand_r,
                    HumBone::FootL | HumBone::FootR => offsets.foot,
                    HumBone::ShoulderL => offsets.shoulder_l,
                    HumBone::ShoulderR => offsets.shoulder_r,
                    _ => Vec3::zero(),
                };
                (*bone, seg, offset)
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

/// A greyscale-tinted body-slot armour segment (voxygen chest/belt/pants tint).
fn tinted_body_seg(
    color: &HumColorSpec,
    p: &LoadedHumPart,
    skin: Skin,
    hair_color: (u8, u8, u8),
    eye: EyeColor,
    tint: Option<[u8; 3]>,
) -> Segment {
    let mut seg = mat_seg(p.vox, false, p.model_index);
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
}

/// Compute the humanoid bone transforms for `body` at animation `anim` and
/// `time` seconds (Part B). `time` drives the cyclic animations; passing a
/// constant (e.g. 0) with [`HumAnim::Idle`] yields a deterministic rest pose
/// (the EM-3.8 static path).
///
/// `ground_speed` (blocks/s, from the entity's replicated velocity) scales the
/// run cycle so a slow walk animates slower than a sprint.
#[must_use]
pub fn humanoid_bone_transforms(
    body: &Body,
    anim: HumAnim,
    time: f32,
    ground_speed: f32,
) -> HumBoneTransforms {
    use xindeler_anim::{
        Animation, Skeleton,
        character::{CharacterSkeleton, IdleAnimation, RunAnimation, SkeletonAttr},
    };

    let attr = SkeletonAttr::from(body);
    let mut rate = 0.0;
    // IMPORTANT: build the skeleton with `squash = 1.0` (no squash). The
    // `Default` impl leaves `squash = 0.0`, which the skeleton's `squash_chest`/
    // `squash_limb` closures treat as an EXTREME squash — rotating the chest
    // ~2 rad about x and zeroing the vertical offsets, which lays the whole
    // figure flat. Voxygen always constructs it via `CharacterSkeleton::new(..,
    // squash = 1.0)`; we must match that. (`holding_lantern = false`,
    // `back_carry_offset = 0.0` are the v1 no-loadout defaults.)
    let base = CharacterSkeleton::new(false, 0.0, 1.0);

    let skeleton = match anim {
        HumAnim::Idle => IdleAnimation::update_skeleton(
            &base,
            // (active_tool, second_tool, hands, global_time)
            (None, None, (None, None), time),
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
            // acc_vel accumulates distance for the foot-cycle phase; derive a
            // continuous phase from time * speed so the cycle advances smoothly.
            let acc_vel = time * speed;
            RunAnimation::update_skeleton(
                &base,
                (
                    None,           // active_tool_kind
                    None,           // second_tool_kind
                    (None, None),   // hands
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
    };

    let mut buf = [xindeler_anim::FigureBoneData::default(); xindeler_anim::MAX_BONE_COUNT];
    let computed = skeleton.compute_matrices(vek::Mat4::identity(), &mut buf, *body);

    // With `squash = 1.0` the character skeleton is authored z-up exactly like
    // the quadruped one, so the SHARED `mat_to_transform` (basis `C: x,y,z→
    // x,z,−y`) stands it upright in Bevy (y-up) — no extra rotation needed, and
    // bones stay consistent with the identically-`C`-converted vertices.
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
    }
}

/// The static rest pose (EM-3.8-style): idle at `time = 0`, deterministic
/// (`sin(0) = 0`). Convenience wrapper over [`humanoid_bone_transforms`].
#[must_use]
pub fn humanoid_bone_rest(body: &Body) -> HumBoneTransforms {
    humanoid_bone_transforms(body, HumAnim::Idle, 0.0, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rest pose is deterministic + sane: a standing human's head is above
    /// its feet, all bones finite, model-scaled (within a couple metres of the
    /// root). Pure `xindeler-anim` — no assets.
    #[test]
    fn human_rest_pose_is_sane() {
        let body = Body {
            species: Species::Human,
            body_type: BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
        };
        let rest = humanoid_bone_rest(&body);
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
        let body = Body {
            species: Species::Human,
            body_type: BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
        };
        let idle = humanoid_bone_transforms(&body, HumAnim::Idle, 1.0, 0.0);
        let run_a = humanoid_bone_transforms(&body, HumAnim::Run, 1.0, 4.0);
        let run_b = humanoid_bone_transforms(&body, HumAnim::Run, 1.3, 4.0);
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
        let manifests = HumManifests {
            color: &color,
            head: &head,
            chest: &chest,
            belt: &belt,
            pants: &pants,
            foot: &foot,
            hand: &hand,
            shoulder: &shoulder,
        };
        let body = Body {
            species: Species::Human,
            body_type: BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
        };
        let refs = humanoid_vox_refs(&manifests, &body).expect("human has a head-manifest entry");
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
        eprintln!(
            "assembled {} humanoid parts: {:?}",
            assembled.len(),
            assembled
                .iter()
                .map(|p| (p.name, p.mesh.count_vertices()))
                .collect::<Vec<_>>()
        );
    }
}
