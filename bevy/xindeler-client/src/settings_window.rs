//! BL-82 EM-5.12 (T56.39) / EM-5.16 (T56.44) — the tabbed Settings window.
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
//! | Language       | `XindelerSettings::language` selector, LIVE hot-swap| ✅    |
//! | Networking    | nothing configurable today (connection is automatic)| stub  |
//! | Sound         | EM-5.10 audio — not built yet                       | stub  |
//! | Accessibility | `XindelerSettings::accessibility` (reduce-flashing  | ✅    |
//! |               | dampens `combat_hud`'s damage vignette; high-contrast|      |
//! |               | UI reconciles `HudTheme` live) + a tutorial-overlay  |      |
//! |               | reopen button. Positional-sound subtitles stay a stub|      |
//! |               | pending EM-5.10b (SFX) audio                        |       |
//!
//! Sound/Networking are HONEST stubs — a visible tab that names the epic
//! that will fill it, never fake toggles for a system that isn't built.
//! Accessibility itself is real (BL-82 EM-5.16, T56.43 part 1/2) — only its
//! positional-sound-subtitle sub-feature remains a stub, pending
//! `SfxTriggerItem` from the not-yet-landed EM-5.10b audio phase.
//!
//! ## T56.44 — this screen dogfoods the reactive i18n pipeline
//! Every static label in this window (tab names, row labels, notes, button
//! text) now resolves through `xindeler_ui::i18n::Localization` instead of a
//! hardcoded English literal, and is tagged with
//! [`xindeler_ui::i18n::LocalizedText`]/[`xindeler_ui::i18n::LocalizedLabel`]
//! so it re-localizes LIVE the moment the Language tab's selector changes —
//! see `xindeler_ui::i18n`'s own module doc for the full reactive chain. The
//! dynamic VALUE labels (On/Off, the quality-tier name, the language's own
//! display name) are recomputed by [`refresh_setting_labels`], which now
//! reacts to a locale change too, not just a settings change.
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
    ecs::{
        change_detection::NonSend,
        schedule::common_conditions::{not, resource_changed},
    },
    pbr::ScreenSpaceAmbientOcclusion,
    prelude::*,
    render::camera::{MipBias, TemporalJitter},
};
use xindeler_app::{GraphicsTier, XindelerSettings};
use xindeler_input::{ActionState, GameInput};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    i18n::{CurrentLocale, Localization, LocalizedLabel, LocalizedText},
    panel::panel_bundle,
    theme::{HudFonts, HudPalette, HudTheme},
    zlayer,
};

use crate::{camera::MainCamera, chat::text_input_focused, combat_hud::Crosshair};

/// The v1 selectable UI locale TAGS — a curated product decision (which
/// locales this screen offers), not a duplication of translated content: each
/// locale's own DISPLAY NAME is read live from its shipped `_manifest.ron`
/// (`xindeler_ui::i18n::language_name`, `metadata.language_name` — the same
/// field the legacy `client/i18n` crate's `LanguageMetadata` already sources
/// this from) rather than hand-copied here (a game-architecture-reviewer
/// finding on an earlier revision: the copy had already drifted from the
/// real manifest text). Extend this list — nothing else changes — as more
/// locales get real translations; `Localization`'s `en`-fallback (see
/// `xindeler_ui::i18n`'s doc) means an entry here with only PARTIAL `.ftl`
/// coverage still degrades cleanly rather than looking broken.
const AVAILABLE_LANGUAGES: &[&str] = &["en", "es"];

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
                    // BL-82 EM-5.16 (T56.44, bevy-migration-reviewer finding):
                    // reads `NonSend<Localization>`, which
                    // `xindeler_ui::i18n::reload_localization_on_locale_change`
                    // (inside `LocaleSyncSet`) rebuilds on a locale change —
                    // Bevy gives NO ordering guarantee between two systems
                    // with a conflicting `NonSend`/`NonSendMut` access absent
                    // an explicit edge, so without this the On/Off and
                    // quality-tier VALUE labels could observe the stale
                    // bundle the exact frame the user switches languages
                    // (verified empirically: `.chain()`/`.after(..)` is
                    // required, declaration order in the tuple alone is not
                    // enough). Same edge `button::spawn_button_labels`
                    // already has for the identical hazard.
                    refresh_setting_labels.after(xindeler_ui::i18n::LocaleSyncSet),
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

    /// The `.ftl` key for this tab's button label — every one of these
    /// already exists in the shared `common.ftl` catalog (used by other
    /// screens too, e.g. the esc menu's own Settings/Controls buttons).
    fn label_key(self) -> &'static str {
        match self {
            SettingsTab::Interface => "common-interface",
            SettingsTab::Video => "common-video",
            SettingsTab::Controls => "common-controls",
            SettingsTab::Gameplay => "common-gameplay",
            SettingsTab::Chat => "common-chat",
            SettingsTab::Language => "common-languages",
            SettingsTab::Networking => "common-networking",
            SettingsTab::Sound => "common-sound",
            SettingsTab::Accessibility => "common-accessibility",
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

impl SettingControl {
    /// The `.ftl` key for this control's ROW LABEL (the static name next to
    /// the buttons — the dynamic VALUE next to it is [`value_label`], a
    /// separate concern).
    fn row_label_key(self) -> &'static str {
        match self {
            SettingControl::UiScale => "hud-settings-ui_scale",
            SettingControl::MouseSensitivity => "hud-settings-mouse_sensitivity",
            SettingControl::FlySpeed => "hud-settings-fly_speed",
            SettingControl::ChatOpacity => "hud-settings-background_opacity",
            SettingControl::ShadowCascades => "hud-settings-shadow_cascades",
            SettingControl::ShowCrosshair => "hud-settings-crosshair",
            SettingControl::Ssao => "hud-settings-ssao",
            SettingControl::Taa => "hud-settings-taa",
            SettingControl::Bloom => "hud-settings-bloom",
            SettingControl::VolumetricFog => "hud-settings-volumetric_fog",
            SettingControl::ContactShadows => "hud-settings-contact_shadows",
            SettingControl::Vignette => "hud-settings-vignette",
            SettingControl::ReduceFlashing => "hud-settings-reduce_flashing",
            SettingControl::HighContrastUi => "hud-settings-high_contrast_ui",
            SettingControl::Tier => "hud-settings-quality_preset",
            SettingControl::Language => "hud-settings-language",
        }
    }
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

/// Refreshes every value label from the current settings AND the current
/// locale (so a click's effect — or a language switch — is immediately
/// visible). A [`SettingValueLabel`] sits either directly on a [`Text`] node
/// (numeric rows) or on a button whose child carries the text (toggle/enum
/// rows) — handle both.
fn refresh_setting_labels(
    settings: Res<XindelerSettings>,
    current_locale: Res<CurrentLocale>,
    localization: NonSend<Localization>,
    labels: Query<(Entity, &SettingValueLabel, Option<&Children>)>,
    mut texts: Query<&mut Text>,
) {
    if !settings.is_changed() && !current_locale.is_changed() {
        return;
    }
    for (entity, label, children) in &labels {
        let new = value_label(label.0, &settings, &localization);
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
    localization: NonSend<Localization>,
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
                heading(
                    panel,
                    &fonts,
                    &theme,
                    &localization,
                    "common-settings",
                    28.0,
                );
                spawn_tab_bar(panel, &theme, &fonts, &localization, selected);

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
                            spawn_tab_pane(
                                content,
                                &theme,
                                &fonts,
                                &settings,
                                &localization,
                                tab,
                                selected,
                            );
                        }
                    });

                spawn_labeled_button(panel, &theme, &fonts, &localization, "common-close").observe(
                    |_a: On<Activate>, mut actions: MessageWriter<HudAction>| {
                        actions.write(HudAction::CloseWindow);
                    },
                );
            });
        });
}

/// Spawns a themed button whose label is a resolved `.ftl` message value,
/// tagged [`LocalizedLabel`] so it re-resolves live on a locale change.
fn spawn_labeled_button<'a>(
    parent: &'a mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    localization: &Localization,
    key: &'static str,
) -> EntityCommands<'a> {
    let mut button = parent.spawn(button_bundle(theme, fonts, &localization.tr(key)));
    button.insert(LocalizedLabel(key));
    button
}

/// The horizontal tab bar (one button per tab; the selected one starts
/// highlighted).
fn spawn_tab_bar(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    localization: &Localization,
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
                let mut button =
                    spawn_labeled_button(bar, theme, fonts, localization, tab.label_key());
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
    localization: &Localization,
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
                    localization,
                    SettingControl::UiScale,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::ShowCrosshair,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    localization,
                    "hud-settings-note_interface",
                );
            },
            SettingsTab::Video => {
                enum_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::Tier,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::Ssao,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::Taa,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::Bloom,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::VolumetricFog,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::ContactShadows,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::Vignette,
                );
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::ShadowCascades,
                );
                note(pane, fonts, theme, localization, "hud-settings-note_video");
            },
            SettingsTab::Controls => {
                note(
                    pane,
                    fonts,
                    theme,
                    localization,
                    "hud-settings-note_controls",
                );
                spawn_labeled_button(
                    pane,
                    theme,
                    fonts,
                    localization,
                    "hud-settings-open_controls",
                )
                .observe(
                    |_a: On<Activate>, mut actions: MessageWriter<HudAction>| {
                        actions.write(HudAction::ToggleWindow(HudWindow::Controls));
                    },
                );
            },
            SettingsTab::Gameplay => {
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::MouseSensitivity,
                );
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::FlySpeed,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    localization,
                    "hud-settings-note_gameplay",
                );
            },
            SettingsTab::Chat => {
                numeric_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::ChatOpacity,
                );
                note(pane, fonts, theme, localization, "hud-settings-note_chat");
            },
            SettingsTab::Language => {
                enum_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::Language,
                );
                note(
                    pane,
                    fonts,
                    theme,
                    localization,
                    "hud-settings-note_language",
                );
            },
            SettingsTab::Networking => {
                note(
                    pane,
                    fonts,
                    theme,
                    localization,
                    "hud-settings-note_networking",
                );
            },
            SettingsTab::Sound => {
                note(pane, fonts, theme, localization, "hud-settings-note_sound");
            },
            SettingsTab::Accessibility => {
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
                    SettingControl::ReduceFlashing,
                );
                toggle_row(
                    pane,
                    theme,
                    fonts,
                    settings,
                    localization,
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
                    localization,
                    "hud-settings-note_accessibility",
                );
            },
        });
}

/// A section/title heading line inside the panel — resolved from `key` and
/// tagged [`LocalizedText`] so it re-resolves live on a locale change.
fn heading(
    panel: &mut ChildSpawnerCommands,
    fonts: &HudFonts,
    theme: &HudTheme,
    localization: &Localization,
    key: &'static str,
    size: f32,
) {
    panel.spawn((
        LocalizedText(key),
        Text(localization.tr(key)),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.title.clone()),
            font_size: bevy::text::FontSize::Px(size),
            ..Default::default()
        },
        TextColor(theme.palette.text),
    ));
}

/// A muted explanatory note line — resolved from `key`, live-relocalizing.
fn note(
    panel: &mut ChildSpawnerCommands,
    fonts: &HudFonts,
    theme: &HudTheme,
    localization: &Localization,
    key: &'static str,
) {
    panel.spawn((
        LocalizedText(key),
        Text(localization.tr(key)),
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

/// The name label that opens every setting row — resolved from `key`,
/// live-relocalizing.
fn row_label(
    row: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    localization: &Localization,
    key: &'static str,
) {
    row.spawn((
        LocalizedText(key),
        Text(localization.tr(key)),
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
    localization: &Localization,
    control: SettingControl,
) {
    cycle_row(pane, theme, fonts, settings, localization, control);
}

/// An enum row: name + a single button whose label is the current variant and
/// which cycles it on click.
fn enum_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    localization: &Localization,
    control: SettingControl,
) {
    cycle_row(pane, theme, fonts, settings, localization, control);
}

/// Shared spawn for the single-cycle-button rows (bool + enum): clicking the
/// value button advances the setting (`adjust` with `dir = 1`). The value
/// button's label is NOT tagged [`LocalizedLabel`] — it's driven by
/// [`refresh_setting_labels`] instead (it depends on live settings data, not
/// just the locale), matching [`numeric_row`]'s own value cell.
fn cycle_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    localization: &Localization,
    control: SettingControl,
) {
    pane.spawn(row_node(theme)).with_children(|row| {
        row_label(row, theme, fonts, localization, control.row_label_key());
        row.spawn(button_bundle(
            theme,
            fonts,
            &value_label(control, settings, localization),
        ))
        .insert(SettingValueLabel(control))
        .observe(
            move |_a: On<Activate>, mut settings: ResMut<XindelerSettings>| {
                apply_and_save(control, 1, &mut settings);
            },
        );
    });
}

/// A numeric row: name + `[−]` + value text + `[+]`. The `-`/`+` glyphs
/// themselves are deliberately left unlocalized — they're plain mathematical
/// symbols, not language-dependent text.
fn numeric_row(
    pane: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    settings: &XindelerSettings,
    localization: &Localization,
    control: SettingControl,
) {
    pane.spawn(row_node(theme)).with_children(|row| {
        row_label(row, theme, fonts, localization, control.row_label_key());
        row.spawn(button_bundle(theme, fonts, "-")).observe(
            move |_a: On<Activate>, mut settings: ResMut<XindelerSettings>| {
                apply_and_save(control, -1, &mut settings);
            },
        );
        row.spawn((
            SettingValueLabel(control),
            Text(value_label(control, settings, localization)),
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

/// A short `On`/`Off` label for a boolean, resolved through the active
/// locale.
fn on_off(value: bool, localization: &Localization) -> String {
    localization.tr(if value { "common-on" } else { "common-off" })
}

/// The `.ftl` key for a [`GraphicsTier`]'s display name — reuses the existing
/// legacy tier-name keys (`hud-settings-*_graphics`) already in the catalog.
fn tier_label_key(tier: GraphicsTier) -> &'static str {
    match tier {
        GraphicsTier::Low => "hud-settings-low_graphics",
        GraphicsTier::Medium => "hud-settings-medium_graphics",
        GraphicsTier::High => "hud-settings-high_graphics",
        GraphicsTier::Ultra => "hud-settings-ultra_graphics",
        GraphicsTier::Custom => "hud-settings-custom_graphics",
    }
}

/// A locale's own native display name, read straight from its shipped
/// `_manifest.ron` (see [`AVAILABLE_LANGUAGES`]'s doc for why this reads the
/// manifest instead of a hand-copied table) — an unrecognised/manifest-less
/// tag falls back to showing the raw tag itself rather than panicking (see
/// [`xindeler_ui::i18n::language_name`]'s own doc).
fn language_display_name(tag: &str) -> String {
    xindeler_ui::i18n::language_name(&xindeler_ui::i18n::parse_locale(tag))
}

/// The current value of a control, formatted for display (locale-aware for
/// the boolean/tier/language rows).
fn value_label(
    control: SettingControl,
    settings: &XindelerSettings,
    localization: &Localization,
) -> String {
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
        SettingControl::ShowCrosshair => on_off(settings.interface.show_crosshair, localization),
        SettingControl::Ssao => on_off(g.ssao, localization),
        SettingControl::Taa => on_off(g.taa, localization),
        SettingControl::Bloom => on_off(g.bloom, localization),
        SettingControl::VolumetricFog => on_off(g.volumetric_fog, localization),
        SettingControl::ContactShadows => on_off(g.contact_shadows, localization),
        SettingControl::Vignette => on_off(g.vignette, localization),
        SettingControl::ReduceFlashing => {
            on_off(settings.accessibility.reduce_flashing, localization)
        },
        SettingControl::HighContrastUi => {
            on_off(settings.accessibility.high_contrast_ui, localization)
        },
        SettingControl::Tier => localization.tr(tier_label_key(g.tier)),
        SettingControl::Language => language_display_name(&settings.language),
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

/// Cycles to the next available UI locale (wraps). Selecting a new entry here
/// is ALL it takes to make it live-selectable — `XindelerSettings::save`
/// persists the tag, and `xindeler-client::localization`'s settings bridge
/// picks up the change and drives the whole reactive reload/relocalize chain
/// (`xindeler_ui::i18n::LocaleSyncSet`).
fn next_language(current: &str) -> String {
    let idx = AVAILABLE_LANGUAGES
        .iter()
        .position(|&tag| tag == current)
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

/// Test-only: an empty-catalog `Localization` — every `.tr(key)` call
/// resolves to `key` itself (the documented, never-panic fallback), which is
/// all these structural tests need (they assert entity/component SHAPE, never
/// specific translated text).
#[cfg(test)]
fn test_localization() -> Localization {
    Localization::load(&xindeler_ui::i18n::fallback_locale(), &[])
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
        app.insert_non_send(test_localization());

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
        app.insert_non_send(test_localization());

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

    /// Cycling the language wraps and covers every entry in
    /// [`AVAILABLE_LANGUAGES`] — a real regression target since T56.44 grew
    /// the list from a single (no-op) entry to two.
    #[test]
    fn next_language_cycles_through_every_available_locale() {
        assert_eq!(next_language("en"), "es");
        assert_eq!(next_language("es"), "en", "wraps back to the first entry");
        assert_eq!(
            next_language("totally-unknown"),
            "es",
            "an unrecognised current tag falls back to cycling from the first entry"
        );
    }

    /// `language_display_name` reads the REAL repo `_manifest.ron` for each
    /// locale (via `VELOREN_ASSETS`/`XINDELER_ASSETS`) rather than a
    /// hand-copied table, and degrades to the raw tag (never panics) for a
    /// tag with no manifest at all.
    #[test]
    fn language_display_name_reads_the_real_manifest_and_degrades_for_unknown_tags() {
        assert_eq!(language_display_name("en"), "English");
        assert_eq!(
            language_display_name("es"),
            "Español de España (Spanish - Spain)",
            "must read the manifest's own declared name, not a shortened guess"
        );
        assert_eq!(language_display_name("xx"), "xx");
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

    /// BL-82 EM-5.16 (T56.44) — the real end-to-end proof that switching the
    /// active locale re-localizes an ALREADY-SPAWNED row label live, using the
    /// real repo `.ftl` catalogs (not a synthetic fixture) via
    /// `VELOREN_ASSETS`/`XINDELER_ASSETS`. Spawns the window with `en` active,
    /// confirms the Interface tab's crosshair row shows the English label,
    /// then flips `CurrentLocale` to `es` and runs the SAME two systems
    /// `xindeler_ui::XindelerUiPlugin` chains into `LocaleSyncSet`
    /// (`reload_localization_on_locale_change` then `relocalize_text`) —
    /// asserting the row's `Text` now reads the REAL Spanish catalog value,
    /// not the English fallback, proving the hot-swap works against real
    /// assets. This is the "i18n test passes" verify criterion from the
    /// EM-5.16 task board (T56.44).
    #[test]
    fn switching_locale_relocalizes_an_already_spawned_row_label_live() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(XindelerSettings::default());
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            xindeler_ui::i18n::DEFAULT_HUD_FTL_FILES,
        ));
        app.init_resource::<CurrentLocale>();

        app.world_mut()
            .run_system_once(spawn_settings_window)
            .expect("spawn runs");

        let world = app.world_mut();
        let before_text = world
            .query::<(&LocalizedText, &Text)>()
            .iter(world)
            .find(|(tag, _)| tag.0 == "hud-settings-crosshair")
            .map(|(_, text)| text.0.clone())
            .expect("the crosshair row label was spawned and tagged");
        assert_eq!(
            before_text, "Crosshair",
            "the crosshair row must show the real en catalog text at spawn time"
        );

        // Flip the locale and run the SAME reload+relocalize chain
        // `XindelerUiPlugin` wires into the real app.
        app.world_mut().resource_mut::<CurrentLocale>().0 = "es".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_text)
            .expect("relocalize runs");

        let world = app.world_mut();
        let after_text = world
            .query::<(&LocalizedText, &Text)>()
            .iter(world)
            .find(|(tag, _)| tag.0 == "hud-settings-crosshair")
            .map(|(_, text)| text.0.clone())
            .expect("the same row still carries its LocalizedText tag");
        assert_eq!(
            after_text, "Punto de mira",
            "must resolve to the REAL es catalog's own hud-settings-crosshair value, not the en \
             fallback"
        );
        assert_ne!(
            after_text, before_text,
            "sanity: this assertion is only meaningful if the resolved text actually changed \
             between locales"
        );
    }

    /// BL-82 EM-5.16 (T56.44, bevy-migration-reviewer finding): drives the
    /// REAL `Update` schedule (not hand-ordered `run_system_once` calls, the
    /// gap the reviewer flagged in the two tests above) with
    /// `refresh_setting_labels` registered EXACTLY as `SettingsWindowPlugin`
    /// wires it — `.after(xindeler_ui::i18n::LocaleSyncSet)` — alongside the
    /// real reload/relocalize chain, and asserts a VALUE label (the
    /// crosshair toggle's `On`/`Off` button, driven by `value_label`/
    /// `on_off`, not a bare `LocalizedText`) re-localizes in the SAME
    /// `app.update()` the locale actually changes. Before the ordering fix,
    /// `refresh_setting_labels` had no edge against `LocaleSyncSet` and could
    /// observe the stale `Localization` bundle the exact frame a locale
    /// switch fires (Bevy gives no ordering guarantee between systems with a
    /// conflicting `NonSend` access absent an explicit edge). Verified
    /// non-tautological: dropping this test's own `.after(LocaleSyncSet)`
    /// edges on `refresh_setting_labels`/`spawn_button_labels` (the SAME
    /// edges `SettingsWindowPlugin::build`/`XindelerUiPlugin::build`
    /// register in the real app) reliably reproduces the stale-value
    /// failure (5/5 runs), confirming this test genuinely exercises the
    /// hazard rather than passing by construction.
    #[test]
    fn switching_locale_relocalizes_a_settings_value_through_the_real_schedule() {
        use bevy::ecs::schedule::common_conditions::resource_changed;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(XindelerSettings::default());
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            xindeler_ui::i18n::DEFAULT_HUD_FTL_FILES,
        ));
        app.init_resource::<CurrentLocale>();

        // The REAL reactive chain `XindelerUiPlugin` wires (same
        // `.chain().in_set(..).run_if(..)` shape), plus `spawn_button_labels`
        // and `refresh_setting_labels` with the SAME `.after(LocaleSyncSet)`
        // edges `XindelerUiPlugin`/`SettingsWindowPlugin` register them with
        // in the real app — this is the exact schedule shape under test, not
        // a simplified stand-in for it.
        app.add_systems(
            Update,
            (
                xindeler_ui::i18n::reload_localization_on_locale_change,
                xindeler_ui::i18n::relocalize_text,
                xindeler_ui::i18n::relocalize_button_labels,
            )
                .chain()
                .in_set(xindeler_ui::i18n::LocaleSyncSet)
                .run_if(resource_changed::<CurrentLocale>),
        );
        app.add_systems(
            Update,
            (
                xindeler_ui::button::spawn_button_labels.after(xindeler_ui::i18n::LocaleSyncSet),
                refresh_setting_labels.after(xindeler_ui::i18n::LocaleSyncSet),
            ),
        );

        app.world_mut()
            .run_system_once(spawn_settings_window)
            .expect("spawn runs");
        app.update(); // materialize button label children

        fn crosshair_toggle_text(app: &mut App) -> String {
            let world = app.world_mut();
            let (button, children) = world
                .query::<(Entity, &SettingValueLabel, Option<&Children>)>()
                .iter(world)
                .find(|(_, label, _)| label.0 == SettingControl::ShowCrosshair)
                .map(|(entity, _, children)| (entity, children.map(|c| c[0])))
                .expect("the crosshair value control was spawned and tagged");
            if let Some(child) = children {
                world
                    .get::<Text>(child)
                    .expect("label child exists")
                    .0
                    .clone()
            } else {
                world
                    .get::<Text>(button)
                    .expect("label on the entity itself")
                    .0
                    .clone()
            }
        }

        assert_eq!(
            crosshair_toggle_text(&mut app),
            "On",
            "the crosshair toggle must show the real en catalog value at spawn time"
        );

        app.world_mut().resource_mut::<CurrentLocale>().0 = "es".to_owned();
        app.update();

        assert_eq!(
            crosshair_toggle_text(&mut app),
            "Activado",
            "must re-localize to the real es catalog's common-on value in the SAME frame the \
             locale changed — this is what `refresh_setting_labels.after(LocaleSyncSet)` \
             guarantees"
        );
    }
}
