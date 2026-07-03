//! Xindeler client — pure Bevy ([Q3]=B: state arrives via replicon; no specs,
//! no xindeler-client-core embedded).
//!
//! Phase 2 (EM-2.2/2.3): windowed app with the HDR camera rig, light rig +
//! atmosphere, a procedural demo scene, and a `--smoke-screenshot <path.png>`
//! harness for autonomous render verification.

mod camera;
mod light;
mod scene;
mod smoke;

use bevy::{image::ImagePlugin, prelude::*, window::WindowResolution};
use xindeler_app::XindelerAppPlugin;

fn main() -> AppExit {
    let smoke_path = smoke::parse_smoke_screenshot_arg();

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            // Crisp voxel pixels: nearest min/mag globally (per-image
            // samplers can still override, e.g. the checker adds repeat).
            .set(ImagePlugin::default_nearest())
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
        scene::DemoScenePlugin,
    ));

    if let Some(path) = smoke_path {
        app.add_plugins(smoke::SmokeScreenshotPlugin { path });
    }

    app.run()
}
