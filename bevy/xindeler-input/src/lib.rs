//! BL-82 EM-5.11 — input rebinding + full gamepad support.
//!
//! A pure input-MODEL crate (no rendering/window/UI concern of its own):
//! [`game_input::GameInput`] (the rebindable action enum, ported from the
//! legacy client), [`keybind::KeyBindings`] (keyboard/mouse) +
//! [`gamepad::GamepadBindings`] (buttons + modifier-chord combos + analog
//! axes with deadzone/inversion) combined into the persisted
//! [`keymap::KeyMap`] Resource, the per-frame resolved [`action_state::
//! ActionState`], and the rebind-capture flow ([`capture`]) the EM-5.11
//! rebinding UI (`xindeler-client::controls_screen`) drives.
//!
//! `xindeler-app`'s `XindelerSettings.controls: KeyMap` field is where this
//! persists (see that crate's `settings.rs` — extended, not forked).

pub mod action_state;
pub mod capture;
pub mod game_input;
pub mod gamepad;
pub mod keybind;
pub mod keymap;

use bevy::{
    app::{App, Plugin, Update},
    ecs::schedule::{IntoScheduleConfigs, SystemSet},
};

pub use crate::{
    action_state::ActionState,
    capture::{RebindOutcome, RebindRequest, RebindTarget},
    game_input::GameInput,
    gamepad::{AxisAction, GamepadBinding, GamepadBindings},
    keybind::{KeyBindings, KeyOrMouse},
    keymap::KeyMap,
};

/// System set [`action_state::update_action_state`] runs in — gameplay
/// systems that read [`ActionState`] order `.after(InputResolveSet)`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputResolveSet;

/// Installs the input-model layer: [`KeyMap`] (if not already provided by
/// `XindelerSettings`), [`ActionState`], the [`RebindRequest`]/
/// [`RebindOutcome`] rebind-capture flow, and the two per-frame systems
/// ([`action_state::update_action_state`] then [`capture::capture_rebind`] —
/// capture runs AFTER state resolution so a rebind's own triggering press
/// doesn't also fire as a stale action this same frame under the OLD
/// binding).
pub struct XindelerInputPlugin;

impl Plugin for XindelerInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeyMap>()
            .init_resource::<ActionState>()
            .init_resource::<RebindRequest>()
            .add_message::<RebindOutcome>()
            .add_systems(
                Update,
                (
                    action_state::update_action_state.in_set(InputResolveSet),
                    capture::capture_rebind.after(InputResolveSet),
                ),
            );
    }
}
