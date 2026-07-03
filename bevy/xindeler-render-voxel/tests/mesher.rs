//! EM-3.1 acceptance tests for the lift-copied greedy mesher (Mapper C5-C7).
//!
//! These run against the REAL meshing API (`generate_mesh` over
//! `VolGrid2d<TerrainChunk>`, exactly the volume type voxygen feeds it) and
//! require no assets. The synthetic worlds replicate the range computation of
//! the upstream caller (voxygen/src/scene/terrain/mod.rs:1064-1098): mesh
//! chunk (1,1) with all 8 neighbours present, xy range = chunk ± 1 border,
//! z range = [min_z - 2, max_z + 2].

use common::{
    figure::{Cell, CellSurface},
    terrain::{Block, BlockKind, MapSizeLg, SpriteKind, TerrainChunk, TerrainChunkMeta},
    vol::WriteVol,
    volumes::{dyna::Dyna, vol_grid_2d::VolGrid2d},
};
use std::sync::Arc;
use vek::*;
use xindeler_render_voxel::mesh::{
    greedy::{self, GreedyMesh},
    mesh::Mesh,
    segment::generate_mesh_base_vol_figure,
    terrain::generate_mesh,
    vertex::{FigureSpriteAtlasData, TerrainVertex, Vertex},
};

const CHUNK: i32 = 32;
/// z range scanned when building synthetic chunks.
const Z_SCAN: i32 = 24;
/// The meshed chunk key — its world xy span is [32, 64).
const MESH_KEY: Vec2<i32> = Vec2 { x: 1, y: 1 };

/// Build a 3×3 grid of chunks around `MESH_KEY` from a world-position → block
/// function, plus the mesh `range` computed the way voxygen's caller does.
fn build_grid(fill: impl Fn(Vec3<i32>) -> Option<Block>) -> (VolGrid2d<TerrainChunk>, Aabb<i32>) {
    let map_size_lg = MapSizeLg::new(Vec2::new(2, 2)).expect("valid test map size");
    let default = Arc::new(TerrainChunk::new(
        0,
        Block::empty(),
        Block::empty(),
        TerrainChunkMeta::void(),
    ));
    let mut grid = VolGrid2d::new(map_size_lg, default).expect("chunk size is a power of two");

    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;
    for kx in 0..=2 {
        for ky in 0..=2 {
            let mut chunk =
                TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
            for lx in 0..CHUNK {
                for ly in 0..CHUNK {
                    for z in 0..Z_SCAN {
                        let wpos = Vec3::new(kx * CHUNK + lx, ky * CHUNK + ly, z);
                        if let Some(block) = fill(wpos) {
                            chunk
                                .set(Vec3::new(lx, ly, z), block)
                                .expect("in-bounds chunk write");
                        }
                    }
                }
            }
            min_z = min_z.min(chunk.get_min_z());
            max_z = max_z.max(chunk.get_max_z());
            grid.insert(Vec2::new(kx, ky), Arc::new(chunk));
        }
    }

    // Replicates the upstream mesh_worker range for chunk MESH_KEY.
    let range = Aabb {
        min: Vec3::new(MESH_KEY.x * CHUNK - 1, MESH_KEY.y * CHUNK - 1, min_z - 2),
        max: Vec3::new(
            (MESH_KEY.x + 1) * CHUNK + 1,
            (MESH_KEY.y + 1) * CHUNK + 1,
            max_z + 2,
        ),
    };
    (grid, range)
}

fn rock() -> Block { Block::new(BlockKind::Rock, Rgb::new(120, 100, 90)) }

/// Split a quads-indexed mesh into its quads (push_quad emits [b, c, a, d]).
fn quads(mesh: &Mesh<TerrainVertex>) -> Vec<[TerrainVertex; 4]> {
    assert!(
        TerrainVertex::QUADS_INDEX.is_some(),
        "terrain vertices are quad-indexed: 4 vertices per quad"
    );
    assert_eq!(mesh.len() % 4, 0);
    mesh.vertices().as_chunks::<4>().0.to_vec()
}

#[test]
fn empty_chunk_produces_no_vertices() {
    let (grid, range) = build_grid(|_| None);
    let (opaque, fluid, shadow, (_bounds, _atlas, _atlas_size, _light, _glow, alt_indices, _sun)) =
        generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));
    assert_eq!(opaque.len(), 0, "air-only chunk must mesh to 0 vertices");
    assert_eq!(fluid.len(), 0);
    assert_eq!(shadow.len(), 0);
    assert_eq!(alt_indices.deep_end, 0);
    assert_eq!(alt_indices.underground_end, 0);
}

#[test]
fn single_block_produces_exactly_six_faces() {
    let target = Vec3::new(48, 48, 8);
    let (grid, range) = build_grid(|wpos| (wpos == target).then(rock));
    let (opaque, fluid, _shadow, _extra) = generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));

    let quads = quads(&opaque);
    assert_eq!(
        quads.len(),
        6,
        "an isolated solid block has exactly 6 exposed faces"
    );
    assert_eq!(opaque.len(), 24, "6 quads × 4 vertices (quad-indexed)");
    assert_eq!(fluid.len(), 0);

    // One quad per axis direction; all 4 vertices of a quad share its normal.
    let mut normals: Vec<Vec3<f32>> = quads
        .iter()
        .map(|q| {
            assert!(q.iter().all(|v| v.norm == q[0].norm));
            q[0].norm
        })
        .collect();
    normals.sort_by(|a, b| a.as_slice().partial_cmp(b.as_slice()).unwrap());
    let mut expected = [
        Vec3::unit_x(),
        -Vec3::unit_x(),
        Vec3::unit_y(),
        -Vec3::unit_y(),
        Vec3::unit_z(),
        -Vec3::unit_z(),
    ];
    expected.sort_by(|a: &Vec3<f32>, b| a.as_slice().partial_cmp(b.as_slice()).unwrap());
    assert_eq!(normals, expected);
}

#[test]
fn flat_floor_fuses_greedily() {
    // 16×16×1 solid slab at z == 8, interior to the meshed chunk.
    const N: i32 = 16;
    let (grid, range) = build_grid(|w| {
        ((40..40 + N).contains(&w.x) && (40..40 + N).contains(&w.y) && w.z == 8).then(rock)
    });
    let (opaque, fluid, _shadow, _extra) = generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));

    // Naively the slab exposes N*N top + N*N bottom + 4*N side faces = 576.
    // Greedy meshing must fuse each maximal coplanar same-meta rectangle into
    // ONE quad: 1 top (16×16) + 1 bottom (16×16) + 4 sides (16×1) = 6 quads.
    let naive_faces = (N * N * 2 + 4 * N) as usize;
    let quads = quads(&opaque);
    assert_eq!(
        quads.len(),
        6,
        "greedy fusion: {} naive faces must collapse to 6 quads",
        naive_faces
    );
    assert_eq!(opaque.len(), 24);
    assert_eq!(fluid.len(), 0);
    assert!(quads.len() * 40 < (N * N) as usize, "quads << N² (6 ≪ 256)");
}

#[test]
fn checkerboard_does_not_fuse() {
    // 4×4×4 3D checkerboard: no two solid blocks are face-adjacent, and no two
    // exposed faces in the same plane are edge-adjacent, so NOTHING can merge.
    let cube_min = Vec3::new(44, 44, 8);
    let (grid, range) = build_grid(|w| {
        let d = w - cube_min;
        ((0..4).contains(&d.x)
            && (0..4).contains(&d.y)
            && (0..4).contains(&d.z)
            && (w.x + w.y + w.z) % 2 == 0)
            .then(rock)
    });
    let (opaque, fluid, _shadow, _extra) = generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));

    // 32 solid blocks (half of 64), all 6 neighbours of each are air, and no
    // solid-solid contact exists → 192 block-air interfaces, one quad each.
    assert_eq!(quads(&opaque).len(), 192, "checkerboard must not fuse");
    assert_eq!(opaque.len(), 768);
    assert_eq!(fluid.len(), 0);
}

/// Deterministic integer hash (no external RNG dep) — the "seed" of the golden
/// chunk. Do not change without regenerating the golden constants.
fn hash(p: Vec3<i32>) -> u32 {
    let mut h = (p.x as u32).wrapping_mul(0x9E37_79B9)
        ^ (p.y as u32).wrapping_mul(0x85EB_CA6B)
        ^ (p.z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h
}

#[test]
fn golden_procedural_chunk_counts() {
    // Deterministic pseudo-random rubble + water pockets, fully inside the
    // meshed chunk (world xy 40..56, z 4..16). Exercises both the opaque and
    // the fluid meshing paths.
    let (grid, range) = build_grid(|w| {
        if (40..56).contains(&w.x) && (40..56).contains(&w.y) && (4..16).contains(&w.z) {
            match hash(w) % 4 {
                0 => Some(rock()),
                1 => Some(Block::water(SpriteKind::Empty)),
                _ => None,
            }
        } else {
            None
        }
    });
    let (opaque, fluid, _shadow, (bounds, _atlas, atlas_size, _light, _glow, alt_indices, _sun)) =
        generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));

    // golden: primera salida verificada del port (2026-07-03, HEAD 6f9afd978c).
    // Any change to these numbers = a behavioural change in the mesher; either
    // an upstream fix was ported on purpose (regenerate + note in Mapper §E)
    // or something broke.
    const GOLDEN_OPAQUE_VERTS: usize = 11016; // 2754 quads
    const GOLDEN_FLUID_VERTS: usize = 7920; // 1980 quads
    const GOLDEN_ATLAS_SIZE: Vec2<u16> = Vec2 { x: 256, y: 256 };
    assert_eq!(
        (opaque.len(), fluid.len(), atlas_size),
        (GOLDEN_OPAQUE_VERTS, GOLDEN_FLUID_VERTS, GOLDEN_ATLAS_SIZE)
    );
    assert_eq!(opaque.len() % 4, 0);
    assert_eq!(fluid.len() % 4, 0);

    // Chunk meta alt() == 0 → everything counts as "surface" for culling.
    assert_eq!(alt_indices.deep_end, 0);
    assert_eq!(alt_indices.underground_end, 0);

    // Bounds sanity: mesh-relative AABB spanning the greedy region.
    assert!(bounds.min.z <= 4.0 && bounds.max.z >= 16.0);
}

#[test]
fn ao_darkens_corner_against_wall() {
    // 8×8 floor at z == 8 (world xy 44..52) with a 3-high wall along its west
    // edge (x == 44, y 44..52, z 9..12). Hand-verifiable AO/light case:
    //
    // Floor-top texels sample the 4 blocks around each texel corner at z == 9
    // (see greedy.rs draw_texels). AO callback is 1.0 for non-opaque, 0.0 for
    // opaque (terrain.rs get_ao ∈ [0, 1]); the average is thresholded > 0.7.
    // - texel column adjacent to the wall, interior rows: 2 of 4 samples are wall →
    //   ao 0.5 → dark (false); light (1+0+1+0)/4 = 0.5 → ⌊0.5·31.5⌋ = 15
    // - same column, end rows (v = 0, v = 8): only 1 wall sample → ao 0.75 → lit;
    //   light 0.75 → ⌊0.75·31.5⌋ = 23 (partially darkened)
    // - every other texel: 4 air samples → ao 1.0 → lit; full sun light 31
    let (grid, range) = build_grid(|w| {
        let floor = (44..52).contains(&w.x) && (44..52).contains(&w.y) && w.z == 8;
        let wall = w.x == 44 && (44..52).contains(&w.y) && (9..12).contains(&w.z);
        (floor || wall).then(rock)
    });
    let (opaque, _fluid, _shadow, (_bounds, atlas, atlas_size, _light, _glow, _alt, _sun)) =
        generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));

    // The floor's top face fuses into a single 7×8 quad (the column under the
    // wall has no exposed top face): find it by normal +z and atlas extent.
    let quads = quads(&opaque);
    let top: Vec<&[TerrainVertex; 4]> = quads
        .iter()
        .filter(|q| {
            let extent = q.iter().fold(Vec2::new(0u16, 0u16), |m, v| {
                Vec2::new(m.x.max(v.atlas_pos.x), m.y.max(v.atlas_pos.y))
            }) - q.iter().fold(Vec2::new(u16::MAX, u16::MAX), |m, v| {
                Vec2::new(m.x.min(v.atlas_pos.x), m.y.min(v.atlas_pos.y))
            });
            q[0].norm == Vec3::unit_z() && extent == Vec2::new(7, 8)
        })
        .collect();
    assert_eq!(top.len(), 1, "exactly one 7×8 floor-top quad expected");
    let rect_min = top[0].iter().fold(Vec2::new(u16::MAX, u16::MAX), |m, v| {
        Vec2::new(m.x.min(v.atlas_pos.x), m.y.min(v.atlas_pos.y))
    });

    // The allocated atlas rect is (dim + 1) texels: 8 × 9.
    let (width, height) = (8u16, 9u16);
    let mut dark = Vec::new();
    let mut partial = 0usize;
    let mut lit = 0usize;
    for v in 0..height {
        for u in 0..width {
            let idx = usize::from(rect_min.y + v) * usize::from(atlas_size.x)
                + usize::from(rect_min.x + u);
            let texel = atlas.col_lights[idx];
            // "AO in [0, 1]" invariant, quantized: light in [0, 31], ao bool.
            assert!(texel.light <= 31);
            match (texel.ao, texel.light) {
                (false, 15) => dark.push((u, v)),
                (true, 23) => partial += 1,
                (true, 31) => lit += 1,
                other => panic!("unexpected floor-top texel {:?} at ({}, {})", other, u, v),
            }
        }
    }
    // 7 fully darkened corner texels hugging the wall, 2 partially occluded
    // at the wall's ends, 63 in full sun. 7 + 2 + 63 = 72 = 8×9.
    assert_eq!(dark.len(), 7, "wall-adjacent texels must be AO-darkened");
    assert_eq!(partial, 2);
    assert_eq!(lit, 63);
    assert!(
        dark.iter().all(|&(u, _)| u == 0),
        "darkened texels must all lie on the wall-side column"
    );
}

#[test]
fn figure_single_cell_produces_six_faces() {
    // segment.rs path (C7): a lone filled voxel in a 3×3×3 figure volume.
    let vol: Dyna<Cell, ()> = Dyna::from_fn(Vec3::new(3, 3, 3), (), |p| {
        if p == Vec3::new(1, 1, 1) {
            Cell::filled(Rgb::new(255, 0, 0), CellSurface::Matte)
        } else {
            Cell::empty()
        }
    });
    let mut greedy =
        GreedyMesh::<FigureSpriteAtlasData>::new(Vec2::new(512, 512), greedy::general_config());
    let mut opaque = Mesh::new();
    let (_, _, _, bounds) = generate_mesh_base_vol_figure(
        vol,
        (&mut greedy, &mut opaque, Vec3::zero(), Vec3::one(), 3),
    );
    let (atlas, atlas_size) = greedy.finalize();

    assert_eq!(opaque.len(), 24, "6 faces × 4 vertices");
    assert!(opaque.vertices().iter().all(|v| v.bone_idx == 3));
    assert!(bounds.min.x <= bounds.max.x);
    assert!(!atlas.col_lights.is_empty());
    assert!(atlas_size.x >= 1 && atlas_size.y >= 1);
}
