//! BL-82 EM-5.18 Phase 1 — the hybrid target-selection system's soft-target
//! scan (design spec `docs/design/specs/2026-07-16-bl82-hybrid-target-
//! selection-design.md` §3.1, plan `docs/design/plans/2026-07-16-bl82-
//! hybrid-target-selection-plan.md` Phase 1).
//!
//! ## What this unblocks
//! `boss_nameplate::SelectedTarget` ships fully render-wired (health/poise/
//! level/name bars all read from it already) but nothing outside the
//! `XINDELER_SMOKE_FORCE_TARGET` smoke override ever set it to `Some(_)` in
//! real gameplay — see that module's doc comment. This module is the real
//! source: every frame, [`update_soft_target`] scans a cone in front of the
//! camera for `Enemy`-aligned mirrored entities, scores the survivors, and
//! writes the winner into `SelectedTarget`. Phase 2 (hard-lock) will layer a
//! `HardLock` resource on top that overrides this soft result when the
//! player presses `Select`; that resource does not exist yet, so this phase
//! always writes the soft-scan result.
//!
//! ## Precedence vs. the smoke override
//! `boss_nameplate::force_target_for_smoke_capture` force-selects whatever
//! non-local mirrored entity exists when `XINDELER_SMOKE_FORCE_TARGET` is
//! set, regardless of alignment/range/cone — a guaranteed visual-test path
//! that must keep working. [`update_soft_target`] resolves the precedence by
//! **skipping its own write entirely** whenever that env var is set (rather
//! than relying on system ordering between the two plugins): when the var is
//! set, this system no-ops and the smoke override is the sole writer; when
//! unset, the smoke override is itself a no-op (see its own gate) and this
//! system is the sole writer. Exactly one of the two ever touches
//! `SelectedTarget` in a given run, so no `.before()`/`.after()` edge between
//! `TargetSelectionPlugin` and `BossNameplateViewPlugin` is needed for this
//! guarantee to hold.
//!
//! ## Purity
//! 100% Bevy + `xindeler-protocol`, like `entity_view.rs`/`boss_nameplate.rs`
//! — no `specs`, no server crate. Compiled only under the `listen-server`/
//! `net-client` features, same gate as every other `Net*`-reading module in
//! this crate. [`best_soft_target`] itself takes no Bevy types at all beyond
//! `Entity`/`Vec3` — it's a plain function, unit-tested without an `App`.

use bevy::prelude::*;
use xindeler_protocol::{NetAlignment, NetHealth, NetLocalPlayer, NetUid};

use crate::{boss_nameplate::SelectedTarget, camera::FlyCam, entity_view::Interpolated};

/// Default acquisition range (metres) — wider than the melee cone on purpose
/// (spec §3.1: "soft-target should acquire before you're in melee range").
pub const DEFAULT_MAX_RANGE: f32 = 25.0;
/// Default acquisition half-angle (degrees) either side of camera-forward.
pub const DEFAULT_HALF_ANGLE_DEG: f32 = 60.0;
/// Default distance-term weight in `P = alpha/d + beta*cos(theta)`.
pub const DEFAULT_ALPHA: f32 = 1.0;
/// Default angle-term weight — dominant so "what you're looking at" wins
/// once a candidate is in range (spec §3.1).
pub const DEFAULT_BETA: f32 = 3.0;
/// Default hysteresis margin: a challenger must beat the current target's
/// score by more than `epsilon * current_score` (a RELATIVE margin — see
/// [`best_soft_target`]'s doc comment) to take over.
pub const DEFAULT_EPSILON: f32 = 0.1;

/// Client-feel tuning for the soft-target scorer (design spec §6): documented
/// default consts, each overridable via an `XINDELER_TARGETING_*` env var for
/// live A/B tuning without a rebuild — the exact pattern
/// [`crate::camera::OcclusionCullingConfig`] already establishes. These are
/// input/feel parameters, not game-balance content, so they stay a code
/// resource here rather than moving into the design repo's RON (same
/// reasoning `OcclusionCullingConfig` documents); migrating them into the
/// persisted `XindelerSettings` is a documented follow-up, out of scope for
/// this crate's change.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct TargetingConfig {
    /// Reject candidates farther than this (metres).
    pub max_range: f32,
    /// Reject candidates outside this half-angle of camera-forward (radians
    /// — converted from the env var's degrees at construction).
    pub half_angle_rad: f32,
    /// Distance-term weight.
    pub alpha: f32,
    /// Angle-term weight.
    pub beta: f32,
    /// Hysteresis margin (relative to the current target's score).
    pub epsilon: f32,
}

impl Default for TargetingConfig {
    fn default() -> Self {
        let max_range = env_f32("XINDELER_TARGETING_MAX_RANGE", DEFAULT_MAX_RANGE);
        let half_angle_deg = env_f32("XINDELER_TARGETING_HALF_ANGLE_DEG", DEFAULT_HALF_ANGLE_DEG);
        let alpha = env_f32("XINDELER_TARGETING_ALPHA", DEFAULT_ALPHA);
        let beta = env_f32("XINDELER_TARGETING_BETA", DEFAULT_BETA);
        let epsilon = env_f32("XINDELER_TARGETING_EPSILON", DEFAULT_EPSILON);
        Self {
            max_range,
            half_angle_rad: half_angle_deg.to_radians(),
            alpha,
            beta,
            epsilon,
        }
    }
}

/// `std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)`
/// — the exact idiom `OcclusionCullingConfig::default` already establishes.
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

/// One scoring candidate: a mirrored entity's Bevy id, world position (Bevy
/// axes), alignment, and liveness. Deliberately NOT a Bevy query item — this
/// is the plain-data boundary that keeps [`best_soft_target`] a pure,
/// App-less-testable function; [`update_soft_target`] is the only thing that
/// builds these from actual ECS queries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetCandidate {
    pub entity: Entity,
    pub pos: Vec3,
    pub alignment: NetAlignment,
    pub alive: bool,
}

/// Zeroes the vertical (Bevy Y-up) component — the same "flatten to the
/// ground plane" helper `player_input.rs::gather_input` already applies to
/// camera-forward/right before deriving movement intent.
fn flatten(v: Vec3) -> Vec3 { Vec3::new(v.x, 0.0, v.z) }

/// The pure soft-target scorer (design spec §3.1): cone-rejects, scores, and
/// applies epsilon-margin hysteresis. No Bevy `Query`/`Res` — everything it
/// needs is a plain argument, so it's unit-testable without an `App`.
///
/// - `player_pos`/`camera_forward` are Bevy-axis world vectors.
/// - `current` is the presently-selected target, if any (for hysteresis).
/// - Rejects a candidate if: not `alive`, `alignment != Enemy`, `d >
///   max_range`, or the flattened angle to it exceeds `half_angle_rad`.
///   Candidates whose horizontal offset from the player is (numerically) zero —
///   directly above/below — have an undefined horizontal angle and are skipped
///   rather than dividing by zero.
/// - Survivors are scored `P = alpha/d + beta*cos(theta)` (spec's formula
///   verbatim) and the highest-`P` candidate is the raw winner.
/// - **Hysteresis** (relative margin): if `current` is itself a surviving
///   candidate, the raw winner only replaces it when `winner_score >
///   current_score + epsilon * current_score` — i.e. the challenger must beat
///   the current target by more than `epsilon` (a FRACTION of the current
///   score, matching spec §3.1's "ε ≈ 0.1·P" notation literally: the margin
///   scales with the current target's own score, not a fixed absolute unit). If
///   `current` is no longer a survivor (died, left the cone/range, or wasn't a
///   candidate at all), the raw winner is returned unconditionally — there is
///   nothing to keep hysteresis against.
pub fn best_soft_target(
    player_pos: Vec3,
    camera_forward: Vec3,
    current: Option<Entity>,
    config: &TargetingConfig,
    candidates: impl IntoIterator<Item = TargetCandidate>,
) -> Option<Entity> {
    let flat_forward = flatten(camera_forward).normalize_or_zero();

    let mut current_score: Option<f32> = None;
    let mut best: Option<(Entity, f32)> = None;

    for candidate in candidates {
        if !candidate.alive || candidate.alignment != NetAlignment::Enemy {
            continue;
        }
        let offset = candidate.pos - player_pos;
        let d = offset.length();
        if d > config.max_range || d <= f32::EPSILON {
            continue;
        }
        let flat_dir = flatten(offset).normalize_or_zero();
        if flat_dir == Vec3::ZERO {
            // Directly above/below the player (or camera-forward itself is
            // degenerate) — the horizontal angle is undefined; skip rather
            // than score a meaningless theta.
            continue;
        }
        let cos_theta = flat_forward.dot(flat_dir).clamp(-1.0, 1.0);
        let theta = cos_theta.acos();
        if theta > config.half_angle_rad {
            continue;
        }

        let score = config.alpha / d + config.beta * cos_theta;
        if Some(candidate.entity) == current {
            current_score = Some(score);
        }
        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((candidate.entity, score));
        }
    }

    let (winner, winner_score) = best?;
    match current_score {
        Some(cur) if Some(winner) != current => {
            if winner_score > cur + config.epsilon * cur {
                Some(winner)
            } else {
                current
            }
        },
        _ => Some(winner),
    }
}

/// Every frame: gathers the local player + camera + candidate mirrored
/// entities, calls [`best_soft_target`], and writes the result into
/// `SelectedTarget`. See the module doc comment for the smoke-override
/// precedence this system implements by skipping its own write.
#[cfg(any(feature = "listen-server", feature = "net-client"))]
pub fn update_soft_target(
    config: Res<TargetingConfig>,
    cameras: Query<&Transform, With<FlyCam>>,
    local_player: Query<(&Transform, Option<&Interpolated>), With<NetLocalPlayer>>,
    candidates: Query<
        (
            Entity,
            &Transform,
            Option<&Interpolated>,
            &NetAlignment,
            &NetHealth,
        ),
        (With<NetUid>, Without<NetLocalPlayer>),
    >,
    mut target: ResMut<SelectedTarget>,
) {
    // Smoke-override precedence: see the module doc comment. Skip our own
    // write entirely so `force_target_for_smoke_capture` is the sole writer
    // while the env var is set, regardless of system ordering.
    if std::env::var("XINDELER_SMOKE_FORCE_TARGET").is_ok_and(|v| v != "0") {
        return;
    }

    // Compute the next selection into a local, then assign only on a real
    // change: an unconditional write to `ResMut<SelectedTarget>` marks it
    // changed EVERY frame, defeating the `is_changed()` early-out in
    // `boss_nameplate::sync_nameplate_visibility` (which is exactly why
    // `SelectedTarget` derives `PartialEq`).
    let next = 'next: {
        let Ok(cam_transform) = cameras.single() else {
            break 'next None;
        };
        let Ok((player_transform, player_interp)) = local_player.single() else {
            // No embedded local player yet (e.g. `net-client`'s spectator-only
            // v1) — degrade clean like every other consumer of
            // `NetLocalPlayer`.
            break 'next None;
        };
        let player_pos = player_interp.map_or(player_transform.translation, |i| i.pos);
        let camera_forward = *cam_transform.forward();

        let candidate_list =
            candidates
                .iter()
                .map(
                    |(entity, transform, interp, alignment, health)| TargetCandidate {
                        entity,
                        pos: interp.map_or(transform.translation, |i| i.pos),
                        alignment: *alignment,
                        alive: health.current > 0.0,
                    },
                );

        best_soft_target(
            player_pos,
            camera_forward,
            target.0,
            &config,
            candidate_list,
        )
    };

    if target.0 != next {
        target.0 = next;
    }
}

/// Ground offset (Bevy Y) the ring is drawn at — just above the target's
/// feet so it doesn't z-fight with flat terrain.
const MARKER_GROUND_OFFSET: f32 = 0.05;
/// Ring radius (metres).
const MARKER_RADIUS: f32 = 0.8;
/// Dim (soft-target) ring color — a translucent white/grey. Phase 2 will add
/// a bright variant for a hard lock (`TargetLockKind`, not built yet).
const MARKER_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.35);

/// The dim world-space marker (spec §3.5): a flat ring gizmo drawn at the
/// current soft-target's feet every frame it's selected. Deliberately the
/// simplest option available in this codebase (confirmed by grep: no
/// existing billboard/quad-mesh precedent anywhere in `bevy/xindeler-client`)
/// — `Gizmos` needs no spawned entity, no new asset, and no despawn/lifecycle
/// bookkeeping; it simply isn't redrawn once `SelectedTarget` clears. Reusing
/// an existing `HudImageKey` was considered and rejected: none of the 59
/// existing keys is a ring/reticle shape, so that path would be a visual
/// misuse, not a real reuse.
#[cfg(any(feature = "listen-server", feature = "net-client"))]
fn draw_soft_target_marker(
    target: Res<SelectedTarget>,
    positions: Query<(&Transform, Option<&Interpolated>)>,
    mut gizmos: Gizmos,
) {
    let Some(entity) = target.0 else { return };
    let Ok((transform, interp)) = positions.get(entity) else {
        return;
    };
    let pos = interp.map_or(transform.translation, |i| i.pos);
    let ring_center = pos + Vec3::Y * MARKER_GROUND_OFFSET;
    // Lay the ring flat on the ground plane: `circle`'s default isometry
    // draws in the XY plane (normal +Z), so rotate -90 deg around X to swing
    // its normal to +Y (up).
    let rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    gizmos.circle(
        Isometry3d::new(ring_center, rotation),
        MARKER_RADIUS,
        MARKER_COLOR,
    );
}

/// Installs the soft-target scan + its dim world marker.
pub struct TargetSelectionPlugin;

impl Plugin for TargetSelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetingConfig>();
        // `boss_nameplate::BossNameplateViewPlugin` also `init_resource`s
        // `SelectedTarget` — `init_resource` is idempotent (only inserts if
        // absent), so registration order between the two plugins doesn't
        // matter.
        app.init_resource::<SelectedTarget>();
        #[cfg(any(feature = "listen-server", feature = "net-client"))]
        {
            // `.in_set(MirrorSet)`: run after this frame's mirroring writes
            // `Transform`/`Interpolated` (the same set `entity_view.rs`'s
            // presentation systems occupy). The nameplate readers
            // (`sync_nameplate_visibility`/`sync_nameplate_content`) are plain
            // `Update` systems, NOT in `GameplaySet`, so this is a best-effort
            // same-frame write with at-most-one-frame latency if Bevy happens
            // to schedule a reader before this system — purely cosmetic (the
            // nameplate would lag a selection change by one frame at most), not
            // a correctness concern, so no explicit edge to the readers is
            // added.
            //
            // `.ambiguous_with(force_target_for_smoke_capture)`: both systems
            // take `ResMut<SelectedTarget>` with no ordering edge between them.
            // The overlap is intentional and correctness is already guaranteed
            // by the `XINDELER_SMOKE_FORCE_TARGET` env gate (each no-ops unless
            // the other's precondition is false — see this module's doc
            // comment), so declare the ambiguity expected to keep Bevy's
            // ambiguity checker quiet.
            app.add_systems(
                Update,
                update_soft_target
                    .after(crate::entity_view::interpolate_entities)
                    .ambiguous_with(crate::boss_nameplate::force_target_for_smoke_capture)
                    .in_set(xindeler_app::MirrorSet),
            );
            // `.after(update_soft_target)`: draw the ring from THIS frame's
            // selection, so the marker can't trail the target by a frame.
            app.add_systems(Update, draw_soft_target_marker.after(update_soft_target));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enemy(entity: Entity, pos: Vec3) -> TargetCandidate {
        TargetCandidate {
            entity,
            pos,
            alignment: NetAlignment::Enemy,
            alive: true,
        }
    }

    fn config() -> TargetingConfig {
        TargetingConfig {
            max_range: DEFAULT_MAX_RANGE,
            half_angle_rad: DEFAULT_HALF_ANGLE_DEG.to_radians(),
            alpha: DEFAULT_ALPHA,
            beta: DEFAULT_BETA,
            epsilon: DEFAULT_EPSILON,
        }
    }

    /// Two in-cone enemies at different distances: the nearer one scores
    /// higher (both have `cos(theta) == 1`, so only the `alpha/d` term
    /// differs) and is picked.
    #[test]
    fn picks_nearest_in_cone_enemy() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let near = Entity::from_raw_u32(1).unwrap();
        let far = Entity::from_raw_u32(2).unwrap();
        let candidates = [
            enemy(near, Vec3::new(0.0, 0.0, -5.0)),
            enemy(far, Vec3::new(0.0, 0.0, -15.0)),
        ];

        let picked = best_soft_target(player_pos, camera_forward, None, &cfg, candidates);

        assert_eq!(picked, Some(near));
    }

    /// A candidate directly behind the player (theta = 180 deg) is outside
    /// the acquisition cone and rejected — with no other candidate, the
    /// result is `None`.
    #[test]
    fn ignores_candidate_behind_camera() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let behind = Entity::from_raw_u32(1).unwrap();
        let candidates = [enemy(behind, Vec3::new(0.0, 0.0, 5.0))];

        let picked = best_soft_target(player_pos, camera_forward, None, &cfg, candidates);

        assert_eq!(picked, None);
    }

    /// A candidate beyond `max_range` (even directly ahead, theta = 0) is
    /// rejected.
    #[test]
    fn ignores_out_of_range_candidate() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let far_away = Entity::from_raw_u32(1).unwrap();
        let candidates = [enemy(far_away, Vec3::new(0.0, 0.0, -(cfg.max_range + 1.0)))];

        let picked = best_soft_target(player_pos, camera_forward, None, &cfg, candidates);

        assert_eq!(picked, None);
    }

    /// A `Wild`-aligned candidate (not `Enemy`) is never targetable in v1
    /// (spec §3.1/FD3), even directly ahead and in range.
    #[test]
    fn ignores_non_enemy_alignment() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let wild = Entity::from_raw_u32(1).unwrap();
        let candidates = [TargetCandidate {
            entity: wild,
            pos: Vec3::new(0.0, 0.0, -5.0),
            alignment: NetAlignment::Wild,
            alive: true,
        }];

        let picked = best_soft_target(player_pos, camera_forward, None, &cfg, candidates);

        assert_eq!(picked, None);
    }

    /// A dead (`alive: false`) candidate is never targetable, even if
    /// otherwise a perfect in-cone `Enemy` candidate.
    #[test]
    fn ignores_dead_candidate() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let dead = Entity::from_raw_u32(1).unwrap();
        let candidates = [TargetCandidate {
            entity: dead,
            pos: Vec3::new(0.0, 0.0, -5.0),
            alignment: NetAlignment::Enemy,
            alive: false,
        }];

        let picked = best_soft_target(player_pos, camera_forward, None, &cfg, candidates);

        assert_eq!(picked, None);
    }

    /// An empty candidate set (the caller already excludes `NetLocalPlayer`
    /// upstream, so "ignoring self" reduces to this) must not panic and must
    /// return `None`.
    #[test]
    fn empty_candidate_set_returns_none_without_panicking() {
        let cfg = config();
        let picked = best_soft_target(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            None,
            &cfg,
            Vec::new(),
        );

        assert_eq!(picked, None);
    }

    /// Hysteresis: a challenger scoring only marginally higher than the
    /// current target (within the `epsilon` relative margin) must NOT steal
    /// the target — the current target is kept.
    #[test]
    fn keeps_current_target_when_challenger_is_within_epsilon_margin() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let current = Entity::from_raw_u32(1).unwrap();
        let challenger = Entity::from_raw_u32(2).unwrap();
        // Both directly ahead (cos(theta) == 1); pick distances so the
        // challenger's score exceeds current's by LESS than epsilon * current.
        // current: d=10 -> alpha/d + beta = 0.1 + 3.0 = 3.1
        // challenger: d=9.9 -> alpha/d + beta ~= 0.10101 + 3.0 = 3.10101
        // delta ~= 0.00101, epsilon * current ~= 0.31 -> well within margin.
        let candidates = [
            enemy(current, Vec3::new(0.0, 0.0, -10.0)),
            enemy(challenger, Vec3::new(0.0, 0.0, -9.9)),
        ];

        let picked = best_soft_target(player_pos, camera_forward, Some(current), &cfg, candidates);

        assert_eq!(
            picked,
            Some(current),
            "a marginal (within-epsilon) challenger must not steal the current target"
        );
    }

    /// Hysteresis: a challenger scoring well beyond the `epsilon` margin DOES
    /// take over.
    #[test]
    fn switches_to_challenger_that_beats_epsilon_margin() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let current = Entity::from_raw_u32(1).unwrap();
        let challenger = Entity::from_raw_u32(2).unwrap();
        // current: d=20 -> alpha/d + beta = 0.05 + 3.0 = 3.05
        // challenger: d=2 -> alpha/d + beta = 0.5 + 3.0 = 3.5
        // delta = 0.45, epsilon * current = 0.305 -> challenger clears it.
        let candidates = [
            enemy(current, Vec3::new(0.0, 0.0, -20.0)),
            enemy(challenger, Vec3::new(0.0, 0.0, -2.0)),
        ];

        let picked = best_soft_target(player_pos, camera_forward, Some(current), &cfg, candidates);

        assert_eq!(
            picked,
            Some(challenger),
            "a challenger that clears the epsilon margin must take over"
        );
    }

    /// If the current target no longer survives the filter (e.g. it died),
    /// the raw winner is returned unconditionally — no hysteresis to apply.
    #[test]
    fn falls_through_to_best_when_current_no_longer_survives() {
        let cfg = config();
        let player_pos = Vec3::ZERO;
        let camera_forward = Vec3::new(0.0, 0.0, -1.0);
        let gone = Entity::from_raw_u32(1).unwrap();
        let alive_enemy = Entity::from_raw_u32(2).unwrap();
        // `gone` is not even in the candidate list this frame (e.g.
        // despawned) — only `alive_enemy` survives.
        let candidates = [enemy(alive_enemy, Vec3::new(0.0, 0.0, -5.0))];

        let picked = best_soft_target(player_pos, camera_forward, Some(gone), &cfg, candidates);

        assert_eq!(picked, Some(alive_enemy));
    }

    /// `TargetingConfig::default()` reads `XINDELER_TARGETING_*` env
    /// overrides — pin the unset-default values (mirrors
    /// `OcclusionCullingConfig`'s own env-default test convention) and one
    /// override, sequentially in one test to avoid cross-test env races.
    #[test]
    fn targeting_config_default_reads_env_overrides() {
        // SAFETY: this test is the sole reader/writer of these env vars in
        // this binary's test suite, and every set/assert/remove happens
        // sequentially within this one function.
        unsafe {
            std::env::remove_var("XINDELER_TARGETING_MAX_RANGE");
            std::env::remove_var("XINDELER_TARGETING_HALF_ANGLE_DEG");
            std::env::remove_var("XINDELER_TARGETING_ALPHA");
            std::env::remove_var("XINDELER_TARGETING_BETA");
            std::env::remove_var("XINDELER_TARGETING_EPSILON");
        }
        let cfg = TargetingConfig::default();
        assert_eq!(cfg.max_range, DEFAULT_MAX_RANGE);
        assert_eq!(cfg.half_angle_rad, DEFAULT_HALF_ANGLE_DEG.to_radians());
        assert_eq!(cfg.alpha, DEFAULT_ALPHA);
        assert_eq!(cfg.beta, DEFAULT_BETA);
        assert_eq!(cfg.epsilon, DEFAULT_EPSILON);

        // SAFETY: see above.
        unsafe {
            std::env::set_var("XINDELER_TARGETING_MAX_RANGE", "50");
        }
        let overridden = TargetingConfig::default();
        assert_eq!(overridden.max_range, 50.0);

        // SAFETY: see above; leave the environment clean.
        unsafe {
            std::env::remove_var("XINDELER_TARGETING_MAX_RANGE");
        }
    }
}
