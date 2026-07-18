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
//! UI-scale seam), (BL-82 EM-5.6) [`slot`] (the drag-drop item-slot
//! primitive bag/trade/hotbar/crafting screens share), and (BL-82 EM-5.17)
//! [`images`] (the `HudImages` art-pack lookup), [`orb_material`] (the
//! `UiMaterial` spike/scaffold for the liquid-orb fill), [`zlayer`] (the
//! shared `GlobalZIndex` vocabulary), and (BL-82 EM-5.17 Phase 3)
//! [`minimap_material`] (the `UiMaterial` giving the minimap its soft
//! radial alpha-feather edge — `orb_material`'s scaffold made a real
//! consumer). Deferred to the
//! screens that first need them (documented here rather than stubbed, per
//! "get the primitives right, don't over-build one-off screen-specific
//! widgets" — spec §2 engineering note): Slider/Checkbox/Radio/TextInput
//! direct wrappers (no v1 screen needs them yet), Modal dialog, Tab bar, and
//! the `.vox`-icon-as-UI-icon path (needed once a screen shows real item/
//! ability icons — EM-5.6's bag/trade screens use a themed text-glyph
//! placeholder meanwhile, see `slot`'s own doc comment). [`scroll`]
//! (ScrollView) landed with EM-5.4 (chat) — the first screen that needed it.

pub mod bar;
pub mod button;
pub mod hud_state;
pub mod i18n;
pub mod images;
pub mod minimap_material;
pub mod notification;
pub mod orb_material;
pub mod panel;
pub mod scale;
pub mod scroll;
pub mod slot;
pub mod theme;
pub mod tooltip;
pub mod zlayer;

use bevy::{
    app::{App, Plugin, Startup, Update},
    ecs::schedule::{IntoScheduleConfigs, common_conditions::resource_changed},
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
        // BL-82 EM-5.16 (T56.44): the i18n seam's own `en`-default catalog +
        // locale tracker. `Localization` is `NonSend` (see that type's own
        // doc comment for why); inserted directly here (not via a Startup
        // system) since `App::insert_non_send` needs `&mut App`, which this
        // `build` already has.
        app.insert_non_send(i18n::Localization::load(
            &i18n::fallback_locale(),
            i18n::DEFAULT_HUD_FTL_FILES,
        ));
        app.init_resource::<i18n::CurrentLocale>();
        app.init_resource::<hud_state::HudState>()
            .add_message::<hud_state::HudAction>()
            .init_resource::<notification::NotificationQueue>()
            // BL-82 EM-5.17 T57.7: `HudImages` loads alongside `HudTheme`/
            // `HudFonts` — both are flat resource-inserting systems with no
            // dependency on each other, so they run in the same Startup
            // batch (order between the two doesn't matter); a future screen
            // needing `HudImages` orders its own spawn system
            // `.after(images::init_images)`, mirroring the existing
            // `.after(theme::init_theme)` convention below.
            .add_systems(Startup, (theme::init_theme, images::init_images))
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
                    bar::update_orb_bars,
                    bar::update_horizontal_image_bars,
                    // BL-82 EM-5.16 (T56.44): ordered AFTER `LocaleSyncSet` so
                    // a `HudButtonLabel` a locale hot-swap just rewrote (via
                    // `i18n::relocalize_button_labels`) propagates onto the
                    // button's real `Text` child in the SAME frame, not one
                    // frame late.
                    button::spawn_button_labels.after(i18n::LocaleSyncSet),
                    button::update_button_visuals,
                    button::update_image_button_visuals,
                    tooltip::update_tooltip,
                    notification::advance_notifications,
                    slot::update_slot_visuals,
                    // BL-82 EM-5.5: the generic HudAction -> HudState wiring
                    // (see that function's own doc comment for why it lives
                    // here rather than per-screen).
                    hud_state::apply_hud_actions,
                ),
            )
            // BL-82 EM-5.16 (T56.44): the reactive i18n hot-swap chain —
            // reload the `Localization` bundle for `CurrentLocale`'s new tag,
            // then re-resolve every tagged `LocalizedText`/`LocalizedLabel`
            // entity, all gated on `CurrentLocale` actually changing (see
            // `i18n`'s own module doc for the full flow + who's responsible
            // for changing `CurrentLocale` in the first place).
            .add_systems(
                Update,
                (
                    i18n::reload_localization_on_locale_change,
                    i18n::relocalize_text,
                    i18n::relocalize_button_labels,
                )
                    .chain()
                    .in_set(i18n::LocaleSyncSet)
                    .run_if(resource_changed::<i18n::CurrentLocale>),
            );
        // BL-82 EM-5.3/EM-5.6: the drag-drop slot primitive's global
        // drag/drop observers + its `SlotDropped` message — not per-entity,
        // every `HudSlot` anywhere in the app is drag-drop-capable the
        // moment it's spawned.
        slot::install_observers(app);
        // BL-82 EM-5.17 T57.9: registers the `OrbLiquidMaterial` UiMaterial
        // scaffolding (embedded WGSL + `UiMaterialPlugin`) — see
        // `orb_material`'s module doc comment for the full spike write-up
        // and why Phase 2's actual orb rendering is expected to use the
        // CPU-clip `bar::spawn_orb_bar` path instead, at least for v1.
        app.add_plugins(orb_material::OrbMaterialPlugin);
        // BL-82 EM-5.17 Phase 3: registers `MinimapFadeMaterial` (embedded
        // WGSL + `UiMaterialPlugin`) — `map_view`'s minimap is this
        // material's real consumer, unlike `OrbLiquidMaterial` above.
        app.add_plugins(minimap_material::MinimapMaterialPlugin);
    }
}
