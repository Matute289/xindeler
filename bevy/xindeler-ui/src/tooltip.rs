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
//!
//! ## BL-82 EM-5.17 T57.16 -- themed background reskin
//! [`TooltipBackground`] is an OPTIONAL companion component (attach it
//! alongside [`Tooltip`] on a hoverable widget) naming which
//! [`crate::images::HudImageKey`] art-pack texture the ONE shared
//! [`HudTooltipLabel`] should show while THAT widget is hovered -- e.g.
//! `inventory_ui.rs`'s item slots attach
//! `TooltipBackground(HudImageKey::InventoryTooltipBg)`, `diary.rs`'s skill
//! nodes attach `TooltipBackground(HudImageKey::SkillTooltipBg)`. A widget
//! that carries `Tooltip` alone (no `TooltipBackground` -- every OTHER
//! hoverable widget in the app, e.g. `combat_hud.rs`'s buff icons,
//! `map_view.rs`'s location markers) keeps the original flat [`HudTheme`]
//! panel-background fill, completely unchanged -- this is an additive,
//! opt-in reskin per T57.16's own scope ("item/skill tooltips"), not a
//! reskin of every tooltip in the app. [`update_tooltip`] swaps the shared
//! label's [`ImageNode`] in/out (inserting it when the CURRENTLY hovered
//! widget carries a background, removing it -- falling back to the flat
//! colour -- when it doesn't), since one label entity is reused across every
//! kind of hover.

use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        query::Changed,
        system::{Commands, Query, Res},
    },
    picking::hover::Hovered,
    prelude::{
        BackgroundColor, Color, Node, PositionType, Text, TextColor, TextFont, UiRect, Val,
        Visibility,
    },
    text::{FontSize, FontSource},
    ui::widget::ImageNode,
};

use crate::{
    images::{HudImageKey, HudImages},
    theme::{HudFonts, HudTheme},
};

/// Attach to any hoverable widget entity (which must ALSO carry
/// `Hovered(false)` — this crate's spawn helpers that support tooltips add it
/// automatically) to give it hover-text. `text` is the resolved display
/// string (already localized by the caller — this primitive does no i18n
/// lookup itself, it just displays what it's given).
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct Tooltip {
    pub text: String,
}

/// Optional companion to [`Tooltip`] -- see the module doc comment's T57.16
/// section. Absent = the shared label keeps its flat [`HudTheme`] colour.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooltipBackground(pub HudImageKey);

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
    hovered: Query<(Entity, &Hovered, &Tooltip, Option<&TooltipBackground>), Changed<Hovered>>,
    mut label: Query<
        (Entity, &mut Text, &mut Visibility, &mut BackgroundColor),
        bevy::ecs::query::With<HudTooltipLabel>,
    >,
    images: Res<HudImages>,
    theme: Res<HudTheme>,
    mut commands: Commands,
) {
    let Ok((label_entity, mut text, mut visibility, mut background_color)) = label.single_mut()
    else {
        return;
    };
    for (_entity, is_hovered, tooltip, background) in &hovered {
        if is_hovered.get() {
            text.0.clone_from(&tooltip.text);
            *visibility = Visibility::Visible;
            match background {
                Some(TooltipBackground(key)) => {
                    // Reskinned (T57.16): the hovered widget names a themed
                    // background -- show it and drop the flat fill so the
                    // image (which already has its own gothic-frame border
                    // baked in) isn't tinted by the panel colour underneath.
                    commands
                        .entity(label_entity)
                        .insert(ImageNode::new(images.get(*key)));
                    background_color.0 = Color::NONE;
                },
                None => {
                    // Every OTHER tooltip (module doc comment): no themed
                    // background named, so fall back to the original flat
                    // fill -- and remove any `ImageNode` a PREVIOUS hover
                    // left behind (the label is one shared entity reused
                    // across every kind of hover, so a stale image from the
                    // last hovered item/skill must not bleed into this one).
                    commands.entity(label_entity).remove::<ImageNode>();
                    background_color.0 = theme.palette.panel_bg;
                },
            }
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
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudImages::dummy());
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
            .spawn((
                HudTooltipLabel,
                Text(String::new()),
                Visibility::Hidden,
                BackgroundColor::default(),
            ))
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

    /// BL-82 EM-5.17 T57.16 — hovering a widget that carries
    /// [`TooltipBackground`] gives the shared label a real `ImageNode` for
    /// that key; hovering a widget WITHOUT one afterwards removes it again
    /// (falling back to the flat colour) — the "additive, opt-in, no stale
    /// image bleeds into the next hover" acceptance bar from the module doc
    /// comment.
    #[test]
    fn a_tooltip_background_shows_and_clears_an_image_node() {
        let mut app = new_app();
        let label = app
            .world_mut()
            .spawn((
                HudTooltipLabel,
                Text(String::new()),
                Visibility::Hidden,
                BackgroundColor::default(),
            ))
            .id();

        let item_slot = app
            .world_mut()
            .spawn((
                Hovered(false),
                Tooltip {
                    text: "Starter Sword".to_owned(),
                },
                TooltipBackground(HudImageKey::InventoryTooltipBg),
            ))
            .id();
        let plain_widget = app
            .world_mut()
            .spawn((Hovered(false), Tooltip {
                text: "Regeneration: +3/s".to_owned(),
            }))
            .id();

        app.world_mut().entity_mut(item_slot).insert(Hovered(true));
        app.world_mut()
            .run_system_once(update_tooltip)
            .expect("update_tooltip runs");
        assert!(
            app.world().get::<ImageNode>(label).is_some(),
            "a widget with TooltipBackground gives the shared label a real ImageNode"
        );

        app.world_mut().entity_mut(item_slot).insert(Hovered(false));
        app.world_mut()
            .entity_mut(plain_widget)
            .insert(Hovered(true));
        app.world_mut()
            .run_system_once(update_tooltip)
            .expect("update_tooltip runs again");
        assert!(
            app.world().get::<ImageNode>(label).is_none(),
            "a plain Tooltip (no TooltipBackground) must clear a PREVIOUS hover's image"
        );
    }
}
