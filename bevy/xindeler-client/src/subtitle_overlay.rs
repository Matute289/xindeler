//! BL-82 EM-5.16 close-out — the positional-sound subtitle overlay. Consumes
//! `SubtitleTriggered` messages (emitted by `crate::sfx`'s mappers when the
//! Accessibility→Subtitles toggle is on), resolves the frozen `subtitle-*`
//! key through `Localization`, and shows a fading bottom-centre row with a
//! direction arrow pointing from the `MainCamera` toward the sound source.

use bevy::prelude::*;
use xindeler_ui::i18n::Localization;

/// Sound-source direction relative to where the listener/camera faces,
/// collapsed to one of 8 compass arrows. `forward`/`right` are the camera's
/// horizontal basis; `to_emitter` is `(emitter_pos - camera_pos)`.
#[must_use]
fn subtitle_arrow(forward: Vec3, right: Vec3, to_emitter: Vec3) -> &'static str {
    // Project onto the horizontal camera basis (ignore vertical).
    let f = to_emitter.dot(forward.normalize_or_zero());
    let r = to_emitter.dot(right.normalize_or_zero());
    if f == 0.0 && r == 0.0 {
        return "•";
    }
    // atan2(right, forward): 0 = dead ahead, +pi/2 = hard right.
    let angle = r.atan2(f);
    // 8 sectors of pi/4, centred on each arrow direction.
    const ARROWS: [&str; 8] = ["↑", "↗", "→", "↘", "↓", "↙", "←", "↖"];
    let sector = (((angle / std::f32::consts::FRAC_PI_4).round() as i32) & 7) as usize;
    ARROWS[sector]
}

/// One positional-sound subtitle to surface, emitted by `crate::sfx`'s SFX
/// mappers when the Subtitles accessibility toggle is on.
#[derive(Message, Debug, Clone)]
pub struct SubtitleTriggered {
    /// A `subtitle-*` key from `sfx.ron` / `hud/subtitles.ftl`.
    pub key: String,
    /// Emitter world position (Bevy y-up), for the direction arrow.
    pub emitter_pos: Vec3,
}

/// Marks the fixed bottom-centre column that subtitle rows are children of.
#[derive(Component)]
struct SubtitleOverlayRoot;

/// A live subtitle row; despawns when `timer` finishes.
#[derive(Component)]
struct SubtitleRow {
    timer: Timer,
}

/// Seconds a subtitle stays on screen (matches legacy voxygen's ~get-out-of-
/// the-way dwell).
const SUBTITLE_TTL_SECS: f32 = 3.0;
/// Newest-N cap so a noisy scene can't stack an unbounded column.
const SUBTITLE_MAX_ROWS: usize = 4;

pub struct SubtitleOverlayPlugin;

impl Plugin for SubtitleOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SubtitleTriggered>()
            .add_systems(Startup, spawn_subtitle_overlay_root)
            .add_systems(
                Update,
                (spawn_subtitle_rows, fade_and_despawn_subtitle_rows),
            );
    }
}

fn spawn_subtitle_overlay_root(mut commands: Commands) {
    commands.spawn((
        SubtitleOverlayRoot,
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Percent(18.0),
            left: Val::Percent(25.0),
            width: Val::Percent(50.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: Val::Px(4.0),
            ..Default::default()
        },
        // Behind interactive HUD, above the world — reuse the crate's HUD z
        // convention (see `xindeler_ui::zlayer`); a mid GlobalZIndex is fine.
        GlobalZIndex(50),
        Pickable::IGNORE,
    ));
}

fn spawn_subtitle_rows(
    mut commands: Commands,
    mut incoming: MessageReader<SubtitleTriggered>,
    localization: NonSend<Localization>,
    camera: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    root: Query<(Entity, Option<&Children>), With<SubtitleOverlayRoot>>,
    rows: Query<Entity, With<SubtitleRow>>,
) {
    let Ok((root_entity, children)) = root.single() else {
        incoming.clear();
        return;
    };
    let cam = camera.single().ok();
    for msg in incoming.read() {
        let arrow = cam.map_or("•", |c| {
            subtitle_arrow(*c.forward(), *c.right(), msg.emitter_pos - c.translation())
        });
        let text = format!("{arrow} {}", localization.tr(&msg.key));
        commands.entity(root_entity).with_children(|p| {
            p.spawn((
                SubtitleRow {
                    timer: Timer::from_seconds(SUBTITLE_TTL_SECS, TimerMode::Once),
                },
                Text::new(text),
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.9)),
                Node {
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                    ..Default::default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            ));
        });
    }
    // Trim to the newest N (despawn oldest overflow).
    if let Some(children) = children {
        let live: Vec<Entity> = children.iter().filter(|c| rows.get(*c).is_ok()).collect();
        if live.len() > SUBTITLE_MAX_ROWS {
            for old in &live[..live.len() - SUBTITLE_MAX_ROWS] {
                commands.entity(*old).despawn();
            }
        }
    }
}

fn fade_and_despawn_subtitle_rows(
    mut commands: Commands,
    time: Res<Time>,
    mut rows: Query<(Entity, &mut SubtitleRow, &mut TextColor)>,
) {
    for (entity, mut row, mut color) in &mut rows {
        row.timer.tick(time.delta());
        let remaining = row.timer.fraction_remaining();
        color.0.set_alpha(0.9 * remaining);
        if row.timer.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_points_the_right_way() {
        let fwd = Vec3::new(0.0, 0.0, -1.0); // camera looks -Z
        let right = Vec3::X;
        assert_eq!(subtitle_arrow(fwd, right, Vec3::new(0.0, 0.0, -5.0)), "↑"); // ahead
        assert_eq!(subtitle_arrow(fwd, right, Vec3::new(5.0, 0.0, 0.0)), "→"); // to the right
        assert_eq!(subtitle_arrow(fwd, right, Vec3::new(-5.0, 0.0, 0.0)), "←"); // to the left
        assert_eq!(subtitle_arrow(fwd, right, Vec3::new(0.0, 0.0, 5.0)), "↓"); // behind
        assert_eq!(subtitle_arrow(fwd, right, Vec3::ZERO), "•"); // on top of listener
    }
}
