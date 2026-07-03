//! EM-3.2 acceptance tests: convert the EM-3.1 golden chunk to a
//! `bevy::mesh::Mesh` and check counts against the golden constants, AO range,
//! and attribute presence.
//!
//! The synthetic world builder + hash replicate `tests/mesher.rs` exactly
//! (same golden seed — keep in sync; the golden constants are shared).
#![cfg(feature = "convert")]

use common::{
    terrain::{Block, BlockKind, MapSizeLg, SpriteKind, TerrainChunk, TerrainChunkMeta},
    vol::WriteVol,
    volumes::vol_grid_2d::VolGrid2d,
};
use std::sync::Arc;
use vek::*;
use xindeler_render_voxel::{
    convert::{
        ATTRIBUTE_BLOCK_LAYER, ATTRIBUTE_VOXEL_AO, fluid_mesh_to_bevy, terrain_mesh_to_bevy,
    },
    mesh::terrain::generate_mesh,
};

use bevy::mesh::{Indices, Mesh as BevyMesh, VertexAttributeValues};

const CHUNK: i32 = 32;
const Z_SCAN: i32 = 24;
const MESH_KEY: Vec2<i32> = Vec2 { x: 1, y: 1 };

/// Same as `tests/mesher.rs::build_grid` (kept in sync — golden contract).
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

/// Same deterministic hash as `tests/mesher.rs` (the golden seed).
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
fn golden_chunk_converts_with_matching_counts() {
    // Golden constants from tests/mesher.rs (EM-3.1, 2026-07-03 @ 6f9afd978c).
    const GOLDEN_OPAQUE_VERTS: usize = 11016; // 2754 quads
    const GOLDEN_FLUID_VERTS: usize = 7920; // 1980 quads

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
    let (opaque, fluid, _shadow, (_bounds, atlas, atlas_size, _light, _glow, _alt, _sun)) =
        generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));
    assert_eq!(
        (opaque.len(), fluid.len()),
        (GOLDEN_OPAQUE_VERTS, GOLDEN_FLUID_VERTS)
    );

    let converted = terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, |kind| {
        // Rock (0x10) is the only solid in the golden chunk.
        assert_eq!(kind, BlockKind::Rock as u8);
        7
    });

    // Vertex count preserved; quad indices expanded 4 -> 6.
    assert_eq!(converted.count_vertices(), GOLDEN_OPAQUE_VERTS);
    let Some(Indices::U32(indices)) = converted.indices() else {
        panic!("terrain mesh must carry explicit U32 indices");
    };
    assert_eq!(indices.len(), GOLDEN_OPAQUE_VERTS / 4 * 6);
    assert!(indices.iter().all(|&i| (i as usize) < GOLDEN_OPAQUE_VERTS));
    // Upstream quad expansion pattern: [0,1,2, 2,1,3] per 4-vertex quad.
    assert_eq!(&indices[..6], &[0, 1, 2, 2, 1, 3]);
    assert_eq!(&indices[6..12], &[4, 5, 6, 6, 5, 7]);

    // All 5 attributes present with per-vertex cardinality.
    for attr in [
        BevyMesh::ATTRIBUTE_POSITION,
        BevyMesh::ATTRIBUTE_NORMAL,
        BevyMesh::ATTRIBUTE_UV_0,
        ATTRIBUTE_VOXEL_AO,
        ATTRIBUTE_BLOCK_LAYER,
    ] {
        let values = converted
            .attribute(attr.id)
            .unwrap_or_else(|| panic!("attribute {} missing", attr.name));
        assert_eq!(values.len(), GOLDEN_OPAQUE_VERTS, "{}", attr.name);
    }

    // AO invariant: every baked value in [0, 1]; the golden rubble has both
    // fully lit faces and occluded creases.
    let Some(VertexAttributeValues::Float32(ao)) = converted.attribute(ATTRIBUTE_VOXEL_AO.id)
    else {
        panic!("VoxelAo must be Float32");
    };
    assert!(ao.iter().all(|a| (0.0..=1.0).contains(a)));
    assert!(ao.contains(&1.0), "some vertex must be fully lit");
    assert!(ao.iter().any(|&a| a < 1.0), "some vertex must be occluded");

    // Layer mapping applied everywhere.
    let Some(VertexAttributeValues::Uint32(layers)) = converted.attribute(ATTRIBUTE_BLOCK_LAYER.id)
    else {
        panic!("BlockLayer must be Uint32");
    };
    assert!(layers.iter().all(|&l| l == 7));

    // Normals survived the z-up -> y-up rotation as signed unit axes.
    let Some(VertexAttributeValues::Float32x3(normals)) =
        converted.attribute(BevyMesh::ATTRIBUTE_NORMAL.id)
    else {
        panic!("normals must be Float32x3");
    };
    assert!(normals.iter().all(|n| {
        let ones = n.iter().filter(|c| c.abs() == 1.0).count();
        let zeros = n.iter().filter(|c| **c == 0.0).count();
        ones == 1 && zeros == 2
    }));

    // Fluid path: counts + attributes.
    let fluid_converted = fluid_mesh_to_bevy(&fluid);
    assert_eq!(fluid_converted.count_vertices(), GOLDEN_FLUID_VERTS);
    let Some(Indices::U32(fluid_indices)) = fluid_converted.indices() else {
        panic!("fluid mesh must carry explicit U32 indices");
    };
    assert_eq!(fluid_indices.len(), GOLDEN_FLUID_VERTS / 4 * 6);
    assert!(
        fluid_converted
            .attribute(BevyMesh::ATTRIBUTE_UV_0.id)
            .is_some()
    );
}

#[test]
fn single_block_conversion_is_fully_lit_and_axis_mapped() {
    let target = Vec3::new(48, 48, 8);
    let (grid, range) = build_grid(|wpos| (wpos == target).then(rock));
    let (opaque, _fluid, _shadow, (_bounds, atlas, atlas_size, ..)) =
        generate_mesh(&grid, (range, Vec2::new(4096, 4096), ()));
    assert_eq!(opaque.len(), 24);

    let converted = terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, |_| 0);
    assert_eq!(converted.count_vertices(), 24);
    let Some(Indices::U32(indices)) = converted.indices() else {
        panic!("expected U32 indices");
    };
    assert_eq!(indices.len(), 36, "6 faces x 2 triangles x 3 indices");

    // A lone block in open sky: every vertex texel is in full sun -> AO 1.0.
    let Some(VertexAttributeValues::Float32(ao)) = converted.attribute(ATTRIBUTE_VOXEL_AO.id)
    else {
        panic!("VoxelAo must be Float32");
    };
    assert!(ao.iter().all(|&a| a == 1.0), "isolated block is unoccluded");

    // z-up -> y-up: the mesher's +z (up) face must come out as +Y in Bevy,
    // and the mesher's y extent must land on (negated) Bevy z.
    let Some(VertexAttributeValues::Float32x3(normals)) =
        converted.attribute(BevyMesh::ATTRIBUTE_NORMAL.id)
    else {
        panic!("normals must be Float32x3");
    };
    assert!(
        normals.iter().any(|n| n[1] == 1.0),
        "an up (+Y) face exists"
    );
    assert!(normals.iter().all(|n| n.iter().any(|c| c.abs() == 1.0)));
}
