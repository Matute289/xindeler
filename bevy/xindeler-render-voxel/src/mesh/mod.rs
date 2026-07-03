// LIFT-COPY (BL-82 EM-3.1, Mapper C5-C7) from voxygen/src/mesh/mod.rs @
// 6f9afd978c
//
// Divergences from the original (keep this list honest for the Mapper §E log):
// - `use crate::render::Mesh` → `use mesh::Mesh` (the CPU-side mesh container
//   is lift-copied from voxygen/src/render/mesh.rs into `mesh::mesh`).
// - Added `pub mod mesh` (container types) and `pub mod vertex` (portable
//   replacements for voxygen's wgpu-packed vertex/atlas types — see vertex.rs
//   for the packed→portable mapping).

pub mod greedy;
// Mirrors voxygen/src/render/mesh.rs (file name kept for diff-porting).
#[expect(clippy::module_inception)] pub mod mesh;
pub mod segment;
pub mod terrain;
pub mod vertex;

use mesh::Mesh;

pub type MeshGen<V, T, S, R> = (Mesh<V>, Mesh<T>, Mesh<S>, R);
