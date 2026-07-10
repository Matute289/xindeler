//! The full `Spinup → Active → Draining → Teardown` state machine (migration
//! spec §5.3; confirmed maximalist scope per worksheet [Q4]=A, 2026-07-10 —
//! no leaner v1). `Teardown`'s actual despawn/GC PAYLOAD is EM-4.6 (T47.8,
//! the next task); this crate only owns getting a dimension correctly INTO
//! `Teardown` — entry/exit conditions, and its interaction with the
//! visibility/mirror systems (see `xindeler-sim-bridge`'s
//! `mirror_admits_new_entity` gate) — see `crate`'s top doc for the full
//! scope-boundary statement.

/// The four lifecycle states every [`crate::registry::DimensionId`] instance
/// moves through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DimensionLifecycle {
    /// World generation is running on the async task pool (see
    /// `crate::spinup`); the dimension has no queryable content yet. No
    /// entrants accepted.
    Spinup,
    /// Fully generated and open. The ONLY state that accepts new entrants
    /// (spec §1.8's acceptance bar: "no new player can join once Draining" —
    /// also true, for the same reason, of `Spinup` and `Teardown`).
    Active,
    /// No new entrants; existing occupants may finish/leave normally. Once
    /// the last occupant leaves, [`crate::registry::DimensionRegistry`]
    /// auto-transitions the dimension to [`DimensionLifecycle::Teardown`]
    /// (see `DimensionRegistry::remove_occupant`) — the real, tested exit
    /// condition this task implements. A PREDICTIVE early entry into this
    /// state (before the literal last player leaves) is EM-4.6's job
    /// (`PredictiveGc`, T47.8) — this task only wires the explicit
    /// admin-command entry (`DrainDimension`).
    Draining,
    /// Terminal state for this task. EM-4.6 hangs the actual GC/despawn
    /// payload off entry into this state; today it is otherwise inert.
    Teardown,
}

impl DimensionLifecycle {
    /// Whether a dimension in this state may accept a new occupant/entrant.
    /// Shared by both [`crate::registry::DimensionRegistry::try_add_occupant`]
    /// (the sim-side occupant bookkeeping this task owns) and
    /// `xindeler-sim-bridge`'s mirror-entity-creation gate (the "interaction
    /// with the mirror system" spec §1.8 asks for) — one predicate, reused,
    /// not two copies that could drift.
    pub fn accepts_new_entrants(self) -> bool { matches!(self, Self::Active) }
}

#[cfg(test)]
mod tests {
    use super::DimensionLifecycle::{Active, Draining, Spinup, Teardown};

    #[test]
    fn only_active_accepts_new_entrants() {
        assert!(!Spinup.accepts_new_entrants());
        assert!(Active.accepts_new_entrants());
        assert!(!Draining.accepts_new_entrants());
        assert!(!Teardown.accepts_new_entrants());
    }
}
