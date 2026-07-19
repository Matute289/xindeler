//! BL-82 EM-5.8 — Social / group / dialogue wire shapes (spec
//! `2026-07-11-bl82-phase5-ui-audio-parity-design.md` §3.2/§6, task board
//! T56.27): [`NetPlayerList`] (who's online), [`NetGroupState`] (party
//! membership/leader/pending invite), and [`NetDialogue`] (NPC dialogue,
//! v1-minimal — the AURORA seam). Follows the exact mirror-pattern doc
//! comment at the top of `crate::narrative` for the server→client half
//! (targeted `SendTargets`/broadcast) and `xindeler_protocol::LocalPlayerInput`
//! for the client→bridge half.
//!
//! ## Why dialogue is real, not a stub
//! `common::rtsim::Dialogue`/`DialogueKind`/`Response` (the NPC dialogue-tree
//! shape AURORA/ORACLE already define) is a fully working, already-shipped
//! server-authoritative system — `client::Client::perform_dialogue`,
//! `common::event::DialogueEvent`,
//! `server::events::interaction::DialogueEvent`, and the agent behavior tree
//! (`server/src/sys/agent/behavior_tree/ interaction.rs`) already speak it
//! end-to-end for the OLD (pre-Bevy) client. [`NetDialogue`] carries that REAL
//! type verbatim (already `Serialize`/ `Deserialize` — it already crosses the
//! wire today via `common_net::msg::ServerGeneral::Dialogue`) rather than
//! inventing a new schema — see the module doc comment on why this is
//! deliberate, not a shortcut.
//!
//! ## The write half: real `FromClient` requests, the unified posture (BL-82 EM-8.3)
//! [`GroupActionRequest`]/[`DialogueResponseRequest`] are the real
//! `bevy_replicon` CLIENT MESSAGES (`add_client_message`, round-trip tested
//! like `PlayerInput`) that drive group/dialogue actions on BOTH shells — the
//! SAME unified path `InventoryActionRequest` already established (see
//! `xindeler-sim-bridge::inventory::resolve_client_entity`'s doc comment). The
//! client UI (`xindeler-client::social_hud`) writes these directly; they
//! surface server-side as `FromClient<_>`, either from a genuinely-remote
//! dedicated-server client (its real `ClientId`) or from the listen-server's
//! own local write echoed back with `ClientId::Server` (`bevy_replicon`'s
//! `add_client_message` local-loopback — see
//! `bevy/xindeler-client/src/listen_server.rs`'s own module doc comment for
//! why that shell runs `bevy_replicon`'s SERVER ROLE ONLY). The bridge's
//! `apply_group_action_requests`/`apply_dialogue_response_requests` resolve
//! the acting sim entity per message (real connection first via
//! `PlayerDimensionSession`, embedded local player as the `ClientId::Server`
//! fallback) and emit the exact sim events the legacy handlers do.
//!
//! Before EM-8.3 this went through a listen-server-only `LocalGroupAction`/
//! `LocalDialogueResponse` + `EmbeddedPlayer` pass-through shortcut (now
//! removed) that had no `EmbeddedPlayer` (and therefore did nothing) on the
//! real dedicated server — the ledger's A1 parity gap, closed the same way
//! EM-5.7's `LocalUnlockSkillRequest` was.
use bevy::{app::App, ecs::message::Message};
use serde::{Deserialize, Serialize};

/// One row of the online-player list (BL-82 EM-5.8). Flattened from the sim's
/// `comp::Player`/`comp::Stats` — the client only needs identity + display
/// name, never `battle_mode`/`uuid` bookkeeping (both stay server-side, spec
/// §3.2 "project, don't dump"). `name` prefers the character's `Stats.name`
/// but falls back to the account `alias` when `Stats` isn't present yet
/// (`xindeler_sim_bridge::social::flatten_name`) — `comp::Player.alias` is
/// the ordinary chosen display/chat name in this codebase (`common::comp::
/// player`), not a secret, so this fallback is not a privacy leak.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetPlayerListEntry {
    /// The player's stable sim `Uid` (see `xindeler_protocol::NetUid`'s own
    /// doc comment for why this crate carries the plain `u64` inner value
    /// rather than `common::uid::Uid` itself).
    pub uid: u64,
    /// Display name — the character's `Stats.name`, already flattened
    /// server-side (`common_i18n::Content::hacky_descriptor`) so the client
    /// never needs the richer `Content`/localization machinery just to show
    /// a name in a list row.
    pub name: String,
}

/// Server → client: every currently-connected player (BL-82 EM-5.8's "who's
/// online" list). Bulk/unbounded data — a MESSAGE, not a per-entity
/// component (spec §3.2's "bulk data = messages" rule), like
/// [`crate::NetFarTerrain`]. Broadcast to every connected client
/// (`SendTargets::All`) — unlike [`NetGroupState`]/[`NetDialogue`], this is
/// NOT private per-player data (every real MMO-lite social list is
/// symmetric), so broadcasting is the permanent design, not a v1 shortcut.
#[derive(Message, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetPlayerList(pub Vec<NetPlayerListEntry>);

/// One member of the local player's current group (BL-82 EM-5.8). Pets are
/// deliberately excluded (matching legacy `voxygen`'s own `group.rs`, which
/// filters `Role::Pet` before ever building its member list). The client
/// resolves live health/energy/buffs by correlating [`Self::uid`] against
/// any mirrored entity's `xindeler_protocol::NetUid` — reusing the
/// already-mirrored per-entity state instead of duplicating it here (the
/// SAME "reuse the already-mirrored NetHealth" pattern EM-5.2's overhead
/// health bars use), degrading to "out of range" when no such entity is
/// currently mirrored (matching legacy's own `hud-group-out_of_range` copy).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetGroupMember {
    pub uid: u64,
    pub name: String,
}

/// An incoming invite the local player can accept/decline (BL-82 EM-5.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetInviteKind {
    Group,
    Trade,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetPendingInvite {
    pub inviter_uid: u64,
    pub inviter_name: String,
    pub kind: NetInviteKind,
    /// Seconds remaining before the sim times the invite out, tracked by the
    /// bridge itself (`xindeler_sim_bridge::social::InviteTimeoutCache`) —
    /// the sim's own `common::comp::invite::Invite` component carries no
    /// timestamp (only the INVITER's `PendingInvites` does), so the bridge
    /// records "first seen" itself, mirroring `PRESENTED_INVITE_TIMEOUT_DUR`
    /// (`server/src/events/invite.rs`, duplicated the same documented way
    /// `crate::interest::CHUNK_FUZZ` duplicates `server::presence::CHUNK_FUZZ`
    /// — that module is not `pub` outside the `server` crate).
    pub remaining_secs: f32,
}

/// Server → client: the local player's current group membership + any
/// incoming invite (BL-82 EM-5.8). A discrete per-player snapshot, not a
/// per-tick continuous sample — sent whenever it changes (mirrors
/// `xindeler_sim_bridge::combat_hud`'s `NetCombo`/`NetXp` change-dedup
/// posture), like [`crate::narrative::HudToast`]. v1 broadcasts to ALL
/// connected clients (`SendTargets::All`) rather than `SendTargets::Single`
/// (contrast [`crate::narrative::HudToast`], which DOES target a single
/// client): there is today no connected-client↔sim-player-identity
/// correlation resource this crate can reach (the same gap
/// `xindeler_protocol::interest`'s own module doc names for `ClientViewpoint`)
/// — the only topology this is exercised on (the listen server) has exactly
/// one recipient anyway, matching `xindeler_sim_bridge::send_terrain_updates`'s
/// own identical "v1 broadcasts to ALL... per-client interest management is a
/// follow-up" posture. Follow-up: once a login/session-derived
/// correlation exists (EM-4.2c/d), switch to `SendTargets::Single` like
/// `HudToast`.
#[derive(Message, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetGroupState {
    pub group_name: Option<String>,
    pub leader: Option<u64>,
    pub members: Vec<NetGroupMember>,
    pub pending_invite: Option<NetPendingInvite>,
}

/// Server → client: an NPC dialogue turn addressed to the local player
/// (BL-82 EM-5.8, v1-minimal — the AURORA seam). Carries the REAL
/// `common::rtsim::Dialogue<true>` verbatim (see this module's doc comment
/// for why) — the client needs the full structured `DialogueKind`/`Response`
/// shape (tags, response options, given items) to construct a well-formed
/// [`DialogueResponseRequest`] back, not a flattened display string. Same v1
/// broadcast caveat as [`NetGroupState`].
#[derive(Message, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetDialogue {
    /// The NPC's stable sim `Uid` — the target of any
    /// [`DialogueResponseRequest`] the player sends back.
    pub sender_uid: u64,
    /// Display name, flattened server-side the same way
    /// [`NetPlayerListEntry::name`] is.
    pub sender_name: String,
    pub dialogue: common::rtsim::Dialogue<true>,
}

/// A group-membership action the local player requests (BL-82 EM-5.8): the
/// payload [`GroupActionRequest`] carries. Mirrors `common::comp::GroupManip`
/// and the `InitiateInviteEvent`/`InviteResponseEvent` shapes the sim already
/// speaks — the bridge's `apply_group_action_requests` (BL-82 EM-8.3)
/// translates this 1:1 into the resolved acting entity's real
/// `InitiateInviteEvent`/`InviteResponseEvent`/`GroupManipEvent` (via
/// `State::emit_event_now`, never a direct sim-state write — the isolation
/// law's "writes go through the sim's public event/intent API" rule).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GroupAction {
    /// Invite the given player (by `Uid`) to the local player's group.
    Invite(u64),
    AcceptInvite,
    DeclineInvite,
    Leave,
    /// Kick the given member (leader-only; the sim itself enforces the
    /// permission check — a non-leader's request is simply rejected
    /// server-side, never trusted client-side).
    Kick(u64),
    /// Hand group leadership to the given member (leader-only, same
    /// server-side enforcement note as [`Self::Kick`]).
    AssignLeader(u64),
}

/// Client → server: the real `bevy_replicon` request driving group actions on
/// BOTH shells (BL-82 EM-8.3 — see this module's doc comment for the full
/// unified-write rationale; the listen-server's own local write is echoed
/// back as `FromClient` with `ClientId::Server`, so this is registered and
/// consumed the same way on every shell, not just a future-remote-client
/// placeholder).
#[derive(Message, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupActionRequest(pub GroupAction);

/// Client → server: the player's reply to an outstanding [`NetDialogue`] — the
/// real `bevy_replicon` request driving dialogue on BOTH shells (BL-82 EM-8.3,
/// same unified posture as [`GroupActionRequest`]). Carries an UNVALIDATED
/// `Dialogue` (`IS_VALIDATED = false`), matching `client::Client::
/// perform_dialogue`'s own signature — the server validates.
#[derive(Message, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DialogueResponseRequest {
    pub target_uid: u64,
    pub dialogue: common::rtsim::Dialogue<false>,
}

/// Registers the social/group/dialogue wire contract: [`NetPlayerList`]/
/// [`NetGroupState`]/[`NetDialogue`] as server messages (broadcast, no entity
/// references — `make_message_independent` like [`crate::TerrainAnchor`]) and
/// [`GroupActionRequest`]/[`DialogueResponseRequest`] as client messages (the
/// `Events` lane, like [`crate::LoginRequest`]).
pub(crate) fn register(app: &mut App) {
    use bevy_replicon::prelude::{ClientMessageAppExt, ServerMessageAppExt};

    app.add_server_message::<NetPlayerList>(crate::XindelerChannel::Events.delivery())
        .make_message_independent::<NetPlayerList>();
    app.add_server_message::<NetGroupState>(crate::XindelerChannel::Events.delivery())
        .make_message_independent::<NetGroupState>();
    app.add_server_message::<NetDialogue>(crate::XindelerChannel::Events.delivery())
        .make_message_independent::<NetDialogue>();

    app.add_client_message::<GroupActionRequest>(crate::XindelerChannel::Events.delivery());
    app.add_client_message::<DialogueResponseRequest>(crate::XindelerChannel::Events.delivery());
}

#[cfg(test)]
mod tests {
    use bevy::{prelude::*, state::app::StatesPlugin};
    use bevy_replicon::{
        prelude::{FromClient, RepliconPlugins, ServerPlugin},
        test_app::ServerTestAppExt,
    };

    use super::*;
    use crate::XindelerProtocolPlugin;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
            XindelerProtocolPlugin,
        ))
        .finish();
        app
    }

    /// [`NetPlayerList`] round-trips server → client, broadcast to every
    /// connected client — the T56.27 "who's online" acceptance bar.
    #[test]
    fn net_player_list_replicates_to_every_client() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let list = NetPlayerList(vec![NetPlayerListEntry {
            uid: 1,
            name: "Hero".to_owned(),
        }]);
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: list.clone(),
        });
        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let received: Vec<_> = client_app
            .world_mut()
            .resource_mut::<Messages<NetPlayerList>>()
            .drain()
            .collect();
        assert_eq!(received, vec![list]);
    }

    /// [`NetGroupState`] (leader/members/pending invite) round-trips
    /// server → client byte-for-byte.
    #[test]
    fn net_group_state_round_trips() {
        use bevy_replicon::prelude::{SendTargets, ToClients};

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let state = NetGroupState {
            group_name: Some("Adventurers".to_owned()),
            leader: Some(1),
            members: vec![
                NetGroupMember {
                    uid: 1,
                    name: "Hero".to_owned(),
                },
                NetGroupMember {
                    uid: 2,
                    name: "Sidekick".to_owned(),
                },
            ],
            pending_invite: Some(NetPendingInvite {
                inviter_uid: 3,
                inviter_name: "Stranger".to_owned(),
                kind: NetInviteKind::Group,
                remaining_secs: 25.0,
            }),
        };
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: state.clone(),
        });
        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let received: Vec<_> = client_app
            .world_mut()
            .resource_mut::<Messages<NetGroupState>>()
            .drain()
            .collect();
        assert_eq!(received, vec![state]);
    }

    /// BL-82 EM-8.2 regression guard: `NetGroupState` sent as
    /// `SendTargets::Single` reaches ONLY the named client — never every
    /// connected one. This is the message-level analogue of
    /// `owner_visibility::skillset_is_owner_scoped_across_two_real_clients`'s
    /// real two-client `VisibilityFilter::Scope` guard: `NetGroupState`/
    /// `NetDialogue` are MESSAGES (not components), so `SendTargets::Single`
    /// — resolved by `xindeler_sim_bridge::social`'s mirrors via
    /// `xindeler_protocol::ActiveReplicaSessions` — is the correct per-owner
    /// mechanism here, not a `VisibilityFilter`. Proves the real bug this
    /// fixes: before EM-8.2, these mirrors hardcoded `SendTargets::All`, so a
    /// non-owning client would have received another player's private group
    /// state too.
    #[test]
    fn net_group_state_targets_only_the_owning_client() {
        use bevy_replicon::{
            prelude::{ClientId, ConnectedClient, SendTargets, ToClients},
            test_app::TestClientEntity,
        };

        fn server_connection_entity(server_app: &mut App, client_app: &App) -> Entity {
            let target = **client_app.world().resource::<TestClientEntity>();
            server_app
                .world_mut()
                .query::<(Entity, &ConnectedClient)>()
                .iter(server_app.world())
                .map(|(e, _)| e)
                .find(|&e| e == target)
                .expect("the client's own connection entity exists server-side")
        }

        let mut server_app = new_app();
        let mut owner_client = new_app();
        let mut other_client = new_app();

        server_app.connect_client(&mut owner_client);
        let owner_entity = server_connection_entity(&mut server_app, &owner_client);
        server_app.connect_client(&mut other_client);

        let state = NetGroupState {
            group_name: Some("Adventurers".to_owned()),
            leader: Some(1),
            members: Vec::new(),
            pending_invite: None,
        };
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::Single(ClientId::Client(owner_entity)),
            message: state.clone(),
        });

        server_app.update();
        server_app.exchange_with_client(&mut owner_client);
        owner_client.update();
        server_app.exchange_with_client(&mut other_client);
        other_client.update();

        let owner_received: Vec<_> = owner_client
            .world_mut()
            .resource_mut::<Messages<NetGroupState>>()
            .drain()
            .collect();
        assert_eq!(
            owner_received,
            vec![state],
            "the owning client must receive its own NetGroupState"
        );

        let other_received: Vec<_> = other_client
            .world_mut()
            .resource_mut::<Messages<NetGroupState>>()
            .drain()
            .collect();
        assert!(
            other_received.is_empty(),
            "a non-owning client must NEVER receive another player's NetGroupState — this is \
             exactly the SendTargets::All leak BL-82 EM-8.2 closes"
        );
    }

    /// The `NetDialogue` analogue of
    /// `net_group_state_targets_only_the_owning_client` above — an NPC
    /// dialogue turn addressed to one player must never reach a different
    /// connected client.
    #[test]
    fn net_dialogue_targets_only_the_owning_client() {
        use bevy_replicon::{
            prelude::{ClientId, ConnectedClient, SendTargets, ToClients},
            test_app::TestClientEntity,
        };
        use common::rtsim::{Dialogue, DialogueId, DialogueKind};

        fn server_connection_entity(server_app: &mut App, client_app: &App) -> Entity {
            let target = **client_app.world().resource::<TestClientEntity>();
            server_app
                .world_mut()
                .query::<(Entity, &ConnectedClient)>()
                .iter(server_app.world())
                .map(|(e, _)| e)
                .find(|&e| e == target)
                .expect("the client's own connection entity exists server-side")
        }

        let mut server_app = new_app();
        let mut owner_client = new_app();
        let mut other_client = new_app();

        server_app.connect_client(&mut owner_client);
        let owner_entity = server_connection_entity(&mut server_app, &owner_client);
        server_app.connect_client(&mut other_client);

        let dialogue = NetDialogue {
            sender_uid: 5,
            sender_name: "Village Elder".to_owned(),
            dialogue: Dialogue {
                id: DialogueId(1),
                kind: DialogueKind::Ack { tag: 0 },
            },
        };
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::Single(ClientId::Client(owner_entity)),
            message: dialogue.clone(),
        });

        server_app.update();
        server_app.exchange_with_client(&mut owner_client);
        owner_client.update();
        server_app.exchange_with_client(&mut other_client);
        other_client.update();

        let owner_received: Vec<_> = owner_client
            .world_mut()
            .resource_mut::<Messages<NetDialogue>>()
            .drain()
            .collect();
        assert_eq!(
            owner_received,
            vec![dialogue],
            "the owning client must receive its own NetDialogue turn"
        );

        let other_received: Vec<_> = other_client
            .world_mut()
            .resource_mut::<Messages<NetDialogue>>()
            .drain()
            .collect();
        assert!(
            other_received.is_empty(),
            "a non-owning client must NEVER receive another player's NetDialogue — this is \
             exactly the SendTargets::All leak BL-82 EM-8.2 closes"
        );
    }

    /// [`GroupActionRequest`] travels client → server and surfaces as
    /// `FromClient<_>` — the wire-shape half (a future remote client's path),
    /// exactly like `crate::tests::player_input_reaches_server`.
    #[test]
    fn group_action_request_reaches_server() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let request = GroupActionRequest(GroupAction::Invite(42));
        client_app.world_mut().write_message(request);

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<GroupActionRequest>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message, request);
    }

    /// [`DialogueResponseRequest`] travels client → server the same way.
    #[test]
    fn dialogue_response_request_reaches_server() {
        use common::rtsim::{Dialogue, DialogueId, DialogueKind};

        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let request = DialogueResponseRequest {
            target_uid: 7,
            dialogue: Dialogue {
                id: DialogueId(1),
                kind: DialogueKind::Ack { tag: 0 },
            },
        };
        client_app.world_mut().write_message(request.clone());

        client_app.update();
        server_app.exchange_with_client(&mut client_app);
        server_app.update();

        let received: Vec<_> = server_app
            .world_mut()
            .resource_mut::<Messages<FromClient<DialogueResponseRequest>>>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message, request);
    }
}
