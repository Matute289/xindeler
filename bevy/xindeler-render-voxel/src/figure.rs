//! EM-3.8 — real `.vox` figures (Mapper C7 mesher + C12 load + C13 anim).
//!
//! Turns a sim `Body` into a set of renderable, correctly-placed voxel PARTS,
//! reusing three pieces that already landed:
//! - the **figure mesher**
//!   ([`crate::mesh::segment::generate_mesh_base_vol_figure`], Mapper C7) —
//!   greedy-meshes one `.vox` [`Segment`] into a `Mesh<TerrainVertex>` + a
//!   per-figure colour atlas ([`FigureSpriteAtlasData`]);
//! - the **`.vox` → [`Segment`]** path in `common::figure` (Mapper C12) —
//!   parses `dot_vox` model data into the `Cell` volume the mesher reads;
//! - **`xindeler-anim`** (Mapper C13) — the kept skeletal-animation crate, used
//!   here only for its REST-POSE bone matrices (each already carries the
//!   per-species `scaler / 11` model scale), so the assembled parts sit where a
//!   standing figure's limbs sit.
//!
//! ## What v1 does (and does NOT)
//! v1 assembles a **static** figure: every part is meshed at its manifest
//! offset and parented at its bone's REST matrix (the idle animation evaluated
//! at `anim_time = 0`, which is deterministic — `sin(0) = 0`). There is NO
//! time-based skeletal animation yet: the parts don't walk/breathe.
//! `TODO(EM-3.8b)`: feed per-frame bone matrices (run the real `*Animation`
//! against the sim's `CharacterState`/velocity) into the child `Transform`s.
//!
//! v1 also covers only the **manifest-driven** bodies (a central + lateral
//! `.vox` manifest keyed by `(species, body_type)`, no armour/recolour): the
//! [`FigureBody::QuadrupedSmall`] path is wired end-to-end (it is the sim's
//! test-NPC body — a Pig). `TODO(EM-3.8b)`: the **humanoid** figure
//! (per-species head/skin/hair/eye recolour via `MatSegment`, the armour
//! loadout, weapons, and the 16-bone character skeleton) is a much larger
//! assembly and stays a placeholder capsule for now; the other quadruped /
//! bird / etc. bodies are additive table entries on the SAME machinery here.
//!
//! ## Colour, not texture arrays (spec §4.2)
//! Terrain uses PBR texture arrays keyed by a per-vertex block layer; figures
//! instead keep the `.vox`'s **per-voxel colour** — the mesher bakes it into
//! the [`FigureColLight`] atlas, and [`figure_part_to_bevy`] samples that atlas
//! per vertex into `Mesh::ATTRIBUTE_COLOR`, rendered by a plain
//! `StandardMaterial` (vertex colours + a matte roughness). No custom material.
//!
//! ## Purity / isolation
//! This module lives in the engine shell (`bevy/`), depends on the LOGIC crates
//! `common` + `xindeler-anim` (legal shell→logic direction), and takes the
//! `.vox` bytes as `&DotVoxData` from the caller — it never loads assets itself
//! (render-voxel keeps `common`'s `no-assets`). The `AssetServer` plumbing that
//! feeds it the bytes lives client-side (`xindeler-client`).

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh as BevyMesh, PrimitiveTopology},
    transform::components::Transform,
};
use common::figure::Segment;
use dot_vox::DotVoxData;
use serde::Deserialize;
use vek::*;

use crate::mesh::{
    greedy::GreedyMesh,
    mesh::Mesh,
    segment::generate_mesh_base_vol_figure,
    vertex::{FigureSpriteAtlasData, TerrainVertex},
};

/// Upstream's max-texture-size hint for the greedy atlas (same value the
/// terrain pipeline uses — figures are tiny, so this never grows).
const MAX_ATLAS_SIZE: Vec2<u16> = Vec2 { x: 4096, y: 4096 };

/// Which figure a mirrored entity displays as, resolved from the replicated
/// `Body` on the client BEFORE it reaches this crate (this crate stays free of
/// the protocol type). v1 only distinguishes the bodies it can actually build;
/// everything else is [`FigureBody::Unsupported`] and the caller keeps a
/// placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FigureBody {
    /// A quadruped-small (Pig, Fox, …): central (head/chest/tail) + lateral
    /// (four feet) manifests, no armour/recolour.
    QuadrupedSmall {
        species: common::comp::quadruped_small::Species,
        body_type: common::comp::quadruped_small::BodyType,
    },
    /// A body v1 does not build a real figure for yet (humanoid, quadruped
    /// medium, birds, …). The caller falls back to its placeholder.
    Unsupported,
}

/// One assembled, ready-to-spawn figure part: a coloured `bevy::Mesh` and the
/// child `Transform` (already in Bevy y-up space, already scaled by the
/// skeleton's model scale) it should be parented at, under the entity that owns
/// the interpolated root `Transform`.
pub struct FigurePart {
    /// Greedy-meshed part with `POSITION`/`NORMAL`/`COLOR` (spec §4.2 —
    /// figures use per-voxel colour, not texture arrays).
    pub mesh: BevyMesh,
    /// Local transform of this part relative to the figure root (bone rest
    /// matrix, Veloren z-up → Bevy y-up).
    pub transform: Transform,
    /// Debug label (bone name) — handy in the entity inspector / logs.
    pub name: &'static str,
}

/// A `.vox` asset the caller has already loaded + parsed, paired with the
/// manifest metadata for ONE part. Keeps this crate ignorant of the asset
/// system: the client resolves `spec.vox` → bytes → [`DotVoxData`] and hands
/// the parsed data in.
pub struct LoadedPart<'a> {
    /// Parsed `.vox` data for the part's model file.
    pub vox: &'a DotVoxData,
    /// Which model inside the `.vox` (upstream `model_index`).
    pub model_index: u32,
    /// Manifest offset (voxel units, pre-scale) that recentres the part around
    /// its bone origin — fed to the mesher exactly as voxygen does.
    pub offset: Vec3<f32>,
    /// Mirror the model across X (upstream `graceful_load_segment_flipped`,
    /// used for the left-side limbs whose `.vox` is the right-side mesh).
    pub flipped: bool,
    /// Which skeleton bone this part is parented to (rest matrix looked up by
    /// the caller-provided bone table).
    pub bone: FigureBoneName,
}

/// The bones a manifest-driven quadruped-small figure has (matches the
/// `make_vox_spec!`/skeleton bone order in `voxygen/src/scene/figure/load.rs` +
/// `voxygen/anim/src/quadruped_small`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FigureBoneName {
    Head,
    Chest,
    LegFl,
    LegFr,
    LegBl,
    LegBr,
    Tail,
}

// ---------------------------------------------------------------------------
// Manifest deserialisation (Mapper C12).
//
// These mirror the SHAPE of the RON manifests in
// `assets/voxygen/voxel/quadruped_small_{central,lateral}_manifest.ron` (the
// field subset EM-3.8 needs — offset + model name + model_index). They are NOT
// the voxygen `load.rs` structs (those pull in the whole figure-cache stack);
// they are the minimal portable read of the SAME data files (names frozen —
// isolation law rule 3). Keyed by `(Species, BodyType)`, both of which derive
// serde in `common`.
// ---------------------------------------------------------------------------

/// A `.vox` reference in a manifest: `("model.name")` or `("model.name", idx)`.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct VoxSimple(pub String);

/// One central/lateral sub-part: offset + which `.vox` + model index.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PartSubSpec {
    /// Offset relative to the initial origin (voxel units).
    pub offset: [f32; 3],
    /// The part's `.vox` specifier (central or lateral — same shape).
    #[serde(alias = "central", alias = "lateral")]
    pub model: VoxSimple,
    pub model_index: u32,
}

/// One `(species, body_type)` central-manifest entry (head/chest/tail).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct QsCentralEntry {
    pub head: PartSubSpec,
    pub chest: PartSubSpec,
    pub tail: PartSubSpec,
}

/// One `(species, body_type)` lateral-manifest entry (the four feet).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct QsLateralEntry {
    pub left_front: PartSubSpec,
    pub right_front: PartSubSpec,
    pub left_back: PartSubSpec,
    pub right_back: PartSubSpec,
}

/// The whole quadruped-small central manifest (`HashMap` in RON → `Vec` of
/// keyed entries; we deserialise into a lookup by the concrete key type).
#[derive(Deserialize, Clone, Debug)]
pub struct QsCentralManifest(
    pub  std::collections::HashMap<
        (
            common::comp::quadruped_small::Species,
            common::comp::quadruped_small::BodyType,
        ),
        QsCentralEntry,
    >,
);

/// The whole quadruped-small lateral manifest.
#[derive(Deserialize, Clone, Debug)]
pub struct QsLateralManifest(
    pub  std::collections::HashMap<
        (
            common::comp::quadruped_small::Species,
            common::comp::quadruped_small::BodyType,
        ),
        QsLateralEntry,
    >,
);

/// The manifest ASSET PATHS (upstream names, frozen — isolation law rule 3).
/// The client loads these two RON files and parses them into the structs above.
pub const QS_CENTRAL_MANIFEST: &str = "voxygen.voxel.quadruped_small_central_manifest";
pub const QS_LATERAL_MANIFEST: &str = "voxygen.voxel.quadruped_small_lateral_manifest";

/// The seven parts a quadruped-small figure needs, as (bone, sub-spec,
/// flipped, `.vox` name) — the caller uses this to know WHICH `.vox` files to
/// load, then calls [`assemble_quadruped_small`] with the parsed data.
///
/// Returns `None` if the manifests have no entry for this `(species,
/// body_type)` — the caller keeps its placeholder (matches voxygen's
/// `not_found` fallback, minus the debug mesh).
#[must_use]
pub fn quadruped_small_part_specs(
    central: &QsCentralManifest,
    lateral: &QsLateralManifest,
    species: common::comp::quadruped_small::Species,
    body_type: common::comp::quadruped_small::BodyType,
) -> Option<Vec<PartSpecRef>> {
    let c = central.0.get(&(species, body_type))?;
    let l = lateral.0.get(&(species, body_type))?;
    // Order matches the skeleton bone order (head, chest, four legs, tail).
    // `flipped` follows voxygen: the LEFT limbs reuse the right `.vox` mirrored
    // (`graceful_load_segment_flipped(.., true, ..)`).
    Some(vec![
        PartSpecRef::new(FigureBoneName::Head, &c.head, false),
        PartSpecRef::new(FigureBoneName::Chest, &c.chest, false),
        PartSpecRef::new(FigureBoneName::LegFl, &l.left_front, true),
        PartSpecRef::new(FigureBoneName::LegFr, &l.right_front, false),
        PartSpecRef::new(FigureBoneName::LegBl, &l.left_back, true),
        PartSpecRef::new(FigureBoneName::LegBr, &l.right_back, false),
        PartSpecRef::new(FigureBoneName::Tail, &c.tail, false),
    ])
}

/// A resolved reference to one part's manifest data (borrowed name + copied
/// scalars) — the caller reads `.vox_name` to load the file, then pairs it back
/// up into a [`LoadedPart`].
pub struct PartSpecRef {
    pub bone: FigureBoneName,
    pub vox_name: String,
    pub model_index: u32,
    pub offset: Vec3<f32>,
    pub flipped: bool,
}

impl PartSpecRef {
    fn new(bone: FigureBoneName, spec: &PartSubSpec, flipped: bool) -> Self {
        Self {
            bone,
            vox_name: spec.model.0.clone(),
            model_index: spec.model_index,
            offset: Vec3::from(spec.offset),
            flipped,
        }
    }
}

// ---------------------------------------------------------------------------
// Meshing + assembly (Mapper C7 + C13).
// ---------------------------------------------------------------------------

/// Meshes ONE loaded `.vox` part into a coloured `bevy::Mesh`.
///
/// Runs the figure mesher (Mapper C7) at the part's manifest offset and scale
/// `1` (the model scale lives in the bone rest matrix, exactly as voxygen's
/// figure cache calls it with `Vec3::one()`), finalises the per-figure colour
/// atlas, then samples each vertex's `atlas_pos` into `ATTRIBUTE_COLOR`.
/// Returns `None` if the part meshes to nothing (empty `.vox`).
#[must_use]
pub fn figure_part_to_bevy(part: &LoadedPart) -> Option<BevyMesh> {
    let segment = Segment::from_vox(part.vox, part.flipped, part.model_index as usize, None);

    let mut greedy =
        GreedyMesh::<FigureSpriteAtlasData>::new(MAX_ATLAS_SIZE, greedy_general_config());
    let mut opaque = Mesh::<TerrainVertex>::new();
    // Bone index 0: v1 is static, so the (unused) bone-in-vertex packing is
    // irrelevant — each PART is its own mesh placed by its own child Transform.
    let _ = generate_mesh_base_vol_figure(
        &segment,
        (&mut greedy, &mut opaque, part.offset, Vec3::one(), 0),
    );
    if opaque.is_empty() {
        return None;
    }
    let (atlas, atlas_size) = greedy.finalize();
    Some(figure_mesh_to_bevy(&opaque, &atlas, atlas_size))
}

/// The greedy allocator config for figures (upstream `greedy::general_config`,
/// re-exposed here so we don't widen the mesh module's public surface).
fn greedy_general_config() -> guillotiere::AllocatorOptions {
    crate::mesh::greedy::general_config()
}

/// Veloren z-up → Bevy y-up (pure rotation, winding preserved — same map the
/// terrain converter bakes; see `convert.rs`).
#[inline]
fn to_bevy(v: Vec3<f32>) -> [f32; 3] { [v.x, v.z, -v.y] }

/// Converts a meshed figure part (+ its colour atlas) into a `bevy::Mesh` with
/// `POSITION`/`NORMAL`/`COLOR` (per-voxel colour, spec §4.2). The atlas index
/// math mirrors the terrain converter's (`convert.rs`): row-major
/// `y * width + x`.
fn figure_mesh_to_bevy(
    mesh: &Mesh<TerrainVertex>,
    atlas: &FigureSpriteAtlasData,
    atlas_size: Vec2<u16>,
) -> BevyMesh {
    let n = mesh.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut colors = Vec::with_capacity(n);

    let texel = |atlas_pos: Vec2<u16>| {
        usize::from(atlas_pos.y) * usize::from(atlas_size.x) + usize::from(atlas_pos.x)
    };

    for v in mesh.vertices() {
        positions.push(to_bevy(v.pos));
        normals.push(to_bevy(v.norm));
        let col = atlas.col_lights[texel(v.atlas_pos)].col;
        // sRGB 8-bit voxel colour → linear-ish vertex colour. bevy multiplies
        // vertex colour into base_color in the (linear) fragment; feed the
        // 0..1 sRGB value and let the material's tonemapper handle it (matches
        // how the terrain palette colours are authored).
        colors.push([
            f32::from(col.r) / 255.0,
            f32::from(col.g) / 255.0,
            f32::from(col.b) / 255.0,
            1.0,
        ]);
    }

    let mut out = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(BevyMesh::ATTRIBUTE_COLOR, colors);
    out.insert_indices(Indices::U32(quad_indices(n)));
    out
}

/// Explicit triangle-list indices equivalent to the shared quad index buffer
/// (`0,1,2, 2,1,3` per 4-vertex quad) — identical to `convert.rs::quad_indices`
/// (figures are quad-indexed like terrain).
fn quad_indices(vert_count: usize) -> Vec<u32> {
    debug_assert_eq!(vert_count % 4, 0, "quad-indexed mesh: 4 verts per quad");
    let mut indices = Vec::with_capacity(vert_count / 4 * 6);
    for quad in 0..(vert_count / 4) as u32 {
        let base = quad * 4;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }
    indices
}

/// Bone rest matrices for a quadruped-small figure, keyed by
/// [`FigureBoneName`], each already in Bevy y-up space and scaled by the model
/// scale.
///
/// Computed from `xindeler-anim` (Mapper C13): the default skeleton run through
/// the idle animation at `anim_time = 0` (deterministic rest pose:
/// `sin(0) = 0`), then `compute_matrices` with an identity base — so each
/// bone matrix carries the per-species `scaler / 11` model scale.
#[must_use]
pub fn quadruped_small_bone_rest(
    species: common::comp::quadruped_small::Species,
    body_type: common::comp::quadruped_small::BodyType,
) -> BoneRest {
    use xindeler_anim::{
        Animation, Skeleton,
        quadruped_small::{IdleAnimation, QuadrupedSmallSkeleton, SkeletonAttr},
    };

    let body = common::comp::quadruped_small::Body { species, body_type };
    let attr = SkeletonAttr::from(&body);
    // Rest pose: idle at t=0 (global_time=0, anim_time=0). `update_skeleton`
    // dispatches to the inner fn (no dynlib in this build).
    let mut rate = 0.0;
    let skeleton = IdleAnimation::update_skeleton(
        &QuadrupedSmallSkeleton::default(),
        0.0,
        0.0,
        &mut rate,
        &attr,
    );
    let mut buf = [xindeler_anim::FigureBoneData::default(); xindeler_anim::MAX_BONE_COUNT];
    let computed = skeleton.compute_matrices(vek::Mat4::identity(), &mut buf, body);

    BoneRest {
        head: mat_to_transform(computed.head),
        chest: mat_to_transform(computed.chest),
        leg_fl: mat_to_transform(computed.leg_fl),
        leg_fr: mat_to_transform(computed.leg_fr),
        leg_bl: mat_to_transform(computed.leg_bl),
        leg_br: mat_to_transform(computed.leg_br),
        tail: mat_to_transform(computed.tail),
    }
}

/// The seven rest-pose bone transforms of a quadruped-small figure (Bevy
/// space, model-scaled).
pub struct BoneRest {
    pub head: Transform,
    pub chest: Transform,
    pub leg_fl: Transform,
    pub leg_fr: Transform,
    pub leg_bl: Transform,
    pub leg_br: Transform,
    pub tail: Transform,
}

impl BoneRest {
    /// The rest transform for a given bone.
    #[must_use]
    pub fn get(&self, bone: FigureBoneName) -> Transform {
        match bone {
            FigureBoneName::Head => self.head,
            FigureBoneName::Chest => self.chest,
            FigureBoneName::LegFl => self.leg_fl,
            FigureBoneName::LegFr => self.leg_fr,
            FigureBoneName::LegBl => self.leg_bl,
            FigureBoneName::LegBr => self.leg_br,
            FigureBoneName::Tail => self.tail,
        }
    }
}

/// A Veloren bone matrix (z-up, model-scaled) → a Bevy `Transform` (y-up).
///
/// The matrix is `R_zup2yup · M`: we apply the SAME `(x,y,z)→(x,z,-y)` basis
/// change the vertex converter bakes, as a similarity transform on the bone
/// matrix, so the part (whose vertices are ALSO converted the same way) lands
/// correctly. Concretely: `M_bevy = C · M_veloren · C⁻¹`, with `C` the z-up→
/// y-up rotation; then decompose to a `Transform`.
fn mat_to_transform(m: vek::Mat4<f32>) -> Transform {
    // C: (x, y, z) -> (x, z, -y). C⁻¹ = Cᵀ (pure rotation): (x, y, z) -> (x, -z,
    // y).
    let c = vek::Mat4::new(
        1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, -1.0, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    );
    let c_inv = vek::Mat4::new(
        1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, -1.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    );
    let bevy_mat = c * m * c_inv;
    let cols = bevy_mat.into_col_arrays();
    Transform::from_matrix(bevy::math::Mat4::from_cols_array_2d(&cols))
}

/// Assembles loaded parts into ready-to-spawn [`FigurePart`]s: meshes each part
/// (Mapper C7) and pairs it with its bone's rest transform (C13). Parts that
/// mesh to nothing are dropped.
#[must_use]
pub fn assemble(parts: &[LoadedPart], rest: &BoneRest) -> Vec<FigurePart> {
    parts
        .iter()
        .filter_map(|part| {
            let mesh = figure_part_to_bevy(part)?;
            Some(FigurePart {
                mesh,
                transform: rest.get(part.bone),
                name: bone_name(part.bone),
            })
        })
        .collect()
}

/// Stable debug label per bone.
fn bone_name(bone: FigureBoneName) -> &'static str {
    match bone {
        FigureBoneName::Head => "head",
        FigureBoneName::Chest => "chest",
        FigureBoneName::LegFl => "leg_fl",
        FigureBoneName::LegFr => "leg_fr",
        FigureBoneName::LegBl => "leg_bl",
        FigureBoneName::LegBr => "leg_br",
        FigureBoneName::Tail => "tail",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::comp::quadruped_small::{BodyType, Species};

    /// The manifest deser structs must parse the REAL Veloren RON (the `.vox`
    /// reference shape is `("model.name")` — a 1-tuple, NOT a bare string).
    /// This locks the `VoxSimple` shape against the actual frozen asset. Needs
    /// the asset tree; run locally with `XINDELER_ASSETS`/`VELOREN_ASSETS`.
    #[test]
    #[ignore = "reads the real quadruped_small manifests: needs the asset tree"]
    fn real_pig_manifests_parse() {
        let root = std::env::var("XINDELER_ASSETS")
            .or_else(|_| std::env::var("VELOREN_ASSETS"))
            .expect("set XINDELER_ASSETS or VELOREN_ASSETS to the assets dir");
        let central: QsCentralManifest = ron::de::from_str(
            &std::fs::read_to_string(format!(
                "{root}/voxygen/voxel/quadruped_small_central_manifest.ron"
            ))
            .unwrap(),
        )
        .expect("central manifest parses");
        let lateral: QsLateralManifest = ron::de::from_str(
            &std::fs::read_to_string(format!(
                "{root}/voxygen/voxel/quadruped_small_lateral_manifest.ron"
            ))
            .unwrap(),
        )
        .expect("lateral manifest parses");

        let specs = quadruped_small_part_specs(&central, &lateral, Species::Pig, BodyType::Female)
            .expect("pig has manifest entries");
        assert_eq!(specs.len(), 7, "head + chest + 4 feet + tail");
        // The head part points at a real `.vox` name (dotted Veloren spec).
        let head = specs
            .iter()
            .find(|s| s.bone == FigureBoneName::Head)
            .unwrap();
        assert!(
            head.vox_name.contains("pig"),
            "head vox name should reference the pig model, got {}",
            head.vox_name
        );
        // Left-front foot is flipped (reuses the right `.vox` mirrored).
        let lfl = specs
            .iter()
            .find(|s| s.bone == FigureBoneName::LegFl)
            .unwrap();
        assert!(lfl.flipped, "left-front leg is the mirrored right .vox");
    }

    /// A tiny 1-voxel `.vox` so the mesher tests need no assets.
    fn one_voxel_vox(color: [u8; 3]) -> DotVoxData {
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
            r: color[0],
            g: color[1],
            b: color[2],
            a: 255,
        };
        DotVoxData {
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
        }
    }

    /// A single filled voxel meshes to a cube: 6 faces × 4 verts = 24 verts,
    /// each carrying the palette colour in `ATTRIBUTE_COLOR`.
    #[test]
    fn single_voxel_meshes_to_coloured_cube() {
        let vox = one_voxel_vox([200, 100, 50]);
        let part = LoadedPart {
            vox: &vox,
            model_index: 0,
            offset: Vec3::zero(),
            flipped: false,
            bone: FigureBoneName::Chest,
        };
        let mesh = figure_part_to_bevy(&part).expect("one voxel meshes to a cube");
        let n = mesh.count_vertices();
        assert_eq!(n, 24, "a cube is 6 quads × 4 verts");
        assert!(
            mesh.attribute(BevyMesh::ATTRIBUTE_COLOR).is_some(),
            "figures carry per-voxel vertex colour (spec §4.2)"
        );
    }

    /// The rest pose is deterministic and non-degenerate: bones land at
    /// distinct, finite positions (a standing Pig's head is above and ahead of
    /// its feet). Needs no assets — pure `xindeler-anim`.
    #[test]
    fn pig_rest_pose_is_sane() {
        let rest = quadruped_small_bone_rest(Species::Pig, BodyType::Female);
        for (name, t) in [
            ("head", rest.head),
            ("chest", rest.chest),
            ("leg_fl", rest.leg_fl),
            ("tail", rest.tail),
        ] {
            assert!(
                t.translation.is_finite(),
                "{name} rest translation must be finite: {:?}",
                t.translation
            );
        }
        // Model-scaled: the whole figure is well under a couple of metres, so
        // no bone is metres away from the root.
        assert!(
            rest.head.translation.length() < 3.0,
            "model scale applied: head near the root, got {:?}",
            rest.head.translation
        );
        // Head is higher than the feet (y-up in Bevy).
        assert!(
            rest.head.translation.y > rest.leg_fl.translation.y,
            "head above feet: head.y={} leg.y={}",
            rest.head.translation.y,
            rest.leg_fl.translation.y
        );
    }

    /// `assemble` glues loaded parts to their rest bones and drops empty ones.
    #[test]
    fn assemble_places_parts_on_bones() {
        let vox = one_voxel_vox([120, 120, 120]);
        let rest = quadruped_small_bone_rest(Species::Pig, BodyType::Female);
        let parts = [
            (FigureBoneName::Head, "head"),
            (FigureBoneName::Chest, "chest"),
        ]
        .map(|(bone, _)| LoadedPart {
            vox: &vox,
            model_index: 0,
            offset: Vec3::zero(),
            flipped: false,
            bone,
        });
        let assembled = assemble(&parts, &rest);
        assert_eq!(assembled.len(), 2, "both non-empty parts assembled");
        // The two parts sit at DIFFERENT places (their bones differ).
        assert_ne!(
            assembled[0].transform.translation, assembled[1].transform.translation,
            "head and chest are on different bones"
        );
    }
}
