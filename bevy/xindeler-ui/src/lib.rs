//! BL-82 EM-5.1 — the Xindeler widget kit: themed primitives on
//! `bevy_ui` + `bevy_ui_widgets` + `EditableText`, a theme/token layer, the
//! i18n seam, the HUD state machine, and the real (queue-based) notification
//! widget that subsumes EM-4.8's throwaway toast.
//!
//! ## Toolkit decision (locked, spec §4/§9 [Q1])
//! `bevy_feathers` is Bevy's own experimental, EDITOR-tooling widget set —
//! its docs say plainly it is "not intended [for game UI]" and recommend
//! "copying this code into your own project" rather than depending on it.
//! This crate does exactly that: it builds on the STABLE pieces (`bevy_ui`'s
//! retained-mode tree, `bevy_ui_widgets`' headless button/checkbox/slider
//! behaviours, `bevy_text`'s `EditableText` input) with Xindeler's OWN theme
//! (copying Feathers' token/theme *approach*, never its crate). **Zero new
//! UI third-party deps** — `fluent`/`unic-langid` (the i18n seam, §4) are the
//! one sanctioned addition, and they are engine-agnostic `.ftl` parsing
//! crates, not a UI toolkit.
//!
//! ## v1 scope (this is the FOUNDATION, not every possible widget)
//! Shipped: themed [`panel`] (background/border/radius), [`bar`] (progress
//! bar/globe — health/energy/poise/XP), [`button`] (wraps
//! `bevy_ui_widgets::Button`), [`tooltip`] (hover, screen-anchored),
//! [`notification`] (the real queued toast, subsuming EM-4.8), [`theme`]
//! (colour/spacing/radius/font tokens), [`i18n`] (the fluent `.ftl` seam),
//! [`hud_state`] (the `Show`-replacement state machine), [`scale`] (the
//! UI-scale seam). Deferred to the screens that first need them (documented
//! here rather than stubbed, per "get the primitives right, don't over-build
//! one-off screen-specific widgets" — spec §2 engineering note): Slider/
//! Checkbox/Radio/TextInput direct wrappers (no v1 screen needs them yet —
//! EM-5.2's proof slice needs Panel/Bar/Button/Tooltip/Notification only),
//! List/Grid, Modal dialog, Tab bar, the drag-drop slot (needed by
//! EM-5.3/5.6/5.7/5.15 — lands with whichever of those is first), and the
//! `.vox`-icon-as-UI-icon path (needed once a screen shows real item/ability
//! icons — EM-5.3's hotbar is the first). [`scroll`] (ScrollView) landed with
//! EM-5.4 (chat) — the first screen that needed it.

pub mod bar;
pub mod button;
pub mod hud_state;
pub mod i18n;
pub mod notification;
pub mod panel;
pub mod scale;
pub mod scroll;
pub mod slot;
pub mod theme;
pub mod tooltip;

use bevy::{
    app::{App, Plugin, Startup, Update},
    ecs::schedule::IntoScheduleConfigs,
};
use bevy_ui_widgets::UiWidgetsPlugins;

/// Installs the whole widget kit: the headless `bevy_ui_widgets` behaviour
/// plugins, the theme (loaded at `Startup`), and the per-frame systems every
/// primitive needs (bar-fill resize, button restyle, tooltip show/hide,
/// notification queue advance). Screens add their OWN widgets on top of
/// these primitives; this plugin owns no screen content.
pub struct XindelerUiPlugin;

impl Plugin for XindelerUiPlugin {
    fn build(&self, app: &mut App) {
        // `UiWidgetsPlugins` is a `PluginGroup`, not a `Plugin` —
        // `is_plugin_added` doesn't apply to it directly, AND (found via the
        // live listen-server smoke run, not just a compile check) its member
        // plugins do NOT no-op on a double-add the way most Bevy plugins do
        // — `bevy`'s own `DefaultPlugins` (which `xindeler-client`'s
        // `AddPlugins` group already includes, since this workspace's `bevy`
        // dependency enables `bevy_ui_widgets` via its default feature set)
        // already adds `UiWidgetsPlugins` once; adding it again here panics
        // ("plugin was already added in application") instead of silently
        // skipping. Guard on one representative member plugin instead.
        if !app.is_plugin_added::<bevy_ui_widgets::ButtonPlugin>() {
            app.add_plugins(UiWidgetsPlugins);
        }
        app.init_resource::<hud_state::HudState>()
            .add_message::<hud_state::HudAction>()
            .init_resource::<notification::NotificationQueue>()
            .add_systems(Startup, theme::init_theme)
            .add_systems(
                Startup,
                (
                    tooltip::spawn_shared_tooltip_label,
                    notification::spawn_shared_notification_widget,
                )
                    .after(theme::init_theme),
            )
            .add_systems(
                Update,
                (
                    bar::update_bars,
                    button::spawn_button_labels,
                    button::update_button_visuals,
                    tooltip::update_tooltip,
                    notification::advance_notifications,
                    slot::update_slot_visuals,
                ),
            );
        // BL-82 EM-5.3: the drag-drop slot primitive's global observers (not
        // per-entity — every `HudSlot` anywhere in the app is drag-drop-
        // capable the moment it's spawned).
        slot::install_observers(app);
    }
}
