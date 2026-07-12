//! BL-82 EM-5.11 (T56.11/T56.12) — keyboard + mouse bindings.
//!
//! Ported from the legacy client's `voxygen/src/settings/control.rs`
//! (`ControlSettings`), with ONE deliberate representational change:
//! bindings are expressed as Bevy's own [`KeyCode`]/[`MouseButton`] (this
//! project's real input types) instead of legacy's winit
//! `Key`/`NamedKey`-based `KeyMouse`.
//!
//! ## Winit → Bevy key-name migration note (T56.12)
//! The legacy client persisted `KeyMouse` values that serialize through
//! winit's `Key`/`NamedKey` representation (e.g. `Key(Character("w"))`,
//! `Key(Named(Escape))`). Bevy's [`KeyCode`] is a flat physical-key enum that
//! serializes as a bare variant name (e.g. `KeyW`, `Escape`) — a structurally
//! different shape, not a drop-in superset. A `settings.ron` written by the
//! legacy client's `keybindings`/gamepad sections is therefore **not**
//! forward-compatible with this crate's `controls` section: this is an
//! intentional clean-break migration (the Bevy client is a from-scratch
//! `settings.ron` writer, not a reader of the retiring client's file), not an
//! automatic converter. A fresh `controls` section always starts from
//! [`KeyBindings::default`] (this module's own default table, independently
//! chosen against Bevy's `KeyCode` — not translated key-by-key from the
//! winit table) and the delta-vs-default persistence model below means a
//! player who never rebinds anything never sees a `controls` section in
//! their file at all, matching the legacy behaviour's spirit even though the
//! underlying key representation changed.

use std::collections::{HashMap, HashSet};

use bevy::input::{keyboard::KeyCode, mouse::MouseButton};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::game_input::GameInput;

/// A single physical keyboard key or mouse button a [`GameInput`] can bind
/// to. Deliberately does NOT include gamepad inputs — those live in
/// [`crate::gamepad::GamepadBindings`], mirroring the legacy split between
/// `ControlSettings` (keyboard/mouse) and `ControllerSettings` (gamepad):
/// a player can have both a keyboard AND a gamepad binding for the same
/// [`GameInput`] active at once, so unifying them into one table would only
/// make conflict detection (which is meaningful WITHIN a device, not across)
/// harder to reason about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyOrMouse {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// Serde-only delta shape: only bindings that differ from
/// [`KeyBindings::default_binding`] are ever written to disk (mirrors the
/// legacy `ControlSettingsSerde` pattern exactly — same rationale: writing
/// all ~90 actions on every save would bury a player's few actual
/// customizations in noise).
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct KeyBindingsSerde {
    keybindings: HashMap<GameInput, Option<KeyOrMouse>>,
}

impl From<KeyBindings> for KeyBindingsSerde {
    fn from(bindings: KeyBindings) -> Self {
        let mut delta = HashMap::new();
        for (input, binding) in bindings.keybindings {
            if KeyBindings::default_binding(input) != binding {
                delta.insert(input, binding);
            }
        }
        KeyBindingsSerde { keybindings: delta }
    }
}

impl Default for KeyBindingsSerde {
    fn default() -> Self { KeyBindings::default().into() }
}

/// Keyboard/mouse bindings for every [`GameInput`], plus the reverse index
/// (`inverse_keybindings`) conflict-detection needs. Round-trips through RON
/// as a delta against [`KeyBindings::default_binding`] (see
/// [`KeyBindingsSerde`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "KeyBindingsSerde", into = "KeyBindingsSerde")]
pub struct KeyBindings {
    pub keybindings: HashMap<GameInput, Option<KeyOrMouse>>,
    pub inverse_keybindings: HashMap<KeyOrMouse, HashSet<GameInput>>,
}

impl From<KeyBindingsSerde> for KeyBindings {
    fn from(serde: KeyBindingsSerde) -> Self {
        let mut bindings = KeyBindings::default();
        for (input, maybe_binding) in serde.keybindings {
            match maybe_binding {
                Some(binding) => bindings.modify_binding(input, binding),
                None => bindings.remove_binding(input),
            }
        }
        bindings
    }
}

impl KeyBindings {
    pub fn remove_binding(&mut self, game_input: GameInput) {
        if let Some(inverse) = self
            .keybindings
            .insert(game_input, None)
            .flatten()
            .and_then(|binding| self.inverse_keybindings.get_mut(&binding))
        {
            inverse.remove(&game_input);
        }
    }

    #[must_use]
    pub fn get_binding(&self, game_input: GameInput) -> Option<KeyOrMouse> {
        self.keybindings.get(&game_input).copied().flatten()
    }

    #[must_use]
    pub fn get_associated_game_inputs(&self, binding: &KeyOrMouse) -> Option<&HashSet<GameInput>> {
        self.inverse_keybindings.get(binding)
    }

    /// Rebinds `game_input` to `binding`, dropping any previous binding for
    /// it. Does NOT clear other actions already bound to `binding` — call
    /// [`Self::has_conflicting_bindings`]/[`Self::get_associated_game_inputs`]
    /// first to surface the conflict to the player instead of silently
    /// double-binding (EM-5.11's conflict-detection requirement).
    pub fn modify_binding(&mut self, game_input: GameInput, binding: KeyOrMouse) {
        if let Some(old_binding) = self.get_binding(game_input)
            && let Some(inverse) = self.inverse_keybindings.get_mut(&old_binding)
        {
            inverse.remove(&game_input);
        }
        self.inverse_keybindings
            .entry(binding)
            .or_default()
            .insert(game_input);
        self.keybindings.insert(game_input, Some(binding));
    }

    /// True when `binding` is bound to two or more [`GameInput`]s that are
    /// NOT allowed to share it ([`GameInput::can_share_bindings`]) — the
    /// conflict-detection model EM-5.11 requires: a rebind is never silently
    /// double-bound, it's surfaced via this check.
    #[must_use]
    pub fn has_conflicting_bindings(&self, binding: KeyOrMouse) -> bool {
        let Some(inputs) = self.inverse_keybindings.get(&binding) else {
            return false;
        };
        inputs
            .iter()
            .any(|&a| inputs.iter().any(|&b| !GameInput::can_share_bindings(a, b)))
    }

    /// The default binding for a fresh install — Bevy `KeyCode`/`MouseButton`
    /// equivalents of the legacy table (see this module's migration note:
    /// chosen independently against Bevy's key set, not machine-translated).
    #[must_use]
    pub fn default_binding(game_input: GameInput) -> Option<KeyOrMouse> {
        use KeyCode as K;
        let key = |k: KeyCode| Some(KeyOrMouse::Key(k));
        match game_input {
            GameInput::Primary => Some(KeyOrMouse::Mouse(MouseButton::Left)),
            GameInput::Secondary => Some(KeyOrMouse::Mouse(MouseButton::Right)),
            GameInput::Roll => Some(KeyOrMouse::Mouse(MouseButton::Middle)),
            GameInput::MapSetMarker => Some(KeyOrMouse::Mouse(MouseButton::Middle)),
            GameInput::SpectateViewpoint => Some(KeyOrMouse::Mouse(MouseButton::Middle)),

            GameInput::Block => key(K::AltLeft),
            GameInput::ToggleCursor => key(K::Comma),
            GameInput::Escape => key(K::Escape),
            GameInput::Chat => key(K::Enter),
            GameInput::Command => key(K::Slash),
            GameInput::MoveForward => key(K::KeyW),
            GameInput::MoveLeft => key(K::KeyA),
            GameInput::MoveBack => key(K::KeyS),
            GameInput::MoveRight => key(K::KeyD),
            GameInput::Jump | GameInput::WallJump => key(K::Space),
            GameInput::Sit => key(K::KeyK),
            GameInput::Crawl => key(K::ArrowDown),
            GameInput::Dance => key(K::KeyJ),
            GameInput::Greet => key(K::KeyH),
            GameInput::Glide => key(K::ControlLeft),
            GameInput::SwimUp => key(K::Space),
            GameInput::SwimDown => key(K::ShiftLeft),
            GameInput::Fly => key(K::KeyH),
            GameInput::Sneak | GameInput::CancelClimb => key(K::ShiftLeft),
            GameInput::ToggleLantern => key(K::KeyG),
            GameInput::Mount => key(K::KeyF),
            GameInput::StayFollow => key(K::KeyV),
            GameInput::Map => key(K::KeyM),
            GameInput::Inventory => key(K::KeyI),
            GameInput::Trade => key(K::KeyT),
            GameInput::Social => key(K::KeyO),
            GameInput::Crafting => key(K::KeyC),
            GameInput::Diary => key(K::KeyP),
            GameInput::Settings => key(K::F10),
            GameInput::Controls => key(K::F1),
            GameInput::ToggleInterface => key(K::F2),
            GameInput::ToggleDebug => key(K::F3),
            GameInput::ToggleChat => key(K::F5),
            GameInput::Fullscreen => key(K::F11),
            GameInput::Screenshot => key(K::F4),
            GameInput::ToggleIngameUi => key(K::F6),
            GameInput::GiveUp | GameInput::Respawn => key(K::Space),
            GameInput::Interact => key(K::KeyE),
            GameInput::ToggleWield => key(K::KeyR),
            GameInput::FreeLook => key(K::KeyL),
            GameInput::AutoWalk => key(K::Period),
            GameInput::ZoomIn => key(K::Equal),
            GameInput::ZoomOut => key(K::Minus),
            GameInput::ZoomLock => None,
            GameInput::CameraClamp => key(K::Quote),
            GameInput::CycleCamera => key(K::Digit0),
            GameInput::Slot1 => key(K::Digit1),
            GameInput::Slot2 => key(K::Digit2),
            GameInput::Slot3 => key(K::Digit3),
            GameInput::Slot4 => key(K::Digit4),
            GameInput::Slot5 => key(K::Digit5),
            GameInput::Slot6 => key(K::Digit6),
            GameInput::Slot7 => key(K::Digit7),
            GameInput::Slot8 => key(K::Digit8),
            GameInput::Slot9 => key(K::Digit9),
            GameInput::Slot10 => key(K::KeyQ),
            GameInput::NextSlot | GameInput::PreviousSlot | GameInput::CurrentSlot => None,
            GameInput::SwapLoadout => key(K::Tab),
            GameInput::Select => key(K::KeyX),
            GameInput::AcceptGroupInvite => key(K::KeyY),
            GameInput::DeclineGroupInvite => key(K::KeyN),
            GameInput::MapZoomIn => key(K::BracketRight),
            GameInput::MapZoomOut => key(K::BracketLeft),
            GameInput::SpectateSpeedBoost => key(K::ControlLeft),
            GameInput::MuteMaster => key(K::AudioVolumeMute),
            GameInput::MuteInactiveMaster | GameInput::MuteSfx | GameInput::MuteAmbience => None,
            GameInput::MuteMusic => key(K::F8),
            GameInput::ToggleWalk => key(K::KeyB),
        }
    }
}

impl Default for KeyBindings {
    fn default() -> Self {
        let mut bindings = KeyBindings {
            keybindings: HashMap::new(),
            inverse_keybindings: HashMap::new(),
        };
        for game_input in GameInput::iter() {
            if let Some(default) = KeyBindings::default_binding(game_input) {
                bindings.modify_binding(game_input, default);
            } else {
                bindings.keybindings.insert(game_input, None);
            }
        }
        bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_an_entry_in_a_fresh_keymap() {
        let bindings = KeyBindings::default();
        for input in GameInput::iter() {
            assert!(
                bindings.keybindings.contains_key(&input),
                "{input:?} missing from fresh KeyBindings"
            );
        }
    }

    #[test]
    fn rebinding_replaces_the_old_inverse_entry() {
        let mut bindings = KeyBindings::default();
        assert_eq!(
            bindings.get_binding(GameInput::MoveForward),
            Some(KeyOrMouse::Key(KeyCode::KeyW))
        );
        bindings.modify_binding(GameInput::MoveForward, KeyOrMouse::Key(KeyCode::KeyZ));
        assert_eq!(
            bindings.get_binding(GameInput::MoveForward),
            Some(KeyOrMouse::Key(KeyCode::KeyZ))
        );
        // The OLD key (W) must no longer report MoveForward as bound to it.
        assert!(
            bindings
                .get_associated_game_inputs(&KeyOrMouse::Key(KeyCode::KeyW))
                .is_none_or(|set| !set.contains(&GameInput::MoveForward))
        );
        assert!(
            bindings
                .get_associated_game_inputs(&KeyOrMouse::Key(KeyCode::KeyZ))
                .unwrap()
                .contains(&GameInput::MoveForward)
        );
    }

    /// The whole conflict-detection point: binding two UNRELATED actions to
    /// the same key must be flagged, not silently double-bound.
    #[test]
    fn binding_two_unrelated_actions_to_one_key_is_flagged_as_conflicting() {
        let mut bindings = KeyBindings::default();
        bindings.modify_binding(GameInput::Inventory, KeyOrMouse::Key(KeyCode::KeyE));
        bindings.modify_binding(GameInput::Interact, KeyOrMouse::Key(KeyCode::KeyE));
        assert!(bindings.has_conflicting_bindings(KeyOrMouse::Key(KeyCode::KeyE)));
        let conflicting = bindings
            .get_associated_game_inputs(&KeyOrMouse::Key(KeyCode::KeyE))
            .unwrap();
        assert!(conflicting.contains(&GameInput::Inventory));
        assert!(conflicting.contains(&GameInput::Interact));
    }

    /// Jump-adjacent actions sharing Space (the real default state) must NOT
    /// be flagged — this is intentional, allowed sharing, not a conflict.
    #[test]
    fn intentionally_shared_defaults_are_not_flagged_as_conflicting() {
        let bindings = KeyBindings::default();
        assert!(!bindings.has_conflicting_bindings(KeyOrMouse::Key(KeyCode::Space)));
    }

    /// Delta persistence: a completely untouched keymap serializes to an
    /// EMPTY `controls` section (no 90-entry dump), and a round-trip through
    /// RON reconstructs the exact same defaults.
    #[test]
    fn untouched_bindings_round_trip_as_an_empty_delta() {
        let bindings = KeyBindings::default();
        let serde_form: KeyBindingsSerde = bindings.clone().into();
        assert!(
            serde_form.keybindings.is_empty(),
            "untouched keymap must serialize as an empty delta"
        );

        let ron_text = ron::ser::to_string(&bindings).expect("KeyBindings serializes");
        let round_tripped: KeyBindings =
            ron::from_str(&ron_text).expect("KeyBindings deserializes");
        assert_eq!(
            round_tripped.get_binding(GameInput::MoveForward),
            bindings.get_binding(GameInput::MoveForward)
        );
    }

    /// Rebinding ONE action serializes ONLY that action's delta — proving
    /// the "delta-vs-default" persistence model actually holds, not just an
    /// empty-case check.
    #[test]
    fn one_rebind_serializes_as_a_single_entry_delta() {
        let mut bindings = KeyBindings::default();
        bindings.modify_binding(GameInput::Jump, KeyOrMouse::Key(KeyCode::KeyZ));
        let serde_form: KeyBindingsSerde = bindings.into();
        assert_eq!(serde_form.keybindings.len(), 1);
        assert_eq!(
            serde_form.keybindings.get(&GameInput::Jump).copied(),
            Some(Some(KeyOrMouse::Key(KeyCode::KeyZ)))
        );
    }

    /// A settings file predating a NEW GameInput variant (i.e. one that was
    /// added after the file was written) must still load — matching the
    /// legacy `old_settings_files_still_load` guarantee: unknown-to-the-file
    /// actions simply fall back to `default_binding` since `KeyBindings::
    /// from(serde)` starts from `KeyBindings::default()` and only overlays
    /// what the file actually specifies.
    #[test]
    fn a_file_missing_newer_actions_still_gets_their_defaults() {
        let text = "(keybindings: {})"; // an empty/old delta
        let bindings: KeyBindings = ron::from_str(text).expect("empty delta parses");
        assert_eq!(
            bindings.get_binding(GameInput::MoveForward),
            Some(KeyOrMouse::Key(KeyCode::KeyW))
        );
    }
}
