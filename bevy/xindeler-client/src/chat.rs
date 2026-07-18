//! BL-82 EM-5.4 — the chat panel: a bounded scrollback + channel tabs +
//! a text input line, reading [`NetChatMsg`] (server → client) and writing
//! [`ChatSendRequest`] (client → server) — the two wire types
//! `xindeler-protocol::chat` defines (see that module's doc comment for the
//! full send/receive design).
//!
//! Compiled only under the `listen-server`/`net-client` cargo features (the
//! only modes where `xindeler-protocol`'s wire types are even linked),
//! matching every other consumer module in this crate (`combat_hud`,
//! `hud_toast`, …).
//!
//! ## Why the input line is a self-owned buffer, NOT `bevy_ui_widgets`' `EditableText`
//! (BL-82 chat rewrite — the FOURTH report of "I can't type in chat"):
//! the original port built the input on `bevy::text::EditableText`. Focusing it
//! (via `InputFocus`) *looked* correct, but typed characters never reached
//! `EditableText::value()` on macOS. Root cause: `bevy_ui_widgets`'
//! `EditableTextInputPlugin` flips `window.ime_enabled = true` the instant an
//! `EditableText` gains focus (its
//! `listen_for_ime_input_when_text_input_focused` system), which on macOS
//! routes every keystroke through the OS input method — and in-progress IME
//! composition text is **deliberately excluded** from `EditableText::value()`.
//! So the caret blinked, the box had focus, but the value stayed empty: exactly
//! the "focus looks right, can't type" symptom that survived three prior
//! InputFocus-bookkeeping patches (PRs #102/#141/#149), all of which only ever
//! touched *setting/clearing* `InputFocus`, never the keyboard→text delivery
//! underneath it. A live windowed harness reproduced it: `smoke-chat-focus FAIL
//! … a real KeyboardInput character never reached EditableText::value() while
//! focused`.
//!
//! The legacy pre-Bevy client (`xindeler-old`, `voxygen/src/hud/chat.rs`) never
//! had this problem because it **owns the input string itself**
//! (`state.input.message: String`), feeding it from raw key events and handing
//! it to the widget purely for display — focus is just "is chat capturing the
//! keyboard", entirely independent of the OS cursor or any IME. This module
//! ports that design: [`ChatInput`] owns the `String` + caret,
//! [`read_chat_input`] folds real [`KeyboardInput`] into it directly (no
//! `EditableText`, no `FocusedInput` dispatch, no IME — we never focus an
//! `EditableText`, so `ime_enabled` is never set), and
//! copy/cut/paste/select-all + standard editing keys are handled explicitly
//! against the engine's [`bevy::clipboard::Clipboard`] resource. [`InputFocus`]
//! is still used as the canonical "chat is focused" signal (so `cursor.rs` and
//! the hotkey-typing guard `text_input_focused` are unchanged), just pointed at
//! a plain `Text` node instead of an `EditableText`.
//!
//! ## Scope (v1, "the whole task in one PR")
//! - Bounded scrollback (`MAX_CHAT_HISTORY` lines — old lines AND their row
//!   entities are evicted, never growing unbounded).
//! - Channel tabs: **All** (view filter only) plus the five sendable channels
//!   (Say/Region/Group/Faction/World) plus **Whisper** (view filter only —
//!   sending a `Tell` is reachable by typing `/tell <alias> <message>`).
//! - Text input: Enter sends; a leading `/` sends a raw
//!   [`ChatSendRequest::Command`]; Tab cycles command-name completions; Up/Down
//!   recall sent-line history; Home/End/arrows/Backspace/Delete move and edit;
//!   Cmd/Ctrl+A/C/X/V select-all/copy/cut/paste.
//! - A simple `@mention` highlight (whole-row tint).
//!
//! ## Visual model — a faithful port of legacy `xindeler-old`'s chat LOOK
//! (BL-82 chat visual rebuild — Matías's "quedó rara, quiero que sea tal cual
//! el viejo" report). The legacy conrod widget (`voxygen/src/hud/chat.rs`) is
//! NOT an opaque bordered window with a permanent tab-button row and a
//! Hide/Chat button — that earlier Bevy approximation is what read as "weird".
//! Legacy is a **translucent black box** (`rgba(0,0,0,0.4)`, its `chat_opacity`
//! default) in the **bottom-left**, with **no frame/border art at all**. Each
//! message line carries a **16×16 colored channel icon** in a left gutter and
//! text **tinted by channel** (World/Say/Region/Group/Faction/Tell each their
//! own color, from `voxygen/src/hud/mod.rs`'s `*_COLOR` consts). The **input
//! line only styles up when focused** — a mode icon + a border colored to the
//! current chat mode over the same translucent black. **Channel tabs reveal on
//! hover/focus** (legacy fades them in only while the mouse is over the box);
//! there is **no minimize button** — visibility is purely the F5 keybind
//! toggle. This module reproduces that as closely as `bevy_ui` allows and
//! **reuses the legacy chat icon PNGs verbatim**
//! (`assets/voxygen/element/ui/chat/icons/*_small.png`, the real 16×16 art the
//! conrod widget referenced — NOT the HUD-D4 orb pack), loaded here via
//! [`ChatIcons`]. The input pipeline (owned buffer, focus, clipboard) is
//! unchanged from the PR #152 rewrite; only the LOOK/STRUCTURE changed.

use bevy::{
    color::Alpha as _,
    image::Image,
    input::keyboard::{Key, KeyboardInput},
    input_focus::{FocusCause, InputFocus},
    picking::events::{Click, Out, Over, Pointer},
    prelude::*,
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

/// The legacy chat box's translucent-black fill — `rgba(0,0,0,0.4)`, matching
/// `xindeler-old`'s `ChatSettings::chat_opacity` default (`0.4`). Deliberately
/// NOT the theme's `panel_bg` (that opaque dark-violet panel fill is what made
/// the earlier port read as a heavy "window"); legacy chat is a light
/// see-through overlay.
///
/// TODO (post look-parity): legacy `chat_opacity`/`chat_size_x` are
/// user-configurable `ChatSettings` fields; this pass bakes their defaults as
/// consts for LOOK parity. True parity (an opacity slider / a resizable box)
/// will need [`CHAT_BG`]/[`PANEL_WIDTH`] to read from a settings resource/RON
/// rather than a `const`.
const CHAT_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.4);

/// The per-channel message text colors, copied verbatim from legacy
/// `voxygen/src/hud/mod.rs`'s `WORLD_COLOR`/`SAY_COLOR`/… `const`s (the source
/// of truth for how each channel looks in the legacy chat box).
const WORLD_COLOR: Color = Color::srgba(0.95, 1.0, 0.95, 1.0);
const SAY_COLOR: Color = Color::srgba(1.0, 0.8, 0.8, 1.0);
const REGION_COLOR: Color = Color::srgba(0.8, 1.0, 0.8, 1.0);
const GROUP_COLOR: Color = Color::srgba(0.47, 0.84, 1.0, 1.0);
const FACTION_COLOR: Color = Color::srgba(0.24, 1.0, 0.48, 1.0);
const TELL_COLOR: Color = Color::srgba(0.98, 0.71, 1.0, 1.0);
/// Legacy `INFO_COLOR` — reused for our `System`/`Npc` channels (legacy shows
/// command-info/meta lines in this teal).
const INFO_COLOR: Color = Color::srgba(0.28, 0.83, 0.71, 1.0);

/// The channel-icon left-gutter width/height (px) — legacy's
/// `CHAT_ICON_WIDTH`/`CHAT_ICON_HEIGHT` (both `16.0`).
const CHAT_ICON_PX: f32 = 16.0;

/// The focused input line's mode-colored border thickness (px) — legacy draws
/// a 2px line border (`CHAT_MARGIN_THICKNESS`) around the focused input.
const INPUT_BORDER_PX: f32 = 2.0;

/// Scrollback cap (BL-82 EM-5.4's own "bounded" requirement).
const MAX_CHAT_HISTORY: usize = 200;

/// Sent-line recall history cap ([`ChatInput::history`]) — Up/Down cycles at
/// most this many recently-sent lines, matching legacy `xindeler-old`'s own
/// `history_max` (32).
const CHAT_INPUT_HISTORY_MAX: usize = 32;

/// The panel's on-screen width (bottom-left, matching legacy's chat placement).
/// Legacy's `DEFAULT_CHAT_BOX_WIDTH` is `470.0`; matched here.
const PANEL_WIDTH: f32 = 470.0;

/// The input line's minimum height (px) so the box is always a visible,
/// clickable rectangle even while its text is empty — without an explicit
/// floor a `Text` node with no glyphs collapses to ~0px and can neither be
/// seen nor clicked.
const INPUT_MIN_HEIGHT_PX: f32 = 26.0;

const PANEL_LEFT_PX: f32 = 16.0;

/// The panel's `bottom` offset (px) when it can sit flush in the true
/// bottom-LEFT corner — i.e. whenever [`chat_panel_needs_lift`] says the
/// health-orb cluster doesn't reach into the panel's own `x` span at the
/// current window width. Matches [`PANEL_LEFT_PX`] for a visually symmetric
/// corner margin (BL-82 chat-panel-polish pass — Matías's "sit in the
/// bottom-left corner" ask, given the upcoming action-bar-frame removal).
const PANEL_BOTTOM_CORNER_PX: f32 = PANEL_LEFT_PX;

/// Safety margin (px) [`PANEL_BOTTOM_LIFTED_PX`] adds on top of
/// [`hud_layout::CLUSTER_TOTAL_HEIGHT_PX`].
const PANEL_BOTTOM_SAFETY_MARGIN_PX: f32 = 24.0;

/// The panel's `bottom` offset (px) in the FALLBACK case — sits the whole
/// panel above the bottom-centre health-orb cluster so the two AABBs never
/// overlap on the Y axis, the same "clear the whole row's height, regardless
/// of X" strategy this constant used unconditionally before the corner-polish
/// pass (BL-82 HUD-responsive-scaling fix, PR #138). Still load-bearing: see
/// [`chat_panel_needs_lift`]'s own doc comment for why plain corner-flush
/// placement can't be used at every window width.
const PANEL_BOTTOM_LIFTED_PX: f32 =
    hud_layout::CLUSTER_TOTAL_HEIGHT_PX + PANEL_BOTTOM_SAFETY_MARGIN_PX;

/// Window width assumed when the real primary window isn't queryable yet (a
/// pre-`Startup`-ordering edge) — matches [`crate::smoke::TARGET_SIZE`]'s
/// default window width, the same convention [`FALLBACK_WINDOW_HEIGHT_PX`]
/// already follows for height.
const FALLBACK_WINDOW_WIDTH_PX: f32 = 1280.0;

/// Whether the chat panel's `x` span ([`PANEL_LEFT_PX`] to `+ `[`PANEL_WIDTH`])
/// would overlap the health orb's own `x` span at `window_width_px` — i.e.
/// whether [`chat_panel_bottom`] must fall back to lifting the whole panel
/// above the cluster ([`PANEL_BOTTOM_LIFTED_PX`]) instead of sitting flush in
/// the true corner ([`PANEL_BOTTOM_CORNER_PX`]).
///
/// BL-82 chat-panel-polish pass: moving the panel down to the true
/// bottom-left corner (Matías's ask) puts its `y` span in the SAME band as
/// the health-orb row's — a purely vertical "sit above the cluster" strategy
/// (PR #138's original [`PANEL_BOTTOM_LIFTED_PX`]-unconditionally fix) no
/// longer keeps them apart on its own, so this checks `x` instead, via
/// [`hud_layout::health_orb_screen_x`] (promoted out of `#[cfg(test)]` for
/// exactly this real call site).
///
/// This can't be dropped in favour of ALWAYS trusting `x`-separation, though:
/// the health orb's own `x` position is `window_width / 2.0 +
/// CLUSTER.health_orb_left`, a NEGATIVE-ish constant offset from screen
/// centre, so it slides right as the window widens — meaning there is a real
/// width BAND (empirically, roughly `1010`–`1970px` given today's cluster
/// geometry, which covers ordinary desktop resolutions like `1280×720` and
/// `1920×1080`) where the orb's `x` span genuinely reaches into the panel's.
/// Below that band the orb is off-screen-left (safe); above it the orb has
/// slid clear to the right (safe) — but the band itself is real and common,
/// so the [`PANEL_BOTTOM_LIFTED_PX`] fallback stays load-bearing, not dead
/// weight, and this function (not a flat "always use the corner" swap) is
/// the actual fix. See
/// `chat_panel_never_overlaps_the_health_orb_bounding_box_at_any_window_size`
/// for the regression test sweeping a wide range of widths.
#[must_use]
fn chat_panel_needs_lift(window_width_px: f32) -> bool {
    let (orb_left, orb_right) = hud_layout::health_orb_screen_x(window_width_px);
    let chat_left = PANEL_LEFT_PX;
    let chat_right = PANEL_LEFT_PX + PANEL_WIDTH;
    chat_left < orb_right && orb_left < chat_right
}

/// The panel's `bottom` offset (px) for `window_width_px` — the true corner
/// margin ([`PANEL_BOTTOM_CORNER_PX`]) whenever that's safe, else the
/// cluster-clearing fallback ([`PANEL_BOTTOM_LIFTED_PX`]). See
/// [`chat_panel_needs_lift`]'s doc comment for the full reasoning.
#[must_use]
fn chat_panel_bottom(window_width_px: f32) -> f32 {
    if chat_panel_needs_lift(window_width_px) {
        PANEL_BOTTOM_LIFTED_PX
    } else {
        PANEL_BOTTOM_CORNER_PX
    }
}

/// [`chat_scroll_height`]'s clamp bounds (px).
const MIN_SCROLL_HEIGHT_PX: f32 = 220.0;
const MAX_SCROLL_HEIGHT_PX: f32 = 320.0;

/// The fraction of window height [`chat_scroll_height`] targets before
/// clamping.
const SCROLL_HEIGHT_WINDOW_FRACTION: f32 = 0.35;

/// The scrollback's height (px) for a given window height, bounded so a tiny
/// window doesn't collapse it and a huge one doesn't grow it without limit.
#[must_use]
fn chat_scroll_height(window_height_px: f32) -> f32 {
    (window_height_px * SCROLL_HEIGHT_WINDOW_FRACTION)
        .clamp(MIN_SCROLL_HEIGHT_PX, MAX_SCROLL_HEIGHT_PX)
}

/// The channel tabs shown, in order: `None` = "All" (view filter only); every
/// `Some(channel)` doubles as a filter AND (for the five sendable kinds) a
/// send-channel selector. `Tell` is a VIEW filter only.
const CHAT_TABS: [(Option<NetChatChannel>, &str); 7] = [
    (None, "All"),
    (Some(NetChatChannel::Say), "Say"),
    (Some(NetChatChannel::Region), "Region"),
    (Some(NetChatChannel::Group), "Group"),
    (Some(NetChatChannel::Faction), "Faction"),
    (Some(NetChatChannel::World), "World"),
    (Some(NetChatChannel::Tell), "Whisper"),
];

/// The legacy per-channel text color for a scrollback line / the input line's
/// current mode — the `*_COLOR` mapping `render_chat_mode`/`render_chat_line`
/// use in `xindeler-old/voxygen/src/hud/chat.rs`.
#[must_use]
fn channel_color(channel: NetChatChannel) -> Color {
    match channel {
        NetChatChannel::World => WORLD_COLOR,
        NetChatChannel::Say => SAY_COLOR,
        NetChatChannel::Region => REGION_COLOR,
        NetChatChannel::Group => GROUP_COLOR,
        NetChatChannel::Faction => FACTION_COLOR,
        NetChatChannel::Tell => TELL_COLOR,
        // Legacy filters NPC lines out of the box entirely and shows meta/system
        // lines in INFO teal; our protocol surfaces both as real channels, so
        // give them the teal too.
        NetChatChannel::Npc | NetChatChannel::System => INFO_COLOR,
    }
}

/// Slash-command names Tab-completion cycles through.
const KNOWN_COMMANDS: &[&str] = &["say", "region", "group", "faction", "world", "tell", "w"];

/// The chat panel's live UI state: the view filter, the next plain-line send
/// channel, and whether the panel is minimized. `collapsed` is session-only
/// (not persisted) for v1.
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

/// The self-owned chat input line — the heart of the rewrite (see the module
/// doc comment for why we don't lean on `EditableText`). Holds the text being
/// typed, the caret, a small "select-all" flag, sent-line recall history, and
/// the Tab-completion cursor. [`read_chat_input`] mutates it from real
/// [`KeyboardInput`]; [`render_chat_input`] mirrors it into the on-screen
/// [`ChatInputBox`] `Text`.
#[derive(Resource, Debug, Default)]
struct ChatInput {
    /// The current line (UTF-8). The caret [`cursor`](Self::cursor) is a byte
    /// index into this that always lands on a `char` boundary.
    buffer: String,
    /// Caret byte-offset into [`buffer`](Self::buffer) (`0..=buffer.len()`).
    cursor: usize,
    /// Single-line "select all" flag (Cmd/Ctrl+A). Any caret move or a fresh
    /// insert/paste clears it; while set, Copy/Cut act on the whole line and
    /// the next insert/paste replaces it. A pragmatic single-line stand-in for
    /// a full selection model (v1) — enough for the practical
    /// select-all→copy / select-all→type gestures.
    select_all: bool,
    /// Sent lines, oldest-first, for Up/Down recall.
    history: Vec<String>,
    /// Cursor into [`history`](Self::history) while recalling (`None` = editing
    /// a fresh line, not browsing history).
    history_pos: Option<usize>,
    /// Tab-completion cycle index.
    completion_cycle: usize,
}

impl ChatInput {
    /// Empties the line and resets every edit-cursor. Used after a send and by
    /// Cmd/Ctrl+X on a whole-line selection.
    fn clear_line(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.select_all = false;
        self.history_pos = None;
        self.completion_cycle = 0;
    }

    /// Replaces the whole line with `s`, caret at the end. Does NOT touch
    /// `history_pos` (history recall calls this while still browsing).
    fn set_line(&mut self, s: String) {
        self.cursor = s.len();
        self.buffer = s;
        self.select_all = false;
        self.completion_cycle = 0;
    }

    /// Inserts `s` at the caret (replacing the whole line first if a select-all
    /// is pending), filtering out control characters and collapsing newlines to
    /// spaces (this is a single-line field).
    fn insert_str(&mut self, s: &str) {
        if self.select_all {
            self.buffer.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        let filtered: String = s
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        if filtered.is_empty() {
            return;
        }
        self.buffer.insert_str(self.cursor, &filtered);
        self.cursor += filtered.len();
        self.history_pos = None;
        self.completion_cycle = 0;
    }

    /// Deletes the character before the caret (or the whole line if a
    /// select-all is pending).
    fn backspace(&mut self) {
        if self.select_all {
            self.clear_line();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let prev_len = self.buffer[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        let start = self.cursor - prev_len;
        self.buffer.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.completion_cycle = 0;
    }

    /// Deletes the character after the caret (or the whole line if a select-all
    /// is pending).
    fn delete(&mut self) {
        if self.select_all {
            self.clear_line();
            return;
        }
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next_len = self.buffer[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.buffer
            .replace_range(self.cursor..self.cursor + next_len, "");
        self.completion_cycle = 0;
    }

    /// Moves the caret one character left.
    fn move_left(&mut self) {
        self.select_all = false;
        if self.cursor > 0 {
            let prev_len = self.buffer[..self.cursor]
                .chars()
                .next_back()
                .map_or(0, char::len_utf8);
            self.cursor -= prev_len;
        }
    }

    /// Moves the caret one character right.
    fn move_right(&mut self) {
        self.select_all = false;
        if self.cursor < self.buffer.len() {
            let next_len = self.buffer[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
            self.cursor += next_len;
        }
    }

    /// Moves the caret to the start of the line.
    fn home(&mut self) {
        self.select_all = false;
        self.cursor = 0;
    }

    /// Moves the caret to the end of the line.
    fn end(&mut self) {
        self.select_all = false;
        self.cursor = self.buffer.len();
    }

    /// The text a Copy/Cut should place on the clipboard: the whole line (the
    /// single-line select-all model — see [`select_all`](Self::select_all)).
    fn selection_text(&self) -> String { self.buffer.clone() }
}

#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatPanelRoot;
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatScrollArea;
/// The on-screen input line. `pub(crate)` because [`text_input_focused`] (a
/// hotkey-typing guard other modules import) and `cursor.rs` both key off
/// `With<ChatInputBox>`. It is a plain `Text` node (NOT an `EditableText`) —
/// see the module doc comment.
#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct ChatInputBox;
/// The 16×16 chat-mode icon shown at the left of the input line while focused
/// — the Bevy analogue of legacy's `chat_input_icon` (`render_chat_mode`).
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatInputModeIcon;
/// The compact channel-tab strip, revealed only while the chat box is hovered
/// or focused (legacy fades its tabs in on hover). Tagged so
/// [`reveal_chat_tabs`] can toggle its whole row's `Display`.
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatTabRow;
/// The per-line 16×16 channel-icon [`ImageNode`] (legacy's left-gutter chat
/// icon).
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatRowIcon;
/// The per-line message [`Text`] node (the text child of a [`ChatRow`]
/// container). Tagged so the scrollback text is queryable independently of the
/// row container + its icon sibling.
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatRowText;
/// Tags every element that hides while [`ChatUiState::collapsed`] — the
/// scrollback, the tab strip, and the input row. Collapsing hides the whole
/// box (legacy's F5 toggle fully hides chat; there is no minimize BUTTON).
///
/// [`sync_chat_collapsed`] toggles these via `Node::display`
/// (`Display::None`/`Flex`), NOT `Visibility::Hidden`, so a collapsed panel
/// genuinely shrinks (a hidden node keeps its layout footprint). Do NOT swap
/// this to `Visibility`.
#[derive(Component, Debug, Clone, Copy, Default)]
struct ChatCollapsible;
/// Tags a spawned chat-line row with its channel, so [`apply_chat_filter`] can
/// toggle its visibility without re-reading transient [`NetChatMsg`] history.
#[derive(Component, Debug, Clone, Copy)]
struct ChatRow(NetChatChannel);
/// Tags a tab button with which channel it selects (`None` = "All").
#[derive(Component, Debug, Clone, Copy)]
struct ChatTab(Option<NetChatChannel>);

/// FIFO of spawned [`ChatRow`] entities, oldest-first — the bookkeeping
/// [`ingest_chat_messages`] needs to evict/despawn the oldest past
/// [`MAX_CHAT_HISTORY`].
#[derive(Resource, Debug, Default)]
struct ChatHistory(Vec<Entity>);

/// Whether the chat box is currently hovered by the pointer — set by the
/// [`Pointer<Over>`]/[`Pointer<Out>`] observers on the [`ChatScrollArea`]
/// message box (NOT the transparent [`ChatPanelRoot`]: hover must track the
/// opaque box the pointer is actually over, and bubbling keeps it hovered while
/// the pointer is on any message row inside) and read by [`reveal_chat_tabs`]
/// to fade the channel-tab strip in on hover (legacy only shows its tabs while
/// the mouse is over the box). Default `false`.
#[derive(Resource, Debug, Default, Clone, Copy)]
struct ChatHovered(bool);

/// The real legacy chat-icon PNGs (`assets/voxygen/element/ui/chat/icons/`),
/// loaded once at `Startup` — the SAME 16×16 art the conrod widget referenced
/// (`chat_world_small` = `world_small.png`, etc.), reused verbatim rather than
/// the HUD-D4 orb pack. One `Handle<Image>` per sendable/viewable channel.
#[derive(Resource, Debug, Clone)]
struct ChatIcons {
    world: Handle<Image>,
    say: Handle<Image>,
    region: Handle<Image>,
    group: Handle<Image>,
    faction: Handle<Image>,
    tell: Handle<Image>,
    /// Legacy `command_info_small` — used for `System`/`Npc` lines.
    info: Handle<Image>,
}

impl ChatIcons {
    /// The legacy chat-icon asset directory (relative to `VELOREN_ASSETS`).
    const DIR: &'static str = "voxygen/element/ui/chat/icons";

    /// Loads every channel's icon handle via the [`AssetServer`]
    /// (hot-reloadable in dev, same as every other asset this workspace loads).
    fn load(asset_server: &AssetServer) -> Self {
        let load = |name: &str| asset_server.load(format!("{}/{name}", Self::DIR));
        Self {
            world: load("world_small.png"),
            say: load("say_small.png"),
            region: load("region_small.png"),
            group: load("group_small.png"),
            faction: load("faction_small.png"),
            tell: load("tell_small.png"),
            info: load("command_info_small.png"),
        }
    }

    /// Test-only constructor: every channel maps to a default (invalid but
    /// real) `Handle<Image>`, so tests that spawn the panel / ingest rows don't
    /// need a live `AssetServer`.
    #[cfg(test)]
    fn dummy() -> Self {
        Self {
            world: Handle::default(),
            say: Handle::default(),
            region: Handle::default(),
            group: Handle::default(),
            faction: Handle::default(),
            tell: Handle::default(),
            info: Handle::default(),
        }
    }

    /// The icon handle for a channel — mirrors legacy's
    /// `render_chat_mode`/`render_chat_line` icon mapping.
    #[must_use]
    fn for_channel(&self, channel: NetChatChannel) -> Handle<Image> {
        match channel {
            NetChatChannel::World => self.world.clone(),
            NetChatChannel::Say => self.say.clone(),
            NetChatChannel::Region => self.region.clone(),
            NetChatChannel::Group => self.group.clone(),
            NetChatChannel::Faction => self.faction.clone(),
            NetChatChannel::Tell => self.tell.clone(),
            NetChatChannel::Npc | NetChatChannel::System => self.info.clone(),
        }
    }
}

/// `Startup` system inserting [`ChatIcons::load`] before [`spawn_chat_panel`]
/// (which reads it for the input line's mode icon).
fn init_chat_icons(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ChatIcons::load(&asset_server));
}

/// Installs the whole chat panel.
pub struct ChatViewPlugin;

impl Plugin for ChatViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<NetChatMsg>();
        app.add_message::<ChatSendRequest>();
        app.init_resource::<ChatUiState>();
        app.init_resource::<ChatHistory>();
        app.init_resource::<ChatInput>();
        app.init_resource::<ChatHovered>();
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        app.add_systems(
            Startup,
            (
                init_chat_icons,
                spawn_chat_panel
                    .after(xindeler_ui::theme::init_theme)
                    .after(init_chat_icons),
                force_collapse_chat_for_smoke_capture,
            ),
        );
        app.add_systems(
            Update,
            (
                seed_chat_for_smoke_capture,
                ingest_chat_messages.after(seed_chat_for_smoke_capture),
                apply_chat_filter,
                sync_chat_tabs,
                reveal_chat_tabs,
                chat_smoke_verify,
                sync_chat_scroll_height_to_window,
                sync_chat_panel_bottom_to_window,
                // Reads `ActionState` — after the frame's real input resolution.
                toggle_chat_via_hotkey.after(xindeler_input::InputResolveSet),
                blur_chat_input_on_escape,
                blur_chat_input_on_collapse.after(toggle_chat_via_hotkey),
                // The keyboard→buffer core. Runs BEFORE `focus_chat_via_hotkey`
                // so that on the exact frame chat gains focus (via Enter), this
                // system — still observing the pre-focus state — drains that
                // Enter instead of typing/submitting it.
                read_chat_input,
                // The keyboard-only path onto `InputFocus`.
                focus_chat_via_hotkey
                    .after(xindeler_input::InputResolveSet)
                    .after(read_chat_input),
                // Mirror the owned buffer into the on-screen input line.
                render_chat_input.after(read_chat_input),
                // Legacy-style mode-colored border/icon on the input, only
                // while focused.
                sync_chat_input_style.after(read_chat_input),
                sync_chat_collapsed
                    .after(toggle_chat_via_hotkey)
                    .after(focus_chat_via_hotkey),
                chat_focus_smoke_verify
                    .after(focus_chat_via_hotkey)
                    .after(toggle_chat_via_hotkey)
                    .after(read_chat_input)
                    .after(blur_chat_input_on_collapse)
                    .after(blur_chat_input_on_escape)
                    .after(sync_chat_collapsed)
                    .after(ingest_chat_messages),
            ),
        );
    }
}

/// SCAFFOLDING for automated live verification of the network round trip
/// (BL-82 EM-5.4), gated by `XINDELER_SMOKE_CHAT_LINE`: writes one
/// [`ChatSendRequest`] and watches the REAL scrollback for the line to come
/// back — proving the full client-send → embedded sim → broadcast →
/// [`NetChatMsg`] → [`ingest_chat_messages`] path works. Exits via `AppExit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ChatSmokeStage {
    #[default]
    WaitingForPlayer,
    Sent,
    Done,
}

const CHAT_SMOKE_TIMEOUT_FRAMES: u32 = 1800;

fn chat_smoke_verify(
    mut configured_line: Local<Option<Option<String>>>,
    mut stage: Local<ChatSmokeStage>,
    mut frames_since_sent: Local<u32>,
    local_player: Query<(), With<xindeler_protocol::NetLocalPlayer>>,
    rows: Query<&Text, With<ChatRowText>>,
    mut send: MessageWriter<ChatSendRequest>,
    mut exit: MessageWriter<AppExit>,
) {
    let line =
        configured_line.get_or_insert_with(|| std::env::var("XINDELER_SMOKE_CHAT_LINE").ok());
    let Some(line) = line else {
        return;
    };

    match *stage {
        ChatSmokeStage::WaitingForPlayer => {
            if local_player.iter().next().is_some() {
                send.write(ChatSendRequest::Channel {
                    channel: NetChatChannel::World,
                    text: line.clone(),
                });
                info!(line = %line, "smoke-chat: sent the scripted line, waiting for the round trip");
                *stage = ChatSmokeStage::Sent;
            }
        },
        ChatSmokeStage::Sent => {
            *frames_since_sent += 1;
            if rows.iter().any(|text| text.0.contains(line.as_str())) {
                info!(line = %line, "smoke-chat: PASS — the scripted line round-tripped into the scrollback");
                exit.write(AppExit::Success);
                *stage = ChatSmokeStage::Done;
            } else if *frames_since_sent > CHAT_SMOKE_TIMEOUT_FRAMES {
                error!(line = %line, "smoke-chat: FAIL — the scripted line never appeared in the scrollback");
                exit.write(AppExit::error());
                *stage = ChatSmokeStage::Done;
            }
        },
        ChatSmokeStage::Done => {},
    }
}

/// SCAFFOLDING for automated live verification of the KEYBOARD-INPUT lifecycle
/// (BL-82 chat rewrite). Gated by `XINDELER_SMOKE_CHAT_FOCUS`.
///
/// This is the harness that FIRST reproduced the "focus looks right, can't
/// type" bug against a real window (the old version asserted
/// `EditableText::value()`; it failed because IME swallowed the keystrokes —
/// see the module doc comment). The rewritten harness asserts the new
/// [`ChatInput::buffer`] instead, driving the exact production path a genuine
/// keypress takes now: `bevy_winit` `KeyboardInput` → [`read_chat_input`] →
/// [`ChatInput`]. It scripts:
/// 1. Seed one scrollback line.
/// 2. Enter ([`GameInput::Chat`]) → assert [`InputFocus`] lands on the input
///    box.
/// 3. Type `hi` one real `KeyboardInput` at a time → assert the buffer accrues
///    it.
/// 4. F5 ([`GameInput::ToggleChat`]) → assert every [`ChatCollapsible`] hides
///    AND the seeded row + typed text both survive (collapse hides, never
///    discards).
/// 5. Enter again → assert the panel re-expands AND `InputFocus` returns with
///    no click.
/// 6. Type `yo` → assert it APPENDS (proving the refocused box is genuinely
///    typable).
///
/// Logs PASS/FAIL and exits via `AppExit`.
const CHAT_FOCUS_SMOKE_WORD_1: &str = "hi";
const CHAT_FOCUS_SMOKE_WORD_2: &str = "yo";
const CHAT_FOCUS_SMOKE_TIMEOUT_FRAMES: u32 = 1800;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ChatFocusSmokeStage {
    #[default]
    WaitForPanel,
    SeedHistory,
    AwaitHistorySeeded,
    PressEnter,
    AwaitFocused,
    TypeWord1(usize),
    AwaitWord1Char(usize),
    PressF5,
    AwaitCollapsed,
    PressEnterAgain,
    AwaitReexpanded,
    TypeWord2(usize),
    AwaitWord2Char(usize),
    Pass,
    Done,
}

/// Maps the lowercase ASCII letters the two test words use to physical
/// [`KeyCode`]s — sufficient for this harness's fixed script.
fn key_code_for_ascii_lowercase(c: char) -> Option<KeyCode> {
    use KeyCode as K;
    Some(match c {
        'a' => K::KeyA,
        'e' => K::KeyE,
        'h' => K::KeyH,
        'i' => K::KeyI,
        'o' => K::KeyO,
        'y' => K::KeyY,
        _ => return None,
    })
}

/// Writes a real press-then-release [`KeyboardInput`] PAIR — the exact shape
/// `bevy_winit` constructs from a genuine OS key tap — so it flows through the
/// REAL production keyboard pipeline. Both edges are sent (never a bare
/// `Pressed`) because `ButtonInput::press` only raises `just_pressed` on a true
/// false→true edge; a never-released key produces no second edge.
fn send_real_key_press(
    keyboard: &mut MessageWriter<KeyboardInput>,
    window: Entity,
    key_code: KeyCode,
    logical_key: Key,
    text: Option<&str>,
) {
    keyboard.write(KeyboardInput {
        key_code,
        logical_key: logical_key.clone(),
        state: bevy::input::ButtonState::Pressed,
        text: text.map(Into::into),
        repeat: false,
        window,
    });
    keyboard.write(KeyboardInput {
        key_code,
        logical_key,
        state: bevy::input::ButtonState::Released,
        text: None,
        repeat: false,
        window,
    });
}

const AWAIT_COLLAPSED_DISPLAY_TOLERANCE_FRAMES: u32 = 10;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn chat_focus_smoke_verify(
    mut enabled: Local<Option<bool>>,
    mut stage: Local<ChatFocusSmokeStage>,
    mut frames_waited: Local<u32>,
    mut collapse_display_wait: Local<u32>,
    windows: Query<Entity, With<PrimaryWindow>>,
    inputs: Query<Entity, With<ChatInputBox>>,
    collapsible: Query<&Node, With<ChatCollapsible>>,
    chat: Res<ChatInput>,
    history: Res<ChatHistory>,
    focus: Res<InputFocus>,
    state: Res<ChatUiState>,
    mut keyboard: MessageWriter<KeyboardInput>,
    mut net_chat: MessageWriter<NetChatMsg>,
    mut exit: MessageWriter<AppExit>,
) {
    let enabled = *enabled
        .get_or_insert_with(|| std::env::var("XINDELER_SMOKE_CHAT_FOCUS").is_ok_and(|v| v != "0"));
    if !enabled {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };

    let fail = |exit: &mut MessageWriter<AppExit>, stage: &ChatFocusSmokeStage, why: &str| {
        error!(?stage, why, "smoke-chat-focus: FAIL");
        exit.write(AppExit::error());
    };

    *frames_waited += 1;
    if *frames_waited > CHAT_FOCUS_SMOKE_TIMEOUT_FRAMES && *stage != ChatFocusSmokeStage::Done {
        fail(&mut exit, &stage, "timed out mid-sequence");
        *stage = ChatFocusSmokeStage::Done;
        return;
    }

    let Ok(input_entity) = inputs.single() else {
        return;
    };

    match *stage {
        ChatFocusSmokeStage::WaitForPanel => {
            *stage = ChatFocusSmokeStage::SeedHistory;
        },
        ChatFocusSmokeStage::SeedHistory => {
            net_chat.write(NetChatMsg {
                channel: NetChatChannel::World,
                sender_uid: None,
                sender_alias: None,
                text: "smoke-chat-focus seeded history line".to_owned(),
            });
            *stage = ChatFocusSmokeStage::AwaitHistorySeeded;
        },
        ChatFocusSmokeStage::AwaitHistorySeeded => {
            if history.0.is_empty() {
                return;
            }
            *stage = ChatFocusSmokeStage::PressEnter;
        },
        ChatFocusSmokeStage::PressEnter => {
            send_real_key_press(&mut keyboard, window, KeyCode::Enter, Key::Enter, None);
            *stage = ChatFocusSmokeStage::AwaitFocused;
        },
        ChatFocusSmokeStage::AwaitFocused => {
            if focus.get() != Some(input_entity) {
                return; // one more frame for `focus_chat_via_hotkey` to run.
            }
            *stage = ChatFocusSmokeStage::TypeWord1(0);
        },
        ChatFocusSmokeStage::TypeWord1(i) => {
            let Some(c) = CHAT_FOCUS_SMOKE_WORD_1.chars().nth(i) else {
                *stage = ChatFocusSmokeStage::PressF5;
                return;
            };
            let Some(key_code) = key_code_for_ascii_lowercase(c) else {
                fail(&mut exit, &stage, "harness bug: unmapped test character");
                *stage = ChatFocusSmokeStage::Done;
                return;
            };
            send_real_key_press(
                &mut keyboard,
                window,
                key_code,
                Key::Character(c.to_string().into()),
                Some(&c.to_string()),
            );
            *stage = ChatFocusSmokeStage::AwaitWord1Char(i);
        },
        ChatFocusSmokeStage::AwaitWord1Char(i) => {
            let expected = &CHAT_FOCUS_SMOKE_WORD_1[..=i];
            if chat.buffer == expected {
                *stage = ChatFocusSmokeStage::TypeWord1(i + 1);
                return;
            }
            if expected.starts_with(chat.buffer.as_str()) {
                return; // a frame or two for `read_chat_input` to fold the key in.
            }
            fail(
                &mut exit,
                &stage,
                "a real KeyboardInput character never reached the chat buffer while focused — the \
                 keyboard→buffer path is broken even though InputFocus looked correct",
            );
            *stage = ChatFocusSmokeStage::Done;
        },
        ChatFocusSmokeStage::PressF5 => {
            send_real_key_press(&mut keyboard, window, KeyCode::F5, Key::F5, None);
            *stage = ChatFocusSmokeStage::AwaitCollapsed;
        },
        ChatFocusSmokeStage::AwaitCollapsed => {
            if !state.collapsed {
                return;
            }
            if collapsible.iter().any(|node| node.display != Display::None) {
                *collapse_display_wait += 1;
                if *collapse_display_wait <= AWAIT_COLLAPSED_DISPLAY_TOLERANCE_FRAMES {
                    return;
                }
                fail(
                    &mut exit,
                    &stage,
                    "F5 did not hide every ChatCollapsible element",
                );
                *stage = ChatFocusSmokeStage::Done;
                return;
            }
            *collapse_display_wait = 0;
            if history.0.is_empty() || chat.buffer != CHAT_FOCUS_SMOKE_WORD_1 {
                fail(
                    &mut exit,
                    &stage,
                    "collapsing chat discarded scrollback history or in-progress text instead of \
                     only hiding it",
                );
                *stage = ChatFocusSmokeStage::Done;
                return;
            }
            *stage = ChatFocusSmokeStage::PressEnterAgain;
        },
        ChatFocusSmokeStage::PressEnterAgain => {
            send_real_key_press(&mut keyboard, window, KeyCode::Enter, Key::Enter, None);
            *stage = ChatFocusSmokeStage::AwaitReexpanded;
        },
        ChatFocusSmokeStage::AwaitReexpanded => {
            if state.collapsed {
                return;
            }
            if focus.get() != Some(input_entity) {
                fail(
                    &mut exit,
                    &stage,
                    "Enter did not refocus the chat input box after a collapse",
                );
                *stage = ChatFocusSmokeStage::Done;
                return;
            }
            *stage = ChatFocusSmokeStage::TypeWord2(0);
        },
        ChatFocusSmokeStage::TypeWord2(i) => {
            let Some(c) = CHAT_FOCUS_SMOKE_WORD_2.chars().nth(i) else {
                *stage = ChatFocusSmokeStage::Pass;
                return;
            };
            let Some(key_code) = key_code_for_ascii_lowercase(c) else {
                fail(&mut exit, &stage, "harness bug: unmapped test character");
                *stage = ChatFocusSmokeStage::Done;
                return;
            };
            send_real_key_press(
                &mut keyboard,
                window,
                key_code,
                Key::Character(c.to_string().into()),
                Some(&c.to_string()),
            );
            *stage = ChatFocusSmokeStage::AwaitWord2Char(i);
        },
        ChatFocusSmokeStage::AwaitWord2Char(i) => {
            let expected = format!(
                "{CHAT_FOCUS_SMOKE_WORD_1}{}",
                &CHAT_FOCUS_SMOKE_WORD_2[..=i]
            );
            if chat.buffer == expected {
                *stage = ChatFocusSmokeStage::TypeWord2(i + 1);
                return;
            }
            if expected.starts_with(chat.buffer.as_str()) {
                return;
            }
            fail(
                &mut exit,
                &stage,
                "a real KeyboardInput character never reached the chat buffer after the \
                 collapse/re-expand round trip",
            );
            *stage = ChatFocusSmokeStage::Done;
        },
        ChatFocusSmokeStage::Pass => {
            info!(
                "smoke-chat-focus: PASS — Enter focused the box, real keystrokes reached the chat \
                 buffer, F5 collapsed without discarding history, and Enter refocused a genuinely \
                 typable box afterward"
            );
            exit.write(AppExit::Success);
            *stage = ChatFocusSmokeStage::Done;
        },
        ChatFocusSmokeStage::Done => {},
    }
}

/// Window height assumed when the real primary window isn't queryable yet
/// (a pre-`Startup`-ordering edge).
const FALLBACK_WINDOW_HEIGHT_PX: f32 = 720.0;

/// Force-collapses the chat panel once at boot when
/// `XINDELER_SMOKE_CHAT_COLLAPSED` is set — the same env-var-gated,
/// smoke-only debug-override convention `diary.rs`'s own
/// `force_open_diary_for_smoke_capture` already establishes:
/// `--smoke-screenshot` has no real keyboard to press F5 with, so this is
/// how a live visual smoke check can confirm the collapsed (minimized)
/// panel's real on-screen size, without adding a bespoke input-injection
/// mechanism to the harness itself. A no-op unless the env var is set —
/// harmless in every normal run.
fn force_collapse_chat_for_smoke_capture(mut state: ResMut<ChatUiState>) {
    if std::env::var("XINDELER_SMOKE_CHAT_COLLAPSED").is_ok_and(|v| v != "0") {
        state.collapsed = true;
    }
}

/// Seeds a representative multi-channel scrollback (and focuses the input line
/// with a sample `/` command) once, when `XINDELER_SMOKE_CHAT_SEED` is set —
/// the same env-var-gated, smoke-only debug-override convention `diary.rs`'s
/// `force_open_diary_for_smoke_capture` uses. `--smoke-screenshot` has no live
/// server to send real chat, so this lets a visual smoke confirm the rebuilt
/// look: the per-line colored channel icons + tinted text, and the focused
/// input line's mode-colored border. Runs exactly once (a `Local` latch) and is
/// a no-op unless the env var is set — harmless in every normal run.
#[allow(clippy::too_many_arguments)]
fn seed_chat_for_smoke_capture(
    mut done: Local<bool>,
    mut net_chat: MessageWriter<NetChatMsg>,
    mut chat: ResMut<ChatInput>,
    mut focus: ResMut<InputFocus>,
    mut state: ResMut<ChatUiState>,
    inputs: Query<Entity, With<ChatInputBox>>,
) {
    if *done || !std::env::var("XINDELER_SMOKE_CHAT_SEED").is_ok_and(|v| v != "0") {
        return;
    }
    let Ok(input_entity) = inputs.single() else {
        return; // wait until the panel has spawned
    };
    *done = true;

    let seed: [(NetChatChannel, Option<&str>, &str); 6] = [
        (
            NetChatChannel::World,
            Some("Aldwin"),
            "anyone near the old mill?",
        ),
        (NetChatChannel::Say, Some("Bryn"), "right behind you!"),
        (
            NetChatChannel::Region,
            Some("Cael"),
            "trader camp just north of here",
        ),
        (
            NetChatChannel::Group,
            Some("Dara"),
            "pulling the boss, get ready",
        ),
        (
            NetChatChannel::Faction,
            Some("Eshe"),
            "the keep is ours tonight",
        ),
        (NetChatChannel::System, None, "Welcome to Xindeler."),
    ];
    for (channel, alias, text) in seed {
        net_chat.write(NetChatMsg {
            channel,
            sender_uid: None,
            sender_alias: alias.map(str::to_owned),
            text: text.to_owned(),
        });
    }

    // Show the focused input line (mode-colored border + mode icon) too.
    state.send_channel = NetChatChannel::World;
    chat.set_line("/world well met, travellers".to_owned());
    focus.set(input_entity, FocusCause::Pressed);
}

/// Spawns the legacy-styled chat box: a **transparent** bottom-left root (no
/// frame art — legacy has none) holding, top to bottom, the translucent-black
/// scrollback (message log), the hover/focus-revealed channel-tab strip, and
/// the input line (a 16×16 mode icon + the [`ChatInputBox`] `Text`, styled with
/// a mode-colored border only while focused — [`sync_chat_input_style`]).
fn spawn_chat_panel(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    icons: Res<ChatIcons>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let window = windows.single().ok();
    let initial_window_width = window.map_or(FALLBACK_WINDOW_WIDTH_PX, Window::width);
    let initial_scroll_height = window.map_or_else(
        || chat_scroll_height(FALLBACK_WINDOW_HEIGHT_PX),
        |window| chat_scroll_height(window.height()),
    );

    let root = commands
        .spawn((
            ChatPanelRoot,
            // The chat box MUST carry `GlobalZIndex(zlayer::CHAT)` = 30 so it
            // sits above the ambient HUD chrome (orbs/action-bar at 20) whose
            // geometry overlaps the bottom-left box — otherwise that chrome
            // silently swallows clicks meant to focus the input line.
            bevy::ui::GlobalZIndex(xindeler_ui::zlayer::CHAT),
            // A TRANSPARENT root: legacy chat has no frame/border/panel fill —
            // only the message box itself is a translucent-black rectangle
            // (below). Absolutely-anchored bottom-left, laid out as a column.
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(PANEL_LEFT_PX),
                bottom: Val::Px(chat_panel_bottom(initial_window_width)),
                flex_direction: FlexDirection::Column,
                width: Val::Px(PANEL_WIDTH),
                ..Default::default()
            },
        ))
        .id();

    commands.entity(root).with_children(|parent| {
        // Scrollable message log — the ONE translucent-black rectangle
        // (`CHAT_BG`, legacy `chat_opacity` 0.4). `scroll_view_bundle` fills it
        // with the theme's opaque `panel_bg`; override that back to the light
        // legacy overlay.
        //
        // Legacy fades the channel tabs in only while the mouse is over the
        // MESSAGE BOX specifically (`rect_of(message_box_bg).is_over(mouse)`),
        // so track hover with an `Over`/`Out` pair on THIS opaque box rather
        // than the transparent root. Moving onto a message row does fire an
        // `Out` on the box, but the row's own `Over` bubbles back up to this
        // observer in the SAME dispatch (Bevy emits `Out` before `Over`), so
        // the end-of-frame `hovered` state stays `true` across the whole box
        // interior — no child→child / gap flicker a root-level pair would show.
        parent
            .spawn(scroll_view_bundle(
                &theme,
                PANEL_WIDTH,
                initial_scroll_height,
            ))
            .insert((ChatScrollArea, ChatCollapsible, BackgroundColor(CHAT_BG)))
            .observe(|_: On<Pointer<Over>>, mut hovered: ResMut<ChatHovered>| hovered.0 = true)
            .observe(|_: On<Pointer<Out>>, mut hovered: ResMut<ChatHovered>| hovered.0 = false);

        // Channel-tab strip — hidden by default; [`reveal_chat_tabs`] shows it
        // while the box is hovered or focused (legacy hover-fade). NOT tagged
        // `ChatCollapsible`: its visibility is owned solely by
        // `reveal_chat_tabs` (which also respects `collapsed`).
        parent
            .spawn((ChatTabRow, Node {
                display: Display::None,
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.xs),
                margin: UiRect::vertical(Val::Px(theme.spacing.xs)),
                ..Default::default()
            }))
            .with_children(|tabs| {
                for (channel, label) in CHAT_TABS {
                    tabs.spawn(button_bundle(&theme, &fonts, label))
                        .insert(ChatTab(channel))
                        .observe(handle_tab_click);
                }
            });

        // Input line: [mode icon][text box]. The mode icon + the text box's
        // border/fill only render while focused (legacy shows the input styling
        // only when it captures the keyboard) — see `sync_chat_input_style`.
        parent
            .spawn((ChatCollapsible, Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(2.0),
                width: Val::Px(PANEL_WIDTH),
                ..Default::default()
            }))
            .with_children(|row| {
                // The 16×16 chat-mode icon (default World). Starts hidden;
                // `sync_chat_input_style` shows + swaps it while focused.
                // `Pickable::IGNORE` so it never steals the focus click.
                row.spawn((
                    ChatInputModeIcon,
                    ImageNode::new(icons.for_channel(NetChatChannel::World)),
                    bevy::picking::Pickable::IGNORE,
                    Visibility::Hidden,
                    Node {
                        width: Val::Px(CHAT_ICON_PX),
                        height: Val::Px(CHAT_ICON_PX),
                        ..Default::default()
                    },
                ));

                // The input text line — always present (so it stays clickable
                // to focus even while unfocused/empty, per the PR #152 fix), a
                // `min_height` floor keeping it a non-zero clickable strip. Its
                // border/fill are transparent until focused.
                row.spawn((
                    ChatInputBox,
                    Text(String::new()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(channel_color(NetChatChannel::World)),
                    Node {
                        flex_grow: 1.0,
                        min_height: Val::Px(INPUT_MIN_HEIGHT_PX),
                        padding: UiRect::all(Val::Px(4.0)),
                        border: UiRect::all(Val::Px(INPUT_BORDER_PX)),
                        ..Default::default()
                    },
                    BackgroundColor(Color::NONE),
                    bevy::ui::BorderColor::all(Color::NONE),
                ))
                .observe(focus_chat_on_click);
            });
    });
}

/// Reveals the channel-tab strip while the box is hovered or the input is
/// focused (legacy fades its tabs in only while the mouse is over the box),
/// and hides it entirely while collapsed. A no-op write-guard avoids
/// re-triggering layout every frame.
fn reveal_chat_tabs(
    hovered: Res<ChatHovered>,
    state: Res<ChatUiState>,
    focus: Res<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
    mut tab_rows: Query<&mut Node, With<ChatTabRow>>,
) {
    let focused = inputs
        .single()
        .is_ok_and(|entity| focus.get() == Some(entity));
    let show = !state.collapsed && (hovered.0 || focused);
    let desired = if show { Display::Flex } else { Display::None };
    for mut node in &mut tab_rows {
        if node.display != desired {
            node.display = desired;
        }
    }
}

/// Styles the input line to match legacy: while focused, the text box shows a
/// mode-colored 2px border over translucent black and the mode icon appears
/// (swapped to the current send channel's icon); while unfocused the border and
/// fill are transparent and the icon is hidden — legacy only draws the input
/// chrome when it captures the keyboard. The buffer text is always tinted the
/// current send channel's color (legacy colors the input text by mode).
fn sync_chat_input_style(
    focus: Res<InputFocus>,
    state: Res<ChatUiState>,
    icons: Res<ChatIcons>,
    inputs: Query<Entity, With<ChatInputBox>>,
    mut boxes: Query<
        (
            &mut BackgroundColor,
            &mut bevy::ui::BorderColor,
            &mut TextColor,
        ),
        With<ChatInputBox>,
    >,
    mut mode_icons: Query<(&mut Visibility, &mut ImageNode), With<ChatInputModeIcon>>,
) {
    let Ok(input_entity) = inputs.single() else {
        return;
    };
    let focused = focus.get() == Some(input_entity);
    let mode = channel_color(state.send_channel);

    if let Ok((mut bg, mut border, mut text_color)) = boxes.single_mut() {
        let desired_bg = if focused { CHAT_BG } else { Color::NONE };
        let desired_border = if focused { mode } else { Color::NONE };
        if bg.0 != desired_bg {
            bg.0 = desired_bg;
        }
        if border.top != desired_border {
            *border = bevy::ui::BorderColor::all(desired_border);
        }
        if text_color.0 != mode {
            text_color.0 = mode;
        }
    }

    if let Ok((mut visibility, mut image)) = mode_icons.single_mut() {
        let desired_vis = if focused {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != desired_vis {
            *visibility = desired_vis;
        }
        let desired_icon = icons.for_channel(state.send_channel);
        if image.image != desired_icon {
            image.image = desired_icon;
        }
    }
}

/// Keeps the scrollback height tracking [`chat_scroll_height`] as the real
/// window is live-resized (a no-op write-guard avoids re-triggering layout
/// every frame).
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

/// Keeps the panel's `bottom` offset tracking [`chat_panel_bottom`] as the
/// real window is live-resized — the true bottom-left corner margin whenever
/// that's safe, falling back to clearing the whole health-orb cluster
/// whenever the current window width puts the two in each other's way (a
/// no-op write-guard avoids re-triggering layout every frame, matching
/// [`sync_chat_scroll_height_to_window`]'s own pattern).
fn sync_chat_panel_bottom_to_window(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<&mut Node, With<ChatPanelRoot>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let desired = Val::Px(chat_panel_bottom(window.width()));
    for mut node in &mut roots {
        if node.bottom != desired {
            node.bottom = desired;
        }
    }
}

/// Renders one [`NetChatMsg`] to its scrollback line text. Legacy conveys the
/// channel via the per-line ICON + text color rather than a bracketed prefix,
/// so this is just `alias: text` (or bare `text` for a senderless line) — the
/// icon/color carry the channel.
fn format_chat_line(msg: &NetChatMsg) -> String {
    match &msg.sender_alias {
        Some(alias) => format!("{alias}: {}", msg.text),
        None => msg.text.clone(),
    }
}

/// A crude "mentions" signal: any whitespace-separated `@token` (with at least
/// one character after the `@`).
fn contains_mention(text: &str) -> bool {
    text.split_whitespace()
        .any(|token| token.starts_with('@') && token.len() > 1)
}

/// Drains arriving [`NetChatMsg`]s, spawning one legacy-style row each — a flex
/// row of a 16×16 channel [`ChatRowIcon`] + a channel-tinted [`ChatRowText`]
/// line (whole-row tint if it looks like a mention) — applying the current view
/// filter, and evicting the oldest past [`MAX_CHAT_HISTORY`].
fn ingest_chat_messages(
    mut commands: Commands,
    mut incoming: MessageReader<NetChatMsg>,
    mut history: ResMut<ChatHistory>,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    icons: Res<ChatIcons>,
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
                BackgroundColor(if mentioned {
                    theme.palette.accent.with_alpha(0.25)
                } else {
                    Color::NONE
                }),
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::FlexStart,
                    column_gap: Val::Px(4.0),
                    padding: UiRect::vertical(Val::Px(1.0)),
                    display: if visible {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    ..Default::default()
                },
            ))
            .with_children(|row| {
                // The left-gutter channel icon — the real legacy 16×16 art.
                row.spawn((
                    ChatRowIcon,
                    ImageNode::new(icons.for_channel(msg.channel)),
                    Node {
                        width: Val::Px(CHAT_ICON_PX),
                        height: Val::Px(CHAT_ICON_PX),
                        flex_shrink: 0.0,
                        margin: UiRect::top(Val::Px(1.0)),
                        ..Default::default()
                    },
                ));
                // The channel-tinted message text (word-wraps within the box).
                row.spawn((
                    ChatRowText,
                    Text(format_chat_line(msg)),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(14.0),
                        ..Default::default()
                    },
                    TextColor(channel_color(msg.channel)),
                    Node {
                        // Leave room for the icon gutter so a wrapped line stays
                        // inside the box width.
                        max_width: Val::Px(PANEL_WIDTH - CHAT_ICON_PX - 12.0),
                        ..Default::default()
                    },
                ));
            })
            .id();
        commands.entity(scroll_entity).add_child(row);

        history.0.push(row);
        if history.0.len() > MAX_CHAT_HISTORY {
            let oldest = history.0.remove(0);
            commands.entity(oldest).despawn();
        }
    }

    if appended_any && let Ok(mut scroll) = scroll_positions.single_mut() {
        scroll.y = f32::MAX / 2.0;
    }
}

/// Re-applies the view filter to every [`ChatRow`]'s [`Node::display`] whenever
/// the filter changes (a tab click).
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

/// Highlights the currently-active view-filter tab (background swap).
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

/// A tab click updates [`ChatUiState`]: "All" only changes the view filter; a
/// sendable channel changes both filter and send channel; view-only kinds
/// (`Tell`) only change the filter.
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

/// Whether the chat input box currently holds keyboard focus ([`InputFocus`]) —
/// the shared "don't fire a hotkey while the player is typing" predicate other
/// modules (`diary`/`inventory_ui`/`social_hud`/`map_view`) compose with
/// `.run_if(not(crate::chat::text_input_focused))`.
///
/// The signal is still [`InputFocus`] pointed at the [`ChatInputBox`] entity
/// even though that box is now a plain `Text` node rather than an
/// `EditableText` — [`focus_chat_via_hotkey`]/[`focus_chat_on_click`] set it,
/// and [`blur_chat_input_on_escape`]/[`blur_chat_input_on_collapse`] clear it.
pub(crate) fn text_input_focused(
    focus: Res<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
) -> bool {
    let Ok(input_entity) = inputs.single() else {
        return false;
    };
    focus.get() == Some(input_entity)
}

/// Clears [`InputFocus`] when Escape is pressed while the chat input box holds
/// it — the explicit blur path [`text_input_focused`] requires (nothing else
/// un-sets focus on its own, so without this the typing guard would suppress
/// every gated hotkey forever after the first time chat was focused). A no-op
/// if the box isn't focused (Escape then falls through to camera
/// cursor-release).
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

/// Clears [`InputFocus`] the moment [`ChatUiState::collapsed`] flips `true`
/// while the box holds it — the same "a transition hid the focused widget, drop
/// the stale focus" fix as [`blur_chat_input_on_escape`], for minimizing.
/// Without it, `text_input_focused` and `cursor.rs`'s mirrored `chat_focused`
/// would keep reporting the hidden box as focused, forcing the OS cursor free
/// forever (camera looks unresponsive). A `Local<bool>` edge-detector fires
/// exactly once on the real false→true transition.
///
/// `pub(crate)`: `cursor.rs`'s `CursorControlPlugin` orders
/// `update_cursor_free.after(Self)` so it reads the same-frame cleared focus.
pub(crate) fn blur_chat_input_on_collapse(
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

/// [`GameInput::Chat`] (`Enter` by default) focuses the chat input box directly
/// — ported from legacy `xindeler-old`'s own
/// `Hud::focus_widget(Some(self.ids.chat))` on the same key. This is the
/// primary keyboard-driven entry: it works regardless of the OS cursor's grab
/// state (the mouse click path only works while the cursor is already free).
/// Un-collapses the panel first if it was minimized. A no-op while the box is
/// ALREADY focused ([`read_chat_input`] owns the Enter-submit case then);
/// ordering this `.after(read_chat_input)` guarantees a single Enter can never
/// both refocus AND immediately submit stale text.
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

/// A click on the input box focuses chat — the mouse counterpart of
/// [`focus_chat_via_hotkey`]. Only reachable while the OS cursor is free (a
/// menu open, or chat already focused); during mouselook the keyboard path is
/// the way in, exactly as in legacy. Because the box is a plain `Text` node,
/// `bevy_ui_widgets`' own `EditableText` click-to-focus observer does NOT apply
/// — this is our explicit replacement.
fn focus_chat_on_click(
    _click: On<Pointer<Click>>,
    mut focus: ResMut<InputFocus>,
    mut state: ResMut<ChatUiState>,
    inputs: Query<Entity, With<ChatInputBox>>,
) {
    let Ok(input_entity) = inputs.single() else {
        return;
    };
    if state.collapsed {
        state.collapsed = false;
    }
    focus.set(input_entity, FocusCause::Pressed);
}

/// [`GameInput::ToggleChat`] (`F5` by default) flips [`ChatUiState::collapsed`]
/// — legacy's toggle-chat keybind fully hides/shows the box (there is no
/// on-screen minimize button).
fn toggle_chat_via_hotkey(action_state: Res<ActionState>, mut state: ResMut<ChatUiState>) {
    if action_state.just_pressed(GameInput::ToggleChat) {
        state.collapsed = !state.collapsed;
    }
}

/// Hides every [`ChatCollapsible`] element (scrollback + input line) while
/// collapsed — legacy's F5 toggle fully hides the chat box, so a collapsed
/// panel leaves nothing on screen (the transparent root itself is invisible).
/// The tab strip is managed separately by [`reveal_chat_tabs`], which also
/// respects `collapsed`.
///
/// Collapsing also force-clears [`ChatHovered`]: hiding the scrollback box
/// (`Display::None`) suppresses its picking, so its [`Pointer<Out>`] observer
/// would never fire to lower a `hovered` latched `true` at collapse time —
/// leaving the resource stuck "hovered" until the next real pointer exit. That
/// is harmless today ([`reveal_chat_tabs`] gates on `!collapsed`), but keeping
/// the resource honest avoids a latent surprise for any future reader of it.
fn sync_chat_collapsed(
    state: Res<ChatUiState>,
    mut hovered: ResMut<ChatHovered>,
    mut collapsible: Query<&mut Node, With<ChatCollapsible>>,
) {
    if !state.is_changed() {
        return;
    }
    if state.collapsed && hovered.0 {
        hovered.0 = false;
    }
    let desired = if state.collapsed {
        Display::None
    } else {
        Display::Flex
    };
    for mut node in &mut collapsible {
        if node.display != desired {
            node.display = desired;
        }
    }
}

/// Mirrors the owned [`ChatInput::buffer`] into the on-screen [`ChatInputBox`]
/// `Text`, drawing a caret (`|`) at the cursor position while focused. Uses a
/// no-op write-guard so an unchanged line doesn't re-trigger text layout.
fn render_chat_input(
    focus: Res<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
    chat: Res<ChatInput>,
    mut texts: Query<&mut Text, With<ChatInputBox>>,
) {
    let Ok(input_entity) = inputs.single() else {
        return;
    };
    let focused = focus.get() == Some(input_entity);
    let Ok(mut text) = texts.single_mut() else {
        return;
    };
    let desired = if focused {
        // `cursor` is always a char boundary, so `split_at` cannot panic.
        let (pre, post) = chat.buffer.split_at(chat.cursor);
        format!("{pre}|{post}")
    } else {
        chat.buffer.clone()
    };
    if text.0 != desired {
        text.0 = desired;
    }
}

/// **The heart of the rewrite.** Folds real [`KeyboardInput`] directly into the
/// owned [`ChatInput`] buffer while the box holds focus — no `EditableText`, no
/// `FocusedInput` dispatch, no IME (see the module doc comment for why the
/// `EditableText` path left the box untypable on macOS). Handles Enter
/// (submit), editing/navigation keys, Tab completion, Up/Down history recall,
/// and Cmd/Ctrl+A/C/X/V against the engine [`bevy::clipboard::Clipboard`].
///
/// While UNfocused it drains its reader so a burst of keys pressed the same
/// frame chat gains focus can't spill into the box.
#[allow(clippy::too_many_arguments)]
fn read_chat_input(
    focus: Res<InputFocus>,
    inputs: Query<Entity, With<ChatInputBox>>,
    mut keyboard: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut chat: ResMut<ChatInput>,
    mut clipboard: ResMut<bevy::clipboard::Clipboard>,
    mut send: MessageWriter<ChatSendRequest>,
    state: Res<ChatUiState>,
) {
    let focused = inputs
        .single()
        .is_ok_and(|entity| focus.get() == Some(entity));
    if !focused {
        keyboard.read().for_each(drop);
        return;
    }

    // The Command modifier: Cmd on macOS, Ctrl elsewhere — but accept EITHER so
    // the shortcuts work regardless of platform/keyboard.
    let cmd = keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
        || keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight);

    for ev in keyboard.read() {
        if !ev.state.is_pressed() {
            continue;
        }
        match &ev.logical_key {
            Key::Enter => submit_chat_line(&mut chat, &mut send, &state),
            Key::Backspace => chat.backspace(),
            Key::Delete => chat.delete(),
            Key::ArrowLeft => chat.move_left(),
            Key::ArrowRight => chat.move_right(),
            Key::Home => chat.home(),
            Key::End => chat.end(),
            Key::ArrowUp => history_recall_prev(&mut chat),
            Key::ArrowDown => history_recall_next(&mut chat),
            Key::Tab => complete_command(&mut chat),
            Key::Character(s) if cmd => match s.to_lowercase().as_str() {
                "a" => chat.select_all = true,
                "c" => {
                    let _ = clipboard.set_text(chat.selection_text());
                },
                "x" => {
                    let _ = clipboard.set_text(chat.selection_text());
                    chat.clear_line();
                },
                "v" => paste_from_clipboard(&mut chat, &mut clipboard),
                _ => {},
            },
            Key::Character(s) => chat.insert_str(s.as_str()),
            Key::Space => chat.insert_str(" "),
            _ => {},
        }
    }
}

/// Sends the current line (a `/command` or a plain channel line), records it in
/// recall history, and clears the buffer — keeping focus so the player can keep
/// chatting (Escape leaves). A no-op on an empty/whitespace line.
fn submit_chat_line(
    chat: &mut ChatInput,
    send: &mut MessageWriter<ChatSendRequest>,
    state: &ChatUiState,
) {
    let trimmed = chat.buffer.trim().to_owned();
    if trimmed.is_empty() {
        return;
    }
    let request = match parse_slash_command(&trimmed) {
        Some((name, args)) => ChatSendRequest::Command { name, args },
        None => ChatSendRequest::Channel {
            channel: state.send_channel,
            text: trimmed.clone(),
        },
    };
    send.write(request);
    if chat.history.last().map(String::as_str) != Some(trimmed.as_str()) {
        chat.history.push(trimmed);
        if chat.history.len() > CHAT_INPUT_HISTORY_MAX {
            chat.history.remove(0);
        }
    }
    chat.clear_line();
}

/// Up-arrow: recall the previous (older) sent line.
fn history_recall_prev(chat: &mut ChatInput) {
    if chat.history.is_empty() {
        return;
    }
    let new_pos = match chat.history_pos {
        None => chat.history.len() - 1,
        Some(0) => 0,
        Some(p) => p - 1,
    };
    let line = chat.history[new_pos].clone();
    chat.set_line(line);
    chat.history_pos = Some(new_pos);
}

/// Down-arrow: recall the next (newer) sent line, or drop back to a fresh empty
/// line past the newest.
fn history_recall_next(chat: &mut ChatInput) {
    match chat.history_pos {
        None => {},
        Some(p) if p + 1 < chat.history.len() => {
            let line = chat.history[p + 1].clone();
            chat.set_line(line);
            chat.history_pos = Some(p + 1);
        },
        Some(_) => chat.clear_line(),
    }
}

/// Tab: cycle command-name completions for a `/command` line, replacing only
/// the command-name token.
fn complete_command(chat: &mut ChatInput) {
    if let Some((name, _)) = parse_slash_command(&chat.buffer) {
        let matches = matching_commands(&name);
        if !matches.is_empty() {
            let next = matches[chat.completion_cycle % matches.len()];
            chat.completion_cycle = chat.completion_cycle.wrapping_add(1);
            let replaced = replace_command_name(&chat.buffer, next);
            chat.buffer = replaced;
            chat.cursor = chat.buffer.len();
        }
    }
}

/// Cmd/Ctrl+V: insert the clipboard's text at the caret. On native targets the
/// read resolves synchronously; a still-pending read (only possible on wasm) is
/// simply dropped this frame (v1 — a documented limitation, not a silent skip).
fn paste_from_clipboard(chat: &mut ChatInput, clipboard: &mut bevy::clipboard::Clipboard) {
    let mut read = clipboard.fetch_text();
    if let Some(Ok(text)) = read.poll_result() {
        chat.insert_str(&text);
    }
}

/// Splits a leading `/command args…` into `(name, args)`, or `None` if `raw`
/// doesn't start with `/`. Args split on whitespace (not quote-aware — a v1
/// cut).
fn parse_slash_command(raw: &str) -> Option<(String, Vec<String>)> {
    let rest = raw.strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let name = parts.next()?.to_owned();
    let args = parts.map(str::to_owned).collect();
    Some((name, args))
}

/// The command-name completions matching `prefix`, in [`KNOWN_COMMANDS`] order.
fn matching_commands(prefix: &str) -> Vec<&'static str> {
    KNOWN_COMMANDS
        .iter()
        .copied()
        .filter(|candidate| candidate.starts_with(prefix))
        .collect()
}

/// Replaces the command-name token of `raw` with `replacement`, leaving
/// everything from the first space onward untouched.
fn replace_command_name(raw: &str, replacement: &str) -> String {
    match raw.find(char::is_whitespace) {
        Some(space) => format!("/{replacement}{}", &raw[space..]),
        None => format!("/{replacement}"),
    }
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
        app.insert_resource(ChatIcons::dummy());
        app.init_resource::<ChatUiState>();
        app.init_resource::<ChatHistory>();
        app.init_resource::<ChatInput>();
        app.init_resource::<ChatHovered>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app
    }

    // ---- ChatInput editing helpers -----------------------------------------

    /// Typing inserts at the caret and advances it; the buffer accrues in
    /// order.
    #[test]
    fn insert_str_appends_and_advances_the_caret() {
        let mut ci = ChatInput::default();
        ci.insert_str("h");
        ci.insert_str("i");
        assert_eq!(ci.buffer, "hi");
        assert_eq!(ci.cursor, 2);
    }

    /// Inserting mid-line respects the caret; newlines collapse to spaces and
    /// control chars are dropped (single-line field).
    #[test]
    fn insert_str_is_caret_aware_and_single_line() {
        let mut ci = ChatInput::default();
        ci.set_line("ac".to_owned());
        ci.cursor = 1;
        ci.insert_str("b");
        assert_eq!(ci.buffer, "abc");
        ci.set_line(String::new());
        ci.insert_str("x\ny");
        assert_eq!(ci.buffer, "x y", "newlines must collapse to spaces");
    }

    /// Backspace/Delete/Home/End/arrows behave like a standard single-line
    /// editor, on char boundaries (exercised here with a multi-byte char).
    #[test]
    fn editing_keys_operate_on_char_boundaries() {
        let mut ci = ChatInput::default();
        ci.set_line("aé b".to_owned()); // 'é' is 2 bytes
        ci.end();
        ci.backspace();
        assert_eq!(ci.buffer, "aé ");
        ci.home();
        ci.move_right(); // past 'a'
        ci.delete(); // deletes 'é' (both bytes)
        assert_eq!(ci.buffer, "a ");
    }

    /// Cmd/Ctrl+A then typing replaces the whole line (single-line select-all).
    #[test]
    fn select_all_then_insert_replaces_the_line() {
        let mut ci = ChatInput::default();
        ci.set_line("old text".to_owned());
        ci.select_all = true;
        ci.insert_str("new");
        assert_eq!(ci.buffer, "new");
        assert_eq!(ci.cursor, 3);
    }

    // ---- read_chat_input end-to-end (headless, real KeyboardInput) ---------

    /// The **core regression test for the rewrite**: a real [`KeyboardInput`]
    /// character reaches the buffer while focused — the exact delivery the old
    /// `EditableText` path silently dropped under macOS IME. Drives the real
    /// production [`read_chat_input`] system (not a helper) via a genuine
    /// `KeyboardInput` message through a focused `InputFocus`, no window/IME
    /// needed.
    #[test]
    fn read_chat_input_folds_a_real_keypress_into_the_buffer_while_focused() {
        let mut app = new_app();
        app.init_resource::<InputFocus>();
        app.add_message::<KeyboardInput>();
        app.insert_resource(bevy::clipboard::Clipboard::default());
        let window = app.world_mut().spawn_empty().id();
        let input = app.world_mut().spawn(ChatInputBox).id();
        app.insert_resource(InputFocus::from_entity(input));

        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyH,
            logical_key: Key::Character("h".into()),
            state: bevy::input::ButtonState::Pressed,
            text: Some("h".into()),
            repeat: false,
            window,
        });
        app.world_mut()
            .run_system_once(read_chat_input)
            .expect("system runs");

        assert_eq!(
            app.world().resource::<ChatInput>().buffer,
            "h",
            "a real focused keypress must reach the owned buffer — the whole point of the rewrite"
        );
    }

    /// While UNfocused, `read_chat_input` must NOT accept keystrokes (and must
    /// drain them so they can't spill in the instant focus is gained).
    #[test]
    fn read_chat_input_ignores_keys_while_unfocused() {
        let mut app = new_app();
        app.init_resource::<InputFocus>(); // nothing focused
        app.add_message::<KeyboardInput>();
        app.insert_resource(bevy::clipboard::Clipboard::default());
        let window = app.world_mut().spawn_empty().id();
        app.world_mut().spawn(ChatInputBox);

        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyH,
            logical_key: Key::Character("h".into()),
            state: bevy::input::ButtonState::Pressed,
            text: Some("h".into()),
            repeat: false,
            window,
        });
        app.world_mut()
            .run_system_once(read_chat_input)
            .expect("system runs");

        assert!(
            app.world().resource::<ChatInput>().buffer.is_empty(),
            "keys typed while chat isn't focused must not enter the buffer"
        );
    }

    /// Enter on a plain line sends a `Channel` request with the current send
    /// channel and clears the buffer.
    #[test]
    fn submit_on_a_plain_line_sends_a_channel_request_and_clears() {
        let mut app = new_app();
        app.world_mut().resource_mut::<ChatUiState>().send_channel = NetChatChannel::Region;
        let mut ci = ChatInput::default();
        ci.set_line("hello there".to_owned());
        app.insert_resource(ci);

        app.world_mut()
            .run_system_once(
                |mut chat: ResMut<ChatInput>,
                 mut send: MessageWriter<ChatSendRequest>,
                 state: Res<ChatUiState>| {
                    submit_chat_line(&mut chat, &mut send, &state);
                },
            )
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
        assert!(
            app.world().resource::<ChatInput>().buffer.is_empty(),
            "the input line must clear after sending"
        );
    }

    /// A leading `/` bypasses the channel tabs and sends a raw `Command`.
    #[test]
    fn submit_on_a_slash_command_sends_a_command_request() {
        let mut app = new_app();
        let mut ci = ChatInput::default();
        ci.set_line("/tell Bob hi".to_owned());
        app.insert_resource(ci);

        app.world_mut()
            .run_system_once(
                |mut chat: ResMut<ChatInput>,
                 mut send: MessageWriter<ChatSendRequest>,
                 state: Res<ChatUiState>| {
                    submit_chat_line(&mut chat, &mut send, &state);
                },
            )
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

    /// Up/Down recall the sent-line history.
    #[test]
    fn history_recall_walks_sent_lines() {
        let mut ci = ChatInput {
            history: vec!["first".to_owned(), "second".to_owned()],
            ..Default::default()
        };
        history_recall_prev(&mut ci);
        assert_eq!(ci.buffer, "second", "Up recalls the newest line first");
        history_recall_prev(&mut ci);
        assert_eq!(ci.buffer, "first");
        history_recall_next(&mut ci);
        assert_eq!(ci.buffer, "second");
        history_recall_next(&mut ci);
        assert!(
            ci.buffer.is_empty(),
            "past the newest, Down returns to a fresh line"
        );
    }

    // ---- scrollback / tabs / filter / collapse -----------------------------

    #[test]
    fn formats_lines_with_and_without_a_resolved_sender() {
        let with_sender = NetChatMsg {
            channel: NetChatChannel::Say,
            sender_uid: Some(NetUid(1)),
            sender_alias: Some("Hero".to_owned()),
            text: "hello".to_owned(),
        };
        // Legacy conveys the channel via the per-line icon + text color, not a
        // bracketed prefix, so the text is just `alias: text`.
        assert_eq!(format_chat_line(&with_sender), "Hero: hello");

        let without_sender = NetChatMsg {
            channel: NetChatChannel::System,
            sender_uid: None,
            sender_alias: None,
            text: "server started".to_owned(),
        };
        assert_eq!(format_chat_line(&without_sender), "server started");
    }

    #[test]
    fn detects_at_mentions() {
        assert!(contains_mention("hey @Hero check this out"));
        assert!(!contains_mention("no mention here"));
        assert!(!contains_mention("a lone @ with nothing after"));
    }

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
        // The row is now a container of [icon, text]: an `ImageNode` gutter icon
        // and a `ChatRowText` line (legacy's icon + colored text).
        let row_children = app
            .world()
            .get::<Children>(row)
            .expect("the row has an icon + text child");
        let has_icon = row_children
            .iter()
            .any(|c| app.world().get::<ChatRowIcon>(c).is_some());
        assert!(has_icon, "the row must carry a channel icon");
        let text = row_children
            .iter()
            .find_map(|c| app.world().get::<Text>(c).map(|t| t.0.clone()))
            .expect("the row has a text child");
        assert_eq!(text, "Hero: hello world");
    }

    /// Finds the `ChatRowText` line under a `ChatRow` container entity.
    fn row_text(app: &App, row: Entity) -> Option<String> {
        app.world()
            .get::<Children>(row)?
            .iter()
            .find_map(|c| app.world().get::<Text>(c).map(|t| t.0.clone()))
    }

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
        let oldest = *history.0.first().expect("at least one row remains");
        assert_eq!(row_text(&app, oldest), Some("line 5".to_owned()));
    }

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

    #[test]
    fn chat_starts_uncollapsed() {
        assert!(!ChatUiState::default().collapsed);
    }

    /// The [`ChatPanelRoot`] MUST carry `GlobalZIndex(zlayer::CHAT)` — the
    /// click-routing regression guard (without it the ambient HUD chrome
    /// overlapping the bottom-left panel swallows the click meant to focus the
    /// input box).
    #[test]
    fn spawn_chat_panel_puts_the_root_on_the_chat_z_layer() {
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
        assert_eq!(z_index.0, xindeler_ui::zlayer::CHAT);
    }

    /// The input box must be spawned as a plain `Text` node, NOT an
    /// `EditableText` — the regression guard for the whole rewrite. If a future
    /// change re-introduces `EditableText` on the input box, `bevy_ui_widgets`
    /// will re-enable IME on focus and macOS typing breaks again.
    #[test]
    fn input_box_is_a_plain_text_node_not_an_editable_text() {
        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_chat_panel)
            .expect("spawn_chat_panel runs");

        let world = app.world_mut();
        let input = world
            .query_filtered::<Entity, With<ChatInputBox>>()
            .single(world)
            .expect("ChatInputBox exists");
        assert!(
            world.get::<Text>(input).is_some(),
            "the input box must be a plain Text node"
        );
        assert!(
            world.get::<bevy::text::EditableText>(input).is_none(),
            "the input box must NOT be an EditableText — that re-enables IME and breaks macOS \
             typing"
        );
    }

    /// Collapsing (F5 / legacy toggle-chat) hides every `ChatCollapsible`
    /// element (scrollback + input line) — legacy fully hides the box, there is
    /// no minimize button. Re-expanding restores them.
    #[test]
    fn sync_chat_collapsed_hides_and_restores_collapsible_elements() {
        let mut app = new_app();
        let scroll_area = app
            .world_mut()
            .spawn((ChatCollapsible, ChatScrollArea, Node::default()))
            .id();
        let input_row = app
            .world_mut()
            .spawn((ChatCollapsible, Node::default()))
            .id();

        // Pointer was over the box when it collapsed: collapsing must force the
        // hover latch back to `false` (the hidden box can no longer emit `Out`).
        app.world_mut().resource_mut::<ChatHovered>().0 = true;
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(sync_chat_collapsed)
            .expect("system runs");

        assert_eq!(
            app.world().get::<Node>(scroll_area).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(input_row).unwrap().display,
            Display::None,
            "the input row must ALSO hide while collapsed"
        );
        assert!(
            !app.world().resource::<ChatHovered>().0,
            "collapsing must clear the hover latch so the resource stays honest"
        );

        app.world_mut().resource_mut::<ChatUiState>().collapsed = false;
        app.world_mut()
            .run_system_once(sync_chat_collapsed)
            .expect("system runs again");

        assert_eq!(
            app.world().get::<Node>(scroll_area).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(input_row).unwrap().display,
            Display::Flex,
            "re-expanding must restore the input row to Display::Flex"
        );
    }

    /// The channel-tab strip reveals only while the box is hovered or the input
    /// is focused, and stays hidden while collapsed (legacy hover-fade).
    #[test]
    fn reveal_chat_tabs_shows_on_hover_or_focus_and_hides_when_collapsed() {
        use bevy::input_focus::InputFocus;

        let mut app = new_app();
        app.init_resource::<InputFocus>();
        let input = app.world_mut().spawn(ChatInputBox).id();
        let tab_row = app
            .world_mut()
            .spawn((ChatTabRow, Node {
                display: Display::None,
                ..Default::default()
            }))
            .id();

        // Not hovered, not focused → hidden.
        app.world_mut()
            .run_system_once(reveal_chat_tabs)
            .expect("runs");
        assert_eq!(
            app.world().get::<Node>(tab_row).unwrap().display,
            Display::None
        );

        // Hovered → shown.
        app.world_mut().resource_mut::<ChatHovered>().0 = true;
        app.world_mut()
            .run_system_once(reveal_chat_tabs)
            .expect("runs");
        assert_eq!(
            app.world().get::<Node>(tab_row).unwrap().display,
            Display::Flex
        );

        // Collapsed wins even while hovered → hidden.
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(reveal_chat_tabs)
            .expect("runs");
        assert_eq!(
            app.world().get::<Node>(tab_row).unwrap().display,
            Display::None
        );

        // Focused (not hovered, not collapsed) → shown.
        app.world_mut().resource_mut::<ChatHovered>().0 = false;
        app.world_mut().resource_mut::<ChatUiState>().collapsed = false;
        app.insert_resource(InputFocus::from_entity(input));
        app.world_mut()
            .run_system_once(reveal_chat_tabs)
            .expect("runs");
        assert_eq!(
            app.world().get::<Node>(tab_row).unwrap().display,
            Display::Flex
        );
    }

    #[test]
    fn toggle_chat_via_hotkey_flips_collapsed() {
        use xindeler_input::{KeyMap, action_state::update_action_state};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
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
        assert!(app.world().resource::<ChatUiState>().collapsed);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::F5);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F5);
        app.update();
        assert!(!app.world().resource::<ChatUiState>().collapsed);
    }

    // ---- focus lifecycle ---------------------------------------------------

    #[test]
    fn escape_blurs_the_chat_input_and_lifts_the_typing_guard() {
        let mut app = new_app();
        let input = app.world_mut().spawn(ChatInputBox).id();
        app.insert_resource(InputFocus::from_entity(input));

        assert!(
            app.world_mut()
                .run_system_once(text_input_focused)
                .expect("condition runs")
        );

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
            "Escape must clear InputFocus, lifting the typing guard"
        );
    }

    #[test]
    fn escape_without_chat_focus_does_not_clear_an_unrelated_focus() {
        let mut app = new_app();
        app.world_mut().spawn(ChatInputBox);
        let other = app.world_mut().spawn_empty().id();
        app.insert_resource(InputFocus::from_entity(other));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.world_mut()
            .run_system_once(blur_chat_input_on_escape)
            .expect("system runs");

        assert_eq!(app.world().resource::<InputFocus>().get(), Some(other));
    }

    /// **Root-cause regression test for "minimizing chat leaves the camera
    /// unresponsive."** Collapsing while the box holds focus must clear
    /// `InputFocus`.
    #[test]
    fn collapsing_chat_blurs_the_focused_input_box() {
        let mut app = new_app();
        let input = app.world_mut().spawn(ChatInputBox).id();
        app.insert_resource(InputFocus::from_entity(input));

        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(blur_chat_input_on_collapse)
            .expect("system runs");

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            None,
            "minimizing chat while it holds focus must clear InputFocus"
        );
    }

    #[test]
    fn blur_on_collapse_is_a_no_op_while_expanded_or_unfocused() {
        let mut app = new_app();
        let input = app.world_mut().spawn(ChatInputBox).id();
        app.insert_resource(InputFocus::from_entity(input));

        app.world_mut()
            .run_system_once(blur_chat_input_on_collapse)
            .expect("system runs");
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(input));

        let other = app.world_mut().spawn_empty().id();
        app.insert_resource(InputFocus::from_entity(other));
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.world_mut()
            .run_system_once(blur_chat_input_on_collapse)
            .expect("system runs");
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(other));
    }

    #[test]
    fn blur_on_collapse_only_fires_on_the_edge_not_every_frame_while_collapsed() {
        let mut app = new_app();
        app.init_resource::<InputFocus>();
        app.add_systems(Update, blur_chat_input_on_collapse);

        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.update();

        let input = app.world_mut().spawn(ChatInputBox).id();
        app.insert_resource(InputFocus::from_entity(input));
        app.world_mut().resource_mut::<ChatUiState>().view_filter = Some(NetChatChannel::Say);
        app.update();

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(input),
            "an unrelated ChatUiState write while already collapsed must not re-clear focus"
        );
    }

    /// **Root-cause regression test for "chat can never be refocused again."**
    /// Enter ([`GameInput::Chat`]) sets `InputFocus` onto the box directly and
    /// un-collapses the panel — the only keyboard-only path in.
    #[test]
    fn enter_focuses_the_chat_input_and_uncollapses_the_panel() {
        use xindeler_input::{KeyMap, action_state::update_action_state};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        app.init_resource::<InputFocus>();
        let input = app.world_mut().spawn(ChatInputBox).id();
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        app.add_systems(Update, (update_action_state, focus_chat_via_hotkey).chain());

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(input),
            "Enter must focus the chat input box directly, no click required"
        );
        assert!(
            !app.world().resource::<ChatUiState>().collapsed,
            "Enter must also un-collapse a minimized panel"
        );
    }

    /// Fires a synthetic [`Pointer<Click>`] at `entity` through the REAL
    /// entity-scoped observer wiring (`world.trigger(..)`), matching this
    /// crate's established convention (see `inventory_ui`'s own
    /// `fire_pointer_click`). It exercises the observer plus its focus logic on
    /// the genuinely-spawned box entity — a deliberate step above the prior
    /// attempts' "hand-set `InputFocus` and assert" — without needing a live
    /// window, camera, or picking-backend hit-test (the inert
    /// `NormalizedRenderTarget::None` target and `Entity::PLACEHOLDER` hit are
    /// placeholders). The fully-real pixel-to-picking-to-click path is covered
    /// live by `chat_focus_smoke_verify`.
    fn fire_pointer_click(world: &mut World, entity: Entity) {
        use bevy::picking::{
            backend::HitData,
            pointer::{Location, PointerButton, PointerId},
        };

        let location = Location {
            target: bevy::camera::NormalizedRenderTarget::None {
                width: 0,
                height: 0,
            },
            position: Vec2::ZERO,
        };
        let click = Click {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: std::time::Duration::ZERO,
            count: 1,
        };
        world.trigger(Pointer::new_without_propagate(
            PointerId::Mouse,
            location,
            click,
            entity,
        ));
    }

    /// **The click-driven focus path — the coverage every one of the three
    /// prior "fixes" (PRs #102/#141/#149) was missing.** They only ever tested
    /// Enter-driven focus; Matías reported the box being unfocusable by *click*
    /// specifically. A real `Pointer<Click>` at the actually-spawned input box
    /// must set `InputFocus` onto it AND un-collapse a minimized panel — the
    /// mouse counterpart to
    /// `enter_focuses_the_chat_input_and_uncollapses_the_panel`.
    #[test]
    fn clicking_the_input_box_focuses_chat_and_uncollapses_the_panel() {
        let mut app = new_app();
        app.init_resource::<InputFocus>();
        app.world_mut()
            .run_system_once(spawn_chat_panel)
            .expect("spawn_chat_panel runs");

        let input = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<ChatInputBox>>()
                .single(world)
                .expect("ChatInputBox exists")
        };
        // Start from the worst case: collapsed AND unfocused.
        app.world_mut().resource_mut::<ChatUiState>().collapsed = true;
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            None,
            "precondition: nothing focused"
        );

        fire_pointer_click(app.world_mut(), input);

        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            Some(input),
            "a real Pointer<Click> on the input box must focus it — the click path, not just Enter"
        );
        assert!(
            !app.world().resource::<ChatUiState>().collapsed,
            "clicking the box must also un-collapse the panel"
        );
    }

    /// The input box must stay default-pickable (no `Pickable` override) so the
    /// picking backend hit-tests it for the click-to-focus path, and the input
    /// line's mode icon sibling must be `Pickable::IGNORE` so it never steals
    /// that focus click.
    #[test]
    fn input_box_is_pickable_and_the_mode_icon_ignores_the_pointer() {
        use bevy::picking::Pickable;
        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_chat_panel)
            .expect("spawn_chat_panel runs");

        let world = app.world_mut();
        let mode_icon = world
            .query_filtered::<Entity, With<ChatInputModeIcon>>()
            .single(world)
            .expect("mode icon exists");
        let input = world
            .query_filtered::<Entity, With<ChatInputBox>>()
            .single(world)
            .expect("input box exists");

        assert_eq!(
            world.get::<Pickable>(mode_icon).copied(),
            Some(Pickable::IGNORE),
            "the mode icon must be Pickable::IGNORE so a click reaches the input box"
        );
        assert!(
            world.get::<Pickable>(input).is_none(),
            "the input box must stay default-pickable (no Pickable override) so the picking \
             backend hit-tests it"
        );
    }

    /// The scrollback box is the light translucent-black legacy overlay
    /// (`CHAT_BG`), NOT the theme's opaque `panel_bg` — the fix for the
    /// "heavy window" look Matías reported.
    #[test]
    fn scrollback_uses_the_translucent_legacy_overlay_not_the_opaque_panel_fill() {
        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_chat_panel)
            .expect("spawn_chat_panel runs");
        let world = app.world_mut();
        let scroll = world
            .query_filtered::<Entity, With<ChatScrollArea>>()
            .single(world)
            .expect("scroll area exists");
        assert_eq!(
            world.get::<BackgroundColor>(scroll).unwrap().0,
            CHAT_BG,
            "the message box must use the light legacy overlay, not the opaque panel fill"
        );
    }

    /// The blank/0px root cause of "the box renders empty and can't be
    /// clicked": an empty `Text` node collapses to ~0px. The box must carry
    /// a `min_height` floor so it is always a visible, clickable rectangle
    /// even while empty.
    #[test]
    fn input_box_has_a_min_height_floor_so_it_is_clickable_while_empty() {
        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_chat_panel)
            .expect("spawn_chat_panel runs");

        let world = app.world_mut();
        let input = world
            .query_filtered::<Entity, With<ChatInputBox>>()
            .single(world)
            .expect("input box exists");
        assert_eq!(
            world.get::<Node>(input).unwrap().min_height,
            Val::Px(INPUT_MIN_HEIGHT_PX),
            "the empty input box must keep a non-zero min height or it collapses to an \
             unclickable, invisible 0px line"
        );
    }

    #[test]
    fn enter_while_already_focused_does_not_reset_the_focus_cause() {
        use xindeler_input::{KeyMap, action_state::update_action_state};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        let input = app.world_mut().spawn(ChatInputBox).id();
        app.insert_resource(InputFocus::default());
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(input, FocusCause::Pressed);
        app.add_systems(Update, (update_action_state, focus_chat_via_hotkey).chain());

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Enter);
        app.update();

        assert_eq!(app.world().resource::<InputFocus>().get(), Some(input));
    }

    // ---- command parsing / completion --------------------------------------

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

    #[test]
    fn command_completion_matches_prefix_and_preserves_the_rest_of_the_line() {
        assert_eq!(matching_commands("s"), vec!["say"]);
        assert_eq!(matching_commands("r"), vec!["region"]);
        assert_eq!(
            replace_command_name("/w Bob hello", "world"),
            "/world Bob hello"
        );
        assert_eq!(replace_command_name("/sa", "say"), "/say");
    }

    /// Tab completion cycles the command-name token in place on the buffer.
    #[test]
    fn tab_completion_cycles_the_command_name_on_the_buffer() {
        let mut ci = ChatInput::default();
        ci.set_line("/w hi".to_owned());
        complete_command(&mut ci);
        assert_eq!(ci.buffer, "/world hi");
        assert_eq!(ci.cursor, ci.buffer.len());
    }

    // ---- responsive sizing -------------------------------------------------

    #[test]
    fn chat_scroll_height_clamps_and_grows_with_window_height() {
        assert_eq!(chat_scroll_height(100.0), MIN_SCROLL_HEIGHT_PX);
        assert_eq!(chat_scroll_height(4000.0), MAX_SCROLL_HEIGHT_PX);
        let short = chat_scroll_height(600.0);
        let tall = chat_scroll_height(900.0);
        assert!(
            tall > short,
            "a taller window must yield a taller scrollback"
        );
    }

    /// The BL-82 HUD-responsive-scaling regression guard (PR #138), updated
    /// for the chat-panel-polish pass: `chat_panel_bottom` (not a flat
    /// constant any more) must keep the panel clear of the health orb's own
    /// bounding box at every window width, whether that width lands in the
    /// "orb reaches into the corner" danger band (needs the
    /// [`PANEL_BOTTOM_LIFTED_PX`] fallback) or not (safe to sit flush at
    /// [`PANEL_BOTTOM_CORNER_PX`]). Sweeps a wide range — narrower AND wider
    /// than [`chat_panel_needs_lift`]'s own doc-comment-documented
    /// `1010`–`1970px` danger band — so a future geometry change can't
    /// silently shrink/misplace that band without tripping this test.
    #[test]
    fn chat_panel_never_overlaps_the_health_orb_bounding_box_at_any_window_size() {
        const CHROME_HEIGHT_PX: f32 = 140.0;
        let window_sizes: &[(f32, f32)] = &[
            (480.0, 320.0),
            (800.0, 600.0),
            (960.0, 540.0),
            (1010.0, 600.0),
            (1280.0, 720.0),
            (1440.0, 900.0),
            (1600.0, 900.0),
            (1920.0, 1080.0),
            (1970.0, 1080.0),
            (2200.0, 1200.0),
            (2560.0, 1440.0),
        ];

        for &(width, height) in window_sizes {
            let bottom = chat_panel_bottom(width);
            let chat_left = PANEL_LEFT_PX;
            let chat_right = PANEL_LEFT_PX + PANEL_WIDTH;
            let chat_top_from_bottom = bottom + chat_scroll_height(height) + CHROME_HEIGHT_PX;
            let chat_bottom_from_bottom = bottom;

            let (orb_left, orb_right) = hud_layout::health_orb_screen_x(width);
            let orb_bottom_from_bottom = hud_layout::CLUSTER_BOTTOM_PX;
            let orb_top_from_bottom = hud_layout::CLUSTER_BOTTOM_PX + hud_layout::ORB_SIZE_PX;

            let x_overlaps = chat_left < orb_right && orb_left < chat_right;
            let y_overlaps = chat_bottom_from_bottom < orb_top_from_bottom
                && orb_bottom_from_bottom < chat_top_from_bottom;

            assert!(
                !(x_overlaps && y_overlaps),
                "chat panel overlaps the health orb at window size {width}x{height}"
            );
        }
    }

    /// [`chat_panel_needs_lift`]/[`chat_panel_bottom`]: a narrow window (the
    /// orb off-screen-left) and a very wide one (the orb slid clear to the
    /// right) both get the true corner margin; common desktop widths land in
    /// the danger band and get lifted above the whole cluster instead.
    ///
    /// **BL-82 chat visual-rebuild note**: widening [`PANEL_WIDTH`] to legacy's
    /// `470` (from `320`) pushes the box's right edge out to `PANEL_LEFT_PX +
    /// PANEL_WIDTH = 486`, so at 1920px (1080p) the health orb's left edge
    /// (`x≈426`, measured via [`hud_layout::health_orb_screen_x`]) now falls
    /// INSIDE the panel's `x` span again — 1080p is back in the danger band
    /// with the wider legacy-width box (the lift correctly clears the cluster).
    /// 1280px stays in the band; the narrow (480px) and very-wide (2560px)
    /// extremes stay safe for the true corner margin.
    #[test]
    fn chat_panel_bottom_sits_flush_in_the_corner_except_in_the_orb_danger_band() {
        assert!(!chat_panel_needs_lift(480.0), "narrow: orb is off-screen");
        assert_eq!(chat_panel_bottom(480.0), PANEL_BOTTOM_CORNER_PX);

        assert!(
            chat_panel_needs_lift(1280.0),
            "1280px is a common desktop width squarely in the danger band"
        );
        assert_eq!(chat_panel_bottom(1280.0), PANEL_BOTTOM_LIFTED_PX);

        assert!(
            chat_panel_needs_lift(1920.0),
            "1920px (1080p): the wider legacy-width (470px) box now reaches the orb's x span, so \
             it needs the cluster-clearing lift (see this test's own doc comment)"
        );
        assert_eq!(chat_panel_bottom(1920.0), PANEL_BOTTOM_LIFTED_PX);

        assert!(
            !chat_panel_needs_lift(2560.0),
            "very wide: the orb has slid clear past the panel's right edge"
        );
        assert_eq!(chat_panel_bottom(2560.0), PANEL_BOTTOM_CORNER_PX);
    }

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

        app.world_mut()
            .get_mut::<Window>(window_entity)
            .unwrap()
            .resolution = bevy::window::WindowResolution::new(1280, 2000);
        app.world_mut()
            .run_system_once(sync_chat_scroll_height_to_window)
            .expect("system runs");

        let node = app.world().get::<Node>(scroll_area).unwrap();
        assert_eq!(node.height, Val::Px(chat_scroll_height(2000.0)));
    }
}
