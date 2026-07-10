//! Reading the default dimension's already-generated world data off a live
//! `server::Server` — the concrete "wrap today's single `Arc<World>` +
//! `IndexOwned`" step both shells ([`xindeler-server-app`],
//! [`xindeler-sim-bridge`]) perform at boot.

use std::sync::Arc;

use bevy::ecs::entity::Entity;
use server::{IndexOwned, World};
use specs::WorldExt;

use crate::registry::{DimensionError, DimensionId, DimensionRegistry};

/// Reads the ALREADY-generated `Arc<World>`/`IndexOwned` off a live
/// `server::Server`'s specs ECS resources
/// (`server/src/lib.rs:541-544` inserts them once, at boot) — the exact pair
/// [`crate::DimensionId::DEFAULT`] wraps. Read-only (isolation law rule 4):
/// never writes into the sim, never re-generates anything.
///
/// # Panics
/// Panics if called before the sim has finished booting (i.e. before
/// `server::Server::new` returned) — both resources are ALWAYS present on a
/// constructed `Server`, so this is a programming-error guard, not a runtime
/// condition callers need to handle.
pub fn read_default_world(server: &server::Server) -> (Arc<World>, IndexOwned) {
    let ecs = server.state().ecs();
    let world = Arc::clone(&ecs.read_resource::<Arc<World>>());
    let index: IndexOwned = (*ecs.read_resource::<IndexOwned>()).clone();
    (world, index)
}

/// The shared "wrap `DimensionId::DEFAULT`" sequence both shells
/// (`xindeler-server-app`, `xindeler-sim-bridge`) perform at boot: register
/// dimension 0 as [`crate::lifecycle::DimensionLifecycle::Spinup`], then
/// immediately complete it with the sim's REAL already-generated world/index
/// (via [`read_default_world`]) — a wrapping refactor of already-existing
/// state, not a behavior change (spec §1.8).
///
/// Takes an already-spawned `root: Entity` rather than spawning one itself,
/// because the two callers spawn it through different mechanisms (one via
/// `app.world_mut().spawn(..)` at `Plugin::build` time, the other via
/// `Commands` inside a Bevy system) — everything AFTER "the root entity
/// exists" is identical between them, and lives here so it can't silently
/// drift between the two call sites.
///
/// # Errors
/// Only if `DimensionId::DEFAULT` is somehow already registered (should
/// never happen — each shell calls this exactly once, at boot).
pub fn wrap_default_dimension(
    registry: &mut DimensionRegistry,
    root: Entity,
    server: &server::Server,
) -> Result<(), DimensionError> {
    registry.insert_spinning_up(DimensionId::DEFAULT, root, 0)?;
    let (world, index) = read_default_world(server);
    registry.complete_spinup(DimensionId::DEFAULT, world, index)
}
