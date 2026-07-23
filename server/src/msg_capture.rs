//! BL-82 EM-8.3b — sim-side per-player OUTGOING message capture.
//!
//! The ledger's Part A1 documented three sim-side paths that route a message
//! to a SPECIFIC player exclusively through that player's legacy
//! `comp::Client` per-connection send queue:
//! - chat, via `StateExt::send_chat` (`server/src/state_ext.rs`) — every arm's
//!   recipient resolution (radius/group/faction/direct-uid) ends in a
//!   `client.send_fallible(ServerGeneral::ChatMsg(..))` call.
//! - outcomes, drained INSIDE `Server::tick` by `entity_sync::Sys::run`
//!   (`server/src/sys/entity_sync.rs`) — the per-client radius filter loop only
//!   ever iterates entities that HAVE a `comp::Client` (`(&clients).join()`).
//! - NPC->player dialogue, via the `DialogueEvent` handler
//!   (`server/src/events/interaction.rs`) — `clients.get(target)`.
//!
//! A real dedicated-server (`xindeler-server-app`) replicon-login player
//! entity NEVER gets a `comp::Client` (that type wraps a legacy
//! `network::Participant`, which a replicon/quinnet connection doesn't have
//! — see `client::Client`'s own doc comment, and BL-82 EM-4.2c's
//! `InventoryUpdateBuffer` precedent in `entity_sync.rs` for the identical
//! "no legacy Client is now a normal, not a bug" posture this module
//! extends to chat/outcomes/dialogue). Before this module there was NO
//! observable point where a Bevy bridge plugin could tap any of these three
//! streams for such a player — each site consumed/sent its payload entirely
//! IN-TICK, direct to the `Client`'s socket, with nothing left over to read
//! post-tick.
//!
//! [`OutgoingMessageCapture`] is the fix: a plain, non-Bevy specs
//! [`specs::prelude::Resource`] that the three call sites above ALSO push
//! into whenever they resolve a recipient with no `comp::Client` — never
//! INSTEAD of the existing `Client` send (a `comp::Client`-having recipient
//! keeps working exactly as it did before this task; only the previously
//! silent no-op case now captures). `xindeler-sim-bridge`'s bridge plugins
//! (`chat.rs`/`sfx.rs`/`social.rs`) drain it once per tick and forward each
//! entry to the correct replicon `ClientId`.
//!
//! ## Identity: `Uid`, not `specs::Entity`, not a second correlation scheme
//! Keyed by [`Uid`] — the SAME identity every other per-recipient BL-82
//! mirror in this codebase settled on (BL-82 EM-8.2's
//! `xindeler_protocol::ActiveReplicaSessions`, used by
//! `xindeler-sim-bridge::social`'s `resolve_recipient_targets`). This module
//! deliberately does NOT import the `bevy_replicon` crate's `ClientId` type or
//! `xindeler_protocol` at all — `server` is a pure sim/logic crate,
//! `./scripts/check-engine-isolation.sh` must keep passing, and `Uid` is
//! already the shared vocabulary: the bevy-side bridge resolves
//! `Uid -> ClientId` itself via `ActiveReplicaSessions`, exactly the same
//! lookup `mirror_group_state`/`mirror_dialogue` already perform for their
//! own per-recipient sends.
//!
//! ## Why the listen-server stays a no-op consumer
//! [`OutgoingMessageCapture`] is registered as a resource on EVERY
//! `server::Server` (`Server::new`, `server/src/lib.rs`) — the listen
//! server's embedded sim included — so `Res`/`ReadExpect`-style access never
//! has to special-case which shell it's running on. In practice the buffer
//! stays empty there: the listen server's one real player always carries a
//! legacy `comp::Client` (it authenticates over the legacy loopback socket,
//! not the replicon handshake — see `ActiveReplicaSessions`'s own doc
//! comment for the identical distinction), so every capture call site below
//! only ever fires for the `comp::Client`-LESS branch, which the listen
//! server's one player never takes.

use common::{comp, outcome::Outcome, rtsim, uid::Uid};

/// One captured chat line + the recipient [`Uid`] the sending arm resolved
/// it for (never the sender's uid — `comp::ChatMsg` already carries the
/// speaker's identity internally where relevant).
#[derive(Debug, Clone)]
pub struct CapturedChat {
    pub recipient: Uid,
    pub msg: comp::ChatMsg,
}

/// One captured [`Outcome`] + the recipient [`Uid`] `entity_sync`'s own
/// per-client view-distance radius filter already resolved it for — the
/// SAME filtering a `comp::Client`-having recipient gets, just captured
/// instead of sent directly to a socket.
#[derive(Debug, Clone)]
pub struct CapturedOutcome {
    pub recipient: Uid,
    pub outcome: Outcome,
}

/// One captured NPC→player dialogue turn.
#[derive(Debug, Clone)]
pub struct CapturedDialogue {
    pub recipient: Uid,
    pub sender: Uid,
    pub dialogue: rtsim::Dialogue<true>,
}

/// A per-tick, per-player capture buffer for the three sim-side outgoing-
/// message paths (chat/outcomes/dialogue) — see the module doc comment for
/// the full rationale. Append-only during a tick (`capture_*`), drained
/// wholesale once per tick by the bevy-side bridge (`drain_*`) — draining
/// clears the buffer, so nothing captured this tick can be double-delivered
/// on a later drain, and a captured message that arrives when nobody drains
/// it (e.g. the sim ticking with no `SimServer`-hosting shell attached at
/// all, which cannot happen in practice but is not assumed against) simply
/// accumulates rather than silently vanishing — no data loss, bounded only
/// by how long draining is skipped.
#[derive(Debug, Default)]
pub struct OutgoingMessageCapture {
    chat: Vec<CapturedChat>,
    outcomes: Vec<CapturedOutcome>,
    dialogue: Vec<CapturedDialogue>,
}

impl OutgoingMessageCapture {
    /// Captures one chat line for `recipient`. Called ONLY for a recipient
    /// resolved to have no `comp::Client` — see each `StateExt::send_chat`
    /// arm's own call site.
    pub fn capture_chat(&mut self, recipient: Uid, msg: comp::ChatMsg) {
        self.chat.push(CapturedChat { recipient, msg });
    }

    /// Captures one `Outcome` for `recipient`, already radius-filtered by
    /// the caller (`entity_sync::Sys::run`) exactly like the `comp::Client`
    /// path it parallels.
    pub fn capture_outcome(&mut self, recipient: Uid, outcome: Outcome) {
        self.outcomes.push(CapturedOutcome { recipient, outcome });
    }

    /// Captures one NPC→player dialogue turn addressed to `recipient`, sent
    /// by `sender`.
    pub fn capture_dialogue(
        &mut self,
        recipient: Uid,
        sender: Uid,
        dialogue: rtsim::Dialogue<true>,
    ) {
        self.dialogue.push(CapturedDialogue {
            recipient,
            sender,
            dialogue,
        });
    }

    /// Drains every chat line captured since the last drain, in capture
    /// order. Degrades clean (empty `Vec`) if nothing was captured this
    /// tick — the normal case on the listen server (see module doc
    /// comment).
    #[must_use]
    pub fn drain_chat(&mut self) -> Vec<CapturedChat> { core::mem::take(&mut self.chat) }

    /// Drains every `Outcome` captured since the last drain.
    #[must_use]
    pub fn drain_outcomes(&mut self) -> Vec<CapturedOutcome> { core::mem::take(&mut self.outcomes) }

    /// Drains every dialogue turn captured since the last drain.
    #[must_use]
    pub fn drain_dialogue(&mut self) -> Vec<CapturedDialogue> {
        core::mem::take(&mut self.dialogue)
    }

    #[cfg(test)]
    pub(crate) fn chat_len(&self) -> usize { self.chat.len() }

    #[cfg(test)]
    pub(crate) fn outcomes_len(&self) -> usize { self.outcomes.len() }

    #[cfg(test)]
    pub(crate) fn dialogue_len(&self) -> usize { self.dialogue.len() }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use common::{comp::ChatType, rtsim::DialogueKind};

    use super::*;

    fn uid(n: u64) -> Uid { Uid(NonZeroU64::new(n).expect("nonzero")) }

    fn dummy_chat_msg(text: &str) -> comp::ChatMsg {
        ChatType::Meta.into_msg(comp::Content::Plain(text.to_owned()))
    }

    fn dummy_dialogue() -> rtsim::Dialogue<true> {
        rtsim::Dialogue {
            id: rtsim::DialogueId(1),
            kind: DialogueKind::Start,
        }
    }

    /// Capturing then draining chat returns exactly what was captured, in
    /// order, tagged with the right recipient — and a SECOND drain (nothing
    /// captured in between) is empty, never re-delivering the same message
    /// twice across a tick boundary.
    #[test]
    fn chat_drains_exactly_what_was_captured_once() {
        let mut capture = OutgoingMessageCapture::default();
        capture.capture_chat(uid(1), dummy_chat_msg("hello"));
        capture.capture_chat(uid(2), dummy_chat_msg("world"));
        assert_eq!(capture.chat_len(), 2);

        let drained = capture.drain_chat();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].recipient, uid(1));
        assert_eq!(drained[0].msg.content().as_plain(), Some("hello"));
        assert_eq!(drained[1].recipient, uid(2));
        assert_eq!(drained[1].msg.content().as_plain(), Some("world"));

        assert_eq!(capture.chat_len(), 0);
        assert!(
            capture.drain_chat().is_empty(),
            "a second drain with nothing captured in between must be empty, not repeat the first \
             drain's messages"
        );
    }

    /// Two recipients' outcomes stay attributed to the correct `Uid` — a
    /// regression guard for the class of bug this whole task exists to
    /// avoid (a message captured for the wrong player).
    #[test]
    fn outcomes_stay_attributed_to_the_correct_recipient() {
        let mut capture = OutgoingMessageCapture::default();
        let pos = vek::Vec3::new(0.0, 0.0, 0.0);
        capture.capture_outcome(uid(10), Outcome::Death { pos });
        capture.capture_outcome(uid(20), Outcome::Death { pos });

        let drained = capture.drain_outcomes();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].recipient, uid(10));
        assert_eq!(drained[1].recipient, uid(20));
    }

    /// Dialogue capture round-trips recipient/sender/payload, and draining
    /// clears the buffer (same "no double-delivery across ticks" guarantee
    /// as chat/outcomes).
    #[test]
    fn dialogue_round_trips_and_drains_once() {
        let mut capture = OutgoingMessageCapture::default();
        capture.capture_dialogue(uid(1), uid(2), dummy_dialogue());
        assert_eq!(capture.dialogue_len(), 1);

        let drained = capture.drain_dialogue();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].recipient, uid(1));
        assert_eq!(drained[0].sender, uid(2));
        assert_eq!(drained[0].dialogue.kind, DialogueKind::Start);

        assert!(capture.drain_dialogue().is_empty());
    }

    /// The three buffers are independent — draining one never disturbs the
    /// others (each `Vec` field is its own `mem::take`).
    #[test]
    fn the_three_buffers_are_independent() {
        let mut capture = OutgoingMessageCapture::default();
        capture.capture_chat(uid(1), dummy_chat_msg("x"));
        capture.capture_outcome(uid(1), Outcome::Death {
            pos: vek::Vec3::new(0.0, 0.0, 0.0),
        });
        capture.capture_dialogue(uid(1), uid(2), dummy_dialogue());

        let _ = capture.drain_chat();
        assert_eq!(
            capture.outcomes_len(),
            1,
            "draining chat must not drain outcomes"
        );
        assert_eq!(
            capture.dialogue_len(),
            1,
            "draining chat must not drain dialogue"
        );
    }
}
