use common_base::span;
use std::time::{Duration, Instant};
use vek::Lerp;

/// A type for maintaining consistent tick/frame pacing.
pub struct Clock {
    // Inputs
    /// This is the dt that the Clock tries to archive with each call of tick.
    target_dt: Duration,

    // Working state
    /// The amount of real time that has passed on the clock
    real_time: Duration,
    /// The amount of game time that has passed on the clock
    game_time: Duration,
    /// The last time the clock was ticked
    last_tick: Instant,
    /// The last time we started performing work
    last_work: Instant,
    /// The number of ticks that have elapsed so far
    tick: u64,

    /// The average time between ticks, seconds
    average_dt: f64,
    /// The average amount of time within each tick in which we're busy (i.e:
    /// not sleeping)
    average_busy: f64,
    /// The average amount of variance between ticks
    average_variance: f64,
    /// The time that passed between the last tick, and the tick before it
    last_real_dt: f64,
    /// The dt to be used for the next game tick, in game time.
    last_game_dt: f64,
}

pub struct ClockStats {
    /// A weighted average of the recent 'busy period' (i.e: time spent doing
    /// work rather than sleeping) per tick.
    pub average_busy_dt: Duration,
    /// A weighted average of the recent number of ticks per second.
    pub average_tps: f64,
    /// A weighted average of the variance of the clock relative to the average
    /// TPS.
    pub average_variance: Duration,
}

/// The weighting used to calculate averages. Must be > 0.0. 1.0 = no averaging.
const SMOOTH_WEIGHT: f64 = 0.05;
/// The proportion of the difference between real and game time that gets
/// applied each tick to keep the two aligned.
const NUDGE_RATE: f64 = 0.05;
/// The maximum dt that the game should ever run at.
const MAX_GAME_DT: f64 = 1.0 / 5.0;

impl Clock {
    pub fn new(target_dt: Duration) -> Self {
        Self {
            target_dt,

            real_time: Duration::ZERO,
            game_time: Duration::ZERO,
            last_tick: Instant::now(),
            last_work: Instant::now(),
            tick: 0,

            average_dt: target_dt.as_secs_f64(),
            average_busy: target_dt.as_secs_f64(),
            average_variance: 0.0,
            last_real_dt: target_dt.as_secs_f64(),
            last_game_dt: target_dt.as_secs_f64(),
        }
    }

    pub fn set_target_dt(&mut self, target_dt: Duration) {
        if target_dt != self.target_dt {
            self.target_dt = target_dt;

            // The target dt has changed, throw out the existing stats to avoid problems
            self.average_dt = target_dt.as_secs_f64();
            self.average_busy = target_dt.as_secs_f64();
            self.average_variance = 0.0;
        }
    }

    pub fn stats(&self) -> ClockStats {
        ClockStats {
            average_busy_dt: Duration::from_secs_f64(self.average_busy),
            average_tps: 1.0 / self.average_dt.max(0.000001),
            average_variance: Duration::from_secs_f64(self.average_variance),
        }
    }

    pub fn real_dt(&self) -> Duration { Duration::from_secs_f64(self.last_real_dt) }

    pub fn game_dt(&self) -> Duration { Duration::from_secs_f64(self.last_game_dt) }

    /// Smooth + nudge the game-clock delta toward real time for one tick, then
    /// clamp it to `[0, MAX_GAME_DT]`.
    ///
    /// The ceiling (`MAX_GAME_DT`) stops a single lag spike from producing a
    /// huge gameplay dt. The **floor of `0`** is equally load-bearing: the game
    /// clock must never run backwards. When `game_time` transiently overshoots
    /// `real_time` — e.g. during the startup stall -> catch-up burst, where a
    /// long LOD/terrain bake first elevates `average_dt` and pushes `real_time`
    /// far ahead, then a run of near-zero-length catch-up frames lets
    /// `game_time` overshoot — the nudge term `(real - game) * NUDGE_RATE` can
    /// exceed the (by then decayed) `average_dt` and drive this negative.
    /// Without the floor, `game_dt()`'s `Duration::from_secs_f64` panics with
    /// "cannot convert float seconds to Duration: value is negative", a hard
    /// crash on startup. A single clamped-to-zero frame is harmless: the nudge
    /// pulls `game_time` back into line over the next few ticks.
    //
    // XINDELER: local addition (not upstream Veloren). This helper only extracts
    // the inline `last_game_dt` expression in `tick()` so it can be unit-tested;
    // the sole behavioural change vs. upstream is widening `.min(MAX_GAME_DT)` to
    // `.clamp(0.0, MAX_GAME_DT)` (adds the missing floor). On a `gitlab` upstream
    // sync, keep the `0.0` floor if upstream still edits this line.
    fn nudged_game_dt(average_dt: f64, real_secs: f64, game_secs: f64) -> f64 {
        (average_dt + (real_secs - game_secs) * NUDGE_RATE).clamp(0.0, MAX_GAME_DT)
    }

    pub fn tick(&mut self) {
        span!(_guard, "tick", "Clock::tick");
        span!(guard, "clock work");

        // Give the tick thread realtime priority to minimise stuttering. Don't do this
        // all the time to avoid upsetting the scheduler.
        if self.tick == 0
        /* .is_multiple_of(30) */
        {
            use thread_priority::*;
            // // We choose scheduler parameters based on averages from previous frames
            // // Try to target a tick period that's consistent with our current FPS (a low
            // but // consistent framerate is a better outcome than one that's
            // faster on paper but // is bouncing around all over the place).
            // let stable_dt = self.average_busy
            //     // Don't try to schedule for a tick rate that's higher than our target,
            // even if we     // could achieve it.
            //     .max(self.target_dt.as_secs_f64());
            // let priority = ThreadPriority::Deadline {
            //     runtime: Duration::from_secs_f64(self.average_busy * 0.5),
            //     deadline: Duration::from_millis(10),
            //     period: Duration::from_secs_f64(stable_dt),
            //     flags: Default::default(),
            // };
            let priority =
                ThreadPriority::Crossplatform(ThreadPriorityValue::try_from(90).unwrap());
            _ = cfg_select! {
                target_os = "linux" => std::thread::current().set_priority_and_policy(
                    // ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Deadline),
                    ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo),
                    priority,
                ),
                _ => std::thread::current().set_priority(priority),
            };
        }

        let this_tick = Instant::now();

        // Calculate average metrics

        let busy_time = self.last_work.elapsed();
        self.average_busy = Lerp::lerp(self.average_busy, busy_time.as_secs_f64(), SMOOTH_WEIGHT);

        let tick_time = (this_tick - self.last_tick).as_secs_f64();
        self.average_dt = Lerp::lerp(self.average_dt, tick_time, SMOOTH_WEIGHT);

        let variance = (tick_time - self.average_dt).abs();
        self.average_variance = Lerp::lerp(self.average_variance, variance, SMOOTH_WEIGHT);

        drop(guard);

        // Sleep for any remaining time before the next tick
        if let Some(sleep_dur) = self.target_dt.checked_sub(busy_time) {
            spin_sleep::sleep(sleep_dur);
        }

        // Update clock state

        self.last_tick = this_tick;
        self.last_work = Instant::now();

        // Progress real and game time
        self.real_time += Duration::from_secs_f64(self.last_real_dt);
        self.game_time += Duration::from_secs_f64(self.last_game_dt);

        // Calculate the deltas for both real and game clocks. The real clock is
        // absolute: we can't alter the progression of time. However, we can
        // alter the game clock and nudge it toward real time. The reason we
        // don't want to keep the two *exactly* in time is that a lag spike on a
        // single tick would cause a corresponding jump in dt on the next tick, which
        // might produce strange results for any dt-dependent gameplay systems.
        // Instead, we gradually nudge the game time back toward real time over
        // several ticks.
        self.last_real_dt = tick_time;
        self.last_game_dt = Self::nudged_game_dt(
            self.average_dt,
            self.real_time.as_secs_f64(),
            self.game_time.as_secs_f64(),
        );

        self.tick += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the startup crash "cannot convert float seconds to
    /// Duration: value is negative". `Clock::game_dt()` calls
    /// `Duration::from_secs_f64(last_game_dt)`, so `last_game_dt` must never be
    /// negative. It is produced by [`Clock::nudged_game_dt`], which used to
    /// only `.min(MAX_GAME_DT)` (a ceiling, no floor): when `game_time`
    /// transiently overshoots `real_time`, the nudge term drives the raw
    /// value below zero.
    #[test]
    fn nudged_game_dt_is_never_negative_when_game_overshoots_real() {
        // game_time far ahead of real_time (the overshoot case). The *raw*,
        // pre-clamp value is unambiguously negative here...
        let raw = 0.05 + (1.0 - 5.0) * NUDGE_RATE;
        assert!(raw < 0.0, "test setup must exercise the negative raw case");
        // ...but the clamped result must be 0, and must convert to a Duration
        // without panicking.
        let dt = Clock::nudged_game_dt(0.05, 1.0, 5.0);
        assert_eq!(
            dt, 0.0,
            "overshoot must clamp the game dt to zero, got {dt}"
        );
        let _ = Duration::from_secs_f64(dt); // must not panic

        // A mild overshoot (avg_dt just under the nudge pull) also clamps to 0.
        let dt = Clock::nudged_game_dt(0.001, 0.0, 0.1);
        assert!(dt >= 0.0, "mild overshoot must not go negative, got {dt}");
        let _ = Duration::from_secs_f64(dt);
    }

    /// The floor must not disturb the normal behaviour: the ceiling still caps
    /// big lag spikes, and the nudge still moves `game_time` toward
    /// `real_time`.
    #[test]
    fn nudged_game_dt_keeps_ceiling_and_normal_nudge() {
        // real_time far ahead of game_time -> large positive nudge, capped.
        let dt = Clock::nudged_game_dt(0.033, 5.0, 1.0);
        assert!(
            (dt - MAX_GAME_DT).abs() < 1e-9,
            "large positive nudge must clamp to the ceiling, got {dt}"
        );
        // real == game -> just the smoothed average, untouched by the clamp.
        let dt = Clock::nudged_game_dt(0.016, 3.0, 3.0);
        assert!(
            (dt - 0.016).abs() < 1e-9,
            "balanced clock returns average_dt, got {dt}"
        );
    }

    /// End-to-end: replay the exact `Clock::tick()` game-dt recurrence over the
    /// adversarial "startup stall then fast catch-up burst" frame-time trace
    /// (a long bake frame, then a run of near-zero-length frames). This is the
    /// pattern that produced the live crash; with the floor, `last_game_dt`
    /// stays >= 0 across the whole trace so no `from_secs_f64` call panics.
    #[test]
    fn startup_stall_then_fast_catchup_never_produces_negative_game_dt() {
        // Mirror the fields the recurrence touches, seeded like `Clock::new`
        // with target_dt = ZERO (the embedded-player clock, EM-4.11).
        let mut real_time = 0.0f64;
        let mut game_time = 0.0f64;
        let mut average_dt = 0.0f64;
        let mut last_real_dt = 0.0f64;
        let mut last_game_dt = 0.0f64;

        // 3s startup stall, then 200 near-zero-length catch-up frames.
        let mut trace = vec![3.0f64];
        trace.extend([0.0005f64; 200]);

        for tau in trace {
            // Advance clocks by the previous tick's deltas (Clock::tick order).
            real_time += last_real_dt;
            assert!(
                last_game_dt >= 0.0,
                "game_time += from_secs_f64(last_game_dt) would panic: {last_game_dt}"
            );
            game_time += last_game_dt;
            // Smooth the average, then recompute the deltas.
            average_dt += (tau - average_dt) * SMOOTH_WEIGHT;
            last_real_dt = tau;
            last_game_dt = Clock::nudged_game_dt(average_dt, real_time, game_time);
            assert!(
                last_game_dt >= 0.0,
                "game_dt() would panic on from_secs_f64({last_game_dt})"
            );
        }
    }
}
