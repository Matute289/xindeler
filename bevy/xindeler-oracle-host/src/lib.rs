//! ORACLE host: DmEvent AssetLoader, instanced dimensions,
//! AtmosphereController, entity factory.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7); [`atmosphere`] (EM-2.4)
//! is the first real system. Remaining systems land per
//! `docs/design/tasks/45-engine-migration-tasks.md`. Isolation law: logic
//! crates never depend on this crate or on Bevy. This crate stays
//! render-free (headless-safe) — apply-to-render systems live in the client.

pub mod atmosphere;

use bevy::app::{App, Plugin};

pub use crate::atmosphere::{
    AtmosphereController, AtmosphereProfile, WeatherEffect, XindelerAtmospherePlugin,
};

pub struct OracleHostPlugin;

impl Plugin for OracleHostPlugin {
    fn build(&self, _app: &mut App) {}
}
