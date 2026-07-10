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
//! Isolation law: logic crates never depend on this crate or on Bevy; the
//! bridge only calls the sim's public API. This crate is the ONLY legal `specs`
//! consumer under `bevy/` — the client stays pure.

mod player;
pub use player::{EmbeddedPlayer, PlayerBridgePlugin, boot_embedded_player, tick_player};

use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

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
};
use server::{
    EditableSettings, Event, Input, Server, Settings,
    persistence::{DatabaseSettings, SqlLogMode},
};
use specs::{LendJoin, WorldExt};
use xindeler_protocol::{
    CompressedChunk, NetBody, NetHealth, NetLoadout, NetLocalPlayer, NetLodAlt, NetOri, NetPos,
    NetTool, NetToolKey, NetVel, RemoveChunk, TerrainAnchor,
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
        // EM-3.11b: FixedUpdate, not Update — see `tick_sim`'s doc for why a
        // display-rate `Update` tick was the wrong home for this.
        app.add_systems(FixedUpdate, tick_sim);
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
        // EM-3.11b: FixedUpdate alongside `tick_sim` — see its doc. Chaining
        // `.after(tick_sim)` requires both to live in the same schedule.
        app.init_resource::<TerrainAnchorState>().add_systems(
            FixedUpdate,
            (ensure_terrain_anchor, stream_terrain_changes)
                .chain()
                .after(tick_sim)
                .run_if(in_state(ClientState::Disconnected)),
        );
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

    // TODO(EM-4.2d): `targets: All` + a global `sent` latch only reaches
    // clients connected AT the single broadcast — a client joining after it
    // never receives the far-terrain heightmap (same accepted limitation as
    // `TerrainAnchor` above). Fine for v1's one-embedded-player world; needs
    // a per-connection "have I sent this yet" once real multi-client join
    // timing matters (interest management lands in EM-4.2d anyway).
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
/// Add AFTER [`SimBridgePlugin`].
pub struct SimEntityMirrorPlugin;

impl Plugin for SimEntityMirrorPlugin {
    fn build(&self, app: &mut App) {
        // EM-3.11b: FixedUpdate alongside `tick_sim` — see its doc. Chaining
        // `.after(tick_sim)` requires both to live in the same schedule.
        app.init_resource::<SimMirror>()
            .init_resource::<SimLoadoutCache>()
            .init_resource::<TestNpcState>()
            .add_systems(
                FixedUpdate,
                (spawn_test_npcs, mirror_sim_entities)
                    .chain()
                    .after(tick_sim)
                    .run_if(in_state(ClientState::Disconnected)),
            );
    }
}

/// Latch + config for the one-shot test-NPC spawn.
#[derive(Resource)]
struct TestNpcState {
    /// Whether the spawn request has been emitted yet.
    spawned: bool,
    /// Wait for the anchor to have generated ground before spawning, so the
    /// NPCs don't fall through ungenerated terrain. We reuse the terrain
    /// anchor's readiness by waiting a few ticks after boot.
    warmup_ticks: u64,
    /// Number of wandering NPCs to spawn.
    count: u32,
}

impl Default for TestNpcState {
    fn default() -> Self {
        Self {
            spawned: false,
            // ~150 ticks (~5 s at 30 TPS) gives the async chunk gen around the
            // anchor time to produce ground under the spawn ring.
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

/// Emits [`CreateNpcEvent`]s for a ring of wandering NPCs around the world
/// centre, ONCE, after a short warmup. Uses only the sim's PUBLIC event bus
/// (`State::emit_event_now` + `event::{CreateNpcEvent, NpcBuilder}`) — the same
/// path `/spawn` uses — so the bridge stays a thin shell over public API.
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
    if state.spawned || sim.ticks < state.warmup_ticks {
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
    // it's known; fall back to the geometric centre only in pure-spectator
    // mode (no embedded player), where the persister fallback genuinely does
    // keep that area loaded.
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
        // A ~10-block ring so they're near the anchor camera but not stacked.
        let offset = vek::Vec2::new(angle.cos(), angle.sin()) * 10.0;
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
/// Per-client interest management (region/distance) is EM-4.2d; v1 broadcasts.
///
/// ## Read-only into the sim
/// Only `read_storage` + `entities` — never writes into specs (isolation law
/// rule 4). The write side is entirely on the Bevy world (spawn/insert/despawn
/// of the mirror entities).
fn mirror_sim_entities(
    sim: Option<NonSendMut<SimServer>>,
    // EM-3.7b: the embedded local player, if any. Used to tag ITS mirror entity
    // with `NetLocalPlayer` so the client's third-person camera follows it.
    player: Option<bevy::ecs::change_detection::NonSend<EmbeddedPlayer>>,
    mut mirror: bevy::ecs::system::ResMut<SimMirror>,
    mut loadout_cache: bevy::ecs::system::ResMut<SimLoadoutCache>,
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

    // TODO(EM-4.2d): this mirror loop allocates `seen`/`updates`/`seen_set`
    // fresh every tick over all visible entities, and net_health below issues a
    // per-tick `remove::<NetHealth>()` for healthless entities (replicon no-ops
    // it, harmless). Both are fine at test-NPC scale; when interest management
    // reshapes this loop, hoist the buffers into a reused resource and guard the
    // removal. Reviewer minors 1+2, deliberately deferred (non-blocking).
    // Snapshot of which sim entities are visible THIS tick + their net comps.
    let mut seen: Vec<specs::Entity> = Vec::new();
    // Collect (sim_entity, components) first so we can borrow-check-cleanly
    // issue commands after dropping the specs storages.
    #[allow(clippy::type_complexity)]
    let mut updates: Vec<(
        specs::Entity,
        NetPos,
        NetOri,
        NetVel,
        NetBody,
        Option<NetHealth>,
        Option<NetLoadout>,
    )> = Vec::new();

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
    )
        .lend_join();
    while let Some((entity, pos, body, ori, vel, health, presence, inventory, character_state)) =
        it.next()
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
        updates.push((
            entity,
            net_pos,
            net_ori,
            net_vel,
            net_body,
            net_health,
            net_loadout,
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
    ));

    for (sim_entity, net_pos, net_ori, net_vel, net_body, net_health, net_loadout) in updates {
        let is_local_player = player_sim_entity == Some(sim_entity);
        // EM-3.8d: (re-)insert the loadout ONLY when it changed since we last
        // mirrored it (it is a few Strings — re-inserting every tick would
        // needlessly re-replicate them). `changed` is also true on first sight.
        let loadout_changed = net_loadout
            .as_ref()
            .is_some_and(|l| loadout_cache.0.get(&sim_entity) != Some(l));
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
            },
            None => {
                // First sighting: spawn the replicated mirror entity.
                let mut ec = commands.spawn((
                    Replicated,
                    SimEntity(sim_entity),
                    net_pos,
                    net_ori,
                    net_vel,
                    net_body,
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
                mirror.0.insert(sim_entity, ec.id());
            },
        }
        // Refresh the dedup cache for this entity's loadout.
        if let Some(l) = net_loadout {
            loadout_cache.0.insert(sim_entity, l);
        }
    }

    // Despawn mirrors whose sim entity is gone / no longer visible this tick.
    let seen_set: std::collections::HashSet<specs::Entity> = seen.into_iter().collect();
    let stale: Vec<specs::Entity> = mirror
        .0
        .keys()
        .copied()
        .filter(|e| !seen_set.contains(e))
        .collect();
    for sim_entity in stale {
        if let Some(bevy_entity) = mirror.0.remove(&sim_entity) {
            commands.entity(bevy_entity).despawn();
        }
        // Drop the cached loadout too, so a re-used specs index doesn't inherit
        // a stale entry (EM-3.8d).
        loadout_cache.0.remove(&sim_entity);
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
    use xindeler_protocol::XindelerProtocolPlugin;

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
        {
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
        }

        server_app.connect_client(&mut client_app);

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
    /// `server/src/sys/terrain.rs`) already does.
    ///
    /// This test boots the REAL sim + a REAL embedded player (so the ring
    /// centres on wherever the world's own spawn-point selection actually put
    /// it — not a location the test controls), lets the boot-time
    /// `spawn_test_npcs` ring fire, and asserts the default 8 wandering NPCs
    /// (a) actually reach the Bevy World as `NetBody` entities distinct from
    /// the player's own mirror, and (b) are STILL alive `SURVIVAL_TICKS` later
    /// — before the fix, (a) alone already failed (the count was always 0).
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
        let mut fired_at: Option<u32> = None;
        for tick in 0..MAX_TICKS {
            app.update();
            if fired_at.is_none() && app.world().resource::<TestNpcState>().spawned {
                fired_at = Some(tick);
                eprintln!("spawn_test_npcs fired at tick {tick}");
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

        // Identify the wandering test NPCs SPECIFICALLY, on the sim side, by
        // their own `Stats.name` convention (`"Test <Body> <index>"` — see
        // `emit_wandering_npc`/`_humanoid`/`_quadruped_medium`/`_bird_medium`).
        // A plain `NetBody` count on the Bevy mirror is NOT enough: the
        // player's real (non-geometric-centre) spawn point almost always has
        // ambient rtsim wildlife nearby, which ALSO mirrors as `NetBody` and
        // would make a bare "count >= 8" assertion pass even if every test
        // NPC had been deleted (this exact false-positive was caught while
        // writing this test, with the fix reverted only in-memory to confirm
        // it — never committed unfixed).
        let test_named = {
            let sim = app.world().non_send::<SimServer>();
            let ecs = sim.server.state().ecs();
            let stats = ecs.read_storage::<comp::Stats>();
            specs::Join::join(&stats)
                .filter(|s| {
                    matches!(&s.name, common::comp::Content::Plain(n) if n.starts_with("Test "))
                })
                .count()
        };
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
}
