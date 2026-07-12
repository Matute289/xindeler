//! BL-82 EM-5.1 T56.1 — the Panel primitive.
//!
//! Legacy's 9-slice `image_frame` widget (`voxygen/src/ui/widgets/
//! image_frame.rs`) is the SHAPE this primitive re-derives (a themed
//! bordered/rounded frame every window sits inside), not the code — v1 uses
//! a flat themed `bevy_ui` background + border + radius (no texture-slicing
//! dependency), which is a real, functional, on-theme panel primitive today;
//! swapping in a genuine 9-slice `.png` frame texture later is a
//! visual-polish follow-up (spec §2's "engineering note": pick the simpler
//! path for v1 if the fancier one proves costly) that only touches this one
//! function, not every screen that calls it.

use bevy::{
    ecs::{bundle::Bundle, component::Component},
    ui::{BackgroundColor, BorderColor, BorderRadius, Node, PositionType, UiRect, Val},
};

use crate::theme::HudTheme;

/// Marker for a themed panel root node (useful for queries/tests, and for a
/// future screen wanting to find "the panel I'm inside").
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudPanel;

/// Returns the bundle for a themed panel root: background fill, border,
/// rounded corners, and padding, all resolved from `theme` (never a literal
/// colour) — a single tunable source instead of scattered per-screen magic
/// numbers.
#[must_use]
pub fn panel_bundle(theme: &HudTheme) -> impl Bundle {
    (
        HudPanel,
        Node {
            padding: UiRect::all(theme.spacing.md_px()),
            border: UiRect::all(Val::Px(2.0)),
            // `BorderRadius` is a FIELD of `Node` in Bevy 0.19 (it only
            // derives `Copy`/`Clone`/`Reflect`, not `Component` — it cannot
            // be spawned as a separate bundle element).
            border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
            ..Default::default()
        },
        BackgroundColor(theme.palette.panel_bg),
        BorderColor::all(theme.palette.panel_border),
    )
}

/// A panel positioned absolutely at a screen corner/edge (the shape every
/// always-on HUD readout — health globe, hotbar, buff strip — needs, as
/// opposed to a centred modal window). `top`/`left`/`right`/`bottom` are
/// `None` for "unset" (matches `bevy_ui`'s own `Val::Auto` convention via
/// `Node`'s defaults).
#[must_use]
pub fn anchored_panel_bundle(
    theme: &HudTheme,
    top: Option<f32>,
    left: Option<f32>,
    right: Option<f32>,
    bottom: Option<f32>,
) -> impl Bundle {
    let mut node = Node {
        position_type: PositionType::Absolute,
        padding: UiRect::all(theme.spacing.md_px()),
        border: UiRect::all(Val::Px(2.0)),
        border_radius: BorderRadius::all(Val::Px(theme.radius.md)),
        ..Default::default()
    };
    if let Some(top) = top {
        node.top = Val::Px(top);
    }
    if let Some(left) = left {
        node.left = Val::Px(left);
    }
    if let Some(right) = right {
        node.right = Val::Px(right);
    }
    if let Some(bottom) = bottom {
        node.bottom = Val::Px(bottom);
    }
    (
        HudPanel,
        node,
        BackgroundColor(theme.palette.panel_bg),
        BorderColor::all(theme.palette.panel_border),
    )
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;

    /// `panel_bundle` spawns a real `HudPanel`-tagged entity with the
    /// theme's background colour applied verbatim — the T56.1 "widget
    /// gallery" acceptance bar, exercised headlessly.
    #[test]
    fn panel_bundle_spawns_with_theme_background() {
        let mut world = World::new();
        let theme = HudTheme::default();
        let entity = world.spawn(panel_bundle(&theme)).id();

        let bg = world
            .get::<BackgroundColor>(entity)
            .expect("panel carries a BackgroundColor");
        assert_eq!(bg.0, theme.palette.panel_bg);
        assert!(world.get::<HudPanel>(entity).is_some());
    }
}
