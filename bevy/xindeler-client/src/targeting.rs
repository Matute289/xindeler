//! BL-82 EM-5.19 — the hybrid target-selection system's soft-target scan
//! (Phase 1) + hard-lock (Phase 2) (design spec
//! `docs/design/specs/2026-07-16-bl82-hybrid-target-selection-design.md`
//! §3.1-§3.4, plan `docs/design/plans/2026-07-16-bl82-hybrid-target-
//! selection-plan.md` Phases 1-2).
//!
//! ## What this unblocks
//! `boss_nameplate::SelectedTarget` ships fully render-wired (health/poise/
//! level/name bars all read from it already) but nothing outside the
//! `XINDELER_SMOKE_FORCE_TARGET` smoke override ever set it to `Some(_)` in
//! real gameplay — see that module's doc comment. This module is the real
//! source: every frame, [`update_soft_target`] scans a cone in front of the
//! camera for `Enemy`-aligned mirrored entities, scores the survivors, and
//! writes the winner into `SelectedTarget`.
//!
//! ## Phase 2 — hard-lock (this update)
//! Pressing `GameInput::Select` (`KeyX`) promotes the current soft-target to
//! a persistent [`HardLock`]: [`update_soft_target`] now yields to it
//! outright (no cone scan while locked), [`apply_hard_lock_facing`]
//! overrides `LocalPlayerInput.look` so the character turns to face it while
//! the camera stays free, and [`release_invalid_hard_lock`] auto-releases it
//! when the target dies, despawns, or drifts past `release_range`. See
//! [`TargetLockKind`] for the bright/dim signal `boss_nameplate.rs` and the
//! world marker read. Directional re-target on a second `Select` press while
//! locked (Phase 3) is out of scope — P2 is press-to-lock/press-to-clear.
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
    /// BL-82 EM-5.19 Phase 2 (§3.4): a hard lock auto-releases once its
    /// target drifts beyond this range — deliberately LARGER than
    /// `max_range` (default `max_range * 1.5`) so stepping back a pace
    /// doesn't instantly drop a lock the player just committed to. P1
    /// carried this field then dropped it as YAGNI (game-architecture review
    /// on PR #115: "belongs in P2 with HardLock/auto-release") — reintroduced
    /// here now that P2's `release_invalid_hard_lock` is the consumer.
    pub release_range: f32,
}

impl Default for TargetingConfig {
    fn default() -> Self {
        let max_range = env_f32("XINDELER_TARGETING_MAX_RANGE", DEFAULT_MAX_RANGE);
        let half_angle_deg = env_f32("XINDELER_TARGETING_HALF_ANGLE_DEG", DEFAULT_HALF_ANGLE_DEG);
        let alpha = env_f32("XINDELER_TARGETING_ALPHA", DEFAULT_ALPHA);
        let beta = env_f32("XINDELER_TARGETING_BETA", DEFAULT_BETA);
        let epsilon = env_f32("XINDELER_TARGETING_EPSILON", DEFAULT_EPSILON);
        // Default is derived from (possibly-overridden) `max_range`, matching
        // spec §3.1's "RELEASE_RANGE ~= MAX_RANGE * 1.5" — but still
        // independently overridable for A/B tuning.
        let release_range = env_f32("XINDELER_TARGETING_RELEASE_RANGE", max_range * 1.5);
        Self {
            max_range,
            half_angle_rad: half_angle_deg.to_radians(),
            alpha,
            beta,
            epsilon,
            release_range,
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

// ---------------------------------------------------------------------------
// BL-82 EM-5.19 Phase 2 — hard-lock (design spec §3.2/§3.4, plan Phase 2,
// task board T58.4-T58.6).
// ---------------------------------------------------------------------------

/// The current hard-lock target, if any (spec §3.2). `Some(_)` while the
/// player has promoted a soft-target via `GameInput::Select` and it hasn't
/// yet auto-released (see [`should_release_hard_lock`]). A plain client-only
/// resource — never networked, matching every other resource in this module.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct HardLock(pub Option<Entity>);

/// Whether [`crate::boss_nameplate::SelectedTarget`] currently reflects a
/// soft scan or a hard lock (spec §3.5) — the signal
/// `boss_nameplate::sync_nameplate_lock_style` and
/// [`draw_soft_target_marker`] read to render bright-vs-dim. Defaults to
/// `Soft` (P1's only style, before any lock exists).
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TargetLockKind {
    #[default]
    Soft,
    Hard,
}

/// A hard-locked target's liveness + distance from the player, as observed
/// THIS frame — the plain-data input to [`should_release_hard_lock`]. `None`
/// (at the call site, not this type) means the entity wasn't found in the
/// mirrored candidate set at all this frame; see that function's doc.
/// Deliberately not a Bevy query item, for the same App-less-testability
/// reason [`TargetCandidate`] already establishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HardLockTargetState {
    pub alive: bool,
    pub distance: f32,
}

/// Auto-release predicate (spec §3.4, T58.6): a hard lock releases this
/// frame when its target is no longer present in the mirrored candidate set
/// at all (`state` is `None` — despawned or walked out of replication
/// range), is no longer alive, or has drifted beyond `release_range` (even
/// while still alive) — release-then-fall-back-to-soft-scanning is the
/// caller's job, this function only decides whether to.
pub fn should_release_hard_lock(state: Option<HardLockTargetState>, release_range: f32) -> bool {
    match state {
        None => true,
        Some(s) => !s.alive || s.distance > release_range,
    }
}

/// Promote/clear on `GameInput::Select` just-pressed (spec §3.2, T58.4).
/// P2 keeps this deliberately simple per the plan: no lock → promote
/// `soft_target` (the value `SelectedTarget` currently holds, since it IS
/// the soft-scan result whenever unlocked); already locked → clear
/// unconditionally, ignoring `soft_target` (directional re-target on a
/// second press is Phase 3's directional cycle, out of scope here).
pub fn toggle_hard_lock(
    current_lock: Option<Entity>,
    soft_target: Option<Entity>,
) -> Option<Entity> {
    if current_lock.is_some() {
        None
    } else {
        soft_target
    }
}

/// Bevy y-up -> sim z-up direction: the SAME conversion
/// `crate::player_input::bevy_to_sim` applies for camera-forward — bevy
/// `(x, y, z)` -> sim `(x, -z, y)`. Deliberately duplicated rather than
/// called cross-module: `player_input` (and `LocalPlayerInput`'s only
/// producer, `gather_input`) is `#[cfg(feature = "listen-server")]`-only
/// (no embedded local player in `net-client`'s spectator-only v1). One line,
/// not worth a shared-helper crate for. `#[cfg(feature = "listen-server")]`
/// (narrower than this module's own `any(listen-server, net-client)` gate)
/// since [`facing_look_toward`] (its only caller) is itself
/// `listen-server`-only — see that fn's doc comment.
#[cfg(feature = "listen-server")]
fn bevy_to_sim(v: Vec3) -> Vec3 { Vec3::new(v.x, -v.z, v.y) }

/// Pure direction helper for the facing override (spec §3.2/FD2, T58.5): the
/// sim-axis unit direction from `player_pos` to `target_pos` (both Bevy-axis
/// world positions), via [`bevy_to_sim`] above. Returns `None` when the
/// positions coincide (an undefined direction) so the caller can leave
/// `LocalPlayerInput.look` untouched that frame rather than write a
/// degenerate zero/NaN vector.
///
/// `#[cfg(feature = "listen-server")]`: its only caller,
/// [`apply_hard_lock_facing`], is itself gated there (no `LocalPlayerInput`
/// producer under `net-client`) — see that system's doc comment.
#[cfg(feature = "listen-server")]
pub fn facing_look_toward(player_pos: Vec3, target_pos: Vec3) -> Option<Vec3> {
    let offset = target_pos - player_pos;
    if offset.length_squared() <= f32::EPSILON {
        return None;
    }
    Some(bevy_to_sim(offset.normalize()))
}

/// Clears an invalid hard lock BEFORE [`update_soft_target`] runs this same
/// frame (T58.6/§3.4), so a release falls back to soft-scanning immediately
/// rather than lagging a frame: missing/dead/out-of-`release_range`.
/// `.chain()`-ordered ahead of `update_soft_target` in
/// [`TargetSelectionPlugin`].
#[cfg(any(feature = "listen-server", feature = "net-client"))]
fn release_invalid_hard_lock(
    config: Res<TargetingConfig>,
    local_player: Query<(&Transform, Option<&Interpolated>), With<NetLocalPlayer>>,
    candidates: Query<(&Transform, Option<&Interpolated>, &NetHealth), With<NetUid>>,
    mut hard_lock: ResMut<HardLock>,
) {
    let Some(locked) = hard_lock.0 else { return };
    // No local player to measure distance from (e.g. spectator mode) — leave
    // the lock as-is rather than releasing on an unrelated degenerate case;
    // `apply_hard_lock_facing` below already no-ops without a local player.
    let Ok((player_transform, player_interp)) = local_player.single() else {
        return;
    };
    let player_pos = player_interp.map_or(player_transform.translation, |i| i.pos);

    let state = candidates
        .get(locked)
        .ok()
        .map(|(transform, interp, health)| {
            let pos = interp.map_or(transform.translation, |i| i.pos);
            HardLockTargetState {
                alive: health.current > 0.0,
                distance: (pos - player_pos).length(),
            }
        });

    if should_release_hard_lock(state, config.release_range) {
        hard_lock.0 = None;
    }
}

/// Every frame: gathers the local player + camera + candidate mirrored
/// entities, calls [`best_soft_target`], and writes the result into
/// `SelectedTarget`. See the module doc comment for the smoke-override
/// precedence this system implements by skipping its own write.
///
/// BL-82 EM-5.19 Phase 2 (T58.4, spec §3.2): now YIELDS to [`HardLock`] when
/// one is active — forces `SelectedTarget` to the locked entity and
/// [`TargetLockKind::Hard`] instead of running the cone scan at all. Ordered
/// `.after(release_invalid_hard_lock)` in the same frame, so a lock that
/// just released (dead/out-of-range/missing) falls through to the soft scan
/// immediately rather than one frame late.
#[cfg(any(feature = "listen-server", feature = "net-client"))]
pub fn update_soft_target(
    config: Res<TargetingConfig>,
    hard_lock: Res<HardLock>,
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
    mut lock_kind: ResMut<TargetLockKind>,
) {
    // Smoke-override precedence: see the module doc comment. Skip our own
    // write entirely so `force_target_for_smoke_capture` is the sole writer
    // while the env var is set, regardless of system ordering.
    if std::env::var("XINDELER_SMOKE_FORCE_TARGET").is_ok_and(|v| v != "0") {
        return;
    }

    if let Some(locked) = hard_lock.0 {
        // Hard-lock wins outright: no cone scan this frame at all.
        let next = Some(locked);
        if target.0 != next {
            target.0 = next;
        }
        if *lock_kind != TargetLockKind::Hard {
            *lock_kind = TargetLockKind::Hard;
        }
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
    if *lock_kind != TargetLockKind::Soft {
        *lock_kind = TargetLockKind::Soft;
    }
}

/// Consumes `GameInput::Select` just-pressed (T58.4, spec §3.2): promotes
/// the current soft-target (whatever `SelectedTarget` holds THIS frame,
/// since [`update_soft_target`] above already ran and — while unlocked — IS
/// the soft-scan result) to a [`HardLock`], or clears an existing one.
/// Ordered `.after(xindeler_input::InputResolveSet)` (the same edge every
/// other just-pressed-reading toggle in this crate uses, e.g.
/// `esc_menu::toggle_esc_menu`) and, via [`GameplaySet`] running after
/// [`MirrorSet`], always after `update_soft_target`'s write this same frame.
#[cfg(any(feature = "listen-server", feature = "net-client"))]
fn handle_hard_lock_input(
    action_state: Res<xindeler_input::ActionState>,
    selected_target: Res<SelectedTarget>,
    mut hard_lock: ResMut<HardLock>,
) {
    if !action_state.just_pressed(xindeler_input::GameInput::Select) {
        return;
    }
    hard_lock.0 = toggle_hard_lock(hard_lock.0, selected_target.0);
}

/// The facing override (T58.5, spec §3.2/FD2): while hard-locked, overrides
/// `LocalPlayerInput.look` to point from the player toward the target, so
/// the sim turns the character's `Ori` to face it
/// (`common::states::utils::update_orientation`, following the same
/// `should_follow_look()`/strafing branch `CharacterState::Talk` already
/// demonstrates for "turn toward a target entity" — see that function's
/// `:706-724`). The camera itself is untouched (mouse-look stays free) —
/// only the character's intended facing changes.
///
/// Gated on the cursor being grabbed — the same condition
/// [`crate::player_input::gather_input`] uses to gate ITS `move_dir`/`jump`
/// writes (that system's own `look` write is unconditional; only
/// movement/jump are grab-gated there). This client's `CursorControlPlugin`
/// already frees the cursor whenever any HUD window is open, so "cursor
/// grabbed" already implies "no menu is capturing input" — no separate menu
/// check is needed, rather than inventing a second gating convention.
/// Ordered `.after(handle_hard_lock_input)` (so a lock promoted
/// THIS frame also gets its facing override the same frame) and
/// `.after(gather_input)` (so this OVERRIDES gather_input's camera-forward
/// look, not the reverse) — both within [`GameplaySet`].
///
/// There is no explicit ordering edge to the bridge's `tick_player` (which
/// applies `LocalPlayerInput` to the sim): `tick_player` carries no
/// `SystemSet` membership today (see `xindeler-sim-bridge::player`'s
/// `PlayerBridgePlugin`), so its relative order vs. `GameplaySet` is already
/// unconstrained/best-effort — exactly the same one-frame-latency tolerance
/// `gather_input` itself already lives with. This system does not change
/// that tolerance, only adds a second writer ordered after the first.
///
/// `#[cfg(feature = "listen-server")]` ONLY (narrower than this module's own
/// `any(listen-server, net-client)` gate): `crate::player_input` — the only
/// producer of `LocalPlayerInput`/`gather_input` — is itself
/// `listen-server`-only (`net-client` has no embedded local player; see its
/// spectator-only-v1 note elsewhere in this file), so this system would have
/// nothing to read/write and no ordering target to reference under
/// `net-client` — registering it there would be dead weight, not a real gap.
#[cfg(feature = "listen-server")]
fn apply_hard_lock_facing(
    hard_lock: Res<HardLock>,
    cursor_options: Query<&bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
    local_player: Query<(&Transform, Option<&Interpolated>), With<NetLocalPlayer>>,
    positions: Query<(&Transform, Option<&Interpolated>)>,
    mut input: ResMut<xindeler_protocol::LocalPlayerInput>,
) {
    let Some(target_entity) = hard_lock.0 else {
        return;
    };
    let grabbed = cursor_options
        .single()
        .is_ok_and(|c| c.grab_mode != bevy::window::CursorGrabMode::None);
    if !grabbed {
        return;
    }
    let Ok((player_transform, player_interp)) = local_player.single() else {
        return;
    };
    let Ok((target_transform, target_interp)) = positions.get(target_entity) else {
        return;
    };
    let player_pos = player_interp.map_or(player_transform.translation, |i| i.pos);
    let target_pos = target_interp.map_or(target_transform.translation, |i| i.pos);

    if let Some(look) = facing_look_toward(player_pos, target_pos) {
        input.look = look;
    }
}

/// Ground offset (Bevy Y) the ring is drawn at — just above the target's
/// feet so it doesn't z-fight with flat terrain.
const MARKER_GROUND_OFFSET: f32 = 0.05;
/// Ring radius (metres).
const MARKER_RADIUS: f32 = 0.8;
/// Dim (soft-target) ring color — a translucent white/grey.
const MARKER_COLOR_SOFT: Color = Color::srgba(1.0, 1.0, 1.0, 0.35);
/// Bright (hard-lock) ring color — BL-82 EM-5.19 Phase 2 (spec §3.5): opaque
/// gold so a lock reads unmistakably different from the dim soft marker.
const MARKER_COLOR_HARD: Color = Color::srgba(1.0, 0.85, 0.2, 0.9);

/// The world-space marker (spec §3.5): a flat ring gizmo drawn at the
/// current target's feet every frame it's selected — dim for a soft target,
/// bright for a hard lock ([`TargetLockKind`], BL-82 EM-5.19 Phase 2).
/// Deliberately the simplest option available in this codebase (confirmed by
/// grep: no existing billboard/quad-mesh precedent anywhere in
/// `bevy/xindeler-client`) — `Gizmos` needs no spawned entity, no new asset,
/// and no despawn/lifecycle bookkeeping; it simply isn't redrawn once
/// `SelectedTarget` clears. Reusing an existing `HudImageKey` was considered
/// and rejected: none of the 59 existing keys is a ring/reticle shape, so
/// that path would be a visual misuse, not a real reuse.
#[cfg(any(feature = "listen-server", feature = "net-client"))]
fn draw_soft_target_marker(
    target: Res<SelectedTarget>,
    lock_kind: Res<TargetLockKind>,
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
    let color = match *lock_kind {
        TargetLockKind::Soft => MARKER_COLOR_SOFT,
        TargetLockKind::Hard => MARKER_COLOR_HARD,
    };
    gizmos.circle(Isometry3d::new(ring_center, rotation), MARKER_RADIUS, color);
}

/// Installs the soft-target scan, the hard-lock promote/facing/auto-release
/// machinery (BL-82 EM-5.19 Phase 2), and the world marker.
pub struct TargetSelectionPlugin;

impl Plugin for TargetSelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TargetingConfig>();
        // `boss_nameplate::BossNameplateViewPlugin` also `init_resource`s
        // `SelectedTarget` — `init_resource` is idempotent (only inserts if
        // absent), so registration order between the two plugins doesn't
        // matter.
        app.init_resource::<SelectedTarget>();
        app.init_resource::<HardLock>();
        app.init_resource::<TargetLockKind>();
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
            // `release_invalid_hard_lock` runs FIRST so a release this frame
            // (missing/dead/out-of-range) falls through to
            // `update_soft_target`'s cone scan the SAME frame, not one frame
            // late. `.ambiguous_with(force_target_for_smoke_capture)`:
            // `update_soft_target` and that system both take
            // `ResMut<SelectedTarget>` with no ordering edge between them —
            // the overlap is intentional and correctness is already
            // guaranteed by the `XINDELER_SMOKE_FORCE_TARGET` env gate (each
            // no-ops unless the other's precondition is false — see this
            // module's doc comment), so declare the ambiguity expected to
            // keep Bevy's ambiguity checker quiet.
            app.add_systems(
                Update,
                (release_invalid_hard_lock, update_soft_target)
                    .chain()
                    .after(crate::entity_view::interpolate_entities)
                    .ambiguous_with(crate::boss_nameplate::force_target_for_smoke_capture)
                    .in_set(xindeler_app::MirrorSet),
            );
            // `.after(update_soft_target)`: draw the ring from THIS frame's
            // selection, so the marker can't trail the target by a frame.
            app.add_systems(Update, draw_soft_target_marker.after(update_soft_target));
            // BL-82 EM-5.19 Phase 2: `Select`-driven promote/clear —
            // `GameplaySet`, which `xindeler_app::sets::configure` already
            // chains AFTER `MirrorSet` above, so it always sees this frame's
            // `update_soft_target` write.
            app.add_systems(
                Update,
                handle_hard_lock_input
                    .after(xindeler_input::InputResolveSet)
                    .in_set(xindeler_app::GameplaySet),
            );
        }
        // BL-82 EM-5.19 Phase 2: the facing override, `listen-server`-only
        // (see `apply_hard_lock_facing`'s own doc comment for why —
        // `player_input`/`LocalPlayerInput` don't exist under `net-client`).
        // Also `GameplaySet`, so it too always runs after this frame's
        // `MirrorSet`/`handle_hard_lock_input` writes.
        #[cfg(feature = "listen-server")]
        {
            app.add_systems(
                Update,
                apply_hard_lock_facing
                    .after(handle_hard_lock_input)
                    .after(crate::player_input::gather_input)
                    .in_set(xindeler_app::GameplaySet),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

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
            release_range: DEFAULT_MAX_RANGE * 1.5,
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
    /// `OcclusionCullingConfig`'s own env-default test convention) and each
    /// override, ALL sequentially in ONE test to avoid cross-test env races.
    ///
    /// BL-82 EM-5.19 Phase 2 (bevy-migration-reviewer finding): the
    /// `release_range` assertions used to live in their own separate test
    /// function, which — despite an identical "sole reader/writer" `SAFETY`
    /// comment — was NOT actually the sole mutator of
    /// `XINDELER_TARGETING_MAX_RANGE`: this test also sets/clears that same
    /// var, and Rust's default multi-threaded test runner can interleave the
    /// two, racing the shared process-global env. Folded into this single
    /// test (which already owns exclusive sequential access to every
    /// `XINDELER_TARGETING_*` var for its whole body) rather than adding a
    /// second env-mutating test that could race it again.
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
            std::env::remove_var("XINDELER_TARGETING_RELEASE_RANGE");
        }
        let cfg = TargetingConfig::default();
        assert_eq!(cfg.max_range, DEFAULT_MAX_RANGE);
        assert_eq!(cfg.half_angle_rad, DEFAULT_HALF_ANGLE_DEG.to_radians());
        assert_eq!(cfg.alpha, DEFAULT_ALPHA);
        assert_eq!(cfg.beta, DEFAULT_BETA);
        assert_eq!(cfg.epsilon, DEFAULT_EPSILON);
        // Default `release_range` derives from `max_range * 1.5` (spec
        // §3.4's "RELEASE_RANGE ~= MAX_RANGE * 1.5") when unset.
        assert_eq!(cfg.release_range, DEFAULT_MAX_RANGE * 1.5);

        // SAFETY: see above.
        unsafe {
            std::env::set_var("XINDELER_TARGETING_MAX_RANGE", "50");
        }
        let overridden = TargetingConfig::default();
        assert_eq!(overridden.max_range, 50.0);
        // Still independently overridable even with `max_range` also
        // overridden above.
        assert_eq!(overridden.release_range, 75.0);

        // SAFETY: see above.
        unsafe {
            std::env::set_var("XINDELER_TARGETING_RELEASE_RANGE", "99");
        }
        let release_overridden = TargetingConfig::default();
        assert_eq!(release_overridden.release_range, 99.0);

        // SAFETY: see above; leave the environment clean.
        unsafe {
            std::env::remove_var("XINDELER_TARGETING_MAX_RANGE");
            std::env::remove_var("XINDELER_TARGETING_RELEASE_RANGE");
        }
    }

    /// [`toggle_hard_lock`] (T58.4): with no current lock, `Select` promotes
    /// the current soft-target to a hard lock.
    #[test]
    fn toggle_hard_lock_promotes_soft_target_when_unlocked() {
        let soft = Entity::from_raw_u32(1).unwrap();
        assert_eq!(toggle_hard_lock(None, Some(soft)), Some(soft));
    }

    /// With no current lock AND no soft target, there's nothing to promote —
    /// stays `None`.
    #[test]
    fn toggle_hard_lock_stays_none_with_no_soft_target() {
        assert_eq!(toggle_hard_lock(None, None), None);
    }

    /// With an existing lock, `Select` clears it — press-to-lock,
    /// press-again-to-clear (P2 scope; directional cycling is Phase 3).
    #[test]
    fn toggle_hard_lock_clears_existing_lock() {
        let locked = Entity::from_raw_u32(1).unwrap();
        let soft = Entity::from_raw_u32(2).unwrap();
        // Whatever the current soft target is, an existing lock always
        // clears rather than re-promoting/switching (P2 keeps it simple).
        assert_eq!(toggle_hard_lock(Some(locked), Some(soft)), None);
    }

    /// [`should_release_hard_lock`] (T58.6/§3.4): a missing target (no
    /// longer in the mirrored candidate set at all — despawned or left
    /// replication range) always releases.
    #[test]
    fn should_release_hard_lock_when_target_missing() {
        assert!(should_release_hard_lock(None, DEFAULT_MAX_RANGE * 1.5));
    }

    /// A dead target releases even if still technically in range.
    #[test]
    fn should_release_hard_lock_when_target_dead() {
        let state = HardLockTargetState {
            alive: false,
            distance: 1.0,
        };
        assert!(should_release_hard_lock(Some(state), 25.0));
    }

    /// A target beyond `release_range` releases even if still alive.
    #[test]
    fn should_release_hard_lock_when_target_out_of_range() {
        let state = HardLockTargetState {
            alive: true,
            distance: 30.0,
        };
        assert!(should_release_hard_lock(Some(state), 25.0));
    }

    /// An alive, in-range target does NOT release.
    #[test]
    fn should_not_release_hard_lock_when_target_alive_and_in_range() {
        let state = HardLockTargetState {
            alive: true,
            distance: 10.0,
        };
        assert!(!should_release_hard_lock(Some(state), 25.0));
    }

    /// [`facing_look_toward`] (T58.5): a target straight ahead (bevy -Z, the
    /// same "forward" convention `player_input`'s camera-forward tests use)
    /// converts to the sim-axis look the existing `bevy_to_sim` helper would
    /// produce for that same bevy direction — proving this helper reuses
    /// that exact conversion rather than inventing a new one.
    /// `#[cfg(feature = "listen-server")]`: the fn under test only exists
    /// under that feature — see its own doc comment.
    #[cfg(feature = "listen-server")]
    #[test]
    fn facing_look_toward_points_at_target_in_sim_axes() {
        let player_pos = Vec3::ZERO;
        let target_pos = Vec3::new(0.0, 0.0, -5.0);

        let look = facing_look_toward(player_pos, target_pos).expect("non-degenerate direction");

        // bevy (0,0,-1) -> sim (x, -z, y) = (0, 1, 0).
        assert!((look - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
        // Direction is normalized.
        assert!((look.length() - 1.0).abs() < 1e-5);
    }

    /// A target at the SAME position as the player has no defined direction
    /// — must return `None` rather than a NaN/zero vector, so the caller can
    /// leave `LocalPlayerInput.look` untouched that frame.
    #[cfg(feature = "listen-server")]
    #[test]
    fn facing_look_toward_returns_none_when_coincident() {
        assert_eq!(facing_look_toward(Vec3::ZERO, Vec3::ZERO), None);
    }

    /// Minimal `App` for the system-level (not just pure-fn) hard-lock tests
    /// below — `MinimalPlugins` + `TransformPlugin` is the same combination
    /// `boss_nameplate.rs`'s own `new_app()` helper uses for its
    /// `run_system_once` tests.
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.init_resource::<TargetingConfig>();
        app.init_resource::<SelectedTarget>();
        app.init_resource::<HardLock>();
        app.init_resource::<TargetLockKind>();
        app
    }

    /// [`update_soft_target`]'s hard-lock short-circuit (T58.4): with a
    /// `HardLock` active, `SelectedTarget` is forced to it and
    /// `TargetLockKind` flips to `Hard` — WITHOUT running the cone scan at
    /// all (no camera/candidates are even spawned here, proving the branch
    /// never touches those queries).
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    #[test]
    fn update_soft_target_yields_to_hard_lock_without_scanning() {
        let mut app = new_app();
        let locked = app.world_mut().spawn_empty().id();
        app.insert_resource(HardLock(Some(locked)));

        app.world_mut()
            .run_system_once(update_soft_target)
            .expect("system runs");

        assert_eq!(app.world().resource::<SelectedTarget>().0, Some(locked));
        assert_eq!(
            *app.world().resource::<TargetLockKind>(),
            TargetLockKind::Hard
        );
    }

    /// [`release_invalid_hard_lock`] (T58.6): a locked entity that no longer
    /// exists in the mirrored candidate set at all (despawned/out-of-range
    /// replication) clears the lock.
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    #[test]
    fn release_invalid_hard_lock_clears_on_missing_target() {
        let mut app = new_app();
        app.world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), NetLocalPlayer));
        // The locked entity is never spawned with `NetUid` at all — it's
        // absent from the candidate query `release_invalid_hard_lock` uses.
        let gone = app.world_mut().spawn_empty().id();
        app.insert_resource(HardLock(Some(gone)));

        app.world_mut()
            .run_system_once(release_invalid_hard_lock)
            .expect("system runs");

        assert_eq!(app.world().resource::<HardLock>().0, None);
    }

    /// An alive, in-range locked target is left alone.
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    #[test]
    fn release_invalid_hard_lock_keeps_alive_in_range_target() {
        let mut app = new_app();
        app.world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), NetLocalPlayer));
        let locked = app
            .world_mut()
            .spawn((Transform::from_xyz(1.0, 0.0, 0.0), NetUid(7), NetHealth {
                current: 10.0,
                max: 10.0,
            }))
            .id();
        app.insert_resource(HardLock(Some(locked)));

        app.world_mut()
            .run_system_once(release_invalid_hard_lock)
            .expect("system runs");

        assert_eq!(app.world().resource::<HardLock>().0, Some(locked));
    }

    /// [`handle_hard_lock_input`] (T58.4): pressing `GameInput::Select`
    /// (`KeyX`) drives `ActionState` through the REAL resolver
    /// (`xindeler_input::action_state::update_action_state`) the same way
    /// `esc_menu::tests::escape_opens_and_closes_only_when_appropriate`
    /// already does (one fresh `App` per single-press scenario, rather than
    /// hand-constructing `ActionState` or simulating multi-frame
    /// just-pressed clearing, which `MinimalPlugins` doesn't drive for us).
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    fn press_select_and_run(
        initial_lock: Option<Entity>,
        soft_target: Option<Entity>,
    ) -> Option<Entity> {
        use xindeler_input::{KeyMap, KeyOrMouse};

        let mut app = new_app();
        app.insert_resource(KeyMap::default());
        app.init_resource::<xindeler_input::ActionState>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(ButtonInput::<bevy::input::mouse::MouseButton>::default());
        app.add_systems(
            Update,
            (
                xindeler_input::action_state::update_action_state,
                handle_hard_lock_input,
            )
                .chain(),
        );
        app.insert_resource(HardLock(initial_lock));
        app.insert_resource(SelectedTarget(soft_target));

        let select_key = app
            .world()
            .resource::<KeyMap>()
            .keyboard
            .get_binding(xindeler_input::GameInput::Select);
        let Some(KeyOrMouse::Key(key)) = select_key else {
            panic!("GameInput::Select has no keyboard binding");
        };
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.update();

        app.world().resource::<HardLock>().0
    }

    /// No lock + a soft target → `Select` promotes it. The `Entity` values
    /// here are bare handles (never spawned into `press_select_and_run`'s
    /// own `App`) — `handle_hard_lock_input` only ever compares/copies
    /// `Option<Entity>`, it never queries the entity itself, so a live
    /// entity isn't needed (same reasoning the pure `toggle_hard_lock` tests
    /// above already rely on with `Entity::from_raw_u32`).
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    #[test]
    fn handle_hard_lock_input_promotes_soft_target_when_unlocked() {
        let soft = Entity::from_raw_u32(1).unwrap();

        assert_eq!(press_select_and_run(None, Some(soft)), Some(soft));
    }

    /// Already locked → `Select` clears it (press-to-lock,
    /// press-again-to-clear — P2 scope, no directional cycling).
    #[cfg(any(feature = "listen-server", feature = "net-client"))]
    #[test]
    fn handle_hard_lock_input_clears_existing_lock() {
        let locked = Entity::from_raw_u32(1).unwrap();

        assert_eq!(press_select_and_run(Some(locked), Some(locked)), None);
    }
}
