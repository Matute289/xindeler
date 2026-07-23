//! Glues `sim` + `shutdown` + `metrics` + (BL-82 EM-4.2b) the new
//! replicon+quinnet transport into the one plugin the EM-4.1 task board line
//! asks for verbatim: "`SimServerPlugin` owning `veloren_server::Server` +
//! `.tick()` system, signal handling, metrics passthrough" — now extended per
//! the EM-4.2b spec (§1.1) to also host `xindeler-sim-bridge`'s entity/terrain
//! mirror + a REAL remote `bevy_replicon` server role, dual-stack alongside
//! the untouched legacy listener.

use std::sync::Arc;

use bevy::{
    app::{App, FixedUpdate, Plugin, Update},
    ecs::schedule::IntoScheduleConfigs,
    state::app::StatesPlugin,
    time::{Fixed, Time},
};
use bevy_replicon::prelude::RepliconPlugins;
use tokio::sync::Notify;
use xindeler_dimensions::{
    DimensionsPlugin, PredictiveGcConfigPlugin, teardown_completed_dimensions,
};
use xindeler_oracle_host::{AiGatewayPlugin, ServerAtmosphereSyncPlugin};
use xindeler_protocol::{
    ActiveReplicaSessions, ClientInterestPlugin, HudToastPlugin, XindelerProtocolPlugin,
};
use xindeler_sim_bridge::{
    ChatBridgePlugin, CombatHudMirrorPlugin, CraftingMirrorPlugin, HotbarMirrorPlugin,
    InventoryMirrorPlugin, PlayerTransferPlugin, SIM_TICK_HZ, ServerOraclePlugin,
    SfxLocomotionMirrorPlugin, SfxOutcomeBridgePlugin, SimBridgePlugin, SimEntityMirrorPlugin,
    SimTerrainStreamPlugin, SkillSetMirrorPlugin, SocialMirrorPlugin, TradeMirrorPlugin, tick_sim,
};
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

use crate::{
    dimensions,
    login::{self, PendingLogins},
    metrics,
    shutdown::{self, ShutdownState},
    sim::{self, SimServerConfig},
};

/// Boots the embedded sim, installs the SIGINT/SIGTERM shutdown flag, starts
/// the metrics passthrough, wires the entity/terrain mirror + the new
/// replicon+quinnet transport (EM-4.2b), and registers the per-tick systems —
/// see the module doc comment. Building this plugin boots a real (possibly
/// slow, asset-dependent) world, same as `server-cli`'s `main` calling
/// `Server::new` directly; unlike `xindeler-sim-bridge` (which defers booting
/// to its caller), this crate IS the shell, so there is no other place to do
/// it.
#[derive(Default)]
pub struct SimServerPlugin {
    pub config: SimServerConfig,
}

impl Plugin for SimServerPlugin {
    fn build(&self, app: &mut App) {
        let sim = sim::boot_dedicated_server(&self.config)
            .expect("failed to create the xindeler dedicated server instance");

        let shutdown_flag = shutdown::register_signals();

        let metrics_registry = Arc::clone(sim.server.metrics_registry());
        let metrics_shutdown = Arc::new(Notify::new());
        metrics::spawn(
            &sim.runtime,
            Arc::clone(&metrics_registry),
            self.config.metrics_addr,
            Arc::clone(&metrics_shutdown),
        );

        // EM-4.2e: AI-gateway config/metrics seam. `self.config.ai_gateway`
        // defaults to `Offline`/no-op but is overridable via `main.rs`'s
        // `XINDELER_SERVER_AI_GATEWAY_CONFIG` env var, so this is a genuinely
        // exercised RON-load path, not just a tested capability. Registers 2
        // zero-value counters on the SAME registry `/metrics` above serves;
        // makes no real AI call (see `xindeler_oracle_host::ai_gateway`'s doc
        // comment).
        app.add_plugins(AiGatewayPlugin {
            config: self.config.ai_gateway.clone(),
            registry: Arc::clone(&metrics_registry),
        });

        // EM-4.5: DimensionRegistry + the full lifecycle state machine.
        // `DimensionsPlugin` first (so `DimensionRegistry`/`SpinupTasks`/the
        // message types exist), THEN wrap `DimensionId::DEFAULT` around the
        // sim's ALREADY-generated `Arc<World>`/`IndexOwned` in place — a
        // wrapping refactor of already-existing state, not a behavior
        // change (see `dimensions.rs`'s doc comment).
        app.add_plugins(DimensionsPlugin);
        // BL-82 EM-4.10 T48.6: loads `predictive_gc`'s tuning constants from
        // `assets/xindeler/dimensions/default.predictive_gc.ron` (retunable
        // without a rebuild) instead of leaving them as the compiled-in
        // `PredictiveGc::default()` `DimensionsPlugin` just inserted above —
        // requires the `AssetPlugin` `main.rs` now adds before this plugin.
        app.add_plugins(PredictiveGcConfigPlugin::default());
        dimensions::install_default_dimension(app, &sim);
        dimensions::init_debug_state(app);
        app.insert_resource(self.config.debug_dimension_commands.clone());
        let dimension_metrics = dimensions::register_metrics(&metrics_registry);
        app.insert_resource(dimension_metrics);
        // Explicit edge (bevy-migration-reviewer + ecs-design-reviewer follow-up,
        // found verifying the phase-4 wave-3 integration): this chain and
        // `DimensionsPlugin`'s own chain (above) both touch `DimensionRegistry`
        // with a genuine read/write conflict (`update_dimension_metrics` reads
        // it right after `teardown_completed_dimensions` mutates it) with no
        // ordering constraint between the two otherwise — today it happens to
        // work only because `add_plugins(DimensionsPlugin)` is called first in
        // this same `build`, an accident of insertion order a future refactor
        // could silently break. Declaring the edge explicitly (rather than
        // relying on that accident) makes `update_dimension_metrics` see this
        // tick's fully up-to-date registry state, matching the same rigor this
        // crate's own `DimensionsPlugin::build` doc comment already applies to
        // its debug isolation sweep.
        //
        // EM-4.10 Finding B follow-up (bevy-migration-reviewer MAJOR finding):
        // `teardown_completed_dimensions` moved `Update` -> `FixedUpdate` (see
        // `xindeler_dimensions::plugin::DimensionsPlugin`'s doc comment) but
        // this chain was left registered in `Update` with a now-VACUOUS
        // `.after(teardown_completed_dimensions)` edge — `.after`/`.before`
        // only order systems within the SAME schedule, so this ordered against
        // zero members of `Update`'s own system graph (confirmed: Bevy treats
        // this as a silent no-op, not a build-time error). Runtime correctness
        // was fortuitously preserved anyway because Bevy's `MainScheduleOrder`
        // always runs `RunFixedMainLoop` (and thus every `FixedUpdate` step
        // queued for the frame) strictly before `Update` — but that made the
        // ordering an accident of Bevy's own schedule structure standing in
        // for what used to be an explicit, declared edge, exactly the
        // "insertion-order accident a future refactor could silently break"
        // class of bug this comment was originally written to eliminate.
        // Moved this chain to `FixedUpdate` too — sim/debug-command bookkeeping
        // belongs at sim cadence like the rest of the dimension-lifecycle
        // chain, not render cadence — so the `.after(..)` edge is a REAL,
        // same-schedule ordering constraint again.
        app.add_systems(
            FixedUpdate,
            (
                dimensions::apply_debug_dimension_commands,
                dimensions::update_dimension_metrics,
            )
                .chain()
                .after(teardown_completed_dimensions),
        );

        app.insert_non_send(sim);
        app.insert_resource(ShutdownState {
            flag: shutdown_flag,
            metrics_shutdown,
        });
        // `check_shutdown` runs in `Update`, which Bevy's `MainScheduleOrder`
        // always runs strictly AFTER the `RunFixedMainLoop` schedule (which
        // drives every `FixedUpdate` step queued for that frame) —
        // structurally, for any tick rate, not merely because
        // `sim::SIM_TICK_INTERVAL` happens to match `SIM_TICK_HZ` below — so
        // a shutdown request still never lands mid-tick (see
        // shutdown.rs's doc comment). `tick_sim` itself is no longer
        // registered here — `SimBridgePlugin` below owns it (EM-4.2b: shared
        // with the listen-server path instead of a second local copy).
        app.add_systems(Update, shutdown::check_shutdown);

        // EM-4.2b: pace the bridge's `FixedUpdate` tick_sim at the sim's real
        // 30 TPS, same as the listen-server client does (see
        // `xindeler_sim_bridge::tick_sim`'s doc comment for why FixedUpdate,
        // not Update, is the right home for this).
        app.insert_resource(Time::<Fixed>::from_hz(SIM_TICK_HZ));

        // Real remote `bevy_replicon` SERVER role + the shared replication
        // contract + the sim↔Bevy bridge (entity mirror + terrain stream —
        // the SAME plugins the listen-server path uses, now hosted here
        // instead, per spec §0.1/§1.1). `RepliconPlugins` is added
        // UNCONFIGURED (library default `ServerPlugin::default()`, which
        // replicates on `FixedPostUpdate`) rather than overridden to
        // `PostUpdate`: `FixedPostUpdate` runs in the SAME `FixedMain`
        // iteration as `SimBridgePlugin`/`SimEntityMirrorPlugin`'s own
        // `FixedUpdate` mirror writes, so every fixed-step mirror update gets
        // replicated — no frame can coalesce/drop an intermediate step (which
        // a `PostUpdate` override, ticking once per RENDER frame regardless
        // of how many `FixedUpdate` steps ran that frame, could do). An
        // earlier draft copied `ServerPlugin::new(PostUpdate)` from
        // `xindeler-sim-bridge`'s OWN test harness (`src/lib.rs`'s
        // `new_test_app`), where that override exists only to satisfy a
        // manually-stepped `TimeUpdateStrategy::ManualDuration` test app that
        // may never run a real `FixedMain` iteration — an unrelated
        // constraint that doesn't apply to this real wall-clock,
        // `ScheduleRunnerPlugin`-paced production shell.
        //
        // No client role is ever activated in this process (`ClientState`
        // stays `Disconnected`), so `SimTerrainStreamPlugin`/
        // `SimEntityMirrorPlugin`'s
        // `run_if(in_state(ClientState::Disconnected))` gate holds
        // continuously — this shell is purely a replication SOURCE, never a
        // sink.
        //
        // `StatesPlugin` is NOT part of `MinimalPlugins` (this shell's own
        // plugin set, `main.rs`) — only `DefaultPlugins` bundles it. Every
        // other place `RepliconPlugins` is added in this codebase gets
        // `StatesPlugin` for free (the listen-server client via
        // `DefaultPlugins`; `xindeler-protocol`'s own tests add it
        // explicitly alongside `MinimalPlugins`). `bevy_replicon`'s
        // `ClientState`/`ServerState` are Bevy `States`, so without this the
        // very first `RepliconPlugins` system to touch `StateTransition`
        // panics at Startup ("the `StateTransition` schedule is missing").
        //
        // Note: this process also compiles/registers a dormant client-role
        // `ClientPlugin`/`ClientMessagePlugin` (Cargo feature unification —
        // see `xindeler-transport`'s crate doc comment, "Both client+server
        // bevy_replicon roles compile into EVERY shell", for the full
        // explanation and why it's harmless and not worth fighting here).
        app.add_plugins((
            StatesPlugin,
            RepliconPlugins,
            XindelerProtocolPlugin,
            SimBridgePlugin,
            SimTerrainStreamPlugin,
            SimEntityMirrorPlugin,
            // BL-82 EM-4.9 follow-up: the generic (ORACLE-agnostic)
            // dimension-transfer mechanism — `ServerOraclePlugin` below also
            // guard-adds this itself (see its own doc comment), but adding it
            // explicitly here too keeps a debug-spun dimension's
            // player-eject-on-teardown behavior correct even if
            // `ServerOraclePlugin`'s DmEvent machinery is ever disabled on a
            // given deployment.
            PlayerTransferPlugin,
            // BL-82 EM-4.2d: per-client interest management, scoping each
            // connected client's replicated-entity visibility to the regions
            // its own `ClientViewpoint` covers (see
            // `xindeler_protocol::interest`'s module doc comment).
            // Integration note (EM-4.2c merge, UPDATED by EM-8.2): a logged-in
            // client's `login.rs` handshake now DOES set a real, character-
            // position-derived `ClientViewpoint` (`handle_character_data`'s
            // `commands.entity(client_entity).insert(ClientViewpoint::new(..))`
            // call) — `xindeler-sim-bridge`'s `apply_default_viewpoint_for_new_
            // clients` stopgap (spectator-style, centred on the world/anchor)
            // now only ever covers a client that HASN'T logged in yet (or
            // never will, e.g. a bare test connection with no `LoginRequest`
            // at all); it never overwrites an already-set `ClientViewpoint`
            // (see that function's own doc comment). Runs in `FixedUpdate`,
            // alongside `SimEntityMirrorPlugin`'s own `RegionKey` writes, so
            // both settle before `RepliconPlugins`' `FixedPostUpdate`
            // replication pass.
            ClientInterestPlugin,
            // BL-82 EM-4.8: the `on_enter_message -> HudToast` narrative
            // hook (`xindeler_protocol::narrative`). Was a permanent no-op
            // before EM-4.9 (`NarrativeHooks` started empty and nothing in
            // this shell registered an entry) — `ServerOraclePlugin` below
            // is now the real caller.
            HudToastPlugin,
            // BL-82 EM-4.9 (Phase D): the atmosphere-replication seam —
            // `DimensionAtmospheres` + `SetClientAtmosphere` targeted
            // message, populated by `ServerOraclePlugin`'s ingest producer.
            ServerAtmosphereSyncPlugin,
        ));

        // BL-82 EM-4.9: the ORACLE ingestion chain (DmEvent -> dimension
        // spinup + factory spawn + narrative hook + chronicle rumor). See
        // `oracle.rs`'s own module doc comment for the full producer chain;
        // `main.rs` already called `register_oracle_source` BEFORE
        // `AssetPlugin` (the other half of the two-phase ordering contract) —
        // `events_dir` is threaded through from that SAME resolved value (see
        // `SimServerConfig::events_dir`'s own doc comment for why).
        app.add_plugins(ServerOraclePlugin {
            events_dir: self.config.events_dir.clone(),
            ..Default::default()
        });

        // BL-82 EM-5.2: the first Phase-5 HUD state-mirror slice
        // (energy/poise/combo/XP/buffs) — reads `SimMirror`, so it must be
        // added after `SimEntityMirrorPlugin` above (which populates it this
        // same tick). Split into its own `add_plugins` call — the main tuple
        // above is already at the 15-plugin ceiling `bevy_app`'s `Plugins`
        // trait impls support (same reason `AtmosphereSyncMessagePlugin`
        // above was already split out).
        app.add_plugins(CombatHudMirrorPlugin);
        // BL-82 EM-5.10b (T56.35): the SFX event mappers' locomotion/combat-
        // move classification (`NetLocomotion`/`NetCombatMove`) — same
        // reasoning/ordering as `CombatHudMirrorPlugin` above (reads only
        // `SimMirror`, no `EmbeddedPlayer` dependency, so it mirrors real
        // remote clients' entities on this dedicated-server shell same as
        // any other entity-visible `Net*` comp).
        app.add_plugins(SfxLocomotionMirrorPlugin);
        // BL-82 EM-5.6: the inventory/bag + two-party-trade mirrors + request
        // applicators — same ordering reasoning as `CombatHudMirrorPlugin`
        // above (reads `SimMirror`, populated by `SimEntityMirrorPlugin`).
        app.add_plugins((InventoryMirrorPlugin, TradeMirrorPlugin));
        // BL-82 EM-5.15: the crafting mirror (recipe book + salvage/repair/
        // modular candidate lists) — same ordering reasoning as
        // `InventoryMirrorPlugin` above (reads `SimMirror`, populated by
        // `SimEntityMirrorPlugin`). Its client → sim intent reuses the existing
        // `InventoryActionRequest` applicator (`InventoryMirrorPlugin`), so no
        // extra applicator is registered here.
        app.add_plugins(CraftingMirrorPlugin);

        // BL-82 EM-5.3: the skillbar/hotbar mirror (resolved ability-pool/
        // slot bindings + per-ability cooldowns) — same reasoning as
        // `CombatHudMirrorPlugin` above. `apply_hotbar_assignment_requests`
        // (part of this plugin, EM-5.3 follow-up: was `apply_local_hotbar_
        // assignment`, which resolved every rebind via the embedded-player
        // shortcut and so silently dropped every real client's request on
        // THIS shell — fixed to resolve via `PlayerDimensionSession`, the
        // SAME pattern `InventoryMirrorPlugin`/`TradeMirrorPlugin` above
        // already use) degrades clean when there is no `EmbeddedPlayer`
        // (a dedicated server serves only real remote clients) — its
        // fallback path simply never fires here, but the real-connection
        // path (the one that matters on THIS shell) does.
        app.add_plugins(HotbarMirrorPlugin);

        // BL-82 EM-8.3: the diary/skill-tree mirror + SP-spend applicator, and
        // the social/party roster + group-state mirror + group/dialogue-request
        // applicators — the dedicated-server-parity half of the technical-debt
        // ledger's A1 finding. Both were listen-server-only before this task
        // because their write/mirror paths were coupled to the single embedded
        // player; they now resolve every real remote client's identity via
        // `PlayerDimensionSession`/`ActiveReplicaSessions` (the SAME pattern
        // `InventoryMirrorPlugin`/`TradeMirrorPlugin` above already use), so
        // they mirror + apply correctly for N genuinely-connected clients here.
        // Same ordering reasoning as the mirrors above (each reads `SimMirror`,
        // populated this same tick by `SimEntityMirrorPlugin`).
        app.add_plugins((SkillSetMirrorPlugin, SocialMirrorPlugin));

        // BL-82 EM-8.3b: closes the ledger's remaining A1 items — chat
        // broadcast, outcome→SFX broadcast, and (folded into
        // `SocialMirrorPlugin` above) NPC→player dialogue capture — which
        // EM-8.3 explicitly could NOT close here because all three depended
        // on OBSERVING the sim's own per-recipient outgoing message stream
        // (chat routed by `StateExt::send_chat`, outcomes drained inside
        // `Server::tick` by `entity_sync`, dialogue delivered via a
        // `comp::Client`), and every one of those paths targeted the sim's
        // legacy `comp::Client` send queue — which a replicon-login player
        // entity never has — consumed INSIDE `Server::tick`, so nothing was
        // left to read post-tick. `server::msg_capture::
        // OutgoingMessageCapture` is the fix: a new sim-side, per-player
        // capture buffer (`server/src/state_ext.rs`'s `send_chat`,
        // `server/src/sys/entity_sync.rs`'s outcome sync, and
        // `server/src/events/interaction.rs`'s `DialogueEvent` handler each
        // ALSO push into it for exactly the recipients with no legacy
        // `comp::Client`). `ChatBridgePlugin`/`SfxOutcomeBridgePlugin` (added
        // here for the first time) each register a `broadcast_captured_*`
        // system alongside their pre-existing listen-server-only
        // `broadcast_embedded_*` one; on THIS shell the embedded-only system
        // is a no-op (no `EmbeddedPlayer` exists), while the captured one
        // resolves every real recipient's `ClientId` via
        // `ActiveReplicaSessions` and targets `SendTargets::Single` — never
        // broadcast. Same ordering reasoning as the mirrors above.
        app.add_plugins((ChatBridgePlugin, SfxOutcomeBridgePlugin));

        // EM-4.2b: the transport seam — this crate names ONLY
        // `xindeler_transport::{ReplicaTransport, TransportConfig,
        // QuinnetTransport}`, never `bevy_replicon_quinnet`/`bevy_quinnet`
        // types (see `xindeler-transport`'s own doc comment for the grep bar
        // this maintains). Binds a port distinct from the legacy
        // `gameserver_protocols` listener(s) — see `sim::DEFAULT_REPLICON_ADDR`
        // and `XINDELER_SERVER_REPLICON_ADDR` in `main.rs`.
        app.add_plugins(
            QuinnetTransport.server_plugins(&TransportConfig::server(self.config.replicon_addr)),
        );

        // BL-82 EM-4.2c: the login/session handshake — see `login.rs`'s
        // module doc comment for the full design (why a SEPARATE
        // `CharacterLoader` instance, the v1 auto-select policy, the
        // documented IP-ban gap). `RepliconCharacterLoader` reads/opens the
        // SAME `saves/` sqlite path `boot_dedicated_server` (via
        // `sim::server_data_dir()`) already pointed the sim's OWN
        // `CharacterLoader` at.
        app.insert_resource(login::boot_replicon_character_loader(
            &sim::server_data_dir(),
        ));
        app.insert_resource(PendingLogins::default());
        app.insert_resource(ActiveReplicaSessions::default());
        // `.before(tick_sim)`: this system creates the sim entity + resolves
        // auth/character-loading BEFORE the sim's own `FixedUpdate` systems
        // (physics, subscription, persistence batching, …) run for this
        // frame, so a character that finishes loading this tick is already
        // fully set up by the time the rest of the sim ticks over it —
        // matching the natural "ingest input, then simulate" ordering the
        // rest of this `FixedUpdate` chain already follows (see `xindeler-
        // sim-bridge`'s own doc comment on `tick_sim`'s
        // `.after`/`.before` chain). Correctness does not actually depend on
        // this ordering (see `login.rs`'s module doc comment: the dedicated
        // `CharacterLoader` instance means there is no shared-channel race
        // either way), but it is the more intuitive ordering.
        app.add_systems(
            bevy::app::FixedUpdate,
            login::handle_replicon_logins.before(tick_sim),
        );
    }
}
