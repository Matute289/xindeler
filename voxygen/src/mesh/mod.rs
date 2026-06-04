pub mod greedy;
pub mod segment;
pub mod terrain;
pub mod transvoxel;

use crate::render::Mesh;

pub type MeshGen<V, T, S, R> = (Mesh<V>, Mesh<T>, Mesh<S>, R);
