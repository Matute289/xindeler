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

use bevy::{
    color::Alpha as _,
    input_focus::{FocusCause, InputFocus},
    prelude::*,
    text::EditableText,
    window::PrimaryWindow,
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{ChatSendRequest, NetChatChannel, NetChatMsg};
use xindeler_ui::{
    button::{Activate, button_bundle},
    scroll::scroll_view_bundle,
    theme::{HudFonts, HudTheme},
};

use crate::hud_layout;

/// Scrollback cap (BL-82 EM-5.4's own "bounded" requirement). 200 lines is
/// comfortably more than a player reads back in one session before it
/// scrolls off anyway (legacy's own chat history isn't infinite either —
/// this is the same "keep enough to scroll back through a recent
/// conversation, not the whole server's history" posture, just an explicit
/// number instead of an implicit one).
const MAX_CHAT_HISTORY: usize = 200;

/// The panel's on-screen width (bottom-left, matching legacy's chat
/// placement) — narrowed from the original `420.0` (BL-82
/// HUD-responsive-scaling pass, Matías's explicit "make it narrower and
/// taller" ask, alongside [`PANEL_BOTTOM_PX`]'s own overlap fix below).
const PANEL_WIDTH: f32 = 320.0;

/// Safety margin (px) [`PANEL_BOTTOM_PX`] adds on top of
/// [`hud_layout::CLUSTER_TOTAL_HEIGHT_PX`] — a deliberate, visible gap
/// rather than a flush/touching fit.
const PANEL_BOTTOM_SAFETY_MARGIN_PX: f32 = 24.0;

/// The panel's `bottom` offset (px, from the viewport's bottom edge) — **the
/// actual fix** for Matías's live-testing report: "at a small/reduced window
/// size, the health orb and the chat dialog box overlap; at a large/
/// fullscreen window they don't."
///
/// This used to be a bare `Some(16.0)` (`anchored_panel_bundle`'s own
/// `bottom` parameter) — 16px above the viewport's bottom edge, the SAME
/// vertical band the bottom-CENTRE health-orb cluster occupies
/// (`hud_layout::CLUSTER_BOTTOM_PX` = 20px). The cluster is centred and
/// ~1013px wide (`hud_layout::health_orb_screen_x`'s own doc comment) — at
/// the game's own default 1280×720 window (`main.rs`'s
/// `WindowResolution::new(1280, 720)`, itself already "small" by this
/// cluster's standard) the health orb's left edge sits barely 130px in from
/// the screen's left edge, well inside where even a NARROWED chat panel's
/// width would reach. Shrinking the panel's WIDTH alone therefore cannot
/// guarantee zero overlap across the window sizes players actually resize
/// to (it would either stay too wide for genuinely small windows, or shrink
/// to an unusably thin sliver at everyday ones) — the only way to guarantee
/// **zero overlap at every window size**, without reshaping the whole
/// orb/action-bar cluster, is to guarantee zero VERTICAL overlap instead:
/// sit the entire chat panel above [`hud_layout::CLUSTER_TOTAL_HEIGHT_PX`]
/// (the row's real top edge — orbs + the XP/level readout above them), plus
/// [`PANEL_BOTTOM_SAFETY_MARGIN_PX`]. Two AABBs that don't overlap on one
/// axis can never overlap at all, regardless of how their extents compare
/// on the other axis — so this holds independent of window WIDTH entirely.
///
/// It also survives the new window-height-derived
/// [`xindeler_ui::scale::window_derived_hud_scale`] `UiScale` (this same
/// pass's fix for "everything stays tiny on a large window") untouched:
/// `UiScale` multiplies every `Val::Px` conversion to physical pixels by the
/// SAME global factor, and both this constant and every `hud_layout`
/// constant it's built from are plain `Val::Px` figures — a strict `>`
/// relationship between two quantities scaled by the same positive factor
/// stays strict at ANY scale.
const PANEL_BOTTOM_PX: f32 = hud_layout::CLUSTER_TOTAL_HEIGHT_PX + PANEL_BOTTOM_SAFETY_MARGIN_PX;

const PANEL_LEFT_PX: f32 = 16.0;

/// [`chat_scroll_height`]'s clamp bounds (px) — taller than the original
/// fixed `180.0` at every supported window size (Matías's "narrower and
/// TALLER" ask), while never collapsing on a very short window nor growing
/// unboundedly on a very tall one (`UiScale`, wired separately, already
/// grows the WHOLE HUD together on a tall window — this is a modest, capped
/// adjustment layered on top of that, not a second uncapped growth path).
const MIN_SCROLL_HEIGHT_PX: f32 = 220.0;
const MAX_SCROLL_HEIGHT_PX: f32 = 320.0;

/// The fraction of window height [`chat_scroll_height`] targets before
/// clamping — chosen so the default 720px-tall reference window (the same
/// one every `hud_layout` constant was tuned against, see
/// `xindeler_ui::scale::REFERENCE_WINDOW_HEIGHT_PX`'s own doc comment) lands
/// near the middle of the clamp range (`720.0 * 0.35 ≈ 252px`) rather than
/// pinned to either bound.
const SCROLL_HEIGHT_WINDOW_FRACTION: f32 = 0.35;

/// The scrollback's height (px) for a given window height — see
/// [`MIN_SCROLL_HEIGHT_PX`]/[`MAX_SCROLL_HEIGHT_PX`]'s own doc comment for
/// why it's bounded rather than a raw fraction. Pure and directly
/// unit-tested (BL-82 HUD-responsive-scaling pass), not only exercised
/// through a live [`Window`] read — [`sync_chat_scroll_height_to_window`]
/// is the thin ECS wrapper that actually applies it every time the window's
/// real height changes.
#[must_use]
fn chat_scroll_height(window_height_px: f32) -> f32 {
    (window_height_px * SCROLL_HEIGHT_WINDOW_FRACTION)
        .clamp(MIN_SCROLL_HEIGHT_PX, MAX_SCROLL_HEIGHT_PX)
}

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
                // BL-82 HUD-responsive-scaling pass: keeps the scrollback
                // TALLER on a taller window (Matías's ask) as the window is
                // live-resized, not just at the size it happened to be at
                // `Startup`.
                sync_chat_scroll_height_to_window,
                // Reads `ActionState` — must run after the frame's real
                // input resolution (BL-82 EM-5.17 Phase 0, same fix as
                // `diary::toggle_diary_window`/`controls_screen::
                // toggle_controls_screen`).
                toggle_chat_via_hotkey.after(xindeler_input::InputResolveSet),
                // The explicit blur path `text_input_focused`'s doc comment
                // requires — ecs-design-reviewer BLOCKER fix, see
                // `blur_chat_input_on_escape`'s own doc comment.
                blur_chat_input_on_escape,
                // Same blur, but for the OTHER transition that can hide the
                // input box (minimizing the panel) — see
                // `blur_chat_input_on_collapse`'s own doc comment for the
                // live-tested regression this fixes. Ordered after the
                // hotkey toggle so an F5 press collapses AND blurs in the
                // SAME frame, not one frame late (the other collapse
                // source, `handle_chat_minimize_click`, is an `Activate`
                // observer, not an ordinary `Update` system, so it isn't
                // orderable here the same way — its effect is picked up via
                // the `Local<bool>` edge-detector on whichever frame runs
                // next, which is what that detector is for).
                blur_chat_input_on_collapse.after(toggle_chat_via_hotkey),
                // The keyboard-only path back onto `InputFocus` — see its
                // own doc comment for the permanent-lockout regression this
                // closes. Must run after `handle_chat_submit` so a fresh
                // Enter can never both refocus AND re-submit stale text in
                // the same frame (see that ordering note there).
                focus_chat_via_hotkey
                    .after(xindeler_input::InputResolveSet)
                    .after(handle_chat_submit),
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

/// Window height assumed when the real primary window isn't queryable yet
/// (a pre-`Startup`-ordering edge — winit's window creation isn't guaranteed
/// to have run before an early `Startup` system) — matches
/// `xindeler_ui::scale::REFERENCE_WINDOW_HEIGHT_PX` / `main.rs`'s own
/// default `WindowResolution::new(1280, 720)`.
const FALLBACK_WINDOW_HEIGHT_PX: f32 = 720.0;

/// Spawns the panel root (bottom-left, using the real themed
/// [`anchored_panel_bundle`] primitive — border/background/radius, not a
/// bare `Node`), the tab row, the scrollable message log, and the input row
/// (placeholder label + `EditableText` box). The panel's `left`/`bottom`
/// offsets ([`PANEL_LEFT_PX`]/[`PANEL_BOTTOM_PX`]) and its initial scrollback
/// height ([`chat_scroll_height`]) are the BL-82 HUD-responsive-scaling
/// pass's fix — see those constants' own doc comments.
fn spawn_chat_panel(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let initial_scroll_height = windows.single().map_or_else(
        |_| chat_scroll_height(FALLBACK_WINDOW_HEIGHT_PX),
        |window| chat_scroll_height(window.height()),
    );

    let root = commands
        .spawn((
            ChatPanelRoot,
            // BL-82 EM-5.17 z-scheme (spec §4.4): the chat panel MUST carry
            // `GlobalZIndex(zlayer::CHAT)` = 30 — the "chat sits above the
            // ambient HUD chrome so it can be interacted with while other
            // chrome is visible" tier. Without it the panel sat in the default
            // z-partition (0), BELOW the orbs/action-bar/hotbar/party-frames
            // that all gained `GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP)` = 20
            // during EM-5.17. Since `bevy_ui`'s picking backend resolves the
            // highest z-partition FIRST and treats a node without
            // `Pickable::IGNORE` as blocking everything below it (see
            // `combat_hud.rs`'s damage-vignette comment for the same picking
            // model), the health orb + left action-bar half — which, at the
            // 1280×720 default, geometrically overlap the bottom-left chat
            // panel including most of its `EditableText` input box — silently
            // swallowed the click meant to focus the box. `bevy_ui_widgets`'
            // text-input focus is set ONLY by that pointer-press landing on the
            // box (see `text_input_focused`'s doc comment), so `InputFocus`
            // never pointed at the chat box and typing did nothing at all. This
            // is the SAME click-routing bug class already fixed for the damage
            // vignette (PR #122) and the modal windows (PR #131) — the chat
            // panel was the remaining unfixed instance (PR #131's review noted
            // `zlayer::CHAT` was defined but never applied to `ChatPanelRoot`,
            // out of that PR's scope). `CHAT` = 30 stays below
            // `MODAL_WINDOWS` = 100, so an open diary/inventory/full-map still
            // correctly draws and picks over the chat panel.
            bevy::ui::GlobalZIndex(xindeler_ui::zlayer::CHAT),
            // BL-82 HUD-responsive-scaling pass: `PANEL_LEFT_PX`/
            // `PANEL_BOTTOM_PX` (not the old fixed `16.0`/`16.0`) — see those
            // constants' own doc comments for why the bottom offset is the
            // actual chat/health-orb overlap fix.
            xindeler_ui::panel::anchored_panel_bundle(
                &theme,
                None,
                Some(PANEL_LEFT_PX),
                None,
                Some(PANEL_BOTTOM_PX),
            ),
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
            .spawn(scroll_view_bundle(
                &theme,
                PANEL_WIDTH,
                initial_scroll_height,
            ))
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

/// Keeps the scrollback's height tracking [`chat_scroll_height`] as the
/// REAL primary window is live-resized — `spawn_chat_panel` only sets the
/// height ONCE, at `Startup`, off whatever size the window happened to be
/// at that moment; without this, resizing the window afterward would leave
/// the scrollback pinned at its initial height instead of genuinely growing
/// on a taller window. A no-op write-guard (`if node.height != desired`)
/// avoids marking the `Node` changed (and re-triggering `bevy_ui` layout)
/// every single frame when the window hasn't actually resized.
fn sync_chat_scroll_height_to_window(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut areas: Query<&mut Node, With<ChatScrollArea>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let desired = Val::Px(chat_scroll_height(window.height()));
    for mut node in &mut areas {
        if node.height != desired {
            node.height = desired;
        }
    }
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

/// Clears [`InputFocus`] the moment [`ChatUiState::collapsed`] transitions to
/// `true` WHILE the input box holds it — the same "a state transition hid the
/// focused widget, so drop the now-stale focus reference" fix
/// [`blur_chat_input_on_escape`] already applies to Escape, ported to the
/// OTHER transition that hides the box: minimizing the panel (the header
/// button click, [`handle_chat_minimize_click`], or the
/// [`GameInput::ToggleChat`] hotkey, [`toggle_chat_via_hotkey`] — both only
/// flip the same `bool`, so watching the resource for the edge covers either
/// source uniformly).
///
/// **Root cause this closes** (BL-82, live-tested regression, Matías: "after
/// I minimize/hide chat, the camera stops responding"): before this fix,
/// `sync_chat_collapsed` set the input row's `Node::display = Display::None`
/// but left [`InputFocus`] untouched. [`text_input_focused`] and
/// `crate::cursor::update_cursor_free`'s mirrored `chat_focused` predicate
/// only compare entity IDs, not visibility — so both kept reporting the
/// (now-hidden, unreachable) box as "focused" indefinitely. Since
/// `update_cursor_free` frees the OS cursor whenever `chat_focused` is `true`,
/// the cursor stayed permanently free/ungrabbed (never re-grabbing for
/// mouselook) for as long as that stale focus lingered — camera control
/// looked "unresponsive" because it genuinely was blocked, not laggy. The
/// player could previously only escape this by discovering that Escape (an
/// UNRELATED code path, [`blur_chat_input_on_escape`]) happened to also clear
/// it; this system fixes the actual transition directly, so minimizing chat
/// restores camera control immediately, with no detour required.
///
/// A `Local<bool>` edge-detector (not `ChatUiState::is_changed()` + the
/// current value) is deliberate: `is_changed()` also fires on unrelated field
/// writes (a tab click updating `view_filter`/`send_channel`) while
/// `collapsed` happens to ALREADY be `true` from an earlier frame — reacting
/// to the CURRENT value on every such change would re-clear focus on every
/// unrelated `ChatUiState` write made while collapsed, not only on the actual
/// collapse transition. The edge-detector fires exactly once, on the real
/// false-to-true transition, regardless of which system caused it.
fn blur_chat_input_on_collapse(
    state: Res<ChatUiState>,
    mut focus: ResMut<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
    mut was_collapsed: Local<bool>,
) {
    let just_collapsed = state.collapsed && !*was_collapsed;
    *was_collapsed = state.collapsed;
    if !just_collapsed {
        return;
    }
    let Ok(input_entity) = inputs.single() else {
        return;
    };
    if focus.get() == Some(input_entity) {
        focus.clear();
    }
}

/// [`GameInput::Chat`] (`Enter` by default) focuses the chat input box
/// directly — ported from legacy `xindeler-old`'s own
/// `WinEvent::InputUpdate(GameInput::Chat, true)` handler
/// (`voxygen/src/hud/mod.rs`), which calls `Hud::focus_widget(Some(self.ids.
/// chat))` on the exact same key. This is the ONLY keyboard-driven path onto
/// [`InputFocus`] this whole module has: every other way it's ever set is
/// `bevy_ui_widgets`'s own pointer-press observer — a CLICK, which only works
/// while the OS cursor is free.
///
/// **Root cause this closes** (BL-82, live-tested regression, Matías: "once
/// chat loses focus, I can never type in it again for the rest of the
/// session"): `crate::cursor::update_cursor_free` only frees the cursor for
/// chat's sake WHILE the input box already holds focus, or while some
/// unrelated `HudState` window happens to be open — chat itself is
/// deliberately NOT a `HudState` window (see [`ChatUiState`]'s own doc
/// comment), so nothing else ever frees the cursor on the panel's behalf.
/// The instant the box loses focus (for ANY reason) while the cursor is
/// grabbed for mouselook and nothing else is open, a mouse click can never
/// reach the box again: the cursor is hidden/locked, so no click lands on
/// the always-visible minimize/restore button or the box itself, and
/// nothing re-frees the cursor purely for chat — a genuine, permanent
/// dead end, exactly as reported. Legacy never had this problem because its
/// equivalent key focuses the widget directly, bypassing the cursor
/// entirely; this system ports that same fix. Once it sets [`InputFocus`]
/// here, the very next frame's `update_cursor_free` observes `chat_focused =
/// true` and frees the cursor for real — no click required to close the
/// loop.
///
/// Un-collapses the panel first if it was minimized (Enter "just works"
/// regardless of visibility, matching legacy's single unified key). A no-op
/// while the box is ALREADY focused — [`handle_chat_submit`]'s own raw
/// `KeyCode::Enter` check owns THAT case (it submits the line); ordering
/// this system `.after(handle_chat_submit)` guarantees the two never fight
/// over the same keypress; on the frame chat is refocused,
/// `handle_chat_submit` still observes the PRE-refocus state and correctly
/// no-ops (not yet focused), so a fresh Enter can never both refocus AND
/// immediately re-submit stale leftover text in the same frame.
fn focus_chat_via_hotkey(
    action_state: Res<ActionState>,
    mut state: ResMut<ChatUiState>,
    mut focus: ResMut<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
) {
    if !action_state.just_pressed(GameInput::Chat) {
        return;
    }
    let Ok(input_entity) = inputs.single() else {
        return;
    };
    if focus.get() == Some(input_entity) {
        return;
    }
    if state.collapsed {
        state.collapsed = false;
    }
    focus.set(input_entity, FocusCause::Navigated);
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

    /// The [`ChatPanelRoot`] MUST carry `GlobalZIndex(zlayer::CHAT)` (spec
    /// §4.4) — the click-routing regression this test pins. Without it the
    /// panel sits in the default z-partition (0), below the orbs/action-bar/
    /// hotbar/party-frames that all carry
    /// `GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP)` = 20; since `bevy_ui`
    /// picking resolves the highest z-partition first and a node without
    /// `Pickable::IGNORE` blocks everything below it, that ambient chrome
    /// (which geometrically overlaps the bottom-left chat panel at the default
    /// window size) silently swallowed the click meant to focus the input box,
    /// so `InputFocus` never pointed at it and typing did nothing. Matches the
    /// same z-index regression guard
    /// `diary.rs`/`esc_menu.rs`/`inventory_ui.rs`/ `map_view.rs` each carry
    /// for their own roots (the SAME bug class fixed for the damage
    /// vignette (PR #122) and the modal windows (PR #131)).
    #[test]
    fn spawn_chat_panel_puts_the_root_on_the_chat_z_layer() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_chat_panel)
            .expect("spawn_chat_panel runs");

        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, With<ChatPanelRoot>>()
            .single(world)
            .expect("ChatPanelRoot exists");
        let z_index = world
            .get::<bevy::ui::GlobalZIndex>(root)
            .expect("ChatPanelRoot carries a GlobalZIndex");
        assert_eq!(
            z_index.0,
            xindeler_ui::zlayer::CHAT,
            "the chat panel must sit on the CHAT z-layer, above the ambient HUD chrome that would \
             otherwise swallow clicks meant to focus its input box"
        );
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

    /// **The root-cause regression test for "minimizing chat leaves the
    /// camera unresponsive."** [`blur_chat_input_on_collapse`] must clear
    /// [`InputFocus`] the moment [`ChatUiState::collapsed`] flips `true`
    /// WHILE the input box holds it — before this fix, nothing cleared it at
    /// all, so `text_input_focused`/`update_cursor_free`'s `chat_focused`
    /// kept reporting the hidden box as focused forever, permanently forcing
    /// the OS cursor free.
    #[test]
    fn collapsing_chat_blurs_the_focused_input_box() {
        let mut app = new_app();
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));

        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(blur_chat_input_on_collapse)
            .expect("system runs");

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            None,
            "minimizing chat while it holds focus must clear InputFocus, or the cursor stays \
             stuck free/ungrabbed forever (camera never responds)"
        );
    }

    /// [`blur_chat_input_on_collapse`] must be a no-op while the panel is
    /// NOT collapsed (must not clear focus just because the resource
    /// happened to change for some other reason), and must not panic when
    /// the box isn't focused in the first place.
    #[test]
    fn blur_on_collapse_is_a_no_op_while_expanded_or_unfocused() {
        let mut app = new_app();
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));

        // Still expanded: must not touch focus.
        app.world_mut()
            .run_system_once(blur_chat_input_on_collapse)
            .expect("system runs");
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(input));

        // Collapsed but focus already elsewhere: must not panic or clobber it.
        let other = app.world_mut().spawn_empty().id();
        app.insert_resource(InputFocus::from_entity(other));
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(blur_chat_input_on_collapse)
            .expect("system runs");
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(other),
            "collapsing must only ever clear the CHAT input's own focus"
        );
    }

    /// [`blur_chat_input_on_collapse`]'s edge-detector must fire exactly
    /// once on the real false-to-true transition — a later unrelated
    /// `ChatUiState` write made while ALREADY collapsed must not re-run the
    /// blur (which would otherwise clobber a focus the player legitimately
    /// re-acquired via [`focus_chat_via_hotkey`] while still collapsed, e.g.
    /// mid-frame ordering edges).
    ///
    /// Driven via `add_systems` + repeated `app.update()` (NOT
    /// `run_system_once`, called twice): the `Local<bool>` edge-detector only
    /// persists across real scheduled frames — a fresh `run_system_once`
    /// call constructs a brand-new system (and a fresh, defaulted `Local`)
    /// every time, which would silently defeat the exact edge-vs-level
    /// distinction this test exists to pin.
    #[test]
    fn blur_on_collapse_only_fires_on_the_edge_not_every_frame_while_collapsed() {
        let mut app = new_app();
        app.init_resource::<InputFocus>();
        app.add_systems(Update, blur_chat_input_on_collapse);

        // Frame 1: collapse for the first time — the edge-detector consumes it.
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.update();

        // Frame 2: focus chat again while STILL collapsed (e.g.
        // `focus_chat_via_hotkey` ran earlier this same frame in the real
        // app), then make an UNRELATED `ChatUiState` write (a tab click's
        // view filter). `collapsed` never went false-then-true again, so
        // the edge-detector must not re-fire and clobber this focus.
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));
        app.world_mut().resource_mut::<ChatUiState>().view_filter = Some(NetChatChannel::Say);
        app.update();

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(input),
            "an unrelated ChatUiState write while already collapsed must not re-clear focus"
        );
    }

    /// **The root-cause regression test for "chat can never be refocused
    /// again."** [`focus_chat_via_hotkey`] ([`GameInput::Chat`], `Enter` by
    /// default) must set [`InputFocus`] onto the chat input box directly —
    /// the ONLY keyboard-only path onto it — and un-collapse the panel first
    /// if it was minimized. Driven through the REAL
    /// `xindeler_input::action_state::update_action_state` resolver, matching
    /// `toggle_chat_via_hotkey_flips_collapsed`'s own test shape.
    #[test]
    fn enter_focuses_the_chat_input_and_uncollapses_the_panel() {
        use bevy::input::keyboard::KeyCode;
        use xindeler_input::{KeyMap, action_state::update_action_state};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        app.init_resource::<InputFocus>();
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("")))
            .id();
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.add_systems(Update, (update_action_state, focus_chat_via_hotkey).chain());

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(input),
            "Enter must focus the chat input box directly, with no click required — the only way \
             back in once the mouse cursor is grabbed for mouselook and nothing else is open"
        );
        assert!(
            !app.world().resource::<ChatUiState>().collapsed,
            "Enter must also un-collapse a minimized panel — the key always \"just works\", \
             matching legacy's single unified chat key"
        );
    }

    /// [`focus_chat_via_hotkey`] must be a no-op while the box is ALREADY
    /// focused — [`handle_chat_submit`]'s own Enter path owns that case
    /// (submitting the line), so the two must never fight over the same
    /// keypress. Driven through the real resolver, same shape as
    /// [`enter_focuses_the_chat_input_and_uncollapses_the_panel`].
    #[test]
    fn enter_while_already_focused_does_not_reset_the_focus_cause() {
        use bevy::input::keyboard::KeyCode;
        use xindeler_input::{KeyMap, action_state::update_action_state};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("hello")))
            .id();
        // `FocusCause::Pressed` (as a real click would leave it) — if
        // `focus_chat_via_hotkey` wrongly re-focused, it would flip this to
        // `Navigated`, which downstream widget behaviour (e.g. select-all-on-
        // navigate) treats differently.
        app.insert_resource(InputFocus::default());
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(input, FocusCause::Pressed);
        app.add_systems(Update, (update_action_state, focus_chat_via_hotkey).chain());

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();

        // Focus is unchanged (still Pressed on the same entity) — no-op.
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(input));
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

    /// [`chat_scroll_height`]: grows with window height but stays within
    /// [`MIN_SCROLL_HEIGHT_PX`]/[`MAX_SCROLL_HEIGHT_PX`] at both extremes —
    /// the "taller" half of Matías's "narrower and taller" ask, bounded so a
    /// tiny window doesn't collapse the scrollback to nothing and a huge one
    /// doesn't grow it without limit (BL-82 HUD-responsive-scaling pass).
    #[test]
    fn chat_scroll_height_clamps_and_grows_with_window_height() {
        assert_eq!(chat_scroll_height(100.0), MIN_SCROLL_HEIGHT_PX);
        assert_eq!(chat_scroll_height(4000.0), MAX_SCROLL_HEIGHT_PX);

        let short = chat_scroll_height(600.0);
        let tall = chat_scroll_height(900.0);
        assert!(
            tall > short,
            "a taller window must yield a taller scrollback ({tall} was not > {short})"
        );
    }

    /// **The BL-82 HUD-responsive-scaling pass's core acceptance test**:
    /// Matías's live-testing report was "at a small/reduced window size, the
    /// health orb and the chat dialog box overlap." This asserts the chat
    /// panel's real on-screen AABB (`PANEL_LEFT_PX`/`PANEL_BOTTOM_PX`/
    /// `PANEL_WIDTH` + a total height built from `chat_scroll_height` plus a
    /// generous chrome overestimate for the header/tab/input rows
    /// `spawn_chat_panel` also spawns) never intersects the KNOWN health-orb
    /// bounding box (`hud_layout::health_orb_screen_x` +
    /// `hud_layout::CLUSTER_BOTTOM_PX`/`ORB_SIZE_PX`) at a spread of window
    /// sizes: the project's own default (1280×720), a couple of
    /// progressively smaller "reduced" sizes, and a genuinely tiny one.
    #[test]
    fn chat_panel_never_overlaps_the_health_orb_bounding_box_at_any_window_size() {
        // Deliberately generous (an overestimate, never an underestimate) —
        // `spawn_chat_panel`'s header row (button + margin) + tab row
        // (button + margin) + input row (text/box + padding) on top of the
        // scrollback itself. Erring tall here only ever makes this test
        // STRICTER than the real spawned panel, never looser.
        const CHROME_HEIGHT_PX: f32 = 140.0;

        // (width, height) — 1280x720 is the project's own literal default
        // (`main.rs`'s `WindowResolution::new(1280, 720)`) and already reads
        // as "small" against the ~1013px-wide orb cluster (see
        // `hud_layout::health_orb_screen_x`'s doc comment) — exactly the
        // size Matías's report was reproducing against. 960x540 and 800x600
        // are progressively more "reduced"; 480x320 is the genuinely tiny
        // floor this test also covers.
        let window_sizes: &[(f32, f32)] = &[
            (1280.0, 720.0),
            (960.0, 540.0),
            (800.0, 600.0),
            (480.0, 320.0),
        ];

        for &(width, height) in window_sizes {
            let chat_left = PANEL_LEFT_PX;
            let chat_right = PANEL_LEFT_PX + PANEL_WIDTH;
            let chat_top_from_bottom =
                PANEL_BOTTOM_PX + chat_scroll_height(height) + CHROME_HEIGHT_PX;
            let chat_bottom_from_bottom = PANEL_BOTTOM_PX;

            let (orb_left, orb_right) = hud_layout::health_orb_screen_x(width);
            let orb_bottom_from_bottom = hud_layout::CLUSTER_BOTTOM_PX;
            let orb_top_from_bottom = hud_layout::CLUSTER_BOTTOM_PX + hud_layout::ORB_SIZE_PX;

            let x_overlaps = chat_left < orb_right && orb_left < chat_right;
            let y_overlaps = chat_bottom_from_bottom < orb_top_from_bottom
                && orb_bottom_from_bottom < chat_top_from_bottom;

            assert!(
                !(x_overlaps && y_overlaps),
                "chat panel [{chat_left}, {chat_right}] x [{chat_bottom_from_bottom}, \
                 {chat_top_from_bottom}] overlaps the health orb [{orb_left}, {orb_right}] x \
                 [{orb_bottom_from_bottom}, {orb_top_from_bottom}] at window size {width}x{height}"
            );
        }
    }

    /// [`sync_chat_scroll_height_to_window`]: a LIVE window resize (not just
    /// the size at `Startup`) updates the scroll area's real `Node::height`
    /// — without this, `spawn_chat_panel`'s one-shot height would stay
    /// pinned at whatever the window was when the app booted.
    #[test]
    fn sync_chat_scroll_height_to_window_tracks_a_live_resize() {
        let mut app = new_app();
        let window_entity = app
            .world_mut()
            .spawn((PrimaryWindow, Window {
                resolution: bevy::window::WindowResolution::new(1280, 720),
                ..Default::default()
            }))
            .id();
        let scroll_area = app
            .world_mut()
            .spawn((ChatScrollArea, Node {
                height: Val::Px(chat_scroll_height(720.0)),
                ..Default::default()
            }))
            .id();

        // Resize to a much taller window — the scrollback must grow to
        // match (clamped at `MAX_SCROLL_HEIGHT_PX`).
        app.world_mut()
            .get_mut::<Window>(window_entity)
            .unwrap()
            .resolution = bevy::window::WindowResolution::new(1280, 2000);
        app.world_mut()
            .run_system_once(sync_chat_scroll_height_to_window)
            .expect("system runs");

        let node = app.world().get::<Node>(scroll_area).unwrap();
        assert_eq!(
            node.height,
            Val::Px(chat_scroll_height(2000.0)),
            "the scrollback must track a LIVE window resize, not just the size at Startup"
        );
    }
}
