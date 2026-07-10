//! AI-gateway readiness seam (BL-82 EM-4.2e, spec §1.4): a config surface +
//! `/metrics` counter stubs that BL-83 (AURORA/NPC RAG over a local vLLM) and
//! BL-85 (ORACLE/AWS Bedrock) will plug real HTTP/gRPC clients into.
//!
//! ## Scope — deliberately config/plumbing only
//! This module makes **zero real calls to any LLM, AWS SDK, or Bedrock API**.
//! There is no HTTP client, no gRPC client, no AWS credentials handling
//! anywhere here — [`AiGatewayConfig`] is read but never dialed, and the two
//! [`register_metrics`] counters read `0` forever until a real caller exists
//! (there is none in this task). BL-83/BL-85 plug a real client in here; this
//! crate must never make a network call itself, regardless of which
//! [`AiExecutionMode`] is configured — a future caller must check `mode`
//! before dialing out once a real client exists.
//!
//! [`AiExecutionMode`] itself lives in `xindeler-protocol` (re-exported here),
//! not in this crate — see that type's doc comment for the crate-dependency
//! reasoning (short version: `xindeler-sim-bridge`, EM-4.2f's mirror, already
//! depends on `xindeler-protocol`, not on this crate, so defining the shared
//! enum there means no new "reverse" dependency edge is ever needed).
//!
//! ## Fallback-behavior contract (spec §1.4 acceptance)
//! Written down now, even though nothing calls it yet, so BL-83/BL-85 don't
//! have to invent it later:
//! - **`mode == Offline`**: a future caller must not attempt any real call at
//!   all (AURORA and ORACLE both inactive) — this is not a "fallback" path, it
//!   is simply not dialing out, matching this task's own zero-calls posture.
//! - **`mode == LocalOnly`**: a future AURORA caller may dial its local vLLM
//!   endpoint; a future ORACLE caller must not dial Bedrock (no Bedrock cost).
//! - **A real call times out** (future work; nothing times out today because
//!   nothing calls out): the caller increments
//!   [`AiGatewayMetrics::fallback_total`] exactly once per timed-out attempt,
//!   then applies [`FallbackPolicy`]:
//!   - [`FallbackPolicy::DefaultBehavior`]: proceed as if the call had never
//!     been attempted (e.g. AURORA falls back to `server-agent`'s existing
//!     default AI; ORACLE skips the narrative augmentation for that tick) — the
//!     player-visible behavior degrades to "no AI," never to an error state.
//!   - [`FallbackPolicy::Disable`]: the caller flips its own local mode to
//!     behave as `Offline` for the rest of the session (stops retrying) — for a
//!     backend that is clearly unreachable, retrying every tick would just pile
//!     up more timeouts.
//! - Every real call attempt (regardless of outcome) increments
//!   [`AiGatewayMetrics::requests_total`] exactly once.

use bevy::{
    app::{App, Plugin},
    ecs::resource::Resource,
};
use prometheus::{IntCounter, Opts, Registry};
use serde::{Deserialize, Serialize};

pub use xindeler_protocol::AiExecutionMode;

/// What a future real caller does when a dialed-out AI call times out (or is
/// otherwise unavailable). See the module doc's "Fallback-behavior contract"
/// for what each variant means concretely; this task defines the contract,
/// it does not implement any caller that exercises it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FallbackPolicy {
    /// Proceed as if the call had never been attempted (degrade to "no AI"
    /// for that tick/caller, never to an error state).
    #[default]
    DefaultBehavior,
    /// Stop retrying for the rest of the session (the caller behaves as
    /// though its own mode were `Offline` from then on).
    Disable,
}

/// Clamp bounds for [`AiGatewayConfig::sanitize`] (anti-chaos, same posture as
/// `atmosphere::bounds`: this config is read from a RON file, an untrusted-ish
/// input surface, so a hostile/buggy value must come out finite and
/// operationally sane rather than propagate to a future real client).
pub mod bounds {
    /// Milliseconds. Floor: below this a real client would spend more time on
    /// connection setup than the timeout allows. Ceiling: a single hung
    /// gateway call must not be allowed to stall a caller for longer than a
    /// minute.
    pub const TIMEOUT_MS: (u32, u32) = (100, 60_000);
}

/// `value` clamped into `(min, max)` (no NaN/inf concept for an unsigned
/// integer — the failure mode this guards is an absurdly small or absurdly
/// large value, e.g. a hand-edited or hostile RON file setting `timeout_ms:
/// 0` or `timeout_ms: 4000000000`).
fn sane_u32(value: u32, (min, max): (u32, u32)) -> u32 { value.clamp(min, max) }

/// RON-loadable AI-gateway configuration (BL-82 EM-4.2e, spec §1.4). Every
/// field has a default so partial files keep loading as the schema grows
/// (`#[serde(default)]`), matching `AtmosphereProfile`'s convention.
///
/// Loading this config, by itself, makes no network call under any
/// [`AiExecutionMode`] — see the module doc comment.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiGatewayConfig {
    /// Which AI backends are currently allowed to be dialed. Defaults to
    /// [`AiExecutionMode::Offline`] — zero AI activity, today's `server-agent`
    /// default AI, unchanged.
    pub mode: AiExecutionMode,
    /// Endpoint URL a future real client would dial (AURORA's local vLLM
    /// endpoint, or left `None` for ORACLE/Bedrock which is addressed via the
    /// AWS SDK's own region/credential resolution, not a bare URL). Read but
    /// never dialed by this crate.
    pub endpoint: Option<String>,
    /// Per-call timeout budget, milliseconds, a future real client would
    /// apply. Clamped into [`bounds::TIMEOUT_MS`] by [`Self::sanitize`].
    pub timeout_ms: u32,
    /// What a future caller does on a timed-out/unavailable call. See the
    /// module doc's fallback-behavior contract.
    pub fallback: FallbackPolicy,
}

impl Default for AiGatewayConfig {
    fn default() -> Self {
        Self {
            mode: AiExecutionMode::Offline,
            endpoint: None,
            timeout_ms: 5_000,
            fallback: FallbackPolicy::DefaultBehavior,
        }
    }
}

impl AiGatewayConfig {
    /// Anti-chaos clamp (spec §1.4): forces `timeout_ms` into
    /// [`bounds::TIMEOUT_MS`]. Runs on every ingestion path (the RON parse
    /// helper below, and any future in-process caller), mirroring
    /// `AtmosphereProfile::sanitize`'s "every ingestion path clamps" rule.
    pub fn sanitize(&mut self) { self.timeout_ms = sane_u32(self.timeout_ms, bounds::TIMEOUT_MS); }

    /// Parses a RON document into a sanitized config (deliberately NOT a full
    /// `bevy_asset` `AssetLoader` — this task ships the config shape, not a
    /// hot-reloadable asset pipeline; a future boot-time loader, or a
    /// `bevy_asset` pipeline analogous to `AtmosphereProfile`'s, can build on
    /// this).
    pub fn from_ron_str(text: &str) -> Result<Self, ron::error::SpannedError> {
        let mut config: Self = ron::from_str(text)?;
        config.sanitize();
        Ok(config)
    }
}

/// The two `/metrics` counters this seam exposes (BL-82 EM-4.2e). Both read
/// `0` forever in this task — nothing increments them, since no real AI call
/// exists yet. Kept as a [`Resource`] so a future real caller can `Res<
/// AiGatewayMetrics>` and increment them without re-registering anything.
#[derive(Resource, Clone)]
pub struct AiGatewayMetrics {
    /// Incremented once per real AI-gateway call attempt (regardless of
    /// outcome), once a real caller exists.
    pub requests_total: IntCounter,
    /// Incremented once per call that hit the [`FallbackPolicy`] path
    /// (timeout/unavailable), once a real caller exists.
    pub fallback_total: IntCounter,
}

/// Registers [`AiGatewayMetrics`]'s two counters on `registry` (the same
/// `prometheus::Registry` `xindeler-server-app::metrics` already serves at
/// `/metrics`). Manual `Opts`/`with_opts`/`registry.register` pattern, matching
/// every other metrics module in this workspace (`server/src/metrics.rs`,
/// `network/src/metrics.rs`) rather than the `prometheus` crate's
/// `register_*_with_registry!` macros, which this codebase never uses.
///
/// # Panics
/// Panics if a metric with either name is already registered on `registry`
/// (a programming error — this function is meant to be called exactly once
/// per registry, at boot, mirroring how `server`'s own metrics are wired).
pub fn register_metrics(registry: &Registry) -> AiGatewayMetrics {
    let requests_total = IntCounter::with_opts(Opts::new(
        "ai_gateway_requests_total",
        "total AI-gateway call attempts (AURORA + ORACLE combined); always 0 until a real client \
         exists (BL-83/BL-85)",
    ))
    .expect("static metric options are always valid");
    registry
        .register(Box::new(requests_total.clone()))
        .expect("ai_gateway_requests_total must not already be registered");

    let fallback_total = IntCounter::with_opts(Opts::new(
        "ai_gateway_fallback_total",
        "total AI-gateway calls that hit the FallbackPolicy path (timeout/unavailable); always 0 \
         until a real client exists (BL-83/BL-85)",
    ))
    .expect("static metric options are always valid");
    registry
        .register(Box::new(fallback_total.clone()))
        .expect("ai_gateway_fallback_total must not already be registered");

    AiGatewayMetrics {
        requests_total,
        fallback_total,
    }
}

/// Wires the AI-gateway seam into a Bevy `App`: inserts a sanitized
/// [`AiGatewayConfig`] (and its `mode` separately as an [`AiExecutionMode`]
/// resource, so a reader that only cares about the mode — e.g. EM-4.2f's
/// mirror — need not depend on the whole config type), and registers the two
/// `/metrics` counters onto `registry`.
///
/// Building this plugin makes no network call — see the module doc comment.
pub struct AiGatewayPlugin {
    /// The config to install. Sanitized on `build` regardless of where it
    /// came from.
    pub config: AiGatewayConfig,
    /// The `prometheus::Registry` to register the two counters onto — the
    /// SAME registry `xindeler-server-app`'s `/metrics` passthrough already
    /// serves (`Server::metrics_registry()`).
    pub registry: std::sync::Arc<Registry>,
}

impl Plugin for AiGatewayPlugin {
    fn build(&self, app: &mut App) {
        let mut config = self.config.clone();
        config.sanitize();
        let metrics = register_metrics(&self.registry);

        bevy::log::info!(
            mode = ?config.mode,
            timeout_ms = config.timeout_ms,
            "ai-gateway seam ready (config/metrics only; makes zero real AI calls)"
        );

        app.insert_resource(config.mode)
            .insert_resource(config)
            .insert_resource(metrics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_offline() {
        assert_eq!(AiGatewayConfig::default().mode, AiExecutionMode::Offline);
    }

    #[test]
    fn default_fallback_is_default_behavior() {
        assert_eq!(
            AiGatewayConfig::default().fallback,
            FallbackPolicy::DefaultBehavior
        );
    }

    #[test]
    fn partial_ron_uses_defaults() {
        let parsed = AiGatewayConfig::from_ron_str("(mode: LocalOnly)").expect("parses");
        assert_eq!(parsed.mode, AiExecutionMode::LocalOnly);
        assert_eq!(parsed.endpoint, None);
        assert_eq!(parsed.timeout_ms, AiGatewayConfig::default().timeout_ms);
        assert_eq!(parsed.fallback, FallbackPolicy::DefaultBehavior);
    }

    #[test]
    fn garbage_timeout_sanitizes_to_bounds() {
        // Absurdly small: a hand-edited/hostile RON file setting `timeout_ms:
        // 0` must not reach a future real client as a zero-timeout call.
        let mut too_small = AiGatewayConfig {
            timeout_ms: 0,
            ..AiGatewayConfig::default()
        };
        too_small.sanitize();
        assert_eq!(too_small.timeout_ms, bounds::TIMEOUT_MS.0);

        // Absurdly large: still a valid u32, but far past any sane timeout
        // (over an hour).
        let mut too_large = AiGatewayConfig {
            timeout_ms: u32::MAX,
            ..AiGatewayConfig::default()
        };
        too_large.sanitize();
        assert_eq!(too_large.timeout_ms, bounds::TIMEOUT_MS.1);

        // Sanitizing an already-sane config is a no-op (double application
        // must not drift — same invariant `AtmosphereProfile::sanitize`
        // documents).
        let mut sane_config = AiGatewayConfig::default();
        sane_config.sanitize();
        assert_eq!(sane_config, AiGatewayConfig::default());
    }

    #[test]
    fn from_ron_str_sanitizes_on_ingestion() {
        let parsed =
            AiGatewayConfig::from_ron_str("(mode: Full, timeout_ms: 999999999)").expect("parses");
        assert_eq!(parsed.mode, AiExecutionMode::Full);
        assert_eq!(parsed.timeout_ms, bounds::TIMEOUT_MS.1);
    }

    #[test]
    fn register_metrics_starts_at_zero() {
        let registry = Registry::new();
        let metrics = register_metrics(&registry);
        assert_eq!(metrics.requests_total.get(), 0);
        assert_eq!(metrics.fallback_total.get(), 0);

        let families = registry.gather();
        let names: Vec<_> = families.iter().map(|f| f.name()).collect();
        assert!(names.contains(&"ai_gateway_requests_total"));
        assert!(names.contains(&"ai_gateway_fallback_total"));
        for family in &families {
            for metric in family.get_metric() {
                assert_eq!(
                    metric.get_counter().get_value(),
                    0.0,
                    "{} must read 0 until a real caller exists",
                    family.name()
                );
            }
        }
    }

    #[test]
    fn plugin_inserts_resources_without_calling_out() {
        use bevy::app::App;

        let registry = std::sync::Arc::new(Registry::new());
        let mut app = App::new();
        app.add_plugins(AiGatewayPlugin {
            config: AiGatewayConfig::default(),
            registry: std::sync::Arc::clone(&registry),
        });

        let mode = *app.world().resource::<AiExecutionMode>();
        assert_eq!(mode, AiExecutionMode::Offline);
        let config = app.world().resource::<AiGatewayConfig>();
        assert_eq!(config.mode, AiExecutionMode::Offline);
        // The metrics resource exists and starts at zero.
        let metrics = app.world().resource::<AiGatewayMetrics>();
        assert_eq!(metrics.requests_total.get(), 0);
        assert_eq!(metrics.fallback_total.get(), 0);
        // Registering onto the SAME registry twice must not happen: the
        // plugin only registered once.
        assert_eq!(registry.gather().len(), 2);
    }
}
