//! ORACLE host: DmEvent AssetLoader, instanced dimensions,
//! AtmosphereController, entity factory, AI-gateway config seam.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7); [`atmosphere`] (EM-2.4),
//! [`ai_gateway`] (EM-4.2e, a config/metrics-only seam for BL-83/BL-85 that
//! makes zero real AI calls itself), [`dm_event`] (EM-4.3/4.4), and
//! [`entity_template`] (EM-4.7, the generic entity factory schema + registry
//! — `xindeler-sim-bridge::entity_factory` holds the other half that
//! actually spawns into the sim) are the first real systems. Remaining
//! systems land per `docs/design/tasks/45-engine-migration-tasks.md` /
//! `docs/design/tasks/47-bl82-phase4-remaining-tasks.md`. Isolation law:
//! logic crates never depend on this crate or on Bevy. This crate stays
//! render-free (headless-safe) — apply-to-render systems live in the client.

pub mod ai_gateway;
pub mod atmosphere;
pub mod chronicle;
pub mod dm_event;
pub mod entity_template;

use bevy::app::{App, Plugin};

pub use crate::{
    ai_gateway::{
        AiExecutionMode, AiGatewayConfig, AiGatewayMetrics, AiGatewayPlugin, FallbackPolicy,
    },
    atmosphere::{
        AmbientSky, AtmosphereController, AtmosphereProfile, WeatherEffect,
        XindelerAtmospherePlugin,
    },
    chronicle::{ChronicleLog, ChroniclePlugin},
    dm_event::{
        DimensionConfig, DmEvent, DmEventLoader, DmEventPlugin, Narrative, ORACLE_EVENTS_DIR_ENV,
        ORACLE_SOURCE, SpawningRules, default_events_dir, register_oracle_source,
    },
    entity_template::{
        AgentPreset, ComponentSpawnRegistry, EntityTemplate, EntityTemplateLoader,
        EntityTemplatePlugin, EntityTemplateStats, PendingAiBehavior, PendingBody,
        PendingEntityTemplateSpawn, PendingFaction, PendingLoot, PendingStats, SpawnClosure,
        spawn_entity_template,
    },
};

pub struct OracleHostPlugin;

impl Plugin for OracleHostPlugin {
    fn build(&self, _app: &mut App) {}
}
