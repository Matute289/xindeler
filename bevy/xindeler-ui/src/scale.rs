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
}
