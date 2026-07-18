//! BL-82 EM-5.12 (T56.38) — the Escape/pause menu.
//!
//! Ports legacy `voxygen`'s esc menu (`voxygen/src/hud/esc_menu.rs`) into the
//! Bevy client as a proper pause panel: **Resume**, **Settings** (opens the
//! tabbed settings window — `crate::settings_window`, `HudWindow::Settings`),
//! **Controls** (opens the EM-5.11 rebinding screen — `crate::controls_screen`,
//! `HudWindow::Controls`), **Servers** (a stub until the EM-5.9 server browser
//! lands — see [`handle_servers_click`]), **Logout** (stub — no character-
//! select/main-menu flow on this embedded-server path yet), and **Quit**
//! (a real `AppExit`).
//!
//! Escape itself is the universal back-out key: [`toggle_esc_menu`] closes
//! whatever window is open, and summons this menu only when nothing is open.
//!
//! The graphics toggles this module used to host inline (SSAO/TAA/shadow
//! cascades — the pre-EM-5.12 "Video slice") moved into the settings window's
//! **Video** tab, alongside the full `GraphicsSettings` set; the live camera
//! reconcile (`apply_graphics_settings`) moved there with them. This module is
//! now pure menu — it owns no settings state.
//!
//! ## BL-82 EM-5.16 (T56.44) — full i18n
//! Every button label and the "Game Menu" heading now resolve through
//! `xindeler_ui::i18n::Localization` and are tagged
//! [`xindeler_ui::i18n::LocalizedLabel`]/[`xindeler_ui::i18n::LocalizedText`],
//! so this screen re-localizes live the moment the settings window's Language
//! tab changes — see that module's own doc comment for the full reactive
//! chain. Resume/Settings/Controls reuse the SAME `common-*` keys the
//! settings window's own tab bar already resolves; Quit reuses the pre-
//! existing `esc_menu-quit_game` key (unmodified, isolation law).
//! Servers/Logout stay honest STUBS (see [`spawn_esc_menu`]'s doc), so their
//! "(soon)" qualifier lives in two NEW keys (`esc_menu-servers_soon`/
//! `esc_menu-logout_soon`) added to `esc_menu.ftl` rather than English text
//! bolted onto a real key.
//!
//! Compiled only under `listen-server`/`net-client`, matching every other
//! `xindeler_ui`-consuming screen module in this crate.

use bevy::{
    ecs::{change_detection::NonSend, schedule::common_conditions::not},
    prelude::*,
};
use xindeler_input::{ActionState, GameInput};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    i18n::{Localization, LocalizedLabel, LocalizedText},
    panel::panel_bundle,
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::{chat::text_input_focused, targeting::hard_lock_active};

/// Installs the esc/pause menu: spawns the (hidden) panel at `Startup`, opens/
/// closes it on Escape, and keeps its visibility synced.
pub struct EscMenuPlugin;

impl Plugin for EscMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            spawn_esc_menu.after(xindeler_ui::theme::init_theme),
        )
        .add_systems(
            Update,
            (
                // Reads `ActionState` — order after the frame's real input
                // resolution (same fix as every other `ActionState`-reading
                // toggle in this crate). Gated on `!text_input_focused` so
                // Escape while typing in chat blurs the chat box instead of
                // opening the pause menu (`chat::blur_chat_input_on_escape`
                // owns that). ALSO gated on `!hard_lock_active` (BL-82
                // EM-5.19 Phase 3, finalizing the seam P2 explicitly
                // deferred): while a hard lock is active, Escape ONLY clears
                // it (`targeting::clear_hard_lock_on_escape`, ordered
                // `.after(toggle_esc_menu)` so it reads this SAME frame's
                // pre-clear lock state — see that system's own doc comment
                // for the full ordering argument) and must NOT also open/
                // close the pause menu on that same press. Also ordered
                // `.after(targeting::release_invalid_hard_lock)`
                // (bevy-migration-reviewer finding on PR #120): that system
                // lives in `MirrorSet`, which has no inherited ordering vs.
                // this plain `Update` system (only `MirrorSet -> GameplaySet`
                // is chained), so without this edge "the locked target
                // dies/leaves range AND Escape is pressed the same frame"
                // would race `hard_lock_active`'s read against the auto-
                // release's `HardLock` write with no scheduling guarantee.
                // Ordered BEFORE `apply_hud_actions` so the open/close it
                // requests lands the SAME frame (the atomic cursor-free
                // chain `cursor::update_cursor_free` relies on).
                toggle_esc_menu
                    .after(xindeler_input::InputResolveSet)
                    .after(crate::targeting::release_invalid_hard_lock)
                    .before(xindeler_ui::hud_state::apply_hud_actions)
                    .run_if(not(text_input_focused))
                    .run_if(not(hard_lock_active)),
                // After `apply_hud_actions` so the panel's `Visibility` matches
                // the window state THIS frame (no one-frame open lag).
                sync_esc_menu_visibility.after(xindeler_ui::hud_state::apply_hud_actions),
            ),
        );
    }
}

/// The pause panel root (its [`Visibility`] mirrors
/// `HudState::is_open(HudWindow::EscMenu)`).
#[derive(Component)]
struct EscMenuRoot;

/// Escape is the universal "back out" key: it closes whatever window is open
/// (the pause menu, Diary, Inventory, Map, …), and only summons the pause menu
/// when NOTHING is open. This mirrors legacy `voxygen`'s `Show::toggle_windows`
/// exactly (Escape closes any open window; with nothing open it opens the esc
/// menu). Making it universal here — rather than relying on each window to
/// carry its own Escape-close handler — means every current AND future
/// [`HudWindow`] gets Escape-to-close for free (only `Map` had its own handler
/// before; Diary/Inventory/Social/Crafting/Controls had none, so Escape did
/// nothing with them open — ecs-design-reviewer finding). Writing a
/// [`HudAction::CloseWindow`] is idempotent ([`HudState::close`] sets `None`),
/// so `map_view::close_full_map_on_escape` also firing on the same frame is
/// harmless — both just close the (one) open window.
///
/// `pub(crate)` (BL-82 EM-5.19 Phase 3): so
/// `targeting::clear_hard_lock_on_escape` can name it in an explicit
/// `.after(toggle_esc_menu)` ordering edge — see that system's doc comment for
/// why the ORDER (not just the `hard_lock_active` run-condition gate above) is
/// load-bearing for "Escape-while-locked clears the lock without also opening
/// the pause menu on the same press."
pub(crate) fn toggle_esc_menu(
    action_state: Res<ActionState>,
    hud_state: Res<HudState>,
    mut actions: MessageWriter<HudAction>,
) {
    if !action_state.just_pressed(GameInput::Escape) {
        return;
    }
    if hud_state.any_window_open() {
        actions.write(HudAction::CloseWindow);
    } else {
        actions.write(HudAction::ToggleWindow(HudWindow::EscMenu));
    }
}

/// Mirrors [`HudState`]'s open window onto the panel's [`Visibility`] (read-
/// only w.r.t. [`HudAction`] — `apply_hud_actions` is the one applier, exactly
/// like `controls_screen::sync_window_visibility`).
fn sync_esc_menu_visibility(
    hud_state: Res<HudState>,
    mut root: Query<&mut Visibility, With<EscMenuRoot>>,
) {
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if hud_state.is_open(HudWindow::EscMenu) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Spawns the centred pause panel: a "Game Menu" title and the menu buttons
/// (Resume / Settings / Controls / Servers / Logout / Quit).
///
/// BL-82 EM-5.17/5.18 click-routing fix: `EscMenuRoot` is a full-screen modal
/// backdrop exactly like `DiaryWindowRoot`/`InventoryWindowRoot`/`FullMapRoot`,
/// and carries `GlobalZIndex(zlayer::MODAL_WINDOWS)` so `bevy_ui` picking
/// (highest z-partition first) routes clicks to the pause panel rather than the
/// always-on ambient HUD chrome (hotbar/orbs) it overlaps.
fn spawn_esc_menu(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    localization: NonSend<Localization>,
) {
    let theme: HudTheme = *theme;
    commands
        .spawn((
            EscMenuRoot,
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
                node.min_width = Val::Px(360.0);
                node.align_items = AlignItems::Stretch;
            });
            panel_entity.with_children(|panel| {
                heading(panel, &fonts, &theme, &localization, "esc_menu-title", 28.0);

                labeled_button(panel, &theme, &fonts, &localization, "common-resume")
                    .observe(handle_resume_click);
                labeled_button(panel, &theme, &fonts, &localization, "common-settings")
                    .observe(handle_settings_click);
                labeled_button(panel, &theme, &fonts, &localization, "common-controls")
                    .observe(handle_controls_click);
                // Stub until the EM-5.9 server browser (T56.31) lands — it is
                // in a separate, not-yet-merged PR chain (#166→#168), and
                // wiring it here would create exactly the cross-PR dependency
                // this task deliberately avoids. TODO(EM-5.9 merge): open the
                // real server browser here.
                labeled_button(
                    panel,
                    &theme,
                    &fonts,
                    &localization,
                    "esc_menu-servers_soon",
                )
                .observe(handle_servers_click);
                // Stub: there is no character-select / main-menu flow to return
                // to on this embedded-server path yet (EM-5.9/5.14). TODO: route
                // to the main menu once that state machine merges.
                labeled_button(panel, &theme, &fonts, &localization, "esc_menu-logout_soon")
                    .observe(handle_logout_click);
                labeled_button(panel, &theme, &fonts, &localization, "esc_menu-quit_game")
                    .observe(handle_quit_click);
            });
        });
}

/// Spawns a themed button whose label is a resolved `.ftl` message value,
/// tagged [`LocalizedLabel`] so it re-resolves live on a locale change (the
/// same small helper `settings_window.rs` uses).
fn labeled_button<'a>(
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

/// Resume closes the pause menu (routes through the generic `HudAction` bus,
/// exactly like every other window's close control).
fn handle_resume_click(_activate: On<Activate>, mut actions: MessageWriter<HudAction>) {
    actions.write(HudAction::CloseWindow);
}

/// Settings opens the tabbed settings window. Opening it closes THIS menu (the
/// single mutually-exclusive `HudWindow` slot), so the pause panel gives way to
/// the settings window.
fn handle_settings_click(_activate: On<Activate>, mut actions: MessageWriter<HudAction>) {
    actions.write(HudAction::ToggleWindow(HudWindow::Settings));
}

/// Controls opens the EM-5.11 rebinding screen (keyboard/mouse + gamepad).
fn handle_controls_click(_activate: On<Activate>, mut actions: MessageWriter<HudAction>) {
    actions.write(HudAction::ToggleWindow(HudWindow::Controls));
}

/// Servers is a stub until the EM-5.9 server browser (T56.31) merges — see
/// [`spawn_esc_menu`]. It logs an honest TODO rather than opening a fake
/// browser.
fn handle_servers_click(_activate: On<Activate>) {
    info!("esc menu: Servers is stubbed until the EM-5.9 server browser lands (T56.31)");
}

/// Logout is a stub: there is no character-select / main-menu flow to return to
/// on this embedded-server path yet (EM-5.9/5.14).
fn handle_logout_click(_activate: On<Activate>) {
    info!("esc menu: Logout is stubbed until the EM-5.9 main-menu state machine lands");
}

/// Quit exits the client cleanly (`AppExit::Success`).
fn handle_quit_click(_activate: On<Activate>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}

/// Test-only: an empty-catalog `Localization` — every `.tr(key)` call
/// resolves to `key` itself (the documented, never-panic fallback), which is
/// all the structural tests below need (they never assert specific
/// translated text — see `switching_locale_relocalizes_the_quit_button_live`
/// for the one test that DOES, which loads the real catalog instead).
#[cfg(test)]
fn test_localization() -> Localization {
    Localization::load(&xindeler_ui::i18n::fallback_locale(), &[])
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// BL-82 EM-5.17/5.18 click-routing fix regression: `EscMenuRoot` is a
    /// full-screen modal backdrop exactly like `DiaryWindowRoot`/
    /// `InventoryWindowRoot`/`FullMapRoot`, and pins that it now actually
    /// carries `GlobalZIndex(MODAL_WINDOWS)`, matching `diary.rs`'s
    /// `spawn_diary_window_uses_skill_tree_bg_and_modal_z_index` test.
    /// Before this fix `EscMenuRoot` had NO `GlobalZIndex` at all (default
    /// z-partition 0) — it sat BELOW the always-on ambient chrome once that
    /// chrome gained its own higher z-index this phase (hotbar/orbs =
    /// `ORBS_ACTION_BAR_PARTY_MINIMAP`=20): wherever the pause panel
    /// visually overlapped the hotbar, `bevy_ui` picking (highest
    /// z-partition first) routed clicks to that ambient chrome instead of
    /// the pause menu underneath — i.e. opening ESC did not actually block
    /// hotbar interaction where they overlapped.
    #[test]
    fn esc_menu_root_carries_the_modal_windows_z_index() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(test_localization());

        app.world_mut()
            .run_system_once(spawn_esc_menu)
            .expect("spawn_esc_menu runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<EscMenuRoot>>()
            .single(world)
            .expect("EscMenuRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
    }

    /// Escape with nothing open requests the pause menu; Escape with ANY
    /// window open (the pause menu OR another panel like the Diary) requests a
    /// close — Escape is the universal back-out key (ecs-design-reviewer
    /// finding: it must close Diary/Inventory/etc., which had no Escape handler
    /// of their own, not just Map/EscMenu).
    #[test]
    fn escape_opens_and_closes_only_when_appropriate() {
        fn run(open: HudWindow) -> Vec<HudAction> {
            use xindeler_input::KeyMap;

            let mut app = App::new();
            app.init_resource::<HudState>();
            app.insert_resource(KeyMap::default());
            app.insert_resource(ActionState::default());
            app.init_resource::<ButtonInput<KeyCode>>();
            app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
            app.add_message::<HudAction>();
            if open != HudWindow::None {
                app.world_mut().resource_mut::<HudState>().toggle(open);
            }
            // Drive Escape through the real resolver so `just_pressed` is set.
            let esc_key = app
                .world()
                .resource::<KeyMap>()
                .keyboard
                .get_binding(GameInput::Escape);
            if let Some(xindeler_input::KeyOrMouse::Key(key)) = esc_key {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(key);
            }
            app.add_systems(
                Update,
                (
                    xindeler_input::action_state::update_action_state,
                    toggle_esc_menu,
                )
                    .chain(),
            );
            app.update();
            app.world_mut()
                .resource_mut::<Messages<HudAction>>()
                .drain()
                .collect()
        }

        assert_eq!(
            run(HudWindow::None),
            vec![HudAction::ToggleWindow(HudWindow::EscMenu)],
            "Escape with nothing open must summon the pause menu"
        );
        assert_eq!(
            run(HudWindow::EscMenu),
            vec![HudAction::CloseWindow],
            "Escape with the pause menu open must close it"
        );
        assert_eq!(
            run(HudWindow::Diary),
            vec![HudAction::CloseWindow],
            "Escape with another window (Diary) open must close it — Escape is the universal \
             back-out key, not just a pause-menu opener"
        );
    }

    /// BL-82 EM-5.19 Phase 3: while a hard lock is active, Escape must NOT
    /// open/close the pause menu at all —
    /// `targeting::clear_hard_lock_on_escape` (not exercised by this
    /// fixture; see `targeting.rs`'s own tests for that half) owns clearing
    /// the lock instead. Builds the REAL `.run_if(not(text_input_focused)).
    /// run_if(not(hard_lock_active))` chain
    /// (unlike `escape_opens_and_closes_only_when_appropriate` above, which
    /// calls the bare `toggle_esc_menu` function with no conditions) so this
    /// actually exercises the gating wiring, not just the predicate.
    #[test]
    fn escape_does_not_open_pause_menu_while_hard_lock_active() {
        use bevy::input_focus::InputFocus;
        use xindeler_input::KeyMap;

        use crate::targeting::HardLock;

        let mut app = App::new();
        app.init_resource::<HudState>();
        app.insert_resource(KeyMap::default());
        app.insert_resource(ActionState::default());
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        // `text_input_focused` (the other run condition in this chain) reads
        // `Res<InputFocus>` unconditionally — it must be present even though
        // this test's whole point is the `hard_lock_active` gate, not chat
        // focus (`chat::tests` is what actually exercises the focused case).
        app.init_resource::<InputFocus>();
        app.add_message::<HudAction>();
        let locked = app.world_mut().spawn_empty().id();
        app.insert_resource(HardLock(Some(locked)));

        let esc_key = app
            .world()
            .resource::<KeyMap>()
            .keyboard
            .get_binding(GameInput::Escape);
        if let Some(xindeler_input::KeyOrMouse::Key(key)) = esc_key {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
        }
        app.add_systems(
            Update,
            (
                xindeler_input::action_state::update_action_state,
                toggle_esc_menu
                    .run_if(not(text_input_focused))
                    .run_if(not(hard_lock_active)),
            )
                .chain(),
        );
        app.update();

        let actions: Vec<HudAction> = app
            .world_mut()
            .resource_mut::<Messages<HudAction>>()
            .drain()
            .collect();
        assert!(
            actions.is_empty(),
            "Escape while a hard lock is active must not emit any HudAction — no pause-menu \
             open/close on the same press"
        );
    }

    /// BL-82 EM-5.16 (T56.44): switching the active locale re-localizes the
    /// already-spawned Quit button live, using the REAL repo `.ftl` catalogs
    /// (not a synthetic fixture) via `VELOREN_ASSETS`/`XINDELER_ASSETS` — the
    /// same real-catalog proof `settings_window.rs`'s own hot-swap test uses,
    /// exercised here against a `LocalizedLabel`-tagged BUTTON (not a bare
    /// `LocalizedText` node), covering the other half of the T56.44 reactive
    /// chain (`relocalize_button_labels` + `button::spawn_button_labels`'s
    /// `Changed<HudButtonLabel>` propagation).
    #[test]
    fn switching_locale_relocalizes_the_quit_button_live() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            xindeler_ui::i18n::DEFAULT_HUD_FTL_FILES,
        ));
        app.init_resource::<xindeler_ui::i18n::CurrentLocale>();
        // `button::spawn_button_labels` is what turns `HudButtonLabel` into a
        // real `Text` child — needed for BOTH the initial spawn and the
        // post-hot-swap relabel this test drives.
        app.add_systems(Update, xindeler_ui::button::spawn_button_labels);

        app.world_mut()
            .run_system_once(spawn_esc_menu)
            .expect("spawn_esc_menu runs");
        app.update(); // let spawn_button_labels give the Quit button its child

        fn quit_button_text(app: &mut App) -> String {
            let world = app.world_mut();
            let button = world
                .query::<(&LocalizedLabel, &Children)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "esc_menu-quit_game")
                .map(|(_, children)| children[0])
                .expect("the Quit button was spawned and tagged");
            world
                .get::<Text>(button)
                .expect("label child exists")
                .0
                .clone()
        }

        assert_eq!(
            quit_button_text(&mut app),
            "Quit Game",
            "the Quit button must show the real en catalog text at spawn time"
        );

        app.world_mut()
            .resource_mut::<xindeler_ui::i18n::CurrentLocale>()
            .0 = "es".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_button_labels)
            .expect("relocalize runs");
        app.update(); // spawn_button_labels propagates the HudButtonLabel change onto Text

        let after = quit_button_text(&mut app);
        assert_eq!(
            after, "Salir del juego",
            "must resolve to the REAL es catalog's own esc_menu-quit_game value, not the en \
             fallback"
        );
    }
}
