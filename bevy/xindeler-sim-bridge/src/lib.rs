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
    app::{App, Plugin, Update},
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
    CompressedChunk, NetBody, NetHealth, NetLoadout, NetLocalPlayer, NetOri, NetPos, NetTool,
    NetToolKey, NetVel, RemoveChunk, TerrainAnchor,
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
        app.init_resource::<SimMirror>()
            .init_resource::<SimLoadoutCache>()
            .init_resource::<TestNpcState>()
            .add_systems(
                Update,
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
            count: 8,
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
    mut state: bevy::ecs::system::ResMut<TestNpcState>,
) {
    let Some(sim) = sim else { return };
    if state.spawned || sim.ticks < state.warmup_ticks {
        return;
    }

    // World-centre XY in sim blocks (same derivation as the terrain anchor).
    let size_chunks = sim.server.world().sim().get_size();
    let chunk_sz = vek::Vec2::new(32.0_f32, 32.0);
    let centre = vek::Vec2::new(size_chunks.x as f32, size_chunks.y as f32) * chunk_sz * 0.5;

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
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos));

    server.state().emit_event_now(CreateNpcEvent {
        pos: comp::Pos(wpos),
        ori: comp::Ori::default(),
        npc,
    });
}

/// Builds a visible starter loadout (sword + worker clothes + lantern) as an
/// [`Inventory`](comp::Inventory) for a test humanoid NPC (EM-3.8d). Mirrors
/// the embedded player's default Warrior kit so the gear a test NPC shows
/// matches the player's. Requires the asset tree (item defs) — only called on
/// the live sim, which always has it.
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
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos));

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
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos));

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
    .with_agent(comp::Agent::from_body(&body).with_patrol_origin(wpos));

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
fn net_loadout_from_inventory(inventory: &comp::Inventory) -> NetLoadout {
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
    }
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
    )
        .lend_join();
    while let Some((entity, pos, body, ori, vel, health, presence, inventory)) = it.next() {
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
        let net_loadout = matches!(body, comp::Body::Humanoid(_))
            .then(|| inventory.map_or_else(NetLoadout::default, net_loadout_from_inventory));
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
        let loadout = net_loadout_from_inventory(&inventory);

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
        // Slots we didn't equip stay empty (figure uses the manifest default).
        assert_eq!(loadout.belt, None);
        assert_eq!(loadout.shoulder, None);
        assert_eq!(loadout.second_tool, None);
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
}
