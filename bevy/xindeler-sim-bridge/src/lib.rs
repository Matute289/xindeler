//! SERVER-side bridge: embeds the specs sim (veloren server) and mirrors state
//! into replicated Bevy entities. The only place both ECS worlds meet.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7). Real systems land per
//! `docs/design/tasks/45-engine-migration-tasks.md`. Isolation law: logic
//! crates never depend on this crate or on Bevy.

use bevy::app::{App, Plugin};

pub struct SimBridgePlugin;

impl Plugin for SimBridgePlugin {
    fn build(&self, _app: &mut App) {}
}
