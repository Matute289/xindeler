//! Voxel rendering: greedy-meshing port (Mapper C5-C7), vertex AO, PBR texture
//! arrays, chunk pipeline.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7). Real systems land per
//! `docs/design/tasks/45-engine-migration-tasks.md`. Isolation law: logic
//! crates never depend on this crate or on Bevy.

pub mod mesh;

use bevy::app::{App, Plugin};

pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, _app: &mut App) {}
}
