//! Replication contract (bevy_replicon): replicated components, channels,
//! client/server events. Shared by client and server — [Q3]=B.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7). Real systems land per
//! `docs/design/tasks/45-engine-migration-tasks.md`. Isolation law: logic
//! crates never depend on this crate or on Bevy.

use bevy::app::{App, Plugin};

pub struct XindelerProtocolPlugin;

impl Plugin for XindelerProtocolPlugin {
    fn build(&self, _app: &mut App) {}
}
