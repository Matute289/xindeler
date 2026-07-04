//! EM-3.6 — listen-server mode (compiled only under the `listen-server`
//! feature).
//!
//! One process = the server SHELL (embedded Veloren sim + `SimBridgePlugin`
//! tick + `SimTerrainStreamPlugin` terrain drain) + the replicon SERVER role +
//! the pure-Bevy client plugins, all on ONE `App`. Terrain the sim streams
//! reaches the EM-3.5 mesh pipeline through the client's
//! [`crate::terrain_stream`] consumer via replicon's local loopback.
//!
//! ## Replicon pattern chosen: SINGLE-APP listen server (server role only)
//! bevy_replicon 0.41 documents two ways to run a listen server (src/lib.rs
//! "Abstracting over configurations"). The RECOMMENDED way runs *only the
//! server role* on the listen-server App and relies on replicon's local
//! message loopback: server messages written as `ToClients<M>` with
//! `SendTargets::All` are ALSO re-emitted locally as plain `M` in the same
//! world while `ClientState::Disconnected` holds (`server/message.rs`,
//! `send_locally` gated on `in_state(ClientState::Disconnected)`). We add
//! `RepliconPlugins` with the SERVER feature; we do NOT add the client role,
//! so there is no second world and no self-replication feedback loop (the
//! "classic way" two-worlds pitfall the crate warns about). The terrain bridge
//! writes `ToClients<CompressedChunk>`; the client's `terrain_stream` reads the
//! locally re-emitted `CompressedChunk` — no transport, no serialization over a
//! socket. Refs: replicon-0.41.1 `src/lib.rs` §"Abstracting over
//! configurations" and `src/server/message.rs`.
//!
//! ## Isolation
//! This module (client `src`) never touches `specs`. It only *adds plugins*
//! from `xindeler-sim-bridge` (the sole legal specs consumer under `bevy/`) and
//! registers the shared `xindeler-protocol`. The engine-isolation guard's
//! `specs` grep over `bevy/xindeler-client/src` stays clean.

use bevy::prelude::*;
use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
use xindeler_app::settings::userdata_dir;
use xindeler_protocol::XindelerProtocolPlugin;
use xindeler_sim_bridge::{SimBridgePlugin, SimTerrainStreamPlugin, boot_test_server};

use crate::terrain_stream::TerrainStreamPlugin;

/// Adds the whole listen-server stack to the client `App`.
///
/// Booting the world is SLOW (~5–10 s, asset + LFS dependent), so we boot it
/// here in `build` (before `app.run()`), rooted at the userdata dir, and insert
/// the `SimServer` as non-send data. If the boot fails (missing assets/LFS) we
/// log and add nothing — the app still runs (as an empty world) rather than
/// panicking the whole client.
pub struct ListenServerPlugin;

impl Plugin for ListenServerPlugin {
    fn build(&self, app: &mut App) {
        // Replicon SERVER role + the shared replication contract. `DefaultPlugins`
        // already provides `StatesPlugin` (needed by replicon's states). Terrain
        // messages are `make_message_independent`, so they flow regardless of the
        // entity replication tick; the listen-server loopback is driven by
        // `ClientState::Disconnected` (never a connected client here).
        app.add_plugins((
            // Replication tick on `PostUpdate` (every frame in this windowed
            // app — there is no fixed timestep configured; matches the
            // frame-paced sim tick). `ServerPlugin::new` interns the label for
            // us (same pattern as the protocol crate's tests).
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
            XindelerProtocolPlugin,
            // Server shell: sim tick + terrain drain (both server-side, specs
            // lives entirely inside these plugins).
            SimBridgePlugin,
            SimTerrainStreamPlugin,
            // Client-side consumer of the streamed terrain.
            TerrainStreamPlugin,
            // The pipeline needs the palette-derived ChunkLayerMap +
            // ChunkMaterials to mesh at all. The synthetic demo isn't added in
            // listen-server mode, so add the shared palette plugin here (it
            // installs both from `block_palette.ron`).
            crate::palette_material::PaletteMaterialPlugin,
        ));

        // Boot the embedded world now and hand it to the bridge.
        let data_dir = userdata_dir().join("listen-server");
        if let Err(err) = std::fs::create_dir_all(&data_dir) {
            error!(
                "listen-server: cannot create data dir {} ({err}); running without a world",
                data_dir.display()
            );
            return;
        }
        info!(
            "listen-server: booting embedded world at {} (this takes several seconds)…",
            data_dir.display()
        );
        match boot_test_server(&data_dir) {
            Ok(sim) => {
                app.insert_non_send(sim);
                info!("listen-server: embedded world booted; streaming terrain");
            },
            Err(err) => {
                error!(
                    "listen-server: failed to boot embedded world ({err}); running without a \
                     world (missing XINDELER_ASSETS / LFS map blobs?)"
                );
            },
        }
    }
}
