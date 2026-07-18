//! Shared App scaffolding: states, schedule/set layout, settings,
//! diagnostics (BL-82 EM-2.1).
//!
//! Used by both the pure-Bevy client and the headless server shell.
//! Isolation law: logic crates never depend on this crate or on Bevy, and
//! this crate never depends on the sim.
//!
//! This crate pulls **no render/window features** — the render stack lives in
//! the `xindeler-client` binary. The optional `fps-overlay` cargo feature
//! (enabled by the client) adds the on-screen FPS overlay, which needs the UI
//! render stack + `default_font` from the binary's Bevy features.

pub mod sets;
pub mod settings;
pub mod state;

use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*, state::app::StatesPlugin};

pub use crate::{
    sets::{GameplaySet, MirrorSet, NetSet, PresentationSet, SimSet},
    settings::{
        ExperimentalGraphics, GraphicsSettings, GraphicsTier, MenuSettings, XindelerSettings,
    },
    state::AppState,
};

/// Registers the shared Xindeler app scaffolding:
///
/// - [`AppState`] (+ `StatesPlugin` if the host app didn't add one),
/// - the canonical [`sets`] layout (spec §2.2),
/// - [`XindelerSettings`] loaded from `<userdata>/settings.ron` at boot,
/// - frame-time diagnostics (+ FPS overlay with the `fps-overlay` feature).
pub struct XindelerAppPlugin;

impl Plugin for XindelerAppPlugin {
    fn build(&self, app: &mut App) {
        // DefaultPlugins adds StatesPlugin; MinimalPlugins (server shell)
        // does not, so make state machinery unconditional here.
        if !app.is_plugin_added::<StatesPlugin>() {
            app.add_plugins(StatesPlugin);
        }
        app.init_state::<AppState>();

        sets::configure(app);

        app.insert_resource(XindelerSettings::load_or_default());

        app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        #[cfg(feature = "fps-overlay")]
        app.add_plugins(bevy::dev_tools::fps_overlay::FpsOverlayPlugin::default());
    }
}
