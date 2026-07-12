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

pub mod ai_mode;
pub mod aurora_overlay;
pub mod chat;
pub mod dimension_id;
pub mod hotbar;
pub mod interest;
pub mod inventory;
pub mod login;
pub mod map;
pub mod narrative;
pub mod owner_visibility;
pub mod social;
pub mod trade;
pub mod visibility;

use bevy::{
    app::{App, Plugin},
    ecs::{component::Component, message::Message, resource::Resource},
    math::{Quat, Vec2, Vec3},
};
use bevy_replicon::prelude::{
    AppRuleExt, AppVisibilityExt, Channel, ClientMessageAppExt, ServerMessageAppExt,
};
use common::terrain::TerrainChunk;
use serde::{Deserialize, Serialize};

pub use crate::{
    ai_mode::AiExecutionMode,
    aurora_overlay::{AuroraNpcState, AuroraOverlay, EmotionalState, IntentKind, MoodKind},
    chat::{ChatSendRequest, NetChatChannel, NetChatMsg},
    dimension_id::DimensionId,
    hotbar::{
        AssignHotbarSlot, LocalAssignHotbarSlot, NetAbilities, NetAuxiliaryAbility,
        NetCooldownEntry, NetCooldowns, NetHotbarSlot,
    },
    interest::{ClientInterestPlugin, ClientViewpoint, chunk_fuzz},
    inventory::{
        InventoryActionRequest, NetEquippedSlot, NetInventory, NetInventorySlot, NetItemStack,
    },
    login::{LoginError, LoginRequest, LoginResult, LoginSuccess, NetCharacterSummary},
    map::{MAP_IMAGE_MAX_DIM, NetMapData, NetMapMarker, NetMapPoi, NetPoiKind, wpos_to_screen_uv},
    narrative::{HudToast, HudToastPlugin, NarrativeHooks},
    owner_visibility::{ClientOwnedUid, NetOwnerOnly},
    social::{
        DialogueResponseRequest, GroupAction, GroupActionRequest, LocalDialogueResponse,
        LocalGroupAction, NetDialogue, NetGroupMember, NetGroupState, NetInviteKind,
        NetPendingInvite, NetPlayerList, NetPlayerListEntry,
    },
    trade::{
        NetIncomingTradeInvite, NetTrade, NetTradeOfferEntry, TradeActionRequest,
        TradeInviteRequest, TradeInviteResponseRequest,
    },
    visibility::{ClientVisibleRegions, RegionKey, region_key_for_pos},
};

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

/// Replicated energy (mana/stamina-equivalent) snapshot of an entity (BL-82
/// EM-5.2 — the first Phase-5 HUD mirror slice, spec §3.2/§6).
///
/// Flattened from the sim's `comp::Energy` (`current()`/`maximum()`), exactly
/// like [`NetHealth`] flattens `comp::Health` — the client only needs the
/// pair for the energy globe, ability-cost gating stays server-side.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetEnergy {
    pub current: f32,
    pub max: f32,
}

/// Replicated poise snapshot of an entity (BL-82 EM-5.2).
///
/// Flattened from the sim's `comp::Poise` (`current()`/`maximum()`) — the
/// HUD's poise indicator only needs the pair, poise-break math stays
/// server-side.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetPoise {
    pub current: f32,
    pub max: f32,
}

/// Replicated combo counter (BL-82 EM-5.2), flattened from `comp::Combo`.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct NetCombo {
    pub counter: u32,
}

/// Replicated character-level/XP-bar snapshot (BL-82 EM-5.2).
///
/// This fork derives `character_level` GLOBALLY from lifetime experience
/// across every `SkillSet` group (`common::comp::skillset`), not per-group —
/// see that module's doc comment. `xp_into_level`/`xp_for_level` are already
/// the COMPACT projection the XP bar needs (progress within the current
/// level + the level's total span), computed from
/// `SkillSet::total_earned_exp()`/`character_level()` and the free
/// `skillset::total_exp_for_level` helper — not the raw lifetime total (which
/// would make the client redo the same subtraction every frame for no
/// benefit — "project, don't dump").
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct NetXp {
    pub level: u16,
    pub xp_into_level: u32,
    pub xp_for_level: u32,
}

/// One active buff/debuff, projected for the HUD strip (BL-82 EM-5.2).
///
/// One entry per DISTINCT active [`common::comp::BuffKind`] on the entity
/// (not one per stack instance) — `strength`/`remaining_secs` describe the
/// kind's current CONTROLLING instance (the strongest, which
/// `comp::Buffs::iter_kind` already sorts first) and `stacks` counts how many
/// instances of that kind are active. This is the compact shape a buff icon
/// (icon + stack badge + duration ring) needs, not the sim's full `Buff`
/// (effects/source/category bookkeeping stays server-side).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct NetBuffEntry {
    pub kind: common::comp::buff::BuffKind,
    pub strength: f32,
    /// Seconds remaining, if the buff has a finite duration (`None` = a
    /// constant/permanent buff, matching `Buff::end_time: Option<Time>`).
    pub remaining_secs: Option<f32>,
    pub stacks: u32,
}

/// Replicated buff/debuff strip (BL-82 EM-5.2): every distinct active
/// [`common::comp::buff::BuffKind`] on the entity, flattened from
/// `comp::Buffs`. A `Vec` component (like [`NetLoadout`]'s strings) rather
/// than per-buff entities — bounded by the (small, fixed) `BuffKind` enum, so
/// this stays a "small `Net*` component", not a bulk-data message (spec
/// §3.2's "bulk data = messages" rule targets unbounded collections like
/// inventory, not this).
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NetBuffs(pub Vec<NetBuffEntry>);

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
/// only reshape the humanoid figure).
///
/// EM-3.8e adds `head` (a helmet item-def-id, resolved against a
/// species-keyed head-armour manifest) and `glider` + `gliding`. `gliding` is,
/// strictly speaking, transient CHARACTER STATE rather than equipped GEAR —
/// but it is bundled here (rather than as its own replicated component)
/// because it needs exactly the change-diffing/dedup machinery this struct
/// already has (`xindeler-sim-bridge::SimLoadoutCache`), and adding a whole
/// second component + registration + query for one `bool` would be more
/// machinery for no more correctness.
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
    /// Head armour (helmet) item-def-id (EM-3.8e). Unlike every other slot
    /// there is no generic "bare helmet" default — an unequipped head shows
    /// no extra mesh at all (the bare head model already IS the head), so
    /// `None` means exactly that, not "use a default helmet".
    pub head: Option<String>,
    /// Equipped glider item-def-id (EM-3.8e). This is the ITEM, independent
    /// of whether the character is currently airborne under it — see
    /// `gliding` for the transient visibility signal.
    pub glider: Option<String>,
    /// Whether the character is currently in a glide-shaped `CharacterState`
    /// (`Glide` or `GlideWield`) — the figure only shows the glider mesh
    /// while this is `true` (EM-3.8e), matching voxygen's own gating (its
    /// `Idle`/`Run` animations bake the glider bone's scale to zero; only
    /// `Glide`/`GlideWield` scale it back to one).
    pub gliding: bool,
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

/// Replicated stable sim identity of an entity (BL-82 EM-4.2f, spec §1.5).
///
/// Wraps the sim's `common::uid::Uid` inner value (a `NonZeroU64`, here a
/// plain `u64` since replicated components need no `common` dependency
/// beyond what already exists — this crate stays a type library over
/// `common`, never a `uid`-allocating one). Every sim entity already carries
/// a `Uid` (player and NPC alike; "for now we expect all entities have a Uid
/// component" — `server/src/state_ext.rs`), so `xindeler-sim-bridge`'s mirror
/// writes this alongside the existing `NetPos`/`NetBody`/etc. for every
/// mirrored entity.
///
/// Before this task, replicated mirror entities had no wire-visible identity
/// that survives a respawn/reconnect or correlates back to the sim's
/// `Uid`/`rtsim::NpcId` — `NetUid` is that correlation point. AURORA (BL-15/
/// BL-83, much later) keys [`AuroraOverlay`] entries by this same `u64` value
/// so it can say "this rendered figure IS `NpcId` X, with persistent
/// memories/relationships."
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NetUid(pub u64);

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

/// Frame-rate-predicted transform of the LOCAL player (BL-82 EM-4.11).
///
/// The listen-server bridge already embeds a full, correct, shared-code
/// client-side predictor — the `xindeler-client-core::Client` inside
/// `xindeler-sim-bridge::player` — the SAME predictor old (pre-Bevy) voxygen
/// used. This component carries that predictor's own per-`Update` (frame-rate)
/// output: `xindeler_sim_bridge::player::mirror_local_player_prediction`
/// writes it every frame from `EmbeddedPlayer::position()`/`velocity()`/
/// `orientation()` (Bevy axes, converted the same way the mirror converts the
/// rest of the sim state), and the render (`xindeler-client::entity_view`)
/// drives the local player's `Transform` from it DIRECTLY (a snap, not an
/// ease) instead of interpolating the authoritative, 30 Hz-sampled
/// [`NetPos`]/[`NetOri`]/[`NetVel`] the way every remote entity still does.
///
/// **NOT replicated.** It never crosses a socket — it is produced and
/// consumed inside the SAME process (the listen-server bridge writes it, the
/// listen-server's own client-side plugins read it), so registering it with
/// `.replicate::<>()` would be both wrong (replicon would try to serialize
/// player-local prediction state to remote clients, which never asked for it
/// and have no use for it — a real remote player is server-authoritative like
/// any other mirrored entity) and pointless (nothing on the wire needs it).
/// [`NetPos`]/[`NetOri`]/[`NetVel`] stay exactly as they are — the
/// reconciliation truth, the remote-entity render source, and the diagnostic
/// baseline — this component only ADDS a second, frame-rate-fresh source for
/// the ONE entity that has one.
#[derive(Component, Clone, Copy, Debug)]
pub struct PredictedLocalTransform {
    pub pos: Vec3,
    pub ori: Quat,
    /// Forward-looking: no current reader (`xindeler-client::entity_view`'s
    /// `interpolate_entities` only drives `Transform` from `pos`/`ori`; its
    /// own `Interpolated` presentation buffer has no velocity field either).
    /// Carried here anyway — mirroring every other mirrored entity's
    /// [`NetVel`] — for a future consumer (camera lean/tilt, animation blend
    /// weight, etc.) that wants the local player's own predicted velocity
    /// without a second lookup; negligible cost (one axis-swap per frame) to
    /// keep it current.
    pub vel: Vec3,
}

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

/// Server → client: the coarse far-terrain grid (EM-3.10b height, BL-82
/// EM-3.11 real colour), one downsampled sample per [`Self::chunk_stride`]²
/// chunks. Sent ONCE at boot — like [`TerrainAnchor`] — since the far terrain
/// never changes during a session, so there is no per-tick replication cost.
///
/// ## Source + downsampling
/// The embedded `client::Client`'s `world_data()` already packs one height
/// (`lod_alt`) and one colour (`lod_base`) sample per CHUNK (not per block),
/// but a default Veloren world is 1024×1024 chunks — far too many quads for a
/// "coarse" far-mesh. The server-side bridge (`xindeler-sim-bridge`) therefore
/// downsamples both layers together (simple stride-pick, capped grid
/// dimension, same `(i, j)` index for every layer) and decodes each sample via
/// the public `WorldData::alt_at`/`col_at` BEFORE sending, so the client does
/// zero Veloren-specific unpacking — it just reads floats and bytes.
///
/// ## Layering (spec §3.2)
/// Each layer is its own separately-compressed blob rather than one
/// struct-of-arrays, so a phase can ship its layer without touching the
/// others' decode paths: [`Self::horizon`] carries the BL-82 EM-3.11 Phase-B
/// horizon-occlusion layer (T49.5) — an empty blob still decodes as `None`
/// (⇒ [`Self::decode_horizon`] reports "layer absent"), which now covers a
/// missing/corrupt payload rather than "not shipped yet" (Phase A is merged).
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetFarTerrain {
    /// Downsampled grid width/height, in samples (row-major storage below).
    pub grid_size: [u32; 2],
    /// How many original chunk-grid cells one downsampled sample covers, on
    /// each axis. The client mesh spaces samples `chunk_stride *
    /// CHUNK_EDGE` Bevy metres apart.
    pub chunk_stride: u32,
    /// lz4-compressed bincode of the row-major `Vec<f32>` altitude samples
    /// (world-space metres). Row-major: `heights[y * grid_size[0] + x]`,
    /// matching `common::grid::Grid`'s convention. Decode with
    /// [`Self::decode_heights`].
    pub heights: Vec<u8>,
    /// lz4-compressed bincode of the row-major `Vec<[u8; 3]>` RGB colour
    /// samples (decoded server-side from `lod_base`'s packed RGBA — alpha is
    /// unused, see `client::WorldData::col_at`), index-aligned with
    /// [`Self::heights`]. Decode with [`Self::decode_colors`].
    pub colors: Vec<u8>,
    /// lz4-compressed bincode of the row-major `Vec<[u8; 4]>` packed
    /// west/east `(angle, occluder-height)` horizon records (BL-82 EM-3.11
    /// Phase B, decoded server-side via `client::WorldData::horizon_at`),
    /// index-aligned with [`Self::heights`]/[`Self::colors`]. Decode with
    /// [`Self::decode_horizon`].
    pub horizon: Vec<u8>,
}

impl NetFarTerrain {
    /// Serializes (bincode `legacy()`) + compresses (lz4, same scheme as
    /// [`CompressedChunk`]) a downsampled height + colour + horizon grid.
    ///
    /// `heights`/`colors`/`horizon` must be the SAME length and index-aligned
    /// (spec §3.1's contract — `colors[k]`/`horizon[k]` describe the exact
    /// same chunk `heights[k]` is the altitude of). This is a cheap,
    /// self-documenting guard against a future caller/refactor accidentally
    /// desyncing the parallel slices (ecs-design-reviewer, BL-82 EM-3.11
    /// Phase A review); the actual sampling loop
    /// (`xindeler-sim-bridge::send_far_terrain_once`) additionally
    /// structurally prevents this by pushing one `(height, colour, horizon)`
    /// tuple per cell rather than growing three Vecs independently.
    #[must_use]
    pub fn encode(
        grid_size: [u32; 2],
        chunk_stride: u32,
        heights: &[f32],
        colors: &[[u8; 3]],
        horizon: &[[u8; 4]],
    ) -> Self {
        debug_assert_eq!(
            heights.len(),
            colors.len(),
            "heights/colors must be index-aligned (same length)"
        );
        debug_assert_eq!(
            heights.len(),
            horizon.len(),
            "heights/horizon must be index-aligned (same length)"
        );
        Self {
            grid_size,
            chunk_stride,
            heights: Self::compress(heights),
            colors: Self::compress(colors),
            horizon: Self::compress(horizon),
        }
    }

    /// Serializes + compresses a slice with the shared lz4+bincode scheme
    /// used by every layer blob in this message (and [`CompressedChunk`]).
    fn compress<T: Serialize>(items: &[T]) -> Vec<u8> {
        let raw = bincode::serde::encode_to_vec(items, bincode::config::legacy())
            .expect("bincode serialization can only fail if a byte limit is set");
        let mut bytes = Vec::with_capacity(raw.len() / 4 + 16);
        let mut table = lz_fear::raw::U32Table::default();
        lz_fear::raw::compress2(&raw, 0, &mut table, &mut bytes)
            .expect("lz4 compression into a Vec<u8> is infallible");
        bytes
    }

    /// Decompresses + deserializes a layer blob. `None` for an empty blob
    /// (the "layer absent" contract, e.g. an unshipped [`Self::horizon`]) or a
    /// corrupt payload; callers additionally length-check against
    /// [`Self::grid_size`].
    fn decompress<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<Vec<T>> {
        if bytes.is_empty() {
            return None;
        }
        let mut raw = Vec::with_capacity(bytes.len() * 2);
        lz_fear::raw::decompress_raw(bytes, &[0; 0], &mut raw, usize::MAX).ok()?;
        bincode::serde::decode_from_slice(&raw, bincode::config::legacy())
            .ok()
            .map(|(items, _)| items)
    }

    /// Number of samples [`Self::grid_size`] declares — every layer's decoded
    /// length must match this exactly.
    fn expected_len(&self) -> usize { self.grid_size[0] as usize * self.grid_size[1] as usize }

    /// Decompresses + deserializes the height layer. `None` = corrupt payload
    /// or a length mismatch against [`Self::grid_size`] (defensive; the local
    /// loopback can't corrupt).
    #[must_use]
    pub fn decode_heights(&self) -> Option<Vec<f32>> {
        let heights: Vec<f32> = Self::decompress(&self.heights)?;
        (heights.len() == self.expected_len()).then_some(heights)
    }

    /// Decompresses + deserializes the colour layer, index-aligned with
    /// [`Self::decode_heights`]. `None` = corrupt payload or a length
    /// mismatch.
    #[must_use]
    pub fn decode_colors(&self) -> Option<Vec<[u8; 3]>> {
        let colors: Vec<[u8; 3]> = Self::decompress(&self.colors)?;
        (colors.len() == self.expected_len()).then_some(colors)
    }

    /// Decompresses + deserializes the (BL-82 EM-3.11 Phase-B) horizon layer.
    /// `None` for an empty blob (layer not yet shipped) as well as a corrupt
    /// payload or a length mismatch — callers cannot distinguish "absent"
    /// from "corrupt" and should treat both as "no horizon data available".
    #[must_use]
    pub fn decode_horizon(&self) -> Option<Vec<[u8; 4]>> {
        let horizon: Vec<[u8; 4]> = Self::decompress(&self.horizon)?;
        (horizon.len() == self.expected_len()).then_some(horizon)
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
///
/// ✅ RESOLVED (BL-82 EM-4.2d, spec §1.3): this used to carry a standing
/// obligation from the EM-3.6/3.7 reviewer — "replicon's default visibility
/// sends every `Replicated` entity to every client... MUST be wired
/// (EM-4.2d) before shipping default-all visibility past the listen-server
/// milestone." [`visibility::RegionKey`]'s `add_visibility_filter`
/// registration below is that wiring: `bevy_replicon`'s filters only scope
/// entities that actually CARRY the filter component, so an entity mirrored
/// WITH a [`visibility::RegionKey`] is only visible to a client whose
/// [`visibility::ClientVisibleRegions`] contains it (default: hidden, until
/// `xindeler-server-app`'s `recompute_client_visible_regions` scopes that
/// client) — `xindeler-sim-bridge`'s mirror attaches a `RegionKey` to every
/// entity it mirrors, so every entity this codebase actually replicates
/// today is scoped, not just newly-written ones. Entities that never carry a
/// `RegionKey` at all (e.g. this crate's own synthetic test fixtures) are
/// simply not affected by this filter — they keep replicon's ordinary
/// default-visible behavior, unrelated to the obligation above.
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
            // BL-82 EM-5.2: the first Phase-5 HUD state-mirror slice (spec
            // §3.2/§6) — energy/poise/combo/XP/buffs, following the
            // NetHealth/NetLoadout pattern exactly.
            .replicate::<NetEnergy>()
            .replicate::<NetPoise>()
            .replicate::<NetCombo>()
            .replicate::<NetXp>()
            .replicate::<NetBuffs>()
            // BL-82 EM-5.3: the skillbar/hotbar mirror — resolved
            // ability-pool/slot-binding projection + per-ability cooldowns.
            .replicate::<NetAbilities>()
            .replicate::<NetCooldowns>()
            // EM-3.7b: the local-player marker on the mirror entity so the
            // client's third-person camera knows which capsule to follow.
            .replicate::<NetLocalPlayer>()
            // EM-4.2f: the entity's stable sim identity (player or NPC), so a
            // future AURORA consumer can correlate a rendered figure back to
            // the sim's Uid/NpcId across respawns/reconnects.
            .replicate::<NetUid>()
            // BL-82 EM-5.6: the inventory/bag + two-party-trade mirrors
            // (spec §3.2/§6) — self-scoped via `NetOwnerOnly` below, NOT
            // broadcast like the entity-visible comps above.
            .replicate::<NetInventory>()
            .replicate::<NetTrade>()
            .replicate::<NetIncomingTradeInvite>();

        // Client → server messages. v0 keeps PlayerInput on the ordered lane
        // (no client-side redundancy/resampling yet); it moves to the
        // unreliable State lane once the input stream sends redundant samples.
        app.add_client_message::<PlayerInput>(XindelerChannel::Events.delivery());
        // BL-82 EM-4.2c: the login/session handshake. Ordered/reliable like
        // PlayerInput — this is a one-shot discrete request, not a per-tick
        // state sample.
        app.add_client_message::<LoginRequest>(XindelerChannel::Events.delivery());
        // BL-82 EM-5.4: a player-typed chat line or slash command — a
        // one-shot discrete request like LoginRequest, not a per-tick state
        // sample.
        app.add_client_message::<ChatSendRequest>(XindelerChannel::Events.delivery());
        // BL-82 EM-5.3: hotbar drag-to-assign — the real wire shape for a
        // future genuinely-remote client (dormant today, same posture as
        // EM-5.4's `ChatSendRequest`/EM-5.8's `GroupActionRequest`: no
        // server-side handler exists for `FromClient<AssignHotbarSlot>` yet).
        app.add_client_message::<hotbar::AssignHotbarSlot>(XindelerChannel::Events.delivery());
        // The listen-server in-process counterpart (see `hotbar`'s own doc
        // comment) — a plain Bevy message, not a replicon message; it never
        // crosses a socket.
        app.add_message::<hotbar::LocalAssignHotbarSlot>();
        // BL-82 EM-5.6: discrete, infrequent gameplay-intent requests
        // (inventory moves, trade invites/actions) — the Events lane
        // (ordered/reliable), same class as LoginRequest above, not the
        // high-frequency State lane.
        app.add_client_message::<InventoryActionRequest>(XindelerChannel::Events.delivery());
        app.add_client_message::<TradeInviteRequest>(XindelerChannel::Events.delivery());
        app.add_client_message::<TradeInviteResponseRequest>(XindelerChannel::Events.delivery());
        app.add_client_message::<TradeActionRequest>(XindelerChannel::Events.delivery());

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
        // EM-3.10b (+ BL-82 EM-3.11 Phase A real colour): the far-terrain
        // height+colour grid, sent once (same ONE-SHOT TIMING as
        // TerrainAnchor — decoupled from entity replication) but on the
        // `Terrain` channel, NOT `Events` (review should-fix #3): its payload
        // is up to ~112 KB compressed (128×128 f32 heights + [u8;3] colours),
        // size-class-comparable to `CompressedChunk` above, not a small
        // discrete event. `Events` is Ordered/reliable — a multi-KB blob
        // there would head-of-line-block chat/connect/disconnect messages
        // behind it, exactly what `Terrain` (Unordered/reliable) exists to
        // avoid.
        app.add_server_message::<NetFarTerrain>(XindelerChannel::Terrain.delivery())
            .make_message_independent::<NetFarTerrain>();
        // BL-82 EM-5.5: the one-shot world map broadcast (background image +
        // site/POI markers). Same one-shot timing + channel choice as
        // `NetFarTerrain` above (a multi-KB image blob belongs on the
        // Unordered/reliable `Terrain` lane, not `Events`, for the exact same
        // head-of-line-blocking reason documented there) — see `map.rs`'s
        // module doc comment.
        app.add_server_message::<map::NetMapData>(XindelerChannel::Terrain.delivery())
            .make_message_independent::<map::NetMapData>();

        // BL-82 EM-4.2c: the login/session handshake reply. Carries no
        // entity references (like TerrainAnchor/NetFarTerrain above), so it
        // must not be queued behind entity replication either.
        app.add_server_message::<LoginResult>(XindelerChannel::Events.delivery())
            .make_message_independent::<LoginResult>();
        // BL-82 EM-4.8: the narrative on-enter-message toast
        // (`narrative::fire_on_enter_toasts`). Carries no entity references
        // (a plain text notice, like `LoginResult`/`TerrainAnchor` above), so
        // it must not be queued behind entity replication either.
        app.add_server_message::<HudToast>(XindelerChannel::Events.delivery())
            .make_message_independent::<HudToast>();
        // BL-82 EM-5.4: the chat message stream. No entity references (a
        // plain classified/rendered line), so it must not be queued behind
        // entity replication either — same reasoning as `HudToast` above.
        app.add_server_message::<NetChatMsg>(XindelerChannel::Events.delivery())
            .make_message_independent::<NetChatMsg>();
        // BL-82 EM-4.2d: per-client interest management. Registering this
        // filter does NOT itself add `RegionKey`/`ClientVisibleRegions` to any
        // entity — it only teaches replicon how to interpret them where they
        // ARE present (`xindeler-sim-bridge`'s mirror writes `RegionKey`;
        // `xindeler-server-app::visibility::recompute_client_visible_regions`
        // writes `ClientVisibleRegions`). See `visibility`'s module doc
        // comment for the full design and why registering it symmetrically
        // here (rather than only server-side) is safe.
        app.add_visibility_filter::<visibility::RegionKey>();

        // BL-82 EM-5.8: social/group/dialogue wire contract (NetPlayerList/
        // NetGroupState/NetDialogue server messages, GroupActionRequest/
        // DialogueResponseRequest client messages, LocalGroupAction/
        // LocalDialogueResponse in-process handoff) — see `social`'s own
        // module doc comment for the full rationale.
        social::register(app);
        // BL-82 EM-5.6: per-owner scoping for `NetInventory`/`NetTrade`/
        // `NetIncomingTradeInvite` — see `owner_visibility`'s module doc
        // comment. Independent of (and additive alongside) the RegionKey
        // filter above: this one only hides three specific components, never
        // the whole entity.
        app.add_visibility_filter::<owner_visibility::NetOwnerOnly>();
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
            // EM-3.8e: exercise the new head/glider/gliding fields too.
            head: Some("common.items.armor.mail.bronze.head".to_owned()),
            glider: Some("common.items.glider.basic_white".to_owned()),
            gliding: true,
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

    /// BL-82 EM-5.2: the first Phase-5 HUD mirror slice (`NetEnergy`/
    /// `NetPoise`/`NetCombo`/`NetXp`/`NetBuffs`) round-trips server → client
    /// exactly like `NetHealth`/`NetLoadout` above — this is the acceptance
    /// bar `ecs-design-reviewer` checks for "every new `Net*` mirror gets a
    /// round-trip test" (spec §3.2/plan "Every mirror PR").
    #[test]
    fn combat_hud_mirror_replicates() {
        use common::comp::buff::BuffKind;

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let energy = NetEnergy {
            current: 42.0,
            max: 100.0,
        };
        let poise = NetPoise {
            current: 10.0,
            max: 30.0,
        };
        let combo = NetCombo { counter: 7 };
        let xp = NetXp {
            level: 5,
            xp_into_level: 120,
            xp_for_level: 500,
        };
        let buffs = NetBuffs(vec![NetBuffEntry {
            kind: BuffKind::Regeneration,
            strength: 2.5,
            remaining_secs: Some(9.5),
            stacks: 2,
        }]);

        server_app
            .world_mut()
            .spawn((Replicated, energy, poise, combo, xp, buffs.clone()));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app
            .world_mut()
            .query::<(&NetEnergy, &NetPoise, &NetCombo, &NetXp, &NetBuffs)>();
        let (got_energy, got_poise, got_combo, got_xp, got_buffs) = q
            .single(client_app.world())
            .expect("the combat-HUD mirror reaches the client");
        assert_eq!(*got_energy, energy);
        assert_eq!(*got_poise, poise);
        assert_eq!(*got_combo, combo);
        assert_eq!(*got_xp, xp);
        assert_eq!(*got_buffs, buffs, "buff strip round-trips byte-for-byte");
    }

    /// BL-82 EM-4.2f acceptance: `NetUid` (the mirrored entity's stable sim
    /// identity) round-trips server → client for BOTH a player-shaped
    /// (`NetLocalPlayer`-tagged humanoid) and an NPC-shaped
    /// (`Body::QuadrupedSmall`) mirrored entity — mirrors
    /// `net_loadout_replicates`'s shape, just for the new identity
    /// component.
    #[test]
    fn net_uid_replicates_for_player_and_npc() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let player_uid = NetUid(1);
        let player_body = NetBody(common::comp::Body::Humanoid(common::comp::humanoid::Body {
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
        let npc_uid = NetUid(2);
        let npc_body = NetBody(common::comp::Body::QuadrupedSmall(
            common::comp::quadruped_small::Body {
                species: common::comp::quadruped_small::Species::Pig,
                body_type: common::comp::quadruped_small::BodyType::Female,
            },
        ));

        server_app
            .world_mut()
            .spawn((Replicated, player_body, player_uid, NetLocalPlayer));
        server_app
            .world_mut()
            .spawn((Replicated, npc_body, npc_uid));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut player_q = client_app
            .world_mut()
            .query::<(&NetUid, &NetBody, &NetLocalPlayer)>();
        let (got_player_uid, got_player_body, _) = player_q
            .single(client_app.world())
            .expect("the player-shaped entity reaches the client with its NetUid");
        assert_eq!(*got_player_uid, player_uid);
        assert_eq!(*got_player_body, player_body);

        let mut npc_q = client_app
            .world_mut()
            .query_filtered::<(&NetUid, &NetBody), Without<NetLocalPlayer>>();
        let (got_npc_uid, got_npc_body) = npc_q
            .single(client_app.world())
            .expect("the NPC-shaped entity reaches the client with its NetUid");
        assert_eq!(*got_npc_uid, npc_uid);
        assert_eq!(*got_npc_body, npc_body);
    }

    /// BL-82 EM-4.2c: `LoginRequest` travels client → server (surfacing as
    /// `FromClient<_>`) and the server's `LoginResult` reply travels back —
    /// the wire-shape half of the login handshake; the actual auth/
    /// persistence logic is exercised by `xindeler-server-app`'s own
    /// acceptance test, not here.
    #[test]
    fn login_request_and_result_round_trip() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let request = LoginRequest {
            token_or_username: "test_user".to_owned(),
            locale: "en-US".to_owned(),
        };
        client_app.world_mut().write_message(request.clone());

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<LoginRequest>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1, "server should receive one login request");
        assert_eq!(received[0].message, request);

        let result = LoginResult {
            outcome: Ok(LoginSuccess {
                characters: vec![NetCharacterSummary {
                    id: common::character::CharacterId(1),
                    alias: "Hero".to_owned(),
                    body: common::comp::Body::Humanoid(common::comp::humanoid::Body {
                        species: common::comp::humanoid::Species::Human,
                        body_type: common::comp::humanoid::BodyType::Male,
                        hair_style: 0,
                        beard: 0,
                        eyes: 0,
                        accessory: 0,
                        hair_color: 0,
                        skin: 0,
                        eye_color: 0,
                    }),
                }],
                selected: Some(common::character::CharacterId(1)),
            }),
        };
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: result.clone(),
        });
        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let received: Vec<_> = client_app
            .world_mut()
            .resource_mut::<Messages<LoginResult>>()
            .drain()
            .collect();
        assert_eq!(received, vec![result]);
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

    /// EM-3.10b (+ BL-82 EM-3.11 Phase A colour, Phase B horizon): a
    /// downsampled altitude+colour+horizon grid survives `encode` →
    /// `decode_heights`/`decode_colors`/`decode_horizon` byte-for-byte
    /// (row-major, matching [`NetFarTerrain::grid_size`]).
    #[test]
    fn net_far_terrain_round_trips() {
        let heights: Vec<f32> = (0..12).map(|i| i as f32 * 1.5).collect();
        let colors: Vec<[u8; 3]> = (0..12).map(|i| [i as u8, i as u8 * 2, 255]).collect();
        let horizon: Vec<[u8; 4]> = (0..12)
            .map(|i| [i as u8, i as u8 * 3, i as u8 * 5, 255])
            .collect();
        let encoded = NetFarTerrain::encode([4, 3], 8, &heights, &colors, &horizon);
        assert_eq!(encoded.grid_size, [4, 3]);
        assert_eq!(encoded.chunk_stride, 8);
        assert!(!encoded.heights.is_empty());
        assert!(!encoded.colors.is_empty());
        assert!(
            !encoded.horizon.is_empty(),
            "Phase B populates the horizon layer"
        );

        assert_eq!(encoded.decode_heights().expect("round-trips"), heights);
        assert_eq!(encoded.decode_colors().expect("round-trips"), colors);
        assert_eq!(encoded.decode_horizon().expect("round-trips"), horizon);
    }

    /// An empty horizon blob (e.g. a message built without ever sampling
    /// `lod_horizon`) decodes as `None` — "layer absent", not corrupt. Kept
    /// as its own test now that Phase B normally populates the layer, so the
    /// "absent" contract itself stays covered.
    #[test]
    fn net_far_terrain_empty_horizon_decodes_as_absent() {
        let heights: Vec<f32> = vec![1.0, 2.0];
        let colors: Vec<[u8; 3]> = vec![[1, 2, 3], [4, 5, 6]];
        let mut encoded = NetFarTerrain::encode([2, 1], 8, &heights, &colors, &[[0, 0, 0, 0]; 2]);
        encoded.horizon = Vec::new();
        assert_eq!(
            encoded.decode_horizon(),
            None,
            "an empty horizon blob decodes as 'layer absent'"
        );
    }

    /// A payload whose decoded length doesn't match `grid_size` is rejected
    /// rather than silently misinterpreted (defensive against a future bug in
    /// the sender) — checked independently for the height, colour, AND
    /// horizon layers, since each is its own compressed blob.
    #[test]
    fn net_far_terrain_rejects_length_mismatch() {
        // `heights`/`colors`/`horizon` are index-aligned with EACH OTHER (all
        // length 3, `encode`'s own `debug_assert_eq!` contract), but none
        // matches the DECLARED 2×2=4 grid — `decode_*` must catch that
        // mismatch.
        let heights: Vec<f32> = vec![1.0, 2.0, 3.0];
        let colors: Vec<[u8; 3]> = vec![[1, 2, 3], [4, 5, 6], [7, 8, 9]];
        let horizon: Vec<[u8; 4]> = vec![[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]];
        let encoded = NetFarTerrain::encode([2, 2], 4, &heights, &colors, &horizon);
        assert_eq!(encoded.decode_heights(), None);
        assert_eq!(encoded.decode_colors(), None);
        assert_eq!(encoded.decode_horizon(), None);
    }

    /// `NetFarTerrain` replicates server → client over the loopback exactly
    /// like [`TerrainAnchor`] (a plain one-shot server message), carrying the
    /// height, colour, AND horizon layers together.
    #[test]
    fn net_far_terrain_replicates() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut app = new_app();
        let heights = vec![10.0, 20.0, 30.0, 40.0];
        let colors: Vec<[u8; 3]> = vec![[10, 20, 30], [40, 50, 60], [70, 80, 90], [100, 110, 120]];
        let horizon: Vec<[u8; 4]> = vec![[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12], [
            13, 14, 15, 16,
        ]];
        let payload = NetFarTerrain::encode([2, 2], 16, &heights, &colors, &horizon);
        app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: payload.clone(),
        });
        app.update();

        let received: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<NetFarTerrain>>()
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

    /// BL-82 EM-5.4: `NetChatMsg` replicates server → client over the real
    /// loopback exactly like `HudToast`/`LoginResult` (a plain broadcast
    /// message, no entity references).
    #[test]
    fn net_chat_msg_replicates() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let payload = NetChatMsg {
            channel: NetChatChannel::Say,
            sender_uid: Some(NetUid(7)),
            sender_alias: Some("Hero".to_owned()),
            text: "hello there".to_owned(),
        };
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: payload.clone(),
        });
        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let received: Vec<_> = client_app
            .world_mut()
            .resource_mut::<Messages<NetChatMsg>>()
            .drain()
            .collect();
        assert_eq!(received, vec![payload]);
    }

    /// Listen-server path: `ToClients<NetChatMsg>` with no connected client
    /// loops back locally, same as `compressed_chunk_loops_back_locally_on_
    /// listen_server` above — this is how the listen-server's own chat
    /// broadcast (`xindeler-sim-bridge::chat::broadcast_embedded_chat`)
    /// reaches its own embedded player's client-side scrollback.
    #[test]
    fn net_chat_msg_loops_back_locally_on_listen_server() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut app = new_app();
        let payload = NetChatMsg {
            channel: NetChatChannel::World,
            sender_uid: None,
            sender_alias: None,
            text: "server started".to_owned(),
        };
        app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: payload.clone(),
        });
        app.update();

        let received: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<NetChatMsg>>()
            .drain()
            .collect();
        assert_eq!(received, vec![payload]);
    }

    /// BL-82 EM-5.4: `ChatSendRequest` (both the `Channel` and `Command`
    /// shapes) travels client → server and surfaces as `FromClient<_>` —
    /// the wire-shape half for a future real remote client, mirroring
    /// `player_input_reaches_server` above.
    #[test]
    fn chat_send_request_reaches_server() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let channel_req = ChatSendRequest::Channel {
            channel: NetChatChannel::Say,
            text: "hello".to_owned(),
        };
        let command_req = ChatSendRequest::Command {
            name: "tell".to_owned(),
            args: vec!["Bob".to_owned(), "hi".to_owned()],
        };
        client_app.world_mut().write_message(channel_req.clone());
        client_app.world_mut().write_message(command_req.clone());

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<ChatSendRequest>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 2, "server should receive both requests");
        assert_eq!(received[0].message, channel_req);
        assert_eq!(received[1].message, command_req);
    }
}
