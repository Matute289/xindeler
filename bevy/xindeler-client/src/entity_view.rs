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
//!
//! ## Purity
//! This module is 100% Bevy + `xindeler-protocol` — NO `specs`, no server
//! crate. The engine-isolation guard greps this crate's `src` for `specs`; it
//! stays clean. Compiled only under the `listen-server` feature.

use bevy::{pbr::PreviousGlobalTransform, prelude::*};
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
        &mut Interpolated,
        &mut Transform,
    )>,
    children_query: Query<&Children>,
) {
    let dt = time.delta_secs();
    for (entity, pos, ori, vel, mut interp, mut transform) in &mut query {
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
