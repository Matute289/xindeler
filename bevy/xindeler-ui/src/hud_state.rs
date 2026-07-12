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

use bevy::ecs::{message::Message, resource::Resource};

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
}
