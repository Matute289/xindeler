//! [`AiExecutionMode`] — the seam BL-83 (AURORA / local vLLM) and BL-85
//! (ORACLE / AWS Bedrock) will gate their real calls on (BL-82 EM-4.2e, spec
//! §1.4). This crate makes zero AI calls itself; the enum only labels intent
//! for a future caller.
//!
//! ## Why this type lives in `xindeler-protocol`, not `xindeler-oracle-host`
//! `xindeler-oracle-host` "owns" the AI-gateway config
//! ([`xindeler_oracle_host::ai_gateway::AiGatewayConfig`], the sibling task
//! EM-4.2e) and superficially looks like the natural home for this enum too.
//! But EM-4.2f's mirror (`xindeler-sim-bridge`) also needs to read the
//! CURRENT mode — to decide whether `AuroraOverlay` gets populated — and
//! `xindeler-sim-bridge` already depends on `xindeler-protocol`, but NOT on
//! `xindeler-oracle-host` (which, before this same EM-4.2e change,
//! `xindeler-client` was the only dependent of; this change adds
//! `xindeler-server-app` as a second one, purely to wire `AiGatewayPlugin` —
//! `xindeler-sim-bridge` still isn't among them). Checking the existing
//! crate-dependency direction: `xindeler-protocol` is
//! the low-level, few-deps wire/shared crate; `xindeler-oracle-host` is
//! higher-level (asset loading, ORACLE content). A higher-level crate
//! depending on a lower-level one is the normal, healthy direction; the
//! reverse — `xindeler-protocol` depending on `xindeler-oracle-host` so it
//! could read this enum — would be backwards and is exactly the kind of edge
//! this crate's whole "low-level wire crate with few deps" design avoids.
//!
//! So: the enum is defined here (zero new dependents need a new edge —
//! `xindeler-sim-bridge` already links this crate), and
//! `xindeler-oracle-host` adds ONE clean forward edge onto `xindeler-protocol`
//! to embed this type inside its own `AiGatewayConfig`. See
//! `xindeler_oracle_host::ai_gateway`'s module doc for the config side of this
//! seam.
use bevy::ecs::resource::Resource;
use serde::{Deserialize, Serialize};

/// Which AI backends are currently allowed to be dialed. Three states, not a
/// bool (BL-82 EM-4.2e worksheet [Q2], 2026-07-10): AURORA (local vLLM) and
/// ORACLE (AWS Bedrock) are gated independently because ORACLE's Bedrock
/// calls cost real money and AURORA's local calls don't.
///
/// **No caller checks this yet** — no HTTP/gRPC/AWS client exists in this
/// workspace at all regardless of which mode is configured. The enum exists
/// so BL-83/BL-85 have a documented switch to check once they add one.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiExecutionMode {
    /// Zero AI activity. Today's `server-agent` default AI, unchanged — this
    /// is not a degraded mode, it is the current game.
    #[default]
    Offline,
    /// AURORA (local vLLM) active; ORACLE (Bedrock) inactive — no Bedrock
    /// cost.
    LocalOnly,
    /// Both AURORA and ORACLE active.
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_offline() {
        assert_eq!(AiExecutionMode::default(), AiExecutionMode::Offline);
    }

    #[test]
    fn round_trips_through_ron() {
        for mode in [
            AiExecutionMode::Offline,
            AiExecutionMode::LocalOnly,
            AiExecutionMode::Full,
        ] {
            let text = ron::to_string(&mode).expect("serializes");
            let back: AiExecutionMode = ron::from_str(&text).expect("deserializes");
            assert_eq!(back, mode);
        }
    }
}
