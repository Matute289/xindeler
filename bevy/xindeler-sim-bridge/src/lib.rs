//! SERVER-side bridge: embeds the specs sim (veloren server) and mirrors state
//! into replicated Bevy entities/messages. The only place both ECS worlds meet.
//!
//! BL-82 Bevy migration:
//! - EM-1.5: [`SimServer`] wraps the real `xindeler-server-core` `Server` (plus
//!   the `Arc<tokio::Runtime>` it requires), [`tick_sim`] drives `Server::tick`
//!   from the Bevy schedule.
//! - EM-3.6 (the terrain half): [`SimTerrainStreamPlugin`] anchors a
//!   server-side presence so the sim keeps chunks loaded around the world
//!   center ([`ensure_terrain_anchor`], via the sim's public
//!   `Server::create_centered_persister`), then [`stream_terrain_changes`]
//!   drains the sim's per-tick `State::terrain_changes()` and emits the
//!   [`CompressedChunk`]/[`RemoveChunk`] server messages the pure-Bevy client
//!   consumes. Runs on the SAME App as the client plugins in listen-server mode
//!   (replicon's local loopback, [Q3]=B).
//! - EM-3.7 (the entity half): [`SimEntityMirrorPlugin`] reads the sim's
//!   client-visible entities off the specs storages every tick
//!   ([`mirror_sim_entities`]) and UPSERTs a `Replicated` Bevy entity per sim
//!   entity carrying
//!   [`NetPos`]/[`NetOri`]/[`NetVel`]/[`NetBody`](+[`NetHealth`]); entities
//!   that vanish (died / left view) get their mirror despawned. The
//!   [`SimMirror`] resource keeps the sim↔Bevy identity map. It also spawns a
//!   handful of wandering test NPCs near the anchor ([`spawn_test_npcs`], via
//!   the public `event::CreateNpcEvent`) so there is something that moves to
//!   watch. Broadcast to all clients (`Replicated` default visibility); per-
//!   client interest management is EM-4.2d.
//!
//! - EM-3.7b (the controllable half): [`PlayerBridgePlugin`] boots an embedded
//!   `xindeler-client-core::Client` over TCP loopback that IS the local player
//!   (see [`player`]); [`player::tick_player`] applies the Bevy keyboard/mouse
//!   (via [`xindeler_protocol::LocalPlayerInput`]) to its `ControllerInputs`.
//!   The mirror ([`mirror_sim_entities`]) tags the player's replicated entity
//!   with [`NetLocalPlayer`] so the pure-Bevy client's third-person camera can
//!   follow it. The terrain persister anchor becomes a FALLBACK, spawned only
//!   if the embedded player never reaches in-game.
//!
//! - EM-4.2f (the identity + AURORA-readiness half): [`mirror_sim_entities`]
//!   additionally writes [`NetUid`] (the sim's `Uid` inner value) for every
//!   mirrored entity — additive, no change to any existing mirrored field.
//!   [`tick_aurora_overlay`] then keeps `xindeler_protocol::AuroraOverlay` in
//!   sync with [`AiExecutionMode`]: empty while `Offline` (today's default
//!   `server-agent` AI, unchanged), one neutral-default
//!   `xindeler_protocol::AuroraNpcState` entry per mirrored NPC (every
//!   `NetUid`-carrying entity that is NOT [`NetLocalPlayer`]) otherwise. See
//!   [`recompute_aurora_overlay`]'s doc comment for the invariant this must
//!   never violate.
//! - EM-4.2d (interest management): [`mirror_sim_entities`] additionally writes
//!   a [`xindeler_protocol::RegionKey`] (via
//!   `xindeler_protocol::region_key_for_pos`, from the sim's raw `Pos` — the
//!   SAME region grid `server/src/sys/subscription.rs` keys entities by) for
//!   every mirrored entity, deduped through [`SimRegionCache`] so it only
//!   re-inserts on an actual region crossing. This is what
//!   `xindeler-server-app`'s per-client `RegionKey`-visibility filter scopes
//!   entities by — replacing the "broadcast to all clients" v1 posture this
//!   module's own doc comment used to describe. Because every mirrored entity
//!   now carries a `RegionKey`, a connected client with no `ClientViewpoint`
//!   would otherwise see NOTHING (a real regression a reviewer caught against
//!   this crate's own `mirrors_sim_npc_to_replicon_client` test and
//!   `xindeler-server-app`'s `replicon_quinnet_dual_stack.rs`) — so
//!   [`SimTerrainStreamPlugin`] also gained
//!   [`apply_default_viewpoint_for_new_clients`], a documented stopgap granting
//!   a spectator-style default viewpoint (world-centre,
//!   [`ANCHOR_VIEW_DISTANCE`]) to any client that doesn't already have one,
//!   standing in for EM-4.2c's real login-derived viewpoint.
//! - EM-4.2b (a second host, no behavior change to the above): the
//!   [`SimBridgePlugin`]/[`SimTerrainStreamPlugin`]/[`SimEntityMirrorPlugin`]
//!   trio is now ALSO added by `xindeler-server-app`'s `SimServerPlugin` — the
//!   dedicated-server shell that owns the real remote
//!   `bevy_replicon`/`xindeler-transport` connection. [`boot_with_settings`]
//!   (factored out of [`boot_test_server`]) lets that shell boot a
//!   [`SimServer`] from the REAL production `Settings`/`EditableSettings` it
//!   reads (`server::Settings::load`), instead of the singleplayer shortcut
//!   this crate's own `boot_test_server` uses for the listen-server path —
//!   `worker_threads`/`thread_name_prefix` are caller-supplied precisely so
//!   this extraction does NOT silently downgrade that shell's real CPU-scaled
//!   tokio runtime sizing to the singleplayer path's small fixed one (see
//!   [`boot_with_settings`]'s own doc comment for the regression this closes).
//!   `xindeler-server-app` does NOT add
//!   [`PlayerBridgePlugin`]/[`LodAltStreamPlugin`] (no embedded local player;
//!   EM-4.2c/login is out of scope there) — the terrain-anchor persister
//!   fallback and the wandering test NPCs cover the acceptance test's "at least
//!   one replicated entity + one terrain chunk" bar on their own, same as they
//!   already do for the listen-server's own spectator fallback path.
//!
//! Isolation law: logic crates never depend on this crate or on Bevy; the
//! bridge only calls the sim's public API. This crate and
//! `xindeler-server-app::login` (BL-82 EM-4.2c) are the two legal `specs`
//! consumers under `bevy/` — the client stays pure. `login` is a sanctioned
//! exception (phase-4 plan §1.2: the replicon login handshake lives in the
//! server-app shell and touches the sim's `ecs()` directly to create the
//! login-session entity and set `Presence`/`PresenceKind`, calling the SAME
//! public entry points — `LoginProvider::verify`/`login_with_ip`,
//! `CharacterLoader`, `StateExt` — the legacy path uses); it does not
//! reimplement or bypass this crate's mirroring, so the "bridge is the only
//! writer into the sim from Bevy" invariant this crate itself upholds is
//! unaffected. See
//! `docs/design/specs/2026-07-10-bl82-wave3-regression-fixes-design.md` Finding
//! F for why this comment needed correcting.

mod entity_factory;
pub use entity_factory::{
    PendingDimensionAttribution, apply_pending_entity_template_spawns, spawn_from_spawning_rules,
};

mod oracle;
pub use oracle::ServerOraclePlugin;

mod player;
pub use player::{EmbeddedPlayer, PlayerBridgePlugin, boot_embedded_player, tick_player};

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bevy::{
    app::{App, FixedUpdate, Plugin, Update},
    ecs::{
        change_detection::NonSendMut,
        component::Component,
        entity::Entity,
        message::MessageWriter,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res},
    },
    math::{Quat, Vec3},
    state::condition::in_state,
    time::Time,
};
use bevy_replicon::prelude::{ClientState, Replicated, SendTargets, ToClients};
use common::{
    comp,
    comp::inventory::{
        item::{ItemDefinitionId, ItemKind, modular},
        slot::{ArmorSlot, EquipSlot},
    },
    event::{CreateNpcEvent, NpcBuilder},
    uid::Uid,
};
use server::{
    EditableSettings, Event, Input, Server, Settings,
    persistence::{DatabaseSettings, SqlLogMode},
    state_ext::StateExt as _,
};
use specs::{LendJoin, WorldExt};
use xindeler_dimensions::{
    DimensionId, DimensionLifecycle, DimensionRegistry, DimensionRoot, DimensionState,
    DimensionsPlugin,
};
use xindeler_protocol::{
    AiExecutionMode, AuroraOverlay, CompressedChunk, NetBody, NetHealth, NetLoadout,
    NetLocalPlayer, NetLodAlt, NetOri, NetPos, NetTool, NetToolKey, NetUid, NetVel, RegionKey,
    RemoveChunk, TerrainAnchor, region_key_for_pos,
};

/// The embedded specs simulation: the authoritative `Server` plus the tokio
/// runtime `Server::new` requires (mirrors server-cli's setup).
///
/// Stored via Bevy 0.19's **non-send** mechanism (`App::insert_non_send` +
/// [`NonSendMut`]), NOT as a `Resource`: `Server` is `Send` but not `Sync`
/// (its specs `SendDispatcher` boxes `dyn RunNow + Send` stages without a
/// `Sync` bound), and Bevy resources require `Send + Sync`. Non-send storage
/// also pins [`tick_sim`] to the main thread — matching how server-cli ticks
/// the sim from its main loop.
///
/// Not inserted by [`SimBridgePlugin`] itself: booting a world is slow and
/// asset-dependent, so the shell decides when/how to construct one (e.g. via
/// [`boot_test_server`]) and inserts it; [`tick_sim`] no-ops until then.
pub struct SimServer {
    /// The authoritative veloren/xindeler simulation.
    pub server: Server,
    /// Runtime backing the sim's async work (networking, persistence).
    /// Kept alive here for the lifetime of the sim.
    pub runtime: Arc<tokio::runtime::Runtime>,
    /// Number of successful [`tick_sim`] passes since boot.
    pub ticks: u64,
    /// Terrain changes snapshotted at the END of the last [`tick_sim`], BEFORE
    /// the sim's `cleanup()` clears them (EM-3.6). [`stream_terrain_changes`]
    /// drains this the same frame. Keys are `[x, y]` (`TerrainGrid`
    /// convention).
    pending_terrain: PendingTerrain,
}

/// Keys of terrain that changed in one sim tick, split by kind. Populated by
/// [`tick_sim`] (before `cleanup`), consumed by [`stream_terrain_changes`].
#[derive(Default)]
struct PendingTerrain {
    /// New or modified chunks — the client (re)meshes these.
    upserted: Vec<[i32; 2]>,
    /// Unloaded chunks — the client drops these.
    removed: Vec<[i32; 2]>,
}

/// Marks a Bevy entity as the mirror of a sim (specs) entity (EM-3.7). Kept on
/// the replicated Bevy entity so [`mirror_sim_entities`] can reconcile the two
/// worlds; NOT replicated itself (a `specs::Entity` is meaningless on the
/// client, which is pure Bevy).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimEntity(pub specs::Entity);

/// sim (specs) entity → mirrored Bevy entity lookup, maintained by
/// [`mirror_sim_entities`] (EM-3.7). One entry per currently-mirrored sim
/// entity; pruned when a sim entity disappears.
#[derive(Resource, Default, Debug)]
pub struct SimMirror(pub HashMap<specs::Entity, Entity>);

/// Last-mirrored [`NetLoadout`] per sim entity (EM-3.8d). The loadout is a
/// handful of `String`s, so — unlike the `Copy` position/body comps that the
/// mirror re-inserts every tick — we only re-insert (and thus re-replicate) it
/// when the equipped gear actually CHANGES. This cache holds the last value we
/// sent; entries are pruned alongside [`SimMirror`] when an entity disappears.
#[derive(Resource, Default, Debug)]
pub struct SimLoadoutCache(pub HashMap<specs::Entity, NetLoadout>);

/// Last-mirrored [`RegionKey`] per sim entity (EM-4.2d), dedup cache mirroring
/// [`SimLoadoutCache`]'s own shape: `RegionKey` is `Copy`/cheap like the
/// position/body comps, but re-inserting it every tick (even unchanged) would
/// re-trigger a full replicon `VisibilityFilter` re-evaluation of this entity
/// against every connected client on every tick, not just on an actual region
/// crossing. Entries are pruned alongside [`SimMirror`]/[`SimLoadoutCache`]
/// when an entity disappears.
#[derive(Resource, Default, Debug)]
pub struct SimRegionCache(pub HashMap<specs::Entity, RegionKey>);

/// The [`DimensionId`] each currently-mirrored sim entity was assigned WHEN
/// FIRST SEEN (BL-82 EM-4.9, Phase C / T51.6) — decided once, in
/// [`mirror_sim_entities`], and never revisited afterward (there is no
/// player/NPC dimension-TRANSFER path yet; a mirror keeps the dimension it
/// was created with for its whole lifetime). Defaults every entity to
/// [`DimensionId::DEFAULT`] unless [`PendingDimensionAttribution`] had a
/// pending non-default assignment waiting for it — see that resource's own
/// doc comment for the full correlation mechanism and its documented limits.
/// Entries are pruned alongside [`SimMirror`]/[`SimLoadoutCache`]/
/// [`SimRegionCache`] when an entity disappears.
#[derive(Resource, Default, Debug)]
pub struct SimEntityDimension(pub HashMap<specs::Entity, DimensionId>);

/// EM-4.10 Finding C: reused scratch buffers for [`mirror_sim_entities`],
/// matching [`SimLoadoutCache`]/[`SimRegionCache`]'s "one resource, cleared
/// not reallocated" pattern. The mirror loop used to allocate fresh `seen`/
/// `updates` `Vec`s (and a fresh `seen_set` `HashSet`) every tick, sized to
/// the mirrored-entity count — an explicit `TODO(EM-4.2d)` acknowledged this
/// was "fine at test-NPC scale", but Wave-3's batch-spawning
/// `spawn_from_spawning_rules` (`entity_factory.rs`) invalidates that
/// assumption: a burst of several dozen NPCs spawning at once now pays this
/// allocation cost every single `FixedUpdate` tick, compounding with the
/// Finding-A/B fixes' remaining cost inside a catch-up burst. `.clear()`ed
/// at the top of each tick instead of freshly allocated; behavior identical.
#[derive(Resource, Default)]
struct MirrorScratch {
    /// Sim entities visible this tick (see [`mirror_sim_entities`]).
    seen: Vec<specs::Entity>,
    /// (sim_entity, components) collected before issuing commands (see
    /// [`mirror_sim_entities`]).
    #[allow(clippy::type_complexity)]
    updates: Vec<(
        specs::Entity,
        NetPos,
        NetOri,
        NetVel,
        NetBody,
        Option<NetHealth>,
        Option<NetLoadout>,
        Option<NetUid>,
        RegionKey,
        // BL-82 EM-4.9: the dimension this entity was assigned (see
        // `SimEntityDimension`'s doc comment) — DEFAULT for every entity
        // today except a factory batch routed at a real event dimension.
        DimensionId,
    )>,
    /// `seen` collapsed into a set for the stale-mirror sweep (see
    /// [`mirror_sim_entities`]).
    seen_set: std::collections::HashSet<specs::Entity>,
}

/// EM-4.10 Finding C: reused scratch buffer for [`recompute_aurora_overlay`],
/// mirroring [`MirrorScratch`]'s reasoning — `tick_aurora_overlay` used to
/// build a fresh `HashSet<u64>` every tick to compute the live-NPC set
/// before pruning `AuroraOverlay`.
#[derive(Resource, Default)]
struct AuroraScratch {
    /// This tick's live NPC uids (see [`recompute_aurora_overlay`]).
    live: std::collections::HashSet<u64>,
}

/// Advances the embedded sim by one tick using the schedule's `dt`, then
/// drains the sim's frontend events and errors into `tracing`.
///
/// Runs in `FixedUpdate` at [`SIM_TICK_HZ`] (EM-3.11b — see below); no-ops
/// until the shell inserts a [`SimServer`] (there is no `resource_exists`
/// equivalent for non-send data, so the gate is the `Option` param).
///
/// ## EM-3.11b: FixedUpdate, not Update
/// This system (plus its `.after(tick_sim)` chain: `ensure_terrain_anchor`,
/// `stream_terrain_changes`, `spawn_test_npcs`, `mirror_sim_entities`, and
/// [`crate::tick_player`]) used to run in `Update`. In the headless
/// `xindeler-server-app` shell that's fine — `ScheduleRunnerPlugin::run_loop`
/// paces the WHOLE App at 30 TPS, so `Update` only fires 30×/s. But in the
/// WINDOWED listen-server path (`xindeler-client --listen-server`) `Update`
/// fires at display rate (60–144 Hz on the dev machines that hit this), so a
/// full `Server::tick` — the entire specs system graph: physics, agent AI,
/// terrain streaming, economy, etc. — ran 2–5× more often than the sim (and
/// the embedded player's `Clock`) was ever designed for. Two real-user
/// symptoms traced back to this:
/// - **Flat ~29 fps unmoved by a release+LTO rebuild** (see
///   `xindeler-client::camera::OcclusionCullingConfig`'s EM-3.10b doc for the
///   original dev-build measurement, and EM-3.11b's release-build follow-up in
///   `docs/backlog/engine-migration.md`): a CPU speedup doesn't move a frame
///   time dominated by 2–5× too much full-server-tick work.
/// - **Visible judder / "robotic" movement**: variable-`dt` physics/character
///   integration run once per (variable-length) render frame is a classic
///   jitter source — very different motion quality than the fixed 1/30 s steps
///   the sim's own systems (and voxygen's, historically) assume.
///
/// `FixedUpdate` decouples sim cadence from render cadence: `Time` inside a
/// `FixedUpdate` system is Bevy's fixed-clock view (a clean `1 / SIM_TICK_HZ`
/// every step, however many steps a given render frame does — 0, 1, or a
/// short catch-up burst), so `tick_sim`'s `dt` is finally what the sim
/// expects regardless of display refresh rate.
pub fn tick_sim(time: Res<Time>, sim: Option<NonSendMut<SimServer>>) {
    let Some(mut sim) = sim else { return };
    let dt = time.delta();
    let events = match sim.server.tick(Input::default(), dt) {
        Ok(events) => events,
        Err(err) => {
            tracing::error!(?err, "sim server tick failed");
            return;
        },
    };
    for event in events {
        match event {
            Event::ClientConnected { .. } => tracing::info!("sim: client connected"),
            Event::ClientDisconnected { .. } => tracing::info!("sim: client disconnected"),
            Event::Chat { msg, .. } => tracing::info!("sim chat: {msg}"),
        }
    }

    // EM-3.6: snapshot this tick's terrain changes BEFORE `cleanup()` clears
    // them. We copy just the keys (cheap: a handful of Vec2<i32> per tick);
    // `stream_terrain_changes` serializes the actual chunks from the grid the
    // same frame. `new` ∪ `modified` collapse to one "upsert" set for the
    // client (it (re)meshes both identically); `removed` is kept separate.
    {
        let changes = sim.server.state().terrain_changes();
        let mut upserted: Vec<[i32; 2]> =
            Vec::with_capacity(changes.new_chunks.len() + changes.modified_chunks.len());
        upserted.extend(changes.new_chunks.iter().map(|k| [k.x, k.y]));
        upserted.extend(changes.modified_chunks.iter().map(|k| [k.x, k.y]));
        let removed: Vec<[i32; 2]> = changes.removed_chunks.iter().map(|k| [k.x, k.y]).collect();
        drop(changes);
        // Accumulate across frames in case `stream_terrain_changes` hasn't run
        // yet (it runs the same Update, but be robust to scheduling): append
        // rather than overwrite.
        sim.pending_terrain.upserted.extend(upserted);
        sim.pending_terrain.removed.extend(removed);
    }

    // Like server-cli's loop: clean up after every tick (clears TerrainChanges).
    sim.server.cleanup();
    sim.ticks += 1;
}

/// The sim's fixed tick rate (30 TPS — matches `xindeler-server-app::sim::
/// SIM_TICK_INTERVAL` and server-cli's `TPS` const, and `player::PLAYER_TPS`
/// the embedded player's `Clock` already assumed). The windowed listen-server
/// shell configures `Time::<Fixed>::from_hz(SIM_TICK_HZ)` (EM-3.11b) so
/// [`tick_sim`] (and [`crate::tick_player`], moved to the same schedule)
/// finally run at the rate the sim was designed for instead of display rate.
pub const SIM_TICK_HZ: f64 = 30.0;

/// Registers the bridge types and the [`tick_sim`] system.
///
/// Deliberately does NOT boot the sim: the shell (or a test) constructs a
/// [`SimServer`] — e.g. with [`boot_test_server`] — and inserts it whenever
/// it's ready; until then [`tick_sim`] simply doesn't run.
pub struct SimBridgePlugin;

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimMirror>();
        // BL-82 EM-4.9 (Phase C): the factory-spawn <-> mirror dimension
        // correlation resources — see `PendingDimensionAttribution`'s and
        // `SimEntityDimension`'s own doc comments. Initialized here (rather
        // than in `SimEntityMirrorPlugin`) since `apply_pending_entity_
        // template_spawns` (this plugin's own chain, below) is the FIRST of
        // the two systems that touches either resource; `init_resource` is
        // idempotent, so `SimEntityMirrorPlugin` initializing them again
        // later is harmless.
        app.init_resource::<entity_factory::PendingDimensionAttribution>();
        app.init_resource::<SimEntityDimension>();
        // EM-4.5: DimensionRegistry + the full lifecycle state machine —
        // every dimension-tagging consumer of this bridge needs it, so it's
        // wired here rather than as an optional add-on plugin (unlike
        // `SimTerrainStreamPlugin`/`SimEntityMirrorPlugin`, which really are
        // optional pieces of the bridge). Guarded: `xindeler-server-app`'s
        // `SimServerPlugin` already adds `DimensionsPlugin` explicitly and
        // EARLY (before this plugin), so its own dimension-setup calls
        // (`install_default_dimension`/`init_debug_state`) can run against
        // an already-initialized `DimensionRegistry` — re-adding it here
        // unconditionally would panic ("plugin was already added"). The
        // listen-server client path (`xindeler-client::ListenServerPlugin`)
        // does NOT pre-add it, so this guarded add is what actually
        // registers it for that path.
        if !app.is_plugin_added::<DimensionsPlugin>() {
            app.add_plugins(DimensionsPlugin);
        }
        app.init_resource::<DefaultDimensionState>();
        // EM-3.11b: FixedUpdate, not Update — see `tick_sim`'s doc for why a
        // display-rate `Update` tick was the wrong home for this.
        // EM-4.7: `apply_pending_entity_template_spawns` resolves any
        // factory-staged spawn requests through the sim's public event bus —
        // see `entity_factory`'s module doc for why a future producer system
        // (EM-4.9) needs an explicit ordering edge against this one.
        app.add_systems(
            FixedUpdate,
            (
                tick_sim,
                ensure_default_dimension,
                apply_pending_entity_template_spawns,
            )
                .chain(),
        );
    }
}

/// One-shot latch for wrapping [`DimensionId::DEFAULT`] around the sim's
/// ALREADY-generated `Arc<World>`/`IndexOwned` (EM-4.5, spec §1.8: "a
/// wrapping refactor of already-existing state, not a behavior change")
/// once [`SimServer`] exists. Mirrors [`TerrainAnchorState`]'s "
/// `SimBridgePlugin` doesn't boot the sim, so wait for it" latch pattern —
/// see [`ensure_default_dimension`].
#[derive(Resource, Default)]
struct DefaultDimensionState {
    wrapped: bool,
}

/// Wraps [`DimensionId::DEFAULT`] the first tick a [`SimServer`] exists; a
/// no-op on every call before and after that (before: no sim yet; after:
/// [`DefaultDimensionState::wrapped`] latch). Runs `.after(tick_sim)` so the
/// sim has definitely finished constructing its world/index by the time this
/// reads them (in practice they're ready the instant `Server::new` returns,
/// well before the first tick, but chaining after `tick_sim` keeps the
/// ordering guarantee explicit rather than relying on that timing detail).
fn ensure_default_dimension(
    sim: Option<NonSendMut<SimServer>>,
    mut state: bevy::ecs::system::ResMut<DefaultDimensionState>,
    mut registry: bevy::ecs::system::ResMut<DimensionRegistry>,
    mut commands: Commands,
) {
    if state.wrapped {
        return;
    }
    // `xindeler-server-app`'s `SimServerPlugin` calls its own
    // `install_default_dimension` directly at `Plugin::build` time (before
    // this system ever runs), wrapping `DimensionId::DEFAULT` into the
    // registry synchronously — this system's own `state.wrapped` latch (a
    // separate resource, private to this crate) has no way to observe that.
    // Without this check, this system would retry+error EVERY tick forever
    // in that shell (the registry already has DEFAULT, so the wrap below
    // always fails `AlreadyExists`, and `state.wrapped` never flips because
    // that only happens in the `Ok` arm). Checking the registry itself —
    // the actual source of truth — instead of trusting a possibly-stale
    // flag closes that gap for any caller, not just this one.
    if registry.get(DimensionId::DEFAULT).is_some() {
        state.wrapped = true;
        return;
    }
    let Some(sim) = sim else { return };

    // The actual "register + complete spinup" sequence is shared with
    // `xindeler-server-app`'s own `install_default_dimension` (see
    // `xindeler_dimensions::wrap_default_dimension`'s doc comment) — only
    // the root-entity-spawning mechanism differs (`Commands` here, a system
    // context, vs. `app.world_mut()` there, a `Plugin::build` context).
    let root = commands.spawn(DimensionId::DEFAULT).id();
    match xindeler_dimensions::wrap_default_dimension(&mut registry, root, &sim.server) {
        Ok(()) => {
            state.wrapped = true;
            tracing::info!("dimension 0 (default) wrapped: Spinup -> Active");
        },
        Err(err) => {
            tracing::error!(?err, "failed to wrap the default dimension");
            commands.entity(root).despawn();
        },
    }
}

// ---------------------------------------------------------------------------
// EM-3.6 — terrain streaming (sim `TerrainChanges` → replicon server messages)
// ---------------------------------------------------------------------------

/// Terrain view distance (in chunks) the server-side anchor keeps loaded.
///
/// Small on purpose for the listen-server proof: a real world is huge and the
/// TERRAIN broadcast still has no interest management (EM-4.2d only wired
/// per-client ENTITY visibility scoping, via `RegionKey` — see
/// [`stream_terrain_changes`]'s own doc comment for why chunk broadcast is a
/// separate, still-open follow-up), so a wide anchor would stream thousands
/// of chunks to a single local client. `MIN_VD` is the sim's own minimum,
/// matching what a freshly-spawned player loads.
pub const ANCHOR_VIEW_DISTANCE: u32 = server::MIN_VD;

/// One-shot latch for the server-side presence anchor + broadcast state.
///
/// The sim only generates/streams chunks around entities that hold a
/// `Presence` (see `server/src/sys/terrain.rs`). Without a real client there is
/// no such entity, so [`ensure_terrain_anchor`] creates a spectator persister
/// once the sim is booted (public API `Server::create_centered_persister`,
/// documented "useful for testing without a client").
#[derive(Resource, Default)]
pub struct TerrainAnchorState {
    /// Whether the centered persister has been spawned yet.
    anchored: bool,
    /// World position (sim axes) of the anchor, once known — broadcast to
    /// clients as the [`TerrainAnchor`] message so they can park the camera.
    anchor_wpos: Option<[f32; 3]>,
    /// Whether the anchor message has been broadcast yet (v1: once).
    anchor_sent: bool,
}

impl TerrainAnchorState {
    /// The world-centre anchor position (sim axes), once known. `None` before
    /// [`ensure_terrain_anchor`] has computed it. EM-4.2d's
    /// [`apply_default_viewpoint_for_new_clients`] reads this to give a newly
    /// connected client a sane default [`xindeler_protocol::ClientViewpoint`].
    #[must_use]
    pub fn anchor_wpos(&self) -> Option<[f32; 3]> { self.anchor_wpos }
}

/// Registers [`TerrainAnchorState`] + the EM-3.6 terrain-stream systems. The
/// systems run only while the App is acting as the terrain source — i.e. NOT a
/// connected client (`ClientState::Disconnected`, which is true for the
/// listen server and singleplayer, false for a pure remote client). This is
/// the same gate replicon uses for its own server-authoritative logic
/// (`server/message.rs`, `send_locally` is
/// `in_state(ClientState::Disconnected)`).
///
/// Add this AFTER [`SimBridgePlugin`] (it reads the same [`SimServer`]).
pub struct SimTerrainStreamPlugin;

impl Plugin for SimTerrainStreamPlugin {
    fn build(&self, app: &mut App) {
        // EM-3.11b: FixedUpdate alongside `tick_sim` — see its doc. Chaining
        // `.after(tick_sim)` requires both to live in the same schedule.
        app.init_resource::<TerrainAnchorState>().add_systems(
            FixedUpdate,
            (
                ensure_terrain_anchor,
                apply_default_viewpoint_for_new_clients,
                stream_terrain_changes,
            )
                .chain()
                .after(tick_sim)
                .run_if(in_state(ClientState::Disconnected)),
        );
    }
}

/// BL-82 EM-4.2d stopgap: grants every newly-connected replicon client
/// (`ConnectedClient`, no `ClientViewpoint` yet) a DEFAULT viewpoint centered
/// on the terrain anchor with a generous ([`ANCHOR_VIEW_DISTANCE`]) view
/// distance, the moment a real anchor position is known.
///
/// ## Why this exists
/// Since EM-4.2d wired `RegionKey`-based visibility scoping, EVERY mirrored
/// entity now carries a `RegionKey`, and a connected client with NO
/// `ClientVisibleRegions` sees NOTHING (see
/// `xindeler_protocol::visibility::ClientVisibleRegions`'s own doc comment).
/// `ClientViewpoint`'s doc comment already says nothing populates it
/// automatically — EM-4.2c's login system is the natural REAL producer,
/// keying a viewpoint off the connecting client's own player entity. But
/// EM-4.2c has not landed yet, and without SOME stopgap, EVERY currently
/// existing acceptance path that connects a plain client with no login at
/// all (this crate's own `mirrors_sim_npc_to_replicon_client` test,
/// `xindeler-server-app`'s `tests/replicon_quinnet_dual_stack.rs`, the
/// `--listen-server`/net-client smoke paths) would silently regress to
/// "the client sees zero entities forever" — a real, reviewer-caught
/// regression risk (BL-82 EM-4.2d review), not a hypothetical one.
///
/// This system is the minimal, honest interim producer: a spectator-style
/// default (world-centre position, `ANCHOR_VIEW_DISTANCE` — the SAME
/// distance the terrain-anchor persister/embedded player already use),
/// exactly mirroring the terrain anchor's own "no real player yet → fall
/// back to a sane spectator default" posture ([`ensure_terrain_anchor`]).
/// **It never touches a client that already has a `ClientViewpoint`** — once
/// EM-4.2c inserts a real, player-position-derived one (or a test sets one
/// directly, as `xindeler-server-app`'s own `tests/interest_management.rs`
/// does), this system leaves it alone permanently for that client.
fn apply_default_viewpoint_for_new_clients(
    anchor: bevy::ecs::system::Res<TerrainAnchorState>,
    clients: bevy::ecs::system::Query<
        Entity,
        (
            bevy::ecs::query::With<bevy_replicon::prelude::ConnectedClient>,
            bevy::ecs::query::Without<xindeler_protocol::ClientViewpoint>,
        ),
    >,
    mut commands: Commands,
) {
    let Some(wpos) = anchor.anchor_wpos() else {
        return;
    };
    for client in &clients {
        commands
            .entity(client)
            .insert(xindeler_protocol::ClientViewpoint::new(
                DimensionId::default(),
                vek::Vec2::new(wpos[0], wpos[1]),
                ANCHOR_VIEW_DISTANCE,
            ));
    }
}

/// Spawns the server-side presence FALLBACK once the sim is booted (only when
/// no embedded player covers terrain), then broadcasts the world-centre anchor
/// position once for the client's initial camera placement.
///
/// ## Persister vs embedded player (EM-3.6 → EM-3.7b)
/// EM-3.6 used `Server::create_centered_persister` — a networking-free
/// `Presence` spectator — as the anchor that keeps chunks loaded. EM-3.7b adds
/// a real embedded `xindeler-client-core::Client` (see [`player`]) that is the
/// controllable local player; it ALSO holds a `Presence`, so it keeps chunks
/// loaded on its own and SUBSUMES the persister's job. The persister therefore
/// becomes a FALLBACK here: it is spawned only when there is no embedded player
/// or the player failed to connect (pure-spectator mode), so terrain still
/// streams and the world is visible. Everything stays inside the sim's public
/// API (persister) / the bridge's own embedded-client module, so the pure Bevy
/// client is unaffected.
fn ensure_terrain_anchor(
    sim: Option<NonSendMut<SimServer>>,
    // EM-3.7b: the embedded local player, if any. When a player is present the
    // persister is a FALLBACK — the player's own `Presence` keeps chunks
    // loaded, so we only spawn the persister if there is no player OR the
    // player failed to reach in-game. While the player is still connecting we
    // WAIT (don't spawn the persister), so we never end up with two anchors.
    player: Option<bevy::ecs::change_detection::NonSend<EmbeddedPlayer>>,
    mut anchor: bevy::ecs::system::ResMut<TerrainAnchorState>,
    mut anchor_writer: MessageWriter<ToClients<TerrainAnchor>>,
) {
    let Some(mut sim) = sim else { return };

    // Does the embedded player cover terrain streaming on its own? An
    // in-game player holds a `Presence` (loads chunks); a player that is still
    // connecting WILL, so we also count it as covering (and simply wait rather
    // than double-anchor). Only a missing OR failed player leaves terrain
    // uncovered → the persister fallback.
    let player_covers_terrain = player.as_ref().is_some_and(|p| !p.is_failed());

    if !anchor.anchored && !player_covers_terrain {
        // FALLBACK: no controllable player is keeping chunks loaded, so spawn
        // the spectator persister (EM-3.6 path). `create_centered_persister` is
        // `#[cfg(feature = "worldgen")]`; the bridge always links `server` with
        // worldgen on, so it is always available.
        sim.server.create_centered_persister(ANCHOR_VIEW_DISTANCE);
        anchor.anchored = true;
        tracing::info!("no embedded player covering terrain; spawned persister fallback");
    }

    // Broadcast the world-centre anchor position ONCE, as soon as we can read a
    // sensible altitude, regardless of which presence is loading chunks. The
    // client parks its spectator camera here until it identifies the player
    // entity (which it then follows in third person — EM-3.7b). Compute it only
    // when we're about to send (cheap, but avoid every-frame work).
    if !anchor.anchor_sent {
        let sim_ref = &sim.server;
        let size_chunks = sim_ref.world().sim().get_size();
        // Mirrors `common::terrain::TerrainChunkSize::RECT_SIZE`
        // (`1 << TERRAIN_CHUNK_BLOCKS_LG` = 32). Kept as a literal because this
        // crate depends on `server`, not `common`, for terrain constants (the
        // client-side terrain_stream, which does depend on common, derives it).
        let chunk_sz = vek::Vec2::new(32.0_f32, 32.0);
        let center_xy = vek::Vec2::new(size_chunks.x as f32, size_chunks.y as f32) * chunk_sz * 0.5;
        let alt = sim_ref
            .world()
            .sim()
            .get_alt_approx(center_xy.map(|e| e as i32))
            .unwrap_or(0.0);
        let wpos = [center_xy.x, center_xy.y, alt];
        anchor.anchor_wpos = Some(wpos);
        anchor_writer.write(ToClients {
            targets: SendTargets::All,
            message: TerrainAnchor { wpos },
        });
        anchor.anchor_sent = true;
        tracing::info!(?wpos, "terrain anchor position broadcast");
    }
}

/// Drains the sim's per-tick `State::terrain_changes()` and broadcasts each
/// change as a replicon server message on the Terrain lane:
/// - `new_chunks` ∪ `modified_chunks` → [`CompressedChunk`] (serialize the
///   `Arc<TerrainChunk>` currently in the grid, lz4+bincode via the protocol
///   codec),
/// - `removed_chunks` → [`RemoveChunk`].
///
/// v1 broadcasts to ALL clients (`SendTargets::All`, which also re-emits
/// locally for the listen server). TODO (still open past EM-4.2d): per-client
/// terrain interest management — only send a chunk to clients whose presence
/// covers it. EM-4.2d (BL-82 T47.6) scoped ENTITY visibility via a
/// `bevy_replicon` `VisibilityFilter` (`RegionKey`/`ClientVisibleRegions`,
/// `xindeler_protocol::visibility`); `CompressedChunk`/`RemoveChunk` are
/// one-shot MESSAGES, not replicated components, so that same
/// entity-visibility mechanism does not apply to them — scoping the terrain
/// broadcast needs its own (structurally different) per-client filter and
/// remains a known, tracked gap, not silently fixed by this comment's mere
/// existence.
///
/// The changes themselves were snapshotted (keys only) inside [`tick_sim`],
/// BEFORE the sim's `cleanup()` cleared its `TerrainChanges` resource; here we
/// drain that snapshot and serialize the chunks still in the grid. Runs
/// `.after(tick_sim)` in the same `Update`.
fn stream_terrain_changes(
    sim: Option<NonSendMut<SimServer>>,
    mut chunk_writer: MessageWriter<ToClients<CompressedChunk>>,
    mut remove_writer: MessageWriter<ToClients<RemoveChunk>>,
) {
    let Some(mut sim) = sim else { return };
    let pending = core::mem::take(&mut sim.pending_terrain);
    if pending.upserted.is_empty() && pending.removed.is_empty() {
        return;
    }

    for key in pending.upserted {
        // The chunk may have been removed again after being upserted this
        // batch; serialize only what's still in the grid.
        let chunk = sim
            .server
            .state()
            .terrain()
            .get_key_arc(vek::Vec2::new(key[0], key[1]))
            .cloned();
        if let Some(chunk) = chunk {
            chunk_writer.write(ToClients {
                targets: SendTargets::All,
                message: CompressedChunk::encode(key, &chunk),
            });
        }
    }
    for key in pending.removed {
        remove_writer.write(ToClients {
            targets: SendTargets::All,
            message: RemoveChunk { key },
        });
    }
}

// ---------------------------------------------------------------------------
// EM-3.10b — far-terrain heightmap (one-shot lod_alt broadcast)
// ---------------------------------------------------------------------------

/// Downsample cap: the client far-mesh is a coarse LOD proxy, not full-res
/// terrain, so the sent grid is bounded to at most this many samples per axis
/// regardless of world size. A default Veloren world's `lod_alt` already
/// packs only one sample per CHUNK (not per block) — but a default world is
/// 1024×1024 chunks, which is still far too many quads for a "coarse"
/// far-mesh and a needlessly large one-shot payload. [`send_lod_alt_once`]
/// stride-samples down to this cap.
const LOD_ALT_MAX_DIM: u32 = 128;

/// One-shot latch for the EM-3.10b far-terrain heightmap broadcast.
#[derive(Resource, Default)]
pub struct LodAltState {
    sent: bool,
}

/// Registers [`LodAltState`] + [`send_lod_alt_once`]. Same gate as the terrain
/// stream (`ClientState::Disconnected`, i.e. this App is the terrain/entity
/// SOURCE). Add AFTER [`PlayerBridgePlugin`] (reads [`EmbeddedPlayer`]).
pub struct LodAltStreamPlugin;

impl Plugin for LodAltStreamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LodAltState>().add_systems(
            Update,
            send_lod_alt_once.run_if(in_state(ClientState::Disconnected)),
        );
    }
}

/// Pure stride/grid-dimension math for [`send_lod_alt_once`]'s downsample,
/// split out so it's unit-testable without a real `WorldData`/`Client`
/// (review should-fix #4 — this arithmetic had zero direct coverage).
/// `stride` is how many chunks one sampled cell covers (≥1, so a world at or
/// below [`LOD_ALT_MAX_DIM`] is sampled 1:1); `grid_w`/`grid_h` are the
/// resulting sample-grid dimensions (each ≥1, even for a degenerate 0-sized
/// input axis, so callers never divide by zero downstream).
fn lod_alt_grid_dims(chunk_w: u16, chunk_h: u16) -> (u32, u32, u32) {
    let stride = u32::from(chunk_w.max(chunk_h))
        .div_ceil(LOD_ALT_MAX_DIM)
        .max(1);
    let grid_w = u32::from(chunk_w).div_ceil(stride).max(1);
    let grid_h = u32::from(chunk_h).div_ceil(stride).max(1);
    (stride, grid_w, grid_h)
}

/// Broadcasts the downsampled `lod_alt` heightmap ONCE, as soon as the
/// embedded local-player [`EmbeddedPlayer`] exists (its `world_data()` is
/// populated synchronously inside `Client::new`, well before the player
/// reaches in-game — see [`EmbeddedPlayer::world_data`]).
///
/// v1 has no spectator-only path: without an embedded player (persister
/// fallback only), there is no `Client`/`WorldData` to read from, so the far
/// mesh simply never arrives and the client keeps the sky+fog fallback — the
/// same acceptable degradation `xindeler_client::lod` documents for the
/// culling-only v1.
fn send_lod_alt_once(
    player: Option<bevy::ecs::change_detection::NonSend<EmbeddedPlayer>>,
    mut state: bevy::ecs::system::ResMut<LodAltState>,
    mut writer: MessageWriter<ToClients<NetLodAlt>>,
) {
    if state.sent {
        return;
    }
    let Some(player) = player else { return };
    let world_data = player.world_data();
    let size = world_data.chunk_size(); // Vec2<u16>, chunk-grid dimensions
    if size.x == 0 || size.y == 0 {
        return; // not populated yet (shouldn't happen once the Client exists)
    }

    let (stride, grid_w, grid_h) = lod_alt_grid_dims(size.x, size.y);

    let mut heights = Vec::with_capacity((grid_w * grid_h) as usize);
    for j in 0..grid_h {
        for i in 0..grid_w {
            let cx = (i * stride).min(u32::from(size.x) - 1);
            let cy = (j * stride).min(u32::from(size.y) - 1);
            #[expect(clippy::cast_possible_wrap, reason = "chunk coords ≪ i32::MAX")]
            let alt = world_data
                .alt_at(vek::Vec2::new(cx as i32, cy as i32))
                .unwrap_or(0.0);
            heights.push(alt);
        }
    }

    // TODO (still open past EM-4.2d): `targets: All` + a global `sent` latch
    // only reaches clients connected AT the single broadcast — a client
    // joining after it never receives the far-terrain heightmap (same
    // accepted limitation as `TerrainAnchor` above). This is a late-JOIN
    // replay gap, not region-scoping — EM-4.2d (T47.6) only wired per-client
    // ENTITY visibility (see `stream_terrain_changes`'s doc comment); it does
    // not touch this one-shot broadcast's join-timing behavior at all. Needs
    // a per-connection "have I sent this yet" once real multi-client join
    // timing matters.
    writer.write(ToClients {
        targets: SendTargets::All,
        message: NetLodAlt::encode([grid_w, grid_h], stride, &heights),
    });
    state.sent = true;
    tracing::info!(
        grid_w,
        grid_h,
        stride,
        "far-terrain lod-alt grid broadcast (EM-3.10b)"
    );
}

// ---------------------------------------------------------------------------
// EM-3.7 — entity mirror (sim specs entities → replicated Bevy entities)
// ---------------------------------------------------------------------------

/// Converts a sim/Veloren world position (`x`-east, `y`-north, `z`-up) into the
/// Bevy world frame (`y`-up) with the SAME pure rotation the voxel converter
/// and terrain pipeline use: `(x, y, z) → (x, z, −y)`
/// (`xindeler-render-voxel` `convert::to_bevy`, `pipeline::chunk_transform`).
/// Keeping entities on this exact mapping is what makes them stand ON the
/// streamed terrain rather than float or sink.
fn sim_pos_to_bevy(p: vek::Vec3<f32>) -> Vec3 { Vec3::new(p.x, p.z, -p.y) }

/// Same rotation applied to an orientation quaternion. `Ori` is z-up; rotating
/// the frame by −90° about x maps its yaw-about-z into Bevy's yaw-about-y so a
/// facing entity looks the right way. `vek::Quaternion` is `(x, y, z, w)`.
fn sim_ori_to_bevy(q: vek::Quaternion<f32>) -> Quat {
    // R = rotation that sends (x,y,z)→(x,z,−y): a −90° turn around +x.
    let frame = Quat::from_rotation_x(-core::f32::consts::FRAC_PI_2);
    let sim = Quat::from_xyzw(q.x, q.y, q.z, q.w);
    (frame * sim * frame.inverse()).normalize()
}

/// Registers the [`SimMirror`] map and the EM-3.7 systems: a one-shot test-NPC
/// spawn and the per-tick entity mirror. Runs only while acting as the terrain/
/// entity SOURCE (`ClientState::Disconnected`, the listen-server / singleplayer
/// gate — same as the terrain stream), and only after [`tick_sim`] so it reads
/// post-tick sim state.
///
/// ## EM-3.11o: also reads [`EmbeddedPlayer`], so also runs after `tick_player`
/// [`spawn_test_npcs`] now centres the wandering-NPC ring on the embedded
/// player's real position (see its doc), so this plugin depends on
/// [`crate::tick_player`] (which writes [`EmbeddedPlayer`]) having already run
/// this step — the same "reads `EmbeddedPlayer`" dependency
/// [`LodAltStreamPlugin`]'s doc already calls out for `send_lod_alt_once`.
/// Unlike that plugin, this one enforces it with an EXPLICIT
/// `.after(tick_player)` schedule constraint, not just a "register after"
/// convention in the doc comment — plugin *registration* order does not by
/// itself guarantee Bevy *execution* order (only `.chain()`/`.before()`/
/// `.after()` do), so relying on the comment alone was the gap a reviewer
/// caught here. The constraint is a no-op if `PlayerBridgePlugin` (and thus
/// `tick_player`) was never registered — see the same-schedule caveat on
/// [`tick_sim`]'s `.after()` usage.
///
/// Add AFTER [`SimBridgePlugin`] AND AFTER [`crate::PlayerBridgePlugin`] (for
/// registration-order hygiene matching the explicit constraint below; the
/// constraint itself is what actually enforces the dependency).
pub struct SimEntityMirrorPlugin;

impl Plugin for SimEntityMirrorPlugin {
    fn build(&self, app: &mut App) {
        // EM-3.11b: FixedUpdate alongside `tick_sim` — see its doc. Chaining
        // `.after(tick_sim)` requires both to live in the same schedule.
        // EM-3.11o: also `.after(tick_player)` — see this plugin's doc for
        // why an explicit constraint (not just registration order) is
        // required now that `spawn_test_npcs` reads `EmbeddedPlayer`.
        //
        // EM-4.2f: `init_resource` only INSERTS if missing (never overwrites),
        // so if `xindeler-oracle-host`'s `AiGatewayPlugin` already inserted a
        // real `AiExecutionMode` (in either add-order), this is a no-op; if
        // this crate runs standalone (e.g. in tests, or before that plugin
        // exists in an App), it defaults to `Offline` — the safe, zero-AI
        // posture.
        //
        // EM-4.5 review follow-up: `.after(ensure_default_dimension)` is now
        // explicit (not just documented) — `mirror_sim_entities` reads/
        // mutates the SAME `DimensionRegistry` `ensure_default_dimension`
        // (registered by `SimBridgePlugin`, added before this plugin) writes
        // to. Both were already only `.after(tick_sim)`, which doesn't order
        // them relative to EACH OTHER; making the edge structural (rather
        // than relying on the benign one-tick self-healing race the mirror's
        // `registry` param doc comment used to describe) removes a real,
        // if harmless, conflicting-resource-access ambiguity.
        app.init_resource::<SimMirror>()
            .init_resource::<SimLoadoutCache>()
            .init_resource::<SimRegionCache>()
            .init_resource::<TestNpcState>()
            .init_resource::<AiExecutionMode>()
            .init_resource::<AuroraOverlay>()
            // EM-4.10 Finding C: reused per-tick scratch buffers — see
            // `MirrorScratch`/`AuroraScratch`'s own doc comments.
            .init_resource::<MirrorScratch>()
            .init_resource::<AuroraScratch>()
            .add_systems(
                FixedUpdate,
                (spawn_test_npcs, mirror_sim_entities, tick_aurora_overlay)
                    .chain()
                    .after(tick_sim)
                    .after(tick_player)
                    .after(ensure_default_dimension)
                    .run_if(in_state(ClientState::Disconnected)),
            )
            // EM-4.6 (T47.8) / EM-4.10 Finding B: `FixedUpdate`, matching
            // `DimensionsPlugin`'s own chain — this system must run in the
            // SAME schedule as, and strictly BEFORE,
            // `xindeler_dimensions::teardown::teardown_completed_dimensions`
            // (added by `DimensionsPlugin` in `FixedUpdate`; see that
            // plugin's own doc comment for its full chain, including WHY the
            // whole thing moved off `Update`: sim-bookkeeping systems have no
            // reason to run at render cadence, and doing so multiplied their
            // cost — this system's own O(dimension count × mirrored
            // entities) scan included — by up to ~5x, a direct contributor
            // to the 2026-07-10 FPS-oscillation regression). Referencing
            // that function directly for `.before(..)` is legal regardless
            // of plugin add-order — Bevy resolves ordering constraints at
            // schedule-build time, after every system in the schedule is
            // registered, the same cross-crate pattern
            // `ensure_default_dimension`/`mirror_sim_entities` already
            // established for `.after(tick_sim)`.
            //
            // EM-4.6 bevy-migration-reviewer follow-up (BLOCKER fix): the
            // `.before(teardown_completed_dimensions)` edge alone left this
            // system's ordering relative to `handle_drain_requests`/
            // `predictive_gc_system` UNCONSTRAINED — both can synchronously
            // flip a dimension straight to `Teardown` within the SAME tick
            // (`DimensionRegistry::begin_draining`'s "zero occupants ->
            // immediate Teardown" rule). Only reading `Res<DimensionRegistry>`
            // (vs. their `ResMut`) meant Bevy's scheduler was free to run this
            // system BEFORE that same-tick transition happened, in which
            // case it would see the dimension as still `Active`, skip it —
            // and then `teardown_completed_dimensions` (downstream via the
            // dimensions-crate `.chain()`) would despawn the `DimensionRoot`
            // cascade that same tick, destroying the `SimEntity`/
            // `DimensionId` tags this system needs before it ever got a
            // second chance to see `Teardown`. Result: the specs entity would
            // NEVER be deleted through `delete_entity_recorded` — a permanent
            // leak. Explicit `.after(..)` edges on BOTH transition sources
            // close the race, mirroring the exact "make the edge structural"
            // fix already applied above for `ensure_default_dimension`/
            // `mirror_sim_entities`. This same-tick race guarantee is
            // preserved verbatim across the `Update` -> `FixedUpdate` move —
            // only the schedule changed, none of the ordering edges did.
            .add_systems(
                FixedUpdate,
                delete_specs_entities_for_torn_down_dimensions
                    .after(xindeler_dimensions::spinup::handle_drain_requests)
                    .after(xindeler_dimensions::predictive_gc::predictive_gc_system)
                    .before(xindeler_dimensions::teardown::teardown_completed_dimensions),
            )
            // BL-82 EM-4.9 (Phase C): releases an NPC-only dimension's
            // occupancy on drain so it can actually reach `Teardown` — see
            // `release_dimension_occupants_on_drain_request`'s own doc
            // comment for the "stuck in Draining forever" gap this closes.
            // Ordered the SAME way as `delete_specs_entities_for_torn_down_
            // dimensions` above (after the drain request is applied, before
            // the teardown-completion pass reads the resulting lifecycle),
            // and explicitly before that sibling system too so a same-tick
            // Draining -> Teardown this system causes is visible to it.
            .add_systems(
                FixedUpdate,
                release_dimension_occupants_on_drain_request
                    .after(xindeler_dimensions::spinup::handle_drain_requests)
                    .before(delete_specs_entities_for_torn_down_dimensions),
            );
    }
}

/// Latch + config for the one-shot test-NPC spawn.
#[derive(Resource)]
struct TestNpcState {
    /// Whether the spawn request has been emitted yet.
    spawned: bool,
    /// Sim tick at which the player-readiness gate (see [`spawn_test_npcs`])
    /// first held. `None` until then; [`Self::warmup_ticks`] counts down from
    /// this tick, NOT from sim boot.
    ///
    /// BL-82 EM-3.11o follow-up: `warmup_ticks` used to be the ENTIRE gate,
    /// counted from sim boot — which reproduces the exact never-loaded-chunk
    /// bug this module fixes if the latch fires while the embedded player is
    /// still mid-connect (`LoadingCharacterList`/`CreatingCharacter`/
    /// `Spawning`, all of which report `player.uid() == None`): `centre`
    /// then falls back to the geometric world-centre, which is only actually
    /// kept loaded by the persister fallback — and that fallback is itself
    /// WITHHELD while a player is still connecting (see
    /// `ensure_terrain_anchor`'s doc). So a bare tick count can't tell "the
    /// player reached in-game before tick 150" apart from "the player is
    /// still connecting past tick 150" — only the same readiness condition
    /// `ensure_terrain_anchor` uses can. `warmup_ticks` is now only a
    /// settle-time FLOOR applied on top of that condition.
    ready_since: Option<u64>,
    /// Wait for the anchor to have generated ground before spawning, so the
    /// NPCs don't fall through ungenerated terrain. Counted from
    /// [`Self::ready_since`] (see its doc), not from sim boot.
    warmup_ticks: u64,
    /// Number of wandering NPCs to spawn.
    count: u32,
}

impl Default for TestNpcState {
    fn default() -> Self {
        Self {
            spawned: false,
            ready_since: None,
            // ~150 ticks (~5 s at 30 TPS) gives the async chunk gen around the
            // anchor/player time to produce ground under the spawn ring,
            // counted from player-readiness (see `ready_since`'s doc), not
            // sim boot.
            warmup_ticks: 150,
            // 8 = two of each of the four figure paths (pig / human / wolf /
            // owl) so every EM-3.8c body type is on the ring.
            //
            // EM-3.11d: overridable via `XINDELER_TEST_NPC_COUNT` so a slow-tick
            // profiling session can densify the scene (more entities → more
            // physics/agent-AI/mirror work per tick) without a recompile. Unset
            // keeps the original EM-3.8c default.
            count: std::env::var("XINDELER_TEST_NPC_COUNT")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(8),
        }
    }
}

/// Ring radius (blocks) the wandering test NPCs are spawned around `centre`
/// (see [`spawn_test_npcs`]) — a shared const so the
/// [`tests::test_npcs_survive_around_the_players_real_spawn_point`] regression
/// test can assert against it without duplicating the literal.
const TEST_NPC_RING_RADIUS: f32 = 10.0;

/// Emits [`CreateNpcEvent`]s for a ring of wandering NPCs around the embedded
/// player's real position (or the world centre in pure-spectator mode), ONCE,
/// gated on the SAME player-readiness condition [`ensure_terrain_anchor`]
/// uses (see `player_ready` below) plus a settle-time warmup floor (see
/// [`TestNpcState::ready_since`]) — BL-82 EM-3.11o. Uses only the sim's
/// PUBLIC event bus (`State::emit_event_now` +
/// `event::{CreateNpcEvent, NpcBuilder}`) — the same path `/spawn` uses — so
/// the bridge stays a thin shell over public API.
///
/// The NPCs get an [`Agent`](comp::Agent) so the sim's AI walks them around
/// (idle wander), which is exactly the moving target EM-3.7's interpolation
/// needs to be verified against.
fn spawn_test_npcs(
    sim: Option<NonSendMut<SimServer>>,
    // BL-82 EM-3.11o: need the embedded player's real position — see the
    // `centre` doc comment below for why the old geometric-centre-only
    // formula silently spawned every test NPC into a never-loaded chunk.
    player: Option<bevy::ecs::change_detection::NonSend<EmbeddedPlayer>>,
    mut state: bevy::ecs::system::ResMut<TestNpcState>,
) {
    let Some(sim) = sim else { return };
    if state.spawned {
        return;
    }

    // BL-82 EM-3.11o: gate on the SAME player-readiness condition
    // `ensure_terrain_anchor` uses for the opposite decision (whether to
    // withhold the persister fallback). A player that is still connecting
    // (`LoadingCharacterList`/`CreatingCharacter`/`Spawning` — all report
    // `uid() == None`) must NOT be treated the same as "no player at all":
    // if we fired here while such a player was still connecting, `centre`
    // below would fall back to the geometric world-centre, but the persister
    // fallback that keeps THAT area loaded is itself withheld while the
    // player is still connecting (see `ensure_terrain_anchor`'s doc) —
    // reproducing the exact never-loaded-chunk bug this module fixes, just
    // via a different trigger (a slow first boot / slow-tick episode landing
    // the warmup latch inside the connect window instead of after it).
    //
    // Ready iff: no embedded player at all (pure spectator — the persister
    // fallback covers the geometric centre), OR the player reached a
    // terminal state: `is_in_game` (its real position is now readable) or
    // `is_failed` (terminal failure — the persister fallback covers it too).
    // Anything else (still connecting) means WAIT — do not fire yet.
    let player_ready = match &player {
        None => true,
        Some(p) => p.is_in_game() || p.is_failed(),
    };
    if !player_ready {
        return;
    }
    // `warmup_ticks` is a settle-time FLOOR counted from the tick readiness
    // FIRST held (see `TestNpcState::ready_since`'s doc), not from sim boot.
    let ready_since = *state.ready_since.get_or_insert(sim.ticks);
    if sim.ticks < ready_since + state.warmup_ticks {
        return;
    }

    // The ring centre MUST be wherever terrain is actually loaded, not the
    // geometric map centre (`world_size / 2`) the pre-EM-3.7b code assumed.
    //
    // Root cause (BL-82 EM-3.11o): `ensure_terrain_anchor`'s centered
    // persister — which used to be the thing keeping the geometric centre's
    // chunks loaded — became a FALLBACK once EM-3.7b added the embedded
    // player: it is only spawned when there's no working player. In the
    // normal case (the player DOES reach in-game), nothing keeps the
    // geometric centre's chunks loaded at all — the world's own spawn-point
    // selection routinely lands the player hundreds of blocks away from it.
    // Spawning the wandering-NPC ring at the geometric centre therefore put
    // every test NPC in a chunk `state.terrain().get_key_real(..)` reports as
    // NOT loaded; `Server::tick`'s "remove NPCs outside the view distance of
    // all players" entity-cleanup phase (`server/src/lib.rs`) deletes any
    // Presence-less, unloaded-chunk entity the same tick it's created — SO
    // EVERY test NPC was destroyed before `mirror_sim_entities` (or anything
    // else client-side) ever saw it. 100% reproducible regardless of NPC
    // count, invisible to any external observer (creation + deletion both
    // happen inside one `Server::tick()` call), and unrelated to the
    // similar-looking-but-distinct EM-3.11l capsule/manifest findings.
    //
    // Fix: centre the ring on the embedded player's own position (its
    // Presence is what actually keeps chunks loaded post-EM-3.7b) whenever
    // it's known; fall back to the geometric centre only when there is no
    // embedded player, or it terminally failed to connect — the only two
    // cases `player_ready` above lets through where the persister fallback
    // (see `ensure_terrain_anchor`) is actually spawned and genuinely keeps
    // that area loaded.
    //
    // Reads the position off the sim's specs ECS (`player_sim_entity` +
    // `comp::Pos`) rather than `EmbeddedPlayer::position()` (a client-side
    // accessor): this matches `mirror_sim_entities`'s own established
    // pattern of reading positions straight from the authoritative sim
    // storages, and keeps this system's dependency on `EmbeddedPlayer`
    // limited to the readiness signals (`uid()`/`is_in_game()`/`is_failed()`)
    // used above, not position data too.
    let centre = player
        .as_ref()
        .and_then(|p| p.uid())
        .and_then(|uid| player::player_sim_entity(&sim, uid))
        .and_then(|e| {
            sim.server
                .state()
                .ecs()
                .read_storage::<comp::Pos>()
                .get(e)
                .map(|p| p.0.xy())
        })
        .unwrap_or_else(|| {
            let size_chunks = sim.server.world().sim().get_size();
            let chunk_sz = vek::Vec2::new(32.0_f32, 32.0);
            vek::Vec2::new(size_chunks.x as f32, size_chunks.y as f32) * chunk_sz * 0.5
        });

    for i in 0..state.count {
        let angle = core::f32::consts::TAU * (i as f32) / (state.count as f32);
        // A ring so they're near the anchor camera but not stacked.
        let offset = vek::Vec2::new(angle.cos(), angle.sin()) * TEST_NPC_RING_RADIUS;
        let xy = centre + offset;
        // Spawn well above the surface; the sim drops them onto the ground
        // (physics) so we don't need the exact altitude.
        let alt = sim
            .server
            .world()
            .sim()
            .get_alt_approx(xy.map(|e| e as i32))
            .unwrap_or(0.0);
        let wpos = vek::Vec3::new(xy.x, xy.y, alt + 3.0);
        // Cycle Pig / Humanoid / Wolf / Owl around the ring so every EM-3.8c
        // figure path (quadruped-small, humanoid, quadruped-medium, bird-medium)
        // has a live subject the smoke camera can frame.
        match i % 4 {
            0 => emit_wandering_npc(&sim.server, wpos, i),
            1 => emit_wandering_humanoid(&sim.server, wpos, i),
            2 => emit_wandering_quadruped_medium(&sim.server, wpos, i),
            _ => emit_wandering_bird_medium(&sim.server, wpos, i),
        }
    }

    tracing::info!(count = state.count, "spawned test NPCs around the anchor");
    state.spawned = true;
}

/// The chunk-anchor every test-NPC spawn must carry (BL-82 EM-3.11o root
/// cause): `Server::tick`'s "remove NPCs outside the view distance of all
/// players" cleanup (`server/src/lib.rs`, entity-cleanup phase) deletes any
/// entity that lacks BOTH a `Presence` (real network clients only) AND an
/// `Anchor` the instant its current chunk isn't `get_key_real` — same tick it
/// was created, before any Bevy-side system ever observes it. Every OTHER NPC
/// spawn path (rtsim wildlife via `server/src/sys/terrain.rs`'s
/// `npc_builder.with_anchor(comp::Anchor::Chunk(key))`) already does this;
/// this bridge's test-NPC builders never did, so a wandering test NPC was
/// deleted the moment it was born whenever its very-first chunk wasn't (yet)
/// `get_key_real`-loaded — 100% reproducible regardless of scale, and
/// invisible to any external observer since creation-then-deletion happens
/// inside one `Server::tick()` call, before `mirror_sim_entities` ever runs.
///
/// ## What this anchor does NOT buy, at spawn time
/// The cleanup predicate is (`server/src/lib.rs`, ~line 1004-1015):
/// `Some(Anchor::Chunk(hc)) => get_key_real(chunk_key).is_none() &&
/// get_key_real(*hc).is_none()`. Since the anchor set here is the entity's OWN
/// spawn chunk (`hc == chunk_key`), the `&&` collapses to one test — this
/// anchor gives ZERO incremental protection over having no anchor at all at
/// the moment of spawn. It only starts mattering LATER, after the entity has
/// wandered away to a chunk different from the one recorded here: its
/// original spawn chunk stays "anchored" (won't itself unload) even if the
/// entity's CURRENT chunk isn't loaded, buying it one step of grace during a
/// wander. The actual fix for the spawn-time deletion this doc describes is
/// centring the ring on a position that IS already loaded (`centre` in
/// [`spawn_test_npcs`]), not this anchor — this anchor is a good practice to
/// match every other NPC spawn path, not the load-bearing fix.
fn chunk_anchor_at(server: &Server, wpos: vek::Vec3<f32>) -> comp::Anchor {
    let key = server
        .state()
        .terrain()
        .pos_key(wpos.map(|e| e.floor() as i32));
    comp::Anchor::Chunk(key)
}

/// Requests one wandering HUMANOID NPC (a random human) at `wpos` through the
/// sim's PUBLIC event bus (EM-3.8b). Same path as [`emit_wandering_npc`], but a
/// `Body::Humanoid` so the client assembles the real 16-bone character figure.
fn emit_wandering_humanoid(server: &Server, wpos: vek::Vec3<f32>, index: u32) {
    // A fixed, valid default human so the figure is deterministic across runs;
    // `validate()` clamps the cosmetic indices to the species' ranges.
    let mut hum = comp::humanoid::Body {
        species: comp::humanoid::Species::Human,
        body_type: comp::humanoid::BodyType::Male,
        hair_style: 0,
        beard: 0,
        eyes: 0,
        accessory: 0,
        hair_color: 0,
        skin: 0,
        eye_color: 0,
    };
    hum.validate();
    let body: comp::Body = hum.into();

    // EM-3.8d: give the test humanoid a real equipped loadout (starter sword +
    // worker armour), so the client assembles REAL gear on it — the same gear
    // the embedded player carries. This makes the wandering-NPC ring an
    // independent demonstration of equipped gear even when the smoke camera
    // frames an NPC rather than the player.
    let inventory = humanoid_test_inventory(body);

    let npc = NpcBuilder::new(
        comp::Stats::new(comp::Content::Plain(format!("Test Human {index}")), body),
        body,
        comp::Alignment::Wild,
    )
    .with_health(comp::Health::new(body))
    .with_inventory(inventory)
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos))
    .with_anchor(chunk_anchor_at(server, wpos));

    server.state().emit_event_now(CreateNpcEvent {
        pos: comp::Pos(wpos),
        ori: comp::Ori::default(),
        npc,
    });
}

/// Builds a visible starter loadout (sword + worker clothes + lantern + a
/// bronze-mail head cap + a glider — EM-3.8e) as an
/// [`Inventory`](comp::Inventory) for a test humanoid NPC (EM-3.8d). Mirrors
/// the embedded player's default Warrior kit (the head cap matches the
/// Warrior's real `warrior.ron` bronze-mail set) so the gear a test NPC shows
/// matches the player's, PLUS a helmet the Warrior's actual starter loadout
/// doesn't include, so the smoke screenshot has something to show the new
/// EM-3.8e helmet rendering on. Requires the asset tree (item defs) — only
/// called on the live sim, which always has it.
fn humanoid_test_inventory(body: comp::Body) -> comp::Inventory {
    use comp::inventory::loadout_builder::LoadoutBuilder;

    let item = comp::Item::new_from_asset_expect;
    let loadout = LoadoutBuilder::empty()
        .active_mainhand(Some(item("common.items.weapons.sword.starter")))
        .chest(Some(item(
            "common.items.armor.misc.chest.worker_purple_brown",
        )))
        .pants(Some(item("common.items.armor.misc.pants.worker_brown")))
        .feet(Some(item("common.items.armor.misc.foot.sandals")))
        .lantern(Some(item("common.items.lantern.black_0")))
        .head(Some(item("common.items.armor.mail.bronze.head")))
        .glider(Some(item("common.items.glider.basic_white")))
        .build();
    comp::Inventory::with_loadout(loadout, body)
}

/// Requests one wandering QUADRUPED-MEDIUM (a Wolf) at `wpos` through the sim's
/// PUBLIC event bus (EM-3.8c) so the client assembles the real QM figure.
fn emit_wandering_quadruped_medium(server: &Server, wpos: vek::Vec3<f32>, index: u32) {
    let body: comp::Body = comp::quadruped_medium::Body {
        species: comp::quadruped_medium::Species::Wolf,
        body_type: comp::quadruped_medium::BodyType::Male,
    }
    .into();

    let npc = NpcBuilder::new(
        comp::Stats::new(comp::Content::Plain(format!("Test Wolf {index}")), body),
        body,
        comp::Alignment::Wild,
    )
    .with_health(comp::Health::new(body))
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos))
    .with_anchor(chunk_anchor_at(server, wpos));

    server.state().emit_event_now(CreateNpcEvent {
        pos: comp::Pos(wpos),
        ori: comp::Ori::default(),
        npc,
    });
}

/// Requests one wandering BIRD-MEDIUM (a Snowy Owl) at `wpos` through the sim's
/// PUBLIC event bus (EM-3.8c) so the client assembles the real bird figure.
fn emit_wandering_bird_medium(server: &Server, wpos: vek::Vec3<f32>, index: u32) {
    let body: comp::Body = comp::bird_medium::Body {
        species: comp::bird_medium::Species::SnowyOwl,
        body_type: comp::bird_medium::BodyType::Male,
    }
    .into();

    let npc = NpcBuilder::new(
        comp::Stats::new(comp::Content::Plain(format!("Test Owl {index}")), body),
        body,
        comp::Alignment::Wild,
    )
    .with_health(comp::Health::new(body))
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos))
    .with_anchor(chunk_anchor_at(server, wpos));

    server.state().emit_event_now(CreateNpcEvent {
        pos: comp::Pos(wpos),
        ori: comp::Ori::default(),
        npc,
    });
}

/// Requests one wandering critter (a Pig, always available + cheap) at `wpos`
/// (sim/world coords, z-up) through the sim's PUBLIC event bus. The NPC gets an
/// [`Agent`](comp::Agent) so the sim AI walks it (idle wander) — the moving
/// target EM-3.7's interpolation is verified against. The entity materializes
/// on the NEXT `Server::tick` (events drain there).
fn emit_wandering_npc(server: &Server, wpos: vek::Vec3<f32>, index: u32) {
    let body: comp::Body = comp::quadruped_small::Body {
        species: comp::quadruped_small::Species::Pig,
        body_type: comp::quadruped_small::BodyType::Female,
    }
    .into();

    let npc = NpcBuilder::new(
        comp::Stats::new(comp::Content::Plain(format!("Test NPC {index}")), body),
        body,
        comp::Alignment::Wild,
    )
    .with_health(comp::Health::new(body))
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos))
    .with_anchor(chunk_anchor_at(server, wpos));

    server.state().emit_event_now(CreateNpcEvent {
        pos: comp::Pos(wpos),
        ori: comp::Ori::default(),
        npc,
    });
}

impl SimServer {
    /// Test/dev helper: request a wandering NPC at a sim/world position,
    /// spawned on the next tick via the public event bus. Same path as the
    /// plugin's [`spawn_test_npcs`]; exposed so headless tests can drive
    /// spawns deterministically without waiting for the warmup latch.
    pub fn spawn_wandering_npc(&self, wpos: vek::Vec3<f32>, index: u32) {
        emit_wandering_npc(&self.server, wpos, index);
    }
}

/// The item-definition-id string an armour slot's equipped item resolves to —
/// the SAME key voxygen's `CharacterCacheKey::key_from_slot` uses to look up
/// the per-item `.vox` in the frozen armour manifests (EM-3.8d). `None` = the
/// slot is empty (the figure uses the manifest `default`).
fn armor_key(inventory: &comp::Inventory, slot: EquipSlot) -> Option<String> {
    inventory
        .equipped(slot)
        .map(|item| match item.item_definition_id() {
            ItemDefinitionId::Simple(id) => id.into_owned(),
            ItemDefinitionId::Compound { simple_base, .. } => simple_base.to_owned(),
            ItemDefinitionId::Modular { pseudo_base, .. } => pseudo_base.to_owned(),
        })
}

/// Builds the replicated [`NetTool`] for an equipped tool item — its
/// weapon-manifest key ([`NetToolKey`], mirroring voxygen's `ToolKey`) plus the
/// `ToolKind`/`Hands` the animation needs to sheathe it. `None` if the item is
/// not actually a tool (defensive; the tool slots only hold tools).
fn net_tool(item: &comp::Item) -> Option<NetTool> {
    let ItemKind::Tool(tool) = &*item.kind() else {
        return None;
    };
    let key = match item.item_definition_id() {
        ItemDefinitionId::Simple(id) => NetToolKey::Tool(id.into_owned()),
        ItemDefinitionId::Compound { simple_base, .. } => NetToolKey::Tool(simple_base.to_owned()),
        ItemDefinitionId::Modular { .. } => {
            // Modular weapons key on `(primary, secondary, hands)` — the same
            // `ModularWeaponKey` the frozen weapon manifest is keyed by.
            let (primary, secondary, hands) = modular::weapon_to_key(item);
            NetToolKey::Modular {
                primary,
                secondary,
                hands,
            }
        },
    };
    Some(NetTool {
        key,
        kind: tool.kind,
        hands: tool.hands,
    })
}

/// Projects a sim `comp::Inventory` down to the figure-relevant equipped gear
/// the humanoid figure assembly consumes (EM-3.8d). Reads the same equip slots
/// voxygen's figure cache does; everything else in the inventory is irrelevant
/// to the rendered model and stays server-side.
///
/// `gliding` (EM-3.8e) is NOT read from the inventory — it's the caller's
/// `CharacterState`-derived signal (see [`is_gliding`]), passed straight
/// through onto [`NetLoadout::gliding`].
fn net_loadout_from_inventory(inventory: &comp::Inventory, gliding: bool) -> NetLoadout {
    NetLoadout {
        active_tool: inventory
            .equipped(EquipSlot::ActiveMainhand)
            .and_then(net_tool),
        second_tool: inventory
            .equipped(EquipSlot::ActiveOffhand)
            .and_then(net_tool),
        chest: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Chest)),
        belt: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Belt)),
        back: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Back)),
        pants: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Legs)),
        shoulder: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Shoulders)),
        hand: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Hands)),
        foot: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Feet)),
        lantern: armor_key(inventory, EquipSlot::Lantern),
        head: armor_key(inventory, EquipSlot::Armor(ArmorSlot::Head)),
        glider: armor_key(inventory, EquipSlot::Glider),
        gliding,
    }
}

/// Whether a sim `CharacterState` is glide-shaped — the figure should show
/// the glider mesh (EM-3.8e). Matches voxygen: both `Glide` (actively
/// airborne) AND `GlideWield` (the glider-out, pre-jump pose) render the
/// glider (`next.glider.scale = Vec3::one()` in both animations); the client
/// approximates both with a single glide animation rather than modelling
/// `GlideWield`'s distinct pose separately — a deliberate simplification, not
/// a gating bug.
fn is_gliding(character_state: Option<&comp::CharacterState>) -> bool {
    // Reuse the sim's own `Glide | GlideWield` predicate rather than
    // hand-rolling the match, so a future upstream change to what counts as
    // "glide-shaped" can't silently drift between the two copies.
    character_state.is_some_and(comp::CharacterState::is_glide_wielded)
}

/// EM-4.5: whether [`mirror_sim_entities`] may create a NEW mirror entity
/// for `dimension` — pulled out as a pure function (rather than inlined)
/// specifically so this "interaction with the mirror/visibility systems"
/// spec §1.8 asks `Draining` to have is unit-testable without booting a real
/// sim (see the `tests` module below). `None` (dimension not registered
/// yet — see the caller's doc comment for when that happens) is treated as
/// "not accepting yet", same as a `Spinup`/`Draining`/`Teardown` dimension.
fn mirror_admits_new_entity(dimension: Option<&DimensionState>) -> bool {
    dimension.is_some_and(DimensionState::accepts_new_entrants)
}

/// Reads the sim's client-visible entities off the specs storages and UPSERTs
/// one `Replicated` Bevy entity per sim entity carrying the replicated net
/// components. Despawns mirrors whose sim entity has disappeared.
///
/// ## Visibility filter
/// Mirrors the authoritative predicate the sim's `RegionMap::tick` uses to
/// decide what is synced to clients (`common/src/region.rs`): an entity is
/// client-visible iff it has a `Pos` AND (has no `Presence` OR its
/// `Presence::kind.sync_me()`). We additionally require a `Body` (nothing to
/// display without one). `Ori`/`Vel`/`Health` are optional (`.maybe()`).
/// This predicate decides whether an entity is mirrored AT ALL; per-client
/// interest management on TOP of that (which of the mirrored entities a given
/// client can see) is EM-4.2d's `RegionKey`, written below and consumed by
/// `xindeler-server-app`'s `RegionKey`-keyed `VisibilityFilter`.
///
/// ## Read-only into the sim
/// Only `read_storage` + `entities` — never writes into specs (isolation law
/// rule 4). The write side is entirely on the Bevy world (spawn/insert/despawn
/// of the mirror entities).
///
/// ## EM-4.5: dimension-gated entity creation
/// [`mirror_admits_new_entity`] gates the `None => spawn` branch below: a
/// dimension that isn't `Active` (not yet wrapped, `Draining`, or
/// `Teardown`) never gets a brand-new mirror entity, while entities ALREADY
/// mirrored keep updating regardless of lifecycle — "existing players may
/// finish/leave normally" during `Draining` (spec §1.8).
fn mirror_sim_entities(
    sim: Option<NonSendMut<SimServer>>,
    // EM-3.7b: the embedded local player, if any. Used to tag ITS mirror entity
    // with `NetLocalPlayer` so the client's third-person camera follows it.
    player: Option<bevy::ecs::change_detection::NonSend<EmbeddedPlayer>>,
    mut mirror: bevy::ecs::system::ResMut<SimMirror>,
    mut loadout_cache: bevy::ecs::system::ResMut<SimLoadoutCache>,
    // EM-4.5: the dimension registry — `DimensionsPlugin` (added by
    // `SimBridgePlugin::build`, which runs before this plugin per its own
    // "Add AFTER SimBridgePlugin" doc) always inserts it, so this is a plain
    // resource, not optional. `ResMut` (not `Res`): besides (a) tagging every
    // NEWLY-mirrored entity with `DimensionId`/`DimensionRoot` — ITS assigned
    // dimension's root entity (BL-82 EM-4.9, Phase C: generalized from a
    // DEFAULT-only tag once entity-factory batches could target a real
    // non-default dimension; see `SimEntityDimension`'s doc comment) — and
    // (b) gating NEW mirror creation on `DimensionLifecycle::
    // accepts_new_entrants`, this system ALSO (c) keeps `DimensionState`'s
    // occupant bookkeeping in sync with what's actually mirrored
    // (`try_add_occupant`/`remove_occupant`) — see the should-fix note this
    // addresses: without it, `begin_draining(DimensionId::DEFAULT)` would
    // ALWAYS see zero occupants (nothing else in this codebase calls
    // `try_add_occupant`) and skip straight to `Teardown` even while real
    // players/NPCs are actively mirrored. `SimEntityMirrorPlugin` registers
    // this system `.after(ensure_default_dimension)` (both are `.after(
    // tick_sim)`, which alone wouldn't order them relative to EACH OTHER —
    // review follow-up: made structural, not just documented), so dimension
    // 0 is ALWAYS already registered by the time this runs with a live
    // `SimServer` — `registry.get(DimensionId::DEFAULT)` returning `None`
    // below is unreachable in practice, just handled defensively.
    mut registry: bevy::ecs::system::ResMut<DimensionRegistry>,
    // EM-4.2d: per-entity region-key cache, mirroring `SimLoadoutCache`'s
    // dedup shape — only re-inserts `RegionKey` when an entity's region
    // actually changed, since replicon's `VisibilityFilter` re-evaluates
    // every connected client on every insert/replace.
    mut region_cache: bevy::ecs::system::ResMut<SimRegionCache>,
    // BL-82 EM-4.9 (Phase C): the pending non-default-dimension attribution
    // queue (populated by `apply_pending_entity_template_spawns`) and the
    // per-entity dimension decision cache — see their own doc comments.
    mut attribution: bevy::ecs::system::ResMut<PendingDimensionAttribution>,
    mut entity_dims: bevy::ecs::system::ResMut<SimEntityDimension>,
    // EM-4.10 Finding C: reused scratch buffers — see `MirrorScratch`'s doc
    // comment. `.clear()`ed below instead of freshly allocated every tick.
    mut scratch: bevy::ecs::system::ResMut<MirrorScratch>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    // The player's sim entity (if in game), so we can mark exactly one mirror.
    let player_sim_entity = player
        .as_ref()
        .and_then(|p| p.uid())
        .and_then(|uid| player::player_sim_entity(&sim, uid));

    let ecs = sim.server.state().ecs();

    let entities = ecs.entities();
    let positions = ecs.read_storage::<comp::Pos>();
    let orientations = ecs.read_storage::<comp::Ori>();
    let velocities = ecs.read_storage::<comp::Vel>();
    let bodies = ecs.read_storage::<comp::Body>();
    let healths = ecs.read_storage::<comp::Health>();
    let presences = ecs.read_storage::<comp::Presence>();
    // EM-3.8d: read the loadout so humanoids mirror their real equipped gear.
    let inventories = ecs.read_storage::<comp::Inventory>();
    // EM-3.8e: read whether each entity is currently gliding, for the
    // NetLoadout::gliding figure-visibility flag.
    let character_states = ecs.read_storage::<comp::CharacterState>();
    // EM-4.2f: read the sim's stable identity so the mirror can carry it onto
    // the wire as `NetUid`. `.maybe()` (not required): every sim entity is
    // expected to have one ("for now we expect all entities have a Uid
    // component" — `server/src/state_ext.rs`), but this stays defensive so a
    // future entity that somehow lacks one still mirrors its other fields
    // rather than being silently dropped (additive-only requirement).
    let uids = ecs.read_storage::<Uid>();
    // BL-82 EM-4.9 (Phase C): read ONLY to gate `PendingDimensionAttribution`
    // consumption to Agent-bearing (NPC) entities — real players never carry
    // `Agent` server-side, so a same-tick player login can never consume a
    // factory batch's queued dimension entry. See `PendingDimensionAttribution`'s
    // doc comment for the full correlation mechanism and its documented limits.
    let agents = ecs.read_storage::<comp::Agent>();

    // EM-4.2d (superseded by EM-4.10 Finding C below): this mirror loop used
    // to allocate `seen`/`updates`/`seen_set` fresh every tick over all
    // visible entities — fine at test-NPC scale, but Wave-3's batch-spawning
    // invalidated that assumption (see `MirrorScratch`'s doc comment for the
    // full reasoning). `net_health`'s per-tick `remove::<NetHealth>()` for
    // healthless entities remains (replicon no-ops it, harmless) — not part
    // of this allocation fix.
    //
    // EM-4.2d update: interest management landed WITHOUT reshaping this loop
    // — `RegionKey` is simply one more per-entity field computed alongside
    // the others below, deduped through `region_cache` exactly like
    // `NetLoadout`'s own dedup, so it does NOT re-insert (and thus does not
    // force a replicon visibility re-evaluation) on every tick, only when an
    // entity actually crosses into a new region.
    //
    // EM-4.10 Finding C: reuse `scratch`'s buffers instead of allocating
    // fresh ones — `.clear()` keeps the already-grown capacity, so this
    // becomes a cheap truncation instead of an allocation once the buffers
    // have warmed up to the steady-state mirrored-entity count. A single
    // reborrow (`&mut *scratch`) up front lets the rest of this function
    // freely take disjoint borrows of `scratch`'s fields (`seen`/`updates`/
    // `seen_set`) as plain struct-field borrows, rather than fighting the
    // borrow checker through `ResMut`'s `DerefMut` on every access.
    let scratch = &mut *scratch;
    scratch.seen.clear();
    scratch.updates.clear();
    let seen = &mut scratch.seen;
    let updates = &mut scratch.updates;

    // `maybe()` makes these MaybeJoin members, so this is a `LendJoin` (lending
    // iterator: `while let Some(..) = it.next()`), not a plain `for`. We copy
    // scalar data out each iteration, so the per-item lifetime is fine.
    let mut it = (
        &entities,
        &positions,
        &bodies,
        orientations.maybe(),
        velocities.maybe(),
        healths.maybe(),
        presences.maybe(),
        inventories.maybe(),
        character_states.maybe(),
        uids.maybe(),
        agents.maybe(),
    )
        .lend_join();
    while let Some((
        entity,
        pos,
        body,
        ori,
        vel,
        health,
        presence,
        inventory,
        character_state,
        uid,
        agent,
    )) = it.next()
    {
        // Region-map visibility predicate (see doc comment).
        if !presence.is_none_or(|p| p.kind.sync_me()) {
            continue;
        }
        // Drop dead entities from the mirror (the client despawns the replica).
        if health.is_some_and(|h| h.is_dead) {
            continue;
        }
        seen.push(entity);

        let net_pos = NetPos(sim_pos_to_bevy(pos.0));
        let net_ori = NetOri(ori.map_or(Quat::IDENTITY, |o| sim_ori_to_bevy(o.to_quat())));
        let net_vel = NetVel(vel.map_or(Vec3::ZERO, |v| sim_pos_to_bevy(v.0)));
        // EM-3.8: replicate the FULL `Body` so the client can pick + assemble
        // the real `.vox` figure (species/body_type, and for humanoids the
        // style/armour fields). `Body` is `Copy`, so this is a plain copy.
        let net_body = NetBody(*body);
        let net_health = health.map(|h| NetHealth {
            current: h.current(),
            max: h.maximum(),
        });
        // EM-3.8d: only humanoids have a figure that armour/tools reshape, so
        // only they carry a loadout: `Body::Humanoid` → `Some(NetLoadout)`,
        // built from the real `Inventory` when present. Every humanoid spawn
        // path today attaches an `Inventory` atomically with `Body` (see
        // `state_ext::create_npc`), but we still fall back to
        // `NetLoadout::default()` rather than `None` here — the client
        // (`figure_view::classify_bodies`) waits indefinitely for a humanoid's
        // `NetLoadout` to arrive, so a future humanoid-spawn path that skips
        // that invariant must not leave the figure stuck as a capsule forever.
        let gliding = is_gliding(character_state);
        let net_loadout = matches!(body, comp::Body::Humanoid(_)).then(|| {
            inventory.map_or_else(
                || NetLoadout {
                    gliding,
                    ..NetLoadout::default()
                },
                |inv| net_loadout_from_inventory(inv, gliding),
            )
        });
        // EM-4.2f: the entity's stable sim identity, verbatim (`Uid` wraps a
        // `NonZeroU64`; `NetUid` carries the same value as a plain `u64` so
        // the wire type stays serde-simple).
        let net_uid = uid.map(|u| NetUid(u.0.get()));
        // BL-82 EM-4.9 (Phase C, T51.6): decide THIS entity's dimension once
        // — reused on every later tick via `entity_dims`, never revisited
        // (no dimension-transfer path exists yet). A brand-new (not-yet-
        // decided) entity that ALSO carries an `Agent` (an NPC — real
        // players never do) consumes the front of the pending attribution
        // queue if the registry still reports that dimension `Active`;
        // otherwise (no Agent, empty queue, or a stale/rejected candidate)
        // it falls back to `DimensionId::DEFAULT`, exactly like every
        // pre-EM-4.9 mirrored entity. See `PendingDimensionAttribution`'s
        // doc comment for the full mechanism and its documented limits.
        let entity_dimension = *entity_dims.0.entry(entity).or_insert_with(|| {
            if agent.is_some()
                && let Some(&candidate) = attribution.0.front()
            {
                attribution.0.pop_front();
                let accepts = registry
                    .get(candidate)
                    .is_some_and(|state| state.accepts_new_entrants());
                if accepts {
                    candidate
                } else {
                    DimensionId::DEFAULT
                }
            } else {
                DimensionId::DEFAULT
            }
        });
        // EM-4.2d: which region this entity currently occupies, computed
        // from the RAW sim position (`pos.0`, sim axes) — NOT `net_pos`
        // (already Bevy-axis-converted above) — matching exactly what
        // `common::region::RegionMap`/`server/src/sys/subscription.rs` key
        // entities by. `entity_dimension` (not a hardcoded
        // `DimensionId::default()`) so two dimensions occupying the SAME
        // grid cell never collide (see `xindeler_protocol::visibility`'s
        // module doc comment for the EM-4.5 extension point this realizes).
        let region_key = region_key_for_pos(entity_dimension, vek::Vec2::new(pos.0.x, pos.0.y));
        updates.push((
            entity,
            net_pos,
            net_ori,
            net_vel,
            net_body,
            net_health,
            net_loadout,
            net_uid,
            region_key,
            entity_dimension,
        ));
    }
    drop(it);
    // Storages borrow `ecs`; drop them before touching `commands`/`mirror`.
    drop((
        entities,
        positions,
        orientations,
        velocities,
        bodies,
        healths,
        presences,
        inventories,
        character_states,
        uids,
        agents,
    ));

    for (
        sim_entity,
        net_pos,
        net_ori,
        net_vel,
        net_body,
        net_health,
        net_loadout,
        net_uid,
        region_key,
        entity_dimension,
    ) in updates.drain(..)
    {
        let is_local_player = player_sim_entity == Some(sim_entity);
        // EM-3.8d: (re-)insert the loadout ONLY when it changed since we last
        // mirrored it (it is a few Strings — re-inserting every tick would
        // needlessly re-replicate them). `changed` is also true on first sight.
        let loadout_changed = net_loadout
            .as_ref()
            .is_some_and(|l| loadout_cache.0.get(&sim_entity) != Some(l));
        // EM-4.2d: same dedup shape as the loadout above — re-inserting
        // `RegionKey` every tick would re-trigger replicon's
        // `VisibilityFilter` re-evaluation for every connected client on
        // every tick, for every mirrored entity, regardless of whether it
        // actually crossed a region boundary. `changed` is also true on
        // first sight (an absent cache entry never equals `Some(region_key)`).
        let region_changed = region_cache.0.get(&sim_entity) != Some(&region_key);
        match mirror.0.get(&sim_entity).copied() {
            Some(bevy_entity) => {
                // UPSERT: overwrite the net comps every tick (server-authoritative
                // snapshot; the client interpolates toward them).
                let mut ec = commands.entity(bevy_entity);
                ec.insert((net_pos, net_ori, net_vel, net_body));
                match net_health {
                    Some(h) => {
                        ec.insert(h);
                    },
                    None => {
                        ec.remove::<NetHealth>();
                    },
                }
                if loadout_changed && let Some(l) = &net_loadout {
                    ec.insert(l.clone());
                }
                // EM-3.7b: keep the local-player marker in sync (it never moves
                // between entities in a session, but stay robust).
                if is_local_player {
                    ec.insert(NetLocalPlayer);
                } else {
                    ec.remove::<NetLocalPlayer>();
                }
                // EM-4.2f: `NetUid` is `Copy`/cheap like the other UPSERTed
                // comps — re-insert every tick when present; leave it alone
                // (never remove) if this entity somehow has no `Uid` this
                // tick, since a stable identity should not flicker away.
                if let Some(u) = net_uid {
                    ec.insert(u);
                }
                // EM-4.2d: only re-insert (and thus re-trigger visibility
                // re-evaluation) when the region actually changed.
                if region_changed {
                    ec.insert(region_key);
                }
            },
            None => {
                // EM-4.5 (BL-82 EM-4.9: generalized from a DEFAULT-only
                // check to THIS entity's own assigned dimension): don't
                // create a NEW mirror for a dimension that isn't accepting
                // new entrants (spec §1.8's acceptance bar extended to the
                // mirror: "no new player can join once Draining" applies
                // just as much to a wandering NPC as to a human player).
                // Existing mirrors (the `Some` arm above) keep updating
                // regardless — "existing players may finish/leave normally"
                // during `Draining`. A dimension that vanished between the
                // join loop's decision (above) and here (raced teardown) is
                // the same defensive `None` case — skip this tick, retry
                // next (the sim entity is NOT lost; it simply stays
                // unmirrored until then).
                let Some(dimension_state) = registry.get(entity_dimension) else {
                    continue;
                };
                if !mirror_admits_new_entity(Some(dimension_state)) {
                    continue;
                }
                let root = dimension_state.root();
                let mut ec = commands.spawn((
                    Replicated,
                    SimEntity(sim_entity),
                    entity_dimension,
                    DimensionRoot(root),
                    net_pos,
                    net_ori,
                    net_vel,
                    net_body,
                    region_key,
                ));
                if let Some(h) = net_health {
                    ec.insert(h);
                }
                if let Some(l) = &net_loadout {
                    ec.insert(l.clone());
                }
                if is_local_player {
                    ec.insert(NetLocalPlayer);
                }
                if let Some(u) = net_uid {
                    ec.insert(u);
                }
                let bevy_entity = ec.id();
                mirror.0.insert(sim_entity, bevy_entity);
                // EM-4.5: this mirror now counts as an occupant of ITS
                // dimension (see the `registry` param's doc comment for why
                // this matters: without it, `begin_draining` would always
                // see zero occupants and skip straight to `Teardown`). Can
                // only fail if the dimension stopped accepting entrants in
                // the instant between the check above and here — impossible
                // within one system's single-threaded body, but handled
                // rather than `.unwrap()`ed for robustness against a future
                // refactor that makes this async.
                if let Err(err) = registry.try_add_occupant(entity_dimension, bevy_entity) {
                    tracing::warn!(
                        ?err,
                        dimension = entity_dimension.0,
                        "failed to register new mirror as a dimension occupant"
                    );
                }
            },
        }
        // Refresh the dedup cache for this entity's loadout / region.
        if let Some(l) = net_loadout {
            loadout_cache.0.insert(sim_entity, l);
        }
        region_cache.0.insert(sim_entity, region_key);
    }

    // Despawn mirrors whose sim entity is gone / no longer visible this
    // tick. EM-4.10 Finding C: reuse `scratch.seen_set` (cleared, then
    // repopulated by draining `seen`) instead of allocating a fresh
    // `HashSet` every tick.
    scratch.seen_set.clear();
    scratch.seen_set.extend(seen.drain(..));
    let stale: Vec<specs::Entity> = mirror
        .0
        .keys()
        .copied()
        .filter(|e| !scratch.seen_set.contains(e))
        .collect();
    for sim_entity in stale {
        // BL-82 EM-4.9: this entity's OWN assigned dimension (defaults to
        // DEFAULT if, somehow, it was never decided — can't happen in
        // practice since every mirrored entity gets an entry the tick it's
        // first seen, but stays a safe fallback rather than an `.unwrap()`).
        let dimension = entity_dims
            .0
            .remove(&sim_entity)
            .unwrap_or(DimensionId::DEFAULT);
        if let Some(bevy_entity) = mirror.0.remove(&sim_entity) {
            commands.entity(bevy_entity).despawn();
            // EM-4.5: this mirror is leaving ITS dimension — the exact
            // "existing players may finish/leave normally" exit condition
            // that drives `Draining -> Teardown` (see `remove_occupant`'s
            // doc comment). A no-op `Ok(false)` if that dimension isn't
            // `Draining` (the common case) or the entity wasn't tracked as
            // an occupant (e.g. it despawned before ever completing
            // `try_add_occupant`, an edge case handled gracefully there).
            let _ = registry.remove_occupant(dimension, bevy_entity);
        }
        // Drop the cached loadout too, so a re-used specs index doesn't inherit
        // a stale entry (EM-3.8d).
        loadout_cache.0.remove(&sim_entity);
        // EM-4.2d: same reasoning for the region-key dedup cache.
        region_cache.0.remove(&sim_entity);
    }
}

// --- EM-4.6 (T47.8): sim-side dimension teardown ---------------------------

/// BL-82 EM-4.6 (T47.8): the SIM-side half of dimension teardown. Before a
/// torn-down dimension's `DimensionRoot` cascade-despawns its Bevy mirror
/// entities (`xindeler_dimensions::teardown::teardown_completed_dimensions`,
/// ordered `.after(this system)` in the SAME `FixedUpdate` schedule — EM-4.10
/// Finding B moved this off `Update` alongside the rest of the dimension-
/// lifecycle chain; see [`SimEntityMirrorPlugin`]'s own wiring), this system
/// removes the
/// CORRESPONDING specs entities from the sim through its own normal delete
/// path (`server::state_ext::StateExt::delete_entity_recorded` — the exact
/// call `server::cmd`'s admin commands and `Server::disconnect_all_clients_
/// if_requested` already use; NEVER a raw storage poke — isolation law rule
/// 4: writes into the sim go through its public API only).
///
/// Must run BEFORE the cascade-despawn: once the root despawns, the mirror
/// entities carrying `SimEntity`/`DimensionId` are gone too, and there would
/// be no way left to know WHICH specs entities belonged to the torn-down
/// dimension.
///
/// ## Today, this is dormant for anything but a bug
/// [`mirror_sim_entities`] only ever tags a NEW mirror with
/// `DimensionId::DEFAULT` (see its own doc comment) — no code path in this
/// codebase yet mirrors a specs entity into any OTHER dimension. This system
/// is nonetheless written dimension-id-generic (it matches whichever id the
/// registry reports as `Teardown`, not just a hardcoded one) so it needs NO
/// changes once EM-4.7/4.9 start spawning real specs-backed NPCs into
/// non-default dimensions — exactly the forward-looking posture spec §1.9
/// asks for ("Sim-side: bridge removes mirrored specs entities via the sim's
/// normal delete path").
///
/// ## `DimensionId::DEFAULT` is deliberately EXCLUDED
/// Same reasoning as `xindeler_dimensions::predictive_gc`'s own exclusion
/// and `xindeler_dimensions::teardown`'s own despawn-refusal (see that
/// module's doc comment for the full "why can `DimensionId::DEFAULT` even
/// reach `Teardown`" explanation) — an INDEPENDENT backstop specific to the
/// sim side: were the always-on default dimension to ever (incorrectly)
/// reach `Teardown`, this system must not delete every currently-mirrored
/// specs entity in the live game (which is what "the torn-down dimension's
/// specs entities" would mean for dimension 0 today).
/// BL-82 EM-4.9 (Phase C): releases every occupant of a dimension the moment
/// its admin-triggered [`DrainDimension`] request is seen — the fix for a
/// gap [`try_add_occupant`]/[`remove_occupant`]'s pre-EM-4.9 usage never
/// surfaced: EVERY mirrored entity (not just connected players) has always
/// counted as an occupant (see the existing `mirrors_sim_npc_to_replicon_
/// client` test's own assertion that a mirrored TEST NPC counts as a
/// dimension-0 occupant), but `remove_occupant` is only EVER called from the
/// mirror's own stale-entity sweep — which fires when a sim entity
/// disappears (dies / a player disconnects). An NPC never "disconnects" on
/// its own, so a dimension populated ONLY by ORACLE-spawned minions (no
/// player ever transferred into it — the exact shape of EM-4.9's minimum
/// gate, per the migration spec's own honesty about the deferred player-
/// transfer leg) would sit in `Draining` FOREVER once admin-drained,
/// silently blocking `DimensionLifecycle::Teardown` — and therefore
/// [`delete_specs_entities_for_torn_down_dimensions`]/`teardown_completed_
/// dimensions` — from ever running. `DimensionId::DEFAULT` is unaffected
/// either way (`begin_draining` already rejects it outright, EM-4.10 Finding
/// D), so this system explicitly skips it (nothing to release, and
/// iterating its — often large — occupant set on every `DrainDimension`
/// would be pure waste).
///
/// This does NOT despawn anything itself — it only clears the OCCUPANCY
/// bookkeeping (`DimensionRegistry::remove_occupant`), which is exactly what
/// lets a fully-occupant-drained `Draining` dimension auto-advance to
/// `Teardown` (the same real, tested exit condition a player logging out of
/// the last-occupied slot already drives — see that method's own doc
/// comment). Once `Teardown` is reached, the EXISTING
/// [`delete_specs_entities_for_torn_down_dimensions`] (sim-side) and
/// `teardown_completed_dimensions` (Bevy-side `DimensionRoot` cascade-despawn)
/// take over, unmodified.
///
/// `.after(handle_drain_requests)` (so this tick's `Active -> Draining`
/// transition, if any, has already happened) and `.before(delete_specs_
/// entities_for_torn_down_dimensions)` (so a same-tick Draining -> Teardown
/// this system causes is visible to that system in the SAME `FixedUpdate`
/// pass, not one tick later).
fn release_dimension_occupants_on_drain_request(
    mut requests: bevy::ecs::message::MessageReader<xindeler_dimensions::DrainDimension>,
    mut registry: bevy::ecs::system::ResMut<DimensionRegistry>,
    mirrors: bevy::ecs::system::Query<(Entity, &DimensionId), bevy::ecs::query::With<SimEntity>>,
) {
    for &xindeler_dimensions::DrainDimension(dimension) in requests.read() {
        if dimension == DimensionId::DEFAULT {
            continue;
        }
        for (entity, tag) in &mirrors {
            if *tag != dimension {
                continue;
            }
            // `Ok(false)`/an error both mean "nothing left to do for this
            // entity" (already untracked, or the dimension isn't Draining) —
            // neither is worth a log; the meaningful signal
            // (`teardowns_total`) is already observed elsewhere.
            let _ = registry.remove_occupant(dimension, entity);
        }
    }
}

fn delete_specs_entities_for_torn_down_dimensions(
    sim: Option<NonSendMut<SimServer>>,
    registry: bevy::ecs::system::Res<DimensionRegistry>,
    mut mirror: bevy::ecs::system::ResMut<SimMirror>,
    query: bevy::ecs::system::Query<(&SimEntity, &DimensionId)>,
) {
    let Some(mut sim) = sim else { return };
    for id in registry.ids() {
        if id == DimensionId::DEFAULT {
            continue;
        }
        if registry.lifecycle(id) != Some(DimensionLifecycle::Teardown) {
            continue;
        }
        for (sim_entity, dim) in query.iter() {
            if *dim != id {
                continue;
            }
            if let Err(err) = sim.server.state_mut().delete_entity_recorded(sim_entity.0) {
                tracing::warn!(
                    ?err,
                    ?id,
                    sim_entity = ?sim_entity.0,
                    "failed to delete a torn-down dimension's mirrored specs entity"
                );
            }
            mirror.0.remove(&sim_entity.0);
        }
    }
}

// --- EM-4.2f: AURORA-readiness overlay -------------------------------------

/// Recomputes `overlay` for the current tick's set of mirrored NPC
/// [`NetUid`]s, given the current [`AiExecutionMode`]. Pure logic (no ECS
/// types) so it is unit-testable without booting Bevy or the sim.
///
/// ## The invariant this function exists to enforce (spec §1.5)
/// **`Offline` means the map is empty, and an empty map means the NPC
/// behaves exactly as today's default `server-agent` AI — this is not a
/// degraded mode, it is the current game.** Outside `Offline`, entries exist
/// but carry only `AuroraNpcState::default` neutral placeholders until
/// BL-83 writes real data:
/// - `AiExecutionMode::Offline` ⇒ `overlay` is cleared unconditionally,
///   regardless of what was in it before (e.g. a mode flip back to `Offline`
///   mid-session must drop any placeholder entries).
/// - `AiExecutionMode::LocalOnly | AiExecutionMode::Full` ⇒ every uid in
///   `live_npc_uids` gets an entry if it doesn't already have one — an EXISTING
///   entry is left untouched (`HashMap::entry(..).or_default()`), so once BL-83
///   starts writing real content into an entry, this function never stomps it
///   back to neutral on a later tick. Entries whose uid is no longer live (NPC
///   despawned/left view) are pruned, so the map never grows unbounded and
///   never describes an NPC that no longer exists.
///
/// This function computes NOTHING about what an NPC is doing — it only
/// decides WHETHER an entry exists and, for a brand-new entry, that it
/// starts at the documented neutral default. No position/stats/behavior
/// history is read here or anywhere in this task.
///
/// ## EM-4.10 Finding C: `scratch` is a reused buffer, not owned locally
/// This used to collect `live_npc_uids` into a fresh `HashSet` every call;
/// the system wrapper ([`tick_aurora_overlay`]) now passes in
/// [`AuroraScratch`]'s reused set instead (`.clear()`ed here, not
/// reallocated), matching [`MirrorScratch`]'s pattern. Still pure/unit-
/// testable — a test just passes its own scratch `HashSet` (see the
/// `tests` module below), which costs nothing at test scale.
fn recompute_aurora_overlay(
    mode: AiExecutionMode,
    overlay: &mut AuroraOverlay,
    live_npc_uids: impl Iterator<Item = u64>,
    scratch: &mut std::collections::HashSet<u64>,
) {
    if mode == AiExecutionMode::Offline {
        overlay.0.clear();
        return;
    }

    scratch.clear();
    scratch.extend(live_npc_uids);
    overlay.0.retain(|uid, _| scratch.contains(uid));
    for &uid in scratch.iter() {
        overlay.0.entry(uid).or_default();
    }
}

/// Bevy-system wrapper over [`recompute_aurora_overlay`]: the "NPCs" are every
/// mirrored entity carrying [`NetUid`] that is NOT [`NetLocalPlayer`].
///
/// v1 caveat: today the ONLY mirrored non-NPC is the one embedded local
/// player (`mirror_sim_entities` tags exactly that one sim entity with
/// `NetLocalPlayer`), so this predicate happens to be correct for the
/// current single-embedded-player listen-server architecture — but it is
/// NOT automatically correct for a future remote/second player: any other
/// player-controlled mirrored entity would carry `NetUid` and no
/// `NetLocalPlayer` tag, and would be silently classified as an NPC here.
/// A future multi-player mirror should add a positive "this is a
/// player" marker (rather than relying on the ABSENCE of
/// `NetLocalPlayer`) before that scenario becomes real.
fn tick_aurora_overlay(
    mode: Res<AiExecutionMode>,
    mut overlay: bevy::ecs::system::ResMut<AuroraOverlay>,
    npcs: bevy::ecs::system::Query<&NetUid, bevy::ecs::query::Without<NetLocalPlayer>>,
    // EM-4.10 Finding C: reused scratch set — see `AuroraScratch`'s doc
    // comment.
    mut scratch: bevy::ecs::system::ResMut<AuroraScratch>,
) {
    recompute_aurora_overlay(
        *mode,
        &mut overlay,
        npcs.iter().map(|uid| uid.0),
        &mut scratch.live,
    );
}

/// Boots a throwaway singleplayer-style server rooted at `data_dir` for
/// tests/dev shells: unused local TCP port, auth disabled, default world
/// (needs `VELOREN_ASSETS`/`XINDELER_ASSETS` + the LFS map blobs), SQLite under
/// `<data_dir>/saves`. Same fixed 2-worker runtime this function has always
/// used (small, dev/test-scale; unaffected by [`boot_with_settings`]'s
/// EM-4.2b generalization below — see that function's doc comment).
pub fn boot_test_server(data_dir: &Path) -> Result<SimServer, server::Error> {
    let settings = Settings::singleplayer(data_dir);
    let editable_settings = EditableSettings::singleplayer(data_dir);
    let database_settings = DatabaseSettings {
        db_dir: data_dir.join("saves"),
        sql_log_mode: SqlLogMode::Disabled,
    };
    boot_with_settings(
        settings,
        editable_settings,
        database_settings,
        data_dir,
        2,
        "tokio-sim-bridge",
    )
}

/// Boots a [`SimServer`] from ALREADY-LOADED settings (BL-82 EM-4.2b).
///
/// Extracted from [`boot_test_server`] so a shell that needs a DIFFERENT
/// settings source — e.g. `xindeler-server-app`'s dedicated-server shell,
/// which reads the real production `<userdata>/server/server_config/
/// settings.ron` via `server::Settings::load` rather than the singleplayer
/// shortcut — can boot the SAME `SimServer` type (with the `pending_terrain`
/// snapshot [`SimTerrainStreamPlugin`] depends on) instead of maintaining a
/// second, divergent copy of this boot recipe. `boot_test_server` is now a
/// thin wrapper over this for the singleplayer-settings case, passing its own
/// unchanged fixed 2-worker sizing.
///
/// `worker_threads`/`thread_name_prefix` are caller-controlled (an
/// EM-4.2b-review fix): the FIRST version of this extraction hardcoded the
/// singleplayer/dev-test path's small fixed `2`-worker sizing for every
/// caller, silently regressing `xindeler-server-app`'s real dedicated-server
/// runtime, which used to scale with host core count
/// (`(num_cpus::get() / 4).max(MIN_RECOMMENDED_TOKIO_THREADS)`, the same
/// formula `server-cli` sizes its own production runtime with). That shell
/// now passes its own formula back through (see its `sim.rs`) instead of
/// silently inheriting the dev-scale default — this function no longer picks
/// a size on any caller's behalf.
pub fn boot_with_settings(
    settings: Settings,
    editable_settings: EditableSettings,
    database_settings: DatabaseSettings,
    data_dir: &Path,
    worker_threads: usize,
    thread_name_prefix: &'static str,
) -> Result<SimServer, server::Error> {
    // `Server::new` requires a runtime it can block on and spawn
    // networking/persistence tasks onto; sizing and thread naming are the
    // CALLER's call (see doc comment above) rather than a one-size-fits-all
    // default baked in here.
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(worker_threads)
            .thread_name_fn(move || {
                static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
                let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
                format!("{thread_name_prefix}-{id}")
            })
            .build()
            .expect("failed to build tokio runtime for the sim"),
    );
    let server = Server::new(
        settings,
        editable_settings,
        database_settings,
        data_dir,
        &|stage| tracing::debug!(?stage, "sim server init"),
        Arc::clone(&runtime),
    )?;
    Ok(SimServer {
        server,
        runtime,
        ticks: 0,
        pending_terrain: PendingTerrain::default(),
    })
}

/// Suggested fixed timestep for pacing a headless shell at the sim's 30 TPS
/// (`ScheduleRunnerPlugin::run_loop(SIM_TICK_INTERVAL)`).
pub const SIM_TICK_INTERVAL: Duration = Duration::from_nanos(33_333_333); // exactly 1/30 s

#[cfg(test)]
mod tests {
    use bevy::{
        MinimalPlugins,
        app::PluginGroup,
        ecs::{
            message::Messages,
            query::{With, Without},
        },
        state::app::StatesPlugin,
        time::{Fixed, TimeUpdateStrategy},
    };
    use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
    use xindeler_protocol::{AuroraNpcState, XindelerProtocolPlugin};

    use super::*;

    /// A world at/under the cap is sampled 1:1 (stride 1, grid == chunk
    /// size) — the common case for any dev/test world smaller than 128×128.
    #[test]
    fn lod_alt_grid_dims_under_cap_is_1_to_1() {
        assert_eq!(lod_alt_grid_dims(64, 64), (1, 64, 64));
        assert_eq!(lod_alt_grid_dims(128, 128), (1, 128, 128));
    }

    /// Non-square world: stride is driven by the LARGER axis, and each axis
    /// downsamples independently by that same stride (not two different
    /// strides), matching `send_lod_alt_once`'s single `stride` field.
    #[test]
    fn lod_alt_grid_dims_non_square_world() {
        // max(1024, 256) = 1024 -> stride = ceil(1024/128) = 8.
        assert_eq!(lod_alt_grid_dims(1024, 256), (8, 128, 32));
    }

    /// The default production Veloren world size — the exact case that
    /// motivated the downsample (1024×1024 chunks, uncapped, would be a
    /// 1024×1024 = 1Mi-sample payload).
    #[test]
    fn lod_alt_grid_dims_default_world_size() {
        assert_eq!(lod_alt_grid_dims(1024, 1024), (8, 128, 128));
    }

    /// A non-power-of-two size that doesn't divide the cap evenly still
    /// yields a grid that covers the WHOLE world (`div_ceil`, not `/`) and
    /// never exceeds `LOD_ALT_MAX_DIM` on either axis.
    #[test]
    fn lod_alt_grid_dims_non_power_of_two() {
        let (stride, grid_w, grid_h) = lod_alt_grid_dims(1000, 777);
        assert_eq!(stride, 8); // ceil(1000/128) = 8
        assert_eq!(grid_w, 125); // ceil(1000/8) = 125
        assert_eq!(grid_h, 98); // ceil(777/8) = 98 (covers all 777, not 776)
        assert!(grid_w <= LOD_ALT_MAX_DIM && grid_h <= LOD_ALT_MAX_DIM);
    }

    /// A degenerate 0-sized axis still yields `grid >= 1` (never 0), so
    /// `send_lod_alt_once`'s `heights` Vec is never empty by construction —
    /// callers guard the *real* 0-size case earlier (`size.x == 0 ...
    /// return`), but the pure function itself must not divide-by-zero or
    /// underflow if ever called with one.
    #[test]
    fn lod_alt_grid_dims_zero_axis_never_yields_zero_grid() {
        let (stride, grid_w, grid_h) = lod_alt_grid_dims(0, 64);
        assert_eq!(stride, 1);
        assert_eq!(grid_w, 1);
        assert_eq!(grid_h, 64);
    }

    /// EM-4.5's "interaction with the mirror/visibility systems" acceptance
    /// bar, exercised as a pure-function unit test: a dimension not yet
    /// registered, or `Spinup`/`Draining`/`Teardown`, never admits a NEW
    /// mirror entity; only `Active` does. Needs real assets
    /// (`DimensionRegistry::insert_spinning_up`/`complete_spinup` go through
    /// `World::empty()`, which loads the color/feature manifests) — same
    /// convention as this crate's other asset-dependent tests, but fast
    /// enough (no real world generation) to not need `#[ignore]`.
    #[test]
    fn mirror_admits_new_entity_only_for_active_dimension() {
        use xindeler_dimensions::{DimensionLifecycle, DimensionRegistry};

        assert!(
            !mirror_admits_new_entity(None),
            "an unregistered dimension (not wrapped yet) must not admit new entrants"
        );

        // EM-4.10 Finding D: a NON-default id from here on —
        // `DimensionRegistry::begin_draining` now rejects
        // `DimensionId::DEFAULT` outright (see that method's own doc
        // comment), so it can no longer reach `Draining`/`Teardown` at all.
        // `mirror_admits_new_entity` is a pure function of `DimensionState`'s
        // lifecycle and doesn't care which id it belongs to, so this swap
        // preserves the test's exact intent.
        let id = DimensionId(1);
        let mut registry = DimensionRegistry::default();
        let root = bevy::ecs::entity::Entity::from_raw_u32(1).unwrap();
        registry.insert_spinning_up(id, root, 0).unwrap();
        assert_eq!(registry.lifecycle(id), Some(DimensionLifecycle::Spinup));
        assert!(
            !mirror_admits_new_entity(registry.get(id)),
            "Spinup must not admit new entrants"
        );

        let (world, index) = server::World::empty();
        registry
            .complete_spinup(id, std::sync::Arc::new(world), index)
            .unwrap();
        assert!(
            mirror_admits_new_entity(registry.get(id)),
            "Active must admit new entrants"
        );

        registry.begin_draining(id).unwrap();
        // No occupants were ever added in this test, so this dimension went
        // straight Draining -> Teardown (see `DimensionRegistry::
        // begin_draining`'s doc comment) — either way, neither state admits
        // a new entrant, which is exactly what this test is proving.
        assert!(
            !mirror_admits_new_entity(registry.get(id)),
            "neither Draining nor Teardown may admit a new entrant"
        );
    }

    /// EM-1.5 acceptance: boot a real test-world `Server` and tick it 100×
    /// inside a headless Bevy `App`.
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with XINDELER_ASSETS"]
    fn boots_and_ticks_100_times() {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(SimBridgePlugin);
        // EM-3.11b: `tick_sim` now runs in `FixedUpdate`, decoupled from
        // `Update`. Pin the fixed step to exactly `SIM_TICK_HZ` and feed a
        // matching real-time delta each `app.update()` call (bevy_time's own
        // test pattern) so one `app.update()` reliably runs FixedUpdate
        // exactly once — otherwise a tight test loop advances real wall time
        // by microseconds per call, far under the 1/30 s threshold, and
        // FixedUpdate simply wouldn't fire.
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);

        for _ in 0..100 {
            app.update();
        }

        let sim = app.world().non_send::<SimServer>();
        // EM-3.11b: `tick_sim` now runs in `FixedUpdate`, which needs the
        // accumulator to reach one full `1 / SIM_TICK_HZ` step before its
        // first run — the very first `app.update()` establishes the
        // baseline instant (0 accumulated time), so 100 update() calls with
        // a steady per-call delta yield 99 fixed steps, not 100 (a one-step
        // startup lag, not dropped ticks — see bevy_time's own fixed-timestep
        // tests for the same off-by-one). Was a strict `== 100` pre-EM-3.11b
        // (dt=0 on frame 1 still ran a — zero-length — `Update` tick).
        assert!(
            (99..=100).contains(&sim.ticks),
            "100 app.update() calls should complete ~100 sim ticks (99 or 100, allowing the \
             fixed-timestep accumulator's one-step startup lag), got {}",
            sim.ticks
        );
        // The sim's own game-time clock advanced with the fixed dt, proving
        // `Server::tick` really ran.
        assert!(
            sim.server.state().get_time() > 0.0,
            "sim game time should advance across 100 ticks"
        );
    }

    /// EM-3.6 acceptance (server side): boot the REAL sim, add the terrain
    /// stream stack (protocol + bridge + `SimTerrainStreamPlugin`) with the
    /// replicon SERVER role in listen-server config (no connected client, so
    /// `ClientState::Disconnected`), and tick until the sim generates chunks
    /// around the centered persister. Assert that:
    ///  - the anchor persister was spawned and its `TerrainAnchor` broadcast,
    ///  - ≥ N chunks were emitted as `ToClients<CompressedChunk>`,
    ///  - those chunks ALSO arrive locally as plain `CompressedChunk` (the
    ///    listen-server loopback), decode back to a real `TerrainChunk`.
    ///
    /// Chunk gen is async (slow-jobs worker threads), so we tick generously.
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with XINDELER_ASSETS"]
    fn streams_real_chunks_to_local_client() {
        const MIN_CHUNKS: usize = 8;
        const MAX_TICKS: u32 = 4000;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            // Replicate on every update (default FixedPostUpdate may not run
            // in a manually-stepped app).
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((XindelerProtocolPlugin, SimBridgePlugin, SimTerrainStreamPlugin))
            .finish();
        // EM-3.11b: see `boots_and_ticks_100_times` — pin FixedUpdate to run
        // exactly once per `app.update()` so this stays a per-tick loop.
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);

        // A local sink: count the `CompressedChunk` / `RemoveChunk` /
        // `TerrainAnchor` the loopback re-emits, decode one to prove validity.
        let mut total_chunks = 0usize;
        let mut anchor: Option<TerrainAnchor> = None;
        let mut decoded_ok = false;

        for tick in 0..MAX_TICKS {
            app.update();

            let world = app.world_mut();
            for msg in world.resource_mut::<Messages<CompressedChunk>>().drain() {
                if !decoded_ok {
                    let chunk = msg.decode().expect("streamed chunk decodes");
                    // A real world chunk spans a non-trivial z range.
                    assert!(chunk.get_max_z() >= chunk.get_min_z());
                    decoded_ok = true;
                }
                total_chunks += 1;
            }
            if anchor.is_none()
                && let Some(a) = world
                    .resource_mut::<Messages<TerrainAnchor>>()
                    .drain()
                    .next()
            {
                anchor = Some(a);
            }

            if total_chunks >= MIN_CHUNKS && anchor.is_some() {
                eprintln!("streamed {total_chunks} chunks by tick {tick}");
                break;
            }
        }

        assert!(
            anchor.is_some(),
            "the terrain anchor must be broadcast locally"
        );
        assert!(
            total_chunks >= MIN_CHUNKS,
            "expected ≥ {MIN_CHUNKS} chunks streamed to the local client, got {total_chunks}"
        );
        assert!(
            decoded_ok,
            "at least one streamed chunk must decode to a TerrainChunk"
        );
    }

    // --- EM-3.7 entity mirror ---------------------------------------------

    /// EM-3.8: `NetBody` now carries the FULL `Body`, so the mirror replicates
    /// it verbatim (a plain copy) — the client resolves species/body_type. This
    /// pins the round-trip: the same `Body` we mirror comes back equal.
    #[test]
    fn net_body_carries_the_full_body() {
        let pig: comp::Body = comp::quadruped_small::Body {
            species: comp::quadruped_small::Species::Pig,
            body_type: comp::quadruped_small::BodyType::Female,
        }
        .into();
        let net = NetBody(pig);
        assert_eq!(net.0, pig, "the mirror replicates the exact Body");
        match net.0 {
            comp::Body::QuadrupedSmall(b) => {
                assert_eq!(b.species, comp::quadruped_small::Species::Pig);
                assert_eq!(b.body_type, comp::quadruped_small::BodyType::Female);
            },
            other => panic!("expected a QuadrupedSmall body, got {other:?}"),
        }
    }

    /// EM-3.8d: the loadout projection reads the figure-relevant equipped items
    /// off a real `Inventory` and maps them to the right `NetLoadout` keys —
    /// the weapon as `NetToolKey::Tool(id)` with its `ToolKind`/`Hands`,
    /// and each armour slot as its item-definition-id string. Builds the
    /// same starter kit the embedded player carries. Needs the asset tree
    /// (item defs).
    #[test]
    #[ignore = "loads real item defs: needs the asset tree; run with XINDELER_ASSETS"]
    fn net_loadout_reads_equipped_gear() {
        use common::comp::tool::{Hands, ToolKind};

        let body: comp::Body = comp::humanoid::Body {
            species: comp::humanoid::Species::Human,
            body_type: comp::humanoid::BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
        }
        .into();
        let inventory = humanoid_test_inventory(body);
        let loadout = net_loadout_from_inventory(&inventory, false);

        let tool = loadout
            .active_tool
            .expect("mainhand starter sword equipped");
        assert_eq!(
            tool.key,
            NetToolKey::Tool("common.items.weapons.sword.starter".to_owned())
        );
        assert_eq!(tool.kind, ToolKind::Sword);
        assert_eq!(tool.hands, Hands::Two);
        assert_eq!(
            loadout.chest.as_deref(),
            Some("common.items.armor.misc.chest.worker_purple_brown")
        );
        assert_eq!(
            loadout.pants.as_deref(),
            Some("common.items.armor.misc.pants.worker_brown")
        );
        assert_eq!(
            loadout.foot.as_deref(),
            Some("common.items.armor.misc.foot.sandals")
        );
        assert_eq!(
            loadout.lantern.as_deref(),
            Some("common.items.lantern.black_0")
        );
        // EM-3.8e: the helmet + glider items resolve too.
        assert_eq!(
            loadout.head.as_deref(),
            Some("common.items.armor.mail.bronze.head")
        );
        assert_eq!(
            loadout.glider.as_deref(),
            Some("common.items.glider.basic_white")
        );
        // Slots we didn't equip stay empty (figure uses the manifest default).
        assert_eq!(loadout.belt, None);
        assert_eq!(loadout.shoulder, None);
        assert_eq!(loadout.second_tool, None);
        // `gliding` passes straight through from the caller's argument.
        assert!(!loadout.gliding);
        let gliding_loadout = net_loadout_from_inventory(&inventory, true);
        assert!(gliding_loadout.gliding);
    }

    /// EM-3.8e (no assets): `is_gliding` matches voxygen's own glider-mesh
    /// gating — both `Glide` and `GlideWield` show the glider, everything
    /// else (including no `CharacterState` at all) doesn't. `Glide` is
    /// positive-tested with a real value (its `Data::new` constructor is
    /// public); `GlideWield::Data` has no public constructor outside a full
    /// `JoinData` (sim-tick context, not constructible in a unit test) — its
    /// `true` branch is exercised by `is_gliding` delegating to
    /// `CharacterState::is_glide_wielded`, which is unit-tested in
    /// `common::comp::character_state` instead.
    #[test]
    fn is_gliding_matches_glide_and_glide_wield_only() {
        assert!(!is_gliding(None));
        assert!(!is_gliding(Some(&comp::CharacterState::Idle(
            Default::default()
        ))));
        assert!(!is_gliding(Some(&comp::CharacterState::Sit)));
        assert!(is_gliding(Some(&comp::CharacterState::Glide(
            common::states::glide::Data::new(1.0, 1.0, comp::Ori::default())
        ))));
    }

    // --- EM-4.2f: AURORA-readiness overlay (no assets, no sim) -------------

    /// The core Offline-fallback invariant (spec §1.5): with
    /// `AiExecutionMode::Offline`, `recompute_aurora_overlay` clears the map
    /// unconditionally, even if live NPCs are present — Offline never gets
    /// entries.
    #[test]
    fn aurora_overlay_stays_empty_while_offline() {
        let mut overlay = AuroraOverlay::default();
        let mut scratch = std::collections::HashSet::new();
        recompute_aurora_overlay(
            AiExecutionMode::Offline,
            &mut overlay,
            [1, 2, 3].into_iter(),
            &mut scratch,
        );
        assert!(
            overlay.0.is_empty(),
            "Offline must never populate AuroraOverlay"
        );
    }

    /// Outside Offline, every live NPC uid gets exactly one entry, and that
    /// entry equals the documented neutral placeholder byte-for-byte (spec
    /// §1.5): empty memory, Idle-only intention at weight 1.0, neutral mood
    /// at zero intensity.
    #[test]
    fn aurora_overlay_populates_neutral_defaults_outside_offline() {
        for mode in [AiExecutionMode::LocalOnly, AiExecutionMode::Full] {
            let mut overlay = AuroraOverlay::default();
            let mut scratch = std::collections::HashSet::new();
            recompute_aurora_overlay(mode, &mut overlay, [10, 20].into_iter(), &mut scratch);

            assert_eq!(
                overlay.0.len(),
                2,
                "one entry per mirrored NPC under {mode:?}"
            );
            for uid in [10, 20] {
                let state = overlay
                    .0
                    .get(&uid)
                    .unwrap_or_else(|| panic!("expected an entry for NPC {uid} under {mode:?}"));
                assert_eq!(state, &AuroraNpcState::default());
                assert!(state.short_term_memory.is_empty());
                assert_eq!(state.intention, vec![(
                    xindeler_protocol::IntentKind::Idle,
                    1.0
                )]);
                assert_eq!(
                    state.emotional_state.mood,
                    xindeler_protocol::MoodKind::Neutral
                );
                assert_eq!(state.emotional_state.intensity, 0.0);
            }
        }
    }

    /// An entry that already carries non-default content (standing in for a
    /// future BL-83 write) must survive a later recompute tick untouched —
    /// this function only decides existence, never overwrites (spec §1.5:
    /// "not a computation", never stomps real data back to neutral).
    #[test]
    fn aurora_overlay_never_overwrites_an_existing_entry() {
        let mut overlay = AuroraOverlay::default();
        let mut scratch = std::collections::HashSet::new();
        recompute_aurora_overlay(
            AiExecutionMode::LocalOnly,
            &mut overlay,
            [7].into_iter(),
            &mut scratch,
        );

        // Simulate a future BL-83 write.
        let mut seeded = AuroraNpcState::default();
        seeded
            .short_term_memory
            .push("something happened".to_owned());
        overlay.0.insert(7, seeded.clone());

        // Recompute again with the SAME live set — must not reset entry 7.
        recompute_aurora_overlay(
            AiExecutionMode::LocalOnly,
            &mut overlay,
            [7].into_iter(),
            &mut scratch,
        );
        assert_eq!(overlay.0.get(&7), Some(&seeded));
    }

    /// An NPC that stops being live (despawned/left view) has its overlay
    /// entry pruned — the map must never describe an NPC that no longer
    /// exists.
    #[test]
    fn aurora_overlay_prunes_npcs_no_longer_live() {
        let mut overlay = AuroraOverlay::default();
        let mut scratch = std::collections::HashSet::new();
        recompute_aurora_overlay(
            AiExecutionMode::Full,
            &mut overlay,
            [1, 2].into_iter(),
            &mut scratch,
        );
        assert_eq!(overlay.0.len(), 2);

        // NPC 2 is gone this tick.
        recompute_aurora_overlay(
            AiExecutionMode::Full,
            &mut overlay,
            [1].into_iter(),
            &mut scratch,
        );
        assert_eq!(overlay.0.len(), 1);
        assert!(overlay.0.contains_key(&1));
        assert!(!overlay.0.contains_key(&2));
    }

    /// Toggling the mode back to Offline mid-session clears any placeholder
    /// (or seeded) entries — Offline always means zero entries, regardless of
    /// prior state.
    #[test]
    fn aurora_overlay_clears_on_toggle_back_to_offline() {
        let mut overlay = AuroraOverlay::default();
        let mut scratch = std::collections::HashSet::new();
        recompute_aurora_overlay(
            AiExecutionMode::Full,
            &mut overlay,
            [1, 2].into_iter(),
            &mut scratch,
        );
        assert_eq!(overlay.0.len(), 2);

        recompute_aurora_overlay(
            AiExecutionMode::Offline,
            &mut overlay,
            [1, 2].into_iter(),
            &mut scratch,
        );
        assert!(overlay.0.is_empty());
    }

    /// System-level wiring check (no real sim needed): [`tick_aurora_overlay`]
    /// reads `NetUid`-carrying Bevy entities directly, excludes the one
    /// tagged `NetLocalPlayer`, and reacts live to `AiExecutionMode` changing
    /// resource value between ticks — proving the acceptance bar ("toggling
    /// AiExecutionMode asserts AuroraOverlay's population state") end-to-end
    /// through the real system, not just the pure function.
    #[test]
    fn tick_aurora_overlay_system_respects_mode_and_local_player_exclusion() {
        let mut app = App::new();
        app.insert_resource(AiExecutionMode::Offline)
            .init_resource::<AuroraOverlay>()
            .init_resource::<AuroraScratch>()
            .add_systems(Update, tick_aurora_overlay);

        app.world_mut().spawn(NetUid(100));
        app.world_mut().spawn((NetUid(200), NetLocalPlayer));

        app.update();
        assert!(
            app.world().resource::<AuroraOverlay>().0.is_empty(),
            "Offline must not populate the overlay even with mirrored entities present"
        );

        *app.world_mut().resource_mut::<AiExecutionMode>() = AiExecutionMode::LocalOnly;
        app.update();
        let overlay = app.world().resource::<AuroraOverlay>();
        assert_eq!(
            overlay.0.len(),
            1,
            "only the non-NetLocalPlayer NetUid entity should get an entry"
        );
        assert!(overlay.0.contains_key(&100));
        assert!(
            !overlay.0.contains_key(&200),
            "the local player must never get an AuroraNpcState entry"
        );
        assert_eq!(overlay.0.get(&100), Some(&AuroraNpcState::default()));

        *app.world_mut().resource_mut::<AiExecutionMode>() = AiExecutionMode::Offline;
        app.update();
        assert!(
            app.world().resource::<AuroraOverlay>().0.is_empty(),
            "toggling back to Offline must clear the overlay"
        );
    }

    /// The sim→Bevy position rotation matches the voxel converter's
    /// `(x, y, z) → (x, z, −y)` (no assets).
    #[test]
    fn sim_pos_maps_z_up_to_y_up() {
        let bevy = sim_pos_to_bevy(vek::Vec3::new(100.0, 200.0, 50.0));
        assert_eq!(bevy, Vec3::new(100.0, 50.0, -200.0));
    }

    /// A yaw about the sim's z-axis becomes a yaw about Bevy's y-axis, so an
    /// entity facing sim-north (+y) faces Bevy −z, and a +x-facing look_vec is
    /// preserved (no assets).
    #[test]
    fn sim_ori_rotates_frame() {
        // A 90° yaw about z (sim up). vek Quaternion (x,y,z,w).
        let half = core::f32::consts::FRAC_PI_2;
        let (s, c) = (half / 2.0).sin_cos();
        let sim_yaw = vek::Quaternion {
            x: 0.0,
            y: 0.0,
            z: s,
            w: c,
        };
        let bevy = sim_ori_to_bevy(sim_yaw);
        // The mapped rotation must be a yaw about Bevy's +y (up): rotating +x by
        // it stays in the xz-plane (y component ~0).
        let rotated = bevy * Vec3::X;
        assert!(
            rotated.y.abs() < 1e-5,
            "a sim yaw must map to a Bevy yaw (stays horizontal), got {rotated:?}"
        );
    }

    /// EM-4.2d drift guard: `xindeler_protocol::interest`'s hand-duplicated
    /// `CHUNK_FUZZ` (kept out of that crate's public API, exposed only via
    /// `chunk_fuzz()`, per its own doc comment's rationale for NOT depending
    /// on `server`) must stay equal to the real
    /// `server::presence::CHUNK_FUZZ` the legacy
    /// `server/src/sys/subscription.rs` trigger actually uses. This crate is
    /// the natural home for the guard: it already depends on both `server`
    /// and `xindeler-protocol` (unlike either of those, which deliberately do
    /// not depend on each other for this one constant). No assets needed.
    #[test]
    fn chunk_fuzz_matches_the_legacy_subscription_constant() {
        assert_eq!(
            xindeler_protocol::chunk_fuzz(),
            server::presence::CHUNK_FUZZ,
            "xindeler_protocol::interest's duplicated CHUNK_FUZZ has drifted from \
             server::presence::CHUNK_FUZZ — update the duplicate to match"
        );
    }

    /// EM-3.7 acceptance (server → client): boot the REAL sim, add the entity
    /// mirror + replicon SERVER role, connect a pure replicon CLIENT App, spawn
    /// an NPC server-side, and tick until:
    ///  - a `Replicated` entity carrying `NetPos`/`NetBody` reaches the client,
    ///  - its `NetPos` sits near the spawn (mapped z-up→y-up),
    ///  - after more ticks the NPC has MOVED (AI wander) → the client `NetPos`
    ///    changed, proving live position updates flow,
    ///  - killing the mirror source (despawn all sim NPCs is hard; instead we
    ///    assert the despawn path by dropping the sim's visibility — covered by
    ///    the unit-level `seen_set` logic; here we assert spawn+move).
    ///
    /// Uses replicon's transport-less test loopback (`ServerTestAppExt`), same
    /// as the protocol crate's tests.
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with XINDELER_ASSETS"]
    fn mirrors_sim_npc_to_replicon_client() {
        use bevy_replicon::{prelude::ClientPlugin, test_app::ServerTestAppExt};

        const MAX_TICKS: u32 = 4000;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        // Server App: real sim + bridge + mirror + replicon server. NOTE: with a
        // CONNECTED client the mirror still runs — its gate is
        // `ClientState::Disconnected`, which is the SERVER app's own client
        // state (always Disconnected on a pure server), so the mirror is active.
        let mut server_app = App::new();
        server_app
            .add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
            ))
            .finish();
        // EM-3.11b: see `boots_and_ticks_100_times` — pin FixedUpdate to run
        // exactly once per `server_app.update()` so this stays a per-tick
        // loop (a tight loop with no sleep otherwise starves the default
        // real-time accumulator, which would make FixedUpdate — and thus the
        // whole sim — never advance).
        server_app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        server_app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));

        // Pure client App: replicon client + the shared protocol (no sim).
        let mut client_app = App::new();
        client_app
            .add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ClientPlugin))
            .add_plugins(XindelerProtocolPlugin)
            .finish();

        server_app.insert_non_send(sim);

        // An NPC spawned in UNGENERATED terrain is culled by the sim, so we
        // must keep chunks loaded around the spawn: create a centered persister
        // (same anchor the terrain stream uses) and wait until ground has
        // generated under the world centre before spawning.
        {
            let mut sim = server_app.world_mut().non_send_mut::<SimServer>();
            sim.server.create_centered_persister(server::MIN_VD);
        }
        // Let the async worldgen produce chunks around the anchor.
        for _ in 0..800 {
            server_app.update();
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre_chunk = vek::Vec2::new(size.x as i32, size.y as i32) / 2;
            if sim
                .server
                .state()
                .terrain()
                .get_key_arc(centre_chunk)
                .is_some()
            {
                break;
            }
        }
        let centre = {
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre = vek::Vec2::new(size.x as f32, size.y as f32) * 32.0 * 0.5;
            let alt = sim
                .server
                .world()
                .sim()
                .get_alt_approx(centre.map(|e| e as i32))
                .unwrap_or(0.0);
            sim.spawn_wandering_npc(vek::Vec3::new(centre.x, centre.y, alt + 3.0), 0);
            eprintln!("spawned NPC at sim centre {centre:?} alt {alt}");
            centre
        };

        server_app.connect_client(&mut client_app);

        // EM-4.2d: `mirror_sim_entities` now writes a `RegionKey` onto every
        // mirrored entity (including this test's wandering NPC), and
        // replicon's `RegionKey`/`ClientVisibleRegions` visibility filter
        // defaults an entity to HIDDEN for any client with no matching
        // `ClientVisibleRegions` — see `xindeler_protocol::visibility`'s doc
        // comment. This test predates login/interest management and only
        // wants to prove the mirror itself replicates a live NPC, so it
        // manually grants the test client a generous region window around
        // the NPC's spawn point (mirrors what EM-4.2c's future login system
        // would eventually compute from a real player's own position).
        {
            use bevy_replicon::prelude::ConnectedClient;
            let client_entity = server_app
                .world_mut()
                .query_filtered::<Entity, bevy::ecs::query::With<ConnectedClient>>()
                .single(server_app.world())
                .expect("exactly one connected client after connect_client");
            let centre_region =
                region_key_for_pos(DimensionId::default(), vek::Vec2::new(centre.x, centre.y));
            let margin = 3; // generous: the wandering NPC never roams this far.
            let regions = (-margin..=margin).flat_map(|dx| {
                (-margin..=margin).map(move |dy| {
                    vek::Vec2::new(centre_region.region.x + dx, centre_region.region.y + dy)
                })
            });
            server_app.world_mut().entity_mut(client_entity).insert(
                xindeler_protocol::ClientVisibleRegions::from_regions(
                    DimensionId::default(),
                    regions,
                ),
            );
        }

        // First-seen client position per replicated entity (client Entity ids
        // are stable across ticks). We assert SOME entity changed position,
        // proving live NetPos updates flow through the mirror.
        let mut first_seen: std::collections::HashMap<Entity, Vec3> =
            std::collections::HashMap::new();
        let mut any_reached = false;
        let mut moved = false;
        for tick in 0..MAX_TICKS {
            server_app.update();
            server_app.exchange_with_client(&mut client_app);
            client_app.update();

            // NetPos+NetBody together only appear on our mirror entities.
            let mut q = client_app
                .world_mut()
                .query::<(Entity, &NetPos, &NetBody)>();
            let samples: Vec<(Entity, Vec3)> = q
                .iter(client_app.world())
                .map(|(e, p, _)| (e, p.0))
                .collect();
            for (entity, pos) in samples {
                any_reached = true;
                match first_seen.get(&entity).copied() {
                    None => {
                        first_seen.insert(entity, pos);
                    },
                    Some(p0) => {
                        if pos.distance(p0) > 0.05 {
                            moved = true;
                            eprintln!(
                                "mirror: replica {entity:?} moved {:.3} m by tick {tick} ({p0:?} \
                                 → {pos:?})",
                                pos.distance(p0)
                            );
                            break;
                        }
                    },
                }
            }
            if moved {
                break;
            }
        }

        assert!(
            any_reached,
            "at least one sim entity must be mirrored to the client"
        );
        assert!(
            moved,
            "a mirrored entity's NetPos must update as the sim moves it (interpolation source)"
        );

        // EM-4.5: the mirrored NPC must have registered as a dimension-0
        // occupant (`mirror_sim_entities`'s `try_add_occupant` call) — this
        // is what makes `begin_draining(DimensionId::DEFAULT)` a REAL
        // "existing players may finish/leave normally" transition instead of
        // always seeing zero occupants and skipping straight to `Teardown`.
        let occupants = server_app
            .world()
            .resource::<DimensionRegistry>()
            .get(DimensionId::DEFAULT)
            .expect("ensure_default_dimension should have wrapped dimension 0 by now")
            .occupant_count();
        assert!(
            occupants >= 1,
            "the mirrored NPC should count as a dimension-0 occupant, got {occupants}"
        );
    }

    /// BL-82 EM-4.7 end-to-end acceptance: a `.entity_template.ron`-shaped
    /// `EntityTemplate` (body/stats/faction/loot/`ai_behavior_override`),
    /// spawned via `spawn_entity_template`, becomes a REAL sim NPC — through
    /// the exact chain this task's checklist demands:
    /// `spawn_entity_template` (Bevy staging entity) →
    /// `apply_pending_entity_template_spawns` (this crate's adapter, sim's
    /// public event bus) → `mirror_sim_entities` (the EXISTING, unmodified
    /// mirror) → a client-visible `NetBody`/`NetUid` entity — proving the
    /// "verbatim reuse of EM-3.8's figure pipeline" claim structurally: the
    /// client-visible shape is indistinguishable from
    /// `spawn_test_npcs`'s own test NPCs, which the figure pipeline already
    /// renders. `Agent`/`Psyche` presets are asserted directly on the sim
    /// entity (the concrete, checkable proof that `ai_behavior_override`
    /// really tunes the real `server-agent` `Agent`, not a stand-in).
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with XINDELER_ASSETS"]
    fn entity_template_factory_spawns_a_real_agro_npc_and_mirrors_it() {
        use xindeler_oracle_host::entity_template::{
            EntityTemplate, EntityTemplateStats, spawn_entity_template,
        };

        const MAX_TICKS: u32 = 4000;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut server_app = App::new();
        server_app
            .add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
            ))
            .finish();
        server_app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        server_app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        server_app.insert_non_send(sim);

        // Keep ground loaded around the world centre (same preamble as
        // `mirrors_sim_npc_to_replicon_client`) so the spawned NPC doesn't
        // fall through ungenerated terrain.
        {
            let mut sim = server_app.world_mut().non_send_mut::<SimServer>();
            sim.server.create_centered_persister(server::MIN_VD);
        }
        for _ in 0..800 {
            server_app.update();
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre_chunk = vek::Vec2::new(size.x as i32, size.y as i32) / 2;
            if sim
                .server
                .state()
                .terrain()
                .get_key_arc(centre_chunk)
                .is_some()
            {
                break;
            }
        }
        let (centre, alt) = {
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre = vek::Vec2::new(size.x as f32, size.y as f32) * 32.0 * 0.5;
            let alt = sim
                .server
                .world()
                .sim()
                .get_alt_approx(centre.map(|e| e as i32))
                .unwrap_or(0.0);
            (centre, alt)
        };

        // The full checklist shape: body/stats/faction/loot/
        // ai_behavior_override, all populated, with a KNOWN-unknown
        // ai_behavior_override handled by a second spawn below.
        let aggro_template = EntityTemplate {
            entity_template_id: "test_dread_wolf".to_owned(),
            body: "wolf".to_owned(),
            stats: EntityTemplateStats {
                name: Some("Test Dread Wolf".to_owned()),
            },
            faction: "enemy".to_owned(),
            loot: Some("common.items.crafting_ing.hide.tough".to_owned()),
            ai_behavior_override: "aggro".to_owned(),
        };
        // Anti-chaos acceptance: an unrecognized `ai_behavior_override`
        // string must fall back to Passive, never panic the spawn.
        let malformed_behavior_template = EntityTemplate {
            entity_template_id: "test_malformed".to_owned(),
            body: "pig".to_owned(),
            stats: EntityTemplateStats {
                name: Some("Test Malformed Pig".to_owned()),
            },
            ai_behavior_override: "definitely_not_a_real_behavior".to_owned(),
            ..EntityTemplate::default()
        };

        {
            let mut commands = server_app.world_mut().commands();
            spawn_entity_template(
                &mut commands,
                &aggro_template,
                [centre.x, centre.y, alt + 3.0],
                xindeler_protocol::DimensionId::DEFAULT,
            );
            spawn_entity_template(
                &mut commands,
                &malformed_behavior_template,
                [centre.x + 5.0, centre.y, alt + 3.0],
                xindeler_protocol::DimensionId::DEFAULT,
            );
        }
        server_app.world_mut().flush();

        // Drive a handful of ticks so `apply_pending_entity_template_spawns`
        // (which runs in the SAME `FixedUpdate` chain as `tick_sim`) resolves
        // the pending requests into real `CreateNpcEvent`s and the sim
        // processes them.
        for _ in 0..10 {
            server_app.update();
        }

        // No staging entity should ever survive processing (win or lose).
        let leftover_pending = server_app
            .world_mut()
            .query::<&xindeler_oracle_host::entity_template::PendingEntityTemplateSpawn>()
            .iter(server_app.world())
            .count();
        assert_eq!(
            leftover_pending, 0,
            "pending entity-template spawn requests must never accumulate"
        );

        // Find our two specific spawns by their unique `stats.name` — a real
        // generated world already has its OWN ambient wildlife (world-gen
        // spawns wolves/pigs too, some with `Alignment::Enemy`), so matching
        // by `(body, alignment)` alone would be ambiguous; the name we gave
        // each template is the one unambiguous handle back to OUR entities.
        let (aggro_wolf, malformed_pig) = {
            use common::comp;
            use specs::{Join, WorldExt};

            let sim = server_app.world().non_send::<SimServer>();
            let ecs = sim.server.state().ecs();
            let agents = ecs.read_storage::<comp::Agent>();
            let bodies = ecs.read_storage::<comp::Body>();
            let stats = ecs.read_storage::<comp::Stats>();
            let alignments = ecs.read_storage::<comp::Alignment>();

            let mut aggro_wolf = None;
            let mut malformed_pig = None;
            for (agent, body, stat, alignment) in (&agents, &bodies, &stats, &alignments).join() {
                match &stat.name {
                    comp::Content::Plain(name) if name == "Test Dread Wolf" => {
                        aggro_wolf = Some((agent.clone(), *body, *alignment));
                    },
                    comp::Content::Plain(name) if name == "Test Malformed Pig" => {
                        malformed_pig = Some((agent.clone(), *body, *alignment));
                    },
                    _ => {},
                }
            }
            (aggro_wolf, malformed_pig)
        };

        let (agent, body, alignment) =
            aggro_wolf.expect("the aggro wolf template should have spawned a real sim NPC by now");
        assert!(
            matches!(body, common::comp::Body::QuadrupedMedium(_)),
            "the \"wolf\" body keyword should resolve to a QuadrupedMedium body"
        );
        assert_eq!(
            alignment,
            common::comp::Alignment::Enemy,
            "faction: \"enemy\""
        );
        assert_eq!(
            agent.psyche.aggro_dist, None,
            "the Aggro preset must skip the warn-up (aggro_no_warn)"
        );
        assert!(
            (agent.psyche.aggro_range_multiplier - 2.0).abs() < f32::EPSILON,
            "the Aggro preset must widen the aggro range"
        );
        assert_eq!(
            agent.psyche.flee_health, 0.0,
            "the Aggro preset must never flee"
        );

        // Anti-chaos acceptance: the malformed `ai_behavior_override` must
        // have spawned SOMETHING (didn't panic / silently vanish) and must
        // carry the Passive preset's signature (`aggro_range_multiplier ==
        // 0.0`), never a real Stalk/Aggro/Flee tuning.
        let (malformed_agent, ..) = malformed_pig
            .expect("the malformed-behavior template should still spawn a real sim NPC");
        assert_eq!(
            malformed_agent.psyche.aggro_range_multiplier, 0.0,
            "an unrecognized ai_behavior_override must fall back to the Passive preset, not panic \
             or silently drop the spawn"
        );
        assert_eq!(
            malformed_agent.psyche.flee_health, 0.0,
            "Passive never flees either"
        );

        // Finally, confirm the mirror picks the aggro NPC up like any other
        // — a client-visible NetBody/NetUid entity, exactly like
        // `mirrors_sim_npc_to_replicon_client` already proves for
        // `spawn_test_npcs`'s own test NPCs. No client connection is needed
        // for this: the LISTEN-SERVER's own local loopback (`ClientState::
        // Disconnected`) already runs the mirror.
        let is_mirrored_wolf_visible = |app: &mut App| {
            let mut q = app.world_mut().query::<&NetBody>();
            q.iter(app.world())
                .any(|body| matches!(body.0, common::comp::Body::QuadrupedMedium(_)))
        };
        let mut mirrored = is_mirrored_wolf_visible(&mut server_app);
        for _ in 0..MAX_TICKS {
            if mirrored {
                break;
            }
            server_app.update();
            mirrored = is_mirrored_wolf_visible(&mut server_app);
        }
        assert!(
            mirrored,
            "the factory-spawned NPC must be mirrored to a NetBody (+NetUid, EM-4.2f) entity, \
             exactly like any other sim NPC — proving EM-3.8's figure pipeline is reused verbatim"
        );
    }

    /// BL-82 EM-4.7 acceptance (task board's literal bar, `tasks/
    /// 45-engine-migration-tasks.md`: "Mist-Bound example spawns 15 clamped
    /// minions with stalker AI in a test dimension"): a `DmEvent` shaped
    /// like a Mist-Bound-style ORACLE event (`spawning_rules` drawing from
    /// the shipped `sentinel_owl` template — an already-authored "stalk"
    /// sample, see `assets/xindeler/entity_templates/
    /// sentinel_owl.entity_template.ron`) spawns exactly 15 real sim NPCs,
    /// every one carrying the Stalk preset's signature, into
    /// `DimensionId::DEFAULT` — v1's "test dimension" (this module's doc
    /// comment on `entity_factory`'s default-dimension-only scope; full
    /// per-DmEvent instanced dimensions are EM-4.9's end-to-end drill, not
    /// this task's job).
    ///
    /// The "clamped" half of the acceptance bar is exercised for real: the
    /// `DmEvent` is FIRST authored with a hostile `spawn_count` (2000, far
    /// past EM-4.4's `bounds::SPAWN_COUNT` ceiling of 200) and run through
    /// the exact same `DmEvent::sanitize` anti-chaos path every ingested
    /// event goes through, proving the clamp actually fires — only THEN
    /// does the test dial `spawn_count` down to the literal 15 the
    /// acceptance bar asks for (15 itself is not a clamp boundary — the
    /// ceiling is 200 — so this test does not pretend it is one).
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with XINDELER_ASSETS"]
    fn mist_bound_spawning_rules_spawn_fifteen_clamped_stalker_minions_in_a_test_dimension() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use xindeler_oracle_host::{
            DmEvent, SpawningRules,
            entity_template::{EntityTemplate, EntityTemplateStats},
        };

        const EXPECTED_MINIONS: usize = 15;
        const MINION_NAME: &str = "Mist-Bound Sentinel";
        // Must match `event.spawning_rules.spawn_radius` below — kept as its
        // own named constant so the terrain-readiness preamble can size its
        // wait against the SAME radius the scatter itself uses.
        const SPAWN_TEST_RADIUS: f32 = 30.0;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut server_app = App::new();
        server_app
            .add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
            ))
            .finish();
        server_app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        server_app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        server_app.insert_non_send(sim);

        // Same terrain-ready preamble as the sibling entity-factory test, but
        // strengthened: unlike that test's two FIXED spawn points (well
        // inside the centre chunk), this test's `spawn_from_spawning_rules`
        // scatters each minion up to `spawning_rules.spawn_radius` (30.0,
        // ~1 chunk) from centre, so a bare "the exact centre chunk is loaded"
        // check isn't enough — a minion landing in a not-yet-generated
        // NEIGHBOUR chunk hits the exact same same-tick-deletion class the
        // EM-3.11o fix (`chunk_anchor_at`'s doc comment) already documents
        // for wandering test NPCs: `Server::tick`'s entity-cleanup phase
        // deletes it before this test (or `mirror_sim_entities`) ever
        // observes it, undercounting `minions.len()` under CPU contention
        // (e.g. running back-to-back with this crate's other boot-a-real-
        // world tests) where the async worldgen workers lag behind the
        // centre chunk's own completion. So wait for EVERY chunk the scatter
        // radius could possibly touch, not just the one at dead centre.
        {
            let mut sim = server_app.world_mut().non_send_mut::<SimServer>();
            sim.server.create_centered_persister(server::MIN_VD);
        }
        const CHUNK_SIZE: f32 = 32.0;
        let scatter_chunk_radius =
            (SPAWN_TEST_RADIUS / CHUNK_SIZE).ceil() as i32 + 1 /* margin */;
        for _ in 0..800 {
            server_app.update();
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre_chunk = vek::Vec2::new(size.x as i32, size.y as i32) / 2;
            let terrain = sim.server.state().terrain();
            let all_touched_chunks_loaded =
                (-scatter_chunk_radius..=scatter_chunk_radius).all(|dx| {
                    (-scatter_chunk_radius..=scatter_chunk_radius).all(|dy| {
                        terrain
                            .get_key_arc(centre_chunk + vek::Vec2::new(dx, dy))
                            .is_some()
                    })
                });
            drop(terrain);
            if all_touched_chunks_loaded {
                break;
            }
        }
        let (centre, alt) = {
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre = vek::Vec2::new(size.x as f32, size.y as f32) * 32.0 * 0.5;
            let alt = sim
                .server
                .world()
                .sim()
                .get_alt_approx(centre.map(|e| e as i32))
                .unwrap_or(0.0);
            (centre, alt)
        };

        // The "Mist-Bound example": a DmEvent whose spawning_rules directive
        // names the shipped `sentinel_owl` template, a uniform "stalk"
        // override (every minion stalks regardless of the template's own
        // default), and a deliberately hostile spawn_count.
        let mut event = DmEvent {
            spawning_rules: SpawningRules {
                entity_templates: vec!["sentinel_owl".to_owned()],
                spawn_count: 2000.0,
                spawn_radius: SPAWN_TEST_RADIUS,
                ai_behavior_override: "stalk".to_owned(),
            },
            ..DmEvent::default()
        };
        event.sanitize();
        assert!(
            event.spawning_rules.spawn_count <= 200.0,
            "a hostile spawn_count must be clamped by the same EM-4.4 anti-chaos bound every \
             DmEvent gets, BEFORE this test ever calls spawn_from_spawning_rules: got {}",
            event.spawning_rules.spawn_count
        );
        // Now dial in the literal count the acceptance bar names.
        event.spawning_rules.spawn_count = EXPECTED_MINIONS as f32;

        // Mirrors the SHIPPED `sentinel_owl.entity_template.ron` asset field
        // for field (`assets/xindeler/entity_templates/
        // sentinel_owl.entity_template.ron`: body/faction identical) except
        // `ai_behavior_override`, deliberately set to "flee" here (not the
        // shipped asset's "stalk") to prove the batch spawn's
        // `spawning_rules.ai_behavior_override` OVERRIDES the template's own
        // value rather than merely reading it.
        let mut templates = HashMap::new();
        templates.insert("sentinel_owl".to_owned(), EntityTemplate {
            entity_template_id: "sentinel_owl".to_owned(),
            body: "snowy_owl".to_owned(),
            stats: EntityTemplateStats {
                name: Some(MINION_NAME.to_owned()),
            },
            faction: "wild".to_owned(),
            loot: None,
            ai_behavior_override: "flee".to_owned(),
        });

        let mut rng = ChaCha8Rng::seed_from_u64(0xBADD_C0DE);
        {
            let mut commands = server_app.world_mut().commands();
            spawn_from_spawning_rules(
                &mut commands,
                &templates,
                &event.spawning_rules,
                [centre.x, centre.y, alt + 3.0],
                xindeler_protocol::DimensionId::DEFAULT,
                &mut rng,
            );
        }
        server_app.world_mut().flush();

        for _ in 0..10 {
            server_app.update();
        }

        // No staging entity should ever survive processing.
        let leftover_pending = server_app
            .world_mut()
            .query::<&xindeler_oracle_host::entity_template::PendingEntityTemplateSpawn>()
            .iter(server_app.world())
            .count();
        assert_eq!(
            leftover_pending, 0,
            "pending entity-template spawn requests must never accumulate"
        );

        let minions = {
            use common::comp;
            use specs::{Join, WorldExt};

            let sim = server_app.world().non_send::<SimServer>();
            let ecs = sim.server.state().ecs();
            let agents = ecs.read_storage::<comp::Agent>();
            let bodies = ecs.read_storage::<comp::Body>();
            let stats = ecs.read_storage::<comp::Stats>();
            let alignments = ecs.read_storage::<comp::Alignment>();

            (&agents, &bodies, &stats, &alignments)
                .join()
                .filter_map(|(agent, body, stat, alignment)| match &stat.name {
                    comp::Content::Plain(name) if name == MINION_NAME => {
                        Some((agent.clone(), *body, *alignment))
                    },
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            minions.len(),
            EXPECTED_MINIONS,
            "the Mist-Bound example's spawning_rules must spawn exactly {EXPECTED_MINIONS} real \
             sim NPCs, no more, no fewer"
        );

        for (agent, body, alignment) in &minions {
            assert_eq!(
                *alignment,
                common::comp::Alignment::Wild,
                "every minion's faction must be \"wild\", per the template (matching the shipped \
                 sentinel_owl.entity_template.ron asset)"
            );
            // Stalk is body-derived defaults, untouched (see `AgentPreset::
            // build_agent`'s doc comment) — assert it matches THIS body's
            // own baseline, not a hardcoded number, so the check holds
            // regardless of species-specific Agent::from_body defaults.
            let baseline = common::comp::Agent::from_body(body);
            assert_eq!(
                agent.psyche.aggro_range_multiplier, baseline.psyche.aggro_range_multiplier,
                "every minion must carry the Stalk preset (spawning_rules.ai_behavior_override \
                 overriding the template's own \"flee\"), not Passive/Aggro/Flee"
            );
            assert_eq!(
                agent.psyche.flee_health, baseline.psyche.flee_health,
                "Stalk must not carry Flee's flee_health override (the template itself said \
                 \"flee\" — spawning_rules must have overridden it)"
            );
            assert_ne!(
                agent.psyche.flee_health, 1.0,
                "if this were still the template's own \"flee\" preset, flee_health would be 1.0 \
                 — spawning_rules.ai_behavior_override must have overridden it to \"stalk\""
            );
        }
    }

    /// BL-82 EM-4.9 (Phase C, T51.6): a factory batch targeted at a REAL
    /// non-default `DimensionId` — not the DEFAULT-only v1 scope EM-4.7
    /// shipped with — is (a) actually spawned (the factory sink no longer
    /// drops it), (b) its MIRROR entities are tagged with THAT dimension,
    /// not `DimensionId::DEFAULT` (proving `mirror_sim_entities`'s
    /// attribution mechanism — `PendingDimensionAttribution`/
    /// `SimEntityDimension` — works end to end), and (c) admin-draining the
    /// event dimension actually reaches `Teardown` and deletes the sim-side
    /// minions, even though nothing ever "logs out" of it (see
    /// `release_dimension_occupants_on_drain_request`'s doc comment for why
    /// that needs its own fix — an NPC-only dimension has no natural
    /// occupant-departure trigger).
    #[test]
    #[ignore = "boots a real world + spins up a second real dimension: needs assets + LFS; run \
                locally with XINDELER_ASSETS"]
    fn entity_factory_routes_into_a_real_non_default_dimension_and_cleans_up_on_retire() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use xindeler_dimensions::{
            DimensionSpinupConfig, DrainDimension, SpinupDimension, WorldGenThreadPool,
        };
        use xindeler_oracle_host::{
            dm_event::SpawningRules,
            entity_template::{EntityTemplate, EntityTemplateStats},
        };

        const EVENT_DIMENSION: DimensionId = DimensionId(7);
        const EXPECTED_MINIONS: usize = 5;
        const MINION_NAME: &str = "Mist-Bound Routing Test Shade";

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");
        let thread_pool = Arc::clone(sim.server.state().thread_pool());

        let mut server_app = App::new();
        server_app
            .add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
            ))
            .finish();
        server_app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        server_app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        // A REAL spinup needs a real `WorldGenThreadPool` (the sim's own,
        // matching `xindeler-server-app::dimensions::install_default_
        // dimension`'s own reuse — no second pool is built).
        server_app.insert_resource(WorldGenThreadPool(thread_pool));
        server_app.insert_non_send(sim);

        // Terrain-ready preamble (same shape the Mist-Bound test uses):
        // minions still physically live in the ONE real terrain (see the
        // module's `entity_factory` doc comment for why), so wait for the
        // centre chunk before ever spawning anything.
        {
            let mut sim = server_app.world_mut().non_send_mut::<SimServer>();
            sim.server.create_centered_persister(server::MIN_VD);
        }
        for _ in 0..800 {
            server_app.update();
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre_chunk = vek::Vec2::new(size.x as i32, size.y as i32) / 2;
            if sim
                .server
                .state()
                .terrain()
                .get_key_arc(centre_chunk)
                .is_some()
            {
                break;
            }
        }
        let (centre, alt) = {
            let sim = server_app.world().non_send::<SimServer>();
            let size = sim.server.world().sim().get_size();
            let centre = vek::Vec2::new(size.x as f32, size.y as f32) * 32.0 * 0.5;
            let alt = sim
                .server
                .world()
                .sim()
                .get_alt_approx(centre.map(|e| e as i32))
                .unwrap_or(0.0);
            (centre, alt)
        };

        // Spin up a SECOND, real dimension — same mechanism the debug-command
        // path (`xindeler-server-app::dimensions`) drives, to `Active`.
        server_app.world_mut().write_message(SpinupDimension {
            id: EVENT_DIMENSION,
            base_seed: 0,
            config: DimensionSpinupConfig::default(),
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            server_app.update();
            if server_app
                .world()
                .resource::<DimensionRegistry>()
                .lifecycle(EVENT_DIMENSION)
                == Some(DimensionLifecycle::Active)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the event dimension never reached Active within the boot deadline"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        // A small batch of Agent-bearing minions, targeted at the EVENT
        // dimension (NOT `DimensionId::DEFAULT`).
        let mut templates = HashMap::new();
        templates.insert("sentinel_owl".to_owned(), EntityTemplate {
            entity_template_id: "sentinel_owl".to_owned(),
            body: "snowy_owl".to_owned(),
            stats: EntityTemplateStats {
                name: Some(MINION_NAME.to_owned()),
            },
            faction: "enemy".to_owned(),
            loot: None,
            ai_behavior_override: "aggro".to_owned(),
        });
        let rules = SpawningRules {
            entity_templates: vec!["sentinel_owl".to_owned()],
            spawn_count: EXPECTED_MINIONS as f32,
            spawn_radius: 10.0,
            ai_behavior_override: "aggro".to_owned(),
        };
        let mut rng = ChaCha8Rng::seed_from_u64(0xF00D_CAFE);
        {
            let mut commands = server_app.world_mut().commands();
            spawn_from_spawning_rules(
                &mut commands,
                &templates,
                &rules,
                [centre.x, centre.y, alt + 3.0],
                EVENT_DIMENSION,
                &mut rng,
            );
        }
        server_app.world_mut().flush();

        for _ in 0..10 {
            server_app.update();
        }

        // (a) actually spawned + (b) mirror-tagged with the EVENT dimension,
        // not DEFAULT. `With<SimEntity>` excludes the dimension's own ROOT
        // entity (`handle_spinup_requests` tags the root itself with a bare
        // `DimensionId` too — it is not one of the 5 minion mirrors).
        let event_tagged_count = server_app
            .world_mut()
            .query_filtered::<&DimensionId, With<SimEntity>>()
            .iter(server_app.world())
            .filter(|id| **id == EVENT_DIMENSION)
            .count();
        assert_eq!(
            event_tagged_count, EXPECTED_MINIONS,
            "all {EXPECTED_MINIONS} minions' mirror entities must be tagged with the EVENT \
             dimension, not DimensionId::DEFAULT — got {event_tagged_count}"
        );
        let region_dimensions_match: bool = server_app
            .world_mut()
            .query_filtered::<(&DimensionId, &xindeler_protocol::RegionKey), With<SimEntity>>()
            .iter(server_app.world())
            .filter(|(id, _)| **id == EVENT_DIMENSION)
            .all(|(_, region)| region.dimension == EVENT_DIMENSION);
        assert!(
            region_dimensions_match,
            "every EVENT-dimension-tagged mirror's RegionKey must ALSO carry the event dimension \
             (interest management scoping falls out of this for free — see \
             xindeler_protocol::visibility)"
        );
        assert_eq!(
            server_app
                .world()
                .resource::<DimensionRegistry>()
                .get(EVENT_DIMENSION)
                .expect("still Active/registered")
                .occupant_count(),
            EXPECTED_MINIONS,
            "every minion must count as an occupant of the EVENT dimension (this is what makes \
             the retire-and-teardown half below a REAL exit condition, not an always-empty one)"
        );

        // (c) admin-drain the event dimension: it must actually reach
        // Teardown (not get stuck in Draining forever) and delete the
        // sim-side minions.
        server_app
            .world_mut()
            .write_message(DrainDimension(EVENT_DIMENSION));
        for _ in 0..20 {
            server_app.update();
        }

        assert!(
            server_app
                .world()
                .resource::<DimensionRegistry>()
                .get(EVENT_DIMENSION)
                .is_none(),
            "the event dimension must be fully removed from the registry after draining (Draining \
             -> Teardown -> removed), not stuck mid-lifecycle"
        );
        assert_eq!(
            server_app
                .world_mut()
                .query::<&DimensionId>()
                .iter(server_app.world())
                .filter(|id| **id == EVENT_DIMENSION)
                .count(),
            0,
            "no mirror entity may still carry the torn-down event dimension's id (zero-leak)"
        );

        let surviving_minions = {
            use common::comp;
            use specs::{Join, WorldExt};

            let sim = server_app.world().non_send::<SimServer>();
            let ecs = sim.server.state().ecs();
            let stats = ecs.read_storage::<comp::Stats>();
            (&stats)
                .join()
                .filter(|stat| matches!(&stat.name, comp::Content::Plain(n) if n == MINION_NAME))
                .count()
        };
        assert_eq!(
            surviving_minions, 0,
            "the event dimension's minions must be deleted from the sim itself on teardown, not \
             just un-mirrored (zero-leak on the sim side, not only the Bevy side)"
        );
    }

    /// Identifies the wandering test NPCs SPECIFICALLY, on the sim side, by
    /// their own `Stats.name` convention (`"Test <Body> <index>"` — see
    /// `emit_wandering_npc`/`_humanoid`/`_quadruped_medium`/`_bird_medium`),
    /// returning each one's current `comp::Pos`. Shared by
    /// [`test_npcs_survive_around_the_players_real_spawn_point`]'s count and
    /// ring-radius assertions.
    fn test_npc_positions(sim: &SimServer) -> Vec<vek::Vec3<f32>> {
        let ecs = sim.server.state().ecs();
        let stats = ecs.read_storage::<comp::Stats>();
        let positions = ecs.read_storage::<comp::Pos>();
        specs::Join::join((&stats, &positions))
            .filter(|(s, _)| {
                matches!(&s.name, common::comp::Content::Plain(n) if n.starts_with("Test "))
            })
            .map(|(_, p)| p.0)
            .collect()
    }

    /// BL-82 EM-3.11o regression: `spawn_test_npcs` used to centre its
    /// wandering-NPC ring on the world's GEOMETRIC centre (`world_size / 2`),
    /// on the assumption that was always where terrain stayed loaded. Once
    /// EM-3.7b made the terrain-anchor persister a FALLBACK (superseded by the
    /// embedded player's own `Presence`), that assumption broke: the world's
    /// own spawn-point selection routinely lands the player hundreds of
    /// blocks from the geometric centre, so nothing kept THAT area's chunks
    /// loaded. Every wandering test NPC was born in a chunk
    /// `state.terrain().get_key_real(..)` reports as unloaded; `Server::tick`'s
    /// "remove NPCs outside the view distance of all players" entity-cleanup
    /// phase deleted each one the SAME tick it was created — before
    /// `mirror_sim_entities` (or anything else client-side) ever ran. No
    /// `NetBody` mirror entity ever appeared, no matter how long a test
    /// (or a real playthrough) waited — 100% reproducible, invisible to any
    /// external observer (creation + deletion happen inside one
    /// `Server::tick()` call).
    ///
    /// Fixed by (a) centring the ring on the embedded player's REAL position
    /// (read off its sim entity) instead of the geometric centre, and
    /// (b) giving every wandering test NPC a `comp::Anchor::Chunk` at its own
    /// spawn chunk, matching what every other NPC spawn path (rtsim wildlife,
    /// `server/src/sys/terrain.rs`) already does; and, follow-up, (c) gating
    /// the whole one-shot latch on the SAME player-readiness condition
    /// `ensure_terrain_anchor` uses, instead of a bare tick count, so the
    /// latch can no longer fire while the player is still mid-connect (which
    /// reproduces the identical bug via a different trigger — see
    /// `TestNpcState::ready_since`'s doc).
    ///
    /// This test boots the REAL sim + a REAL embedded player (so the ring
    /// centres on wherever the world's own spawn-point selection actually put
    /// it — not a location the test controls), lets the boot-time
    /// `spawn_test_npcs` ring fire, and asserts the default 8 wandering NPCs
    /// (a) actually reach the Bevy World as `NetBody` entities distinct from
    /// the player's own mirror, (b) are STILL alive `SURVIVAL_TICKS` later —
    /// before the fix, (a) alone already failed (the count was always 0) —
    /// and (c) actually spawned WITHIN `TEST_NPC_RING_RADIUS` of the player's
    /// own real position at fire time, not merely "survived" (a bare survival
    /// count can't distinguish "correctly centred on the player" from "this
    /// run's player happened to reach in-game before the warmup latch fired"
    /// — the pre-fix bug only bit when the player was STILL connecting at
    /// fire time, which a fast/lucky run could dodge even with the old code).
    #[test]
    #[ignore = "boots a real world + embedded player: needs assets + LFS; run with XINDELER_ASSETS"]
    fn test_npcs_survive_around_the_players_real_spawn_point() {
        const MAX_TICKS: u32 = 6000;
        const SURVIVAL_TICKS: u32 = 300;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut sim = boot_test_server(data_dir.path()).expect("failed to boot test server");
        let player = boot_embedded_player(&mut sim).expect("failed to boot embedded player");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                SimBridgePlugin,
                SimEntityMirrorPlugin,
                PlayerBridgePlugin,
            ))
            .finish();
        // EM-3.11b: see `boots_and_ticks_100_times` — pin FixedUpdate to run
        // exactly once per `app.update()`.
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);
        app.insert_non_send(player);

        // Tick until `spawn_test_npcs` has fired (its one-shot latch), then
        // keep ticking `SURVIVAL_TICKS` further so a same-tick-of-creation
        // deletion (the exact bug) has ample time to show up as an empty
        // count, and a later, slower deletion (e.g. wandering into a still-
        // unloaded neighbour chunk) would too.
        //
        // BL-82 EM-3.11o strengthening: a bare "still alive after
        // SURVIVAL_TICKS" count can't tell "correctly centred on the player"
        // apart from "this run's player happened to reach in-game before the
        // warmup latch fired" (a pre-fix run and a post-fix run could both
        // pass that assertion on a fast machine — the OLD bug only bit when
        // the player was STILL connecting at fire time). So also capture (a)
        // the player's own real sim position at the exact tick the ring
        // fired, and (b) each wandering NPC's position `RING_CHECK_DELAY`
        // ticks later — early enough that idle-wander AI hasn't carried them
        // far — and assert every one of them actually landed within
        // `TEST_NPC_RING_RADIUS` (+ a small tolerance) of that captured
        // centre.
        const RING_CHECK_DELAY: u32 = 5;
        /// Slack (blocks) on top of `TEST_NPC_RING_RADIUS`, covering the
        /// handful of physics/AI ticks between spawn and the position sample
        /// (fall-to-ground settling, a few steps of idle wander).
        const RING_CHECK_TOLERANCE: f32 = 6.0;

        let mut fired_at: Option<u32> = None;
        let mut spawn_centre: Option<vek::Vec2<f32>> = None;
        let mut ring_positions: Option<Vec<vek::Vec3<f32>>> = None;
        for tick in 0..MAX_TICKS {
            app.update();
            if fired_at.is_none() && app.world().resource::<TestNpcState>().spawned {
                fired_at = Some(tick);
                // Read the player's real position straight off the sim ECS —
                // the same source `spawn_test_npcs`'s `centre` itself reads —
                // at the exact tick the ring was centred on it.
                let sim = app.world().non_send::<SimServer>();
                let player = app.world().non_send::<EmbeddedPlayer>();
                spawn_centre = player.uid().and_then(|uid| {
                    player::player_sim_entity(sim, uid).and_then(|e| {
                        sim.server
                            .state()
                            .ecs()
                            .read_storage::<comp::Pos>()
                            .get(e)
                            .map(|p| p.0.xy())
                    })
                });
                eprintln!("spawn_test_npcs fired at tick {tick}, ring centre {spawn_centre:?}");
            }
            if let Some(f) = fired_at
                && ring_positions.is_none()
                && tick >= f + RING_CHECK_DELAY
            {
                ring_positions = Some(test_npc_positions(app.world().non_send::<SimServer>()));
            }
            if let Some(f) = fired_at
                && tick >= f + SURVIVAL_TICKS
            {
                break;
            }
        }
        let fired_at = fired_at.expect("spawn_test_npcs never fired within MAX_TICKS");
        assert!(
            fired_at + SURVIVAL_TICKS < MAX_TICKS,
            "ran out of MAX_TICKS before completing the {SURVIVAL_TICKS}-tick survival window"
        );
        let spawn_centre = spawn_centre.expect(
            "the player's real sim position must be readable at the exact tick spawn_test_npcs \
             fired — i.e. the player must already be in-game by then. If this fails, the \
             player-readiness gate let the latch fire too early (see `spawn_test_npcs`'s \
             `player_ready`).",
        );
        let ring_positions = ring_positions
            .expect("ring position sample must have been taken within the survival window");
        let max_allowed = TEST_NPC_RING_RADIUS + RING_CHECK_TOLERANCE;
        for pos in &ring_positions {
            let dist = (pos.xy() - spawn_centre).magnitude();
            assert!(
                dist <= max_allowed,
                "a wandering test NPC was {dist:.1} blocks from the player's real spawn-time \
                 position {spawn_centre:?} (expected <= {max_allowed:.1}) — this is exactly what \
                 the EM-3.11o bug would produce: a geometric-world-centre fallback firing while \
                 the player was still connecting puts the ring far from the player instead of \
                 around it"
            );
        }

        // Identify the wandering test NPCs SPECIFICALLY, on the sim side (see
        // `test_npc_positions`'s doc for why a plain `NetBody` count on the
        // Bevy mirror is NOT enough: the player's real (non-geometric-centre)
        // spawn point almost always has ambient rtsim wildlife nearby, which
        // ALSO mirrors as `NetBody` and would make a bare "count >= 8"
        // assertion pass even if every test NPC had been deleted — this
        // exact false-positive was caught while writing this test, with the
        // fix reverted only in-memory to confirm it — never committed
        // unfixed).
        let test_named = test_npc_positions(app.world().non_send::<SimServer>()).len();
        let expected = TestNpcState::default().count as usize;
        assert_eq!(
            test_named, expected,
            "expected all {expected} wandering test NPCs to still exist in the sim \
             {SURVIVAL_TICKS} ticks after spawn (got {test_named}) — before the EM-3.11o fix \
             every one of them was deleted the SAME tick it was created (its spawn chunk was \
             never actually loaded around the player's real, non-geometric-centre spawn point),so \
             this count was always 0 regardless of how long the test waited"
        );

        // Also confirm at least that many non-player `NetBody` entities made
        // it across the sim↔Bevy mirror too (client-visible, not just alive
        // server-side) — the mirror/figure-build half of the pipeline this
        // bug hunt is about.
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, (With<NetBody>, Without<NetLocalPlayer>)>();
        let mirrored = q.iter(app.world()).count();
        assert!(
            mirrored >= expected,
            "expected at least the {expected} wandering test NPCs among the mirrored (non-player) \
             NetBody entities {SURVIVAL_TICKS} ticks after spawn (got {mirrored} total, which may \
             also include ambient wildlife)"
        );
    }

    // --- EM-4.6 (T47.8): sim-side dimension teardown -----------------------

    /// [`delete_specs_entities_for_torn_down_dimensions`]'s own acceptance:
    /// once a (non-default) dimension reaches `Teardown`, the specs entity
    /// tagged as one of its mirrors is ACTUALLY deleted from the sim through
    /// its normal delete path — never a raw storage poke.
    ///
    /// Manually tags a Bevy entity `(SimEntity, DimensionId(1))` rather than
    /// going through [`mirror_sim_entities`] (which today only ever tags
    /// `DimensionId::DEFAULT` — see this test's own in-body comment) —
    /// exactly the forward-looking scenario a future EM-4.7 spawn-into-
    /// dimension flow will produce.
    #[test]
    #[ignore = "boots a real world: needs assets; run locally with VELOREN_ASSETS=\"$(pwd)/assets\""]
    fn tearing_down_a_dimension_deletes_its_mirrored_specs_entity() {
        use specs::Builder as _;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        app.add_plugins(SimBridgePlugin);
        // EM-4.10 Finding B: `FixedUpdate`, matching the production wiring
        // (`SimEntityMirrorPlugin::build`) and `DimensionsPlugin`'s own
        // chain, which this system orders itself against below — both must
        // live in the SAME schedule for `.after`/`.before` to mean anything.
        app.add_systems(
            FixedUpdate,
            delete_specs_entities_for_torn_down_dimensions
                .after(xindeler_dimensions::spinup::handle_drain_requests)
                .after(xindeler_dimensions::predictive_gc::predictive_gc_system)
                .before(xindeler_dimensions::teardown::teardown_completed_dimensions),
        );
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);

        // A real specs NPC, created synchronously through the sim's own
        // public `StateExt::create_npc` (the SAME entry point
        // `server::cmd`'s admin `/spawn` command uses) — not the
        // event-based `CreateNpcEvent` path this crate's other tests use,
        // so the concrete `specs::Entity` is known immediately rather than
        // needing to scan for it after an async event is processed.
        let specs_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let body: comp::Body = comp::quadruped_small::Body {
                species: comp::quadruped_small::Species::Pig,
                body_type: comp::quadruped_small::BodyType::Female,
            }
            .into();
            sim.server
                .state_mut()
                .create_npc(
                    comp::Pos(vek::Vec3::new(0.0, 0.0, 0.0)),
                    comp::Ori::default(),
                    comp::Stats::new(comp::Content::Plain("Teardown Test Pig".to_string()), body),
                    comp::SkillSet::default(),
                    Some(comp::Health::new(body)),
                    comp::Poise::new(body),
                    comp::Inventory::with_empty(),
                    body,
                    body.scale(),
                )
                .build()
        };
        app.update();

        {
            let sim = app.world().non_send::<SimServer>();
            assert!(
                sim.server.state().ecs().entities().is_alive(specs_entity),
                "the synthetic NPC should be alive before teardown"
            );
        }

        // Manually tag a Bevy mirror entity as belonging to a SECOND
        // dimension — today's `mirror_sim_entities` only ever tags
        // `DimensionId::DEFAULT` (see its own doc comment); this stands in
        // for a future EM-4.7 spawn-into-dimension flow.
        let bevy_mirror = app
            .world_mut()
            .spawn((SimEntity(specs_entity), DimensionId(1)))
            .id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(specs_entity, bevy_mirror);

        // Spin dimension 1 up via the registry's direct API (fast path — no
        // real procgen needed for a test that's about sim-entity deletion,
        // not terrain) and drive it straight to `Teardown` (zero
        // REGISTRY-tracked occupants, a separate bookkeeping concept from
        // the `SimEntity` tag above — see `DimensionRegistry::
        // begin_draining`'s own "already-empty dimension tears down
        // immediately" documented behavior).
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            let root = Entity::from_raw_u32(9001).expect("small test entity id");
            registry
                .insert_spinning_up(DimensionId(1), root, 0)
                .unwrap();
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(DimensionId(1), std::sync::Arc::new(world), index)
                .unwrap();
            registry
                .begin_draining(DimensionId(1))
                .expect("Active -> Draining is legal");
        }

        // A few ticks: `delete_specs_entities_for_torn_down_dimensions` runs
        // BEFORE `teardown_completed_dimensions` removes the registry entry,
        // and `tick_sim`'s own `sim.server.cleanup()` (called every
        // `FixedUpdate` tick) lets specs fully process the deletion.
        for _ in 0..10 {
            app.update();
        }

        let sim = app.world().non_send::<SimServer>();
        assert!(
            !sim.server.state().ecs().entities().is_alive(specs_entity),
            "the torn-down dimension's mirrored specs entity should have been deleted through the \
             sim's normal delete path (StateExt::delete_entity_recorded)"
        );
    }

    /// Regression test for the bevy-migration-reviewer's BLOCKER finding on
    /// the first version of this system: it must not lose a mirrored specs
    /// entity when a dimension flips `Active -> Teardown` in the SAME tick
    /// [`delete_specs_entities_for_torn_down_dimensions`] runs in. (Function
    /// name kept as "frame" for git-history continuity; EM-4.10 Finding B
    /// moved the whole chain from `Update`/render-frame cadence to
    /// `FixedUpdate`/sim-tick cadence — the race this test proves is now a
    /// same-TICK race, not a same-frame one, but it's the identical race.)
    ///
    /// Unlike [`tearing_down_a_dimension_deletes_its_mirrored_specs_entity`]
    /// above (which drives the transition directly via
    /// `DimensionRegistry::begin_draining`, entirely OUTSIDE any
    /// `app.update()`, so it never actually exercises same-tick scheduling
    /// order), this test sends a REAL [`xindeler_dimensions::DrainDimension`]
    /// message and lets [`xindeler_dimensions::spinup::handle_drain_requests`]
    /// perform the `Active -> Teardown` flip (immediate, since the dimension
    /// has zero REGISTRY-tracked occupants) inside the very same
    /// `FixedUpdate` tick this deletion system also runs in — the exact race
    /// window the reviewer identified: without an explicit `.after(..)` edge
    /// on both `handle_drain_requests` and `predictive_gc_system`, Bevy was
    /// free to schedule this system BEFORE the flip happened, see the
    /// dimension as still `Active`, skip it, and then lose the mirror tags
    /// forever to `teardown_completed_dimensions`'s same-tick cascade
    /// despawn — permanently leaking the specs entity.
    #[test]
    #[ignore = "boots a real world: needs assets; run locally with VELOREN_ASSETS=\"$(pwd)/assets\""]
    fn tearing_down_via_a_real_drain_message_in_the_same_frame_still_deletes_the_specs_entity() {
        use specs::Builder as _;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(data_dir.path()).expect("failed to boot test server");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build());
        // `SimBridgePlugin` brings in `DimensionsPlugin` (guarded add), which
        // is what actually registers `handle_drain_requests`/
        // `predictive_gc_system`/`teardown_completed_dimensions` in
        // `FixedUpdate` (EM-4.10 Finding B) — the real production wiring,
        // not a hand-picked subset.
        app.add_plugins(SimBridgePlugin);
        // EM-4.10 Finding B: `FixedUpdate`, matching production — must be
        // the SAME schedule as the systems ordered against below.
        app.add_systems(
            FixedUpdate,
            delete_specs_entities_for_torn_down_dimensions
                .after(xindeler_dimensions::spinup::handle_drain_requests)
                .after(xindeler_dimensions::predictive_gc::predictive_gc_system)
                .before(xindeler_dimensions::teardown::teardown_completed_dimensions),
        );
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));
        app.insert_non_send(sim);

        let specs_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let body: comp::Body = comp::quadruped_small::Body {
                species: comp::quadruped_small::Species::Pig,
                body_type: comp::quadruped_small::BodyType::Female,
            }
            .into();
            sim.server
                .state_mut()
                .create_npc(
                    comp::Pos(vek::Vec3::new(0.0, 0.0, 0.0)),
                    comp::Ori::default(),
                    comp::Stats::new(
                        comp::Content::Plain("Same-Frame Teardown Test Pig".to_string()),
                        body,
                    ),
                    comp::SkillSet::default(),
                    Some(comp::Health::new(body)),
                    comp::Poise::new(body),
                    comp::Inventory::with_empty(),
                    body,
                    body.scale(),
                )
                .build()
        };
        app.update();

        // Manually tag a Bevy mirror entity as belonging to a SECOND
        // dimension (see the sibling test above for why this is the
        // forward-looking stand-in for a future EM-4.7 spawn-into-dimension
        // flow).
        let bevy_mirror = app
            .world_mut()
            .spawn((SimEntity(specs_entity), DimensionId(2)))
            .id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(specs_entity, bevy_mirror);

        // Spin dimension 2 up to `Active` (zero registry-tracked occupants)
        // via the registry's direct API — no real procgen needed.
        {
            let mut registry = app.world_mut().resource_mut::<DimensionRegistry>();
            let root = Entity::from_raw_u32(9002).expect("small test entity id");
            registry
                .insert_spinning_up(DimensionId(2), root, 0)
                .unwrap();
            let (world, index) = server::World::empty();
            registry
                .complete_spinup(DimensionId(2), std::sync::Arc::new(world), index)
                .unwrap();
            assert_eq!(
                registry.lifecycle(DimensionId(2)),
                Some(DimensionLifecycle::Active),
                "dimension 2 must still be Active before the real drain message is sent"
            );
        }

        // The real admin-command message, not a direct registry call — this
        // is what makes the flip happen INSIDE `handle_drain_requests`, in
        // the same `FixedUpdate` schedule run as
        // `delete_specs_entities_for_torn_down_dimensions`, exercising the
        // actual race window.
        app.world_mut()
            .write_message(xindeler_dimensions::DrainDimension(DimensionId(2)));

        // A few ticks: the first `app.update()` after the message is written
        // is the one where `handle_drain_requests` flips Active -> Teardown
        // AND (with the ordering fix) this system observes that same-frame
        // Teardown state before `teardown_completed_dimensions` cascades the
        // despawn. Without the `.after(..)` fix, this loop could still pass
        // by luck (Bevy's default executor isn't adversarial) — the point of
        // this regression test is that the ordering is now DECLARED, not
        // merely observed to work once.
        for _ in 0..10 {
            app.update();
        }

        let sim = app.world().non_send::<SimServer>();
        assert!(
            !sim.server.state().ecs().entities().is_alive(specs_entity),
            "the specs entity mirrored into a dimension that flipped Active -> Teardown via a \
             REAL DrainDimension message (not a direct registry call) should still have been \
             deleted through the sim's normal delete path — no same-frame scheduling race should \
             be able to lose it"
        );
    }
}
