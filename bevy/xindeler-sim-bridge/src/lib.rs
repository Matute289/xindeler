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
//!
//! Isolation law: logic crates never depend on this crate or on Bevy; the
//! bridge only calls the sim's public API. This crate is the ONLY legal `specs`
//! consumer under `bevy/` — the client stays pure.

use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use bevy::{
    app::{App, Plugin, Update},
    ecs::{
        change_detection::NonSendMut, component::Component, entity::Entity, message::MessageWriter,
        resource::Resource, schedule::IntoScheduleConfigs, system::Res,
    },
    state::condition::in_state,
    time::Time,
};
use bevy_replicon::prelude::{ClientState, SendTargets, ToClients};
use server::{
    EditableSettings, Event, Input, Server, Settings,
    persistence::{DatabaseSettings, SqlLogMode},
};
use xindeler_protocol::{CompressedChunk, RemoveChunk, TerrainAnchor};

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

/// Marks a Bevy entity as the mirror of a sim (specs) entity.
///
/// Placeholder for EM-3.6/3.7 — no system populates it yet.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimEntity(pub specs::Entity);

/// sim (specs) entity → mirrored Bevy entity lookup.
///
/// Placeholder for EM-3.6/3.7 — no system populates it yet.
#[derive(Resource, Default, Debug)]
pub struct SimMirror(pub HashMap<specs::Entity, Entity>);

/// Advances the embedded sim by one tick using Bevy's frame `dt`, then drains
/// the sim's frontend events and errors into `tracing`.
///
/// Runs in `Update` (the shell is expected to pace the whole `App` at the
/// server TPS — `ScheduleRunnerPlugin::run_loop`); no-ops until the shell
/// inserts a [`SimServer`] (there is no `resource_exists` equivalent for
/// non-send data, so the gate is the `Option` param).
///
/// TODO(EM-3.7/EM-4.1): in the WINDOWED listen-server path the App is paced at
/// display rate, so this ticks at 60–144 Hz instead of the 30 TPS the headless
/// shell uses. Game-time stays correct (dt-driven) but full sim work runs
/// 2–5× too often and diverges from server cadence. Wire this onto a
/// `FixedUpdate` / `Time::<Fixed>::from_hz(30.0)` schedule (moving
/// `stream_terrain_changes` with it) when the playable path lands — it is a
/// v1-visual-proof deferral, not a shipping cadence.
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

/// Registers the bridge types and the [`tick_sim`] system.
///
/// Deliberately does NOT boot the sim: the shell (or a test) constructs a
/// [`SimServer`] — e.g. with [`boot_test_server`] — and inserts it whenever
/// it's ready; until then [`tick_sim`] simply doesn't run.
pub struct SimBridgePlugin;

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimMirror>();
        app.add_systems(Update, tick_sim);
    }
}

// ---------------------------------------------------------------------------
// EM-3.6 — terrain streaming (sim `TerrainChanges` → replicon server messages)
// ---------------------------------------------------------------------------

/// Terrain view distance (in chunks) the server-side anchor keeps loaded.
///
/// Small on purpose for the listen-server proof: a real world is huge and the
/// naive broadcast has no interest management yet (EM-4.2d), so a wide anchor
/// would stream thousands of chunks to a single local client. `MIN_VD` is the
/// sim's own minimum, matching what a freshly-spawned player loads.
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
        app.init_resource::<TerrainAnchorState>().add_systems(
            Update,
            (ensure_terrain_anchor, stream_terrain_changes)
                .chain()
                .after(tick_sim)
                .run_if(in_state(ClientState::Disconnected)),
        );
    }
}

/// Spawns the server-side presence anchor once the sim is booted, then reads
/// back a sensible world position for the camera and broadcasts it once.
///
/// ## Why a centered persister (not an embedded `Client`)
/// EM-1.6 showed a full embedded `xindeler-client-core::Client` anchor works
/// over TCP loopback, but it is heavy (a second network stack, registration,
/// character creation). The sim exposes a PUBLIC, purpose-built alternative —
/// `Server::create_centered_persister` — that spawns exactly the `Presence`
/// spectator entity the terrain system needs, with no networking. It lives
/// entirely inside the sim's public API, so the bridge stays a thin shell and
/// the client stays pure. Chosen for v1; the embedded-Client path remains the
/// fallback for when we need a *controllable* character (EM-3.7).
fn ensure_terrain_anchor(
    sim: Option<NonSendMut<SimServer>>,
    mut anchor: bevy::ecs::system::ResMut<TerrainAnchorState>,
    mut anchor_writer: MessageWriter<ToClients<TerrainAnchor>>,
) {
    let Some(mut sim) = sim else { return };

    if !anchor.anchored {
        // `create_centered_persister` is `#[cfg(feature = "worldgen")]` in the
        // server crate; the bridge always links `server` with its default
        // features (worldgen on), so this is always available here.
        sim.server.create_centered_persister(ANCHOR_VIEW_DISTANCE);
        anchor.anchored = true;

        // World-center XY (chunk keys only depend on XY); z = the sim's
        // approximate surface altitude there so the spectator camera starts
        // near the ground rather than at half the world's block height.
        let sim_ref = &sim.server;
        let size_chunks = sim_ref.world().sim().get_size();
        // Mirrors `common::terrain::TerrainChunkSize::RECT_SIZE`
        // (`1 << TERRAIN_CHUNK_BLOCKS_LG` = 32). Kept as a literal here because
        // this crate depends only on `server`, not `common` directly — adding a
        // whole dep for one constant isn't worth it (client-side terrain_stream,
        // which does depend on common, derives it properly). Revisit when
        // EM-3.7 makes this crate mirror common comp types anyway.
        let chunk_sz = vek::Vec2::new(32.0_f32, 32.0);
        let center_xy = vek::Vec2::new(size_chunks.x as f32, size_chunks.y as f32) * chunk_sz * 0.5;
        let alt = sim_ref
            .world()
            .sim()
            .get_alt_approx(center_xy.map(|e| e as i32))
            .unwrap_or(0.0);
        anchor.anchor_wpos = Some([center_xy.x, center_xy.y, alt]);
        tracing::info!(?anchor.anchor_wpos, "terrain anchor persister spawned");
    }

    if !anchor.anchor_sent
        && let Some(wpos) = anchor.anchor_wpos
    {
        anchor_writer.write(ToClients {
            targets: SendTargets::All,
            message: TerrainAnchor { wpos },
        });
        anchor.anchor_sent = true;
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
/// locally for the listen server). TODO(EM-4.2d): per-client interest
/// management — only send a chunk to clients whose presence covers it.
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

/// Boots a throwaway singleplayer-style server rooted at `data_dir` for
/// tests/dev shells: unused local TCP port, auth disabled, default world
/// (needs `VELOREN_ASSETS`/`XINDELER_ASSETS` + the LFS map blobs), SQLite under
/// `<data_dir>/saves`.
pub fn boot_test_server(data_dir: &Path) -> Result<SimServer, server::Error> {
    let settings = Settings::singleplayer(data_dir);
    let editable_settings = EditableSettings::singleplayer(data_dir);
    let database_settings = DatabaseSettings {
        db_dir: data_dir.join("saves"),
        sql_log_mode: SqlLogMode::Disabled,
    };
    // Small multi-thread runtime, same shape as server-cli's (Server::new
    // requires a runtime it can block on and spawn network tasks onto).
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("tokio-sim-bridge")
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
        MinimalPlugins, app::PluginGroup, ecs::message::Messages, state::app::StatesPlugin,
    };
    use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
    use xindeler_protocol::XindelerProtocolPlugin;

    use super::*;

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
        app.insert_non_send(sim);

        for _ in 0..100 {
            app.update();
        }

        let sim = app.world().non_send::<SimServer>();
        assert_eq!(
            sim.ticks, 100,
            "every app.update() should have completed one successful sim tick"
        );
        // The sim's own game-time clock advanced with the Bevy dt (delta is 0
        // only on the very first update), proving `Server::tick` really ran.
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
}
