//! Low-rate diagnostics drain for the `cpal` backend.
//!
//! ## Why this is NOT "the Kira pump" its name might suggest
//! `CpalBackend`'s actual audio servicing (advancing tweens, mixing samples,
//! pruning finished sounds) runs entirely on Kira's own dedicated audio
//! callback thread and is never reachable from `AudioManager`'s public API —
//! see the crate root doc comment for the full empirical + source-level
//! justification. There is nothing here to "drive" from a Bevy schedule.
//!
//! What DOES need a host-side call is `CpalBackend::pop_error()`: `cpal`
//! stream errors (device disconnected, format renegotiation failure, ...)
//! land in a bounded queue that only a caller draining it will ever see —
//! `voxygen`'s own `AudioFrontend::get_cpu_usage` established the same
//! "poll the backend from game code" precedent for `pop_cpu_usage()`. This
//! system is that drain, at a deliberately low, throttled rate (not
//! `FixedUpdate`/`Time<Fixed>`, which some other Bevy-migration crate may
//! already own for gameplay ticking — a `Local<Timer>` gate on the ordinary
//! `Update` schedule costs nothing on the frames it doesn't fire, without
//! repurposing the app-wide fixed timestep).
//!
//! 2 Hz was picked as "clearly fast enough that a stream error surfaces
//! within a fraction of a second, clearly slow enough to be free" — there is
//! no gameplay-correctness reason to poll faster, since nothing timing-
//! sensitive depends on this path (contrast the actual audio thread, which
//! runs at the real sample rate regardless of this system's cadence).

use std::time::Duration;

use bevy::prelude::*;
use tracing::warn;

use crate::manager::AudioBackend;

/// How often [`pump_audio_diagnostics`] drains buffered `cpal` stream errors.
const DIAGNOSTICS_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) fn pump_audio_diagnostics(
    time: Res<Time>,
    mut timer: Local<Option<Timer>>,
    mut backend: ResMut<AudioBackend>,
) {
    let timer = timer.get_or_insert_with(|| Timer::new(DIAGNOSTICS_INTERVAL, TimerMode::Repeating));
    if !timer.tick(time.delta()).just_finished() {
        return;
    }
    let Some(manager) = backend.manager_mut() else {
        return;
    };
    while let Some(err) = manager.backend_mut().pop_error() {
        warn!(?err, "xindeler-audio: cpal stream error");
    }
}
