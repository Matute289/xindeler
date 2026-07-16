//! BL-82 EM-5.8 — Social / group / dialogue HUD (spec
//! `2026-07-11-bl82-phase5-ui-audio-parity-design.md` §3.2/§6, task board
//! T56.28): the online **player list** (search is a follow-up — see this
//! module's own note) with invite, always-on **group/party frames** (member
//! health/energy, leader marker, kick/leave/assign-leader, invite accept/
//! decline banner), and a **v1-minimal NPC dialogue** window (message text +
//! response options / acknowledge).
//!
//! Builds on the EM-5.1 widget kit (`xindeler-ui`: panel/bar/button/tooltip)
//! and the EM-5.2 mirror pattern reference (`combat_hud.rs`): every widget
//! here reads REAL mirrored sim state
//! (`xindeler_protocol::{NetPlayerList,NetGroupState,NetDialogue}`,
//! projected by `xindeler_sim_bridge::social`) — no mocked data.
//!
//! ## Group/party member health+energy: reuse, don't duplicate
//! [`xindeler_protocol::NetGroupState`] carries only identity (uid + name);
//! a group member's live health/energy is read by correlating that uid
//! against any currently-mirrored entity's `xindeler_protocol::NetUid` —
//! exactly the "reuse the already-mirrored `NetHealth`" pattern EM-5.2's own
//! overhead health bars use. A member with no currently-mirrored entity
//! (out of interest range) shows "out of range" (matching legacy
//! `voxygen`'s own `hud-group-out_of_range` copy).
//!
//! ## Dialogue v1-minimal — what's real vs. deliberately deferred
//! [`NetDialogue`]/[`LocalDialogueResponse`] carry the REAL
//! `common::rtsim::Dialogue`/`DialogueKind`/`Response` shape (see
//! `xindeler_protocol::social`'s module doc for why) — this is genuine
//! server-authoritative dialogue, not a stub. What IS deliberately
//! v1-minimal:
//! - Message TEXT is flattened via `Content::hacky_descriptor()` (a
//!   last-resort, not-pretty-but-never-panics rendering) rather than full
//!   Fluent-arg-substituted localization — the real i18n depth is EM-5.16's
//!   job.
//! - The player-initiated "Start" trigger is a raw `KeyCode::KeyT` ("Talk")
//!   near the nearest mirrored entity, not a real keybind (EM-5.11 owns keybind
//!   infrastructure; this mirrors how `camera.rs`/`player_input.rs` already
//!   read raw `KeyCode`s directly today, pre-keybind-system).
//! - No dialogue-tree AUTHORING UI or quest-reward item icons — v1 shows plain
//!   text response buttons; `given_item`/reward display is a follow-up once
//!   AURORA (BL-83) actually drives interesting trees.
//!
//! Compiled only under the `listen-server`/`net-client` cargo features, same
//! gate as every other `Net*`-reading module in this crate.

use bevy::{ecs::schedule::common_conditions::not, prelude::*};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    GroupAction, LocalDialogueResponse, LocalGroupAction, NetDialogue, NetGroupState, NetHealth,
    NetLocalPlayer, NetPlayerList, NetUid,
};
use xindeler_ui::{
    bar::{BarValue, spawn_bar},
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    theme::{HudFonts, HudTheme},
};

use crate::chat::text_input_focused;

/// Installs the whole EM-5.8 social/group/dialogue HUD.
pub struct SocialHudViewPlugin;

impl Plugin for SocialHudViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveDialogue>()
            .init_resource::<CurrentGroupState>()
            .add_systems(
                Startup,
                spawn_social_hud.after(xindeler_ui::theme::init_theme),
            )
            .add_systems(
                Update,
                (
                    // Reads `ActionState` — must run after the frame's real
                    // input resolution (BL-82 EM-5.17 Phase 0, same fix as
                    // `diary::toggle_diary_window`/`controls_screen::
                    // toggle_controls_screen`). Also gated on
                    // `!text_input_focused` so typing "o" in the chat box
                    // doesn't ALSO open the Social window.
                    toggle_social_window
                        .after(xindeler_input::InputResolveSet)
                        .run_if(not(text_input_focused)),
                    sync_social_window_visibility,
                    sync_player_list,
                    sync_group_state,
                    sync_group_panel,
                    sync_invite_banner,
                    sync_active_dialogue,
                    sync_dialogue_panel,
                    handle_talk_key,
                ),
            );
    }
}

// ---------------------------------------------------------------------
// Player list (the toggled Social window)
// ---------------------------------------------------------------------

/// "Talk" key: starts/continues dialogue with the nearest mirrored entity. A
/// raw key, not a real keybind — see this module's doc comment (the Social
/// window's OWN toggle key was converted to the real, rebindable
/// [`GameInput::Social`] as part of BL-82 EM-5.17 Phase 0 — see
/// [`toggle_social_window`] — but `TALK_KEY` stays raw, out of scope for that
/// fix).
const TALK_KEY: KeyCode = KeyCode::KeyT;
/// Only entities within this many world units of the local player answer a
/// [`TALK_KEY`] press.
const TALK_RANGE: f32 = 8.0;

#[derive(Component)]
struct SocialWindowRoot;
#[derive(Component)]
struct PlayerListRoot;
#[derive(Component)]
struct PlayerListRow;
/// Tags a player-list row's Invite button with the row's uid.
#[derive(Component, Clone, Copy)]
struct InviteTarget(u64);

/// Toggles [`HudWindow::Social`] on [`GameInput::Social`] (`O` by default,
/// rebindable). BL-82 EM-5.17 Phase 0: this used to read the raw,
/// non-rebindable `ButtonInput<KeyCode>` with a hardcoded `KeyCode::KeyG` —
/// which is a genuine independent bug beyond just being non-rebindable: `G`
/// is actually bound to `GameInput::ToggleLantern` by default, not Social, so
/// this system was reading the WRONG key even before rebinding entered the
/// picture. `GameInput::Social`'s real default binding is `O`
/// (`xindeler_input::keybind`).
fn toggle_social_window(action_state: Res<ActionState>, mut actions: MessageWriter<HudAction>) {
    if action_state.just_pressed(GameInput::Social) {
        actions.write(HudAction::ToggleWindow(HudWindow::Social));
    }
}

fn sync_social_window_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<SocialWindowRoot>>,
) {
    if !hud_state.is_changed() {
        return;
    }
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = if hud_state.is_open(HudWindow::Social) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Rebuilds the player-list rows whenever a fresh [`NetPlayerList`] arrives.
/// Skips the local player's own row (never invite yourself) — resolved via
/// the local player's own mirrored [`NetUid`].
fn sync_player_list(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    mut list: MessageReader<NetPlayerList>,
    local_uid: Query<&NetUid, With<NetLocalPlayer>>,
    root: Query<(Entity, Option<&Children>), With<PlayerListRoot>>,
    rows: Query<Entity, With<PlayerListRow>>,
) {
    let Some(latest) = list.read().last() else {
        return;
    };
    let Ok((root_entity, children)) = root.single() else {
        return;
    };
    let my_uid = local_uid.single().ok().map(|u| u.0);

    if let Some(children) = children {
        for child in children.iter() {
            if rows.get(child).is_ok() {
                commands.entity(child).despawn();
            }
        }
    }

    commands.entity(root_entity).with_children(|parent| {
        for entry in &latest.0 {
            if Some(entry.uid) == my_uid {
                continue;
            }
            parent
                .spawn((PlayerListRow, Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: theme.spacing.sm_px(),
                    align_items: AlignItems::Center,
                    ..Default::default()
                }))
                .with_children(|row| {
                    row.spawn((
                        Text(entry.name.clone()),
                        TextFont {
                            font: bevy::text::FontSource::Handle(fonts.body.clone()),
                            font_size: bevy::text::FontSize::Px(16.0),
                            ..Default::default()
                        },
                        TextColor(theme.palette.text),
                    ));
                    row.spawn(button_bundle(&theme, &fonts, "Invite"))
                        .insert(InviteTarget(entry.uid))
                        .observe(
                            |activate: On<Activate>,
                             targets: Query<&InviteTarget>,
                             mut actions: MessageWriter<LocalGroupAction>| {
                                if let Ok(target) = targets.get(activate.entity) {
                                    actions.write(LocalGroupAction(GroupAction::Invite(
                                        target.0,
                                    )));
                                }
                            },
                        );
                });
        }
    });
}

// ---------------------------------------------------------------------
// Group / party panel (always visible while in a group)
// ---------------------------------------------------------------------

#[derive(Component)]
struct GroupPanelRoot;
#[derive(Component)]
struct GroupMembersRoot;
#[derive(Component)]
struct GroupMemberRow;
#[derive(Component)]
struct LeaveButton;
#[derive(Component, Clone, Copy)]
struct KickTarget(u64);
#[derive(Component, Clone, Copy)]
struct AssignLeaderTarget(u64);

#[derive(Component)]
struct InviteBannerRoot;
#[derive(Component)]
struct InviteBannerText;
#[derive(Component)]
struct AcceptInviteButton;
#[derive(Component)]
struct DeclineInviteButton;

/// The last-received [`NetGroupState`] — the single source
/// [`sync_group_panel`]/ [`sync_invite_banner`] both read, so a screen
/// resize/redraw never needs to re-derive it from a `MessageReader` a second
/// time.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
struct CurrentGroupState(NetGroupState);

fn sync_group_state(
    mut events: MessageReader<NetGroupState>,
    mut current: ResMut<CurrentGroupState>,
) {
    if let Some(latest) = events.read().last() {
        current.0 = latest.clone();
    }
}

/// Rebuilds the group member rows (name + health/energy bars, resolved by
/// correlating the member's uid against any currently-mirrored entity's
/// [`NetUid`] — "out of range" if none is mirrored right now) whenever
/// [`CurrentGroupState`] changes; hides the whole panel when not in a group.
#[allow(clippy::too_many_arguments)]
fn sync_group_panel(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    state: Res<CurrentGroupState>,
    mut panel_visibility: Query<&mut Visibility, With<GroupPanelRoot>>,
    root: Query<(Entity, Option<&Children>), With<GroupMembersRoot>>,
    rows: Query<Entity, With<GroupMemberRow>>,
    mirrored: Query<(&NetUid, Option<&NetHealth>)>,
    local_uid: Query<&NetUid, With<NetLocalPlayer>>,
) {
    if !state.is_changed() {
        return;
    }
    let my_uid = local_uid.single().ok().map(|u| u.0);

    let in_group = !state.0.members.is_empty();
    if let Ok(mut visibility) = panel_visibility.single_mut() {
        *visibility = if in_group {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if !in_group {
        return;
    }

    let Ok((root_entity, children)) = root.single() else {
        return;
    };
    if let Some(children) = children {
        for child in children.iter() {
            if rows.get(child).is_ok() {
                commands.entity(child).despawn();
            }
        }
    }

    // Built with plain top-level `commands` throughout (never nested inside
    // a `with_children` closure) because [`spawn_bar`] needs a real `&mut
    // Commands` to spawn its bar entity — a `ChildSpawner` closure parameter
    // doesn't expose one. Each row is spawned standalone then explicitly
    // parented via `add_child`.
    for member in &state.0.members {
        let is_leader = state.0.leader == Some(member.uid);
        let health = mirrored
            .iter()
            .find(|(uid, _)| uid.0 == member.uid)
            .and_then(|(_, health)| health);

        let row_entity = commands
            .spawn((GroupMemberRow, Node {
                flex_direction: FlexDirection::Row,
                column_gap: theme.spacing.sm_px(),
                align_items: AlignItems::Center,
                ..Default::default()
            }))
            .id();
        commands.entity(root_entity).add_child(row_entity);

        let label = if is_leader {
            format!("★ {}", member.name)
        } else {
            member.name.clone()
        };
        let name_entity = commands
            .spawn((
                Text(label),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(if is_leader {
                    theme.palette.accent
                } else {
                    theme.palette.text
                }),
            ))
            .id();
        commands.entity(row_entity).add_child(name_entity);

        match health {
            Some(health) => {
                let bar_entity = spawn_bar(
                    &mut commands,
                    &theme,
                    theme.palette.health,
                    theme.palette.health_bg,
                    80.0,
                    10.0,
                    BarValue::new(health.current, health.max),
                );
                commands.entity(row_entity).add_child(bar_entity);
            },
            None => {
                let out_of_range = commands
                    .spawn((
                        Text("(out of range)".to_owned()),
                        TextFont {
                            font: bevy::text::FontSource::Handle(fonts.body.clone()),
                            font_size: bevy::text::FontSize::Px(13.0),
                            ..Default::default()
                        },
                        TextColor(theme.palette.text_muted),
                    ))
                    .id();
                commands.entity(row_entity).add_child(out_of_range);
            },
        }

        // Never show Kick/Make-Leader on the local player's OWN row — the
        // server enforces leader-only permission regardless, but a
        // self-target button is confusing UX, not just a no-op
        // (bevy-migration-reviewer follow-up).
        if Some(member.uid) != my_uid {
            let kick_uid = member.uid;
            let kick_entity = commands
                .spawn(button_bundle(&theme, &fonts, "Kick"))
                .insert(KickTarget(kick_uid))
                .observe(
                    |activate: On<Activate>,
                     targets: Query<&KickTarget>,
                     mut actions: MessageWriter<LocalGroupAction>| {
                        if let Ok(target) = targets.get(activate.entity) {
                            actions.write(LocalGroupAction(GroupAction::Kick(target.0)));
                        }
                    },
                )
                .id();
            commands.entity(row_entity).add_child(kick_entity);

            let leader_uid = member.uid;
            let assign_entity = commands
                .spawn(button_bundle(&theme, &fonts, "Make Leader"))
                .insert(AssignLeaderTarget(leader_uid))
                .observe(
                    |activate: On<Activate>,
                     targets: Query<&AssignLeaderTarget>,
                     mut actions: MessageWriter<LocalGroupAction>| {
                        if let Ok(target) = targets.get(activate.entity) {
                            actions.write(LocalGroupAction(GroupAction::AssignLeader(target.0)));
                        }
                    },
                )
                .id();
            commands.entity(row_entity).add_child(assign_entity);
        }
    }
}

/// Shows/hides the incoming-invite accept/decline banner from
/// [`CurrentGroupState::pending_invite`].
fn sync_invite_banner(
    state: Res<CurrentGroupState>,
    mut banner_visibility: Query<&mut Visibility, With<InviteBannerRoot>>,
    mut text: Query<&mut Text, With<InviteBannerText>>,
) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut visibility) = banner_visibility.single_mut() else {
        return;
    };
    match &state.0.pending_invite {
        Some(invite) => {
            *visibility = Visibility::Visible;
            if let Ok(mut text) = text.single_mut() {
                text.0 = format!(
                    "{} invites you to their group ({:.0}s)",
                    invite.inviter_name, invite.remaining_secs
                );
            }
        },
        None => *visibility = Visibility::Hidden,
    }
}

// ---------------------------------------------------------------------
// Dialogue (v1-minimal)
// ---------------------------------------------------------------------

#[derive(Component)]
struct DialoguePanelRoot;
#[derive(Component)]
struct DialogueSenderText;
#[derive(Component)]
struct DialogueMessageText;
#[derive(Component)]
struct DialogueResponsesRoot;
#[derive(Component)]
struct DialogueResponseRow;

/// The dialogue currently on screen, if any (BL-82 EM-5.8). `None` = the
/// panel is closed. Set from every [`NetDialogue`] arrival; cleared on
/// `DialogueKind::End` or the panel's own close button.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
struct ActiveDialogue(Option<NetDialogue>);

fn sync_active_dialogue(
    mut events: MessageReader<NetDialogue>,
    mut active: ResMut<ActiveDialogue>,
) {
    if let Some(latest) = events.read().last() {
        active.0 = Some(latest.clone());
    }
}

/// Rebuilds the dialogue panel's text + response buttons whenever
/// [`ActiveDialogue`] changes; hides it (and auto-closes on
/// `DialogueKind::End`) otherwise.
fn sync_dialogue_panel(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    mut active: ResMut<ActiveDialogue>,
    mut panel_visibility: Query<&mut Visibility, With<DialoguePanelRoot>>,
    mut sender_text: Query<&mut Text, (With<DialogueSenderText>, Without<DialogueMessageText>)>,
    mut message_text: Query<&mut Text, (With<DialogueMessageText>, Without<DialogueSenderText>)>,
    root: Query<(Entity, Option<&Children>), With<DialogueResponsesRoot>>,
    rows: Query<Entity, With<DialogueResponseRow>>,
) {
    if !active.is_changed() {
        return;
    }

    use common::rtsim::DialogueKind;

    // `DialogueKind::End` auto-closes the panel — never leaves a stale
    // conversation on screen.
    if let Some(dialogue) = &active.0
        && matches!(dialogue.dialogue.kind, DialogueKind::End)
    {
        active.0 = None;
    }

    let Ok(mut visibility) = panel_visibility.single_mut() else {
        return;
    };
    let Some(dialogue) = &active.0 else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Visible;

    if let Ok(mut text) = sender_text.single_mut() {
        text.0 = dialogue.sender_name.clone();
    }
    let message = dialogue
        .dialogue
        .message()
        .map_or_else(String::new, |c| c.hacky_descriptor().to_owned());
    if let Ok(mut text) = message_text.single_mut() {
        text.0 = message;
    }

    let Ok((root_entity, children)) = root.single() else {
        return;
    };
    if let Some(children) = children {
        for child in children.iter() {
            if rows.get(child).is_ok() {
                commands.entity(child).despawn();
            }
        }
    }

    let dialogue_id = dialogue.dialogue.id;
    let sender_uid = dialogue.sender_uid;
    commands.entity(root_entity).with_children(|parent| {
        match &dialogue.dialogue.kind {
            DialogueKind::Question { tag, responses, .. } => {
                for (response_id, response) in responses {
                    let label = response.msg.hacky_descriptor().to_owned();
                    let tag = *tag;
                    let response_id = *response_id;
                    let response_content = response.clone();
                    parent
                        .spawn((DialogueResponseRow, button_bundle(&theme, &fonts, &label)))
                        .observe(
                            move |_activate: On<Activate>,
                                  mut actions: MessageWriter<LocalDialogueResponse>| {
                                actions.write(LocalDialogueResponse {
                                    target_uid: sender_uid,
                                    dialogue: common::rtsim::Dialogue {
                                        id: dialogue_id,
                                        kind: DialogueKind::Response {
                                            tag,
                                            response: response_content.clone(),
                                            response_id,
                                        },
                                    },
                                });
                            },
                        );
                }
            },
            DialogueKind::Statement { tag, .. } => {
                let tag = *tag;
                parent
                    .spawn((DialogueResponseRow, button_bundle(&theme, &fonts, "Continue")))
                    .observe(
                        move |_activate: On<Activate>,
                              mut actions: MessageWriter<LocalDialogueResponse>| {
                            actions.write(LocalDialogueResponse {
                                target_uid: sender_uid,
                                dialogue: common::rtsim::Dialogue {
                                    id: dialogue_id,
                                    kind: DialogueKind::Ack { tag },
                                },
                            });
                        },
                    );
            },
            // `Start`/`End`/`Marker`/`Ack`/`Response` carry no player-facing
            // choice — a plain Close button lets the player dismiss the
            // panel without sending anything further.
            _ => {
                parent
                    .spawn((DialogueResponseRow, button_bundle(&theme, &fonts, "Close")))
                    .observe(
                        |_activate: On<Activate>, mut active: ResMut<ActiveDialogue>| {
                            active.0 = None;
                        },
                    );
            },
        }
    });
}

/// Starts a fresh dialogue exchange with the nearest mirrored entity within
/// [`TALK_RANGE`] on [`TALK_KEY`] — the v1-minimal player-initiated trigger
/// (see this module's doc comment for why this isn't a real keybind yet).
/// A no-op if a dialogue is already active (avoids stomping the in-flight
/// exchange) or nothing is in range.
fn handle_talk_key(
    keys: Res<ButtonInput<KeyCode>>,
    active: Res<ActiveDialogue>,
    mut next_id: Local<u64>,
    local_player: Query<&GlobalTransform, With<NetLocalPlayer>>,
    others: Query<(&GlobalTransform, &NetUid), Without<NetLocalPlayer>>,
    mut actions: MessageWriter<LocalDialogueResponse>,
) {
    if !keys.just_pressed(TALK_KEY) || active.0.is_some() {
        return;
    }
    let Ok(my_transform) = local_player.single() else {
        return;
    };
    let my_pos = my_transform.translation();

    let nearest = others
        .iter()
        .map(|(transform, uid)| (transform.translation().distance(my_pos), uid.0))
        .filter(|(distance, _)| *distance <= TALK_RANGE)
        .min_by(|(a, _), (b, _)| a.total_cmp(b));

    let Some((_, target_uid)) = nearest else {
        return;
    };

    *next_id += 1;
    actions.write(LocalDialogueResponse {
        target_uid,
        dialogue: common::rtsim::Dialogue {
            id: common::rtsim::DialogueId(*next_id),
            kind: common::rtsim::DialogueKind::Start,
        },
    });
}

// ---------------------------------------------------------------------
// Startup: spawn every panel (hidden/empty until real data/state arrives)
// ---------------------------------------------------------------------

fn spawn_social_hud(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    // Social window: player list + search (search input is a follow-up —
    // v1 shows the full roster).
    commands
        .spawn((
            SocialWindowRoot,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(80.0),
                right: Val::Px(16.0),
                width: Val::Px(260.0),
                padding: UiRect::all(theme.spacing.md_px()),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.sm_px(),
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            bevy::ui::BorderColor::all(theme.palette.panel_border),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text("Online Players".to_owned()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.title.clone()),
                    font_size: bevy::text::FontSize::Px(20.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
            parent.spawn((PlayerListRoot, Node {
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.xs_px(),
                ..Default::default()
            }));
        });

    // Group/party panel: always visible while in a group (top-right, below
    // the Social window's anchor so the two never overlap).
    commands
        .spawn((GroupPanelRoot, Visibility::Hidden, Node {
            position_type: PositionType::Absolute,
            top: Val::Px(16.0),
            right: Val::Px(320.0),
            width: Val::Px(220.0),
            flex_direction: FlexDirection::Column,
            row_gap: theme.spacing.xs_px(),
            ..Default::default()
        }))
        .with_children(|parent| {
            parent
                .spawn(button_bundle(&theme, &fonts, "Leave Group"))
                .insert(LeaveButton)
                .observe(
                    |_activate: On<Activate>, mut actions: MessageWriter<LocalGroupAction>| {
                        actions.write(LocalGroupAction(GroupAction::Leave));
                    },
                );
            parent.spawn((GroupMembersRoot, Node {
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.xs_px(),
                ..Default::default()
            }));
        });

    // Incoming-invite banner (always visible when a pending invite exists).
    commands
        .spawn((
            InviteBannerRoot,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(16.0),
                left: Val::Percent(50.0),
                padding: UiRect::all(theme.spacing.md_px()),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.sm_px(),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            bevy::ui::BorderColor::all(theme.palette.panel_border),
        ))
        .with_children(|parent| {
            parent.spawn((
                InviteBannerText,
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
            parent
                .spawn((Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: theme.spacing.sm_px(),
                    ..Default::default()
                },))
                .with_children(|row| {
                    row.spawn(button_bundle(&theme, &fonts, "Accept"))
                        .insert(AcceptInviteButton)
                        .observe(
                            |_activate: On<Activate>,
                             mut actions: MessageWriter<LocalGroupAction>| {
                                actions.write(LocalGroupAction(GroupAction::AcceptInvite));
                            },
                        );
                    row.spawn(button_bundle(&theme, &fonts, "Decline"))
                        .insert(DeclineInviteButton)
                        .observe(
                            |_activate: On<Activate>,
                             mut actions: MessageWriter<LocalGroupAction>| {
                                actions.write(LocalGroupAction(GroupAction::DeclineInvite));
                            },
                        );
                });
        });

    // Dialogue panel (v1-minimal): sender name + message + response/ack/
    // close buttons.
    commands
        .spawn((
            DialoguePanelRoot,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(120.0),
                left: Val::Percent(50.0),
                width: Val::Px(420.0),
                padding: UiRect::all(theme.spacing.md_px()),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.sm_px(),
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            bevy::ui::BorderColor::all(theme.palette.panel_border),
        ))
        .with_children(|parent| {
            parent.spawn((
                DialogueSenderText,
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.title.clone()),
                    font_size: bevy::text::FontSize::Px(18.0),
                    ..Default::default()
                },
                TextColor(theme.palette.accent),
            ));
            parent.spawn((
                DialogueMessageText,
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
            parent.spawn((DialogueResponsesRoot, Node {
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.xs_px(),
                ..Default::default()
            }));
        });
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use common::rtsim::{Dialogue, DialogueId, DialogueKind, Response};
    use xindeler_protocol::{NetGroupMember, NetInviteKind, NetPendingInvite, NetPlayerListEntry};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.init_resource::<CurrentGroupState>();
        app.init_resource::<ActiveDialogue>();
        app
    }

    /// Toggling [`HudState`] into/out of [`HudWindow::Social`] shows/hides
    /// the Social window root — the T56.28 "player list toggle" acceptance
    /// bar.
    #[test]
    fn toggling_hud_state_shows_and_hides_the_social_window() {
        let mut app = new_app();
        let root = app
            .world_mut()
            .spawn((SocialWindowRoot, Visibility::Hidden))
            .id();
        let mut hud_state = HudState::default();
        hud_state.toggle(HudWindow::Social);
        app.insert_resource(hud_state);

        app.world_mut()
            .run_system_once(sync_social_window_visibility)
            .expect("system runs");
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Visible
        );

        let mut hud_state = *app.world().resource::<HudState>();
        hud_state.toggle(HudWindow::Social);
        app.insert_resource(hud_state);
        app.world_mut()
            .run_system_once(sync_social_window_visibility)
            .expect("system runs again");
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Hidden
        );
    }

    /// The player list renders one row per entry EXCEPT the local player's
    /// own uid (never show yourself an "Invite" row).
    #[test]
    fn player_list_skips_the_local_players_own_row() {
        let mut app = new_app();
        app.add_message::<NetPlayerList>();
        app.world_mut().insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        let root = app.world_mut().spawn(PlayerListRoot).id();
        app.world_mut().spawn((NetLocalPlayer, NetUid(1)));

        app.world_mut().write_message(NetPlayerList(vec![
            NetPlayerListEntry {
                uid: 1,
                name: "Me".to_owned(),
            },
            NetPlayerListEntry {
                uid: 2,
                name: "Other".to_owned(),
            },
        ]));
        app.world_mut()
            .run_system_once(sync_player_list)
            .expect("system runs");
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(root)
            .expect("rows were spawned")
            .iter()
            .collect();
        let row_count = children
            .iter()
            .filter(|c| app.world().get::<PlayerListRow>(**c).is_some())
            .count();
        assert_eq!(
            row_count, 1,
            "exactly one row (the OTHER player) — the local player's own uid must be skipped"
        );
    }

    /// A group member whose uid correlates to a currently-mirrored entity's
    /// [`NetUid`] gets a real health bar; a member with no mirrored entity
    /// gets an "(out of range)" label instead — never a stale/garbage bar.
    #[test]
    fn group_panel_shows_health_bar_when_mirrored_and_out_of_range_text_otherwise() {
        let mut app = new_app();
        app.world_mut().insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        let panel = app
            .world_mut()
            .spawn((GroupPanelRoot, Visibility::Hidden))
            .id();
        let root = app.world_mut().spawn(GroupMembersRoot).id();

        // Member 1 IS currently mirrored (has NetHealth); member 2 is not.
        app.world_mut().spawn((NetUid(1), NetHealth {
            current: 40.0,
            max: 100.0,
        }));

        app.world_mut().resource_mut::<CurrentGroupState>().0 = NetGroupState {
            group_name: Some("Party".to_owned()),
            leader: Some(1),
            members: vec![
                NetGroupMember {
                    uid: 1,
                    name: "Leader".to_owned(),
                },
                NetGroupMember {
                    uid: 2,
                    name: "Absent".to_owned(),
                },
            ],
            pending_invite: None,
        };

        app.world_mut()
            .run_system_once(sync_group_panel)
            .expect("system runs");
        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(panel).unwrap(),
            Visibility::Visible,
            "the panel must show once the player is in a group"
        );

        let member_rows: Vec<Entity> = app
            .world()
            .get::<Children>(root)
            .expect("member rows were spawned")
            .iter()
            .filter(|c| app.world().get::<GroupMemberRow>(*c).is_some())
            .collect();
        assert_eq!(member_rows.len(), 2, "one row per group member");

        // Exactly one row's children carry a real BarValue (the mirrored
        // member); exactly one carry the "(out of range)" text.
        let mut bar_rows = 0;
        let mut out_of_range_rows = 0;
        for row in &member_rows {
            let children = app.world().get::<Children>(*row).expect("row has children");
            for child in children.iter() {
                if app.world().get::<BarValue>(child).is_some() {
                    bar_rows += 1;
                }
                if let Some(text) = app.world().get::<Text>(child)
                    && text.0.contains("out of range")
                {
                    out_of_range_rows += 1;
                }
            }
        }
        assert_eq!(bar_rows, 1, "the mirrored member gets a real health bar");
        assert_eq!(
            out_of_range_rows, 1,
            "the non-mirrored member gets the out-of-range label"
        );
    }

    /// The invite banner shows the inviter's name/timeout and hides again
    /// once the invite clears.
    #[test]
    fn invite_banner_shows_text_and_hides_when_invite_clears() {
        let mut app = new_app();
        let banner = app
            .world_mut()
            .spawn((InviteBannerRoot, Visibility::Hidden))
            .id();
        let text_entity = app
            .world_mut()
            .spawn((InviteBannerText, Text(String::new())))
            .id();

        app.world_mut().resource_mut::<CurrentGroupState>().0 = NetGroupState {
            pending_invite: Some(NetPendingInvite {
                inviter_uid: 9,
                inviter_name: "Stranger".to_owned(),
                kind: NetInviteKind::Group,
                remaining_secs: 12.0,
            }),
            ..Default::default()
        };
        app.world_mut()
            .run_system_once(sync_invite_banner)
            .expect("system runs");
        assert_eq!(
            *app.world().get::<Visibility>(banner).unwrap(),
            Visibility::Visible
        );
        assert!(
            app.world()
                .get::<Text>(text_entity)
                .unwrap()
                .0
                .contains("Stranger")
        );

        app.world_mut().resource_mut::<CurrentGroupState>().0 = NetGroupState::default();
        app.world_mut()
            .run_system_once(sync_invite_banner)
            .expect("system runs again");
        assert_eq!(
            *app.world().get::<Visibility>(banner).unwrap(),
            Visibility::Hidden,
            "the banner must hide once the invite clears"
        );
    }

    /// A `DialogueKind::Question` renders one response button per option;
    /// `DialogueKind::End` auto-closes the panel — never leaves a stale
    /// conversation on screen.
    #[test]
    fn dialogue_panel_renders_question_responses_and_end_auto_closes() {
        let mut app = new_app();
        app.world_mut().insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        let panel = app
            .world_mut()
            .spawn((DialoguePanelRoot, Visibility::Hidden))
            .id();
        let sender_text = app
            .world_mut()
            .spawn((DialogueSenderText, Text(String::new())))
            .id();
        let message_text = app
            .world_mut()
            .spawn((DialogueMessageText, Text(String::new())))
            .id();
        let responses_root = app.world_mut().spawn(DialogueResponsesRoot).id();

        app.world_mut().resource_mut::<ActiveDialogue>().0 = Some(NetDialogue {
            sender_uid: 5,
            sender_name: "Quest Giver".to_owned(),
            dialogue: Dialogue {
                id: DialogueId(1),
                kind: DialogueKind::Question {
                    tag: 0,
                    msg: common_i18n::Content::Plain("Will you help?".to_owned()),
                    responses: vec![
                        (0, Response {
                            msg: common_i18n::Content::Plain("Yes".to_owned()),
                            given_item: None,
                        }),
                        (1, Response {
                            msg: common_i18n::Content::Plain("No".to_owned()),
                            given_item: None,
                        }),
                    ],
                },
            },
        });
        app.world_mut()
            .run_system_once(sync_dialogue_panel)
            .expect("system runs");
        app.update();

        assert_eq!(
            *app.world().get::<Visibility>(panel).unwrap(),
            Visibility::Visible
        );
        assert_eq!(
            app.world().get::<Text>(sender_text).unwrap().0,
            "Quest Giver"
        );
        assert_eq!(
            app.world().get::<Text>(message_text).unwrap().0,
            "Will you help?"
        );
        let response_rows: Vec<Entity> = app
            .world()
            .get::<Children>(responses_root)
            .expect("response buttons were spawned")
            .iter()
            .filter(|c| app.world().get::<DialogueResponseRow>(*c).is_some())
            .collect();
        assert_eq!(response_rows.len(), 2, "one button per response option");

        // Now the NPC ends the conversation — the panel must auto-close.
        app.world_mut().resource_mut::<ActiveDialogue>().0 = Some(NetDialogue {
            sender_uid: 5,
            sender_name: "Quest Giver".to_owned(),
            dialogue: Dialogue {
                id: DialogueId(1),
                kind: DialogueKind::End,
            },
        });
        app.world_mut()
            .run_system_once(sync_dialogue_panel)
            .expect("system runs again");
        assert_eq!(
            *app.world().get::<Visibility>(panel).unwrap(),
            Visibility::Hidden,
            "DialogueKind::End must auto-close the panel"
        );
        assert!(
            app.world().resource::<ActiveDialogue>().0.is_none(),
            "the active dialogue must be cleared, not just hidden"
        );
    }

    /// [`handle_talk_key`] targets the nearest mirrored entity within
    /// [`TALK_RANGE`] and ignores one that's further away.
    #[test]
    fn handle_talk_key_targets_the_nearest_entity_within_range() {
        let mut app = new_app();
        app.add_message::<LocalDialogueResponse>();
        app.world_mut().spawn((
            NetLocalPlayer,
            GlobalTransform::from_translation(Vec3::ZERO),
        ));
        // Near (within TALK_RANGE): should be targeted.
        app.world_mut().spawn((
            NetUid(11),
            GlobalTransform::from_translation(Vec3::new(2.0, 0.0, 0.0)),
        ));
        // Far (outside TALK_RANGE): must be ignored.
        app.world_mut().spawn((
            NetUid(22),
            GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
        ));

        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(TALK_KEY);
        app.insert_resource(keys);

        app.world_mut()
            .run_system_once(handle_talk_key)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalDialogueResponse>>()
            .drain()
            .collect();
        assert_eq!(sent.len(), 1, "exactly one dialogue Start is sent");
        assert_eq!(
            sent[0].target_uid, 11,
            "the NEAR entity is targeted, not the far one"
        );
        assert!(matches!(sent[0].dialogue.kind, DialogueKind::Start));
    }

    /// No entity within [`TALK_RANGE`]: [`handle_talk_key`] sends nothing.
    #[test]
    fn handle_talk_key_ignores_a_press_with_nothing_in_range() {
        let mut app = new_app();
        app.add_message::<LocalDialogueResponse>();
        app.world_mut().spawn((
            NetLocalPlayer,
            GlobalTransform::from_translation(Vec3::ZERO),
        ));
        app.world_mut().spawn((
            NetUid(99),
            GlobalTransform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
        ));

        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(TALK_KEY);
        app.insert_resource(keys);

        app.world_mut()
            .run_system_once(handle_talk_key)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalDialogueResponse>>()
            .drain()
            .collect();
        assert!(sent.is_empty(), "nothing in range must send nothing");
    }
}
