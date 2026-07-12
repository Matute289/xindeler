//! BL-82 EM-5.11 — input capture for the rebinding UI: "click Rebind, press
//! the new key/button" as a tiny request/response state machine, so the UI
//! screen (`xindeler-client::controls_screen`) never has to read raw
//! `ButtonInput`/`Gamepad` state itself.

use bevy::{
    ecs::{
        message::{Message, MessageWriter},
        resource::Resource,
        system::{Query, Res, ResMut},
    },
    input::{ButtonInput, gamepad::Gamepad, keyboard::KeyCode, mouse::MouseButton},
};

use crate::{game_input::GameInput, gamepad::GamepadBinding, keybind::KeyOrMouse, keymap::KeyMap};

/// Which device the UI is currently listening on for the next input, and
/// which action the result will be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebindTarget {
    Keyboard(GameInput),
    Gamepad(GameInput),
}

/// Set by the UI (e.g. clicking a "Rebind" button next to an action row) to
/// start listening; cleared automatically once [`capture_rebind`] resolves a
/// binding (or the UI can clear it itself to cancel).
#[derive(Resource, Default)]
pub struct RebindRequest(pub Option<RebindTarget>);

/// Fired once a pending [`RebindRequest`] resolves. `conflicts` lists every
/// OTHER [`GameInput`] now sharing this physical binding that is NOT allowed
/// to (per [`GameInput::can_share_bindings`]) — the UI renders this as a
/// warning next to the row instead of the rebind silently double-binding
/// with no feedback (EM-5.11's conflict-detection requirement).
#[derive(Message, Debug, Clone)]
pub struct RebindOutcome {
    pub input: GameInput,
    pub conflicts: Vec<GameInput>,
}

/// While a [`RebindRequest`] is pending, watches for the next keyboard key /
/// mouse button (for [`RebindTarget::Keyboard`]) or gamepad button (for
/// [`RebindTarget::Gamepad`]) press, applies it to the [`KeyMap`], clears the
/// request, and reports the outcome (incl. any conflicts) via
/// [`RebindOutcome`]. A no-op when no request is pending, so this system can
/// stay unconditionally scheduled.
pub fn capture_rebind(
    mut keymap: ResMut<KeyMap>,
    mut request: ResMut<RebindRequest>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    mut outcomes: MessageWriter<RebindOutcome>,
) {
    let Some(target) = request.0 else {
        return;
    };

    match target {
        RebindTarget::Keyboard(input) => {
            let binding = keys
                .get_just_pressed()
                .next()
                .copied()
                .map(KeyOrMouse::Key)
                .or_else(|| {
                    mouse
                        .get_just_pressed()
                        .next()
                        .copied()
                        .map(KeyOrMouse::Mouse)
                });
            let Some(binding) = binding else {
                return; // still waiting for the next press
            };
            keymap.keyboard.modify_binding(input, binding);
            let conflicts =
                conflicting_inputs(keymap.keyboard.get_associated_game_inputs(&binding), input);
            request.0 = None;
            outcomes.write(RebindOutcome { input, conflicts });
        },
        RebindTarget::Gamepad(input) => {
            let Some(button) = gamepads
                .iter()
                .find_map(|gamepad| gamepad.get_just_pressed().next().copied())
            else {
                return;
            };
            let binding = GamepadBinding::Button(button);
            keymap.gamepad.modify_button_binding(input, binding);
            let conflicts =
                conflicting_inputs(keymap.gamepad.get_associated_game_inputs(&binding), input);
            request.0 = None;
            outcomes.write(RebindOutcome { input, conflicts });
        },
    }
}

/// Every [`GameInput`] sharing a binding with `subject` that is NOT allowed
/// to (excludes `subject` itself and any legitimately-shared pair, e.g.
/// Jump/Respawn on Space).
fn conflicting_inputs(
    sharing: Option<&std::collections::HashSet<GameInput>>,
    subject: GameInput,
) -> Vec<GameInput> {
    sharing
        .into_iter()
        .flatten()
        .copied()
        .filter(|&other| other != subject && !GameInput::can_share_bindings(subject, other))
        .collect()
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::message::Messages};

    use super::*;

    fn new_test_app() -> App {
        let mut app = App::new();
        app.insert_resource(KeyMap::default());
        app.insert_resource(RebindRequest::default());
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(ButtonInput::<MouseButton>::default());
        app.add_message::<RebindOutcome>();
        app.add_systems(bevy::app::Update, capture_rebind);
        app
    }

    /// Requesting a keyboard rebind for `Inventory`, then pressing U (a key
    /// with no default binding at all): resolves the request, updates the
    /// KeyMap, and reports NO conflicts.
    #[test]
    fn keyboard_rebind_resolves_on_next_keypress() {
        let mut app = new_test_app();
        app.world_mut().resource_mut::<RebindRequest>().0 =
            Some(RebindTarget::Keyboard(GameInput::Inventory));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyU);
        app.update();

        assert!(app.world().resource::<RebindRequest>().0.is_none());
        assert_eq!(
            app.world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Inventory),
            Some(KeyOrMouse::Key(KeyCode::KeyU))
        );
        let outcomes = app.world().resource::<Messages<RebindOutcome>>();
        let mut reader = outcomes.get_cursor();
        let outcome = reader.read(outcomes).next().expect("one outcome fired");
        assert_eq!(outcome.input, GameInput::Inventory);
        assert!(outcome.conflicts.is_empty());
    }

    /// Rebinding `Interact` onto `I` — the default key for `Inventory` — must
    /// report `Inventory` as a conflict (they cannot share bindings), NOT
    /// silently double-bind with no feedback.
    #[test]
    fn rebinding_onto_an_occupied_key_reports_the_conflict() {
        let mut app = new_test_app();
        app.world_mut().resource_mut::<RebindRequest>().0 =
            Some(RebindTarget::Keyboard(GameInput::Interact));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyI);
        app.update();

        assert_eq!(
            app.world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Interact),
            Some(KeyOrMouse::Key(KeyCode::KeyI))
        );
        let outcomes = app.world().resource::<Messages<RebindOutcome>>();
        let mut reader = outcomes.get_cursor();
        let outcome = reader.read(outcomes).next().expect("one outcome fired");
        assert_eq!(outcome.conflicts, vec![GameInput::Inventory]);
    }

    /// With no request pending, the system is a total no-op even if keys are
    /// held — it must never react to ordinary gameplay input.
    #[test]
    fn no_pending_request_is_a_no_op() {
        let mut app = new_test_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyK);
        app.update();
        assert_eq!(
            app.world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Inventory),
            Some(KeyOrMouse::Key(KeyCode::KeyI)), // unchanged default
        );
    }
}
