//! EM-3.7 — client-side presentation of the mirrored sim entities
//! (listen-server mode only).
//!
//! The server-side [`xindeler_sim_bridge::SimEntityMirrorPlugin`] replicates
//! one entity per client-visible sim entity carrying [`NetPos`]/[`NetOri`]/
//! [`NetVel`]/[`NetBody`](+[`NetHealth`]). This module turns each replicated
//! entity into something VISIBLE and SMOOTH:
//!
//! 1. On `Added<NetBody>` we attach a PRESENTATION bundle — a placeholder mesh
//!    (a capsule) with a [`StandardMaterial`] coloured by the body-class id, a
//!    [`Transform`], and an [`Interpolated`] buffer seeded to the first sample.
//!    No `.vox` model yet — the real figure pipeline is EM-3.8.
//! 2. Every frame [`interpolate_entities`] eases each entity's `Transform`
//!    toward its latest `NetPos`/`NetOri` (dead-reckoned by `NetVel`) instead
//!    of snapping, so low-rate/ jittery net samples render as smooth motion.
//!    The constants are ported verbatim from voxygen's interpolation system
//!    (`voxygen/src/ecs/sys/interpolation.rs`): exponential lerp at rate 10/s
//!    toward `pos + vel·0.03`, slerp orientation at rate 10/s, and a hard SNAP
//!    when the target jumps more than 64 m (teleports / first frame) so we
//!    never "drift" across the whole map. **EM-3.11g:** a SNAP also clears
//!    [`bevy::pbr::PreviousGlobalTransform`] on the entity + every figure-part
//!    child, so Bevy reports zero motion for that one teleport frame instead of
//!    the true, extreme jump — see [`interpolate_entities`]'s doc comment for
//!    the TAA-ghosting bug this fixes ("detached ghost hand" report).
//!    **EM-3.11r:** the local player's OWN mirrored entity (tagged
//!    [`NetLocalPlayer`]) eases at a much higher, distinct rate
//!    ([`LOCAL_PLAYER_POS_LERP_RATE`]) than every remote entity's
//!    [`POS_LERP_RATE`] — its `NetPos` comes from this same process's own
//!    embedded sim tick (no real network jitter to hide), and the shared 10/s
//!    rate was measurably lagging it up to 0.80 m behind its own already-
//!    landed height after a jump, reading as "still airborne" (see that
//!    constant's doc comment for the full jump/landing investigation).
//!
//! ## Purity
//! This module is 100% Bevy + `xindeler-protocol` — NO `specs`, no server
//! crate. The engine-isolation guard greps this crate's `src` for `specs`; it
//! stays clean. Compiled only under the `listen-server` feature.

use bevy::{pbr::PreviousGlobalTransform, prelude::*};
use xindeler_protocol::{NetBody, NetLocalPlayer, NetOri, NetPos, NetVel};

/// Per-frame exponential-lerp rate for position (voxygen
/// `POS_LERP_RATE_FACTOR`). `lerp(cur, target, RATE·dt)` each frame → a smooth
/// critically-ish-damped ease that reaches the target quickly at 60+ fps but
/// tolerates sparse net updates. Applies to every REMOTE mirrored entity
/// (NPCs, other players once real multiplayer lands) — tuned to hide real
/// network jitter/packet loss, same as voxygen's original.
const POS_LERP_RATE: f32 = 10.0;
/// Orientation slerp rate (voxygen `base_ori_interp` default, non-object).
const ORI_LERP_RATE: f32 = 10.0;
/// Position lerp rate for the LOCAL player's own mirrored entity specifically
/// (BL-82 EM-3.11r fix). In the `--listen-server` architecture the local
/// player's `NetPos` is NOT a real network sample — it comes from THIS SAME
/// process's own embedded sim tick over loopback, so there is no real jitter
/// or packet loss to smooth away, only the 30 Hz `FixedUpdate` step size vs.
/// a much higher `Update` render rate.
///
/// This was verified with a real, velocity-gated A/B, not just a plausible
/// story: `XINDELER_SMOKE_JUMP_SPAM=1` (a scripted, deterministic repeated
/// jump-press/release driver) + `XINDELER_LANDING_PERF_LOG=1`
/// (`log_local_player_landing_gap`, which tracks the MINIMUM `|NetVel.y|`
/// seen during each render/sim vertical-gap episode — this is what
/// distinguishes a REAL "still floating after actually landing" defect from
/// the harmless, intentional `VEL_LEAD` look-ahead that's expected while
/// still genuinely airborne). At the old shared rate (10/s, reusing
/// `POS_LERP_RATE`), 76 of 121 logged episodes in a 45s run had
/// `min_abs_vel_y == 0.0` — i.e. the sim's own velocity had ALREADY settled
/// to exactly zero (genuinely landed, not moving) while the render still sat
/// up to 0.80 m above the true height for as long as 510 ms. That is a real,
/// reproducible "the character hasn't touched down / jumped from mid-air"
/// artifact, matching Matías's report exactly — and it is NOT a sim-side
/// grounded-state bug: the jump gate itself never once allowed a jump to
/// fire while `on_ground: None` (verified: 0 such cases in ~800+ logged
/// jump-gate checks across the same class of run —
/// `common/src/states/utils.rs::handle_jump`'s `XINDELER_JUMP_PERF_LOG`
/// diagnostic). At this rate (60/s), the SAME test (91 episodes across a
/// 45s run) produced ZERO episodes with `min_abs_vel_y == 0.0` — every
/// remaining gap coincided with the sim's own velocity still being ≥ 5 m/s
/// (i.e. genuinely still airborne, the expected/harmless `VEL_LEAD` case) —
/// confirming the fix actually closes the defect rather than just making it
/// harder to observe.
///
/// ## A real trade-off, checked rather than assumed away
/// `step_pos`'s `t = (rate·dt).min(1.0)` SATURATES to a full per-frame snap
/// (no easing at all that frame) whenever `dt ≥ 1/rate` — i.e. at render
/// rates ≤ 60 fps with this constant, which `bevy-migration-reviewer`
/// correctly flagged as potentially reintroducing visible per-tick
/// quantization during ORDINARY (non-jumping) walking, not just fixing the
/// landing case. Checked, not dismissed: a gentler `20/s` was tried with the
/// SAME velocity-gated repro and still left 15 of 96 episodes with
/// `min_abs_vel_y == 0.0` (one 430 ms / 0.30 m) — i.e. a materially gentler
/// rate does NOT close the reported bug, so there is no "free" middle ground
/// here. Also measured (temporarily, `XINDELER_TEMP_STEP_LOG`, since
/// reverted) the local player's per-frame step size during a plain 20s
/// straight walk at 60/s: median non-zero step ≈0.27 m recurring roughly
/// every 3rd render frame (consistent with an effective 8-9 m/s walk speed
/// sampled at 30 Hz), ~31% of frames showing zero delta in between. That
/// confirms the reviewer's math: motion IS quantized to sim-tick granularity
/// rather than continuously eased across the render's higher frame rate.
///
/// This is a genuine, disclosed trade-off, not a clear bug: prioritizing
/// zero added lag (fixing the confirmed "floating after landing" report)
/// over inter-tick smoothness for the ONE entity that's a zero-real-jitter,
/// same-process source — closer in spirit to the old client's own frame-rate
/// local prediction (which also only ever shows the CURRENT true position,
/// see the trade-off note below) than to the jitter-hiding smoothing this
/// same mechanism correctly provides for remote entities. Whether the
/// resulting per-tick quantization reads as noticeably worse in a real human
/// play session (vs. the previous, smoother-but-laggy motion) is a
/// perceptual question an automated repro cannot fully settle — flagged for
/// Matías's own eyeball on ordinary walking in his next session, same as
/// several prior EM-3.11 rounds' visual fixes.
///
/// This narrows the specific "renders above an already-landed position"
/// artifact but does not eliminate 30 Hz sampling lag in general — the
/// architecturally complete fix (per a parallel `xindeler-old` comparison,
/// `docs/design/specs/2026-07-11-xindeler-old-comparison-research.md`) is to
/// predict the local player at frame rate (as the legacy voxygen client
/// does) instead of interpolating a fixed-rate mirror of it; that would also
/// remove the quantization trade-off above, not just the lag. That is a
/// bigger architectural change tracked as a follow-up, not done here.
const LOCAL_PLAYER_POS_LERP_RATE: f32 = 60.0;
/// Dead-reckoning lead: aim the lerp at `pos + vel·VEL_LEAD` so a steadily
/// moving entity doesn't render one net-sample behind its own motion (voxygen
/// uses the same `+ vel.0 * 0.03`).
const VEL_LEAD: f32 = 0.03;
/// Beyond this jump (metres) we SNAP instead of interpolating — teleports and
/// the very first sample (voxygen's `64.0 * 64.0` distance-squared gate).
const SNAP_DISTANCE: f32 = 64.0;

/// The smoothed presentation transform for a mirrored entity. Distinct from the
/// raw replicated `NetPos`/`NetOri` (server truth): the mesh's `Transform`
/// reads from HERE so it eases rather than teleporting between net samples.
#[derive(Component, Clone, Copy, Debug)]
pub struct Interpolated {
    pub pos: Vec3,
    pub ori: Quat,
}

/// Installs the presentation + interpolation systems for mirrored entities.
pub struct EntityViewPlugin;

impl Plugin for EntityViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                add_presentation,
                interpolate_entities,
                log_local_player_landing_gap,
            )
                .chain(),
        );
    }
}

/// A distinct, readable colour per body class. Placeholder palette for the
/// capsule fallback — EM-3.8 replaces supported bodies with the real `.vox`
/// model (`figure_view`), but unsupported bodies (humanoid, etc.) keep this
/// capsule until EM-3.8b. Reads the replicated full `Body` (EM-3.8 enrichment).
fn body_class_color(body: &common::comp::Body) -> Color {
    use common::comp::Body::*;
    let (r, g, b) = match body {
        Humanoid(_) => (0.90, 0.30, 0.30),                  // red
        QuadrupedSmall(_) => (0.40, 0.80, 0.40),            // green
        QuadrupedMedium(_) => (0.35, 0.55, 0.95),           // blue
        BirdMedium(_) | BirdLarge(_) => (0.95, 0.80, 0.30), // yellow
        FishMedium(_) | FishSmall(_) => (0.75, 0.45, 0.90), // purple
        Dragon(_) => (0.95, 0.60, 0.25),                    // orange
        _ => (0.80, 0.80, 0.80),                            // grey
    };
    Color::srgb(r, g, b)
}

/// A rough capsule size (radius, half-height) per body so a humanoid isn't the
/// same blob as a dragon. Placeholder scale only — supported bodies get their
/// real model (EM-3.8); this is the unsupported-body fallback.
fn body_class_capsule(body: &common::comp::Body) -> (f32, f32) {
    use common::comp::Body::*;
    match body {
        Humanoid(_) => (0.4, 0.9),
        QuadrupedSmall(_) => (0.4, 0.4),
        QuadrupedMedium(_) => (0.7, 0.7),
        Dragon(_) => (1.5, 2.0),
        BipedLarge(_) => (0.8, 1.4),
        Golem(_) => (1.2, 1.6),
        _ => (0.5, 0.7),
    }
}

/// On first sight of a mirrored entity (it just gained `NetBody`), attach the
/// placeholder mesh + material + a `Transform` + the `Interpolated` buffer
/// seeded to the current net sample (so it appears at the right place, not at
/// the origin, and the first interpolation step is a no-op).
fn add_presentation(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    // `Added<NetBody>` fires once, the frame the replicated entity's NetBody
    // first arrives. NetPos is required-present alongside (the bridge always
    // spawns both), but be defensive and default it.
    query: Query<(Entity, &NetBody, Option<&NetPos>, Option<&NetOri>), Added<NetBody>>,
) {
    for (entity, body, pos, ori) in &query {
        // BL-82 EM-4.2b: a plain, always-on log line for every NEWLY mirrored
        // entity — used by the EM-4.2b acceptance test
        // (`bevy/xindeler-server-app/tests/replicon_quinnet_dual_stack.rs`)
        // to confirm at least one replicated entity crossed the NEW
        // replicon+quinnet transport, by grepping the net-client process's
        // stdout. Low-frequency (fires once per entity, on spawn) and
        // harmless under every other mode.
        info!(?entity, body = ?body.0, "presentation attached to a newly mirrored entity");
        let (radius, half_length) = body_class_capsule(&body.0);
        let mesh = meshes.add(Capsule3d::new(radius, half_length * 2.0));
        let material = materials.add(StandardMaterial {
            base_color: body_class_color(&body.0),
            perceptual_roughness: 0.8,
            ..default()
        });
        let start_pos = pos.map_or(Vec3::ZERO, |p| p.0);
        let start_ori = ori.map_or(Quat::IDENTITY, |o| o.0);
        commands.entity(entity).insert((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(start_pos).with_rotation(start_ori),
            Visibility::default(),
            Interpolated {
                pos: start_pos,
                ori: start_ori,
            },
        ));
    }
}

/// Eases each mirrored entity's presentation `Transform` toward its latest net
/// sample, dead-reckoned by velocity, and SNAPS on large jumps. Ported from
/// voxygen's interpolation system (see module docs).
///
/// ## EM-3.11g: suppressing the motion-vector ghost on a SNAP
/// A SNAP is a legitimate, deliberate one-frame teleport of the WHOLE
/// presentation hierarchy (this entity's `Transform` plus every figure-part
/// child `figure_view` parents under it — head/limbs/weapon/…, EM-3.8b). Bevy
/// computes each mesh's TAA motion vector from `GlobalTransform` vs the
/// previous frame's `PreviousGlobalTransform` (`bevy_pbr::prepass`); a snap
/// reports that whole-hierarchy jump as a real, extreme one-frame motion
/// vector, which is exactly the kind of discontinuity that makes a TAA
/// reprojection accept a bogus history sample instead of rejecting it — a
/// SMALL, high-contrast part (a skin/glove-coloured hand against a
/// differently-coloured torso/background) is far more likely to visibly
/// "ghost" this way than the larger, more uniform torso, which reads as a hand
/// detaching from the body and sitting frozen at its pre-snap ground position
/// for the ~1 s the TAA history takes to wash the bad sample out (Matías's
/// 2026-07-04 report; `docs/backlog/engine-migration.md` EM-3.11f logged an
/// earlier, unreproduced sighting of the same thing as a "ghost/teleport"
/// blip). A render-frame interpolation buffer only advances a few centimetres
/// per frame (`POS_LERP_RATE`), so crossing `SNAP_DISTANCE` needs it to fall
/// badly behind the sim's authoritative `NetPos` — exactly what the
/// documented, still-open frame-hitch/stutter issue (EM-3.11c/d) can cause.
///
/// The fix does not touch WHY a snap fires (that is EM-3.11c/d's job); it
/// suppresses the motion-vector artifact a snap causes REGARDLESS of cause,
/// the same way Bevy itself treats a brand-new mesh: `update_mesh_previous_
/// global_transforms` (`bevy_pbr::prepass`) only seeds
/// `PreviousGlobalTransform` for entities that lack it, and the GPU-instance
/// builder falls back to `world_from_local` (i.e. "no motion this frame")
/// whenever it's absent. So on a snap we REMOVE `PreviousGlobalTransform` from
/// this entity and every descendant (the figure parts, if already assembled):
/// the snap frame renders with a suppressed (zero) motion vector instead of the
/// true, extreme one, and Bevy re-seeds it correctly the very next frame —
/// normal small per-frame motion vectors resume immediately after.
fn interpolate_entities(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &NetPos,
        &NetOri,
        Option<&NetVel>,
        Option<&NetLocalPlayer>,
        &mut Interpolated,
        &mut Transform,
    )>,
    children_query: Query<&Children>,
) {
    let dt = time.delta_secs();
    for (entity, pos, ori, vel, local_player, mut interp, mut transform) in &mut query {
        let target = pos.0;
        let far = interp.pos.distance_squared(target) >= SNAP_DISTANCE * SNAP_DISTANCE;
        // BL-82 EM-3.11r: the local player's own `NetPos` has no real network
        // jitter to smooth (see `LOCAL_PLAYER_POS_LERP_RATE`'s doc comment),
        // so it eases at a much higher rate than remote mirrored entities.
        let pos_lerp_rate = if local_player.is_some() {
            LOCAL_PLAYER_POS_LERP_RATE
        } else {
            POS_LERP_RATE
        };
        interp.pos = step_pos(
            interp.pos,
            target,
            vel.map_or(Vec3::ZERO, |v| v.0),
            dt,
            pos_lerp_rate,
        );
        interp.ori = if far {
            // Teleport / first real sample: snap orientation too.
            ori.0
        } else {
            interp.ori.slerp(ori.0, (ORI_LERP_RATE * dt).min(1.0))
        };
        transform.translation = interp.pos;
        transform.rotation = interp.ori;

        if far {
            clear_previous_transform(&mut commands, entity, &children_query);
        }
    }
}

/// Removes [`PreviousGlobalTransform`] from `entity` and every descendant
/// (recursively), so Bevy treats their next frame's motion vector as zero
/// instead of the true, one-frame teleport delta a SNAP just caused (see
/// [`interpolate_entities`]'s doc comment). A no-op on entities that never had
/// the component (e.g. a figure not yet assembled — still just the
/// placeholder capsule on the root entity itself).
fn clear_previous_transform(
    commands: &mut Commands,
    entity: Entity,
    children_query: &Query<&Children>,
) {
    commands.entity(entity).remove::<PreviousGlobalTransform>();
    if let Ok(children) = children_query.get(entity) {
        for &child in children {
            clear_previous_transform(commands, child, children_query);
        }
    }
}

/// Vertical (Bevy y) gap between the local player's authoritative [`NetPos`]
/// and its eased [`Interpolated`] render position beyond which
/// [`log_local_player_landing_gap`] considers the render "floating" relative
/// to the sim's own ground truth.
const LANDING_GAP_THRESHOLD: f32 = 0.15;

/// BL-82 EM-3.11r diagnostic (Matías's "jump a lot and sometimes I don't reach
/// the ground, looks like jumping from mid-air" report). The interpolation
/// smoothing in [`interpolate_entities`] EASES the render toward the
/// authoritative `NetPos` rather than snapping (by design, see the module
/// docs) — during a fast jump/land cycle that lag can leave the rendered
/// model visibly ABOVE the sim's true (already-grounded) height for a
/// stretch of frames, which would look exactly like "still airborne" even
/// though the sim itself already registered the landing. This logs the
/// START and END of each such episode for the local player only (opt-in,
/// `XINDELER_LANDING_PERF_LOG=1`, cached `Local` read — same pattern as
/// `sprite_view.rs`'s `XINDELER_SPRITE_PERF_LOG`) so a real play session can
/// confirm/refute the render-lag hypothesis instead of guessing at it: pair
/// this with `XINDELER_JUMP_PERF_LOG=1` (`common/src/states/utils.rs::
/// handle_jump`) to see whether a "jump gate denied" or a "jump fired while
/// on_ground: None" ever coincides with a logged gap episode — the former
/// would confirm this is purely visual, the latter would point back to a
/// real sim-side grounded-state bug instead.
fn log_local_player_landing_gap(
    time: Res<Time>,
    query: Query<(&NetPos, Option<&NetVel>, &Interpolated), With<NetLocalPlayer>>,
    mut enabled: Local<Option<bool>>,
    // (episode start time, peak gap so far, MINIMUM |vel.y| seen so far). The
    // min-abs-velocity tracking disambiguates two very different mechanisms
    // that both show up as "gap > threshold": (a) the deliberate `VEL_LEAD`
    // dead-reckoning look-ahead (`step_pos`'s `target + vel·VEL_LEAD`), which
    // is large ONLY while `vel.y` is large (i.e. genuinely airborne, still
    // rising/falling fast) and is not a bug; vs. (b) real convergence lag —
    // the render still sitting above the sim's height AFTER `vel.y` has
    // already settled near zero (i.e. the sim itself says "landed, not
    // moving") — which WOULD be the "looks like it hasn't touched down"
    // artifact Matías described. If `min_abs_vel_y` at episode end is small,
    // the gap was genuine post-landing lag; if it stayed large the whole
    // episode, the gap was just the intentional lead term tracking a still-
    // airborne, fast-moving sample.
    mut episode_start: Local<Option<(f64, f32, f32)>>,
) {
    let enabled = *enabled
        .get_or_insert_with(|| std::env::var("XINDELER_LANDING_PERF_LOG").is_ok_and(|v| v != "0"));
    if !enabled {
        return;
    }
    let Ok((pos, vel, interp)) = query.single() else {
        return;
    };
    // Bevy y-up: positive gap = render sits ABOVE the sim's authoritative
    // height (i.e. looks like it hasn't landed yet).
    let gap = interp.pos.y - pos.0.y;
    let abs_vel_y = vel.map_or(0.0, |v| v.0.y.abs());
    let now = time.elapsed_secs_f64();
    match (*episode_start, gap > LANDING_GAP_THRESHOLD) {
        (None, true) => *episode_start = Some((now, gap, abs_vel_y)),
        (Some((start, peak, min_vel)), true) => {
            *episode_start = Some((start, peak.max(gap), min_vel.min(abs_vel_y)));
        },
        (Some((start, peak, min_vel)), false) => {
            info!(
                duration_ms = ((now - start) * 1000.0) as u64,
                peak_gap = peak,
                min_abs_vel_y = min_vel,
                "BL-82 EM-3.11r local-player render/sim landing gap episode ended"
            );
            *episode_start = None;
        },
        (None, false) => {},
    }
}

/// The pure interpolation step, factored out of [`interpolate_entities`] so it
/// can be unit-tested without a GPU/App. Returns the new interpolated position.
/// (System stays the single caller; keeping the math here documents + tests the
/// EM-3.7 smoothing decision in isolation.) `rate` is the caller-selected
/// per-frame lerp rate (BL-82 EM-3.11r: [`POS_LERP_RATE`] for remote entities,
/// [`LOCAL_PLAYER_POS_LERP_RATE`] for the local player's own entity).
fn step_pos(current: Vec3, target: Vec3, vel: Vec3, dt: f32, rate: f32) -> Vec3 {
    if current.distance_squared(target) < SNAP_DISTANCE * SNAP_DISTANCE {
        let t = (rate * dt).min(1.0);
        current.lerp(target + vel * VEL_LEAD, t)
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small move interpolates PARTWAY (no snap) — the anti-jitter property:
    /// the rendered position eases toward the target, it does not jump to it in
    /// one frame.
    #[test]
    fn small_move_eases_not_snaps() {
        let cur = Vec3::new(0.0, 0.0, 0.0);
        let target = Vec3::new(1.0, 0.0, 0.0);
        let next = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0, POS_LERP_RATE);
        // POS_LERP_RATE/60 ≈ 0.167 → moves ~16% of the way, not 100%.
        assert!(next.x > 0.0 && next.x < target.x, "eased partway: {next:?}");
        assert!(next.x < 0.5, "must not snap to the target in one frame");
    }

    /// Repeated steps CONVERGE to (near) the target — the ease actually
    /// arrives.
    #[test]
    fn repeated_steps_converge() {
        let target = Vec3::new(5.0, 2.0, -3.0);
        let mut cur = Vec3::ZERO;
        for _ in 0..300 {
            cur = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0, POS_LERP_RATE);
        }
        assert!(cur.distance(target) < 0.01, "converged to target: {cur:?}");
    }

    /// A jump beyond the snap distance SNAPS (teleport / first sample) instead
    /// of drifting slowly across the map.
    #[test]
    fn far_jump_snaps() {
        let cur = Vec3::ZERO;
        let target = Vec3::new(200.0, 0.0, 0.0); // > SNAP_DISTANCE
        let next = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0, POS_LERP_RATE);
        assert_eq!(next, target, "far jumps snap");
    }

    /// Velocity leads the target so a steadily-moving entity doesn't lag: with
    /// positive velocity the eased position aims slightly PAST the raw sample.
    #[test]
    fn velocity_leads_target() {
        let cur = Vec3::new(1.0, 0.0, 0.0);
        let target = Vec3::new(1.0, 0.0, 0.0);
        let vel = Vec3::new(10.0, 0.0, 0.0);
        let with_vel = step_pos(cur, target, vel, 1.0 / 60.0, POS_LERP_RATE);
        let no_vel = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0, POS_LERP_RATE);
        assert!(
            with_vel.x > no_vel.x,
            "dead-reckoning must lead the raw sample"
        );
    }

    /// BL-82 EM-3.11r regression: the local player's higher lerp rate closes
    /// the SAME gap measurably faster than the remote-entity rate — the fix
    /// for the "jump a lot, sometimes I don't reach the ground" report (a
    /// real scripted repro found the rendered local player lagging up to
    /// 0.80 m / 510 ms behind its own already-landed sim position using the
    /// old shared 10/s rate; see `LOCAL_PLAYER_POS_LERP_RATE`'s doc comment).
    #[test]
    fn local_player_rate_converges_faster_than_remote_rate() {
        let cur = Vec3::ZERO;
        let target = Vec3::new(0.0, 1.0, 0.0); // a 1m vertical gap, e.g. a landing
        let dt = 1.0 / 60.0;
        let remote_after_one_frame = step_pos(cur, target, Vec3::ZERO, dt, POS_LERP_RATE);
        let local_after_one_frame =
            step_pos(cur, target, Vec3::ZERO, dt, LOCAL_PLAYER_POS_LERP_RATE);
        assert!(
            local_after_one_frame.y > remote_after_one_frame.y,
            "local player rate must close a fresh gap faster: local={local_after_one_frame:?} \
             remote={remote_after_one_frame:?}"
        );

        // After a realistic post-landing stretch of render frames (~100ms),
        // the local player's rate must have converged FAR closer to the
        // authoritative position than the remote rate would have.
        let frames = (0.1 / dt) as u32;
        let mut local = cur;
        let mut remote = cur;
        for _ in 0..frames {
            local = step_pos(local, target, Vec3::ZERO, dt, LOCAL_PLAYER_POS_LERP_RATE);
            remote = step_pos(remote, target, Vec3::ZERO, dt, POS_LERP_RATE);
        }
        assert!(
            target.y - local.y < 0.05,
            "local player must be within 5cm of the ground after ~100ms: {local:?}"
        );
        assert!(
            target.y - remote.y > target.y - local.y,
            "remote rate must still lag further behind than the local rate at the same instant"
        );
    }

    /// EM-3.11g regression: a SNAP (far jump) must clear
    /// [`PreviousGlobalTransform`] on the mirrored entity AND every figure-part
    /// child, so Bevy reports zero motion for the teleport frame instead of a
    /// bogus, extreme one-frame motion vector a TAA reprojection can turn into
    /// a lingering ghost (see [`interpolate_entities`]'s doc comment). A small
    /// in-band move must NOT touch it — only an actual snap should.
    #[test]
    fn snap_clears_previous_transform_on_self_and_children() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, interpolate_entities);

        // Beyond SNAP_DISTANCE from the seeded Interpolated position.
        let target = Vec3::new(200.0, 0.0, 0.0);

        let child = app
            .world_mut()
            .spawn((Transform::default(), PreviousGlobalTransform::default()))
            .id();
        let grandchild = app
            .world_mut()
            .spawn((
                Transform::default(),
                PreviousGlobalTransform::default(),
                ChildOf(child),
            ))
            .id();
        let root = app
            .world_mut()
            .spawn((
                NetPos(target),
                NetOri(Quat::IDENTITY),
                Interpolated {
                    pos: Vec3::ZERO,
                    ori: Quat::IDENTITY,
                },
                Transform::default(),
                PreviousGlobalTransform::default(),
            ))
            .id();
        app.world_mut().entity_mut(child).insert(ChildOf(root));

        app.update();

        assert!(
            app.world().get::<PreviousGlobalTransform>(root).is_none(),
            "the snapped root must have PreviousGlobalTransform cleared"
        );
        assert!(
            app.world().get::<PreviousGlobalTransform>(child).is_none(),
            "a direct figure-part child must have PreviousGlobalTransform cleared too"
        );
        assert!(
            app.world()
                .get::<PreviousGlobalTransform>(grandchild)
                .is_none(),
            "the clear must recurse past one level of hierarchy"
        );
    }

    /// The non-snap path (small in-band move) must leave
    /// [`PreviousGlobalTransform`] untouched — only a genuine teleport should
    /// suppress the motion vector.
    #[test]
    fn small_move_leaves_previous_transform_untouched() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, interpolate_entities);

        // Well within SNAP_DISTANCE.
        let target = Vec3::new(1.0, 0.0, 0.0);

        let root = app
            .world_mut()
            .spawn((
                NetPos(target),
                NetOri(Quat::IDENTITY),
                Interpolated {
                    pos: Vec3::ZERO,
                    ori: Quat::IDENTITY,
                },
                Transform::default(),
                PreviousGlobalTransform::default(),
            ))
            .id();

        app.update();

        assert!(
            app.world().get::<PreviousGlobalTransform>(root).is_some(),
            "a normal eased move must not clear PreviousGlobalTransform"
        );
    }
}
