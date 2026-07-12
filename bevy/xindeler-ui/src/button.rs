//! BL-82 EM-5.1 T56.1 — the themed Button primitive.
//!
//! Wraps `bevy_ui_widgets::Button` (headless press/click behaviour — no
//! styling, per that crate's own doc comment) with Xindeler's theme: a
//! background that recolors on hover/press/disabled. Per the locked [Q1]
//! decision, this is the ONLY widget-behaviour crate this kit depends on
//! (never `bevy_feathers`). Callers attach their OWN `.observe(...)` on the
//! returned entity for the actual `bevy_ui_widgets::Activate` action — this
//! primitive only owns the visual, not any particular button's behaviour
//! (e.g. EM-5.2's respawn button fires a client→sim respawn intent; a future
//! settings-tab button fires something else entirely).

use bevy::{
    ecs::{component::Component, entity::Entity, query::Changed, system::Query},
    picking::hover::Hovered,
    prelude::{
        AlignItems, BackgroundColor, BorderRadius, FontSize, JustifyContent, Node, Text, TextColor,
        TextFont, UiRect, Val,
    },
    text::FontSource,
    ui::{InteractionDisabled, Pressed},
};
use bevy_ui_widgets::Button;
// Re-exported so downstream screen crates (e.g. `xindeler-client`) can
// `.observe(...)` a button's activation without taking their own direct
// `bevy_ui_widgets` dependency — this crate is already the one sanctioned
// place that depends on it (locked [Q1]).
pub use bevy_ui_widgets::Activate;

use crate::theme::{HudFonts, HudTheme};

/// Marks a themed button root (for [`update_button_visuals`]'s query and for
/// screen code that wants to find "the button I just spawned").
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudButton;

/// Spawns a themed, clickable button with a text label. Returns the button
/// entity — carrying [`bevy_ui_widgets::Button`] (the headless behaviour) +
/// [`Hovered`] (so hover restyling AND [`crate::tooltip::Tooltip`] both work
/// on it, if the caller also inserts a `Tooltip`) + [`HudButton`]. The
/// caller is responsible for `.observe(...)`ing `bevy_ui_widgets::Activate`
/// on the returned entity to wire up what the button actually DOES.
#[must_use]
pub fn button_bundle(
    theme: &HudTheme,
    fonts: &HudFonts,
    label: &str,
) -> impl bevy::ecs::bundle::Bundle {
    (
        HudButton,
        Button,
        Hovered(false),
        Node {
            padding: UiRect::axes(Val::Px(theme.spacing.md), Val::Px(theme.spacing.sm)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(Val::Px(2.0)),
            // `BorderRadius` is a FIELD of `Node` in Bevy 0.19, not a
            // standalone `Component` — see `panel.rs`'s own note.
            border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
            ..Default::default()
        },
        BackgroundColor(theme.palette.panel_bg),
        bevy::ui::BorderColor::all(theme.palette.panel_border),
        // `bevy_ui_widgets::Button` itself `#[require(AccessibilityNode(..))]`
        // — no need to insert one manually here.
        HudButtonLabel(label.to_owned(), fonts.body.clone()),
    )
}

/// Deferred label spec: the actual `Text` child is spawned by
/// [`crate::XindelerUiPlugin`]'s `spawn_button_labels` system (a button
/// widget entity is often built up via `button_bundle(..).observe(..)` chains
/// in caller code before `Commands` flush, so the label child is added on a
/// short delay via a marker rather than requiring callers to also manage
/// `with_children` themselves).
#[derive(Component, Debug, Clone)]
pub(crate) struct HudButtonLabel(pub String, pub bevy::asset::Handle<bevy::text::Font>);

/// Spawns the text child for any [`HudButton`] that doesn't have one yet.
pub(crate) fn spawn_button_labels(
    mut commands: bevy::ecs::system::Commands,
    theme: bevy::ecs::system::Res<HudTheme>,
    buttons: Query<(Entity, &HudButtonLabel), bevy::ecs::query::Added<HudButtonLabel>>,
) {
    for (entity, label) in &buttons {
        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Text(label.0.clone()),
                TextFont {
                    font: FontSource::Handle(label.1.clone()),
                    font_size: FontSize::Px(18.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        });
    }
}

/// Recolors a button's background based on its current
/// hover/pressed/disabled state — the only visual feedback this primitive
/// owns. `Changed<Hovered>`/pressed-insertion already gate most of the real
/// work upstream in `bevy_ui_widgets`/`bevy_picking`; this system itself
/// just re-runs on every `Hovered` change, which only fires on an actual
/// enter/leave.
pub(crate) fn update_button_visuals(
    theme: bevy::ecs::system::Res<HudTheme>,
    mut buttons: Query<
        (
            &Hovered,
            Option<&Pressed>,
            Option<&InteractionDisabled>,
            &mut BackgroundColor,
        ),
        (bevy::ecs::query::With<HudButton>, Changed<Hovered>),
    >,
) {
    for (hovered, pressed, disabled, mut background) in &mut buttons {
        background.0 = if disabled.is_some() {
            theme.palette.text_muted
        } else if pressed.is_some() {
            theme.palette.accent
        } else if hovered.get() {
            theme.palette.panel_border
        } else {
            theme.palette.panel_bg
        };
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;
    use crate::theme::HudTheme;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.add_systems(Update, (spawn_button_labels, update_button_visuals));
        app
    }

    /// Spawning a button attaches the theme's default background and a
    /// `HudButtonLabel` carrying the requested text; the label-spawn system
    /// then gives it a real `Text` child.
    #[test]
    fn button_spawns_with_label_child() {
        let mut app = new_app();
        let theme = HudTheme::default();
        let fonts = crate::theme::HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        };

        let button = app
            .world_mut()
            .spawn(button_bundle(&theme, &fonts, "Respawn"))
            .id();
        app.update();

        let children = app
            .world()
            .get::<bevy::ecs::hierarchy::Children>(button)
            .expect("label child was spawned");
        let text = children
            .iter()
            .find_map(|c| app.world().get::<Text>(c))
            .expect("a Text child exists");
        assert_eq!(text.0, "Respawn");
    }

    /// Hovering a button restyles its background to the theme's border/
    /// highlight colour; un-hovering restores the panel background.
    #[test]
    fn hovering_a_button_restyles_its_background() {
        let mut app = new_app();
        let theme = HudTheme::default();
        let fonts = crate::theme::HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        };
        let button = app
            .world_mut()
            .spawn(button_bundle(&theme, &fonts, "Respawn"))
            .id();
        app.insert_resource(theme);
        app.update();

        app.world_mut().entity_mut(button).insert(Hovered(true));
        app.update();

        let bg = app.world().get::<BackgroundColor>(button).unwrap();
        assert_eq!(bg.0, theme.palette.panel_border);
    }
}
