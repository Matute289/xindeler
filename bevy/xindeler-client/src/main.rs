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
//!
//! EM-4.2b: `--connect <addr>` (needs the `net-client` cargo feature) is a
//! genuinely REMOTE client role — no embedded sim, no `xindeler-sim-bridge` —
//! connecting over a real network socket (via `xindeler-transport`) to a
//! SEPARATE `xindeler-server-app` process. It reuses the SAME client-side
//! consumption modules (`terrain_stream`, `entity_view`, `figure_view`,
//! `sprite_view`, `lod`, `far_terrain`, `far_terrain_material`)
//! `--listen-server` uses, verbatim — the
//! wire shape (`xindeler-protocol`) is unchanged, only the transport
//! underneath it and who hosts the sim. See `net_client.rs`.

mod atmosphere;
mod camera;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod combat_hud;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod entity_view;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod far_terrain;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod far_terrain_material;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod figure_view;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod hud_toast;
mod light;
#[cfg(feature = "listen-server")]
mod listen_server;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod lod;
#[cfg(feature = "net-client")] mod net_client;
mod palette_material;
mod perf_log;
#[cfg(feature = "listen-server")]
mod player_input;
mod post;
mod scene;
mod smoke;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod social_hud;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod sprite_view;
#[cfg(any(feature = "listen-server", feature = "net-client"))]
mod terrain_stream;
mod voxel_demo;

use bevy::{
    asset::AssetPlugin,
    image::ImagePlugin,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use xindeler_app::XindelerAppPlugin;
use xindeler_render_voxel::VoxelRenderPlugin;

/// Window present mode, overridable via `XINDELER_PRESENT_MODE` (EM-3.11b,
/// default corrected in EM-3.11m — see below).
///
/// ## The actual Bevy 0.19 default (verified against source, not memory)
/// `bevy_window::window::PresentMode` derives `Default` with `#[default]` on
/// its `Fifo` variant, and `Window::default()` sets
/// `present_mode: Default::default()` (both in
/// `bevy_window-0.19.0/src/window.rs`) — so a vanilla, untouched Bevy
/// `Window` resolves to [`PresentMode::Fifo`], **not** `AutoVsync`. `Fifo`'s
/// own doc comment is unambiguous: "No tearing will be observed. ... If you
/// don't know what mode to choose, choose this mode. This is traditionally
/// called 'Vsync On'." It is also the only mode in the enum with an
/// *unconditional* no-tearing guarantee and is supported on every platform.
///
/// The EM-3.11b comment that used to sit here mistakenly asserted the
/// vanilla default was `AutoVsync` and then used that same (wrong) value as
/// this override's "unset" fallback — so this client was actually booting
/// with `AutoVsync` explicitly requested, not Bevy's real default. That
/// matters for EM-3.11m (mouse-look tearing report) because `AutoVsync`
/// picks [`PresentMode::FifoRelaxed`] when the backend advertises it, and
/// `FifoRelaxed`'s own doc says tearing *will* be observed "if frames last
/// more than one vblank as the front buffer" — precisely what a slightly-late
/// frame during fast camera rotation looks like. `FifoRelaxed` is documented
/// as AMD/Vulkan-specific, so on this machine's macOS/Metal backend
/// `AutoVsync` most likely still resolved to plain `Fifo` in practice — but
/// "most likely, pending backend capability detection" is not a foundation
/// to leave under a tearing bug's default path when an unconditional
/// guarantee (`Fifo`) is one line away.
///
/// EM-3.11m: the unset/unrecognised fallback now requests `Fifo` explicitly.
/// Trade-off considered and accepted: `Fifo` can add up to ~1 frame of input
/// latency versus `Mailbox`/`Immediate` (`get_current_texture` blocks until
/// the presentation queue has a free slot) — but that is the standard "Vsync
/// On" cost every frame-capped game ships with by default, it's Bevy's own
/// recommended "if you don't know what to choose" mode, and visible tearing
/// during mouse-look is the worse of the two UX problems being weighed here.
/// Kept as an env override (not a `GraphicsSettings` field yet — same
/// deferral as `OcclusionCullingConfig` on `camera.rs`; `xindeler-app`
/// settings integration is out of scope here) so re-testing other modes
/// doesn't need a code change: `XINDELER_PRESENT_MODE=novsync`/`immediate`
/// for no-sync, `mailbox`/`fifo_relaxed`/`fifo`/`vsync` as before, and
/// `auto`/`autovsync` to explicitly opt back into the old (no-longer-default)
/// `AutoVsync` behaviour for future A/B testing.
///
/// ## EM-3.11b measurement (kept for history — a *different* symptom: fps,
/// not tearing)
/// See `docs/backlog/engine-migration.md` EM-3.11b for the before/after
/// average-frame-time numbers measured on this scene; that investigation
/// found vsync on/off made "no measurable difference" in average frame time.
/// It did not measure tearing, which is a temporal/per-frame artifact
/// independent of the average fps — see EM-3.11m for that half of the story.
fn present_mode_from_env() -> PresentMode {
    match std::env::var("XINDELER_PRESENT_MODE").as_deref() {
        Ok("novsync" | "immediate") => PresentMode::AutoNoVsync,
        Ok("mailbox") => PresentMode::Mailbox,
        Ok("fifo_relaxed") => PresentMode::FifoRelaxed,
        Ok("fifo" | "vsync") => PresentMode::Fifo,
        Ok("auto" | "autovsync") => PresentMode::AutoVsync,
        // Unset or anything unrecognised: request a real, universally
        // supported synced mode (Bevy's own true default) rather than the
        // tearing-prone AutoVsync/FifoRelaxed fallback path (EM-3.11m).
        _ => PresentMode::Fifo,
    }
}

/// `--connect <addr>` value, if present (EM-4.2b). A trailing flag with no
/// following argument is treated as absent (falls through to the synthetic
/// demo) rather than panicking — the same tolerant posture `--listen-server`
/// (a plain boolean flag) already has.
fn connect_addr_from_args() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--connect")
        .and_then(|i| args.get(i + 1))
        .cloned()
}

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

    // EM-4.2b: `--connect <addr>` is a genuinely remote client role — see
    // `net_client.rs`. Mutually exclusive with `--listen-server` (checked
    // below, alongside the synthetic-demo fallback).
    let connect_addr = connect_addr_from_args();
    #[cfg(not(feature = "net-client"))]
    if connect_addr.is_some() {
        eprintln!(
            "--connect requires the `net-client` cargo feature (rebuild with --features \
             net-client)"
        );
        return AppExit::error();
    }
    if listen_server && connect_addr.is_some() {
        eprintln!("--listen-server and --connect are mutually exclusive");
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
                    present_mode: present_mode_from_env(),
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
        // EM-3.10b: opt-in (`XINDELER_PERF_LOG=1`) periodic frame-time log,
        // used to measure the GPU occlusion-culling toggle; a no-op system
        // otherwise.
        perf_log::PerfLogPlugin,
    ));

    // The synthetic 5×5 demo, the listen-server's embedded terrain, and the
    // EM-4.2b remote net-client are mutually exclusive: all three drive the
    // SAME pipeline (one ChunkVolumeProvider), so only one may install a
    // provider.
    if listen_server {
        #[cfg(feature = "listen-server")]
        app.add_plugins(listen_server::ListenServerPlugin);
    } else if let Some(addr) = connect_addr.as_deref() {
        #[cfg(feature = "net-client")]
        {
            match addr.parse::<std::net::SocketAddr>() {
                Ok(server_addr) => {
                    app.add_plugins(net_client::NetClientPlugin {
                        config: xindeler_transport::TransportConfig::client(server_addr),
                    });
                },
                Err(err) => {
                    eprintln!("--connect: invalid address {addr:?}: {err}");
                    return AppExit::error();
                },
            }
        }
        #[cfg(not(feature = "net-client"))]
        {
            // Unreachable: the early `#[cfg(not(feature = "net-client"))]`
            // check above already returned. Kept exhaustive so this `if let`
            // arm type-checks identically regardless of feature set.
            let _ = addr;
        }
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
                // BL-82 EM-3.12: dedicated framing for the camera-collision
                // bug repro (Matías's "miro al personaje desde abajo" report
                // — looking up at the character from below, camera passing
                // through the floor). Opt-in (`XINDELER_SMOKE_CAMERA_
                // COLLISION=1`) and mutually exclusive with the figure/
                // sprite/water framing below: all of those override the
                // camera `Transform` directly AFTER `third_person_camera`
                // too, so whichever group runs would otherwise clobber the
                // other's shot.
                let camera_collision_smoke =
                    std::env::var("XINDELER_SMOKE_CAMERA_COLLISION").is_ok_and(|v| v != "0");
                if camera_collision_smoke {
                    app.add_plugins(player_input::SmokeCameraCollisionPlugin);
                } else {
                    // EM-3.8: frame a real `.vox` NPC figure in the capture
                    // (the spawn pillar occludes the player — EM-3.7b caveat).
                    app.add_plugins(player_input::SmokeFigureCamPlugin);
                    // EM-3.9: once sprites build, override the framing to the
                    // densest vegetation patch on open lit terrain (the figure
                    // sits in the dark spawn interior). Runs after the figure
                    // cam.
                    app.add_plugins(player_input::SmokeSpriteCamPlugin);
                    // EM-3.9b: if a fluid (water) chunk meshed anywhere in the
                    // streamed window, override once more to frame it — shows
                    // the animated water shader. No-ops (keeps sprite/figure
                    // framing) when the world seed has no nearby water. Runs
                    // after the sprite cam.
                    app.add_plugins(player_input::SmokeWaterCamPlugin);
                }
            }
        },
        Some(smoke::SmokeMode::Atmosphere(out_dir)) => {
            app.add_plugins(smoke::SmokeAtmospherePlugin { out_dir });
        },
        Some(smoke::SmokeMode::PerfRun(out_csv)) => {
            // BL-82 EM-3.11n: only meaningful against the real streamed
            // terrain — the synthetic demo has no chunk-streaming/meshing
            // load to compare straight vs. diagonal movement against.
            if !listen_server {
                eprintln!("--smoke-perf-run requires --listen-server");
                return AppExit::error();
            }
            app.add_plugins(smoke::SmokePerfRunPlugin { out_csv });
            #[cfg(feature = "listen-server")]
            app.add_plugins(player_input::SmokeAutoMovePlugin);
        },
        None => {},
    }

    app.run()
}

#[cfg(test)]
mod present_mode_tests {
    use super::*;

    /// EM-3.11m regression test: this can't observe real tearing (that needs
    /// a human eyeball on a real display mid-scanout — see the commit
    /// message / hand-off report for that caveat), but it CAN pin the pure
    /// mapping from env var to `PresentMode` so the default never silently
    /// drifts back to a mode without an unconditional no-tearing guarantee.
    /// Runs single-threaded within this one test (env vars are process-wide
    /// state) by doing all assertions sequentially in one `#[test]` fn.
    #[test]
    fn present_mode_from_env_defaults_to_fifo_and_respects_overrides() {
        // SAFETY: this test is the sole reader/writer of
        // `XINDELER_PRESENT_MODE` in this binary's test suite, and all
        // set/assert/remove steps run sequentially within this one test
        // function, so there is no cross-thread data race on the var.
        unsafe {
            std::env::remove_var("XINDELER_PRESENT_MODE");
        }
        assert_eq!(
            present_mode_from_env(),
            PresentMode::Fifo,
            "unset XINDELER_PRESENT_MODE must resolve to Bevy's real, unconditionally-no-tearing \
             default (Fifo), not AutoVsync"
        );

        let cases: &[(&str, PresentMode)] = &[
            ("vsync", PresentMode::Fifo),
            ("fifo", PresentMode::Fifo),
            ("fifo_relaxed", PresentMode::FifoRelaxed),
            ("mailbox", PresentMode::Mailbox),
            ("novsync", PresentMode::AutoNoVsync),
            ("immediate", PresentMode::AutoNoVsync),
            ("auto", PresentMode::AutoVsync),
            ("autovsync", PresentMode::AutoVsync),
            // Typos/unrecognised values fall back to the safe default too.
            ("bogus-typo", PresentMode::Fifo),
        ];
        for (value, expected) in cases {
            // SAFETY: see justification above.
            unsafe {
                std::env::set_var("XINDELER_PRESENT_MODE", value);
            }
            assert_eq!(
                present_mode_from_env(),
                *expected,
                "XINDELER_PRESENT_MODE={value:?} should map to {expected:?}"
            );
        }

        // SAFETY: see justification above; leave the environment clean.
        unsafe {
            std::env::remove_var("XINDELER_PRESENT_MODE");
        }
    }
}
