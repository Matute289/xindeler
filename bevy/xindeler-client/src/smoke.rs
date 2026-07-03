//! Screenshot smoke harness (EM-2.2): `--smoke-screenshot <path.png>` runs
//! the app normally, waits ~90 frames for TAA to converge, captures a frame,
//! then exits. If the capture hasn't landed by 600 frames the app exits
//! non-zero.
//!
//! ## Why an offscreen render target and not `Screenshot::primary_window()`
//!
//! Window screenshots copy the camera's output for that frame, but when the
//! window is never composited by the OS (launched from a background/agent
//! context, hidden space, CI) macOS never presents it and the captured
//! attachment stays cleared — verified empirically: even a minimal upstream
//! scene captures pure black here, while an image target captures fine. So
//! in smoke mode we retarget the main camera to an offscreen `Image` (same
//! resolution as the window, full pipeline — TAA/SSAO/fog/bloom all run) and
//! `Screenshot::image` that. Interactive runs are unaffected (no flag = no
//! retarget).

use std::path::PathBuf;

use bevy::{
    camera::RenderTarget,
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    },
};

/// Frames to wait before capturing (TAA history convergence).
const WARMUP_FRAMES: u32 = 90;
/// Safety timeout: exit non-zero if the capture never completes.
const TIMEOUT_FRAMES: u32 = 600;
/// Offscreen target size (matches the default window resolution).
const TARGET_SIZE: (u32, u32) = (1280, 720);

/// Parses `--smoke-screenshot <path>` from `std::env::args` (no clap).
pub fn parse_smoke_screenshot_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--smoke-screenshot" {
            match args.next() {
                Some(path) => return Some(PathBuf::from(path)),
                None => {
                    eprintln!("--smoke-screenshot requires a <path.png> argument");
                    std::process::exit(2);
                },
            }
        }
    }
    None
}

pub struct SmokeScreenshotPlugin {
    pub path: PathBuf,
}

impl Plugin for SmokeScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SmokeScreenshot {
            path: self.path.clone(),
            target: Handle::default(),
            frames: 0,
            requested: false,
            captured: false,
        })
        // PostStartup: the camera rig spawns its camera in Startup.
        .add_systems(PostStartup, retarget_camera_to_image)
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
}

/// Points the main camera at an offscreen image so the capture is
/// independent of OS window compositing (see module docs).
fn retarget_camera_to_image(
    mut commands: Commands,
    mut state: ResMut<SmokeScreenshot>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<Entity, With<Camera3d>>,
) {
    let image = Image::new_target_texture(
        TARGET_SIZE.0,
        TARGET_SIZE.1,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    );
    let handle = images.add(image);
    state.target = handle.clone();
    for camera in &cameras {
        commands
            .entity(camera)
            .insert(RenderTarget::Image(handle.clone().into()));
    }
}

fn drive_smoke_screenshot(mut state: ResMut<SmokeScreenshot>, mut commands: Commands) {
    state.frames += 1;

    if state.captured {
        // `save_to_disk` already ran inside the same capture trigger, so the
        // file is on disk by now.
        info!("smoke screenshot saved to {}", state.path.display());
        commands.write_message(AppExit::Success);
        return;
    }

    if state.frames >= TIMEOUT_FRAMES {
        error!(
            "smoke screenshot timed out after {TIMEOUT_FRAMES} frames (path: {})",
            state.path.display()
        );
        commands.write_message(AppExit::error());
        return;
    }

    if !state.requested && state.frames >= WARMUP_FRAMES {
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
