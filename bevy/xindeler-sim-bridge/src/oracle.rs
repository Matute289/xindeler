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

use std::collections::{HashMap, HashSet};

use bevy::{
    app::{App, FixedUpdate, Plugin},
    asset::{AssetEvent, AssetId, AssetServer, Assets, Handle},
    ecs::{
        change_detection::{NonSend, NonSendMut},
        message::{MessageReader, MessageWriter},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Res, ResMut},
    },
    log::{info, warn},
};
use common::comp;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use specs::WorldExt;
use xindeler_dimensions::{
    DimensionActivated, DimensionId, DimensionLifecycle, DimensionRegistry, DimensionSpinupConfig,
    DimensionTornDown, DrainDimension, SpinupDimension,
};
use xindeler_oracle_host::{
    ChroniclePlugin, DimensionAtmospheres, DmEvent, DmEventPlugin, EntityTemplate,
    EntityTemplatePlugin, OracleEventManifest, OracleEventManifestPlugin,
};
use xindeler_protocol::NarrativeHooks;

use crate::{
    SimEntityDimension, SimServer,
    player_transfer::{PlayerDimensionSession, TransferPlayerDimension},
};

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

/// One event's player-transfer trigger zone (BL-82 EM-4.9 follow-up, closing
/// the "no live player-transfer trigger" gap): a circle in world-space XY,
/// sim axes, matching the SAME `origin`/`spawn_radius` [`spawn_event_minions`]
/// already scatters its minions around/within — see [`EventTransferZones`]'s
/// doc comment for why reusing that exact geometry (rather than inventing a
/// new `DmEvent` schema field) is the deliberate v1 choice.
#[derive(Debug, Clone, Copy)]
struct TransferZone {
    origin: vek::Vec2<f32>,
    radius: f32,
}

/// Every currently-`Active` event's [`TransferZone`], keyed by its
/// [`DimensionId`] (BL-82 EM-4.9 follow-up). Populated by
/// [`spawn_event_minions`] the moment it resolves a REAL origin (i.e. only once
/// a live [`SimServer`] and generated terrain make that origin meaningful — a
/// sim-less test producer run never registers a zone, so
/// [`detect_player_dimension_entry`] has nothing to act on there), and cleared
/// by [`cleanup_dimension_side_tables_on_teardown`] alongside the other
/// per-dimension side tables this module already prunes on teardown.
///
/// ## Why reuse `spawn_radius`, not a new schema field
/// `DmEvent.spawning_rules.spawn_radius` already IS "how far from this
/// event's origin its content extends" — the exact zone a player should
/// consider themselves to have "entered the event". Adding a SEPARATE
/// trigger-radius field would let an event author's minion-scatter radius and
/// player-entry radius drift apart for no expressive benefit v1 needs; the
/// shipped `mist_bound.dmevent.ron` fixture needs zero changes as a result.
/// A future event that genuinely wants a different entry radius than its
/// scatter radius can widen this to a dedicated field then — not a breaking
/// change, since this table's shape is internal to this module.
#[derive(Resource, Debug, Default)]
struct EventTransferZones(HashMap<DimensionId, TransferZone>);

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
    mut zones: ResMut<EventTransferZones>,
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
                // BL-82 EM-4.9 follow-up: register this event's player-transfer
                // trigger zone the SAME instant its origin becomes real (a
                // live sim + generated terrain) — see `EventTransferZones`'s
                // own doc comment for why this reuses `spawn_radius` verbatim
                // rather than a new schema field.
                zones.0.insert(dimension, TransferZone {
                    origin: centre,
                    radius: dm_event.spawning_rules.spawn_radius.max(0.0),
                });
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

/// The v1 player-transfer TRIGGER (BL-82 EM-4.9 follow-up — see this crate's
/// `player_transfer` module doc comment for why proximity was chosen over an
/// explicit narrative-hook command). Every tick, checks every tracked REAL
/// player's own sim position against every currently-`Active` event's
/// [`TransferZone`] and emits [`TransferPlayerDimension`] the instant a
/// `DimensionId::DEFAULT`-resident player enters one.
///
/// "Tracked player" here means a sim entity linked by a
/// [`PlayerDimensionSession`] — a real, logged-in replicon session
/// (`xindeler-server-app::login`'s own doc comment describes how that link is
/// populated). The listen-server's embedded local player is deliberately NOT
/// a candidate here: `ServerOraclePlugin` is never added to the listen-server
/// client (`DmEventPlugin`'s own doc comment: "only a server-side host is
/// meant to ever register [it]"), so this system never even runs there in
/// practice — but it is written to iterate ANY linked session generically
/// rather than hardcode a single-player assumption, in case a future
/// (dedicated-server, multi-session) run of `xindeler-client`'s embedded path
/// ever changes that.
///
/// ## One-way crossing (v1, deliberate)
/// Only players currently resident in `DimensionId::DEFAULT` are considered
/// for entry — a player already inside a (different) dimension is never
/// re-evaluated for ANOTHER proximity transfer by this system. Combined with
/// the `DimensionLifecycle::Active` check below, this also closes a subtler
/// race: without it, a player ejected by [`eject_players_before_dimension_
/// teardown`] back to `DEFAULT` while still standing inside a just-retired
/// event's (not yet fully torn down) zone would otherwise be pulled straight
/// back in on the very next tick.
fn detect_player_dimension_entry(
    sim: Option<NonSend<SimServer>>,
    registry: Res<DimensionRegistry>,
    zones: Res<EventTransferZones>,
    sessions: Query<&PlayerDimensionSession>,
    entity_dims: Res<SimEntityDimension>,
    mut transfer_writer: MessageWriter<TransferPlayerDimension>,
) {
    if zones.0.is_empty() {
        return; // cheap bail-out — no active event has a real zone yet
    }
    let Some(sim) = sim else { return };

    let ecs = sim.server.state().ecs();
    let positions = ecs.read_storage::<comp::Pos>();

    for session in &sessions {
        let sim_entity = session.0;
        let Some(pos) = positions.get(sim_entity) else {
            continue;
        };
        let current = entity_dims
            .0
            .get(&sim_entity)
            .copied()
            .unwrap_or(DimensionId::DEFAULT);
        if current != DimensionId::DEFAULT {
            continue; // one-way crossing — see this function's own doc comment
        }
        let xy = vek::Vec2::new(pos.0.x, pos.0.y);
        for (&dimension, zone) in &zones.0 {
            if registry.lifecycle(dimension) != Some(DimensionLifecycle::Active) {
                continue;
            }
            if (xy - zone.origin).magnitude_squared() <= zone.radius * zone.radius {
                transfer_writer.write(TransferPlayerDimension {
                    sim_entity,
                    target: dimension,
                });
                break; // one transfer per player per tick is enough
            }
        }
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
    mut zones: ResMut<EventTransferZones>,
) {
    for event in torn_down.read() {
        atmospheres.remove(event.id);
        hooks.unregister(event.id);
        // BL-82 EM-4.9 follow-up: same bounding rationale as the two tables
        // above — without this, `EventTransferZones` would grow by one stale
        // entry per retired event for the life of the process. A stale zone
        // is also more than just a leak: `detect_player_dimension_entry`'s
        // own `DimensionLifecycle::Active` re-check already guards against it
        // ever firing a transfer into a dead dimension, but removing the
        // entry here means the (bounded, cheap) proximity scan doesn't keep
        // paying for a permanently-dead zone forever either.
        zones.0.remove(&event.id);
    }
}

// The canonical, shipped events this drill's producer knows about by name
// are now DATA (BL-82 EM-4.9 follow-up, data-driven-content cleanup):
// `xindeler_oracle_host::OracleEventManifest`, loaded from
// `assets/xindeler/oracle_events/manifest.oracle_manifest.ron`
// (`xindeler_oracle_host::oracle_manifest::DEFAULT_MANIFEST_ASSET_PATH`).
// Before this cleanup the list was a compiled-in `&[&str]` constant —
// shipping a second canonical event required a Rust code change + recompile;
// see that module's own doc comment for the full rationale. A general "scan
// the whole oracle:// directory for any file" watcher remains a nicer v2
// (out of scope here — see `request_well_known_events_from_manifest`'s doc
// comment for why even an explicit per-file pre-request is required at all).
// Bare filenames (not `oracle://`-prefixed) — `request_well_known_events_
// from_manifest` builds the asset-server load path, `retire_dm_events`
// builds the on-disk path; both derive from the SAME manifest entries so
// they can never drift apart.

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

/// Holds the strong handle to the loaded [`OracleEventManifest`] (keeps it —
/// and its file watch, if `file_watcher` is active — alive for the App's
/// whole lifetime, same rationale [`WellKnownEventHandles`] documents for the
/// individual `DmEvent` handles it names) plus the set of filenames already
/// requested (so a manifest hot-reload that re-lists an already-requested
/// name is a no-op, not a duplicate `AssetServer::load` call).
#[derive(Resource)]
struct OracleEventManifestHandle {
    handle: Handle<OracleEventManifest>,
    requested_filenames: HashSet<String>,
}

/// Holds the [`Handle`]s [`request_well_known_events_from_manifest`]
/// requests, alive for the App's whole lifetime — without a surviving strong
/// handle, `Assets<DmEvent>` would unload the asset again the moment the
/// initial (possibly failed, if the file doesn't exist yet) load settles, and
/// no later file-watcher event would have anything to attach to.
#[derive(Resource, Default)]
struct WellKnownEventHandles(Vec<Handle<DmEvent>>);

/// `AssetId<DmEvent> -> filename` for every handle
/// [`request_well_known_events_from_manifest`] requested — lets
/// [`retire_dm_events`] map a registered `DmEvent` asset back to the on-disk
/// filename it must poll for existence.
#[derive(Resource, Default)]
struct WellKnownEventFilenames(HashMap<AssetId<DmEvent>, String>);

/// Requests the [`OracleEventManifest`] asset at `Startup` (whether or not the
/// file exists yet — same "request eagerly, tolerate a failed first attempt"
/// posture every RON-asset boot path in this codebase already follows, e.g.
/// `xindeler_dimensions::predictive_gc`'s `PredictiveGcConfigPlugin`).
fn request_oracle_event_manifest(
    asset_server: Res<AssetServer>,
    manifest_path: Res<OracleEventManifestPath>,
    mut commands: Commands,
) {
    commands.insert_resource(OracleEventManifestHandle {
        handle: asset_server.load(manifest_path.0.clone()),
        requested_filenames: HashSet::new(),
    });
}

/// Asset path (relative to the asset source root) the manifest is loaded
/// from — a resource (not a captured closure) so [`request_oracle_event_
/// manifest`] stays a plain function item like every other system in this
/// module, mirroring [`OracleEventsDir`]'s own "small config resource, not a
/// closure" convention.
#[derive(Resource, Debug, Clone)]
struct OracleEventManifestPath(String);

/// Once the [`OracleEventManifest`] asset loads (or hot-reloads with newly
/// added entries), requests an [`AssetServer`] handle for every NOT-yet-
/// requested `event_filenames` entry, whether or not that file exists yet.
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
/// ever fires, so [`ingest_dm_events`] never sees it. Requesting each
/// well-known path eagerly (the file usually doesn't exist yet, so this first
/// load is EXPECTED to fail quietly — same as the `dm_event.rs` test's own
/// "let the failed first attempt settle" step) means the file WRITE later is
/// what completes an already-outstanding request, which the watcher DOES
/// observe.
fn request_well_known_events_from_manifest(
    asset_server: Res<AssetServer>,
    manifest_assets: Res<Assets<OracleEventManifest>>,
    mut manifest_events: MessageReader<AssetEvent<OracleEventManifest>>,
    mut manifest_handle: Option<ResMut<OracleEventManifestHandle>>,
    mut handles: ResMut<WellKnownEventHandles>,
    mut filenames: ResMut<WellKnownEventFilenames>,
) {
    let Some(manifest_handle) = manifest_handle.as_mut() else {
        return; // `request_oracle_event_manifest` hasn't run yet this frame
    };
    let relevant = manifest_events.read().any(|event| {
        matches!(event, AssetEvent::Added { id } | AssetEvent::Modified { id }
            if *id == manifest_handle.handle.id())
    });
    if !relevant {
        return;
    }
    let Some(manifest) = manifest_assets.get(&manifest_handle.handle) else {
        return;
    };
    for filename in &manifest.event_filenames {
        if manifest_handle.requested_filenames.contains(filename) {
            continue;
        }
        manifest_handle.requested_filenames.insert(filename.clone());
        let handle: Handle<DmEvent> = asset_server.load(format!("oracle://{filename}"));
        filenames.0.insert(handle.id(), filename.clone());
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
    /// Asset path (relative to the asset source root) the well-known-event
    /// manifest is loaded from — see [`request_oracle_event_manifest`].
    /// Defaults to [`xindeler_oracle_host::oracle_manifest::
    /// DEFAULT_MANIFEST_ASSET_PATH`] (the shipped manifest) for any caller
    /// that doesn't need to override it, mirroring
    /// `xindeler_dimensions::predictive_gc::PredictiveGcConfigPlugin::
    /// config_path`'s own "overridable, sane default" convention.
    pub manifest_path: String,
}

impl Default for ServerOraclePlugin {
    fn default() -> Self {
        Self {
            events_dir: xindeler_oracle_host::default_events_dir(),
            manifest_path: xindeler_oracle_host::oracle_manifest::DEFAULT_MANIFEST_ASSET_PATH
                .to_owned(),
        }
    }
}

impl Plugin for ServerOraclePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            DmEventPlugin,
            EntityTemplatePlugin,
            ChroniclePlugin,
            OracleEventManifestPlugin,
        ));
        // BL-82 EM-4.9 follow-up: `detect_player_dimension_entry` (below)
        // emits `TransferPlayerDimension`, which needs
        // `crate::PlayerTransferPlugin`'s `add_message` registration to
        // exist first — guarded (mirrors `SimBridgePlugin`'s own
        // `is_plugin_added::<DimensionsPlugin>` check) so this plugin is
        // self-sufficient regardless of whether a caller already added
        // `PlayerTransferPlugin` separately (both `xindeler-server-app::
        // plugin::SimServerPlugin` and `xindeler-client::listen_server` do,
        // for the generic debug-spinup case — re-adding here would be
        // harmless either way since `add_plugins` on an already-added plugin
        // only panics for plugins that don't declare themselves idempotent,
        // and `is_plugin_added` avoids that entirely).
        if !app.is_plugin_added::<crate::PlayerTransferPlugin>() {
            app.add_plugins(crate::PlayerTransferPlugin);
        }
        app.init_resource::<NextDimensionId>();
        app.init_resource::<OracleEventRegistry>();
        app.init_resource::<EntityTemplateHandles>();
        app.init_resource::<WellKnownEventHandles>();
        app.init_resource::<WellKnownEventFilenames>();
        app.init_resource::<EventTransferZones>();
        app.insert_resource(OracleEventsDir(self.events_dir.clone()));
        app.insert_resource(OracleEventManifestPath(self.manifest_path.clone()));
        app.add_systems(bevy::app::Startup, request_oracle_event_manifest);
        app.add_systems(FixedUpdate, request_well_known_events_from_manifest);

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
        // BL-82 EM-4.9 follow-up: the proximity player-transfer trigger — see
        // `detect_player_dimension_entry`'s own doc comment. Must run AFTER
        // `spawn_event_minions` (which is what actually populates
        // `EventTransferZones`) and BEFORE `apply_player_dimension_transfers`
        // (see that function's own doc comment for why its ordering relative
        // to the teardown chain is load-bearing).
        app.add_systems(
            FixedUpdate,
            detect_player_dimension_entry
                .after(spawn_event_minions)
                .before(crate::player_transfer::apply_player_dimension_transfers),
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
            ..Default::default()
        });
        app.finish();
        app.update();

        // Drop the SHIPPED Mist-Bound fixture into the watched oracle dir.
        // `ServerOraclePlugin`'s own manifest→handle-request chain
        // (`request_oracle_event_manifest` at `Startup`, then
        // `request_well_known_events_from_manifest`) already requested this
        // exact well-known path a moment ago (see that function's doc
        // comment for why a pre-existing outstanding handle is required for
        // the watcher to notice this write at all) —
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

    /// BL-82 EM-4.9 follow-up (comprehensive-review Finding 3): pins the
    /// cross-crate invariant `xindeler_oracle_host::dm_event::bounds::
    /// SPAWN_RADIUS`'s own doc comment describes — its ceiling MUST stay
    /// comfortably under half the world size THIS crate's [`event_gen_opts`]
    /// actually spins up event dimensions with. Before this test the
    /// invariant was enforced ONLY by cross-referencing doc comments between
    /// two different crates, no compile-time or test-level assertion — and
    /// this EXACT invariant already broke silently once (an earlier
    /// `SPAWN_RADIUS` ceiling of 2000.0 exceeded the world size spun up for
    /// event dimensions, so `spawn_event_minions`'s terrain-readiness gate
    /// waited forever for chunks outside the generated range, and minions
    /// silently never spawned — see `dm_event::bounds::SPAWN_RADIUS`'s own
    /// doc comment for the full postmortem). A future edit to EITHER
    /// constant in isolation (tightening/loosening `event_gen_opts`'s
    /// `x_lg`/`y_lg`, or raising `SPAWN_RADIUS`'s ceiling) now fails CI
    /// instead of silently reintroducing that bug.
    #[test]
    fn spawn_radius_ceiling_stays_under_the_event_dimension_half_extent() {
        use xindeler_oracle_host::dm_event::bounds::SPAWN_RADIUS;

        // Mirrors `spawn_event_minions`'s own local `CHUNK_SIZE` constant
        // (32 blocks/chunk, `common::terrain::TERRAIN_CHUNK_BLOCKS_LG = 5` —
        // `1 << 5 == 32`) — duplicated as a literal here for the exact same
        // reason `spawn_event_minions` duplicates it rather than importing
        // `common`: this shell crate's only other source of that number is
        // the same local convention, so keeping this test's copy in the same
        // style keeps both readable side by side without a new dependency
        // edge just for one constant.
        const CHUNK_SIZE: f32 = 32.0;

        // Computes the SAME half-extent-from-centre `world::sim::WorldSim::
        // get_size` (`MapSizeLg::chunks()`, `(1 << x_lg, 1 << y_lg)` chunks)
        // yields for a REAL spinup of `opts` — without actually spinning up
        // a `WorldSim` (far too slow/heavy for a unit test), since
        // `event_gen_opts()` only returns the `GenOpts` shape, not a live
        // world. Mirrors `spawn_event_minions`'s own
        // `size * CHUNK_SIZE * 0.5` computation exactly.
        let half_extent_of = |lg: u32| (1u32 << lg) as f32 * CHUNK_SIZE / 2.0;

        let opts = event_gen_opts();
        // The tighter of the two axes is the real constraint — `spawn_event_
        // minions` scatters a circular radius around the centre, so BOTH
        // axes must comfortably fit it, not just one (today `x_lg == y_lg`,
        // but this stays correct if that ever changes).
        let world_half_extent = half_extent_of(opts.x_lg).min(half_extent_of(opts.y_lg));

        assert!(
            SPAWN_RADIUS.1 < world_half_extent,
            "dm_event::bounds::SPAWN_RADIUS.1 ({}) must stay strictly under event_gen_opts()'s \
             own world half-extent ({world_half_extent}) — otherwise spawn_event_minions's \
             terrain-readiness gate waits forever for chunks outside the generated range and \
             minions silently never spawn (this exact bug happened once before with a prior \
             2000.0 ceiling; see dm_event::bounds::SPAWN_RADIUS's doc comment)",
            SPAWN_RADIUS.1
        );
    }
}
