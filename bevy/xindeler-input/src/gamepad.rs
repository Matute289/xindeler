//! BL-82 EM-5.11 (T56.13) — full gamepad support (§Q5=A, included in v1).
//!
//! Modelled on the legacy client's `voxygen/src/settings/controller.rs`
//! (~1200 LOC, gilrs-backed): button map + modifier-chord "layer" combos +
//! analog stick axis map with per-axis deadzone/inversion. This is NOT a
//! line-for-line port — most of that file's bulk is a **menu-navigation**
//! input layer (`MenuInput`/`AnalogButtonMenuAction`/`AxisMenuAction`) for
//! conrod's gamepad-driven menu cursor, which has no equivalent yet in this
//! Bevy client (no screen does gamepad UI navigation as of EM-5.1/5.2) — that
//! is a real, deliberate, DOCUMENTED deferral (follows the same screen as it
//! arrives, e.g. EM-5.9's server browser), not a silently dropped feature.
//! What full gamepad parity for the *game* (not menus) means, and what this
//! module implements in full: **button bindings** (game_button_map),
//! **modifier-chord combos** (layer_button_map: hold a modifier button to
//! reach an alternate action on the same physical button), **deadzones**
//! (per-axis, same rescale formula as `apply_axis_deadzone`), and **axis
//! inversion** (`inverted_axes`).

use std::collections::{HashMap, HashSet};

use bevy::input::gamepad::{GamepadAxis, GamepadButton};
use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoEnumIterator};

use crate::game_input::GameInput;

/// A physical gamepad input a [`GameInput`] can bind to: either a plain
/// button, or a **modifier chord** — `button` only fires the action while
/// `modifier` is ALSO held. This is the "layer" mechanic from legacy's
/// `layer_button_map`: e.g. holding a shoulder button remaps the face
/// buttons to a second layer of actions, doubling the effective button count
/// without adding physical buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadBinding {
    Button(GamepadButton),
    Chord {
        modifier: GamepadButton,
        button: GamepadButton,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct GamepadBindingsSerde {
    button_map: HashMap<GameInput, Option<GamepadBinding>>,
    axis_map: HashMap<AxisAction, AxisBinding>,
    axis_deadzones: HashMap<AxisAction, f32>,
}

impl From<GamepadBindings> for GamepadBindingsSerde {
    fn from(bindings: GamepadBindings) -> Self {
        let mut button_delta = HashMap::new();
        for (input, binding) in bindings.button_map {
            if GamepadBindings::default_button_binding(input) != binding {
                button_delta.insert(input, binding);
            }
        }
        let mut axis_delta = HashMap::new();
        for (action, binding) in bindings.axis_map {
            if AxisAction::default_binding(action) != binding {
                axis_delta.insert(action, binding);
            }
        }
        let mut deadzone_delta = HashMap::new();
        for (action, deadzone) in bindings.axis_deadzones {
            if (deadzone - DEFAULT_AXIS_DEADZONE).abs() > f32::EPSILON {
                deadzone_delta.insert(action, deadzone);
            }
        }
        GamepadBindingsSerde {
            button_map: button_delta,
            axis_map: axis_delta,
            axis_deadzones: deadzone_delta,
        }
    }
}

impl Default for GamepadBindingsSerde {
    fn default() -> Self { GamepadBindings::default().into() }
}

/// The analog stick axes [`GameInput`]-independent movement/look are driven
/// from — kept separate from the digital [`GameInput`] buttons the same way
/// legacy split `game_axis_map`/`AxisGameAction` from `game_button_map`:
/// movement/camera are continuous quantities, not press/release actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter)]
pub enum AxisAction {
    MoveX,
    MoveY,
    LookX,
    LookY,
}

/// A logical [`AxisAction`] resolves to one physical [`GamepadAxis`], with
/// its own inversion flag (legacy's `inverted_axes`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxisBinding {
    pub axis: GamepadAxis,
    pub inverted: bool,
}

impl AxisAction {
    #[must_use]
    pub fn default_binding(self) -> AxisBinding {
        let (axis, inverted) = match self {
            AxisAction::MoveX => (GamepadAxis::LeftStickX, false),
            AxisAction::MoveY => (GamepadAxis::LeftStickY, false),
            AxisAction::LookX => (GamepadAxis::RightStickX, false),
            // Vertical look is the one axis players commonly invert
            // (flight-sim convention); default un-inverted, same as legacy.
            AxisAction::LookY => (GamepadAxis::RightStickY, false),
        };
        AxisBinding { axis, inverted }
    }
}

/// Default per-axis-ACTION deadzone (legacy's own fallback constant in
/// `apply_axis_deadzone`: `unwrap_or(&0.2)`). Kept per-action (not per
/// physical-axis, as legacy's `HashMap<Axis, f32>` was) since that's the
/// more useful tuning granularity for a player (e.g. a looser look-stick
/// deadzone than a movement-stick deadzone) and this crate's `AxisAction`
/// set is small/stable enough that this is not a meaningfully different
/// shape in practice.
pub const DEFAULT_AXIS_DEADZONE: f32 = 0.2;

/// Gamepad bindings for every [`GameInput`] (buttons + chords) plus the
/// analog stick axis map (movement/look), with the reverse index conflict
/// detection needs — the exact same shape [`crate::keybind::KeyBindings`]
/// uses for keyboard/mouse, applied to gamepad inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "GamepadBindingsSerde", into = "GamepadBindingsSerde")]
pub struct GamepadBindings {
    pub button_map: HashMap<GameInput, Option<GamepadBinding>>,
    pub inverse_button_map: HashMap<GamepadBinding, HashSet<GameInput>>,
    pub axis_map: HashMap<AxisAction, AxisBinding>,
    pub axis_deadzones: HashMap<AxisAction, f32>,
}

impl From<GamepadBindingsSerde> for GamepadBindings {
    fn from(serde: GamepadBindingsSerde) -> Self {
        let mut bindings = GamepadBindings::default();
        for (input, maybe_binding) in serde.button_map {
            match maybe_binding {
                Some(binding) => bindings.modify_button_binding(input, binding),
                None => bindings.remove_button_binding(input),
            }
        }
        for (action, binding) in serde.axis_map {
            bindings.axis_map.insert(action, binding);
        }
        for (action, deadzone) in serde.axis_deadzones {
            bindings.axis_deadzones.insert(action, deadzone);
        }
        bindings
    }
}

impl GamepadBindings {
    pub fn remove_button_binding(&mut self, game_input: GameInput) {
        if let Some(inverse) = self
            .button_map
            .insert(game_input, None)
            .flatten()
            .and_then(|binding| self.inverse_button_map.get_mut(&binding))
        {
            inverse.remove(&game_input);
        }
    }

    #[must_use]
    pub fn get_button_binding(&self, game_input: GameInput) -> Option<GamepadBinding> {
        self.button_map.get(&game_input).copied().flatten()
    }

    #[must_use]
    pub fn get_associated_game_inputs(
        &self,
        binding: &GamepadBinding,
    ) -> Option<&HashSet<GameInput>> {
        self.inverse_button_map.get(binding)
    }

    pub fn modify_button_binding(&mut self, game_input: GameInput, binding: GamepadBinding) {
        if let Some(old) = self.get_button_binding(game_input)
            && let Some(inverse) = self.inverse_button_map.get_mut(&old)
        {
            inverse.remove(&game_input);
        }
        self.inverse_button_map
            .entry(binding)
            .or_default()
            .insert(game_input);
        self.button_map.insert(game_input, Some(binding));
    }

    /// Same shape as [`crate::keybind::KeyBindings::has_conflicting_bindings`]
    /// — surfaces (rather than silently allows) two unrelated actions bound
    /// to the same physical button/chord.
    #[must_use]
    pub fn has_conflicting_bindings(&self, binding: GamepadBinding) -> bool {
        let Some(inputs) = self.inverse_button_map.get(&binding) else {
            return false;
        };
        inputs
            .iter()
            .any(|&a| inputs.iter().any(|&b| !GameInput::can_share_bindings(a, b)))
    }

    /// A reasonable modern-controller default layout. Deliberately partial
    /// (not every one of the ~90 [`GameInput`]s has a natural face-button
    /// mapping — e.g. `Slot1..Slot10` stay keyboard-only in v1, same
    /// documented-deferral posture as this module's menu-input note) rather
    /// than force awkward bindings onto every action just to hit a count.
    #[must_use]
    pub fn default_button_binding(game_input: GameInput) -> Option<GamepadBinding> {
        use GamepadButton as B;
        let button = |b: GamepadButton| Some(GamepadBinding::Button(b));
        let chord = |modifier: GamepadButton, button: GamepadButton| {
            Some(GamepadBinding::Chord { modifier, button })
        };
        match game_input {
            GameInput::Jump => button(B::South),
            GameInput::Roll => button(B::East),
            GameInput::Interact => button(B::West),
            GameInput::ToggleWield => button(B::North),
            GameInput::Primary => button(B::RightTrigger2),
            GameInput::Secondary => button(B::LeftTrigger2),
            GameInput::Block => button(B::LeftTrigger),
            GameInput::ToggleLantern => button(B::RightTrigger),
            GameInput::Map => button(B::Select),
            GameInput::Escape => button(B::Start),
            GameInput::SwapLoadout => button(B::RightThumb),
            GameInput::Sneak => button(B::LeftThumb),
            GameInput::Inventory => button(B::DPadUp),
            GameInput::Diary => button(B::DPadDown),
            GameInput::Social => button(B::DPadLeft),
            GameInput::Crafting => button(B::DPadRight),
            // Modifier-chord example (legacy's "layer" mechanic): holding
            // the LEFT trigger (already `Block`'s plain binding) while
            // pressing South reaches Sit instead of Jump — one physical
            // button, two actions, gated by the modifier layer.
            GameInput::Sit => chord(B::LeftTrigger, B::South),
            _ => None,
        }
    }
}

impl Default for GamepadBindings {
    fn default() -> Self {
        let mut bindings = GamepadBindings {
            button_map: HashMap::new(),
            inverse_button_map: HashMap::new(),
            axis_map: HashMap::new(),
            axis_deadzones: HashMap::new(),
        };
        for game_input in GameInput::iter() {
            if let Some(default) = GamepadBindings::default_button_binding(game_input) {
                bindings.modify_button_binding(game_input, default);
            } else {
                bindings.button_map.insert(game_input, None);
            }
        }
        for action in AxisAction::iter() {
            bindings
                .axis_map
                .insert(action, AxisAction::default_binding(action));
            bindings
                .axis_deadzones
                .insert(action, DEFAULT_AXIS_DEADZONE);
        }
        bindings
    }
}

impl GamepadBindings {
    /// Applies this action's configured deadzone + inversion to a raw
    /// `[-1.0, 1.0]` axis reading. Same radial-rescale formula as legacy's
    /// `apply_axis_deadzone` (linearly remaps the post-deadzone range back
    /// out to the full `[0, 1]` magnitude instead of leaving a dead jump at
    /// the deadzone boundary).
    #[must_use]
    pub fn apply_axis(&self, action: AxisAction, raw: f32) -> f32 {
        let deadzone = *self
            .axis_deadzones
            .get(&action)
            .unwrap_or(&DEFAULT_AXIS_DEADZONE);
        let inverted = self
            .axis_map
            .get(&action)
            .map(|b| b.inverted)
            .unwrap_or(false);
        let value = apply_deadzone(deadzone, raw);
        if inverted { -value } else { value }
    }
}

/// Pure deadzone-rescale function (unit-tested independent of any ECS/App):
/// values within `[-threshold, threshold]` clamp to zero; values beyond it
/// are linearly rescaled so the output still spans the full `[-1.0, 1.0]`
/// range (no discontinuous jump right past the deadzone edge).
#[must_use]
pub fn apply_deadzone(threshold: f32, value: f32) -> f32 {
    let threshold = threshold.clamp(0.0, 1.0);
    let value_abs = value.abs();
    if value_abs <= threshold || threshold >= 1.0 {
        0.0
    } else if threshold <= 0.0 {
        value
    } else {
        (value_abs - threshold) / (1.0 - threshold) * value.signum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_an_entry_in_a_fresh_gamepad_map() {
        let bindings = GamepadBindings::default();
        for input in GameInput::iter() {
            assert!(bindings.button_map.contains_key(&input));
        }
        for action in AxisAction::iter() {
            assert!(bindings.axis_map.contains_key(&action));
            assert!(bindings.axis_deadzones.contains_key(&action));
        }
    }

    #[test]
    fn a_plain_button_binding_fires_the_action() {
        let bindings = GamepadBindings::default();
        assert_eq!(
            bindings.get_button_binding(GameInput::Jump),
            Some(GamepadBinding::Button(GamepadButton::South))
        );
    }

    /// The modifier-chord ("layer") default: `Sit` requires LeftTrigger held
    /// together with South pressed — distinct from `Jump`'s plain-South
    /// binding. This is the "hold a shoulder button to reach a second layer"
    /// mechanic.
    #[test]
    fn a_chord_binding_is_distinct_from_the_plain_button() {
        let bindings = GamepadBindings::default();
        assert_eq!(
            bindings.get_button_binding(GameInput::Sit),
            Some(GamepadBinding::Chord {
                modifier: GamepadButton::LeftTrigger,
                button: GamepadButton::South
            })
        );
        // Jump's plain South binding and Sit's chord are different keys in
        // the inverse map — pressing South alone must not ALSO satisfy Sit.
        assert_ne!(
            bindings.get_button_binding(GameInput::Jump),
            bindings.get_button_binding(GameInput::Sit)
        );
    }

    #[test]
    fn conflicting_gamepad_buttons_are_flagged() {
        let mut bindings = GamepadBindings::default();
        bindings.modify_button_binding(GameInput::Map, GamepadBinding::Button(GamepadButton::West));
        bindings.modify_button_binding(
            GameInput::Interact,
            GamepadBinding::Button(GamepadButton::West),
        );
        assert!(bindings.has_conflicting_bindings(GamepadBinding::Button(GamepadButton::West)));
    }

    /// Below the deadzone: zero. Right at 1.0: still 1.0 (no rescale
    /// overshoot). A value halfway between the deadzone and 1.0 rescales to
    /// roughly half of the FULL post-deadzone range, not half of 1.0 raw.
    #[test]
    fn deadzone_rescale_matches_the_legacy_formula() {
        assert_eq!(apply_deadzone(0.2, 0.1), 0.0);
        assert!((apply_deadzone(0.2, 1.0) - 1.0).abs() < 1e-6);
        // (0.6 - 0.2) / (1.0 - 0.2) = 0.5
        assert!((apply_deadzone(0.2, 0.6) - 0.5).abs() < 1e-6);
        // Sign is preserved for negative input.
        assert!((apply_deadzone(0.2, -0.6) + 0.5).abs() < 1e-6);
    }

    /// Axis inversion flips the sign AFTER the deadzone rescale.
    #[test]
    fn axis_inversion_flips_the_sign() {
        let mut bindings = GamepadBindings::default();
        bindings.axis_map.insert(AxisAction::LookY, AxisBinding {
            axis: GamepadAxis::RightStickY,
            inverted: true,
        });
        let normal = bindings.apply_axis(AxisAction::MoveY, 0.6);
        let inverted = bindings.apply_axis(AxisAction::LookY, 0.6);
        assert!(normal > 0.0);
        assert!(inverted < 0.0);
        assert!((normal + inverted).abs() < 1e-6);
    }

    /// Delta persistence for the gamepad map too: an untouched
    /// `GamepadBindings` serializes to empty button/axis-map/deadzone
    /// deltas.
    #[test]
    fn untouched_gamepad_bindings_round_trip_as_an_empty_delta() {
        let bindings = GamepadBindings::default();
        let serde_form: GamepadBindingsSerde = bindings.clone().into();
        assert!(serde_form.button_map.is_empty());
        assert!(serde_form.axis_map.is_empty());
        assert!(serde_form.axis_deadzones.is_empty());

        let ron_text = ron::ser::to_string(&bindings).expect("serializes");
        let round_tripped: GamepadBindings = ron::from_str(&ron_text).expect("deserializes");
        assert_eq!(
            round_tripped.get_button_binding(GameInput::Jump),
            bindings.get_button_binding(GameInput::Jump)
        );
    }
}
