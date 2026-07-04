//! Voxel rendering: greedy-meshing port (Mapper C5-C7), vertex AO, PBR texture
//! arrays, chunk pipeline.
//!
//! BL-82 Bevy migration. Layered by cargo feature so the mesher core stays
//! free of bevy render machinery (see Cargo.toml):
//! - always: `mesh` — the EM-3.1 lift-copied greedy mesher (CPU-only),
//! - `convert`: EM-3.2 `Mesh<TerrainVertex>` → `bevy::mesh::Mesh` with the
//!   custom `VOXEL_AO`/`BLOCK_LAYER` attributes,
//! - `material`: EM-3.3 `VoxelMaterialExt` (PBR texture arrays + vertex-AO
//!   WGSL) + the EM-3.4 data-driven [`palette`] (`block_palette.ron` asset,
//!   loader, procedural texture arrays with real mip chains) — both registered
//!   by [`VoxelRenderPlugin`],
//! - `pipeline` (default): EM-3.5 async chunk meshing ([`pipeline`]) — dirty
//!   queue → `AsyncComputeTaskPool` → budgeted uploads.
//!
//! Isolation law: logic crates never depend on this crate or on Bevy.

#[cfg(feature = "convert")] pub mod convert;
#[cfg(feature = "material")] pub mod material;
pub mod mesh;
#[cfg(feature = "material")] pub mod palette;
#[cfg(feature = "pipeline")] pub mod pipeline;

use bevy::app::{App, Plugin};

pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "material")]
        {
            use bevy::asset::AssetApp;
            app.add_plugins(material::VoxelMaterialPlugin)
                .init_asset::<palette::BlockPalette>()
                .init_asset_loader::<palette::BlockPaletteLoader>();
        }
        #[cfg(feature = "pipeline")]
        app.add_plugins(pipeline::ChunkMeshPipelinePlugin);
        #[cfg(not(feature = "material"))]
        let _ = app;
    }
}
