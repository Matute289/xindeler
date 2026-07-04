//! EM-3.4/3.5 demo: a 5×5 grid of REAL synthetic chunks meshed through the
//! async chunk pipeline ([`xindeler_render_voxel::pipeline`]) and textured
//! by the data-driven block palette (`assets/xindeler/render/
//! block_palette.ron`) — the EM-3.3 single-chunk diorama, grown up:
//!
//! - the terrain generator is a CONTINUOUS function of WORLD coordinates
//!   (hills, an east terrace band, water pools, emissive crystal veins), so the
//!   world-space UV tiling must read seamless across chunk borders — that
//!   continuity is exactly what the smoke screenshot verifies,
//! - meshing runs on `AsyncComputeTaskPool` with budgeted uploads
//!   ([`ChunkUploadBudget`](xindeler_render_voxel::pipeline::ChunkUploadBudget),
//!   default 2/frame) — nothing meshes on the main thread,
//! - the block-kind → layer mapping, PBR params, texture arrays (with real mip
//!   chains) and material knobs all come from the palette asset; editing it
//!   while the client runs rebuilds the arrays IN PLACE and re-meshes every
//!   chunk (the change pops — palettes are content edits, not ambience
//!   transitions).

use std::sync::Arc;

use bevy::prelude::*;
use common::{
    terrain::{Block, BlockKind, MapSizeLg, SpriteKind, TerrainChunk, TerrainChunkMeta},
    vol::WriteVol,
    volumes::vol_grid_2d::VolGrid2d,
};
use vek::{Rgb, Vec2 as VVec2, Vec3 as VVec3};
use xindeler_render_voxel::{
    material::{VoxelMaterial, VoxelMaterialExt},
    palette::{BlockPalette, PALETTE_ASSET_PATH, build_block_texture_arrays},
    pipeline::{ChunkLayerMap, ChunkMaterials, ChunkMeshQueue, ChunkVolume, ChunkVolumeProvider},
};

pub struct VoxelDemoPlugin;

impl Plugin for VoxelDemoPlugin {
    fn build(&self, app: &mut App) {
        // Sky ambient lives in the ATMOSPHERE module (AtmospherePlugin) — not
        // here — so deleting this demo never silently changes scene lighting.
        app.add_systems(Startup, setup_voxel_demo)
            .add_systems(Update, apply_block_palette);
    }
}

// ---------------------------------------------------------------------------
// Synthetic terrain (deterministic, world-coordinate continuous — no RNG)
// ---------------------------------------------------------------------------

const CHUNK: i32 = 32;
/// Meshed chunk keys: `1..=GRID` on both axes (world xy ∈ `[32, 192)`); the
/// surrounding ring (keys 0 and `GRID + 1`) exists in the volume but stays
/// EMPTY of blocks, so the mesher closes the outer walls of the slab — a
/// self-contained diorama whose INTERIOR chunk borders are seamless.
const GRID: i32 = 5;
/// World x where the terrace band starts (mid-chunk 3 — deliberately NOT on
/// a chunk border, so a step there is a real generator feature and never
/// mistaken for a chunk seam).
const TERRACE_START_X: i32 = 112;
/// Water surface height for the valley pools.
const WATER_LEVEL: i32 = 4;
const MAX_HEIGHT: i32 = 15;

/// Deterministic integer hash (same scheme as the EM-3.1 golden tests).
fn hash(p: VVec3<i32>) -> u32 {
    let mut h = (p.x as u32).wrapping_mul(0x9E37_79B9)
        ^ (p.y as u32).wrapping_mul(0x85EB_CA6B)
        ^ (p.z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h
}

/// Terrain-ish heightfield over WORLD (wx, wy): smooth multi-chunk hills,
/// terraced into 2-block steps east of [`TERRACE_START_X`] (crisp AO
/// creases). Being a pure function of world coordinates is what makes the
/// 5×5 grid continuous across chunk borders.
#[expect(clippy::cast_precision_loss, reason = "world coords < 256")]
fn height(wx: i32, wy: i32) -> i32 {
    let (x, y) = (wx as f32, wy as f32);
    // Lower frequencies than the old single-chunk diorama so hills SPAN
    // chunks (borders cut through slopes — any seam would be obvious).
    let smooth =
        7.0 + 3.5 * (x * 0.09).sin() + 3.0 * (y * 0.07).cos() + 1.8 * ((x + y) * 0.045).sin();
    #[expect(clippy::cast_possible_truncation, reason = "clamped small range")]
    let mut h = smooth.round() as i32;
    if wx >= TERRACE_START_X {
        // Terraces: quantise to 2-block steps.
        h = (h / 2) * 2;
    }
    h.clamp(1, MAX_HEIGHT)
}

/// Block for world position `wpos` (a pure world-coordinate function; the
/// grid builder samples it only inside the meshed 5×5 window so the border
/// ring stays empty).
fn block_at(wpos: VVec3<i32>) -> Option<Block> {
    if wpos.z < 0 {
        return None;
    }
    let h = height(wpos.x, wpos.y);
    if wpos.z < h {
        // Solid column: Earth topsoil, Rock body, sparse crystal veins.
        let kind = if wpos.z == h - 1 && h > WATER_LEVEL {
            BlockKind::Earth
        } else if hash(wpos).is_multiple_of(53) {
            BlockKind::GlowingRock
        } else {
            BlockKind::Rock
        };
        Some(Block::new(kind, Rgb::new(120, 100, 90)))
    } else if wpos.z <= WATER_LEVEL {
        // Valley pools (exercise the fluid meshing + conversion path).
        Some(Block::water(SpriteKind::Empty))
    } else {
        None
    }
}

/// Builds the shared volume: populated chunks for keys `1..=GRID`, empty
/// chunks for the surrounding ring (mesher closes the outer walls).
fn build_world() -> Arc<VolGrid2d<TerrainChunk>> {
    // 8×8 chunk map fits keys 0..=GRID+1.
    let map_size_lg = MapSizeLg::new(VVec2::new(3, 3)).expect("valid demo map size");
    let default = Arc::new(TerrainChunk::new(
        0,
        Block::empty(),
        Block::empty(),
        TerrainChunkMeta::void(),
    ));
    let mut grid = VolGrid2d::new(map_size_lg, default).expect("chunk size is a power of two");

    for kx in 0..=GRID + 1 {
        for ky in 0..=GRID + 1 {
            let mut chunk =
                TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
            let populated = (1..=GRID).contains(&kx) && (1..=GRID).contains(&ky);
            if populated {
                for lx in 0..CHUNK {
                    for ly in 0..CHUNK {
                        for z in 0..=MAX_HEIGHT {
                            let wpos = VVec3::new(kx * CHUNK + lx, ky * CHUNK + ly, z);
                            if let Some(block) = block_at(wpos) {
                                chunk
                                    .set(VVec3::new(lx, ly, z), block)
                                    .expect("in-bounds chunk write");
                            }
                        }
                    }
                }
            }
            grid.insert(VVec2::new(kx, ky), Arc::new(chunk));
        }
    }
    Arc::new(grid)
}

/// Marks every demo chunk dirty (initial build + palette hot reload — the
/// layer LUT is baked per vertex, so palette edits require re-meshing).
fn mark_all_chunks_dirty(queue: &mut ChunkMeshQueue) {
    for kx in 1..=GRID {
        for ky in 1..=GRID {
            queue.mark_dirty(VVec2::new(kx, ky));
        }
    }
}

// ---------------------------------------------------------------------------
// Setup: volume provider + palette load
// ---------------------------------------------------------------------------

/// Strong handle keeping the palette (and its file watch) alive.
#[derive(Resource)]
struct DemoPaletteHandle(Handle<BlockPalette>);

fn setup_voxel_demo(mut commands: Commands, asset_server: Res<AssetServer>) {
    let world = build_world();
    // Serve only the meshed window; z bounds are the generator's globals.
    commands.insert_resource(ChunkVolumeProvider::new(move |key| {
        ((1..=GRID).contains(&key.x) && (1..=GRID).contains(&key.y))
            .then(|| ChunkVolume::with_z_bounds(world.clone(), key, 0, MAX_HEIGHT))
    }));
    // Typed load: the palette loader claims the plain `ron` extension and
    // bevy disambiguates by asset type (palette.rs docs).
    commands.insert_resource(DemoPaletteHandle(
        asset_server.load::<BlockPalette>(PALETTE_ASSET_PATH),
    ));
    // Chunks are marked dirty by apply_block_palette once the palette lands
    // (meshing before the layer LUT exists would bake layer 0 everywhere).
}

// ---------------------------------------------------------------------------
// Palette apply / hot reload
// ---------------------------------------------------------------------------

/// Consumes `AssetEvent<BlockPalette>` (Added/Modified — hot reload via
/// `file_watcher`, same mechanism as the atmosphere profiles):
/// - builds the three texture arrays (real mip chains) from the palette,
/// - first load: creates the shared terrain + fluid materials and installs
///   [`ChunkMaterials`]/[`ChunkLayerMap`];
/// - reload: updates the SAME material assets in place (live chunk entities
///   keep their handles — no respawn) and swaps the layer LUT;
/// - both: re-marks every chunk dirty (per-vertex layers ⇒ re-mesh). The visual
///   change POPS by design (documented in palette.rs).
fn apply_block_palette(
    mut commands: Commands,
    mut events: MessageReader<AssetEvent<BlockPalette>>,
    palettes: Res<Assets<BlockPalette>>,
    handle: Res<DemoPaletteHandle>,
    mut images: ResMut<Assets<Image>>,
    mut voxel_materials: ResMut<Assets<VoxelMaterial>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    existing: Option<Res<ChunkMaterials>>,
    layer_map: Option<ResMut<ChunkLayerMap>>,
    mut queue: ResMut<ChunkMeshQueue>,
) {
    let changed = events.read().any(|event| {
        matches!(
            event,
            AssetEvent::Added { id } | AssetEvent::Modified { id }
                if *id == handle.0.id()
        )
    });
    if !changed {
        return;
    }
    let Some(palette) = palettes.get(&handle.0) else {
        return;
    };

    // Procedural arrays from palette data (base color + noise, flat normals,
    // full mip chain + nearest/linear-mip sampler — palette.rs docs).
    let arrays = build_block_texture_arrays(palette);
    let albedo = images.add(arrays.albedo);
    let normal = images.add(arrays.normal);
    let mra = images.add(arrays.mra);

    // The interim fluid material is palette data too (Water entry): stock
    // transparent StandardMaterial until EM-3.9's dedicated water shader.
    let water = palette
        .blocks
        .get(&BlockKind::Water)
        .cloned()
        .unwrap_or_default();
    let fluid_material = StandardMaterial {
        base_color: Color::srgba(
            water.base_color[0],
            water.base_color[1],
            water.base_color[2],
            // Opacity is palette data too (BlockLayerDef::alpha, EM-3.4 m3).
            water.alpha,
        ),
        perceptual_roughness: water.roughness.max(0.045),
        alpha_mode: AlphaMode::Blend,
        ..Default::default()
    };

    if let Some(materials) = existing {
        // HOT RELOAD: mutate the shared assets in place so every live chunk
        // entity keeps its material handle. The replaced image handles drop
        // with the old extension values (assets GC'd).
        info!("block palette reloaded; rebuilding texture arrays + re-meshing all chunks");
        if let Some(mut material) = voxel_materials.get_mut(&materials.terrain) {
            material.extension.albedo = albedo;
            material.extension.normal = normal;
            material.extension.mra = mra;
            material.extension.emissive_strength = palette.material.emissive_strength;
            material.extension.ao_strength = palette.material.ao_strength;
        }
        if let Some(mut material) = std_materials.get_mut(&materials.fluid) {
            *material = fluid_material;
        }
    } else {
        info!("block palette loaded; building terrain material");
        let terrain = voxel_materials.add(VoxelMaterial {
            base: StandardMaterial {
                // Textures come from the arrays; keep the base fully neutral.
                base_color: Color::WHITE,
                ..Default::default()
            },
            extension: VoxelMaterialExt {
                albedo,
                normal,
                mra,
                emissive_strength: palette.material.emissive_strength,
                ao_strength: palette.material.ao_strength,
            },
        });
        let fluid = std_materials.add(fluid_material);
        commands.insert_resource(ChunkMaterials { terrain, fluid });
    }

    // Snapshot the kind→layer LUT for the mesh tasks, then re-mesh. On
    // reload the swap MUST be in place (ResMut): `commands.insert_resource`
    // is deferred to the end of the frame, while the dirty marks below are
    // immediate — with no ordering edge against `spawn_chunk_mesh_tasks`,
    // the re-marked chunks could all drain capturing the STALE Arc and the
    // layer remap would never apply. First load keeps the deferred insert:
    // the spawn system is gated on `resource_exists::<ChunkLayerMap>`, so
    // nothing can drain the queue before the resource lands.
    match layer_map {
        Some(mut map) => map.0 = Arc::new(palette.layer_lut()),
        None => commands.insert_resource(ChunkLayerMap(Arc::new(palette.layer_lut()))),
    }
    mark_all_chunks_dirty(&mut queue);
}

#[cfg(test)]
mod tests {
    use super::*;
    use xindeler_render_voxel::{
        convert::{ATTRIBUTE_BLOCK_LAYER, ATTRIBUTE_VOXEL_AO, terrain_mesh_to_bevy},
        mesh::terrain::generate_mesh,
    };

    fn shipped_palette() -> BlockPalette {
        let text = include_str!("../../../assets/xindeler/render/block_palette.ron");
        let mut palette: BlockPalette = ron::from_str(text).expect("block_palette.ron parses");
        palette.sanitize();
        palette
    }

    /// The generator must have no chunk-index dependence (it only ever sees
    /// world coordinates), and its field must be SMOOTH across every
    /// interior chunk border — a wall-sized step there would read as a
    /// seam in the smoke screenshot.
    #[test]
    fn generator_is_continuous_across_chunk_borders() {
        for border in 2..=GRID {
            let wx = border * CHUNK; // first column of the eastern chunk
            for wy in CHUNK..(GRID + 1) * CHUNK {
                let step = (height(wx, wy) - height(wx - 1, wy)).abs();
                assert!(
                    step <= 3,
                    "suspicious cliff ({step}) at chunk border wx={wx}, wy={wy}"
                );
            }
        }
    }

    /// The demo volume must mesh + convert through the same code path the
    /// pipeline tasks run (no GPU needed), exercise BOTH the opaque and
    /// fluid paths, bake occluded AO, and use the three opaque palette
    /// layers — otherwise the smoke screenshot cannot show what EM-3.4/3.5
    /// must show.
    #[test]
    fn demo_chunks_mesh_with_palette_layers() {
        let world = build_world();
        let palette = shipped_palette();
        let lut = palette.layer_lut();

        let mut all_layers: Vec<u32> = Vec::new();
        let mut any_fluid = false;
        let mut any_occlusion = false;
        for key in [VVec2::new(1, 1), VVec2::new(3, 3), VVec2::new(5, 5)] {
            let volume = ChunkVolume::with_z_bounds(world.clone(), key, 0, MAX_HEIGHT);
            let (opaque, fluid, _shadow, (_bounds, atlas, atlas_size, ..)) =
                generate_mesh(&volume.grid, (volume.range, VVec2::new(4096, 4096), ()));
            assert!(!opaque.is_empty(), "demo chunk {key:?} must have terrain");
            any_fluid |= !fluid.is_empty();

            let converted =
                terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, |k| lut[usize::from(k)]);
            use bevy::mesh::VertexAttributeValues;
            let Some(VertexAttributeValues::Float32(ao)) =
                converted.attribute(ATTRIBUTE_VOXEL_AO.id)
            else {
                panic!("VoxelAo must be Float32");
            };
            assert!(ao.iter().all(|a| (0.0..=1.0).contains(a)));
            any_occlusion |= ao.iter().any(|&a| a < 0.9);
            let Some(VertexAttributeValues::Uint32(layers)) =
                converted.attribute(ATTRIBUTE_BLOCK_LAYER.id)
            else {
                panic!("BlockLayer must be Uint32");
            };
            all_layers.extend(layers);
        }
        assert!(any_fluid, "demo pools must exercise the fluid path");
        assert!(any_occlusion, "demo creases must bake visible occlusion");
        for kind in [BlockKind::Rock, BlockKind::Earth, BlockKind::GlowingRock] {
            assert!(
                all_layers.contains(&lut[kind as u8 as usize]),
                "demo terrain must use the palette layer of {kind:?}"
            );
        }
    }

    /// The provider serves exactly the meshed window.
    #[test]
    fn provider_serves_only_the_demo_window() {
        let world = build_world();
        let provider = ChunkVolumeProvider::new(move |key| {
            ((1..=GRID).contains(&key.x) && (1..=GRID).contains(&key.y))
                .then(|| ChunkVolume::with_z_bounds(world.clone(), key, 0, MAX_HEIGHT))
        });
        assert!(provider.fetch(VVec2::new(1, 1)).is_some());
        assert!(provider.fetch(VVec2::new(GRID, GRID)).is_some());
        assert!(provider.fetch(VVec2::new(0, 1)).is_none());
        assert!(provider.fetch(VVec2::new(GRID + 1, 3)).is_none());
    }
}
