//! BL-82 EM-5.4 — the chat wire contract (spec §2 EM-5.4, §3.2's "bulk data
//! = messages, not per-component replication" rule).
//!
//! There is no per-message *entity* — a chat line is not attached to
//! anything replicated, unlike `NetHealth`/`NetEnergy`/etc. So (per spec
//! §3.2) this travels as plain replicon MESSAGES, not a replicated
//! component:
//! - [`NetChatMsg`]: server → client, one already-classified/rendered chat
//!   line, broadcast on the `Events` lane like [`crate::narrative::HudToast`]
//!   (`make_message_independent`, registered in
//!   [`crate::XindelerProtocolPlugin`]).
//! - [`ChatSendRequest`]: client → server, either a channel-scoped line (the
//!   tab the player currently has selected) or a raw `/command arg1 arg2…` the
//!   player typed directly — full slash-command parity, not just the six named
//!   channels.
//!
//! ## Why [`ChatSendRequest`] has two shapes (`Channel`/`Command`), not one
//! The sim's chat model (`common::comp::chat::ChatMode`/`ChatType`) is
//! entirely COMMAND-DRIVEN server-side: `/say`, `/region`, `/group`,
//! `/world`, `/faction` set the sender's persistent `ChatMode` component AND
//! send the message in one shot (`server/src/cmd.rs`'s `handle_say`/
//! `handle_region`/…); `/tell <alias> <message…>` is the same shape with a
//! target alias as the first arg. So a channel-tab send and a manually-typed
//! `/command` are, from the WIRE's point of view, the exact same underlying
//! action (`ClientGeneral::Command(name, args)`) — [`ChatSendRequest::Channel`]
//! is just a convenience shape so the UI doesn't have to hand-format a
//! command string for its own tab buttons, while [`ChatSendRequest::Command`]
//! covers everything else the player might type (`/tell`, `/w`, admin
//! commands, …) verbatim. Both arms resolve to the SAME
//! `client::Client::send_command` call bridge-side
//! (`xindeler-sim-bridge::chat`) — see that module's doc comment.
//!
//! ## Why no `target` field on `Channel`
//! `Tell` needs a target alias, but `/tell <alias> <message>` already round-
//! trips correctly through [`ChatSendRequest::Command`] (the server rejoins
//! `args[1..]` into the message text itself — see `handle_tell`), so `Tell`
//! is reachable purely by typing the command; it does not need its own
//! structured send path. [`NetChatChannel::Tell`] still exists as a
//! *receive-side* classification (so the scrollback can filter/tag whisper
//! lines), it is just not one of the auto-formatted send tabs.

use bevy::ecs::message::Message;
use common::{comp::chat::ChatType, uid::Uid};
use serde::{Deserialize, Serialize};

/// Compact wire classification of a chat line's channel (BL-82 EM-5.4).
///
/// Collapses the sim's `ChatType<String>` (16 variants, several carrying
/// generic group/faction-name payloads the client doesn't need to see) into
/// the handful of channels the chat UI actually distinguishes as tabs —
/// "project, don't dump" (spec §3.2).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NetChatChannel {
    /// Nearby players (`ChatType::Say`).
    Say,
    /// Same world region (`ChatType::Region`).
    Region,
    /// The sender's current group/party (`ChatType::Group`).
    Group,
    /// The sender's current faction (`ChatType::Faction`).
    Faction,
    /// Every connected player (`ChatType::World`).
    World,
    /// A private one-on-one message (`ChatType::Tell`) — receive/filter
    /// only, see the module doc comment for why sending it goes through
    /// [`ChatSendRequest::Command`] instead of this channel directly.
    Tell,
    /// NPC speech, shown in chat as well as (elsewhere) a speech bubble
    /// (`ChatType::Npc`/`NpcSay`/`NpcTell`).
    Npc,
    /// Automated/read-only lines: join/leave, command output/errors, kill
    /// feed, group/faction meta-notices, and anything else — collapses
    /// `ChatType::{Online,Offline,CommandInfo,CommandError,Kill,GroupMeta,
    /// FactionMeta,Meta}`. Never a valid SEND target.
    System,
}

impl NetChatChannel {
    /// Classifies a sim [`ChatType`], returning the collapsed channel plus
    /// the speaking entity's [`Uid`] (`None` for channel kinds with no
    /// single speaker, e.g. [`NetChatChannel::System`]).
    ///
    /// Pure data mapping (no ECS access) — the actual sender-alias lookup
    /// needs a live sim storage read and stays in
    /// `xindeler-sim-bridge::chat`, which calls this first.
    #[must_use]
    pub fn from_chat_type(chat_type: &ChatType<String>) -> (Self, Option<Uid>) {
        match *chat_type {
            ChatType::Say(uid) => (Self::Say, Some(uid)),
            ChatType::Region(uid) => (Self::Region, Some(uid)),
            ChatType::Group(uid, _) => (Self::Group, Some(uid)),
            ChatType::Faction(uid, _) => (Self::Faction, Some(uid)),
            ChatType::World(uid) => (Self::World, Some(uid)),
            ChatType::Tell(from, _to) => (Self::Tell, Some(from)),
            ChatType::Npc(uid) | ChatType::NpcSay(uid) => (Self::Npc, Some(uid)),
            ChatType::NpcTell(from, _to) => (Self::Npc, Some(from)),
            ChatType::Online(_)
            | ChatType::Offline(_)
            | ChatType::CommandInfo
            | ChatType::CommandError
            | ChatType::Kill(..)
            | ChatType::GroupMeta(_)
            | ChatType::FactionMeta(_)
            | ChatType::Meta => (Self::System, None),
        }
    }

    /// The command keyword this channel auto-formats to when sent via
    /// [`ChatSendRequest::Channel`] — matches `ServerChatCommand::keyword()`
    /// exactly for the five sendable channels (`common::cmd`), `None` for
    /// the three receive-only kinds ([`Self::Tell`]/[`Self::Npc`]/
    /// [`Self::System`]).
    #[must_use]
    pub const fn send_command_name(self) -> Option<&'static str> {
        match self {
            Self::Say => Some("say"),
            Self::Region => Some("region"),
            Self::Group => Some("group"),
            Self::Faction => Some("faction"),
            Self::World => Some("world"),
            Self::Tell | Self::Npc | Self::System => None,
        }
    }
}

/// One Fluent argument value carried on the wire for a localized chat line
/// (BL-82 EM-5.16). The recursive `common_i18n::Content` arg tree is
/// deliberately NOT put on the wire: the bridge flattens every arg to one of
/// these two leaf shapes server-side (a nested localized arg degrades to its
/// `hacky_descriptor` plain string — a documented v1 cut, since chat args are
/// `Nat` or plain strings in practice, never nested localized content).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum NetChatArg {
    /// A plain-text argument (a player alias, a flattened nested content, …).
    Text(String),
    /// A natural-number argument (`common_i18n::LocalizationArg::Nat`).
    Nat(u64),
}

/// A wire-serialisable localized chat payload (BL-82 EM-5.16): a `.ftl` key
/// (optionally one attribute) plus already-flattened Fluent args, resolved
/// CLIENT-side via `xindeler_ui::i18n::Localization::tr_args`/`tr_attr` so a
/// live language switch re-localizes the scrollback. Built bridge-side from a
/// `common_i18n::Content::Localized`/`Key`/`Attr` (command feedback) OR from a
/// localizable `ChatType` (Online/Offline join/leave) — see
/// `xindeler-sim-bridge::chat`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NetLocalizedContent {
    /// The Fluent message key (e.g. `players-list-header`,
    /// `hud-chat-online_msg`).
    pub key: String,
    /// The message ATTRIBUTE, when the source was a `Content::Attr(key, attr)`
    /// (resolved via `Localization::tr_attr`, which takes no args). `None` for
    /// the common value-message case (resolved via `tr_args`).
    pub attr: Option<String>,
    /// Flattened Fluent args (order irrelevant — Fluent looks them up by name).
    pub args: Vec<(String, NetChatArg)>,
}

/// One chat line delivered to the client (BL-82 EM-5.4). Server → client
/// broadcast (`SendTargets::All` — chat has no per-recipient narrowing yet;
/// `Tell`'s privacy is enforced server-side by who the sim actually routes
/// the underlying `ChatMsg` to, same as legacy). No entity references, so it
/// registers `make_message_independent` like
/// [`crate::narrative::HudToast`]/[`crate::LoginResult`].
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetChatMsg {
    pub channel: NetChatChannel,
    /// The speaking entity's stable sim identity — `None` for a channel with
    /// no single speaker ([`System`]). The sanctioned wire type for this
    /// (`crate::NetUid`, already used to correlate a mirrored figure back to
    /// its sim `Uid`/`NpcId` for AURORA, BL-15/BL-83) rather than a bare
    /// `u64`, so a future consumer (self-mention highlight, click-to-whisper)
    /// can match it directly against a mirrored entity's own `NetUid`
    /// component without a manual re-wrap.
    ///
    /// [`System`]: NetChatChannel::System
    pub sender_uid: Option<crate::NetUid>,
    /// The speaker's display alias, resolved SERVER-SIDE
    /// (`xindeler-sim-bridge::chat` reads `comp::Player::alias` off the
    /// sim). `None` when the sender has no resolvable alias (an NPC, or a
    /// system line).
    pub sender_alias: Option<String>,
    /// Already-rendered plain text. v1 renders via `Content::as_plain()`
    /// (falling back to a placeholder for genuinely localized `Content` —
    /// full Fluent rendering of arbitrary chat `Content` is EM-5.16's job,
    /// the i18n-depth epic); this keeps the wire shape simple and the
    /// client dumb.
    pub text: String,
    /// BL-82 EM-5.16: the OPTIONAL structured localized payload. `Some` for a
    /// genuinely localized line (command feedback, join/leave) — the client
    /// resolves it through `Localization::tr_args`/`tr_attr` at render time and
    /// re-resolves it on a locale change. `None` for a plain player-typed line,
    /// whose already-final text is in [`Self::text`]. When `Some`,
    /// [`Self::text`] still carries a best-effort server-side fallback (the
    /// bare key) for any consumer that ignores the payload.
    /// `#[serde(default)]` is additive at the Rust type level (any code still
    /// building the pre-EM-5.16 four-field literal keeps compiling once this
    /// field gets a default in a struct-update `..Default::default()`-style
    /// site, and self-describing formats like `ron` degrade old data
    /// gracefully). It is NOT a cross-version wire guarantee on bincode/
    /// postcard (non-self-describing): decoding an old byte stream that
    /// lacks this trailing field would still fail — harmless here since
    /// client and server (embedded listen-server + co-shipped replicon
    /// server) always ship from the same build, so no old `NetChatMsg` bytes
    /// ever cross a version boundary.
    #[serde(default)]
    pub localized: Option<NetLocalizedContent>,
}

/// Client → server: send a chat line (BL-82 EM-5.4). Registered as a
/// replicon CLIENT message (`add_client_message`, like
/// [`crate::PlayerInput`]/[`crate::LoginRequest`]) for a future real remote
/// client (EM-4.2c-derived control); the LISTEN-SERVER path reads the same
/// plain local `Messages<ChatSendRequest>` queue directly (no replicon
/// client role exists there — see `xindeler-sim-bridge::chat`'s doc comment)
/// and applies it to the embedded player's real `client::Client`, exactly
/// how movement (`LocalPlayerInput`) and jump (`client.handle_input`)
/// already reach the sim: a genuine call through the Client's own public
/// network API, never a direct sim-state write (isolation law).
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ChatSendRequest {
    /// A channel-tab-scoped line — see the module doc comment for why this
    /// covers only the five sendable channels (`Say`/`Region`/`Group`/
    /// `Faction`/`World`), never `Tell`/`Npc`/`System`.
    Channel {
        channel: NetChatChannel,
        text: String,
    },
    /// A raw `/command arg1 arg2 …` the player typed directly (leading `/`
    /// already stripped from `name`) — covers `/tell`, `/w`, and any other
    /// server chat command, full parity with legacy's slash-command support.
    Command { name: String, args: Vec<String> },
}

#[cfg(test)]
mod tests {
    use common::uid::Uid;

    use super::*;

    fn uid(n: u64) -> Uid { Uid(std::num::NonZeroU64::new(n).expect("non-zero test uid")) }

    /// Every player-sendable `ChatType` variant classifies to the matching
    /// [`NetChatChannel`] and carries the speaker's `Uid`.
    #[test]
    fn player_chat_types_classify_with_their_speaker_uid() {
        let cases = [
            (ChatType::Say(uid(1)), NetChatChannel::Say),
            (ChatType::Region(uid(1)), NetChatChannel::Region),
            (
                ChatType::Group(uid(1), "party".to_owned()),
                NetChatChannel::Group,
            ),
            (
                ChatType::Faction(uid(1), "reds".to_owned()),
                NetChatChannel::Faction,
            ),
            (ChatType::World(uid(1)), NetChatChannel::World),
        ];
        for (chat_type, expected) in cases {
            let (channel, sender) = NetChatChannel::from_chat_type(&chat_type);
            assert_eq!(channel, expected, "{chat_type:?}");
            assert_eq!(sender, Some(uid(1)), "{chat_type:?}");
        }
    }

    /// `Tell(from, to)` classifies as `Tell`, keyed by the SENDER (`from`),
    /// not the recipient.
    #[test]
    fn tell_classifies_with_the_sender_not_the_recipient() {
        let (channel, sender) = NetChatChannel::from_chat_type(&ChatType::Tell(uid(1), uid(2)));
        assert_eq!(channel, NetChatChannel::Tell);
        assert_eq!(sender, Some(uid(1)));
    }

    /// NPC speech variants all collapse to `Npc`, keyed by the speaker.
    #[test]
    fn npc_speech_variants_collapse_to_npc_channel() {
        for (chat_type, speaker) in [
            (ChatType::Npc(uid(9)), uid(9)),
            (ChatType::NpcSay(uid(9)), uid(9)),
            (ChatType::NpcTell(uid(9), uid(2)), uid(9)),
        ] {
            let (channel, sender) = NetChatChannel::from_chat_type(&chat_type);
            assert_eq!(channel, NetChatChannel::Npc);
            assert_eq!(sender, Some(speaker));
        }
    }

    /// Meta/automated variants collapse to `System` with no sender.
    #[test]
    fn automated_variants_collapse_to_system_with_no_sender() {
        use common::comp::chat::{KillSource, KillType};

        let cases = [
            ChatType::Online(uid(1)),
            ChatType::Offline(uid(1)),
            ChatType::CommandInfo,
            ChatType::CommandError,
            ChatType::Kill(KillSource::FallDamage, uid(1)),
            ChatType::GroupMeta("party".to_owned()),
            ChatType::FactionMeta("reds".to_owned()),
            ChatType::Meta,
        ];
        for chat_type in cases {
            let (channel, sender) = NetChatChannel::from_chat_type(&chat_type);
            assert_eq!(channel, NetChatChannel::System, "{chat_type:?}");
            assert_eq!(sender, None, "{chat_type:?}");
        }
        // Silence an unused-import warning if `KillType` ever stops being
        // needed directly (kept for readability of the `KillSource` variant
        // above).
        let _ = KillType::Other;
    }

    /// Only the five channel-tab-sendable kinds resolve a command name;
    /// `Tell`/`Npc`/`System` are receive-only.
    #[test]
    fn only_five_channels_resolve_a_send_command_name() {
        assert_eq!(NetChatChannel::Say.send_command_name(), Some("say"));
        assert_eq!(NetChatChannel::Region.send_command_name(), Some("region"));
        assert_eq!(NetChatChannel::Group.send_command_name(), Some("group"));
        assert_eq!(NetChatChannel::Faction.send_command_name(), Some("faction"));
        assert_eq!(NetChatChannel::World.send_command_name(), Some("world"));
        assert_eq!(NetChatChannel::Tell.send_command_name(), None);
        assert_eq!(NetChatChannel::Npc.send_command_name(), None);
        assert_eq!(NetChatChannel::System.send_command_name(), None);
    }

    /// A `NetChatMsg` carrying a `NetLocalizedContent` payload (key + attr +
    /// mixed `Text`/`Nat` args) survives a bincode round trip unchanged — the
    /// additive `localized` field is on the wire exactly as built.
    #[test]
    fn localized_chat_payload_round_trips() {
        let msg = NetChatMsg {
            channel: NetChatChannel::System,
            sender_uid: None,
            sender_alias: None,
            text: "players-list-header".to_owned(),
            localized: Some(NetLocalizedContent {
                key: "players-list-header".to_owned(),
                attr: None,
                args: vec![
                    ("count".to_owned(), NetChatArg::Nat(2)),
                    (
                        "player_list".to_owned(),
                        NetChatArg::Text("Hero, Villain".to_owned()),
                    ),
                ],
            }),
        };
        let bytes =
            bincode::serde::encode_to_vec(&msg, bincode::config::legacy()).expect("serialize");
        let (back, _): (NetChatMsg, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::legacy())
                .expect("deserialize");
        assert_eq!(back, msg);
    }
}
