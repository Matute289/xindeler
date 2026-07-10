//! BL-82 EM-4.8 (task board T47.10, worksheet [Q3]=A) — the client-side half
//! of the `on_enter_message -> HudToast` narrative hook: a MINIMAL `bevy_ui`
//! timed-fade toast.
//!
//! Deliberately NOT Phase 5's real HUD/notification system — just enough to
//! prove the server-side hook (`xindeler_protocol::narrative`) fires
//! end-to-end and renders as something visible. Phase 5 can replace this
//! rendering later without touching the hook: it only ever reads the plain
//! [`xindeler_protocol::HudToast`] message, exactly like every other
//! consumer module in this crate (`entity_view`, `terrain_stream`, ...)
//! reads its own server message.
//!
//! Compiled only under the `listen-server`/`net-client` cargo features (the
//! only modes where `xindeler-protocol`/`HudToast` are even linked — see
//! `main.rs`'s `#[cfg]`-gated module list).

use bevy::prelude::*;
use xindeler_protocol::HudToast;

/// Total on-screen time (seconds) once a toast starts showing, INCLUDING the
/// fade-out — deliberately a small, hardcoded constant (not data-driven):
/// this is scaffolding, not a tuned UX value (see module doc comment).
const TOAST_DURATION_SECS: f32 = 6.0;
/// How much of [`TOAST_DURATION_SECS`], at the end, is spent fading to
/// transparent rather than shown at full opacity.
const TOAST_FADE_SECS: f32 = 1.5;

/// Marks the toast's root UI node (the one whose [`Visibility`] toggles).
#[derive(Component)]
struct HudToastRoot;

/// Marks the toast's text node (the one whose content/alpha get updated).
#[derive(Component)]
struct HudToastText;

/// Seconds remaining until the current toast fully hides. `0.0` (the
/// default) means "nothing showing".
#[derive(Resource, Default)]
struct ActiveToast {
    remaining_secs: f32,
}

/// Installs the minimal toast UI + the systems that show/fade it on
/// [`HudToast`] arrival.
pub struct HudToastViewPlugin;

impl Plugin for HudToastViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveToast>()
            .add_systems(Startup, spawn_toast_ui)
            .add_systems(Update, (receive_toasts, fade_toast).chain());
    }
}

/// Spawns a single, initially-invisible, top-centred text node — the whole
/// UI surface this module owns. `position_type: Absolute` + a full-width
/// parent with `justify_content: Center` centres the text without any
/// manual pixel-offset math.
fn spawn_toast_ui(mut commands: Commands) {
    commands
        .spawn((
            HudToastRoot,
            Node {
                display: Display::Flex,
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Px(24.0),
                justify_content: JustifyContent::Center,
                padding: UiRect::horizontal(Val::Px(16.0)),
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                HudToastText,
                Text(String::new()),
                TextFont::from_font_size(22.0),
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.0)),
            ));
        });
}

/// On every [`HudToast`] arrival: sets the toast's text, resets the fade
/// timer to the full [`TOAST_DURATION_SECS`], and makes the root visible.
/// Multiple toasts arriving in quick succession simply restart the timer
/// with the LATEST text — v1 has no queueing (deliberately minimal, see
/// module doc comment).
fn receive_toasts(
    mut events: MessageReader<HudToast>,
    mut active: ResMut<ActiveToast>,
    mut roots: Query<&mut Visibility, With<HudToastRoot>>,
    mut texts: Query<&mut Text, With<HudToastText>>,
) {
    for toast in events.read() {
        active.remaining_secs = TOAST_DURATION_SECS;
        for mut visibility in &mut roots {
            *visibility = Visibility::Visible;
        }
        for mut text in &mut texts {
            text.0.clone_from(&toast.text);
        }
    }
}

/// Counts [`ActiveToast::remaining_secs`] down every frame; during the last
/// [`TOAST_FADE_SECS`] the text's alpha eases linearly to zero, and once it
/// hits zero the root goes back to [`Visibility::Hidden`].
fn fade_toast(
    time: Res<Time>,
    mut active: ResMut<ActiveToast>,
    mut roots: Query<&mut Visibility, With<HudToastRoot>>,
    mut colors: Query<&mut TextColor, With<HudToastText>>,
) {
    if active.remaining_secs <= 0.0 {
        return;
    }

    active.remaining_secs = (active.remaining_secs - time.delta_secs()).max(0.0);
    let alpha = (active.remaining_secs / TOAST_FADE_SECS).clamp(0.0, 1.0);
    for mut color in &mut colors {
        color.0.set_alpha(alpha);
    }

    if active.remaining_secs <= 0.0 {
        for mut visibility in &mut roots {
            *visibility = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(HudToastViewPlugin);
        app.add_message::<HudToast>();
        // Runs `Startup` (spawns the toast UI) once. `MinimalPlugins`'
        // real-clock `time_system` also runs here, but only the FIRST time —
        // every later time-sensitive assertion below drives `Time` directly
        // via `advance_by` + a directly-invoked system (never a second
        // `app.update()`), so it never gets silently overwritten by a
        // real-wall-clock delta.
        app.update();
        app
    }

    /// Receiving a `HudToast` makes the root visible and sets the text.
    #[test]
    fn receiving_a_toast_shows_it() {
        let mut app = new_app();
        app.world_mut().write_message(HudToast {
            text: "Welcome to the mist-shrouded manor.".to_owned(),
        });
        app.world_mut()
            .run_system_once(receive_toasts)
            .expect("receive_toasts runs");

        let mut roots = app.world_mut().query::<(&Visibility, &HudToastRoot)>();
        let (visibility, _) = roots.single(app.world()).expect("exactly one toast root");
        assert_eq!(*visibility, Visibility::Visible);

        let mut texts = app.world_mut().query::<(&Text, &HudToastText)>();
        let (text, _) = texts.single(app.world()).expect("exactly one toast text");
        assert_eq!(text.0, "Welcome to the mist-shrouded manor.");

        let active = app.world().resource::<ActiveToast>();
        assert!(active.remaining_secs > 0.0);
    }

    /// After enough elapsed time, the toast fades out and hides again.
    #[test]
    fn toast_hides_after_its_duration_elapses() {
        let mut app = new_app();
        app.world_mut().write_message(HudToast {
            text: "brief notice".to_owned(),
        });
        app.world_mut()
            .run_system_once(receive_toasts)
            .expect("receive_toasts runs");

        // Advance the generic `Time` resource directly and invoke
        // `fade_toast` directly (never a second `app.update()`, which would
        // let `MinimalPlugins`' real-clock `time_system` recompute the delta
        // from wall-clock `Instant::now()` and clobber this controlled
        // jump).
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                TOAST_DURATION_SECS + 1.0,
            ));
        app.world_mut()
            .run_system_once(fade_toast)
            .expect("fade_toast runs");

        let mut roots = app.world_mut().query::<(&Visibility, &HudToastRoot)>();
        let (visibility, _) = roots.single(app.world()).expect("exactly one toast root");
        assert_eq!(
            *visibility,
            Visibility::Hidden,
            "toast must hide once its duration elapses"
        );

        let active = app.world().resource::<ActiveToast>();
        assert_eq!(active.remaining_secs, 0.0);
    }
}
