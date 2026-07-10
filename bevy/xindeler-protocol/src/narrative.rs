//! BL-82 EM-4.8 — the `on_enter_message → HudToast` narrative hook (task
//! board T47.10, spec §1.11).
//!
//! `DmEvent.narrative.on_enter_message` (`xindeler-oracle-host::dm_event`) is
//! meant to greet a player the moment their mirrored entity enters the
//! dimension a given `DmEvent` spins up. Two things this module does NOT
//! know or decide (deliberately, per the spec's scope boundary — "no
//! DmEvent-triggered auto-spinup" is EM-4.9's job, same posture
//! `xindeler-dimensions`' own module doc comment takes for its own spinup
//! trigger):
//! - which [`DimensionId`] a given `DmEvent` ends up spinning up (EM-4.5/4.9
//!   decide that at spinup time);
//! - when to call [`NarrativeHooks::register_on_enter_message`] with that
//!   pairing — a future EM-4.9 caller does this once, right after a
//!   `DmEvent`-driven spinup resolves its target `DimensionId`.
//!
//! What THIS module owns is the back half of the hook, fully real and
//! tested: [`NarrativeHooks`] is a plain `DimensionId -> message` registry,
//! and [`fire_on_enter_toasts`] watches every connected client's
//! [`ClientViewpoint`] (already the per-client "which dimension is this
//! client currently in" signal EM-4.2d/`ClientInterestPlugin` maintains) and
//! fires exactly ONE [`HudToast`] server message, targeted with
//! `SendTargets::Single`, to the one client whose viewpoint just settled on
//! a hooked dimension — never a broadcast (`SendTargets::All`).
//!
//! ## Why this lives in `xindeler-protocol`, not `xindeler-oracle-host`
//! Mirrors `interest.rs`'s own reasoning exactly: this module needs
//! [`ClientViewpoint`] + `bevy_replicon`'s `ToClients`/`SendTargets`/
//! `ClientId`, all of which already live in/are already dependencies of this
//! crate. `xindeler-oracle-host` (which owns the `DmEvent`/`Narrative`
//! schema) already depends on `xindeler-protocol` — a future EM-4.9 caller
//! living there can import [`NarrativeHooks`] with zero new dependency
//! edges; the reverse direction would be backwards, exactly the argument
//! `ai_mode.rs`/`interest.rs` already make for their own types.
use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        component::Component,
        entity::Entity,
        message::{Message, MessageWriter},
        resource::Resource,
        system::{Commands, Query, Res},
    },
};
use bevy_replicon::prelude::{ClientId, SendTargets, ToClients};
use serde::{Deserialize, Serialize};

use crate::{dimension_id::DimensionId, interest::ClientViewpoint};

/// Server → client: a short narrative flavour-text notification (BL-82
/// EM-4.8). Travels on the `Events` lane (ordered/reliable, like
/// [`crate::LoginResult`]) — a discrete one-shot notice, not a per-tick
/// state sample. Carries no entity references, so it is registered
/// `make_message_independent` (see [`crate::XindelerProtocolPlugin`]),
/// exactly like `LoginResult`/`TerrainAnchor`.
///
/// Client-side render is a minimal `bevy_ui` timed-fade toast
/// (`xindeler-client::hud_toast`, worksheet [Q3]=A) — deliberately NOT
/// Phase 5's real HUD/notification system; Phase 5 can replace the
/// rendering later without touching this message type or the hook that
/// fires it.
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HudToast {
    pub text: String,
}

/// Registry: which [`DimensionId`] a player's mirrored entity must settle on
/// to trigger an on-enter [`HudToast`], and what text to send.
///
/// Populated by whoever decides a `DmEvent`'s target dimension — today,
/// nobody (no `DmEvent`-triggered auto-spinup exists yet, EM-4.9's job); this
/// task's own tests exercise the hook by registering directly, exactly the
/// way a future EM-4.9 caller will the moment it resolves that association.
/// Starts empty: an app that never registers anything (today's actual
/// running server) sees [`fire_on_enter_toasts`] as a permanent, harmless
/// no-op, mirroring EM-4.2e's `AiGatewayConfig` "seam with zero real callers
/// yet" posture.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct NarrativeHooks(HashMap<DimensionId, String>);

impl NarrativeHooks {
    /// Registers `message` to fire once for any client whose
    /// [`ClientViewpoint::dimension`] becomes (or already is, the first time
    /// it's observed) `dimension`. Overwrites any prior registration for the
    /// same dimension — there is exactly one `on_enter_message` per `DmEvent`
    /// (spec §1.11's schema), so a second call for the same dimension is
    /// either a re-registration after a teardown/respawn cycle or a
    /// programming error on the caller's part; neither should panic.
    pub fn register_on_enter_message(&mut self, dimension: DimensionId, message: impl Into<String>) {
        self.0.insert(dimension, message.into());
    }

    /// The registered on-enter message for `dimension`, if any.
    #[must_use]
    pub fn on_enter_message(&self, dimension: DimensionId) -> Option<&str> {
        self.0.get(&dimension).map(String::as_str)
    }

    /// Whether any dimension currently has a registered on-enter message.
    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

/// Per-connected-client bookkeeping: the last [`DimensionId`] this client was
/// already notified about entering, so [`fire_on_enter_toasts`] fires at most
/// once per client per dimension-entry (a client sitting still in an already-
/// hooked dimension across many ticks must not receive a fresh toast every
/// tick just because [`ClientViewpoint`] gets rewritten by position updates).
/// `pub(crate)`: purely this module's internal dedup state.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NarrativeToastState {
    last_dimension: DimensionId,
}

/// BL-82 EM-4.8's active half: for every connected client entity carrying a
/// [`ClientViewpoint`], checks whether its CURRENT dimension has a
/// registered [`NarrativeHooks`] entry the client hasn't already been
/// notified about, and if so sends exactly one [`HudToast`] targeted with
/// `SendTargets::Single` at that one client — never `SendTargets::All`. A
/// client whose dimension has no hook, or who was already notified for the
/// dimension it's currently in, gets nothing.
///
/// `pub(crate)`, not `pub`: external callers only need [`HudToastPlugin`]
/// (which registers this system), matching `interest::
/// recompute_client_visible_regions`'s own visibility convention.
pub(crate) fn fire_on_enter_toasts(
    mut commands: Commands,
    hooks: Res<NarrativeHooks>,
    clients: Query<(Entity, &ClientViewpoint, Option<&NarrativeToastState>)>,
    mut toasts: MessageWriter<ToClients<HudToast>>,
) {
    for (entity, viewpoint, state) in &clients {
        let already_notified = state.is_some_and(|s| s.last_dimension == viewpoint.dimension);
        if already_notified {
            continue;
        }

        if let Some(text) = hooks.on_enter_message(viewpoint.dimension) {
            toasts.write(ToClients {
                targets: SendTargets::Single(ClientId::from(entity)),
                message: HudToast {
                    text: text.to_owned(),
                },
            });
        }

        // Cache the CURRENT dimension regardless of whether it had a hook —
        // this is what makes "already notified" mean "for THIS dimension",
        // so leaving a hooked dimension and later re-entering it fires
        // again, while sitting still in it (hooked or not) never re-fires.
        commands.entity(entity).insert(NarrativeToastState {
            last_dimension: viewpoint.dimension,
        });
    }
}

/// Registers [`HudToastPlugin`]'s system in `FixedUpdate` (same schedule
/// `ClientInterestPlugin` uses, so a freshly-recomputed [`ClientViewpoint`]
/// is visible to this system before `bevy_replicon`'s own `FixedPostUpdate`
/// replication pass — see that plugin's doc comment for why ordering
/// relative to the mirror/viewpoint writers does not matter for
/// correctness, only for how many ticks a transition can lag).
pub struct HudToastPlugin;

impl Plugin for HudToastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NarrativeHooks>()
            .add_systems(FixedUpdate, fire_on_enter_toasts);
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        app::App,
        ecs::system::RunSystemOnce,
        prelude::{Messages, MinimalPlugins},
        state::app::StatesPlugin,
    };
    use bevy_replicon::{prelude::RepliconPlugins, test_app::ServerTestAppExt};
    use vek::Vec2;

    use super::*;
    use crate::XindelerProtocolPlugin;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins,
            XindelerProtocolPlugin,
        ));
        // `XindelerProtocolPlugin` only registers the `HudToast` MESSAGE
        // type (wire schema); `NarrativeHooks` + `fire_on_enter_toasts`
        // itself live in the separate `HudToastPlugin` (added by
        // `xindeler-server-app` in the real shell). Tests call
        // `fire_on_enter_toasts` directly via `run_system_once` rather than
        // relying on `HudToastPlugin`'s `FixedUpdate` registration (see
        // `interest.rs`'s own tests for the same direct-call convention), so
        // only the resource is needed here, not the whole plugin.
        app.init_resource::<NarrativeHooks>();
        // `.finish()`/`.cleanup()`: a manually-`update()`-driven `App` never
        // reaches Bevy's plugin-lifecycle `finish`/`cleanup` phase on its
        // own (only `App::run()` does) — harmless for the single-client
        // `run_system_once`-driven tests below, but required for
        // `only_the_matching_client_receives_the_toast_not_a_broadcast`'s
        // TWO real `connect_client` handshakes: without it,
        // `bevy_replicon::client::send_protocol_hash` (an `OnEnter
        // (ClientState::Connected)` system) panics reading a `ProtocolHash`
        // resource that `RepliconPlugins`' own `finish()` is what actually
        // computes/inserts — same root cause
        // `xindeler-server-app/tests/interest_management.rs`'s own
        // `new_server_app`/`new_client_app` helpers already document for
        // the real-socket transport, just triggered here by a SECOND
        // in-process client connecting rather than a real socket.
        app.finish();
        app.cleanup();
        app
    }

    fn drain_toasts(app: &mut App) -> Vec<ToClients<HudToast>> {
        app.world_mut()
            .resource_mut::<Messages<ToClients<HudToast>>>()
            .drain()
            .collect()
    }

    /// A client whose viewpoint sits in a hooked dimension gets exactly one
    /// `HudToast`, targeted (not broadcast) at that client.
    #[test]
    fn client_in_hooked_dimension_gets_a_single_targeted_toast() {
        let mut app = new_app();
        let dimension = DimensionId(7);
        app.world_mut()
            .resource_mut::<NarrativeHooks>()
            .register_on_enter_message(dimension, "The gate to Ravenloft creaks open.");

        let client = app
            .world_mut()
            .spawn(ClientViewpoint::new(dimension, Vec2::new(0.0, 0.0), 1))
            .id();

        app.world_mut()
            .run_system_once(fire_on_enter_toasts)
            .expect("system runs");

        let toasts = drain_toasts(&mut app);
        assert_eq!(toasts.len(), 1, "exactly one toast must be sent");
        assert!(
            matches!(toasts[0].targets, SendTargets::Single(id) if id == ClientId::from(client)),
            "the toast must target exactly the one client via SendTargets::Single, got {:?}",
            toasts[0].targets
        );
        assert_eq!(
            toasts[0].message,
            HudToast {
                text: "The gate to Ravenloft creaks open.".to_owned(),
            }
        );
    }

    /// A client whose viewpoint dimension has no registered hook gets
    /// nothing.
    #[test]
    fn client_in_unhooked_dimension_gets_nothing() {
        let mut app = new_app();
        app.world_mut()
            .resource_mut::<NarrativeHooks>()
            .register_on_enter_message(DimensionId(7), "hooked dimension only");

        app.world_mut().spawn(ClientViewpoint::new(
            DimensionId(1),
            Vec2::new(0.0, 0.0),
            1,
        ));

        app.world_mut()
            .run_system_once(fire_on_enter_toasts)
            .expect("system runs");

        assert!(
            drain_toasts(&mut app).is_empty(),
            "a client not in the hooked dimension must receive nothing"
        );
    }

    /// A client sitting still in an already-notified dimension across
    /// multiple runs of the system does NOT get a fresh toast every time.
    #[test]
    fn already_notified_client_is_not_re_toasted() {
        let mut app = new_app();
        let dimension = DimensionId(3);
        app.world_mut()
            .resource_mut::<NarrativeHooks>()
            .register_on_enter_message(dimension, "welcome");

        app.world_mut()
            .spawn(ClientViewpoint::new(dimension, Vec2::new(0.0, 0.0), 1));

        app.world_mut()
            .run_system_once(fire_on_enter_toasts)
            .expect("first run");
        assert_eq!(drain_toasts(&mut app).len(), 1, "first run sends one toast");

        app.world_mut()
            .run_system_once(fire_on_enter_toasts)
            .expect("second run, same dimension");
        assert!(
            drain_toasts(&mut app).is_empty(),
            "a second run with an unchanged dimension must not re-toast"
        );
    }

    /// The literal T47.10 acceptance bar over the REAL replicon wire
    /// (in-process test harness, mirroring `crate::tests`'
    /// `login_request_and_result_round_trip` shape): two connected clients,
    /// one in the hooked dimension and one NOT — only the matching client's
    /// `Messages<HudToast>` receives anything; the other client receives
    /// NOTHING (not merely "a different message" — literally zero).
    #[test]
    fn only_the_matching_client_receives_the_toast_not_a_broadcast() {
        let mut server_app = new_app();
        let mut client_in = new_app();
        let mut client_out = new_app();

        // Diff the server-side `ConnectedClient` entity set around each
        // `connect_client` call (mirrors
        // `xindeler-server-app/tests/interest_management.rs`'s own
        // `connect_client` helper) rather than assuming query iteration
        // order matches spawn order.
        let connected_entities = |app: &mut App| -> Vec<Entity> {
            app.world_mut()
                .query_filtered::<Entity, bevy::ecs::query::With<bevy_replicon::prelude::ConnectedClient>>()
                .iter(app.world())
                .collect()
        };

        server_app.connect_client(&mut client_in);
        let entity_in = *connected_entities(&mut server_app)
            .first()
            .expect("client_in's ConnectedClient entity exists");

        server_app.connect_client(&mut client_out);
        let entity_out = *connected_entities(&mut server_app)
            .iter()
            .find(|e| **e != entity_in)
            .expect("client_out's NEW ConnectedClient entity exists");

        let target_dimension = DimensionId(42);
        server_app
            .world_mut()
            .resource_mut::<NarrativeHooks>()
            .register_on_enter_message(target_dimension, "Welcome to the mist-shrouded manor.");

        server_app
            .world_mut()
            .entity_mut(entity_in)
            .insert(ClientViewpoint::new(
                target_dimension,
                Vec2::new(0.0, 0.0),
                1,
            ));
        server_app
            .world_mut()
            .entity_mut(entity_out)
            .insert(ClientViewpoint::new(
                DimensionId::DEFAULT,
                Vec2::new(0.0, 0.0),
                1,
            ));

        server_app
            .world_mut()
            .run_system_once(fire_on_enter_toasts)
            .expect("system runs");

        server_app.update();
        server_app.exchange_with_client(&mut client_in);
        server_app.exchange_with_client(&mut client_out);
        client_in.update();
        client_out.update();

        let received_in: Vec<_> = client_in
            .world_mut()
            .resource_mut::<Messages<HudToast>>()
            .drain()
            .collect();
        let received_out: Vec<_> = client_out
            .world_mut()
            .resource_mut::<Messages<HudToast>>()
            .drain()
            .collect();

        assert_eq!(
            received_in,
            vec![HudToast {
                text: "Welcome to the mist-shrouded manor.".to_owned(),
            }],
            "the client in the target dimension must receive exactly the registered toast"
        );
        assert!(
            received_out.is_empty(),
            "the client NOT in the target dimension must receive NOTHING — not a broadcast"
        );
    }
}
