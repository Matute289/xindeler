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
    asset::Handle,
    ecs::{component::Component, entity::Entity, query::Changed, system::Query},
    image::Image,
    picking::hover::Hovered,
    prelude::{
        AlignItems, BackgroundColor, BorderRadius, FontSize, JustifyContent, Node, Text, TextColor,
        TextFont, UiRect, Val,
    },
    text::FontSource,
    ui::{InteractionDisabled, Pressed, widget::ImageNode},
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

/// BL-82 EM-5.17 T57.8 — the three HUD-D4 button-state textures a
/// [`image_button_bundle`] carries, so [`update_image_button_visuals`] can
/// swap the button's `ImageNode` per its live `Hovered`/`Pressed` state.
#[derive(Component, Debug, Clone)]
pub struct HudButtonImages {
    pub normal: Handle<Image>,
    pub hover: Handle<Image>,
    pub pressed: Handle<Image>,
}

/// Spawns an image-backed themed button: same headless `bevy_ui_widgets::
/// Button` behaviour + label-child machinery as [`button_bundle`], but the
/// background is an [`ImageNode`] swapped between `images.normal`/`.hover`/
/// `.pressed` (e.g. `button_normal.png`/`button_hover.png`/
/// `button_pressed.png` via [`crate::images::HudImages`]) instead of
/// [`button_bundle`]'s flat-colour restyle. Purely additive — every existing
/// `button_bundle(..)` call site is completely untouched.
///
/// Note: `bevy_ui`'s own `Node` component `#[require(.., BackgroundColor,
/// ..)]`s a transparent default, so an image-backed button unavoidably still
/// carries a (fully transparent, never touched) `BackgroundColor` — this is
/// harmless (the opaque button texture fully covers it) but means "doesn't
/// have the component" isn't the actual isolation mechanism.
/// [`update_button_visuals`]'s query is explicitly scoped
/// `Without<HudButtonImages>` instead, so it structurally cannot repaint an
/// image-backed button's (irrelevant) transparent background.
#[must_use]
pub fn image_button_bundle(
    theme: &HudTheme,
    fonts: &HudFonts,
    label: &str,
    images: HudButtonImages,
) -> impl bevy::ecs::bundle::Bundle {
    let normal = images.normal.clone();
    (
        HudButton,
        Button,
        Hovered(false),
        Node {
            padding: UiRect::axes(Val::Px(theme.spacing.md), Val::Px(theme.spacing.sm)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..Default::default()
        },
        ImageNode::new(normal),
        images,
        HudButtonLabel(label.to_owned(), fonts.body.clone()),
    )
}

/// Recolors — err, re-TEXTURES — an image-backed button's [`ImageNode`]
/// based on its current hover/pressed state, the image-backed counterpart to
/// [`update_button_visuals`]. Same `Changed<Hovered>` gate (matching that
/// function's own documented limitation: a `Pressed`-only change with no
/// `Hovered` change in the same frame won't re-trigger this system either —
/// an existing, accepted characteristic this mirrors rather than fixes, to
/// keep the two systems' behaviour consistent).
pub(crate) fn update_image_button_visuals(
    mut buttons: Query<
        (&HudButtonImages, &Hovered, Option<&Pressed>, &mut ImageNode),
        (bevy::ecs::query::With<HudButton>, Changed<Hovered>),
    >,
) {
    for (images, hovered, pressed, mut image_node) in &mut buttons {
        image_node.image = if pressed.is_some() {
            images.pressed.clone()
        } else if hovered.get() {
            images.hover.clone()
        } else {
            images.normal.clone()
        };
    }
}

/// Deferred label spec: the actual `Text` child is spawned by
/// [`crate::XindelerUiPlugin`]'s `spawn_button_labels` system (a button
/// widget entity is often built up via `button_bundle(..).observe(..)` chains
/// in caller code before `Commands` flush, so the label child is added on a
/// short delay via a marker rather than requiring callers to also manage
/// `with_children` themselves).
///
/// BL-82 EM-5.16 (T56.44): also the live hot-swap seam for a button's label —
/// `spawn_button_labels` reacts to ANY change to this field, not just its
/// initial `Added` insertion, so `crate::i18n::relocalize_button_labels`
/// mutating `.0` on a locale change propagates onto the real `Text` child
/// in-place (see that system's doc comment). `pub` (not `pub(crate)`) only
/// because `spawn_button_labels`/`relocalize_button_labels` are themselves
/// `pub` (screen-crate tests drive the real reactive chain directly, e.g.
/// `xindeler-client::esc_menu`'s hot-swap regression test) — Rust's
/// private-interfaces lint requires a public function's query types to be at
/// least as visible as the function; this is still an internal
/// implementation detail in spirit (constructible only via
/// [`button_bundle`]/[`image_button_bundle`]).
#[derive(Component, Debug, Clone)]
pub struct HudButtonLabel(pub String, pub bevy::asset::Handle<bevy::text::Font>);

/// Spawns the text child for a [`HudButton`] the first time its
/// [`HudButtonLabel`] appears, and updates that SAME child in place on every
/// later change (the T56.44 hot-swap seam — see [`HudButtonLabel`]'s own doc
/// comment) rather than spawning a second, duplicate child.
pub fn spawn_button_labels(
    mut commands: bevy::ecs::system::Commands,
    theme: bevy::ecs::system::Res<HudTheme>,
    buttons: Query<
        (
            Entity,
            &HudButtonLabel,
            Option<&bevy::ecs::hierarchy::Children>,
        ),
        bevy::ecs::query::Changed<HudButtonLabel>,
    >,
    mut texts: Query<&mut Text>,
) {
    for (entity, label, children) in &buttons {
        if let Some(children) = children {
            let existing = children
                .iter()
                .copied()
                .find(|&child| texts.contains(child));
            if let Some(child) = existing {
                if let Ok(mut text) = texts.get_mut(child)
                    && text.0 != label.0
                {
                    text.0 = label.0.clone();
                }
                continue;
            }
        }
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
/// enter/leave. `Without<HudButtonImages>` (BL-82 EM-5.17) so this never
/// repaints an [`image_button_bundle`]'s (irrelevant, always-transparent —
/// `Node` requires a default `BackgroundColor` on every node) background;
/// [`update_image_button_visuals`] owns that button's visuals instead.
pub(crate) fn update_button_visuals(
    theme: bevy::ecs::system::Res<HudTheme>,
    mut buttons: Query<
        (
            &Hovered,
            Option<&Pressed>,
            Option<&InteractionDisabled>,
            &mut BackgroundColor,
        ),
        (
            bevy::ecs::query::With<HudButton>,
            bevy::ecs::query::Without<HudButtonImages>,
            Changed<Hovered>,
        ),
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

    /// BL-82 EM-5.16 (T56.44): a LATER change to `HudButtonLabel` (e.g. a
    /// locale hot-swap re-resolving the label) updates the EXISTING text
    /// child in place — it must not spawn a second, duplicate child.
    #[test]
    fn relabeling_a_button_updates_its_existing_child_in_place() {
        let mut app = new_app();
        let theme = HudTheme::default();
        let fonts = crate::theme::HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        };

        let button = app
            .world_mut()
            .spawn(button_bundle(&theme, &fonts, "Settings"))
            .id();
        app.update();
        let child_before = *app
            .world()
            .get::<bevy::ecs::hierarchy::Children>(button)
            .expect("label child was spawned")
            .first()
            .expect("exactly one child");

        app.world_mut()
            .get_mut::<HudButtonLabel>(button)
            .expect("HudButtonLabel present")
            .0 = "Opciones".to_owned();
        app.update();

        let children = app
            .world()
            .get::<bevy::ecs::hierarchy::Children>(button)
            .expect("still has children");
        assert_eq!(children.len(), 1, "relabeling must not add a second child");
        assert_eq!(children[0], child_before, "the SAME child entity is reused");
        let text = app
            .world()
            .get::<Text>(child_before)
            .expect("text child still exists");
        assert_eq!(text.0, "Opciones");
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

    /// Three distinguishable `Handle<Image>`s, built without a real
    /// `AssetServer` (mirrors this crate's other headless resource tests) —
    /// `Handle::Uuid` handles compare by their UUID, so three DIFFERENT
    /// UUIDs give three genuinely distinct handles `assert_ne!` can tell
    /// apart, unlike three `Handle::default()`s which would all be equal.
    fn distinct_button_images() -> HudButtonImages {
        use bevy::asset::uuid::Uuid;
        HudButtonImages {
            normal: Handle::Uuid(Uuid::from_u128(1), core::marker::PhantomData),
            hover: Handle::Uuid(Uuid::from_u128(2), core::marker::PhantomData),
            pressed: Handle::Uuid(Uuid::from_u128(3), core::marker::PhantomData),
        }
    }

    fn new_image_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        // Registers BOTH visual systems (not just the image one) — this is
        // what actually proves `update_button_visuals`'s new
        // `Without<HudButtonImages>` filter keeps it from touching an
        // image-backed button, not merely that this test never registered it.
        app.add_systems(
            Update,
            (
                spawn_button_labels,
                update_button_visuals,
                update_image_button_visuals,
            ),
        );
        app
    }

    /// `image_button_bundle` spawns with the NORMAL texture applied and a
    /// label child — the T57.8 additive acceptance bar for the image-backed
    /// button.
    #[test]
    fn image_button_spawns_with_normal_texture_and_label() {
        let mut app = new_image_app();
        let theme = HudTheme::default();
        let fonts = crate::theme::HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        };
        let images = distinct_button_images();
        let normal = images.normal.clone();

        let button = app
            .world_mut()
            .spawn(image_button_bundle(&theme, &fonts, "Continue", images))
            .id();
        app.update();

        let image_node = app.world().get::<ImageNode>(button).unwrap();
        assert_eq!(image_node.image, normal);

        let children = app
            .world()
            .get::<bevy::ecs::hierarchy::Children>(button)
            .expect("label child was spawned");
        let text = children
            .iter()
            .find_map(|c| app.world().get::<Text>(c))
            .expect("a Text child exists");
        assert_eq!(text.0, "Continue");
    }

    /// Hovering an image-backed button swaps its `ImageNode` to the hover
    /// texture; a subsequent `Pressed` insertion (without a `Hovered`
    /// re-trigger) is NOT expected to re-swap in this same frame — matching
    /// `update_button_visuals`'s own documented `Changed<Hovered>`-only
    /// limitation, which this system deliberately mirrors for consistency.
    /// ALSO asserts `update_button_visuals` (registered in this same test
    /// app, see `new_image_app`) never repaints this button's unavoidable
    /// default `BackgroundColor` (`Node`'s own required-components default,
    /// not something either button constructor adds deliberately) — proving
    /// its `Without<HudButtonImages>` filter actually isolates the two
    /// systems, not merely that a plain test never registered the flat one.
    #[test]
    fn hovering_an_image_button_swaps_to_hover_texture_and_leaves_background_untouched() {
        let mut app = new_image_app();
        let theme = HudTheme::default();
        let fonts = crate::theme::HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        };
        let images = distinct_button_images();
        let hover = images.hover.clone();

        let button = app
            .world_mut()
            .spawn(image_button_bundle(&theme, &fonts, "Continue", images))
            .id();
        app.update();
        let background_before = *app.world().get::<BackgroundColor>(button).unwrap();

        app.world_mut().entity_mut(button).insert(Hovered(true));
        app.update();

        let image_node = app.world().get::<ImageNode>(button).unwrap();
        assert_eq!(image_node.image, hover);
        assert_eq!(
            *app.world().get::<BackgroundColor>(button).unwrap(),
            background_before,
            "update_button_visuals must never repaint an image-backed button's background"
        );
    }
}
