//! BL-82 EM-4.5: `DimensionRegistry` + `DimensionId` + the full
//! `Spinup → Active → Draining → Teardown` instanced-dimension lifecycle
//! (migration spec `2026-07-02-bevy-migration-design.md` §5.3, detailed in
//! `2026-07-10-bl82-phase4-remaining-plan.md` §0.3/§1.8, task board
//! `47-bl82-phase4-remaining-tasks.md` T47.7).
//!
//! ## Why a separate crate, not an extension of `xindeler-sim-bridge`
//! T47.7 explicitly leaves the placement call to the implementer/reviewer.
//! This is a NEW, load-bearing core-resource shape — the first
//! multi-instance-world concept in a codebase that has, until this task,
//! genuinely had zero `Realm`/`Dimension`/`Instance` type anywhere (confirmed
//! 2026-07-10 research against `server/src/lib.rs:542-544`/`:927` and the
//! `world`/`common`/`rtsim` crates). Isolating it in its own crate:
//! - keeps `xindeler-sim-bridge` (already ~1700 lines covering terrain
//!   streaming, entity mirroring, the embedded local player, and LOD broadcast)
//!   reviewable, rather than bolting a second big, independent concern onto it;
//! - lets `xindeler-server-app` depend on dimension machinery WITHOUT also
//!   pulling in `xindeler-sim-bridge`'s embedded-local-player/listen-server
//!   machinery, which is conceptually a `xindeler-client` concern
//!   (`xindeler-server-app` is the dedicated headless shell; it has no embedded
//!   local player and never will);
//! - matches `dm_event.rs`'s OWN documented expectation (EM-4.3/4.4, landed
//!   just before this task): "when EM-4.5 does the real wiring, it must land in
//!   `xindeler-server-app` only, never `xindeler-client`... a crate/ feature
//!   boundary would make that structural rather than doc-comment- only" — a
//!   dedicated crate IS that structural boundary for the dimension-registry
//!   half (though `xindeler-sim-bridge` still gains a THIN, tightly-scoped
//!   dependency on this crate for `DimensionId` tagging on mirrored entities,
//!   since dimension-0's occupants ARE the sim's mirrored entities — see that
//!   crate's own doc update).
//!
//! ## Scope boundary (this task, T47.7)
//! - No GC/despawn PAYLOAD for `Teardown` (EM-4.6/T47.8, next task) — the state
//!   itself, its entry/exit conditions, and its interaction with the
//!   mirror/visibility gate (`DimensionLifecycle::accepts_new_entrants`) are
//!   real and tested here; what happens once a dimension SITS in `Teardown` is
//!   the next task's payload.
//! - No `DmEvent`-triggered auto-spinup (EM-4.9) — [`spinup::SpinupDimension`]
//!   is sent by an explicit debug/admin trigger (see `xindeler-server-app`'s
//!   `dimensions.rs`), not by a `DmEvent` file arriving. It reuses
//!   [`xindeler_oracle_host::DimensionConfig`] specifically so that wiring,
//!   when it lands, is a small addition, not a rework.
//! - No predictive/heuristic auto-drain (EM-4.6/T47.8's `PredictiveGc`) —
//!   `Active -> Draining` is an explicit admin command
//!   ([`spinup::DrainDimension`]) in this task.
//!
//! ## EM-4.6 (T47.8) update, 2026-07-10
//! Both exclusions immediately above are now DONE: the `Teardown` GC payload
//! ([`teardown::teardown_completed_dimensions`], the BL-16 chronicle hook
//! [`teardown::extract_persistent_side_effects_before_teardown`]) and the
//! predictive/heuristic auto-drain ([`predictive_gc`]) are implemented in
//! their own modules below — kept as separate files rather than folded into
//! `registry`/`spinup` so EM-4.5's own reviewed, tested code stays untouched
//! and the new payload's surface area is easy to review on its own. See
//! `teardown`'s module doc for the one deliberate safety addition beyond the
//! literal task text: [`registry::DimensionId::DEFAULT`] is never actually
//! despawned by the GC payload, even though nothing in EM-4.5's own state
//! machine forbids it from reaching `Teardown`.

pub mod component;
pub mod lifecycle;
pub mod plugin;
pub mod predictive_gc;
pub mod registry;
pub mod sim_source;
pub mod spinup;
pub mod teardown;

pub use component::{DimensionMembers, DimensionRoot};
pub use lifecycle::DimensionLifecycle;
pub use plugin::DimensionsPlugin;
pub use predictive_gc::{
    DEFAULT_CONFIG_ASSET_PATH as PREDICTIVE_GC_DEFAULT_CONFIG_ASSET_PATH, PredictiveGc,
    PredictiveGcAsset, PredictiveGcConfigPlugin, PredictiveGcLoader, PredictiveGcTracker,
    PredictiveGcTrackers, predictive_gc_system,
};
pub use registry::{
    DimensionError, DimensionId, DimensionRegistry, DimensionState, IsolationViolation,
    sweep_isolation,
};
pub use sim_source::{read_default_world, wrap_default_dimension};
pub use spinup::{
    DimensionActivated, DimensionSpinupConfig, DrainDimension, SpinupDimension, WorldGenThreadPool,
};
pub use teardown::{
    DimensionTornDown, extract_persistent_side_effects_before_teardown,
    teardown_completed_dimensions,
};
