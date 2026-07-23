//! BL-82 EM-5.4 — the chat bridge: projects the embedded player's real
//! `client::Client` chat traffic onto the wire ([`NetChatMsg`]) and applies
//! player-typed sends ([`ChatSendRequest`]) back onto that same Client.
//!
//! Unlike [`crate::combat_hud`] (a per-entity `Net*` component mirror read
//! straight off the sim's ECS storages), chat has no per-message *entity* to
//! project onto — spec §3.2's "bulk data = messages, not per-component
//! replication" rule. So this module's two systems both revolve around the
//! embedded [`EmbeddedPlayer`]'s `client::Client`, not [`crate::SimMirror`]:
//!
//! - [`broadcast_embedded_chat`] drains [`EmbeddedPlayer::drain_pending_chat`]
//!   (chat lines [`crate::player::tick_player`] captured off `client.tick()`'s
//!   returned events THIS frame — the only place they surface) and broadcasts
//!   each as a [`ToClients<NetChatMsg>`], resolving the speaker's display alias
//!   off the sim's `comp::Player` storage (a real ECS read, but keyed by `Uid`,
//!   not by [`crate::SimMirror`]'s sim↔Bevy entity map — a chat line's speaker
//!   may not even be a currently-mirrored/visible entity, e.g. a world-channel
//!   message from someone far away).
//! - [`apply_chat_send_requests`] reads plain local [`ChatSendRequest`]
//!   messages (the client UI writes these via `MessageWriter` in the SAME
//!   listen-server App — there is no `bevy_replicon` CLIENT role in that
//!   topology at all, see `xindeler-client::listen_server`'s own module doc
//!   comment, so these never arrive wrapped in `FromClient<_>` the way a real
//!   remote client's send eventually will) and applies each to the embedded
//!   Client via [`EmbeddedPlayer::send_chat_request`] — a genuine network send
//!   over the loopback socket to the real embedded `Server`, never a direct
//!   sim-state write (isolation law rule 4), exactly how movement
//!   (`LocalPlayerInput` → `client.tick`) and jump (`client.handle_input`)
//!   already reach the sim.
//!
//! Both systems run in `Update` (frame rate, matching
//! [`crate::player::tick_player`]'s own schedule — chat is driven by the
//! embedded Client's tick, not the sim's 30 Hz `FixedUpdate`), ordered
//! `.after(tick_player)` so a chat event captured THIS frame is broadcast the
//! SAME frame, not one frame stale.
//!
//! ## BL-82 EM-8.3b: [`broadcast_captured_chat`] — the dedicated-server half
//! [`broadcast_embedded_chat`] above stays listen-server-only by design (one
//! [`EmbeddedPlayer`] inbox, `SendTargets::All` — safe only because a
//! listen-server has exactly one real chat participant). Closing chat for a
//! REAL dedicated-server client needed a NEW sim-side hook (EM-8.3b): every
//! `StateExt::send_chat` (`server/src/state_ext.rs`) arm now ALSO captures
//! into `server::msg_capture::OutgoingMessageCapture`, keyed by recipient
//! `Uid`, for exactly the recipients that have no legacy `comp::Client` (a
//! real replicon-login player never has one). [`broadcast_captured_chat`]
//! drains that buffer every `FixedUpdate` tick and resolves each recipient's
//! `Uid` to a real `ClientId` via `xindeler_protocol::ActiveReplicaSessions`
//! (BL-82 EM-8.2 — the SAME correlation resource `crate::social`'s
//! `NetGroupState`/`NetDialogue` mirrors already use for the identical
//! problem), targeting `SendTargets::Single` per recipient — NEVER `All`, so
//! one player's private/proximity-scoped line can never leak to a different
//! connected client. A captured `Uid` with no correlated session (a
//! disconnect/login race) is simply DROPPED, not broadcast and not routed to
//! `SendTargets::SERVER_ONLY` (unlike `crate::social::
//! resolve_recipient_targets`'s fallback) — there is no legitimate "local
//! echo" target on a dedicated server with no embedded player, so guessing
//! one would risk misdelivering a captured line to the wrong place. Runs in
//! `FixedUpdate` (sim cadence — the capture buffer is written by the sim's
//! OWN tick, unlike [`broadcast_embedded_chat`]'s frame-rate embedded-Client
//! read), `.after(tick_sim)`, matching `crate::social`'s own mirror
//! ordering. [`ChatBridgePlugin`] now registers BOTH systems; on the listen
//! server, [`broadcast_captured_chat`] is a harmless no-op (its capture
//! buffer stays empty there — see `msg_capture`'s own doc comment for why).

use bevy::{
    app::{App, FixedUpdate, Plugin, Update},
    ecs::{
        change_detection::{NonSendMut, Res},
        message::{MessageReader, MessageWriter},
        schedule::IntoScheduleConfigs,
    },
};
use bevy_replicon::prelude::{SendTargets, ToClients};
use common::{comp, uid::IdMaps};
use server::msg_capture::CapturedChat;
use specs::WorldExt;
use xindeler_protocol::{
    ActiveReplicaSessions, ChatSendRequest, NetChatChannel, NetChatMsg, NetUid,
};

use crate::{EmbeddedPlayer, SimServer, player::tick_player, tick_sim};

/// Resolves `uid`'s display alias off the sim's `comp::Player` storage
/// (`None` if the uid doesn't currently resolve to a live entity, or that
/// entity carries no `Player` component — e.g. an NPC speaker).
fn resolve_sender_alias(sim: &SimServer, uid: common::uid::Uid) -> Option<String> {
    let ecs = sim.server.state().ecs();
    let entity = ecs.read_resource::<IdMaps>().uid_entity(uid)?;
    ecs.read_storage::<comp::Player>()
        .get(entity)
        .map(|player| player.alias.clone())
}

/// Renders a chat message's [`comp::Content`] to plain text. v1 uses
/// `Content::as_plain()` where possible (the overwhelming common case —
/// player-typed lines are always `Content::Plain`); genuinely localized
/// content (system/command messages built from a `Content::Localized` key)
/// falls back to its `hacky_descriptor()` wrapped in brackets rather than
/// rendering nothing — full Fluent rendering of arbitrary chat `Content` is
/// EM-5.16's job (the i18n-depth epic), not this task's.
fn render_content(content: &comp::Content) -> String {
    content
        .as_plain()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("[{}]", content.hacky_descriptor()))
}

/// Projects one sim [`comp::ChatMsg`] onto the wire [`NetChatMsg`] shape:
/// classify the channel + speaker uid ([`NetChatChannel::from_chat_type`],
/// pure data mapping), resolve the speaker's alias off the sim (`None` for a
/// speakerless/unresolvable line), and render the content to plain text.
fn project_chat_msg(sim: &SimServer, msg: &comp::ChatMsg) -> NetChatMsg {
    let (channel, sender_uid) = NetChatChannel::from_chat_type(&msg.chat_type);
    let sender_alias = sender_uid.and_then(|uid| resolve_sender_alias(sim, uid));
    NetChatMsg {
        channel,
        sender_uid: sender_uid.map(|uid| NetUid(uid.0.get())),
        sender_alias,
        text: render_content(msg.content()),
    }
}

/// Drains every chat line the embedded player's Client received this frame
/// and broadcasts each as a [`ToClients<NetChatMsg>`] (`SendTargets::All` —
/// see the module doc comment for why chat has no per-recipient narrowing at
/// this layer; a `Tell`'s privacy is already enforced by the sim only ever
/// routing it to the intended recipient's `Client` in the first place, so a
/// listen-server with exactly one embedded player never leaks anyone else's
/// whisper). A no-op (and the pending queue simply never accumulates,
/// `drain_pending_chat` still runs to keep it empty) if either the sim or the
/// embedded player doesn't exist yet.
pub fn broadcast_embedded_chat(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSendMut<EmbeddedPlayer>>,
    mut writer: MessageWriter<ToClients<NetChatMsg>>,
) {
    let (Some(sim), Some(mut player)) = (sim, player) else {
        return;
    };
    for msg in player.drain_pending_chat() {
        writer.write(ToClients {
            targets: SendTargets::All,
            message: project_chat_msg(&sim, &msg),
        });
    }
}

/// Reads every locally-queued [`ChatSendRequest`] (see the module doc
/// comment for why these are plain local messages, not `FromClient<_>`, on
/// the listen-server path) and applies each to the embedded player's real
/// Client via [`EmbeddedPlayer::send_chat_request`]. Drains the reader even
/// with no [`EmbeddedPlayer`] present (nothing to apply to yet, e.g. before
/// the world finishes booting) so requests never pile up across frames.
pub fn apply_chat_send_requests(
    player: Option<NonSendMut<EmbeddedPlayer>>,
    mut requests: MessageReader<ChatSendRequest>,
) {
    match player {
        Some(mut player) => {
            for request in requests.read() {
                player.send_chat_request(request);
            }
        },
        None => {
            requests.read().for_each(drop);
        },
    }
}

/// BL-82 EM-8.3b: drains `server::msg_capture::OutgoingMessageCapture`'s
/// captured chat lines (see the module doc comment) and forwards each to its
/// resolved recipient `ClientId` via [`ActiveReplicaSessions`] — targeted
/// `SendTargets::Single`, NEVER `All`. A captured recipient `Uid` with no
/// correlated session is dropped (see module doc comment for why, not routed
/// to a fallback). A no-op if no [`SimServer`] exists yet.
pub fn broadcast_captured_chat(
    sim: Option<NonSendMut<SimServer>>,
    active: Res<ActiveReplicaSessions>,
    mut writer: MessageWriter<ToClients<NetChatMsg>>,
) {
    let Some(sim) = sim else { return };
    let captured = sim
        .server
        .state()
        .ecs()
        .write_resource::<server::msg_capture::OutgoingMessageCapture>()
        .drain_chat();
    for CapturedChat { recipient, msg } in captured {
        let Some(client_id) = active.client_for_uid(recipient.0.get()) else {
            continue;
        };
        writer.write(ToClients {
            targets: SendTargets::Single(client_id),
            message: project_chat_msg(&sim, &msg),
        });
    }
}

/// Registers all three chat-bridge systems: the two listen-server-only
/// embedded-player systems in `Update`, `.after(tick_player)` (see the
/// module doc comment for why this schedule/ordering) — add alongside
/// [`crate::PlayerBridgePlugin`] (after it — same convention
/// [`crate::CombatHudMirrorPlugin`] follows for its own `.after`
/// dependency) in whichever shell hosts the embedded player — plus (BL-82
/// EM-8.3b) [`broadcast_captured_chat`] in `FixedUpdate`, `.after(tick_sim)`,
/// which works on ANY shell (listen server included, where it is a
/// harmless no-op — see that function's own doc comment).
/// `init_resource::<ActiveReplicaSessions>()` is idempotent (BL-82 EM-8.2/8.3
/// precedent, see `crate::social::SocialMirrorPlugin`'s own doc comment) so
/// this plugin's `broadcast_captured_chat` never panics for want of the
/// resource existing even if added before `SocialMirrorPlugin`.
pub struct ChatBridgePlugin;

impl Plugin for ChatBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveReplicaSessions>();
        app.add_systems(
            Update,
            (apply_chat_send_requests, broadcast_embedded_chat).after(tick_player),
        );
        app.add_systems(FixedUpdate, broadcast_captured_chat.after(tick_sim));
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        app::App,
        ecs::{message::Messages, system::RunSystemOnce},
        prelude::MinimalPlugins,
    };
    use common::{comp::chat::ChatType, resources::BattleMode};
    use specs::Builder;

    use super::*;
    use crate::boot_test_server;

    /// [`project_chat_msg`]: a plain `Say` line from a real `comp::Player`
    /// entity resolves the speaker's alias AND renders the plain text
    /// verbatim.
    #[test]
    fn projects_a_say_message_with_resolved_alias_and_plain_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut sim = boot_test_server(dir.path()).expect("test server boots");

        let uid = {
            let ecs = sim.server.state_mut().ecs_mut();
            let entity = ecs
                .create_entity()
                .with(comp::Player::new(
                    "Hero".to_owned(),
                    BattleMode::PvE,
                    uuid::Uuid::nil(),
                    None,
                ))
                .build();
            // `IdMaps::allocate` is the real server-side path that mints a
            // fresh `Uid` AND registers the `uid_entity` mapping in one call
            // (`common::uid::IdMaps`'s own doc: "Only used on the server") —
            // the same call a real player-creation path makes, so
            // `resolve_sender_alias`'s lookup resolves exactly like it would
            // for a genuine connected player.
            ecs.write_resource::<IdMaps>().allocate(entity)
        };

        let msg: comp::ChatMsg =
            ChatType::Say(uid).into_msg(comp::Content::Plain("hello there".to_owned()));
        let net = project_chat_msg(&sim, &msg);

        assert_eq!(net.channel, NetChatChannel::Say);
        assert_eq!(net.sender_uid, Some(NetUid(uid.0.get())));
        assert_eq!(net.sender_alias.as_deref(), Some("Hero"));
        assert_eq!(net.text, "hello there");
    }

    /// A `System`-collapsed chat type (no speaker) projects with no
    /// `sender_uid`/`sender_alias`, and non-plain `Content` falls back to a
    /// bracketed descriptor rather than rendering nothing.
    #[test]
    fn projects_a_system_message_with_no_sender_and_a_content_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sim = boot_test_server(dir.path()).expect("test server boots");

        let msg: comp::ChatMsg =
            ChatType::CommandInfo.into_msg(comp::Content::localized("command-help"));
        let net = project_chat_msg(&sim, &msg);

        assert_eq!(net.channel, NetChatChannel::System);
        assert_eq!(net.sender_uid, None);
        assert_eq!(net.sender_alias, None);
        assert!(
            net.text.starts_with('[') && net.text.ends_with(']'),
            "non-plain content must fall back to a bracketed descriptor, got {:?}",
            net.text
        );
    }

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<ChatSendRequest>();
        app.add_message::<ToClients<NetChatMsg>>();
        app.init_resource::<ActiveReplicaSessions>();
        app.insert_non_send(sim);
        app
    }

    /// [`apply_chat_send_requests`] with no [`EmbeddedPlayer`] present drains
    /// the reader (never panics, never leaves requests piling up) — the
    /// "degrade clean" rule (spec §3.2), exercised before the player exists
    /// (e.g. mid-world-boot).
    #[test]
    fn apply_with_no_embedded_player_drains_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.world_mut().write_message(ChatSendRequest::Channel {
            channel: NetChatChannel::Say,
            text: "hello".to_owned(),
        });

        app.world_mut()
            .run_system_once(apply_chat_send_requests)
            .expect("system runs without a player");
    }

    /// [`broadcast_embedded_chat`] with no [`EmbeddedPlayer`] present is a
    /// harmless no-op (no broadcast written) — the sim alone isn't enough.
    #[test]
    fn broadcast_with_no_embedded_player_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(broadcast_embedded_chat)
            .expect("system runs without a player");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ToClients<NetChatMsg>>>()
            .drain()
            .collect();
        assert!(sent.is_empty(), "nothing to broadcast without a player");
    }

    /// BL-82 EM-8.3b end-to-end: a real `StateExt::send_chat` `Say` call
    /// captures for a `comp::Client`-less recipient WITHIN range (via the new
    /// sim-side hook in `server::state_ext::send_chat`), and
    /// [`broadcast_captured_chat`] resolves that recipient's real `ClientId`
    /// via `ActiveReplicaSessions` and delivers it `SendTargets::Single` —
    /// while a second `comp::Client`-less recipient OUTSIDE `SAY_DISTANCE`
    /// receives nothing at all. This is the exact regression this whole task
    /// exists to prevent: a message captured for the wrong player (here,
    /// captured despite being out of range) or delivered to the wrong
    /// client.
    #[test]
    fn captured_say_chat_reaches_only_the_in_range_recipients_own_client() {
        use bevy_replicon::prelude::ClientId;
        use common::{
            comp::Pos,
            uid::{IdMaps, Uid},
        };
        use server::state_ext::StateExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let (uid_near, uid_far) = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();

            // `IdMaps::allocate` only registers the uid<->entity MAPPING — it
            // does NOT insert the `Uid` component onto the entity's own
            // storage (confirmed against `common::uid::IdMaps::allocate`'s
            // real body). `send_chat`'s `Say` arm joins on a REAL `Uid`
            // component (`&uids` in the recipient loop), so every entity
            // that must be found by that join needs BOTH: the mapping (for
            // `entity_from_uid`) AND the component itself — matching the
            // exact pattern `crate::social::tests::
            // mirror_group_state_targets_each_connected_player_privately`
            // already establishes for the identical reason.
            let mut spawn_player_at = |x: f32| {
                let entity = ecs
                    .create_entity()
                    .with(comp::Player::new(
                        "recipient".to_owned(),
                        common::resources::BattleMode::PvE,
                        uuid::Uuid::nil(),
                        None,
                    ))
                    .with(Pos(vek::Vec3::new(x, 0.0, 0.0)))
                    .build();
                let uid = ecs.write_resource::<IdMaps>().allocate(entity);
                ecs.write_storage::<Uid>().insert(entity, uid).unwrap();
                uid
            };
            let uid_near = spawn_player_at(50.0); // within SAY_DISTANCE (100.0)
            let uid_far = spawn_player_at(500.0); // outside SAY_DISTANCE

            // The speaker: any entity with a resolvable Uid + Pos.
            let speaker_entity = ecs
                .create_entity()
                .with(Pos(vek::Vec3::new(0.0, 0.0, 0.0)))
                .build();
            let speaker_uid = ecs.write_resource::<IdMaps>().allocate(speaker_entity);
            ecs.write_storage::<Uid>()
                .insert(speaker_entity, speaker_uid)
                .unwrap();

            sim.server.state().send_chat(
                comp::ChatType::Say(speaker_uid).into_msg(comp::Content::Plain("hello".to_owned())),
                false,
            );

            (uid_near, uid_far)
        };

        let client_near = ClientId::Client(bevy::ecs::entity::Entity::from_raw_u32(1).unwrap());
        let client_far = ClientId::Client(bevy::ecs::entity::Entity::from_raw_u32(2).unwrap());
        {
            let mut active = app.world_mut().resource_mut::<ActiveReplicaSessions>();
            active.insert(uid_near.0.get(), client_near);
            active.insert(uid_far.0.get(), client_far);
        }

        app.world_mut()
            .run_system_once(broadcast_captured_chat)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ToClients<NetChatMsg>>>()
            .drain()
            .collect();

        assert_eq!(
            sent.len(),
            1,
            "exactly one captured chat message must be relayed — only the in-range recipient, \
             never the out-of-range one"
        );
        assert!(
            matches!(sent[0].targets, SendTargets::Single(cid) if cid == client_near),
            "the captured message must target the IN-RANGE recipient's own client, never a \
             broadcast and never the wrong client"
        );
        assert_eq!(sent[0].message.text, "hello");
    }
}
