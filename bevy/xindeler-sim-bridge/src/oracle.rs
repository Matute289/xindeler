//! BL-82 EM-4.9 — the ORACLE ingestion chain, wired into the real
//! `xindeler-server-app` binary for the first time (task board T51.1-51.3,
//! spec §3.A/§3.B/§3.C's atmosphere-table half/§3.D's server-side half).
//!
//! Before this task, `xindeler_oracle_host::{DmEventPlugin,
//! EntityTemplatePlugin, ChroniclePlugin}` and `register_oracle_source` were
//! added to no running binary at all — every prior task (EM-4.3/4.4/4.7/4.8)
//! deliberately deferred the real wiring here. [`ServerOraclePlugin`] is that
//! wiring: it adds the three dormant plugins (the `oracle://` `AssetSource`
//! itself is registered separately, in `xindeler-server-app::main`, BEFORE
//! `AssetPlugin` — see that module's own two-phase-ordering comment) and
//! owns the producer system set that actually turns a dropped `.dmevent.ron`
//! file into a real encounter:
//!
//! 1. [`ingest_dm_events`] (`AssetEvent::Added`/`Modified`): allocates a fresh
//!    [`DimensionId`] ([`NextDimensionId`]), records it in
//!    [`OracleEventRegistry`], emits `SpinupDimension` (mapping
//!    `DmEvent.dimension_config` onto a documented, modest [`event_gen_opts`]
//!    world-gen shape), records the event's atmosphere into
//!    [`DimensionAtmospheres`] (Phase D's table), and registers the
//!    `on_enter_message` narrative hook. `world_rumor` → chronicle already
//!    auto-fires (`xindeler_oracle_host::chronicle::chronicle_hook_system`, no
//!    wiring needed here).
//! 2. [`spawn_event_minions`] (once the dimension reaches `Active` —
//!    [`DimensionActivated`]): resolves `spawning_rules.entity_templates` from
//!    `Assets<EntityTemplate>` (requesting+waiting on any not-yet-loaded
//!    handle), pre-loads the scatter-radius chunks around the ONE real
//!    terrain's centre (see `entity_factory`'s module doc's "one physical
//!    world" note), then calls [`crate::spawn_from_spawning_rules`] targeting
//!    the event's own `DimensionId`. Retries (no-ops, never panics) on every
//!    tick until both preconditions hold.
//! 3. [`retire_dm_events`] (`AssetEvent::Removed`): marks the event retired and
//!    emits `DrainDimension` —
//!    [`crate::release_dimension_occupants_on_drain_request`] (EM-4.9, Phase C)
//!    is what actually lets an NPC-only event dimension reach `Teardown` and
//!    get cleaned up from there, unmodified.
//!
//! Anti-chaos: every value already passed `DmEvent::sanitize`/
//! `EntityTemplate::sanitize` in their respective loaders — this module never
//! panics on a missing asset, an unknown template id, or a dimension that
//! vanished between `Active` and the spawn attempt; it `warn!`s and moves on.
//!
//! ## Why this lives in `xindeler-sim-bridge`, not `xindeler-server-app`
//! `xindeler-server-app` is a binary-only package (no `[lib]` target — see
//! `xindeler-protocol::interest`'s own doc comment for the same reasoning
//! applied to `ClientInterestPlugin`), so nothing under its own `tests/`
//! integration tests can `use` this module's items directly; every one of
//! that crate's existing tests instead proves behavior via a spawned
//! subprocess. [`ServerOraclePlugin`] needs a FAST, in-process Style-B test
//! (T51.9-E2, the CI-runnable partial) — so it lives here, in the crate that
//! IS a proper library and that `xindeler-server-app` already depends on
//! (a normal forward dependency; `xindeler-server-app::plugin::
//! SimServerPlugin` just adds `xindeler_sim_bridge::ServerOraclePlugin`, no
//! new edge). `xindeler-server-app::main` still owns the ONE call this
//! plugin's own doc comment requires happen before it
//! (`register_oracle_source`, BEFORE `AssetPlugin` — a `main.rs`-level
//! concern, unaffected by where the plugin struct itself is defined).

use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    asset::{AssetEvent, AssetId, AssetServer, Assets, Handle},
    ecs::{
        change_detection::NonSendMut,
        message::{MessageReader, MessageWriter},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res, ResMut},
    },
    log::{info, warn},
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use xindeler_dimensions::{
    DimensionActivated, DimensionId, DimensionLifecycle, DimensionRegistry, DimensionSpinupConfig,
    DimensionTornDown, DrainDimension, SpinupDimension,
};
use xindeler_oracle_host::{
    ChroniclePlugin, DimensionAtmospheres, DmEvent, DmEventPlugin, EntityTemplate,
    EntityTemplatePlugin,
};
use xindeler_protocol::NarrativeHooks;

use crate::SimServer;

/// Arbitrary — real per-event seed derivation beyond `DmEvent.dimension_config
/// .seed_modifier` (which already XORs onto whatever base seed is handed to
/// `SpinupDimension`) is out of this plumbing task's scope; what THIS task
/// proves is that the resulting dimension is independently generated and
/// isolated (same posture `xindeler-server-app::dimensions`'s own debug
/// command already documents for its `base_seed`).
const EVENT_BASE_SEED: u32 = 0;

/// The world-gen SHAPE a `DmEvent`-triggered dimension spins up with —
/// `DmEvent` itself doesn't carry a `GenOpts` (spec: EM-4.9 must decide how
/// one picks one). `x_lg`/`y_lg` = 5 (32 chunks/axis × 32 blocks/chunk = 1024
/// blocks/axis, i.e. a ±512-block half-extent from the dimension's centre —
/// matching `DimensionSpinupConfig::default()`'s own dev/test-fast choice)
/// comfortably holds the DmEvent schema's own `spawn_radius` clamp ceiling
/// (`xindeler_oracle_host::dm_event::bounds::SPAWN_RADIUS`, tightened to
/// 400.0 specifically so it stays inside this world size — see that
/// constant's own doc comment for the coupling this creates: the two must be
/// revisited together), while staying fast enough to spin up within the
/// drill's own polling deadlines. Named explicitly here (not silently
/// inherited from the default) so retuning it for a real production event
/// doesn't also affect the unrelated debug-command spinup path.
fn event_gen_opts() -> server::GenOpts {
    server::GenOpts {
        x_lg: 5,
        y_lg: 5,
        ..server::GenOpts::default()
    }
}

/// Converts an `EntityTemplate` id (`DmEvent.spawning_rules.entity_templates`
/// — a human-readable name, e.g. `"mist_bound_shade"`) to the asset path the
/// shipped sample templates live at. Matches the existing convention every
/// shipped `.entity_template.ron` already follows (`entity_template_id` ==
/// the filename stem).
fn entity_template_asset_path(id: &str) -> String {
    format!("xindeler/entity_templates/{id}.entity_template.ron")
}

/// Monotone allocator for fresh event-dimension ids. Starts at 1 —
/// `DimensionId::DEFAULT` (`0`) is never allocated by this producer.
///
/// ## Operational footgun (bevy-migration-reviewer MINOR finding, not fixed
/// here — a doc note only)
/// This allocator and `xindeler-server-app::dimensions`'s
/// `XINDELER_DEBUG_SPINUP_DIMENSION` debug/admin trigger (an operator-
/// supplied, arbitrary `u64`) both write into the SAME `DimensionRegistry`
/// namespace with no reserved-range separation. Running the debug command
/// with an id THIS allocator also happens to pick (e.g.
/// `XINDELER_DEBUG_SPINUP_DIMENSION=1` on a server that has also ingested
/// its first `DmEvent`) collides — handled gracefully today
/// (`DimensionRegistry::insert_spinning_up` rejects an already-registered id
/// rather than corrupting state, so this is not a safety issue), just a
/// confusing "why did my debug spinup silently fail" operational trap. Not
/// worth a real reservation scheme for the ONE-canonical-event v1 scope this
/// task covers; revisit if/when a second real ORACLE event or a wider debug-
/// tooling surface makes the collision likely rather than theoretical.
#[derive(Resource, Debug, Clone, Copy)]
struct NextDimensionId(u64);

impl Default for NextDimensionId {
    fn default() -> Self { Self(1) }
}

impl NextDimensionId {
    fn allocate(&mut self) -> DimensionId {
        let id = DimensionId(self.0);
        self.0 += 1;
        id
    }
}

/// Bookkeeping for one ingested `DmEvent` (BL-82 EM-4.9).
#[derive(Debug, Clone, Copy)]
struct ActiveEvent {
    dimension: DimensionId,
    /// Set once [`DimensionActivated`] fires for [`Self::dimension`].
    active: bool,
    /// Set once [`spawn_event_minions`] has successfully spawned this
    /// event's minions (or given up after the dimension vanished — see that
    /// function's doc comment) — a one-shot latch so a slow-to-resolve
    /// template/terrain precondition is retried, never re-spawned.
    spawned: bool,
    /// Set once [`retire_dm_events`] has emitted this event's
    /// `DrainDimension` — a one-shot latch so a second `AssetEvent::Removed`
    /// (shouldn't happen, but anti-chaos) never double-drains.
    retired: bool,
}

/// Every currently-tracked `DmEvent` this producer has ingested, keyed by its
/// asset id (BL-82 EM-4.9).
#[derive(Resource, Debug, Default)]
struct OracleEventRegistry(HashMap<AssetId<DmEvent>, ActiveEvent>);

/// Cache of requested [`EntityTemplate`] asset handles, keyed by the
/// `entity_template_id` string a `DmEvent`'s `spawning_rules` names (BL-82
/// EM-4.9) — holds the handle alive (an `AssetServer::load` result with no
/// surviving `Handle` gets unloaded again) and avoids re-requesting the same
/// path every tick while it's still loading.
#[derive(Resource, Debug, Default)]
struct EntityTemplateHandles(HashMap<String, Handle<EntityTemplate>>);

/// `AssetEvent::Added`/`Modified` → allocate a dimension, spin it up, record
/// its atmosphere + narrative hook (BL-82 EM-4.9, T51.2/T51.3). Never
/// re-ingests the SAME asset id twice (a `Modified` re-fire after the initial
/// `Added` — e.g. a hot-reload no-op rewrite — is a no-op here).
fn ingest_dm_events(
    mut events: MessageReader<AssetEvent<DmEvent>>,
    assets: Res<Assets<DmEvent>>,
    mut next_id: ResMut<NextDimensionId>,
    mut registry_state: ResMut<OracleEventRegistry>,
    mut spinup_writer: MessageWriter<SpinupDimension>,
    mut hooks: ResMut<NarrativeHooks>,
    mut atmospheres: ResMut<DimensionAtmospheres>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        if registry_state.0.contains_key(id) {
            continue;
        }
        let Some(dm_event) = assets.get(*id) else {
            continue;
        };

        let dimension = next_id.allocate();
        registry_state.0.insert(*id, ActiveEvent {
            dimension,
            active: false,
            spawned: false,
            retired: false,
        });

        spinup_writer.write(SpinupDimension {
            id: dimension,
            base_seed: EVENT_BASE_SEED,
            config: DimensionSpinupConfig {
                dimension_config: dm_event.dimension_config.clone(),
                world_gen: event_gen_opts(),
            },
        });
        atmospheres.set(dimension, dm_event.atmosphere.clone());
        if let Some(message) = &dm_event.narrative.on_enter_message {
            hooks.register_on_enter_message(dimension, message.clone());
        }

        info!(
            dimension = dimension.0,
            "mist-bound: dimension {} spinning up", dimension.0
        );
    }
}

/// Once a `DmEvent`'s dimension reaches `Active`, resolves its
/// `spawning_rules.entity_templates` and calls
/// [`crate::spawn_from_spawning_rules`] into that dimension (BL-82 EM-4.9,
/// T51.2). Runs every tick (not just the tick `DimensionActivated` fires) so
/// a not-yet-loaded template handle or a not-yet-generated terrain chunk
/// simply defers to the next tick — never a panic, never a permanent drop
/// unless the dimension itself disappears first (a defensive give-up,
/// logged once).
fn spawn_event_minions(
    sim: Option<NonSendMut<SimServer>>,
    mut commands: Commands,
    registry: Res<DimensionRegistry>,
    mut registry_state: ResMut<OracleEventRegistry>,
    dm_assets: Res<Assets<DmEvent>>,
    template_assets: Res<Assets<EntityTemplate>>,
    asset_server: Res<AssetServer>,
    mut handles: ResMut<EntityTemplateHandles>,
    mut activated: MessageReader<DimensionActivated>,
) {
    for DimensionActivated(dimension) in activated.read() {
        for active_event in registry_state.0.values_mut() {
            if active_event.dimension == *dimension {
                active_event.active = true;
            }
        }
    }

    let pending: Vec<AssetId<DmEvent>> = registry_state
        .0
        .iter()
        .filter(|(_, e)| e.active && !e.spawned && !e.retired)
        .map(|(id, _)| *id)
        .collect();

    for asset_id in pending {
        let dimension = registry_state.0[&asset_id].dimension;
        let Some(dm_event) = dm_assets.get(asset_id) else {
            continue; // asset unloaded again before we got to it; retry
        };

        if !registry
            .get(dimension)
            .is_some_and(|state| state.lifecycle() == DimensionLifecycle::Active)
        {
            warn!(
                dimension = dimension.0,
                "mist-bound: dimension vanished before its minions could spawn; giving up"
            );
            if let Some(active_event) = registry_state.0.get_mut(&asset_id) {
                active_event.spawned = true; // stop retrying a dead dimension
            }
            continue;
        }

        // Resolve every named template, requesting a handle for any not yet
        // seen; defer this event entirely until ALL of them have loaded.
        let mut templates = HashMap::new();
        let mut all_loaded = true;
        for template_id in &dm_event.spawning_rules.entity_templates {
            let handle = handles
                .0
                .entry(template_id.clone())
                .or_insert_with(|| asset_server.load(entity_template_asset_path(template_id)))
                .clone();
            match template_assets.get(&handle) {
                Some(template) => {
                    templates.insert(template_id.clone(), template.clone());
                },
                None => all_loaded = false,
            }
        }
        if !all_loaded {
            continue; // retry next tick
        }

        // Origin + terrain-readiness gate (BL-82 EM-3.11o): minions still
        // physically live in the ONE real terrain (see `entity_factory`'s
        // module doc for why), so scatter around its already-loaded centre
        // and wait for every chunk the scatter radius could touch before
        // ever spawning — the SAME "pre-load or anchor" fix this crate's own
        // Mist-Bound test already demonstrates, applied here to the real
        // producer.
        let origin = match &sim {
            Some(sim) => {
                let size = sim.server.world().sim().get_size();
                let centre_chunk = vek::Vec2::new(size.x as i32, size.y as i32) / 2;
                const CHUNK_SIZE: f32 = 32.0;
                let scatter_chunk_radius =
                    (dm_event.spawning_rules.spawn_radius / CHUNK_SIZE).ceil() as i32 + 1;
                let terrain = sim.server.state().terrain();
                let ready = (-scatter_chunk_radius..=scatter_chunk_radius).all(|dx| {
                    (-scatter_chunk_radius..=scatter_chunk_radius).all(|dy| {
                        terrain
                            .get_key_arc(centre_chunk + vek::Vec2::new(dx, dy))
                            .is_some()
                    })
                });
                drop(terrain);
                if !ready {
                    continue; // retry next tick
                }
                let centre = vek::Vec2::new(size.x as f32, size.y as f32) * CHUNK_SIZE * 0.5;
                let alt = sim
                    .server
                    .world()
                    .sim()
                    .get_alt_approx(centre.map(|e| e as i32))
                    .unwrap_or(0.0);
                [centre.x, centre.y, alt + 3.0]
            },
            // No live sim (e.g. a fast, sim-less headless test exercising
            // just this producer) — nothing to gate readiness against;
            // `spawn_from_spawning_rules` only spawns transient Bevy staging
            // entities regardless (see that function's own doc comment).
            None => [0.0, 0.0, 100.0],
        };

        let mut rng = ChaCha8Rng::seed_from_u64(dimension.0);
        let spawned = crate::spawn_from_spawning_rules(
            &mut commands,
            &templates,
            &dm_event.spawning_rules,
            origin,
            dimension,
            &mut rng,
        );

        if let Some(active_event) = registry_state.0.get_mut(&asset_id) {
            active_event.spawned = true;
        }
        info!(
            dimension = dimension.0,
            count = spawned.len(),
            "mist-bound: spawned {} minions into dimension {}",
            spawned.len(),
            dimension.0
        );
    }
}

/// Polls whether each tracked, not-yet-retired event's underlying FILE still
/// exists on disk, and drains its dimension the moment it doesn't (BL-82
/// EM-4.9, T51.2's "on Removed" step).
///
/// ## Why this polls the filesystem, NOT `AssetEvent::Removed`
/// An earlier version of this system read `AssetEvent::Removed` — the
/// natural-looking signal, and what the task's own design doc describes. It
/// does not work: verified empirically (the E2E drill's first run) against
/// `bevy_asset` 0.19's own `AssetServer::handle_internal_asset_events`
/// (`bevy_asset::server`) that a filesystem `AssetSourceEvent::RemovedAsset`
/// ONLY calls `reload_parent_folders` — it never reloads (or unloads) the
/// removed path's own asset. `AssetEvent::Removed` is fired ONLY when an
/// asset's last strong `Handle` is dropped (ref-count reaches zero), which
/// never happens here: [`WellKnownEventHandles`] deliberately holds a
/// permanent handle (see its own doc comment for why) specifically so the
/// watcher can observe a LATER re-write of the same well-known path — so the
/// asset itself is NEVER unloaded by deleting the file underneath it; it just
/// silently keeps its last-successfully-loaded content, with the reload
/// attempt failing (a logged `bevy_asset::server` "Path not found" error,
/// harmless noise, not a panic). So retirement can only be observed the same
/// way a human dropping the file DID it: by checking whether the file is
/// still there.
fn retire_dm_events(
    events_dir: Res<OracleEventsDir>,
    well_known_paths: Res<WellKnownEventFilenames>,
    mut registry_state: ResMut<OracleEventRegistry>,
    mut drain_writer: MessageWriter<DrainDimension>,
) {
    // Collect the ids to retire first (rather than mutating while iterating
    // `registry_state.0` — we need `&mut registry_state` below to REMOVE the
    // entry, which a live `.iter_mut()` borrow would conflict with).
    let to_retire: Vec<AssetId<DmEvent>> = registry_state
        .0
        .iter()
        .filter(|(asset_id, active_event)| {
            !active_event.retired
                && well_known_paths
                    .0
                    .get(*asset_id)
                    .is_some_and(|filename| !events_dir.0.join(filename).exists())
        })
        .map(|(asset_id, _)| *asset_id)
        .collect();

    for asset_id in to_retire {
        // BL-82 EM-4.9 follow-up (bevy-migration-reviewer MAJOR finding):
        // REMOVE the entry entirely, not just flag it `retired`.
        // `WellKnownEventHandles` holds the SAME asset id's `Handle` for the
        // App's whole lifetime (see its own doc comment for why), so a
        // later re-write of this well-known path reloads into the SAME
        // `AssetId` — `ingest_dm_events`'s own dedup check
        // (`registry_state.0.contains_key(id)`) would otherwise silently
        // refuse to re-ingest it FOREVER after the first retire, with no
        // warning anywhere. Removing here is what lets a human (or a test)
        // drop the SAME canonical event file again after retiring it once.
        let Some(active_event) = registry_state.0.remove(&asset_id) else {
            continue;
        };
        drain_writer.write(DrainDimension(active_event.dimension));
        info!(
            dimension = active_event.dimension.0,
            "mist-bound: retiring dimension {}", active_event.dimension.0
        );
    }
}

/// Drops a torn-down dimension's entries from [`DimensionAtmospheres`] and
/// [`NarrativeHooks`] (BL-82 EM-4.9 follow-up, bevy-migration-reviewer MINOR
/// finding #4): without this, both tables would grow by one stale entry per
/// retired event for the life of the server process — [`retire_dm_events`]
/// removing its OWN [`OracleEventRegistry`] entry bounds THAT table, but
/// these two side tables are populated independently (by [`ingest_dm_events`])
/// and need their own cleanup on the same `DimensionTornDown` edge every
/// other per-dimension observer in this codebase already reacts to (see
/// `xindeler-server-app::dimensions::DimensionMetrics`'s own
/// `teardowns_total` counter for the same message).
fn cleanup_dimension_side_tables_on_teardown(
    mut torn_down: MessageReader<DimensionTornDown>,
    mut atmospheres: ResMut<DimensionAtmospheres>,
    mut hooks: ResMut<NarrativeHooks>,
) {
    for event in torn_down.read() {
        atmospheres.remove(event.id);
        hooks.unregister(event.id);
    }
}

/// The ONE canonical, shipped event this drill's producer knows about by
/// name (`assets/xindeler/oracle_events/mist_bound.dmevent.ron`). A general
/// "scan the whole oracle:// directory for any file" watcher is a nicer v2
/// (out of this task's scope, which is specifically the ONE Mist-Bound
/// example) — see [`request_well_known_events`]'s doc comment for why even
/// this single well-known path needs an explicit pre-request. Bare filenames
/// (not `oracle://`-prefixed) — [`request_well_known_events`] builds the
/// asset-server load path, [`retire_dm_events`] builds the on-disk path;
/// both derive from this ONE list so they can never drift apart.
const WELL_KNOWN_EVENT_FILENAMES: &[&str] = &["mist_bound.dmevent.ron"];

/// The directory `oracle://` is rooted at (BL-82 EM-4.9) — resolved
/// independently here via the SAME `xindeler_oracle_host::default_events_dir`
/// call `xindeler-server-app::main` already used for
/// `register_oracle_source` (both read the identical
/// `XINDELER_ORACLE_EVENTS_DIR` env var, so the two calls always agree; a
/// constructor field would need threading this plugin's insertion point
/// through `main.rs`, for no benefit over the already-pure, already-public
/// helper function). [`retire_dm_events`] uses this to check a well-known
/// event's file existence directly — see that function's own doc comment for
/// why polling the filesystem, not an `AssetEvent`, is required.
#[derive(Resource, Debug, Clone)]
struct OracleEventsDir(std::path::PathBuf);

/// Holds the [`Handle`]s [`request_well_known_events`] requests, alive for
/// the App's whole lifetime — without a surviving strong handle,
/// `Assets<DmEvent>` would unload the asset again the moment the initial
/// (possibly failed, if the file doesn't exist yet) load settles, and no
/// later file-watcher event would have anything to attach to.
#[derive(Resource, Default)]
struct WellKnownEventHandles(Vec<Handle<DmEvent>>);

/// `AssetId<DmEvent> -> filename` for every handle
/// [`request_well_known_events`] requested — lets [`retire_dm_events`] map a
/// registered `DmEvent` asset back to the on-disk filename it must poll for
/// existence.
#[derive(Resource, Default)]
struct WellKnownEventFilenames(HashMap<AssetId<DmEvent>, &'static str>);

/// Requests every [`WELL_KNOWN_EVENT_FILENAMES`] entry at `Startup`, whether
/// or not the file exists yet.
///
/// ## Why this is required, not just a nicety
/// `bevy_asset`'s file watcher ONLY reloads paths that already have an
/// OUTSTANDING handle (verified against `bevy_asset`'s own test suite,
/// `reloads_asset_after_source_event` — see `xindeler_oracle_host::dm_event`'s
/// own hot-reload test for the same finding) — a brand-new, never-requested
/// path dropped into the watched `oracle://` directory is NOT auto-discovered
/// merely by watching the directory. Without this system, a human (or this
/// drill's test) dropping `mist_bound.dmevent.ron` live into
/// `<userdata>/oracle_events` would silently do NOTHING — no `AssetEvent`
/// ever fires, so [`ingest_dm_events`] never sees it. Requesting the
/// well-known path eagerly at boot (the file usually doesn't exist yet, so
/// this first load is EXPECTED to fail quietly — same as the `dm_event.rs`
/// test's own "let the failed first attempt settle" step) means the file
/// WRITE later is what completes an already-outstanding request, which the
/// watcher DOES observe.
fn request_well_known_events(
    asset_server: Res<AssetServer>,
    mut handles: ResMut<WellKnownEventHandles>,
    mut filenames: ResMut<WellKnownEventFilenames>,
) {
    for filename in WELL_KNOWN_EVENT_FILENAMES {
        let handle: Handle<DmEvent> = asset_server.load(format!("oracle://{filename}"));
        filenames.0.insert(handle.id(), filename);
        handles.0.push(handle);
    }
}

/// Wires the dormant ORACLE ingestion chain into a real Bevy `App` (BL-82
/// EM-4.9, T51.1). Add AFTER `AssetPlugin` (this plugin's
/// `DmEventPlugin`/`EntityTemplatePlugin` need `AssetServer` to already
/// exist) and after `xindeler_oracle_host::register_oracle_source` has been
/// called with the SAME `events_dir` (BEFORE `AssetPlugin` — see that
/// function's own two-phase ordering doc comment; `xindeler-server-app::main`
/// owns that call and threads its one resolved `events_dir` value through to
/// both). Never added to `xindeler-client` — only a server-side host is meant
/// to ever register [`DmEventPlugin`].
pub struct ServerOraclePlugin {
    /// The `oracle://` watch directory — MUST be the exact same path handed
    /// to `xindeler_oracle_host::register_oracle_source`, or
    /// [`retire_dm_events`]'s filesystem poll will check the wrong
    /// directory. Defaults to `xindeler_oracle_host::default_events_dir()`
    /// (which itself honors `XINDELER_ORACLE_EVENTS_DIR`) for any caller that
    /// doesn't need to override it (e.g. an in-process test that also calls
    /// `register_oracle_source` with the default).
    pub events_dir: std::path::PathBuf,
}

impl Default for ServerOraclePlugin {
    fn default() -> Self {
        Self {
            events_dir: xindeler_oracle_host::default_events_dir(),
        }
    }
}

impl Plugin for ServerOraclePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((DmEventPlugin, EntityTemplatePlugin, ChroniclePlugin));
        app.init_resource::<NextDimensionId>();
        app.init_resource::<OracleEventRegistry>();
        app.init_resource::<EntityTemplateHandles>();
        app.init_resource::<WellKnownEventHandles>();
        app.init_resource::<WellKnownEventFilenames>();
        app.insert_resource(OracleEventsDir(self.events_dir.clone()));
        app.add_systems(bevy::app::Startup, request_well_known_events);

        app.add_systems(
            FixedUpdate,
            ingest_dm_events.before(xindeler_dimensions::spinup::handle_spinup_requests),
        );
        app.add_systems(
            FixedUpdate,
            retire_dm_events.before(xindeler_dimensions::spinup::handle_drain_requests),
        );
        app.add_systems(
            FixedUpdate,
            spawn_event_minions
                .after(xindeler_dimensions::spinup::poll_spinup_tasks)
                .before(crate::apply_pending_entity_template_spawns),
        );
        app.add_systems(
            FixedUpdate,
            cleanup_dimension_side_tables_on_teardown
                .after(xindeler_dimensions::teardown::teardown_completed_dimensions),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        MinimalPlugins,
        app::{App, PluginGroup},
        asset::AssetPlugin,
        ecs::message::Messages,
    };
    use xindeler_dimensions::{DimensionActivated, DimensionsPlugin, SpinupDimension};
    use xindeler_oracle_host::entity_template::PendingEntityTemplateSpawn;

    use super::*;

    /// Style-B (T51.9-E2, the fast CI-runnable partial): boots a headless
    /// `App` with the REAL `ServerOraclePlugin` (no `SimServer`/real world —
    /// `spawn_event_minions`'s `sim: Option<...>` handles that gracefully,
    /// see its own doc comment) pointed at the REAL repo `assets/` root (text
    /// RON assets, never LFS — see CLAUDE.md's Git-LFS policy — so this
    /// needs no `VELOREN_ASSETS`/LFS and stays fast/non-`#[ignore]`d) and a
    /// tempdir `oracle://` source, and proves the producer→spinup→spawn→hooks
    /// chain: dropping the shipped `mist_bound.dmevent.ron` (a) allocates a
    /// dimension and emits a matching `SpinupDimension`, (b) appends its
    /// `world_rumor` to the chronicle, (c) registers its `on_enter_message`
    /// narrative hook, and (d) — once manually told the dimension reached
    /// `Active` (this test never drives real worldgen to completion, so it
    /// synthesizes that one edge itself) — resolves the REAL shipped
    /// `mist_bound_shade.entity_template.ron` and stages exactly 15 pending
    /// factory-spawn requests tagged with that dimension.
    #[test]
    fn ingest_spinup_spawn_and_hooks_chain_without_a_real_sim() {
        use std::time::{Duration, Instant};

        let repo_assets_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .canonicalize()
            .expect("workspace assets/ dir exists");
        let oracle_dir = tempfile::tempdir().expect("tempdir");
        let oracle_root = oracle_dir
            .path()
            .canonicalize()
            .expect("canonicalize oracle tempdir (macOS symlink gotcha)");

        let mut app = App::new();
        xindeler_oracle_host::register_oracle_source(&mut app, &oracle_root);
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(AssetPlugin {
            file_path: repo_assets_dir.to_string_lossy().into_owned(),
            ..Default::default()
        });
        // `HudToastPlugin`/`ServerAtmosphereSyncPlugin` both register
        // `bevy_replicon` server messages (`add_server_message`), which
        // needs `RepliconPlugins`'/`StatesPlugin`'s own scaffolding already
        // present — same requirement every other test in this workspace that
        // touches those plugins (e.g. `xindeler_protocol::narrative`'s own
        // tests) already documents. `HudToast` itself (the message TYPE, as
        // opposed to `NarrativeHooks`/`fire_on_enter_toasts`, which
        // `HudToastPlugin` alone provides) is registered by
        // `XindelerProtocolPlugin`, not `HudToastPlugin` — the real shell
        // (`xindeler-server-app::plugin::SimServerPlugin`) always adds both
        // together; this test must too, or `fire_on_enter_toasts`' own
        // `MessageWriter<ToClients<HudToast>>` panics ("Message not
        // initialized").
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy_replicon::prelude::RepliconPlugins);
        app.add_plugins(xindeler_protocol::XindelerProtocolPlugin);
        app.add_plugins(DimensionsPlugin);
        app.add_plugins(xindeler_protocol::HudToastPlugin);
        app.add_plugins(xindeler_oracle_host::ServerAtmosphereSyncPlugin);
        // `events_dir` MUST match the `oracle_root` `register_oracle_source`
        // above was given, or `retire_dm_events`'s filesystem poll checks the
        // wrong directory (see `ServerOraclePlugin::events_dir`'s own doc
        // comment) — not exercised by THIS test (it never retires), but
        // wrong-by-default would be a silent trap for a future test that does.
        app.add_plugins(ServerOraclePlugin {
            events_dir: oracle_root.clone(),
        });
        app.finish();
        app.update();

        // Drop the SHIPPED Mist-Bound fixture into the watched oracle dir.
        // `ServerOraclePlugin`'s own `request_well_known_events` (`Startup`)
        // already requested this exact well-known path a moment ago (see
        // that function's doc comment for why a pre-existing outstanding
        // handle is required for the watcher to notice this write at all) —
        // let that settle first, mirroring `dm_event.rs`'s own hot-reload
        // test's "let the failed first attempt settle" step.
        for _ in 0..20 {
            app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        let dmevent_text =
            include_str!("../../../assets/xindeler/oracle_events/mist_bound.dmevent.ron");
        std::fs::write(
            oracle_dir.path().join("mist_bound.dmevent.ron"),
            dmevent_text,
        )
        .expect("write the fixture into the watched dir");

        // (a) wait for `SpinupDimension` to fire and the world_rumor to
        // reach the chronicle.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut spun_up_dimension = None;
        while Instant::now() < deadline {
            app.update();
            if let Some(msg) = app
                .world_mut()
                .resource_mut::<Messages<SpinupDimension>>()
                .drain()
                .next()
            {
                spun_up_dimension = Some(msg.id);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let dimension = spun_up_dimension
            .expect("ingest_dm_events must emit SpinupDimension for the dropped file");
        assert_ne!(dimension, xindeler_dimensions::DimensionId::DEFAULT);

        assert!(
            app.world()
                .resource::<xindeler_oracle_host::ChronicleLog>()
                .iter()
                .any(|entry| entry.contains("Mist-Bound")),
            "the world_rumor must have reached the chronicle log"
        );
        assert_eq!(
            app.world()
                .resource::<xindeler_protocol::NarrativeHooks>()
                .on_enter_message(dimension),
            Some("The veil parts. You have crossed into the Mist-Bound.")
        );
        assert!(
            app.world()
                .resource::<xindeler_oracle_host::DimensionAtmospheres>()
                .get(dimension)
                .is_some_and(|profile| profile.time_lock == Some(23.5)),
            "the event's atmosphere (locked 23.5h) must be recorded in the per-dimension table"
        );

        // (d) synthesize the Active edge (this test never inserts a real
        // `WorldGenThreadPool`, so `handle_spinup_requests` — which needs one
        // — drops the `SpinupDimension` message above with a `warn!` rather
        // than ever registering the dimension; `spawn_event_minions`'s own
        // registry-liveness gate would otherwise "give up" immediately). Register
        // the dimension directly via the SAME two-call
        // `insert_spinning_up`/`complete_spinup` sequence
        // `xindeler_dimensions::registry`'s own unit tests use for a
        // no-real-worldgen stand-in (`World::empty()` needs no assets at
        // all), THEN fire `DimensionActivated` and confirm the producer
        // stages exactly 15 pending factory-spawn requests, all tagged with
        // the event dimension, resolved from the REAL shipped
        // `entity_template.ron`.
        {
            let root = app.world_mut().spawn(dimension).id();
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            registry
                .insert_spinning_up(dimension, root, 0)
                .expect("dimension not already registered");
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(dimension, std::sync::Arc::new(world), index)
                .expect("Spinup -> Active");
        }
        app.world_mut().write_message(DimensionActivated(dimension));
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            app.update();
            let staged = app
                .world_mut()
                .query::<&PendingEntityTemplateSpawn>()
                .iter(app.world())
                .filter(|p| p.dimension == dimension)
                .count();
            if staged == 15 {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "spawn_event_minions never staged the expected 15 PendingEntityTemplateSpawn requests \
             for dimension {dimension:?} within the deadline"
        );
    }
}
