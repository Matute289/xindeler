//! EM-3.2 — conversion `mesh::Mesh<TerrainVertex>` (+ atlas) →
//! [`bevy::mesh::Mesh`].
//!
//! This is a CONVERSION, not a re-derivation (spec §4.1): the greedy mesher's
//! output stays byte-similar to upstream; everything Bevy-specific happens
//! here, behind the `convert` cargo feature so the mesher core keeps zero
//! bevy-render machinery.
//!
//! ## Decisions (documented per the EM-3.2 acceptance)
//!
//! **Coordinate map (Veloren z-up → Bevy y-up).** The mesher emits
//! right-handed z-up positions/normals; Bevy is right-handed y-up. We bake the
//! pure rotation `(x, y, z) → (x, z, -y)` into positions AND normals at
//! conversion time (a rotation preserves handedness, so triangle winding and
//! front-face orientation survive). Chunk entity `Transform`s therefore live
//! fully in Bevy space; they must stay integer block translations (no
//! rotation/scale) or the world-space texture tiling below breaks.
//!
//! **UV_0 = world-space tiling per quad (spec §4.2).** Each vertex gets the
//! planar projection of its (converted, mesh-local) position onto its face
//! plane, so one voxel = exactly one UV unit and a repeat-addressed texture
//! tiles seamlessly across greedy-merged quads of any size:
//! - dominant `|n.y|` (floors/ceilings): `uv = (x, z)`
//! - dominant `|n.x|` (east/west walls):  `uv = (z, y)`
//! - otherwise    (north/south walls):    `uv = (x, y)`
//!
//! Interpolation across a planar quad is linear in position, so per-vertex
//! UVs reproduce the exact per-fragment planar mapping. The material's WGSL
//! derives its tangent basis from the SAME table (see `material/voxel.wgsl`)
//! — keep the two in sync. Because chunk translations are integer blocks and
//! the tiling period is 1.0, adjacent chunks stay texture-continuous.
//!
//! **`ATTRIBUTE_VOXEL_AO` = baked texel sample, CPU-side (v1).** Upstream
//! samples the ColLight atlas per-fragment; v1 instead samples the atlas ONCE
//! per vertex (at the vertex's `atlas_pos`) while building the mesh and bakes
//! `ColLight::light / 31.0` — the mesher's BFS sky-visibility × the classic
//! 3-neighbour corner-occlusion (opaque corner samples contribute 0, see
//! `terrain.rs::get_ao`/`get_light` and the hand-verified AO histogram in
//! `tests/mesher.rs::ao_darkens_corner_against_wall`). The value is in
//! `[0, 1]` and is multiplied into INDIRECT light only (spec §4.3) by
//! `VoxelMaterialExt`. Known v1 loss, accepted + documented: interior texels
//! of a large greedy quad are not sampled (only its 4 corners), so a dark
//! crease strictly inside one huge merged rect is missed — terrain-ish
//! content splits quads at exactly those creases, so corners dominate in
//! practice. The thresholded `ColLight::ao` bool, `glow` (→ emissive) and
//! per-block `col` tint are NOT consumed: EM-3.4's block palette decided the
//! policy — emissive and colour are PER LAYER (palette-baked into the
//! texture arrays), not per voxel; the atlas `glow`/`col` channels stay
//! unread (upstream diff surface only).
//!
//! **`ATTRIBUTE_BLOCK_LAYER` = texture-array layer from the kinds atlas.**
//! The vertex itself does not carry a block kind, but its `atlas_pos` indexes
//! `TerrainAtlasData::kinds` (one `BlockKind as u8` per texel), so we sample
//! that and map it through a caller-provided `kind → layer` function — the
//! mapping is DATA the caller owns (EM-3.4: `BlockPalette::layer_lut` from
//! `block_palette.ron`, snapshotted per EM-3.5 mesh task).
//! Same v1 caveat as AO: greedy merges across kind boundaries, so a mixed
//! quad takes its corner kinds (flat-interpolated in the shader).
//!
//! **Indices.** Terrain/fluid vertices are quad-indexed upstream
//! (`Vertex::QUADS_INDEX`, 4 verts/quad; `push_quad` pushes `[b, c, a, d]`
//! and upstream's shared index buffer draws `0,1,2, 2,1,3` per quad — see
//! `mesh.rs::push_quad`). Bevy has no shared quad index buffer, so we emit
//! the equivalent explicit `Indices::U32` triangle list.
//!
//! **`RenderAssetUsages::RENDER_WORLD`**: the CPU copy is dropped after
//! upload (VRAM discipline, spec §4.1 — matters for dimension GC).

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh as BevyMesh, MeshVertexAttribute, PrimitiveTopology, VertexFormat},
};
use vek::*;

use crate::mesh::{
    mesh::Mesh,
    vertex::{FluidVertex, IndexFormat, TerrainAtlasData, TerrainVertex, Vertex},
};

/// Per-vertex ambient-occlusion factor in `[0, 1]` (1 = unoccluded), baked
/// from the mesher's ColLight atlas. Consumed by `VoxelMaterialExt`, which
/// multiplies it into indirect light only (spec §4.3). Id from the spec
/// (stable — must never collide with another attribute id).
pub const ATTRIBUTE_VOXEL_AO: MeshVertexAttribute =
    MeshVertexAttribute::new("VoxelAo", 988_540_917, VertexFormat::Float32);

/// Per-vertex texture-array layer index (albedo/normal/MRA arrays share it),
/// flat-interpolated in the shader. Id from the spec (see above).
pub const ATTRIBUTE_BLOCK_LAYER: MeshVertexAttribute =
    MeshVertexAttribute::new("BlockLayer", 988_540_918, VertexFormat::Uint32);

/// Per-vertex river-flow velocity (xz plane, Bevy space) baked from the fluid
/// mesher's `FluidVertex::river_velocity` (EM-3.9). The interim stock water
/// `StandardMaterial` does not consume it, but carrying it keeps the data
/// intact for the dedicated water shader (EM-3.9b: scroll the surface / spawn
/// wakes along the flow). Distinct id so it can never collide with the terrain
/// attributes above.
pub const ATTRIBUTE_RIVER_VELOCITY: MeshVertexAttribute =
    MeshVertexAttribute::new("RiverVelocity", 988_540_919, VertexFormat::Float32x2);

/// Veloren z-up → Bevy y-up (pure rotation: winding preserved).
#[inline]
fn to_bevy(v: Vec3<f32>) -> [f32; 3] { [v.x, v.z, -v.y] }

/// Planar world-space UV projection for an axis-aligned face (see module
/// docs; MUST stay in sync with the tangent-basis table in
/// `material/voxel.wgsl`). `pos`/`norm` are already Bevy-space.
#[inline]
fn planar_uv(pos: [f32; 3], norm: [f32; 3]) -> [f32; 2] {
    let [x, y, z] = pos;
    let [nx, ny, nz] = norm.map(f32::abs);
    if ny >= nx && ny >= nz {
        [x, z]
    } else if nx >= nz {
        [z, y]
    } else {
        [x, y]
    }
}

/// Explicit triangle-list indices equivalent to upstream's shared quad index
/// buffer (`0,1,2, 2,1,3` per 4-vertex quad).
fn quad_indices(vert_count: usize) -> Vec<u32> {
    debug_assert_eq!(vert_count % 4, 0, "quad-indexed mesh: 4 verts per quad");
    let mut indices = Vec::with_capacity(vert_count / 4 * 6);
    for quad in 0..(vert_count / 4) as u32 {
        let base = quad * 4;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }
    indices
}

/// Converts the greedy mesher's opaque terrain output (+ its ColLight/kinds
/// atlas) into a renderable [`bevy::mesh::Mesh`] carrying
/// `POSITION`/`NORMAL`/`UV_0`/[`ATTRIBUTE_VOXEL_AO`]/[`ATTRIBUTE_BLOCK_LAYER`].
///
/// `kind_to_layer` maps a `BlockKind as u8` (from the kinds atlas) to a
/// texture-array layer — pass the block-palette mapping
/// (`BlockPalette::layer_lut`, EM-3.4).
///
/// # Panics
/// Debug-panics if the mesh is not quad-indexed (terrain always is) or an
/// `atlas_pos` lies outside `atlas_size` (mesher contract).
pub fn terrain_mesh_to_bevy(
    mesh: &Mesh<TerrainVertex>,
    atlas: &TerrainAtlasData,
    atlas_size: Vec2<u16>,
    mut kind_to_layer: impl FnMut(u8) -> u32,
) -> BevyMesh {
    debug_assert_eq!(TerrainVertex::QUADS_INDEX, Some(IndexFormat::Uint32));

    let n = mesh.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut aos = Vec::with_capacity(n);
    let mut layers = Vec::with_capacity(n);

    // Row-major texel index, exactly how the mesher writes the atlas
    // (`greedy.rs::draw_texels`; also asserted by the EM-3.1 AO test).
    let texel = |atlas_pos: Vec2<u16>| {
        usize::from(atlas_pos.y) * usize::from(atlas_size.x) + usize::from(atlas_pos.x)
    };

    for v in mesh.vertices() {
        let pos = to_bevy(v.pos);
        let norm = to_bevy(v.norm);
        positions.push(pos);
        normals.push(norm);
        uvs.push(planar_uv(pos, norm));

        let idx = texel(v.atlas_pos);
        let col_light = &atlas.col_lights[idx];
        // Baked sky-visibility × corner occlusion, quantised 0..=31 by
        // `make_col_light` (see module docs for what is deliberately unused).
        aos.push(f32::from(col_light.light.min(31)) / 31.0);
        layers.push(kind_to_layer(atlas.kinds[idx]));
    }

    let mut out = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(BevyMesh::ATTRIBUTE_UV_0, uvs);
    out.insert_attribute(ATTRIBUTE_VOXEL_AO, aos);
    out.insert_attribute(ATTRIBUTE_BLOCK_LAYER, layers);
    out.insert_indices(Indices::U32(quad_indices(n)));
    out
}

/// Converts the fluid (water) mesh: `POSITION`/`NORMAL`/`UV_0` +
/// [`ATTRIBUTE_RIVER_VELOCITY`] (EM-3.9). Fluids render with `WaterMaterial`
/// (`material::water`, EM-3.9b) — a translucent blue extended material (the
/// palette's Water entry) whose vertex shader reads the per-vertex river
/// velocity carried here to advect the surface UVs (EM-3.9 only carried the
/// attribute; EM-3.9b's dedicated water shader is what consumes it).
/// `river_velocity` is in the Veloren xy plane; z-up→y-up maps that to Bevy's
/// xz ground plane as `(vx, -vy)` (the same rotation `to_bevy` applies).
pub fn fluid_mesh_to_bevy(mesh: &Mesh<FluidVertex>) -> BevyMesh {
    debug_assert!(FluidVertex::QUADS_INDEX.is_some());

    let n = mesh.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut velocities = Vec::with_capacity(n);
    for v in mesh.vertices() {
        let pos = to_bevy(v.pos);
        let norm = to_bevy(v.norm);
        positions.push(pos);
        normals.push(norm);
        uvs.push(planar_uv(pos, norm));
        // Veloren xy flow → Bevy ground-plane xz: (vx, vy) → (vx, -vy).
        velocities.push([v.river_velocity.x, -v.river_velocity.y]);
    }

    let mut out = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(BevyMesh::ATTRIBUTE_UV_0, uvs);
    out.insert_attribute(ATTRIBUTE_RIVER_VELOCITY, velocities);
    out.insert_indices(Indices::U32(quad_indices(n)));
    out
}
