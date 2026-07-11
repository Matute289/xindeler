//! ORACLE host: DmEvent AssetLoader, instanced dimensions,
//! AtmosphereController, entity factory, AI-gateway config seam.
//!
//! BL-82 Bevy migration — skeleton crate (EM-0.7); [`atmosphere`] (EM-2.4),
//! [`ai_gateway`] (EM-4.2e, a config/metrics-only seam for BL-83/BL-85 that
//! makes zero real AI calls itself), [`dm_event`] (EM-4.3/4.4), and
//! [`entity_template`] (EM-4.7, the generic entity factory schema —
//! `xindeler-sim-bridge::entity_factory` holds the other half that
//! actually spawns into the sim) are the first real systems.
//! [`atmosphere_sync`] (EM-4.9, Phase D) is the newest: the per-dimension
//! `SetClientAtmosphere` targeted message, the server→client half of the
//! Mist-Bound drill's atmosphere-replication seam. [`oracle_manifest`]
//! (EM-4.9 follow-up, data-driven-content cleanup) is the RON asset naming
//! which canonical `.dmevent.ron` files the host should proactively request
//! a handle for — see its own doc comment for why. Remaining systems land per
//! `docs/design/tasks/45-engine-migration-tasks.md` /
//! `docs/design/tasks/47-bl82-phase4-remaining-tasks.md` /
//! `docs/design/tasks/51-bl82-em49-e2e-drill-tasks.md`. Isolation law: logic
//! crates never depend on this crate or on Bevy. This crate stays render-free
//! (headless-safe) — apply-to-render systems live in the client.

pub mod ai_gateway;
pub mod atmosphere;
pub mod atmosphere_sync;
pub mod chronicle;
pub mod dm_event;
pub mod entity_template;
pub mod oracle_manifest;

use bevy::app::{App, Plugin};

pub use crate::{
    ai_gateway::{
        AiExecutionMode, AiGatewayConfig, AiGatewayMetrics, AiGatewayPlugin, FallbackPolicy,
    },
    atmosphere::{
        AmbientSky, AtmosphereController, AtmosphereProfile, WeatherEffect,
        XindelerAtmospherePlugin,
    },
    atmosphere_sync::{
        AtmosphereSyncMessagePlugin, DimensionAtmospheres, ServerAtmosphereSyncPlugin,
        SetClientAtmosphere,
    },
    chronicle::{ChronicleLog, ChroniclePlugin},
    dm_event::{
        DimensionConfig, DmEvent, DmEventLoader, DmEventPlugin, Narrative, ORACLE_EVENTS_DIR_ENV,
        ORACLE_SOURCE, SpawningRules, default_events_dir, register_oracle_source,
    },
    entity_template::{
        AgentPreset, EntityTemplate, EntityTemplateLoader, EntityTemplatePlugin,
        EntityTemplateStats, PendingAiBehavior, PendingBody, PendingEntityTemplateSpawn,
        PendingFaction, PendingLoot, PendingStats, spawn_entity_template,
    },
    oracle_manifest::{
        DEFAULT_MANIFEST_ASSET_PATH, OracleEventManifest, OracleEventManifestLoader,
        OracleEventManifestPlugin,
    },
};

pub struct OracleHostPlugin;

impl Plugin for OracleHostPlugin {
    fn build(&self, _app: &mut App) {}
}
