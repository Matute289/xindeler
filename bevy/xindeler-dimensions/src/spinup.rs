//! Spinning up a brand-new [`DimensionId`] at runtime (migration spec §5.3 /
//! `2026-07-10-bl82-phase4-remaining-plan.md` §1.8): `world`'s generator runs
//! with a seed modifier + (today, aspirational) biome profile on Bevy's
//! `AsyncComputeTaskPool` — never blocking the tick that requests it.
//!
//! ## What "seed_modifier" and "biome profile" concretely map to today
//! Neither concept exists as a dedicated type in `world`/`common` (confirmed
//! 2026-07-10 research: zero hits for `seed_modifier`/`biome_profile` in
//! those crates). This module reuses
//! [`xindeler_oracle_host::DimensionConfig`] (EM-4.3/4.4) directly
//! — the exact `{ seed_modifier: u64, biome_profile: String }` shape a
//! future `DmEvent`-triggered spinup (EM-4.9) will hand in — rather than
//! re-declaring a parallel config struct that could drift from it.
//! `seed_modifier` XORs onto a base seed to produce the REAL `u32` seed
//! `World::generate` takes (truncated — `DimensionConfig`'s own doc already
//! notes "any `u64` is a legal seed", so a truncation is a safe, documented
//! narrowing, not data loss that matters). `biome_profile` is carried through
//! for observability/future use but does NOT yet reshape terrain generation
//! — there is no `world`-side biome-parameter schema to map it onto (same
//! honesty `dm_event.rs`'s own doc comment states about the field: "no
//! `world`-side allowlist exists yet"). The actual terrain SHAPE knob for a
//! spinup is [`DimensionSpinupConfig::world_gen`] (a real `server::GenOpts`),
//! which `DmEvent`'s schema doesn't carry at all today — EM-4.9 will need to
//! decide how a `DmEvent` picks one; this task just needs SOME real,
//! test-fast default.

use std::sync::Arc;

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on},
};
use server::{FileOpts, GenOpts, IndexOwned, World, WorldOpts};
use xindeler_oracle_host::DimensionConfig;

use crate::{component::DimensionId, registry::DimensionRegistry};

/// Config for spinning up a NEW dimension. Wraps the reused
/// [`DimensionConfig`] (seed/biome, from `DmEvent`) plus the world-gen SHAPE
/// knob `DmEvent` doesn't carry — see the module doc.
#[derive(Debug, Clone)]
pub struct DimensionSpinupConfig {
    /// Reused verbatim from `xindeler_oracle_host::DimensionConfig`
    /// — see the module doc for what each field maps onto today.
    pub dimension_config: DimensionConfig,
    /// Real `world`-crate generation shape (map size / erosion / map kind).
    pub world_gen: GenOpts,
}

impl Default for DimensionSpinupConfig {
    fn default() -> Self {
        Self {
            dimension_config: DimensionConfig::default(),
            // Deliberately tiny (dev/test default): a full
            // `DEFAULT_WORLD_MAP`-sized `generate()` would make every
            // dimension spinup as slow as booting a whole second production
            // server. A caller building a "real" second dimension for actual
            // play should override this with a larger `GenOpts`.
            //
            // `x_lg`/`y_lg` must stay >= 4 (16 chunks/axis): `WorldSim::
            // seed_elements` divides the chunk-grid size by a fixed
            // `cell_size = 16` (`world/src/sim/mod.rs`) — below that, integer
            // division yields a 0-sized location grid axis and a subsequent
            // `% 0` panics. 5 (32 chunks/axis) keeps a safety margin above
            // that exact boundary while staying fast to generate.
            world_gen: GenOpts {
                x_lg: 5,
                y_lg: 5,
                ..GenOpts::default()
            },
        }
    }
}

/// Requests spinning up a brand-new [`DimensionId`] — the "debug/admin
/// command" trigger this task implements (a full `DmEvent`-triggered spinup
/// is EM-4.9's job). Send via `MessageWriter<SpinupDimension>`.
#[derive(Message, Debug, Clone)]
pub struct SpinupDimension {
    pub id: DimensionId,
    /// Base world seed this dimension's `seed_modifier` XORs onto.
    pub base_seed: u32,
    pub config: DimensionSpinupConfig,
}

/// Requests an EXISTING `Active` dimension begin draining — the admin
/// command half of spec §1.8's `Draining` transition (the PREDICTIVE
/// auto-trigger is EM-4.6's job, T47.8).
#[derive(Message, Debug, Clone, Copy)]
pub struct DrainDimension(pub DimensionId);

/// The sim's rayon thread pool, reused for dimension-spinup world generation
/// (the SAME `Arc<rayon::ThreadPool>` `Server::state().thread_pool()`
/// already built for the default dimension's own worldgen — no second pool
/// is built). Inserted by the shell once a live sim exists (mirrors how
/// `xindeler-sim-bridge`/`xindeler-server-app` insert their own `SimServer`
/// non-send resource only once the sim is ready).
#[derive(Resource, Clone)]
pub struct WorldGenThreadPool(pub Arc<rayon::ThreadPool>);

/// One in-flight spinup: the dimension it will complete, plus the Bevy
/// `Task` doing the actual `World::generate` work.
struct SpinupTask {
    id: DimensionId,
    task: Task<(World, IndexOwned)>,
}

/// In-flight spinup tasks, polled to completion by [`poll_spinup_tasks`].
#[derive(Resource, Default)]
pub struct SpinupTasks(Vec<SpinupTask>);

/// Reads [`SpinupDimension`] messages, registers the dimension in
/// [`crate::lifecycle::DimensionLifecycle::Spinup`] immediately, then spawns
/// the ACTUAL procgen work (`World::generate` — the same call
/// `server::Server::new` makes today for the default dimension, just with a
/// distinct seed/size) onto `AsyncComputeTaskPool`, never blocking this
/// system or the tick that requested it.
pub fn handle_spinup_requests(
    mut commands: Commands,
    mut registry: ResMut<DimensionRegistry>,
    mut requests: MessageReader<SpinupDimension>,
    mut tasks: ResMut<SpinupTasks>,
    thread_pool: Option<Res<WorldGenThreadPool>>,
) {
    let Some(thread_pool) = thread_pool else {
        for req in requests.read() {
            tracing::warn!(
                id = ?req.id,
                "SpinupDimension requested but no WorldGenThreadPool is inserted yet; dropping"
            );
        }
        return;
    };

    for req in requests.read() {
        if registry.contains(req.id) {
            tracing::warn!(id = ?req.id, "spinup requested for a dimension that already exists; ignoring");
            continue;
        }

        let root = commands.spawn(DimensionId(req.id.0)).id();
        if let Err(err) = registry.insert_spinning_up(
            req.id,
            root,
            req.config.dimension_config.seed_modifier as u32,
        ) {
            tracing::error!(?err, id = ?req.id, "failed to register spinning-up dimension");
            commands.entity(root).despawn();
            continue;
        }

        let seed = req.base_seed ^ (req.config.dimension_config.seed_modifier as u32);
        let gen_opts = req.config.world_gen.clone();
        let pool_handle = Arc::clone(&thread_pool.0);
        let id = req.id;

        let pool = AsyncComputeTaskPool::get();
        let task = pool.spawn(async move {
            World::generate(
                seed,
                WorldOpts {
                    seed_elements: true,
                    world_file: FileOpts::Generate(gen_opts),
                    calendar: None,
                },
                &pool_handle,
                &|_stage| {},
            )
        });
        tasks.0.push(SpinupTask { id, task });
        tracing::info!(
            ?id,
            seed,
            "dimension spinup started (Spinup) — generating on the async task pool"
        );
    }
}

/// Polls in-flight spinup tasks; once one finishes, completes the
/// dimension's transition into `Active` (real world/index now in hand).
/// Mirrors the existing `is_finished()`/`block_on` pattern
/// `xindeler-render-voxel::pipeline::apply_chunk_meshes` already uses for
/// its own `AsyncComputeTaskPool` chunk-mesh tasks.
pub fn poll_spinup_tasks(mut registry: ResMut<DimensionRegistry>, mut tasks: ResMut<SpinupTasks>) {
    let mut i = 0;
    while i < tasks.0.len() {
        if tasks.0[i].task.is_finished() {
            let SpinupTask { id, task } = tasks.0.swap_remove(i);
            let (world, index) = block_on(task); // finished — returns immediately
            match registry.complete_spinup(id, Arc::new(world), index) {
                Ok(()) => tracing::info!(?id, "dimension spinup complete (Spinup -> Active)"),
                Err(err) => tracing::error!(?err, ?id, "failed to complete dimension spinup"),
            }
        } else {
            i += 1;
        }
    }
}

/// Reads [`DrainDimension`] admin-command messages and applies them via
/// [`DimensionRegistry::begin_draining`], logging (not panicking) on a
/// rejected request (e.g. the dimension isn't `Active`).
pub fn handle_drain_requests(
    mut registry: ResMut<DimensionRegistry>,
    mut requests: MessageReader<DrainDimension>,
) {
    for &DrainDimension(id) in requests.read() {
        match registry.begin_draining(id) {
            Ok(true) => tracing::info!(
                ?id,
                "dimension entering Draining (admin command) — had zero occupants, torn down \
                 immediately"
            ),
            Ok(false) => tracing::info!(?id, "dimension entering Draining (admin command)"),
            Err(err) => tracing::warn!(?err, ?id, "drain request rejected"),
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::{MinimalPlugins, app::PluginGroup};

    use super::*;
    use crate::plugin::DimensionsPlugin;

    /// Drives a REAL `SpinupDimension` message through a headless `App`
    /// (`DimensionsPlugin` + a real `WorldGenThreadPool`) to `Active`,
    /// proving the async task-pool path (spawn → poll → complete) actually
    /// works end to end, not just the registry's own direct-call API tested
    /// in `registry.rs`. Needs real assets (`Index::new` loads the color/
    /// feature manifests) — same convention as the rest of this crate's
    /// asset-dependent tests.
    #[test]
    #[ignore = "boots a real (tiny) world on the async task pool: needs assets; run locally with \
                VELOREN_ASSETS=\"$(pwd)/assets\""]
    fn spinup_dimension_message_reaches_active_via_the_task_pool() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(DimensionsPlugin);
        app.insert_resource(WorldGenThreadPool(Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap(),
        )));

        app.world_mut().write_message(SpinupDimension {
            id: DimensionId(1),
            base_seed: 0,
            config: DimensionSpinupConfig::default(),
        });

        // Generation is async and runs on REAL OS worker threads regardless
        // of how fast this loop calls `app.update()` — a tight loop with no
        // pacing can blast through many iterations before the background
        // generation has had any real wall-clock time to run, so this polls
        // against a wall-clock deadline (with a small per-iteration sleep)
        // rather than a fixed iteration count.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut lifecycle = None;
        while std::time::Instant::now() < deadline {
            app.update();
            let registry = app.world().resource::<DimensionRegistry>();
            lifecycle = registry.lifecycle(DimensionId(1));
            if lifecycle == Some(crate::lifecycle::DimensionLifecycle::Active) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            lifecycle,
            Some(crate::lifecycle::DimensionLifecycle::Active),
            "dimension should reach Active once its async spinup task completes"
        );
    }
}
