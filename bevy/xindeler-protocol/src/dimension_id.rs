//! [`DimensionId`] — the identifier tagging which dimension/instance a Bevy
//! entity belongs to (migration spec §5.3, BL-82 EM-4.5).
//!
//! Lives in `xindeler-protocol` (not `xindeler-dimensions`, where the rest of
//! EM-4.5's registry/lifecycle machinery lives) for the same reason
//! `AiExecutionMode` does (see `ai_mode.rs`'s doc comment): this is the
//! low-level, few-deps crate every higher-level consumer can depend on
//! without a cycle. Concretely: `xindeler-dimensions` already depends on
//! `xindeler-oracle-host`, which depends on `xindeler-protocol` — so
//! `xindeler-protocol` depending back on `xindeler-dimensions` would be
//! circular. `xindeler-dimensions` re-exports this type (`pub use
//! xindeler_protocol::DimensionId` in its own `component.rs`) so existing
//! callers (`xindeler_dimensions::DimensionId`) are unaffected; this is the
//! single canonical definition, not a duplicate.
//!
//! EM-4.2d's per-client interest management (`visibility.rs`) uses this same
//! type to key `RegionKey`/`ClientVisibleRegions` — originally a
//! deliberately-scoped placeholder (`DimensionId(u32)`, always `0`) built
//! before EM-4.5 landed; folded into this real, registry-backed identifier
//! once both branches merged, exactly as that module's own doc comment
//! anticipated ("this merge is a rename/replace of one small type").

use bevy::ecs::component::Component;

/// Identifies which dimension/instance a Bevy entity belongs to.
/// `DimensionId::DEFAULT` (`DimensionId(0)`) is the always-present default
/// dimension — today's single game world, wrapped (not changed) by
/// `xindeler_dimensions::DimensionRegistry`.
///
/// Deliberately a plain, independently-queryable component (in ADDITION to
/// `xindeler_dimensions::DimensionRoot`): a system that only needs "which
/// dimension is this in" can filter with `Query<&DimensionId>` without
/// walking a relationship, while `DimensionRoot` is reserved for the
/// cascade-despawn / hierarchy-shaped questions.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DimensionId(pub u64);

impl DimensionId {
    /// The always-present default dimension — today's single game world.
    pub const DEFAULT: DimensionId = DimensionId(0);
}
