//! BL-82 EM-5.17 — the cursor-free aggregator: the ONE place that decides,
//! each frame, whether the OS cursor should be free (visible + ungrabbed
//! for clicking UI) or grabbed (hidden, for camera mouselook).
//!
//! ## The bug this fixes
//! Opening the Diary/Inventory/Map or focusing the chat box did NOT free the
//! OS cursor, so none of the new panel-based HUD (BL-82 EM-5.17 phases 0-7)
//! was clickable at all — the cursor stayed hidden+grabbed for mouselook (or a
//! stray click re-grabbed it). Matías hit this live (record19.mov): a Diary
//! with plainly-visible clickable tabs, but no cursor anywhere on screen.
//!
//! ## The rule (ported from legacy `voxygen`)
//! Legacy `voxygen` computed `want_grab = !any_window_requires_cursor() &&
//! !typing()` (`voxygen/src/hud/mod.rs`) and grabbed the cursor iff
//! `want_grab`. This module is the Bevy port of that decision:
//! [`update_cursor_free`] writes [`crate::camera::CursorFree`] =
//! [`HudState::any_window_open`] OR chat-input focus. `HudState`'s single
//! mutually-exclusive [`HudWindow`] slot (open Inventory/Diary/Map/Social/
//! Crafting/Settings/Controls, and the esc/pause menu once EM-5.12 lands)
//! collapses legacy's OR-of-every-window into one comparison, so every panel —
//! including any added later — participates automatically. The camera's
//! [`crate::camera::cursor_grab`] reads that one signal and forces the cursor
//! free (and blocks click-to-grab) whenever it is set, re-grabbing for
//! mouselook once it clears.
//!
//! ## Why a resource, not a run-condition
//! `chat::text_input_focused` already exists as an `Fn(..) -> bool`
//! run-condition (gating hotkeys while typing). The cursor decision needs the
//! SAME signal, but as a *value* aggregated with `HudState` and consumed by a
//! system in `camera.rs` (which is compiled in every mode, including the
//! HUD-less demo, where `HudState`/chat don't exist). Funnelling it through
//! one always-present `CursorFree` resource — written only here, under the
//! same `listen-server`/`net-client` features that add the HUD/chat — keeps
//! `camera.rs` feature-agnostic (it just reads a bool) and gives every future
//! modal a single place to be covered.
//!
//! Compiled only under `listen-server`/`net-client`, matching every other
//! `xindeler_ui`/chat-consuming module in this crate.

use bevy::{input_focus::InputFocus, prelude::*};
use xindeler_app::AppState;
use xindeler_ui::hud_state::HudState;

use crate::{camera::CursorFree, chat::ChatInputBox};

/// Installs the cursor-free aggregator. Ordered AFTER
/// [`xindeler_ui::hud_state::apply_hud_actions`] (so it reads the `HudState`
/// a window toggle produced THIS frame, not a frame-stale one), AFTER
/// [`crate::chat::blur_chat_input_on_collapse`] (BL-82 "chat still unusable"
/// round 3 hardening, bevy-migration-reviewer finding: without this edge,
/// `update_cursor_free` could read a STALE [`InputFocus`] still pointing at
/// the just-hidden chat box on the very frame it collapses, leaving the
/// cursor free one extra — self-healing, but needless — frame; ordering
/// this after the blur guarantees it always observes the SAME frame's
/// already-cleared focus), and BEFORE [`crate::camera::FlyCamSet`] (so
/// [`crate::camera::cursor_grab`] reads the freshly-aggregated value the
/// same frame). This same-frame chain is what lets a window opened by a
/// keypress free the cursor with no visible one-frame lag, and lets the
/// camera's re-grab latch fire correctly when it closes.
pub struct CursorControlPlugin;

impl Plugin for CursorControlPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            update_cursor_free
                .after(xindeler_ui::hud_state::apply_hud_actions)
                .after(crate::chat::blur_chat_input_on_collapse)
                .before(crate::camera::FlyCamSet),
        );
    }
}

/// Aggregates the cursor-free predicate into [`CursorFree`]: `true` iff any
/// HUD window is open ([`HudState::any_window_open`]) OR the chat input box
/// currently holds keyboard focus.
///
/// The chat-focus half mirrors `chat::text_input_focused` (the canonical
/// run-condition of the same signal) — kept inline here rather than reusing
/// that system directly because a single writer must OR both signals into one
/// resource, and Bevy run-conditions compose into a system's *gating*, not
/// into a value two systems could safely co-write.
fn update_cursor_free(
    hud_state: Res<HudState>,
    focus: Res<InputFocus>,
    state: Res<State<AppState>>,
    chat_inputs: Query<Entity, With<ChatInputBox>>,
    mut cursor_free: ResMut<CursorFree>,
) {
    let chat_focused = chat_inputs
        .single()
        .is_ok_and(|entity| focus.get() == Some(entity));
    // BL-82 EM-5.9 (T56.29): the main menu + connecting screen are pure
    // `bevy_ui` overlays the player CLICKS — the cursor must stay free/visible
    // there regardless of `HudState` (there is no gameplay mouselook to grab
    // for). `Demo`/`InGame` keep the legacy window/chat-driven behaviour so the
    // demo fly-cam and real gameplay mouselook still grab as before.
    let menu_open = matches!(
        state.get(),
        AppState::MainMenu | AppState::Connecting | AppState::CharSelect
    );
    let free = menu_open || hud_state.any_window_open() || chat_focused;
    // Write only on a real change so `Changed`-gated readers (and the
    // resource's change tick) aren't churned every frame.
    if cursor_free.0 != free {
        cursor_free.0 = free;
    }
}

#[cfg(test)]
mod tests {
    use bevy::{ecs::system::RunSystemOnce, text::EditableText};
    use xindeler_ui::hud_state::HudWindow;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.init_resource::<HudState>();
        app.init_resource::<InputFocus>();
        app.init_resource::<CursorFree>();
        // `update_cursor_free` reads `State<AppState>` (BL-82 EM-5.9): insert it
        // directly (no `StatesPlugin`/transition machinery needed — these tests
        // only read `state.get()`). `InGame` keeps the legacy window/chat-driven
        // behaviour these cases assert.
        app.insert_resource(State::new(AppState::InGame));
        app
    }

    /// With a HUD window open, the aggregator resolves cursor-free = `true`
    /// (the cursor must become visible/clickable) — the core regression:
    /// opening any panel frees the cursor.
    #[test]
    fn open_window_makes_the_cursor_free() {
        let mut app = new_app();
        app.world_mut()
            .resource_mut::<HudState>()
            .toggle(HudWindow::Diary);
        // A chat input exists but is NOT focused, so only the window drives it.
        app.world_mut().spawn((ChatInputBox, EditableText::new("")));

        app.world_mut()
            .run_system_once(update_cursor_free)
            .expect("system runs");

        assert!(
            app.world().resource::<CursorFree>().0,
            "an open HUD window must resolve the cursor to free (visible/clickable)"
        );
    }

    /// With chat input focused (no window open), the aggregator still resolves
    /// cursor-free = `true` — typing in chat needs the pointer free too.
    #[test]
    fn chat_focus_makes_the_cursor_free() {
        let mut app = new_app();
        let input = app
            .world_mut()
            .spawn((ChatInputBox, EditableText::new("")))
            .id();
        app.insert_resource(InputFocus::from_entity(input));

        app.world_mut()
            .run_system_once(update_cursor_free)
            .expect("system runs");

        assert!(
            app.world().resource::<CursorFree>().0,
            "chat input focus must resolve the cursor to free"
        );
    }

    /// BL-82 EM-5.9 (T56.29): in the main menu (and the connecting screen) the
    /// cursor must be free/visible so the player can click the menu — even with
    /// no HUD window open and no chat focus (there is no gameplay mouselook to
    /// grab for). This is what makes the menu buttons clickable.
    #[test]
    fn main_menu_forces_the_cursor_free() {
        let mut app = new_app();
        // Start from a stale `false` to prove the state actively frees it.
        app.world_mut().resource_mut::<CursorFree>().0 = false;
        app.insert_resource(State::new(AppState::MainMenu));
        // No window open, no chat focus — only the AppState should free it.
        app.world_mut().spawn((ChatInputBox, EditableText::new("")));

        app.world_mut()
            .run_system_once(update_cursor_free)
            .expect("system runs");

        assert!(
            app.world().resource::<CursorFree>().0,
            "the main menu must resolve the cursor to free (visible/clickable) regardless of \
             HudState — the menu is clicked, not mouselooked"
        );
    }

    /// With everything closed and nothing focused (and not paused — a pause
    /// menu is just another open `HudWindow`), the aggregator resolves
    /// cursor-free = `false`: the cursor stays grabbed/hidden for mouselook.
    #[test]
    fn nothing_open_keeps_the_cursor_grabbed() {
        let mut app = new_app();
        // Start from a stale `true` to prove the aggregator actively clears it.
        app.world_mut().resource_mut::<CursorFree>().0 = true;
        app.world_mut().spawn((ChatInputBox, EditableText::new("")));

        app.world_mut()
            .run_system_once(update_cursor_free)
            .expect("system runs");

        assert!(
            !app.world().resource::<CursorFree>().0,
            "no window open and no chat focus must resolve the cursor to grabbed (mouselook)"
        );
    }
}
