//! BL-82 EM-4.9 (Phase D, T51.8) — per-dimension atmosphere replication
//! server → client: the one genuinely new protocol piece the drill needs.
//!
//! Mirrors [`crate::dm_event`]'s narrative-hook sibling
//! (`xindeler_protocol::narrative`'s `HudToast`/`SendTargets::Single`)
//! exactly, just for [`AtmosphereProfile`] instead of a flavour-text string:
//! - [`DimensionAtmospheres`] is a server-side `DimensionId ->
//!   AtmosphereProfile` table, populated by `xindeler-sim-bridge::oracle`'s
//!   ingest producer the moment a `DmEvent` resolves its target dimension
//!   (mirrors `xindeler_protocol::narrative::NarrativeHooks`'s own "populated
//!   by whoever decides a DmEvent's target dimension" posture).
//! - [`send_atmosphere_on_dimension_change`] watches every connected client's
//!   `ClientViewpoint` (the same per-client "which dimension is this client in"
//!   signal `fire_on_enter_toasts` already reads) and sends exactly ONE
//!   [`SetClientAtmosphere`], targeted with `SendTargets::Single`, to the one
//!   client whose viewpoint just settled on a dimension THIS table has an entry
//!   for — never a broadcast.
//! - The client receives it and calls `AtmosphereController::retarget` (the
//!   apply side already exists and animates over `transition_secs` — no new
//!   render code; see `xindeler-client::atmosphere`'s receiver system).
//!
//! ## Why this lives here, not `xindeler-protocol`
//! [`AtmosphereProfile`] is this crate's own type (`crate::atmosphere`);
//! `xindeler-protocol` never depends on `xindeler-oracle-host` (the reverse
//! edge already exists — see [`crate::dm_event`]'s own module doc comment),
//! so a message carrying this payload cannot be declared over there without
//! either duplicating the type or introducing a cycle. This crate already
//! depends on `xindeler-protocol` (for [`ClientViewpoint`]/[`DimensionId`]),
//! so it can register + send/receive its OWN `bevy_replicon` message directly
//! — the same non-circular direction every other cross-crate edge in this
//! workspace already takes.
//!
//! ## Listen-server note
//! For a single-App listen server (no remote client), nothing populates
//! [`DimensionAtmospheres`] beyond the default profile a fresh boot never
//! needs replicated at all — this seam is dormant there today (no
//! `DmEvent`-triggered producer runs in that shell), which is the same safe,
//! documented posture `HudToastPlugin` itself takes for a listen-server with
//! no narrative hooks registered. The networked (`--connect`) path, driven
//! by `xindeler-sim-bridge::oracle`, is what actually exercises it.
use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        component::Component,
        entity::Entity,
        message::Message,
        resource::Resource,
        system::{Commands, Query, Res},
    },
};
use bevy_replicon::prelude::{Channel, ClientId, SendTargets, ServerMessageAppExt, ToClients};
use serde::{Deserialize, Serialize};
use xindeler_protocol::{ClientViewpoint, DimensionId};

use crate::atmosphere::AtmosphereProfile;

/// Server → client: retarget the client's live [`AtmosphereController`]
/// (`xindeler-client::atmosphere`) at `profile`. Travels on the ordered
/// Events lane (a discrete, occasional retarget, like
/// [`crate::narrative::HudToast`]), so it is registered
/// `make_message_independent` too — it carries no entity references.
///
/// [`AtmosphereController`]: crate::atmosphere::AtmosphereController
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SetClientAtmosphere(pub AtmosphereProfile);

/// Server-side per-dimension atmosphere table (BL-82 EM-4.9): the default
/// dimension has no entry (the client already boots with the shipped default
/// profile locally — nothing to replicate there); an event dimension gets
/// one the moment its `DmEvent.atmosphere` is resolved at spinup.
#[derive(Resource, Debug, Clone, Default)]
pub struct DimensionAtmospheres(HashMap<DimensionId, AtmosphereProfile>);

impl DimensionAtmospheres {
    /// Records/overwrites `dimension`'s atmosphere profile.
    pub fn set(&mut self, dimension: DimensionId, profile: AtmosphereProfile) {
        self.0.insert(dimension, profile);
    }

    /// Drops `dimension`'s entry (BL-82 EM-4.9 follow-up, bevy-migration-
    /// reviewer MINOR finding): called once a dimension tears down, so this
    /// table doesn't grow by one stale entry per retired event for the life
    /// of the server process. A no-op if `dimension` had no entry.
    pub fn remove(&mut self, dimension: DimensionId) { self.0.remove(&dimension); }

    /// The registered profile for `dimension`, if any.
    #[must_use]
    pub fn get(&self, dimension: DimensionId) -> Option<&AtmosphereProfile> {
        self.0.get(&dimension)
    }
}

/// Per-connected-client bookkeeping: the last [`DimensionId`] this client was
/// already sent an atmosphere for, so [`send_atmosphere_on_dimension_change`]
/// fires at most once per client per dimension-entry — mirrors
/// `xindeler_protocol::narrative::NarrativeToastState` exactly.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AtmosphereSyncState {
    last_dimension: DimensionId,
}

/// BL-82 EM-4.9's active half: for every connected client entity carrying a
/// [`ClientViewpoint`], checks whether its CURRENT dimension has a
/// registered [`DimensionAtmospheres`] entry the client hasn't already
/// received, and if so sends exactly one [`SetClientAtmosphere`] targeted
/// with `SendTargets::Single` at that one client — never `SendTargets::All`.
///
/// `pub(crate)`: external callers only need [`ServerAtmosphereSyncPlugin`]
/// (which registers this system).
pub(crate) fn send_atmosphere_on_dimension_change(
    mut commands: Commands,
    table: Res<DimensionAtmospheres>,
    clients: Query<(Entity, &ClientViewpoint, Option<&AtmosphereSyncState>)>,
    mut out: bevy::ecs::message::MessageWriter<ToClients<SetClientAtmosphere>>,
) {
    for (entity, viewpoint, state) in &clients {
        let already_sent = state.is_some_and(|s| s.last_dimension == viewpoint.dimension);
        if already_sent {
            continue;
        }

        if let Some(profile) = table.get(viewpoint.dimension) {
            out.write(ToClients {
                targets: SendTargets::Single(ClientId::from(entity)),
                message: SetClientAtmosphere(profile.clone()),
            });
        }

        // Cache the CURRENT dimension regardless of whether it had an
        // entry — mirrors `NarrativeToastState`'s own "already notified
        // means for THIS dimension" reasoning: leaving a hooked dimension
        // and later re-entering it sends again, sitting still never re-sends.
        commands.entity(entity).insert(AtmosphereSyncState {
            last_dimension: viewpoint.dimension,
        });
    }
}

/// Registers ONLY the [`SetClientAtmosphere`] message type — symmetric,
/// added on BOTH client and server Apps (like `XindelerProtocolPlugin`
/// itself), so the replicon protocol hash/channel registry always matches.
///
/// Split from [`ServerAtmosphereSyncPlugin`] (which owns the
/// [`DimensionAtmospheres`] table together with the sending system,
/// server-only) exactly like `HudToast`'s message registration
/// (`XindelerProtocolPlugin`) is split from the sending logic
/// (`HudToastPlugin`) — except this message can't live in the SAME
/// crate/plugin as `HudToast`'s, see the module doc comment for why.
pub struct AtmosphereSyncMessagePlugin;

impl Plugin for AtmosphereSyncMessagePlugin {
    fn build(&self, app: &mut App) {
        app.add_server_message::<SetClientAtmosphere>(Channel::Ordered)
            .make_message_independent::<SetClientAtmosphere>();
    }
}

/// Server-only: [`DimensionAtmospheres`] together with
/// [`send_atmosphere_on_dimension_change`], plus
/// [`AtmosphereSyncMessagePlugin`] if not already added (defensive —
/// `xindeler-server-app` is expected to be the only caller). Never add this on
/// the client — only [`AtmosphereSyncMessagePlugin`] belongs there (see
/// `xindeler-client::atmosphere`'s receiver-side registration).
pub struct ServerAtmosphereSyncPlugin;

impl Plugin for ServerAtmosphereSyncPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AtmosphereSyncMessagePlugin>() {
            app.add_plugins(AtmosphereSyncMessagePlugin);
        }
        app.init_resource::<DimensionAtmospheres>()
            .add_systems(FixedUpdate, send_atmosphere_on_dimension_change);
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

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin, RepliconPlugins));
        app.add_plugins(AtmosphereSyncMessagePlugin);
        app.init_resource::<DimensionAtmospheres>();
        app.finish();
        app.cleanup();
        app
    }

    /// A client whose viewpoint sits in a dimension with a registered
    /// atmosphere entry gets exactly one `SetClientAtmosphere`, targeted (not
    /// broadcast) at that client — the Style-B (headless, no real sim) proof
    /// T51.8 asks for.
    #[test]
    fn client_in_dimension_with_a_registered_profile_gets_a_single_targeted_message() {
        let mut app = new_app();
        let dimension = DimensionId(7);
        let profile = AtmosphereProfile {
            fog_density: 0.5,
            time_lock: Some(23.5),
            ..Default::default()
        };
        app.world_mut()
            .resource_mut::<DimensionAtmospheres>()
            .set(dimension, profile.clone());

        let client = app
            .world_mut()
            .spawn(ClientViewpoint::new(dimension, Vec2::new(0.0, 0.0), 1))
            .id();

        app.world_mut()
            .run_system_once(send_atmosphere_on_dimension_change)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ToClients<SetClientAtmosphere>>>()
            .drain()
            .collect();
        assert_eq!(sent.len(), 1, "exactly one message must be sent");
        assert!(
            matches!(sent[0].targets, SendTargets::Single(id) if id == ClientId::from(client)),
            "the message must target exactly the one client via SendTargets::Single, got {:?}",
            sent[0].targets
        );
        assert_eq!(sent[0].message, SetClientAtmosphere(profile));
    }

    /// A client whose viewpoint dimension has NO registered atmosphere entry
    /// (e.g. the default dimension, which never needs one — the client
    /// already boots with the shipped default profile) gets nothing.
    #[test]
    fn client_in_an_unregistered_dimension_gets_nothing() {
        let mut app = new_app();
        app.world_mut().spawn(ClientViewpoint::new(
            DimensionId::DEFAULT,
            Vec2::new(0.0, 0.0),
            1,
        ));

        app.world_mut()
            .run_system_once(send_atmosphere_on_dimension_change)
            .expect("system runs");

        assert!(
            app.world_mut()
                .resource_mut::<Messages<ToClients<SetClientAtmosphere>>>()
                .drain()
                .next()
                .is_none(),
            "a client whose dimension has no registered profile must receive nothing"
        );
    }

    /// A client sitting still in an already-notified dimension across
    /// multiple runs does NOT get re-sent the same profile every tick.
    #[test]
    fn already_notified_client_is_not_resent() {
        let mut app = new_app();
        let dimension = DimensionId(3);
        app.world_mut()
            .resource_mut::<DimensionAtmospheres>()
            .set(dimension, AtmosphereProfile::default());
        app.world_mut()
            .spawn(ClientViewpoint::new(dimension, Vec2::new(0.0, 0.0), 1));

        app.world_mut()
            .run_system_once(send_atmosphere_on_dimension_change)
            .expect("first run");
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<ToClients<SetClientAtmosphere>>>()
                .drain()
                .count(),
            1
        );

        app.world_mut()
            .run_system_once(send_atmosphere_on_dimension_change)
            .expect("second run, same dimension");
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<ToClients<SetClientAtmosphere>>>()
                .drain()
                .count(),
            0,
            "a second run with an unchanged dimension must not re-send"
        );
    }

    /// The T51.8 acceptance bar over the REAL replicon wire (in-process test
    /// harness): two connected clients, one whose viewpoint is in a
    /// registered dimension and one whose isn't — only the matching client's
    /// `Messages<SetClientAtmosphere>` receives anything.
    #[test]
    fn only_the_matching_client_receives_the_message_not_a_broadcast() {
        let mut server_app = new_app();
        let mut client_in = new_app();
        let mut client_out = new_app();

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
        let profile = AtmosphereProfile {
            fog_density: 0.9,
            ..Default::default()
        };
        server_app
            .world_mut()
            .resource_mut::<DimensionAtmospheres>()
            .set(target_dimension, profile.clone());

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
            .run_system_once(send_atmosphere_on_dimension_change)
            .expect("system runs");

        server_app.update();
        server_app.exchange_with_client(&mut client_in);
        server_app.exchange_with_client(&mut client_out);
        client_in.update();
        client_out.update();

        let received_in: Vec<_> = client_in
            .world_mut()
            .resource_mut::<Messages<SetClientAtmosphere>>()
            .drain()
            .collect();
        let received_out: Vec<_> = client_out
            .world_mut()
            .resource_mut::<Messages<SetClientAtmosphere>>()
            .drain()
            .collect();

        assert_eq!(
            received_in,
            vec![SetClientAtmosphere(profile)],
            "the client in the target dimension must receive exactly the registered profile"
        );
        assert!(
            received_out.is_empty(),
            "the client NOT in the target dimension must receive NOTHING — not a broadcast"
        );
    }
}
