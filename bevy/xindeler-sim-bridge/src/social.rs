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
//!   any shell hosting the sim, not just the listen server).
//! - [`mirror_group_state`]/[`mirror_dialogue`]: read the SIM too, but scoped
//!   to the ONE player this process actually has a UI for — the embedded local
//!   player ([`EmbeddedPlayer::uid`]). A genuinely multi-remote-client
//!   dedicated server would need a connected-client↔sim-player-identity
//!   correlation to scope these per-recipient instead of broadcasting — BL-82
//!   EM-8.2 closes that gap (technical-debt ledger Part A2):
//!   [`xindeler_protocol::ActiveReplicaSessions`] now answers "which `ClientId`
//!   controls the sim player identified by this `Uid`", so both mirrors resolve
//!   their `SendTargets` from it instead of hardcoding `SendTargets::All`.
//!   Because these mirrors are STILL only ever computed for the ONE
//!   embedded/local player (this doc comment's scoping note above is otherwise
//!   unchanged — registering a per-remote-client version of this mirror on the
//!   dedicated server is EM-8.3's job, not this one's), the embedded player's
//!   own `Uid` is never actually a key in that map (the embedded player
//!   authenticates over the legacy loopback transport, never through the
//!   replicon login handshake that populates it) — so the resolved target
//!   degrades to `SendTargets::SERVER_ONLY`
//!   (`SendTargets::Single(ClientId::Server)`), which `bevy_replicon` only ever
//!   re-emits LOCALLY (the listen-server's own local echo), never to any real
//!   connected client. This is the exact fix the ledger's "every client would
//!   see every other player's private group/dialogue state" risk names: a real
//!   second replicon client connected alongside the embedded/local player can
//!   no longer receive the embedded player's own private group invite / NPC
//!   dialogue turn. If a FUTURE caller ever does populate an entry for the
//!   embedded player's own `Uid` (e.g. once EM-8.3 gives this mirror a real
//!   per-remote-player identity), the lookup correctly targets that one real
//!   client instead — the fallback is a safe default, not a hardcoded
//!   assumption.
//! - [`apply_local_group_actions`]/[`apply_local_dialogue_response`]: drain the
//!   client's [`xindeler_protocol::LocalGroupAction`]/
//!   [`xindeler_protocol::LocalDialogueResponse`] messages and call the new
//!   [`EmbeddedPlayer`] pass-through methods — a genuine client→server network
//!   round-trip over the embedded `Client`'s loopback socket (the isolation
//!   law's "writes go through the sim's public event/intent API" rule), never a
//!   direct sim-state write.

use std::collections::HashMap;

use bevy::{
    app::{App, FixedUpdate, Plugin},
    ecs::{
        change_detection::NonSendMut,
        message::{MessageReader, MessageWriter},
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Res, ResMut},
    },
};
use bevy_replicon::prelude::{SendTargets, ToClients};
use common::{comp, comp::invite::InviteKind, uid::Uid};
use specs::{Join, WorldExt};
use xindeler_protocol::{
    ActiveReplicaSessions, GroupAction, LocalDialogueResponse, LocalGroupAction, NetDialogue,
    NetGroupMember, NetGroupState, NetInviteKind, NetPendingInvite, NetPlayerList,
    NetPlayerListEntry,
};

use crate::{
    SimServer,
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
    last_sent: Option<NetGroupState>,
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

/// Reads the embedded local player's group membership/leader/incoming invite
/// straight off the sim ([`comp::Group`], [`comp::group::GroupManager`],
/// [`comp::invite::Invite`]) and sends [`NetGroupState`] whenever it changes.
/// No-ops until BOTH a [`SimServer`] and an in-game [`EmbeddedPlayer`] exist —
/// this mirror only has a point of view for the ONE player this process
/// actually hosts a UI for (see this module's doc comment, including the
/// BL-82 EM-8.2 note on how `targets` below is resolved and why it degrades
/// to `SendTargets::SERVER_ONLY` rather than `SendTargets::All`).
pub fn mirror_group_state(
    sim: Option<NonSendMut<SimServer>>,
    player: Option<NonSendMut<EmbeddedPlayer>>,
    active: Res<ActiveReplicaSessions>,
    mut cache: ResMut<GroupStateCache>,
    mut writer: MessageWriter<ToClients<NetGroupState>>,
) {
    let Some(sim) = sim else { return };
    let Some(player) = player else { return };
    if !player.is_in_game() {
        return;
    }
    let Some(my_uid) = player.uid() else { return };
    let Some(my_entity) = player_sim_entity(&sim, my_uid) else {
        return;
    };

    let ecs = sim.server.state().ecs();
    let groups = ecs.read_storage::<comp::Group>();
    let group_manager = ecs.read_resource::<comp::group::GroupManager>();
    let alignments = ecs.read_storage::<comp::Alignment>();
    let uids = ecs.read_storage::<Uid>();
    let entities = ecs.entities();
    let stats = ecs.read_storage::<comp::Stats>();
    let invites = ecs.read_storage::<comp::invite::Invite>();

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

    if cache.last_sent.as_ref() != Some(&state) {
        cache.last_sent = Some(state.clone());
        writer.write(ToClients {
            targets: resolve_recipient_targets(&active, my_uid),
            message: state,
        });
    }
}

/// Drains [`EmbeddedPlayer::take_pending_dialogue`] and sends one
/// [`NetDialogue`] per NPC-initiated dialogue turn, addressed to the
/// RECIPIENT player (the embedded/local player this turn is FOR, not the NPC
/// speaker) — see this module's doc comment for how `targets` is resolved
/// (BL-82 EM-8.2) and why it degrades to `SendTargets::SERVER_ONLY` rather
/// than `SendTargets::All`. No-ops until both a [`SimServer`] (to resolve the
/// sender's display name) and an [`EmbeddedPlayer`] exist.
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

/// Reconstructs a [`Uid`] from its wire `u64` inner value (the inverse of
/// `NetUid`'s own `u.0.get()` — see `xindeler_protocol::NetUid`'s doc
/// comment). `None` for `0` (never a legal `Uid` — `NonZeroU64`), matching
/// every other "malformed input degrades clean" posture in this crate.
fn uid_from_u64(value: u64) -> Option<Uid> { std::num::NonZeroU64::new(value).map(Uid::from) }

/// Drains [`LocalGroupAction`] (the listen-server's in-process client→bridge
/// handoff — see `xindeler_protocol::social`'s module doc comment) and calls
/// the matching [`EmbeddedPlayer`] pass-through method. A no-op (messages are
/// simply dropped, matching every other "degrade clean, no panic" mirror
/// system in this crate) until an [`EmbeddedPlayer`] exists.
pub fn apply_local_group_actions(
    player: Option<NonSendMut<EmbeddedPlayer>>,
    mut actions: MessageReader<LocalGroupAction>,
) {
    let Some(mut player) = player else {
        actions.clear();
        return;
    };
    for LocalGroupAction(action) in actions.read() {
        match *action {
            GroupAction::Invite(uid) => {
                if let Some(uid) = uid_from_u64(uid) {
                    player.send_group_invite(uid, InviteKind::Group);
                }
            },
            GroupAction::AcceptInvite => player.accept_invite(),
            GroupAction::DeclineInvite => player.decline_invite(),
            GroupAction::Leave => player.leave_group(),
            GroupAction::Kick(uid) => {
                if let Some(uid) = uid_from_u64(uid) {
                    player.kick_from_group(uid);
                }
            },
            GroupAction::AssignLeader(uid) => {
                if let Some(uid) = uid_from_u64(uid) {
                    player.assign_group_leader(uid);
                }
            },
        }
    }
}

/// Drains [`LocalDialogueResponse`] and forwards it to
/// [`EmbeddedPlayer::perform_dialogue`].
pub fn apply_local_dialogue_response(
    player: Option<NonSendMut<EmbeddedPlayer>>,
    mut responses: MessageReader<LocalDialogueResponse>,
) {
    let Some(mut player) = player else {
        responses.clear();
        return;
    };
    for response in responses.read() {
        if let Some(uid) = uid_from_u64(response.target_uid) {
            player.perform_dialogue(uid, response.dialogue.clone());
        }
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
        // `.chain()` (ecs-design-reviewer follow-up): four of these five
        // systems take `Option<NonSendMut<EmbeddedPlayer>>` — an undeclared
        // ambiguity is harmless here (an action applied this tick can't
        // affect the sim before a LATER tick anyway, since
        // `apply_local_group_actions`/`apply_local_dialogue_response` only
        // enqueue a real network send on the embedded `Client`, dispatched
        // by the NEXT `tick_player` call), but every other multi-system
        // group in this crate that shares exclusive access closes the
        // ordering explicitly (`PlayerBridgePlugin`'s own `tick_player`/
        // `mirror_local_player_prediction` chain) — this does the same, for
        // documentation/determinism: apply this frame's player intent
        // first, then project the (necessarily one-tick-stale) sim state
        // back out.
        // BL-82 EM-8.2: `ActiveReplicaSessions` lives in `xindeler-protocol`
        // and is NOT auto-initialized by `XindelerProtocolPlugin` (see that
        // type's own doc comment) — `init_resource` is idempotent, so this is
        // safe alongside `xindeler-server-app`'s own explicit insert and
        // guarantees `mirror_group_state`/`mirror_dialogue`'s non-`Option`
        // `Res<ActiveReplicaSessions>` param never panics for want of the
        // resource existing, on ANY app this plugin is added to (listen
        // server today; a dedicated server too, once EM-8.3 registers this
        // plugin there).
        app.init_resource::<ActiveReplicaSessions>()
            .init_resource::<PlayerListCache>()
            .init_resource::<GroupStateCache>()
            .add_systems(
                FixedUpdate,
                (
                    apply_local_group_actions,
                    apply_local_dialogue_response,
                    mirror_player_list,
                    mirror_group_state,
                    mirror_dialogue,
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
}
