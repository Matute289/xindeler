//! BL-82 EM-5.1 T56.5 — the real queued notification/toast widget,
//! **subsuming EM-4.8's throwaway `HudToastViewPlugin`**
//! (`bevy/xindeler-client/src/hud_toast.rs`).
//!
//! EM-4.8's own module doc comment says it plainly: "Phase 5 can replace
//! this rendering later without touching the hook: it only ever reads the
//! plain `xindeler_protocol::HudToast` message." This module is that
//! replacement — a generic notification primitive with NO dependency on
//! `xindeler-protocol` (this crate stays a pure widget-kit crate; the
//! `HudToast` → [`NotificationQueue::push`] bridging happens in
//! `xindeler-client::hud_toast`, which already depends on
//! `xindeler-protocol`). Unlike the old single-slot "last toast wins"
//! behaviour, this is a real FIFO QUEUE — several notifications arriving in
//! quick succession all get shown, one after another, instead of the latest
//! silently clobbering the others. This is also the presentation primitive a
//! later loot-pickup feed (EM-5.6) reuses (spec §2 EM-5.1 point 5).

use std::collections::VecDeque;

use bevy::{
    color::Alpha as _,
    ecs::{
        resource::Resource,
        system::{Commands, Res, ResMut},
    },
    prelude::{BackgroundColor, Node, PositionType, Text, TextColor, TextFont, Val, Visibility},
    text::{FontSize, FontSource},
    time::Time,
};

use crate::theme::{HudFonts, HudTheme};

/// Total on-screen time (seconds) once a notification starts showing,
/// including its fade-out.
const NOTIFICATION_DURATION_SECS: f32 = 6.0;
/// How much of [`NOTIFICATION_DURATION_SECS`], at the end, fades to
/// transparent.
const NOTIFICATION_FADE_SECS: f32 = 1.5;

/// The pending queue + the currently-showing entry's remaining time. A
/// `Resource` (not per-entity state) — there is exactly one shared toast
/// slot on screen at a time, same visual budget as the EM-4.8 predecessor,
/// just fed from a real queue instead of "last write wins".
#[derive(Resource, Debug, Default)]
pub struct NotificationQueue {
    pending: VecDeque<String>,
    current: Option<String>,
    remaining_secs: f32,
}

impl NotificationQueue {
    /// Enqueues `text` to show once the current notification (and everything
    /// queued before it) finishes.
    pub fn push(&mut self, text: impl Into<String>) { self.pending.push_back(text.into()); }

    /// The text currently on screen, if any.
    #[must_use]
    pub fn current(&self) -> Option<&str> { self.current.as_deref() }

    /// How many notifications are queued behind the current one (0 if the
    /// queue is otherwise empty).
    #[must_use]
    pub fn pending_len(&self) -> usize { self.pending.len() }
}

/// Marks the shared toast label the queue renders into.
#[derive(bevy::ecs::component::Component, Debug, Clone, Copy, Default)]
pub struct HudNotificationLabel;

/// Marks the toast root (whose [`Visibility`] toggles).
#[derive(bevy::ecs::component::Component, Debug, Clone, Copy, Default)]
pub struct HudNotificationRoot;

/// Spawns the ONE shared notification root+label this whole crate's
/// [`advance_notifications`] system drives, hidden by default (top-centre,
/// matching EM-4.8's own placement). Every producer (the `HudToast` bridge,
/// a future loot-pickup feed) just calls [`NotificationQueue::push`] —
/// nothing else spawns its own toast UI.
pub(crate) fn spawn_shared_notification_widget(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
) {
    commands
        .spawn((
            HudNotificationRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(24.0),
                left: Val::Percent(50.0),
                ..Default::default()
            },
            BackgroundColor(theme.palette.panel_bg),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                HudNotificationLabel,
                Text(String::new()),
                TextFont {
                    font: FontSource::Handle(fonts.title.clone()),
                    font_size: FontSize::Px(22.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        });
}

/// Advances the queue: if nothing is currently showing and the queue is
/// non-empty, pops the front and starts its timer; otherwise counts the
/// current notification's remaining time down and fades/hides/advances to
/// the next one once it elapses.
pub(crate) fn advance_notifications(
    time: bevy::ecs::system::Res<Time>,
    mut queue: ResMut<NotificationQueue>,
    mut roots: bevy::ecs::system::Query<
        &mut Visibility,
        bevy::ecs::query::With<HudNotificationRoot>,
    >,
    mut texts: bevy::ecs::system::Query<
        (&mut bevy::prelude::Text, &mut bevy::prelude::TextColor),
        bevy::ecs::query::With<HudNotificationLabel>,
    >,
) {
    fn try_start_next(
        queue: &mut NotificationQueue,
        roots: &mut bevy::ecs::system::Query<
            &mut Visibility,
            bevy::ecs::query::With<HudNotificationRoot>,
        >,
    ) {
        if queue.current.is_none()
            && let Some(next) = queue.pending.pop_front()
        {
            queue.current = Some(next);
            queue.remaining_secs = NOTIFICATION_DURATION_SECS;
            for mut visibility in roots.iter_mut() {
                *visibility = Visibility::Visible;
            }
        }
    }

    try_start_next(&mut queue, &mut roots);

    let Some(current_text) = queue.current.clone() else {
        return;
    };

    queue.remaining_secs = (queue.remaining_secs - time.delta_secs()).max(0.0);
    let alpha = (queue.remaining_secs / NOTIFICATION_FADE_SECS).clamp(0.0, 1.0);
    for (mut text, mut color) in &mut texts {
        text.0.clone_from(&current_text);
        color.0.set_alpha(alpha);
    }

    if queue.remaining_secs <= 0.0 {
        queue.current = None;
        for mut visibility in &mut roots {
            *visibility = Visibility::Hidden;
        }
        // Immediately try to start the next queued notification in the SAME
        // run, so a caller polling once per elapsed duration (this module's
        // own test) sees the queue advance within one call, not lag a whole
        // extra frame behind — the fresh notification's own full duration
        // starts clean (it does not inherit this frame's already-spent delta).
        try_start_next(&mut queue, &mut roots);
    }
}

#[cfg(test)]
mod tests {
    use bevy::{ecs::system::RunSystemOnce, prelude::*};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<NotificationQueue>();
        app.world_mut()
            .spawn((HudNotificationRoot, Visibility::Hidden));
        app.world_mut().spawn((
            HudNotificationLabel,
            Text(String::new()),
            TextColor(Color::WHITE),
        ));
        app.update();
        app
    }

    /// Pushing two notifications shows the FIRST one immediately; the
    /// second stays queued (not clobbered) until the first's duration
    /// elapses.
    #[test]
    fn queue_shows_notifications_in_order_not_last_write_wins() {
        let mut app = new_app();
        app.world_mut()
            .resource_mut::<NotificationQueue>()
            .push("first");
        app.world_mut()
            .resource_mut::<NotificationQueue>()
            .push("second");

        app.world_mut()
            .run_system_once(advance_notifications)
            .expect("first run shows the first notification");
        assert_eq!(
            app.world().resource::<NotificationQueue>().current(),
            Some("first")
        );
        assert_eq!(
            app.world().resource::<NotificationQueue>().pending_len(),
            1,
            "the second notification stays queued, not dropped"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                NOTIFICATION_DURATION_SECS + 1.0,
            ));
        app.world_mut()
            .run_system_once(advance_notifications)
            .expect("second run advances to the next notification");
        assert_eq!(
            app.world().resource::<NotificationQueue>().current(),
            Some("second"),
            "the queued second notification now shows"
        );
    }
}
