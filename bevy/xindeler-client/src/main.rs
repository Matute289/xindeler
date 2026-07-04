//! Xindeler client — pure Bevy ([Q3]=B: state arrives via replicon; no specs,
//! no xindeler-client-core embedded).
//!
//! Phase 2 (EM-2.2/2.3/2.4/2.6): windowed app with the HDR camera rig, light
//! rig + atmosphere (data-driven, hot-reloadable), a vignette post-process
//! slot, a procedural demo scene, and two autonomous verification harnesses:
//! `--smoke-screenshot <path.png>` and `--smoke-atmosphere <out_dir>`.
//!
//! EM-3.6: `--listen-server` (needs the `listen-server` cargo feature) boots
//! an embedded Veloren world in-process and streams its REAL terrain to the
//! mesh pipeline via replicon loopback, replacing the synthetic 5×5 demo.

mod atmosphere;
mod camera;
#[cfg(feature = "listen-server")] mod entity_view;
mod light;
#[cfg(feature = "listen-server")]
mod listen_server;
mod palette_material;
#[cfg(feature = "listen-server")]
mod player_input;
mod post;
mod scene;
mod smoke;
#[cfg(feature = "listen-server")]
mod terrain_stream;
mod voxel_demo;

use bevy::{asset::AssetPlugin, image::ImagePlugin, prelude::*, window::WindowResolution};
use xindeler_app::XindelerAppPlugin;
use xindeler_render_voxel::VoxelRenderPlugin;

fn main() -> AppExit {
    let smoke_mode = smoke::parse_smoke_args();
    // EM-3.6: `--listen-server` boots the embedded world and streams REAL
    // terrain instead of the synthetic 5×5 demo.
    let listen_server = std::env::args().any(|a| a == "--listen-server");
    #[cfg(not(feature = "listen-server"))]
    if listen_server {
        eprintln!(
            "--listen-server requires the `listen-server` cargo feature (rebuild with --features \
             listen-server)"
        );
        return AppExit::error();
    }

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
        // EM-3.3: VoxelMaterialExt registration + the async chunk pipeline.
        VoxelRenderPlugin,
    ));

    // The synthetic 5×5 demo and the real listen-server terrain are mutually
    // exclusive: both drive the SAME pipeline (one ChunkVolumeProvider), so
    // only one may install a provider.
    if listen_server {
        #[cfg(feature = "listen-server")]
        app.add_plugins(listen_server::ListenServerPlugin);
    } else {
        app.add_plugins(voxel_demo::VoxelDemoPlugin);
    }

    match smoke_mode {
        Some(smoke::SmokeMode::Screenshot(path)) => {
            app.add_plugins(smoke::SmokeScreenshotPlugin {
                path,
                // The world boot (~5–10 s) + terrain streaming needs a much
                // longer warmup than the static demo scene.
                listen_server,
            });
            // EM-3.7b smoke scaffolding: with no real keyboard, drive the
            // embedded player forward so the third-person camera shows it
            // walking on the real terrain in the capture. Listen-server only.
            #[cfg(feature = "listen-server")]
            if listen_server {
                app.add_plugins(player_input::SmokeAutoMovePlugin);
            }
        },
        Some(smoke::SmokeMode::Atmosphere(out_dir)) => {
            app.add_plugins(smoke::SmokeAtmospherePlugin { out_dir });
        },
        None => {},
    }

    app.run()
}
