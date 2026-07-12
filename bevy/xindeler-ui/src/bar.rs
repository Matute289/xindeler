//! BL-82 EM-5.1 T56.1 — the Progress bar / globe primitive.
//!
//! The shape every always-on combat readout needs (health/energy/poise/XP —
//! legacy's `skillbar.rs` bars): a background track + a fill child whose
//! width tracks a `current/max` value. [`BarValue`] is the single piece of
//! state a caller (EM-5.2's mirror-reading systems) updates; [`update_bars`]
//! is the one system that turns it into a fill-width, `Changed<BarValue>`-
//! gated so it costs nothing on ticks where nothing changed.

use bevy::{
    ecs::{
        component::Component,
        hierarchy::Children,
        query::{Changed, With},
        system::{Commands, Query},
    },
    ui::{BackgroundColor, Node, Val},
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
/// still).
pub(crate) fn update_bars(
    bars: Query<(&BarValue, &Children), Changed<BarValue>>,
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
}
