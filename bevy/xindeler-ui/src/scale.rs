//! BL-82 EM-5.1 T56.6 — the UI-scale foundation (legacy `ui/scale.rs`
//! analog).
//!
//! Bevy already ships a global [`bevy::ui::UiScale`] resource that the whole
//! `bevy_ui` layout pass multiplies every logical pixel by — there is no need
//! to reinvent scaling math. This module is the thin, tunable seam over it:
//! [`hud_scale_from`] turns a plain `f32` multiplier (the persisted value —
//! `xindeler_app::settings::XindelerSettings::ui_scale`, EM-5.12's future
//! settings tab) into the concrete [`bevy::ui::UiScale`] value the client
//! inserts as a resource. Kept in `xindeler-ui` (not `xindeler-app`, which
//! this crate does not depend on) so the widget kit owns the whole
//! presentation seam; the persisted VALUE still lives in `xindeler-app`'s
//! existing settings resource (the established persistence seam), threaded
//! through by whichever shell wires the two together (a one-line glue system
//! in `xindeler-client`, once EM-5.12 adds the settings tab that changes it
//! at runtime).

use bevy::ui::UiScale;

/// Clamp bounds for a sane UI scale — guards against a corrupt/malicious
/// `settings.ron` value collapsing the whole HUD to zero size or blowing it
/// up past the window bounds.
pub const MIN_UI_SCALE: f32 = 0.5;
pub const MAX_UI_SCALE: f32 = 2.0;

/// Builds the [`UiScale`] resource value for a given multiplier, clamped to
/// `[`[`MIN_UI_SCALE`]`, `[`MAX_UI_SCALE`]`]` — degrade clean rather than
/// panic on an out-of-range persisted value.
#[must_use]
pub fn hud_scale_from(multiplier: f32) -> UiScale {
    UiScale(multiplier.clamp(MIN_UI_SCALE, MAX_UI_SCALE))
}

/// BL-82 HUD-responsive-scaling pass (Matías: "at a large/fullscreen window
/// the HUD elements — orbs, action bar, etc. — stay the exact same small
/// fixed pixel size instead of getting visually bigger to match the larger
/// window") — the window-height-derived component of the multiplier
/// [`window_derived_hud_scale`] feeds into [`hud_scale_from`].
///
/// The reference height matches `xindeler-client`'s own default
/// `WindowResolution::new(1280, 720)` (`main.rs`) — the size every
/// `hud_layout`/`combat_hud`/`chat` fixed `Val::Px` constant was actually
/// eyeballed against via `--smoke-screenshot`. Anchoring the multiplier's
/// identity point there (not, say, 1080p) means a window AT this size renders
/// pixel-identical to today, before this pass — no regression for the
/// common case, only a genuine change for windows that differ from it.
pub const REFERENCE_WINDOW_HEIGHT_PX: f32 = 720.0;

/// [`window_derived_hud_scale`]'s pure per-window-height multiplier:
/// identity (`1.0`) at/below [`REFERENCE_WINDOW_HEIGHT_PX`], growing
/// linearly above it.
///
/// Deliberately **one-directional** — it never drops below `1.0` for a
/// window SHORTER than the reference. Two reasons: (1) the small-window
/// overlap bug this same pass fixes (`chat.rs`'s panel vs the bottom-centre
/// health orb) is reasoned about, and unit-tested, against the PLAIN
/// `hud_layout` constants at their literal (unscaled) values — layering an
/// automatic shrink-below-1.0 heuristic on top would mean that reasoning no
/// longer matches what actually renders, reopening the exact bug this pass
/// closes. (2) a user who explicitly wants a smaller HUD already has the
/// persisted `XindelerSettings::ui_scale` multiplier (still combined in by
/// [`window_derived_hud_scale`]) — that path can still go below `1.0`
/// (down to [`MIN_UI_SCALE`]), just not as an UNREQUESTED side effect of
/// resizing the window smaller.
#[must_use]
pub fn window_height_multiplier(window_height_px: f32) -> f32 {
    if window_height_px <= 0.0 {
        return 1.0; // guard against a degenerate/zero window report
    }
    (window_height_px / REFERENCE_WINDOW_HEIGHT_PX).max(1.0)
}

/// The actual [`UiScale`] value a client shell should hold at the given
/// window height + persisted user multiplier — combines
/// [`window_height_multiplier`] (the new, window-size-derived half of this
/// pass) with the pre-existing user-facing `ui_scale` setting
/// [`hud_scale_from`] already applied on its own, then clamps the product
/// the same way. Whichever shell wires the two seams together (per this
/// module's own original doc comment) should call this, not `hud_scale_from`
/// directly, once it also wants window-size-derived growth.
#[must_use]
pub fn window_derived_hud_scale(window_height_px: f32, settings_multiplier: f32) -> UiScale {
    hud_scale_from(window_height_multiplier(window_height_px) * settings_multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity multiplier (`1.0`, `XindelerSettings::default().ui_scale`)
    /// round-trips to `UiScale(1.0)` — the "T56.6 verify: scale factor
    /// visibly resizes the gallery" bar starts from a known-good identity.
    #[test]
    fn identity_multiplier_round_trips() {
        assert_eq!(hud_scale_from(1.0).0, 1.0);
    }

    /// Out-of-range values clamp instead of producing a degenerate (zero or
    /// huge) HUD.
    #[test]
    fn out_of_range_values_clamp() {
        assert_eq!(hud_scale_from(0.0).0, MIN_UI_SCALE);
        assert_eq!(hud_scale_from(10.0).0, MAX_UI_SCALE);
    }

    /// At/below the reference height, the window-derived multiplier stays
    /// exactly `1.0` — a window at (or smaller than) the size the HUD's own
    /// pixel constants were tuned against must render identically to before
    /// this pass, never shrink further on its own.
    #[test]
    fn window_multiplier_is_identity_at_or_below_reference_height() {
        assert_eq!(window_height_multiplier(REFERENCE_WINDOW_HEIGHT_PX), 1.0);
        assert_eq!(window_height_multiplier(480.0), 1.0);
        assert_eq!(window_height_multiplier(1.0), 1.0);
    }

    /// Above the reference height, the multiplier grows proportionally —
    /// the actual fix for "everything stays tiny on a large/fullscreen
    /// window."
    #[test]
    fn window_multiplier_grows_above_reference_height() {
        assert_eq!(window_height_multiplier(1440.0), 2.0);
        assert!((window_height_multiplier(1080.0) - 1.5).abs() < 1e-6);
    }

    /// A degenerate (zero/negative) reported window height must not divide
    /// by zero or produce a negative/NaN scale — falls back to identity.
    #[test]
    fn window_multiplier_guards_degenerate_window_height() {
        assert_eq!(window_height_multiplier(0.0), 1.0);
        assert_eq!(window_height_multiplier(-100.0), 1.0);
    }

    /// [`window_derived_hud_scale`] combines both multipliers and still
    /// clamps to [`MAX_UI_SCALE`] — an extremely tall window times a
    /// generous user multiplier must not blow the HUD up unboundedly.
    #[test]
    fn window_derived_scale_combines_and_clamps() {
        assert_eq!(
            window_derived_hud_scale(REFERENCE_WINDOW_HEIGHT_PX, 1.0).0,
            1.0
        );
        assert_eq!(window_derived_hud_scale(1440.0, 1.0).0, 2.0);
        assert_eq!(
            window_derived_hud_scale(REFERENCE_WINDOW_HEIGHT_PX, 0.5).0,
            0.5
        );
        assert_eq!(
            window_derived_hud_scale(4000.0, 2.0).0,
            MAX_UI_SCALE,
            "an extreme window height + a generous user multiplier must still clamp"
        );
    }
}
