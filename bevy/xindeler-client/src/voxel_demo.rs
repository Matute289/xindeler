//! EM-3.3 demo: a REAL synthetic chunk, meshed with the ported greedy mesher
//! (EM-3.1), converted with the EM-3.2 attribute pipeline and rendered with
//! `VoxelMaterialExt` — hills + terraces + a water pool + emissive crystal
//! veins, all deterministic and generated in code (isolation law: no new
//! files under `assets/`; test textures are procedural).
//!
//! What the smoke screenshot must show (EM-3.3 acceptance): crisp
//! nearest-sampled texels, normal-mapped relief, roughness/metallic response,
//! and vertex-AO darkening at concave corners (creases, terrace steps, the
//! pool rim) that affects INDIRECT light only.
//!
//! Layer mapping is a code stub — TODO(EM-3.4): `block_palette.ron` owns
//! block kind → layer + PBR params, and the loader builds the arrays from
//! real textures at startup.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use common::{
    terrain::{Block, BlockKind, MapSizeLg, SpriteKind, TerrainChunk, TerrainChunkMeta},
    vol::WriteVol,
    volumes::vol_grid_2d::VolGrid2d,
};
use std::sync::Arc;
use vek::{Aabb, Rgb, Vec2 as VVec2, Vec3 as VVec3};
use xindeler_render_voxel::{
    convert::{fluid_mesh_to_bevy, terrain_mesh_to_bevy},
    material::{VoxelMaterial, VoxelMaterialExt},
    mesh::terrain::generate_mesh,
};

pub struct VoxelDemoPlugin;

impl Plugin for VoxelDemoPlugin {
    fn build(&self, app: &mut App) {
        // Sky ambient lives in the ATMOSPHERE module (AtmospherePlugin) — not
        // here — so deleting this demo never silently changes scene lighting.
        app.add_systems(Startup, spawn_voxel_demo);
    }
}

// ---------------------------------------------------------------------------
// Synthetic terrain (deterministic — no RNG deps)
// ---------------------------------------------------------------------------

const CHUNK: i32 = 32;
/// The meshed chunk key; its world xy span is [32, 64). Neighbour chunks stay
/// EMPTY so the mesher closes the sides — a self-contained diorama slab whose
/// walls also demo the world-space tiling on big greedy quads.
const MESH_KEY: VVec2<i32> = VVec2 { x: 1, y: 1 };
/// Water surface height for the valley pool.
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

/// Terrain-ish heightfield over chunk-local (u, v) ∈ [0, 32): smooth hills,
/// terraced into 2-block steps on the east half (crisp AO creases).
#[expect(clippy::cast_precision_loss, reason = "chunk-local coords < 32")]
fn height(u: i32, v: i32) -> i32 {
    let (uf, vf) = (u as f32, v as f32);
    let smooth =
        7.0 + 3.5 * (uf * 0.32).sin() + 3.0 * (vf * 0.26).cos() + 1.8 * ((uf + vf) * 0.14).sin();
    #[expect(clippy::cast_possible_truncation, reason = "clamped small range")]
    let mut h = smooth.round() as i32;
    if u >= 16 {
        // Terraces: quantise to 2-block steps.
        h = (h / 2) * 2;
    }
    h.clamp(1, MAX_HEIGHT)
}

/// Block for world position `wpos` inside the demo chunk, or `None` (air).
fn block_at(wpos: VVec3<i32>) -> Option<Block> {
    let u = wpos.x - MESH_KEY.x * CHUNK;
    let v = wpos.y - MESH_KEY.y * CHUNK;
    if !(0..CHUNK).contains(&u) || !(0..CHUNK).contains(&v) || wpos.z < 0 {
        return None;
    }
    let h = height(u, v);
    if wpos.z < h {
        // Solid column: Earth topsoil, Rock body, sparse crystal veins.
        // Colours are carried but unused by the v1 material (convert.rs docs).
        let kind = if wpos.z == h - 1 && h > WATER_LEVEL {
            BlockKind::Earth
        } else if hash(wpos).is_multiple_of(53) {
            BlockKind::GlowingRock
        } else {
            BlockKind::Rock
        };
        Some(Block::new(kind, Rgb::new(120, 100, 90)))
    } else if wpos.z <= WATER_LEVEL {
        // Valley pool (exercises the fluid meshing + conversion path).
        Some(Block::water(SpriteKind::Empty))
    } else {
        None
    }
}

/// Builds the 3×3 chunk grid around [`MESH_KEY`] and the mesh range, exactly
/// like the upstream caller (voxygen/src/scene/terrain/mod.rs:1064-1098):
/// xy = chunk ± 1 border, z = [min_z - 2, max_z + 2].
fn build_grid() -> (VolGrid2d<TerrainChunk>, Aabb<i32>) {
    let map_size_lg = MapSizeLg::new(VVec2::new(2, 2)).expect("valid demo map size");
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
            min_z = min_z.min(chunk.get_min_z());
            max_z = max_z.max(chunk.get_max_z());
            grid.insert(VVec2::new(kx, ky), Arc::new(chunk));
        }
    }

    let range = Aabb {
        min: VVec3::new(MESH_KEY.x * CHUNK - 1, MESH_KEY.y * CHUNK - 1, min_z - 2),
        max: VVec3::new(
            (MESH_KEY.x + 1) * CHUNK + 1,
            (MESH_KEY.y + 1) * CHUNK + 1,
            max_z + 2,
        ),
    };
    (grid, range)
}

/// Block kind → texture-array layer. TODO(EM-3.4): `block_palette.ron`.
fn kind_to_layer(kind: u8) -> u32 {
    match kind {
        k if k == BlockKind::Earth as u8 => 1,
        k if k == BlockKind::GlowingRock as u8 => 2,
        _ => 0, // Rock + any unmapped kind
    }
}

// ---------------------------------------------------------------------------
// Procedural test texture arrays (3 layers: stone / earth / crystal)
// ---------------------------------------------------------------------------

const TEX_SIZE: u32 = 32;
const LAYERS: u32 = 3;

/// 2D value noise in [0, 1] from the integer hash (per layer salt).
fn noise(x: u32, y: u32, salt: i32) -> f32 {
    #[expect(clippy::cast_possible_wrap, reason = "texel coords < 32")]
    let h = hash(VVec3::new(x as i32, y as i32, salt));
    (h % 1024) as f32 / 1023.0
}

/// Repeat + nearest-min/mag (crisp texels, spec §4.4), linear mip.
fn nearest_repeat_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    })
}

/// Builds a `TextureDimension::D2` array image from a per-texel closure.
fn array_image(format: TextureFormat, texel: impl Fn(u32, u32, u32) -> [u8; 4]) -> Image {
    let mut data = Vec::with_capacity((TEX_SIZE * TEX_SIZE * LAYERS * 4) as usize);
    for layer in 0..LAYERS {
        for y in 0..TEX_SIZE {
            for x in 0..TEX_SIZE {
                data.extend_from_slice(&texel(x, y, layer));
            }
        }
    }
    // Demo textures are MIP-LESS (mip_level_count = 1), so the spec §4.4
    // anti-shimmer pairing (nearest min/mag + LINEAR MIP over a real chain) is
    // not exercised here — expect distant sparkle on the diorama. Real mip
    // generation is an explicit requirement of the EM-3.4 texture-array loader.
    let mut image = Image::new(
        Extent3d {
            width: TEX_SIZE,
            height: TEX_SIZE,
            depth_or_array_layers: LAYERS,
        },
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = nearest_repeat_sampler();
    image
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "quantising [0,1] floats to u8"
)]
fn quantise(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0) as u8 }

/// Crystal vein mask for layer 2 (shared by albedo + MRA emissive channel).
fn vein(x: u32, y: u32) -> bool { noise(x / 2, y / 2, 7) > 0.78 }

/// sRGB albedo: gray stone / brown earth / dark crystal with cyan veins.
fn albedo_texel(x: u32, y: u32, layer: u32) -> [u8; 4] {
    let n = noise(x, y, layer as i32);
    match layer {
        0 => {
            // Stone: mid-gray value noise + darker "mortar" grid lines.
            let mortar = x.is_multiple_of(8) || y.is_multiple_of(8);
            let base = if mortar { 0.32 } else { 0.48 + 0.18 * n };
            [quantise(base), quantise(base), quantise(base * 1.04), 255]
        },
        1 => {
            // Earth: warm brown noise.
            let base = 0.75 + 0.25 * n;
            [
                quantise(0.55 * base),
                quantise(0.38 * base),
                quantise(0.22 * base),
                255,
            ]
        },
        _ => {
            // Crystal: dark slate + bright cyan veins (high contrast).
            if vein(x, y) {
                [90, 230, 255, 255]
            } else {
                let base = 0.12 + 0.08 * n;
                [
                    quantise(base),
                    quantise(base * 1.1),
                    quantise(base * 1.5),
                    255,
                ]
            }
        },
    }
}

/// Height used to derive the normal maps (linear, per layer).
fn bump_height(x: u32, y: u32, layer: u32) -> f32 {
    let x = x % TEX_SIZE;
    let y = y % TEX_SIZE;
    match layer {
        // Stone: recessed mortar grid + gentle noise.
        0 => {
            let mortar = x.is_multiple_of(8) || y.is_multiple_of(8);
            (if mortar { 0.0 } else { 0.7 }) + 0.3 * noise(x, y, 0)
        },
        // Earth: pure noise bumps.
        1 => noise(x, y, 1),
        // Crystal: raised veins.
        _ => {
            if vein(x, y) {
                1.0
            } else {
                0.4
            }
        },
    }
}

/// Linear tangent-space normal map, finite-differenced from [`bump_height`].
fn normal_texel(x: u32, y: u32, layer: u32) -> [u8; 4] {
    const STRENGTH: f32 = 0.8;
    let dx = (bump_height(x + 1, y, layer) - bump_height(x + TEX_SIZE - 1, y, layer)) * STRENGTH;
    let dy = (bump_height(x, y + 1, layer) - bump_height(x, y + TEX_SIZE - 1, layer)) * STRENGTH;
    let n = Vec3::new(-dx, -dy, 1.0).normalize();
    [
        quantise(n.x * 0.5 + 0.5),
        quantise(n.y * 0.5 + 0.5),
        quantise(n.z * 0.5 + 0.5),
        255,
    ]
}

/// Linear MRA: R=metallic, G=roughness, B=texture AO, A=emissive mask
/// (channel convention documented in `material/mod.rs`).
fn mra_texel(x: u32, y: u32, layer: u32) -> [u8; 4] {
    let n = noise(x, y, layer as i32 + 100);
    match layer {
        0 => [0, quantise(0.8 + 0.15 * n), 255, 0],
        1 => [0, quantise(0.95), 255, 0],
        _ => {
            if vein(x, y) {
                // Shiny emissive veins.
                [quantise(0.1), quantise(0.2), 255, quantise(0.85)]
            } else {
                [quantise(0.05), quantise(0.45), 255, 0]
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

fn spawn_voxel_demo(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut voxel_materials: ResMut<Assets<VoxelMaterial>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
) {
    // Mesh the chunk through the REAL ported pipeline.
    let (grid, range) = build_grid();
    let (opaque, fluid, _shadow, (_bounds, atlas, atlas_size, ..)) =
        // Startup-only SYNCHRONOUS meshing — fine for this one-shot diorama, but the
        // real chunk pipeline MUST mesh on AsyncComputeTaskPool with budgeted uploads.
        // TODO(EM-3.5): do not pattern-copy this call into the chunk pipeline.
        generate_mesh(&grid, (range, VVec2::new(4096, 4096), ()));
    let terrain_mesh = terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, kind_to_layer);
    let fluid_mesh = (!fluid.is_empty()).then(|| fluid_mesh_to_bevy(&fluid));

    let material = voxel_materials.add(VoxelMaterial {
        base: StandardMaterial {
            // Textures come from the arrays; keep the base fully neutral.
            base_color: Color::WHITE,
            ..Default::default()
        },
        extension: VoxelMaterialExt {
            albedo: images.add(array_image(TextureFormat::Rgba8UnormSrgb, albedo_texel)),
            normal: images.add(array_image(TextureFormat::Rgba8Unorm, normal_texel)),
            mra: images.add(array_image(TextureFormat::Rgba8Unorm, mra_texel)),
            // HDR luminance for the crystal veins (bloom-visible at EV100 13).
            emissive_strength: 60_000.0,
            // See material/mod.rs: compensates the v1 per-vertex AO bake.
            ao_strength: 2.5,
        },
    });

    // Converter output is Bevy-space, chunk-local: x ∈ [0, 33], y = height,
    // z ∈ [-33, 0]. Center the slab on the origin; INTEGER translation only
    // (converter contract: world-space tiling + analytic tangents).
    let chunk_transform = Transform::from_xyz(-16.0, 0.0, 16.0);

    commands.spawn((
        Mesh3d(meshes.add(terrain_mesh)),
        MeshMaterial3d(material),
        chunk_transform,
    ));

    if let Some(fluid_mesh) = fluid_mesh {
        commands.spawn((
            Mesh3d(meshes.add(fluid_mesh)),
            // Stock transparent water until EM-3.9's dedicated shader.
            MeshMaterial3d(std_materials.add(StandardMaterial {
                base_color: Color::srgba(0.15, 0.35, 0.6, 0.6),
                perceptual_roughness: 0.08,
                alpha_mode: AlphaMode::Blend,
                ..Default::default()
            })),
            chunk_transform,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xindeler_render_voxel::convert::{ATTRIBUTE_BLOCK_LAYER, ATTRIBUTE_VOXEL_AO};

    /// The demo terrain is deterministic: it must mesh + convert (no GPU
    /// needed), exercise BOTH the opaque and fluid paths, use all 3 texture
    /// layers, and bake occluded (< 1.0) AO — otherwise the smoke screenshot
    /// cannot show what EM-3.3 must show.
    #[test]
    fn demo_chunk_meshes_and_converts() {
        let (grid, range) = build_grid();
        let (opaque, fluid, _shadow, (_bounds, atlas, atlas_size, ..)) =
            generate_mesh(&grid, (range, VVec2::new(4096, 4096), ()));
        assert!(!opaque.is_empty(), "demo terrain must produce geometry");
        assert!(!fluid.is_empty(), "demo pool must exercise the fluid path");

        let converted = terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, kind_to_layer);
        use bevy::mesh::VertexAttributeValues;
        let Some(VertexAttributeValues::Float32(ao)) = converted.attribute(ATTRIBUTE_VOXEL_AO.id)
        else {
            panic!("VoxelAo must be Float32");
        };
        assert!(ao.iter().all(|a| (0.0..=1.0).contains(a)));
        assert!(
            ao.iter().any(|&a| a < 0.9),
            "demo creases must bake visible occlusion"
        );
        let Some(VertexAttributeValues::Uint32(layers)) =
            converted.attribute(ATTRIBUTE_BLOCK_LAYER.id)
        else {
            panic!("BlockLayer must be Uint32");
        };
        for layer in 0..LAYERS {
            assert!(
                layers.contains(&layer),
                "demo terrain must use texture layer {layer}"
            );
        }
    }
}
