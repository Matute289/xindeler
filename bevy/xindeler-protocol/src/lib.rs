//! Replication contract (bevy_replicon): replicated components, channels,
//! client/server messages. Shared by client and server — [Q3]=B.
//!
//! BL-82 Bevy migration — EM-1.5b: replicated component set v0 (`NetPos`,
//! `NetOri`, `NetHealth`, `NetBody`), the [`XindelerChannel`] lanes, the
//! `PlayerInput` client message, and [`XindelerProtocolPlugin`] registering
//! everything **symmetrically** (the same plugin runs on client and server, so
//! the replication rule/message registries — and therefore the protocol hash —
//! always match). Isolation law: logic crates never depend on this crate or on
//! Bevy.
//!
//! [`XindelerProtocolPlugin`] must be added **after** `RepliconPlugins`
//! (`AppRuleExt::replicate` / `ClientMessageAppExt::add_client_message` need
//! replicon's registries in place).

use bevy::{
    app::{App, Plugin},
    ecs::{component::Component, message::Message},
    math::{Quat, Vec2, Vec3},
};
use bevy_replicon::prelude::{AppRuleExt, Channel, ClientMessageAppExt, ServerMessageAppExt};
use common::terrain::TerrainChunk;
use serde::{Deserialize, Serialize};

/// Replicated world position of an entity (server-authoritative).
///
/// v0 of the replication set: mirrored from the sim's `comp::Pos` by
/// `xindeler-sim-bridge` (EM-3.6) and consumed by the pure-Bevy client.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetPos(pub Vec3);

/// Replicated orientation of an entity (server-authoritative).
///
/// Mirrored from the sim's `comp::Ori` (which is quaternion-backed).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetOri(pub Quat);

/// Replicated health snapshot of an entity (server-authoritative).
///
/// Flattened from the sim's `comp::Health`; the client only needs the pair for
/// HUD/nameplate display — combat math stays server-side.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetHealth {
    pub current: f32,
    pub max: f32,
}

/// Replicated body identifier (which model/species an entity displays as).
///
/// v0 placeholder: an opaque `u32` key into the client-side body/model table;
/// the real `comp::Body` → key mapping lands with the mirror (EM-3.6/3.7).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetBody(pub u32);

/// Client → server input sample (v0 placeholder shape).
///
/// Sent as a replicon *client message*; it surfaces on the server wrapped in
/// `FromClient<PlayerInput>` with the sender's `client_id`.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct PlayerInput {
    /// Horizontal movement intent (unit-ish vector, XY plane).
    pub move_dir: Vec2,
    /// Whether the jump control is pressed this sample.
    pub jump: bool,
    /// Camera/look direction.
    pub look: Vec3,
}

/// Server → client terrain stream: one sim chunk, bincode(legacy)-serialized
/// and lz4-compressed (EM-3.6; same scheme Veloren's COMPRESSED net streams
/// use — `network/src/message.rs`). Travels on the [`XindelerChannel::Terrain`]
/// lane (reliable, unordered — cross-chunk ordering is irrelevant).
///
/// Sent by the server-side bridge for every new/modified chunk the sim's
/// `TerrainChanges` reports; v1 is a broadcast to all clients (per-client
/// interest management = EM-4.2d).
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CompressedChunk {
    /// 2D chunk key (upstream `TerrainGrid` convention).
    pub key: [i32; 2],
    /// lz4-compressed bincode of the sim's `TerrainChunk`.
    pub bytes: Vec<u8>,
}

impl CompressedChunk {
    /// Serializes (bincode `legacy()`, matching `common-net`) + compresses
    /// (lz4 raw block, `lz_fear` — the crate Veloren's net stack already
    /// pins) one chunk.
    #[must_use]
    pub fn encode(key: [i32; 2], chunk: &TerrainChunk) -> Self {
        let raw = bincode::serde::encode_to_vec(chunk, bincode::config::legacy())
            .expect("bincode serialization can only fail if a byte limit is set");
        let mut bytes = Vec::with_capacity(raw.len() / 4 + 16);
        let mut table = lz_fear::raw::U32Table::default();
        lz_fear::raw::compress2(&raw, 0, &mut table, &mut bytes)
            .expect("lz4 compression into a Vec<u8> is infallible");
        Self { key, bytes }
    }

    /// Decompresses + deserializes the payload. `None` = corrupt payload
    /// (callers log and drop; the local loopback can't corrupt, so this only
    /// matters once a real transport lands in EM-4.2b).
    ///
    /// The decompressed-size cap mirrors `network/src/message.rs`
    /// (`usize::MAX`); a hostile-input budget is an EM-4.2d (hardening)
    /// concern, not a loopback one.
    #[must_use]
    pub fn decode(&self) -> Option<TerrainChunk> {
        let mut raw = Vec::with_capacity(self.bytes.len() * 2);
        lz_fear::raw::decompress_raw(&self.bytes, &[0; 0], &mut raw, usize::MAX).ok()?;
        bincode::serde::decode_from_slice(&raw, bincode::config::legacy())
            .ok()
            .map(|(chunk, _)| chunk)
    }
}

/// Server → client: the sim unloaded a chunk; drop it (store + mesh).
///
/// Same Terrain lane as [`CompressedChunk`]. ⚠️ The lane is UNORDERED: over a
/// real transport a remove could overtake the chunk it removes. The v1
/// loopback preserves order (local `Messages` drain); revisit with interest
/// management (EM-4.2d).
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveChunk {
    /// 2D chunk key (upstream `TerrainGrid` convention).
    pub key: [i32; 2],
}

/// Server → client: world position (sim coordinates, z-up) of the terrain
/// presence anchor — where chunks are being kept loaded around (EM-3.6's
/// centered persister). v1 pragmatic: sent once at boot on the ordered
/// Events lane; the client parks its spectator camera over it.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct TerrainAnchor {
    /// Sim/world position (Veloren axes: x-east, y-north, z-up).
    pub wpos: [f32; 3],
}

/// Logical channel lanes of the Xindeler protocol.
///
/// bevy_replicon 0.41 has **no named custom-channel registry**: component
/// replication always travels on its two built-in server channels
/// (`ServerChannel::Updates` reliable-ordered + `ServerChannel::Mutations`
/// unreliable), and every message/event registered via
/// `add_client_message`/`add_server_message`/`add_*_event` implicitly creates
/// **one channel of its own** in [`bevy_replicon::prelude::RepliconChannels`],
/// parameterized only by a [`Channel`] delivery guarantee.
///
/// So this enum documents our three lanes and pins the delivery guarantee each
/// one uses at registration time ([`Self::delivery`]):
///
/// - [`State`](Self::State) → [`Channel::Unreliable`]: latest-wins gameplay
///   state samples (e.g. input). Component replication itself does NOT use this
///   lane — replicon's built-ins already implement the reliable+latest
///   dual-channel scheme.
/// - [`Terrain`](Self::Terrain) → [`Channel::Unordered`]: reliable bulk chunk
///   payloads (bincode+lz4, EM-3.x); cross-chunk ordering is irrelevant, so
///   unordered-reliable avoids head-of-line blocking.
/// - [`Events`](Self::Events) → [`Channel::Ordered`]: discrete gameplay
///   events/commands that must all arrive, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum XindelerChannel {
    /// Latest-wins state samples (unreliable).
    State,
    /// Bulk terrain payloads (reliable, unordered).
    Terrain,
    /// Discrete gameplay events (reliable, ordered).
    Events,
}

impl XindelerChannel {
    /// The replicon delivery guarantee this lane registers with.
    pub const fn delivery(self) -> Channel {
        match self {
            XindelerChannel::State => Channel::Unreliable,
            XindelerChannel::Terrain => Channel::Unordered,
            XindelerChannel::Events => Channel::Ordered,
        }
    }
}

/// Registers the whole replication contract, symmetrically for client and
/// server (identical registration order ⇒ identical replicon protocol hash).
///
/// Add **after** `RepliconPlugins`.
/// ⚠️ OBLIGATION (spec §6.2, reviewer M1): replicon's default visibility sends
/// every `Replicated` entity to every client. Before the EM-3.6 mirror starts
/// spawning `Replicated` entities, per-client visibility scoping (interest
/// management — `bevy_replicon` visibility filters, region/distance +
/// `DimensionId`) MUST be wired (EM-4.2d). Do not ship default-all visibility
/// past the listen-server milestone.
pub struct XindelerProtocolPlugin;

impl Plugin for XindelerProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Replicated component set v0. Entities additionally need replicon's
        // `Replicated` marker on the server to be sent at all.
        app.replicate::<NetPos>()
            .replicate::<NetOri>()
            .replicate::<NetHealth>()
            .replicate::<NetBody>();

        // Client → server messages. v0 keeps PlayerInput on the ordered lane
        // (no client-side redundancy/resampling yet); it moves to the
        // unreliable State lane once the input stream sends redundant samples.
        app.add_client_message::<PlayerInput>(XindelerChannel::Events.delivery());

        // Server → client messages (EM-3.6 terrain stream). The server writes
        // `ToClients<CompressedChunk>` etc.; replicon fans them out to clients
        // AND, in listen-server mode (`ClientState::Disconnected`), re-emits
        // them locally as plain `CompressedChunk` in the same App — that local
        // path is exactly how the single-App listen server receives its own
        // terrain (see `server/message.rs::send_locally`, gated on
        // `ClientState::Disconnected`).
        //
        // `make_message_independent`: these carry NO entity references, so they
        // must NOT be queued behind entity replication (the default) — the
        // terrain stream is decoupled from the entity/component tick.
        app.add_server_message::<CompressedChunk>(XindelerChannel::Terrain.delivery())
            .make_message_independent::<CompressedChunk>();
        app.add_server_message::<RemoveChunk>(XindelerChannel::Terrain.delivery())
            .make_message_independent::<RemoveChunk>();
        app.add_server_message::<TerrainAnchor>(XindelerChannel::Events.delivery())
            .make_message_independent::<TerrainAnchor>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::{prelude::*, state::app::StatesPlugin};
    use bevy_replicon::{
        prelude::{FromClient, Replicated, RepliconPlugins, ServerPlugin},
        test_app::ServerTestAppExt,
    };

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            // Tick replication on every `app.update()` (default is FixedPostUpdate,
            // which may not run in a manually-stepped test app).
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
            XindelerProtocolPlugin,
        ))
        .finish();
        app
    }

    /// EM-1.5b acceptance: client + server Apps exchange a replicated entity
    /// over replicon's transport-less test loopback.
    #[test]
    fn replicates_entity_server_to_client() {
        let mut server_app = new_app();
        let mut client_app = new_app();

        server_app.connect_client(&mut client_app);

        let pos = NetPos(Vec3::new(1.0, 2.0, 3.0));
        let ori = NetOri(Quat::from_rotation_z(core::f32::consts::FRAC_PI_2));
        let health = NetHealth {
            current: 42.0,
            max: 100.0,
        };
        let body = NetBody(7);
        server_app
            .world_mut()
            .spawn((Replicated, pos, ori, health, body));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut replicated = client_app
            .world_mut()
            .query::<(&NetPos, &NetOri, &NetHealth, &NetBody)>();
        let (got_pos, got_ori, got_health, got_body) = replicated
            .single(client_app.world())
            .expect("exactly one replicated entity should reach the client");
        assert_eq!(*got_pos, pos);
        assert_eq!(*got_ori, ori);
        assert_eq!(*got_health, health);
        assert_eq!(*got_body, body);
    }

    /// `PlayerInput` travels client → server and surfaces as `FromClient<_>`.
    #[test]
    fn player_input_reaches_server() {
        let mut server_app = new_app();
        let mut client_app = new_app();

        server_app.connect_client(&mut client_app);

        let input = PlayerInput {
            move_dir: Vec2::new(0.0, 1.0),
            jump: true,
            look: Vec3::new(0.0, 1.0, 0.5),
        };
        client_app.world_mut().write_message(input);

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<PlayerInput>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1, "server should receive one input message");
        assert_eq!(received[0].message, input);
    }

    /// A real `TerrainChunk` survives `encode` → `decode` byte-for-byte.
    #[test]
    fn compressed_chunk_round_trips() {
        use common::{
            terrain::{Block, BlockKind, TerrainChunk, TerrainChunkMeta},
            vol::{ReadVol, WriteVol},
        };
        use vek::{Rgb, Vec3 as VVec3};

        let mut chunk =
            TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
        let block = Block::new(BlockKind::Rock, Rgb::new(120, 100, 90));
        chunk
            .set(VVec3::new(3, 4, 5), block)
            .expect("in-bounds write");

        let encoded = CompressedChunk::encode([2, -7], &chunk);
        assert_eq!(encoded.key, [2, -7]);
        assert!(!encoded.bytes.is_empty());

        let decoded = encoded.decode().expect("round-trips");
        assert_eq!(decoded.get(VVec3::new(3, 4, 5)).ok(), Some(&block));
    }

    /// Listen-server path: a server writing `ToClients<CompressedChunk>` in an
    /// App with NO connected client (i.e. `ClientState::Disconnected`) receives
    /// it back locally as `CompressedChunk` — this is the single-App loopback
    /// the listen server relies on (replicon's `send_locally`).
    #[test]
    fn compressed_chunk_loops_back_locally_on_listen_server() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut app = new_app();
        // No `connect_client`: the App is a server that is ALSO the only
        // client (ClientState defaults to Disconnected).
        let payload = CompressedChunk {
            key: [1, 2],
            bytes: vec![9, 8, 7],
        };
        app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: payload.clone(),
        });
        app.update();

        let received: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CompressedChunk>>()
            .drain()
            .collect();
        assert_eq!(
            received,
            vec![payload],
            "listen server must see its own terrain locally"
        );
    }
}
