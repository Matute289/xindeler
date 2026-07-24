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
//! - `figure`: EM-3.8 real `.vox` figures ([`figure`]) — assembles per-body
//!   voxel parts (figure mesher + `xindeler-anim` rest-pose bones) into
//!   coloured `bevy::Mesh`es. Off by default (the default set is terrain-only);
//!   the client opts in.
//! - `figure` also carries EM-3.9 block [`sprite`]s (grass/flowers/props;
//!   EM-3.9b widened the kind whitelist to the whole `Plant` category and
//!   attempted a wind-sway shader that broke sprite lighting and was reverted;
//!   EM-3.9c ships a normal-consistent v2 ([`material::SpriteWindMaterial`])
//!   plus a further `Furniture`/`Decor`/ `Lamp`/`Container` kind widening — see
//!   `xindeler-client::sprite_view`'s module docs) — per-chunk instance
//!   collection + `.vox` meshing (reusing the figure segment mesher). Fluids
//!   (water) need no new TOP-LEVEL module: the terrain mesher already emits a
//!   fluid mesh that [`pipeline`] renders, carrying the EM-3.9 river velocity
//!   via [`convert::fluid_mesh_to_bevy`] into EM-3.9b's dedicated animated
//!   [`material::WaterMaterial`] shader (a `material` submodule).
//!
//! Isolation law: logic crates never depend on this crate or on Bevy.

#[cfg(feature = "convert")] pub mod convert;
#[cfg(feature = "figure")] pub mod figure;
#[cfg(feature = "item-icon")] pub mod item_icon;
#[cfg(feature = "material")] pub mod material;
pub mod mesh;
#[cfg(feature = "material")] pub mod palette;
#[cfg(feature = "pipeline")] pub mod pipeline;
#[cfg(feature = "figure")] pub mod sprite;

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
