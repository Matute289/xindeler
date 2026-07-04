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
//!    never "drift" across the whole map.
//!
//! ## Purity
//! This module is 100% Bevy + `xindeler-protocol` — NO `specs`, no server
//! crate. The engine-isolation guard greps this crate's `src` for `specs`; it
//! stays clean. Compiled only under the `listen-server` feature.

use bevy::prelude::*;
use xindeler_protocol::{NetBody, NetOri, NetPos, NetVel};

/// Per-frame exponential-lerp rate for position (voxygen
/// `POS_LERP_RATE_FACTOR`). `lerp(cur, target, RATE·dt)` each frame → a smooth
/// critically-ish-damped ease that reaches the target quickly at 60+ fps but
/// tolerates sparse net updates.
const POS_LERP_RATE: f32 = 10.0;
/// Orientation slerp rate (voxygen `base_ori_interp` default, non-object).
const ORI_LERP_RATE: f32 = 10.0;
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
        app.add_systems(Update, (add_presentation, interpolate_entities).chain());
    }
}

/// A distinct, readable colour per body-class id (the [`NetBody`] key from the
/// bridge's `body_class_id`). Placeholder palette only — the real per-species
/// model + texture is EM-3.8.
fn body_class_color(class: u32) -> Color {
    // A small qualitative palette; classes past its length wrap.
    const PALETTE: [(f32, f32, f32); 8] = [
        (0.90, 0.30, 0.30), // 0 humanoid — red
        (0.40, 0.80, 0.40), // 1 quadruped small — green
        (0.35, 0.55, 0.95), // 2 quadruped medium — blue
        (0.95, 0.80, 0.30), // 3 bird medium — yellow
        (0.75, 0.45, 0.90), // 4 fish medium — purple
        (0.95, 0.60, 0.25), // 5 dragon — orange
        (0.30, 0.85, 0.85), // 6 bird large — cyan
        (0.85, 0.85, 0.85), // 7 fish small — grey
    ];
    let (r, g, b) = PALETTE[(class as usize) % PALETTE.len()];
    Color::srgb(r, g, b)
}

/// A rough visual size per body-class so a humanoid isn't the same blob as a
/// dragon. Radius, half-height (capsule). Placeholder scale only (EM-3.8).
fn body_class_capsule(class: u32) -> (f32, f32) {
    match class {
        0 => (0.4, 0.9),  // humanoid
        1 => (0.4, 0.4),  // quadruped small
        2 => (0.7, 0.7),  // quadruped medium
        5 => (1.5, 2.0),  // dragon
        8 => (0.8, 1.4),  // biped large
        11 => (1.2, 1.6), // golem
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
        let (radius, half_length) = body_class_capsule(body.0);
        let mesh = meshes.add(Capsule3d::new(radius, half_length * 2.0));
        let material = materials.add(StandardMaterial {
            base_color: body_class_color(body.0),
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
fn interpolate_entities(
    time: Res<Time>,
    mut query: Query<(
        &NetPos,
        &NetOri,
        Option<&NetVel>,
        &mut Interpolated,
        &mut Transform,
    )>,
) {
    let dt = time.delta_secs();
    for (pos, ori, vel, mut interp, mut transform) in &mut query {
        let target = pos.0;
        let far = interp.pos.distance_squared(target) >= SNAP_DISTANCE * SNAP_DISTANCE;
        interp.pos = step_pos(interp.pos, target, vel.map_or(Vec3::ZERO, |v| v.0), dt);
        interp.ori = if far {
            // Teleport / first real sample: snap orientation too.
            ori.0
        } else {
            interp.ori.slerp(ori.0, (ORI_LERP_RATE * dt).min(1.0))
        };
        transform.translation = interp.pos;
        transform.rotation = interp.ori;
    }
}

/// The pure interpolation step, factored out of [`interpolate_entities`] so it
/// can be unit-tested without a GPU/App. Returns the new interpolated position.
/// (System stays the single caller; keeping the math here documents + tests the
/// EM-3.7 smoothing decision in isolation.)
fn step_pos(current: Vec3, target: Vec3, vel: Vec3, dt: f32) -> Vec3 {
    if current.distance_squared(target) < SNAP_DISTANCE * SNAP_DISTANCE {
        let t = (POS_LERP_RATE * dt).min(1.0);
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
        let next = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0);
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
            cur = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0);
        }
        assert!(cur.distance(target) < 0.01, "converged to target: {cur:?}");
    }

    /// A jump beyond the snap distance SNAPS (teleport / first sample) instead
    /// of drifting slowly across the map.
    #[test]
    fn far_jump_snaps() {
        let cur = Vec3::ZERO;
        let target = Vec3::new(200.0, 0.0, 0.0); // > SNAP_DISTANCE
        let next = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0);
        assert_eq!(next, target, "far jumps snap");
    }

    /// Velocity leads the target so a steadily-moving entity doesn't lag: with
    /// positive velocity the eased position aims slightly PAST the raw sample.
    #[test]
    fn velocity_leads_target() {
        let cur = Vec3::new(1.0, 0.0, 0.0);
        let target = Vec3::new(1.0, 0.0, 0.0);
        let vel = Vec3::new(10.0, 0.0, 0.0);
        let with_vel = step_pos(cur, target, vel, 1.0 / 60.0);
        let no_vel = step_pos(cur, target, Vec3::ZERO, 1.0 / 60.0);
        assert!(
            with_vel.x > no_vel.x,
            "dead-reckoning must lead the raw sample"
        );
    }
}
