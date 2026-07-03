//! Xindeler client — pure Bevy ([Q3]=B: state arrives via replicon; no specs,
//! no xindeler-client-core embedded).
//!
//! Phase 2 (EM-2.2/2.3/2.4/2.6): windowed app with the HDR camera rig, light
//! rig + atmosphere (data-driven, hot-reloadable), a vignette post-process
//! slot, a procedural demo scene, and two autonomous verification harnesses:
//! `--smoke-screenshot <path.png>` and `--smoke-atmosphere <out_dir>`.

mod atmosphere;
mod camera;
mod light;
mod post;
mod scene;
mod smoke;

use bevy::{asset::AssetPlugin, image::ImagePlugin, prelude::*, window::WindowResolution};
use xindeler_app::XindelerAppPlugin;

fn main() -> AppExit {
    let smoke_mode = smoke::parse_smoke_args();

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            // Crisp voxel pixels: nearest min/mag globally (per-image
            // samplers can still override, e.g. the checker adds repeat).
            .set(ImagePlugin::default_nearest())
            // Point bevy at the real game asset dir (XINDELER_ASSETS > VELOREN_ASSETS or
            // <cwd>/assets): the built-in default resolves relative to this
            // crate's manifest dir, not the workspace (see `assets_root`).
            // Hot reload: the `file_watcher` cargo feature compiles the
            // watcher in, but only DEV builds watch — a shipped client must
            // not fs-watch the entire asset tree (reviewer m1). The
            // --smoke-atmosphere harness runs dev, so the gate stays real.
            .set(AssetPlugin {
                file_path: atmosphere::assets_root().to_string_lossy().into_owned(),
                watch_for_changes_override: Some(cfg!(debug_assertions)),
                ..Default::default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Xindeler".to_owned(),
                    resolution: WindowResolution::new(1280, 720),
                    ..Default::default()
                }),
                ..Default::default()
            }),
    )
    .add_plugins((
        XindelerAppPlugin,
        camera::CameraRigPlugin,
        light::LightRigPlugin,
        atmosphere::AtmospherePlugin,
        post::PostProcessPlugin,
        scene::DemoScenePlugin,
    ));

    match smoke_mode {
        Some(smoke::SmokeMode::Screenshot(path)) => {
            app.add_plugins(smoke::SmokeScreenshotPlugin { path });
        },
        Some(smoke::SmokeMode::Atmosphere(out_dir)) => {
            app.add_plugins(smoke::SmokeAtmospherePlugin { out_dir });
        },
        None => {},
    }

    app.run()
}
