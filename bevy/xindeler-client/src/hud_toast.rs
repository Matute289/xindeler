//! BL-82 EM-4.8 (task board T47.10, worksheet [Q3]=A) — the client-side half
//! of the `on_enter_message -> HudToast` narrative hook.
//!
//! BL-82 EM-5.1 T56.5: this module now ONLY bridges — it reads
//! [`xindeler_protocol::HudToast`] messages and pushes their text into
//! [`xindeler_ui::notification::NotificationQueue`], the real queued
//! notification widget (`XindelerUiPlugin`, added alongside
//! `combat_hud::CombatHudViewPlugin` in every shell that also adds this
//! plugin). The throwaway single-slot "last write wins" rendering EM-4.8
//! shipped (a one-off `HudToastRoot`/`HudToastText`/fade-timer) is GONE —
//! `xindeler-ui`'s widget now owns rendering/queueing/fading. This module
//! never touches the [`HudToast`] message type or the server-side
//! `xindeler_protocol::narrative` hook — exactly the seam EM-4.8's own
//! original doc comment said Phase 5 would use ("Phase 5 can replace this
//! rendering later without touching the hook").
//!
//! Compiled only under the `listen-server`/`net-client` cargo features (the
//! only modes where `xindeler-protocol`/`HudToast` are even linked — see
//! `main.rs`'s `#[cfg]`-gated module list).

use bevy::prelude::*;
use xindeler_protocol::HudToast;
use xindeler_ui::notification::NotificationQueue;

/// Bridges [`HudToast`] arrivals into the shared [`NotificationQueue`].
/// `Option<ResMut<NotificationQueue>>` (not a bare `ResMut`) so this system
/// degrades clean (silently drops the toast) rather than panicking on a
/// shell that, for whatever future reason, adds this plugin without also
/// adding `xindeler_ui::XindelerUiPlugin` — every shell that adds this
/// plugin today also adds `combat_hud::CombatHudViewPlugin` (which does),
/// but this system does not hard-depend on that co-registration ordering.
pub struct HudToastViewPlugin;

impl Plugin for HudToastViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, bridge_hud_toasts_to_notification_queue);
    }
}

fn bridge_hud_toasts_to_notification_queue(
    mut events: MessageReader<HudToast>,
    queue: Option<ResMut<NotificationQueue>>,
) {
    let Some(mut queue) = queue else {
        return;
    };
    for toast in events.read() {
        queue.push(toast.text.clone());
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<HudToast>();
        app.init_resource::<NotificationQueue>();
        app
    }

    /// Receiving a `HudToast` pushes its text into the shared queue — the
    /// real acceptance bar for T56.5's "subsumes EM-4.8" claim (the queue
    /// itself is tested generically in `xindeler-ui`; this test is the
    /// BRIDGE proof). Asserts on `pending_len()` (not `current()`): actually
    /// promoting a pending entry to "current" is `xindeler_ui::notification::
    /// advance_notifications`'s job, a `pub(crate)` system this crate has no
    /// access to run directly — the bridge's own contract ends at "the text
    /// reached the queue", which `pending_len()` proves.
    #[test]
    fn a_hud_toast_arrival_is_pushed_into_the_notification_queue() {
        let mut app = new_app();
        app.world_mut().write_message(HudToast {
            text: "Welcome to the mist-shrouded manor.".to_owned(),
        });
        app.world_mut()
            .run_system_once(bridge_hud_toasts_to_notification_queue)
            .expect("bridge system runs");

        let queue = app.world().resource::<NotificationQueue>();
        assert_eq!(
            queue.pending_len(),
            1,
            "the toast's text must reach the shared queue"
        );
    }

    /// Several toasts arriving in the same frame all queue (FIFO), unlike
    /// the retired EM-4.8 rendering's "last write wins" behaviour.
    #[test]
    fn several_toasts_in_one_frame_all_queue_in_order() {
        let mut app = new_app();
        app.world_mut().write_message(HudToast {
            text: "first".to_owned(),
        });
        app.world_mut().write_message(HudToast {
            text: "second".to_owned(),
        });
        app.world_mut()
            .run_system_once(bridge_hud_toasts_to_notification_queue)
            .expect("bridge system runs");

        let queue = app.world().resource::<NotificationQueue>();
        assert_eq!(
            queue.pending_len(),
            2,
            "both toasts must be queued (nothing has been promoted to 'current' yet — that's \
             advance_notifications' job)"
        );
    }

    /// A shell that somehow never inserted `NotificationQueue` degrades
    /// clean (the toast is silently dropped) rather than panicking.
    #[test]
    fn missing_notification_queue_resource_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<HudToast>();
        // Deliberately NOT calling `init_resource::<NotificationQueue>()`.
        app.world_mut().write_message(HudToast {
            text: "should be dropped, not panic".to_owned(),
        });
        app.world_mut()
            .run_system_once(bridge_hud_toasts_to_notification_queue)
            .expect("bridge system runs without the queue resource present");
    }
}
