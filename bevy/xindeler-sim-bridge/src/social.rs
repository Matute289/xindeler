//! BL-82 EM-5.8 — Social / group / dialogue mirror (spec §3.2/§6, task board
//! T56.27/T56.28): projects the sim's real, already-shipped `comp::Player`/
//! `comp::group`/`comp::invite`/`rtsim::Dialogue` state onto
//! `xindeler_protocol::social`'s wire types, following the exact
//! `NetHealth`/`NetLoadout` UPSERT-or-broadcast-on-change pattern
//! `crate::combat_hud` already established — but, like that module, as its
//! own separate additive system set rather than folded into
//! `mirror_sim_entities`.
//!
//! Three READ mirrors + two ACTION-applying systems:
//! - [`mirror_player_list`]: reads every `comp::Player`-tagged sim entity
//!   directly off [`SimServer`] (no [`EmbeddedPlayer`] needed — this works for
//!   any shell hosting the sim, not just the listen server). Broadcast to all
//!   (a roster is public), unchanged.
//! - [`mirror_group_state`]: BL-82 EM-8.3 GENERALIZED to EVERY connected player
//!   — it iterates every fully-logged-in replicon session
//!   ([`xindeler_protocol::ActiveReplicaSessions::iter`]) UNIONED with the
//!   listen-server's single in-game embedded local player, computes each one's
//!   own group state off the sim, and sends it TARGETED to just that recipient.
//!   BL-82 EM-8.2 supplied the correlation ([`ActiveReplicaSessions`] answers
//!   "which `ClientId` controls the sim player with this `Uid`"); EM-8.3 uses
//!   it here to scope the per-recipient send via [`resolve_recipient_targets`]:
//!   a real client's own `SendTargets::Single`, or `SendTargets::SERVER_ONLY`
//!   (local echo) for the embedded player — NEVER `SendTargets::All`, so one
//!   player's private group/invite state can never leak to another connected
//!   client (the ledger's Part A2 leak, now closed for the true N-client case).
//! - [`mirror_dialogue`]: the NPC→player READ direction stays
//!   LISTEN-SERVER-ONLY BY DESIGN (see that system's own doc comment) — an NPC
//!   dialogue turn surfaces via the embedded `client::Client`'s own inbox,
//!   which only the listen server's one embedded player has.
//!   [`broadcast_captured_dialogue`] (BL-82 EM-8.3b) covers the real
//!   dedicated-server case via a NEW sim-side per-player outgoing-message
//!   capture hook (`server::msg_capture::OutgoingMessageCapture`, the SAME hook
//!   `crate::chat`/`crate::sfx`'s own captured-broadcast systems drain), so
//!   this is no longer an open gap — both functions run side by side, each a
//!   no-op on the shell it doesn't apply to.
//! - [`apply_group_action_requests`]/[`apply_dialogue_response_requests`]:
//!   BL-82 EM-8.3 UNIFIED onto the `FromClient` write path — they drain the
//!   real `bevy_replicon` [`xindeler_protocol::GroupActionRequest`]/
//!   [`xindeler_protocol::DialogueResponseRequest`] client messages (a
//!   genuinely-remote client's real send, OR the listen-server's own local
//!   write echoed back with `ClientId::Server`), resolve the acting sim entity
//!   per message via [`crate::inventory::resolve_client_entity`], and emit the
//!   EXACT sim events the legacy handlers do (`InitiateInviteEvent`/
//!   `InviteResponseEvent`/`GroupManipEvent`/`DialogueEvent`) — the isolation
//!   law's "writes go through the sim's public event/intent API" rule, never a
//!   direct sim-state write, and no longer coupled to `EmbeddedPlayer` (which a
//!   dedicated server lacks — the ledger's A1 parity gap).

use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        change_detection::{NonSend, NonSendMut},
        message::{MessageReader, MessageWriter},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Query, Res, ResMut},
    },
};
use bevy_replicon::prelude::{FromClient, SendTargets, ToClients};
use common::{
    comp,
    comp::invite::{InviteKind, InviteResponse},
    event::{DialogueEvent, GroupManipEvent, InitiateInviteEvent, InviteResponseEvent},
    uid::{IdMaps, Uid},
};
use specs::{Join, WorldExt};
use xindeler_protocol::{
    ActiveReplicaSessions, DialogueResponseRequest, GroupAction, GroupActionRequest, NetDialogue,
    NetGroupMember, NetGroupState, NetInviteKind, NetPendingInvite, NetPlayerList,
    NetPlayerListEntry,
};

use crate::{
    PlayerDimensionSession, SimServer,
    inventory::resolve_client_entity,
    player::{EmbeddedPlayer, player_sim_entity},
    tick_sim,
};

/// Mirrors `PRESENTED_INVITE_TIMEOUT_DUR` (`server/src/events/invite.rs`,
/// `= 30s`) exactly — duplicated rather than imported for the SAME reason
/// `xindeler_protocol::interest::CHUNK_FUZZ` duplicates
/// `server::presence::CHUNK_FUZZ`: that module is not `pub` outside the
/// `server` crate. If the real timeout is ever retuned, this constant must
/// be updated to match by hand.
const PRESENTED_INVITE_TIMEOUT_SECS: f32 = 30.0;

/// Flattens a sim entity's display name: `comp::Stats.name`
/// (`common_i18n::Content::hacky_descriptor` — the same "last-resort, not
/// pretty, but never panics" flattening every other name-carrying `Net*`
/// field in this crate would use) if the entity has one, else `fallback`
/// (used for a `comp::Player` with no `Stats` yet, mirroring
/// `xindeler_sim_bridge`'s general "degrade clean" posture).
fn flatten_name(stats: Option<&comp::Stats>, fallback: &str) -> String {
    stats.map_or_else(
        || fallback.to_owned(),
        |s| s.name.hacky_descriptor().to_owned(),
    )
}

/// Last-broadcast [`NetPlayerList`] contents, so re-broadcasting an
/// UNCHANGED list every tick doesn't force a wire send — the same dedup
/// posture `crate::combat_hud::CombatHudMirrorCache` uses for `NetCombo`/
/// `NetXp`/`NetBuffs`.
#[derive(Resource, Default, Debug)]
pub struct PlayerListCache(Vec<NetPlayerListEntry>);

/// Reads every currently-connected `comp::Player`-tagged sim entity and
/// broadcasts [`NetPlayerList`] whenever the (sorted, so ordering churn alone
/// never counts as a change) list differs from [`PlayerListCache`]. A no-op
/// (returns immediately) if no [`SimServer`] is booted yet — same early-out
/// every other mirror system in this crate uses.
pub fn mirror_player_list(
    sim: Option<NonSendMut<SimServer>>,
    mut cache: ResMut<PlayerListCache>,
    mut writer: MessageWriter<ToClients<NetPlayerList>>,
) {
    let Some(sim) = sim else { return };
    let ecs = sim.server.state().ecs();

    let entities = ecs.entities();
    let players = ecs.read_storage::<comp::Player>();
    let uids = ecs.read_storage::<Uid>();
    let stats = ecs.read_storage::<comp::Stats>();

    let mut entries: Vec<NetPlayerListEntry> = (&entities, &players, &uids)
        .join()
        .map(|(entity, player, uid)| NetPlayerListEntry {
            uid: uid.0.get(),
            name: flatten_name(stats.get(entity), &player.alias),
        })
        .collect();
    entries.sort_by_key(|e| e.uid);

    if cache.0 != entries {
        cache.0.clone_from(&entries);
        writer.write(ToClients {
            targets: SendTargets::All,
            message: NetPlayerList(entries),
        });
    }
}

/// Last-broadcast [`NetGroupState`] + per-inviter "first seen" instants (BL-82
/// EM-5.8 — the sim's own `comp::invite::Invite` component carries no
/// timestamp, only the INVITER's `PendingInvites` does; the bridge tracks its
/// own "first seen" the same way [`crate::combat_hud::CombatHudMirrorCache`]
/// tracks last-mirrored values, scoped to this module).
#[derive(Resource, Default, Debug)]
pub struct GroupStateCache {
    /// Last-sent [`NetGroupState`] PER recipient `Uid` (BL-82 EM-8.3 — was a
    /// single `Option` when this mirror only ever computed the ONE embedded
    /// player's group; now keyed by uid so each of N connected dedicated-server
    /// players dedups independently).
    last_sent: HashMap<Uid, NetGroupState>,
    invite_first_seen: HashMap<Uid, std::time::Instant>,
}

/// Resolves the [`SendTargets`] an EM-5.8 per-recipient mirror
/// ([`mirror_group_state`]/[`mirror_dialogue`]) should send to, given the
/// mirror's own subject `Uid` (BL-82 EM-8.2 — see this module's doc comment
/// for the full rationale). Prefers the REAL correlated client if
/// [`ActiveReplicaSessions`] has one; falls back to
/// `SendTargets::SERVER_ONLY` (`SendTargets::Single(ClientId::Server)`) —
/// NEVER `SendTargets::All` — which `bevy_replicon` only ever re-emits
/// LOCALLY (the listen-server's own local echo, `server/message.rs::
/// send_locally`), so a real second connected client can never receive
/// private state addressed to a DIFFERENT player this way.
fn resolve_recipient_targets(active: &ActiveReplicaSessions, recipient_uid: Uid) -> SendTargets {
    active
        .client_for_uid(recipient_uid.0.get())
        .map_or(SendTargets::SERVER_ONLY, SendTargets::Single)
}

/// Reads EVERY connected player's own group membership/leader/incoming invite
/// straight off the sim ([`comp::Group`], [`comp::group::GroupManager`],
/// [`comp::invite::Invite`]) and sends each one its OWN [`NetGroupState`]
/// (targeted, never broadcast) whenever it changes (BL-82 EM-8.3). No-ops until
/// a [`SimServer`] exists.
///
/// ## The recipient set (dedicated server + listen server, unified)
/// The players this shell has a UI recipient for are: every fully-logged-in
/// replicon client ([`ActiveReplicaSessions::iter`] — the dedicated server's N
/// real clients), UNIONED with the listen-server's single in-game embedded
/// local player (which never appears in that map — it authenticates over the
/// legacy loopback, not the replicon handshake, see
/// [`ActiveReplicaSessions`]'s own doc comment). Each recipient's `Uid` maps to
/// a [`SendTargets`] resolved via [`resolve_recipient_targets`]: a real
/// client's own `ClientId::Single`, or `SendTargets::SERVER_ONLY` (local echo)
/// for the embedded player — NEVER `SendTargets::All`, so one player's private
/// group/invite state can never leak to a different connected client (the
/// ledger's Part A2 leak this closes for the multi-client case).
pub fn mirror_group_state(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSend<EmbeddedPlayer>>,
    active: Res<ActiveReplicaSessions>,
    mut cache: ResMut<GroupStateCache>,
    mut writer: MessageWriter<ToClients<NetGroupState>>,
) {
    let Some(sim) = sim else { return };

    // Build the recipient set: every real replicon client, plus the embedded
    // local player (listen server) if it's in game. A `HashMap` dedups the
    // (impossible-today) case an embedded player's uid is ALSO a real session.
    let mut recipients: HashMap<Uid, SendTargets> = HashMap::new();
    for (uid, client_id) in active.iter() {
        if let Some(uid) = uid_from_u64(uid) {
            recipients.insert(uid, SendTargets::Single(client_id));
        }
    }
    if let Some(player) = player.as_deref()
        && player.is_in_game()
        && let Some(my_uid) = player.uid()
    {
        recipients
            .entry(my_uid)
            .or_insert_with(|| resolve_recipient_targets(&active, my_uid));
    }

    // Prune per-recipient dedup/timeout caches for identities that are gone
    // (a disconnected client, or the embedded player leaving game).
    cache
        .last_sent
        .retain(|uid, _| recipients.contains_key(uid));
    cache
        .invite_first_seen
        .retain(|uid, _| recipients.contains_key(uid));

    let ecs = sim.server.state().ecs();
    let id_maps = ecs.read_resource::<IdMaps>();
    let groups = ecs.read_storage::<comp::Group>();
    let group_manager = ecs.read_resource::<comp::group::GroupManager>();
    let alignments = ecs.read_storage::<comp::Alignment>();
    let uids = ecs.read_storage::<Uid>();
    let entities = ecs.entities();
    let stats = ecs.read_storage::<comp::Stats>();
    let invites = ecs.read_storage::<comp::invite::Invite>();

    for (my_uid, targets) in recipients {
        let Some(my_entity) = id_maps.uid_entity(my_uid) else {
            // This session's uid doesn't currently resolve to a live entity
            // (mid-login, or just disconnected) — skip it, keep the rest.
            continue;
        };

        let (group_name, leader, members) = match groups.get(my_entity).copied() {
            Some(group) => {
                let info = group_manager.group_info(group);
                let leader_uid = info
                    .and_then(|info| uids.get(info.leader))
                    .map(|u| u.0.get());
                let members: Vec<NetGroupMember> =
                    comp::group::members(group, &groups, &entities, &alignments, &uids)
                        .filter(|(_, role)| matches!(role, comp::group::Role::Member))
                        .filter_map(|(entity, _)| {
                            uids.get(entity).map(|uid| NetGroupMember {
                                uid: uid.0.get(),
                                name: flatten_name(stats.get(entity), "Unknown"),
                            })
                        })
                        .collect();
                (info.map(|i| i.name.clone()), leader_uid, members)
            },
            None => (None, None, Vec::new()),
        };

        let pending_invite = invites.get(my_entity).map(|invite| {
            let now = std::time::Instant::now();
            let first_seen = *cache.invite_first_seen.entry(my_uid).or_insert(now);
            let elapsed = now.saturating_duration_since(first_seen).as_secs_f32();
            let remaining_secs = (PRESENTED_INVITE_TIMEOUT_SECS - elapsed).max(0.0);
            let inviter_uid = uids.get(invite.inviter).map_or(0, |u| u.0.get());
            NetPendingInvite {
                inviter_uid,
                inviter_name: flatten_name(stats.get(invite.inviter), "Someone"),
                kind: match invite.kind {
                    InviteKind::Group => NetInviteKind::Group,
                    InviteKind::Trade => NetInviteKind::Trade,
                },
                remaining_secs,
            }
        });
        if pending_invite.is_none() {
            cache.invite_first_seen.remove(&my_uid);
        }

        let state = NetGroupState {
            group_name,
            leader,
            members,
            pending_invite,
        };

        if cache.last_sent.get(&my_uid) != Some(&state) {
            cache.last_sent.insert(my_uid, state.clone());
            writer.write(ToClients {
                targets,
                message: state,
            });
        }
    }
}

/// Drains [`EmbeddedPlayer::take_pending_dialogue`] and sends one
/// [`NetDialogue`] per NPC-initiated dialogue turn, addressed to the
/// RECIPIENT player (the embedded/local player this turn is FOR, not the NPC
/// speaker) — see this module's doc comment for how `targets` is resolved
/// (BL-82 EM-8.2) and why it degrades to `SendTargets::SERVER_ONLY` rather
/// than `SendTargets::All`. No-ops until both a [`SimServer`] (to resolve the
/// sender's display name) and an [`EmbeddedPlayer`] exist.
///
/// ## Listen-server-only by design (real remote clients: see
/// [`broadcast_captured_dialogue`] below)
/// The NPC→player READ direction here is scoped to the embedded local
/// player: an NPC-initiated dialogue turn surfaces via the embedded
/// `client::Client`'s `ClientEvent::Dialogue` (captured in
/// `crate::player::capture_social_events`) — a mechanism only the
/// listen-server's one embedded player has at all.
/// [`broadcast_captured_dialogue`] covers the real-remote-client case (BL-82
/// EM-8.3b) via the NEW sim-side capture hook, so this function's own scope is
/// unchanged and intentional, not a remaining gap. The player→NPC WRITE
/// direction ([`apply_dialogue_response_requests`]) IS fully generalized to
/// real remote clients below.
pub fn mirror_dialogue(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSendMut<EmbeddedPlayer>>,
    active: Res<ActiveReplicaSessions>,
    mut writer: MessageWriter<ToClients<NetDialogue>>,
) {
    let Some(sim) = sim else { return };
    let Some(mut player) = player else { return };
    let Some(recipient_uid) = player.uid() else {
        return;
    };

    let pending = player.take_pending_dialogue();
    if pending.is_empty() {
        return;
    }

    let ecs = sim.server.state().ecs();
    let stats = ecs.read_storage::<comp::Stats>();
    let targets = resolve_recipient_targets(&active, recipient_uid);

    for (sender_uid, dialogue) in pending {
        let sender_entity = player_sim_entity(&sim, sender_uid);
        let sender_name = flatten_name(sender_entity.and_then(|e| stats.get(e)), "Someone");
        writer.write(ToClients {
            targets,
            message: NetDialogue {
                sender_uid: sender_uid.0.get(),
                sender_name,
                dialogue,
            },
        });
    }
}

/// BL-82 EM-8.3b: the real-dedicated-server counterpart to [`mirror_dialogue`]
/// above. `server::events::interaction`'s `DialogueEvent` handler now ALSO
/// captures an NPC→player turn into `server::msg_capture::
/// OutgoingMessageCapture` (keyed by the RECIPIENT player's `Uid`, sender
/// carried alongside) for exactly the recipients that have no legacy
/// `comp::Client` — the case [`mirror_dialogue`] structurally cannot reach
/// (it only ever reads the embedded player's own inbox). This system drains
/// that buffer every tick and, for each captured turn, resolves the
/// recipient's real `ClientId` via [`ActiveReplicaSessions`] (dropping it,
/// same posture as [`crate::chat::broadcast_captured_chat`], if no session
/// currently correlates — no fallback target is guessed), flattens the
/// sender's display name the SAME way [`mirror_dialogue`] does, and sends
/// [`SendTargets::Single`] — never broadcast, so one player's NPC
/// conversation can never reach a different connected client. A no-op if no
/// [`SimServer`] exists yet; a harmless no-op on the listen server too (the
/// capture buffer stays empty there — the listen server's one player always
/// has a legacy `comp::Client`, see `msg_capture`'s own doc comment).
pub fn broadcast_captured_dialogue(
    sim: Option<NonSendMut<SimServer>>,
    active: Res<ActiveReplicaSessions>,
    mut writer: MessageWriter<ToClients<NetDialogue>>,
) {
    let Some(sim) = sim else { return };
    let captured = sim
        .server
        .state()
        .ecs()
        .write_resource::<server::msg_capture::OutgoingMessageCapture>()
        .drain_dialogue();
    if captured.is_empty() {
        return;
    }

    let ecs = sim.server.state().ecs();
    let stats = ecs.read_storage::<comp::Stats>();

    for server::msg_capture::CapturedDialogue {
        recipient,
        sender,
        dialogue,
    } in captured
    {
        let Some(client_id) = active.client_for_uid(recipient.0.get()) else {
            continue;
        };
        let sender_entity = player_sim_entity(&sim, sender);
        let sender_name = flatten_name(sender_entity.and_then(|e| stats.get(e)), "Someone");
        writer.write(ToClients {
            targets: SendTargets::Single(client_id),
            message: NetDialogue {
                sender_uid: sender.0.get(),
                sender_name,
                dialogue,
            },
        });
    }
}

/// Reconstructs a [`Uid`] from its wire `u64` inner value (the inverse of
/// `NetUid`'s own `u.0.get()` — see `xindeler_protocol::NetUid`'s doc
/// comment). `None` for `0` (never a legal `Uid` — `NonZeroU64`), matching
/// every other "malformed input degrades clean" posture in this crate.
fn uid_from_u64(value: u64) -> Option<Uid> { std::num::NonZeroU64::new(value).map(Uid::from) }

/// Drains [`GroupActionRequest`]s and re-emits each as the matching sim group
/// event (BL-82 EM-8.3 — the unified `FromClient` write path, replacing the
/// old `LocalGroupAction` + `EmbeddedPlayer` pass-through that silently did
/// nothing on the real dedicated server). Resolves the acting entity PER
/// MESSAGE via [`resolve_client_entity`] (real connection first via
/// [`PlayerDimensionSession`], embedded local player as the `ClientId::Server`
/// fallback — the SAME pattern [`crate::trade`]'s own invite applicators use),
/// then emits the EXACT event the sim's own message handlers emit:
/// `InitiateInviteEvent`/`InviteResponseEvent`/`GroupManipEvent`
/// (`server::events::invite`/`group_manip` — range/permission/leader checks
/// all stay server-side, so a non-leader's kick etc. is rejected sim-side,
/// never trusted here).
pub fn apply_group_action_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSend<EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<GroupActionRequest>>,
) {
    let Some(sim) = sim else {
        requests.clear();
        return;
    };
    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            continue;
        };
        let state = sim.server.state();
        match message.0 {
            GroupAction::Invite(uid) => {
                if let Some(uid) = uid_from_u64(uid) {
                    state.emit_event_now(InitiateInviteEvent(entity, uid, InviteKind::Group));
                }
            },
            GroupAction::AcceptInvite => {
                state.emit_event_now(InviteResponseEvent(entity, InviteResponse::Accept));
            },
            GroupAction::DeclineInvite => {
                state.emit_event_now(InviteResponseEvent(entity, InviteResponse::Decline));
            },
            GroupAction::Leave => {
                state.emit_event_now(GroupManipEvent(entity, comp::GroupManip::Leave));
            },
            GroupAction::Kick(uid) => {
                if let Some(uid) = uid_from_u64(uid) {
                    state.emit_event_now(GroupManipEvent(entity, comp::GroupManip::Kick(uid)));
                }
            },
            GroupAction::AssignLeader(uid) => {
                if let Some(uid) = uid_from_u64(uid) {
                    state.emit_event_now(GroupManipEvent(
                        entity,
                        comp::GroupManip::AssignLeader(uid),
                    ));
                }
            },
        }
    }
}

/// Drains [`DialogueResponseRequest`]s (the player→NPC reply) and re-emits each
/// as `common::event::DialogueEvent(sender, target, dialogue)` — the SAME event
/// `client::Client::perform_dialogue` ultimately drives on the sim
/// (`server::events::interaction`'s `DialogueEvent` handler validates it). The
/// unified `FromClient` write path (BL-82 EM-8.3), resolving the acting entity
/// PER MESSAGE like [`apply_group_action_requests`] above, and the NPC target
/// via the sim's own `IdMaps` (a stale/despawned target simply skips, never
/// panics).
pub fn apply_dialogue_response_requests(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSend<EmbeddedPlayer>>,
    sessions: Query<&PlayerDimensionSession>,
    mut requests: MessageReader<FromClient<DialogueResponseRequest>>,
) {
    let Some(sim) = sim else {
        requests.clear();
        return;
    };
    for FromClient { client_id, message } in requests.read() {
        let Some(entity) = resolve_client_entity(*client_id, &sim, player.as_deref(), &sessions)
        else {
            continue;
        };
        let Some(target_uid) = uid_from_u64(message.target_uid) else {
            continue;
        };
        let Some(target_entity) = player_sim_entity(&sim, target_uid) else {
            continue;
        };
        sim.server.state().emit_event_now(DialogueEvent(
            entity,
            target_entity,
            message.dialogue.clone(),
        ));
    }
}

/// Registers the whole EM-5.8 social/group/dialogue mirror + action-applying
/// systems in `FixedUpdate`, `.after(tick_sim)` (fresh sim state this tick,
/// matching `crate::combat_hud::CombatHudMirrorPlugin`'s own ordering). Add
/// AFTER [`crate::player::PlayerBridgePlugin`] (the group/dialogue/action
/// systems read/write [`EmbeddedPlayer`]).
pub struct SocialMirrorPlugin;

impl Plugin for SocialMirrorPlugin {
    fn build(&self, app: &mut App) {
        // `.chain()` (ecs-design-reviewer follow-up): every one of these
        // systems takes exclusive `NonSendMut<SimServer>` (and most also
        // `NonSend<EmbeddedPlayer>`), so Bevy would otherwise have to serialize
        // them on that shared access anyway — chaining makes the order explicit
        // and deterministic: apply this frame's player intent (group/dialogue
        // requests, via `emit_event_now` — processed by the NEXT `tick_sim`)
        // FIRST, then project the (necessarily one-tick-stale) sim state back
        // out, matching `PlayerBridgePlugin`'s own `tick_player`/
        // `mirror_local_player_prediction` chain.
        // BL-82 EM-8.2/8.3: `ActiveReplicaSessions` lives in `xindeler-protocol`
        // and is NOT auto-initialized by `XindelerProtocolPlugin` (see that
        // type's own doc comment) — `init_resource` is idempotent, so this is
        // safe alongside `xindeler-server-app`'s own explicit insert and
        // guarantees `mirror_group_state`/`mirror_dialogue`'s non-`Option`
        // `Res<ActiveReplicaSessions>` param never panics for want of the
        // resource existing, on ANY app this plugin is added to (listen server
        // AND, as of EM-8.3, the dedicated server).
        app.init_resource::<ActiveReplicaSessions>()
            .init_resource::<PlayerListCache>()
            .init_resource::<GroupStateCache>()
            .add_systems(
                FixedUpdate,
                (
                    apply_group_action_requests,
                    apply_dialogue_response_requests,
                    mirror_player_list,
                    mirror_group_state,
                    mirror_dialogue,
                    // BL-82 EM-8.3b: the real-dedicated-server counterpart to
                    // `mirror_dialogue` above (drains the NEW sim-side
                    // capture hook instead of `EmbeddedPlayer`'s inbox) —
                    // see its own doc comment.
                    broadcast_captured_dialogue,
                )
                    .chain()
                    .after(tick_sim),
            );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use common::comp::invite::InviteKind as SimInviteKind;
    use specs::Builder;

    use super::*;
    use crate::boot_test_server;

    /// These direct-system-call tests exercise the mirror logic without a
    /// real `bevy_replicon` server/client pair (the wire-level round-trip is
    /// `xindeler-protocol`'s own job, see `social::tests::
    /// net_player_list_replicates_to_every_client` et al. there) — but the
    /// `MessageWriter<ToClients<T>>` parameters still need `Messages<
    /// ToClients<T>>` registered as a plain Bevy message type, which
    /// `add_server_message` would otherwise do; a bare `add_message` is
    /// enough for that (`ToClients` itself carries no replicon-specific
    /// registration requirement beyond being a nameable, `Send + Sync`
    /// type).
    fn register_broadcast_messages(app: &mut App) {
        use bevy_replicon::prelude::ToClients;
        app.add_message::<ToClients<NetPlayerList>>();
        app.add_message::<ToClients<NetGroupState>>();
        app.add_message::<ToClients<NetDialogue>>();
    }

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_broadcast_messages(&mut app);
        app.init_resource::<ActiveReplicaSessions>();
        app.init_resource::<PlayerListCache>();
        app.init_resource::<GroupStateCache>();
        app.insert_non_send(sim);
        app
    }

    /// No [`SimServer`] yet: [`mirror_player_list`] is a harmless no-op.
    #[test]
    fn player_list_no_sim_is_a_harmless_no_op() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        register_broadcast_messages(&mut app);
        app.init_resource::<PlayerListCache>();
        app.world_mut()
            .run_system_once(mirror_player_list)
            .expect("system runs without a SimServer");
    }

    /// A sim entity carrying `Player`+`Uid` (+ `Stats` for the display name)
    /// is mirrored into a broadcast [`NetPlayerList`] entry.
    #[test]
    fn mirrors_connected_players_into_a_broadcast_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let uid = common::uid::Uid(std::num::NonZeroU64::new(11).unwrap());
            let body = comp::Body::Humanoid(comp::humanoid::Body::random());
            let mut stats = comp::Stats::empty(body);
            stats.name = common_i18n::Content::Plain("Hero".to_owned());
            ecs.create_entity()
                .with(comp::Player::new(
                    "hero_alias".to_owned(),
                    common::resources::BattleMode::PvE,
                    uuid::Uuid::nil(),
                    None,
                ))
                .with(uid)
                .with(stats)
                .build();
        }

        app.world_mut()
            .run_system_once(mirror_player_list)
            .expect("system runs");

        let received: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::prelude::Messages<
                bevy_replicon::prelude::ToClients<NetPlayerList>,
            >>()
            .drain()
            .collect();
        assert_eq!(received.len(), 1, "exactly one broadcast on first mirror");
        assert_eq!(received[0].message.0, vec![NetPlayerListEntry {
            uid: 11,
            name: "Hero".to_owned(),
        }]);
    }

    /// An unchanged player roster does NOT re-broadcast every tick.
    #[test]
    fn unchanged_player_list_does_not_rebroadcast() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(mirror_player_list)
            .expect("first run");
        app.world_mut()
            .resource_mut::<bevy::prelude::Messages<
                bevy_replicon::prelude::ToClients<NetPlayerList>,
            >>()
            .drain()
            .count();

        app.world_mut()
            .run_system_once(mirror_player_list)
            .expect("second run, nothing changed");
        let received: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::prelude::Messages<
                bevy_replicon::prelude::ToClients<NetPlayerList>,
            >>()
            .drain()
            .collect();
        assert!(
            received.is_empty(),
            "an empty-vs-empty unchanged list must not re-broadcast"
        );
    }

    /// No [`EmbeddedPlayer`]/[`SimServer`]: [`mirror_group_state`] is a
    /// harmless no-op.
    #[test]
    fn group_state_no_embedded_player_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.world_mut()
            .run_system_once(mirror_group_state)
            .expect("system runs without an EmbeddedPlayer");
    }

    /// BL-82 EM-8.2 regression guard: with no active replicon session for the
    /// recipient `Uid` (today's ALWAYS case for the embedded/local player —
    /// see this module's doc comment), [`resolve_recipient_targets`] must
    /// resolve to `SendTargets::SERVER_ONLY` — NEVER `SendTargets::All`. This
    /// is the actual fix for the ledger's Part A2 leak: `SERVER_ONLY` only
    /// ever re-emits locally, so a real second connected client can never
    /// receive it.
    #[test]
    fn resolve_recipient_targets_defaults_to_server_only_without_a_session() {
        use bevy_replicon::prelude::ClientId;

        let active = ActiveReplicaSessions::default();
        let recipient = Uid(std::num::NonZeroU64::new(7).unwrap());

        // `SendTargets` is not `PartialEq` (upstream bevy_replicon type), so
        // this asserts via pattern match rather than `assert_eq!`.
        assert!(matches!(
            resolve_recipient_targets(&active, recipient),
            SendTargets::Single(ClientId::Server)
        ));
    }

    /// BL-82 EM-8.2: once `ActiveReplicaSessions` DOES carry a real session
    /// for the recipient `Uid` (the forward-compatible case a future
    /// per-remote-player mirror, EM-8.3, would exercise), the resolved
    /// targets must be `SendTargets::Single` addressed to THAT exact client —
    /// never a broadcast, and never silently falling back to `SERVER_ONLY`
    /// once a real correlation exists.
    #[test]
    fn resolve_recipient_targets_prefers_the_correlated_client_when_present() {
        use bevy::ecs::entity::Entity;
        use bevy_replicon::prelude::ClientId;

        let mut active = ActiveReplicaSessions::default();
        let recipient = Uid(std::num::NonZeroU64::new(7).unwrap());
        let client = ClientId::Client(Entity::from_raw_u32(3).expect("valid entity index"));
        active.insert(recipient.0.get(), client);

        assert!(matches!(
            resolve_recipient_targets(&active, recipient),
            SendTargets::Single(resolved) if resolved == client
        ));
    }

    /// [`InviteKind`] round-trips through [`NetInviteKind`] (a cheap enum
    /// mapping regression guard).
    #[test]
    fn invite_kind_maps_both_variants() {
        assert_eq!(
            match SimInviteKind::Group {
                SimInviteKind::Group => NetInviteKind::Group,
                SimInviteKind::Trade => NetInviteKind::Trade,
            },
            NetInviteKind::Group
        );
        assert_eq!(
            match SimInviteKind::Trade {
                SimInviteKind::Group => NetInviteKind::Group,
                SimInviteKind::Trade => NetInviteKind::Trade,
            },
            NetInviteKind::Trade
        );
    }

    /// BL-82 EM-8.3 acceptance: [`mirror_group_state`], GENERALIZED beyond the
    /// single embedded player, sends EVERY fully-logged-in replicon client its
    /// OWN [`NetGroupState`] — targeted `SendTargets::Single` to that exact
    /// client, NEVER broadcast. This is the multi-client form of the ledger's
    /// Part A2 fix: with two connected players, each receives exactly one
    /// group-state message addressed only to itself, so one player's private
    /// group/invite state can never reach the other.
    #[test]
    fn mirror_group_state_targets_each_connected_player_privately() {
        use bevy::ecs::entity::Entity;
        use bevy_replicon::prelude::{ClientId, ToClients};
        use common::uid::IdMaps;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        // Two connected sim players, each with a `Uid` registered in `IdMaps`
        // the same way a real login path does.
        let mut spawn_player = || {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let entity = ecs.create_entity().build();
            let mut uids = ecs.write_storage::<Uid>();
            let mut id_maps = ecs.write_resource::<IdMaps>();
            let uid = id_maps.allocate(entity);
            uids.insert(entity, uid).unwrap();
            uid
        };
        let uid_a = spawn_player();
        let uid_b = spawn_player();

        // Each maps to a DISTINCT replicon client (the dedicated server's real
        // per-connection correlation, EM-8.2).
        let client_a = ClientId::Client(Entity::from_raw_u32(10).expect("valid index"));
        let client_b = ClientId::Client(Entity::from_raw_u32(20).expect("valid index"));
        {
            let mut active = app.world_mut().resource_mut::<ActiveReplicaSessions>();
            active.insert(uid_a.0.get(), client_a);
            active.insert(uid_b.0.get(), client_b);
        }

        app.world_mut()
            .run_system_once(mirror_group_state)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::prelude::Messages<ToClients<NetGroupState>>>()
            .drain()
            .collect();

        assert_eq!(
            sent.len(),
            2,
            "each of the two connected players gets its own group-state message"
        );
        // Every send is `Single` (never `All`), and each client is targeted
        // exactly once — the exact Part A2 no-leak guarantee for N clients.
        let mut hit_a = 0;
        let mut hit_b = 0;
        for msg in &sent {
            match msg.targets {
                SendTargets::Single(cid) if cid == client_a => hit_a += 1,
                SendTargets::Single(cid) if cid == client_b => hit_b += 1,
                SendTargets::Single(other) => {
                    panic!("group state targeted an unexpected client {other:?}")
                },
                _ => panic!(
                    "group state must be Single per-recipient, never broadcast (Part A2 leak)"
                ),
            }
        }
        assert_eq!(
            hit_a, 1,
            "client A must receive exactly its own group state"
        );
        assert_eq!(
            hit_b, 1,
            "client B must receive exactly its own group state"
        );
    }

    /// BL-82 EM-8.3 owner-attribution guard for the write side: client A's
    /// [`GroupActionRequest`] resolves to A's OWN sim entity as the acting
    /// entity — the analogue of `skillset`'s
    /// `skill_unlock_request_is_scoped_to_the_sender_not_another_client` for
    /// group actions. Reads the sim's real `EventBus<GroupManipEvent>`
    /// directly (`common::event::EventBus::recv_all`, the same API
    /// `server/src/cmd.rs` uses to inspect queued events) to prove the
    /// EMITTED event carries client A's entity, not client B's — never a
    /// misattributed action, which would let one client kick/leave/reassign
    /// leadership on behalf of a DIFFERENT player.
    #[test]
    fn group_action_request_resolves_to_the_senders_own_entity_not_another_clients() {
        use bevy_replicon::prelude::ClientId;
        use common::event::{EventBus, GroupManipEvent};

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());
        app.add_message::<FromClient<GroupActionRequest>>();

        let (entity_a, entity_b) = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            (ecs.create_entity().build(), ecs.create_entity().build())
        };
        let conn_a = app.world_mut().spawn(PlayerDimensionSession(entity_a)).id();
        // B has its own real connection but never sends a request this tick.
        let _conn_b = app.world_mut().spawn(PlayerDimensionSession(entity_b)).id();

        app.world_mut().write_message(FromClient {
            client_id: ClientId::Client(conn_a),
            message: GroupActionRequest(GroupAction::Leave),
        });
        app.world_mut()
            .run_system_once(apply_group_action_requests)
            .expect("applicator runs");

        let sim = app.world().non_send::<SimServer>();
        let ecs = sim.server.state().ecs();
        let events: Vec<_> = ecs
            .read_resource::<EventBus<GroupManipEvent>>()
            .recv_all()
            .collect();

        assert_eq!(
            events.len(),
            1,
            "exactly one GroupManipEvent must be queued for the one request sent"
        );
        let GroupManipEvent(acting_entity, manip) = &events[0];
        assert_eq!(
            *acting_entity, entity_a,
            "the emitted event's acting entity must be A (the real sender), never B (a different \
             connected client that sent nothing) — a misattribution here would let one client act \
             on another player's behalf"
        );
        assert_eq!(*manip, comp::GroupManip::Leave);
    }

    /// BL-82 EM-8.3b [`broadcast_captured_dialogue`]: a captured NPC→player
    /// dialogue turn relays ONLY to its own recipient's correlated
    /// `ClientId` — never a different connected client's, and never a
    /// captured turn addressed to an uncorrelated recipient. Same class of
    /// regression guard as `sfx::tests::
    /// captured_outcome_relays_only_to_its_own_correlated_client` — a
    /// dialogue turn is private (an NPC conversation), so misdelivering it
    /// to the wrong client would be a real leak, not just cosmetic.
    #[test]
    fn captured_dialogue_relays_only_to_its_own_correlated_recipient() {
        use bevy_replicon::prelude::ClientId;
        use common::rtsim::{Dialogue, DialogueId, DialogueKind};

        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let recipient_a = Uid(std::num::NonZeroU64::new(101).unwrap());
        let recipient_b = Uid(std::num::NonZeroU64::new(102).unwrap());
        let npc_sender = Uid(std::num::NonZeroU64::new(200).unwrap());
        let client_a = ClientId::Client(bevy::ecs::entity::Entity::from_raw_u32(50).unwrap());

        let dialogue = Dialogue::<true> {
            id: DialogueId(1),
            kind: DialogueKind::Start,
        };

        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut capture = ecs.write_resource::<server::msg_capture::OutgoingMessageCapture>();
            capture.capture_dialogue(recipient_a, npc_sender, dialogue.clone());
            // `recipient_b` has NO correlated session below — must be
            // dropped, never delivered to `client_a` by mistake.
            capture.capture_dialogue(recipient_b, npc_sender, dialogue.clone());
        }
        app.world_mut()
            .resource_mut::<ActiveReplicaSessions>()
            .insert(recipient_a.0.get(), client_a);

        app.world_mut()
            .run_system_once(broadcast_captured_dialogue)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::prelude::Messages<bevy_replicon::prelude::ToClients<NetDialogue>>>()
            .drain()
            .collect();

        assert_eq!(
            sent.len(),
            1,
            "only recipient_a (correlated) is relayed; recipient_b (uncorrelated) must be dropped"
        );
        assert!(
            matches!(sent[0].targets, SendTargets::Single(cid) if cid == client_a),
            "the relayed dialogue must target recipient_a's own client"
        );
        assert_eq!(sent[0].message.sender_uid, npc_sender.0.get());
        assert_eq!(sent[0].message.dialogue.kind, DialogueKind::Start);
    }
}
