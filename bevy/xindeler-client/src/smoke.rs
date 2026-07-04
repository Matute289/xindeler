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

use std::path::{Path, PathBuf};

use bevy::{
    camera::RenderTarget,
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
}

/// Parses `--smoke-screenshot <path.png>` / `--smoke-atmosphere <out_dir>`
/// from `std::env::args` (no clap).
pub fn parse_smoke_args() -> Option<SmokeMode> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mode = match arg.as_str() {
            "--smoke-screenshot" => SmokeMode::Screenshot as fn(PathBuf) -> SmokeMode,
            "--smoke-atmosphere" => SmokeMode::Atmosphere,
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

fn drive_smoke_screenshot(
    mut state: ResMut<SmokeScreenshot>,
    mut commands: Commands,
    upload_stats: Res<xindeler_render_voxel::pipeline::ChunkUploadStats>,
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

    if !state.requested && state.frames >= state.warmup_frames && terrain_ready {
        if state.listen_server {
            info!(
                "listen-server smoke: capturing after {} frames ({} chunks meshed)",
                state.frames, upload_stats.total_uploads
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
}
