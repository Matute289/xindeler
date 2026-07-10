//! Per-client interest management (BL-82 EM-4.2d, spec §1.3 / task board
//! T47.6): a `bevy_replicon` visibility filter keyed by the SAME region grid
//! the legacy TCP/QUIC stack already computes
//! (`common::region::{RegionMap, regions_in_vd}`,
//! `server/src/sys/subscription.rs`), so the legacy stack (via
//! `RegionSubscription`/`entity_sync.rs`) and the replicon stack converge on
//! the same definition of "in view" for a given position + view distance.
//!
//! Before this task, `XindelerProtocolPlugin`'s own doc comment carried a
//! standing obligation (EM-3.6/EM-3.7's reviewer, quoted verbatim): "replicon's
//! default visibility sends every `Replicated` entity to every client... MUST
//! be wired (EM-4.2d) before shipping default-all visibility past the
//! listen-server milestone." This module is that wiring.
//!
//! ## The pieces
//! - [`RegionKey`]: the `(dimension, region)` a REPLICATED ENTITY currently
//!   occupies. Written by `xindeler-sim-bridge`'s mirror
//!   (`mirror_sim_entities`) from the sim's own `comp::Pos`, via
//!   [`region_key_for_pos`] — the exact region-grid math
//!   `common::region::RegionMap`'s (private) `pos_key` uses. `RegionKey` is
//!   NEVER passed to `XindelerProtocolPlugin`'s `.replicate::<T>()` list, so it
//!   never reaches the wire — it stays server-only bookkeeping that only drives
//!   the filter below, exactly like [`ClientVisibleRegions`].
//! - [`ClientVisibleRegions`]: the SET of region keys a connected client can
//!   currently see. Written by `xindeler-server-app`'s
//!   `recompute_client_visible_regions` system from a `ClientViewpoint` (that
//!   crate owns the actual viewpoint component — a server-shell concern, not a
//!   shared wire type), recomputed on the same chunk-boundary-crossing trigger
//!   `server/src/sys/subscription.rs::Sys::run` uses.
//! - The [`bevy_replicon::prelude::VisibilityFilter`] impl on [`RegionKey`]: an
//!   entity is visible to a client iff the client's [`ClientVisibleRegions`]
//!   contains the entity's [`RegionKey`] — the same "team membership" shape
//!   `VisibilityFilter`'s own doc example (`Team(u8)`) demonstrates, just with
//!   set-membership instead of scalar equality.
//! - [`XindelerProtocolPlugin`] registers the filter (`add_visibility_filter`)
//!   symmetrically alongside the rest of the replication contract — see that
//!   plugin's doc comment. Filter registration does NOT feed `bevy_replicon`'s
//!   `ProtocolHash` (unlike `.replicate`/`.add_*_message` calls — verified
//!   against `bevy_replicon` 0.41.1's source: only
//!   `shared::replication::rules`/`shared::message::*` touch `ProtocolHasher`,
//!   `server::visibility` never does), so registering it on the client too is a
//!   safety/consistency choice, not a hash-parity requirement — it stays
//!   symmetric so both sides' `FilterRegistry` bit assignments always match,
//!   and dormant client-side exactly like every other "both roles compile into
//!   every shell" registration this codebase already documents (see
//!   `xindeler-transport`'s crate doc comment).
//!
//! ## `DimensionId` — a deliberate placeholder (EM-4.5 extension point)
//! [`DimensionId`] is a trivial `u32` wrapper, always `0` today — no dimension
//! concept exists anywhere in this codebase yet (confirmed by
//! `specs/2026-07-10-bl82-phase4-remaining-plan.md` §0.3: "zero. No
//! `Realm`/`Dimension`/`Instance` type exists anywhere"). EM-4.5
//! (`DimensionRegistry` + the real `DimensionId`, a SIBLING task/branch not
//! yet merged into this trunk) will introduce the authoritative,
//! registry-backed identifier. This placeholder is deliberately scoped to
//! THIS module only (not re-exported as some workspace-wide "the"
//! `DimensionId`) so that merge is a rename/replace of one small type, not a
//! redesign of [`RegionKey`]/[`ClientVisibleRegions`] — folding a real
//! `(DimensionId, region_key)` tuple into per-client visibility is exactly
//! the extension point EM-4.2d's task brief asks this task to leave.

use std::collections::HashSet;

use bevy::ecs::{component::Component, entity::Entity};
use bevy_replicon::prelude::VisibilityFilter;
use vek::Vec2;

/// Placeholder dimension identifier — see the module doc comment's "EM-4.5
/// extension point" section. Always `0` (the only dimension that exists
/// today) until `EM-4.5`'s `DimensionRegistry` lands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DimensionId(pub u32);

/// The `(dimension, region)` an entity currently occupies, using the SAME
/// 512-block region grid `common::region::RegionMap` does (`REGION_SIZE`).
///
/// `Copy` + immutable: a moving entity gets a freshly-computed `RegionKey`
/// [`Commands::insert`](bevy::prelude::Commands::insert)ed (replacing the old
/// value) rather than mutated in place — replicon's `VisibilityFilter`
/// observers fire on that insert/replace and re-evaluate every connected
/// client's visibility of this one entity (`O(clients)`, not `O(entities ×
/// clients)` per tick).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[component(immutable)]
pub struct RegionKey {
    pub dimension: DimensionId,
    pub region: Vec2<i32>,
}

impl VisibilityFilter for RegionKey {
    type ClientComponent = ClientVisibleRegions;
    type Scope = Entity;

    fn is_visible(&self, _client: Entity, component: Option<&Self::ClientComponent>) -> bool {
        component.is_some_and(|visible| visible.0.contains(self))
    }
}

/// The set of [`RegionKey`]s a connected client can currently see.
///
/// Immutable (replicon's `VisibilityFilter::ClientComponent` bound): a
/// client's visible-region set is REPLACED wholesale
/// ([`Commands::insert`](bevy::prelude::Commands::insert)) on each recompute,
/// never mutated in place — see
/// `xindeler-server-app::visibility::recompute_client_visible_regions`.
///
/// A client with NO `ClientVisibleRegions` component (e.g. freshly connected,
/// before anything sets its `ClientViewpoint`) sees NOTHING: replicon treats a
/// missing `ClientComponent` as `is_visible == false` for every
/// `RegionKey`-tagged entity — see [`VisibilityFilter::ClientComponent`]'s own
/// doc comment ("If the component is missing on a replicated entity, it is
/// treated as if `is_visible` would return `false`" — the symmetric case,
/// missing on the CLIENT side, is [`RegionKey::is_visible`]'s own `None` arm
/// above). "Blind until scoped" is the deliberately safe default, mirroring
/// how a legacy client sees nothing before `RegionSubscription` is
/// initialized
/// (`server/src/sys/subscription.rs::initialize_region_subscription`).
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
#[component(immutable)]
pub struct ClientVisibleRegions(pub HashSet<RegionKey>);

impl ClientVisibleRegions {
    /// Builds a visible-region set from an iterator of region keys (e.g.
    /// `common::region::regions_in_vd`'s output, mapped into [`RegionKey`]s
    /// for a fixed [`DimensionId`]).
    #[must_use]
    pub fn from_regions(
        dimension: DimensionId,
        regions: impl IntoIterator<Item = Vec2<i32>>,
    ) -> Self {
        Self(
            regions
                .into_iter()
                .map(|region| RegionKey { dimension, region })
                .collect(),
        )
    }
}

/// Computes the [`RegionKey`] a world-space XY position (sim axes: x-east,
/// y-north — the SAME convention `common::region::regions_in_vd` takes)
/// falls into, mirroring `common::region::RegionMap`'s own (private)
/// `pos_key` exactly:
/// `server/src/sys/subscription.rs`'s own callers compute it as
/// `pos.0.map(|e| e as i32)` (float→i32 **truncation**, not floor) fed into
/// `pos_key`'s `pos.map(|e| e >> REGION_LOG2)` (an **arithmetic** right shift
/// — floor division by the power-of-two `REGION_SIZE`). This function
/// reproduces both steps in that exact order so it agrees with the legacy
/// computation bit-for-bit, including for negative coordinates.
///
/// `common::region::RegionMap::pos_key` itself is a private associated
/// function (not part of `common`'s public API), and this codebase's
/// isolation law reserves logic-crate edits for upstream merges (CLAUDE.md,
/// "Documentation & Git Policy" — logic crates like `common` are edited only
/// by upstream syncs + the rename tool). Reimplementing this one arithmetic
/// identity here — rather than widening that crate's visibility — keeps this
/// task's diff entirely inside the Bevy migration surface. `REGION_SIZE` is
/// `pub` and, per its own construction (`1 << REGION_LOG2`), guaranteed a
/// power of two, so `div_euclid` (used here, since `i32`'s `>>` isn't
/// directly expressible as a portable safe arithmetic-shift call on an
/// already-truncated value in the same one-liner) and the private
/// implementation's bit-shift agree for every input.
#[must_use]
pub fn region_key_for_pos(dimension: DimensionId, pos_xy: Vec2<f32>) -> RegionKey {
    let truncated = pos_xy.map(|e| e as i32);
    RegionKey {
        dimension,
        region: truncated.map(|e| e.div_euclid(common::region::REGION_SIZE as i32)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(x: i32, y: i32) -> Vec2<i32> { Vec2::new(x, y) }

    #[test]
    fn region_key_for_pos_matches_expected_grid_cells() {
        let d = DimensionId::default();
        // REGION_SIZE = 512: the whole first region spans [0, 511] on each
        // axis.
        assert_eq!(region_key_for_pos(d, Vec2::new(0.0, 0.0)).region, key(0, 0));
        assert_eq!(
            region_key_for_pos(d, Vec2::new(511.9, 0.0)).region,
            key(0, 0)
        );
        assert_eq!(
            region_key_for_pos(d, Vec2::new(512.0, 0.0)).region,
            key(1, 0)
        );
        // Negative coordinates floor toward negative infinity (matching an
        // arithmetic right shift), not toward zero.
        assert_eq!(
            region_key_for_pos(d, Vec2::new(-1.0, -1.0)).region,
            key(-1, -1)
        );
        assert_eq!(
            region_key_for_pos(d, Vec2::new(-512.0, -513.0)).region,
            key(-1, -2)
        );
    }

    #[test]
    fn region_key_carries_the_given_dimension() {
        let dim = DimensionId(7);
        assert_eq!(region_key_for_pos(dim, Vec2::new(0.0, 0.0)).dimension, dim);
    }

    #[test]
    fn client_with_no_regions_sees_nothing() {
        let entity_region = RegionKey {
            dimension: DimensionId::default(),
            region: key(0, 0),
        };
        assert!(!entity_region.is_visible(Entity::PLACEHOLDER, None));
    }

    #[test]
    fn client_sees_only_regions_it_has() {
        let a = RegionKey {
            dimension: DimensionId::default(),
            region: key(0, 0),
        };
        let b = RegionKey {
            dimension: DimensionId::default(),
            region: key(5, 5),
        };
        let visible = ClientVisibleRegions::from_regions(DimensionId::default(), [key(0, 0)]);
        assert!(a.is_visible(Entity::PLACEHOLDER, Some(&visible)));
        assert!(!b.is_visible(Entity::PLACEHOLDER, Some(&visible)));
    }

    #[test]
    fn different_dimensions_with_the_same_region_do_not_match() {
        // The extension point this task leaves for EM-4.5: two regions with
        // the same grid cell but different dimensions are DISTINCT keys.
        let entity_region = RegionKey {
            dimension: DimensionId(1),
            region: key(0, 0),
        };
        let visible = ClientVisibleRegions::from_regions(DimensionId(0), [key(0, 0)]);
        assert!(!entity_region.is_visible(Entity::PLACEHOLDER, Some(&visible)));
    }
}
