//! ORACLE host: DmEvent AssetLoader, instanced dimensions,
//! AtmosphereController, entity factory.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7). Real systems land per
//! `docs/design/tasks/45-engine-migration-tasks.md`. Isolation law: logic
//! crates never depend on this crate or on Bevy.

use bevy::app::{App, Plugin};

pub struct OracleHostPlugin;

impl Plugin for OracleHostPlugin {
    fn build(&self, _app: &mut App) {}
}
