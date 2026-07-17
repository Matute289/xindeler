//! BL-82 HUD-responsive-scaling pass — wires the T56.6 `UiScale` foundation
//! (`xindeler_ui::scale`) to the ACTUAL window size, closing the gap that
//! module's own original doc comment flagged as still open ("threaded
//! through by whichever shell wires the two together, once EM-5.12 adds the
//! settings tab"). Before this, `xindeler_ui::scale::hud_scale_from` existed
//! and was fully tested, but nothing in `xindeler-client` ever read it or
//! inserted a non-default [`UiScale`] resource — Matías's live-testing
//! report ("at a large/fullscreen window, the HUD elements stay the exact
//! same small fixed pixel size instead of getting visually bigger") is
//! exactly what an unwired `UiScale` (always its `Default` of `1.0`,
//! regardless of window size) produces.
//!
//! Runs UNCONDITIONALLY (not gated behind `listen-server`/`net-client`, the
//! feature flags most other HUD modules in this crate carry) since every
//! client mode — including the synthetic voxel demo — renders the same
//! `bevy_ui` tree via the SAME global [`UiScale`] resource `bevy_ui`'s
//! `UiPlugin` always registers.

use bevy::{prelude::*, window::PrimaryWindow};
use xindeler_app::XindelerSettings;
use xindeler_ui::scale::window_derived_hud_scale;

pub struct HudScalePlugin;

impl Plugin for HudScalePlugin {
    fn build(&self, app: &mut App) { app.add_systems(Update, sync_hud_scale_to_window); }
}

/// Recomputes [`UiScale`] from the primary window's CURRENT height plus the
/// persisted [`XindelerSettings::ui_scale`] user multiplier, every frame the
/// combined value actually changes (a window resize, or a live settings
/// change once EM-5.12 adds the settings tab this module's own doc comment
/// names) — a no-op write-guard on every other frame, so this never marks
/// [`UiScale`] changed (and re-triggers `bevy_ui` layout) without a real
/// reason to.
fn sync_hud_scale_to_window(
    windows: Query<&Window, With<PrimaryWindow>>,
    settings: Res<XindelerSettings>,
    mut ui_scale: ResMut<UiScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let desired = window_derived_hud_scale(window.height(), settings.ui_scale);
    if (ui_scale.0 - desired.0).abs() > f32::EPSILON {
        ui_scale.0 = desired.0;
    }
}

#[cfg(test)]
mod tests {
    use bevy::window::WindowResolution;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<UiScale>();
        app.insert_resource(XindelerSettings::default());
        app
    }

    /// A window at (or below) the reference height leaves [`UiScale`] at
    /// its default identity — no regression for the common/default case.
    #[test]
    fn reference_height_window_keeps_identity_scale() {
        let mut app = new_app();
        app.world_mut().spawn((PrimaryWindow, Window {
            resolution: WindowResolution::new(1280, 720),
            ..Default::default()
        }));

        app.add_systems(Update, sync_hud_scale_to_window);
        app.update();

        assert_eq!(app.world().resource::<UiScale>().0, 1.0);
    }

    /// **The core acceptance test for "HUD stays tiny on a large/fullscreen
    /// window"**: a genuinely large window (well past the 720px reference
    /// height) must grow [`UiScale`] above `1.0` — the actual fix.
    #[test]
    fn large_window_grows_the_hud_scale() {
        let mut app = new_app();
        app.world_mut().spawn((PrimaryWindow, Window {
            resolution: WindowResolution::new(2560, 1440),
            ..Default::default()
        }));

        app.add_systems(Update, sync_hud_scale_to_window);
        app.update();

        let scale = app.world().resource::<UiScale>().0;
        assert!(
            scale > 1.0,
            "a 1440px-tall window must grow the HUD scale above identity, got {scale}"
        );
    }

    /// A LIVE resize (not just the size at boot) updates [`UiScale`] again —
    /// this system runs every `Update`, not just once at `Startup`.
    #[test]
    fn a_live_resize_updates_the_scale_again() {
        let mut app = new_app();
        let window = app
            .world_mut()
            .spawn((PrimaryWindow, Window {
                resolution: WindowResolution::new(1280, 720),
                ..Default::default()
            }))
            .id();

        app.add_systems(Update, sync_hud_scale_to_window);
        app.update();
        assert_eq!(app.world().resource::<UiScale>().0, 1.0);

        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution = WindowResolution::new(1920, 1080);
        app.update();
        let scale = app.world().resource::<UiScale>().0;
        assert!(
            (scale - 1.5).abs() < 1e-5,
            "resizing to a 1080px-tall window must grow the scale to ~1.5, got {scale}"
        );
    }

    /// The persisted user `ui_scale` setting still combines in — a player
    /// who explicitly wants a SMALLER HUD (below `1.0`) can still get one,
    /// even though the window-derived component alone never shrinks below
    /// identity.
    #[test]
    fn the_persisted_user_multiplier_still_applies() {
        let mut app = new_app();
        app.world_mut().spawn((PrimaryWindow, Window {
            resolution: WindowResolution::new(1280, 720),
            ..Default::default()
        }));
        app.world_mut().resource_mut::<XindelerSettings>().ui_scale = 0.75;

        app.add_systems(Update, sync_hud_scale_to_window);
        app.update();

        assert_eq!(app.world().resource::<UiScale>().0, 0.75);
    }
}
