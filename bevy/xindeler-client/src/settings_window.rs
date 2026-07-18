//! BL-82 EM-5.12 (T56.39) — the tabbed Settings window.
//!
//! Ports legacy `voxygen`'s `settings_window/` (Interface/Video/Sound/
//! Controls/Gameplay/Chat/Language tabs) into the Bevy client as ONE modal
//! window (`HudWindow::Settings`) with a left-hand tab bar + a per-tab content
//! pane. It is the ORGANIZING surface for the settings that already exist
//! scattered across the client — it invents almost no new backend:
//!
//! | Tab           | Backend it drives                                   | Real? |
//! |---------------|-----------------------------------------------------|-------|
//! | Interface     | `XindelerSettings::ui_scale` (→ `hud_scale.rs`),    | ✅    |
//! |               | `interface.show_crosshair` (→ `combat_hud::Crosshair`)|      |
//! | Video         | `GraphicsSettings` (tier presets + all toggles);    | ✅    |
//! |               | SSAO/TAA apply LIVE, the rest persist for next boot |       |
//! | Controls      | opens the EM-5.11 rebinding screen (`HudWindow::    | ✅    |
//! |               | Controls`, keyboard+gamepad) — not rebuilt here     |       |
//! | Gameplay      | `CameraSettings` (mouse sensitivity, fly speed)     | ✅    |
//! | Chat          | `chat.opacity` (→ `chat::sync_chat_scroll_opacity`) | ✅    |
//! | Language       | `XindelerSettings::language` selector              | ~     |
//! | Networking    | nothing configurable today (connection is automatic)| stub  |
//! | Sound         | EM-5.10 audio — not built yet                       | stub  |
//! | Accessibility | `XindelerSettings::accessibility` (reduce-flashing  | ✅    |
//! |               | dampens `combat_hud`'s damage vignette; high-contrast|      |
//! |               | UI reconciles `HudTheme` live) + a tutorial-overlay  |      |
//! |               | reopen button. Positional-sound subtitles stay a stub|      |
//! |               | pending EM-5.10b (SFX) audio                        |       |
//!
//! Language is `~` (functional-but-simple): the selector persists
//! `XindelerSettings::language`, but the `xindeler-ui` i18n seam ships the
//! `en` catalog only until EM-5.16 wires the full reactive Fluent pipeline +
//! hot-swap, so only `en` resolves for now (the tab says so). Sound/
//! Networking stay HONEST stubs — a visible tab that names the epic that
//! will fill it, never fake toggles for a system that isn't built.
//! Accessibility itself is real now (BL-82 EM-5.16, T56.43 part 1/2) — only
//! its positional-sound-subtitle sub-feature remains a stub, pending
//! `SfxTriggerItem` from the not-yet-landed EM-5.10b audio phase.
//!
//! ## Live graphics apply (moved here from the old `esc_menu`)
//! [`apply_graphics_settings`] reconciles the live camera to
//! [`GraphicsSettings`] whenever it changes (SSAO/TAA inserted/removed on the
//! camera — no restart), exactly as the pre-EM-5.12 esc-menu Video slice did;
//! the shadow-cascade COUNT still applies on the NEXT launch (changing it on
//! the live sun aborts `bevy_light` — see this function's doc). Every change
//! persists to `settings.ron` and forces `GraphicsTier::Custom` so a
//! hand-edited toggle isn't clobbered by a preset on next load.
//!
//! Compiled only under `listen-server`/`net-client`, matching every other
//! `xindeler_ui`-consuming screen module in this crate.

use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    color::Alpha as _,
    core_pipeline::prepass::DepthPrepass,
    ecs::schedule::common_conditions::{not, resource_changed},
    pbr::ScreenSpaceAmbientOcclusion,
    prelude::*,
    render::camera::{MipBias, TemporalJitter},
};
use xindeler_app::{GraphicsTier, XindelerSettings};
use xindeler_input::{ActionState, GameInput};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    panel::panel_bundle,
    theme::{HudFonts, HudPalette, HudTheme},
    zlayer,
};

use crate::{camera::MainCamera, chat::text_input_focused, combat_hud::Crosshair};

/// The v1 selectable UI locales. Only `en` has a catalog today (the i18n seam
/// is `en`-only until EM-5.16); extend this list — nothing else changes — as
/// real `.ftl` locales land.
const AVAILABLE_LANGUAGES: &[&str] = &["en"];

// Numeric-control step / clamp bounds (px-free, per setting).
const UI_SCALE_STEP: f32 = 0.1;
const UI_SCALE_MIN: f32 = 0.5;
const UI_SCALE_MAX: f32 = 2.0;
const SENS_STEP: f32 = 0.0005;
const SENS_MIN: f32 = 0.0005;
const SENS_MAX: f32 = 0.01;
const FLY_STEP: f32 = 2.0;
const FLY_MIN: f32 = 2.0;
const FLY_MAX: f32 = 40.0;
const OPACITY_STEP: f32 = 0.05;
const MIN_SHADOW_CASCADES: u8 = 1;
const MAX_SHADOW_CASCADES: u8 = 4;

/// Installs the tabbed settings window: spawns the (hidden) modal at
/// `Startup`, opens it on the `Settings` input (F10 by default), keeps its
/// visibility + tab panes + value labels synced, and applies graphics changes
/// live.
pub struct SettingsWindowPlugin;

impl Plugin for SettingsWindowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedSettingsTab>()
            .add_systems(
                Startup,
                spawn_settings_window.after(xindeler_ui::theme::init_theme),
            )
            .add_systems(
                Update,
                (
                    // Reads `ActionState` — order after the frame's real input
                    // resolution, like every other `ActionState`-reading
                    // toggle in this crate. Gated on `!text_input_focused` so
                    // F10 (or a rebind) while typing in chat is swallowed by
                    // the chat box, not the settings window.
                    toggle_settings_window
                        .after(xindeler_input::InputResolveSet)
                        .before(xindeler_ui::hud_state::apply_hud_actions)
                        .run_if(not(text_input_focused)),
                    sync_settings_window_visibility
                        .after(xindeler_ui::hud_state::apply_hud_actions),
                    sync_tab_panes,
                    refresh_setting_labels,
                    sync_crosshair_visibility,
                    apply_graphics_settings.run_if(resource_changed::<XindelerSettings>),
                    apply_accessibility_theme.run_if(resource_changed::<XindelerSettings>),
                ),
            );
    }
}

/// The nine settings tabs (mirrors legacy `voxygen`'s tab set, plus the two
/// new-stack epics Networking/Accessibility get their own honest-stub tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
enum SettingsTab {
    #[default]
    Interface,
    Video,
    Controls,
    Gameplay,
    Chat,
    Language,
    Networking,
    Sound,
    Accessibility,
}

impl SettingsTab {
    const ALL: &'static [SettingsTab] = &[
        SettingsTab::Interface,
        SettingsTab::Video,
        SettingsTab::Controls,
        SettingsTab::Gameplay,
        SettingsTab::Chat,
        SettingsTab::Language,
        SettingsTab::Networking,
        SettingsTab::Sound,
        SettingsTab::Accessibility,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Interface => "Interface",
            SettingsTab::Video => "Video",
            SettingsTab::Controls => "Controls",
            SettingsTab::Gameplay => "Gameplay",
            SettingsTab::Chat => "Chat",
            SettingsTab::Language => "Language",
            SettingsTab::Networking => "Networking",
            SettingsTab::Sound => "Sound",
            SettingsTab::Accessibility => "Accessibility",
        }
    }
}

/// The currently-shown settings tab (client-local UI state, not persisted).
#[derive(Resource, Default)]
struct SelectedSettingsTab(SettingsTab);

/// The full-screen modal backdrop root (its [`Visibility`] mirrors
/// `HudState::is_open(HudWindow::Settings)`).
#[derive(Component)]
struct SettingsWindowRoot;

/// A tab button in the tab bar (its background highlights when selected).
#[derive(Component, Clone, Copy)]
struct SettingsTabButton(SettingsTab);

/// The content pane for one tab (its [`Visibility`] mirrors the selected tab).
#[derive(Component, Clone, Copy)]
struct SettingsTabPane(SettingsTab);

/// One adjustable setting. Every button in the window edits one of these; the
/// value label next to it reads back through [`value_label`].
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum SettingControl {
    // Numeric (−/+ buttons):
    UiScale,
    MouseSensitivity,
    FlySpeed,
    ChatOpacity,
    ShadowCascades,
    // Boolean (single cycle button):
    ShowCrosshair,
    Ssao,
    Taa,
    Bloom,
    VolumetricFog,
    ContactShadows,
    Vignette,
    ReduceFlashing,
    HighContrastUi,
    // Enum (single cycle button):
    Tier,
    Language,
}

/// Tags the [`Text`] (or a button whose child is the text) that displays a
/// control's current value, so [`refresh_setting_labels`] can relabel it.
#[derive(Component, Clone, Copy)]
struct SettingValueLabel(SettingControl);

/// F10 (or whatever [`GameInput::Settings`] is bound to) toggles the window —
/// same convention every other HUD window uses.
fn toggle_settings_window(action_state: Res<ActionState>, mut actions: MessageWriter<HudAction>) {
    if action_state.just_pressed(GameInput::Settings) {
        actions.write(HudAction::ToggleWindow(HudWindow::Settings));
    }
}

/// Mirrors [`HudState`]'s open window onto the root's [`Visibility`] (read-only
/// w.r.t. [`HudAction`] — `apply_hud_actions` is the one applier).
fn sync_settings_window_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<SettingsWindowRoot>>,
) {
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if hud_state.is_open(HudWindow::Settings) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Shows the selected tab's pane (and hides the rest) + highlights the active
/// tab button. Gated on `is_changed` so it only walks the panes when the
/// selection actually changes.
fn sync_tab_panes(
    selected: Res<SelectedSettingsTab>,
    theme: Res<HudTheme>,
    mut panes: Query<(&SettingsTabPane, &mut Visibility)>,
    mut tabs: Query<(&SettingsTabButton, &mut BackgroundColor)>,
) {
    if !selected.is_changed() {
        return;
    }
    for (pane, mut visibility) in &mut panes {
        *visibility = if pane.0 == selected.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    for (tab, mut bg) in &mut tabs {
        bg.0 = if tab.0 == selected.0 {
            theme.palette.accent
        } else {
            theme.palette.panel_bg
        };
    }
}

/// Crosshair [`Visibility`] follows `interface.show_crosshair` — the live
/// Interface-tab toggle. One entity, so no `is_changed` gate is needed; it
/// only writes when the value actually differs.
fn sync_crosshair_visibility(
    settings: Res<XindelerSettings>,
    mut crosshair: Query<&mut Visibility, With<Crosshair>>,
) {
    let Ok(mut visibility) = crosshair.single_mut() else {
        return;
    };
    let desired = if settings.interface.show_crosshair {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if *visibility != desired {
        *visibility = desired;
    }
}

/// Refreshes every value label from the current settings (so a click's effect
/// is immediately visible). A [`SettingValueLabel`] sits either directly on a
/// [`Text`] node (numeric rows) or on a button whose child carries the text
/// (toggle/enum rows) — handle both.
fn refresh_setting_labels(
    settings: Res<XindelerSettings>,
    labels: Query<(Entity, &SettingValueLabel, Option<&Children>)>,
    mut texts: Query<&mut Text>,
) {
    if !settings.is_changed() {
        return;
    }
    for (entity, label, children) in &labels {
        let new = value_label(label.0, &settings);
        if let Ok(mut text) = texts.get_mut(entity) {
            if text.0 != new {
                text.0 = new;
            }
            continue;
        }
        if let Some(children) = children {
            for &child in children {
                if let Ok(mut text) = texts.get_mut(child)
                    && text.0 != new
                {
                    text.0 = new.clone();
                }
            }
        }
    }
}

/// Spawns the whole window: a full-screen modal backdrop, a titled panel, the
/// tab bar, and one content pane per [`SettingsTab`].
fn spawn_settings_window(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    settings: Res<XindelerSettings>,
) {
    let theme: HudTheme = *theme;
    let selected = SettingsTab::default();
    commands
        .spawn((
            SettingsWindowRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::MODAL_WINDOWS),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .with_children(|screen| {
            let mut panel_entity = screen.spawn(panel_bundle(&theme));
            let row_gap_px = theme.spacing.sm;
            panel_entity.entry::<Node>().and_modify(move |mut node| {
                node.flex_direction = FlexDirection::Column;
                node.row_gap = Val::Px(row_gap_px);
                node.min_width = Val::Px(520.0);
                node.max_height = Val::Percent(88.0);
                node.align_items = AlignItems::Stretch;
            });
            panel_entity.with_children(|panel| {
                heading(panel, &fonts, &theme, "Settings", 28.0);
                spawn_tab_bar(panel, &theme, &fonts, selected);

                // Content region: one pane per tab, only the selected one
                // visible. `overflow: clip_y` keeps a long tab (Video) inside
                // the panel instead of spilling past it.
                panel
                    .spawn(Node {
                        flex_direction: FlexDirection::Column,
                        min_height: Val::Px(260.0),
                        max_height: Val::Percent(70.0),
                        overflow: Overflow::clip_y(),
                        ..Default::default()
                    })
                    .with_children(|content| {
                        for &tab in SettingsTab::ALL {
                            spawn_tab_pane(content, &theme, &fonts, &settings, tab, selected);
                        }
                    });

                panel.spawn(button_bundle(&theme, &fonts, "Close")).observe(
                    |_a: On<Activate>, mut actions: MessageWriter<HudAction>| {
                        actions.write(HudAction::CloseWindow);
                    },
                );
            });
        });
}

/// The horizontal tab bar (one button per tab; the selected one starts
/// highlighted).
fn spawn_tab_bar(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    selected: SettingsTab,
) {
    panel
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(theme.spacing.xs),
            flex_wrap: FlexWrap::Wrap,
            row_gap: Val::Px(theme.spacing.xs),
            ..Default::default()
        })
        .with_children(|bar| {
            for &tab in SettingsTab::ALL {
                let mut button = bar.spawn(button_bundle(theme, fonts, tab.label()));
                button.insert(SettingsTabButton(tab));
                if tab == selected {
                    button.insert(BackgroundColor(theme.palette.accent));
                }
                button.observe(
                    move |_a: On<Activate>, mut sel: ResMut<SelectedSettingsTab>| {
                        sel.0 = tab;
                    },
                );
            }
        });
}

/// Spawns one tab's content pane (hidden unless it's the initially-selected
/// tab).
fn spawn_tab_pane(
    content: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    tab: SettingsTab,
    selected: SettingsTab,
) {
    let visibility = if tab == selected {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    content
        .spawn((SettingsTabPane(tab), visibility, Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(theme.spacing.sm),
            ..Default::default()
        }))
        .with_children(|pane| match tab {
            SettingsTab::Interface => {
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "UI scale",
                    SettingControl::UiScale,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Crosshair",
                    SettingControl::ShowCrosshair,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    "Further HUD-element toggles (bars, buff strip) arrive with their screens.",
                );
            },
            SettingsTab::Video => {
                enum_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Quality preset",
                    SettingControl::Tier,
                );
                toggle_row(pane, theme, fonts, settings, "SSAO", SettingControl::Ssao);
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Anti-aliasing (TAA)",
                    SettingControl::Taa,
                );
                toggle_row(pane, theme, fonts, settings, "Bloom", SettingControl::Bloom);
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Volumetric fog",
                    SettingControl::VolumetricFog,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Contact shadows",
                    SettingControl::ContactShadows,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Vignette",
                    SettingControl::Vignette,
                );
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Shadow cascades",
                    SettingControl::ShadowCascades,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    "SSAO and anti-aliasing apply immediately. Other video options (and the \
                     shadow-cascade count) apply on the next launch. Editing any of these \
                     switches the preset to Custom.",
                );
            },
            SettingsTab::Controls => {
                note(
                    pane,
                    fonts,
                    theme,
                    "Rebind keyboard, mouse and gamepad controls in the dedicated Controls screen.",
                );
                pane.spawn(button_bundle(theme, fonts, "Open Controls / Rebinding"))
                    .observe(|_a: On<Activate>, mut actions: MessageWriter<HudAction>| {
                        actions.write(HudAction::ToggleWindow(HudWindow::Controls));
                    });
            },
            SettingsTab::Gameplay => {
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Mouse sensitivity",
                    SettingControl::MouseSensitivity,
                );
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Fly-cam speed",
                    SettingControl::FlySpeed,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    "Camera zoom and auto-walk settings arrive with the third-person camera \
                     polish.",
                );
            },
            SettingsTab::Chat => {
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Background opacity",
                    SettingControl::ChatOpacity,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    "Chat box opacity applies live to the scrollback panel.",
                );
            },
            SettingsTab::Language => {
                enum_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Language",
                    SettingControl::Language,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    "Only English ships in v1. Full multi-language coverage and live hot-swap \
                     arrive with EM-5.16.",
                );
            },
            SettingsTab::Networking => {
                note(
                    pane,
                    fonts,
                    theme,
                    "No configurable networking settings yet — the client connects automatically. \
                     Connection tuning arrives with the server browser (EM-5.9).",
                );
            },
            SettingsTab::Sound => {
                note(
                    pane,
                    fonts,
                    theme,
                    "Sound settings coming soon (EM-5.10 audio).",
                );
            },
            SettingsTab::Accessibility => {
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "Reduce flashing",
                    SettingControl::ReduceFlashing,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    "High-contrast UI",
                    SettingControl::HighContrastUi,
                );
                pane.spawn(button_bundle(theme, fonts, "Show tutorial again"))
                    .observe(|_a: On<Activate>, mut actions: MessageWriter<HudAction>| {
                        actions.write(HudAction::ToggleWindow(HudWindow::Tutorial));
                    });
                note(
                    pane,
                    fonts,
                    theme,
                    "Reduce flashing dampens the low-health damage vignette to fully transparent \
                     (the death screen itself always still shows at 0 health). High-contrast UI \
                     brightens muted text and makes panel backgrounds fully opaque — full effect \
                     on next launch. Subtitles for positional sound cues are pending EM-5.10b \
                     (SFX) audio.",
                );
            },
        });
}

/// A section/title heading line inside the panel.
fn heading(
    panel: &mut ChildSpawnerCommands,
    fonts: &HudFonts,
    theme: &HudTheme,
    text: &str,
    size: f32,
) {
    panel.spawn((
        Text(text.to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.title.clone()),
            font_size: bevy::text::FontSize::Px(size),
            ..Default::default()
        },
        TextColor(theme.palette.text),
    ));
}

/// A muted explanatory note line.
fn note(panel: &mut ChildSpawnerCommands, fonts: &HudFonts, theme: &HudTheme, text: &str) {
    panel.spawn((
        Text(text.to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(13.0),
            ..Default::default()
        },
        TextColor(theme.palette.text_muted),
        Node {
            max_width: Val::Px(480.0),
            ..Default::default()
        },
    ));
}

/// The name label that opens every setting row.
fn row_label(row: &mut ChildSpawnerCommands, theme: &HudTheme, fonts: &HudFonts, name: &str) {
    row.spawn((
        Text(name.to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(16.0),
            ..Default::default()
        },
        TextColor(theme.palette.text),
        Node {
            width: Val::Px(200.0),
            ..Default::default()
        },
    ));
}

fn row_node(theme: &HudTheme) -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        column_gap: Val::Px(theme.spacing.sm),
        align_items: AlignItems::Center,
        ..Default::default()
    }
}

/// A boolean row: name + a single button whose label is `On`/`Off` and which
/// flips the value on click.
fn toggle_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    name: &str,
    control: SettingControl,
) {
    cycle_row(pane, theme, fonts, settings, name, control);
}

/// An enum row: name + a single button whose label is the current variant and
/// which cycles it on click.
fn enum_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    name: &str,
    control: SettingControl,
) {
    cycle_row(pane, theme, fonts, settings, name, control);
}

/// Shared spawn for the single-cycle-button rows (bool + enum): clicking the
/// value button advances the setting (`adjust` with `dir = 1`).
fn cycle_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    name: &str,
    control: SettingControl,
) {
    pane.spawn(row_node(theme)).with_children(|row| {
        row_label(row, theme, fonts, name);
        row.spawn(button_bundle(theme, fonts, &value_label(control, settings)))
            .insert(SettingValueLabel(control))
            .observe(
                move |_a: On<Activate>, mut settings: ResMut<XindelerSettings>| {
                    apply_and_save(control, 1, &mut settings);
                },
            );
    });
}

/// A numeric row: name + `[−]` + value text + `[+]`.
fn numeric_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    name: &str,
    control: SettingControl,
) {
    pane.spawn(row_node(theme)).with_children(|row| {
        row_label(row, theme, fonts, name);
        row.spawn(button_bundle(theme, fonts, "-")).observe(
            move |_a: On<Activate>, mut settings: ResMut<XindelerSettings>| {
                apply_and_save(control, -1, &mut settings);
            },
        );
        row.spawn((
            SettingValueLabel(control),
            Text(value_label(control, settings)),
            TextFont {
                font: bevy::text::FontSource::Handle(fonts.body.clone()),
                font_size: bevy::text::FontSize::Px(16.0),
                ..Default::default()
            },
            TextColor(theme.palette.text),
            Node {
                width: Val::Px(72.0),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
        ));
        row.spawn(button_bundle(theme, fonts, "+")).observe(
            move |_a: On<Activate>, mut settings: ResMut<XindelerSettings>| {
                apply_and_save(control, 1, &mut settings);
            },
        );
    });
}

/// Applies a control change and persists `settings.ron`.
fn apply_and_save(control: SettingControl, dir: i8, settings: &mut XindelerSettings) {
    adjust(control, dir, settings);
    if let Err(err) = settings.save() {
        error!("settings window: failed to persist settings.ron after a change: {err}");
    }
}

/// A short `On`/`Off` label for a boolean.
fn on_off(value: bool) -> String { if value { "On" } else { "Off" }.to_owned() }

/// The current value of a control, formatted for display.
fn value_label(control: SettingControl, settings: &XindelerSettings) -> String {
    let g = &settings.graphics;
    match control {
        SettingControl::UiScale => format!("{:.0}%", settings.ui_scale * 100.0),
        SettingControl::MouseSensitivity => format!("{:.4}", settings.camera.mouse_sensitivity),
        SettingControl::FlySpeed => format!("{:.0}", settings.camera.fly_speed),
        SettingControl::ChatOpacity => format!("{:.0}%", settings.chat.opacity * 100.0),
        SettingControl::ShadowCascades => g
            .shadow_cascades
            .clamp(MIN_SHADOW_CASCADES, MAX_SHADOW_CASCADES)
            .to_string(),
        SettingControl::ShowCrosshair => on_off(settings.interface.show_crosshair),
        SettingControl::Ssao => on_off(g.ssao),
        SettingControl::Taa => on_off(g.taa),
        SettingControl::Bloom => on_off(g.bloom),
        SettingControl::VolumetricFog => on_off(g.volumetric_fog),
        SettingControl::ContactShadows => on_off(g.contact_shadows),
        SettingControl::Vignette => on_off(g.vignette),
        SettingControl::ReduceFlashing => on_off(settings.accessibility.reduce_flashing),
        SettingControl::HighContrastUi => on_off(settings.accessibility.high_contrast_ui),
        SettingControl::Tier => format!("{:?}", g.tier),
        SettingControl::Language => settings.language.clone(),
    }
}

/// Whether editing this control must force [`GraphicsTier::Custom`] — true for
/// the toggles a tier preset OWNS (so a hand-edit isn't clobbered on reload),
/// false for tier-independent ones (vignette) and non-graphics settings.
fn forces_custom_tier(control: SettingControl) -> bool {
    matches!(
        control,
        SettingControl::Ssao
            | SettingControl::Taa
            | SettingControl::Bloom
            | SettingControl::VolumetricFog
            | SettingControl::ContactShadows
            | SettingControl::ShadowCascades
    )
}

/// Advances a control's value. `dir` is `+1`/`-1` for numeric rows and `+1`
/// (cycle forward) for boolean/enum rows.
fn adjust(control: SettingControl, dir: i8, settings: &mut XindelerSettings) {
    let df = f32::from(dir);
    match control {
        SettingControl::UiScale => {
            settings.ui_scale =
                snap((settings.ui_scale + UI_SCALE_STEP * df).clamp(UI_SCALE_MIN, UI_SCALE_MAX));
        },
        SettingControl::MouseSensitivity => {
            settings.camera.mouse_sensitivity = round4(
                (settings.camera.mouse_sensitivity + SENS_STEP * df).clamp(SENS_MIN, SENS_MAX),
            );
        },
        SettingControl::FlySpeed => {
            settings.camera.fly_speed =
                (settings.camera.fly_speed + FLY_STEP * df).clamp(FLY_MIN, FLY_MAX);
        },
        SettingControl::ChatOpacity => {
            settings.chat.opacity =
                snap((settings.chat.opacity + OPACITY_STEP * df).clamp(0.0, 1.0));
        },
        SettingControl::ShadowCascades => {
            let next = i32::from(settings.graphics.shadow_cascades) + i32::from(dir);
            settings.graphics.shadow_cascades = next.clamp(
                i32::from(MIN_SHADOW_CASCADES),
                i32::from(MAX_SHADOW_CASCADES),
            ) as u8;
        },
        SettingControl::ShowCrosshair => {
            settings.interface.show_crosshair = !settings.interface.show_crosshair;
        },
        SettingControl::Ssao => settings.graphics.ssao = !settings.graphics.ssao,
        SettingControl::Taa => settings.graphics.taa = !settings.graphics.taa,
        SettingControl::Bloom => settings.graphics.bloom = !settings.graphics.bloom,
        SettingControl::VolumetricFog => {
            settings.graphics.volumetric_fog = !settings.graphics.volumetric_fog;
        },
        SettingControl::ContactShadows => {
            settings.graphics.contact_shadows = !settings.graphics.contact_shadows;
        },
        SettingControl::Vignette => settings.graphics.vignette = !settings.graphics.vignette,
        SettingControl::ReduceFlashing => {
            settings.accessibility.reduce_flashing = !settings.accessibility.reduce_flashing;
        },
        SettingControl::HighContrastUi => {
            settings.accessibility.high_contrast_ui = !settings.accessibility.high_contrast_ui;
        },
        SettingControl::Tier => {
            settings.graphics.tier = next_tier(settings.graphics.tier);
            // A non-Custom tier's preset re-applies its toggle values.
            settings.graphics.sanitize();
            return;
        },
        SettingControl::Language => {
            settings.language = next_language(&settings.language);
            return;
        },
    }
    // A hand-edited toggle a tier preset owns must survive reload — a
    // non-Custom tier would clobber it on next `sanitize()`.
    if forces_custom_tier(control) {
        settings.graphics.tier = GraphicsTier::Custom;
    }
}

/// Rounds to 2 decimals (cancels `f32` step-accumulation drift on the
/// percentage-style controls).
fn snap(value: f32) -> f32 { (value * 100.0).round() / 100.0 }

/// Rounds to 4 decimals (mouse sensitivity's display precision).
fn round4(value: f32) -> f32 { (value * 10_000.0).round() / 10_000.0 }

/// Cycles the graphics tier `Low → Medium → High → Ultra → Custom → Low`.
fn next_tier(tier: GraphicsTier) -> GraphicsTier {
    match tier {
        GraphicsTier::Low => GraphicsTier::Medium,
        GraphicsTier::Medium => GraphicsTier::High,
        GraphicsTier::High => GraphicsTier::Ultra,
        GraphicsTier::Ultra => GraphicsTier::Custom,
        GraphicsTier::Custom => GraphicsTier::Low,
    }
}

/// Cycles to the next available UI locale (wraps). With a single-entry
/// [`AVAILABLE_LANGUAGES`] this is a no-op — the point is that adding a locale
/// makes it selectable with no other change.
fn next_language(current: &str) -> String {
    let idx = AVAILABLE_LANGUAGES
        .iter()
        .position(|&l| l == current)
        .unwrap_or(0);
    AVAILABLE_LANGUAGES[(idx + 1) % AVAILABLE_LANGUAGES.len()].to_owned()
}

/// Reconciles the live CAMERA render components (SSAO/TAA) to match
/// [`XindelerSettings`] — this is what makes those two toggles apply WITHOUT a
/// restart. Idempotent: only inserts/removes a component when the live state
/// doesn't already match. Moved verbatim from the pre-EM-5.12 `esc_menu`
/// (which no longer owns any graphics UI).
///
/// It deliberately does NOT touch the sun's `CascadeShadowConfig`: changing
/// the cascade COUNT on the live directional light aborts the client from a
/// `bevy_light` internal (stale per-thread visibility scratch in
/// `check_dir_light_mesh_visibility`). The cascade count is read once at
/// startup by `crate::light::spawn_light_rig`, so a changed value is simply
/// persisted here and takes effect on the next launch.
fn apply_graphics_settings(
    settings: Res<XindelerSettings>,
    mut commands: Commands,
    cameras: Query<
        (
            Entity,
            Has<ScreenSpaceAmbientOcclusion>,
            Has<TemporalAntiAliasing>,
        ),
        With<MainCamera>,
    >,
) {
    let g = &settings.graphics;
    for (camera, has_ssao, has_taa) in &cameras {
        match (g.ssao, has_ssao) {
            (true, false) => {
                commands
                    .entity(camera)
                    .insert(ScreenSpaceAmbientOcclusion::default());
            },
            (false, true) => {
                commands
                    .entity(camera)
                    .remove::<ScreenSpaceAmbientOcclusion>();
            },
            _ => {},
        }
        match (g.taa, has_taa) {
            (true, false) => {
                commands
                    .entity(camera)
                    .insert((DepthPrepass, TemporalAntiAliasing::default()));
            },
            (false, true) => {
                // Removing ONLY `TemporalAntiAliasing` leaves `TemporalJitter`'s
                // frozen sub-pixel offset + `MipBias`'s sharpening applied — a
                // permanent jitter/sharpen residue. Drop those too for a clean
                // no-TAA baseline (bevy-migration-reviewer finding, EM-5.12
                // predecessor). `DepthPrepass`/`MotionVectorPrepass` stay
                // resident (harmless).
                commands
                    .entity(camera)
                    .remove::<TemporalAntiAliasing>()
                    .remove::<TemporalJitter>()
                    .remove::<MipBias>();
            },
            _ => {},
        }
    }
}

/// Reconciles the live [`HudTheme`] palette to `accessibility.high_contrast_ui`
/// — brightens muted secondary text to the same value as regular body text,
/// and makes panel backgrounds fully opaque. Unlike
/// [`apply_graphics_settings`]'s SSAO/TAA (which reconcile a LIVE render
/// component every widget re-reads every frame), most `bevy_ui` widgets in this
/// crate bake their `TextColor`/ `BackgroundColor` from [`HudTheme`] ONCE at
/// spawn time — so this system keeps the theme resource itself always correct
/// (a widget spawned/ respawned/reopened after a toggle picks up the new
/// palette immediately), but an ALREADY-open panel's already-baked colours only
/// refresh the next time that panel is (re)spawned — same "applies on next
/// launch" honesty the Video tab's shadow-cascade count already documents for a
/// comparable live-vs-baked gap.
fn apply_accessibility_theme(settings: Res<XindelerSettings>, mut theme: ResMut<HudTheme>) {
    let defaults = HudPalette::default();
    let desired = if settings.accessibility.high_contrast_ui {
        HudPalette {
            text_muted: defaults.text,
            panel_bg: defaults.panel_bg.with_alpha(1.0),
            ..defaults
        }
    } else {
        defaults
    };
    if theme.palette.text_muted != desired.text_muted {
        theme.palette.text_muted = desired.text_muted;
    }
    if theme.palette.panel_bg != desired.panel_bg {
        theme.palette.panel_bg = desired.panel_bg;
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// `SettingsWindowRoot` is a full-screen modal backdrop exactly like
    /// `DiaryWindowRoot`/`EscMenuRoot`/`ControlsScreenRoot`, and must carry
    /// `GlobalZIndex(MODAL_WINDOWS)` so `bevy_ui` picking routes clicks to it
    /// rather than the always-on ambient chrome underneath it.
    #[test]
    fn settings_window_root_carries_the_modal_windows_z_index() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(XindelerSettings::default());

        app.world_mut()
            .run_system_once(spawn_settings_window)
            .expect("spawn_settings_window runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<SettingsWindowRoot>>()
            .single(world)
            .expect("SettingsWindowRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
    }

    /// One pane per tab is spawned.
    #[test]
    fn spawns_one_pane_per_tab() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(XindelerSettings::default());

        app.world_mut()
            .run_system_once(spawn_settings_window)
            .expect("spawn runs");

        let world = app.world_mut();
        let pane_count = world.query::<&SettingsTabPane>().iter(world).count();
        assert_eq!(pane_count, SettingsTab::ALL.len());
    }

    /// Editing a preset-owned graphics toggle flips the tier to `Custom` so a
    /// preset can't clobber it on reload; editing the tier itself re-applies
    /// its preset values.
    #[test]
    fn editing_a_graphics_toggle_forces_custom_tier() {
        let mut settings = XindelerSettings::default();
        assert_eq!(settings.graphics.tier, GraphicsTier::Ultra);
        adjust(SettingControl::Ssao, 1, &mut settings);
        assert_eq!(
            settings.graphics.tier,
            GraphicsTier::Custom,
            "flipping SSAO must switch the tier to Custom"
        );
        assert!(
            !settings.graphics.ssao,
            "SSAO flipped off from the Ultra default"
        );

        // Cycle the tier onward to a preset (Custom -> Low) — it re-applies
        // the Low preset, so SSAO comes back off (Low disables it) and the
        // tier is the concrete preset, not Custom.
        adjust(SettingControl::Tier, 1, &mut settings);
        assert_eq!(settings.graphics.tier, GraphicsTier::Low);
        assert!(!settings.graphics.ssao, "Low preset disables SSAO");
        assert_eq!(settings.graphics.shadow_cascades, 1);
    }

    /// Vignette is tier-independent — editing it must NOT force Custom.
    #[test]
    fn editing_vignette_keeps_the_tier() {
        let mut settings = XindelerSettings::default();
        adjust(SettingControl::Vignette, 1, &mut settings);
        assert_eq!(settings.graphics.tier, GraphicsTier::Ultra);
        assert!(!settings.graphics.vignette);
    }

    /// BL-82 EM-5.16 (T56.43): the two Accessibility toggles flip their real
    /// `XindelerSettings.accessibility` fields (not a graphics setting, so
    /// they must NOT touch the graphics tier either).
    #[test]
    fn accessibility_toggles_flip_their_real_fields() {
        let mut settings = XindelerSettings::default();
        assert!(!settings.accessibility.reduce_flashing);
        assert!(!settings.accessibility.high_contrast_ui);

        adjust(SettingControl::ReduceFlashing, 1, &mut settings);
        assert!(settings.accessibility.reduce_flashing);
        assert_eq!(
            settings.graphics.tier,
            GraphicsTier::Ultra,
            "an accessibility toggle must not touch the graphics tier"
        );

        adjust(SettingControl::HighContrastUi, 1, &mut settings);
        assert!(settings.accessibility.high_contrast_ui);

        // Cycling again flips each back off.
        adjust(SettingControl::ReduceFlashing, 1, &mut settings);
        adjust(SettingControl::HighContrastUi, 1, &mut settings);
        assert!(!settings.accessibility.reduce_flashing);
        assert!(!settings.accessibility.high_contrast_ui);
    }

    /// [`apply_accessibility_theme`] reconciles the live [`HudTheme`]:
    /// enabling `high_contrast_ui` brightens muted text to the same value as
    /// regular body text and makes the panel background fully opaque;
    /// disabling it reverts both to the plain default palette.
    #[test]
    fn high_contrast_toggle_reconciles_the_live_theme() {
        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        app.insert_resource(HudTheme::default());
        app.add_systems(Update, apply_accessibility_theme);

        app.update();
        let defaults = HudPalette::default();
        assert_eq!(
            app.world().resource::<HudTheme>().palette.text_muted,
            defaults.text_muted,
            "off by default — the plain muted text colour"
        );

        app.world_mut()
            .resource_mut::<XindelerSettings>()
            .accessibility
            .high_contrast_ui = true;
        app.update();
        let theme = app.world().resource::<HudTheme>();
        assert_eq!(
            theme.palette.text_muted, defaults.text,
            "high-contrast must brighten muted text to full-contrast body text"
        );
        assert_eq!(
            theme.palette.panel_bg.alpha(),
            1.0,
            "high-contrast must make the panel background fully opaque"
        );

        app.world_mut()
            .resource_mut::<XindelerSettings>()
            .accessibility
            .high_contrast_ui = false;
        app.update();
        assert_eq!(
            app.world().resource::<HudTheme>().palette,
            defaults,
            "turning high-contrast back off must revert the palette to the plain default"
        );
    }

    /// Numeric controls step and clamp inside their bounds.
    #[test]
    fn numeric_controls_step_and_clamp() {
        let mut settings = XindelerSettings {
            ui_scale: UI_SCALE_MAX,
            ..Default::default()
        };

        adjust(SettingControl::UiScale, 1, &mut settings);
        assert_eq!(
            settings.ui_scale, UI_SCALE_MAX,
            "UI scale clamps at the max"
        );
        adjust(SettingControl::UiScale, -1, &mut settings);
        assert!((settings.ui_scale - (UI_SCALE_MAX - UI_SCALE_STEP)).abs() < 1e-4);

        settings.chat.opacity = 0.0;
        adjust(SettingControl::ChatOpacity, -1, &mut settings);
        assert_eq!(settings.chat.opacity, 0.0, "opacity clamps at zero");

        settings.graphics.shadow_cascades = MAX_SHADOW_CASCADES;
        adjust(SettingControl::ShadowCascades, 1, &mut settings);
        assert_eq!(
            settings.graphics.shadow_cascades, MAX_SHADOW_CASCADES,
            "cascade count clamps at the max (no wrap — numeric, not cyclic)"
        );
    }

    /// The crosshair toggle flips the boolean; the sync system drives its
    /// live `Visibility`.
    #[test]
    fn crosshair_toggle_drives_visibility() {
        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        let crosshair = app
            .world_mut()
            .spawn((Crosshair, Visibility::Inherited))
            .id();
        app.add_systems(Update, sync_crosshair_visibility);

        // Default (show_crosshair = true) -> stays visible.
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(crosshair).unwrap(),
            Visibility::Inherited
        );

        app.world_mut()
            .resource_mut::<XindelerSettings>()
            .interface
            .show_crosshair = false;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(crosshair).unwrap(),
            Visibility::Hidden,
            "turning the crosshair off must hide the reticle live"
        );
    }

    /// [`apply_graphics_settings`] reconciles the live camera: enabling SSAO/
    /// TAA inserts the components; disabling removes them — the "applies live,
    /// no restart" acceptance bar (moved from the pre-EM-5.12 esc_menu).
    #[test]
    fn apply_reconciles_camera_components_to_settings() {
        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        let camera = app.world_mut().spawn(MainCamera).id();
        app.add_systems(Update, apply_graphics_settings);

        app.update();
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_some()
        );
        assert!(app.world().get::<TemporalAntiAliasing>(camera).is_some());

        {
            let mut settings = app.world_mut().resource_mut::<XindelerSettings>();
            settings.graphics.ssao = false;
            settings.graphics.taa = false;
        }
        app.update();
        assert!(
            app.world()
                .get::<ScreenSpaceAmbientOcclusion>(camera)
                .is_none()
        );
        assert!(app.world().get::<TemporalAntiAliasing>(camera).is_none());
    }

    /// Changing `shadow_cascades` must NOT touch the live sun's
    /// `CascadeShadowConfig` — re-inserting a different `num_cascades` on the
    /// running light aborts `bevy_light` (see [`apply_graphics_settings`]'s
    /// doc). The count applies at startup by `light::spawn_light_rig` instead.
    #[test]
    fn changing_shadow_cascades_does_not_mutate_the_live_sun() {
        use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder};

        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        let sun_config = CascadeShadowConfigBuilder {
            num_cascades: 1,
            maximum_distance: 500.0,
            ..Default::default()
        }
        .build();
        let bounds_before = sun_config.bounds.len();
        let sun = app.world_mut().spawn(sun_config).id();
        app.add_systems(Update, apply_graphics_settings);

        app.update();
        {
            let mut settings = app.world_mut().resource_mut::<XindelerSettings>();
            settings.graphics.shadow_cascades = 3;
        }
        app.update();

        let after = app
            .world()
            .get::<CascadeShadowConfig>(sun)
            .expect("sun still carries its cascade config");
        assert_eq!(
            after.bounds.len(),
            bounds_before,
            "apply_graphics_settings must not reconfigure the live sun's cascade count"
        );
    }
}
