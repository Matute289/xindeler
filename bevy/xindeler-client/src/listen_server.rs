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

use std::time::Duration;

use bevy::prelude::*;
use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
use xindeler_app::settings::userdata_dir;
use xindeler_oracle_host::AtmosphereSyncMessagePlugin;
use xindeler_protocol::XindelerProtocolPlugin;
use xindeler_sim_bridge::{
    CombatHudMirrorPlugin, LodAltStreamPlugin, MapDataStreamPlugin, PlayerBridgePlugin,
    PlayerTransferPlugin, SIM_TICK_HZ, SimBridgePlugin, SimEntityMirrorPlugin,
    SimTerrainStreamPlugin, boot_embedded_player, boot_test_server,
};

use crate::{
    atmosphere::AtmosphereSyncViewPlugin, combat_hud::CombatHudViewPlugin,
    entity_view::EntityViewPlugin, far_terrain::FarTerrainPlugin, figure_view::FigureViewPlugin,
    hud_toast::HudToastViewPlugin, lod::LodCullingPlugin, map_view::MapViewPlugin,
    player_input::PlayerInputPlugin, sprite_view::SpriteViewPlugin,
    terrain_stream::TerrainStreamPlugin,
};

/// Adds the whole listen-server stack to the client `App`.
///
/// Booting the world is SLOW (~5–10 s, asset + LFS dependent), so we boot it
/// here in `build` (before `app.run()`), rooted at the userdata dir, and insert
/// the `SimServer` as non-send data. If the boot fails (missing assets/LFS) we
/// log and add nothing — the app still runs (as an empty world) rather than
/// panicking the whole client.
///
/// ## EM-3.7 scope: PASSIVE mirror (A) + CONTROLLABLE player (B, EM-3.7b).
/// EM-3.7 proved the passive half: the sim's entities (rtsim NPCs + test Pigs)
/// replicate to the pure-Bevy client as interpolated placeholder capsules
/// (`EntityViewPlugin`) on the real streamed terrain.
///
/// EM-3.7b adds a CONTROLLABLE local player, via the EM-1.6 smoke-bot pattern:
/// an embedded `xindeler-client-core::Client` lives inside
/// `xindeler-sim-bridge` (server-side crate — the only place a second
/// sim/client stack is legal), connects over TCP loopback to the sim we boot
/// here, creates/selects a default character, and spawns in-game
/// (`boot_embedded_player` + `PlayerBridgePlugin`). The Bevy keyboard/mouse
/// (read by `player_input`, reusing the fly-cam's grab + yaw) becomes a
/// `xindeler_protocol::LocalPlayerInput` resource that the
/// bridge's `tick_player` applies to that Client's `ControllerInputs` each
/// frame. The bridge tags the player's mirror entity with `NetLocalPlayer`, and
/// the client's third-person camera (`player_input::third_person_camera`)
/// follows it; `F` toggles back to the free fly-cam for debugging.
///
/// The embedded player IS the terrain presence (holds a `Presence`), so the
/// EM-3.6 `create_centered_persister` anchor is now only a FALLBACK — spawned
/// when the player is absent or fails to connect (spectator mode). Trade-off:
/// the listen server hosts the sim AND a loopback Client, heavier than the
/// passive persister, which is why the two halves were split.
pub struct ListenServerPlugin;

impl Plugin for ListenServerPlugin {
    fn build(&self, app: &mut App) {
        // EM-3.11b: pace the embedded sim + player at the sim's real 30 TPS
        // via `FixedUpdate`, decoupled from the window's display-rate
        // `Update`. Previously `tick_sim`/`tick_player` ran once per `Update`
        // — 60–144 Hz on the dev machines that hit this — which ran the FULL
        // specs system graph (physics, agent AI, terrain streaming, ...)
        // 2–5× too often and fed it a variable, non-30-Hz `dt`. See
        // `xindeler_sim_bridge::tick_sim`'s doc for the full root-cause
        // writeup and `docs/backlog/engine-migration.md` EM-3.11b for the
        // before/after measurement.
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));

        // EM-3.11c (hotfix, 2026-07-09): `Time<Fixed>`'s catch-up accumulator
        // follows `Time<Virtual>`'s `max_delta`, which Bevy defaults to 250ms
        // — Bevy's own docs name the failure mode this enables verbatim: a
        // "death spiral" where, if a single FixedUpdate step already takes
        // longer than its 1/SIM_TICK_HZ budget (confirmed: the still-open
        // EM-3.11b "slow server tick" finding measured 50-200ms/tick, already
        // over the ~33ms budget at 30Hz), the accumulator lets MULTIPLE
        // catch-up ticks queue and run back-to-back in one render frame,
        // each one itself slow, falling further behind every frame — this is
        // exactly what turned the EM-3.11b FixedUpdate migration into a
        // regression from ~29fps to ~3-4fps (Matías, in-game, same day).
        // Clamping `max_delta` to one tick's own period caps FixedUpdate to
        // AT MOST one catch-up step per render frame — the game degrades to
        // "as slow as a single tick takes" under load instead of spiraling
        // further. This does NOT fix the underlying slow-tick root cause
        // (still open, likely agent-AI/rtsim pathfinding cost per the perf
        // agent's PR #34 report) — it only removes the compounding-catchup
        // amplifier on top of it. **EM-3.11d** (this branch) profiles the
        // underlying slow-tick cost itself — see
        // `xindeler_sim_bridge::tick_sim` / `common_ecs::Job::run` /
        // `server::Server::tick`'s "Slow server tick" log for the
        // instrumentation and `docs/backlog/engine-migration.md` EM-3.11d for
        // the findings.
        app.insert_resource(Time::<Virtual>::from_max_delta(Duration::from_secs_f64(
            1.0 / SIM_TICK_HZ,
        )));

        // Replicon SERVER role + the shared replication contract. `DefaultPlugins`
        // already provides `StatesPlugin` (needed by replicon's states). Terrain
        // messages are `make_message_independent`, so they flow regardless of the
        // entity replication tick; the listen-server loopback is driven by
        // `ClientState::Disconnected` (never a connected client here).
        app.add_plugins((
            // Replication tick on `PostUpdate` (every RENDER frame — display
            // rate, deliberately NOT tied to the sim's `FixedUpdate` cadence,
            // EM-3.11b: replicon just no-ops when nothing changed since the
            // last check). `ServerPlugin::new` interns the label for us (same
            // pattern as the protocol crate's tests).
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
            XindelerProtocolPlugin,
            // Server shell: sim tick + terrain drain (both server-side, specs
            // lives entirely inside these plugins).
            SimBridgePlugin,
            SimTerrainStreamPlugin,
            // Server shell: the embedded local-player Client tick + input apply
            // (EM-3.7b). Specs/client crate stays inside the bridge.
            //
            // EM-3.11o: registered BEFORE `SimEntityMirrorPlugin` — its
            // `spawn_test_npcs` system reads `EmbeddedPlayer` (written by this
            // plugin's `tick_player`), matching the "add AFTER
            // PlayerBridgePlugin" convention `LodAltStreamPlugin` below
            // already follows. BL-82 EM-4.11: `tick_player` now runs in
            // `Update` (frame-rate local prediction), NOT `FixedUpdate`
            // alongside `SimEntityMirrorPlugin`'s systems any more, so the
            // explicit `.after(tick_player)` constraint that plugin used to
            // carry has been removed (a cross-schedule ordering constraint no
            // longer applies) — see that plugin's doc for the resulting
            // one-frame-stale-but-harmless read. This registration order is
            // now purely a documentation convention, not load-bearing.
            PlayerBridgePlugin,
            // Server shell: entity mirror (sim entities → replicated Bevy
            // entities) + one-shot test-NPC spawn (EM-3.7). Specs stays inside.
            SimEntityMirrorPlugin,
            // Server shell: one-shot far-terrain heightmap broadcast (EM-3.10b).
            // Reads the embedded player's `world_data()`, so it's added after
            // `PlayerBridgePlugin`. Specs/client-core stays inside the bridge.
            LodAltStreamPlugin,
            // Client-side consumer of the streamed terrain.
            TerrainStreamPlugin,
            // Client-side presentation of the mirrored entities: placeholder
            // meshes + interpolation (EM-3.7). Pure Bevy — no specs.
            EntityViewPlugin,
            // EM-3.8: real `.vox` figures — for supported bodies, replaces the
            // capsule with the assembled Veloren voxel model. Pure Bevy.
            FigureViewPlugin,
            // EM-3.9: block sprites (grass/flowers) instanced over the streamed
            // terrain from the same CompressedChunk stream. Pure Bevy.
            SpriteViewPlugin,
            // EM-3.10: distance-band culling of the streamed chunks + sprites
            // (frustum culling is already automatic via mesh Aabbs). Pure Bevy.
            LodCullingPlugin,
            // EM-3.10b: coarse far-terrain mesh from the one-shot lod-alt
            // heightmap, filling the horizon beyond the chunk band. Pure Bevy.
            FarTerrainPlugin,
            // Client-side: keyboard/mouse → LocalPlayerInput + third-person
            // camera following the player's mirror (EM-3.7b). Pure Bevy.
            PlayerInputPlugin,
            // The pipeline needs the palette-derived ChunkLayerMap +
            // ChunkMaterials to mesh at all. The synthetic demo isn't added in
            // listen-server mode, so add the shared palette plugin here (it
            // installs both from `block_palette.ron`).
            crate::palette_material::PaletteMaterialPlugin,
        ));
        // BL-82 EM-5.2: the first Phase-5 HUD state-mirror slice
        // (energy/poise/combo/XP/buffs) — reads `SimMirror`, so it must be
        // added after `SimEntityMirrorPlugin` above (which populates it this
        // same tick). Split into its own call — the tuple above is already
        // at the 15-plugin ceiling `bevy_app`'s `Plugins` trait impls
        // support (same reason `AtmosphereSyncMessagePlugin` below is
        // already split out).
        app.add_plugins(CombatHudMirrorPlugin);
        // BL-82 EM-5.5: the one-shot map-data broadcast (background image +
        // site/POI markers) — reads `EmbeddedPlayer`, so it's added after
        // `PlayerBridgePlugin`, matching `LodAltStreamPlugin`'s own
        // registration convention exactly. Split into its own call — the
        // tuple above is already at the 15-plugin ceiling.
        app.add_plugins(MapDataStreamPlugin);
        // BL-82 EM-4.9 (Phase D): symmetric `SetClientAtmosphere` message
        // registration — see `xindeler_oracle_host::atmosphere_sync`'s
        // module doc comment for why this can't live in
        // `XindelerProtocolPlugin` itself. Dormant on the listen-server path
        // today (nothing populates `DimensionAtmospheres` there — see that
        // plugin's own doc comment), harmless to register. Split into its
        // own call — the tuple above is already at the 15-plugin ceiling.
        app.add_plugins(AtmosphereSyncMessagePlugin);
        // BL-82 EM-4.8: minimal timed-fade `bevy_ui` toast, rendered on
        // `HudToast` arrival (the server-side hook lives in
        // `xindeler_protocol::narrative`). Pure Bevy. Split into its own
        // `add_plugins` call — the tuple above is already at the 15-plugin
        // ceiling `bevy_app`'s `Plugins` trait impls support.
        app.add_plugins(HudToastViewPlugin);
        // BL-82 EM-4.9 (Phase D): retargets `AtmosphereController` on
        // `SetClientAtmosphere` arrival.
        app.add_plugins(AtmosphereSyncViewPlugin);
        // BL-82 EM-4.9 follow-up: the generic (ORACLE-agnostic)
        // dimension-transfer mechanism (mirror `DimensionId`/`DimensionRoot`
        // retag + occupant bookkeeping + player-eject-on-teardown). Dormant
        // for ORACLE events specifically on this path (`ServerOraclePlugin`
        // is never added to the listen-server client — see
        // `xindeler_sim_bridge::oracle`'s own doc comment), but still real
        // and exercised by the generic `XINDELER_DEBUG_SPINUP_DIMENSION`/
        // `XINDELER_DEBUG_DRAIN_DIMENSION` debug-command path (a listen-server
        // admin transferring the embedded local player into a debug-spun
        // dimension must still be ejected cleanly on teardown). Split into
        // its own call — the tuple above is already at the 15-plugin
        // ceiling.
        app.add_plugins(PlayerTransferPlugin);
        // BL-82 EM-5.2: the core combat HUD (health/energy/poise/XP/combo
        // globes, buff strip, crosshair, death/respawn, overhead health
        // bars) reading the mirror above. Pure Bevy.
        app.add_plugins(CombatHudViewPlugin);
        // BL-82 EM-5.5: the minimap + full map screens, reading the
        // `MapDataStreamPlugin` broadcast above + the already-mirrored local
        // player `NetPos`/`NetOri`. Pure Bevy.
        app.add_plugins(MapViewPlugin);

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
            Ok(mut sim) => {
                // EM-3.7b: boot the embedded local-player Client over TCP
                // loopback to the sim we just booted (its listener is already
                // live). This blocks on the handshake (~hundreds of ms) but runs
                // once, right after the multi-second world boot. On failure we
                // still insert the sim and run — the terrain-anchor persister
                // fallback covers streaming, just without a controllable player.
                match boot_embedded_player(&mut sim) {
                    Ok(player) => {
                        app.insert_non_send(sim);
                        app.insert_non_send(player);
                        info!(
                            "listen-server: embedded world + local player booted; player is \
                             controllable once spawned"
                        );
                    },
                    Err(err) => {
                        app.insert_non_send(sim);
                        warn!(
                            "listen-server: embedded player failed to connect ({err}); running as \
                             spectator (terrain persister fallback, no controllable player)"
                        );
                    },
                }
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
