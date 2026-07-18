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
    CharListMirrorPlugin, ChatBridgePlugin, CombatHudMirrorPlugin, ConnectStage, EmbeddedPlayer,
    HotbarMirrorPlugin, LodAltStreamPlugin, LodZoneStreamPlugin, MapDataStreamPlugin,
    PlayerBridgePlugin, PlayerTransferPlugin, SIM_TICK_HZ, SimBridgePlugin, SimEntityMirrorPlugin,
    SimServer, SimTerrainStreamPlugin, SocialMirrorPlugin, boot_embedded_player_reporting,
    boot_test_server,
};

use crate::{
    atmosphere::AtmosphereSyncViewPlugin, char_select::CharSelectViewPlugin, chat::ChatViewPlugin,
    combat_hud::CombatHudViewPlugin, controls_screen::ControlsScreenPlugin,
    entity_view::EntityViewPlugin, far_terrain::FarTerrainPlugin, figure_view::FigureViewPlugin,
    hotbar::HotbarViewPlugin, hud_toast::HudToastViewPlugin,
    localization::ClientLocalizationPlugin, lod::LodCullingPlugin, lod_objects::LodObjectsPlugin,
    map_view::MapViewPlugin, player_input::PlayerInputPlugin, social_hud::SocialHudViewPlugin,
    sprite_view::SpriteViewPlugin, terrain_stream::TerrainStreamPlugin,
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
///
/// ## BL-82 EM-5.9 (T56.29): eager boot vs. menu-deferred boot
/// The whole plugin stack (every server-bridge + client-view plugin) is
/// registered in [`Plugin::build`] regardless of [`boot_eagerly`], but the
/// actual *world boot* (the multi-second [`boot_test_server`] +
/// [`boot_embedded_player`]) is split out into [`boot_offline_world`]:
/// - `boot_eagerly: true` — the `--listen-server` dev/smoke bypass: boot the
///   world here in `build` exactly as before (so every existing smoke path is
///   byte-for-byte unchanged).
/// - `boot_eagerly: false` — the main-menu path: register the stack but leave
///   the world unbooted; the menu's "Play / Connect (offline)" flow calls
///   [`boot_offline_world`] on `OnEnter(AppState::Connecting)`. This is safe
///   because EVERY bridge system reads the sim/player through
///   `Option<NonSend<…>>` (no-op until it exists) and every one-shot broadcast
///   latches only AFTER it has run with the player present — so the stack idles
///   cleanly with no world, then wakes up the instant one is booted.
///
/// [`boot_eagerly`]: ListenServerPlugin::boot_eagerly
pub struct ListenServerPlugin {
    /// Boot the embedded world eagerly in [`Plugin::build`] (the
    /// `--listen-server` dev/smoke bypass). When `false`, the stack's systems
    /// are registered but the world boot is deferred to [`boot_offline_world`]
    /// (the menu-driven offline connect path).
    pub boot_eagerly: bool,
    /// BL-82 EM-5.14: when `true` (the `--char-select` launch flag), boot into
    /// the character-select screen ([`AppState::CharSelect`]) with a manual-
    /// selection embedded player instead of the default "auto-load the first
    /// character and spawn straight in" behaviour. `false` preserves the
    /// original boot exactly. The actual `AppState` choice is centralized in
    /// `main.rs` (matching the EM-5.9 main-menu path's own pattern), not set
    /// here.
    pub char_select: bool,
}

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
        // BL-82 EM-5.8: the social/group/dialogue server-side mirror
        // (NetPlayerList/NetGroupState/NetDialogue projection + LocalGroupAction/
        // LocalDialogueResponse action application) — reads/writes
        // `EmbeddedPlayer`, so it must be added after `PlayerBridgePlugin`
        // above (registration order doesn't matter for the `.after(tick_sim)`
        // ordering itself, just for this doc convention). Split into its own
        // call — the tuple above is already at the 15-plugin ceiling.
        app.add_plugins(SocialMirrorPlugin);
        // BL-82 EM-5.5: the one-shot map-data broadcast (background image +
        // site/POI markers) — reads `EmbeddedPlayer`, so it's added after
        // `PlayerBridgePlugin`, matching `LodAltStreamPlugin`'s own
        // registration convention exactly. Split into its own call — the
        // tuple above is already at the 15-plugin ceiling.
        app.add_plugins(MapDataStreamPlugin);
        // BL-82 EM-3.11-FH Phase C: the streamed LOD-object zone mirror
        // (distant trees/structures) — same ordering reasoning as
        // `MapDataStreamPlugin` above (reads `EmbeddedPlayer`, added after
        // `PlayerBridgePlugin`). Split into its own call — the tuple above
        // is already at the 15-plugin ceiling.
        app.add_plugins(LodZoneStreamPlugin);
        // Client-side: bakes + renders the streamed LOD-object zones onto
        // the far-terrain horizon (reuses `FarTerrainMaterial`'s bend/
        // dissolve shader so objects sit on the same curved surface). Reads
        // `NetLodZone`/`NetLodZoneRemove`, so ordering relative to
        // `FarTerrainPlugin` above doesn't matter (separate mesh/material
        // instances, no shared state). Split into its own call for the same
        // 15-plugin-ceiling reason.
        app.add_plugins(LodObjectsPlugin);
        // BL-82 EM-5.4: the chat bridge (broadcasts the embedded player's
        // real chat traffic as `NetChatMsg` + applies `ChatSendRequest`
        // sends back onto that same Client) — reads/writes `EmbeddedPlayer`
        // (populated by `PlayerBridgePlugin` above), not `SimMirror`, so
        // ordering relative to `SimEntityMirrorPlugin` doesn't matter here
        // (see `xindeler_sim_bridge::chat`'s own module doc comment).
        app.add_plugins(ChatBridgePlugin);
        // BL-82 EM-5.3: the skillbar/hotbar mirror (resolved ability-pool/
        // slot bindings + per-ability cooldowns) — same reasoning/ordering
        // as `CombatHudMirrorPlugin` above (reads `SimMirror`, split into its
        // own call, the tuple above is already at the 15-plugin ceiling).
        app.add_plugins(HotbarMirrorPlugin);
        // BL-82 EM-5.6: the inventory/bag + two-party-trade mirrors +
        // request applicators (spec §3.2/§6) — same ordering reasoning as
        // `CombatHudMirrorPlugin` above (reads `SimMirror`).
        app.add_plugins((
            xindeler_sim_bridge::InventoryMirrorPlugin,
            xindeler_sim_bridge::TradeMirrorPlugin,
        ));
        // BL-82 EM-5.15: the crafting mirror (recipe book + salvage/repair/
        // modular candidate lists) — same ordering reasoning as
        // `InventoryMirrorPlugin` above (reads `SimMirror`). Its client → sim
        // intent reuses the existing `InventoryActionRequest` applicator, so no
        // extra applicator is added here.
        app.add_plugins(xindeler_sim_bridge::CraftingMirrorPlugin);
        // BL-82 EM-5.7: the character diary / skill-tree mirror
        // (`NetSkillSet`/`NetAbilityPool`) + the SP-spend request applicator
        // — same ordering reasoning as `CombatHudMirrorPlugin`/`HotbarMirrorPlugin`
        // above (reads `SimMirror`).
        app.add_plugins(xindeler_sim_bridge::SkillSetMirrorPlugin);
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
        // BL-82 EM-5.16 (T56.44): the settings-bridge half of the reactive
        // i18n pipeline — needs `xindeler_ui::i18n`'s `CurrentLocale`
        // resource, which `CombatHudViewPlugin`'s own `XindelerUiPlugin`
        // (just added above) inserts.
        app.add_plugins(ClientLocalizationPlugin);
        // BL-82 EM-5.8: the social/group/dialogue HUD (player list, group/
        // party frames, invite banner, v1-minimal NPC dialogue) reading the
        // `SocialMirrorPlugin` mirror above. Pure Bevy.
        app.add_plugins(SocialHudViewPlugin);
        // BL-82 EM-5.5: the minimap + full map screens, reading the
        // `MapDataStreamPlugin` broadcast above + the already-mirrored local
        // player `NetPos`/`NetOri`. Pure Bevy.
        app.add_plugins(MapViewPlugin);
        // BL-82 EM-5.11: the input-rebinding screen (keyboard/mouse +
        // gamepad). Reuses the widget kit `CombatHudViewPlugin` already
        // added (`XindelerUiPlugin`) — no new UI plugin registration needed.
        app.add_plugins(ControlsScreenPlugin);
        // BL-82 EM-5.4: the chat panel (scrollback, channel tabs, input box)
        // reading `NetChatMsg`/writing `ChatSendRequest`. Pure Bevy.
        app.add_plugins(ChatViewPlugin);
        // BL-82 EM-5.17: the cursor-free aggregator — frees the OS cursor
        // (visible + ungrabbed, so panels are clickable) whenever any HUD
        // window is open or the chat input is focused, and re-grabs for
        // mouselook otherwise. Reads `HudState` (from `CombatHudViewPlugin`'s
        // `XindelerUiPlugin`) + chat focus (from `ChatViewPlugin` above), so
        // it's added after both. Pure Bevy.
        app.add_plugins(crate::cursor::CursorControlPlugin);
        // BL-82 EM-5.12 (T56.38): the Escape/pause menu (Resume / Settings /
        // Controls / Servers / Logout / Quit). Reads `HudState`
        // (`CombatHudViewPlugin`'s `XindelerUiPlugin`) + writes `HudAction`.
        // Pure Bevy.
        app.add_plugins(crate::esc_menu::EscMenuPlugin);
        // BL-82 EM-5.12 (T56.39): the tabbed settings window (Interface/Video/
        // Controls/Gameplay/Chat/Language/Networking/Sound/Accessibility),
        // persisting via `XindelerSettings::save()` and applying live graphics
        // changes to the camera. Reuses the widget kit `CombatHudViewPlugin`
        // already added (`XindelerUiPlugin`). Pure Bevy.
        app.add_plugins(crate::settings_window::SettingsWindowPlugin);
        // BL-82 EM-5.3: the skillbar/hotbar screen (drag-to-assign, keybind
        // labels, cooldown sweeps) reading the mirror above. Reuses the
        // widget kit `CombatHudViewPlugin` already added (`XindelerUiPlugin`)
        // — no new UI plugin registration needed.
        app.add_plugins(HotbarViewPlugin);
        // BL-82 EM-5.6: the inventory/bag + paper-doll screen, loot feed, and
        // the two-party trade window — pure Bevy, reading the mirrors above.
        app.add_plugins((
            crate::inventory_ui::InventoryUiPlugin,
            crate::trade_ui::TradeUiPlugin,
        ));
        // BL-82 EM-5.7: the character diary / skill-tree screen (Stats tab +
        // the generic per-group tree renderer + Abilities tab) reading the
        // `SkillSetMirrorPlugin` mirror above. Pure Bevy.
        app.add_plugins(crate::diary::DiaryUiPlugin);
        // BL-82 EM-5.15: the crafting screen (recipes/salvage/repair/modular
        // tabs) reading the `CraftingMirrorPlugin` mirror above. Pure Bevy.
        app.add_plugins(crate::crafting_ui::CraftingUiPlugin);
        // BL-82 EM-5.17 Phase 5: the boss/target nameplate — panel + bars
        // fully built, reading `NetHealth`/`NetPoise`/`NetXp` off whatever
        // entity `SelectedTarget` resolves to.
        app.add_plugins(crate::boss_nameplate::BossNameplateViewPlugin);
        // BL-82 EM-5.18 Phase 1: the hybrid target-selection system's
        // soft-target scan — a continuous camera-cone scorer that populates
        // `SelectedTarget` with the nearest `Enemy`-aligned candidate in
        // front of the player, unblocking the nameplate above (it now
        // renders during real gameplay, not just under
        // `XINDELER_SMOKE_FORCE_TARGET`). Pure Bevy; see `targeting`'s
        // module doc comment for the precedence between this system and the
        // smoke override.
        app.add_plugins(crate::targeting::TargetSelectionPlugin);

        // BL-82 EM-5.14: the character-select screen + its char-list mirror,
        // added ONLY when launched with `--char-select` so the default boot
        // (auto-load + spawn straight in) is completely unaffected. `CharList
        // MirrorPlugin` (bridge) broadcasts the embedded player's roster;
        // `CharSelectViewPlugin` (client) is the screen + 3D preview. Both are
        // dormant unless `AppState::CharSelect` is active (that choice is
        // centralized in `main.rs`, matching the EM-5.9 main-menu path).
        if self.char_select {
            app.add_plugins((CharListMirrorPlugin, CharSelectViewPlugin));
        }

        // Boot the embedded world now (the `--listen-server` bypass) or leave
        // it to the menu's offline-connect flow (see [`boot_offline_world`]).
        if self.boot_eagerly {
            if self.char_select {
                // BL-82 EM-5.14: char-select mode must NOT auto-pick a
                // character — park the embedded player awaiting the UI's
                // selection. `set_manual_selection` must run BEFORE the first
                // tick (we are still in `build`, no frame has run), so this
                // calls the pure `boot_offline_world_parts` directly instead of
                // the `boot_offline_world` wrapper (which inserts immediately,
                // with no hook to flip the flag first).
                match boot_offline_world_parts(&|_| {}) {
                    Ok((sim, mut player)) => {
                        if let Some(player) = player.as_mut() {
                            player.set_manual_selection(true);
                        }
                        let has_player = player.is_some();
                        app.insert_non_send(sim);
                        if let Some(player) = player {
                            app.insert_non_send(player);
                        }
                        if has_player {
                            info!(
                                "listen-server: embedded world + local player booted; awaiting \
                                 character selection"
                            );
                        } else {
                            warn!(
                                "listen-server: running as spectator (terrain persister fallback, \
                                 no controllable player)"
                            );
                        }
                    },
                    Err(err) => error!(
                        "listen-server: failed to boot embedded world ({err}); running without a \
                         world (missing XINDELER_ASSETS / LFS map blobs?)"
                    ),
                }
            } else {
                boot_offline_world(app.world_mut());
            }
        }
    }
}

/// Boots the embedded world + local player and inserts them as non-send
/// resources into `world`. Returns `true` if a world was booted (with OR
/// without a controllable player — the spectator/persister fallback still
/// counts as "a world is up"), `false` if the world itself could not boot
/// (missing assets / LFS blobs) and the caller should surface a connect error.
///
/// This is the SAME boot sequence [`ListenServerPlugin`] used to run inline in
/// `build`; it is extracted so the main menu (BL-82 EM-5.9 T56.29) can trigger
/// it at runtime on `OnEnter(AppState::Connecting)` rather than eagerly at
/// startup. It blocks for several seconds (world gen + the loopback handshake),
/// so callers run it from an exclusive `&mut World` system on the connecting
/// screen, having already rendered a "Connecting…" frame first.
pub fn boot_offline_world(world: &mut World) -> bool {
    match boot_offline_world_parts(&|_| {}) {
        Ok((sim, player)) => {
            world.insert_non_send(sim);
            if let Some(player) = player {
                world.insert_non_send(player);
                info!(
                    "listen-server: embedded world + local player booted; player is controllable \
                     once spawned"
                );
            } else {
                warn!(
                    "listen-server: running as spectator (terrain persister fallback, no \
                     controllable player)"
                );
            }
            true
        },
        Err(err) => {
            error!(
                "listen-server: failed to boot embedded world ({err}); running without a world \
                 (missing XINDELER_ASSETS / LFS map blobs?)"
            );
            false
        },
    }
}

/// The pure (World-free) half of [`boot_offline_world`]: boots the embedded
/// world + local player and RETURNS them instead of inserting them, reporting
/// each real boot stage through `progress` (BL-82 EM-5.9 T56.30).
///
/// This is what makes a genuine staged loading screen possible: the connecting
/// screen runs this on a background thread (both objects are `Send`), passing a
/// `progress` closure that writes into a cell the Bevy main thread polls, so
/// the window keeps rendering live progress instead of freezing through the
/// multi-second boot. The eager `--listen-server` path calls the thin
/// [`boot_offline_world`] wrapper above with a no-op `progress`, so its
/// behaviour is byte-for-byte unchanged.
///
/// Returns:
/// - `Err` — the world itself could not boot (missing assets / LFS blobs); the
///   caller surfaces a connect error.
/// - `Ok((sim, Some(player)))` — full boot with a controllable local player.
/// - `Ok((sim, None))` — the sim booted but the embedded player failed to
///   connect; the caller runs as a spectator (terrain-anchor persister
///   fallback), exactly as [`boot_offline_world`] always has.
pub fn boot_offline_world_parts(
    progress: &(dyn Fn(ConnectStage) + Send + Sync),
) -> Result<(SimServer, Option<EmbeddedPlayer>), String> {
    let data_dir = userdata_dir().join("listen-server");
    std::fs::create_dir_all(&data_dir)
        .map_err(|err| format!("cannot create data dir {}: {err}", data_dir.display()))?;
    info!(
        "listen-server: booting embedded world at {} (this takes several seconds)…",
        data_dir.display()
    );
    progress(ConnectStage::GeneratingWorld);
    let mut sim = boot_test_server(&data_dir).map_err(|err| format!("{err:?}"))?;

    // EM-3.7b: boot the embedded local-player Client over TCP loopback to the
    // sim we just booted (its listener is already live). This blocks on the
    // real handshake (~hundreds of ms, staged through `progress`) but runs
    // once, right after the multi-second world boot. On failure we still return
    // the sim so the terrain-anchor persister fallback can cover streaming, just
    // without a controllable player.
    progress(ConnectStage::EstablishingConnection);
    match boot_embedded_player_reporting(&mut sim, progress) {
        Ok(player) => Ok((sim, Some(player))),
        Err(err) => {
            warn!("listen-server: embedded player failed to connect ({err}); spectator fallback");
            Ok((sim, None))
        },
    }
}
