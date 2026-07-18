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
//! ## ⚠️ Known gap: not wired into `xindeler-server-app` yet (disclosed, not
//! silently narrowed — ecs-design-reviewer finding, BL-82 EM-5.4)
//! [`ChatBridgePlugin`] is added ONLY by `xindeler-client::listen_server`
//! today. `xindeler-server-app` (the real dedicated multiplayer server) has
//! NO chat wiring at all. This module's design — one [`EmbeddedPlayer`]'s
//! inbox, broadcast via `SendTargets::All` — is safe ONLY because a
//! listen-server has exactly ONE real chat participant; the sim's own
//! `Server::send_chat` (`server/src/state_ext.rs`) already does correct
//! per-client recipient narrowing (Say/Region by distance, Tell by uid, …),
//! so copying this exact shape onto `xindeler-server-app`'s multi-client
//! topology would leak one player's private/proximity-scoped lines to every
//! other connected client. A real fix needs a NEW bridge reading each active
//! replica session's own chat inbox and targeting `SendTargets::Single` per
//! recipient — not a copy-paste of this module — resolving each recipient's
//! `ClientId` via `xindeler_protocol::ActiveReplicaSessions` (BL-82 EM-8.2:
//! the SAME correlation resource `crate::social`'s `NetGroupState`/
//! `NetDialogue` mirrors now use for the identical problem, so this future
//! chat fix no longer needs to invent its own). Tracked in
//! `docs/backlog/engine-migration.md`'s EM-5.4 row as a required follow-up
//! before the EM-5.13 cutover (§Q7=A locks "full parity" — multi-player chat
//! on the real server is core, not optional).

use bevy::{
    app::{App, Plugin, Update},
    ecs::{
        change_detection::NonSendMut,
        message::{MessageReader, MessageWriter},
        schedule::IntoScheduleConfigs,
    },
};
use bevy_replicon::prelude::{SendTargets, ToClients};
use common::{comp, uid::IdMaps};
use specs::WorldExt;
use xindeler_protocol::{ChatSendRequest, NetChatChannel, NetChatMsg, NetUid};

use crate::{EmbeddedPlayer, SimServer, player::tick_player};

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

/// Registers both chat-bridge systems in `Update`, `.after(tick_player)` (see
/// the module doc comment for why this schedule/ordering, not
/// `FixedUpdate`/`tick_sim`). Add alongside [`crate::PlayerBridgePlugin`]
/// (after it — same convention [`crate::CombatHudMirrorPlugin`] follows for
/// its own `.after` dependency) in whichever shell hosts the embedded player
/// (only the listen-server client does today; `xindeler-server-app` has no
/// embedded player, so this plugin has nothing to do there and is simply not
/// added).
pub struct ChatBridgePlugin;

impl Plugin for ChatBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (apply_chat_send_requests, broadcast_embedded_chat).after(tick_player),
        );
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
}
