//! [`DimensionRegistry`] — the resource wrapping every live dimension
//! (migration spec §5.3, grounded against the real single-world insertion
//! points per `2026-07-10-bl82-phase4-remaining-plan.md` §0.3).
//!
//! Today's single `Arc<World>` + `IndexOwned` (`server/src/lib.rs:542-544`)
//! becomes [`DimensionId::DEFAULT`]'s [`DimensionState`] here — a WRAPPING
//! refactor of already-existing state, not a behavior change: nothing about
//! how the default dimension generates/ticks/persists changes, this resource
//! is purely an additive index over state the sim already owns and already
//! exposes via its own public API (`Server::world()`,
//! `State::ecs().read_resource::<IndexOwned>()`, `State::thread_pool()`).

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use bevy::prelude::*;
use common::terrain::TerrainChunk;
use server::{IndexOwned, World};
use vek::Vec2;

use crate::lifecycle::DimensionLifecycle;

/// Identifies which dimension/instance an entity/chunk belongs to. Re-export
/// of [`crate::component::DimensionId`] under this module too, since the
/// registry is keyed by it — see that module for the full doc.
pub use crate::component::DimensionId;

/// Per-dimension state: lifecycle + the generated world data + a lazily
/// populated chunk cache + occupant bookkeeping. Not `Clone` — the
/// [`DimensionRegistry`] is the one owner.
pub struct DimensionState {
    lifecycle: DimensionLifecycle,
    /// This dimension's root entity (see [`crate::component::DimensionRoot`]).
    root: Entity,
    /// The generated procgen world driving this dimension's terrain (shared,
    /// read-only after generation — `Arc` matches the sim's own storage of
    /// the SAME type at `server/src/lib.rs:541-544`).
    world: Arc<World>,
    /// Biome/site/civ index for `world` (see `world::index`).
    index: IndexOwned,
    /// Seed modifier this dimension was spun up with (§1.8's "seed_modifier"
    /// — reused verbatim from `xindeler_oracle_host::dm_event::
    /// DimensionConfig`, see `crate::spinup`).
    seed_modifier: u32,
    /// On-demand-generated chunks, keyed exactly like the sim's own
    /// `TerrainGrid` (`Vec2<i32>`) but scoped to JUST this dimension — the
    /// concrete mechanism proving terrain never cross-contaminates between
    /// dimensions (spec §1.8's acceptance bar): two dimensions' chunk stores
    /// are two entirely separate `HashMap`s, generated from two entirely
    /// separate `World`/`IndexOwned` pairs.
    chunk_store: HashMap<Vec2<i32>, Arc<TerrainChunk>>,
    /// Bevy entities currently "in" this dimension that count toward the
    /// Draining→Teardown exit condition (e.g. connected players) — NOT every
    /// `DimensionRoot`-tagged entity counts (a cached terrain-chunk entity,
    /// if one ever exists, never blocks teardown on its own).
    occupants: HashSet<Entity>,
}

impl DimensionState {
    /// Current lifecycle state.
    pub fn lifecycle(&self) -> DimensionLifecycle { self.lifecycle }

    /// This dimension's root entity.
    pub fn root(&self) -> Entity { self.root }

    /// Number of tracked occupants.
    pub fn occupant_count(&self) -> usize { self.occupants.len() }

    /// Whether this dimension currently accepts new occupants/entrants.
    pub fn accepts_new_entrants(&self) -> bool { self.lifecycle.accepts_new_entrants() }

    /// The seed modifier this dimension was spun up with.
    pub fn seed_modifier(&self) -> u32 { self.seed_modifier }

    /// The generated world backing this dimension (read-only).
    pub fn world(&self) -> &World { &self.world }

    /// The biome/site/civ index backing this dimension (read-only).
    pub fn index(&self) -> &IndexOwned { &self.index }

    /// Number of chunks generated so far for this dimension.
    pub fn generated_chunk_count(&self) -> usize { self.chunk_store.len() }
}

/// Errors from [`DimensionRegistry`]'s mutators — surfaced (not silently
/// no-op'd) so a caller or test can assert the exact rejection reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionError {
    /// No dimension is registered under this id.
    NotFound(DimensionId),
    /// A dimension already exists under this id.
    AlreadyExists(DimensionId),
    /// The dimension exists but is not `Active`, so it rejects a new
    /// entrant.
    NotAcceptingEntrants(DimensionId, DimensionLifecycle),
    /// The dimension is in the wrong lifecycle state for the requested
    /// transition (`.1` = actual, `.2` = required).
    WrongLifecycle(DimensionId, DimensionLifecycle, DimensionLifecycle),
    /// `World::generate_chunk` itself failed for this dimension/position.
    GenerationFailed(DimensionId, Vec2<i32>),
}

impl fmt::Display for DimensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "dimension {id:?} does not exist"),
            Self::AlreadyExists(id) => write!(f, "dimension {id:?} already exists"),
            Self::NotAcceptingEntrants(id, lifecycle) => {
                write!(
                    f,
                    "dimension {id:?} is {lifecycle:?} and does not accept new entrants"
                )
            },
            Self::WrongLifecycle(id, actual, expected) => write!(
                f,
                "dimension {id:?} is {actual:?}, expected {expected:?} for this transition"
            ),
            Self::GenerationFailed(id, pos) => {
                write!(f, "chunk generation failed for dimension {id:?} at {pos:?}")
            },
        }
    }
}

impl std::error::Error for DimensionError {}

/// The resource wrapping every live dimension. See the module doc for the
/// "wrapping refactor, not a behavior change" framing of
/// [`DimensionId::DEFAULT`].
#[derive(Resource, Default)]
pub struct DimensionRegistry {
    dimensions: HashMap<DimensionId, DimensionState>,
}

impl DimensionRegistry {
    /// Read-only lookup.
    pub fn get(&self, id: DimensionId) -> Option<&DimensionState> { self.dimensions.get(&id) }

    /// Whether `id` is currently registered (in ANY lifecycle state).
    pub fn contains(&self, id: DimensionId) -> bool { self.dimensions.contains_key(&id) }

    /// Every currently-registered dimension id, in unspecified order.
    pub fn ids(&self) -> impl Iterator<Item = DimensionId> + '_ { self.dimensions.keys().copied() }

    /// Convenience: the lifecycle of `id`, if it exists.
    pub fn lifecycle(&self, id: DimensionId) -> Option<DimensionLifecycle> {
        self.dimensions.get(&id).map(DimensionState::lifecycle)
    }

    /// Registers a brand-new dimension already in
    /// [`DimensionLifecycle::Spinup`], backed by a placeholder EMPTY world
    /// (`World::empty()` — the same "no worldgen yet" placeholder the `world`
    /// crate itself provides). The caller is expected to call
    /// [`Self::complete_spinup`] once the REAL generation (sync, for
    /// [`DimensionId::DEFAULT`]'s wrap at boot, or async via
    /// [`crate::spinup`] for a brand-new dimension) is ready. Fails if `id`
    /// already exists.
    pub fn insert_spinning_up(
        &mut self,
        id: DimensionId,
        root: Entity,
        seed_modifier: u32,
    ) -> Result<(), DimensionError> {
        if self.dimensions.contains_key(&id) {
            return Err(DimensionError::AlreadyExists(id));
        }
        let (empty_world, empty_index) = World::empty();
        self.dimensions.insert(id, DimensionState {
            lifecycle: DimensionLifecycle::Spinup,
            root,
            world: Arc::new(empty_world),
            index: empty_index,
            seed_modifier,
            chunk_store: HashMap::new(),
            occupants: HashSet::new(),
        });
        Ok(())
    }

    /// Completes a [`DimensionLifecycle::Spinup`] dimension: installs the
    /// REAL generated `world`/`index` and transitions to
    /// [`DimensionLifecycle::Active`]. Fails if `id` doesn't exist or isn't
    /// currently `Spinup` (a dimension can only spin up once).
    pub fn complete_spinup(
        &mut self,
        id: DimensionId,
        world: Arc<World>,
        index: IndexOwned,
    ) -> Result<(), DimensionError> {
        let state = self
            .dimensions
            .get_mut(&id)
            .ok_or(DimensionError::NotFound(id))?;
        if state.lifecycle != DimensionLifecycle::Spinup {
            return Err(DimensionError::WrongLifecycle(
                id,
                state.lifecycle,
                DimensionLifecycle::Spinup,
            ));
        }
        state.world = world;
        state.index = index;
        state.lifecycle = DimensionLifecycle::Active;
        Ok(())
    }

    /// Admin-command entry point: `Active` → `Draining` (spec §1.8: "no new
    /// entrants, existing players may finish/leave normally"). Fails unless
    /// the dimension is currently `Active` — draining is a one-way door, and
    /// a `Spinup`/already-`Draining`/`Teardown` dimension can't (re-)enter it
    /// this way.
    ///
    /// If the dimension ALREADY has zero occupants at the moment draining
    /// begins (e.g. it never had any, or they all left already), it is
    /// already "fully drained" by definition — this immediately advances it
    /// straight through to `Teardown` in the same call, rather than sitting
    /// in `Draining` forever waiting for a `remove_occupant` call that will
    /// never come (a dimension whose only occupants NEVER existed can't be
    /// drained by someone leaving it). Returns whether that happened, so a
    /// caller/test can observe it precisely — mirrors
    /// [`Self::remove_occupant`]'s own return value for the "did this call
    /// just tear the dimension down" question.
    pub fn begin_draining(&mut self, id: DimensionId) -> Result<bool, DimensionError> {
        let state = self
            .dimensions
            .get_mut(&id)
            .ok_or(DimensionError::NotFound(id))?;
        if state.lifecycle != DimensionLifecycle::Active {
            return Err(DimensionError::WrongLifecycle(
                id,
                state.lifecycle,
                DimensionLifecycle::Active,
            ));
        }
        state.lifecycle = DimensionLifecycle::Draining;
        if state.occupants.is_empty() {
            state.lifecycle = DimensionLifecycle::Teardown;
            return Ok(true);
        }
        Ok(false)
    }

    /// Registers `entity` as an occupant of `id` — fails (rather than
    /// silently admitting) if the dimension isn't `Active` (spec §1.8's
    /// acceptance bar: "no new player can join once Draining", and equally
    /// true of `Spinup`/`Teardown`).
    pub fn try_add_occupant(
        &mut self,
        id: DimensionId,
        entity: Entity,
    ) -> Result<(), DimensionError> {
        let state = self
            .dimensions
            .get_mut(&id)
            .ok_or(DimensionError::NotFound(id))?;
        if !state.accepts_new_entrants() {
            return Err(DimensionError::NotAcceptingEntrants(id, state.lifecycle));
        }
        state.occupants.insert(entity);
        Ok(())
    }

    /// Removes `entity` as an occupant of `id`. If this empties a `Draining`
    /// dimension, auto-advances it to `Teardown` — the natural, tested exit
    /// condition spec §1.8 describes ("existing players may finish/leave
    /// normally"), not an admin command. Returns whether that
    /// Draining→Teardown transition just happened, so a caller/test can
    /// observe it precisely. A no-op (`Ok(false)`) if `entity` wasn't
    /// tracked, or if `id` is in any state other than `Draining` — an
    /// occupant leaving an `Active` dimension never auto-tears it down (only
    /// the explicit [`Self::begin_draining`] admin command starts that
    /// clock).
    pub fn remove_occupant(
        &mut self,
        id: DimensionId,
        entity: Entity,
    ) -> Result<bool, DimensionError> {
        let state = self
            .dimensions
            .get_mut(&id)
            .ok_or(DimensionError::NotFound(id))?;
        state.occupants.remove(&entity);
        if state.lifecycle == DimensionLifecycle::Draining && state.occupants.is_empty() {
            state.lifecycle = DimensionLifecycle::Teardown;
            return Ok(true);
        }
        Ok(false)
    }

    /// EM-4.6: removes a [`DimensionLifecycle::Teardown`] dimension's entry
    /// from the registry entirely and returns its owned [`DimensionState`] —
    /// the actual GC "drop the per-dimension chunk store whole" step (spec
    /// §1.9): the caller (`crate::teardown::teardown_completed_dimensions`)
    /// gets the state back just long enough to read `root()` (for the
    /// `DimensionRoot` cascade-despawn) and hand it to the BL-16 chronicle
    /// hook, then drops it — which frees `chunk_store`/`world`/`index`
    /// (an `Arc`, dropped here unless something else outside the registry
    /// still holds a clone, which nothing in this codebase does) all at
    /// once, ordinary `Drop`, no manual cleanup needed.
    ///
    /// Fails (leaving the registry untouched) if `id` doesn't exist or isn't
    /// currently `Teardown` — this is a one-way, one-shot removal, not a
    /// generic "delete any dimension" escape hatch, so a caller can't
    /// accidentally rip a still-`Active`/`Draining` dimension out from under
    /// its occupants.
    pub fn remove_torn_down(&mut self, id: DimensionId) -> Result<DimensionState, DimensionError> {
        let lifecycle = self
            .dimensions
            .get(&id)
            .ok_or(DimensionError::NotFound(id))?
            .lifecycle;
        if lifecycle != DimensionLifecycle::Teardown {
            return Err(DimensionError::WrongLifecycle(
                id,
                lifecycle,
                DimensionLifecycle::Teardown,
            ));
        }
        Ok(self
            .dimensions
            .remove(&id)
            .expect("just confirmed present above"))
    }

    /// Generates (or returns the already-cached) chunk at `pos` for
    /// dimension `id`, via the SAME `World::generate_chunk` call the sim's
    /// own `ChunkGenerator`/`World::find_accessible_pos` use — just scoped to
    /// THIS dimension's own `world`/`index`/`chunk_store`, so two
    /// dimensions' chunks at the identical `Vec2` key are never confused
    /// (spec §1.8's terrain-isolation acceptance bar).
    pub fn generate_chunk(
        &mut self,
        id: DimensionId,
        pos: Vec2<i32>,
    ) -> Result<Arc<TerrainChunk>, DimensionError> {
        let state = self
            .dimensions
            .get_mut(&id)
            .ok_or(DimensionError::NotFound(id))?;
        if let Some(chunk) = state.chunk_store.get(&pos) {
            return Ok(Arc::clone(chunk));
        }
        let index_ref = state.index.as_index_ref();
        let (chunk, _supplement) = state
            .world
            .generate_chunk(index_ref, pos, None, || true, None)
            .map_err(|()| DimensionError::GenerationFailed(id, pos))?;
        let chunk = Arc::new(chunk);
        state.chunk_store.insert(pos, Arc::clone(&chunk));
        Ok(chunk)
    }
}

/// A violation found by [`sweep_isolation`] (spec §5.3's `debug_assert`
/// sweep, made a `Result` so tests assert on the exact failure rather than
/// relying on panic-catching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationViolation {
    /// An entity is tagged with a `DimensionId` that no longer/never existed
    /// in the registry.
    UnknownDimension {
        entity: Entity,
        dimension: DimensionId,
    },
    /// An entity's `DimensionId` doesn't match the dimension its
    /// `DimensionRoot` relationship actually points at.
    WrongRoot {
        entity: Entity,
        dimension: DimensionId,
        expected_root: Entity,
        actual_root: Entity,
    },
    /// Two different dimensions share the same root entity — a
    /// bookkeeping bug that would make cascade-despawn take out BOTH
    /// dimensions' content at once.
    SharedRoot {
        root: Entity,
        a: DimensionId,
        b: DimensionId,
    },
}

/// Sweeps the registry + a snapshot of live `(Entity, DimensionId,
/// DimensionRoot)` tuples for the isolation invariant every live dimension
/// must hold, in ANY lifecycle state (spec §1.8's acceptance bar): every
/// entity tagged `DimensionId(id)` must ALSO carry `DimensionRoot(root)`
/// where `root` is EXACTLY `registry.get(id).root()` — proving no entity is
/// tagged with one dimension's id but parented to a different (or no)
/// dimension's root — plus no two dimensions share a root entity.
///
/// Takes a plain iterator (rather than a live `Query`) so it's usable both
/// from a Bevy system (see `crate::plugin::debug_assert_dimension_isolation`)
/// and directly from a unit test with synthetic data.
pub fn sweep_isolation(
    registry: &DimensionRegistry,
    tagged_entities: impl Iterator<Item = (Entity, DimensionId, Entity)>,
) -> Result<(), IsolationViolation> {
    for (entity, dimension, root_ptr) in tagged_entities {
        let Some(state) = registry.get(dimension) else {
            return Err(IsolationViolation::UnknownDimension { entity, dimension });
        };
        if state.root() != root_ptr {
            return Err(IsolationViolation::WrongRoot {
                entity,
                dimension,
                expected_root: state.root(),
                actual_root: root_ptr,
            });
        }
    }

    let mut seen_roots: HashMap<Entity, DimensionId> = HashMap::new();
    for id in registry.ids() {
        let root = registry.get(id).expect("just enumerated from ids()").root();
        if let Some(&other) = seen_roots.get(&root) {
            if other != id {
                return Err(IsolationViolation::SharedRoot {
                    root,
                    a: other,
                    b: id,
                });
            }
        } else {
            seen_roots.insert(root, id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two dummy, non-registry-backed entities/roots to drive the pure
    /// sweep function without spinning up a real Bevy `World`.
    fn dummy_entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("small test index is always a valid raw entity id")
    }

    #[test]
    fn sweep_accepts_correctly_tagged_entities() {
        let mut registry = DimensionRegistry::default();
        let root_a = dummy_entity(1);
        let root_b = dummy_entity(2);
        registry
            .insert_spinning_up(DimensionId(0), root_a, 0)
            .expect("fresh registry");
        registry
            .insert_spinning_up(DimensionId(1), root_b, 7)
            .expect("fresh registry");

        let member_a = dummy_entity(10);
        let member_b = dummy_entity(11);
        let tagged = vec![
            (member_a, DimensionId(0), root_a),
            (member_b, DimensionId(1), root_b),
        ];
        assert_eq!(sweep_isolation(&registry, tagged.into_iter()), Ok(()));
    }

    #[test]
    fn sweep_catches_an_entity_tagged_with_the_wrong_root() {
        let mut registry = DimensionRegistry::default();
        let root_a = dummy_entity(1);
        let root_b = dummy_entity(2);
        registry
            .insert_spinning_up(DimensionId(0), root_a, 0)
            .unwrap();
        registry
            .insert_spinning_up(DimensionId(1), root_b, 7)
            .unwrap();

        // Cross-contamination: tagged DimensionId(0) but parented to
        // dimension 1's root.
        let corrupted = dummy_entity(99);
        let tagged = vec![(corrupted, DimensionId(0), root_b)];
        assert_eq!(
            sweep_isolation(&registry, tagged.into_iter()),
            Err(IsolationViolation::WrongRoot {
                entity: corrupted,
                dimension: DimensionId(0),
                expected_root: root_a,
                actual_root: root_b,
            })
        );
    }

    #[test]
    fn sweep_catches_an_entity_tagged_with_an_unknown_dimension() {
        let registry = DimensionRegistry::default();
        let entity = dummy_entity(5);
        let tagged = vec![(entity, DimensionId(42), dummy_entity(1))];
        assert_eq!(
            sweep_isolation(&registry, tagged.into_iter()),
            Err(IsolationViolation::UnknownDimension {
                entity,
                dimension: DimensionId(42)
            })
        );
    }

    #[test]
    fn sweep_catches_two_dimensions_sharing_a_root() {
        let mut registry = DimensionRegistry::default();
        let shared_root = dummy_entity(1);
        registry
            .insert_spinning_up(DimensionId(0), shared_root, 0)
            .unwrap();
        registry
            .insert_spinning_up(DimensionId(1), shared_root, 1)
            .unwrap();

        let result = sweep_isolation(&registry, std::iter::empty());
        assert!(matches!(result, Err(IsolationViolation::SharedRoot { .. })));
    }

    #[test]
    fn default_dimension_wraps_via_insert_then_complete_spinup() {
        // Mirrors the EXACT two-call sequence `xindeler-server-app`'s
        // boot-time wrap uses: dimension 0 starts life the same way any
        // dimension does (`insert_spinning_up`, a placeholder empty
        // world), then is immediately completed with the REAL already-
        // generated world/index — no special-cased code path, same
        // transition function every dimension goes through.
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        registry
            .insert_spinning_up(DimensionId::DEFAULT, root, 0)
            .expect("first registration of DEFAULT should succeed");
        assert_eq!(
            registry.lifecycle(DimensionId::DEFAULT),
            Some(DimensionLifecycle::Spinup)
        );

        let (world, index) = World::empty(); // stands in for the sim's real Arc<World>/IndexOwned
        registry
            .complete_spinup(DimensionId::DEFAULT, Arc::new(world), index)
            .expect("completing a Spinup dimension should succeed");
        assert_eq!(
            registry.lifecycle(DimensionId::DEFAULT),
            Some(DimensionLifecycle::Active)
        );
    }

    #[test]
    fn cannot_register_the_same_dimension_twice() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        registry
            .insert_spinning_up(DimensionId(3), root, 0)
            .unwrap();
        assert_eq!(
            registry.insert_spinning_up(DimensionId(3), root, 0),
            Err(DimensionError::AlreadyExists(DimensionId(3)))
        );
    }

    #[test]
    fn cannot_complete_spinup_twice() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        registry
            .insert_spinning_up(DimensionId(3), root, 0)
            .unwrap();
        let (w1, i1) = World::empty();
        registry
            .complete_spinup(DimensionId(3), Arc::new(w1), i1)
            .unwrap();

        let (w2, i2) = World::empty();
        assert_eq!(
            registry.complete_spinup(DimensionId(3), Arc::new(w2), i2),
            Err(DimensionError::WrongLifecycle(
                DimensionId(3),
                DimensionLifecycle::Active,
                DimensionLifecycle::Spinup
            ))
        );
    }

    /// Full 4-state walk (spec §1.8's own acceptance bar: "a test drives a
    /// dimension through all four states in order and asserts the
    /// entry/exit invariants at each transition").
    #[test]
    fn full_lifecycle_walk_with_entry_exit_invariants() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        let id = DimensionId(9);

        // --- Spinup: no entrants accepted yet. ---
        registry.insert_spinning_up(id, root, 3).unwrap();
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Spinup));
        let hopeful = dummy_entity(100);
        assert_eq!(
            registry.try_add_occupant(id, hopeful),
            Err(DimensionError::NotAcceptingEntrants(
                id,
                DimensionLifecycle::Spinup
            ))
        );

        // --- Active: entrants accepted. ---
        let (world, index) = World::empty();
        registry
            .complete_spinup(id, Arc::new(world), index)
            .unwrap();
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Active));
        let occupant_a = dummy_entity(101);
        let occupant_b = dummy_entity(102);
        registry
            .try_add_occupant(id, occupant_a)
            .expect("Active accepts entrants");
        registry
            .try_add_occupant(id, occupant_b)
            .expect("Active accepts entrants");
        assert_eq!(registry.get(id).unwrap().occupant_count(), 2);

        // --- Draining: admin-triggered; no NEW entrants; existing occupants
        // unaffected and may still leave normally. ---
        let tore_down_immediately = registry
            .begin_draining(id)
            .expect("Active -> Draining is legal");
        assert!(
            !tore_down_immediately,
            "occupants are still present, so this must not skip straight to Teardown"
        );
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Draining));
        let latecomer = dummy_entity(103);
        assert_eq!(
            registry.try_add_occupant(id, latecomer),
            Err(DimensionError::NotAcceptingEntrants(
                id,
                DimensionLifecycle::Draining
            )),
            "no new entrant may join a Draining dimension"
        );
        // One existing occupant leaves — dimension stays Draining (still
        // one occupant left, so it must NOT tear down yet).
        let just_left = registry.remove_occupant(id, occupant_a).unwrap();
        assert!(
            !just_left,
            "should not auto-teardown while an occupant remains"
        );
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Draining));

        // --- Teardown: the LAST occupant leaves -> real, tested exit
        // condition (not an admin command). ---
        let just_left = registry.remove_occupant(id, occupant_b).unwrap();
        assert!(
            just_left,
            "the last occupant leaving a Draining dimension should tear it down"
        );
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Teardown));
    }

    #[test]
    fn begin_draining_rejects_non_active_dimensions() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        let id = DimensionId(4);
        registry.insert_spinning_up(id, root, 0).unwrap();
        // Still Spinup — draining a not-yet-active dimension is nonsensical.
        assert_eq!(
            registry.begin_draining(id),
            Err(DimensionError::WrongLifecycle(
                id,
                DimensionLifecycle::Spinup,
                DimensionLifecycle::Active
            ))
        );
    }

    /// A dimension that never had (or no longer has) any occupants when
    /// draining begins is, by definition, already fully drained — this is
    /// the exact case `xindeler-server-app`'s admin-command demo hits (no
    /// Bevy-side occupant ever gets registered there), so it must not get
    /// stuck in `Draining` forever waiting for a `remove_occupant` call that
    /// will never come.
    #[test]
    fn begin_draining_an_occupant_less_dimension_tears_down_immediately() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        let id = DimensionId(5);
        registry.insert_spinning_up(id, root, 0).unwrap();
        let (world, index) = World::empty();
        registry
            .complete_spinup(id, Arc::new(world), index)
            .unwrap();
        assert_eq!(registry.get(id).unwrap().occupant_count(), 0);

        let tore_down_immediately = registry
            .begin_draining(id)
            .expect("Active -> Draining is legal");
        assert!(
            tore_down_immediately,
            "an already-empty dimension should skip straight to Teardown"
        );
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Teardown));
    }

    /// EM-4.6's own removal step: only a `Teardown` dimension can be
    /// removed, and removing it actually drops it from the map (so a
    /// subsequent `get`/`contains` sees nothing — the registry-side half of
    /// spec §1.9's "zero entities/resources carry the dead `DimensionId`"
    /// acceptance bar).
    #[test]
    fn remove_torn_down_removes_a_teardown_dimension_from_the_registry() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        let id = DimensionId(6);
        registry.insert_spinning_up(id, root, 0).unwrap();
        let (world, index) = World::empty();
        registry
            .complete_spinup(id, Arc::new(world), index)
            .unwrap();
        registry
            .begin_draining(id)
            .expect("Active -> Draining is legal");
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Teardown));

        let removed = registry.remove_torn_down(id).expect("id is Teardown");
        assert_eq!(removed.root(), root);
        assert!(!registry.contains(id));
        assert!(registry.get(id).is_none());
    }

    #[test]
    fn remove_torn_down_rejects_a_dimension_that_is_not_teardown() {
        let mut registry = DimensionRegistry::default();
        let root = dummy_entity(0);
        let id = DimensionId(7);
        registry.insert_spinning_up(id, root, 0).unwrap();
        let (world, index) = World::empty();
        registry
            .complete_spinup(id, Arc::new(world), index)
            .unwrap();
        // Still Active, never drained. `DimensionState` (the `Ok` payload)
        // has no `PartialEq`/`Debug` (holds an `Arc<World>`), so `unwrap_err`/
        // `assert_eq!` on the whole `Result` don't work here — match instead.
        match registry.remove_torn_down(id) {
            Err(err) => assert_eq!(
                err,
                DimensionError::WrongLifecycle(
                    id,
                    DimensionLifecycle::Active,
                    DimensionLifecycle::Teardown
                )
            ),
            Ok(_) => panic!("removing a non-Teardown dimension should be rejected"),
        }
        // Untouched — still registered, still Active.
        assert!(registry.contains(id));
    }

    #[test]
    fn remove_torn_down_rejects_an_unknown_dimension() {
        let mut registry = DimensionRegistry::default();
        match registry.remove_torn_down(DimensionId(999)) {
            Err(err) => assert_eq!(err, DimensionError::NotFound(DimensionId(999))),
            Ok(_) => panic!("removing an unregistered dimension should be rejected"),
        }
    }

    /// The two dimensions' chunk stores are entirely separate `HashMap`s
    /// generated from entirely separate (tiny, real) `World`/`IndexOwned`
    /// pairs — proving a chunk generated for one dimension can never leak
    /// into another's store, even at an IDENTICAL `Vec2` key. Needs real
    /// assets (`Index::new` loads the color/feature manifests) — matches the
    /// existing `#[ignore]` convention for asset-dependent tests elsewhere
    /// in this codebase (`xindeler-sim-bridge`'s `boots_and_ticks_100_times`
    /// etc.).
    #[test]
    #[ignore = "boots real (tiny) worlds: needs assets; run locally with \
                VELOREN_ASSETS=\"$(pwd)/assets\""]
    fn chunk_generation_is_isolated_per_dimension() {
        use server::{FileOpts, GenOpts, World as ServerWorld, WorldOpts};

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        // x_lg/y_lg must stay >= 4 (see spinup.rs's doc comment on the same
        // constraint) — 5 keeps a safety margin.
        let tiny = GenOpts {
            x_lg: 5,
            y_lg: 5,
            ..GenOpts::default()
        };

        let (world_a, index_a) = ServerWorld::generate(
            1,
            WorldOpts {
                seed_elements: true,
                world_file: FileOpts::Generate(tiny.clone()),
                calendar: None,
            },
            &pool,
            &|_| {},
        );
        let (world_b, index_b) = ServerWorld::generate(
            2, // different seed -> a genuinely different world
            WorldOpts {
                seed_elements: true,
                world_file: FileOpts::Generate(tiny),
                calendar: None,
            },
            &pool,
            &|_| {},
        );

        let mut registry = DimensionRegistry::default();
        let root_a = dummy_entity(0);
        let root_b = dummy_entity(1);
        registry
            .insert_spinning_up(DimensionId(0), root_a, 0)
            .unwrap();
        registry
            .complete_spinup(DimensionId(0), Arc::new(world_a), index_a)
            .unwrap();
        registry
            .insert_spinning_up(DimensionId(1), root_b, 1)
            .unwrap();
        registry
            .complete_spinup(DimensionId(1), Arc::new(world_b), index_b)
            .unwrap();

        let pos = Vec2::new(0, 0);
        let chunk_a = registry
            .generate_chunk(DimensionId(0), pos)
            .expect("dimension 0 chunk gen");
        let chunk_b = registry
            .generate_chunk(DimensionId(1), pos)
            .expect("dimension 1 chunk gen");

        // Not the SAME Arc (no cache aliasing across dimensions)...
        assert!(!Arc::ptr_eq(&chunk_a, &chunk_b));
        // ...and each dimension's own store holds exactly its own chunk.
        assert_eq!(
            registry
                .get(DimensionId(0))
                .unwrap()
                .generated_chunk_count(),
            1
        );
        assert_eq!(
            registry
                .get(DimensionId(1))
                .unwrap()
                .generated_chunk_count(),
            1
        );
        // Re-requesting the same key returns the cached Arc, not a fresh
        // generation (same dimension, same pointer).
        let chunk_a_again = registry.generate_chunk(DimensionId(0), pos).unwrap();
        assert!(Arc::ptr_eq(&chunk_a, &chunk_a_again));
    }
}
