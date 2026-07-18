//! BL-82 EM-5.9 (T56.29) — the main menu + first-run disclaimer + login screen.
//!
//! This is the client's FIRST real menu-vs-gameplay state machine. Until now
//! the client booted straight into gameplay (`--listen-server`) or the graphics
//! demo (no flags); there was no title screen. This module adds the
//! [`AppState::MainMenu`] → [`AppState::Connecting`] → [`AppState::InGame`]
//! flow, driving the REAL EM-4.2c handshake for the offline (singleplayer,
//! embedded-world) path.
//!
//! ## Screens (a single `MenuScreen` sub-state inside `AppState::MainMenu`)
//! 1. **Disclaimer** (first run only) — a one-time "this is pre-alpha" gate,
//!    persisted via [`xindeler_app::MenuSettings::disclaimer_accepted`] so it
//!    never shows again once accepted (legacy `voxygen`'s `show_disclaimer`).
//! 2. **Main** — title + Play / Options / Quit (legacy's main menu).
//! 3. **Login** — username / password / server-address fields + an
//!    Offline/Online toggle + Connect/Back, with a live error line (legacy's
//!    `LoginBanner`).
//!
//! ## Offline vs. online (how the real handshake distinguishes them)
//! - **Offline** (the default, and the path VERIFIED end-to-end): Connect boots
//!   the in-process embedded world via
//!   [`crate::listen_server::boot_offline_world`] — which runs the SAME real
//!   `client::Client::new` handshake the `--listen-server` bypass uses
//!   (username `listen_host`, empty password, against the auth-disabled
//!   singleplayer sim). No fake path: this is the genuine EM-4.2c offline
//!   handshake.
//! - **Online**: the username/password/server fields + toggle are fully built
//!   and validated, and the entered values persist — but the actual remote
//!   transport is **deferred to EM-5.9 T56.31** (the server browser). A
//!   menu-mode process pre-commits to the embedded (`bevy_replicon`
//!   SERVER-role) transport at build time, so it cannot also dial a remote
//!   server this task; Connect-while-Online therefore surfaces an honest notice
//!   instead of a fake connection. See the PR writeup for the full rationale
//!   (no auth server is reachable in this environment to exercise online
//!   anyway).
//!
//! ## Text input: a self-owned buffer, NOT `EditableText`
//! Identical reasoning to `chat.rs`: `bevy_ui_widgets`' `EditableText` flips
//! `ime_enabled` on focus, and on macOS the composed characters never reach
//! `value()`. So the login fields fold real [`KeyboardInput`] into plain
//! `String`s directly (see [`read_login_input`]), exactly like the chat line.
//!
//! Compiled only under the `listen-server` feature (it is the offline-world
//! host the menu drives); a no-feature build keeps booting straight into the
//! demo scene.

use bevy::{
    input::keyboard::{Key, KeyboardInput},
    prelude::*,
};
use xindeler_app::{AppState, XindelerSettings};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::HudState,
    panel::panel_bundle,
    theme::{HudFonts, HudTheme},
    zlayer,
};

/// Installs the main-menu / disclaimer / login flow + the connecting-screen
/// transition. Every system is gated on the relevant [`AppState`], so none of
/// this runs during actual gameplay.
pub struct MainMenuPlugin;

impl Plugin for MainMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuScreen>()
            .init_resource::<LoginForm>()
            .init_resource::<ConnectProgress>()
            .add_systems(OnEnter(AppState::MainMenu), enter_main_menu)
            .add_systems(OnExit(AppState::MainMenu), despawn_menu_root)
            .add_systems(OnEnter(AppState::Connecting), enter_connecting)
            .add_systems(OnExit(AppState::Connecting), despawn_connecting_root)
            .add_systems(OnEnter(AppState::InGame), close_hud_windows_on_enter_game)
            .add_systems(
                Update,
                (
                    build_menu,
                    read_login_input,
                    render_login_fields.after(build_menu),
                    render_status_line.after(build_menu),
                )
                    .run_if(in_state(AppState::MainMenu)),
            )
            // The offline world boot runs as an exclusive `&mut World` system
            // (it inserts the sim/player as non-send resources and blocks for
            // several seconds). It gets its own `add_systems` call because an
            // exclusive system can't share a tuple with the parallel ones above.
            .add_systems(
                Update,
                drive_connecting.run_if(in_state(AppState::Connecting)),
            );

        // BL-82 EM-5.9 (T56.29) verification hook: when
        // `XINDELER_SMOKE_AUTOCONNECT=1`, auto-trigger an offline Connect a few
        // frames after the menu appears so a `--smoke-screenshot` run captures
        // the whole menu → Connecting → in-game chain without synthetic clicks
        // (see `main.rs`'s `world_boots`).
        if std::env::var("XINDELER_SMOKE_AUTOCONNECT").is_ok() {
            app.add_systems(
                Update,
                smoke_autoconnect.run_if(in_state(AppState::MainMenu)),
            );
        }
    }
}

/// The `XINDELER_SMOKE_AUTOCONNECT` hook: after the menu has painted a few
/// frames, force Offline mode and Connect — driving the real menu → Connecting
/// → in-game transition for the screenshot harness.
fn smoke_autoconnect(
    mut form: ResMut<LoginForm>,
    mut settings: ResMut<XindelerSettings>,
    mut next: ResMut<NextState<AppState>>,
    mut frames: Local<u32>,
) {
    *frames += 1;
    // Let a couple of menu frames render before auto-connecting.
    if *frames != 20 {
        return;
    }
    form.online = false;
    attempt_connect(&mut form, &mut settings, &mut next);
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Which sub-screen of [`AppState::MainMenu`] is showing.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
enum MenuScreen {
    /// First-run pre-alpha disclaimer gate (skipped once accepted).
    #[default]
    Disclaimer,
    /// Title + Play / Options / Quit.
    Main,
    /// Username / password / server + Offline/Online toggle + Connect/Back.
    Login,
}

/// Which login field currently has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoginField {
    Username,
    Password,
    Server,
}

impl LoginField {
    /// Tab-cycle order.
    const ORDER: [LoginField; 3] = [
        LoginField::Username,
        LoginField::Password,
        LoginField::Server,
    ];
}

/// The login form's live state — self-owned text buffers (see the module doc
/// comment for why not `EditableText`) plus the mode toggle and the current
/// error/notice line.
#[derive(Resource, Debug, Default)]
struct LoginForm {
    username: String,
    password: String,
    server: String,
    /// `true` = Online (multiplayer), `false` = Offline (singleplayer
    /// embedded).
    online: bool,
    /// Which field the typed characters go into (`None` = nothing focused).
    focused: Option<LoginField>,
    /// The status/error line shown under the form (also used for the Options
    /// stub notice on the Main screen).
    error: Option<String>,
}

impl LoginForm {
    /// The mutable buffer for a field.
    fn buffer_mut(&mut self, field: LoginField) -> &mut String {
        match field {
            LoginField::Username => &mut self.username,
            LoginField::Password => &mut self.password,
            LoginField::Server => &mut self.server,
        }
    }

    /// Inserts `s` (control-char-filtered) into the focused field, if any.
    fn insert_focused(&mut self, s: &str) {
        let Some(field) = self.focused else { return };
        let filtered: String = s.chars().filter(|c| !c.is_control()).collect();
        if filtered.is_empty() {
            return;
        }
        self.buffer_mut(field).push_str(&filtered);
    }

    /// Deletes the last character of the focused field, if any.
    fn backspace_focused(&mut self) {
        let Some(field) = self.focused else { return };
        self.buffer_mut(field).pop();
    }

    /// Advances focus to the next field (Tab), wrapping, starting at the first
    /// field when nothing is focused yet.
    fn cycle_focus(&mut self) {
        let next = match self.focused {
            None => LoginField::ORDER[0],
            Some(current) => {
                let idx = LoginField::ORDER
                    .iter()
                    .position(|f| *f == current)
                    .unwrap_or(0);
                LoginField::ORDER[(idx + 1) % LoginField::ORDER.len()]
            },
        };
        self.focused = Some(next);
    }
}

/// Per-connection-attempt progress on the [`AppState::Connecting`] screen.
/// Reset on `OnEnter(Connecting)`; drives the deferred boot in
/// [`drive_connecting`].
#[derive(Resource, Debug, Default)]
struct ConnectProgress {
    frames: u32,
    done: bool,
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// The full-screen menu backdrop + panel (rebuilt whenever [`MenuScreen`]
/// changes).
#[derive(Component)]
struct MenuRoot;

/// The on-screen text node mirroring a login field's buffer.
#[derive(Component, Clone, Copy)]
struct FieldText(LoginField);

/// A login field's bordered box (its border highlights when focused).
#[derive(Component, Clone, Copy)]
struct FieldBox(LoginField);

/// The Offline/Online toggle button's label (relabelled from [`LoginForm`]).
#[derive(Component)]
struct ModeToggleLabel;

/// The status/error line under the form.
#[derive(Component)]
struct StatusLineText;

/// The transitional "Connecting…" screen root.
#[derive(Component)]
struct ConnectingRoot;

// ---------------------------------------------------------------------------
// Enter / exit
// ---------------------------------------------------------------------------

/// Seeds the login form from persisted settings and picks the entry sub-screen
/// (Disclaimer on first run, otherwise straight to Main) — but ONLY on the
/// first entry to [`AppState::MainMenu`]. A later return (e.g. a failed offline
/// boot bouncing back from [`AppState::Connecting`]) preserves the in-memory
/// form + screen + error [`drive_connecting`] set, so the boot error stays
/// visible on the Login screen rather than being reset away.
fn enter_main_menu(
    settings: Res<XindelerSettings>,
    mut form: ResMut<LoginForm>,
    mut screen: ResMut<MenuScreen>,
    mut initialized: Local<bool>,
) {
    // TODO(EM-5.x): once an in-game "return to main menu" (logout) path exists,
    // reset this latch on `InGame → MainMenu` so the form re-seeds from
    // freshly-loaded settings. Today there is no such path, and keeping the
    // latch is what preserves a failed-boot error bouncing back from
    // `Connecting` (see `drive_connecting`).
    if *initialized {
        return;
    }
    *initialized = true;
    form.username = settings.menu.username.clone();
    form.server = settings.menu.server_address.clone();
    form.online = settings.menu.online;
    form.focused = None;
    form.error = None;
    *screen = if settings.menu.disclaimer_accepted {
        MenuScreen::Main
    } else {
        MenuScreen::Disclaimer
    };
    // Debug/verification hook: force the initial sub-screen so a
    // `--smoke-screenshot` run can capture each screen (disclaimer/main/login)
    // without synthetic clicks. No effect unless the env var is set.
    if let Ok(force) = std::env::var("XINDELER_SMOKE_MENU_SCREEN") {
        *screen = match force.as_str() {
            "main" => MenuScreen::Main,
            "login" => MenuScreen::Login,
            _ => MenuScreen::Disclaimer,
        };
    }
}

/// Tears the whole menu tree down when leaving [`AppState::MainMenu`].
fn despawn_menu_root(mut commands: Commands, roots: Query<Entity, With<MenuRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

/// Spawns the "Connecting…" placeholder and resets the boot progress counter.
fn enter_connecting(
    mut commands: Commands,
    theme: Option<Res<HudTheme>>,
    fonts: Option<Res<HudFonts>>,
    form: Res<LoginForm>,
    mut progress: ResMut<ConnectProgress>,
) {
    *progress = ConnectProgress::default();
    let (Some(theme), Some(fonts)) = (theme, fonts) else {
        return;
    };
    let theme: HudTheme = *theme;
    let msg = if form.online {
        "Connecting to server…".to_owned()
    } else {
        "Generating your world…\nThis can take several seconds on first launch.".to_owned()
    };
    commands
        .spawn((
            ConnectingRoot,
            // Above every HUD layer so it fully covers the (empty) gameplay
            // chrome that spawned behind it.
            GlobalZIndex(zlayer::TOAST + 100),
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
            BackgroundColor(MENU_BACKDROP),
        ))
        .with_children(|screen| {
            screen.spawn((
                Text(msg),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.title.clone()),
                    font_size: bevy::text::FontSize::Px(24.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
                Node {
                    max_width: Val::Px(520.0),
                    ..Default::default()
                },
            ));
        });
}

/// Removes the "Connecting…" placeholder when leaving [`AppState::Connecting`].
fn despawn_connecting_root(mut commands: Commands, roots: Query<Entity, With<ConnectingRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

/// Safety net: on entering gameplay, close any HUD window a stray hotkey might
/// have toggled open behind the (opaque) menu while it was up.
fn close_hud_windows_on_enter_game(hud_state: Option<ResMut<HudState>>) {
    if let Some(mut hud_state) = hud_state {
        hud_state.close();
    }
}

// ---------------------------------------------------------------------------
// The connecting flow (deferred offline boot)
// ---------------------------------------------------------------------------

/// Drives the offline connection: waits a couple of frames so the "Connecting…"
/// screen actually paints, then boots the embedded world (the real EM-4.2c
/// offline handshake) and transitions to gameplay — or back to the login screen
/// with a real error if the world can't boot.
///
/// An exclusive `&mut World` system:
/// [`crate::listen_server::boot_offline_world`] inserts the sim/player as
/// non-send resources and blocks for several seconds.
fn drive_connecting(world: &mut World) {
    let (frames, done, online) = {
        let progress = world.resource::<ConnectProgress>();
        let online = world.resource::<LoginForm>().online;
        (progress.frames, progress.done, online)
    };
    if done {
        return;
    }
    // Online never reaches here (Connect stays on the login screen for online —
    // see `attempt_connect`); guard anyway so a future online path can't
    // silently boot an embedded world.
    if online {
        world.resource_mut::<ConnectProgress>().done = true;
        world
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
        return;
    }
    // Let the "Connecting…" frame render before the multi-second blocking boot.
    if frames < 2 {
        world.resource_mut::<ConnectProgress>().frames = frames + 1;
        return;
    }
    world.resource_mut::<ConnectProgress>().done = true;

    let booted = crate::listen_server::boot_offline_world(world);
    if booted {
        world
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
    } else {
        {
            let mut form = world.resource_mut::<LoginForm>();
            form.error = Some(
                "Could not start a world (missing assets or map data). Check XINDELER_ASSETS / \
                 the LFS map blobs and try again."
                    .to_owned(),
            );
        }
        *world.resource_mut::<MenuScreen>() = MenuScreen::Login;
        world
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
    }
}

// ---------------------------------------------------------------------------
// Build (spawn the current screen)
// ---------------------------------------------------------------------------

/// Fully opaque menu backdrop (covers the empty gameplay chrome behind it).
const MENU_BACKDROP: Color = Color::srgb(0.04, 0.05, 0.08);

/// (Re)builds the menu tree whenever the current [`MenuScreen`] changes (or the
/// root is missing, e.g. on first entry or after a return to the menu). Keeps a
/// `Local` of the last-built screen so it doesn't rebuild every frame.
fn build_menu(
    mut commands: Commands,
    theme: Option<Res<HudTheme>>,
    fonts: Option<Res<HudFonts>>,
    screen: Res<MenuScreen>,
    form: Res<LoginForm>,
    roots: Query<Entity, With<MenuRoot>>,
    mut last_built: Local<Option<MenuScreen>>,
) {
    let root_exists = !roots.is_empty();
    if root_exists && *last_built == Some(*screen) {
        return;
    }
    let (Some(theme), Some(fonts)) = (theme, fonts) else {
        return;
    };
    let theme: HudTheme = *theme;

    for root in &roots {
        commands.entity(root).despawn();
    }

    let mut root = commands.spawn((
        MenuRoot,
        GlobalZIndex(zlayer::TOAST + 100),
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
        BackgroundColor(MENU_BACKDROP),
    ));
    root.with_children(|screen_node| {
        let mut panel_entity = screen_node.spawn(panel_bundle(&theme));
        let row_gap_px = theme.spacing.md;
        panel_entity.entry::<Node>().and_modify(move |mut node| {
            node.flex_direction = FlexDirection::Column;
            node.row_gap = Val::Px(row_gap_px);
            node.min_width = Val::Px(420.0);
            node.max_width = Val::Px(560.0);
            node.align_items = AlignItems::Stretch;
        });
        panel_entity.with_children(|panel| match *screen {
            MenuScreen::Disclaimer => spawn_disclaimer(panel, &theme, &fonts),
            MenuScreen::Main => spawn_main(panel, &theme, &fonts),
            MenuScreen::Login => spawn_login(panel, &theme, &fonts, &form),
        });
    });

    *last_built = Some(*screen);
}

/// A section/title heading line.
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

/// A body-text paragraph.
fn body_text(
    panel: &mut ChildSpawnerCommands,
    fonts: &HudFonts,
    color: Color,
    text: &str,
    size: f32,
) {
    panel.spawn((
        Text(text.to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(size),
            ..Default::default()
        },
        TextColor(color),
    ));
}

/// The first-run pre-alpha disclaimer.
fn spawn_disclaimer(panel: &mut ChildSpawnerCommands, theme: &HudTheme, fonts: &HudFonts) {
    heading(panel, fonts, theme, "Xindeler", 34.0);
    heading(panel, fonts, theme, "Disclaimer", 22.0);
    body_text(
        panel,
        fonts,
        theme.palette.text,
        "Xindeler is early, pre-alpha software built on the Veloren engine. Expect bugs, missing \
         features, placeholder art, and changes that can wipe characters and worlds between \
         builds. Nothing here is final.\n\nBy continuing you acknowledge this is an unfinished \
         work in progress.",
        15.0,
    );
    panel
        .spawn(button_bundle(theme, fonts, "I understand — continue"))
        .observe(accept_disclaimer);
    panel
        .spawn(button_bundle(theme, fonts, "Quit"))
        .observe(quit_game);
}

/// The main menu: Play / Options / Quit.
fn spawn_main(panel: &mut ChildSpawnerCommands, theme: &HudTheme, fonts: &HudFonts) {
    heading(panel, fonts, theme, "Xindeler", 40.0);
    body_text(
        panel,
        fonts,
        theme.palette.text_muted,
        "A voxel RPG — BL-82 Bevy client",
        14.0,
    );
    panel
        .spawn(button_bundle(theme, fonts, "Play"))
        .observe(go_to_login);
    panel
        .spawn(button_bundle(theme, fonts, "Options"))
        .observe(options_notice);
    panel
        .spawn(button_bundle(theme, fonts, "Quit"))
        .observe(quit_game);
    status_line(panel, fonts, theme);
}

/// The login screen: mode toggle + username / password / server + Connect/Back.
fn spawn_login(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    form: &LoginForm,
) {
    heading(panel, fonts, theme, "Play", 30.0);

    // Offline/Online mode toggle.
    panel
        .spawn(button_bundle(theme, fonts, mode_label(form.online)))
        .insert(ModeToggleLabel)
        .observe(toggle_mode);

    login_field(panel, theme, fonts, LoginField::Username, "Username");
    login_field(panel, theme, fonts, LoginField::Password, "Password");
    login_field(panel, theme, fonts, LoginField::Server, "Server address");

    body_text(
        panel,
        fonts,
        theme.palette.text_muted,
        "Offline hosts a private singleplayer world. Online multiplayer connects via the in-game \
         server browser (coming soon).",
        12.0,
    );

    panel
        .spawn(button_bundle(theme, fonts, "Connect"))
        .observe(connect_clicked);
    panel
        .spawn(button_bundle(theme, fonts, "Back"))
        .observe(back_to_main);
    status_line(panel, fonts, theme);
}

/// A labelled, focusable login field (label + bordered value box). Clicking the
/// box focuses it; [`render_login_fields`] mirrors the buffer into it.
fn login_field(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    field: LoginField,
    label: &str,
) {
    body_text(panel, fonts, theme.palette.text_muted, label, 13.0);
    panel
        .spawn((
            FieldBox(field),
            // `bevy_ui`'s picking backend treats a node WITHOUT a `Pickable`
            // component as pickable (see `combat_hud`'s own note), so this box
            // receives `Pointer<Click>` for focus without one.
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(theme.spacing.sm), Val::Px(theme.spacing.xs)),
                border: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
                min_height: Val::Px(28.0),
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            bevy::ui::BorderColor::all(theme.palette.panel_border),
        ))
        .observe(move |_: On<Pointer<Click>>, mut form: ResMut<LoginForm>| {
            form.focused = Some(field);
        })
        .with_children(|b| {
            b.spawn((
                FieldText(field),
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        });
}

/// The shared status/error line under a screen's controls.
fn status_line(panel: &mut ChildSpawnerCommands, fonts: &HudFonts, theme: &HudTheme) {
    panel.spawn((
        StatusLineText,
        Text(String::new()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(14.0),
            ..Default::default()
        },
        TextColor(theme.palette.danger),
    ));
}

fn mode_label(online: bool) -> &'static str {
    if online {
        "Mode: Online (multiplayer)"
    } else {
        "Mode: Offline (singleplayer)"
    }
}

// ---------------------------------------------------------------------------
// Rendering (mirror state -> nodes)
// ---------------------------------------------------------------------------

/// Mirrors the [`LoginForm`] buffers into the on-screen field text (password
/// masked, focused field gets a caret + accent border, empty unfocused fields
/// show a placeholder) and relabels the mode toggle.
fn render_login_fields(
    form: Res<LoginForm>,
    theme: Option<Res<HudTheme>>,
    mut fields: Query<(&FieldText, &mut Text), Without<ModeToggleLabel>>,
    mut boxes: Query<(&FieldBox, &mut bevy::ui::BorderColor)>,
    toggles: Query<&Children, With<ModeToggleLabel>>,
    mut toggle_texts: Query<&mut Text, (Without<FieldText>, Without<StatusLineText>)>,
) {
    let Some(theme) = theme else { return };
    for (field, mut text) in &mut fields {
        let focused = form.focused == Some(field.0);
        let display = match field.0 {
            LoginField::Password => "•".repeat(form.password.chars().count()),
            LoginField::Username => form.username.clone(),
            LoginField::Server => form.server.clone(),
        };
        let new = if display.is_empty() && !focused {
            placeholder(field.0).to_owned()
        } else if focused {
            format!("{display}_")
        } else {
            display
        };
        if text.0 != new {
            text.0 = new;
        }
    }
    for (field, mut border) in &mut boxes {
        let colour = if form.focused == Some(field.0) {
            theme.palette.accent
        } else {
            theme.palette.panel_border
        };
        *border = bevy::ui::BorderColor::all(colour);
    }
    for children in &toggles {
        for &child in children {
            if let Ok(mut text) = toggle_texts.get_mut(child) {
                let label = mode_label(form.online).to_owned();
                if text.0 != label {
                    text.0 = label;
                }
            }
        }
    }
}

fn placeholder(field: LoginField) -> &'static str {
    match field {
        LoginField::Username => "(click to type a username)",
        LoginField::Password => "(optional — for authenticated servers)",
        LoginField::Server => "(offline: not needed)",
    }
}

/// Mirrors [`LoginForm::error`] into the status line.
fn render_status_line(form: Res<LoginForm>, mut lines: Query<&mut Text, With<StatusLineText>>) {
    let msg = form.error.clone().unwrap_or_default();
    for mut text in &mut lines {
        if text.0 != msg {
            text.0 = msg.clone();
        }
    }
}

// ---------------------------------------------------------------------------
// Keyboard input (login fields)
// ---------------------------------------------------------------------------

/// Folds real [`KeyboardInput`] into the focused login field (see the module
/// doc comment for why not `EditableText`): Tab cycles fields, Enter connects,
/// Backspace deletes, Escape returns to the main menu, printable characters
/// insert. No-ops unless the Login sub-screen is showing.
fn read_login_input(
    mut keyboard: MessageReader<KeyboardInput>,
    mut screen: ResMut<MenuScreen>,
    mut form: ResMut<LoginForm>,
    mut settings: ResMut<XindelerSettings>,
    mut next: ResMut<NextState<AppState>>,
) {
    if *screen != MenuScreen::Login {
        keyboard.read().for_each(drop);
        return;
    }
    for ev in keyboard.read() {
        if !ev.state.is_pressed() {
            continue;
        }
        match &ev.logical_key {
            Key::Tab => form.cycle_focus(),
            Key::Enter => attempt_connect(&mut form, &mut settings, &mut next),
            Key::Escape => {
                form.error = None;
                form.focused = None;
                *screen = MenuScreen::Main;
            },
            Key::Backspace => form.backspace_focused(),
            Key::Space => form.insert_focused(" "),
            Key::Character(s) => form.insert_focused(s.as_str()),
            _ => {},
        }
    }
}

// ---------------------------------------------------------------------------
// Button actions
// ---------------------------------------------------------------------------

fn accept_disclaimer(
    _activate: On<Activate>,
    mut settings: ResMut<XindelerSettings>,
    mut screen: ResMut<MenuScreen>,
) {
    settings.menu.disclaimer_accepted = true;
    if let Err(err) = settings.save() {
        error!("main menu: failed to persist disclaimer acknowledgement: {err}");
    }
    *screen = MenuScreen::Main;
}

fn go_to_login(
    _activate: On<Activate>,
    mut screen: ResMut<MenuScreen>,
    mut form: ResMut<LoginForm>,
) {
    form.error = None;
    *screen = MenuScreen::Login;
}

fn back_to_main(
    _activate: On<Activate>,
    mut screen: ResMut<MenuScreen>,
    mut form: ResMut<LoginForm>,
) {
    form.error = None;
    form.focused = None;
    *screen = MenuScreen::Main;
}

fn options_notice(_activate: On<Activate>, mut form: ResMut<LoginForm>) {
    form.error = Some(
        "Settings live in the in-game Esc menu (a full settings screen is EM-5.12).".to_owned(),
    );
}

fn toggle_mode(_activate: On<Activate>, mut form: ResMut<LoginForm>) {
    form.online = !form.online;
    form.error = None;
}

fn quit_game(_activate: On<Activate>, mut commands: Commands) {
    commands.write_message(AppExit::Success);
}

fn connect_clicked(
    _activate: On<Activate>,
    mut form: ResMut<LoginForm>,
    mut settings: ResMut<XindelerSettings>,
    mut next: ResMut<NextState<AppState>>,
) {
    attempt_connect(&mut form, &mut settings, &mut next);
}

/// The shared Connect handler (Connect button + Enter). Persists the entered
/// fields, then either kicks off the offline world boot (→
/// [`AppState::Connecting`]) or, for online, surfaces the honest
/// deferred-transport notice.
fn attempt_connect(
    form: &mut LoginForm,
    settings: &mut XindelerSettings,
    next: &mut NextState<AppState>,
) {
    settings.menu.username = form.username.clone();
    settings.menu.server_address = form.server.clone();
    settings.menu.online = form.online;
    if let Err(err) = settings.save() {
        error!("main menu: failed to persist login fields: {err}");
    }

    if form.online {
        // The online UI/validation path is fully wired; the remote transport is
        // deferred to the server browser (EM-5.9 T56.31) — a menu-mode process
        // pre-commits to the embedded transport at build time, so it cannot
        // also dial a remote server here. Surface an honest notice rather than
        // a fake connection (see the module doc comment).
        if form.server.trim().is_empty() {
            form.error = Some("Enter a server address (or switch to Offline).".to_owned());
        } else {
            form.error = Some(
                "Online multiplayer connects via the in-game server browser (EM-5.9 T56.31). This \
                 build hosts a singleplayer world — switch to Offline to play now."
                    .to_owned(),
            );
        }
        return;
    }

    form.error = None;
    next.set(AppState::Connecting);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_form_focus_cycles_and_edits() {
        let mut form = LoginForm::default();
        assert_eq!(form.focused, None);
        form.cycle_focus();
        assert_eq!(form.focused, Some(LoginField::Username));
        form.insert_focused("mati");
        assert_eq!(form.username, "mati");
        form.backspace_focused();
        assert_eq!(form.username, "mat");

        form.cycle_focus();
        assert_eq!(form.focused, Some(LoginField::Password));
        form.cycle_focus();
        assert_eq!(form.focused, Some(LoginField::Server));
        // Wraps back to the first field.
        form.cycle_focus();
        assert_eq!(form.focused, Some(LoginField::Username));
    }

    #[test]
    fn insert_filters_control_characters() {
        let mut form = LoginForm {
            focused: Some(LoginField::Server),
            ..Default::default()
        };
        form.insert_focused("host\n\t:14004");
        assert_eq!(form.server, "host:14004");
    }

    /// Offline Connect requests the Connecting state (the real boot happens
    /// there); it must NOT stay on the menu.
    #[test]
    fn offline_connect_requests_the_connecting_state() {
        let mut form = LoginForm {
            online: false,
            ..Default::default()
        };
        let mut settings = XindelerSettings::default();
        let mut next = NextState::<AppState>::default();

        attempt_connect(&mut form, &mut settings, &mut next);

        assert!(
            matches!(next, NextState::Pending(AppState::Connecting)),
            "offline Connect must transition to the Connecting state"
        );
        assert!(
            form.error.is_none(),
            "a clean offline Connect shows no error"
        );
    }

    /// Online Connect stays on the menu (transport deferred) and surfaces a
    /// real notice — it must never silently boot an embedded world.
    #[test]
    fn online_connect_stays_on_the_menu_with_a_notice() {
        let mut form = LoginForm {
            online: true,
            server: "play.example.com:14004".to_owned(),
            ..Default::default()
        };
        let mut settings = XindelerSettings::default();
        let mut next = NextState::<AppState>::default();

        attempt_connect(&mut form, &mut settings, &mut next);

        assert!(
            matches!(next, NextState::Unchanged),
            "online Connect must NOT change AppState (remote transport is deferred)"
        );
        assert!(
            form.error.is_some(),
            "online Connect must surface a notice, not fail silently"
        );
    }

    /// Empty server address on online mode is a distinct, real validation
    /// error.
    #[test]
    fn online_connect_with_empty_server_is_a_validation_error() {
        let mut form = LoginForm {
            online: true,
            server: "   ".to_owned(),
            ..Default::default()
        };
        let mut settings = XindelerSettings::default();
        let mut next = NextState::<AppState>::default();

        attempt_connect(&mut form, &mut settings, &mut next);
        assert!(
            form.error
                .as_deref()
                .unwrap_or_default()
                .contains("server address"),
            "an empty online server address must produce a validation error"
        );
    }
}
