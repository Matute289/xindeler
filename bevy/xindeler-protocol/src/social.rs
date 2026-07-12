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
//! ## Why client→server actions are TWO types each (wire + local)
//! Exactly mirroring [`crate::PlayerInput`]/[`crate::LocalPlayerInput`]'s own
//! split: [`GroupActionRequest`]/[`DialogueResponseRequest`] are real
//! `bevy_replicon` CLIENT MESSAGES (`add_client_message`, round-trip tested
//! like `PlayerInput`) — the wire shape a FUTURE genuinely-remote client
//! (EM-4.2b's `net-client`, once group/dialogue gameplay is wired there) will
//! send. But today's only client with an actual controllable player is the
//! LISTEN SERVER (`bevy/xindeler-client`'s `listen-server` feature), which —
//! per `bevy/xindeler-client/src/listen_server.rs`'s own module doc comment —
//! runs `bevy_replicon`'s SERVER ROLE ONLY (no client role, no second world),
//! so there is no connected replicon client to ever produce a `FromClient<_>`
//! for a message this App's own UI writes. [`LocalGroupAction`]/
//! [`LocalDialogueResponse`] are the plain (non-replicon) Bevy messages that
//! actually drive gameplay today, written by the client-side UI and read the
//! SAME frame by `xindeler_sim_bridge::social`'s action-consuming systems —
//! exactly the shared-in-process handoff [`crate::LocalPlayerInput`]'s own doc
//! comment describes for movement input.
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
/// [`DialogueResponseRequest`]/[`LocalDialogueResponse`] back, not a
/// flattened display string. Same v1 broadcast caveat as [`NetGroupState`].
#[derive(Message, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NetDialogue {
    /// The NPC's stable sim `Uid` — the target of any
    /// [`DialogueResponseRequest`]/[`LocalDialogueResponse`] the player sends
    /// back.
    pub sender_uid: u64,
    /// Display name, flattened server-side the same way
    /// [`NetPlayerListEntry::name`] is.
    pub sender_name: String,
    pub dialogue: common::rtsim::Dialogue<true>,
}

/// A group-membership action the local player requests (BL-82 EM-5.8): the
/// shared payload both [`GroupActionRequest`] (wire) and [`LocalGroupAction`]
/// (in-process) carry. Mirrors `common::comp::GroupManip` + the
/// `InitiateInviteEvent`/`InviteResponseEvent` shapes the sim already speaks
/// — the bridge translates this 1:1 into the embedded player's real
/// `client::Client::send_invite`/`accept_invite`/`decline_invite`/
/// `leave_group`/`kick_from_group`/`assign_group_leader` calls (a genuine
/// client→server network round-trip over the loopback socket, never a
/// direct sim-state write — the isolation law's "writes go through the
/// sim's public event/intent API" rule).
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

/// Client → server WIRE message (a future genuinely-remote client's request)
/// — see this module's doc comment for why this is registered
/// (`add_client_message`, round-trip tested) but NOT what drives the
/// listen-server's own gameplay today.
#[derive(Message, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupActionRequest(pub GroupAction);

/// The in-process handoff that actually drives the listen server's group UI
/// today — see this module's doc comment for the full rationale (mirrors
/// [`crate::LocalPlayerInput`]).
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct LocalGroupAction(pub GroupAction);

/// Client → server WIRE message: the player's reply to an outstanding
/// [`NetDialogue`] (a future genuinely-remote client's request). Carries an
/// UNVALIDATED `Dialogue` (`IS_VALIDATED = false`), matching
/// `client::Client::perform_dialogue`'s own signature — the server validates.
#[derive(Message, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DialogueResponseRequest {
    pub target_uid: u64,
    pub dialogue: common::rtsim::Dialogue<false>,
}

/// The in-process handoff that actually drives the listen server's dialogue
/// UI today — see this module's doc comment. Wraps the exact same payload as
/// [`DialogueResponseRequest`] (kept as a distinct type, not a re-use, for
/// the same "wire type vs local type" separation [`crate::PlayerInput`]/
/// [`crate::LocalPlayerInput`] already establish).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct LocalDialogueResponse {
    pub target_uid: u64,
    pub dialogue: common::rtsim::Dialogue<false>,
}

/// Registers the social/group/dialogue wire contract: [`NetPlayerList`]/
/// [`NetGroupState`]/[`NetDialogue`] as server messages (broadcast, no entity
/// references — `make_message_independent` like [`crate::TerrainAnchor`]),
/// [`GroupActionRequest`]/[`DialogueResponseRequest`] as client messages (the
/// `Events` lane, like [`crate::LoginRequest`]), and
/// [`LocalGroupAction`]/[`LocalDialogueResponse`] as plain Bevy messages
/// (harmless, dormant on any App that never writes them — same "both roles
/// compile into every shell" posture `xindeler-transport`'s doc comment
/// establishes for replicon's client/server roles).
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

    app.add_message::<LocalGroupAction>();
    app.add_message::<LocalDialogueResponse>();
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

    /// [`LocalGroupAction`]/[`LocalDialogueResponse`] are plain Bevy messages
    /// — a single App can write and read them back the same frame with no
    /// replicon/connection involved at all (the listen-server shape).
    #[test]
    fn local_messages_are_plain_in_process_bevy_messages() {
        use common::rtsim::{Dialogue, DialogueId, DialogueKind};

        let mut app = new_app();
        app.world_mut()
            .write_message(LocalGroupAction(GroupAction::AcceptInvite));
        app.world_mut().write_message(LocalDialogueResponse {
            target_uid: 9,
            dialogue: Dialogue {
                id: DialogueId(2),
                kind: DialogueKind::Ack { tag: 1 },
            },
        });
        app.update();

        let actions: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalGroupAction>>()
            .drain()
            .collect();
        assert_eq!(actions, vec![LocalGroupAction(GroupAction::AcceptInvite)]);

        let responses: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalDialogueResponse>>()
            .drain()
            .collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].target_uid, 9);
    }
}
