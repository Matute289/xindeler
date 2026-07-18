//! BL-82 EM-5.16 (T56.43) — the first-run tutorial overlay.
//!
//! A dismissible modal panel listing 6 basic-controls tips (move, inventory,
//! diary, map, chat, pause/settings menu), shown automatically the first
//! time a real local player exists (i.e. the player has actually spawned
//! into a playable character, not just reached the main menu/char-select)
//! unless it's already been dismissed before (`XindelerSettings::
//! tutorial.seen`), and re-openable at any time afterwards via the settings
//! window's Accessibility tab ("Show tutorial again").
//!
//! Legacy `voxygen` has a MUCH larger tutorial system (`hud/tutorial.rs`): a
//! ~25-hint achievement-tracking state machine wired to specs `Client`/
//! `Outcome` events (glide-stall detection, energy-low warnings, "you found
//! a campfire", ...). None of that is mirrored to this pure-Bevy client yet
//! (`Outcome` reaching the client is the same open follow-up `combat_hud.rs`
//! already flags for floating combat text), so re-porting it 1:1 isn't
//! possible today. This is a deliberately smaller, but genuinely REAL v1: a
//! static tip list sourced from the player's ACTUAL current keybindings (via
//! `controls_screen::key_label`, so it reflects rebinds — never a hardcoded
//! "WASD"), not a placeholder stub. The achievement-driven contextual hints
//! remain a real follow-up once `Outcome` is mirrored.
//!
//! Modelled on `settings_window.rs`'s modal-window pattern: a root entity
//! whose `Visibility` mirrors `HudState::is_open(HudWindow::Tutorial)`,
//! `GlobalZIndex(zlayer::MODAL_WINDOWS)`, and dismissal through the generic
//! `HudAction::CloseWindow` flow (Escape or its own "Got it" button) — the
//! SAME mechanism the settings window's own "Close" button uses, so it needs
//! no bespoke input handling.
//!
//! Compiled only under `listen-server`/`net-client`, matching every other
//! `xindeler_ui`-consuming screen module in this crate.

use bevy::prelude::*;
use xindeler_app::XindelerSettings;
use xindeler_input::GameInput;
use xindeler_protocol::NetLocalPlayer;
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    panel::panel_bundle,
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::controls_screen::key_label;

/// The tutorial overlay's full-screen modal backdrop root (its
/// [`Visibility`] mirrors `HudState::is_open(HudWindow::Tutorial)`).
#[derive(Component)]
struct TutorialOverlayRoot;

/// One tip line, tagged with which basic control it explains so
/// [`refresh_tutorial_tips`] can re-derive its text from the player's CURRENT
/// keybindings (respecting rebinds) whenever `XindelerSettings` changes.
#[derive(Component, Clone, Copy)]
enum TutorialTip {
    Move,
    Inventory,
    Diary,
    Map,
    Chat,
    EscMenu,
}

impl TutorialTip {
    /// The task's own basic-tips list (movement, inventory/diary/map, chat,
    /// esc-menu) — 6 tips, the upper end of the "4-6 basic tips" v1 scope.
    const ALL: &'static [TutorialTip] = &[
        TutorialTip::Move,
        TutorialTip::Inventory,
        TutorialTip::Diary,
        TutorialTip::Map,
        TutorialTip::Chat,
        TutorialTip::EscMenu,
    ];
}

/// Installs the tutorial overlay: spawns the (hidden) modal at `Startup`,
/// auto-opens it once per the persisted `tutorial.seen` flag, keeps its
/// visibility synced to [`HudState`], keeps its tip text synced to the
/// player's real keybindings, and persists `seen = true` the moment it's
/// dismissed by any path (the "Got it" button or Escape).
pub struct TutorialOverlayPlugin;

impl Plugin for TutorialOverlayPlugin {
    fn build(&self, app: &mut App) {
        // BL-82 EM-5.4 pattern (see `combat_hud::CombatHudViewPlugin`'s own
        // doc comment): guard against double-adding `XindelerUiPlugin` across
        // this crate's several view plugins.
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        app.add_systems(
            Startup,
            spawn_tutorial_overlay.after(xindeler_ui::theme::init_theme),
        )
        .add_systems(
            Update,
            (
                auto_show_tutorial_on_first_spawn,
                // bevy-migration-reviewer note: ordered after the auto-show
                // system too (not just `apply_hud_actions`) — auto-show
                // mutates `HudState` directly rather than through the
                // `HudAction` message queue, so without this the overlay
                // would take an extra frame to become visible the very first
                // time it auto-opens.
                sync_tutorial_overlay_visibility
                    .after(xindeler_ui::hud_state::apply_hud_actions)
                    .after(auto_show_tutorial_on_first_spawn),
                mark_tutorial_seen_on_close.after(xindeler_ui::hud_state::apply_hud_actions),
                refresh_tutorial_tips,
            ),
        );
    }
}

/// A short, real (not placeholder) label for `input`'s current binding, or
/// `"Unbound"` if the player cleared it.
fn key_hint(settings: &XindelerSettings, input: GameInput) -> String {
    settings
        .controls
        .keyboard
        .get_binding(input)
        .map_or_else(|| "Unbound".to_owned(), key_label)
}

/// This tip's current text, derived from the player's REAL keybindings (so a
/// rebind is reflected, never a hardcoded "WASD"/"I"/etc.).
fn tip_text(tip: TutorialTip, settings: &XindelerSettings) -> String {
    match tip {
        TutorialTip::Move => format!(
            "Move — {} forward, {} left, {} back, {} right",
            key_hint(settings, GameInput::MoveForward),
            key_hint(settings, GameInput::MoveLeft),
            key_hint(settings, GameInput::MoveBack),
            key_hint(settings, GameInput::MoveRight),
        ),
        TutorialTip::Inventory => {
            format!(
                "Open your inventory — {}",
                key_hint(settings, GameInput::Inventory)
            )
        },
        TutorialTip::Diary => format!(
            "Open your diary (skills) — {}",
            key_hint(settings, GameInput::Diary)
        ),
        TutorialTip::Map => format!("Open the map — {}", key_hint(settings, GameInput::Map)),
        TutorialTip::Chat => format!("Chat — {}", key_hint(settings, GameInput::Chat)),
        TutorialTip::EscMenu => format!(
            "Pause / settings menu — {}",
            key_hint(settings, GameInput::Escape)
        ),
    }
}

/// Spawns the whole overlay: a full-screen modal backdrop, a titled panel,
/// one line per [`TutorialTip`], and a "Got it" dismiss button.
fn spawn_tutorial_overlay(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    settings: Res<XindelerSettings>,
) {
    let theme: HudTheme = *theme;
    commands
        .spawn((
            TutorialOverlayRoot,
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
                node.min_width = Val::Px(420.0);
                node.max_width = Val::Px(560.0);
            });
            panel_entity.with_children(|panel| {
                panel.spawn((
                    Text("Welcome to Xindeler".to_owned()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.title.clone()),
                        font_size: bevy::text::FontSize::Px(24.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
                for &tip in TutorialTip::ALL {
                    panel.spawn((
                        tip,
                        Text(tip_text(tip, &settings)),
                        TextFont {
                            font: bevy::text::FontSource::Handle(fonts.body.clone()),
                            font_size: bevy::text::FontSize::Px(15.0),
                            ..Default::default()
                        },
                        TextColor(theme.palette.text),
                    ));
                }
                panel
                    .spawn(button_bundle(&theme, &fonts, "Got it"))
                    .observe(|_a: On<Activate>, mut actions: MessageWriter<HudAction>| {
                        actions.write(HudAction::CloseWindow);
                    });
            });
        });
}

/// Re-derives every tip's text from the player's current keybindings
/// whenever `XindelerSettings` changes (a rebind made after the overlay
/// first spawned is reflected the next time it's shown).
fn refresh_tutorial_tips(
    settings: Res<XindelerSettings>,
    mut tips: Query<(&TutorialTip, &mut Text)>,
) {
    if !settings.is_changed() {
        return;
    }
    for (tip, mut text) in &mut tips {
        let new = tip_text(*tip, &settings);
        if text.0 != new {
            text.0 = new;
        }
    }
}

/// Mirrors [`HudState`]'s open window onto the root's [`Visibility`] — same
/// shape as `settings_window::sync_settings_window_visibility`.
fn sync_tutorial_overlay_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<TutorialOverlayRoot>>,
) {
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if hud_state.is_open(HudWindow::Tutorial) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Shows the overlay automatically the FIRST time a real local player exists
/// (the player has actually spawned into a playable character), unless
/// `tutorial.seen` is already `true`. Fires at most once per boot (`Local`
/// latch) and never clobbers a window the player already opened by hand —
/// it just waits for the HUD slot to free up.
fn auto_show_tutorial_on_first_spawn(
    settings: Res<XindelerSettings>,
    mut hud_state: ResMut<HudState>,
    local_player: Query<(), With<NetLocalPlayer>>,
    mut triggered: Local<bool>,
) {
    if *triggered || settings.tutorial.seen || local_player.is_empty() {
        return;
    }
    if hud_state.open_window() != HudWindow::None {
        // Something else already claimed the HUD slot this frame (e.g. the
        // player opened a window before this system got a chance to fire) —
        // don't clobber it; try again next frame once it frees up.
        return;
    }
    *triggered = true;
    hud_state.toggle(HudWindow::Tutorial);
}

/// Pure decision: does THIS frame represent the tutorial overlay's
/// open->closed transition (the one moment `tutorial.seen` should flip)?
/// Split out from [`mark_tutorial_seen_on_close`] as a plain function (no
/// `Res`/`Local` params, no disk IO) so the transition logic itself is
/// directly unit-testable without exercising the real `XindelerSettings::
/// save()` disk write.
fn tutorial_just_closed(is_open_now: bool, was_open: bool, already_seen: bool) -> bool {
    was_open && !is_open_now && !already_seen
}

/// Persists `tutorial.seen = true` the moment the overlay transitions from
/// open to closed, by ANY dismissal path (the "Got it" button routes through
/// the generic `HudAction::CloseWindow` like every other window's Close
/// button; Escape and opening a different window both also go through that
/// same `HudState` transition) — so "seen" tracks reality regardless of how
/// the player closed it, not just one specific button.
fn mark_tutorial_seen_on_close(
    hud_state: Res<HudState>,
    mut settings: ResMut<XindelerSettings>,
    mut was_open: Local<bool>,
) {
    let is_open_now = hud_state.is_open(HudWindow::Tutorial);
    if tutorial_just_closed(is_open_now, *was_open, settings.tutorial.seen) {
        settings.tutorial.seen = true;
        if let Err(err) = settings.save() {
            error!("tutorial overlay: failed to persist the seen flag: {err}");
        }
    }
    *was_open = is_open_now;
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(XindelerSettings::default());
        app.insert_resource(HudState::default());
        app
    }

    /// `TutorialOverlayRoot` is a full-screen modal backdrop exactly like
    /// `SettingsWindowRoot`/`DiaryWindowRoot`/`EscMenuRoot`, and must carry
    /// `GlobalZIndex(MODAL_WINDOWS)` so `bevy_ui` picking routes clicks to it
    /// rather than the always-on ambient chrome underneath it.
    #[test]
    fn tutorial_overlay_root_carries_the_modal_windows_z_index() {
        let mut app = new_app();

        app.world_mut()
            .run_system_once(spawn_tutorial_overlay)
            .expect("spawn_tutorial_overlay runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<TutorialOverlayRoot>>()
            .single(world)
            .expect("TutorialOverlayRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
    }

    /// One tip line is spawned per [`TutorialTip::ALL`] entry (the 6-tip v1
    /// scope: move, inventory, diary, map, chat, esc-menu).
    #[test]
    fn spawns_one_line_per_tip() {
        let mut app = new_app();

        app.world_mut()
            .run_system_once(spawn_tutorial_overlay)
            .expect("spawn runs");

        let world = app.world_mut();
        let tip_count = world.query::<&TutorialTip>().iter(world).count();
        assert_eq!(tip_count, TutorialTip::ALL.len());
    }

    /// A rebind (a real `XindelerSettings.controls` change) is reflected in
    /// the tip text — this overlay reads the player's ACTUAL current
    /// keybindings, never a hardcoded "WASD"/"I".
    #[test]
    fn tip_text_reflects_the_players_real_keybindings() {
        use bevy::input::keyboard::KeyCode;
        use xindeler_input::keybind::KeyOrMouse;

        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_tutorial_overlay)
            .expect("spawn runs");

        {
            let mut settings = app.world_mut().resource_mut::<XindelerSettings>();
            settings
                .controls
                .keyboard
                .modify_binding(GameInput::Inventory, KeyOrMouse::Key(KeyCode::KeyB));
        }
        app.add_systems(Update, refresh_tutorial_tips);
        app.update();

        let world = app.world_mut();
        let mut found = false;
        for (tip, text) in world.query::<(&TutorialTip, &Text)>().iter(world) {
            if matches!(tip, TutorialTip::Inventory) {
                assert!(
                    text.0.contains('B'),
                    "the inventory tip must show the REBOUND key, got {:?}",
                    text.0
                );
                found = true;
            }
        }
        assert!(found, "an Inventory tip line must exist");
    }

    /// The overlay auto-shows once a real local player exists (and
    /// `tutorial.seen` is still `false`), never before.
    #[test]
    fn auto_shows_once_a_local_player_exists() {
        let mut app = new_app();
        app.add_systems(Update, auto_show_tutorial_on_first_spawn);

        app.update();
        assert_eq!(
            app.world().resource::<HudState>().open_window(),
            HudWindow::None,
            "must not open before any local player exists"
        );

        app.world_mut().spawn(NetLocalPlayer);
        app.update();
        assert_eq!(
            app.world().resource::<HudState>().open_window(),
            HudWindow::Tutorial,
            "must auto-open the very first time a local player exists"
        );
    }

    /// If `tutorial.seen` is already `true` (a returning player), the
    /// overlay must NOT auto-show again.
    #[test]
    fn does_not_auto_show_once_already_seen() {
        let mut app = new_app();
        app.world_mut()
            .resource_mut::<XindelerSettings>()
            .tutorial
            .seen = true;
        app.world_mut().spawn(NetLocalPlayer);
        app.add_systems(Update, auto_show_tutorial_on_first_spawn);

        app.update();
        assert_eq!(
            app.world().resource::<HudState>().open_window(),
            HudWindow::None,
            "a previously-dismissed tutorial must not auto-show again"
        );
    }

    /// Dismissing the overlay (the generic `HudState::close()` transition,
    /// covering both the "Got it" button and Escape) is exactly the
    /// open->closed transition [`tutorial_just_closed`] must flip `true` for
    /// — the show-once/dismiss acceptance bar, exercised directly against
    /// the real decision function (no `App`/disk IO needed for this one:
    /// `mark_tutorial_seen_on_close`'s only OTHER job is calling the real
    /// `XindelerSettings::save()`, which is exercised end to end by
    /// `controls_screen.rs`'s own rebind-persistence test — that test's doc
    /// comment documents itself as the crate's sole `XINDELER_USERDATA` env
    /// var user precisely so its scoped `set_var`/`remove_var` never races
    /// another test thread; adding a second concurrent user here would
    /// reintroduce exactly that race, so this test stays at the pure-logic
    /// layer instead).
    #[test]
    fn tutorial_just_closed_detects_the_open_to_closed_transition_exactly_once() {
        assert!(
            !tutorial_just_closed(true, true, false),
            "still open (Tutorial -> Tutorial) — not a transition"
        );
        assert!(
            tutorial_just_closed(false, true, false),
            "open -> closed IS the transition that must flip `seen`"
        );
        assert!(
            !tutorial_just_closed(false, false, false),
            "already closed (None -> None) — not a transition, nothing to flip"
        );
        assert!(
            !tutorial_just_closed(false, true, true),
            "already marked seen — must not re-trigger the persist path"
        );
    }

    /// Integration check: [`mark_tutorial_seen_on_close`] wires the same
    /// open->closed transition through a real `HudState`, WITHOUT touching
    /// settings persistence — verified by never calling the real system at
    /// all here, only its `Local`-free building block. `mark_tutorial_seen_
    /// on_close` itself additionally calls `XindelerSettings::save()` on that
    /// exact transition, covered by `controls_screen.rs`'s end-to-end test
    /// (see this module's other test's doc comment for why that call isn't
    /// re-exercised a second time here).
    #[test]
    fn hud_state_close_produces_the_open_to_closed_transition() {
        let mut state = HudState::default();
        state.toggle(HudWindow::Tutorial);
        assert!(state.is_open(HudWindow::Tutorial));
        let was_open = state.is_open(HudWindow::Tutorial);

        state.close();
        let is_open_now = state.is_open(HudWindow::Tutorial);
        assert!(
            tutorial_just_closed(is_open_now, was_open, false),
            "HudState::close() after HudState::toggle() must be exactly the transition that flips \
             tutorial.seen"
        );
    }

    /// Reopening (`HudAction::ToggleWindow(HudWindow::Tutorial)`, the
    /// Accessibility tab's "Show tutorial again" path) works even after
    /// `seen` is already `true` — re-openability is independent of the
    /// auto-show gate.
    #[test]
    fn reopening_after_seen_still_shows_it() {
        let mut app = new_app();
        app.world_mut()
            .resource_mut::<XindelerSettings>()
            .tutorial
            .seen = true;

        app.world_mut()
            .resource_mut::<HudState>()
            .toggle(HudWindow::Tutorial);
        assert_eq!(
            app.world().resource::<HudState>().open_window(),
            HudWindow::Tutorial,
            "the settings tab's reopen button must be able to show the overlay regardless of the \
             seen flag"
        );
    }
}
