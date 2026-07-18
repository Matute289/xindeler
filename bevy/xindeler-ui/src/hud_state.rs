//! BL-82 EM-5.1 T56.4 — the HUD state machine, replacing legacy `voxygen`'s
//! `Hud`/`Show` (`voxygen/src/hud/mod.rs`, "the god file": "a flat `pub
//! struct Show` bag of booleans/sub-states gates which panels render each
//! frame").
//!
//! v1 is deliberately small: [`HudWindow`] is the ONE currently-open
//! secondary window (an enum, not a bag of independent booleans — this is
//! what makes mutual exclusivity structural rather than "every screen
//! remembers to close every sibling window"), and [`HudAction`] is the
//! generic HUD→gameplay action event every later screen (respawn, a settings
//! change, a hotbar drag-drop) reports through, replacing legacy's ~76-variant
//! `Event` enum with an open, per-screen-extensible message instead of one
//! giant enum every screen must add a variant to.

use bevy::ecs::{
    message::{Message, MessageReader},
    resource::Resource,
    system::ResMut,
};

/// The one currently-open secondary/full-screen window. `None` = just the
/// always-on combat HUD (globes/hotbar/buff strip — EM-5.2's proof slice).
/// An enum (not independent booleans) makes "only one window open at a time"
/// structural: setting a new value always closes whatever was open before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HudWindow {
    #[default]
    None,
    Inventory,
    Diary,
    Map,
    Social,
    Chat,
    Crafting,
    Settings,
    /// BL-82 EM-5.11 — the input-rebinding screen (`xindeler-client::
    /// controls_screen`). Its own window ahead of EM-5.12 folding it in as a
    /// tab of the full esc-menu settings window (§ that epic's task board
    /// note) — this is a real, standalone entry point, not a stub.
    Controls,
    /// BL-82 EM-5.12 — the Escape/pause menu (`xindeler-client::esc_menu`):
    /// Resume + a Video/Graphics settings tab. Opening it pauses interaction
    /// like any other window (it participates in [`HudState::any_window_open`],
    /// so the cursor frees automatically). Legacy `voxygen`'s esc menu carried
    /// more entries (Settings tabs Interface/Gameplay/Controls/Sound/Video/
    /// Language, plus Character Selection / Report Bug / Logout / Quit,
    /// `voxygen/src/hud/esc_menu.rs`) — those remain future scope; this variant
    /// is the Resume + Video-settings slice.
    EscMenu,
    /// BL-82 EM-5.16 (T56.43) — the first-run tutorial overlay
    /// (`xindeler-client::tutorial_overlay`): a dismissible panel listing
    /// basic-controls tips, shown automatically once (`XindelerSettings::
    /// tutorial.seen`) and re-openable any time afterwards (e.g. the
    /// Accessibility settings tab's "Show tutorial again" button).
    /// Participates in the mutually-exclusive window slot + cursor-free rule
    /// exactly like every other real window above.
    Tutorial,
}

/// The current HUD window state. `Resource`, not per-entity — there is
/// exactly one HUD per client.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HudState {
    open_window: HudWindow,
}

impl HudState {
    #[must_use]
    pub fn open_window(&self) -> HudWindow { self.open_window }

    #[must_use]
    pub fn is_open(&self, window: HudWindow) -> bool { self.open_window == window }

    /// Whether ANY secondary/full-screen window is currently open (i.e.
    /// [`open_window`](Self::open_window) is not [`HudWindow::None`]).
    ///
    /// This is the "a modal-like panel needs the pointer" half of the shared
    /// cursor-free rule (BL-82 EM-5.17 — the "cursor doesn't appear when a UI
    /// panel is open" fix): whenever this is `true`, the OS cursor must be
    /// visible + ungrabbed so the player can actually click the panel's
    /// controls, and camera mouselook must be suspended. It is the Bevy port
    /// of legacy `voxygen`'s `Show::any_window_requires_cursor()`
    /// (`voxygen/src/hud/mod.rs`), except that this project's single
    /// mutually-exclusive [`HudWindow`] slot collapses legacy's
    /// OR-of-every-window-boolean into one comparison — so EVERY existing
    /// window (Inventory/Diary/Map/Social/Crafting/Settings/Controls) AND
    /// every window added later (e.g. the EM-5.12 esc/pause menu) participates
    /// in the cursor rule automatically, with no per-window bookkeeping to
    /// forget. The client's cursor aggregator combines this with chat-input
    /// focus (`chat::text_input_focused`) — the two together are the full "is
    /// the cursor free" predicate, mirroring legacy's `want_grab =
    /// !any_window_requires_cursor() && !typing()`.
    #[must_use]
    pub fn any_window_open(&self) -> bool { self.open_window != HudWindow::None }

    /// Opens `window`, closing whatever was previously open (a no-op toggle:
    /// re-toggling the ALREADY-open window closes it back to `None`,
    /// matching legacy's own toggle-key convention for e.g. the inventory
    /// key).
    pub fn toggle(&mut self, window: HudWindow) {
        self.open_window = if self.open_window == window {
            HudWindow::None
        } else {
            window
        };
    }

    /// Closes whatever window is open (the Esc-key action).
    pub fn close(&mut self) { self.open_window = HudWindow::None; }
}

/// The HUD→gameplay action event flow (replaces legacy's giant `Event`
/// enum). v1 carries only what EM-5.1/5.2 need; later screens add variants
/// as they land (an open enum in one crate, not per-screen boolean flags
/// scattered across the app).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudAction {
    /// A window's toggle control (button/keybind) was activated.
    ToggleWindow(HudWindow),
    /// The Esc/close control was activated.
    CloseWindow,
    /// The death screen's respawn button was activated (EM-5.2).
    Respawn,
}

/// Applies [`HudAction::ToggleWindow`]/[`HudAction::CloseWindow`] to
/// [`HudState`] — the generic action->state wiring every screen epic that
/// opens a REAL secondary window needs. Added here (the foundation crate,
/// alongside [`HudState`]/[`HudAction`] themselves) rather than duplicated
/// per-screen (BL-82 EM-5.5 is the first screen epic to need a real
/// secondary window — EM-5.2's proof slice only ever wrote `HudAction`, it
/// never needed anything to read `ToggleWindow`/`CloseWindow` back out).
/// [`HudAction::Respawn`] (and any future screen-specific variant) is left
/// for that screen's own system to drain, exactly as `combat_hud::
/// handle_respawn_button` already does — this system only owns the two
/// generic window-state variants.
pub fn apply_hud_actions(mut state: ResMut<HudState>, mut actions: MessageReader<HudAction>) {
    for action in actions.read() {
        match action {
            HudAction::ToggleWindow(window) => state.toggle(*window),
            HudAction::CloseWindow => state.close(),
            HudAction::Respawn => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening a second window closes the first — the T56.4 "two exclusive
    /// windows can't co-open" acceptance bar.
    #[test]
    fn opening_a_window_closes_the_previous_one() {
        let mut state = HudState::default();
        state.toggle(HudWindow::Inventory);
        assert!(state.is_open(HudWindow::Inventory));

        state.toggle(HudWindow::Diary);
        assert!(
            !state.is_open(HudWindow::Inventory),
            "opening Diary must close Inventory — only one window open at a time"
        );
        assert!(state.is_open(HudWindow::Diary));
    }

    /// Re-toggling the already-open window closes it (back to `None`).
    #[test]
    fn re_toggling_the_open_window_closes_it() {
        let mut state = HudState::default();
        state.toggle(HudWindow::Map);
        state.toggle(HudWindow::Map);
        assert_eq!(state.open_window(), HudWindow::None);
    }

    /// [`HudState::any_window_open`] is the "any modal-like panel needs the
    /// pointer" half of the shared cursor-free rule (BL-82 EM-5.17): `false`
    /// only when nothing is open, `true` for every real window — including a
    /// window variant added later (proven here with [`HudWindow::Settings`],
    /// the esc/pause-menu family EM-5.12 grows, so the cursor rule keeps
    /// covering new panels with no extra bookkeeping).
    #[test]
    fn any_window_open_tracks_the_open_slot() {
        let mut state = HudState::default();
        assert!(
            !state.any_window_open(),
            "a fresh HUD (no window open) must report no modal — cursor stays grabbed for \
             mouselook"
        );

        state.toggle(HudWindow::Diary);
        assert!(
            state.any_window_open(),
            "opening a window must report a modal — cursor must free up so its controls are \
             clickable"
        );

        state.toggle(HudWindow::Settings);
        assert!(
            state.any_window_open(),
            "a later-added window variant (the esc/pause menu family) participates automatically"
        );

        state.close();
        assert!(
            !state.any_window_open(),
            "closing the last window must return to no-modal — cursor re-grabs for mouselook"
        );
    }

    /// [`apply_hud_actions`] wires `HudAction::ToggleWindow`/`CloseWindow`
    /// onto the real [`HudState`] — the BL-82 EM-5.5 acceptance bar for this
    /// foundational addition (nothing drained these two variants before).
    #[test]
    fn apply_hud_actions_toggles_and_closes_the_real_state() {
        use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HudState>();
        app.add_message::<HudAction>();

        app.world_mut()
            .write_message(HudAction::ToggleWindow(HudWindow::Map));
        app.world_mut()
            .run_system_once(apply_hud_actions)
            .expect("system runs");
        assert!(app.world().resource::<HudState>().is_open(HudWindow::Map));

        app.world_mut().write_message(HudAction::CloseWindow);
        app.world_mut()
            .run_system_once(apply_hud_actions)
            .expect("system runs again");
        assert!(!app.world().resource::<HudState>().is_open(HudWindow::Map));
    }

    /// `HudAction::Respawn` (a screen-specific variant) leaves `HudState`
    /// untouched — this system only owns the two generic window variants.
    #[test]
    fn apply_hud_actions_ignores_screen_specific_variants() {
        use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HudState>();
        app.add_message::<HudAction>();

        app.world_mut()
            .write_message(HudAction::ToggleWindow(HudWindow::Inventory));
        app.world_mut().write_message(HudAction::Respawn);
        app.world_mut()
            .run_system_once(apply_hud_actions)
            .expect("system runs");

        assert!(
            app.world()
                .resource::<HudState>()
                .is_open(HudWindow::Inventory),
            "Respawn must not clobber whatever window Toggle just opened"
        );
    }
}
