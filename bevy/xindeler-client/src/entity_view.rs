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
//! 2. Every frame [`interpolate_entities`] handles TWO distinct sources, picked
//!    per-entity:
//!    - **The local player** (tagged [`NetLocalPlayer`]), once a
//!      [`PredictedLocalTransform`] exists for it: renders DIRECTLY from that
//!      component — a snap, no ease, no lerp rate at all (BL-82 EM-4.11). That
//!      component is the SAME embedded `xindeler-client-core::Client` predictor
//!      old (pre-Bevy) voxygen used, now ticked once per rendered `Update`
//!      frame (`xindeler_sim_bridge::player::tick_player`) instead of at the
//!      sim's 30 Hz `FixedUpdate` rate — see that component's doc comment and
//!      `docs/design/specs/2026-07-11-bl82-frame-rate-prediction-design.md` for
//!      the full root-cause/design writeup. This is the source that used to be
//!      eased at a distinct, higher rate (EM-3.11r's
//!      `LOCAL_PLAYER_POS_LERP_RATE`, since retired): that mitigation narrowed
//!      the "renders above an already-landed position" symptom without removing
//!      the underlying 30 Hz sampling lag or its per-tick motion quantization;
//!      predicting at frame rate removes both at the source, not just the
//!      symptom, and was verified to do so (EM-4.11 Phase A/B acceptance
//!      capture: the isolated single-frame zero-delta gaps that were the
//!      literal tick-quantization signature — 101 of them in a 20s
//!      straight-walk baseline — dropped to ZERO after this fix).
//!    - **Every remote mirrored entity** (no `NetLocalPlayer`, or the local
//!      player before its very first prediction lands, pre-spawn): eases toward
//!      its latest `NetPos`/`NetOri` (dead-reckoned by `NetVel`) exactly as
//!      before, unaffected by this change. The constants are ported verbatim
//!      from voxygen's interpolation system
//!      (`voxygen/src/ecs/sys/interpolation.rs`): exponential lerp at rate 10/s
//!      toward `pos + vel·0.03`, slerp orientation at rate 10/s, and a hard
//!      SNAP when the target jumps more than 64 m (teleports / first frame) so
//!      we never "drift" across the whole map.
//!
//!    **EM-3.11g** (applies to BOTH sources above): a SNAP also clears
//!    [`bevy::pbr::PreviousGlobalTransform`] on the entity + every figure-part
//!    child, so Bevy reports zero motion for that one teleport frame instead of
//!    the true, extreme jump — see [`interpolate_entities`]'s doc comment for
//!    the TAA-ghosting bug this fixes ("detached ghost hand" report).
//!
//! ## Purity
//! This module is 100% Bevy + `xindeler-protocol` — NO `specs`, no server
//! crate. The engine-isolation guard greps this crate's `src` for `specs`; it
//! stays clean. Compiled only under the `listen-server`/`net-client` features
//! (the local-player prediction path is `listen-server`-only — see
//! [`EntityViewPlugin::build`]'s doc for why).

use bevy::{pbr::PreviousGlobalTransform, prelude::*};
use xindeler_protocol::{NetBody, NetLocalPlayer, NetOri, NetPos, NetVel, PredictedLocalTransform};

/// Per-frame exponential-lerp rate for position (voxygen
/// `POS_LERP_RATE_FACTOR`). `lerp(cur, target, RATE·dt)` each frame → a smooth
/// critically-ish-damped ease that reaches the target quickly at 60+ fps but
/// tolerates sparse net updates. Applies to every REMOTE mirrored entity
/// (NPCs, other players once real multiplayer lands) — tuned to hide real
/// network jitter/packet loss, same as voxygen's original.
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
        // BL-82 (4-reviewer pass follow-up, MAJOR finding): `.in_set(MirrorSet)`
        // is NEW — `xindeler_app::sets` already `configure_sets(Update,
        // (MirrorSet, GameplaySet).chain())`, declaring that EVERY system in
        // `MirrorSet` must finish before ANY system in `GameplaySet` starts,
        // but until now nothing anywhere in `bevy/` actually tagged a system
        // with `MirrorSet` — the set existed, un-applied, closing no real
        // ordering gap. `player_input.rs`'s `third_person_camera`
        // (`.after(FlyCamSet).in_set(GameplaySet)`) queries
        // `(&Transform, Option<&Interpolated>)` on the SAME local-player
        // mirror entity `interpolate_entities` writes, with NO ordering edge
        // between the two plugins — Bevy was free to run the camera before
        // this frame's interpolation, reading last frame's `Transform`.
        // Harmless while the local player eased smoothly (a one-frame-stale
        // ease is imperceptible), but EM-4.11 changed the local player's
        // render source to a hard SNAP (`PredictedLocalTransform`, no ease at
        // all) — a one-frame-stale read of a SNAP is a visible position/
        // collision mismatch, plausibly part of the lingering camera-flicker/
        // clip symptoms from the EM-3.11/4.11/3.12 investigation arc. Tagging
        // these two systems (rather than adding one targeted `.after(..)` edge
        // to `PlayerInputPlugin`) closes the WHOLE class of gap at once: every
        // current AND future `GameplaySet` system is now guaranteed to read
        // this frame's mirrored/interpolated state, not just
        // `third_person_camera`. Verified safe: every existing `.in_set(..)`
        // call in `bevy/` uses only `GameplaySet`/`PresentationSet` (`grep -rn
        // "in_set(" bevy/`) — `MirrorSet`/`NetSet`/`SimSet` were all
        // previously unused — so this is the FIRST system ever placed in
        // `MirrorSet`, meaning there is no pre-existing ordering this could
        // conflict with, only the (already-declared, previously vacuous)
        // `MirrorSet -> GameplaySet` constraint becoming real.
        let systems = (add_presentation, interpolate_entities)
            .chain()
            .in_set(xindeler_app::MirrorSet);
        // BL-82 EM-4.11: `interpolate_entities` reads `PredictedLocalTransform`,
        // written by `xindeler_sim_bridge::mirror_local_player_prediction`
        // (`PlayerBridgePlugin`, also `Update`, a DIFFERENT plugin — no
        // implicit ordering exists between two plugins' systems). Without
        // this EXPLICIT constraint the write and the read have no guaranteed
        // relative order, so the render could read a stale (previous-frame)
        // prediction — exactly the kind of same-frame visibility bug this
        // constraint exists to rule out (caught empirically: the Phase-B
        // acceptance numbers barely improved without it). `mirror_local_
        // player_prediction` mutates the component in place after its first
        // frame (a plain `Query<&mut _>` write, not `Commands`), so this
        // `.after()` is a cheap, direct ordering edge on the steady-state
        // path; only the very first frame (the component doesn't exist yet)
        // needs `Commands::insert`, in which case Bevy also auto-inserts the
        // `ApplyDeferred` sync point this same edge implies.
        //
        // Gated on `listen-server`: `xindeler-sim-bridge` (and therefore
        // `mirror_local_player_prediction`) is only linked under that
        // feature — this exact module also compiles under `net-client`
        // alone (a genuinely remote client, no embedded sim), where
        // `PredictedLocalTransform` is never written by anything and the
        // local player simply falls through to the `NetPos`-easing branch
        // below, so no ordering constraint is needed (or resolvable) there.
        #[cfg(feature = "listen-server")]
        let systems = systems.after(xindeler_sim_bridge::mirror_local_player_prediction);
        app.add_systems(Update, systems);
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
// `pub(crate)`: BL-82 EM-5.18 P1's `targeting::update_soft_target` orders
// itself `.after(interpolate_entities)` (same `MirrorSet`) so it reads this
// frame's eased `Transform`/`Interpolated`, not last frame's — see that
// system's own doc comment.
pub(crate) fn interpolate_entities(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &NetPos,
        &NetOri,
        Option<&NetVel>,
        Option<&NetLocalPlayer>,
        Option<&PredictedLocalTransform>,
        &mut Interpolated,
        &mut Transform,
    )>,
    children_query: Query<&Children>,
) {
    let dt = time.delta_secs();
    for (entity, pos, ori, vel, local_player, predicted, mut interp, mut transform) in &mut query {
        // BL-82 EM-4.11: the local player renders straight from its OWN
        // frame-rate prediction (a snap, not an ease) once one exists — see
        // `xindeler_protocol::PredictedLocalTransform`'s doc comment. This is
        // the architecturally complete fix EM-3.11r's retired
        // `LOCAL_PLAYER_POS_LERP_RATE` mitigation named as a follow-up: the
        // prediction is already smooth, frame-rate, zero-jitter, same-process
        // data, so there is nothing left to ease. `Interpolated` is still
        // updated (not just `Transform`) so a
        // later frame that loses the prediction (the pre-spawn/no-mirror
        // window, or a spectator fallback) resumes easing from the right
        // place instead of an origin default. Every REMOTE entity — and the
        // local player before its first prediction ever lands — falls through
        // to the unchanged `NetPos` easing below.
        if local_player.is_some()
            && let Some(predicted) = predicted
        {
            let far = interp.pos.distance_squared(predicted.pos) >= SNAP_DISTANCE * SNAP_DISTANCE;
            interp.pos = predicted.pos;
            interp.ori = predicted.ori;
            transform.translation = interp.pos;
            transform.rotation = interp.ori;
            if far {
                clear_previous_transform(&mut commands, entity, &children_query);
            }
            continue;
        }

        let target = pos.0;
        let far = interp.pos.distance_squared(target) >= SNAP_DISTANCE * SNAP_DISTANCE;
        // BL-82 EM-4.11: this branch is now reached by the local player ONLY
        // before its first `PredictedLocalTransform` has ever landed
        // (pre-spawn) — the shared `POS_LERP_RATE` is fine there, same as any
        // other not-yet-predicted first sample; retired the EM-3.11r
        // dedicated higher rate (`LOCAL_PLAYER_POS_LERP_RATE`), superseded by
        // the prediction branch above for every subsequent frame.
        interp.pos = step_pos(
            interp.pos,
            target,
            vel.map_or(Vec3::ZERO, |v| v.0),
            dt,
            POS_LERP_RATE,
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

/// The pure interpolation step, factored out of [`interpolate_entities`] so it
/// can be unit-tested without a GPU/App. Returns the new interpolated position.
/// (System stays the single caller; keeping the math here documents + tests the
/// EM-3.7 smoothing decision in isolation.) `rate` is the caller-selected
/// per-frame lerp rate — [`POS_LERP_RATE`] for every caller today (remote
/// entities, and the local player's own pre-prediction fallback; BL-82
/// EM-4.11 retired the EM-3.11r local-player-specific higher rate now that the
/// local player renders from [`PredictedLocalTransform`] instead).
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

    /// BL-82 EM-4.11 regression: with a [`PredictedLocalTransform`] present,
    /// the LOCAL player's `Transform` must equal the predicted pose EXACTLY
    /// (a snap, no ease) — even a single frame must land dead-on, unlike the
    /// old `NetPos`-easing path this supersedes for the local player. A
    /// REMOTE entity (no `NetLocalPlayer`, no `PredictedLocalTransform`) in
    /// the SAME `app.update()` must still ease toward its `NetPos` as before
    /// (`small_move_eases_not_snaps`'s property), proving the new branch only
    /// affects the local player.
    #[test]
    fn local_player_with_prediction_snaps_remote_still_eases() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, interpolate_entities);

        // The predicted pose is FAR from both the seeded Interpolated AND the
        // (irrelevant, for the local player) NetPos — proving the render
        // follows the prediction, not NetPos, and does so in one frame.
        let predicted_pos = Vec3::new(5.0, 1.0, -3.0);
        let predicted_ori = Quat::from_rotation_y(1.0);
        let stale_net_pos = Vec3::new(500.0, 0.0, 500.0);

        let local_player = app
            .world_mut()
            .spawn((
                NetPos(stale_net_pos),
                NetOri(Quat::IDENTITY),
                NetLocalPlayer,
                PredictedLocalTransform {
                    pos: predicted_pos,
                    ori: predicted_ori,
                    vel: Vec3::ZERO,
                },
                Interpolated {
                    pos: Vec3::ZERO,
                    ori: Quat::IDENTITY,
                },
                Transform::default(),
            ))
            .id();

        // A remote entity, same frame: small in-band move, no prediction.
        let remote_target = Vec3::new(1.0, 0.0, 0.0);
        let remote = app
            .world_mut()
            .spawn((
                NetPos(remote_target),
                NetOri(Quat::IDENTITY),
                Interpolated {
                    pos: Vec3::ZERO,
                    ori: Quat::IDENTITY,
                },
                Transform::default(),
            ))
            .id();

        // `Time::delta_secs()` is 0.0 on the very FIRST `app.update()` call
        // (no prior frame to diff against) — the local-player snap branch
        // doesn't need a nonzero dt, but the remote entity's ease does (a
        // zero dt would trivially leave it at the origin either way, which
        // wouldn't prove anything). A second update gives `Time` a real,
        // nonzero delta.
        app.update();
        app.update();

        let local_transform = app.world().get::<Transform>(local_player).unwrap();
        assert_eq!(
            local_transform.translation, predicted_pos,
            "the local player must render EXACTLY at the predicted position, not eased toward \
             NetPos"
        );
        assert_eq!(
            local_transform.rotation, predicted_ori,
            "the local player must render EXACTLY at the predicted orientation"
        );

        let remote_transform = app.world().get::<Transform>(remote).unwrap();
        assert!(
            remote_transform.translation.x > 0.0
                && remote_transform.translation.x < remote_target.x,
            "a remote entity with no prediction must still ease partway, not snap: {:?}",
            remote_transform.translation
        );
    }

    /// BL-82 (4-reviewer pass, MAJOR finding — Finding 3): regression test
    /// for the missing ordering edge between [`EntityViewPlugin`]'s systems
    /// and `player_input.rs`'s `third_person_camera`
    /// (`.after(FlyCamSet).in_set(GameplaySet)`), which queries
    /// `(&Transform, Option<&Interpolated>)` on the SAME local-player mirror
    /// entity [`interpolate_entities`] writes.
    ///
    /// Uses the REAL [`EntityViewPlugin`] (so a future regression that
    /// removes `.in_set(xindeler_app::MirrorSet)` from its registration is
    /// actually caught here, not just in a hand-rolled stand-in
    /// registration) plus the REAL `xindeler_app::sets::configure` ordering
    /// (`configure_sets(Update, (MirrorSet, GameplaySet).chain())`), and a
    /// fake `GameplaySet`-tagged consumer standing in for
    /// `third_person_camera`'s read of `Transform`.
    ///
    /// Drives the local-player SNAP path (`PredictedLocalTransform`, EM-4.11)
    /// specifically because it makes the property EXACTLY checkable: every
    /// frame, `interpolate_entities` sets `Transform` to be bit-for-bit equal
    /// to that frame's `PredictedLocalTransform` (no easing/lerp to fuzz the
    /// comparison — see the module doc's "hard SNAP, not a smoothed ease" for
    /// why EM-4.11 made a stale read of this specific path newly visible). A
    /// driver system in `PreUpdate` (which Bevy's default schedule order
    /// always runs before `Update`, needing no extra constraint) writes a
    /// FRESH, frame-indexed `PredictedLocalTransform` every tick; if the
    /// consumer ever reads anything OTHER than THIS tick's value, the
    /// mirror-before-gameplay ordering was violated.
    #[test]
    fn entity_view_systems_run_before_gameplay_set_consumers() {
        use xindeler_app::{GameplaySet, MirrorSet};

        #[derive(Resource, Default)]
        struct FrameCounter(u32);

        #[derive(Resource, Default)]
        struct ObservedTransform(Option<Vec3>);

        fn drive_prediction(
            mut counter: ResMut<FrameCounter>,
            mut query: Query<&mut PredictedLocalTransform, With<NetLocalPlayer>>,
        ) {
            counter.0 += 1;
            let x = counter.0 as f32;
            for mut predicted in &mut query {
                predicted.pos = Vec3::new(x, 0.0, 0.0);
            }
        }

        fn observe_as_gameplay_would(
            query: Query<&Transform, With<NetLocalPlayer>>,
            mut observed: ResMut<ObservedTransform>,
        ) {
            observed.0 = query.iter().next().map(|t| t.translation);
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // `add_presentation` needs `Assets<Mesh>`/`Assets<StandardMaterial>`
        // as valid resources to run (even though its `Added<NetBody>` query
        // matches nothing here — this test's entity never gets a `NetBody`,
        // only the components `add_presentation` would otherwise have
        // attached, pre-seeded directly, matching this file's OTHER
        // `interpolate_entities`-only tests above).
        app.init_resource::<Assets<Mesh>>();
        app.init_resource::<Assets<StandardMaterial>>();
        // The REAL shared set ORDERING — `xindeler_app::sets::configure`
        // declares this exact constraint too
        // (`configure_sets(Update, (MirrorSet, GameplaySet).chain())`), but
        // is `pub(crate)` (only callable from `XindelerAppPlugin`, which also
        // unconditionally pulls in `bevy_dev_tools`'s FPS overlay per this
        // crate's own `Cargo.toml` — needs the full render/window/font stack,
        // not just `MinimalPlugins`). Declaring the same constraint directly
        // against the REAL `MirrorSet`/`GameplaySet` types keeps this test
        // scoped to the property under test (does tagging
        // `EntityViewPlugin`'s systems `.in_set(MirrorSet)` actually order
        // them before a `GameplaySet` consumer?) without an unrelated
        // dependency on the FPS overlay.
        app.configure_sets(Update, (MirrorSet, GameplaySet).chain());
        // The REAL plugin under test — NOT `net-client`/`listen-server`
        // gated, so this compiles and registers identically regardless of
        // which feature this crate's tests happen to run under.
        app.add_plugins(EntityViewPlugin);

        app.init_resource::<FrameCounter>();
        app.init_resource::<ObservedTransform>();
        app.add_systems(PreUpdate, drive_prediction);
        app.add_systems(Update, observe_as_gameplay_would.in_set(GameplaySet));

        app.world_mut().spawn((
            NetLocalPlayer,
            NetPos(Vec3::new(999.0, 999.0, 999.0)),
            NetOri(Quat::IDENTITY),
            PredictedLocalTransform {
                pos: Vec3::ZERO,
                ori: Quat::IDENTITY,
                vel: Vec3::ZERO,
            },
            Interpolated {
                pos: Vec3::ZERO,
                ori: Quat::IDENTITY,
            },
            Transform::default(),
        ));

        for frame in 1..=20u32 {
            app.update();
            let expected = Vec3::new(frame as f32, 0.0, 0.0);
            let observed = app.world().resource::<ObservedTransform>().0;
            assert_eq!(
                observed,
                Some(expected),
                "frame {frame}: the GameplaySet consumer must observe THIS SAME frame's \
                 interpolated/snapped Transform, not a stale one — without `interpolate_entities` \
                 (via `EntityViewPlugin`) being ordered `.in_set(MirrorSet)` (before \
                 `GameplaySet`), this could read last frame's value instead"
            );
        }
    }
}
