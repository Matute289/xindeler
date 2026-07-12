//! BL-82 EM-5.11 (T56.11/T56.12) — [`KeyMap`], the top-level persisted
//! Resource: keyboard/mouse bindings ([`crate::keybind::KeyBindings`]) +
//! gamepad bindings ([`crate::gamepad::GamepadBindings`]). This is the value
//! `xindeler-app`'s `XindelerSettings.controls` field holds — persisted via
//! the EXISTING `settings.ron` save/load seam (extended, not forked; see
//! that crate's own `settings.rs`).

use bevy::ecs::resource::Resource;
use serde::{Deserialize, Serialize};

use crate::{gamepad::GamepadBindings, keybind::KeyBindings};

/// All rebindable input for the client: keyboard/mouse + gamepad, together.
/// `#[serde(default)]` on both fields (via each field's own `Default`/delta
/// serde impl) means an OLD `settings.ron` predating this section — or the
/// `controls` KEY simply being absent — loads as
/// `KeyBindings::default()`/`GamepadBindings::default()`, same backward-
/// compat guarantee `XindelerSettings::ui_scale` already established.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KeyMap {
    pub keyboard: KeyBindings,
    pub gamepad: GamepadBindings,
}

#[cfg(test)]
mod tests {
    use bevy::input::{gamepad::GamepadButton, keyboard::KeyCode};

    use super::*;
    use crate::{game_input::GameInput, gamepad::GamepadBinding, keybind::KeyOrMouse};

    /// A completely fresh `KeyMap` round-trips through RON as an (almost)
    /// empty document — both sub-sections serialize as empty deltas (see
    /// each module's own delta test); this is the top-level integration
    /// check that `settings.ron`'s `controls` section on a fresh install is
    /// small, not a 90-action dump.
    #[test]
    fn a_fresh_keymap_round_trips_and_keeps_its_defaults() {
        let keymap = KeyMap::default();
        let text = ron::ser::to_string_pretty(&keymap, ron::ser::PrettyConfig::default())
            .expect("KeyMap serializes");
        let round_tripped: KeyMap = ron::from_str(&text).expect("KeyMap deserializes");
        assert_eq!(
            round_tripped.keyboard.get_binding(GameInput::MoveForward),
            Some(KeyOrMouse::Key(KeyCode::KeyW))
        );
        assert_eq!(
            round_tripped.gamepad.get_button_binding(GameInput::Jump),
            Some(GamepadBinding::Button(GamepadButton::South))
        );
    }

    /// Missing the WHOLE `controls` section (predates EM-5.11 entirely)
    /// still parses to full defaults — the exact scenario a real pre-EM-5.11
    /// `settings.ron` hits once `XindelerSettings` gains this field.
    #[test]
    fn an_empty_document_parses_to_full_defaults() {
        let keymap: KeyMap = ron::from_str("()").expect("empty KeyMap parses");
        assert_eq!(
            keymap.keyboard.get_binding(GameInput::Jump),
            Some(KeyOrMouse::Key(KeyCode::Space))
        );
    }
}
