//! [`DimensionId`] + the [`DimensionRoot`]/[`DimensionMembers`] relationship
//! pair (BL-82 EM-4.5, migration spec §5.3) — the tagging half of "every
//! entity/chunk/light/fog-volume belonging to an instance" carries an
//! identity back to its dimension.
//!
//! This is the FIRST use of Bevy 0.19's `#[relationship]`/
//! `#[relationship_target]` macros anywhere in this codebase (confirmed via
//! grep, 2026-07-10) — the same first-class mechanism `ChildOf`/`Children`
//! use internally, which is exactly why it was picked: cascade-despawn (EM-4.6
//! teardown, next task) falls out of the SAME machinery Bevy already ships,
//! rather than a hand-rolled parent-tracking scheme.

use bevy::{ecs::entity::EntityHashSet, prelude::*};

/// Identifies which dimension/instance a Bevy entity belongs to (migration
/// spec §5.3). `DimensionId::DEFAULT` (`DimensionId(0)`) is the
/// always-present default dimension — today's single game world, wrapped
/// (not changed) by [`crate::registry::DimensionRegistry`].
///
/// Canonically defined in `xindeler_protocol::dimension_id` (EM-4.2d
/// review-follow-up: that crate's own per-client visibility scoping
/// (`RegionKey`) needs this same identifier, and `xindeler-protocol` is the
/// low-level crate every consumer can depend on without a cycle — see that
/// module's doc comment for the full reasoning). Re-exported here so every
/// existing `xindeler_dimensions::DimensionId` caller is unaffected.
pub use xindeler_protocol::DimensionId;

/// Relationship-parent tagging an entity as belonging to a dimension's root
/// entity (Bevy relationships, stable since 0.16). Despawning the root
/// entity cascades through every entity holding a `DimensionRoot` pointing
/// at it — the exact mechanism EM-4.6 (next task) will use for teardown;
/// this task only establishes the relationship, it does not despawn.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = DimensionMembers)]
pub struct DimensionRoot(pub Entity);

/// Auto-maintained by Bevy from every live [`DimensionRoot`] pointing at this
/// entity — the entity holding this component IS a dimension's root.
///
/// `linked_spawn` is the attribute that actually grants cascade-DESPAWN
/// (verified against `bevy_ecs::relationship`'s own doc comment, 2026-07-10 —
/// without it, a `#[relationship_target]` only auto-maintains the collection,
/// it does NOT despawn members when the root despawns): despawning the root
/// entity despawns every entity still holding a `DimensionRoot` pointing at
/// it, exactly the mechanism EM-4.6 (next task) needs for teardown.
///
/// ## EM-4.10 Finding A: `EntityHashSet`, not the default `Vec<Entity>`
/// Every mirrored sim entity (every NPC, wildlife body, the player — the
/// entire live population) is tagged/untagged against the single, ever-
/// growing [`DimensionId::DEFAULT`] root on every sighting/departure
/// (`xindeler-sim-bridge`'s `mirror_sim_entities`). Bevy's default
/// relationship-target backing is `Vec<Entity>`, whose
/// `RelationshipSourceCollection::remove` is O(n) in the current member
/// count (scan + shift) — a single frame that despawns several mirrors pays
/// O(n·k) against the WHOLE live population, growing worse as the
/// population grows (the exact "FPS oscillating wildly, worsening over the
/// session" signature diagnosed 2026-07-10). `EntityHashSet` turns
/// insert/remove into O(1) and is a first-class
/// `RelationshipSourceCollection` impl Bevy ships for exactly this swap.
/// Membership order is irrelevant here (nothing depends on iteration
/// order), so a set is strictly better with no downside.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = DimensionRoot, linked_spawn)]
pub struct DimensionMembers(EntityHashSet);

impl DimensionMembers {
    /// Iterates the entities currently tagged as members of this root.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ { self.0.iter().copied() }

    /// Number of tagged members.
    pub fn len(&self) -> usize { self.0.len() }

    /// Whether this root currently has no tagged members.
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawning a root + a member wires the relationship both ways: the
    /// member's `DimensionRoot` points at the root, and the root's
    /// (auto-maintained) `DimensionMembers` lists the member back.
    #[test]
    fn relationship_wires_both_directions() {
        let mut world = World::new();
        let root = world.spawn(DimensionId(7)).id();
        let member = world.spawn((DimensionId(7), DimensionRoot(root))).id();

        let members = world
            .get::<DimensionMembers>(root)
            .expect("Bevy auto-inserts the relationship target");
        assert_eq!(members.len(), 1);
        assert!(members.iter().eq([member]));
    }

    /// Despawning the root cascades to the member (Bevy relationships'
    /// built-in cascade-despawn) — the exact mechanism EM-4.6 will lean on
    /// for real teardown; proven here at the component-wiring level, not
    /// reproduced by hand.
    #[test]
    fn despawning_root_cascades_to_members() {
        let mut world = World::new();
        let root = world.spawn(DimensionId(1)).id();
        let member = world.spawn((DimensionId(1), DimensionRoot(root))).id();

        world.despawn(root);

        assert!(
            world.get_entity(member).is_err(),
            "member should have been cascade-despawned with its root"
        );
    }

    /// EM-4.10 Finding A: churn micro-test proving `EntityHashSet`-backed
    /// `DimensionMembers` correctly maintains membership under a spawn/
    /// despawn burst — the exact shape (many members tagged, then a chunk of
    /// them removed in one go) that used to pay `Vec`'s O(n) remove cost per
    /// despawned member, against the WHOLE remaining population, every time.
    /// This test only asserts correctness (the O(1)-vs-O(n) complexity claim
    /// isn't itself something a unit test can measure), but a member set
    /// this large would have been the exact shape that made the old `Vec`
    /// backing's cost visible.
    #[test]
    fn churn_despawning_half_the_members_leaves_the_other_half_correctly_tracked() {
        let mut world = World::new();
        let root = world.spawn(DimensionId(3)).id();

        const MEMBER_COUNT: usize = 200;
        let members: Vec<Entity> = (0..MEMBER_COUNT)
            .map(|_| world.spawn((DimensionId(3), DimensionRoot(root))).id())
            .collect();

        assert_eq!(
            world
                .get::<DimensionMembers>(root)
                .expect("root has the auto-maintained collection")
                .len(),
            MEMBER_COUNT
        );

        // Despawn every other member — a representative "several mirrors
        // left view this frame" churn burst.
        let (despawned, kept): (Vec<(usize, Entity)>, Vec<(usize, Entity)>) = members
            .into_iter()
            .enumerate()
            .partition(|(i, _)| i % 2 == 0);
        let despawned: Vec<Entity> = despawned.into_iter().map(|(_, e)| e).collect();
        let kept: Vec<Entity> = kept.into_iter().map(|(_, e)| e).collect();

        for entity in &despawned {
            world.despawn(*entity);
        }

        let remaining = world
            .get::<DimensionMembers>(root)
            .expect("root still has members left");
        assert_eq!(remaining.len(), kept.len());
        assert!(!remaining.is_empty());
        for entity in &kept {
            assert!(
                remaining.iter().any(|e| e == *entity),
                "surviving member {entity:?} must still be tracked"
            );
        }
        for entity in &despawned {
            assert!(
                !remaining.iter().any(|e| e == *entity),
                "despawned member {entity:?} must no longer be tracked"
            );
        }
    }
}
