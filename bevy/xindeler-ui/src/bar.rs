//! BL-82 EM-5.1 T56.1 — the Progress bar / globe primitive.
//!
//! The shape every always-on combat readout needs (health/energy/poise/XP —
//! legacy's `skillbar.rs` bars): a background track + a fill child whose
//! width tracks a `current/max` value. [`BarValue`] is the single piece of
//! state a caller (EM-5.2's mirror-reading systems) updates; [`update_bars`]
//! is the one system that turns it into a fill-width, `Changed<BarValue>`-
//! gated so it costs nothing on ticks where nothing changed.

use bevy::{
    asset::Handle,
    ecs::{
        component::Component,
        hierarchy::Children,
        query::{Changed, With},
        system::{Commands, Query},
    },
    image::Image,
    picking::Pickable,
    ui::{BackgroundColor, Node, PositionType, Val, widget::ImageNode},
};

use crate::theme::HudTheme;

/// The value a bar displays: `current` / `max`. Any caller (an EM-5.2 HUD
/// system reading `NetHealth`/`NetEnergy`/etc.) writes this; [`update_bars`]
/// is the only thing that reads it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct BarValue {
    pub current: f32,
    pub max: f32,
}

impl BarValue {
    #[must_use]
    pub fn new(current: f32, max: f32) -> Self { Self { current, max } }

    /// Clamped `[0, 1]` fraction — `max <= 0` degrades to `0.0` rather than
    /// NaN/inf (spec §3.2 "degrade clean", applied to widget math too).
    #[must_use]
    pub fn fraction(self) -> f32 {
        if self.max <= 0.0 {
            0.0
        } else {
            (self.current / self.max).clamp(0.0, 1.0)
        }
    }
}

/// Marks a bar's fill child (the node whose width [`update_bars`] resizes).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudBarFill;

/// Marks a bar's container (the node carrying [`BarValue`] + [`Children`]
/// that includes exactly one [`HudBarFill`]).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudBar;

/// Spawns a themed bar: a background-track container (`width`×`height` px,
/// `bg_color`) with one [`HudBarFill`] child (`fill_color`) sized to `value`'s
/// current fraction. Returns the container [`bevy::ecs::entity::Entity`] —
/// callers update the display later purely by mutating [`BarValue`] on it
/// (via `Commands::entity(id).insert(BarValue::new(..))` or a `Query<&mut
/// BarValue>`), never by touching the fill child directly.
pub fn spawn_bar(
    commands: &mut Commands,
    theme: &HudTheme,
    fill_color: bevy::color::Color,
    bg_color: bevy::color::Color,
    width_px: f32,
    height_px: f32,
    value: BarValue,
) -> bevy::ecs::entity::Entity {
    commands
        .spawn((
            HudBar,
            value,
            Node {
                width: Val::Px(width_px),
                height: Val::Px(height_px),
                overflow: bevy::ui::Overflow::clip(),
                // `BorderRadius` is a FIELD of `Node` in Bevy 0.19, not a
                // standalone `Component` — see `panel.rs`'s own note.
                border_radius: bevy::ui::BorderRadius::all(Val::Px(theme.radius.sm)),
                ..Default::default()
            },
            BackgroundColor(bg_color),
        ))
        .with_children(|parent| {
            parent.spawn((
                HudBarFill,
                Node {
                    width: Val::Percent(value.fraction() * 100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                BackgroundColor(fill_color),
            ));
        })
        .id()
}

/// Resizes every bar's fill child to its container's current
/// [`BarValue`] fraction — the only system that ever touches a fill child's
/// `Node`. `Changed<BarValue>`-gated: costs nothing on frames where nothing
/// updated the value (most frames, for a full-health/energy player standing
/// still). `With<HudBar>`-scoped (added alongside [`HudOrbBar`], BL-82
/// EM-5.17) so this doesn't also iterate every orb bar's container — those
/// resize via [`update_orb_bars`] instead.
pub(crate) fn update_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudBar>)>,
    mut fills: Query<&mut Node, With<HudBarFill>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(mut node) = fills.get_mut(child) {
                node.width = Val::Percent(value.fraction() * 100.0);
            }
        }
    }
}

/// BL-82 EM-5.17 T57.8 — marks an orb bar's fill child (the node
/// [`update_orb_bars`] resizes, bottom-anchored). Distinct from
/// [`HudBarFill`] since the two resize DIFFERENT axes ([`HudBarFill`]:
/// width; this one: height) — sharing one marker across both would let a
/// query accidentally treat a horizontal fill as a vertical one.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudOrbBarFill;

/// Marks an orb bar's container — the vertical, bottom-anchored counterpart
/// to [`HudBar`]. See [`spawn_orb_bar`]'s doc comment for why this is a
/// PARALLEL primitive rather than an `Orientation` parameter grafted onto
/// [`spawn_bar`]/[`HudBar`] itself.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudOrbBar;

/// Spawns a themed VERTICAL, bottom-anchored bar — the liquid-fill-orb shape
/// a later phase needs (spec §3.1): a circular frame with a liquid image
/// inside that rises/falls with a `current/max` fraction. `fill_image` is
/// the liquid texture (e.g. `health_liquid.png`); `frame_image`, if given, is
/// spawned as a full-size sibling `ImageNode` ON TOP of the fill, with
/// [`Pickable::IGNORE`] so it never blocks interaction with whatever's
/// underneath — the frame PNGs in the HUD-D4 pack have an alpha-transparent
/// centre, so the fill shows through the frame's circular cutout while the
/// frame's own opaque ring/carving still renders over the fill's square
/// corners (no separate circular-clip shader needed for this v1 — see
/// `crate::orb_material`'s module doc comment for the full `UiMaterial`
/// spike + why v1 deliberately stays with this CPU-clip mechanism).
///
/// ## Why a PARALLEL primitive, not an `Orientation` param on `spawn_bar`
/// [`spawn_bar`]'s signature is a real, already-shipped API with existing
/// call sites (`combat_hud.rs`/`hotbar.rs`/etc.) — adding a required
/// `Orientation` parameter to it would break every one of them (the exact
/// thing this whole phase's "additive, don't touch existing call sites"
/// brief rules out). A second, size-matched function pair
/// (`spawn_orb_bar`/[`HudOrbBar`]/[`update_orb_bars`], mirroring `spawn_bar`/
/// [`HudBar`]/[`update_bars`]'s exact shape one-for-one) costs a little
/// duplication but means zero existing caller needs to change, which this
/// crate's own established precedent favours (see `slot.rs`'s "prefer the
/// wrapper that needs zero existing-callsite changes" posture, applied here
/// to a sibling function instead of a wrapper since the fill AXIS itself
/// differs, not just an extra parameter).
#[must_use]
pub fn spawn_orb_bar(
    commands: &mut Commands,
    theme: &HudTheme,
    fill_image: Handle<Image>,
    frame_image: Option<Handle<Image>>,
    width_px: f32,
    height_px: f32,
    value: BarValue,
) -> bevy::ecs::entity::Entity {
    let container = commands
        .spawn((HudOrbBar, value, Node {
            width: Val::Px(width_px),
            height: Val::Px(height_px),
            overflow: bevy::ui::Overflow::clip(),
            // A liquid-fill orb is circular in the final art (the frame
            // PNG's alpha carves the circle) — a plain square clip
            // region is enough since the frame overlay masks the
            // corners; see the module doc comment above.
            border_radius: bevy::ui::BorderRadius::all(Val::Px(theme.radius.sm)),
            ..Default::default()
        }))
        .with_children(|parent| {
            parent.spawn((
                HudOrbBarFill,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(value.fraction() * 100.0),
                    ..Default::default()
                },
                ImageNode::new(fill_image),
            ));
        })
        .id();

    if let Some(frame_image) = frame_image {
        commands.entity(container).with_children(|parent| {
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                ImageNode::new(frame_image),
                // The frame is pure decoration on top of the fill — it must
                // never intercept pointer events meant for whatever's
                // beneath it (the orb's own hover/tooltip, if any).
                Pickable::IGNORE,
            ));
        });
    }

    container
}

/// Resizes every orb bar's fill child to its container's current
/// [`BarValue`] fraction, bottom-anchored (grows the fill's HEIGHT, unlike
/// [`update_bars`]'s width resize) — the vertical counterpart to
/// [`update_bars`], same `Changed<BarValue>` gate.
pub(crate) fn update_orb_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudOrbBar>)>,
    mut fills: Query<&mut Node, With<HudOrbBarFill>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(mut node) = fills.get_mut(child) {
                node.height = Val::Percent(value.fraction() * 100.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, update_bars);
        app
    }

    /// Spawning a bar at half value gives the fill child a 50% width; a
    /// later `BarValue` mutation resizes it on the next update — the T56.1
    /// acceptance bar for this primitive.
    #[test]
    fn bar_fill_tracks_value_changes() {
        let mut app = new_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_bar(
                &mut commands,
                &theme,
                theme.palette.health,
                theme.palette.health_bg,
                200.0,
                20.0,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let fill_width = |app: &mut App, container: Entity| -> Val {
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let fill_entity = children
                .into_iter()
                .find(|&e| app.world().get::<HudBarFill>(e).is_some())
                .expect("bar has a fill child");
            app.world().get::<Node>(fill_entity).unwrap().width
        };

        assert_eq!(fill_width(&mut app, container), Val::Percent(50.0));

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 25.0;
        app.update();

        assert_eq!(fill_width(&mut app, container), Val::Percent(25.0));
    }

    /// A zero/negative `max` degrades to a `0.0` fraction, never NaN — the
    /// "degrade clean" rule applied to bar math.
    #[test]
    fn zero_max_degrades_to_zero_fraction() {
        let value = BarValue::new(10.0, 0.0);
        assert_eq!(value.fraction(), 0.0);
    }

    fn new_orb_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, update_orb_bars);
        app
    }

    /// `spawn_orb_bar` at half value gives the fill child a 50% HEIGHT
    /// (bottom-anchored, unlike the horizontal bar's width fill); a later
    /// `BarValue` mutation resizes it on the next update — the T57.8
    /// acceptance bar for the vertical orb-bar primitive.
    #[test]
    fn orb_bar_fill_tracks_value_changes_by_height() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                160.0,
                160.0,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let fill_height = |app: &mut App, container: Entity| -> Val {
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let fill_entity = children
                .into_iter()
                .find(|&e| app.world().get::<HudOrbBarFill>(e).is_some())
                .expect("orb bar has a fill child");
            app.world().get::<Node>(fill_entity).unwrap().height
        };

        assert_eq!(fill_height(&mut app, container), Val::Percent(50.0));

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 25.0;
        app.update();

        assert_eq!(fill_height(&mut app, container), Val::Percent(25.0));
    }

    /// The optional frame overlay, when given, spawns as a SECOND child
    /// carrying [`Pickable::IGNORE`] — the "decoration never blocks
    /// interaction with what's underneath" contract.
    #[test]
    fn orb_bar_frame_overlay_ignores_picking() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                160.0,
                160.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        assert_eq!(children.len(), 2, "fill child + frame overlay child");
        let frame_entity = children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_none())
            .expect("a non-fill (frame) child exists");
        assert_eq!(
            *app.world().get::<Pickable>(frame_entity).unwrap(),
            Pickable::IGNORE
        );
    }

    /// Omitting the frame overlay spawns only the fill child — the frame
    /// stays a genuinely OPTIONAL parameter.
    #[test]
    fn orb_bar_without_frame_spawns_only_fill_child() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                None,
                160.0,
                160.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        assert_eq!(children.len(), 1, "only the fill child, no frame overlay");
    }
}
