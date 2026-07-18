//! Top-level application flow states (EM-2.1).

use bevy::prelude::*;

/// The coarse client/app flow: menu -> connecting -> in game.
///
/// [`AppState::Demo`] is a **transitional Phase-2 state**: it boots straight
/// into the EM-2.2 demo scene so the graphics pipeline (TAA/SSAO/fog/shadows)
/// can be validated without menus. It stays the *compiled-in* default (so the
/// smoke harnesses and a no-feature `cargo run` keep booting straight into the
/// demo scene), but the real interactive client now overrides the initial
/// state to [`AppState::MainMenu`] at boot (BL-82 EM-5.9 / T56.29, via
/// `App::insert_state` in `xindeler-client`'s `main`) whenever it is built with
/// the world-hosting `listen-server` feature and launched without an explicit
/// `--listen-server`/`--connect`/`--smoke-*` bypass.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// Title screen / login / server selection (BL-82 EM-5.9).
    MainMenu,
    /// Character list + creation (EM-5.14 — not yet wired).
    CharSelect,
    /// Transitional: a connection/boot is in progress (BL-82 EM-5.9 T56.29).
    /// A minimal "Connecting…" placeholder today; EM-5.9 T56.30 enhances this
    /// into a staged-progress/MOTD/credits loading screen.
    Connecting,
    /// Connected and playing.
    InGame,
    /// Transitional: EM-2.x graphics demo scene (no menus, no server).
    #[default]
    Demo,
}
