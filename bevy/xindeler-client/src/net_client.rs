//! EM-4.2b — client-role counterpart to the new real replicon+quinnet
//! transport (compiled only under the `net-client` cargo feature).
//!
//! Unlike [`crate::listen_server`] (which HOSTS an embedded sim +
//! `bevy_replicon` SERVER role in-process, looping back locally through
//! replicon's `send_locally` path), this module is a genuinely REMOTE THIN
//! CLIENT: it adds `bevy_replicon`'s CLIENT role + `XindelerProtocolPlugin` +
//! `xindeler_transport::QuinnetTransport::client_plugins(...)` — never
//! touching `bevy_replicon_quinnet`/`bevy_quinnet` types directly, see
//! `xindeler-transport`'s own doc comment — and connects over a REAL
//! loopback/network UDP socket to a SEPARATE `xindeler-server-app` process.
//!
//! It reuses the SAME client-side consumption plugins the listen-server path
//! uses ([`crate::terrain_stream::TerrainStreamPlugin`],
//! [`crate::entity_view::EntityViewPlugin`],
//! [`crate::figure_view::FigureViewPlugin`],
//! [`crate::sprite_view::SpriteViewPlugin`],
//! [`crate::lod::LodCullingPlugin`], [`crate::far_terrain::FarTerrainPlugin`])
//! VERBATIM — the wire shape (`xindeler-protocol`) does not change, only the
//! transport underneath it and who hosts the sim.
//!
//! ## Scope: spectator-only (v1)
//! There is no embedded local player and no `xindeler-sim-bridge` dependency
//! at all — login/session handshake (EM-4.2c) is a separate, not-yet-landed
//! task, so this mode has no way to authenticate a controllable character
//! yet. It behaves like the listen-server's own "no embedded player" fallback:
//! a spectator camera parked over the server's terrain anchor
//! ([`crate::terrain_stream::place_camera_on_anchor`]), watching whatever the
//! server mirrors (the wandering test NPCs `xindeler_sim_bridge::
//! spawn_test_npcs` spawns). `xindeler-server-app`'s own acceptance test uses
//! the `XINDELER_SERVER_NO_AUTH` escape hatch on the SERVER side for this same
//! reason — no new auth machinery is needed here either.
//!
//! ## Purity
//! This module — and everything it activates — uses NO `specs`, and (unlike
//! `listen-server`) doesn't even link `xindeler-sim-bridge`: a real remote
//! client has nothing to host. The engine-isolation guard's grep over this
//! crate's `src` stays clean, more directly than under `listen-server`'s own
//! sanctioned exception.

use bevy::prelude::*;
use bevy_replicon::prelude::RepliconPlugins;
use xindeler_oracle_host::AtmosphereSyncMessagePlugin;
use xindeler_protocol::XindelerProtocolPlugin;
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

use crate::{
    atmosphere::AtmosphereSyncViewPlugin, chat::ChatViewPlugin, combat_hud::CombatHudViewPlugin,
    controls_screen::ControlsScreenPlugin, entity_view::EntityViewPlugin,
    far_terrain::FarTerrainPlugin, figure_view::FigureViewPlugin, hotbar::HotbarViewPlugin,
    hud_toast::HudToastViewPlugin, inventory_ui::InventoryUiPlugin, lod::LodCullingPlugin,
    map_view::MapViewPlugin, palette_material::PaletteMaterialPlugin,
    social_hud::SocialHudViewPlugin, sprite_view::SpriteViewPlugin,
    terrain_stream::TerrainStreamPlugin, trade_ui::TradeUiPlugin,
};

/// Adds the whole net-client stack to the client `App`: `bevy_replicon`'s
/// client role, the shared protocol, the real transport (dialing
/// `self.config.server_addr`), and the client-side consumption plugins listed
/// in the module doc comment.
pub struct NetClientPlugin {
    pub config: TransportConfig,
}

impl Plugin for NetClientPlugin {
    fn build(&self, app: &mut App) {
        info!(
            server_addr = %self.config.server_addr,
            "net-client: connecting to a remote xindeler-server-app over the replicon+quinnet \
             transport (EM-4.2b)"
        );

        // `bevy_replicon`'s CLIENT role (unconfigured `RepliconPlugins`
        // already includes it — see `xindeler-transport`'s crate doc
        // comment, "Both client+server bevy_replicon roles compile into
        // EVERY shell", for why both roles' ECS scaffolding being
        // technically present here is harmless: only the role with an
        // actually-open transport connection does anything) + the shared
        // replication contract. Must precede the transport plugins (see
        // `QuinnetTransport`'s `Startup`-ordering doc comment for why this
        // is a correctness requirement, not a style preference).
        app.add_plugins((RepliconPlugins, XindelerProtocolPlugin));
        // BL-82 EM-4.9 (Phase D): symmetric `SetClientAtmosphere` message
        // registration — see `xindeler_oracle_host::atmosphere_sync`'s
        // module doc comment for why this can't live in
        // `XindelerProtocolPlugin` itself.
        app.add_plugins(AtmosphereSyncMessagePlugin);

        // The transport seam: dials `self.config.server_addr` at `Startup`.
        app.add_plugins(QuinnetTransport.client_plugins(&self.config));

        // Client-side presentation — verbatim reuse of the listen-server
        // path's own consumer plugins (see module doc comment).
        app.add_plugins((
            TerrainStreamPlugin,
            EntityViewPlugin,
            FigureViewPlugin,
            SpriteViewPlugin,
            LodCullingPlugin,
            FarTerrainPlugin,
            // The chunk pipeline needs the palette-derived ChunkLayerMap +
            // ChunkMaterials to mesh at all (same reason
            // `listen_server::ListenServerPlugin` adds this).
            PaletteMaterialPlugin,
            // BL-82 EM-4.8: minimal timed-fade `bevy_ui` toast, rendered on
            // `HudToast` arrival — verbatim reuse, same as every other
            // consumer plugin in this list (module doc comment).
            HudToastViewPlugin,
            // BL-82 EM-4.9 (Phase D): retargets `AtmosphereController` on
            // `SetClientAtmosphere` arrival.
            AtmosphereSyncViewPlugin,
            // BL-82 EM-5.2: the core combat HUD (health/energy/poise/XP/
            // combo globes, buff strip, crosshair, death/respawn) reading
            // the EM-5.2 mirror off `NetLocalPlayer` — verbatim reuse, same
            // as every other consumer plugin in this list.
            CombatHudViewPlugin,
            // BL-82 EM-5.8: the social/group/dialogue HUD — on this
            // spectator-only remote path there is no embedded player to
            // act for, so this only ever shows the (real, broadcast)
            // `NetPlayerList`; group/dialogue actions the UI writes simply
            // have no consumer yet (a genuinely remote group/dialogue
            // gameplay path is a follow-up, see `social_hud`'s own module
            // doc comment).
            SocialHudViewPlugin,
            // BL-82 EM-5.5: the minimap + full map screens — verbatim reuse,
            // same as every other consumer plugin in this list. Consumer-
            // only here: `MapDataStreamPlugin` (the `NetMapData` producer) is
            // an `EmbeddedPlayer`-reading listen-server-only plugin (mirrors
            // `LodAltStreamPlugin`'s own exact limitation, module doc
            // comment) — this spectator-only net-client mode never boots an
            // `EmbeddedPlayer`, so the map screens simply stay empty here
            // (spec §3.2 "degrade clean"), same as the far-terrain mesh does
            // today.
            MapViewPlugin,
            // BL-82 EM-5.11: the input-rebinding screen (keyboard/mouse +
            // gamepad) — reuses `CombatHudViewPlugin`'s own `XindelerUiPlugin`
            // registration, same as every other consumer plugin in this list.
            ControlsScreenPlugin,
            // BL-82 EM-5.4: the chat panel — verbatim reuse too. The
            // RECEIVE-side code (NetChatMsg -> scrollback) is wire-shape
            // correct over this real transport, but currently moot in
            // practice: `xindeler-server-app` (the server this path
            // connects to) has no chat bridge wired up at all yet — a
            // disclosed gap, see `xindeler-sim-bridge::chat`'s own module
            // doc comment and `docs/backlog/engine-migration.md`'s EM-5.4
            // row. SENDING is ALSO a no-op until EM-4.2c's login lands a
            // controllable session here (there is no
            // `xindeler-sim-bridge`/embedded player on this path at all —
            // see this module's own doc comment), so a typed line simply
            // queues a `ChatSendRequest` nobody answers yet, degrading
            // clean rather than panicking.
            ChatViewPlugin,
            // BL-82 EM-5.3: the skillbar/hotbar screen — verbatim reuse, same
            // as every other consumer plugin in this list. This mode has no
            // embedded/controllable local player yet (module doc comment,
            // "Scope: spectator-only (v1)"), so every one of its systems
            // degrades clean (no `NetLocalPlayer` entity to query) exactly
            // like `CombatHudViewPlugin` already does here.
            HotbarViewPlugin,
            // BL-82 EM-5.6: the inventory/bag + paper-doll screen and the
            // two-party trade window — verbatim reuse, same as every other
            // consumer plugin in this list (both are pure Bevy, reading the
            // `NetInventory`/`NetTrade`/`NetIncomingTradeInvite` mirrors the
            // SAME way regardless of which transport carried them here).
            InventoryUiPlugin,
            TradeUiPlugin,
        ));
    }
}
