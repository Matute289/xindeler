//! Autonomous render-verification harnesses (EM-2.2 / EM-2.4).
//!
//! Two modes, mutually exclusive:
//! - `--smoke-screenshot <path.png>`: run normally, wait ~90 frames for TAA to
//!   converge, capture one frame, exit (EM-2.2).
//! - `--smoke-atmosphere <out_dir>`: capture `atmo-before.png` on the default
//!   atmosphere profile, then OVERWRITE `assets/xindeler/atmosphere/
//!   default.atmo.ron` on disk with an extreme profile (red fog, high density,
//!   0.5 s transition), wait for the asset system to pick the change up
//!   (file_watcher; `AssetServer::reload` fallback after a grace period,
//!   honestly logged), let the transition settle ~120 frames, capture
//!   `atmo-after.png`, restore the original RON, and exit — non-zero unless
//!   BOTH PNGs exist non-empty on disk and the RON was restored (EM-2.4).
//!
//! ## Why an offscreen render target and not `Screenshot::primary_window()`
//!
//! Window screenshots copy the camera's output for that frame, but when the
//! window is never composited by the OS (launched from a background/agent
//! context, hidden space, CI) macOS never presents it and the captured
//! attachment stays cleared — verified empirically: even a minimal upstream
//! scene captures pure black here, while an image target captures fine. So
//! in smoke mode we retarget the main camera to an offscreen `Image` (same
//! resolution as the window, full pipeline — TAA/SSAO/fog/bloom/vignette all
//! run) and `Screenshot::image` that. Interactive runs are unaffected (no
//! flag = no retarget).

use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

use bevy::{
    camera::RenderTarget,
    diagnostic::{Diagnostic, DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
};
use xindeler_oracle_host::{AtmosphereController, AtmosphereProfile};

use crate::atmosphere::{PROFILE_ASSET_PATH, assets_root};

/// Frames to wait before the (first) capture (TAA history convergence).
const WARMUP_FRAMES: u32 = 90;
/// Safety timeout for the single-screenshot mode.
const TIMEOUT_FRAMES: u32 = 600;
/// EM-3.6 listen-server warmup: the embedded world boots (~5–10 s) then streams
/// terrain that meshes over several frames — far longer than the static demo.
/// A generous ceiling at ~30–60 fps; the frame-count gate is the safety net,
/// the real trigger is "warmup elapsed AND terrain has meshed" (see
/// [`drive_smoke_screenshot`]).
const LISTEN_SERVER_WARMUP_FRAMES: u32 = 1200;
/// Matching timeout for listen-server mode (cold asset I/O + boot can stall).
const LISTEN_SERVER_TIMEOUT_FRAMES: u32 = 6000;
/// Offscreen target size (matches the default window resolution).
const TARGET_SIZE: (u32, u32) = (1280, 720);

/// Frames to let the 0.5 s extreme transition finish + TAA re-converge
/// before the second capture.
const SETTLE_FRAMES: u32 = 120;
/// Frames to give the file watcher before falling back to an explicit
/// `AssetServer::reload` (~4 s at 60 fps; the notify debounce is well under
/// a second, so a real watcher fires long before this).
const RELOAD_FALLBACK_FRAMES: u32 = 240;
/// Frames to wait for ANY reload detection (watcher or fallback) before
/// giving up.
const RELOAD_TIMEOUT_FRAMES: u32 = 900;
/// Global safety net for the whole atmosphere sequence.
const ATMO_TIMEOUT_FRAMES: u32 = 3000;

/// Extreme profile written over `default.atmo.ron` mid-run. Deliberately
/// NOT fully opaque fog: near-field checkerboard detail must survive so the
/// after-PNG keeps real entropy (the harness gate rejects trivial images).
const EXTREME_PROFILE_RON: &str = r"// TEMPORARY smoke-harness profile (--smoke-atmosphere).
// If you are reading this in git, a smoke run died before restoring the
// original file: `git checkout -- assets/xindeler/atmosphere/default.atmo.ron`.
(
    fog_density: 0.04,
    fog_color: (1.0, 0.05, 0.05),
    fog_volume_density: 0.6,
    ambient_light_intensity: 0.4,
    weather_effect: Storm,
    transition_secs: 0.5,
)
";

/// CLI-selected smoke mode.
pub enum SmokeMode {
    Screenshot(PathBuf),
    Atmosphere(PathBuf),
    /// BL-82 EM-3.11n: `--smoke-perf-run <out.csv>` — see
    /// [`SmokePerfRunPlugin`].
    PerfRun(PathBuf),
}

/// Listen-server smoke gate (EM-3.7b): set `true` by the player rig once the
/// controllable player has spawned AND visibly moved, so the screenshot fires
/// on a frame that actually shows the walking character (not empty terrain).
/// Defined here (always compiled) so the always-compiled smoke driver can read
/// it; only the listen-server `player_input` rig ever WRITES it. Defaults to
/// `true` so non-listen-server smoke and any path that never installs the rig
/// are unaffected.
#[derive(Resource)]
pub struct SmokePlayerMoved(pub bool);

impl Default for SmokePlayerMoved {
    fn default() -> Self { Self(true) }
}

/// Parses `--smoke-screenshot <path.png>` / `--smoke-atmosphere <out_dir>`
/// from `std::env::args` (no clap).
pub fn parse_smoke_args() -> Option<SmokeMode> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mode = match arg.as_str() {
            "--smoke-screenshot" => SmokeMode::Screenshot as fn(PathBuf) -> SmokeMode,
            "--smoke-atmosphere" => SmokeMode::Atmosphere,
            "--smoke-perf-run" => SmokeMode::PerfRun,
            _ => continue,
        };
        match args.next() {
            Some(path) => return Some(mode(PathBuf::from(path))),
            None => {
                eprintln!("{arg} requires a path argument");
                std::process::exit(2);
            },
        }
    }
    None
}

/// Where the default profile lives ON DISK (the asset path is relative to
/// the asset source root main.rs hands to `AssetPlugin`).
fn profile_disk_path() -> PathBuf { assets_root().join(PROFILE_ASSET_PATH) }

/// Points the main camera at an offscreen image so captures are independent
/// of OS window compositing (see module docs). Returns the target handle.
fn retarget_camera_to_image(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    cameras: &Query<Entity, With<Camera3d>>,
) -> Handle<Image> {
    let image = Image::new_target_texture(
        TARGET_SIZE.0,
        TARGET_SIZE.1,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    );
    let handle = images.add(image);
    for camera in cameras {
        commands
            .entity(camera)
            .insert(RenderTarget::Image(handle.clone().into()));
    }
    handle
}

/// `true` iff `path` exists as a non-empty file. bevy's `save_to_disk`
/// SWALLOWS IO errors (logs + continues, bevy_render screenshot.rs), so
/// `ScreenshotCaptured` alone doesn't prove a file was written — the
/// harnesses must never exit 0 without real PNGs (reviewer major-1).
fn file_written(path: &Path) -> bool { std::fs::metadata(path).is_ok_and(|m| m.len() > 0) }

// ---------------------------------------------------------------------------
// Mode 1: --smoke-screenshot (EM-2.2)
// ---------------------------------------------------------------------------

pub struct SmokeScreenshotPlugin {
    pub path: PathBuf,
    /// EM-3.6: in listen-server mode, use the long warmup/timeout AND wait for
    /// real terrain to mesh before capturing.
    pub listen_server: bool,
}

impl Plugin for SmokeScreenshotPlugin {
    fn build(&self, app: &mut App) {
        let (warmup, timeout) = if self.listen_server {
            (LISTEN_SERVER_WARMUP_FRAMES, LISTEN_SERVER_TIMEOUT_FRAMES)
        } else {
            (WARMUP_FRAMES, TIMEOUT_FRAMES)
        };
        app.insert_resource(SmokeScreenshot {
            path: self.path.clone(),
            target: Handle::default(),
            frames: 0,
            requested: false,
            captured: false,
            listen_server: self.listen_server,
            warmup_frames: warmup,
            timeout_frames: timeout,
        })
        // Listen-server capture PREFERS a frame where the player has walked;
        // start `false` there and let the rig raise it. If the player never
        // spawns (spectator fallback), the driver falls back to a terrain-only
        // capture after `PLAYER_WAIT_FRAMES` so the smoke never wrongly fails.
        .insert_resource(SmokePlayerMoved(!self.listen_server))
        // PostStartup: the camera rig spawns its camera in Startup.
        .add_systems(PostStartup, retarget_for_screenshot)
        .add_systems(Update, drive_smoke_screenshot);
    }
}

#[derive(Resource)]
struct SmokeScreenshot {
    path: PathBuf,
    target: Handle<Image>,
    frames: u32,
    requested: bool,
    captured: bool,
    listen_server: bool,
    warmup_frames: u32,
    timeout_frames: u32,
}

fn retarget_for_screenshot(
    mut commands: Commands,
    mut state: ResMut<SmokeScreenshot>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<Entity, With<Camera3d>>,
) {
    state.target = retarget_camera_to_image(&mut commands, &mut images, &cameras);
}

/// Extra frames past the warmup we give the embedded player to spawn + walk
/// before falling back to a terrain-only listen-server capture (so a failed /
/// slow player never turns into a false smoke failure).
const PLAYER_WAIT_FRAMES: u32 = 900;

fn drive_smoke_screenshot(
    mut state: ResMut<SmokeScreenshot>,
    mut commands: Commands,
    upload_stats: Res<xindeler_render_voxel::pipeline::ChunkUploadStats>,
    player_moved: Res<SmokePlayerMoved>,
) {
    state.frames += 1;

    if state.captured {
        if file_written(&state.path) {
            info!("smoke screenshot saved to {}", state.path.display());
            commands.write_message(AppExit::Success);
        } else {
            error!(
                "screenshot capture completed but no file was written at {} (unwritable path / \
                 bad extension?)",
                state.path.display()
            );
            commands.write_message(AppExit::error());
        }
        return;
    }

    if state.frames >= state.timeout_frames {
        error!(
            "smoke screenshot timed out after {} frames (path: {}, meshed chunks: {})",
            state.timeout_frames,
            state.path.display(),
            upload_stats.total_uploads,
        );
        commands.write_message(AppExit::error());
        return;
    }

    // In listen-server mode the warmup covers world boot + streaming, but the
    // exact time-to-first-mesh is data-dependent — so ALSO require that real
    // terrain has actually meshed (and the pipeline has drained, so no chunks
    // pop in mid-capture) before firing.
    let terrain_ready =
        !state.listen_server || (upload_stats.total_uploads > 0 && upload_stats.in_flight == 0);

    // EM-3.7b: prefer a frame where the controllable player has spawned + moved
    // (the rig raises `SmokePlayerMoved`). If it hasn't happened within a
    // generous window past the warmup, fall back to a terrain-only capture so a
    // slow/failed player never turns into a false smoke failure.
    let player_ready = player_moved.0 || state.frames >= state.warmup_frames + PLAYER_WAIT_FRAMES;

    if !state.requested && state.frames >= state.warmup_frames && terrain_ready && player_ready {
        if state.listen_server {
            info!(
                "listen-server smoke: capturing after {} frames ({} chunks meshed, \
                 player_moved={})",
                state.frames, upload_stats.total_uploads, player_moved.0
            );
        }
        state.requested = true;
        commands
            .spawn(Screenshot::image(state.target.clone()))
            .observe(save_to_disk(state.path.clone()))
            .observe(
                |_: On<ScreenshotCaptured>, mut state: ResMut<SmokeScreenshot>| {
                    state.captured = true;
                },
            );
    }
}

// ---------------------------------------------------------------------------
// Mode 2: --smoke-atmosphere (EM-2.4)
// ---------------------------------------------------------------------------

pub struct SmokeAtmospherePlugin {
    pub out_dir: PathBuf,
}

impl Plugin for SmokeAtmospherePlugin {
    fn build(&self, app: &mut App) {
        if let Err(err) = std::fs::create_dir_all(&self.out_dir) {
            eprintln!(
                "--smoke-atmosphere: cannot create {} ({err})",
                self.out_dir.display()
            );
            std::process::exit(2);
        }
        app.insert_resource(SmokeAtmosphere {
            out_dir: self.out_dir.clone(),
            target: Handle::default(),
            frames: 0,
            phase: AtmoPhase::WarmupA,
            phase_frames: 0,
            original_ron: None,
            reload_requested: false,
            detected_via: None,
            captured: false,
        })
        .add_systems(PostStartup, retarget_for_atmosphere)
        .add_systems(Update, drive_smoke_atmosphere);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtmoPhase {
    /// Waiting out TAA warmup on the default profile.
    WarmupA,
    /// `atmo-before.png` requested; waiting for the capture.
    CapturingA,
    /// RON swapped on disk; waiting for `AssetEvent::Modified`.
    AwaitReload,
    /// Reload detected; letting the transition + TAA settle.
    Settle,
    /// `atmo-after.png` requested; waiting for the capture.
    CapturingB,
}

#[derive(Resource)]
struct SmokeAtmosphere {
    out_dir: PathBuf,
    target: Handle<Image>,
    frames: u32,
    phase: AtmoPhase,
    phase_frames: u32,
    /// Original bytes of `default.atmo.ron`, held while the extreme profile
    /// is on disk. `Some` == "we owe a restore".
    original_ron: Option<String>,
    reload_requested: bool,
    detected_via: Option<&'static str>,
    captured: bool,
}

impl SmokeAtmosphere {
    fn before_path(&self) -> PathBuf { self.out_dir.join("atmo-before.png") }

    fn after_path(&self) -> PathBuf { self.out_dir.join("atmo-after.png") }

    /// Puts the original RON back. Returns false if the write failed (the
    /// run must then exit non-zero and say so).
    fn restore_ron(&mut self) -> bool {
        let Some(original) = self.original_ron.take() else {
            return true;
        };
        match std::fs::write(profile_disk_path(), &original) {
            Ok(()) => {
                info!("restored original {}", profile_disk_path().display());
                true
            },
            Err(err) => {
                self.original_ron = Some(original);
                error!(
                    "FAILED to restore {} ({err}) — restore it manually (git checkout)",
                    profile_disk_path().display()
                );
                false
            },
        }
    }

    fn enter(&mut self, phase: AtmoPhase) {
        self.phase = phase;
        self.phase_frames = 0;
        self.captured = false;
    }
}

/// Last-resort restore: covers unwind paths (window closed, panic in another
/// system) where the driver never reached its own restore call.
impl Drop for SmokeAtmosphere {
    fn drop(&mut self) {
        if let Some(original) = self.original_ron.take()
            && let Err(err) = std::fs::write(profile_disk_path(), &original)
        {
            eprintln!(
                "smoke-atmosphere: FAILED to restore {} on drop ({err})",
                profile_disk_path().display()
            );
        }
    }
}

fn retarget_for_atmosphere(
    mut commands: Commands,
    mut state: ResMut<SmokeAtmosphere>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<Entity, With<Camera3d>>,
) {
    state.target = retarget_camera_to_image(&mut commands, &mut images, &cameras);
}

fn request_capture(commands: &mut Commands, target: Handle<Image>, path: PathBuf) {
    commands
        .spawn(Screenshot::image(target))
        .observe(save_to_disk(path))
        .observe(
            |_: On<ScreenshotCaptured>, mut state: ResMut<SmokeAtmosphere>| {
                state.captured = true;
            },
        );
}

fn fail(state: &mut SmokeAtmosphere, commands: &mut Commands, why: &str) {
    error!("--smoke-atmosphere FAILED: {why}");
    state.restore_ron();
    commands.write_message(AppExit::error());
}

#[expect(clippy::too_many_lines, reason = "linear phase machine, clearer flat")]
fn drive_smoke_atmosphere(
    mut state: ResMut<SmokeAtmosphere>,
    mut commands: Commands,
    mut events: MessageReader<AssetEvent<AtmosphereProfile>>,
    controller: Res<AtmosphereController>,
    asset_server: Res<AssetServer>,
) {
    state.frames += 1;
    state.phase_frames += 1;

    if state.frames >= ATMO_TIMEOUT_FRAMES {
        let why = format!(
            "global timeout after {ATMO_TIMEOUT_FRAMES} frames (phase {:?})",
            state.phase
        );
        fail(&mut state, &mut commands, &why);
        return;
    }

    match state.phase {
        AtmoPhase::WarmupA => {
            if state.phase_frames >= WARMUP_FRAMES {
                let path = state.before_path();
                info!("capturing baseline {}", path.display());
                request_capture(&mut commands, state.target.clone(), path);
                state.enter(AtmoPhase::CapturingA);
            }
        },
        AtmoPhase::CapturingA => {
            if state.captured {
                if !file_written(&state.before_path()) {
                    let why = format!("{} was not written", state.before_path().display());
                    fail(&mut state, &mut commands, &why);
                    return;
                }
                // Swap the profile on disk. Keep the original bytes in memory
                // AND rely on Drop as the unwind-path safety net.
                let disk = profile_disk_path();
                match std::fs::read_to_string(&disk) {
                    Ok(original) => state.original_ron = Some(original),
                    Err(err) => {
                        let why = format!("cannot read {} ({err})", disk.display());
                        fail(&mut state, &mut commands, &why);
                        return;
                    },
                }
                if let Err(err) = std::fs::write(&disk, EXTREME_PROFILE_RON) {
                    let why = format!("cannot write extreme profile to {} ({err})", disk.display());
                    fail(&mut state, &mut commands, &why);
                    return;
                }
                info!(
                    "wrote extreme profile over {}; waiting for hot reload",
                    disk.display()
                );
                state.enter(AtmoPhase::AwaitReload);
            } else if state.phase_frames >= TIMEOUT_FRAMES {
                fail(&mut state, &mut commands, "baseline capture timed out");
            }
        },
        AtmoPhase::AwaitReload => {
            let modified = events.read().any(|event| {
                matches!(
                    event,
                    AssetEvent::Modified { id } | AssetEvent::Added { id }
                        if *id == controller.handle.id()
                )
            });
            if modified {
                let via = if state.reload_requested {
                    // Ambiguous in the worst case (watcher may land right
                    // after the fallback fired) — report the fallback since
                    // we can no longer attribute the event to the watcher.
                    "AssetServer::reload fallback"
                } else {
                    "file_watcher"
                };
                state.detected_via = Some(via);
                info!("profile change detected via {via}; settling {SETTLE_FRAMES} frames");
                state.enter(AtmoPhase::Settle);
            } else if state.phase_frames >= RELOAD_TIMEOUT_FRAMES {
                fail(
                    &mut state,
                    &mut commands,
                    "no AssetEvent::Modified after the RON swap (watcher AND reload fallback both \
                     failed)",
                );
            } else if !state.reload_requested && state.phase_frames >= RELOAD_FALLBACK_FRAMES {
                warn!(
                    "file_watcher did not fire within {RELOAD_FALLBACK_FRAMES} frames; falling \
                     back to AssetServer::reload"
                );
                state.reload_requested = true;
                asset_server.reload(PROFILE_ASSET_PATH);
            }
        },
        AtmoPhase::Settle => {
            if state.phase_frames >= SETTLE_FRAMES {
                let path = state.after_path();
                info!("capturing post-transition {}", path.display());
                request_capture(&mut commands, state.target.clone(), path);
                state.enter(AtmoPhase::CapturingB);
            }
        },
        AtmoPhase::CapturingB => {
            if state.captured {
                if !file_written(&state.after_path()) {
                    let why = format!("{} was not written", state.after_path().display());
                    fail(&mut state, &mut commands, &why);
                    return;
                }
                if !state.restore_ron() {
                    commands.write_message(AppExit::error());
                    return;
                }
                info!(
                    "--smoke-atmosphere OK: {} + {} (hot reload via {})",
                    state.before_path().display(),
                    state.after_path().display(),
                    state.detected_via.unwrap_or("<unknown>"),
                );
                commands.write_message(AppExit::Success);
            } else if state.phase_frames >= TIMEOUT_FRAMES {
                fail(
                    &mut state,
                    &mut commands,
                    "post-transition capture timed out",
                );
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Mode 3: --smoke-perf-run (BL-82 EM-3.11n)
// ---------------------------------------------------------------------------

/// BL-82 EM-3.11n — a scripted, repeatable straight-vs-diagonal frame-time A/B
/// harness (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md`, round 8:
/// Matías reported diagonal movement feels distinctly choppier than straight
/// movement). Pairs with [`crate::player_input::SmokeMovePattern`]
/// (`XINDELER_SMOKE_MOVE_PATTERN=straight|diagonal`): run the SAME command
/// twice, once per pattern, from the same fresh-world spawn, and diff the two
/// CSVs this writes.
///
/// Runs `--listen-server` (a real embedded world + terrain stream — this is
/// meaningless against the synthetic demo), waits out the usual
/// world-boot/first-mesh warmup (same gate `SmokeScreenshotPlugin` uses:
/// terrain meshed + player spawned/walked, so both conditions start measuring
/// from an equivalent state), THEN records every raw (unsmoothed) per-frame
/// `FrameTimeDiagnosticsPlugin` sample for `XINDELER_PERF_RUN_SECS` seconds
/// (default [`PERF_RUN_DURATION_SECS_DEFAULT`]) while [`SmokeAutoMovePlugin`]
/// (`crate::player_input`) drives the walk, and writes them to `out_csv` (one
/// `frame_index,frame_time_ms` row per sample) plus logs a summary
/// (mean/min/max/p50/p95/p99/stdev) via `target: "smoke_perf_run"` so a plain
/// `2>&1 | grep smoke_perf_run` captures it without parsing the CSV. No
/// pinned pass/fail threshold — frame times are display/vsync/scene-dependent
/// (`XINDELER_PRESENT_MODE`, EM-3.11b), so this is a MEASUREMENT harness, not
/// an assertion; compare the two CSVs' distributions by hand or with an
/// external script.
pub struct SmokePerfRunPlugin {
    pub out_csv: PathBuf,
}

/// Default measurement window (seconds), overridable via
/// `XINDELER_PERF_RUN_SECS` — long enough to cross several chunk boundaries at
/// a normal walk speed, short enough for a quick local A/B.
const PERF_RUN_DURATION_SECS_DEFAULT: f32 = 30.0;

fn perf_run_duration_secs() -> f32 {
    std::env::var("XINDELER_PERF_RUN_SECS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(PERF_RUN_DURATION_SECS_DEFAULT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PerfRunPhase {
    /// World boot + first-mesh + player-spawn gate (mirrors
    /// `SmokeScreenshotPlugin`'s listen-server gate).
    Warmup,
    /// Recording raw per-frame samples for `duration_secs`.
    Measuring,
}

#[derive(Resource)]
struct SmokePerfRun {
    out_csv: PathBuf,
    frames: u32,
    phase: PerfRunPhase,
    phase_frames: u32,
    duration_secs: f32,
    elapsed_secs: f32,
    /// Raw (unsmoothed) per-frame `FrameTimeDiagnosticsPlugin` samples, ms.
    samples: Vec<f64>,
    /// Wall-clock (Unix epoch ms) at the moment each `samples` entry was
    /// recorded (BL-82 EM-3.11p). Frame-index alone can't be correlated
    /// against `RUST_LOG` timestamps from OTHER systems (e.g. a sim-side
    /// "slow system execution" warning, or a region-crossing entity-sync
    /// burst) — this closes that gap so a future investigation can line a
    /// frame-time spike up against whatever else was logged at that instant
    /// without guessing from frame count × average fps.
    sample_times_ms: Vec<u64>,
}

impl Plugin for SmokePerfRunPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SmokePerfRun {
            out_csv: self.out_csv.clone(),
            frames: 0,
            phase: PerfRunPhase::Warmup,
            phase_frames: 0,
            duration_secs: perf_run_duration_secs(),
            elapsed_secs: 0.0,
            samples: Vec::new(),
            sample_times_ms: Vec::new(),
        })
        // Same gate SmokeScreenshotPlugin uses in listen-server mode: prefer
        // waiting for the player rig to confirm real movement.
        .insert_resource(SmokePlayerMoved(false))
        .add_systems(Update, drive_smoke_perf_run);
    }
}

/// Aggregate stats over one measurement window — logged, not asserted (frame
/// times are inherently noisy/display-dependent, see the plugin docs).
#[derive(Debug)]
struct PerfSummary {
    count: usize,
    mean_ms: f64,
    min_ms: f64,
    max_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    stdev_ms: f64,
}

fn summarize(samples: &[f64]) -> PerfSummary {
    if samples.is_empty() {
        return PerfSummary {
            count: 0,
            mean_ms: f64::NAN,
            min_ms: f64::NAN,
            max_ms: f64::NAN,
            p50_ms: f64::NAN,
            p95_ms: f64::NAN,
            p99_ms: f64::NAN,
            stdev_ms: f64::NAN,
        };
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let count = sorted.len();
    let mean = sorted.iter().sum::<f64>() / count as f64;
    let variance = sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / count as f64;
    let percentile = |p: f64| -> f64 {
        let idx = ((p * (count - 1) as f64).round() as usize).min(count - 1);
        sorted[idx]
    };
    PerfSummary {
        count,
        mean_ms: mean,
        min_ms: sorted[0],
        max_ms: sorted[count - 1],
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        p99_ms: percentile(0.99),
        stdev_ms: variance.sqrt(),
    }
}

/// `sample_times_ms` (BL-82 EM-3.11p): wall-clock (Unix epoch ms) per sample,
/// so a spike row can be lined up against `RUST_LOG` timestamps from other
/// systems (sim-side "slow system execution", a region-crossing entity-sync
/// burst, etc.) without guessing from frame count × average fps. Empty (or
/// shorter than `samples`) is tolerated — callers/tests that only care about
/// `frame_time_ms` can pass `&[]` and get `0` in that column.
fn write_perf_csv(path: &Path, samples: &[f64], sample_times_ms: &[u64]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    writeln!(file, "frame_index,frame_time_ms,epoch_ms")?;
    for (i, ms) in samples.iter().enumerate() {
        let epoch_ms = sample_times_ms.get(i).copied().unwrap_or(0);
        writeln!(file, "{i},{ms},{epoch_ms}")?;
    }
    Ok(())
}

/// Current wall-clock time as Unix epoch milliseconds, saturating to `0` in
/// the practically-impossible case the system clock predates the epoch.
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn drive_smoke_perf_run(
    mut state: ResMut<SmokePerfRun>,
    mut commands: Commands,
    upload_stats: Res<xindeler_render_voxel::pipeline::ChunkUploadStats>,
    player_moved: Res<SmokePlayerMoved>,
    diagnostics: Res<DiagnosticsStore>,
    time: Res<Time>,
) {
    state.frames += 1;
    state.phase_frames += 1;

    match state.phase {
        PerfRunPhase::Warmup => {
            let terrain_ready = upload_stats.total_uploads > 0 && upload_stats.in_flight == 0;
            let player_ready = player_moved.0
                || state.phase_frames >= LISTEN_SERVER_WARMUP_FRAMES + PLAYER_WAIT_FRAMES;
            if state.phase_frames >= LISTEN_SERVER_WARMUP_FRAMES && terrain_ready && player_ready {
                info!(
                    "smoke-perf-run: warmup done after {} frames ({} chunks meshed, \
                     player_moved={}) — measuring for {}s",
                    state.phase_frames,
                    upload_stats.total_uploads,
                    player_moved.0,
                    state.duration_secs
                );
                state.phase = PerfRunPhase::Measuring;
                state.phase_frames = 0;
                state.elapsed_secs = 0.0;
                state.samples.clear();
                state.sample_times_ms.clear();
            } else if state.frames >= LISTEN_SERVER_TIMEOUT_FRAMES {
                error!(
                    "smoke-perf-run: warmup timed out after {} frames",
                    state.frames
                );
                commands.write_message(AppExit::error());
            }
        },
        PerfRunPhase::Measuring => {
            state.elapsed_secs += time.delta_secs();
            if let Some(ms) = diagnostics
                .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
                .and_then(Diagnostic::value)
            {
                state.samples.push(ms);
                state.sample_times_ms.push(now_epoch_ms());
            }
            if state.elapsed_secs >= state.duration_secs {
                let summary = summarize(&state.samples);
                // Log each field explicitly (rather than `?summary`) so a
                // plain `grep smoke_perf_run` line has every stat readable
                // without needing to know `PerfSummary`'s `Debug` layout.
                info!(
                    target: "smoke_perf_run",
                    count = summary.count,
                    mean_ms = summary.mean_ms,
                    min_ms = summary.min_ms,
                    max_ms = summary.max_ms,
                    p50_ms = summary.p50_ms,
                    p95_ms = summary.p95_ms,
                    p99_ms = summary.p99_ms,
                    stdev_ms = summary.stdev_ms,
                    "smoke-perf-run complete"
                );
                match write_perf_csv(&state.out_csv, &state.samples, &state.sample_times_ms) {
                    Ok(()) => {
                        info!(
                            "smoke-perf-run: wrote {} samples to {}",
                            state.samples.len(),
                            state.out_csv.display()
                        );
                        commands.write_message(AppExit::Success);
                    },
                    Err(err) => {
                        error!(
                            "smoke-perf-run: failed to write {} ({err})",
                            state.out_csv.display()
                        );
                        commands.write_message(AppExit::error());
                    },
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extreme_profile_parses_and_differs_from_default() {
        let extreme: AtmosphereProfile =
            ron::from_str(EXTREME_PROFILE_RON).expect("extreme profile RON parses");
        let default = AtmosphereProfile::default();
        assert_ne!(extreme, default);
        assert!(extreme.fog_density > default.fog_density * 2.0);
        assert!((extreme.transition_secs - 0.5).abs() < f32::EPSILON);
        assert_eq!(extreme.fog_color, [1.0, 0.05, 0.05]);
    }

    /// [`summarize`] on a known small sample set: mean/min/max/percentiles
    /// match hand-computed values, and stdev is 0 for a constant series.
    #[test]
    fn summarize_known_samples() {
        let samples = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let s = summarize(&samples);
        assert_eq!(s.count, 5);
        assert!((s.mean_ms - 30.0).abs() < 1e-9);
        assert_eq!(s.min_ms, 10.0);
        assert_eq!(s.max_ms, 50.0);
        assert_eq!(s.p50_ms, 30.0);

        let flat = vec![16.6; 100];
        let flat_summary = summarize(&flat);
        assert!(
            flat_summary.stdev_ms < 1e-9,
            "constant series has zero stdev"
        );
    }

    #[test]
    fn summarize_empty_is_nan_not_panic() {
        let s = summarize(&[]);
        assert_eq!(s.count, 0);
        assert!(s.mean_ms.is_nan());
    }

    /// [`write_perf_csv`] produces a header + one row per sample, parseable
    /// back out — this is the file the straight-vs-diagonal comparison reads.
    #[test]
    fn perf_csv_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("xindeler-perf-csv-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.csv");
        write_perf_csv(&path, &[16.0, 33.3, 8.0], &[1_000, 1_016, 1_049]).expect("write succeeds");
        let contents = std::fs::read_to_string(&path).expect("file exists");
        let mut lines = contents.lines();
        assert_eq!(lines.next(), Some("frame_index,frame_time_ms,epoch_ms"));
        assert_eq!(lines.next(), Some("0,16,1000"));
        assert_eq!(lines.next(), Some("1,33.3,1016"));
        assert_eq!(lines.next(), Some("2,8,1049"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// BL-82 EM-3.11p: a shorter/empty `sample_times_ms` (a caller that only
    /// cares about `frame_time_ms`, e.g. an older harness or a unit test)
    /// must not panic — missing timestamps fall back to `0`, not an
    /// out-of-bounds index.
    #[test]
    fn perf_csv_tolerates_missing_timestamps() {
        let dir = std::env::temp_dir().join(format!(
            "xindeler-perf-csv-notime-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.csv");
        write_perf_csv(&path, &[16.0, 33.3], &[]).expect("write succeeds even with no timestamps");
        let contents = std::fs::read_to_string(&path).expect("file exists");
        let mut lines = contents.lines();
        assert_eq!(lines.next(), Some("frame_index,frame_time_ms,epoch_ms"));
        assert_eq!(lines.next(), Some("0,16,0"));
        assert_eq!(lines.next(), Some("1,33.3,0"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
