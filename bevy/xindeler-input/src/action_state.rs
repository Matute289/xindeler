//! BL-82 EM-5.11 — [`ActionState`]: the resolved, per-frame, device-agnostic
//! input state every gameplay system reads instead of raw `ButtonInput<
//! KeyCode>`/`ButtonInput<MouseButton>`/`Gamepad` queries.
//! [`update_action_state`] is the ONE place that walks [`KeyMap`] and asks "is
//! this [`GameInput`] active right now", combining keyboard+mouse+gamepad — so
//! rebinding an action in [`KeyMap`] changes the in-game effect immediately,
//! with no other system needing to know which physical device fired it.

use std::collections::HashSet;

use bevy::{
    ecs::{
        resource::Resource,
        system::{Query, Res, ResMut},
    },
    input::{ButtonInput, gamepad::Gamepad, keyboard::KeyCode, mouse::MouseButton},
    math::Vec2,
};
use strum::IntoEnumIterator;

use crate::{
    game_input::GameInput,
    gamepad::{AxisAction, GamepadBinding},
    keybind::KeyOrMouse,
    keymap::KeyMap,
};

/// The resolved input state for this frame. Digital [`GameInput`]s are
/// available via [`Self::pressed`]/[`Self::just_pressed`]/
/// [`Self::just_released`]; the two analog gamepad sticks (deadzone +
/// inversion already applied, per [`KeyMap::gamepad`]) are exposed directly
/// as [`Self::move_axis`]/[`Self::look_axis`] since movement/look are
/// continuous quantities, not press/release actions (same split
/// [`crate::gamepad::GamepadBindings`] itself keeps between buttons and
/// axes).
#[derive(Resource, Default, Debug, Clone)]
pub struct ActionState {
    pressed: HashSet<GameInput>,
    just_pressed: HashSet<GameInput>,
    just_released: HashSet<GameInput>,
    /// Gamepad left-stick, deadzone/inversion-applied, `[-1.0, 1.0]` per
    /// axis. Does NOT include keyboard WASD — callers combine both (see
    /// `xindeler-client::player_input::gather_input`), since keyboard is
    /// digital (full-magnitude) and analog stick partial-magnitude movement
    /// should not be silently overridden by an idle keyboard.
    pub move_axis: Vec2,
    /// Gamepad right-stick, deadzone/inversion-applied. Mouse look stays a
    /// separate, existing signal (`AccumulatedMouseMotion`) — this is
    /// additive gamepad look, not a mouse replacement.
    pub look_axis: Vec2,
}

impl ActionState {
    #[must_use]
    pub fn pressed(&self, input: GameInput) -> bool { self.pressed.contains(&input) }

    #[must_use]
    pub fn just_pressed(&self, input: GameInput) -> bool { self.just_pressed.contains(&input) }

    #[must_use]
    pub fn just_released(&self, input: GameInput) -> bool { self.just_released.contains(&input) }
}

/// Recomputes [`ActionState`] every frame from the real `ButtonInput<
/// KeyCode>`/`ButtonInput<MouseButton>` resources + every connected
/// [`Gamepad`] entity, resolved against the current [`KeyMap`]. Runs before
/// any gameplay system that reads [`ActionState`] (callers order
/// `.after(update_action_state)`, mirroring how `player_input::gather_input`
/// already orders after the camera systems it reads).
pub fn update_action_state(
    keymap: Res<KeyMap>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    mut state: ResMut<ActionState>,
) {
    state.pressed.clear();
    state.just_pressed.clear();
    state.just_released.clear();

    for input in GameInput::iter() {
        let (mut is_pressed, mut is_just_pressed, mut is_just_released) = (false, false, false);

        if let Some(binding) = keymap.keyboard.get_binding(input) {
            match binding {
                KeyOrMouse::Key(key) => {
                    is_pressed |= keys.pressed(key);
                    is_just_pressed |= keys.just_pressed(key);
                    is_just_released |= keys.just_released(key);
                },
                KeyOrMouse::Mouse(button) => {
                    is_pressed |= mouse.pressed(button);
                    is_just_pressed |= mouse.just_pressed(button);
                    is_just_released |= mouse.just_released(button);
                },
            }
        }

        if let Some(binding) = keymap.gamepad.get_button_binding(input) {
            for gamepad in &gamepads {
                match binding {
                    GamepadBinding::Button(button) => {
                        is_pressed |= gamepad.pressed(button);
                        is_just_pressed |= gamepad.just_pressed(button);
                        is_just_released |= gamepad.just_released(button);
                    },
                    GamepadBinding::Chord { modifier, button } => {
                        let chord_held = gamepad.pressed(modifier) && gamepad.pressed(button);
                        is_pressed |= chord_held;
                        // "Just pressed" for a chord means the trigger
                        // button was freshly pressed WHILE the modifier was
                        // already held (the common real-world order: hold
                        // the shoulder button, then tap a face button).
                        is_just_pressed |= chord_held && gamepad.just_pressed(button);
                        // Releasing EITHER half of the chord ends it.
                        is_just_released |= !chord_held
                            && (gamepad.just_released(button) || gamepad.just_released(modifier));
                    },
                }
            }
        }

        if is_pressed {
            state.pressed.insert(input);
        }
        if is_just_pressed {
            state.just_pressed.insert(input);
        }
        if is_just_released {
            state.just_released.insert(input);
        }
    }

    let mut move_axis = Vec2::ZERO;
    let mut look_axis = Vec2::ZERO;
    for gamepad in &gamepads {
        move_axis += gamepad_axis_pair(&keymap, gamepad, AxisAction::MoveX, AxisAction::MoveY);
        look_axis += gamepad_axis_pair(&keymap, gamepad, AxisAction::LookX, AxisAction::LookY);
    }
    state.move_axis = move_axis.clamp_length_max(1.0);
    state.look_axis = look_axis.clamp_length_max(1.0);
}

/// Reads the two physical axes bound to `x_action`/`y_action`, applies each
/// one's configured deadzone + inversion via [`KeyMap::gamepad`], and
/// returns them as a `Vec2`. Missing bindings/disconnected reads fall back
/// to `0.0` (degrade clean — no gamepad connected must never panic or stall
/// input).
fn gamepad_axis_pair(
    keymap: &KeyMap,
    gamepad: &Gamepad,
    x_action: AxisAction,
    y_action: AxisAction,
) -> Vec2 {
    let raw = |action: AxisAction| -> f32 {
        keymap
            .gamepad
            .axis_map
            .get(&action)
            .and_then(|binding| gamepad.get_unclamped(binding.axis))
            .unwrap_or(0.0)
    };
    Vec2::new(
        keymap.gamepad.apply_axis(x_action, raw(x_action)),
        keymap.gamepad.apply_axis(y_action, raw(y_action)),
    )
}

#[cfg(test)]
mod tests {
    use bevy::app::App;

    use super::*;

    fn new_test_app() -> App {
        let mut app = App::new();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(ButtonInput::<MouseButton>::default());
        app.init_resource::<ActionState>();
        app.add_systems(bevy::app::Update, update_action_state);
        app
    }

    /// The default keyboard binding for `MoveForward` (W) drives
    /// `ActionState::pressed` once the underlying `ButtonInput<KeyCode>`
    /// reports it held — proving a rebind (below) actually changes behavior,
    /// not just data.
    #[test]
    fn pressing_the_bound_key_sets_the_action_pressed() {
        let mut app = new_test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        let state = app.world().resource::<ActionState>();
        assert!(state.pressed(GameInput::MoveForward));
        assert!(!state.pressed(GameInput::MoveBack));
    }

    /// Rebinding `MoveForward` away from W to Z: pressing W (the OLD key)
    /// must no longer drive the action, and pressing Z (the new key) must.
    /// This is the literal EM-5.11 acceptance bar: "rebinding an action
    /// changes the in-game effect."
    #[test]
    fn rebinding_changes_which_physical_key_drives_the_action() {
        let mut app = new_test_app();
        app.world_mut()
            .resource_mut::<KeyMap>()
            .keyboard
            .modify_binding(GameInput::MoveForward, KeyOrMouse::Key(KeyCode::KeyZ));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        assert!(
            !app.world()
                .resource::<ActionState>()
                .pressed(GameInput::MoveForward)
        );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyW);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyZ);
        app.update();
        assert!(
            app.world()
                .resource::<ActionState>()
                .pressed(GameInput::MoveForward)
        );
    }

    /// A mouse-bound action (Primary = left click) is read the same way as
    /// a keyboard one.
    #[test]
    fn mouse_bindings_drive_actions_too() {
        let mut app = new_test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        assert!(
            app.world()
                .resource::<ActionState>()
                .pressed(GameInput::Primary)
        );
    }

    /// No gamepad connected (no `Gamepad` entity in the world): the analog
    /// axes degrade cleanly to zero instead of panicking or stalling.
    #[test]
    fn no_gamepad_connected_degrades_to_zero_axes() {
        let mut app = new_test_app();
        app.update();
        let state = app.world().resource::<ActionState>();
        assert_eq!(state.move_axis, Vec2::ZERO);
        assert_eq!(state.look_axis, Vec2::ZERO);
    }

    /// Pins the gamepad analog sign convention (reviewer follow-up — this
    /// path previously had no coverage): a real connected `Gamepad` with a
    /// POSITIVE raw `LeftStickY`/`RightStickY` reading produces a POSITIVE
    /// `move_axis.y`/`look_axis.y` (no accidental sign flip, no double
    /// inversion) — the default [`crate::gamepad::AxisBinding`] is
    /// un-inverted, so this is the "no inversion configured" baseline every
    /// consumer (`player_input::gather_input`, `camera::fly_cam_look`)
    /// builds on. Also confirms the deadzone rescale is genuinely applied
    /// (0.8 raw → 0.75 after the default 0.2 deadzone, matching
    /// `gamepad::apply_deadzone`'s own formula, not a passthrough).
    #[test]
    fn gamepad_stick_sign_and_deadzone_are_applied_as_documented() {
        use bevy::input::gamepad::{Gamepad, GamepadAxis};

        let mut app = new_test_app();
        let gamepad = app.world_mut().spawn(Gamepad::default()).id();
        app.world_mut()
            .get_mut::<Gamepad>(gamepad)
            .unwrap()
            .analog_mut()
            .set(GamepadAxis::LeftStickY, 0.8);
        app.world_mut()
            .get_mut::<Gamepad>(gamepad)
            .unwrap()
            .analog_mut()
            .set(GamepadAxis::RightStickY, 0.8);
        app.update();

        let state = app.world().resource::<ActionState>();
        // (0.8 - 0.2) / (1.0 - 0.2) = 0.75 — the default deadzone (0.2),
        // un-inverted.
        assert!(
            (state.move_axis.y - 0.75).abs() < 1e-5,
            "move_axis.y = {}",
            state.move_axis.y
        );
        assert!(
            (state.look_axis.y - 0.75).abs() < 1e-5,
            "look_axis.y = {}",
            state.look_axis.y
        );
        // A negative raw reading must flip the sign, not just clamp to zero.
        app.world_mut()
            .get_mut::<Gamepad>(gamepad)
            .unwrap()
            .analog_mut()
            .set(GamepadAxis::LeftStickY, -0.8);
        app.update();
        assert!(app.world().resource::<ActionState>().move_axis.y < 0.0);
    }
}
