//! BL-82 EM-5.4 — the ScrollView primitive EM-5.1's own doc comment deferred
//! ("List/Grid/ScrollView … lands with whichever screen needs it first" —
//! chat is that screen). A thin themed bundle helper over
//! `bevy_ui_widgets::ScrollArea` (the headless mouse-wheel/trackpad
//! behaviour, already registered by [`crate::XindelerUiPlugin`] via
//! `UiWidgetsPlugins`) — no new widget-behaviour dependency, per the locked
//! [Q1] decision. [`ScrollArea`] itself is re-exported here (mirroring
//! `button.rs`'s own `Activate` re-export) so callers never need their own
//! direct `bevy_ui_widgets` dependency — this crate is already the one
//! sanctioned place that depends on it.

use bevy::ui::{BackgroundColor, FlexDirection, Node, Overflow, OverflowAxis, Val};
pub use bevy_ui_widgets::ScrollArea;

use crate::theme::HudTheme;

/// Returns the bundle for a themed, fixed-size, vertically-scrolling
/// container. Mouse-wheel/trackpad scrolling works automatically once
/// spawned — [`bevy_ui_widgets::ScrollAreaPlugin`]'s own observer (part of
/// `UiWidgetsPlugins`) handles it. Horizontal overflow is clipped, not
/// scrolled (every v1 consumer — the chat scrollback — only needs vertical
/// scroll; a horizontal variant is a trivial follow-up if a future screen
/// needs one).
#[must_use]
pub fn scroll_view_bundle(
    theme: &HudTheme,
    width: f32,
    height: f32,
) -> impl bevy::ecs::bundle::Bundle {
    (
        ScrollArea,
        Node {
            width: Val::Px(width),
            height: Val::Px(height),
            flex_direction: FlexDirection::Column,
            overflow: Overflow {
                x: OverflowAxis::Clip,
                y: OverflowAxis::Scroll,
            },
            ..Default::default()
        },
        BackgroundColor(theme.palette.panel_bg),
    )
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;

    /// `scroll_view_bundle` spawns a real `ScrollArea`-tagged entity sized
    /// exactly as requested, with vertical (not horizontal) scroll enabled.
    #[test]
    fn scroll_view_bundle_spawns_with_requested_size_and_vertical_scroll() {
        let mut world = World::new();
        let theme = HudTheme::default();
        let entity = world.spawn(scroll_view_bundle(&theme, 400.0, 150.0)).id();

        assert!(world.get::<ScrollArea>(entity).is_some());
        let node = world.get::<Node>(entity).expect("carries a Node");
        assert_eq!(node.width, Val::Px(400.0));
        assert_eq!(node.height, Val::Px(150.0));
        assert_eq!(node.overflow.y, OverflowAxis::Scroll);
        assert_eq!(node.overflow.x, OverflowAxis::Clip);
    }
}
