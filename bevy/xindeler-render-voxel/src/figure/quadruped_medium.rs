//! EM-3.8c — the QUADRUPED-MEDIUM figure path (wolves, bears, deer, …).
//!
//! Additive body on the SAME machinery as the quadruped-small pig (EM-3.8): a
//! central manifest (head/neck/jaw/ears/torso-front/torso-back/tail) + a
//! lateral manifest (four legs + four feet), meshed part-by-part by the shared
//! figure mesher ([`super::segment_to_bevy`]) and placed at the
//! `xindeler-anim` `quadruped_medium` skeleton's bone matrices — animated
//! (idle vs run) exactly like the humanoid.
//!
//! ## What we port (from `voxygen/src/scene/figure/load.rs`)
//! - `QuadrupedMediumCentralSpec` / `QuadrupedMediumLateralSpec` — the two
//!   `(Species, BodyType)`-keyed manifests. We read the SAME frozen RON with
//!   the minimal deser structs below (isolation law rule 3).
//! - The bone order + `flipped` rule of the `mesh_*` functions: the LEFT legs
//!   and feet (`*_fl`, `*_bl`) reuse the right `.vox` mirrored
//!   (`graceful_load_segment_flipped(.., true, ..)`); the rest load unflipped.
//!
//! ## Purity
//! Same as the parent module — engine-shell code over the LOGIC crates
//! (`common` + `xindeler-anim`); the caller feeds parsed `.vox` bytes.

use bevy::transform::components::Transform;
use serde::Deserialize;
use vek::*;

use super::{FigureAnim, FigurePart, LoadedPart, VoxSimple, mat_to_transform};
use common::comp::quadruped_medium::{BodyType, Species};

/// The manifest ASSET PATHS (upstream names, frozen — isolation law rule 3).
pub const QM_CENTRAL_MANIFEST: &str = "voxygen.voxel.quadruped_medium_central_manifest";
pub const QM_LATERAL_MANIFEST: &str = "voxygen.voxel.quadruped_medium_lateral_manifest";

// ---------------------------------------------------------------------------
// Manifest deser (minimal read of the frozen RON — mirrors load.rs shapes)
// ---------------------------------------------------------------------------
//
// `VoxSimple` is imported from the parent module (EM-3.8e dedup — it used to
// be redefined identically in every additive body module).

/// One central/lateral sub-part: offset + which `.vox` + model index.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct QmSubSpec {
    pub offset: [f32; 3],
    #[serde(alias = "central", alias = "lateral")]
    pub model: VoxSimple,
    pub model_index: u32,
}

/// One `(species, body_type)` central-manifest entry.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct QmCentralEntry {
    pub head: QmSubSpec,
    pub neck: QmSubSpec,
    pub jaw: QmSubSpec,
    pub ears: QmSubSpec,
    pub torso_front: QmSubSpec,
    pub torso_back: QmSubSpec,
    pub tail: QmSubSpec,
}

/// One `(species, body_type)` lateral-manifest entry (four legs + four feet).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct QmLateralEntry {
    pub leg_fl: QmSubSpec,
    pub leg_fr: QmSubSpec,
    pub leg_bl: QmSubSpec,
    pub leg_br: QmSubSpec,
    pub foot_fl: QmSubSpec,
    pub foot_fr: QmSubSpec,
    pub foot_bl: QmSubSpec,
    pub foot_br: QmSubSpec,
}

/// The whole quadruped-medium central manifest.
#[derive(Deserialize, Clone, Debug)]
pub struct QmCentralManifest(pub std::collections::HashMap<(Species, BodyType), QmCentralEntry>);

/// The whole quadruped-medium lateral manifest.
#[derive(Deserialize, Clone, Debug)]
pub struct QmLateralManifest(pub std::collections::HashMap<(Species, BodyType), QmLateralEntry>);

// ---------------------------------------------------------------------------
// Bones
// ---------------------------------------------------------------------------

/// The bones a quadruped-medium figure has (matches
/// `ComputedQuadrupedMediumSkeleton`). `ears` is parented separately in the
/// skeleton, but for placement we treat it as its own bone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QmBone {
    Head,
    Neck,
    Jaw,
    Ears,
    TorsoFront,
    TorsoBack,
    Tail,
    LegFl,
    LegFr,
    LegBl,
    LegBr,
    FootFl,
    FootFr,
    FootBl,
    FootBr,
}

fn qm_bone_label(bone: QmBone) -> &'static str {
    match bone {
        QmBone::Head => "head",
        QmBone::Neck => "neck",
        QmBone::Jaw => "jaw",
        QmBone::Ears => "ears",
        QmBone::TorsoFront => "torso_front",
        QmBone::TorsoBack => "torso_back",
        QmBone::Tail => "tail",
        QmBone::LegFl => "leg_fl",
        QmBone::LegFr => "leg_fr",
        QmBone::LegBl => "leg_bl",
        QmBone::LegBr => "leg_br",
        QmBone::FootFl => "foot_fl",
        QmBone::FootFr => "foot_fr",
        QmBone::FootBl => "foot_bl",
        QmBone::FootBr => "foot_br",
    }
}

/// A resolved reference to one QM part's manifest data (like the shared
/// [`PartSpecRef`] but keyed by [`QmBone`]).
pub struct QmPartSpecRef {
    pub bone: QmBone,
    pub vox_name: String,
    pub model_index: u32,
    pub offset: Vec3<f32>,
    pub flipped: bool,
}

impl QmPartSpecRef {
    fn new(bone: QmBone, spec: &QmSubSpec, flipped: bool) -> Self {
        Self {
            bone,
            vox_name: spec.model.0.clone(),
            model_index: spec.model_index,
            offset: Vec3::from(spec.offset),
            flipped,
        }
    }
}

/// The parts a quadruped-medium figure needs. `flipped` follows voxygen: the
/// LEFT legs/feet (`*_fl`, `*_bl`) reuse the right `.vox` mirrored. Returns
/// `None` if either manifest lacks an entry for this `(species, body_type)`
/// (caller keeps its placeholder).
#[must_use]
pub fn quadruped_medium_part_specs(
    central: &QmCentralManifest,
    lateral: &QmLateralManifest,
    species: Species,
    body_type: BodyType,
) -> Option<Vec<QmPartSpecRef>> {
    let c = central.0.get(&(species, body_type))?;
    let l = lateral.0.get(&(species, body_type))?;
    Some(vec![
        QmPartSpecRef::new(QmBone::Head, &c.head, false),
        QmPartSpecRef::new(QmBone::Neck, &c.neck, false),
        QmPartSpecRef::new(QmBone::Jaw, &c.jaw, false),
        QmPartSpecRef::new(QmBone::Ears, &c.ears, false),
        QmPartSpecRef::new(QmBone::TorsoFront, &c.torso_front, false),
        QmPartSpecRef::new(QmBone::TorsoBack, &c.torso_back, false),
        QmPartSpecRef::new(QmBone::Tail, &c.tail, false),
        QmPartSpecRef::new(QmBone::LegFl, &l.leg_fl, true),
        QmPartSpecRef::new(QmBone::LegFr, &l.leg_fr, false),
        QmPartSpecRef::new(QmBone::LegBl, &l.leg_bl, true),
        QmPartSpecRef::new(QmBone::LegBr, &l.leg_br, false),
        QmPartSpecRef::new(QmBone::FootFl, &l.foot_fl, true),
        QmPartSpecRef::new(QmBone::FootFr, &l.foot_fr, false),
        QmPartSpecRef::new(QmBone::FootBl, &l.foot_bl, true),
        QmPartSpecRef::new(QmBone::FootBr, &l.foot_br, false),
    ])
}

// ---------------------------------------------------------------------------
// Bone transforms (rest + animated)
// ---------------------------------------------------------------------------

/// The full set of quadruped-medium bone transforms (Bevy space, model-scaled).
pub struct QmBoneTransforms {
    pub head: Transform,
    pub neck: Transform,
    pub jaw: Transform,
    pub ears: Transform,
    pub torso_front: Transform,
    pub torso_back: Transform,
    pub tail: Transform,
    pub leg_fl: Transform,
    pub leg_fr: Transform,
    pub leg_bl: Transform,
    pub leg_br: Transform,
    pub foot_fl: Transform,
    pub foot_fr: Transform,
    pub foot_bl: Transform,
    pub foot_br: Transform,
}

impl QmBoneTransforms {
    #[must_use]
    pub fn get(&self, bone: QmBone) -> Transform {
        match bone {
            QmBone::Head => self.head,
            QmBone::Neck => self.neck,
            QmBone::Jaw => self.jaw,
            QmBone::Ears => self.ears,
            QmBone::TorsoFront => self.torso_front,
            QmBone::TorsoBack => self.torso_back,
            QmBone::Tail => self.tail,
            QmBone::LegFl => self.leg_fl,
            QmBone::LegFr => self.leg_fr,
            QmBone::LegBl => self.leg_bl,
            QmBone::LegBr => self.leg_br,
            QmBone::FootFl => self.foot_fl,
            QmBone::FootFr => self.foot_fr,
            QmBone::FootBl => self.foot_bl,
            QmBone::FootBr => self.foot_br,
        }
    }
}

/// Animated quadruped-medium bone transforms (idle vs run), `acc` driving the
/// foot-cycle phase (blocks travelled) and `time` the idle sway.
#[must_use]
pub fn quadruped_medium_bone_transforms(
    species: Species,
    body_type: BodyType,
    anim: FigureAnim,
    acc: f32,
    time: f32,
    ground_speed: f32,
) -> QmBoneTransforms {
    use xindeler_anim::{
        Animation, Skeleton,
        quadruped_medium::{IdleAnimation, QuadrupedMediumSkeleton, RunAnimation, SkeletonAttr},
    };

    let body = common::comp::quadruped_medium::Body { species, body_type };
    let attr = SkeletonAttr::from(&body);
    let base = QuadrupedMediumSkeleton::default();
    let mut rate = 0.0;

    let skeleton = match anim {
        FigureAnim::Idle => IdleAnimation::update_skeleton(&base, time, time, &mut rate, &attr),
        FigureAnim::Run | FigureAnim::Fly => {
            // QM run dependency: `(velocity: f32, orientation, last_ori,
            // global_time, avg_vel, acc_vel)` — same shape as QS.
            let speed = ground_speed.max(0.5);
            let ori = Vec3::new(0.0, 1.0, 0.0);
            let vel = Vec3::new(0.0, speed, 0.0);
            RunAnimation::update_skeleton(
                &base,
                (speed, ori, ori, time, vel, acc),
                time,
                &mut rate,
                &attr,
            )
        },
    };

    let mut buf = [xindeler_anim::FigureBoneData::default(); xindeler_anim::MAX_BONE_COUNT];
    let computed = skeleton.compute_matrices(vek::Mat4::identity(), &mut buf, body);

    QmBoneTransforms {
        head: mat_to_transform(computed.head),
        neck: mat_to_transform(computed.neck),
        jaw: mat_to_transform(computed.jaw),
        ears: mat_to_transform(computed.ears),
        torso_front: mat_to_transform(computed.torso_front),
        torso_back: mat_to_transform(computed.torso_back),
        tail: mat_to_transform(computed.tail),
        leg_fl: mat_to_transform(computed.leg_fl),
        leg_fr: mat_to_transform(computed.leg_fr),
        leg_bl: mat_to_transform(computed.leg_bl),
        leg_br: mat_to_transform(computed.leg_br),
        foot_fl: mat_to_transform(computed.foot_fl),
        foot_fr: mat_to_transform(computed.foot_fr),
        foot_bl: mat_to_transform(computed.foot_bl),
        foot_br: mat_to_transform(computed.foot_br),
    }
}

/// The static rest pose (idle at `time = 0`). Convenience wrapper.
#[must_use]
pub fn quadruped_medium_bone_rest(species: Species, body_type: BodyType) -> QmBoneTransforms {
    quadruped_medium_bone_transforms(species, body_type, FigureAnim::Idle, 0.0, 0.0, 0.0)
}

// ---------------------------------------------------------------------------
// Assemble
// ---------------------------------------------------------------------------

/// A loaded QM `.vox` paired back with its bone/offset/flip, ready to mesh
/// (EM-3.8e dedup: a type alias over the shared, bone-generic [`LoadedPart`]
/// rather than a hand-duplicated struct).
pub type LoadedQmPart<'a> = LoadedPart<'a, QmBone>;

/// Meshes each loaded part (shared figure mesher) and pairs it with its bone's
/// transform. Parts that mesh to nothing are dropped. `bones` is the pose
/// (rest or animated). Returns the placed [`FigurePart`]s keyed by a generic
/// label — the caller maps them to child entities by bone via
/// [`assemble_with_bones`] instead when it needs per-frame animation.
#[must_use]
pub fn assemble(parts: &[LoadedQmPart], bones: &QmBoneTransforms) -> Vec<(QmBone, FigurePart)> {
    parts
        .iter()
        .filter_map(|part| {
            let mesh = super::figure_part_to_bevy(part)?;
            Some((part.bone, FigurePart {
                mesh,
                transform: bones.get(part.bone),
                name: qm_bone_label(part.bone),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rest pose is deterministic + sane: all bones finite, model-scaled,
    /// head above the feet. Pure `xindeler-anim` — no assets.
    #[test]
    fn qm_rest_pose_is_sane() {
        let rest = quadruped_medium_bone_rest(Species::Wolf, BodyType::Male);
        for (name, t) in [
            ("head", rest.head),
            ("torso_front", rest.torso_front),
            ("foot_fl", rest.foot_fl),
            ("tail", rest.tail),
        ] {
            assert!(
                t.translation.is_finite(),
                "{name} rest translation must be finite: {:?}",
                t.translation
            );
        }
        assert!(
            rest.head.translation.y > rest.foot_fl.translation.y,
            "head above feet: head.y={} foot.y={}",
            rest.head.translation.y,
            rest.foot_fl.translation.y
        );
        assert!(
            rest.head.translation.length() < 5.0,
            "model scale applied: head near root, got {:?}",
            rest.head.translation
        );
    }

    /// END-TO-END with the REAL frozen manifests + `.vox` assets: parse both QM
    /// manifests, load every part's `.vox`, assemble a Wolf → assert we get a
    /// full set of non-empty parts (> 0). Needs the asset tree; run locally
    /// with `XINDELER_ASSETS`/`VELOREN_ASSETS`.
    #[test]
    #[ignore = "reads the real quadruped_medium manifests + .vox: needs the asset tree"]
    fn real_wolf_assembles() {
        use dot_vox::load_bytes;

        let root = std::env::var("XINDELER_ASSETS")
            .or_else(|_| std::env::var("VELOREN_ASSETS"))
            .expect("set XINDELER_ASSETS or VELOREN_ASSETS to the assets dir");
        let read_ron = |dotted: &str| -> String {
            let path = format!("{root}/{}.ron", dotted.replace('.', "/"));
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
        };
        let central: QmCentralManifest =
            ron::de::from_str(&read_ron(QM_CENTRAL_MANIFEST)).expect("QM central manifest");
        let lateral: QmLateralManifest =
            ron::de::from_str(&read_ron(QM_LATERAL_MANIFEST)).expect("QM lateral manifest");
        let specs = quadruped_medium_part_specs(&central, &lateral, Species::Wolf, BodyType::Male)
            .expect("wolf has manifest entries");
        let voxes: Vec<dot_vox::DotVoxData> = specs
            .iter()
            .map(|s| {
                let path = format!("{root}/voxygen/voxel/{}.vox", s.vox_name.replace('.', "/"));
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
                load_bytes(&bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"))
            })
            .collect();
        let loaded: Vec<LoadedQmPart> = specs
            .iter()
            .zip(&voxes)
            .map(|(s, vox)| LoadedQmPart {
                vox,
                model_index: s.model_index,
                offset: s.offset,
                flipped: s.flipped,
                bone: s.bone,
            })
            .collect();
        let rest = quadruped_medium_bone_rest(Species::Wolf, BodyType::Male);
        let assembled = assemble(&loaded, &rest);
        assert!(
            !assembled.is_empty(),
            "wolf should assemble to > 0 parts, got {}",
            assembled.len()
        );
        for (_, p) in &assembled {
            assert!(p.mesh.count_vertices() > 0, "part {} has vertices", p.name);
        }
        eprintln!(
            "assembled {} QM parts: {:?}",
            assembled.len(),
            assembled.iter().map(|(_, p)| p.name).collect::<Vec<_>>()
        );
    }

    /// BL-82 EM-3.11l regression: every `Species` × `BodyType` this project
    /// ships must resolve to `Some` specs with every sub-part's `vox_name`
    /// non-empty, so no shipped quadruped-medium species can silently fall
    /// through `classify_bodies` and get stuck on the placeholder capsule
    /// forever. Two distinct completeness bugs this catches:
    /// - a species/body_type key missing from either manifest entirely
    ///   (`quadruped_medium_part_specs` returns `None`);
    /// - `#[serde(default)]` on `QmSubSpec`/`QmCentralEntry`/`QmLateralEntry`
    ///   means a species key PRESENT in the manifest but missing an individual
    ///   bone sub-field silently defaults to an EMPTY `vox_name` instead of
    ///   erroring at parse time.
    ///
    /// Same "does every kind resolve to real data, no silent fallback" shape
    /// as `sprite::tests::sprite_kinds_have_non_black_colour_data`.
    #[test]
    #[ignore = "reads the real quadruped_medium manifests: needs the asset tree"]
    fn qm_species_have_complete_manifest_specs() {
        let root = std::env::var("XINDELER_ASSETS")
            .or_else(|_| std::env::var("VELOREN_ASSETS"))
            .expect("set XINDELER_ASSETS or VELOREN_ASSETS to the assets dir");
        let read_ron = |dotted: &str| -> String {
            let path = format!("{root}/{}.ron", dotted.replace('.', "/"));
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
        };
        let central: QmCentralManifest =
            ron::de::from_str(&read_ron(QM_CENTRAL_MANIFEST)).expect("QM central manifest");
        let lateral: QmLateralManifest =
            ron::de::from_str(&read_ron(QM_LATERAL_MANIFEST)).expect("QM lateral manifest");

        let mut problems = Vec::new();
        for &species in Species::ALL.iter() {
            for &body_type in BodyType::ALL.iter() {
                match quadruped_medium_part_specs(&central, &lateral, species, body_type) {
                    None => problems.push(format!("{species:?}/{body_type:?}: NO manifest entry")),
                    Some(specs) => {
                        for s in &specs {
                            if s.vox_name.is_empty() {
                                problems.push(format!(
                                    "{species:?}/{body_type:?}: bone {:?} has EMPTY vox_name \
                                     (defaulted sub-spec)",
                                    s.bone
                                ));
                            }
                        }
                    },
                }
            }
        }
        if !problems.is_empty() {
            panic!(
                "{} incomplete quadruped-medium specs:\n{}",
                problems.len(),
                problems.join("\n")
            );
        }
    }

    /// Idle vs run differ, and run advances with the acc phase.
    #[test]
    fn qm_run_differs_from_idle_and_advances_by_acc() {
        let idle = quadruped_medium_bone_transforms(
            Species::Wolf,
            BodyType::Male,
            FigureAnim::Idle,
            0.0,
            1.0,
            0.0,
        );
        let run_a = quadruped_medium_bone_transforms(
            Species::Wolf,
            BodyType::Male,
            FigureAnim::Run,
            2.0,
            1.0,
            6.0,
        );
        let run_b = quadruped_medium_bone_transforms(
            Species::Wolf,
            BodyType::Male,
            FigureAnim::Run,
            8.0,
            1.0,
            6.0,
        );
        assert_ne!(
            idle.foot_fl.translation, run_a.foot_fl.translation,
            "run should move the feet vs idle"
        );
        assert_ne!(
            run_a.foot_fl.translation, run_b.foot_fl.translation,
            "run cycle advances with the acc accumulator"
        );
    }
}
