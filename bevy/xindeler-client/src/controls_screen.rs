//! BL-82 EM-5.11 (T56.11–T56.13) — the input-rebinding screen.
//!
//! A `bevy_ui` screen built on the EM-5.1 widget kit (`Panel`/`Button`/
//! `HudTheme`/`HudFonts`) — the first consumer of that kit beyond EM-5.2's
//! combat HUD. Lists a curated set of core actions (not all ~90
//! [`GameInput`]s — see [`CURATED_ACTIONS`]'s doc comment for why), each with
//! TWO rebind buttons (keyboard/mouse + gamepad) side by side, so keyboard
//! rebinding and full gamepad support (§Q5=A) live in the SAME screen rather
//! than a hidden second tab. Clicking a rebind button arms
//! [`xindeler_input::RebindRequest`]; the next physical input
//! ([`xindeler_input::capture::capture_rebind`], already running every frame
//! via `XindelerInputPlugin`) resolves it, and this module's own
//! [`persist_and_track_conflicts`] system reacts to the
//! [`xindeler_input::RebindOutcome`] it fires: it writes the new
//! [`xindeler_input::KeyMap`] back into [`XindelerSettings::controls`] and
//! saves `settings.ron` immediately (the "rebind → restart → persists"
//! acceptance bar), and tracks any conflict for that row's warning label.
//!
//! Toggled by the `Controls` [`GameInput`] itself (F1 by default) — reading
//! it through [`xindeler_input::ActionState`] like any other action means
//! rebinding the Controls action away from F1 immediately changes which key
//! opens this very screen, proving the whole pipeline end-to-end.
//!
//! Compiled only under `listen-server`/`net-client` (the only modes that add
//! [`xindeler_ui::XindelerUiPlugin`] today via [`crate::combat_hud::
//! CombatHudViewPlugin`] — same posture as every other screen module in this
//! crate).
//!
//! ## BL-82 EM-5.16 (T56.44 follow-up) — full i18n
//! The heading and every [`CURATED_ACTIONS`] row label now resolve through
//! [`Localization`] and are tagged [`LocalizedText`], so this screen
//! re-localizes live on a locale change — see `settings_window.rs`'s own doc
//! comment for the full reactive chain. Row labels use
//! [`GameInput::ftl_key`], resolving the SAME `gameinput-*` keys
//! `gameinput.ftl` already carries for every variant (ported verbatim from
//! legacy `voxygen`) — replacing the old `display_name()` camelCase-splitting
//! heuristic this module used to fall back on. The "awaiting key"/"unbound"
//! button states reuse the pre-existing `hud-settings-awaitingkey`/
//! `hud-settings-unbound` keys (legacy `hud/settings.ftl` — not currently
//! resolved by `settings_window.rs` itself, this screen is their one real
//! consumer today); the per-row conflict warning uses a new
//! `hud-controls-conflicts_with` key (`hud/controls.ftl`).

use std::collections::HashMap;

use bevy::{ecs::change_detection::NonSend, prelude::*};
use xindeler_app::XindelerSettings;
use xindeler_input::{
    ActionState, GameInput, GamepadBinding, KeyMap, KeyOrMouse, RebindOutcome, RebindRequest,
    RebindTarget,
};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    i18n::{CurrentLocale, Localization, LocalizedLabel, LocalizedText},
    panel::panel_bundle,
    theme::{HudFonts, HudTheme},
    zlayer,
};

/// The curated action set this v1 screen surfaces, grouped for the layout.
/// NOT all ~90 [`GameInput`]s — a from-scratch flat list of ninety rebind
/// rows would be a wall of near-identical UI for a v1 screen with no
/// search/filter/scroll-view yet (`xindeler-ui`'s own doc comment defers
/// List/Grid/ScrollView to whichever screen needs them first). This is a
/// deliberate, documented v1 scope cut (same posture as EM-5.1/5.2's own
/// partial rows) covering the actions a new player actually rebinds most:
/// movement, core combat, and the main window toggles. Every OTHER
/// [`GameInput`] still has a real default binding and is fully rebindable
/// via [`KeyMap`] directly (e.g. a future debug/console path, or EM-5.12's
/// fuller settings tab) — this screen is a curated view onto it, not the
/// only way to change it.
const CURATED_ACTIONS: &[GameInput] = &[
    GameInput::MoveForward,
    GameInput::MoveBack,
    GameInput::MoveLeft,
    GameInput::MoveRight,
    GameInput::Jump,
    GameInput::Sneak,
    GameInput::Roll,
    GameInput::Primary,
    GameInput::Secondary,
    GameInput::Block,
    GameInput::Interact,
    GameInput::ToggleWield,
    GameInput::Inventory,
    GameInput::Map,
    GameInput::Diary,
    GameInput::Social,
    GameInput::Crafting,
    GameInput::Chat,
    GameInput::Settings,
    GameInput::Controls,
    GameInput::Escape,
];

/// Installs the controls screen: spawns the (initially hidden) window at
/// `Startup`, keeps its visibility synced to [`HudState`], refreshes binding
/// labels every frame, and reacts to rebind outcomes (persist + conflict
/// tracking).
pub struct ControlsScreenPlugin;

impl Plugin for ControlsScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RebindConflicts>()
            .add_systems(
                Startup,
                spawn_controls_screen.after(xindeler_ui::theme::init_theme),
            )
            .add_systems(
                Update,
                (
                    // `toggle_controls_screen` reads `ActionState` — order
                    // after the frame's real input resolution (reviewer
                    // finding: this system had no ordering constraint at
                    // all, unlike `camera::FlyCamSet`/`player_input::
                    // gather_input`, which both explicitly depend on it).
                    toggle_controls_screen.after(xindeler_input::InputResolveSet),
                    sync_window_visibility,
                    // BL-82 EM-5.16 (T56.44 follow-up): reads `NonSend<
                    // Localization>`, rebuilt by `reload_localization_on_
                    // locale_change` inside `LocaleSyncSet` — same explicit
                    // ordering edge `settings_window.rs`'s own
                    // `refresh_setting_labels` documents needing (Bevy gives
                    // no ordering guarantee between two systems with
                    // conflicting `NonSend`/`NonSendMut` access absent one).
                    refresh_binding_labels.after(xindeler_ui::i18n::LocaleSyncSet),
                    persist_and_track_conflicts,
                ),
            );
    }
}

/// Per-action conflicts reported by the most recent rebind of that action
/// (cleared once a later rebind of the SAME action resolves cleanly).
/// Client-local UI state, not persisted — a conflict is a live warning about
/// the CURRENT keymap, re-derivable from it, not save-worthy data itself.
#[derive(Resource, Default)]
struct RebindConflicts(HashMap<GameInput, Vec<GameInput>>);

#[derive(Component)]
struct ControlsScreenRoot;

/// Tags a rebind button: which action + which device column it edits.
#[derive(Component, Clone, Copy)]
struct BindingButton {
    input: GameInput,
    device: Device,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Device {
    Keyboard,
    Gamepad,
}

/// Tags the small warning-text child under a row, naming which action's
/// conflicts it displays.
#[derive(Component)]
struct ConflictLabel(GameInput);

/// F1 (or whatever [`GameInput::Controls`] is currently bound to — this
/// reads it through [`ActionState`], not a hardcoded `KeyCode`) toggles the
/// screen open/closed, same convention every other HUD window uses
/// ([`HudState::toggle`]).
fn toggle_controls_screen(action_state: Res<ActionState>, mut actions: MessageWriter<HudAction>) {
    if action_state.just_pressed(GameInput::Controls) {
        actions.write(HudAction::ToggleWindow(HudWindow::Controls));
    }
}

/// Syncs this screen's root [`Visibility`] to
/// `HudState::is_open(HudWindow::Controls)`.
///
/// Read-only with respect to [`HudAction`] — [`xindeler_ui::hud_state::
/// apply_hud_actions`] is the ONE place [`HudAction::ToggleWindow`]/
/// `CloseWindow` gets applied to [`HudState`]. This function used to ALSO
/// drain a `MessageReader<HudAction>` and call `hud_state.toggle(*window)`
/// itself (BL-82 EM-5.17 Phase 0 root cause): since every `MessageReader`
/// keeps its own independent read cursor, both systems consumed the SAME
/// `ToggleWindow` message every frame, so a single keypress toggled the
/// window on then immediately back off again — a silent double-apply that
/// broke every `HudAction`-routed window (Diary/Map/Controls).
fn sync_window_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<ControlsScreenRoot>>,
) {
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if hud_state.is_open(HudWindow::Controls) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Spawns the whole screen: a centred panel with one row per
/// [`CURATED_ACTIONS`] entry, each with a name label, a keyboard/mouse rebind
/// button, a gamepad rebind button, and an (initially empty) conflict
/// warning label.
///
/// BL-82 EM-5.17/5.18 click-routing fix follow-up: `ControlsScreenRoot` is a
/// full-screen modal backdrop structurally identical to `DiaryWindowRoot`/
/// `InventoryWindowRoot`/`EscMenuRoot`/`FullMapRoot` (`Visibility::Hidden`,
/// `PositionType::Absolute` at 100%x100%, mutually exclusive with those via
/// `HudState`'s single `open_window` slot) but was missed by that same pass
/// — it too was spawned without `GlobalZIndex(zlayer::MODAL_WINDOWS)`, so it
/// sat at the default z-partition (0), BELOW the always-on ambient chrome
/// (hotbar/orbs = `ORBS_ACTION_BAR_PARTY_MINIMAP`=20): wherever the Controls
/// screen visually overlapped that chrome, `bevy_ui` picking (highest
/// z-partition first) routed clicks to the chrome in front instead of the
/// Controls panel underneath.
fn spawn_controls_screen(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    keymap: Res<KeyMap>,
    localization: NonSend<Localization>,
) {
    // `HudTheme` is `Copy` — an owned value here (rather than the `Res`
    // borrow) is what lets `and_modify`'s closure below capture it `move`
    // without a `'static` lifetime error.
    let theme: HudTheme = *theme;
    commands
        .spawn((
            ControlsScreenRoot,
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
            // BL-82 EM-5.2 lesson (see that epic's post-ship follow-up fix):
            // `Node` is a SINGLE struct component — a second `insert(Node
            // {..})` after `panel_bundle`'s own spawn would REPLACE the
            // whole thing, silently discarding its padding/border/radius.
            // `entry::<Node>().and_modify(..)` mutates the EXISTING Node's
            // fields in place instead.
            // `and_modify`'s closure must be `'static`/`Send`/`Sync` (it may
            // run as a deferred command) — capture the one value it needs as
            // a plain `f32` `move`, not a borrow of `theme`.
            let row_gap_px = theme.spacing.xs;
            panel_entity.entry::<Node>().and_modify(move |mut node| {
                node.flex_direction = FlexDirection::Column;
                node.row_gap = Val::Px(row_gap_px);
                node.max_height = Val::Percent(85.0);
                node.overflow = Overflow::clip_y();
            });
            panel_entity.with_children(|panel| {
                // BL-82 EM-5.16 (T56.44): resolved through the active locale
                // and tagged `LocalizedText` so it re-localizes live; reuses
                // `common-controls` (the same key the settings window's own
                // Controls tab button and the esc menu's Controls button
                // resolve), not a new one.
                panel.spawn((
                    LocalizedText("common-controls"),
                    Text(localization.tr("common-controls")),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.title.clone()),
                        font_size: bevy::text::FontSize::Px(28.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
                // This screen is opened as its own mutually-exclusive
                // `HudWindow` (via `settings_window.rs`'s Controls tab OR its
                // own `GameInput::Controls` hotkey), so once here there was
                // previously no way back to the Settings window short of
                // Escape (which closes everything, not just this one). Since
                // `HudState::toggle` is a single-slot swap (see its own doc
                // comment), simply toggling to `Settings` both opens it AND
                // implicitly closes this Controls screen — no explicit close
                // needed first.
                panel
                    .spawn(button_bundle(
                        &theme,
                        &fonts,
                        &localization.tr("common-back"),
                    ))
                    .insert(LocalizedLabel("common-back"))
                    .observe(
                        |_activate: On<Activate>, mut actions: MessageWriter<HudAction>| {
                            actions.write(HudAction::ToggleWindow(HudWindow::Settings));
                        },
                    );
                for &input in CURATED_ACTIONS {
                    spawn_row(panel, &theme, &fonts, &keymap, &localization, input);
                }
            });
        });
}

fn spawn_row(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    keymap: &KeyMap,
    localization: &Localization,
    input: GameInput,
) {
    panel
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(theme.spacing.sm),
            align_items: AlignItems::Center,
            ..Default::default()
        })
        .with_children(|row| {
            // BL-82 EM-5.16 (T56.44 follow-up): resolves the REAL per-action
            // `gameinput-*` key (`GameInput::ftl_key`) — `gameinput.ftl`
            // already carries one for every variant, ported verbatim from
            // legacy `voxygen`. Tagged `LocalizedText` so it re-localizes
            // live, replacing the old `display_name()` camelCase-splitting
            // heuristic.
            row.spawn((
                LocalizedText(input.ftl_key()),
                Text(localization.tr(input.ftl_key())),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
                Node {
                    width: Val::Px(160.0),
                    ..Default::default()
                },
            ));

            let keyboard_label = keymap
                .keyboard
                .get_binding(input)
                .map_or_else(|| localization.tr("hud-settings-unbound"), key_label);
            row.spawn(button_bundle(theme, fonts, &keyboard_label))
                .insert(BindingButton {
                    input,
                    device: Device::Keyboard,
                })
                .observe(
                    move |_activate: On<Activate>, mut request: ResMut<RebindRequest>| {
                        request.0 = Some(RebindTarget::Keyboard(input));
                    },
                );

            let gamepad_label = keymap.gamepad.get_button_binding(input).map_or_else(
                || localization.tr("hud-settings-unbound"),
                gamepad_binding_label,
            );
            row.spawn(button_bundle(theme, fonts, &gamepad_label))
                .insert(BindingButton {
                    input,
                    device: Device::Gamepad,
                })
                .observe(
                    move |_activate: On<Activate>, mut request: ResMut<RebindRequest>| {
                        request.0 = Some(RebindTarget::Gamepad(input));
                    },
                );

            row.spawn((
                ConflictLabel(input),
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(13.0),
                    ..Default::default()
                },
                TextColor(theme.palette.danger),
            ));
        });
}

/// Recomputes each rebind button's label from the CURRENT [`KeyMap`] (showing
/// the locale's "awaiting key" text while its own request is the pending one)
/// and each row's conflict warning from [`RebindConflicts`]. Gated on
/// `is_changed()` across every resource it reads (including [`CurrentLocale`]
/// — see below) rather than the unconditional every-frame walk this function
/// used pre-T56.44; cheap either way at this screen's row count
/// (`CURATED_ACTIONS.len()` × 2 buttons), but the locale-change requirement
/// below needed a real change source to react to.
///
/// BL-82 EM-5.16 (T56.44 follow-up): also reruns on a locale change (ordered
/// `.after(LocaleSyncSet)`, see the plugin registration) so the "awaiting
/// key"/"unbound"/"conflicts with" text re-resolves live, not just on the next
/// keymap edit — it reuses the pre-existing `hud-settings-awaitingkey`/
/// `hud-settings-unbound` keys (legacy `hud/settings.ftl`; this screen is
/// their one real consumer today — see the module doc comment) rather than
/// inventing near-duplicates.
fn refresh_binding_labels(
    keymap: Res<KeyMap>,
    request: Res<RebindRequest>,
    conflicts: Res<RebindConflicts>,
    current_locale: Res<CurrentLocale>,
    localization: NonSend<Localization>,
    buttons: Query<(&BindingButton, &Children)>,
    mut conflict_labels: Query<(&ConflictLabel, &mut Text), Without<BindingButton>>,
    // Disjoint from `conflict_labels` via `Without<ConflictLabel>` — both
    // queries would otherwise alias `&mut Text` on the same entities and
    // panic at schedule build time.
    mut texts: Query<&mut Text, Without<ConflictLabel>>,
) {
    if !keymap.is_changed()
        && !request.is_changed()
        && !conflicts.is_changed()
        && !current_locale.is_changed()
    {
        return;
    }
    for (binding, children) in &buttons {
        let pending = matches!(
            (request.0, binding.device),
            (Some(RebindTarget::Keyboard(i)), Device::Keyboard) if i == binding.input
        ) || matches!(
            (request.0, binding.device),
            (Some(RebindTarget::Gamepad(i)), Device::Gamepad) if i == binding.input
        );
        let label = if pending {
            localization.tr("hud-settings-awaitingkey")
        } else {
            match binding.device {
                Device::Keyboard => keymap
                    .keyboard
                    .get_binding(binding.input)
                    .map_or_else(|| localization.tr("hud-settings-unbound"), key_label),
                Device::Gamepad => keymap
                    .gamepad
                    .get_button_binding(binding.input)
                    .map_or_else(
                        || localization.tr("hud-settings-unbound"),
                        gamepad_binding_label,
                    ),
            }
        };
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child)
                && text.0 != label
            {
                text.0 = label.clone();
            }
        }
    }

    for (marker, mut text) in &mut conflict_labels {
        let new_text = conflicts
            .0
            .get(&marker.0)
            .filter(|c| !c.is_empty())
            .map_or_else(String::new, |c| {
                format!(
                    "{}: {}",
                    localization.tr("hud-controls-conflicts_with"),
                    c.iter()
                        .map(|i| localization.tr(i.ftl_key()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

/// Reacts to every [`RebindOutcome`]: writes the (already-mutated, per
/// `capture_rebind`) [`KeyMap`] back into [`XindelerSettings::controls`] and
/// saves `settings.ron` immediately — the "rebind → restart → persists"
/// acceptance bar — and updates [`RebindConflicts`] for that row's warning
/// label.
fn persist_and_track_conflicts(
    mut outcomes: MessageReader<RebindOutcome>,
    keymap: Res<KeyMap>,
    mut settings: ResMut<XindelerSettings>,
    mut conflicts: ResMut<RebindConflicts>,
) {
    let mut any = false;
    for outcome in outcomes.read() {
        any = true;
        if outcome.conflicts.is_empty() {
            conflicts.0.remove(&outcome.input);
        } else {
            conflicts.0.insert(outcome.input, outcome.conflicts.clone());
        }
    }
    if any {
        settings.controls = keymap.clone();
        if let Err(err) = settings.save() {
            error!("controls screen: failed to persist settings.ron after a rebind: {err}");
        }
    }
}

/// A short, readable label for a keyboard/mouse binding (`KeyW` → `W`,
/// `Digit1` → `1`, `Mouse(Left)` → `LMB`, anything else falls back to its
/// `Debug` form so a rare/exotic key still shows SOMETHING rather than
/// silently blanking).
///
/// `pub(crate)` (BL-82 EM-5.3): `hotbar.rs` reuses this exact formatter for
/// its slot keybind labels rather than duplicating the prettifier.
pub(crate) fn key_label(binding: KeyOrMouse) -> String {
    match binding {
        KeyOrMouse::Key(key) => {
            let debug = format!("{key:?}");
            debug
                .strip_prefix("Key")
                .or_else(|| debug.strip_prefix("Digit"))
                .map_or(debug.clone(), str::to_owned)
        },
        KeyOrMouse::Mouse(bevy::input::mouse::MouseButton::Left) => "LMB".to_owned(),
        KeyOrMouse::Mouse(bevy::input::mouse::MouseButton::Right) => "RMB".to_owned(),
        KeyOrMouse::Mouse(bevy::input::mouse::MouseButton::Middle) => "MMB".to_owned(),
        KeyOrMouse::Mouse(other) => format!("{other:?}"),
    }
}

/// A short label for a gamepad binding — a plain button shows its variant
/// name; a chord shows `"<modifier>+<button>"` (e.g. `"LeftTrigger+South"`),
/// visibly distinct from a plain binding so the "layer" mechanic is legible
/// in the UI, not just in data.
fn gamepad_binding_label(binding: GamepadBinding) -> String {
    match binding {
        GamepadBinding::Button(button) => format!("{button:?}"),
        GamepadBinding::Chord { modifier, button } => format!("{modifier:?}+{button:?}"),
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// BL-82 EM-5.17/5.18 click-routing fix regression: `ControlsScreenRoot`
    /// is a full-screen modal backdrop structurally identical to
    /// `DiaryWindowRoot`/`InventoryWindowRoot`/`EscMenuRoot`/`FullMapRoot`,
    /// and pins that it now actually carries `GlobalZIndex(MODAL_WINDOWS)`,
    /// matching `diary.rs`'s `spawn_diary_window_uses_skill_tree_bg_and_
    /// modal_z_index` test. Before this fix `ControlsScreenRoot` had NO
    /// `GlobalZIndex` at all (default z-partition 0) — a same-bug-class miss
    /// from the pass that fixed the other three siblings — so it sat BELOW
    /// the always-on ambient chrome (hotbar/orbs =
    /// `ORBS_ACTION_BAR_PARTY_MINIMAP`=20): wherever the Controls screen
    /// visually overlapped that chrome, `bevy_ui` picking (highest
    /// z-partition first) routed clicks to the chrome in front instead of
    /// the Controls panel underneath.
    #[test]
    fn controls_screen_root_carries_the_modal_windows_z_index() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(KeyMap::default());
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &[],
        ));

        app.world_mut()
            .run_system_once(spawn_controls_screen)
            .expect("spawn_controls_screen runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<ControlsScreenRoot>>()
            .single(world)
            .expect("ControlsScreenRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
    }

    #[test]
    fn key_label_strips_the_key_prefix() {
        assert_eq!(
            key_label(KeyOrMouse::Key(bevy::input::keyboard::KeyCode::KeyW)),
            "W"
        );
        assert_eq!(
            key_label(KeyOrMouse::Key(bevy::input::keyboard::KeyCode::Digit1)),
            "1"
        );
        assert_eq!(
            key_label(KeyOrMouse::Mouse(bevy::input::mouse::MouseButton::Left)),
            "LMB"
        );
    }

    #[test]
    fn gamepad_chord_label_is_visibly_distinct_from_a_plain_button() {
        use bevy::input::gamepad::GamepadButton;
        let plain = gamepad_binding_label(GamepadBinding::Button(GamepadButton::South));
        let chord = gamepad_binding_label(GamepadBinding::Chord {
            modifier: GamepadButton::LeftTrigger,
            button: GamepadButton::South,
        });
        assert_ne!(plain, chord);
        assert!(chord.contains('+'));
    }

    /// The literal EM-5.11 acceptance bar, end to end against the REAL
    /// systems (`xindeler_input::capture::capture_rebind` +
    /// [`persist_and_track_conflicts`]) and a real `XindelerSettings::save`/
    /// `load_or_default` round trip through a real temp-dir file — not a
    /// mock: "rebind a key, see it persist, see a conflict get flagged."
    ///
    /// Scoped to `XINDELER_USERDATA` since no other test in this crate reads
    /// or writes it (grepped before adding this test); all mutations are
    /// sequential within this one `#[test]` fn, matching the safety posture
    /// `main.rs`'s own `XINDELER_PRESENT_MODE` test already documents.
    #[test]
    fn end_to_end_rebind_persists_to_disk_and_flags_a_conflict() {
        use bevy::{
            app::{App, Update},
            input::{ButtonInput, keyboard::KeyCode},
        };
        use xindeler_input::capture::capture_rebind;

        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: see doc comment above — sole owner of this env var in the
        // crate's test suite, all edits sequential within this one test.
        unsafe {
            std::env::set_var(xindeler_app::settings::USERDATA_ENV, dir.path());
        }

        let mut app = App::new();
        app.insert_resource(XindelerSettings::default());
        app.insert_resource(KeyMap::default());
        app.insert_resource(RebindRequest::default());
        app.insert_resource(RebindConflicts::default());
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        app.add_message::<RebindOutcome>();
        app.add_systems(
            Update,
            (capture_rebind, persist_and_track_conflicts).chain(),
        );

        // 1. Rebind Jump onto U (unbound by default) — must persist cleanly,
        // no conflict.
        app.world_mut().resource_mut::<RebindRequest>().0 =
            Some(RebindTarget::Keyboard(GameInput::Jump));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyU);
        app.update();

        assert_eq!(
            app.world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Jump),
            Some(KeyOrMouse::Key(KeyCode::KeyU))
        );
        assert!(
            !app.world()
                .resource::<RebindConflicts>()
                .0
                .contains_key(&GameInput::Jump)
        );

        let persisted = XindelerSettings::load_or_default();
        assert_eq!(
            persisted.controls.keyboard.get_binding(GameInput::Jump),
            Some(KeyOrMouse::Key(KeyCode::KeyU)),
            "settings.ron on disk must reflect the rebind — the \"rebind → restart → persists\" \
             bar"
        );

        // 2. Rebind Interact onto the SAME key U — a genuine conflict with
        // Jump (they may not share a binding).
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::KeyU); // fresh press edge, not the stale one from step 1
        app.world_mut().resource_mut::<RebindRequest>().0 =
            Some(RebindTarget::Keyboard(GameInput::Interact));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyU);
        app.update();

        assert_eq!(
            app.world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Interact),
            Some(KeyOrMouse::Key(KeyCode::KeyU))
        );
        let conflicts = app.world().resource::<RebindConflicts>();
        assert_eq!(
            conflicts.0.get(&GameInput::Interact),
            Some(&vec![GameInput::Jump]),
            "rebinding onto an already-occupied key must flag the conflict, not silently \
             double-bind"
        );

        // SAFETY: see comment above.
        unsafe {
            std::env::remove_var(xindeler_app::settings::USERDATA_ENV);
        }
    }

    /// BL-82 EM-5.17 Phase 0 regression: [`sync_window_visibility`] must NOT
    /// also apply [`HudAction::ToggleWindow`] — [`xindeler_ui::hud_state::
    /// apply_hud_actions`] is the one and only applier. Before the fix, both
    /// systems drained the SAME message (each `MessageReader` keeps its own
    /// cursor), so writing ONE `ToggleWindow(Diary)` and running a single
    /// `app.update()` left `HudState` toggled ON then immediately back OFF —
    /// this assertion would have failed (`is_open` returning `false`)
    /// against the pre-fix code. With the duplicate consumer removed, one
    /// `ToggleWindow` message flips the window open exactly once.
    #[test]
    fn a_single_toggle_window_action_opens_the_window_exactly_once() {
        use bevy::app::{App, Update};
        use xindeler_ui::hud_state::apply_hud_actions;

        let mut app = App::new();
        app.init_resource::<HudState>();
        app.add_message::<HudAction>();
        app.add_systems(Update, (apply_hud_actions, sync_window_visibility));

        app.world_mut()
            .write_message(HudAction::ToggleWindow(HudWindow::Diary));
        app.update();

        assert!(
            app.world().resource::<HudState>().is_open(HudWindow::Diary),
            "a single ToggleWindow(Diary) message must leave the Diary window open — a second \
             consumer double-applying the same message would toggle it back off"
        );
    }

    /// BL-82 EM-5.16 (T56.44 follow-up): a row's action label resolves the
    /// REAL `gameinput-*` key from the repo's own catalog at spawn time, and
    /// switching the active locale re-localizes it live — the same real-
    /// catalog hot-swap proof `settings_window.rs`/`esc_menu.rs` use, applied
    /// to the `GameInput::ftl_key()` row labels this follow-up added.
    #[test]
    fn switching_locale_relocalizes_a_curated_action_row_label_live() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(KeyMap::default());
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            xindeler_ui::i18n::DEFAULT_HUD_FTL_FILES,
        ));
        app.init_resource::<CurrentLocale>();

        app.world_mut()
            .run_system_once(spawn_controls_screen)
            .expect("spawn_controls_screen runs");

        fn move_forward_label(app: &mut App) -> String {
            let world = app.world_mut();
            world
                .query::<(&LocalizedText, &Text)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == GameInput::MoveForward.ftl_key())
                .map(|(_, text)| text.0.clone())
                .expect("the Move Forward row label was spawned and tagged")
        }

        assert_eq!(
            move_forward_label(&mut app),
            "Move Forward",
            "must show the real en catalog text at spawn time"
        );

        app.world_mut().resource_mut::<CurrentLocale>().0 = "es".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_text)
            .expect("relocalize runs");

        assert_eq!(
            move_forward_label(&mut app),
            "Avanzar",
            "must resolve to the REAL es catalog's own gameinput-moveforward value, not the en \
             fallback"
        );
    }
}
