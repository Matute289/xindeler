//! Per-client interest management (BL-82 EM-4.2d, task board T47.6): the
//! "new visibility system" wiring [`visibility::RegionKey`]'s `bevy_replicon`
//! filter to each connected client's own [`ClientViewpoint`]. Complements
//! [`crate::visibility`] (the passive data SHAPES this module's ACTIVE
//! system consumes/produces).
//!
//! ## Why this lives in `xindeler-protocol`, not `xindeler-server-app`
//! `xindeler-server-app` is a binary-only package (no `[lib]` target), so its
//! own `tests/` integration tests cannot `use` anything from its `src/` at
//! all — every existing test there proves behavior by spawning the compiled
//! binary as a subprocess (`CARGO_BIN_EXE_xindeler-server-app`) instead.
//! [`recompute_client_visible_regions`] is exactly the kind of pure,
//! ECS-only logic this task needs to unit-test directly (no real sim, no
//! subprocess, no assets) — so it lives here, in the crate that IS a proper
//! library and that `xindeler-server-app` already depends on, exactly the
//! same reasoning `ai_mode.rs`'s own doc comment gives for hosting
//! `AiExecutionMode` here rather than in a crate that needed a new backwards
//! edge to reach it. `xindeler-server-app::plugin::SimServerPlugin` just adds
//! [`ClientInterestPlugin`] — a normal forward dependency, no new edge.
//!
//! This module IS server-only in spirit (nothing on the client ever inserts
//! a [`ClientViewpoint`] or gains anything from [`ClientInterestPlugin`]),
//! but — like the rest of `xindeler_replicon`'s "both roles compile into
//! every shell" pattern (see `xindeler-transport`'s crate doc comment) — it
//! is harmless, dormant dead weight if a client `App` ever added it by
//! mistake, so no `#[cfg]` gate is needed to keep it correct.
//!
//! ## `CHUNK_FUZZ` — a deliberate, documented duplication
//! `server::presence::CHUNK_FUZZ` (`= 2`) is the exact constant
//! `server/src/sys/subscription.rs::Sys::run` uses for its own
//! chunk-boundary-crossing fuzz border. This module reproduces the same
//! value as [`CHUNK_FUZZ`] rather than importing it from the `server` crate,
//! because `xindeler-protocol` is deliberately the low-level, few-dependency
//! wire/shared crate every Bevy shell links (client AND server) — adding a
//! dependency on `server` (a large, database/quinn/persistence-heavy crate)
//! just for one `u32` constant would be a backwards architectural weight,
//! not a small edge. If `server::presence::CHUNK_FUZZ` is ever retuned, this
//! constant must be updated to match by hand — there is no compiler
//! enforcement of that link, only this doc comment and the mirrored value.

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        system::{Commands, Query},
    },
    prelude::{App, FixedUpdate, Plugin},
};
use common::{
    region::regions_in_vd,
    terrain::{CoordinateConversions, TerrainChunkSize},
    vol::RectVolSize,
};
use vek::Vec2;

use crate::visibility::{ClientVisibleRegions, DimensionId};

/// Mirrors `server::presence::CHUNK_FUZZ` exactly — see this module's doc
/// comment for why it is duplicated rather than imported. `pub(crate)` (not
/// private) so a cross-crate drift guard can compare it against the real
/// constant: `xindeler-sim-bridge` already depends on both `server` and this
/// crate (unlike this crate, which deliberately does NOT depend on `server`
/// — see the module doc comment), so it is the natural home for
/// `assert_eq!(xindeler_protocol::interest::chunk_fuzz(),
/// server::presence::CHUNK_FUZZ)`.
pub(crate) const CHUNK_FUZZ: u32 = 2;

/// Reads [`CHUNK_FUZZ`] from outside this crate (e.g.
/// `xindeler-sim-bridge`'s own drift-guard test) without making the constant
/// itself part of this crate's public API surface.
#[doc(hidden)]
#[must_use]
pub const fn chunk_fuzz() -> u32 { CHUNK_FUZZ }

/// Registers [`recompute_client_visible_regions`] in `FixedUpdate` — the same
/// schedule `xindeler-sim-bridge`'s mirror/terrain systems run in on the
/// dedicated-server shell, so both settle before `bevy_replicon`'s own
/// default `FixedPostUpdate` replication pass (see
/// `xindeler-server-app::plugin::SimServerPlugin`'s own doc comment for why
/// `RepliconPlugins` is added unconfigured/library-default there).
///
/// Ordering relative to `xindeler-sim-bridge`'s `RegionKey`-writing mirror
/// system does NOT matter for correctness: visibility re-evaluation is driven
/// by `bevy_replicon`'s own Insert/Remove observers on `RegionKey`/
/// `ClientVisibleRegions` (see [`crate::visibility`]'s doc comment), not by
/// which system runs first within `FixedUpdate` — both converge to the same
/// final `ClientVisibility` state before `FixedPostUpdate` reads it,
/// regardless of intra-`FixedUpdate` system order.
pub struct ClientInterestPlugin;

impl Plugin for ClientInterestPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, recompute_client_visible_regions);
    }
}

/// Per connected-client component: the world position + view distance to
/// scope that client's replicated-entity visibility around (BL-82 EM-4.2d).
///
/// ## What populates this today
/// There is no login/session handshake yet (EM-4.2c — a SIBLING Wave-2 task,
/// not a dependency of this one): nothing in this codebase maps "a connected
/// replicon client" to "the sim/mirrored player entity it controls", so
/// there is no REAL source for a connecting client's own world position yet.
/// Without ANY producer at all, a client with no `ClientViewpoint` would stay
/// BLIND — an empty [`ClientVisibleRegions`], see that type's own doc
/// comment — which would silently regress every existing no-login
/// acceptance path (this reviewer-caught risk is exactly why
/// `xindeler-sim-bridge::apply_default_viewpoint_for_new_clients` exists): it
/// grants a spectator-style DEFAULT viewpoint (world-centre,
/// `ANCHOR_VIEW_DISTANCE`) to any newly-connected client that doesn't
/// already have one, mirroring the terrain anchor's own "no real player yet
/// → sane spectator fallback" posture.
///
/// `ClientViewpoint` is still a small, public, documented HOOK for the REAL
/// future producer: EM-4.2c's login system, once it resolves a connecting
/// client to its own mirrored player entity and can read that entity's
/// position each tick (or, alternatively, a future client-reported
/// spectator/camera position — either way, [`recompute_client_visible_regions`]
/// only needs SOME `ClientViewpoint`, it does not care where it came from).
/// Once EM-4.2c (or a test, like
/// `xindeler-server-app/tests/interest_management.rs`) inserts a REAL one,
/// the default-viewpoint stopgap never touches that client again (it only
/// acts on clients with none).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ClientViewpoint {
    /// Which dimension this viewpoint's [`Self::pos`] is expressed in — see
    /// [`crate::visibility`]'s module doc comment for the EM-4.5 extension
    /// point this threads through to [`crate::visibility::RegionKey`].
    pub dimension: DimensionId,
    /// World-space XY position (sim axes: x-east, y-north — the SAME
    /// convention `common::region::regions_in_vd` takes), NOT Bevy-axis-
    /// converted.
    pub pos: Vec2<f32>,
    /// View distance in CHUNKS (matches `common::ViewDistances`/
    /// `Presence::entity_view_distance`'s own unit — the legacy stack's
    /// `RegionSubscription` uses the identical unit for the identical
    /// `regions_in_vd` call).
    pub view_distance: u32,
}

impl ClientViewpoint {
    #[must_use]
    pub fn new(dimension: DimensionId, pos: Vec2<f32>, view_distance: u32) -> Self {
        Self {
            dimension,
            pos,
            view_distance,
        }
    }
}

/// Cached recompute-trigger state, mirroring
/// `server::presence::RegionSubscription`'s own `fuzzy_chunk`/
/// `last_entity_view_distance` fields exactly, so
/// [`recompute_client_visible_regions`] fires on the identical condition the
/// legacy system's own `Sys::run` does. Not `pub`: purely this module's
/// internal bookkeeping, never read outside
/// [`recompute_client_visible_regions`].
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct RegionRecomputeState {
    fuzzy_chunk: Vec2<i32>,
    last_view_distance: u32,
}

/// The BL-82 EM-4.2d visibility system. For every connected client entity
/// carrying a [`ClientViewpoint`], recomputes and re-inserts
/// [`ClientVisibleRegions`] (replacing the previous value wholesale) exactly
/// when the legacy stack's own chunk-boundary-crossing trigger
/// (`server/src/sys/subscription.rs::Sys::run`) would fire.
///
/// Unlike that legacy system — which diffs regions in/out incrementally,
/// with a WIDER fuzz specifically on the removal side (`REGION_FUZZ`) to
/// avoid rapid resubscribe/unsubscribe thrashing near a boundary — this
/// system REPLACES the whole [`ClientVisibleRegions`] set wholesale on each
/// recompute: `bevy_replicon`'s own `VisibilityFilter` observers already do
/// the incremental old-vs-new membership diffing for us (see
/// [`crate::visibility`]'s doc comment), so there is no need to hand-roll the
/// legacy system's own two-sided hysteresis — the plain "add" formula
/// (`common::region::regions_in_vd`) is sufficient here.
///
/// `pub(crate)`, not `pub`: external callers only need [`ClientInterestPlugin`]
/// (which registers this system), never the system function itself — keeping
/// it crate-private avoids exposing the private [`RegionRecomputeState`]
/// query parameter in a public signature.
pub(crate) fn recompute_client_visible_regions(
    mut commands: Commands,
    clients: Query<(Entity, &ClientViewpoint, Option<&RegionRecomputeState>)>,
) {
    let chunk_size = TerrainChunkSize::RECT_SIZE.reduce_max() as f32;

    for (entity, viewpoint, state) in &clients {
        let chunk = viewpoint.pos.as_::<i32>().wpos_to_cpos();
        let vd = viewpoint.view_distance;

        // Same trigger `server/src/sys/subscription.rs::Sys::run` uses: only
        // recompute when moving to a new chunk (fuzzy-bordered, to avoid
        // rapid triggering along chunk boundaries) or when the view distance
        // itself changed. A brand-new `ClientViewpoint` (no cached state yet)
        // always recomputes — mirrors
        // `initialize_region_subscription`'s unconditional first computation.
        let needs_recompute = match state {
            None => true,
            Some(state) => {
                (chunk != state.fuzzy_chunk
                    && (state
                        .fuzzy_chunk
                        .map2(TerrainChunkSize::RECT_SIZE, |e, sz| {
                            (e as f32 + 0.5) * sz as f32
                        })
                        - viewpoint.pos)
                        .map2(TerrainChunkSize::RECT_SIZE, |e, sz| {
                            e.abs() > (sz / 2 + CHUNK_FUZZ) as f32
                        })
                        .reduce_or())
                    || state.last_view_distance != vd
            },
        };
        if !needs_recompute {
            continue;
        }

        let regions = regions_in_vd(
            vek::Vec3::new(viewpoint.pos.x, viewpoint.pos.y, 0.0),
            (vd as f32 * chunk_size) + (CHUNK_FUZZ as f32 + chunk_size) * 2.0f32.sqrt(),
        );
        commands.entity(entity).insert((
            ClientVisibleRegions::from_regions(viewpoint.dimension, regions),
            RegionRecomputeState {
                fuzzy_chunk: chunk,
                last_view_distance: vd,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, state::app::StatesPlugin};
    use bevy_replicon::prelude::RepliconPlugins;

    use super::*;
    use crate::XindelerProtocolPlugin;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::MinimalPlugins,
            StatesPlugin,
            RepliconPlugins,
            XindelerProtocolPlugin,
        ));
        app
    }

    /// A brand-new `ClientViewpoint` (no cached recompute state yet) always
    /// gets a `ClientVisibleRegions` on the very first recompute pass.
    #[test]
    fn first_sight_always_recomputes() {
        let mut app = new_app();
        let client = app
            .world_mut()
            .spawn(ClientViewpoint::new(
                DimensionId::default(),
                Vec2::new(0.0, 0.0),
                1,
            ))
            .id();

        app.world_mut()
            .run_system_once(recompute_client_visible_regions)
            .expect("system runs");

        let visible = app
            .world()
            .get::<ClientVisibleRegions>(client)
            .expect("a fresh ClientViewpoint gets a ClientVisibleRegions on first recompute");
        assert!(
            !visible.0.is_empty(),
            "the client's own region must be in its visible set"
        );
    }

    /// Two clients with far-apart, non-overlapping viewpoints end up with
    /// disjoint visible-region sets (the core T47.6 acceptance property, at
    /// the unit level).
    #[test]
    fn far_apart_clients_get_disjoint_region_sets() {
        let mut app = new_app();
        // REGION_SIZE = 512 blocks; view_distance is in chunks and a chunk is
        // (much) smaller than a region, so separating the two viewpoints by
        // many region-widths guarantees no overlap for any small `vd`.
        let client_a = app
            .world_mut()
            .spawn(ClientViewpoint::new(
                DimensionId::default(),
                Vec2::new(0.0, 0.0),
                1,
            ))
            .id();
        let client_b = app
            .world_mut()
            .spawn(ClientViewpoint::new(
                DimensionId::default(),
                Vec2::new(20_000.0, 20_000.0),
                1,
            ))
            .id();

        app.world_mut()
            .run_system_once(recompute_client_visible_regions)
            .expect("system runs");

        let regions_a = app.world().get::<ClientVisibleRegions>(client_a).unwrap();
        let regions_b = app.world().get::<ClientVisibleRegions>(client_b).unwrap();
        assert!(
            regions_a.0.is_disjoint(&regions_b.0),
            "far-apart clients must not share any visible region key"
        );
    }

    /// A viewpoint that crosses into a new chunk (well past the `CHUNK_FUZZ`
    /// border) recomputes on the next pass; one that stays within the SAME
    /// chunk does not (matches `RegionSubscription`'s own fuzzy-chunk gate).
    #[test]
    fn recompute_only_fires_on_chunk_boundary_crossing() {
        let mut app = new_app();
        let client = app
            .world_mut()
            .spawn(ClientViewpoint::new(
                DimensionId::default(),
                Vec2::new(0.0, 0.0),
                1,
            ))
            .id();
        app.world_mut()
            .run_system_once(recompute_client_visible_regions)
            .expect("system runs");
        let first = app
            .world()
            .get::<ClientVisibleRegions>(client)
            .unwrap()
            .clone();

        // Small jitter well within the same chunk (chunk edge = 32 blocks by
        // default `TerrainChunkSize`; see `common::terrain`) — must NOT
        // change the visible set.
        app.world_mut()
            .entity_mut(client)
            .insert(ClientViewpoint::new(
                DimensionId::default(),
                Vec2::new(1.0, 1.0),
                1,
            ));
        app.world_mut()
            .run_system_once(recompute_client_visible_regions)
            .expect("system runs");
        let unchanged = app
            .world()
            .get::<ClientVisibleRegions>(client)
            .unwrap()
            .clone();
        assert_eq!(
            first, unchanged,
            "a small in-chunk jitter must not change the visible-region set"
        );

        // A large jump crosses many chunks AND regions — must recompute to a
        // different set.
        app.world_mut()
            .entity_mut(client)
            .insert(ClientViewpoint::new(
                DimensionId::default(),
                Vec2::new(20_000.0, 20_000.0),
                1,
            ));
        app.world_mut()
            .run_system_once(recompute_client_visible_regions)
            .expect("system runs");
        let moved = app.world().get::<ClientVisibleRegions>(client).unwrap();
        assert_ne!(
            &unchanged, moved,
            "crossing many chunks/regions must produce a different visible-region set"
        );
    }
}
