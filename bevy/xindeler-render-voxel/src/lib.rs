//! Voxel rendering: greedy-meshing port (Mapper C5-C7), vertex AO, PBR texture
//! arrays, chunk pipeline.
//!
//! BL-82 Bevy migration. Layered by cargo feature so the mesher core stays
//! free of bevy render machinery (see Cargo.toml):
//! - always: `mesh` — the EM-3.1 lift-copied greedy mesher (CPU-only),
//! - `convert` (default): EM-3.2 `Mesh<TerrainVertex>` → `bevy::mesh::Mesh`
//!   with the custom `VOXEL_AO`/`BLOCK_LAYER` attributes,
//! - `material`: EM-3.3 `VoxelMaterialExt` (PBR texture arrays + vertex-AO
//!   WGSL), registered by [`VoxelRenderPlugin`].
//!
//! Isolation law: logic crates never depend on this crate or on Bevy.

#[cfg(feature = "convert")] pub mod convert;
#[cfg(feature = "material")] pub mod material;
pub mod mesh;

use bevy::app::{App, Plugin};

pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "material")]
        app.add_plugins(material::VoxelMaterialPlugin);
        #[cfg(not(feature = "material"))]
        let _ = app;
    }
}
