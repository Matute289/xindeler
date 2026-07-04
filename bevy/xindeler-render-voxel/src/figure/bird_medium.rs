//! EM-3.8c — the BIRD-MEDIUM figure path (owls, ducks, eagles, …).
//!
//! Additive body on the SAME machinery as the quadrupeds: a central manifest
//! (head/chest/tail) + a lateral manifest (two wings-in, two wings-out, two
//! legs), meshed by the shared figure mesher and placed at the
//! `xindeler-anim` `bird_medium` skeleton's bone matrices. Animated with idle
//! (perched), run (walking on the ground) or fly (airborne flap) picked from
//! the entity's replicated velocity.
//!
//! Ported from `voxygen/src/scene/figure/load.rs`
//! (`BirdMediumCentralSpec`/`BirdMediumLateralSpec`): the LEFT parts
//! (`wing_in_l`, `wing_out_l`, `leg_l`) reuse the right `.vox` mirrored
//! (`graceful_load_segment_flipped(.., true, ..)`); the right parts load
//! unflipped.
//!
//! ## Purity — same as the parent module.

use bevy::transform::components::Transform;
use serde::Deserialize;
use vek::*;

use super::{FigureAnim, FigurePart, LoadedPart, mat_to_transform};
use common::comp::bird_medium::{BodyType, Species};

/// The manifest ASSET PATHS (upstream names, frozen — isolation law rule 3).
pub const BM_CENTRAL_MANIFEST: &str = "voxygen.voxel.bird_medium_central_manifest";
pub const BM_LATERAL_MANIFEST: &str = "voxygen.voxel.bird_medium_lateral_manifest";

// ---------------------------------------------------------------------------
// Manifest deser
// ---------------------------------------------------------------------------

/// A `.vox` reference in a manifest: `("model.name")`.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct VoxSimple(pub String);

/// One central/lateral sub-part: offset + which `.vox` + model index.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BmSubSpec {
    pub offset: [f32; 3],
    #[serde(alias = "central", alias = "lateral")]
    pub model: VoxSimple,
    pub model_index: u32,
}

/// One `(species, body_type)` central-manifest entry.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BmCentralEntry {
    pub head: BmSubSpec,
    pub chest: BmSubSpec,
    pub tail: BmSubSpec,
}

/// One `(species, body_type)` lateral-manifest entry.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BmLateralEntry {
    pub wing_in_l: BmSubSpec,
    pub wing_in_r: BmSubSpec,
    pub wing_out_l: BmSubSpec,
    pub wing_out_r: BmSubSpec,
    pub leg_l: BmSubSpec,
    pub leg_r: BmSubSpec,
}

/// The whole bird-medium central manifest.
#[derive(Deserialize, Clone, Debug)]
pub struct BmCentralManifest(pub std::collections::HashMap<(Species, BodyType), BmCentralEntry>);

/// The whole bird-medium lateral manifest.
#[derive(Deserialize, Clone, Debug)]
pub struct BmLateralManifest(pub std::collections::HashMap<(Species, BodyType), BmLateralEntry>);

// ---------------------------------------------------------------------------
// Bones
// ---------------------------------------------------------------------------

/// The bones a bird-medium figure has (matches `ComputedBirdMediumSkeleton`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BmBone {
    Head,
    Chest,
    Tail,
    WingInL,
    WingInR,
    WingOutL,
    WingOutR,
    LegL,
    LegR,
}

fn bm_bone_label(bone: BmBone) -> &'static str {
    match bone {
        BmBone::Head => "head",
        BmBone::Chest => "chest",
        BmBone::Tail => "tail",
        BmBone::WingInL => "wing_in_l",
        BmBone::WingInR => "wing_in_r",
        BmBone::WingOutL => "wing_out_l",
        BmBone::WingOutR => "wing_out_r",
        BmBone::LegL => "leg_l",
        BmBone::LegR => "leg_r",
    }
}

/// A resolved reference to one bird part's manifest data.
pub struct BmPartSpecRef {
    pub bone: BmBone,
    pub vox_name: String,
    pub model_index: u32,
    pub offset: Vec3<f32>,
    pub flipped: bool,
}

impl BmPartSpecRef {
    fn new(bone: BmBone, spec: &BmSubSpec, flipped: bool) -> Self {
        Self {
            bone,
            vox_name: spec.model.0.clone(),
            model_index: spec.model_index,
            offset: Vec3::from(spec.offset),
            flipped,
        }
    }
}

/// The parts a bird-medium figure needs. The LEFT parts reuse the right `.vox`
/// mirrored. Returns `None` if either manifest lacks an entry.
#[must_use]
pub fn bird_medium_part_specs(
    central: &BmCentralManifest,
    lateral: &BmLateralManifest,
    species: Species,
    body_type: BodyType,
) -> Option<Vec<BmPartSpecRef>> {
    let c = central.0.get(&(species, body_type))?;
    let l = lateral.0.get(&(species, body_type))?;
    Some(vec![
        BmPartSpecRef::new(BmBone::Head, &c.head, false),
        BmPartSpecRef::new(BmBone::Chest, &c.chest, false),
        BmPartSpecRef::new(BmBone::Tail, &c.tail, false),
        BmPartSpecRef::new(BmBone::WingInL, &l.wing_in_l, true),
        BmPartSpecRef::new(BmBone::WingInR, &l.wing_in_r, false),
        BmPartSpecRef::new(BmBone::WingOutL, &l.wing_out_l, true),
        BmPartSpecRef::new(BmBone::WingOutR, &l.wing_out_r, false),
        BmPartSpecRef::new(BmBone::LegL, &l.leg_l, true),
        BmPartSpecRef::new(BmBone::LegR, &l.leg_r, false),
    ])
}

// ---------------------------------------------------------------------------
// Bone transforms
// ---------------------------------------------------------------------------

/// The full set of bird-medium bone transforms (Bevy space, model-scaled).
pub struct BmBoneTransforms {
    pub head: Transform,
    pub chest: Transform,
    pub tail: Transform,
    pub wing_in_l: Transform,
    pub wing_in_r: Transform,
    pub wing_out_l: Transform,
    pub wing_out_r: Transform,
    pub leg_l: Transform,
    pub leg_r: Transform,
}

impl BmBoneTransforms {
    #[must_use]
    pub fn get(&self, bone: BmBone) -> Transform {
        match bone {
            BmBone::Head => self.head,
            BmBone::Chest => self.chest,
            BmBone::Tail => self.tail,
            BmBone::WingInL => self.wing_in_l,
            BmBone::WingInR => self.wing_in_r,
            BmBone::WingOutL => self.wing_out_l,
            BmBone::WingOutR => self.wing_out_r,
            BmBone::LegL => self.leg_l,
            BmBone::LegR => self.leg_r,
        }
    }
}

/// Animated bird-medium bone transforms. `Idle` = perched, `Run` = walking on
/// the ground, `Fly` = airborne flap. `acc` drives the walk/flap phase (blocks
/// travelled), `time` the idle sway.
#[must_use]
pub fn bird_medium_bone_transforms(
    species: Species,
    body_type: BodyType,
    anim: FigureAnim,
    acc: f32,
    time: f32,
    ground_speed: f32,
) -> BmBoneTransforms {
    use xindeler_anim::{
        Animation, Skeleton,
        bird_medium::{
            BirdMediumSkeleton, FlyAnimation, IdleAnimation, RunAnimation, SkeletonAttr,
        },
    };

    let body = common::comp::bird_medium::Body { species, body_type };
    let attr = SkeletonAttr::from(&body);
    let base = BirdMediumSkeleton::default();
    let mut rate = 0.0;

    let speed = ground_speed.max(0.5);
    let ori = Vec3::new(0.0, 1.0, 0.0);
    let vel = Vec3::new(0.0, speed, 0.0);

    let skeleton = match anim {
        FigureAnim::Idle => IdleAnimation::update_skeleton(&base, time, time, &mut rate, &attr),
        FigureAnim::Run => {
            // Bird run dependency: `(velocity: Vec3, orientation, last_ori,
            // avg_vel, acc_vel)` — velocity is a Vec3 here (not f32).
            RunAnimation::update_skeleton(&base, (vel, ori, ori, vel, acc), time, &mut rate, &attr)
        },
        FigureAnim::Fly => {
            // Bird fly dependency: `(velocity, orientation, last_ori)`.
            FlyAnimation::update_skeleton(&base, (vel, ori, ori), time, &mut rate, &attr)
        },
    };

    let mut buf = [xindeler_anim::FigureBoneData::default(); xindeler_anim::MAX_BONE_COUNT];
    let computed = skeleton.compute_matrices(vek::Mat4::identity(), &mut buf, body);

    BmBoneTransforms {
        head: mat_to_transform(computed.head),
        chest: mat_to_transform(computed.chest),
        tail: mat_to_transform(computed.tail),
        wing_in_l: mat_to_transform(computed.wing_in_l),
        wing_in_r: mat_to_transform(computed.wing_in_r),
        wing_out_l: mat_to_transform(computed.wing_out_l),
        wing_out_r: mat_to_transform(computed.wing_out_r),
        leg_l: mat_to_transform(computed.leg_l),
        leg_r: mat_to_transform(computed.leg_r),
    }
}

/// The static rest pose (idle at `time = 0`).
#[must_use]
pub fn bird_medium_bone_rest(species: Species, body_type: BodyType) -> BmBoneTransforms {
    bird_medium_bone_transforms(species, body_type, FigureAnim::Idle, 0.0, 0.0, 0.0)
}

// ---------------------------------------------------------------------------
// Assemble
// ---------------------------------------------------------------------------

/// A loaded bird `.vox` paired back with its bone/offset/flip.
pub struct LoadedBmPart<'a> {
    pub vox: &'a dot_vox::DotVoxData,
    pub model_index: u32,
    pub offset: Vec3<f32>,
    pub flipped: bool,
    pub bone: BmBone,
}

/// Meshes each loaded part and pairs it with its bone's transform. Empty parts
/// dropped.
#[must_use]
pub fn assemble(parts: &[LoadedBmPart], bones: &BmBoneTransforms) -> Vec<(BmBone, FigurePart)> {
    parts
        .iter()
        .filter_map(|part| {
            let loaded = LoadedPart {
                vox: part.vox,
                model_index: part.model_index,
                offset: part.offset,
                flipped: part.flipped,
                bone: super::FigureBoneName::Chest, // unused: we mesh directly
            };
            let mesh = super::figure_part_to_bevy(&loaded)?;
            Some((part.bone, FigurePart {
                mesh,
                transform: bones.get(part.bone),
                name: bm_bone_label(part.bone),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rest pose is deterministic + sane: all bones finite, model-scaled,
    /// head above the legs. Pure `xindeler-anim` — no assets.
    #[test]
    fn bm_rest_pose_is_sane() {
        let rest = bird_medium_bone_rest(Species::SnowyOwl, BodyType::Male);
        for (name, t) in [
            ("head", rest.head),
            ("chest", rest.chest),
            ("wing_in_l", rest.wing_in_l),
            ("leg_l", rest.leg_l),
        ] {
            assert!(
                t.translation.is_finite(),
                "{name} rest translation must be finite: {:?}",
                t.translation
            );
        }
        assert!(
            rest.head.translation.y > rest.leg_l.translation.y,
            "head above legs: head.y={} leg.y={}",
            rest.head.translation.y,
            rest.leg_l.translation.y
        );
        assert!(
            rest.head.translation.length() < 5.0,
            "model scale applied: head near root, got {:?}",
            rest.head.translation
        );
    }

    /// END-TO-END with the REAL frozen manifests + `.vox`: parse both bird
    /// manifests, load every part's `.vox`, assemble a Snowy Owl → assert > 0
    /// non-empty parts. Needs the asset tree; run locally.
    #[test]
    #[ignore = "reads the real bird_medium manifests + .vox: needs the asset tree"]
    fn real_owl_assembles() {
        use dot_vox::load_bytes;

        let root = std::env::var("XINDELER_ASSETS")
            .or_else(|_| std::env::var("VELOREN_ASSETS"))
            .expect("set XINDELER_ASSETS or VELOREN_ASSETS to the assets dir");
        let read_ron = |dotted: &str| -> String {
            let path = format!("{root}/{}.ron", dotted.replace('.', "/"));
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
        };
        let central: BmCentralManifest =
            ron::de::from_str(&read_ron(BM_CENTRAL_MANIFEST)).expect("BM central manifest");
        let lateral: BmLateralManifest =
            ron::de::from_str(&read_ron(BM_LATERAL_MANIFEST)).expect("BM lateral manifest");
        let specs = bird_medium_part_specs(&central, &lateral, Species::SnowyOwl, BodyType::Male)
            .expect("owl has manifest entries");
        let voxes: Vec<dot_vox::DotVoxData> = specs
            .iter()
            .map(|s| {
                let path = format!("{root}/voxygen/voxel/{}.vox", s.vox_name.replace('.', "/"));
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
                load_bytes(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"))
            })
            .collect();
        let loaded: Vec<LoadedBmPart> = specs
            .iter()
            .zip(&voxes)
            .map(|(s, vox)| LoadedBmPart {
                vox,
                model_index: s.model_index,
                offset: s.offset,
                flipped: s.flipped,
                bone: s.bone,
            })
            .collect();
        let rest = bird_medium_bone_rest(Species::SnowyOwl, BodyType::Male);
        let assembled = assemble(&loaded, &rest);
        assert!(
            !assembled.is_empty(),
            "owl should assemble to > 0 parts, got {}",
            assembled.len()
        );
        for (_, p) in &assembled {
            assert!(p.mesh.count_vertices() > 0, "part {} has vertices", p.name);
        }
        eprintln!(
            "assembled {} bird parts: {:?}",
            assembled.len(),
            assembled.iter().map(|(_, p)| p.name).collect::<Vec<_>>()
        );
    }

    /// Idle, run and fly are three distinct poses; run advances by acc.
    #[test]
    fn bm_anims_differ() {
        let idle = bird_medium_bone_transforms(
            Species::SnowyOwl,
            BodyType::Male,
            FigureAnim::Idle,
            0.0,
            1.0,
            0.0,
        );
        let run = bird_medium_bone_transforms(
            Species::SnowyOwl,
            BodyType::Male,
            FigureAnim::Run,
            2.0,
            1.0,
            6.0,
        );
        let fly = bird_medium_bone_transforms(
            Species::SnowyOwl,
            BodyType::Male,
            FigureAnim::Fly,
            2.0,
            1.0,
            6.0,
        );
        assert_ne!(
            idle.wing_in_l.translation, fly.wing_in_l.translation,
            "flying should move the wings vs perched idle"
        );
        let run_b = bird_medium_bone_transforms(
            Species::SnowyOwl,
            BodyType::Male,
            FigureAnim::Run,
            9.0,
            1.0,
            6.0,
        );
        assert_ne!(
            run.leg_l.translation, run_b.leg_l.translation,
            "run cycle advances with the acc accumulator"
        );
    }
}
