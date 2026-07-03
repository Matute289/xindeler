//! Top-level application flow states (EM-2.1).

use bevy::prelude::*;

/// The coarse client/app flow: menu -> character select -> in game.
///
/// [`AppState::Demo`] is a **transitional Phase-2 state**: it boots straight
/// into the EM-2.2 demo scene so the graphics pipeline (TAA/SSAO/fog/shadows)
/// can be validated without menus. It is the default for now and goes away
/// once EM-5.x lands the real main-menu flow (default then becomes
/// [`AppState::MainMenu`]).
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// Title screen / server selection.
    MainMenu,
    /// Character list + creation.
    CharSelect,
    /// Connected and playing.
    InGame,
    /// Transitional: EM-2.x graphics demo scene (no menus, no server).
    #[default]
    Demo,
}
