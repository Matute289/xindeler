//! Shared block-palette → material pipeline setup (EM-3.4/3.5).
//!
//! Loads `block_palette.ron`, builds its procedural texture arrays, and
//! installs the pipeline's [`ChunkMaterials`] + [`ChunkLayerMap`] resources —
//! the material inputs the async chunk pipeline needs before ANY chunk can
//! mesh. On hot reload it mutates the shared material assets in place and swaps
//! the layer LUT, then re-marks every currently-indexed chunk dirty.
//!
//! Extracted from the EM-3.5 voxel demo so BOTH the synthetic demo AND the
//! EM-3.6 listen-server (real streamed terrain) share it — without it, the
//! listen-server path would install a `ChunkVolumeProvider` but no
//! `ChunkLayerMap`/`ChunkMaterials`, so `spawn_chunk_mesh_tasks` (gated on
//! `ChunkLayerMap`) and `apply_chunk_meshes` (gated on `ChunkMaterials`) would
//! never run and nothing would ever mesh.

use std::sync::Arc;

use bevy::prelude::*;
use common::terrain::BlockKind;
use xindeler_render_voxel::{
    material::{VoxelMaterial, VoxelMaterialExt},
    palette::{BlockPalette, PALETTE_ASSET_PATH, build_block_texture_arrays},
    pipeline::{ChunkLayerMap, ChunkMaterials, ChunkMeshIndex, ChunkMeshQueue},
};

/// Loads the block palette and keeps the pipeline materials + layer LUT in
/// sync with it (initial load + hot reload). Idempotent-safe to add once.
pub struct PaletteMaterialPlugin;

impl Plugin for PaletteMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_palette)
            .add_systems(Update, apply_palette);
    }
}

/// Strong handle keeping the palette asset (and its file watch) alive.
#[derive(Resource)]
struct PaletteHandle(Handle<BlockPalette>);

fn load_palette(mut commands: Commands, asset_server: Res<AssetServer>) {
    // Typed load: the palette loader claims the plain `ron` extension and bevy
    // disambiguates by asset type (palette.rs docs).
    commands.insert_resource(PaletteHandle(
        asset_server.load::<BlockPalette>(PALETTE_ASSET_PATH),
    ));
}

/// Consumes `AssetEvent<BlockPalette>` (Added/Modified):
/// - first load: builds the texture arrays, creates the shared terrain + fluid
///   materials, installs [`ChunkMaterials`]/[`ChunkLayerMap`];
/// - reload: mutates the SAME material assets in place (live chunk entities
///   keep their handles) and swaps the layer LUT, then re-marks every indexed
///   chunk dirty (per-vertex layers ⇒ re-mesh).
fn apply_palette(
    mut commands: Commands,
    mut events: MessageReader<AssetEvent<BlockPalette>>,
    palettes: Res<Assets<BlockPalette>>,
    handle: Res<PaletteHandle>,
    mut images: ResMut<Assets<Image>>,
    mut voxel_materials: ResMut<Assets<VoxelMaterial>>,
    mut std_materials: ResMut<Assets<StandardMaterial>>,
    existing: Option<Res<ChunkMaterials>>,
    layer_map: Option<ResMut<ChunkLayerMap>>,
    index: Res<ChunkMeshIndex>,
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
            water.alpha,
        ),
        perceptual_roughness: water.roughness.max(0.045),
        alpha_mode: AlphaMode::Blend,
        ..Default::default()
    };

    let is_reload = existing.is_some();
    if let Some(materials) = existing {
        // HOT RELOAD: mutate the shared assets in place so every live chunk
        // entity keeps its material handle.
        info!("block palette reloaded; rebuilding texture arrays + re-meshing indexed chunks");
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

    // Snapshot the kind→layer LUT for the mesh tasks. On reload the swap MUST be
    // in place (ResMut): `commands.insert_resource` is deferred to end-of-frame
    // while the dirty marks below are immediate — with no ordering edge against
    // `spawn_chunk_mesh_tasks`, re-marked chunks could drain capturing the STALE
    // Arc. First load keeps the deferred insert (the spawn system is gated on
    // `resource_exists::<ChunkLayerMap>`, so nothing drains before it lands).
    match layer_map {
        Some(mut map) => map.0 = Arc::new(palette.layer_lut()),
        None => commands.insert_resource(ChunkLayerMap(Arc::new(palette.layer_lut()))),
    }

    // On RELOAD, re-mesh every chunk currently in the index (layers changed).
    // On FIRST load there is nothing indexed yet — the content owner (demo or
    // terrain stream) marks its chunks dirty; once ChunkLayerMap exists they
    // mesh. (The demo marks on load; the stream marks on receive.)
    if is_reload {
        let keys: Vec<_> = index.keys().collect();
        for key in keys {
            queue.mark_dirty(key);
        }
    }
}
