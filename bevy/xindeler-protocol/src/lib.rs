//! Replication contract (bevy_replicon): replicated components, channels,
//! client/server messages. Shared by client and server — [Q3]=B.
//!
//! BL-82 Bevy migration — EM-1.5b: replicated component set v0 (`NetPos`,
//! `NetOri`, `NetVel`, `NetHealth`, `NetBody`), the [`XindelerChannel`] lanes,
//! the
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
    ecs::{component::Component, message::Message, resource::Resource},
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

/// Replicated velocity of an entity (server-authoritative, Bevy axes).
///
/// Mirrored from the sim's `comp::Vel`; the client uses it for dead-reckoning
/// in its interpolation buffer (EM-3.7 — mirrors voxygen's `pos + vel * 0.03`
/// lead so an entity that keeps moving between the low-rate net samples doesn't
/// visibly lag its own motion).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetVel(pub Vec3);

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
/// EM-3.8: carries the FULL sim `comp::Body` — not just a class id. Real
/// `.vox` figures need the exact `(species, body_type)` (and, for humanoids,
/// the head/skin/hair/eye/armour fields) to pick and assemble the right model
/// pieces, so a `u32` class key (EM-3.7's placeholder) is not enough. `Body`
/// is `Copy + Serialize + Deserialize` and lives in `common` (which this crate
/// already links for `TerrainChunk`), so replicating it verbatim is the honest
/// minimal enrichment; the client resolves it to a `FigureBody`
/// (`xindeler-render-voxel::figure`). The client stays specs-free — it only
/// pattern-matches this plain data enum, never touches the ECS.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetBody(pub common::comp::Body);

/// Replicated figure-relevant equipped gear of a humanoid entity (EM-3.8d).
///
/// This is the COMPACT projection of the sim's `comp::Inventory`/loadout that
/// the humanoid figure assembly needs — NOT the whole inventory. It carries
/// exactly what changes the rendered figure model:
/// - the active/second **tools** as `(ToolKey, ToolKind, Hands)` so the right
///   weapon `.vox` sheathes on the back bone with the correct pose;
/// - the equipped **armour** pieces per figure slot, as their
///   item-definition-id **string** (the same key voxygen's `CharacterCacheKey`
///   uses to look up the per-item `.vox` in the frozen armour manifests) —
///   chest/belt/back/pants/ shoulder/hand/foot plus the lantern.
///
/// Everything is a plain item-id string or a small enum, so the wire cost is a
/// handful of short strings per humanoid (sent only when it changes — the
/// mirror dedups, see `xindeler-sim-bridge`). The client
/// (`xindeler-render-voxel`) resolves these keys against the manifests; it
/// stays specs-free (this is plain data). Only humanoids carry it (armour/tools
/// only reshape the humanoid figure). Head-slot helmets + the glider are
/// deferred to EM-3.8e (they need a species-keyed head manifest /
/// glide-state-gated visibility we don't mirror yet), so they are intentionally
/// absent here.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetLoadout {
    /// Active main-hand tool (drives the `main` weapon bone + its sheathe
    /// pose).
    pub active_tool: Option<NetTool>,
    /// Active off-hand tool (drives the `second` weapon bone), if
    /// dual-wielding.
    pub second_tool: Option<NetTool>,
    /// Chest armour item-def-id (`None` = default/naked torso model).
    pub chest: Option<String>,
    /// Belt armour item-def-id.
    pub belt: Option<String>,
    /// Back armour item-def-id (cape/pack meshed on the `back` bone).
    pub back: Option<String>,
    /// Leg armour item-def-id (rides the `shorts` bone).
    pub pants: Option<String>,
    /// Shoulder armour item-def-id (sided).
    pub shoulder: Option<String>,
    /// Hand armour item-def-id (sided).
    pub hand: Option<String>,
    /// Foot armour item-def-id (sided).
    pub foot: Option<String>,
    /// Lantern item-def-id (meshed on the `lantern` bone at the hip).
    pub lantern: Option<String>,
}

/// A replicated equipped tool: the weapon-manifest key plus the `ToolKind`/
/// `Hands` the animation needs to pose it (EM-3.8d). `ToolKind`/`Hands` are the
/// plain `common` data enums (this crate already links `common`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetTool {
    /// Key into the frozen `biped_weapon_manifest` (the `.vox` + offset).
    pub key: NetToolKey,
    /// Weapon category (Sword/Axe/…), for the back-sheathe pose.
    pub kind: common::comp::tool::ToolKind,
    /// One- vs two-handed (drives the sheathe placement).
    pub hands: common::comp::tool::Hands,
}

/// The figure-manifest key of a tool, mirroring voxygen's `ToolKey` shape so
/// the render crate can reconstruct the exact map key (EM-3.8d): a simple item
/// id, or a modular weapon's `(primary, secondary, hands)` key.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NetToolKey {
    /// A non-modular tool, keyed by its item-definition-id.
    Tool(String),
    /// A modular weapon, keyed by `(primary-component, secondary-component,
    /// hands)` — the
    /// `common::comp::inventory::item::modular::ModularWeaponKey`.
    Modular {
        primary: String,
        secondary: String,
        hands: common::comp::tool::Hands,
    },
}

/// Marks the mirrored entity that is THIS client's own player (EM-3.7b).
///
/// The listen-server bridge hosts an embedded `xindeler-client-core::Client`
/// that IS the local player; the bridge tags that player's mirror entity with
/// this component so the pure-Bevy client can tell which of the replicated
/// capsules to follow with the third-person camera. It carries no data — its
/// mere presence is the signal. Replicated like the Net* comps (a marker
/// component with no fields still round-trips through replicon).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetLocalPlayer;

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

/// The current local-player control sample, shared IN-PROCESS between the pure
/// Bevy client (which reads keyboard/mouse) and the listen-server bridge (which
/// applies it to the embedded `Client`'s `ControllerInputs`) — EM-3.7b.
///
/// ## Why a shared `Resource`, not the `PlayerInput` replicon message
/// [`PlayerInput`] is the WIRE shape for a *remote* client sending input to a
/// *remote* server (EM-4.2c). In the single-App listen server the input
/// producer and the sim consumer live in the SAME Bevy world, so routing input
/// through replicon's client→server loopback would be pointless serialization
/// (and replicon's local client message loopback is a different path than the
/// server-message one the terrain stream uses). Instead the client writes this
/// resource each frame and the bridge reads it the same frame — a direct
/// in-world handoff. The fields already match [`PlayerInput`] so the remote
/// path (EM-4.2c) can serialize this verbatim later.
///
/// Vectors are in SIM/world axes (x-east, y-north, z-up), already resolved from
/// the camera-relative keyboard intent by the client input system — so the
/// bridge stays a thin applicator and never needs Bevy↔sim axis knowledge for
/// input.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct LocalPlayerInput {
    /// Horizontal movement intent in sim axes (XY plane), magnitude ≤ 1.
    pub move_dir: Vec2,
    /// Whether the jump control is held this sample.
    pub jump: bool,
    /// Look direction in sim axes (x-east, y-north, z-up), unit-ish.
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

/// Server → client: the coarse far-terrain heightmap (EM-3.10b), one
/// downsampled altitude sample per [`Self::chunk_stride`]² chunks. Sent ONCE at
/// boot — like [`TerrainAnchor`] — since the far terrain never changes during a
/// session, so there is no per-tick replication cost.
///
/// ## Source + downsampling
/// The embedded `client::Client`'s `world_data().lod_alt` already packs one
/// sample per CHUNK (not per block), but a default Veloren world is
/// 1024×1024 chunks — far too many quads for a "coarse" far-mesh. The
/// server-side bridge (`xindeler-sim-bridge`) therefore downsamples it (simple
/// stride-pick, capped grid dimension) and decodes each sample to a plain
/// world-space altitude (metres, sim z-up) via the public `WorldData::alt_at`
/// BEFORE sending, so the client does zero Veloren-specific unpacking — it
/// just reads floats.
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetLodAlt {
    /// Downsampled grid width/height, in samples (row-major storage below).
    pub grid_size: [u32; 2],
    /// How many original chunk-grid cells one downsampled sample covers, on
    /// each axis. The client mesh spaces samples `chunk_stride *
    /// CHUNK_EDGE` Bevy metres apart.
    pub chunk_stride: u32,
    /// lz4-compressed bincode of the row-major `Vec<f32>` altitude samples
    /// (world-space metres). Row-major: `heights[y * grid_size[0] + x]`,
    /// matching `common::grid::Grid`'s convention.
    pub bytes: Vec<u8>,
}

impl NetLodAlt {
    /// Serializes (bincode `legacy()`) + compresses (lz4, same scheme as
    /// [`CompressedChunk`]) a downsampled altitude grid.
    #[must_use]
    pub fn encode(grid_size: [u32; 2], chunk_stride: u32, heights: &[f32]) -> Self {
        let raw = bincode::serde::encode_to_vec(heights, bincode::config::legacy())
            .expect("bincode serialization can only fail if a byte limit is set");
        let mut bytes = Vec::with_capacity(raw.len() / 4 + 16);
        let mut table = lz_fear::raw::U32Table::default();
        lz_fear::raw::compress2(&raw, 0, &mut table, &mut bytes)
            .expect("lz4 compression into a Vec<u8> is infallible");
        Self {
            grid_size,
            chunk_stride,
            bytes,
        }
    }

    /// Decompresses + deserializes the payload. `None` = corrupt payload or a
    /// length mismatch against [`Self::grid_size`] (defensive; the local
    /// loopback can't corrupt).
    #[must_use]
    pub fn decode(&self) -> Option<Vec<f32>> {
        let mut raw = Vec::with_capacity(self.bytes.len() * 2);
        lz_fear::raw::decompress_raw(&self.bytes, &[0; 0], &mut raw, usize::MAX).ok()?;
        let (heights, _): (Vec<f32>, _) =
            bincode::serde::decode_from_slice(&raw, bincode::config::legacy()).ok()?;
        let expected = self.grid_size[0] as usize * self.grid_size[1] as usize;
        (heights.len() == expected).then_some(heights)
    }
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
            .replicate::<NetVel>()
            .replicate::<NetHealth>()
            .replicate::<NetBody>()
            // EM-3.8d: the humanoid's figure-relevant equipped gear (weapon(s) +
            // armour) so the client assembles the real character, not a fixed
            // test loadout. Only humanoids carry it; a plain-data component.
            .replicate::<NetLoadout>()
            // EM-3.7b: the local-player marker on the mirror entity so the
            // client's third-person camera knows which capsule to follow.
            .replicate::<NetLocalPlayer>();

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
        // EM-3.10b: the far-terrain heightmap, sent once (same ONE-SHOT
        // TIMING as TerrainAnchor — decoupled from entity replication) but
        // on the `Terrain` channel, NOT `Events` (review should-fix #3): its
        // payload is up to ~64 KB compressed (128×128 f32 samples), size-
        // class-comparable to `CompressedChunk` above, not a small discrete
        // event. `Events` is Ordered/reliable — a multi-KB blob there would
        // head-of-line-block chat/connect/disconnect messages behind it,
        // exactly what `Terrain` (Unordered/reliable) exists to avoid.
        app.add_server_message::<NetLodAlt>(XindelerChannel::Terrain.delivery())
            .make_message_independent::<NetLodAlt>();
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
        let vel = NetVel(Vec3::new(0.5, 0.0, -0.25));
        let health = NetHealth {
            current: 42.0,
            max: 100.0,
        };
        let body = NetBody(common::comp::Body::QuadrupedSmall(
            common::comp::quadruped_small::Body {
                species: common::comp::quadruped_small::Species::Pig,
                body_type: common::comp::quadruped_small::BodyType::Female,
            },
        ));
        server_app
            .world_mut()
            .spawn((Replicated, pos, ori, vel, health, body));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut replicated =
            client_app
                .world_mut()
                .query::<(&NetPos, &NetOri, &NetVel, &NetHealth, &NetBody)>();
        let (got_pos, got_ori, got_vel, got_health, got_body) = replicated
            .single(client_app.world())
            .expect("exactly one replicated entity should reach the client");
        assert_eq!(*got_pos, pos);
        assert_eq!(*got_ori, ori);
        assert_eq!(*got_vel, vel);
        assert_eq!(*got_health, health);
        assert_eq!(*got_body, body);
    }

    /// EM-3.8d: `NetLoadout` (weapon + armour keys) replicates server → client
    /// verbatim over the loopback — it is registered + serde-safe like the
    /// other Net* comps, just carrying `String`/enum data rather than
    /// `Copy` scalars.
    #[test]
    fn net_loadout_replicates() {
        use common::comp::tool::{Hands, ToolKind};

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let loadout = NetLoadout {
            active_tool: Some(NetTool {
                key: NetToolKey::Tool("common.items.weapons.sword.starter".to_owned()),
                kind: ToolKind::Sword,
                hands: Hands::Two,
            }),
            second_tool: None,
            chest: Some("common.items.armor.misc.chest.worker_purple_brown".to_owned()),
            pants: Some("common.items.armor.misc.pants.worker_brown".to_owned()),
            foot: Some("common.items.armor.misc.foot.sandals".to_owned()),
            lantern: Some("common.items.lantern.black_0".to_owned()),
            ..Default::default()
        };
        let body = NetBody(common::comp::Body::Humanoid(common::comp::humanoid::Body {
            species: common::comp::humanoid::Species::Human,
            body_type: common::comp::humanoid::BodyType::Male,
            hair_style: 0,
            beard: 0,
            eyes: 0,
            accessory: 0,
            hair_color: 0,
            skin: 0,
            eye_color: 0,
        }));
        server_app
            .world_mut()
            .spawn((Replicated, body, loadout.clone()));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app.world_mut().query::<&NetLoadout>();
        let got = q
            .single(client_app.world())
            .expect("the humanoid loadout reaches the client");
        assert_eq!(*got, loadout, "the loadout round-trips byte-for-byte");
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

    /// EM-3.10b: a downsampled altitude grid survives `encode` → `decode`
    /// byte-for-byte (row-major, matching [`NetLodAlt::grid_size`]).
    #[test]
    fn net_lod_alt_round_trips() {
        let heights: Vec<f32> = (0..12).map(|i| i as f32 * 1.5).collect();
        let encoded = NetLodAlt::encode([4, 3], 8, &heights);
        assert_eq!(encoded.grid_size, [4, 3]);
        assert_eq!(encoded.chunk_stride, 8);
        assert!(!encoded.bytes.is_empty());

        let decoded = encoded.decode().expect("round-trips");
        assert_eq!(decoded, heights);
    }

    /// A payload whose decoded length doesn't match `grid_size` is rejected
    /// rather than silently misinterpreted (defensive against a future bug in
    /// the sender).
    #[test]
    fn net_lod_alt_rejects_length_mismatch() {
        let heights: Vec<f32> = vec![1.0, 2.0, 3.0];
        // `encode` doesn't validate its own input — 3 elements into a
        // declared 2×2=4 grid — so `decode` must catch the mismatch instead.
        let encoded = NetLodAlt::encode([2, 2], 4, &heights);
        assert_eq!(encoded.decode(), None);
    }

    /// `NetLodAlt` replicates server → client over the loopback exactly like
    /// [`TerrainAnchor`] (a plain one-shot server message).
    #[test]
    fn net_lod_alt_replicates() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut app = new_app();
        let heights = vec![10.0, 20.0, 30.0, 40.0];
        let payload = NetLodAlt::encode([2, 2], 16, &heights);
        app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: payload.clone(),
        });
        app.update();

        let received: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<NetLodAlt>>()
            .drain()
            .collect();
        assert_eq!(received, vec![payload]);
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
