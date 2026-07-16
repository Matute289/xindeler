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
//! a group member's live health/energy/level is read by correlating that uid
//! against any currently-mirrored entity's `xindeler_protocol::NetUid` —
//! exactly the "reuse the already-mirrored `NetHealth`" pattern EM-5.2's own
//! overhead health bars use. A member with no currently-mirrored entity
//! (out of interest range) shows "out of range" (matching legacy
//! `voxygen`'s own `hud-group-out_of_range` copy).
//!
//! ## BL-82 EM-5.17 Phase 4 — HUD-D4 party-frame reskin (spec §3.4)
//! The group/party panel's member rows are reskinned into the Diablo-style
//! layout: a 64×64 circular portrait (`HudImageKey::PartyPortraitFrame` over
//! a neutral placeholder fill — no real portrait render pipeline exists, per
//! spec §5), a `PartyLevelBadge` at the portrait's bottom edge, a name label,
//! DUAL horizontal bars, and a voice-chat icon. This is a RENDER-LAYER
//! reskin of [`SocialMirrorPlugin`]'s already-mirrored data — no new
//! protocol/mirror work. Three real data gaps, each documented at its call
//! site rather than silently invented:
//! - **Voice-chat state**: no protocol field carries a live per-member voice
//!   state — every row defaults to [`PartyVoiceState::Inactive`] (see that
//!   enum's own doc comment).
//! - **Portrait face**: a neutral flat-colour fill sits behind the frame PNG
//!   (spec §5's documented placeholder-until-render-to-texture-exists).
//! - **Second ("dual") bar**: the mirrored `NetXp`/`NetHealth`/[`NetEnergy`]
//!   trio is already correlated by uid the same way health is — the second bar
//!   reads `NetEnergy` (the mana/energy resource, matching Phase 2's own
//!   health→angel/poise→stamina/energy→mana orb mapping), not an invented
//!   field.
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

use bevy::{
    ecs::schedule::common_conditions::not,
    picking::Pickable,
    prelude::*,
    ui::{GlobalZIndex, widget::ImageNode},
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    GroupAction, LocalDialogueResponse, LocalGroupAction, NetDialogue, NetEnergy, NetGroupState,
    NetHealth, NetLocalPlayer, NetPlayerList, NetUid, NetXp,
};
use xindeler_ui::{
    bar::{BarValue, spawn_bar},
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    images::{HudImageKey, HudImages},
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::chat::text_input_focused;

/// BL-82 EM-5.17 Phase 4 — a party portrait's fixed footprint (spec §3.4:
/// "a ~64×64 circular portrait").
const PARTY_PORTRAIT_SIZE_PX: f32 = 64.0;
/// The level badge sits AT the portrait's bottom edge, slightly overlapping
/// it (spec §3.4) — half the badge's own size, so it visually straddles the
/// portrait's bottom rim rather than floating below or fully inside it.
const PARTY_LEVEL_BADGE_SIZE_PX: f32 = 24.0;
/// The voice-chat icon size — small, sitting inline next to the name label.
const PARTY_VOICE_ICON_SIZE_PX: f32 = 16.0;
/// Each dual bar's footprint (spec §3.4's "a horizontal health bar to its
/// right" — extended to two, per the "Barras Duales" follow-up).
const PARTY_BAR_WIDTH_PX: f32 = 120.0;
const PARTY_BAR_HEIGHT_PX: f32 = 10.0;

/// A neutral placeholder fill behind the portrait frame — spec §5: "Real
/// rendered portraits (party/self/boss) — placeholder art until a
/// render-to-texture portrait pipeline exists." No per-character face
/// texture exists yet, so every portrait shows this same flat tone; a real
/// portrait pipeline is a documented follow-up, not silently invented here.
const PARTY_PORTRAIT_PLACEHOLDER_FILL: Color = Color::srgba(0.30, 0.28, 0.26, 1.0);

/// BL-82 EM-5.17 Phase 4 — the three voice-chat icon states (spec §3.4).
///
/// **Data gap**: no protocol field (`NetGroupMember` carries only `uid`/
/// `name`) tells the client a member's live voice-chat state — until one
/// exists, [`sync_group_panel`] always passes [`Self::Inactive`] for every
/// row. This enum + [`voice_icon_key`] exist so the STATE→ASSET mapping
/// itself is real and independently tested, even though the input is
/// currently a constant rather than real telemetry.
///
/// `Active`/`Muted` are only ever constructed by this module's own
/// `voice_icon_key_maps_every_state_to_its_own_asset` test today (no
/// production call site passes them yet, per the data-gap note above) —
/// `#[allow(dead_code)]` documents that as deliberate, not an oversight, so a
/// non-`--all-targets` clippy run doesn't flag a real, tested mapping as
/// unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum PartyVoiceState {
    Active,
    Inactive,
    Muted,
}

/// Maps a [`PartyVoiceState`] to the [`HudImageKey`] asset that represents it
/// (spec §3.4's `party_voice_active/inactive/muted.png` trio).
fn voice_icon_key(state: PartyVoiceState) -> HudImageKey {
    match state {
        PartyVoiceState::Active => HudImageKey::PartyVoiceActive,
        PartyVoiceState::Inactive => HudImageKey::PartyVoiceInactive,
        PartyVoiceState::Muted => HudImageKey::PartyVoiceMuted,
    }
}

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

/// BL-82 EM-5.17 Phase 4 — marks a party row's portrait-frame `ImageNode`
/// child (`HudImageKey::PartyPortraitFrame`), so a test/query can find it
/// without hunting through the whole subtree by `ImageNode` alone (several
/// sibling nodes in a row also carry an `ImageNode` — the level badge, the
/// voice icon).
#[derive(Component)]
struct PartyPortraitFrameImage;
/// Marks a party row's level-badge `ImageNode` child
/// (`HudImageKey::PartyLevelBadge`) — see [`PartyPortraitFrameImage`]'s doc
/// comment for why a dedicated marker beats a bare `ImageNode` query.
#[derive(Component)]
struct PartyLevelBadgeImage;
/// Marks a party row's voice-chat icon `ImageNode` child — see
/// [`PartyPortraitFrameImage`]'s doc comment.
#[derive(Component)]
struct PartyVoiceIconImage;
/// Marks a party row's name `Text` label.
#[derive(Component)]
struct PartyNameLabel;

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

/// Rebuilds the group member rows into the HUD-D4 party-frame layout (spec
/// §3.4, BL-82 EM-5.17 Phase 4): a circular portrait (frame over a
/// placeholder fill) with a level badge, a name label + voice-chat icon, and
/// DUAL horizontal bars (health + energy), all resolved by correlating the
/// member's uid against any currently-mirrored entity's [`NetUid`] — "out of
/// range" if none is mirrored right now, matching the pre-reskin behaviour.
/// Runs whenever [`CurrentGroupState`] changes; hides the whole panel when
/// not in a group.
#[allow(clippy::too_many_arguments)]
fn sync_group_panel(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
    state: Res<CurrentGroupState>,
    mut panel_visibility: Query<&mut Visibility, With<GroupPanelRoot>>,
    root: Query<(Entity, Option<&Children>), With<GroupMembersRoot>>,
    rows: Query<Entity, With<GroupMemberRow>>,
    mirrored: Query<(
        &NetUid,
        Option<&NetHealth>,
        Option<&NetEnergy>,
        Option<&NetXp>,
    )>,
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
    // doesn't expose one. Each node is spawned standalone then explicitly
    // parented via `add_child`.
    for member in &state.0.members {
        let is_leader = state.0.leader == Some(member.uid);
        let mirrored_entry = mirrored.iter().find(|(uid, ..)| uid.0 == member.uid);
        let health = mirrored_entry.and_then(|(_, health, _, _)| health);
        let energy = mirrored_entry.and_then(|(_, _, energy, _)| energy);
        let level = mirrored_entry
            .and_then(|(_, _, _, xp)| xp)
            .map(|xp| xp.level);

        let row_entity = commands
            .spawn((GroupMemberRow, Node {
                flex_direction: FlexDirection::Row,
                column_gap: theme.spacing.sm_px(),
                align_items: AlignItems::Center,
                ..Default::default()
            }))
            .id();
        commands.entity(root_entity).add_child(row_entity);

        // --- Portrait stack (spec §3.4: 64×64 circular portrait + level
        // badge at its bottom edge) ---
        let portrait_entity = commands
            .spawn(Node {
                position_type: PositionType::Relative,
                width: Val::Px(PARTY_PORTRAIT_SIZE_PX),
                height: Val::Px(PARTY_PORTRAIT_SIZE_PX),
                flex_shrink: 0.0,
                ..Default::default()
            })
            .id();
        commands.entity(row_entity).add_child(portrait_entity);

        let placeholder_fill = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    border_radius: BorderRadius::all(Val::Px(PARTY_PORTRAIT_SIZE_PX / 2.0)),
                    ..Default::default()
                },
                BackgroundColor(PARTY_PORTRAIT_PLACEHOLDER_FILL),
            ))
            .id();
        commands.entity(portrait_entity).add_child(placeholder_fill);

        let frame_entity = commands
            .spawn((
                PartyPortraitFrameImage,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                ImageNode::new(images.get(HudImageKey::PartyPortraitFrame)),
                // Pure decoration over the placeholder fill — must never
                // intercept pointer events (matches `spawn_orb_bar`'s own
                // frame-overlay convention).
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(portrait_entity).add_child(frame_entity);

        let badge_entity = commands
            .spawn((
                PartyLevelBadgeImage,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(-(PARTY_LEVEL_BADGE_SIZE_PX / 2.0)),
                    left: Val::Percent(50.0),
                    margin: UiRect::left(Val::Px(-(PARTY_LEVEL_BADGE_SIZE_PX / 2.0))),
                    width: Val::Px(PARTY_LEVEL_BADGE_SIZE_PX),
                    height: Val::Px(PARTY_LEVEL_BADGE_SIZE_PX),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..Default::default()
                },
                ImageNode::new(images.get(HudImageKey::PartyLevelBadge)),
            ))
            .id();
        commands.entity(portrait_entity).add_child(badge_entity);

        // The badge PNG is the decorative frame; the level NUMBER only
        // renders when the member is currently mirrored (real data, never a
        // guessed/default level).
        if let Some(level) = level {
            let level_text = commands
                .spawn((
                    Text(format!("{level}")),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(11.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ))
                .id();
            commands.entity(badge_entity).add_child(level_text);
        }

        // --- Info column: name + voice icon, then dual bars ---
        let info_entity = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: theme.spacing.xs_px(),
                ..Default::default()
            })
            .id();
        commands.entity(row_entity).add_child(info_entity);

        let name_row_entity = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: theme.spacing.xs_px(),
                align_items: AlignItems::Center,
                ..Default::default()
            })
            .id();
        commands.entity(info_entity).add_child(name_row_entity);

        let label = if is_leader {
            format!("★ {}", member.name)
        } else {
            member.name.clone()
        };
        let name_entity = commands
            .spawn((
                PartyNameLabel,
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
        commands.entity(name_row_entity).add_child(name_entity);

        // Voice-chat state has no mirrored data source yet (see
        // `PartyVoiceState`'s own doc comment) — always Inactive until a
        // real field lands.
        let voice_icon_entity = commands
            .spawn((
                PartyVoiceIconImage,
                Node {
                    width: Val::Px(PARTY_VOICE_ICON_SIZE_PX),
                    height: Val::Px(PARTY_VOICE_ICON_SIZE_PX),
                    ..Default::default()
                },
                ImageNode::new(images.get(voice_icon_key(PartyVoiceState::Inactive))),
            ))
            .id();
        commands
            .entity(name_row_entity)
            .add_child(voice_icon_entity);

        let bars_row_entity = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: theme.spacing.xs_px(),
                ..Default::default()
            })
            .id();
        commands.entity(info_entity).add_child(bars_row_entity);

        match mirrored_entry {
            Some(_) => {
                if let Some(health) = health {
                    let bar_entity = spawn_bar(
                        &mut commands,
                        &theme,
                        theme.palette.health,
                        theme.palette.health_bg,
                        PARTY_BAR_WIDTH_PX,
                        PARTY_BAR_HEIGHT_PX,
                        BarValue::new(health.current, health.max),
                    );
                    commands.entity(bars_row_entity).add_child(bar_entity);
                }
                if let Some(energy) = energy {
                    let bar_entity = spawn_bar(
                        &mut commands,
                        &theme,
                        theme.palette.energy,
                        theme.palette.energy_bg,
                        PARTY_BAR_WIDTH_PX,
                        PARTY_BAR_HEIGHT_PX,
                        BarValue::new(energy.current, energy.max),
                    );
                    commands.entity(bars_row_entity).add_child(bar_entity);
                }
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
                commands.entity(bars_row_entity).add_child(out_of_range);
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

    // Group/party panel: always visible while in a group. BL-82 EM-5.17
    // Phase 4: repositioned to spec §3.4's confirmed anchor (top-left column,
    // `top:100,left:20`, `row_gap:20` between member rows) — was top-right,
    // which the pre-reskin flat layout used purely to avoid overlapping the
    // Social window (now at top-right still, so the two remain disjoint).
    // `GlobalZIndex` per spec §4.4: party frames share the ambient always-on
    // HUD chrome layer with the orbs/action-bar/minimap.
    commands
        .spawn((
            GroupPanelRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::ORBS_ACTION_BAR_PARTY_MINIMAP),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(100.0),
                left: Val::Px(20.0),
                width: Val::Px(280.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(20.0),
                ..Default::default()
            },
        ))
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
                row_gap: Val::Px(20.0),
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
    use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce};
    use common::rtsim::{Dialogue, DialogueId, DialogueKind, Response};
    use xindeler_protocol::{NetGroupMember, NetInviteKind, NetPendingInvite, NetPlayerListEntry};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // BL-82 EM-5.17 Phase 4: `sync_group_panel` now reads `Res<HudImages>`
        // — `AssetPlugin` gives us a real `AssetServer` to build one via
        // `HudImages::load` (mirroring `far_terrain.rs`/`sprite_view.rs`'s
        // own established "AssetPlugin::default() for a real AssetServer in
        // a headless test" convention); no other test in this module needs
        // it, but adding it unconditionally here is harmless.
        app.add_plugins(AssetPlugin::default());
        // `AssetServer::load::<Image>` panics unless the `Image` asset type
        // is registered — normally done by `ImagePlugin` (part of
        // `DefaultPlugins`, unavailable headlessly since it pulls in the
        // render app). `init_asset` alone is enough for a headless test that
        // only needs `Handle<Image>` allocation, not real decoding (same
        // minimal-registration precedent as `orb_material.rs`'s own
        // `app.init_asset::<OrbLiquidMaterial>()`).
        app.init_asset::<Image>();
        app.insert_resource(HudTheme::default());
        app.init_resource::<CurrentGroupState>();
        app.init_resource::<ActiveDialogue>();
        app
    }

    /// Builds a real (test) [`HudImages`] via the app's [`AssetServer`] —
    /// `HudImages`'s fields are private to `xindeler_ui::images`, so
    /// `HudImages::load` (the only public constructor) is the one way a
    /// downstream crate's test can get one.
    fn insert_hud_images(app: &mut App) {
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
    }

    /// Collects every descendant (children, grandchildren, …) of `root` —
    /// BL-82 EM-5.17 Phase 4's party row nests its visuals (portrait stack /
    /// info column / name row / bars row) several levels deep, unlike the
    /// pre-reskin flat layout where a row's bar/text children were direct
    /// children of the row itself.
    fn all_descendants(app: &App, root: Entity) -> Vec<Entity> {
        let mut result = Vec::new();
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if let Some(children) = app.world().get::<Children>(entity) {
                for child in children.iter() {
                    result.push(child);
                    stack.push(child);
                }
            }
        }
        result
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
    /// [`NetUid`] gets real DUAL health+energy bars (BL-82 EM-5.17 Phase 4's
    /// "Barras Duales"); a member with no mirrored entity gets an "(out of
    /// range)" label instead — never a stale/garbage bar. Bars now nest
    /// several levels below the row (portrait/info-column/bars-row), so this
    /// searches the whole subtree, not just direct children.
    #[test]
    fn group_panel_shows_dual_bars_when_mirrored_and_out_of_range_text_otherwise() {
        let mut app = new_app();
        insert_hud_images(&mut app);
        app.world_mut().insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        let panel = app
            .world_mut()
            .spawn((GroupPanelRoot, Visibility::Hidden))
            .id();
        let root = app.world_mut().spawn(GroupMembersRoot).id();

        // Member 1 IS currently mirrored (has NetHealth + NetEnergy); member
        // 2 is not.
        app.world_mut().spawn((
            NetUid(1),
            NetHealth {
                current: 40.0,
                max: 100.0,
            },
            NetEnergy {
                current: 20.0,
                max: 50.0,
            },
        ));

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

        // Exactly one row's subtree carries two real BarValue entities (the
        // mirrored member's dual health+energy bars); exactly one carries
        // the "(out of range)" text and no bars at all.
        let mut bar_rows_with_two_bars = 0;
        let mut out_of_range_rows = 0;
        for row in &member_rows {
            let descendants = all_descendants(&app, *row);
            let bar_count = descendants
                .iter()
                .filter(|&&e| app.world().get::<BarValue>(e).is_some())
                .count();
            if bar_count == 2 {
                bar_rows_with_two_bars += 1;
            }
            let has_out_of_range = descendants.iter().any(|&e| {
                app.world()
                    .get::<Text>(e)
                    .is_some_and(|text| text.0.contains("out of range"))
            });
            if has_out_of_range {
                out_of_range_rows += 1;
            }
        }
        assert_eq!(
            bar_rows_with_two_bars, 1,
            "the mirrored member gets real dual health+energy bars"
        );
        assert_eq!(
            out_of_range_rows, 1,
            "the non-mirrored member gets the out-of-range label"
        );
    }

    /// BL-82 EM-5.17 Phase 4's core acceptance bar (spec §3.4): given a fully
    /// mirrored party member, the rebuilt row carries a portrait-frame
    /// `ImageNode` keyed to `HudImageKey::PartyPortraitFrame`, a level badge
    /// showing the member's real mirrored level, the name label, and TWO
    /// bar entities (health + energy — "Barras Duales").
    #[test]
    fn party_row_renders_portrait_frame_level_badge_name_and_dual_bars() {
        let mut app = new_app();
        insert_hud_images(&mut app);
        app.world_mut().insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.world_mut().spawn((GroupPanelRoot, Visibility::Hidden));
        let root = app.world_mut().spawn(GroupMembersRoot).id();

        app.world_mut().spawn((
            NetUid(7),
            NetHealth {
                current: 80.0,
                max: 100.0,
            },
            NetEnergy {
                current: 30.0,
                max: 60.0,
            },
            NetXp {
                level: 12,
                xp_into_level: 0,
                xp_for_level: 100,
            },
        ));

        app.world_mut().resource_mut::<CurrentGroupState>().0 = NetGroupState {
            group_name: Some("Party".to_owned()),
            leader: Some(7),
            members: vec![NetGroupMember {
                uid: 7,
                name: "Ally".to_owned(),
            }],
            pending_invite: None,
        };

        app.world_mut()
            .run_system_once(sync_group_panel)
            .expect("system runs");
        app.update();

        let member_row = app
            .world()
            .get::<Children>(root)
            .expect("member row spawned")
            .iter()
            .find(|c| app.world().get::<GroupMemberRow>(*c).is_some())
            .expect("member row exists");
        let descendants = all_descendants(&app, member_row);

        let images = app.world().resource::<HudImages>().clone();

        let frame_entity = descendants
            .iter()
            .copied()
            .find(|&e| app.world().get::<PartyPortraitFrameImage>(e).is_some())
            .expect("portrait frame image exists");
        assert_eq!(
            app.world().get::<ImageNode>(frame_entity).unwrap().image,
            images.get(HudImageKey::PartyPortraitFrame),
            "the portrait frame ImageNode uses the correct HudImageKey"
        );

        let badge_entity = descendants
            .iter()
            .copied()
            .find(|&e| app.world().get::<PartyLevelBadgeImage>(e).is_some())
            .expect("level badge image exists");
        assert_eq!(
            app.world().get::<ImageNode>(badge_entity).unwrap().image,
            images.get(HudImageKey::PartyLevelBadge),
            "the level badge ImageNode uses the correct HudImageKey"
        );
        let level_text = app
            .world()
            .get::<Children>(badge_entity)
            .expect("badge has a level-number text child")
            .iter()
            .find_map(|c| app.world().get::<Text>(c))
            .expect("level text exists");
        assert_eq!(
            level_text.0, "12",
            "the badge shows the member's real mirrored level"
        );

        let name_entity = descendants
            .iter()
            .copied()
            .find(|&e| app.world().get::<PartyNameLabel>(e).is_some())
            .expect("name label exists");
        assert_eq!(
            app.world().get::<Text>(name_entity).unwrap().0,
            "★ Ally",
            "the leader star + name render as before"
        );

        let bar_count = descendants
            .iter()
            .filter(|&&e| app.world().get::<BarValue>(e).is_some())
            .count();
        assert_eq!(
            bar_count, 2,
            "a fully-mirrored member gets two bar entities (health + energy)"
        );
    }

    /// [`voice_icon_key`] maps every [`PartyVoiceState`] to its own, distinct
    /// [`HudImageKey`] — the STATE→ASSET mapping this phase adds, even
    /// though every production call site currently only ever passes
    /// `Inactive` (documented data gap, see `PartyVoiceState`'s own doc
    /// comment).
    #[test]
    fn voice_icon_key_maps_every_state_to_its_own_asset() {
        assert_eq!(
            voice_icon_key(PartyVoiceState::Active),
            HudImageKey::PartyVoiceActive
        );
        assert_eq!(
            voice_icon_key(PartyVoiceState::Inactive),
            HudImageKey::PartyVoiceInactive
        );
        assert_eq!(
            voice_icon_key(PartyVoiceState::Muted),
            HudImageKey::PartyVoiceMuted
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
