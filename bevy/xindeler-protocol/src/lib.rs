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
use bevy_replicon::prelude::{AppRuleExt, Channel, ClientMessageAppExt};
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
}
