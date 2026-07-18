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

use std::{
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bevy::{
    input::keyboard::{Key, KeyboardInput},
    prelude::*,
    tasks::{IoTaskPool, Task, block_on},
};
use serde::Deserialize;
use xindeler_app::{AppState, SavedServer, XindelerSettings};
use xindeler_sim_bridge::{ConnectStage, EmbeddedPlayer, SimServer};
use xindeler_ui::{
    button::{Activate, HudButtonImages, button_bundle, image_button_bundle},
    hud_state::HudState,
    panel::panel_bundle,
    scroll::scroll_view_bundle,
    theme::{HudFonts, HudTheme},
    zlayer,
};

/// What the offline-world boot thread produces (BL-82 EM-5.9 T56.30):
/// `Err` = the world couldn't boot; `Ok((sim, Some(player)))` = full boot;
/// `Ok((sim, None))` = spectator fallback (sim up, no controllable player).
type BootOutcome = Result<(SimServer, Option<EmbeddedPlayer>), String>;

/// Installs the main-menu / disclaimer / login flow + the connecting-screen
/// transition. Every system is gated on the relevant [`AppState`], so none of
/// this runs during actual gameplay.
pub struct MainMenuPlugin;

impl Plugin for MainMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuScreen>()
            .init_resource::<LoginForm>()
            .init_resource::<ServerBrowser>()
            .init_resource::<LoadingTips>()
            .init_resource::<Credits>()
            // Load the data-driven tips + credits once at startup (RON under
            // `assets/xindeler/ui/`); the loading screen reads them each connect.
            .add_systems(Startup, load_loading_screen_assets)
            // BL-82 main-menu visual parity: load the real, pre-existing legacy
            // main-menu chrome art (background, logo, button + input textures)
            // once at startup so the menu matches the frozen `voxygen/src/menu/`
            // reference instead of the flat placeholder it shipped with.
            .add_systems(Startup, load_menu_images)
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
                    read_browser_input,
                    render_login_fields.after(build_menu),
                    render_status_line.after(build_menu),
                    render_browser_fields.after(build_menu),
                    // The server-browser concurrency + live list (BL-82 EM-5.9
                    // T56.31): kick off / drain the async ping/version queries,
                    // (re)build the row list when it structurally changes, and
                    // repaint each row's live status text every frame.
                    service_server_queries,
                    rebuild_server_list.after(build_menu),
                    render_server_rows
                        .after(rebuild_server_list)
                        .after(service_server_queries),
                )
                    .run_if(in_state(AppState::MainMenu)),
            )
            // Live loading-screen paint (stage text, real progress bar, spinner,
            // MOTD) — a parallel system reading the shared boot-progress cell.
            .add_systems(
                Update,
                render_connecting.run_if(in_state(AppState::Connecting)),
            )
            // The boot handoff runs as an exclusive `&mut World` system: it
            // inserts the booted sim/player as non-send resources and drives the
            // Connecting → InGame transition. Exclusive systems can't share a
            // tuple, and it runs AFTER the paint so the freshly-reported stage
            // is on screen the same frame. The multi-second boot itself now runs
            // on a background thread (see `enter_connecting`), so this stays
            // cheap (a poll) and the window keeps rendering throughout.
            .add_systems(
                Update,
                drive_connecting
                    .run_if(in_state(AppState::Connecting))
                    .after(render_connecting),
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
    /// BL-82 EM-5.9 (T56.31) — the multiplayer server browser: the saved-server
    /// list with live-queried ping/version/player-count, an add/delete flow, a
    /// refresh action, and a Connect that drives the chosen server through the
    /// login connect path.
    ServerBrowser,
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

// ---------------------------------------------------------------------------
// Server browser (BL-82 EM-5.9 T56.31) — concurrent ping/version querying
// ---------------------------------------------------------------------------
//
// This is the multiplayer half of the login flow. The old `voxygen` "server
// browser" was only a persisted `Vec<String>` of addresses with NO live query
// (you learned a server's version solely by fully connecting); this rebuilds it
// properly: a saved list whose ping/version/player-count/MOTD are queried
// CONCURRENTLY off the main thread and painted into each row AS its own probe
// completes (never blocking on the slowest server).
//
// ## What a probe can honestly learn (see `probe_server`)
// The new game transport is QUIC/`bevy_replicon` and exposes NO lightweight
// status query — a server's version/player-count is knowable only by completing
// the full login handshake, which the menu-mode process cannot do (it is
// build-time-committed to the embedded SERVER-role transport; see #166/#167 and
// this module's doc comment). So the v1 probe measures what IS universally
// available — reachability + ping, via a real TCP connect — and additionally
// parses an OPTIONAL one-line status banner (version/players/MOTD) if the peer
// sends one. Real Xindeler servers don't answer that banner yet (a tiny
// side-channel status responder is the natural follow-up), so today they'd show
// ping only; a local listener that DOES answer it shows the full row — which is
// exactly how the concurrency + parse path is exercised by the tests.

/// The default game port appended to a saved address that omits one (legacy
/// `voxygen`'s default game port; kept identical so a bare hostname resolves
/// the same way it did in the old client).
const DEFAULT_GAME_PORT: u16 = 14004;

/// How long a single probe waits for the TCP connection to establish before
/// reporting the server unreachable.
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// After connecting, how long the probe waits for the OPTIONAL one-line status
/// banner. Short: real game servers don't send one, so on timeout the probe
/// still returns a good ping — just without the extra fields.
const PROBE_STATUS_READ_TIMEOUT: Duration = Duration::from_millis(400);

/// The live, queried status of one server (populated asynchronously as its
/// probe completes). Everything past `ping_ms` is optional because the only
/// thing the v1 probe learns from ANY reachable endpoint is the ping;
/// version/players/MOTD arrive only if the server answers the status banner.
#[derive(Clone, Debug, Default, PartialEq)]
struct ServerStatus {
    /// Round-trip time of the TCP connect, in milliseconds.
    ping_ms: Option<u32>,
    /// The server's reported version string, if it answered the status banner.
    version: Option<String>,
    /// `(online, cap)` player counts, if reported.
    players: Option<(u32, u32)>,
    /// The server's message-of-the-day, if reported.
    motd: Option<String>,
}

/// The probe outcome for one server: `Ok` = reachable (with whatever it told
/// us), `Err` = unreachable / resolve failure (a human-readable reason).
type ProbeResult = Result<ServerStatus, String>;

/// The optional one-line status banner a server MAY send right after accepting
/// the probe's TCP connection, as a single line of RON:
/// `(version:"0.1.0",players:Some((3,20)),motd:"Welcome!")`. Every field is
/// `#[serde(default)]` so a partial/empty banner still parses.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ServerStatusBanner {
    version: String,
    players: Option<(u32, u32)>,
    motd: String,
}

/// The per-row query lifecycle.
#[derive(Clone, Debug, Default, PartialEq)]
enum QueryState {
    /// Not yet queried this session.
    #[default]
    Idle,
    /// A probe task is in flight.
    Querying,
    /// The probe finished with this result.
    Done(ProbeResult),
}

/// One row of the browser: the saved identity + its live query state.
#[derive(Clone, Debug)]
struct ServerRow {
    address: String,
    nickname: String,
    state: QueryState,
}

/// Which add-server field currently has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AddField {
    Address,
    Nickname,
}

impl AddField {
    const ORDER: [AddField; 2] = [AddField::Address, AddField::Nickname];
}

/// The whole in-memory server-browser state: the working row list (a live copy
/// of `settings.menu.servers` plus each row's query state), the in-flight probe
/// tasks (one optional slot per row, index-aligned), the selected row, the
/// add-server form buffers, and a structural revision the list-rebuild system
/// watches. Only the addresses/nicknames are persisted (via
/// `settings.menu.servers`); the live query fields are re-queried each session.
#[derive(Resource, Default)]
struct ServerBrowser {
    rows: Vec<ServerRow>,
    /// One optional in-flight probe per row, index-aligned with `rows`.
    tasks: Vec<Option<Task<ProbeResult>>>,
    /// The selected row index (what Connect / Delete act on).
    selected: Option<usize>,
    /// Add-server form buffers + which field has focus.
    add_address: String,
    add_nickname: String,
    add_focused: Option<AddField>,
    /// Bumped whenever the row SET changes (load/add/delete) so the list-
    /// rebuild system respawns the row nodes. Status updates within a row do
    /// NOT bump this — they repaint text in place.
    list_revision: u64,
    /// Set when the browser (re)opens or Refresh is pressed: the next service
    /// tick fires a fresh probe for every row.
    needs_refresh: bool,
    /// The browser's own status/notice line.
    status: Option<String>,
}

impl ServerBrowser {
    /// Reloads the working row list from persisted settings (called when the
    /// browser opens), resetting query state and requesting a fresh round of
    /// probes.
    fn load_from(&mut self, servers: &[SavedServer]) {
        self.rows = servers
            .iter()
            .map(|s| ServerRow {
                address: s.address.clone(),
                nickname: s.nickname.clone(),
                state: QueryState::Idle,
            })
            .collect();
        self.tasks = (0..self.rows.len()).map(|_| None).collect();
        self.selected = (!self.rows.is_empty()).then_some(0);
        self.list_revision = self.list_revision.wrapping_add(1);
        self.needs_refresh = true;
        self.status = None;
    }

    /// The add-form buffer for a field.
    fn add_buffer_mut(&mut self, field: AddField) -> &mut String {
        match field {
            AddField::Address => &mut self.add_address,
            AddField::Nickname => &mut self.add_nickname,
        }
    }

    /// Inserts `s` (control-char-filtered) into the focused add-form field.
    fn insert_add(&mut self, s: &str) {
        let Some(field) = self.add_focused else {
            return;
        };
        let filtered: String = s.chars().filter(|c| !c.is_control()).collect();
        if filtered.is_empty() {
            return;
        }
        self.add_buffer_mut(field).push_str(&filtered);
    }

    /// Deletes the last character of the focused add-form field.
    fn backspace_add(&mut self) {
        let Some(field) = self.add_focused else {
            return;
        };
        self.add_buffer_mut(field).pop();
    }

    /// Advances add-form focus to the next field (Tab), wrapping.
    fn cycle_add_focus(&mut self) {
        let next = match self.add_focused {
            None => AddField::ORDER[0],
            Some(current) => {
                let idx = AddField::ORDER
                    .iter()
                    .position(|f| *f == current)
                    .unwrap_or(0);
                AddField::ORDER[(idx + 1) % AddField::ORDER.len()]
            },
        };
        self.add_focused = Some(next);
    }
}

/// Resolves `host` / `host:port` to socket addresses, appending
/// [`DEFAULT_GAME_PORT`] when no port is given. Tries the string as-is first
/// (covers `host:port` and `ip:port`), then with the default port appended
/// (covers a bare `host`/`ip`). Bracketed IPv6 (`[::1]:14004`) works via the
/// as-is path; a bare IPv6 literal isn't supported (documented edge — users
/// enter hostnames or `ip:port`).
fn resolve_addr(address: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return Err("empty address".to_owned());
    }
    for candidate in [trimmed.to_owned(), format!("{trimmed}:{DEFAULT_GAME_PORT}")] {
        if let Ok(iter) = candidate.to_socket_addrs() {
            let addrs: Vec<_> = iter.collect();
            if !addrs.is_empty() {
                return Ok(addrs);
            }
        }
    }
    Err(format!("could not resolve '{trimmed}'"))
}

/// Reads the OPTIONAL one-line status banner, returning `(version, players,
/// motd)`. Absent (timeout / EOF) or garbled → all `None` (still reachable,
/// ping-only). Runs on a probe thread — the read blocks.
fn read_status_banner(
    stream: &mut TcpStream,
) -> (Option<String>, Option<(u32, u32)>, Option<String>) {
    let _ = stream.set_read_timeout(Some(PROBE_STATUS_READ_TIMEOUT));
    let mut buf = [0u8; 512];
    let read = stream.read(&mut buf);
    let Ok(n) = read else {
        return (None, None, None);
    };
    if n == 0 {
        return (None, None, None);
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    let line = text.lines().next().unwrap_or("").trim();
    match ron::from_str::<ServerStatusBanner>(line) {
        Ok(banner) => (
            (!banner.version.is_empty()).then_some(banner.version),
            banner.players,
            (!banner.motd.is_empty()).then_some(banner.motd),
        ),
        Err(_) => (None, None, None),
    }
}

/// Probes one server: resolves its address, opens a TCP connection (the connect
/// RTT is the ping), and optionally reads the status banner. Blocks throughout
/// (DNS + `connect_timeout` + banner read), so it only ever runs on an
/// [`IoTaskPool`] thread (blocking socket I/O belongs on the I/O pool, not
/// the CPU-bound compute pool the terrain mesher uses). See the section header
/// above for the honest scope of what it can/can't learn.
fn probe_server(address: String) -> ProbeResult {
    let resolved = resolve_addr(&address)?;
    let start = Instant::now();
    let mut last_err = String::from("no addresses resolved");
    let mut connected = None;
    for addr in resolved {
        match TcpStream::connect_timeout(&addr, PROBE_CONNECT_TIMEOUT) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            },
            Err(err) => last_err = err.to_string(),
        }
    }
    let Some(mut stream) = connected else {
        return Err(last_err);
    };
    let ping_ms = start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    let (version, players, motd) = read_status_banner(&mut stream);
    Ok(ServerStatus {
        ping_ms: Some(ping_ms),
        version,
        players,
        motd,
    })
}

/// A one-line human summary of a row's query state (painted into its status
/// text each frame by [`render_server_rows`]).
fn status_summary(state: &QueryState) -> String {
    match state {
        QueryState::Idle => "…".to_owned(),
        QueryState::Querying => "querying…".to_owned(),
        QueryState::Done(Err(err)) => {
            let mut reason = err.clone();
            reason.truncate(40);
            format!("unreachable ({reason})")
        },
        QueryState::Done(Ok(status)) => {
            let ping = status
                .ping_ms
                .map_or_else(|| "—".to_owned(), |p| format!("{p} ms"));
            let version = status.version.as_deref().unwrap_or("?");
            let players = status
                .players
                .map_or_else(|| "—".to_owned(), |(n, cap)| format!("{n}/{cap}"));
            format!("{ping}   v: {version}   players: {players}")
        },
    }
}

/// The server-browser concurrency engine (BL-82 EM-5.9 T56.31): on a refresh
/// request it fires one [`IoTaskPool`] probe PER row (all concurrent,
/// off the main thread), and every frame it drains any FINISHED probe into its
/// row — so each row's ping/version paints the frame its own probe completes,
/// never blocking on the slowest server. Runs every menu frame; a cheap no-op
/// unless a refresh was requested or a probe just finished.
fn service_server_queries(mut browser: ResMut<ServerBrowser>) {
    poll_and_dispatch_probes(&mut browser);
}

/// The [`service_server_queries`] core, factored out so tests can drive it
/// directly (against real local listeners) without a full Bevy `App`. On a
/// refresh request it dispatches one concurrent [`IoTaskPool`] probe
/// per row; every call it drains any finished probe into its row.
fn poll_and_dispatch_probes(browser: &mut ServerBrowser) {
    if browser.needs_refresh {
        browser.needs_refresh = false;
        let pool = IoTaskPool::get();
        browser.tasks = (0..browser.rows.len()).map(|_| None).collect();
        let addresses: Vec<String> = browser.rows.iter().map(|r| r.address.clone()).collect();
        for (i, address) in addresses.into_iter().enumerate() {
            browser.rows[i].state = QueryState::Querying;
            let task = pool.spawn(async move { probe_server(address) });
            browser.tasks[i] = Some(task);
        }
    }

    for i in 0..browser.tasks.len() {
        if browser.tasks[i].as_ref().is_some_and(Task::is_finished)
            && let Some(task) = browser.tasks[i].take()
        {
            let result = block_on(task);
            if let Some(row) = browser.rows.get_mut(i) {
                row.state = QueryState::Done(result);
            }
        }
    }
}

/// Data-driven rotating gameplay tips shown on the loading screen, loaded once
/// from `assets/xindeler/ui/loading_tips.ron` (BL-82 EM-5.9 T56.30). A random
/// entry is picked each time the connecting screen appears.
#[derive(Resource, Debug, Default)]
struct LoadingTips(Vec<String>);

/// Data-driven loading-screen credits, loaded once from
/// `assets/xindeler/ui/credits.ron` (BL-82 EM-5.9 T56.30).
#[derive(Resource, Debug, Default, Deserialize)]
struct Credits {
    /// The engine/community attribution line shown under the credits heading.
    #[serde(default)]
    engine_note: String,
    /// Role → the people credited for it.
    #[serde(default)]
    entries: Vec<CreditEntry>,
}

#[derive(Debug, Deserialize)]
struct CreditEntry {
    role: String,
    names: Vec<String>,
}

/// The live state of one offline connection attempt (BL-82 EM-5.9 T56.30).
///
/// Inserted on `OnEnter(Connecting)` and removed when the attempt resolves.
/// The multi-second world boot runs on a background thread; `stage` is the
/// shared cell it reports genuine [`ConnectStage`] transitions into, and
/// `outcome` is where it drops the finished [`BootOutcome`]. Both are
/// `Arc<Mutex<…>>` so the resource stays `Send + Sync` (a plain Bevy
/// `Resource`) even though the booted sim/player themselves are `!Sync`.
#[derive(Resource)]
struct ConnectTask {
    /// The most recent real boot stage, written by the boot thread, polled by
    /// [`render_connecting`].
    stage: Arc<Mutex<ConnectStage>>,
    /// The finished boot result, dropped in by the boot thread exactly once.
    outcome: Arc<Mutex<Option<BootOutcome>>>,
    /// The boot thread's handle, joined when the attempt resolves.
    handle: Option<std::thread::JoinHandle<()>>,
    /// Frame counter driving the loading-screen spinner glyph (a pure activity
    /// indicator — NOT progress; the bar is the real progress signal).
    spinner_frames: u32,
    /// The server's MOTD, resolved from the connected client once the boot
    /// completes; shown on the "Entering world" beat before gameplay.
    motd: Option<String>,
    /// `Some(n)` once the world has booted and we're holding the loading screen
    /// for `n` more frames so the MOTD is readable before entering. A short,
    /// bounded greeting beat (real progress is already complete), so the flow
    /// can never hang here. `None` while the boot is still in flight.
    entering_frames: Option<u32>,
}

/// Frames to hold the "Entering world" screen (showing the MOTD) after the boot
/// completes, before switching to gameplay. ~1.2 s at 60 fps — long enough to
/// read a short greeting, short enough not to feel like a stall. This is a
/// deliberate readable beat AFTER real progress is done, not faked progress.
const ENTERING_HOLD_FRAMES: u32 = 72;

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

/// The connecting / loading screen root.
#[derive(Component)]
struct ConnectingRoot;

/// The loading screen's current-stage line (updated from [`ConnectStage`]).
#[derive(Component)]
struct ConnectStageText;

/// The loading screen's progress-bar FILL node (its width % tracks the real
/// [`ConnectStage::progress_fraction`]).
#[derive(Component)]
struct ConnectBarFill;

/// The loading screen's activity spinner glyph.
#[derive(Component)]
struct ConnectSpinnerText;

/// The loading screen's server MOTD line (empty until the boot resolves).
#[derive(Component)]
struct ConnectMotdText;

/// The scrollable container the server-browser rows are (re)built into.
#[derive(Component)]
struct ServerListContainer;

/// A server-browser row node, tagged with its index into
/// [`ServerBrowser::rows`] (for click-to-select + selection highlighting).
#[derive(Component, Clone, Copy)]
struct ServerRowUi(usize);

/// The live-status text of a server-browser row (ping/version/players),
/// repainted each frame from that row's [`QueryState`].
#[derive(Component, Clone, Copy)]
struct ServerRowStatus(usize);

/// The on-screen text mirroring an add-server field's buffer.
#[derive(Component, Clone, Copy)]
struct AddFieldText(AddField);

/// An add-server field's bordered box (border highlights when focused).
#[derive(Component, Clone, Copy)]
struct AddFieldBox(AddField);

/// The server browser's own status/notice line.
#[derive(Component)]
struct BrowserStatusText;

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
    mut browser: ResMut<ServerBrowser>,
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
            "browser" => {
                browser.load_from(&settings.menu.servers);
                MenuScreen::ServerBrowser
            },
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

/// Kicks off the offline world boot on a BACKGROUND thread and spawns the
/// loading screen (stage line + real progress bar + spinner + tip + credits +
/// an empty MOTD line). Running the multi-second boot off the main thread is
/// what lets the window keep rendering live progress instead of freezing; the
/// boot reports genuine [`ConnectStage`] transitions into a shared cell that
/// [`render_connecting`] polls, and drops its finished [`BootOutcome`] into a
/// second cell that [`drive_connecting`] hands off to the ECS.
fn enter_connecting(
    mut commands: Commands,
    theme: Option<Res<HudTheme>>,
    fonts: Option<Res<HudFonts>>,
    images: Option<Res<MenuImages>>,
    form: Res<LoginForm>,
    tips: Res<LoadingTips>,
    credits: Res<Credits>,
) {
    // Online never actually reaches here (Connect stays on the login screen for
    // online — see `attempt_connect`); guard so a stray future online path can't
    // silently boot an embedded world. No ConnectTask is inserted, so
    // `drive_connecting` bounces straight back to the menu.
    if form.online {
        return;
    }

    // Shared cells the boot thread writes and the main thread polls.
    let stage = Arc::new(Mutex::new(ConnectStage::Starting));
    let outcome: Arc<Mutex<Option<BootOutcome>>> = Arc::new(Mutex::new(None));
    let stage_writer = Arc::clone(&stage);
    let outcome_writer = Arc::clone(&outcome);
    let handle = std::thread::Builder::new()
        .name("offline-world-boot".to_owned())
        .spawn(move || {
            let report = move |s: ConnectStage| {
                if let Ok(mut cell) = stage_writer.lock() {
                    *cell = s;
                }
            };
            let result = crate::listen_server::boot_offline_world_parts(&report);
            if let Ok(mut cell) = outcome_writer.lock() {
                *cell = Some(result);
            }
        })
        .expect("failed to spawn offline-world boot thread");

    commands.insert_resource(ConnectTask {
        stage,
        outcome,
        handle: Some(handle),
        spinner_frames: 0,
        motd: None,
        entering_frames: None,
    });

    let (Some(theme), Some(fonts)) = (theme, fonts) else {
        return;
    };
    let theme: HudTheme = *theme;
    let tip = pick_tip(&tips);
    build_connecting_screen(
        &mut commands,
        &theme,
        &fonts,
        images.as_deref(),
        &tip,
        &credits,
    );
}

/// Spawns the loading screen tree (called once on `OnEnter(Connecting)`).
fn build_connecting_screen(
    commands: &mut Commands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
    tip: &str,
    credits: &Credits,
) {
    let mut root = commands.spawn((
        ConnectingRoot,
        // Above every HUD layer so it fully covers the (still loading)
        // gameplay chrome that spawns behind it.
        GlobalZIndex(zlayer::TOAST + 100),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            row_gap: Val::Px(theme.spacing.md),
            padding: UiRect::all(Val::Px(theme.spacing.lg)),
            ..Default::default()
        },
        BackgroundColor(MENU_BACKDROP),
    ));
    // Full-screen background art behind the loading UI (legacy paints a menu
    // background on the connecting screen too; kept deterministic by reusing
    // the same static `bg_main.jpg` rather than the legacy random `bg_N`).
    // Dimmed via the image tint so the (panel-less) loading text stays legible
    // over the photo backdrop.
    if let Some(images) = images {
        root.insert(bevy::ui::widget::ImageNode {
            image: images.background.clone(),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            color: Color::srgb(0.35, 0.35, 0.40),
            ..Default::default()
        });
    }
    root.with_children(|screen| {
        // Title.
        screen.spawn((
            Text("Xindeler".to_owned()),
            TextFont {
                font: bevy::text::FontSource::Handle(fonts.title.clone()),
                font_size: bevy::text::FontSize::Px(40.0),
                ..Default::default()
            },
            TextColor(theme.palette.text),
        ));

        // Stage line + spinner (row).
        screen
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.sm),
                align_items: AlignItems::Center,
                ..Default::default()
            })
            .with_children(|row| {
                row.spawn((
                    ConnectSpinnerText,
                    Text(SPINNER_FRAMES[0].to_owned()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(18.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.accent),
                ));
                row.spawn((
                    ConnectStageText,
                    Text(stage_label(ConnectStage::Starting).to_owned()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(18.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
            });

        // Progress bar: a fixed-width track with a fill whose width tracks
        // the REAL stage fraction (updated in `render_connecting`).
        screen
            .spawn((
                Node {
                    width: Val::Px(360.0),
                    height: Val::Px(10.0),
                    border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
                    ..Default::default()
                },
                BackgroundColor(theme.palette.xp_bg),
            ))
            .with_children(|track| {
                let frac = ConnectStage::Starting.progress_fraction();
                track.spawn((
                    ConnectBarFill,
                    Node {
                        width: Val::Percent(frac * 100.0),
                        height: Val::Percent(100.0),
                        border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
                        ..Default::default()
                    },
                    BackgroundColor(theme.palette.accent),
                ));
            });

        // MOTD line (empty until the boot resolves).
        screen.spawn((
            ConnectMotdText,
            Text(String::new()),
            TextFont {
                font: bevy::text::FontSource::Handle(fonts.body.clone()),
                font_size: bevy::text::FontSize::Px(16.0),
                ..Default::default()
            },
            TextColor(theme.palette.accent),
            Node {
                max_width: Val::Px(520.0),
                ..Default::default()
            },
        ));

        // Rotating gameplay tip.
        screen.spawn((
            Text(format!("Tip: {tip}")),
            TextFont {
                font: bevy::text::FontSource::Handle(fonts.body.clone()),
                font_size: bevy::text::FontSize::Px(14.0),
                ..Default::default()
            },
            TextColor(theme.palette.text_muted),
            Node {
                max_width: Val::Px(520.0),
                margin: UiRect::top(Val::Px(theme.spacing.md)),
                ..Default::default()
            },
        ));

        // Credits footer.
        if !credits.engine_note.is_empty() || !credits.entries.is_empty() {
            let mut lines = Vec::new();
            if !credits.engine_note.is_empty() {
                lines.push(credits.engine_note.clone());
            }
            for entry in &credits.entries {
                lines.push(format!("{}: {}", entry.role, entry.names.join(", ")));
            }
            screen.spawn((
                Text(lines.join("\n")),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(11.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text_muted),
                Node {
                    max_width: Val::Px(520.0),
                    margin: UiRect::top(Val::Px(theme.spacing.lg)),
                    ..Default::default()
                },
            ));
        }
    });
}

/// Removes the loading screen when leaving [`AppState::Connecting`], and drops
/// any leftover [`ConnectTask`] (joining its thread) as a safety net — the
/// normal path removes it in [`drive_connecting`].
/// An exclusive `&mut World` system (not just `Commands`) SPECIFICALLY so it
/// can route any leftover [`ConnectTask`] through [`finish_connect_task`] —
/// reviewer minor (bevy-migration-reviewer): a plain `commands.remove_resource`
/// would DROP the `JoinHandle` without joining it, silently detaching the
/// thread. Unreachable in the normal flow (both `drive_connecting` exits
/// already call `finish_connect_task`, so no task is left by the time this
/// runs), but symmetric and safe if a future path ever forces `AppState` away
/// from `Connecting` mid-boot.
fn despawn_connecting_root(world: &mut World) {
    let roots: Vec<Entity> = world
        .query_filtered::<Entity, With<ConnectingRoot>>()
        .iter(world)
        .collect();
    for root in roots {
        world.despawn(root);
    }
    finish_connect_task(world);
}

/// Safety net: on entering gameplay, close any HUD window a stray hotkey might
/// have toggled open behind the (opaque) menu while it was up.
fn close_hud_windows_on_enter_game(hud_state: Option<ResMut<HudState>>) {
    if let Some(mut hud_state) = hud_state {
        hud_state.close();
    }
}

// ---------------------------------------------------------------------------
// The connecting flow (background offline boot + live loading screen)
// ---------------------------------------------------------------------------

/// The spinner's animation frames (a pure activity indicator).
// Reviewer minor (bevy-migration-reviewer): plain ASCII, not Braille Patterns
// glyphs — the HUD body font's glyph coverage for that Unicode block isn't
// guaranteed, so Braille frames risk rendering as tofu/blank. ASCII is
// guaranteed present in any font this UI already renders text with.
const SPINNER_FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
/// Frames the spinner holds each glyph (so it ticks at a readable rate).
const SPINNER_HOLD: u32 = 6;

/// The human-readable label for a real boot stage.
fn stage_label(stage: ConnectStage) -> &'static str {
    match stage {
        ConnectStage::Starting => "Preparing…",
        ConnectStage::GeneratingWorld => "Generating your world…",
        ConnectStage::EstablishingConnection => "Establishing connection…",
        ConnectStage::CheckingVersion => "Checking server version…",
        ConnectStage::Authenticating => "Authenticating…",
        ConnectStage::LoadingWorldData => "Loading world data…",
        ConnectStage::PreparingClient => "Preparing client…",
        ConnectStage::EnteringWorld => "Entering world…",
    }
}

/// Picks a tip pseudo-randomly (dependency-free: seeded off the wall clock, so
/// a fresh tip appears each time the loading screen opens).
fn pick_tip(tips: &LoadingTips) -> String {
    if tips.0.is_empty() {
        return String::new();
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    tips.0[seed % tips.0.len()].clone()
}

/// Paints the live loading screen each frame: stage text + real progress bar +
/// animated spinner, and the MOTD once known. Reads the shared boot-stage cell
/// (or the "Entering world" beat once the boot has resolved).
fn render_connecting(
    mut task: Option<ResMut<ConnectTask>>,
    mut stage_text: Query<&mut Text, (With<ConnectStageText>, Without<ConnectMotdText>)>,
    mut motd_text: Query<&mut Text, (With<ConnectMotdText>, Without<ConnectStageText>)>,
    mut spinner: Query<
        &mut Text,
        (
            With<ConnectSpinnerText>,
            Without<ConnectStageText>,
            Without<ConnectMotdText>,
        ),
    >,
    mut bar: Query<&mut Node, With<ConnectBarFill>>,
) {
    let Some(task) = task.as_mut() else { return };

    // During the "Entering world" hold the displayed stage is forced to
    // `EnteringWorld`; otherwise it's whatever the boot thread last reported.
    let stage = if task.entering_frames.is_some() {
        ConnectStage::EnteringWorld
    } else {
        task.stage.lock().map(|s| *s).unwrap_or_default()
    };

    for mut text in &mut stage_text {
        let label = stage_label(stage);
        if text.0 != label {
            text.0 = label.to_owned();
        }
    }
    for mut node in &mut bar {
        node.width = Val::Percent(stage.progress_fraction() * 100.0);
    }

    // Advance the spinner glyph.
    task.spinner_frames = task.spinner_frames.wrapping_add(1);
    let glyph =
        SPINNER_FRAMES[(task.spinner_frames / SPINNER_HOLD) as usize % SPINNER_FRAMES.len()];
    for mut text in &mut spinner {
        if text.0 != glyph {
            text.0 = glyph.to_owned();
        }
    }

    // MOTD, once resolved by the boot handoff.
    if let Some(motd) = &task.motd {
        for mut text in &mut motd_text {
            if &text.0 != motd {
                text.0 = motd.clone();
            }
        }
    }
}

/// The ECS handoff: once the background boot resolves, insert the booted
/// sim/player as non-send resources, resolve the MOTD, hold a short readable
/// "Entering world" beat, then transition to gameplay — or bounce back to the
/// login screen with a real error if the world couldn't boot.
///
/// An exclusive `&mut World` system because `insert_non_send` needs `&mut
/// World`. It no longer BLOCKS (the boot runs on a thread now); each frame it
/// just polls the shared outcome cell, so the window stays responsive.
fn drive_connecting(world: &mut World) {
    // No task (defensive: the online guard, or a torn-down attempt) → bounce.
    if !world.contains_resource::<ConnectTask>() {
        world
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
        return;
    }

    // Already committed to entering: count down the readable MOTD beat, then go.
    if let Some(n) = world.resource::<ConnectTask>().entering_frames {
        if n + 1 >= ENTERING_HOLD_FRAMES {
            finish_connect_task(world);
            world
                .resource_mut::<NextState<AppState>>()
                .set(AppState::InGame);
        } else {
            world.resource_mut::<ConnectTask>().entering_frames = Some(n + 1);
        }
        return;
    }

    // Poll for the finished boot (None until the thread drops it in).
    let outcome = {
        let task = world.resource::<ConnectTask>();
        task.outcome.lock().ok().and_then(|mut cell| cell.take())
    };
    // Reviewer finding (bevy-migration-reviewer, MAJOR): if the boot thread
    // PANICS (worldgen assertion, a missing-asset `expect`, ...), it never
    // reaches the `outcome_writer` store, so the cell above stays `None`
    // forever — polling `outcome` alone would hang the loading screen with no
    // error and no way out (worse than the pre-thread code, where a boot
    // panic surfaced/aborted on the main thread). Detect that case: the
    // thread has FINISHED (so it can never write anything more) but produced
    // no outcome — treat it exactly like a boot `Err`, so the flow always
    // resolves one way or the other and can never silently hang.
    let thread_died_silently = outcome.is_none()
        && world
            .resource::<ConnectTask>()
            .handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished);
    let Some(outcome) = outcome else {
        if thread_died_silently {
            info!("connecting: offline world boot thread ended without a result (likely panicked)");
            finish_connect_task(world);
            {
                let mut form = world.resource_mut::<LoginForm>();
                form.error = Some(
                    "Could not start a world (the boot process crashed). Check the logs and try \
                     again."
                        .to_owned(),
                );
            }
            *world.resource_mut::<MenuScreen>() = MenuScreen::Login;
            world
                .resource_mut::<NextState<AppState>>()
                .set(AppState::MainMenu);
        }
        return;
    };

    match outcome {
        Ok((sim, player)) => {
            // Resolve the MOTD from the connected client before we hand the
            // player off to the ECS as a non-send resource.
            let motd = player.as_ref().and_then(EmbeddedPlayer::server_motd);
            world.insert_non_send(sim);
            if let Some(player) = player {
                world.insert_non_send(player);
            }
            {
                let mut task = world.resource_mut::<ConnectTask>();
                task.motd = motd;
                // Begin the short readable "Entering world" beat. The world is
                // already booted and ticking behind the (opaque) screen; this
                // is a greeting beat, not faked progress, and it is bounded so
                // the flow can never hang.
                task.entering_frames = Some(0);
            }
        },
        Err(err) => {
            info!("connecting: offline world boot failed: {err}");
            finish_connect_task(world);
            {
                let mut form = world.resource_mut::<LoginForm>();
                form.error = Some(
                    "Could not start a world (missing assets or map data). Check XINDELER_ASSETS \
                     / the LFS map blobs and try again."
                        .to_owned(),
                );
            }
            *world.resource_mut::<MenuScreen>() = MenuScreen::Login;
            world
                .resource_mut::<NextState<AppState>>()
                .set(AppState::MainMenu);
        },
    }
}

/// Removes the [`ConnectTask`] and joins its (already-finished) boot thread.
fn finish_connect_task(world: &mut World) {
    if let Some(mut task) = world.remove_resource::<ConnectTask>()
        && let Some(handle) = task.handle.take()
    {
        let _ = handle.join();
    }
}

/// Loads the data-driven loading-screen tips + credits from RON once at startup
/// (mirrors `diary.rs`'s synchronous manifest-read pattern). Missing/broken
/// files degrade to empty defaults with a warning — the loading screen still
/// works, just without tips/credits.
fn load_loading_screen_assets(mut commands: Commands) {
    let root = xindeler_ui::i18n::assets_root();

    let tips_path = root.join("xindeler/ui/loading_tips.ron");
    match std::fs::read_to_string(&tips_path).map(|t| ron::de::from_str::<Vec<String>>(&t)) {
        Ok(Ok(tips)) => commands.insert_resource(LoadingTips(tips)),
        Ok(Err(e)) => warn!(?tips_path, error = ?e, "loading: failed to parse loading_tips.ron"),
        Err(e) => warn!(?tips_path, error = ?e, "loading: failed to read loading_tips.ron"),
    }

    let credits_path = root.join("xindeler/ui/credits.ron");
    match std::fs::read_to_string(&credits_path).map(|t| ron::de::from_str::<Credits>(&t)) {
        Ok(Ok(credits)) => commands.insert_resource(credits),
        Ok(Err(e)) => warn!(?credits_path, error = ?e, "loading: failed to parse credits.ron"),
        Err(e) => warn!(?credits_path, error = ?e, "loading: failed to read credits.ron"),
    }
}

// ---------------------------------------------------------------------------
// Menu chrome art (BL-82 main-menu visual parity)
// ---------------------------------------------------------------------------
//
// The Bevy menu (built across EM-5.9 #166-168) shipped with a flat dark
// backdrop + plain themed buttons/fields — no reused art. This resource loads
// the SAME real, pre-existing legacy main-menu assets the frozen conrod
// reference (`voxygen/src/menu/main/ui/mod.rs`'s `Imgs`) loads, so the Bevy
// screen matches the legacy client's actual look instead of approximating it.
// Loaded once at `Startup` (hot-reloadable in dev), mirroring `chat.rs`'s
// `ChatIcons` / the inventory rebuild's asset-reuse precedent this session.

/// The real legacy main-menu chrome art, reused verbatim. Legacy asset key →
/// real file (all present as LFS blobs in this repo):
/// - `voxygen.background.bg_main`                        →
///   `background/bg_main.jpg`
/// - `voxygen.element.v_logo`                            → `element/v_logo.png`
/// - `voxygen.element.ui.generic.buttons.button{,_hover,_press}`
/// - `voxygen.element.ui.generic.textbox`                → the input-field
///   frame
#[derive(Resource, Debug, Clone)]
struct MenuImages {
    /// The static title-screen background (`bg_main.jpg`, 1920×1080) — the
    /// legacy menu's signature full-screen art (the frozen reference paints it
    /// behind every non-connecting screen; a random `bg_N` is used only on the
    /// connecting screen, which this port keeps deterministic by reusing
    /// `bg_main` there too — see [`build_connecting_screen`]).
    background: Handle<Image>,
    /// The wordmark logo shown at the top of the menu panel (`v_logo.png`,
    /// 346×111).
    logo: Handle<Image>,
    /// The carved-button chrome, swapped on hover/press by the widget kit's
    /// [`update_image_button_visuals`](xindeler_ui::button).
    button: Handle<Image>,
    button_hover: Handle<Image>,
    button_press: Handle<Image>,
    /// The input-field frame drawn behind login / add-server text fields
    /// (`textbox.png`, 169×25).
    textbox: Handle<Image>,
}

impl MenuImages {
    fn load(asset_server: &AssetServer) -> Self {
        let load = |path: &str| asset_server.load(path.to_owned());
        Self {
            background: load("voxygen/background/bg_main.jpg"),
            logo: load("voxygen/element/v_logo.png"),
            button: load("voxygen/element/ui/generic/buttons/button.png"),
            button_hover: load("voxygen/element/ui/generic/buttons/button_hover.png"),
            button_press: load("voxygen/element/ui/generic/buttons/button_press.png"),
            textbox: load("voxygen/element/ui/generic/textbox.png"),
        }
    }

    /// The three button-state textures the widget kit's
    /// [`image_button_bundle`] hover/press swapper expects.
    fn button_images(&self) -> HudButtonImages {
        HudButtonImages {
            normal: self.button.clone(),
            hover: self.button_hover.clone(),
            pressed: self.button_press.clone(),
        }
    }
}

/// `Startup` system inserting [`MenuImages`] (loaded via the real
/// [`AssetServer`]), mirroring [`load_loading_screen_assets`].
fn load_menu_images(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(MenuImages::load(&asset_server));
}

/// The logo's native aspect ratio (346×111 `v_logo.png`) — used so the panel
/// header image keeps its proportions at a fixed display width.
const LOGO_ASPECT: f32 = 346.0 / 111.0;

/// Spawns the wordmark logo at the top of the menu panel (image-backed,
/// aspect-preserved). Shown on every sub-screen so the branding is constant,
/// mirroring the legacy menu's ever-present `v_logo`.
fn spawn_logo(panel: &mut ChildSpawnerCommands, images: &MenuImages) {
    panel.spawn((
        bevy::ui::widget::ImageNode {
            image: images.logo.clone(),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            ..Default::default()
        },
        Node {
            width: Val::Px(220.0),
            aspect_ratio: Some(LOGO_ASPECT),
            align_self: AlignSelf::Center,
            margin: UiRect::bottom(Val::Px(4.0)),
            ..Default::default()
        },
    ));
}

// ---------------------------------------------------------------------------
// Build (spawn the current screen)
// ---------------------------------------------------------------------------

/// Opaque fallback backdrop shown behind the menu while the background art is
/// still loading (and in the headless tests, which spawn the menu without a
/// [`MenuImages`] resource). Once [`MenuImages::background`] resolves, the
/// full-screen `bg_main.jpg` [`ImageNode`](bevy::ui::widget::ImageNode) covers
/// it.
const MENU_BACKDROP: Color = Color::srgb(0.04, 0.05, 0.08);

/// (Re)builds the menu tree whenever the current [`MenuScreen`] changes (or the
/// root is missing, e.g. on first entry or after a return to the menu). Keeps a
/// `Local` of the last-built screen so it doesn't rebuild every frame.
fn build_menu(
    mut commands: Commands,
    theme: Option<Res<HudTheme>>,
    fonts: Option<Res<HudFonts>>,
    images: Option<Res<MenuImages>>,
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
    let images = images.as_deref();

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
        // Fallback backdrop while the art loads (and in the headless tests,
        // which spawn the menu without a `MenuImages` resource). Covered by the
        // full-screen `bg_main.jpg` below once it resolves.
        BackgroundColor(MENU_BACKDROP),
    ));
    // The legacy menu's signature full-screen background art.
    if let Some(images) = images {
        root.insert(bevy::ui::widget::ImageNode {
            image: images.background.clone(),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            ..Default::default()
        });
    }
    root.with_children(|screen_node| {
        // Version line, top-centre (legacy paints `Veloren {version}` here).
        screen_node.spawn((
            Text(format!("Xindeler v{}", env!("CARGO_PKG_VERSION"))),
            TextFont {
                font: bevy::text::FontSource::Handle(fonts.body.clone()),
                font_size: bevy::text::FontSize::Px(12.0),
                ..Default::default()
            },
            TextColor(theme.palette.text_muted),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(6.0),
                ..Default::default()
            },
        ));

        let mut panel_entity = screen_node.spawn(panel_bundle(&theme));
        let row_gap_px = theme.spacing.md;
        panel_entity.entry::<Node>().and_modify(move |mut node| {
            node.flex_direction = FlexDirection::Column;
            node.row_gap = Val::Px(row_gap_px);
            node.min_width = Val::Px(420.0);
            node.max_width = Val::Px(560.0);
            node.align_items = AlignItems::Stretch;
        });
        panel_entity.with_children(|panel| {
            // The wordmark logo crowns the disclaimer/main/login screens
            // (legacy's ever-present `v_logo`). The content-heavy server browser
            // is its own dense layout (legacy's `servers.rs` likewise has no big
            // central logo), so it's skipped there to keep the panel inside the
            // viewport height. Only when the art is loaded.
            if let Some(images) = images
                && *screen != MenuScreen::ServerBrowser
            {
                spawn_logo(panel, images);
            }
            match *screen {
                MenuScreen::Disclaimer => spawn_disclaimer(panel, &theme, &fonts, images),
                MenuScreen::Main => spawn_main(panel, &theme, &fonts, images),
                MenuScreen::Login => spawn_login(panel, &theme, &fonts, images, &form),
                MenuScreen::ServerBrowser => spawn_server_browser(panel, &theme, &fonts, images),
            }
        });
    });

    *last_built = Some(*screen);
}

/// Spawns a menu button — image-backed with the legacy carved-button chrome
/// (`button.png` + hover/press) when [`MenuImages`] is loaded, falling back to
/// the flat themed button otherwise (the headless tests spawn the menu without
/// the image resource). Returns the button entity so the caller can chain its
/// `.observe(...)` action, exactly like a bare `button_bundle` spawn.
fn menu_button<'a>(
    panel: &'a mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
    label: &str,
) -> bevy::ecs::system::EntityCommands<'a> {
    match images {
        Some(images) => {
            let mut button = panel.spawn(image_button_bundle(
                theme,
                fonts,
                label,
                images.button_images(),
            ));
            // Uniform button width (legacy's buttons are all one plate width),
            // centred in the panel — an explicit width overrides the panel's
            // `align_items: Stretch`, so also re-centre via `align_self`.
            button.entry::<Node>().and_modify(|mut node| {
                node.width = Val::Px(MENU_BUTTON_WIDTH);
                node.height = Val::Px(MENU_BUTTON_HEIGHT);
                node.align_self = AlignSelf::Center;
            });
            // Stretch the carved-button plate across the (wider than native)
            // button box so long labels still sit on real button chrome.
            // `image_button_bundle` defaults to `NodeImageMode::Auto`, which
            // would leave a long button's texture short of its edges; the
            // hover/press swapper only mutates `image`, so the mode survives.
            button
                .entry::<bevy::ui::widget::ImageNode>()
                .and_modify(|mut image| {
                    image.image_mode = bevy::ui::widget::NodeImageMode::Stretch;
                });
            button
        },
        None => panel.spawn(button_bundle(theme, fonts, label)),
    }
}

/// Uniform menu-button dimensions (roughly the `button.png` plate's 106×26
/// aspect, scaled up) — wide enough for the longest label
/// ("I understand — continue") so every button shows the carved plate chrome.
const MENU_BUTTON_WIDTH: f32 = 300.0;
const MENU_BUTTON_HEIGHT: f32 = 42.0;

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
fn spawn_disclaimer(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
) {
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
    menu_button(panel, theme, fonts, images, "I understand — continue").observe(accept_disclaimer);
    menu_button(panel, theme, fonts, images, "Quit").observe(quit_game);
}

/// The main menu: Play / Options / Quit.
fn spawn_main(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
) {
    body_text(
        panel,
        fonts,
        theme.palette.text_muted,
        "A voxel RPG — BL-82 Bevy client",
        14.0,
    );
    menu_button(panel, theme, fonts, images, "Play").observe(go_to_login);
    menu_button(panel, theme, fonts, images, "Multiplayer").observe(open_server_browser);
    menu_button(panel, theme, fonts, images, "Options").observe(options_notice);
    menu_button(panel, theme, fonts, images, "Quit").observe(quit_game);
    status_line(panel, fonts, theme);
}

/// The login screen: mode toggle + username / password / server + Connect/Back.
fn spawn_login(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
    form: &LoginForm,
) {
    heading(panel, fonts, theme, "Play", 30.0);

    // Offline/Online mode toggle.
    menu_button(panel, theme, fonts, images, mode_label(form.online))
        .insert(ModeToggleLabel)
        .observe(toggle_mode);

    login_field(
        panel,
        theme,
        fonts,
        images,
        LoginField::Username,
        "Username",
    );
    login_field(
        panel,
        theme,
        fonts,
        images,
        LoginField::Password,
        "Password",
    );
    login_field(
        panel,
        theme,
        fonts,
        images,
        LoginField::Server,
        "Server address",
    );

    body_text(
        panel,
        fonts,
        theme.palette.text_muted,
        "Offline hosts a private singleplayer world. Online multiplayer connects via the in-game \
         server browser (coming soon).",
        12.0,
    );

    menu_button(panel, theme, fonts, images, "Connect").observe(connect_clicked);
    menu_button(panel, theme, fonts, images, "Server browser").observe(open_server_browser);
    menu_button(panel, theme, fonts, images, "Back").observe(back_to_main);
    status_line(panel, fonts, theme);
}

/// A labelled, focusable login field (label + bordered value box). Clicking the
/// box focuses it; [`render_login_fields`] mirrors the buffer into it.
fn login_field(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
    field: LoginField,
    label: &str,
) {
    body_text(panel, fonts, theme.palette.text_muted, label, 13.0);
    let mut field_box = panel.spawn((
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
    ));
    // Legacy input-field frame (`textbox.png`) behind the value; the themed
    // focus border still highlights on top (see `render_login_fields`).
    if let Some(images) = images {
        field_box.insert(bevy::ui::widget::ImageNode {
            image: images.textbox.clone(),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            ..Default::default()
        });
    }
    field_box
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

/// The server browser: a scrollable saved-server list (live ping/version/
/// players per row), an add-server form, and Refresh / Connect / Delete / Back.
/// Builds only the STATIC chrome + the empty [`ServerListContainer`]; the rows
/// inside it are (re)built by [`rebuild_server_list`] and repainted each frame
/// by [`render_server_rows`].
fn spawn_server_browser(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
) {
    heading(panel, fonts, theme, "Server Browser", 30.0);
    body_text(
        panel,
        fonts,
        theme.palette.text_muted,
        "Saved multiplayer servers. Ping / version / players are queried live and concurrently; \
         pick a server and Connect. (Remote play is still being brought online — see the notes on \
         Connect.)",
        12.0,
    );

    // The scrollable list container — rows are spawned into this by
    // `rebuild_server_list`.
    panel
        .spawn(scroll_view_bundle(theme, 460.0, 120.0))
        .insert(ServerListContainer);

    // Add-server form.
    body_text(panel, fonts, theme.palette.text_muted, "Add a server", 13.0);
    add_field(
        panel,
        theme,
        fonts,
        images,
        AddField::Address,
        "Address (host or host:port)",
    );
    add_field(
        panel,
        theme,
        fonts,
        images,
        AddField::Nickname,
        "Nickname (optional)",
    );
    menu_button(panel, theme, fonts, images, "Add to list").observe(add_server_clicked);

    // Action buttons.
    menu_button(panel, theme, fonts, images, "Refresh").observe(refresh_clicked);
    menu_button(panel, theme, fonts, images, "Connect to selected")
        .observe(connect_selected_clicked);
    menu_button(panel, theme, fonts, images, "Delete selected").observe(delete_selected_clicked);
    menu_button(panel, theme, fonts, images, "Back").observe(browser_back);

    // The browser's own status/notice line.
    panel.spawn((
        BrowserStatusText,
        Text(String::new()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(13.0),
            ..Default::default()
        },
        TextColor(theme.palette.accent),
        Node {
            max_width: Val::Px(460.0),
            ..Default::default()
        },
    ));
}

/// A labelled, focusable add-server field (label + bordered value box), mirror
/// of [`login_field`] for the browser's own `add_*` buffers.
fn add_field(
    panel: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: Option<&MenuImages>,
    field: AddField,
    label: &str,
) {
    body_text(panel, fonts, theme.palette.text_muted, label, 12.0);
    let mut field_box = panel.spawn((
        AddFieldBox(field),
        Node {
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(theme.spacing.sm), Val::Px(theme.spacing.xs)),
            border: UiRect::all(Val::Px(2.0)),
            border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
            min_height: Val::Px(26.0),
            ..Default::default()
        },
        BackgroundColor(theme.palette.panel_bg),
        bevy::ui::BorderColor::all(theme.palette.panel_border),
    ));
    if let Some(images) = images {
        field_box.insert(bevy::ui::widget::ImageNode {
            image: images.textbox.clone(),
            image_mode: bevy::ui::widget::NodeImageMode::Stretch,
            ..Default::default()
        });
    }
    field_box
        .observe(
            move |_: On<Pointer<Click>>, mut browser: ResMut<ServerBrowser>| {
                browser.add_focused = Some(field);
            },
        )
        .with_children(|b| {
            b.spawn((
                AddFieldText(field),
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(15.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        });
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

/// Placeholder text for an empty, unfocused add-server field.
fn add_placeholder(field: AddField) -> &'static str {
    match field {
        AddField::Address => "(e.g. play.example.com:14004)",
        AddField::Nickname => "(optional display name)",
    }
}

/// Mirrors the browser's add-form buffers into their field text (focused field
/// gets a caret + accent border, empty unfocused fields show a placeholder) and
/// its status/notice line — the browser's analog of [`render_login_fields`] +
/// [`render_status_line`].
fn render_browser_fields(
    browser: Res<ServerBrowser>,
    theme: Option<Res<HudTheme>>,
    mut fields: Query<(&AddFieldText, &mut Text), Without<BrowserStatusText>>,
    mut boxes: Query<(&AddFieldBox, &mut bevy::ui::BorderColor)>,
    mut status: Query<&mut Text, (With<BrowserStatusText>, Without<AddFieldText>)>,
) {
    let Some(theme) = theme else { return };
    for (field, mut text) in &mut fields {
        let focused = browser.add_focused == Some(field.0);
        let value = match field.0 {
            AddField::Address => browser.add_address.clone(),
            AddField::Nickname => browser.add_nickname.clone(),
        };
        let new = if value.is_empty() && !focused {
            add_placeholder(field.0).to_owned()
        } else if focused {
            format!("{value}_")
        } else {
            value
        };
        if text.0 != new {
            text.0 = new;
        }
    }
    for (field, mut border) in &mut boxes {
        let colour = if browser.add_focused == Some(field.0) {
            theme.palette.accent
        } else {
            theme.palette.panel_border
        };
        *border = bevy::ui::BorderColor::all(colour);
    }
    let msg = browser.status.clone().unwrap_or_default();
    for mut text in &mut status {
        if text.0 != msg {
            text.0 = msg.clone();
        }
    }
}

/// (Re)builds the server-list row nodes into [`ServerListContainer`] whenever
/// the row SET changes (open/add/delete, tracked via
/// [`ServerBrowser::list_revision`]) or the container was just respawned by
/// [`build_menu`]. The per-row live status text is NOT set here — it's
/// repainted every frame by [`render_server_rows`]. Ordered
/// `.after(build_menu)` so it sees the freshly-spawned container the frame the
/// browser screen appears (same pattern [`render_login_fields`] relies on).
fn rebuild_server_list(
    mut commands: Commands,
    screen: Res<MenuScreen>,
    browser: Res<ServerBrowser>,
    theme: Option<Res<HudTheme>>,
    fonts: Option<Res<HudFonts>>,
    containers: Query<(Entity, Option<&Children>), With<ServerListContainer>>,
    mut last_revision: Local<Option<u64>>,
) {
    if *screen != MenuScreen::ServerBrowser {
        // Force a rebuild the next time the browser opens (the container is torn
        // down with the rest of the menu tree on screen change).
        *last_revision = None;
        return;
    }
    let Ok((container, children)) = containers.single() else {
        return;
    };
    let container_empty = children.is_none_or(|c| c.is_empty());
    let needs_rebuild = *last_revision != Some(browser.list_revision)
        || (container_empty && !browser.rows.is_empty());
    if !needs_rebuild {
        return;
    }
    let (Some(theme), Some(fonts)) = (theme, fonts) else {
        return;
    };

    // Clear any existing rows before respawning.
    if let Some(children) = children {
        for &child in children {
            commands.entity(child).despawn();
        }
    }

    commands.entity(container).with_children(|list| {
        if browser.rows.is_empty() {
            list.spawn((
                Text("No saved servers yet — add one below.".to_owned()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(14.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text_muted),
                Node {
                    padding: UiRect::all(Val::Px(theme.spacing.sm)),
                    ..Default::default()
                },
            ));
        }
        for (i, row) in browser.rows.iter().enumerate() {
            let name = if row.nickname.trim().is_empty() {
                row.address.clone()
            } else {
                format!("{}  —  {}", row.nickname, row.address)
            };
            list.spawn((
                ServerRowUi(i),
                Node {
                    width: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(theme.spacing.sm)),
                    margin: UiRect::bottom(Val::Px(theme.spacing.xs)),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
                    row_gap: Val::Px(2.0),
                    ..Default::default()
                },
                BackgroundColor(theme.palette.panel_bg),
                bevy::ui::BorderColor::all(theme.palette.panel_border),
            ))
            .observe(
                move |_: On<Pointer<Click>>, mut browser: ResMut<ServerBrowser>| {
                    browser.selected = Some(i);
                },
            )
            .with_children(|r| {
                r.spawn((
                    Text(name),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(15.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
                r.spawn((
                    ServerRowStatus(i),
                    Text(status_summary(&row.state)),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(12.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text_muted),
                ));
            });
        }
    });
    *last_revision = Some(browser.list_revision);
}

/// Repaints each server row's live status text (ping/version/players) from its
/// current [`QueryState`] and highlights the selected row's border — so a row
/// visibly updates the frame its own async probe completes.
fn render_server_rows(
    screen: Res<MenuScreen>,
    browser: Res<ServerBrowser>,
    theme: Option<Res<HudTheme>>,
    mut status_texts: Query<(&ServerRowStatus, &mut Text)>,
    mut row_borders: Query<(&ServerRowUi, &mut bevy::ui::BorderColor)>,
) {
    if *screen != MenuScreen::ServerBrowser {
        return;
    }
    let Some(theme) = theme else { return };
    for (tag, mut text) in &mut status_texts {
        let new = browser
            .rows
            .get(tag.0)
            .map_or_else(String::new, |row| status_summary(&row.state));
        if text.0 != new {
            text.0 = new;
        }
    }
    for (tag, mut border) in &mut row_borders {
        let colour = if browser.selected == Some(tag.0) {
            theme.palette.accent
        } else {
            theme.palette.panel_border
        };
        *border = bevy::ui::BorderColor::all(colour);
    }
}

/// Folds keyboard input into the browser's add-server form when the browser
/// sub-screen is showing (Tab cycles Address/Nickname, Backspace deletes,
/// Escape returns to the main menu, printable characters insert) — the
/// browser's analog of [`read_login_input`]. Drains events (no-op) on any other
/// sub-screen so nothing leaks between screens.
fn read_browser_input(
    mut keyboard: MessageReader<KeyboardInput>,
    mut screen: ResMut<MenuScreen>,
    mut browser: ResMut<ServerBrowser>,
) {
    if *screen != MenuScreen::ServerBrowser {
        keyboard.read().for_each(drop);
        return;
    }
    for ev in keyboard.read() {
        if !ev.state.is_pressed() {
            continue;
        }
        match &ev.logical_key {
            Key::Tab => browser.cycle_add_focus(),
            Key::Backspace => browser.backspace_add(),
            Key::Space => browser.insert_add(" "),
            Key::Escape => {
                browser.add_focused = None;
                browser.status = None;
                *screen = MenuScreen::Main;
            },
            Key::Character(s) => browser.insert_add(s.as_str()),
            _ => {},
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
    form.error =
        Some("Settings are available from the in-game Esc menu once you're playing.".to_owned());
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

// ---------------------------------------------------------------------------
// Server-browser button actions (BL-82 EM-5.9 T56.31)
// ---------------------------------------------------------------------------

/// Opens the server browser: loads the working row list from persisted settings
/// and requests a fresh concurrent query round.
fn open_server_browser(
    _activate: On<Activate>,
    mut screen: ResMut<MenuScreen>,
    mut browser: ResMut<ServerBrowser>,
    settings: Res<XindelerSettings>,
) {
    browser.load_from(&settings.menu.servers);
    browser.add_focused = None;
    *screen = MenuScreen::ServerBrowser;
}

/// Adds the address (+ optional nickname) in the add-form to the persisted
/// saved-server list, then reloads + re-queries the list.
fn add_server_clicked(
    _activate: On<Activate>,
    mut browser: ResMut<ServerBrowser>,
    mut settings: ResMut<XindelerSettings>,
) {
    let address = browser.add_address.trim().to_owned();
    if address.is_empty() {
        browser.status = Some("Enter an address to add a server.".to_owned());
        return;
    }
    if settings.menu.servers.iter().any(|s| s.address == address) {
        browser.status = Some(format!("'{address}' is already in the list."));
        return;
    }
    let nickname = browser.add_nickname.trim().to_owned();
    settings.menu.servers.push(SavedServer {
        address: address.clone(),
        nickname,
    });
    if let Err(err) = settings.save() {
        error!("server browser: failed to persist saved servers: {err}");
    }
    browser.add_address.clear();
    browser.add_nickname.clear();
    browser.add_focused = None;
    browser.load_from(&settings.menu.servers);
    // Select the freshly-added server (it's the last row) for a quick Connect.
    browser.selected = browser.rows.len().checked_sub(1);
    browser.status = Some(format!("Added '{address}'."));
}

/// Re-fires the concurrent ping/version query for every saved server.
fn refresh_clicked(_activate: On<Activate>, mut browser: ResMut<ServerBrowser>) {
    browser.needs_refresh = true;
    browser.status = Some("Refreshing…".to_owned());
}

/// Connects to the selected server by driving the SAME login connect path (sets
/// the login form's server + Online mode, persists it, and calls
/// [`attempt_connect`]). Because a menu-mode process is build-time-committed to
/// the embedded transport, the online branch of `attempt_connect` still
/// surfaces the honest deferred-remote-transport notice today (#166/#167)
/// rather than establishing a real remote session — this wires the pick into
/// that same path (and will reach `Connecting` unchanged once the runtime
/// remote transport lands).
fn connect_selected_clicked(
    _activate: On<Activate>,
    mut browser: ResMut<ServerBrowser>,
    mut form: ResMut<LoginForm>,
    mut settings: ResMut<XindelerSettings>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(row) = browser.selected.and_then(|i| browser.rows.get(i)) else {
        browser.status = Some("Select a server first.".to_owned());
        return;
    };
    let address = row.address.clone();
    form.server = address.clone();
    form.online = true;
    attempt_connect(&mut form, &mut settings, &mut next);
    // Mirror whatever `attempt_connect` decided onto the browser's status line
    // (the deferred-online notice today; a real "Connecting…" once remote lands).
    browser.status = form
        .error
        .clone()
        .or_else(|| Some(format!("Connecting to {address}…")));
}

/// Removes the selected server from the persisted list, then reloads.
fn delete_selected_clicked(
    _activate: On<Activate>,
    mut browser: ResMut<ServerBrowser>,
    mut settings: ResMut<XindelerSettings>,
) {
    let Some(idx) = browser
        .selected
        .filter(|i| *i < settings.menu.servers.len())
    else {
        browser.status = Some("Select a server to delete.".to_owned());
        return;
    };
    let removed = settings.menu.servers.remove(idx);
    if let Err(err) = settings.save() {
        error!("server browser: failed to persist saved servers: {err}");
    }
    browser.load_from(&settings.menu.servers);
    browser.status = Some(format!("Removed '{}'.", removed.address));
}

/// Leaves the server browser back to the main menu.
fn browser_back(
    _activate: On<Activate>,
    mut screen: ResMut<MenuScreen>,
    mut browser: ResMut<ServerBrowser>,
) {
    browser.add_focused = None;
    browser.status = None;
    *screen = MenuScreen::Main;
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

    // -----------------------------------------------------------------------
    // Server browser (BL-82 EM-5.9 T56.31)
    // -----------------------------------------------------------------------

    use std::{io::Write, net::TcpListener, thread};

    /// A local TCP listener standing in for a server, purely for the query
    /// protocol (NOT a real game server): accepts one connection and, if
    /// `banner` is set, writes it before closing. Returns its `host:port`.
    fn spawn_probe_listener(banner: Option<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("local addr").to_string();
        thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next()
                && let Some(banner) = banner
            {
                let _ = stream.write_all(banner.as_bytes());
                let _ = stream.flush();
                // Drop `stream`/`listener` → connection closes (EOF for the
                // probe's read).
            }
        });
        addr
    }

    /// A `host:port` on localhost with NOTHING listening (bind, learn the port,
    /// drop the listener) — a connect there is refused, i.e. "unreachable".
    fn unused_local_addr() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind for free port");
        let addr = listener.local_addr().expect("local addr").to_string();
        drop(listener);
        addr
    }

    fn saved(address: &str) -> SavedServer {
        SavedServer {
            address: address.to_owned(),
            nickname: String::new(),
        }
    }

    #[test]
    fn resolve_addr_appends_default_port_and_rejects_empty() {
        // A bare loopback IP gets the default game port appended.
        let resolved = resolve_addr("127.0.0.1").expect("resolves");
        assert!(resolved.iter().any(|a| a.port() == DEFAULT_GAME_PORT));
        // An explicit port is honored as-is.
        let resolved = resolve_addr("127.0.0.1:6000").expect("resolves");
        assert!(resolved.iter().any(|a| a.port() == 6000));
        // Empty is an error, not a panic.
        assert!(resolve_addr("   ").is_err());
    }

    #[test]
    fn status_summary_reflects_each_query_state() {
        assert_eq!(status_summary(&QueryState::Querying), "querying…");
        let ok = QueryState::Done(Ok(ServerStatus {
            ping_ms: Some(12),
            version: Some("0.1.0".to_owned()),
            players: Some((3, 20)),
            motd: None,
        }));
        let summary = status_summary(&ok);
        assert!(summary.contains("12 ms"));
        assert!(summary.contains("0.1.0"));
        assert!(summary.contains("3/20"));
        let err = QueryState::Done(Err("connection refused".to_owned()));
        assert!(status_summary(&err).starts_with("unreachable"));
    }

    /// The heart of T56.31: three servers probed CONCURRENTLY, each row filled
    /// in independently as its own probe resolves — a full one (ping + parsed
    /// version/players banner), a ping-only one (reachable, no banner), and an
    /// unreachable one — all against real local sockets.
    #[test]
    fn concurrent_probes_populate_each_row_independently() {
        // The `IoTaskPool` the production code uses; init it directly
        // (no full Bevy `App` needed for this logic test).
        IoTaskPool::get_or_init(bevy::tasks::TaskPool::default);

        let full =
            spawn_probe_listener(Some("(version:\"9.9.9\",players:Some((1,5)),motd:\"hi\")"));
        let ping_only = spawn_probe_listener(None);
        let dead = unused_local_addr();

        let mut browser = ServerBrowser::default();
        browser.load_from(&[saved(&full), saved(&ping_only), saved(&dead)]);
        assert!(browser.needs_refresh, "load_from requests a query round");

        // First call dispatches all three probes concurrently; keep polling
        // until every row has resolved (or a generous timeout).
        let start = Instant::now();
        loop {
            poll_and_dispatch_probes(&mut browser);
            let all_done = browser
                .rows
                .iter()
                .all(|r| matches!(r.state, QueryState::Done(_)));
            if all_done {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "probes did not all resolve in time"
            );
            thread::sleep(Duration::from_millis(20));
        }

        // Row 0: full status — reachable, real ping, parsed version + players.
        match &browser.rows[0].state {
            QueryState::Done(Ok(status)) => {
                assert!(status.ping_ms.is_some(), "a reachable server has a ping");
                assert_eq!(status.version.as_deref(), Some("9.9.9"));
                assert_eq!(status.players, Some((1, 5)));
                assert_eq!(status.motd.as_deref(), Some("hi"));
            },
            other => panic!("row 0 expected full status, got {other:?}"),
        }
        // Row 1: reachable, ping only (no banner ⇒ version/players stay None).
        match &browser.rows[1].state {
            QueryState::Done(Ok(status)) => {
                assert!(status.ping_ms.is_some());
                assert_eq!(status.version, None);
                assert_eq!(status.players, None);
            },
            other => panic!("row 1 expected ping-only status, got {other:?}"),
        }
        // Row 2: unreachable.
        assert!(
            matches!(browser.rows[2].state, QueryState::Done(Err(_))),
            "row 2 (nothing listening) must be unreachable"
        );
    }

    /// `load_from` builds one row per saved server, selects the first, and
    /// requests a refresh — the state the browser opens in.
    #[test]
    fn load_from_builds_rows_and_selects_first() {
        let mut browser = ServerBrowser::default();
        browser.load_from(&[saved("a:1"), saved("b:2")]);
        assert_eq!(browser.rows.len(), 2);
        assert_eq!(browser.tasks.len(), 2);
        assert_eq!(browser.selected, Some(0));
        assert!(browser.needs_refresh);

        let mut empty = ServerBrowser::default();
        empty.load_from(&[]);
        assert!(empty.rows.is_empty());
        assert_eq!(empty.selected, None);
    }

    /// The add-form edits route into the address/nickname buffers and cycle
    /// focus (Tab), mirroring the login form's own behaviour.
    #[test]
    fn add_form_focus_cycles_and_edits() {
        let mut browser = ServerBrowser::default();
        assert_eq!(browser.add_focused, None);
        browser.cycle_add_focus();
        assert_eq!(browser.add_focused, Some(AddField::Address));
        browser.insert_add("host:14004");
        assert_eq!(browser.add_address, "host:14004");
        browser.cycle_add_focus();
        assert_eq!(browser.add_focused, Some(AddField::Nickname));
        browser.insert_add("My Server");
        assert_eq!(browser.add_nickname, "My Server");
        browser.backspace_add();
        assert_eq!(browser.add_nickname, "My Serve");
        // Wraps back to the first field.
        browser.cycle_add_focus();
        assert_eq!(browser.add_focused, Some(AddField::Address));
    }

    // -----------------------------------------------------------------------
    // zlayer audit (BL-82 EM-5.14 follow-up)
    // -----------------------------------------------------------------------

    /// [`MenuRoot`] is a full-screen, independently-positioned window (the
    /// main-menu screen owns the ENTIRE display while it's up — there is
    /// nothing else on screen underneath it to sit below, but it still must
    /// stay above whatever gameplay chrome may already exist from a prior
    /// session, hence the same `TOAST + 100` tier `ConnectingRoot` uses).
    /// Mirrors `trade_ui.rs`'s `invite_and_trade_window_roots_carry_the_
    /// modal_windows_z_index` pattern: pins that `build_menu` actually
    /// attaches the `GlobalZIndex`, not just that the doc comment claims it.
    #[test]
    fn menu_root_carries_a_z_index_above_every_hud_layer() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(MenuScreen::default());
        app.insert_resource(LoginForm::default());

        app.world_mut()
            .run_system_once(build_menu)
            .expect("build_menu runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<MenuRoot>>()
            .single(world)
            .expect("MenuRoot exists")
            .0;
        assert_eq!(z_index, zlayer::TOAST + 100);
    }

    /// [`ConnectingRoot`] (the offline-boot loading screen) must fully cover
    /// whatever gameplay chrome is already spawned behind it while the world
    /// boots — see [`build_connecting_screen`]'s own spawn-site comment.
    /// Calls [`build_connecting_screen`] directly (not the `enter_connecting`
    /// system) so the test never kicks off a real background world-boot
    /// thread — only the UI-spawn half is under test here.
    #[test]
    fn connecting_root_carries_a_z_index_above_every_hud_layer() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let theme = HudTheme::default();
        let fonts = HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        };
        let credits = Credits::default();

        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                build_connecting_screen(&mut commands, &theme, &fonts, None, "", &credits);
            })
            .expect("build_connecting_screen runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<ConnectingRoot>>()
            .single(world)
            .expect("ConnectingRoot exists")
            .0;
        assert_eq!(z_index, zlayer::TOAST + 100);
    }
}
