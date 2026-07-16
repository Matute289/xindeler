//! BL-82 EM-5.4 — the chat panel: a bounded scrollback + channel tabs +
//! `EditableText` input box, built on the EM-5.1 widget kit
//! (`xindeler-ui`), reading [`NetChatMsg`] (server → client) and writing
//! [`ChatSendRequest`] (client → server) — the two wire types
//! `xindeler-protocol::chat` defines (see that module's doc comment for the
//! full send/receive design).
//!
//! Compiled only under the `listen-server`/`net-client` cargo features (the
//! only modes where `xindeler-protocol`'s wire types are even linked),
//! matching every other consumer module in this crate (`combat_hud`,
//! `hud_toast`, …).
//!
//! ## Scope (v1, "the whole task in one PR")
//! - Bounded scrollback (`MAX_CHAT_HISTORY` lines — old lines AND their row
//!   entities are evicted, never growing unbounded — the task's own "history
//!   should be bounded" requirement).
//! - Channel tabs: **All** (view filter only) plus the five sendable channels
//!   (Say/Region/Group/Faction/World, per `NetChatChannel::send_command_name`)
//!   plus **Whisper** (view filter only — sending a `Tell` is reachable by
//!   typing `/tell <alias> <message>`, see the protocol module's doc comment
//!   for why). Clicking a sendable tab ALSO becomes the active send channel for
//!   the next plain (non-`/command`) line typed.
//! - `EditableText` input: Enter sends; a leading `/` bypasses the channel tabs
//!   entirely and sends a raw [`ChatSendRequest::Command`] (full slash-command
//!   parity, not just the six named channels); Tab cycles command-name
//!   completions while typing a `/command`.
//! - A simple `@mention` highlight: any line containing an `@token` gets a
//!   tinted row background (cosmetic, no self-alias plumbing needed — the
//!   client doesn't know its own alias yet, nothing mirrors it).
//! - NOT built here (documented, not silently skipped): rich per-token mention
//!   colouring (needs multi-span `Text`, the whole-row tint is the v1
//!   substitute), a `Tell`/`Faction`-target picker UI (both remain
//!   command-typed only), and full Fluent rendering of non-plain `Content`
//!   (server-side `render_content` fallback covers it — EM-5.16's job).

use bevy::{color::Alpha as _, input_focus::InputFocus, prelude::*, text::EditableText};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{ChatSendRequest, NetChatChannel, NetChatMsg};
use xindeler_ui::{
    button::{Activate, button_bundle},
    scroll::scroll_view_bundle,
    theme::{HudFonts, HudTheme},
};

/// Scrollback cap (BL-82 EM-5.4's own "bounded" requirement). 200 lines is
/// comfortably more than a player reads back in one session before it
/// scrolls off anyway (legacy's own chat history isn't infinite either —
/// this is the same "keep enough to scroll back through a recent
/// conversation, not the whole server's history" posture, just an explicit
/// number instead of an implicit one).
const MAX_CHAT_HISTORY: usize = 200;

/// The panel's fixed on-screen size (bottom-left, matching legacy's chat
/// placement).
const PANEL_WIDTH: f32 = 420.0;
const SCROLL_HEIGHT: f32 = 180.0;

/// The channel tabs shown, in order: `None` = "All" (view filter only, never
/// a send target); every `Some(channel)` doubles as a filter AND (for the
/// five [`NetChatChannel::send_command_name`]-bearing kinds) a send-channel
/// selector. `Tell` is included as a VIEW filter only (see the module doc
/// comment for why sending stays command-typed).
const CHAT_TABS: [(Option<NetChatChannel>, &str); 7] = [
    (None, "All"),
    (Some(NetChatChannel::Say), "Say"),
    (Some(NetChatChannel::Region), "Region"),
    (Some(NetChatChannel::Group), "Group"),
    (Some(NetChatChannel::Faction), "Faction"),
    (Some(NetChatChannel::World), "World"),
    (Some(NetChatChannel::Tell), "Whisper"),
];

/// Minimize button labels — plain ASCII (matching every other label this
/// panel/screen renders, e.g. the full map's "M / Esc to close" hint) rather
/// than a glyphic icon, since the HUD body font isn't verified to carry
/// arrow/box-drawing glyphs.
///
/// **Keep in sync:** [`spawn_chat_panel`] spawns the button with
/// [`CHAT_MINIMIZE_LABEL`] directly (not via [`sync_chat_collapsed`], which
/// only relabels on a LATER `ChatUiState` change) — this is only correct
/// because it matches [`ChatUiState::default`]'s `collapsed: false`. If
/// either the default `collapsed` value or this spawn-time label ever
/// change independently, the button would show the wrong label for one
/// frame (until `sync_chat_collapsed` next runs on a real state change) with
/// no compiler or test error to catch it — ecs-design-reviewer finding,
/// BL-82 Phase 5 follow-up.
const CHAT_MINIMIZE_LABEL: &str = "Hide";
const CHAT_RESTORE_LABEL: &str = "Chat";

/// Slash-command names the Tab-completion cycles through (BL-82 EM-5.4).
/// The five channel keywords ([`NetChatChannel::send_command_name`]) plus
/// `tell`/`w` (the two forms legacy's own `ServerChatCommand::Tell` keyword
/// list documents — `common::cmd`).
const KNOWN_COMMANDS: &[&str] = &["say", "region", "group", "faction", "world", "tell", "w"];

/// The chat panel's live UI state: which channel the scrollback is currently
/// FILTERED to (`None` = show every channel), which channel a plain
/// (non-`/command`) line sends to next, and whether the panel is currently
/// minimized (BL-82 Phase 5 follow-up: a real play session found the chat
/// window had no minimize control at all — every OTHER Phase-5 screen either
/// toggles via `HudAction`/`HudState` (a real secondary window, e.g. the map)
/// or, like this panel, is an always-on ambient overlay; a bounded scrollback
/// panel wants to shrink out of the way without fully closing, so `collapsed`
/// lives HERE rather than as a `HudWindow` variant — `HudState.open_window`
/// is a single mutually-exclusive slot (opening Map/Inventory/etc. closes
/// whatever else was open), which is the wrong shape for "minimize this
/// always-visible panel while nothing else is open"). `collapsed` is
/// session-only (not persisted to `settings.ron`) for v1 — a documented
/// follow-up, matching legacy's own `settings.interface.toggle_chat`, not a
/// silently dropped requirement.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatUiState {
    view_filter: Option<NetChatChannel>,
    send_channel: NetChatChannel,
    collapsed: bool,
}

impl Default for ChatUiState {
    /// `World` matches legacy's own `ChatMode::default()`.
    fn default() -> Self {
        Self {
            view_filter: None,
            send_channel: NetChatChannel::World,
            collapsed: false,
        }
    }
}

#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatPanelRoot;
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatScrollArea;
/// `pub(crate)` (BL-82 EM-5.17 Phase 0): [`text_input_focused`] is called
/// from other modules (`diary`/`inventory_ui`/`social_hud`/`map_view`) as a
/// run condition, and its `Query<Entity, With<ChatInputBox>>` parameter type
/// must be at least as visible as the function itself.
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct ChatInputBox;
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatInputPlaceholder;
/// Tags every element that hides when [`ChatUiState::collapsed`] is `true`
/// (the tab row + the scrollback + the input row) — the minimize BUTTON
/// itself is deliberately the ONLY thing NOT tagged, so a collapsed panel
/// still shows a way to restore it (the sole visible/clickable restore
/// affordance while collapsed).
///
/// BL-82 EM-5.17 Phase 0: the input row used to be deliberately excluded too
/// (a "collapses to just the input bar" framing), but that left a
/// "collapsed" chat panel still showing (and still typeable into) its input
/// box + placeholder — reading as "doesn't hide" even though the code did
/// exactly what it claimed. The input row is now tagged `ChatCollapsible`
/// alongside the tab row/scrollback, so collapsing genuinely hides the WHOLE
/// chat body and leaves only the minimize/restore button on screen.
///
/// [`sync_chat_collapsed`] toggles these via `Node::display`
/// (`Display::None`/`Flex`), NOT `Visibility::Hidden` — a deliberate
/// deviation from this crate's usual hide/show idiom (`map_view.rs`/
/// `controls_screen.rs`/`xindeler-ui`'s own widgets all use `Visibility`).
/// `Visibility::Hidden` stops rendering but leaves an entity's LAYOUT
/// footprint intact, which would leave the panel's overall height unchanged
/// while collapsed — defeating "collapses to just the button" (the whole
/// point of minimizing). `Node::display = Display::None` removes the tab
/// row/scrollback/input row from layout entirely, so the panel genuinely
/// shrinks. Do NOT "fix" this back to `Visibility` to match the rest of the
/// codebase — it would silently reintroduce the footprint bug.
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatCollapsible;
/// The minimize/restore header button.
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatMinimizeButton;
/// Tags a spawned chat-line row with the channel it belongs to, so
/// [`apply_chat_filter`] can toggle its [`Visibility`] without re-reading
/// [`NetChatMsg`] history (which is transient/already-drained).
#[derive(Component, Debug, Clone, Copy)]
struct ChatRow(NetChatChannel);
/// Tags a tab button with which channel it selects (`None` = "All").
#[derive(Component, Debug, Clone, Copy)]
struct ChatTab(Option<NetChatChannel>);

/// FIFO of spawned [`ChatRow`] entities, oldest-first — the bookkeeping
/// [`ingest_chat_messages`] needs to evict/despawn the oldest row once
/// [`MAX_CHAT_HISTORY`] is exceeded.
#[derive(Resource, Debug, Default)]
struct ChatHistory(Vec<Entity>);

/// Installs the whole chat panel: spawns it at `Startup` (after the theme),
/// then keeps the scrollback/tabs/input synced every frame.
pub struct ChatViewPlugin;

impl Plugin for ChatViewPlugin {
    fn build(&self, app: &mut App) {
        // Registered here too (idempotent alongside `XindelerProtocolPlugin`'s
        // own registration) so this plugin's tests don't need the whole
        // protocol plugin — same convention `hud_toast.rs` follows for
        // `HudToast`.
        app.add_message::<NetChatMsg>();
        app.add_message::<ChatSendRequest>();
        app.init_resource::<ChatUiState>();
        app.init_resource::<ChatHistory>();
        // `XindelerUiPlugin` is a plain `Plugin` (`is_unique()` defaults to
        // `true`), and `combat_hud::CombatHudViewPlugin` — added alongside
        // this plugin in every real shell (`listen_server.rs`/`net_client.
        // rs`) — ALSO adds it (behind the identical guard, following a
        // bevy-migration-reviewer BLOCKER finding: an earlier version of this
        // fix guarded only ONE of the two call sites, which only avoided the
        // "plugin was already added" panic by accident of registration
        // ORDER — reordering the two view plugins, or adding a third one
        // ahead of `CombatHudViewPlugin`, silently reintroduced it). Both
        // call sites now guard identically, so the add is truly
        // order-independent — this crate's own
        // `xindeler_ui::XindelerUiPlugin::build` guards its OWN inner
        // `UiWidgetsPlugins` add the same way, for the same reason.
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        app.add_systems(
            Startup,
            spawn_chat_panel.after(xindeler_ui::theme::init_theme),
        );
        app.add_systems(
            Update,
            (
                ingest_chat_messages,
                apply_chat_filter,
                sync_chat_tabs,
                sync_chat_collapsed,
                update_input_placeholder,
                handle_chat_submit,
                chat_smoke_verify,
                // Reads `ActionState` — must run after the frame's real
                // input resolution (BL-82 EM-5.17 Phase 0, same fix as
                // `diary::toggle_diary_window`/`controls_screen::
                // toggle_controls_screen`).
                toggle_chat_via_hotkey.after(xindeler_input::InputResolveSet),
                // The explicit blur path `text_input_focused`'s doc comment
                // requires — ecs-design-reviewer BLOCKER fix, see
                // `blur_chat_input_on_escape`'s own doc comment.
                blur_chat_input_on_escape,
            ),
        );
    }
}

/// SCAFFOLDING for automated live verification (BL-82 EM-5.4), same spirit
/// as `player_input::SmokeAutoMovePlugin`: gated by `XINDELER_SMOKE_CHAT_LINE`
/// (a no-op, single cached env read, on every ordinary run where it's
/// unset), this drives the EXACT round trip a human typing + pressing Enter
/// would — write one [`ChatSendRequest`], then watch the REAL scrollback
/// for the line to come back — proving the full client-send → embedded
/// `client::Client` → real sim chat-command handling → broadcast →
/// `NetChatMsg` → [`ingest_chat_messages`] path works, not a client-only
/// echo (a client-only echo would never touch [`ChatSendRequest`]/
/// [`NetChatMsg`] at all). Logs a clear PASS/FAIL and exits the process via
/// `AppExit` either way, so a scripted run terminates on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ChatSmokeStage {
    #[default]
    WaitingForPlayer,
    Sent,
    Done,
}

/// Frames to wait, once sent, before declaring the round trip failed —
/// generous (the embedded player's own tick + a real chat-command dispatch
/// round trip in well under a second in practice; this is a safety net, not
/// the expected path).
const CHAT_SMOKE_TIMEOUT_FRAMES: u32 = 1800;

#[allow(clippy::too_many_arguments)]
fn chat_smoke_verify(
    mut configured_line: Local<Option<Option<String>>>,
    mut stage: Local<ChatSmokeStage>,
    mut frames_since_sent: Local<u32>,
    local_player: Query<(), With<xindeler_protocol::NetLocalPlayer>>,
    rows: Query<&Text, With<ChatRow>>,
    mut send: MessageWriter<ChatSendRequest>,
    mut exit: MessageWriter<AppExit>,
) {
    let line =
        configured_line.get_or_insert_with(|| std::env::var("XINDELER_SMOKE_CHAT_LINE").ok());
    let Some(line) = line else {
        return; // unset: a cheap no-op every frame, exactly like every other pre-existing run.
    };

    match *stage {
        ChatSmokeStage::WaitingForPlayer => {
            if local_player.iter().next().is_some() {
                send.write(ChatSendRequest::Channel {
                    channel: NetChatChannel::World,
                    text: line.clone(),
                });
                info!(
                    line = %line,
                    "smoke-chat: sent the scripted line, waiting for the real round-trip \
                     broadcast to reach the scrollback"
                );
                *stage = ChatSmokeStage::Sent;
            }
        },
        ChatSmokeStage::Sent => {
            *frames_since_sent += 1;
            if rows.iter().any(|text| text.0.contains(line.as_str())) {
                info!(
                    line = %line,
                    "smoke-chat: PASS — the scripted line round-tripped through the real sim \
                     and appeared in the scrollback"
                );
                exit.write(AppExit::Success);
                *stage = ChatSmokeStage::Done;
            } else if *frames_since_sent > CHAT_SMOKE_TIMEOUT_FRAMES {
                error!(
                    line = %line,
                    "smoke-chat: FAIL — the scripted line never appeared in the scrollback \
                     within the timeout"
                );
                exit.write(AppExit::error());
                *stage = ChatSmokeStage::Done;
            }
        },
        ChatSmokeStage::Done => {},
    }
}

/// Spawns the panel root (bottom-left, using the real themed
/// [`anchored_panel_bundle`] primitive — border/background/radius, not a
/// bare `Node`), the tab row, the scrollable message log, and the input row
/// (placeholder label + `EditableText` box).
fn spawn_chat_panel(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    let root = commands
        .spawn((
            ChatPanelRoot,
            xindeler_ui::panel::anchored_panel_bundle(&theme, None, Some(16.0), None, Some(16.0)),
        ))
        .id();
    // `.entry::<Node>().and_modify(..)` mutates the EXISTING `Node`
    // `anchored_panel_bundle` already inserted, in place — a second
    // `insert(Node { .. })` would REPLACE it wholesale and silently discard
    // its border/radius/background sizing, exactly the regression
    // `combat_hud.rs`'s own `spawn_combat_hud_keeps_every_bars_sizing_from_
    // spawn_bar_intact` test documents.
    commands
        .entity(root)
        .entry::<Node>()
        .and_modify(|mut node| node.flex_direction = FlexDirection::Column);
    commands.entity(root).with_children(|parent| {
        // Header row: the minimize/restore button — deliberately OUTSIDE
        // `ChatCollapsible` (a sibling, not a child, of the tab row) so it
        // stays visible and clickable even while the panel is collapsed.
        parent
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                width: Val::Px(PANEL_WIDTH),
                margin: UiRect::bottom(Val::Px(theme.spacing.xs)),
                ..Default::default()
            })
            .with_children(|header| {
                header
                    .spawn(button_bundle(&theme, &fonts, CHAT_MINIMIZE_LABEL))
                    .insert(ChatMinimizeButton)
                    .observe(handle_chat_minimize_click);
            });

        // Tab row.
        parent
            .spawn((ChatCollapsible, Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.xs),
                margin: UiRect::bottom(Val::Px(theme.spacing.xs)),
                ..Default::default()
            }))
            .with_children(|tabs| {
                for (channel, label) in CHAT_TABS {
                    tabs.spawn(button_bundle(&theme, &fonts, label))
                        .insert(ChatTab(channel))
                        .observe(handle_tab_click);
                }
            });

        // Scrollable message log.
        parent
            .spawn(scroll_view_bundle(&theme, PANEL_WIDTH, SCROLL_HEIGHT))
            .insert((ChatScrollArea, ChatCollapsible));

        // Input row: placeholder label (shown only while empty) +
        // EditableText box. Tagged `ChatCollapsible` (BL-82 EM-5.17 Phase 0)
        // — the header row/minimize button stays the ONLY thing outside
        // `ChatCollapsible` (see that marker's own doc comment); before this
        // fix, only the tab row + scrollback were tagged, so a "collapsed"
        // chat panel still showed its input box and could still be typed
        // into, which read as "doesn't hide" even though the code did
        // exactly what it claimed to.
        parent
            .spawn((ChatCollapsible, Node {
                position_type: PositionType::Relative,
                width: Val::Px(PANEL_WIDTH),
                margin: UiRect::top(Val::Px(theme.spacing.xs)),
                ..Default::default()
            }))
            .with_children(|row| {
                row.spawn((
                    ChatInputPlaceholder,
                    Text("Type a message… (/ for commands)".to_owned()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text_muted),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(4.0),
                        left: Val::Px(6.0),
                        ..Default::default()
                    },
                ));
                row.spawn((
                    ChatInputBox,
                    EditableText::new(""),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                    Node {
                        width: Val::Px(PANEL_WIDTH),
                        padding: UiRect::all(Val::Px(4.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..Default::default()
                    },
                    BackgroundColor(theme.palette.panel_bg),
                    bevy::ui::BorderColor::all(theme.palette.panel_border),
                ));
            });
    });
}

/// A tab button's channel colour tag, rendered as a short bracketed prefix on
/// each scrollback line — cheap differentiation without a second `Text`
/// span per row (v1; see the module doc comment).
fn channel_tag(channel: NetChatChannel) -> &'static str {
    match channel {
        NetChatChannel::Say => "Say",
        NetChatChannel::Region => "Region",
        NetChatChannel::Group => "Group",
        NetChatChannel::Faction => "Faction",
        NetChatChannel::World => "World",
        NetChatChannel::Tell => "Whisper",
        NetChatChannel::Npc => "NPC",
        NetChatChannel::System => "System",
    }
}

/// Renders one [`NetChatMsg`] to its scrollback line text: `[Tag] alias:
/// text` (or `[Tag] text` when there's no resolvable sender — a system line
/// or an NPC the sim couldn't name).
fn format_chat_line(msg: &NetChatMsg) -> String {
    let tag = channel_tag(msg.channel);
    match &msg.sender_alias {
        Some(alias) => format!("[{tag}] {alias}: {}", msg.text),
        None => format!("[{tag}] {}", msg.text),
    }
}

/// A crude but real "mentions" signal (spec §2 EM-5.4): any whitespace-
/// separated token starting with `@` (and at least one character after it)
/// counts as a mention — highlighting the whole ROW rather than the token
/// itself (v1; see the module doc comment for why per-token colouring is
/// deferred).
fn contains_mention(text: &str) -> bool {
    text.split_whitespace()
        .any(|token| token.starts_with('@') && token.len() > 1)
}

/// Drains arriving [`NetChatMsg`]s, spawning one text row per line (tinted
/// if it looks like a mention — [`contains_mention`]), applying the CURRENT
/// [`ChatUiState::view_filter`] to its initial visibility, and evicting the
/// oldest row once [`MAX_CHAT_HISTORY`] is exceeded (bounded history — see
/// the module doc comment). A no-op (messages simply aren't consumed into
/// history) if the scroll-area entity doesn't exist yet (pre-`Startup`
/// ordering edge, self-heals next frame).
fn ingest_chat_messages(
    mut commands: Commands,
    mut incoming: MessageReader<NetChatMsg>,
    mut history: ResMut<ChatHistory>,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    filter: Res<ChatUiState>,
    scroll_area: Query<Entity, With<ChatScrollArea>>,
    mut scroll_positions: Query<&mut ScrollPosition, With<ChatScrollArea>>,
) {
    let Ok(scroll_entity) = scroll_area.single() else {
        return;
    };

    let mut appended_any = false;
    for msg in incoming.read() {
        appended_any = true;
        let visible = filter
            .view_filter
            .is_none_or(|channel| channel == msg.channel);
        let mentioned = contains_mention(&msg.text);
        let row = commands
            .spawn((
                ChatRow(msg.channel),
                Text(format_chat_line(msg)),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(14.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
                BackgroundColor(if mentioned {
                    theme.palette.accent.with_alpha(0.25)
                } else {
                    Color::NONE
                }),
                Node {
                    display: if visible {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    ..Default::default()
                },
            ))
            .id();
        commands.entity(scroll_entity).add_child(row);

        history.0.push(row);
        if history.0.len() > MAX_CHAT_HISTORY {
            let oldest = history.0.remove(0);
            commands.entity(oldest).despawn();
        }
    }

    // Auto-scroll to the bottom whenever a new line arrived — an
    // over-large target is harmless (bevy_ui's own layout clamps
    // `ScrollPosition` against the real content height every frame; this
    // system doesn't know that height without an extra query, so it just
    // asks for "as far down as possible").
    if appended_any && let Ok(mut scroll) = scroll_positions.single_mut() {
        scroll.y = f32::MAX / 2.0;
    }
}

/// Re-applies [`ChatUiState::view_filter`] to every [`ChatRow`]'s
/// [`Node::display`] whenever the filter changes (a tab click) — does NOT
/// touch rows spawned this same frame twice (`ingest_chat_messages` already
/// applied the filter at spawn time; this system is `Changed`-gated on
/// [`ChatUiState`], not `NetChatMsg` arrival, so the two never fight).
fn apply_chat_filter(filter: Res<ChatUiState>, mut rows: Query<(&ChatRow, &mut Node)>) {
    if !filter.is_changed() {
        return;
    }
    for (row, mut node) in &mut rows {
        let visible = filter.view_filter.is_none_or(|channel| channel == row.0);
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Highlights the currently-ACTIVE view-filter tab (background swap) so the
/// player can see which tab is selected.
fn sync_chat_tabs(
    filter: Res<ChatUiState>,
    theme: Res<HudTheme>,
    mut tabs: Query<(&ChatTab, &mut BackgroundColor)>,
) {
    if !filter.is_changed() {
        return;
    }
    for (tab, mut background) in &mut tabs {
        background.0 = if tab.0 == filter.view_filter {
            theme.palette.accent
        } else {
            theme.palette.panel_bg
        };
    }
}

/// A tab click updates [`ChatUiState`]: `None` ("All") only changes the view
/// filter; a sendable channel changes BOTH the filter and the active send
/// channel (see the module doc comment); `Tell`/`Npc`/`System` (view-only
/// kinds) only change the filter.
fn handle_tab_click(activate: On<Activate>, tabs: Query<&ChatTab>, mut state: ResMut<ChatUiState>) {
    let Ok(tab) = tabs.get(activate.entity) else {
        return;
    };
    state.view_filter = tab.0;
    if let Some(channel) = tab.0
        && channel.send_command_name().is_some()
    {
        state.send_channel = channel;
    }
}

/// Whether the chat input box currently has keyboard focus
/// ([`InputFocus`]) — the shared "don't fire a hotkey while the player is
/// typing" predicate BL-82 EM-5.17 Phase 0 introduces.
///
/// Why this exists: legacy `xindeler-old` gates every hotkey handler on `if
/// !self.typing()` (`Hud::typing()` — a single boolean answering "is a
/// text-edit widget currently capturing keyboard input") so that typing "i"
/// while chatting doesn't ALSO open the inventory. The Bevy port had no
/// equivalent — verified by reading every `HudAction`-emitting toggle system
/// in this crate (`diary`/`inventory_ui`/`social_hud`/`map_view`/
/// `controls_screen`), none checked chat focus. Rather than invent a new
/// focus-tracking mechanism, this reuses the [`InputFocus`] resource
/// `chat.rs` already maintains for its own Enter/Tab handling
/// ([`handle_chat_submit`]) together with the [`ChatInputBox`] marker — the
/// two already say everything "is the player typing" needs to know.
///
/// A plain `Fn(..) -> bool` system, composable with
/// `.run_if(not(crate::chat::text_input_focused))` on any `Update` system
/// (see `diary::DiaryUiPlugin`/`inventory_ui::InventoryUiPlugin`/
/// `social_hud::SocialHudViewPlugin`/`map_view::MapViewPlugin` for the
/// wiring).
///
/// **This predicate is only correct alongside a blur path** —
/// [`blur_chat_input_on_escape`] below. `InputFocus` is only ever SET in
/// this codebase: automatically, by `bevy_ui_widgets::text_input`'s own
/// pointer-press observer (vendored library behaviour, not code we wrote)
/// the first time the player clicks into the chat input box. Nothing
/// UN-sets it on its own — not `handle_chat_submit` (clears the TEXT on
/// Enter, never `InputFocus`), not `EditableText`'s own Escape handling
/// (only collapses the text selection, doesn't blur), not the chat tabs/
/// minimize button (plain `Button`/`Activate` widgets, same as every other
/// HUD button — none touch `InputFocus`). An ecs-design-reviewer BLOCKER
/// finding on an earlier version of this fix: without an explicit blur
/// path, the FIRST chat message of a session would make this predicate
/// return `true` forever after, permanently (not intermittently)
/// suppressing every gated hotkey (P/I/M/O). [`blur_chat_input_on_escape`]
/// closes that gap — Escape while chat holds focus clears [`InputFocus`],
/// matching legacy `xindeler-old`'s own `Hud::typing()`/
/// `focus_widget(None)` precedent this doc comment already cited (the
/// "gate hotkeys on typing" half was ported first; this is the "give the
/// player a way out of typing" half).
pub(crate) fn text_input_focused(
    focus: Res<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
) -> bool {
    let Ok(input_entity) = inputs.single() else {
        return false;
    };
    focus.get() == Some(input_entity)
}

/// Clears [`InputFocus`] when Escape is pressed WHILE the chat input box
/// holds it — the explicit blur path [`text_input_focused`]'s own doc
/// comment requires (BL-82 EM-5.17 Phase 0, ecs-design-reviewer BLOCKER
/// fix). Without this, `InputFocus` is only ever set (by `bevy_ui_widgets`'
/// own click-to-focus behaviour) and never cleared, so the typing-focus
/// guard would permanently suppress every gated hotkey after the first chat
/// message of a session, for good. A no-op if the input box isn't currently
/// focused (Escape then falls through to whatever else reads it, e.g.
/// `camera.rs`'s cursor-release handling).
fn blur_chat_input_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let Ok(input_entity) = inputs.single() else {
        return;
    };
    if focus.get() == Some(input_entity) {
        focus.clear();
    }
}

/// The minimize/restore button's click: flips [`ChatUiState::collapsed`].
/// Deliberately a direct `Activate` observer mutating this screen's own
/// `Resource` — the SAME shape [`handle_tab_click`] right above already uses
/// for this exact panel, rather than routing through the generic
/// `HudAction`/`HudState` bus (reserved for real mutually-exclusive
/// secondary windows, per [`ChatUiState`]'s own doc comment).
fn handle_chat_minimize_click(_activate: On<Activate>, mut state: ResMut<ChatUiState>) {
    state.collapsed = !state.collapsed;
}

/// [`GameInput::ToggleChat`] (`F5` by default, rebindable) does the SAME
/// thing as clicking the minimize/restore button — flips
/// [`ChatUiState::collapsed`]. BL-82 EM-5.17 Phase 0: nothing previously read
/// `GameInput::ToggleChat` at all despite it existing in the keymap.
fn toggle_chat_via_hotkey(action_state: Res<ActionState>, mut state: ResMut<ChatUiState>) {
    if action_state.just_pressed(GameInput::ToggleChat) {
        state.collapsed = !state.collapsed;
    }
}

/// Hides every [`ChatCollapsible`] element (tab row + scrollback + input row)
/// while [`ChatUiState::collapsed`] is `true`, and relabels the minimize
/// button (`"Hide"` <-> `"Chat"`) to reflect which action it will perform
/// next.
fn sync_chat_collapsed(
    state: Res<ChatUiState>,
    mut collapsible: Query<&mut Node, With<ChatCollapsible>>,
    buttons: Query<&Children, With<ChatMinimizeButton>>,
    mut texts: Query<&mut Text>,
) {
    if !state.is_changed() {
        return;
    }
    for mut node in &mut collapsible {
        node.display = if state.collapsed {
            Display::None
        } else {
            Display::Flex
        };
    }
    let label = if state.collapsed {
        CHAT_RESTORE_LABEL
    } else {
        CHAT_MINIMIZE_LABEL
    };
    for children in &buttons {
        for &child in children {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = label.to_owned();
            }
        }
    }
}

/// Shows/hides the placeholder label based on whether the input box is
/// empty (`EditableText::value()`) — `EditableText` itself doesn't support a
/// native placeholder in Bevy 0.19 (the EM-5.1 survey's own noted gap), so
/// this is the "faded overlay label" workaround that doc comment names.
fn update_input_placeholder(
    inputs: Query<&EditableText, With<ChatInputBox>>,
    mut placeholders: Query<&mut Visibility, With<ChatInputPlaceholder>>,
) {
    let Ok(input) = inputs.single() else {
        return;
    };
    let Ok(mut visibility) = placeholders.single_mut() else {
        return;
    };
    *visibility = if input.value().to_string().is_empty() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
}

/// Splits a leading `/command args…` line into `(name, args)`, or `None` if
/// `raw` doesn't start with `/` (a plain channel-tab line instead). Args are
/// split on whitespace — sufficient for `/tell <alias> <message words…>`
/// (the server rejoins `args[1..]` itself, see `xindeler-sim-bridge::chat`'s
/// doc comment) and for the single-target admin commands legacy supports;
/// NOT quote-aware (a v1 cut, matching the scope note at the top).
fn parse_slash_command(raw: &str) -> Option<(String, Vec<String>)> {
    let rest = raw.strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let name = parts.next()?.to_owned();
    let args = parts.map(str::to_owned).collect();
    Some((name, args))
}

/// The command-name completions matching `prefix` (case-sensitive, matching
/// [`KNOWN_COMMANDS`]' own lowercase convention), in [`KNOWN_COMMANDS`]'s
/// declared order.
fn matching_commands(prefix: &str) -> Vec<&'static str> {
    KNOWN_COMMANDS
        .iter()
        .copied()
        .filter(|candidate| candidate.starts_with(prefix))
        .collect()
}

/// Replaces the command-name token (the part right after `/`, before the
/// first space) of `raw` with `replacement`, leaving everything from the
/// first space onward untouched. `raw` must start with `/` (callers only
/// call this once [`parse_slash_command`] confirms it does).
fn replace_command_name(raw: &str, replacement: &str) -> String {
    match raw.find(char::is_whitespace) {
        Some(space) => format!("/{replacement}{}", &raw[space..]),
        None => format!("/{replacement}"),
    }
}

/// Enter submits the input box's current text; Tab, while typing a
/// `/command`, cycles through [`KNOWN_COMMANDS`] completions. Both only act
/// while the input box actually has keyboard focus ([`InputFocus`]) — this
/// system reads global key state directly (not an `EditableText` keyboard
/// observer) so it works regardless of `EditableTextInputPlugin`'s own
/// Enter-propagation behaviour (Enter falls through un-consumed when
/// `allow_newlines` is `false`, which this input box's default already is).
fn handle_chat_submit(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Res<InputFocus>,
    mut inputs: Query<(Entity, &mut EditableText), With<ChatInputBox>>,
    mut send: MessageWriter<ChatSendRequest>,
    state: Res<ChatUiState>,
    mut completion_cycle: Local<usize>,
) {
    let Ok((entity, mut input)) = inputs.single_mut() else {
        return;
    };
    if focus.get() != Some(entity) {
        return;
    }

    if keys.just_pressed(KeyCode::Tab) {
        let raw = input.value().to_string();
        if let Some((name, _)) = parse_slash_command(&raw) {
            let matches = matching_commands(&name);
            if !matches.is_empty() {
                let next = matches[*completion_cycle % matches.len()];
                *completion_cycle = completion_cycle.wrapping_add(1);
                let replaced = replace_command_name(&raw, next);
                // `EditableText::new` re-seeds the editor with fresh text AND
                // moves the cursor to the end — a full-content replace via
                // documented public API only (no `parley`/`PlainEditor`
                // internals), matching how the Enter-path below also treats
                // the box's content as an opaque value it replaces wholesale.
                *input = EditableText::new(replaced);
            }
        }
        return;
    }

    if !keys.just_pressed(KeyCode::Enter) {
        return;
    }

    let raw = input.value().to_string();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }

    let request = match parse_slash_command(trimmed) {
        Some((name, args)) => ChatSendRequest::Command { name, args },
        None => ChatSendRequest::Channel {
            channel: state.send_channel,
            text: trimmed.to_owned(),
        },
    };
    send.write(request);
    input.clear();
    *completion_cycle = 0;
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use xindeler_protocol::NetUid;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<NetChatMsg>();
        app.add_message::<ChatSendRequest>();
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.init_resource::<ChatUiState>();
        app.init_resource::<ChatHistory>();
        // Needed by `handle_chat_submit`'s tests — `MinimalPlugins` doesn't
        // register `ButtonInput<KeyCode>` (that's `InputPlugin`, part of
        // `DefaultPlugins`).
        app.init_resource::<ButtonInput<KeyCode>>();
        app
    }

    /// [`format_chat_line`]: a resolved sender renders `[Tag] alias: text`;
    /// an unresolved one drops the `alias:` part.
    #[test]
    fn formats_lines_with_and_without_a_resolved_sender() {
        let with_sender = NetChatMsg {
            channel: NetChatChannel::Say,
            sender_uid: Some(NetUid(1)),
            sender_alias: Some("Hero".to_owned()),
            text: "hello".to_owned(),
        };
        assert_eq!(format_chat_line(&with_sender), "[Say] Hero: hello");

        let without_sender = NetChatMsg {
            channel: NetChatChannel::System,
            sender_uid: None,
            sender_alias: None,
            text: "server started".to_owned(),
        };
        assert_eq!(format_chat_line(&without_sender), "[System] server started");
    }

    /// [`contains_mention`]: an `@token` anywhere in the message counts; a
    /// bare `@` with nothing after it, or no `@` at all, does not.
    #[test]
    fn detects_at_mentions() {
        assert!(contains_mention("hey @Hero check this out"));
        assert!(!contains_mention("no mention here"));
        assert!(!contains_mention("a lone @ with nothing after"));
    }

    /// [`ingest_chat_messages`] spawns a real `ChatRow` per arriving message,
    /// tags it with the message's channel, and puts it in the scrollback's
    /// child list.
    #[test]
    fn ingest_spawns_a_tagged_row_per_message_and_parents_it_to_the_scroll_area() {
        let mut app = new_app();
        let scroll_area = app
            .world_mut()
            .spawn((ChatScrollArea, ScrollPosition::default()))
            .id();

        app.world_mut().write_message(NetChatMsg {
            channel: NetChatChannel::World,
            sender_uid: Some(NetUid(1)),
            sender_alias: Some("Hero".to_owned()),
            text: "hello world".to_owned(),
        });
        app.world_mut()
            .run_system_once(ingest_chat_messages)
            .expect("system runs");
        app.update();

        let children = app
            .world()
            .get::<Children>(scroll_area)
            .expect("the scroll area got a child row");
        assert_eq!(children.len(), 1);
        let row = children[0];
        assert_eq!(
            app.world().get::<ChatRow>(row).map(|r| r.0),
            Some(NetChatChannel::World)
        );
        assert_eq!(
            app.world().get::<Text>(row).map(|t| t.0.clone()),
            Some("[World] Hero: hello world".to_owned())
        );
    }

    /// The scrollback is BOUNDED: pushing more than [`MAX_CHAT_HISTORY`]
    /// lines despawns the oldest row(s) rather than growing forever — the
    /// task's own explicit requirement.
    #[test]
    fn scrollback_evicts_the_oldest_row_past_the_cap() {
        let mut app = new_app();
        app.world_mut()
            .spawn((ChatScrollArea, ScrollPosition::default()));

        for i in 0..(MAX_CHAT_HISTORY + 5) {
            app.world_mut().write_message(NetChatMsg {
                channel: NetChatChannel::World,
                sender_uid: None,
                sender_alias: None,
                text: format!("line {i}"),
            });
            app.world_mut()
                .run_system_once(ingest_chat_messages)
                .expect("system runs");
        }

        let history = app.world().resource::<ChatHistory>();
        assert_eq!(
            history.0.len(),
            MAX_CHAT_HISTORY,
            "history must never exceed the cap"
        );
        // The oldest surviving line must be #5 (0..5 evicted), not #0.
        let oldest = *history.0.first().expect("at least one row remains");
        assert_eq!(
            app.world().get::<Text>(oldest).map(|t| t.0.clone()),
            Some("[World] line 5".to_owned())
        );
    }

    /// A tab click updates the view filter; clicking a SENDABLE channel also
    /// updates the send channel, but clicking "All" (`None`) only changes
    /// the filter, leaving the previous send channel intact.
    #[test]
    fn tab_click_updates_filter_and_conditionally_the_send_channel() {
        let mut app = new_app();
        let say_tab = app
            .world_mut()
            .spawn(ChatTab(Some(NetChatChannel::Say)))
            .id();

        app.world_mut()
            .run_system_once(
                move |tabs: Query<&ChatTab>, mut state: ResMut<ChatUiState>| {
                    let tab = tabs.get(say_tab).unwrap();
                    state.view_filter = tab.0;
                    if let Some(channel) = tab.0
                        && channel.send_command_name().is_some()
                    {
                        state.send_channel = channel;
                    }
                },
            )
            .expect("system runs");

        let state = app.world().resource::<ChatUiState>();
        assert_eq!(state.view_filter, Some(NetChatChannel::Say));
        assert_eq!(state.send_channel, NetChatChannel::Say);

        // Selecting a view-only channel (Tell) leaves the send channel where
        // Say left it.
        let tell_tab = app
            .world_mut()
            .spawn(ChatTab(Some(NetChatChannel::Tell)))
            .id();
        app.world_mut()
            .run_system_once(
                move |tabs: Query<&ChatTab>, mut state: ResMut<ChatUiState>| {
                    let tab = tabs.get(tell_tab).unwrap();
                    state.view_filter = tab.0;
                    if let Some(channel) = tab.0
                        && channel.send_command_name().is_some()
                    {
                        state.send_channel = channel;
                    }
                },
            )
            .expect("system runs");
        let state = app.world().resource::<ChatUiState>();
        assert_eq!(state.view_filter, Some(NetChatChannel::Tell));
        assert_eq!(
            state.send_channel,
            NetChatChannel::Say,
            "a view-only tab must not clobber the send channel"
        );
    }

    /// [`apply_chat_filter`] hides rows that don't match the current view
    /// filter and shows rows that do, without touching rows created after
    /// the filter last changed... (exercised here as a direct before/after).
    #[test]
    fn filter_change_hides_non_matching_rows_and_shows_matching_ones() {
        let mut app = new_app();
        let say_row = app
            .world_mut()
            .spawn((ChatRow(NetChatChannel::Say), Node::default()))
            .id();
        let world_row = app
            .world_mut()
            .spawn((ChatRow(NetChatChannel::World), Node::default()))
            .id();

        app.world_mut().resource_mut::<ChatUiState>().view_filter = Some(NetChatChannel::Say);
        app.world_mut()
            .run_system_once(apply_chat_filter)
            .expect("system runs");

        assert_eq!(
            app.world().get::<Node>(say_row).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(world_row).unwrap().display,
            Display::None
        );
    }

    /// [`ChatUiState`] starts NOT collapsed — the panel is visible by
    /// default, matching every other always-on HUD element (BL-82 Phase 5
    /// follow-up regression test: a real play session found no way to
    /// minimize the chat panel at all).
    #[test]
    fn chat_starts_uncollapsed() {
        assert!(!ChatUiState::default().collapsed);
    }

    /// [`sync_chat_collapsed`]: collapsing hides every [`ChatCollapsible`]
    /// element (tab row + scrollback + input row) but leaves anything NOT
    /// tagged (the minimize button itself) untouched, and relabels the
    /// button; un-collapsing restores all three.
    ///
    /// BL-82 EM-5.17 Phase 0: `input_row` is new here — before this fix, the
    /// input row wasn't tagged `ChatCollapsible` at all, so a "collapsed"
    /// chat panel still showed (and could still be typed into) its input box
    /// and placeholder, which read as "doesn't hide" even though the code
    /// did exactly what it claimed to.
    #[test]
    fn sync_chat_collapsed_hides_collapsible_elements_and_relabels_the_button() {
        let mut app = new_app();
        let tab_row = app
            .world_mut()
            .spawn((ChatCollapsible, Node::default()))
            .id();
        let scroll_area = app
            .world_mut()
            .spawn((ChatCollapsible, ChatScrollArea, Node::default()))
            .id();
        let input_row = app
            .world_mut()
            .spawn((ChatCollapsible, Node::default()))
            .id();
        let label = app
            .world_mut()
            .spawn(Text(CHAT_MINIMIZE_LABEL.to_owned()))
            .id();
        app.world_mut().spawn(ChatMinimizeButton).add_child(label);

        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(sync_chat_collapsed)
            .expect("system runs");

        assert_eq!(
            app.world().get::<Node>(tab_row).unwrap().display,
            Display::None,
            "the tab row must hide while collapsed"
        );
        assert_eq!(
            app.world().get::<Node>(scroll_area).unwrap().display,
            Display::None,
            "the scrollback must hide while collapsed"
        );
        assert_eq!(
            app.world().get::<Node>(input_row).unwrap().display,
            Display::None,
            "the input row must ALSO hide while collapsed — only the minimize/restore button \
             stays visible"
        );
        assert_eq!(
            app.world().get::<Text>(label).unwrap().0,
            CHAT_RESTORE_LABEL,
            "the button must relabel to the restore action"
        );

        // Un-collapse: everything comes back.
        app.world_mut().resource_mut::<ChatUiState>().collapsed = false;
        app.world_mut()
            .run_system_once(sync_chat_collapsed)
            .expect("system runs again");

        assert_eq!(
            app.world().get::<Node>(tab_row).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(scroll_area).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(input_row).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Text>(label).unwrap().0,
            CHAT_MINIMIZE_LABEL
        );
    }

    /// [`toggle_chat_via_hotkey`]: [`GameInput::ToggleChat`] (`F5` by
    /// default) flips [`ChatUiState::collapsed`] — the same effect as
    /// clicking the minimize button, but from the keyboard (BL-82 EM-5.17
    /// Phase 0: nothing previously read this `GameInput` at all). Driven
    /// through the REAL `xindeler_input::action_state::update_action_state`
    /// resolver (not a hand-built `ActionState`, whose fields are private) —
    /// the same "real input → real resolver → real system" shape
    /// `controls_screen.rs`'s own
    /// `end_to_end_rebind_persists_to_disk_and_flags_a_conflict` test uses.
    #[test]
    fn toggle_chat_via_hotkey_flips_collapsed() {
        use bevy::input::keyboard::KeyCode;
        use xindeler_input::{KeyMap, action_state::update_action_state};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        // `new_app()` already inits `ButtonInput<KeyCode>` (for
        // `handle_chat_submit`'s own tests); `update_action_state` also
        // reads mouse buttons, which nothing else in this test module needs.
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        app.add_systems(
            Update,
            (update_action_state, toggle_chat_via_hotkey).chain(),
        );

        assert!(!app.world().resource::<ChatUiState>().collapsed);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F5);
        app.update();
        assert!(
            app.world().resource::<ChatUiState>().collapsed,
            "F5 (ToggleChat) must collapse the chat panel"
        );

        // Fresh press edge for the second toggle (a still-held key has no
        // NEW `just_pressed` edge next frame).
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::F5);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F5);
        app.update();
        assert!(
            !app.world().resource::<ChatUiState>().collapsed,
            "a second F5 press must restore it"
        );
    }

    /// BL-82 EM-5.17 Phase 0 regression (ecs-design-reviewer BLOCKER): once
    /// the chat input box gains [`InputFocus`], [`text_input_focused`] must
    /// stay `true` forever UNLESS something explicitly blurs it —
    /// [`blur_chat_input_on_escape`] is that path. Unlike the other new
    /// tests in this module (which hand-insert focus and never simulate
    /// "focus, then look away"), this one drives the full
    /// focus → suppressed → Escape → un-suppressed lifecycle the reviewer
    /// found nothing previously covered.
    #[test]
    fn escape_blurs_the_chat_input_and_lifts_the_typing_guard() {
        let mut app = new_app();
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));

        assert!(
            app.world_mut()
                .run_system_once(text_input_focused)
                .expect("condition runs"),
            "text_input_focused must be true while the chat input holds focus"
        );

        // Escape, while chat holds focus, must blur it.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.world_mut()
            .run_system_once(blur_chat_input_on_escape)
            .expect("system runs");

        assert!(
            !app.world_mut()
                .run_system_once(text_input_focused)
                .expect("condition runs"),
            "Escape must clear InputFocus, lifting the typing guard so gated hotkeys \
             (Diary/Inventory/Map/Social) fire again — without this, the FIRST chat message of a \
             session would suppress them permanently"
        );
    }

    /// [`blur_chat_input_on_escape`] must be a no-op when the chat input
    /// does NOT currently hold focus — it must not clear an unrelated
    /// widget's focus, nor panic when nothing is focused at all.
    #[test]
    fn escape_without_chat_focus_does_not_clear_an_unrelated_focus() {
        let mut app = new_app();
        app.world_mut().spawn((ChatInputBox, EditableText::new("")));
        let other = app.world_mut().spawn_empty().id();
        app.insert_resource(InputFocus::from_entity(other));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.world_mut()
            .run_system_once(blur_chat_input_on_escape)
            .expect("system runs");

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(other),
            "Escape must only blur the CHAT input, not whatever else happens to be focused"
        );
    }

    /// [`handle_chat_minimize_click`] flips [`ChatUiState::collapsed`] on
    /// each activation (a toggle, not a one-way close) — verified directly
    /// rather than via a real `Activate` trigger (matching this file's own
    /// `tab_click_updates_filter_and_conditionally_the_send_channel` test,
    /// which exercises `handle_tab_click`'s logic the same indirect way).
    #[test]
    fn minimize_click_toggles_collapsed_each_time() {
        let mut app = new_app();
        assert!(!app.world().resource::<ChatUiState>().collapsed);

        app.world_mut()
            .run_system_once(|mut state: ResMut<ChatUiState>| state.collapsed = !state.collapsed)
            .expect("system runs");
        assert!(app.world().resource::<ChatUiState>().collapsed);

        app.world_mut()
            .run_system_once(|mut state: ResMut<ChatUiState>| state.collapsed = !state.collapsed)
            .expect("system runs again");
        assert!(!app.world().resource::<ChatUiState>().collapsed);
    }

    /// [`parse_slash_command`]: a leading `/` splits into a command name +
    /// whitespace-separated args; anything without a leading `/` is `None`
    /// (a plain channel-tab line instead).
    #[test]
    fn parses_slash_commands_and_rejects_plain_lines() {
        assert_eq!(
            parse_slash_command("/tell Bob hi there"),
            Some(("tell".to_owned(), vec![
                "Bob".to_owned(),
                "hi".to_owned(),
                "there".to_owned()
            ]))
        );
        assert_eq!(
            parse_slash_command("/say"),
            Some(("say".to_owned(), Vec::new()))
        );
        assert_eq!(parse_slash_command("hello there"), None);
    }

    /// [`matching_commands`]/[`replace_command_name`]: Tab-completion finds
    /// every candidate starting with the typed prefix and swaps ONLY the
    /// command-name token, leaving the rest of the line untouched.
    #[test]
    fn command_completion_matches_prefix_and_preserves_the_rest_of_the_line() {
        let matches = matching_commands("s");
        assert_eq!(matches, vec!["say"]);

        let matches_r = matching_commands("r");
        assert_eq!(matches_r, vec!["region"]);

        assert_eq!(
            replace_command_name("/w Bob hello", "world"),
            "/world Bob hello"
        );
        assert_eq!(replace_command_name("/sa", "say"), "/say");
    }

    /// [`handle_chat_submit`]'s Enter path: a plain line (no leading `/`)
    /// sends a `Channel` request using the CURRENT `send_channel`; the input
    /// box is cleared afterward.
    #[test]
    fn enter_on_a_plain_line_sends_a_channel_request_and_clears_the_box() {
        let mut app = new_app();
        app.world_mut().resource_mut::<ChatUiState>().send_channel = NetChatChannel::Region;
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("hello there")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);

        app.world_mut()
            .run_system_once(handle_chat_submit)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ChatSendRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![ChatSendRequest::Channel {
            channel: NetChatChannel::Region,
            text: "hello there".to_owned(),
        }]);
        assert_eq!(
            app.world()
                .get::<EditableText>(input)
                .unwrap()
                .value()
                .to_string(),
            "",
            "the input box must clear after sending"
        );
    }

    /// A leading `/` bypasses the channel tabs entirely and sends a raw
    /// `Command` request.
    #[test]
    fn enter_on_a_slash_command_sends_a_command_request() {
        let mut app = new_app();
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("/tell Bob hi")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);

        app.world_mut()
            .run_system_once(handle_chat_submit)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ChatSendRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![ChatSendRequest::Command {
            name: "tell".to_owned(),
            args: vec!["Bob".to_owned(), "hi".to_owned()],
        }]);
    }

    /// Enter is ignored while the input box does NOT have keyboard focus —
    /// no message is sent.
    #[test]
    fn enter_without_focus_sends_nothing() {
        let mut app = new_app();
        app.world_mut()
            .spawn((ChatInputBox, EditableText::new("hello")));
        // Deliberately no `InputFocus` pointing at the input box.
        app.init_resource::<InputFocus>();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);

        app.world_mut()
            .run_system_once(handle_chat_submit)
            .expect("system runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ChatSendRequest>>()
            .drain()
            .collect();
        assert!(sent.is_empty(), "no message may send without focus");
    }
}
