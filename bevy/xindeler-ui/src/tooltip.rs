//! BL-82 EM-5.1 T56.1 — the Tooltip primitive (hover, screen-anchored v1).
//!
//! Every buff/debuff icon, item, and ability slot in later screens needs a
//! hover tooltip (legacy `item_tooltip.rs`/`buffs.rs`'s title+description
//! text). This v1 primitive covers the SCREEN-anchored case (a floating
//! label that appears near the hovered widget) using the real
//! `bevy_picking::hover::Hovered` component — the documented, change-detection
//! -friendly way to know an entity's hover state in Bevy 0.19 (no `Hovered`
//! marker existed before 0.19; earlier Bevy used a since-removed
//! `Interaction` enum component). World-anchored tooltips (overhead
//! nameplates hovering over an in-world entity) are a follow-up once EM-5.2's
//! overhead widgets need one — the shape (a text panel driven by the same
//! [`Tooltip`] content component) is identical, only the anchor math differs.

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        query::Changed,
        system::{Commands, Query, Res},
    },
    picking::hover::Hovered,
    prelude::{
        BackgroundColor, Node, PositionType, Text, TextColor, TextFont, UiRect, Val, Visibility,
    },
    text::{FontSize, FontSource},
};

use crate::theme::{HudFonts, HudTheme};

/// Attach to any hoverable widget entity (which must ALSO carry
/// `Hovered(false)` — this crate's spawn helpers that support tooltips add it
/// automatically) to give it hover-text. `text` is the resolved display
/// string (already localized by the caller — this primitive does no i18n
/// lookup itself, it just displays what it's given).
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct Tooltip {
    pub text: String,
}

/// Marks the (single, shared) floating tooltip label this crate maintains.
/// [`update_tooltip`] repositions/retexts/shows-or-hides this ONE entity
/// rather than spawning a new label per hovered widget.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudTooltipLabel;

/// Spawns the ONE shared tooltip label this whole crate's [`update_tooltip`]
/// system drives, hidden by default. Every screen's hoverable widgets
/// (a buff icon, an item slot, an ability) just carry [`Hovered`]+[`Tooltip`]
/// — they never spawn their own label. v1 is screen-anchored at a fixed
/// position (near the top of the screen, below the always-on combat HUD
/// bars); cursor-following / world-anchored placement is a documented
/// follow-up (module doc comment), not this task's job.
pub(crate) fn spawn_shared_tooltip_label(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
) {
    commands.spawn((
        HudTooltipLabel,
        Text(String::new()),
        TextFont {
            font: FontSource::Handle(fonts.body.clone()),
            font_size: FontSize::Px(16.0),
            ..Default::default()
        },
        TextColor(theme.palette.text),
        BackgroundColor(theme.palette.panel_bg),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(120.0),
            left: Val::Px(16.0),
            padding: UiRect::all(Val::Px(theme.spacing.xs)),
            ..Default::default()
        },
        Visibility::Hidden,
    ));
}

/// Shows the shared [`HudTooltipLabel`] with the hovered widget's [`Tooltip`]
/// text whenever a [`Hovered`] flag flips, and hides it again when nothing is
/// hovered — `Changed<Hovered>`-gated, so idle frames (nothing (un)hovered)
/// cost nothing.
pub(crate) fn update_tooltip(
    hovered: Query<(Entity, &Hovered, &Tooltip), Changed<Hovered>>,
    mut label: Query<(&mut Text, &mut Visibility), bevy::ecs::query::With<HudTooltipLabel>>,
) {
    let Ok((mut text, mut visibility)) = label.single_mut() else {
        return;
    };
    for (_entity, is_hovered, tooltip) in &hovered {
        if is_hovered.get() {
            text.0.clone_from(&tooltip.text);
            *visibility = Visibility::Visible;
        } else {
            *visibility = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::{ecs::system::RunSystemOnce, prelude::*};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app
    }

    /// Hovering a widget shows the shared tooltip label with its text;
    /// un-hovering hides it again — the T56.1 "Tooltip (hover ...)"
    /// acceptance bar.
    #[test]
    fn hovering_a_widget_shows_its_tooltip_text() {
        let mut app = new_app();
        let label = app
            .world_mut()
            .spawn((HudTooltipLabel, Text(String::new()), Visibility::Hidden))
            .id();

        let widget = app
            .world_mut()
            .spawn((Hovered(false), Tooltip {
                text: "Regeneration: +3/s".to_owned(),
            }))
            .id();

        // Flip to hovered — `Hovered` is `#[component(immutable)]`, so use
        // `insert` (not a `&mut` query) to trigger change detection, matching
        // how `bevy_picking::hover::update_is_hovered` itself would update it.
        app.world_mut().entity_mut(widget).insert(Hovered(true));
        app.world_mut()
            .run_system_once(update_tooltip)
            .expect("update_tooltip runs");

        let (text, visibility) = (
            app.world().get::<Text>(label).unwrap().clone(),
            *app.world().get::<Visibility>(label).unwrap(),
        );
        assert_eq!(text.0, "Regeneration: +3/s");
        assert_eq!(visibility, Visibility::Visible);

        app.world_mut().entity_mut(widget).insert(Hovered(false));
        app.world_mut()
            .run_system_once(update_tooltip)
            .expect("update_tooltip runs again");
        let visibility = *app.world().get::<Visibility>(label).unwrap();
        assert_eq!(visibility, Visibility::Hidden);
    }
}
