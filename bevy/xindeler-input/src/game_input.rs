//! BL-82 EM-5.11 (T56.11) — [`GameInput`], the rebindable action enum.
//!
//! Ported 1:1 from the legacy client's `voxygen/src/game_input.rs` (same
//! variant set, same `strum` i18n-key strings — `assets/voxygen/i18n/**`
//! already carries `gameinput-*` Fluent keys for every one of these, so this
//! port is directly reusable by EM-5.16's i18n depth work with zero renaming)
//! MINUS two things that don't apply to a from-scratch Bevy client yet:
//! - `#[cfg(feature = "egui-ui")] ToggleEguiDebug` — the shipped Bevy client
//!   has no egui debug overlay (CLAUDE.md: voxygen/egui is not a workspace
//!   member); dropping a variant that can never fire here is not a parity gap,
//!   it is removing dead surface the old client only had because of its OWN
//!   optional egui feature.
//!
//! This crate owns NO winit/conrod types — bindings are expressed against
//! Bevy's own `KeyCode`/`MouseButton`/`GamepadButton` (see [`crate::keymap`]),
//! not the legacy `KeyMouse`.

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, EnumIter, EnumString};

/// A rebindable action the game recognises after input mapping. One binding
/// (or none) per variant lives in a [`crate::keymap::KeyMap`].
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    AsRefStr,
    EnumIter,
    EnumString,
)]
pub enum GameInput {
    #[strum(serialize = "gameinput-primary")]
    Primary,
    #[strum(serialize = "gameinput-secondary")]
    Secondary,
    #[strum(serialize = "gameinput-block")]
    Block,
    #[strum(serialize = "gameinput-roll")]
    Roll,
    #[strum(serialize = "gameinput-moveforward")]
    MoveForward,
    #[strum(serialize = "gameinput-moveback")]
    MoveBack,
    #[strum(serialize = "gameinput-moveleft")]
    MoveLeft,
    #[strum(serialize = "gameinput-moveright")]
    MoveRight,
    #[strum(serialize = "gameinput-swimup")]
    SwimUp,
    #[strum(serialize = "gameinput-swimdown")]
    SwimDown,
    #[strum(serialize = "gameinput-jump")]
    Jump,
    #[strum(serialize = "gameinput-walljump")]
    WallJump,
    #[strum(serialize = "gameinput-cancelclimb")]
    CancelClimb,
    #[strum(serialize = "gameinput-interact")]
    Interact,
    #[strum(serialize = "gameinput-trade")]
    Trade,
    #[strum(serialize = "gameinput-glide")]
    Glide,
    #[strum(serialize = "gameinput-togglelantern")]
    ToggleLantern,
    #[strum(serialize = "gameinput-swaploadout")]
    SwapLoadout,
    #[strum(serialize = "gameinput-togglewield")]
    ToggleWield,
    #[strum(serialize = "gameinput-sneak")]
    Sneak,
    #[strum(serialize = "gameinput-sit")]
    Sit,
    #[strum(serialize = "gameinput-crawl")]
    Crawl,
    #[strum(serialize = "gameinput-dance")]
    Dance,
    #[strum(serialize = "gameinput-greet")]
    Greet,
    #[strum(serialize = "gameinput-mount")]
    Mount,
    #[strum(serialize = "gameinput-stayfollow")]
    StayFollow,
    #[strum(serialize = "gameinput-togglewalk")]
    ToggleWalk,
    #[strum(serialize = "gameinput-autowalk")]
    AutoWalk,
    #[strum(serialize = "gameinput-freelook")]
    FreeLook,
    #[strum(serialize = "gameinput-giveup")]
    GiveUp,
    #[strum(serialize = "gameinput-respawn")]
    Respawn,
    #[strum(serialize = "gameinput-inventory")]
    Inventory,
    #[strum(serialize = "gameinput-map")]
    Map,
    #[strum(serialize = "gameinput-settings")]
    Settings,
    #[strum(serialize = "gameinput-crafting")]
    Crafting,
    #[strum(serialize = "gameinput-diary")]
    Diary,
    #[strum(serialize = "gameinput-chat")]
    Chat,
    #[strum(serialize = "gameinput-social")]
    Social,
    #[strum(serialize = "gameinput-escape")]
    Escape,
    #[strum(serialize = "gameinput-controls")]
    Controls,
    #[strum(serialize = "gameinput-select")]
    Select,
    #[strum(serialize = "gameinput-acceptgroupinvite")]
    AcceptGroupInvite,
    #[strum(serialize = "gameinput-declinegroupinvite")]
    DeclineGroupInvite,
    #[strum(serialize = "gameinput-previousslot")]
    PreviousSlot,
    #[strum(serialize = "gameinput-nextslot")]
    NextSlot,
    #[strum(serialize = "gameinput-currentslot")]
    CurrentSlot,
    #[strum(serialize = "gameinput-slot1")]
    Slot1,
    #[strum(serialize = "gameinput-slot2")]
    Slot2,
    #[strum(serialize = "gameinput-slot3")]
    Slot3,
    #[strum(serialize = "gameinput-slot4")]
    Slot4,
    #[strum(serialize = "gameinput-slot5")]
    Slot5,
    #[strum(serialize = "gameinput-slot6")]
    Slot6,
    #[strum(serialize = "gameinput-slot7")]
    Slot7,
    #[strum(serialize = "gameinput-slot8")]
    Slot8,
    #[strum(serialize = "gameinput-slot9")]
    Slot9,
    #[strum(serialize = "gameinput-slot10")]
    Slot10,
    #[strum(serialize = "gameinput-zoomin")]
    ZoomIn,
    #[strum(serialize = "gameinput-zoomout")]
    ZoomOut,
    #[strum(serialize = "gameinput-togglecursor")]
    ToggleCursor,
    #[strum(serialize = "gameinput-zoomlock")]
    ZoomLock,
    #[strum(serialize = "gameinput-mapzoomin")]
    MapZoomIn,
    #[strum(serialize = "gameinput-mapzoomout")]
    MapZoomOut,
    #[strum(serialize = "gameinput-map-locationmarkerbutton")]
    MapSetMarker,
    #[strum(serialize = "gameinput-fullscreen")]
    Fullscreen,
    #[strum(serialize = "gameinput-screenshot")]
    Screenshot,
    #[strum(serialize = "gameinput-command")]
    Command,
    #[strum(serialize = "gameinput-fly")]
    Fly,
    #[strum(serialize = "gameinput-toggleinterface")]
    ToggleInterface,
    #[strum(serialize = "gameinput-toggledebug")]
    ToggleDebug,
    #[strum(serialize = "gameinput-togglechat")]
    ToggleChat,
    #[strum(serialize = "gameinput-toggleingameui")]
    ToggleIngameUi,
    #[strum(serialize = "gameinput-cameraclamp")]
    CameraClamp,
    #[strum(serialize = "gameinput-cyclecamera")]
    CycleCamera,
    #[strum(serialize = "gameinput-spectatespeedboost")]
    SpectateSpeedBoost,
    #[strum(serialize = "gameinput-spectateviewpoint")]
    SpectateViewpoint,
    #[strum(serialize = "gameinput-mutemaster")]
    MuteMaster,
    #[strum(serialize = "gameinput-muteinactivemaster")]
    MuteInactiveMaster,
    #[strum(serialize = "gameinput-mutemusic")]
    MuteMusic,
    #[strum(serialize = "gameinput-mutesfx")]
    MuteSfx,
    #[strum(serialize = "gameinput-muteambience")]
    MuteAmbience,
}

impl GameInput {
    /// The Fluent i18n key for this action's display name (`assets/voxygen/
    /// i18n/**` already has `gameinput-*` entries for every variant here).
    /// Same accessor shape as the legacy client's `get_localization_key`.
    #[must_use]
    pub fn localization_key(&self) -> &str { self.as_ref() }

    /// The SAME key as [`Self::localization_key`], but `'static` — needed by
    /// `xindeler_ui::i18n::LocalizedText`/`LocalizedLabel`, which store their
    /// key as a `&'static str` component field. `AsRefStr`'s generated
    /// `as_ref(&self)` ties its return to `&self`'s (non-`'static`) lifetime
    /// even though the underlying data is always one of these `'static`
    /// string literals, so `bevy/xindeler-client::controls_screen` (the one
    /// caller, BL-82 EM-5.16 follow-up) needs this instead. Mirrors the
    /// `#[strum(serialize = ..)]` attribute on each variant above 1:1; the
    /// `ftl_key_matches_localization_key` test below guards against the two
    /// ever drifting apart.
    #[must_use]
    pub const fn ftl_key(self) -> &'static str {
        match self {
            GameInput::Primary => "gameinput-primary",
            GameInput::Secondary => "gameinput-secondary",
            GameInput::Block => "gameinput-block",
            GameInput::Roll => "gameinput-roll",
            GameInput::MoveForward => "gameinput-moveforward",
            GameInput::MoveBack => "gameinput-moveback",
            GameInput::MoveLeft => "gameinput-moveleft",
            GameInput::MoveRight => "gameinput-moveright",
            GameInput::SwimUp => "gameinput-swimup",
            GameInput::SwimDown => "gameinput-swimdown",
            GameInput::Jump => "gameinput-jump",
            GameInput::WallJump => "gameinput-walljump",
            GameInput::CancelClimb => "gameinput-cancelclimb",
            GameInput::Interact => "gameinput-interact",
            GameInput::Trade => "gameinput-trade",
            GameInput::Glide => "gameinput-glide",
            GameInput::ToggleLantern => "gameinput-togglelantern",
            GameInput::SwapLoadout => "gameinput-swaploadout",
            GameInput::ToggleWield => "gameinput-togglewield",
            GameInput::Sneak => "gameinput-sneak",
            GameInput::Sit => "gameinput-sit",
            GameInput::Crawl => "gameinput-crawl",
            GameInput::Dance => "gameinput-dance",
            GameInput::Greet => "gameinput-greet",
            GameInput::Mount => "gameinput-mount",
            GameInput::StayFollow => "gameinput-stayfollow",
            GameInput::ToggleWalk => "gameinput-togglewalk",
            GameInput::AutoWalk => "gameinput-autowalk",
            GameInput::FreeLook => "gameinput-freelook",
            GameInput::GiveUp => "gameinput-giveup",
            GameInput::Respawn => "gameinput-respawn",
            GameInput::Inventory => "gameinput-inventory",
            GameInput::Map => "gameinput-map",
            GameInput::Settings => "gameinput-settings",
            GameInput::Crafting => "gameinput-crafting",
            GameInput::Diary => "gameinput-diary",
            GameInput::Chat => "gameinput-chat",
            GameInput::Social => "gameinput-social",
            GameInput::Escape => "gameinput-escape",
            GameInput::Controls => "gameinput-controls",
            GameInput::Select => "gameinput-select",
            GameInput::AcceptGroupInvite => "gameinput-acceptgroupinvite",
            GameInput::DeclineGroupInvite => "gameinput-declinegroupinvite",
            GameInput::PreviousSlot => "gameinput-previousslot",
            GameInput::NextSlot => "gameinput-nextslot",
            GameInput::CurrentSlot => "gameinput-currentslot",
            GameInput::Slot1 => "gameinput-slot1",
            GameInput::Slot2 => "gameinput-slot2",
            GameInput::Slot3 => "gameinput-slot3",
            GameInput::Slot4 => "gameinput-slot4",
            GameInput::Slot5 => "gameinput-slot5",
            GameInput::Slot6 => "gameinput-slot6",
            GameInput::Slot7 => "gameinput-slot7",
            GameInput::Slot8 => "gameinput-slot8",
            GameInput::Slot9 => "gameinput-slot9",
            GameInput::Slot10 => "gameinput-slot10",
            GameInput::ZoomIn => "gameinput-zoomin",
            GameInput::ZoomOut => "gameinput-zoomout",
            GameInput::ToggleCursor => "gameinput-togglecursor",
            GameInput::ZoomLock => "gameinput-zoomlock",
            GameInput::MapZoomIn => "gameinput-mapzoomin",
            GameInput::MapZoomOut => "gameinput-mapzoomout",
            GameInput::MapSetMarker => "gameinput-map-locationmarkerbutton",
            GameInput::Fullscreen => "gameinput-fullscreen",
            GameInput::Screenshot => "gameinput-screenshot",
            GameInput::Command => "gameinput-command",
            GameInput::Fly => "gameinput-fly",
            GameInput::ToggleInterface => "gameinput-toggleinterface",
            GameInput::ToggleDebug => "gameinput-toggledebug",
            GameInput::ToggleChat => "gameinput-togglechat",
            GameInput::ToggleIngameUi => "gameinput-toggleingameui",
            GameInput::CameraClamp => "gameinput-cameraclamp",
            GameInput::CycleCamera => "gameinput-cyclecamera",
            GameInput::SpectateSpeedBoost => "gameinput-spectatespeedboost",
            GameInput::SpectateViewpoint => "gameinput-spectateviewpoint",
            GameInput::MuteMaster => "gameinput-mutemaster",
            GameInput::MuteInactiveMaster => "gameinput-muteinactivemaster",
            GameInput::MuteMusic => "gameinput-mutemusic",
            GameInput::MuteSfx => "gameinput-mutesfx",
            GameInput::MuteAmbience => "gameinput-muteambience",
        }
    }

    /// Returns true if `a` and `b` may be bound to the same physical input at
    /// the same time without a *disallowed* conflict — e.g. the player can't
    /// jump and climb at once, so `Jump`/`CancelClimb` are safe to share.
    /// (Same disjoint-set-"find" shape as the legacy client's
    /// `can_share_bindings` — see [`Self::representative_bindings`].)
    #[must_use]
    pub fn can_share_bindings(a: GameInput, b: GameInput) -> bool {
        let bindings_a = a.representative_bindings();
        let bindings_b = b.representative_bindings();

        if bindings_a.is_empty() && bindings_b.is_empty() {
            return a == b;
        }
        if bindings_a.is_empty() {
            return bindings_b.contains(&a);
        }
        if bindings_b.is_empty() {
            return bindings_a.contains(&b);
        }
        bindings_a.iter().any(|x| bindings_b.contains(x))
    }

    /// If two [`GameInput`]s are allowed to share a binding, this returns a
    /// slice naming the "representative" action(s) they're allowed to share
    /// with (disjoint-set find, not full symmetric closure — mirrors the
    /// legacy table 1:1).
    fn representative_bindings(self) -> &'static [GameInput] {
        match self {
            GameInput::SwimUp | GameInput::Respawn | GameInput::GiveUp => &[GameInput::Jump],
            GameInput::AutoWalk | GameInput::FreeLook => &[GameInput::FreeLook],
            GameInput::SpectateSpeedBoost => &[GameInput::Glide],
            GameInput::WallJump => &[GameInput::Mount, GameInput::Jump],
            GameInput::SwimDown | GameInput::Sneak | GameInput::CancelClimb => &[GameInput::Roll],
            GameInput::SpectateViewpoint => &[GameInput::MapSetMarker],
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    /// Every variant round-trips through its `strum` string (needed for both
    /// the `settings.ron` HashMap keys — RON serializes enum keys by variant
    /// name, not the `strum` string, so this isn't load-bearing for
    /// persistence — and for the i18n key lookup, which IS load-bearing).
    #[test]
    fn every_variant_has_a_stable_i18n_key() {
        for input in GameInput::iter() {
            let key = input.localization_key();
            assert!(key.starts_with("gameinput-"), "{input:?} -> {key}");
        }
    }

    /// [`GameInput::ftl_key`] duplicates the `#[strum(serialize = ..)]`
    /// association as a `'static`-returning match (see its own doc comment
    /// for why) — this pins that the duplication never drifts from the
    /// derive-generated [`GameInput::localization_key`].
    #[test]
    fn ftl_key_matches_localization_key() {
        for input in GameInput::iter() {
            assert_eq!(
                input.ftl_key(),
                input.localization_key(),
                "{input:?}: ftl_key() must match the strum-derived localization_key()"
            );
        }
    }

    /// Jump-adjacent actions (swim-up/respawn/give-up) may share a physical
    /// binding with Jump (and with each other, transitively through Jump) —
    /// this is the exact case the old client's "space does double duty"
    /// design relies on.
    #[test]
    fn jump_adjacent_actions_can_share_bindings() {
        assert!(GameInput::can_share_bindings(
            GameInput::Jump,
            GameInput::Respawn
        ));
        assert!(GameInput::can_share_bindings(
            GameInput::SwimUp,
            GameInput::GiveUp
        ));
    }

    /// Unrelated actions must NOT be reported as shareable — e.g. binding
    /// both Jump and Inventory to the same key is a genuine conflict.
    #[test]
    fn unrelated_actions_cannot_share_bindings() {
        assert!(!GameInput::can_share_bindings(
            GameInput::Jump,
            GameInput::Inventory
        ));
    }

    /// An action can always "share" a binding with itself (reflexive case —
    /// both empty representative sets fall back to plain equality).
    #[test]
    fn an_action_shares_a_binding_with_itself() {
        assert!(GameInput::can_share_bindings(
            GameInput::Inventory,
            GameInput::Inventory
        ));
    }
}
