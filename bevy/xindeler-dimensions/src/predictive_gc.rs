//! BL-82 EM-4.6 (T47.8) predictive GC heuristic — worksheet [Q4]=A, spec
//! §1.9's DELIBERATELY narrow scoping: a **simple, transparent heuristic**,
//! NOT a trained/ML model (there is zero telemetry to train one on, and this
//! module must not pretend otherwise).
//!
//! `Active -> Draining` fires EARLY (before the literal last player leaves)
//! when BOTH:
//! - (a) the dimension's live occupant count has been strictly DECREASING for
//!   the last [`PredictiveGc::decline_window_samples`] (`N`) consecutive
//!   samples, taken every [`PredictiveGc::sample_period`]; AND
//! - (b) no new occupant has arrived in the last
//!   [`PredictiveGc::no_arrival_window`] (`M`) seconds.
//!
//! # These thresholds are PLACEHOLDER defaults
//! [`PredictiveGc::default`]'s `N`/`M`/sample period are NOT tuned against
//! real data — no real instanced-dimension usage exists yet to calibrate
//! against (spec §1.9 says this explicitly, twice, on purpose). Expect to
//! retune them by hand once EM-4.9's E2E drill and any real usage after it
//! produce actual session-length data. This module deliberately does NOT
//! auto-tune `N`/`M` — that is explicitly out of scope (spec §1.9's own
//! scope boundary) until real telemetry exists to justify it.
//!
//! ## BL-82 EM-4.10 T48.6 — these placeholders are now RON-retunable
//! [`PredictiveGc`] is genuinely LIVE IN PRODUCTION the moment more than one
//! dimension exists (`DimensionsPlugin` is unconditionally added by
//! `xindeler-server-app::SimServerPlugin`) — so these were production balance
//! numbers living as compiled-in Rust constants, against this project's
//! data-driven-content convention. [`PredictiveGcAsset`] +
//! [`PredictiveGcLoader`] load the same shape from a RON file
//! (`assets/xindeler/dimensions/default.predictive_gc.ron`), mirroring
//! `xindeler_oracle_host::atmosphere`'s `AtmosphereProfile`/
//! `AtmosphereProfileLoader` split exactly (minus the interpolation — a
//! config reload here just replaces [`PredictiveGc`] outright, there is
//! nothing to animate). [`PredictiveGcConfigPlugin`] is the opt-in Bevy
//! wiring; see its doc comment for why it is deliberately SEPARATE from
//! [`crate::plugin::DimensionsPlugin`] (which keeps inserting this module's
//! compiled-in `Default` as the sane fallback the task's own acceptance bar
//! asks for).
//!
//! The decision logic itself ([`PredictiveGcTracker`]) is a small, pure,
//! wall-clock-independent state machine — deliberately factored out of the
//! Bevy system ([`predictive_gc_system`]) so the heuristic's exact firing
//! behavior is unit-testable against a SCRIPTED sequence of samples (spec
//! §1.9's own acceptance bar) without waiting out real `sample_period`/
//! `no_arrival_window` durations in a test.

use std::{collections::HashMap, time::Duration};

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    prelude::*,
    reflect::TypePath,
};
use serde::{Deserialize, Serialize};

use crate::{
    component::DimensionId, lifecycle::DimensionLifecycle, registry::DimensionRegistry,
    spinup::DrainDimension,
};

/// Config for [`PredictiveGcTracker`]/[`predictive_gc_system`]. See the
/// module doc comment for the full "these are placeholder defaults" caveat —
/// repeated here, on the type itself, so it surfaces in generated docs
/// wherever this struct is referenced, not just in the module-level prose.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictiveGc {
    /// `N`: number of consecutive samples of a STRICTLY decreasing occupant
    /// count required before the "declining" half of the heuristic is
    /// satisfied.
    pub decline_window_samples: u32,
    /// How often a sample is taken (spec §1.9's "e.g. every 30s").
    pub sample_period: Duration,
    /// `M`: no occupant may have arrived within this many seconds, in
    /// addition to the decline window, for the heuristic to fire.
    pub no_arrival_window: Duration,
}

impl Default for PredictiveGc {
    /// **Placeholder defaults, not tuned against real data.** Real
    /// calibration requires production telemetry on actual
    /// instanced-dimension session lengths, which does not exist yet —
    /// expect to retune once EM-4.9's drill and any real usage after it
    /// produce data (spec §1.9, verbatim rationale).
    fn default() -> Self {
        Self {
            decline_window_samples: 3,
            sample_period: Duration::from_secs(30),
            no_arrival_window: Duration::from_secs(120),
        }
    }
}

// ---------------------------------------------------------------------------
// RON asset (BL-82 EM-4.10 T48.6) — see the module doc's own section
// ---------------------------------------------------------------------------

/// Canonical asset path (relative to the `assets/` source root) of the
/// shipped config [`PredictiveGcConfigPlugin`] boots with. Values MUST equal
/// [`PredictiveGc::default`] (unit-tested below) so the first load is a
/// behavior-preserving no-op, exactly like `default.atmo.ron`'s own
/// same-values-as-the-Rust-`Default` contract.
pub const DEFAULT_CONFIG_ASSET_PATH: &str = "xindeler/dimensions/default.predictive_gc.ron";

/// RON-loadable shape of [`PredictiveGc`]'s three tuning constants. Kept as a
/// SEPARATE type from `PredictiveGc` itself (rather than making `PredictiveGc`
/// an `Asset`) so `PredictiveGc` stays a plain `Resource` every existing unit
/// test in this module already constructs directly (`PredictiveGc { .. }`) —
/// this newtype is only the `bevy_asset` boundary, mirroring
/// `xindeler_oracle_host::atmosphere`'s `AtmosphereProfile`/
/// `AtmosphereController` split.
///
/// Durations round-trip as whole seconds (`u64`), not `std::time::Duration`
/// directly — this codebase has no established `serde`-for-`Duration`
/// convention, and `AtmosphereProfile::transition_secs` already establishes
/// "seconds as a plain number" as the house style for RON-authored durations.
/// Whole seconds are ample precision for tuning a heuristic that operates on
/// minutes-scale windows.
#[derive(Asset, TypePath, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PredictiveGcAsset {
    pub decline_window_samples: u32,
    pub sample_period_secs: u64,
    pub no_arrival_window_secs: u64,
}

impl Default for PredictiveGcAsset {
    fn default() -> Self {
        let defaults = PredictiveGc::default();
        Self {
            decline_window_samples: defaults.decline_window_samples,
            sample_period_secs: defaults.sample_period.as_secs(),
            no_arrival_window_secs: defaults.no_arrival_window.as_secs(),
        }
    }
}

impl From<PredictiveGcAsset> for PredictiveGc {
    fn from(asset: PredictiveGcAsset) -> Self {
        Self {
            // `.max(1)`: `PredictiveGcTracker::observe` already defends
            // against 0 internally, but a hand-edited/hostile RON file
            // setting `decline_window_samples: 0` should read as "1" at the
            // config boundary too, not merely be silently tolerated deeper
            // in the pipeline (same "clamp on the way in" posture every
            // other RON-loaded config in `bevy/` documents, e.g.
            // `AiGatewayConfig::sanitize`).
            decline_window_samples: asset.decline_window_samples.max(1),
            sample_period: Duration::from_secs(asset.sample_period_secs),
            no_arrival_window: Duration::from_secs(asset.no_arrival_window_secs),
        }
    }
}

/// Async [`AssetLoader`] for `predictive_gc.ron` (mirrors
/// `AtmosphereProfileLoader`/`EntityTemplateLoader`'s shape exactly).
#[derive(Default, TypePath)]
pub struct PredictiveGcLoader;

impl AssetLoader for PredictiveGcLoader {
    type Asset = PredictiveGcAsset;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let asset: PredictiveGcAsset = ron::de::from_bytes(&bytes)?;
        Ok(asset)
    }

    fn extensions(&self) -> &[&str] { &["predictive_gc.ron"] }
}

/// Holds the strong handle keeping the loaded config (and its file watch, if
/// the host app's `AssetPlugin` has `file_watcher` on) alive — same
/// rationale `AtmosphereController::handle` documents.
#[derive(Resource)]
struct PredictiveGcConfigHandle(Handle<PredictiveGcAsset>);

/// Registers the [`PredictiveGcAsset`] RON asset pipeline and keeps the live
/// [`PredictiveGc`] resource in sync with whatever [`Self::config_path`]
/// resolves to, including a hot-reloaded edit (when the host app's
/// `AssetPlugin` has `file_watcher` active) — so retuning these constants
/// never requires a rebuild.
///
/// Requires an `AssetPlugin` already present in the host `App` (same
/// contract [`xindeler_oracle_host::atmosphere::XindelerAtmospherePlugin`]
/// documents).
///
/// ## Deliberately SEPARATE from [`crate::plugin::DimensionsPlugin`]
/// `DimensionsPlugin` is added by dozens of existing tests via bare
/// `MinimalPlugins` (no `AssetPlugin`/`AssetServer`) — folding asset
/// registration into it directly would panic every one of them
/// (`init_asset_loader` requires `AssetServer` to already exist). A host app
/// that HAS an `AssetPlugin` (the real dedicated-server shell,
/// `xindeler-server-app`) adds THIS plugin in ADDITION to `DimensionsPlugin`;
/// a bare test that only wants the registry/lifecycle machinery keeps getting
/// `DimensionsPlugin`'s own `PredictiveGc::default()` untouched — the "sane
/// fallback `Default` for tests that don't want to load an asset" this
/// task's own acceptance bar asks for.
pub struct PredictiveGcConfigPlugin {
    /// Asset path (relative to the asset source root) of the config to boot
    /// with.
    pub config_path: String,
}

impl Default for PredictiveGcConfigPlugin {
    fn default() -> Self {
        Self {
            config_path: DEFAULT_CONFIG_ASSET_PATH.to_owned(),
        }
    }
}

impl Plugin for PredictiveGcConfigPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<PredictiveGcAsset>()
            .init_asset_loader::<PredictiveGcLoader>()
            .add_systems(FixedUpdate, apply_predictive_gc_config);

        let path = self.config_path.clone();
        app.add_systems(
            Startup,
            move |asset_server: Res<AssetServer>, mut commands: Commands| {
                commands.insert_resource(PredictiveGcConfigHandle(asset_server.load(path.clone())));
            },
        );
    }
}

/// Applies a loaded/reloaded [`PredictiveGcAsset`] onto the live
/// [`PredictiveGc`] resource. A no-op until the handle resolves (`Startup`'s
/// `asset_server.load` is async) — [`predictive_gc_system`] keeps reading
/// [`crate::plugin::DimensionsPlugin`]'s compiled-in default until then,
/// exactly the behavior-preserving boot sequence this task's acceptance bar
/// asks for. Lives in `FixedUpdate` (not `Update`), matching every other
/// sim-bookkeeping system in this crate post-EM-4.10 Finding B.
fn apply_predictive_gc_config(
    handle: Option<Res<PredictiveGcConfigHandle>>,
    assets: Res<Assets<PredictiveGcAsset>>,
    mut events: MessageReader<AssetEvent<PredictiveGcAsset>>,
    mut config: ResMut<PredictiveGc>,
) {
    let Some(handle) = handle else { return };
    let relevant = events.read().any(|event| match event {
        AssetEvent::Added { id } | AssetEvent::Modified { id } => *id == handle.0.id(),
        _ => false,
    });
    if !relevant {
        return;
    }
    if let Some(asset) = assets.get(&handle.0) {
        let converted = PredictiveGc::from(*asset);
        // Skip identical reloads (e.g. a file rewritten with unchanged
        // content) so `ResMut::deref_mut` doesn't dirty the change tick for
        // no actual change — same "check-before-write" discipline
        // `AtmosphereController::is_settled_at` documents for its own
        // reload path.
        if converted == *config {
            return;
        }
        *config = converted;
        tracing::info!(
            decline_window_samples = config.decline_window_samples,
            sample_period_secs = config.sample_period.as_secs(),
            no_arrival_window_secs = config.no_arrival_window.as_secs(),
            "predictive_gc: applied config from RON asset"
        );
    }
}

/// A small, pure, wall-clock-independent state machine implementing the
/// heuristic decision for ONE dimension. No Bevy dependency at all (mirrors
/// `crate::registry::sweep_isolation`'s "pure function, thin system wrapper"
/// style) — feed it `(elapsed_time, occupant_count)` observations and it
/// tells you, deterministically, exactly when the heuristic's trigger
/// conditions are both satisfied.
#[derive(Debug, Clone)]
pub struct PredictiveGcTracker {
    config: PredictiveGc,
    /// Occupant counts sampled every `sample_period`, oldest first, capped at
    /// `decline_window_samples` entries (older samples are dropped).
    recent_samples: Vec<usize>,
    /// Elapsed time of the last observed INCREASE in occupant count (an
    /// "arrival") — checked on every `observe()` call regardless of the
    /// sampling cadence, so a very recent arrival always blocks a fire even
    /// between two sample ticks.
    last_arrival_at: Option<Duration>,
    last_known_count: Option<usize>,
    last_sampled_at: Option<Duration>,
}

impl PredictiveGcTracker {
    pub fn new(config: PredictiveGc) -> Self {
        Self {
            config,
            recent_samples: Vec::new(),
            last_arrival_at: None,
            last_known_count: None,
            last_sampled_at: None,
        }
    }

    /// Feeds one observation at elapsed time `now` with the dimension's
    /// current occupant `count`. Returns `true` exactly the first instant
    /// both trigger conditions hold — callers should treat that as a one-shot
    /// signal (the real system removes the tracker once it fires, since the
    /// dimension leaves `Active` and the heuristic no longer applies).
    ///
    /// `now` must be non-decreasing across calls (a real wall clock only
    /// moves forward); this is a precondition, not defensively checked,
    /// since both callers (the Bevy system, driven by `Time::elapsed()`, and
    /// tests, driven by a manually-incremented `Duration`) already uphold it
    /// by construction.
    pub fn observe(&mut self, now: Duration, count: usize) -> bool {
        // Track arrivals on EVERY observation, not just sampled ones, so a
        // last-second arrival always resets the no-arrival clock immediately
        // rather than waiting for the next sample tick to notice it.
        match self.last_known_count {
            Some(last) if count > last => self.last_arrival_at = Some(now),
            // First observation ever: a dimension is only `Active` (and thus
            // only ever observed) once it has at least one occupant, so
            // treat the very first sighting as the baseline arrival — the
            // no-arrival clock starts from dimension creation, not from an
            // undefined state.
            None => self.last_arrival_at = Some(now),
            _ => {},
        }
        self.last_known_count = Some(count);

        let should_sample = self
            .last_sampled_at
            .is_none_or(|t| now.saturating_sub(t) >= self.config.sample_period);
        if !should_sample {
            return false;
        }
        self.last_sampled_at = Some(now);
        self.recent_samples.push(count);
        let cap = self.config.decline_window_samples.max(1) as usize;
        if self.recent_samples.len() > cap {
            let excess = self.recent_samples.len() - cap;
            self.recent_samples.drain(0..excess);
        }

        let has_enough_samples = self.recent_samples.len() >= cap;
        let strictly_decreasing =
            has_enough_samples && self.recent_samples.windows(2).all(|w| w[0] > w[1]);
        let no_recent_arrival = self
            .last_arrival_at
            .is_none_or(|t| now.saturating_sub(t) >= self.config.no_arrival_window);

        strictly_decreasing && no_recent_arrival
    }
}

/// Per-dimension [`PredictiveGcTracker`]s, keyed by dimension. Entries are
/// created lazily on first observation and removed once a dimension either
/// fires (leaves `Active`) or stops being `Active` for any other reason
/// (drained/torn down by an admin command, or simply removed from the
/// registry) — a stale tracker for a dimension that no longer exists (or
/// re-entered `Spinup` under a REUSED id, which never happens today but
/// would be a correctness trap if it ever did) must never linger.
#[derive(Resource, Default)]
pub struct PredictiveGcTrackers(HashMap<DimensionId, PredictiveGcTracker>);

/// Bevy-system wrapper: feeds every currently-`Active`, non-default
/// dimension's occupant count into its [`PredictiveGcTracker`] once per
/// frame, and requests `Active -> Draining` (via [`DrainDimension`], the SAME
/// admin-command message [`crate::spinup::handle_drain_requests`] already
/// consumes — no parallel transition path) the instant the heuristic fires.
///
/// **[`DimensionId::DEFAULT`] is deliberately EXCLUDED** — the always-on
/// default dimension's occupant count legitimately drops to a handful (or
/// zero, overnight) as part of normal, healthy play; predictively draining
/// (and thus eventually tearing down — see `crate::teardown`'s module doc)
/// the persistent game world itself would be catastrophic. This heuristic
/// only ever applies to actual INSTANCED dimensions (Mist-Bound-style
/// event/dungeon instances), which is the entire point of spec §1.9's
/// mechanism in the first place.
pub fn predictive_gc_system(
    time: Res<Time>,
    config: Res<PredictiveGc>,
    mut trackers: ResMut<PredictiveGcTrackers>,
    registry: Res<DimensionRegistry>,
    mut drain_requests: MessageWriter<DrainDimension>,
) {
    let now = time.elapsed();
    let active_ids: Vec<DimensionId> = registry
        .ids()
        .filter(|&id| {
            id != DimensionId::DEFAULT && registry.lifecycle(id) == Some(DimensionLifecycle::Active)
        })
        .collect();

    // Drop trackers for dimensions that are no longer Active (drained by an
    // admin command, torn down, or otherwise gone) — see the resource's own
    // doc comment for why a stale tracker must not linger.
    trackers.0.retain(|id, _| active_ids.contains(id));

    for id in active_ids {
        let Some(state) = registry.get(id) else {
            continue;
        };
        let count = state.occupant_count();
        let tracker = trackers
            .0
            .entry(id)
            .or_insert_with(|| PredictiveGcTracker::new(*config));

        if tracker.observe(now, count) {
            tracing::info!(
                ?id,
                occupant_count = count,
                "predictive GC firing: occupant count declining with no recent arrivals \
                 (placeholder N/M thresholds — see predictive_gc.rs's module doc)"
            );
            drain_requests.write(DrainDimension(id));
            trackers.0.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PredictiveGc {
        PredictiveGc {
            decline_window_samples: 3,
            sample_period: Duration::from_secs(30),
            no_arrival_window: Duration::from_secs(60),
        }
    }

    /// The core acceptance bar (spec §1.9): a scripted DECREASING population
    /// sequence fires the heuristic strictly BEFORE the last player leaves
    /// (i.e. while `count` is still > 0).
    #[test]
    fn fires_before_the_last_player_leaves_under_a_declining_sequence() {
        let mut tracker = PredictiveGcTracker::new(config());
        let mut fired_at: Option<(Duration, usize)> = None;

        // t=0: 10 occupants (baseline arrival). t=30: 8. t=60: 6. t=90: 4.
        // t=120: 2. t=150: 0 (literal last-player-leaves point).
        let schedule = [
            (0u64, 10usize),
            (30, 8),
            (60, 6),
            (90, 4),
            (120, 2),
            (150, 0),
        ];
        for (secs, count) in schedule {
            let now = Duration::from_secs(secs);
            if tracker.observe(now, count) && fired_at.is_none() {
                fired_at = Some((now, count));
            }
        }

        let (fired_at, count_when_fired) =
            fired_at.expect("a strictly-declining, arrival-free sequence must fire");
        assert!(
            count_when_fired > 0,
            "must fire while occupants are still present (count={count_when_fired}), not only \
             once the last one has already left"
        );
        assert!(
            fired_at < Duration::from_secs(150),
            "must fire strictly before the last-player-leaves instant (t=150s)"
        );
    }

    /// No false positives: a STABLE population never fires.
    #[test]
    fn does_not_fire_for_a_stable_population() {
        let mut tracker = PredictiveGcTracker::new(config());
        for i in 0..20 {
            let now = Duration::from_secs(i * 30);
            assert!(
                !tracker.observe(now, 5),
                "a flat, stable occupant count must never fire the heuristic"
            );
        }
    }

    /// No false positives: a GROWING population never fires, even with some
    /// noise (a temporary dip that reverses before it becomes a genuine
    /// decline).
    #[test]
    fn does_not_fire_for_a_growing_population() {
        let mut tracker = PredictiveGcTracker::new(config());
        let schedule = [
            (0u64, 2usize),
            (30, 4),
            (60, 6),
            (90, 5), // one-sample dip
            (120, 8),
            (150, 10),
        ];
        for (secs, count) in schedule {
            assert!(
                !tracker.observe(Duration::from_secs(secs), count),
                "a growing (or dip-then-recovering) population must never fire"
            );
        }
    }

    /// A decline that STOPS because a new arrival resets the no-arrival
    /// clock must not fire, even though the SAMPLED history stays
    /// "declining enough" throughout — condition (b) alone must gate it.
    ///
    /// Uses a wider `no_arrival_window` (100s, vs. the shared `config()`
    /// helper's 60s) than the sample cadence needs on its own, specifically
    /// so condition (a) (3 declining samples) becomes true STRICTLY before
    /// condition (b) (no arrival in the last 100s) would — that gap is what
    /// makes it possible to isolate condition (b) as the one thing an
    /// intervening arrival resets, without also disturbing the sampled
    /// decline trend (the arrival itself lands BETWEEN two sample ticks, at
    /// a value that keeps every actually-sampled reading monotonically
    /// decreasing).
    #[test]
    fn a_recent_arrival_blocks_firing_even_during_a_decline() {
        let cfg = PredictiveGc {
            decline_window_samples: 3,
            sample_period: Duration::from_secs(30),
            no_arrival_window: Duration::from_secs(100),
        };
        let mut tracker = PredictiveGcTracker::new(cfg);
        // Sampled trend: 10 -> 8 -> 6 -> 4 -> 2, strictly decreasing
        // throughout. Condition (a) is satisfied from t=60 onward (3
        // samples); condition (b) alone (no_arrival_window=100s from the
        // t=0 baseline arrival) would only become true at t>=100 — i.e.
        // starting at the t=120 sample below, absent any new arrival.
        assert!(!tracker.observe(Duration::from_secs(0), 10));
        assert!(!tracker.observe(Duration::from_secs(30), 8));
        assert!(!tracker.observe(Duration::from_secs(60), 6));
        assert!(
            !tracker.observe(Duration::from_secs(90), 4),
            "condition (b) (no arrival in the last 100s from the t=0 baseline) isn't satisfied \
             yet at t=90"
        );
        // A genuine arrival (count INCREASES vs. the immediately previous
        // observation) lands between sample ticks — not itself a sample
        // (95-90=5s < the 30s sample_period) — resetting the no-arrival
        // clock to t=95.
        assert!(!tracker.observe(Duration::from_secs(95), 5));
        // Without that arrival, t=120 (120-0=120s >= 100s) would have fired
        // (see `fires_before_the_last_player_leaves_under_a_declining_sequence`
        // for that unblocked case) — the sampled value here (2) still keeps
        // the decline chain intact (6 -> 4 -> 2), so condition (a) alone
        // would otherwise be satisfied.
        assert!(
            !tracker.observe(Duration::from_secs(120), 2),
            "a recent arrival (t=95) must block firing at t=120 even though the sampled decline \
             trend (6 -> 4 -> 2) still reads as satisfying condition (a) alone"
        );
    }

    /// Doesn't fire before enough samples exist yet (can't judge "strictly
    /// decreasing for N samples" with fewer than N samples).
    #[test]
    fn does_not_fire_before_enough_samples_exist() {
        let mut tracker = PredictiveGcTracker::new(config());
        assert!(!tracker.observe(Duration::from_secs(0), 10));
        assert!(!tracker.observe(Duration::from_secs(30), 8));
        // Only 2 samples so far; window is 3 — must not fire yet.
    }

    /// [`predictive_gc_system`]'s own exclusion: `DimensionId::DEFAULT` must
    /// never receive a `DrainDimension` request from this system, even under
    /// an aggressively declining occupant count — the EXACT sample shape
    /// that would fire the heuristic for any other dimension (verified by
    /// this test using a real Bevy `App` + real (tiny) sleeps, so the
    /// `sample_period`/`no_arrival_window` cadence is driven by the SAME
    /// `TimePlugin`-updated `Res<Time>` the production system reads — no
    /// manual clock-poking that could silently diverge from real behavior).
    #[test]
    fn system_never_predictively_drains_the_default_dimension() {
        use bevy::{MinimalPlugins, app::PluginGroup};

        use crate::{plugin::DimensionsPlugin, spinup::DrainDimension};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);
        app.insert_resource(PredictiveGc {
            decline_window_samples: 2,
            sample_period: Duration::from_millis(1),
            no_arrival_window: Duration::from_millis(1),
        });
        app.init_resource::<PredictiveGcTrackers>();
        app.add_systems(Update, predictive_gc_system);

        let root = app.world_mut().spawn(DimensionId::DEFAULT).id();
        let mut occupants = Vec::new();
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry
                .insert_spinning_up(DimensionId::DEFAULT, root, 0)
                .unwrap();
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(DimensionId::DEFAULT, std::sync::Arc::new(world), index)
                .unwrap();
            for i in 0..6u32 {
                let dummy = Entity::from_raw_u32(i + 1).unwrap();
                registry
                    .try_add_occupant(DimensionId::DEFAULT, dummy)
                    .unwrap();
                occupants.push(dummy);
            }
        }
        app.update(); // let the first (full-population) sample land

        // Aggressively decline the DEFAULT dimension's occupant count across
        // several real frames, with real (tiny) sleeps so each `app.update()`
        // crosses the (deliberately minuscule) sample_period/no_arrival_window
        // — the exact declining shape that fires the heuristic for a
        // non-default dimension (see the `PredictiveGcTracker` tests above).
        for dummy in occupants {
            app.world_mut()
                .resource_mut::<DimensionRegistry>()
                .remove_occupant(DimensionId::DEFAULT, dummy)
                .ok();
            std::thread::sleep(Duration::from_millis(2));
            app.update();
        }

        let drains = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<DrainDimension>>();
        assert!(
            drains.is_empty(),
            "DimensionId::DEFAULT must never receive a predictive DrainDimension request"
        );
    }

    /// BL-82 EM-4.10 T48.6 acceptance: [`PredictiveGcAsset::default`] (what a
    /// missing/empty RON file falls back to) converts to EXACTLY
    /// [`PredictiveGc::default`] — the "shipped defaults are behavior-
    /// preserving" contract, checked at the type level rather than only by
    /// the shipped file's literal contents (the next test covers that).
    #[test]
    fn predictive_gc_asset_default_round_trips_to_predictive_gc_default() {
        let converted: PredictiveGc = PredictiveGcAsset::default().into();
        assert_eq!(converted, PredictiveGc::default());
    }

    /// The shipped `default.predictive_gc.ron` asset parses (via a plain RON
    /// deserialize, no `AssetServer` needed) and converts to EXACTLY
    /// [`PredictiveGc::default`] — mirrors
    /// `entity_template::tests::shipped_sample_templates_parse_and_are_already_sane`'s
    /// "the checked-in fixture, not just the type default" rigor.
    #[test]
    fn shipped_default_config_parses_and_matches_predictive_gc_default() {
        let text = include_str!("../../../assets/xindeler/dimensions/default.predictive_gc.ron");
        let parsed: PredictiveGcAsset =
            ron::from_str(text).expect("default.predictive_gc.ron parses");
        let converted: PredictiveGc = parsed.into();
        assert_eq!(
            converted,
            PredictiveGc::default(),
            "the shipped RON file must carry the SAME values as PredictiveGc::default() so the \
             first load is a behavior-preserving no-op"
        );
    }

    /// BL-82 EM-4.10 T48.6's actual bar: "boots with identical values from
    /// RON" — proven end-to-end with a REAL `AssetServer` reading the REAL
    /// shipped file off disk (not `include_str!`), starting from a
    /// deliberately WRONG `PredictiveGc` so the assertion can only pass if
    /// the shipped asset genuinely overwrote it (a silently-failed load would
    /// leave the wrong value in place and fail this test).
    #[test]
    #[ignore = "boots a real AssetServer against the shipped assets dir; run locally with \
                VELOREN_ASSETS=\"$(pwd)/assets\" cargo test -p xindeler-dimensions -- --ignored"]
    fn shipped_predictive_gc_config_loads_via_a_real_asset_server() {
        use std::path::PathBuf;

        use bevy::{MinimalPlugins, app::PluginGroup, asset::AssetPlugin};

        let assets_root = std::env::var_os("XINDELER_ASSETS")
            .or_else(|| std::env::var_os("VELOREN_ASSETS"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map_or_else(|_| PathBuf::from("assets"), |cwd| cwd.join("assets"))
            });

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(AssetPlugin {
            file_path: assets_root.to_string_lossy().into_owned(),
            ..Default::default()
        });
        // Deliberately wrong: only a genuine load of the shipped RON file
        // (not merely leaving the Rust `Default` alone) can turn this into
        // `PredictiveGc::default()` below.
        app.insert_resource(PredictiveGc {
            decline_window_samples: 999,
            sample_period: Duration::from_secs(1),
            no_arrival_window: Duration::from_secs(1),
        });
        app.add_plugins(PredictiveGcConfigPlugin::default());

        let mut loaded = false;
        for _ in 0..500 {
            app.update();
            if *app.world().resource::<PredictiveGc>() == PredictiveGc::default() {
                loaded = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            loaded,
            "the shipped default.predictive_gc.ron asset should have loaded and overwritten the \
             deliberately-wrong initial PredictiveGc within ~2.5s"
        );
    }
}
