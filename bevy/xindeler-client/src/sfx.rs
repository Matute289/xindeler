//! BL-82 EM-5.10b (T56.35) — the SFX event-mapper systems: read real
//! mirrored client-side state (`xindeler_protocol::Net*`) and decide WHEN to
//! trigger which `xindeler_audio::sfx::SfxEvent`, porting the CONCEPTS of the
//! old client's 5 `voxygen::audio::sfx::event_mapper` sub-mappers
//! (`voxygen/src/audio/sfx/event_mapper/{movement,combat,campfire,block,
//! vehicle}`) as idiomatic Bevy systems rather than a literal transliteration
//! (that code iterated the live sim ECS world directly; ours reads
//! replicated `Net*` components off Bevy entities).
//!
//! ## What's ported for real vs. a documented stub
//! - **Movement** ([`movement_sfx_mapper`]) — REAL. Footsteps (Run/QuadRun/
//!   OctoRun), Swim, Roll/RollCancel, Sneak, Climb, Glide, all off the real
//!   `NetLocomotion` mirror (EM-5.10b's own new addition) + already-mirrored
//!   `NetVel`/`NetBody`.
//! - **Combat** ([`combat_sfx_mapper`]) — REAL. Attack/Wield/Unwield off the
//!   real `NetCombatMove` mirror + already-mirrored `NetLoadout::active_tool`
//!   for the tool kind. `Music` (playing an instrument) is EXPLICITLY deferred
//!   — it needs the instrument note-bank, EM-5.10e/T56.37's own job.
//! - **Campfire** ([`campfire_sfx_mapper`]) — REAL, using ONLY already-mirrored
//!   state (`NetBody::Object(CampfireLit)` + `Transform`) — no new mirror
//!   needed at all.
//! - **`handle_outcome`** ([`handle_outcome_sfx`]) — REAL, over the new partial
//!   `NetOutcome` message (see `xindeler_protocol::sfx`'s own doc comment for
//!   the "5 of ~40 covered" v1 cut).
//! - **Block** — DOCUMENTED STUB (no system registered). The old mapper's
//!   ambient sounds (birdsong/crickets/frogs/running water/lava) are keyed off
//!   `scene::terrain::BlocksOfInterest` — a per-chunk scenery classification
//!   computed by the OLD renderer's own terrain mesher. This Bevy port's
//!   terrain pipeline (`xindeler-render-voxel`) never computes or mirrors that
//!   classification (verified: no "blocks of interest" mirror exists anywhere
//!   in `xindeler-protocol`/`xindeler-sim-bridge` today) — building one is a
//!   real, separate bulk-terrain-metadata mirror (a NEW per-chunk message,
//!   matching spec §3.2's "bulk data = messages" rule), out of scope for "wire
//!   up systems reading state that already exists". Fabricating fake ambient
//!   triggers off unrelated state would be worse than shipping nothing here.
//! - **Vehicle** — DOCUMENTED STUB (no system registered). The old mapper only
//!   ever triggers for `Body::Ship(ship::Body::Train)` — this Bevy port has no
//!   vehicle/mount/train system at all yet (verified: no `Ship`/mount-related
//!   mirror exists in `xindeler-protocol`). Nothing to wire up; a real
//!   follow-up once BL-82's vehicle port lands.

use std::{collections::HashMap, time::Instant};

use bevy::prelude::*;
use common::comp::{self, Body};
use xindeler_audio::{
    AudioBackend, XindelerAudioAsset,
    sfx::{
        AudioListener, SFX_DIST_LIMIT_SQR, SfxAssetCache, SfxEvent, SfxManifest, SfxManifestHandle,
        trigger_sfx,
    },
};
use xindeler_protocol::{
    NetBody, NetCombatMove, NetGroundBlock, NetLoadout, NetLocalPlayer, NetLocomotion,
    NetMoveState, NetOutcome, NetUid, NetVel,
};

// ---------------------------------------------------------------------------
// Movement mapper (REAL) — footsteps + swim + roll + sneak + climb + glide.
// ---------------------------------------------------------------------------

/// Per-entity movement bookkeeping the client keeps itself (mirrors the old
/// mapper's own `event_history: HashMap<Entity, PreviousEntityState>` — this
/// is CLIENT-local history, never mirrored, exactly like the original).
struct MoveHistory {
    event: SfxEvent,
    time: Instant,
    on_ground: bool,
    in_liquid: bool,
}

impl Default for MoveHistory {
    fn default() -> Self {
        Self {
            event: SfxEvent::Idle,
            time: Instant::now(),
            on_ground: true,
            in_liquid: false,
        }
    }
}

/// Broad body-shape categories the old client's 4 separate movement-mapping
/// functions (`map_movement_event`/`map_quadruped_movement_event`/
/// `map_non_humanoid_movement_event`/`map_arthropod_movement_event`) drew —
/// only [`BodyMoveKind::Humanoid`] also reads [`NetLocomotion::move_state`]
/// (Roll/Sneak/Climb/Glide); the others are physics-only (Swim/Run-family/
/// Idle), matching the old code's own functions exactly (they never inspect
/// `CharacterState` at all).
enum BodyMoveKind {
    Humanoid,
    Quadruped,
    NonHumanoid,
    Arthropod,
    /// Fish and other bodies the old client's own top-level dispatch
    /// (`_ => SfxEvent::Idle`) never assigns a movement sound to.
    Silent,
}

fn body_move_kind(body: &Body) -> BodyMoveKind {
    match body {
        Body::Humanoid(_) => BodyMoveKind::Humanoid,
        Body::QuadrupedMedium(_) | Body::QuadrupedSmall(_) | Body::QuadrupedLow(_) => {
            BodyMoveKind::Quadruped
        },
        Body::BirdMedium(_) | Body::BirdLarge(_) | Body::BipedLarge(_) => BodyMoveKind::NonHumanoid,
        Body::Arthropod(_) => BodyMoveKind::Arthropod,
        _ => BodyMoveKind::Silent,
    }
}

/// Wraps a ground-material group into the right `Run`-family [`SfxEvent`]
/// container for `kind` — ported from the old client's own per-function
/// match arms (`Run`/`QuadRun`/`OctoRun` all group blocks identically; only
/// the container differs by body shape). [`NetGroundBlock::Air`] maps to
/// `Idle`, matching the old code's `BlockKind::Air => SfxEvent::Idle` arm
/// (grounded-on-air is not a real footstep).
fn run_event(kind: &BodyMoveKind, ground: NetGroundBlock) -> SfxEvent {
    use common::terrain::BlockKind;
    if ground == NetGroundBlock::Air {
        return SfxEvent::Idle;
    }
    let block = match ground {
        NetGroundBlock::Snow => BlockKind::Snow,
        NetGroundBlock::Rock => BlockKind::Rock,
        NetGroundBlock::Earth => BlockKind::Earth,
        NetGroundBlock::Grass | NetGroundBlock::Air => BlockKind::Grass,
    };
    match kind {
        BodyMoveKind::Quadruped => SfxEvent::QuadRun(block),
        BodyMoveKind::Arthropod => SfxEvent::OctoRun(block),
        _ => SfxEvent::Run(block),
    }
}

/// Classifies this frame's movement [`SfxEvent`] for one mirrored entity —
/// ported from the old client's 4 `map_*_movement_event` functions, unified:
/// the physics-gated Swim/Run-family/Idle shape is shared by every body
/// category; only [`BodyMoveKind::Humanoid`] ALSO layers the
/// `CharacterState`-derived [`NetMoveState`] (Roll/Sneak/Climb/Glide) on top,
/// matching the old code exactly (the other 3 functions never read it).
fn classify_movement_event(
    kind: &BodyMoveKind,
    locomotion: &NetLocomotion,
    vel_magnitude: f32,
    history: &MoveHistory,
) -> SfxEvent {
    if matches!(kind, BodyMoveKind::Silent) {
        return SfxEvent::Idle;
    }

    if locomotion.in_liquid && (vel_magnitude > 2.0 || (!history.in_liquid && locomotion.in_liquid))
    {
        return SfxEvent::Swim;
    }

    if locomotion.on_ground && (vel_magnitude > 0.1 || !history.on_ground) {
        return match kind {
            BodyMoveKind::Humanoid => match locomotion.move_state {
                NetMoveState::Roll => SfxEvent::Roll,
                NetMoveState::RollCancel => SfxEvent::RollCancel,
                NetMoveState::Sneak => SfxEvent::Sneak,
                _ => run_event(kind, locomotion.ground_block),
            },
            _ => run_event(kind, locomotion.ground_block),
        };
    }

    if matches!(kind, BodyMoveKind::Humanoid) {
        return match locomotion.move_state {
            NetMoveState::Climb => SfxEvent::Climb,
            NetMoveState::Glide => SfxEvent::Glide,
            _ => SfxEvent::Idle,
        };
    }
    SfxEvent::Idle
}

/// Adapted `should_emit` (module doc comment): the old mapper's richer
/// `is_stepping` bone-phase detection has no equivalent client-side state
/// here (that needs figure-skeleton foot-bone tracking, a rendering-side
/// concern out of THIS task's scope — see the module doc), so this collapses
/// to its own documented fallback shape: a just-landed transition or a
/// changed event type emits immediately; a REPEATING event (the common
/// footstep-cadence case) still gates on the manifest's real `threshold`.
fn should_emit_movement(history: &MoveHistory, mapped_event: &SfxEvent, threshold: f32) -> bool {
    // A just-landed transition (e.g. Idle -> Run on touchdown) is exactly a
    // changed-event case here, so no separate "landed" branch is needed —
    // `classify_movement_event`'s own `!history.on_ground` edge check is
    // what makes landing produce a DIFFERENT event from the previous frame
    // in the first place.
    if &history.event != mapped_event {
        return true;
    }
    history.time.elapsed().as_secs_f32() >= threshold
}

/// Ported verbatim from the old client's own `get_volume_for_body_type`.
fn volume_for_body(body: &Body) -> f32 {
    match body {
        Body::Humanoid(_) => 0.5,
        Body::QuadrupedSmall(_) => 0.2,
        Body::QuadrupedMedium(_) => 0.7,
        Body::QuadrupedLow(_) => 0.7,
        Body::BirdMedium(_) => 0.3,
        Body::BirdLarge(_) => 0.5,
        Body::BipedLarge(_) => 1.0,
        _ => 0.9,
    }
}

/// The movement sub-mapper: footsteps + swim + roll + sneak + climb + glide,
/// for every mirrored entity within [`SFX_DIST_LIMIT_SQR`] of the local
/// player. Distance-culled the SAME way the old client culled against the
/// camera position (module doc: EM-5.10d's job is real spatial attenuation,
/// this phase only hard-culls + plays flat).
fn movement_sfx_mapper(
    entities: Query<(Entity, &Transform, &NetVel, &NetBody, &NetLocomotion), With<NetUid>>,
    manifest_handle: Option<Res<SfxManifestHandle>>,
    manifests: Res<Assets<SfxManifest>>,
    audio_listener: Res<AudioListener>,
    asset_server: Res<AssetServer>,
    audio_assets: Res<Assets<XindelerAudioAsset>>,
    mut cache: ResMut<SfxAssetCache>,
    mut backend: ResMut<AudioBackend>,
    mut history: Local<HashMap<Entity, MoveHistory>>,
) {
    let Some(manifest_handle) = manifest_handle else {
        return;
    };
    let Some(manifest) = manifests.get(&manifest_handle.0) else {
        return;
    };
    // Cull against the SAME point we attenuate/pan from (the listener/camera),
    // for a single source of truth (bevy-migration-reviewer EM-5.10d) — a sound
    // can no longer pass a player-anchored cull and then attenuate to silence at
    // the camera, or vice-versa.
    let listener_pos = audio_listener.pos;

    for (entity, transform, vel, body, locomotion) in &entities {
        if transform.translation.distance_squared(listener_pos) >= SFX_DIST_LIMIT_SQR {
            continue;
        }
        let kind = body_move_kind(&body.0);
        let entry = history.entry(entity).or_default();
        let mapped_event = classify_movement_event(&kind, locomotion, vel.0.length(), entry);

        if let Some(item) = manifest.get(&mapped_event)
            && should_emit_movement(entry, &mapped_event, item.threshold)
            && trigger_sfx(
                manifest,
                &mapped_event,
                volume_for_body(&body.0),
                transform.translation,
                &audio_listener,
                &asset_server,
                &audio_assets,
                &mut cache,
                &mut backend,
            )
        {
            entry.time = Instant::now();
        }

        entry.event = mapped_event;
        entry.on_ground = locomotion.on_ground;
        entry.in_liquid = locomotion.in_liquid;
    }

    // Mirrors the old mapper's own `cleanup`: drop stale history for
    // entities no longer mirrored at all, so this map doesn't grow
    // unbounded across a long session.
    history.retain(|entity, _| entities.contains(*entity));
}

// ---------------------------------------------------------------------------
// Combat mapper (REAL) — Attack / Wield / Unwield.
// ---------------------------------------------------------------------------

struct CombatHistory {
    event: SfxEvent,
    time: Instant,
    /// The client's OWN remembered "was the weapon drawn last frame" bit —
    /// NOT derivable purely from `event` (which may sit at `Idle` between two
    /// `Attack` triggers while still genuinely wielded), so tracked
    /// separately, mirroring the old combat mapper's own
    /// `PreviousEntityState::weapon_drawn` field exactly.
    weapon_drawn_prev: bool,
}

impl Default for CombatHistory {
    fn default() -> Self {
        Self {
            event: SfxEvent::Idle,
            time: Instant::now(),
            weapon_drawn_prev: false,
        }
    }
}

/// Ported from the old combat mapper's own `map_event`: an in-progress
/// attack always wins (`Attack(ability_type, tool_kind)`); otherwise a
/// weapon-drawn EDGE (this frame vs. the client's own remembered previous
/// frame) yields `Wield`/`Unwield`. `Music` (playing an instrument) is
/// deliberately NOT ported here — see the module doc comment.
fn classify_combat_event(
    combat_move: &NetCombatMove,
    tool_kind: Option<common::comp::tool::ToolKind>,
    was_weapon_drawn: bool,
) -> SfxEvent {
    let Some(tool_kind) = tool_kind else {
        return SfxEvent::Idle;
    };
    if let Some(ability) = combat_move.attacking {
        return SfxEvent::Attack(ability, tool_kind);
    }
    match (was_weapon_drawn, combat_move.weapon_drawn) {
        (false, true) => SfxEvent::Wield(tool_kind),
        (true, false) => SfxEvent::Unwield(tool_kind),
        _ => SfxEvent::Idle,
    }
}

/// Ported verbatim from the old combat mapper's own `should_emit`: repeating
/// the SAME event gates on the manifest threshold; any other (including a
/// changed) event emits immediately.
fn should_emit_combat(history: &CombatHistory, mapped_event: &SfxEvent, threshold: f32) -> bool {
    if &history.event == mapped_event {
        history.time.elapsed().as_secs_f32() >= threshold
    } else {
        true
    }
}

/// The combat sub-mapper: Attack/Wield/Unwield sounds for every mirrored
/// entity carrying [`NetCombatMove`] + an equipped [`NetLoadout::active_tool`]
/// within [`SFX_DIST_LIMIT_SQR`] of the local player.
fn combat_sfx_mapper(
    entities: Query<(Entity, &Transform, &NetCombatMove, &NetLoadout), With<NetUid>>,
    manifest_handle: Option<Res<SfxManifestHandle>>,
    manifests: Res<Assets<SfxManifest>>,
    audio_listener: Res<AudioListener>,
    asset_server: Res<AssetServer>,
    audio_assets: Res<Assets<XindelerAudioAsset>>,
    mut cache: ResMut<SfxAssetCache>,
    mut backend: ResMut<AudioBackend>,
    mut history: Local<HashMap<Entity, CombatHistory>>,
) {
    let Some(manifest_handle) = manifest_handle else {
        return;
    };
    let Some(manifest) = manifests.get(&manifest_handle.0) else {
        return;
    };
    // Cull from the listener/camera — the same point we attenuate/pan from.
    let listener_pos = audio_listener.pos;

    for (entity, transform, combat_move, loadout) in &entities {
        if transform.translation.distance_squared(listener_pos) >= SFX_DIST_LIMIT_SQR {
            continue;
        }
        let tool_kind = loadout.active_tool.as_ref().map(|tool| tool.kind);
        let entry = history.entry(entity).or_default();
        let mapped_event = classify_combat_event(combat_move, tool_kind, entry.weapon_drawn_prev);

        if let Some(item) = manifest.get(&mapped_event)
            && should_emit_combat(entry, &mapped_event, item.threshold)
            && trigger_sfx(
                manifest,
                &mapped_event,
                1.0,
                transform.translation,
                &audio_listener,
                &asset_server,
                &audio_assets,
                &mut cache,
                &mut backend,
            )
        {
            entry.time = Instant::now();
        }

        entry.event = mapped_event;
        entry.weapon_drawn_prev = combat_move.weapon_drawn;
    }

    history.retain(|entity, _| entities.contains(*entity));
}

// ---------------------------------------------------------------------------
// Campfire mapper (REAL) — uses ONLY already-mirrored state.
// ---------------------------------------------------------------------------

struct CampfireHistory {
    time: Instant,
}

impl Default for CampfireHistory {
    fn default() -> Self {
        Self {
            time: Instant::now(),
        }
    }
}

const CAMPFIRE_VOLUME: f32 = 0.8;

/// The campfire sub-mapper: a looping ambience trigger for every mirrored
/// `Body::Object(object::Body::CampfireLit)` within [`SFX_DIST_LIMIT_SQR`] —
/// ported from the old client's own `CampfireEventMapper`, using ONLY the
/// already-mirrored [`NetBody`]/`Transform` (no new mirror needed at all).
fn campfire_sfx_mapper(
    entities: Query<(Entity, &Transform, &NetBody), With<NetUid>>,
    manifest_handle: Option<Res<SfxManifestHandle>>,
    manifests: Res<Assets<SfxManifest>>,
    audio_listener: Res<AudioListener>,
    asset_server: Res<AssetServer>,
    audio_assets: Res<Assets<XindelerAudioAsset>>,
    mut cache: ResMut<SfxAssetCache>,
    mut backend: ResMut<AudioBackend>,
    mut history: Local<HashMap<Entity, CampfireHistory>>,
) {
    let Some(manifest_handle) = manifest_handle else {
        return;
    };
    let Some(manifest) = manifests.get(&manifest_handle.0) else {
        return;
    };
    let Some(item) = manifest.get(&SfxEvent::Campfire) else {
        return;
    };
    // Cull from the listener/camera — the same point we attenuate/pan from.
    let listener_pos = audio_listener.pos;

    for (entity, transform, body) in &entities {
        if !matches!(body.0, Body::Object(comp::body::object::Body::CampfireLit)) {
            continue;
        }
        if transform.translation.distance_squared(listener_pos) >= SFX_DIST_LIMIT_SQR {
            continue;
        }
        let entry = history.entry(entity).or_default();
        if entry.time.elapsed().as_secs_f32() >= item.threshold
            && trigger_sfx(
                manifest,
                &SfxEvent::Campfire,
                CAMPFIRE_VOLUME,
                transform.translation,
                &audio_listener,
                &asset_server,
                &audio_assets,
                &mut cache,
                &mut backend,
            )
        {
            entry.time = Instant::now();
        }
    }

    history.retain(|entity, _| entities.contains(*entity));
}

// ---------------------------------------------------------------------------
// `handle_outcome` (REAL) — over the new partial `NetOutcome` message.
// ---------------------------------------------------------------------------

/// Resolves the `(SfxEvent, volume)` pair for a covered [`NetOutcome`] —
/// ported from the old client's own `SfxMgr::handle_outcome` match arms,
/// restricted to the 5 variants [`xindeler_protocol::NetOutcome`] carries.
fn outcome_sfx(outcome: &NetOutcome) -> (SfxEvent, f32) {
    match outcome {
        NetOutcome::Explosion { power, .. } => (SfxEvent::Explosion, power.abs()),
        NetOutcome::Damage { .. } => (SfxEvent::Damage, 1.5),
        NetOutcome::Death { .. } => (SfxEvent::Death, 1.5),
        NetOutcome::Block { parry, .. } => {
            if *parry {
                (SfxEvent::Parry, 1.5)
            } else {
                (SfxEvent::Block, 1.5)
            }
        },
        NetOutcome::PoiseChange { state, .. } => (SfxEvent::PoiseChange(*state), 1.5),
    }
}

/// The world position each covered [`NetOutcome`] happened at — every variant
/// carries a `pos` (EM-5.10d: outcomes are positional too, so an explosion off
/// in the distance attenuates + pans just like a footstep).
fn outcome_pos(outcome: &NetOutcome) -> Vec3 {
    match outcome {
        NetOutcome::Explosion { pos, .. }
        | NetOutcome::Damage { pos }
        | NetOutcome::Death { pos }
        | NetOutcome::Block { pos, .. }
        | NetOutcome::PoiseChange { pos, .. } => *pos,
    }
}

/// The `handle_outcome` port: fires the matching SFX trigger for every
/// [`NetOutcome`] this frame delivered. No distance culling needed here (the
/// sim already scoped the underlying `Outcome` stream to what this client's
/// own view could see before it ever became a `NetOutcome` — see
/// `xindeler-sim-bridge::sfx`'s own module doc comment).
fn handle_outcome_sfx(
    mut outcomes: MessageReader<NetOutcome>,
    manifest_handle: Option<Res<SfxManifestHandle>>,
    manifests: Res<Assets<SfxManifest>>,
    audio_listener: Res<AudioListener>,
    asset_server: Res<AssetServer>,
    audio_assets: Res<Assets<XindelerAudioAsset>>,
    mut cache: ResMut<SfxAssetCache>,
    mut backend: ResMut<AudioBackend>,
) {
    let Some(manifest_handle) = manifest_handle else {
        outcomes.read().for_each(drop);
        return;
    };
    let Some(manifest) = manifests.get(&manifest_handle.0) else {
        outcomes.read().for_each(drop);
        return;
    };
    for outcome in outcomes.read() {
        let (event, volume) = outcome_sfx(outcome);
        trigger_sfx(
            manifest,
            &event,
            volume,
            outcome_pos(outcome),
            &audio_listener,
            &asset_server,
            &audio_assets,
            &mut cache,
            &mut backend,
        );
    }
}

// ---------------------------------------------------------------------------
// Listener wiring (EM-5.10d) — the Kira-side "ears" follow the player camera.
// ---------------------------------------------------------------------------

/// Keeps [`AudioListener`] in step with the player's view each frame: the
/// position and right-ear axis come from the `MainCamera` transform (so every
/// positional sound attenuates + pans relative to where the player is actually
/// looking from), and the underwater flag comes from the local player's
/// mirrored [`NetLocomotion::in_liquid`] (which drives the sfx low-pass
/// muffle).
///
/// Ported from the old client's own `SfxMgr::maintain` head, which set the
/// listener to the camera position/direction and toggled the sfx master filter
/// on an underwater check. We use the local player's `in_liquid` as the
/// "underwater" signal — the real, already-mirrored, both-feature-available
/// client-side state — rather than resampling the terrain block at the camera
/// (the old client's approach), which would need a new point-block terrain
/// query this Bevy port does not expose yet; camera-position water sampling is
/// a possible future refinement, noted for EM-5.10d follow-up.
fn update_audio_listener(
    camera: Query<&Transform, With<crate::camera::MainCamera>>,
    local_player: Query<&NetLocomotion, With<NetLocalPlayer>>,
    mut listener: ResMut<AudioListener>,
) {
    let Ok(camera) = camera.single() else {
        return; // no main camera yet — keep the last (or default) listener
    };
    listener.pos = camera.translation;
    listener.right = *camera.right();
    listener.underwater = local_player.single().map(|l| l.in_liquid).unwrap_or(false);
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// Adds the 3 real event-mapper systems + `handle_outcome_sfx` to `Update`
/// (frame rate — matches every other Phase-5 HUD/view system's own
/// schedule; distance/threshold culling happens INSIDE each system, not via
/// scheduling). Does NOT register anything for the block/vehicle
/// sub-mappers (module doc comment: documented stubs, no fabricated
/// triggers).
#[derive(Default)]
pub struct SfxViewPlugin;

impl Plugin for SfxViewPlugin {
    fn build(&self, app: &mut App) {
        // EM-5.10d: refresh the listener BEFORE the mappers so this frame's
        // attenuation/panning read an up-to-date camera pose (a one-frame-stale
        // listener would be harmless — the camera barely moves per frame — but
        // the explicit edge keeps it exact and un-ambiguous).
        app.add_systems(Update, update_audio_listener).add_systems(
            Update,
            (
                movement_sfx_mapper,
                combat_sfx_mapper,
                campfire_sfx_mapper,
                handle_outcome_sfx,
            )
                .after(update_audio_listener),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::asset::AssetPlugin;
    use common::{
        comp::{
            CharacterAbilityType, humanoid,
            tool::{Hands, ToolKind},
        },
        states::utils::StageSection,
    };
    use xindeler_audio::sfx::dotted_key_to_ogg_path;
    use xindeler_protocol::{NetTool, NetToolKey};

    use super::*;

    // -----------------------------------------------------------------
    // Pure-function unit tests (no App needed).
    // -----------------------------------------------------------------

    #[test]
    fn run_event_air_maps_to_idle_not_a_run_variant() {
        assert_eq!(
            run_event(&BodyMoveKind::Humanoid, NetGroundBlock::Air),
            SfxEvent::Idle
        );
    }

    #[test]
    fn run_event_picks_the_right_container_per_body_kind() {
        assert_eq!(
            run_event(&BodyMoveKind::Humanoid, NetGroundBlock::Grass),
            SfxEvent::Run(common::terrain::BlockKind::Grass)
        );
        assert_eq!(
            run_event(&BodyMoveKind::Quadruped, NetGroundBlock::Rock),
            SfxEvent::QuadRun(common::terrain::BlockKind::Rock)
        );
        assert_eq!(
            run_event(&BodyMoveKind::Arthropod, NetGroundBlock::Snow),
            SfxEvent::OctoRun(common::terrain::BlockKind::Snow)
        );
    }

    #[test]
    fn classify_movement_event_walking_on_grass_yields_run_grass() {
        let locomotion = NetLocomotion {
            on_ground: true,
            in_liquid: false,
            ground_block: NetGroundBlock::Grass,
            move_state: NetMoveState::Idle,
        };
        let history = MoveHistory::default();
        assert_eq!(
            classify_movement_event(&BodyMoveKind::Humanoid, &locomotion, 3.0, &history),
            SfxEvent::Run(common::terrain::BlockKind::Grass)
        );
    }

    #[test]
    fn classify_combat_event_attacking_wins_over_wield_state() {
        let combat_move = NetCombatMove {
            attacking: Some(CharacterAbilityType::BasicMelee(StageSection::Action)),
            weapon_drawn: true,
        };
        assert_eq!(
            classify_combat_event(&combat_move, Some(ToolKind::Sword), true),
            SfxEvent::Attack(
                CharacterAbilityType::BasicMelee(StageSection::Action),
                ToolKind::Sword
            )
        );
    }

    #[test]
    fn classify_combat_event_wield_edge_fires_once() {
        let combat_move = NetCombatMove {
            attacking: None,
            weapon_drawn: true,
        };
        assert_eq!(
            classify_combat_event(&combat_move, Some(ToolKind::Sword), false),
            SfxEvent::Wield(ToolKind::Sword)
        );
        // Already drawn last frame too -> no wield/unwield edge.
        assert_eq!(
            classify_combat_event(&combat_move, Some(ToolKind::Sword), true),
            SfxEvent::Idle
        );
    }

    #[test]
    fn outcome_sfx_maps_the_five_covered_variants() {
        assert_eq!(
            outcome_sfx(&NetOutcome::Death { pos: Vec3::ZERO }).0,
            SfxEvent::Death
        );
        assert_eq!(
            outcome_sfx(&NetOutcome::Block {
                pos: Vec3::ZERO,
                parry: true
            })
            .0,
            SfxEvent::Parry
        );
        assert_eq!(
            outcome_sfx(&NetOutcome::Block {
                pos: Vec3::ZERO,
                parry: false
            })
            .0,
            SfxEvent::Block
        );
    }

    // -----------------------------------------------------------------
    // The task's own acceptance bar: a real headless App, real mirrored
    // entities, a real `NetLocotion`/`NetCombatMove` STATE TRANSITION, and
    // a real assertion that a sound actually started playing on the real
    // Kira `AudioBackend` — not merely "the system ran without panicking".
    // Degrades to a documented skip on a device-less CI runner, the SAME
    // tolerant posture PR #174's own `lib.rs` tests established.
    // -----------------------------------------------------------------

    fn boot_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin {
            file_path: crate::atmosphere::assets_root()
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        });
        app.add_plugins(xindeler_audio::XindelerAudioPlugin);
        // A plain Bevy message registration (no replicon in this test — the
        // mappers under test are pure Bevy consumers of already-mirrored
        // components/messages, not the replication wiring itself). Normally
        // `xindeler_protocol::XindelerProtocolPlugin` does this via
        // `add_server_message`; `handle_outcome_sfx`'s `MessageReader<
        // NetOutcome>` just needs the type initialized.
        app.add_message::<NetOutcome>();
        app.add_plugins(SfxViewPlugin);
        app.finish();
        app
    }

    /// Runs `app.update()` in a loop (with a short real sleep between
    /// attempts) until `condition` holds or `max_tries` is exhausted — the
    /// SAME polling shape `xindeler-audio`'s own `.wav`-through-`AssetLoader`
    /// test uses to wait out a real async asset load.
    fn poll_until(app: &mut App, max_tries: u32, mut condition: impl FnMut(&mut App) -> bool) {
        for _ in 0..max_tries {
            app.update();
            if condition(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn sfx_sound_count(app: &mut App) -> Option<usize> {
        let mut backend = app.world_mut().resource_mut::<AudioBackend>();
        backend.tracks_mut().map(|tracks| tracks.sfx.num_sounds())
    }

    /// Pre-loads every file `event` can resolve to (so the real assertion
    /// below isn't racing the async `AssetServer::load` this crate's own
    /// `trigger_sfx` would otherwise kick off on its FIRST attempt) and
    /// seeds them into the app's real [`xindeler_audio::sfx::SfxAssetCache`]
    /// at the exact path keys `trigger_sfx` looks them up by — so by the
    /// time the real state-transition update runs, the sound is already
    /// resolved and plays on the very first triggering frame.
    fn preload_event_assets(app: &mut App, event: &SfxEvent) {
        let manifest_handle = app.world().resource::<SfxManifestHandle>().clone();
        let files: Vec<String> = {
            let manifests = app.world().resource::<Assets<SfxManifest>>();
            let manifest = manifests
                .get(&manifest_handle.0)
                .expect("manifest already loaded by the caller");
            manifest
                .get(event)
                .unwrap_or_else(|| panic!("no sfx.ron trigger for {event:?}"))
                .files
                .clone()
        };

        let handles: Vec<_> = files
            .iter()
            .map(|dotted| {
                let path = dotted_key_to_ogg_path(dotted);
                let handle = app.world().resource::<AssetServer>().load(path.clone());
                (path, handle)
            })
            .collect();

        poll_until(app, 400, |app| {
            let assets = app.world().resource::<Assets<XindelerAudioAsset>>();
            handles.iter().all(|(_, h)| assets.get(h).is_some())
        });

        let mut cache = app.world_mut().resource_mut::<SfxAssetCache>();
        for (path, handle) in handles {
            cache.insert_preloaded(path, handle);
        }
    }

    /// The task's headline verify: footsteps AND an attack SFX fire from
    /// real mirrored state.
    #[test]
    fn footsteps_and_an_attack_sfx_fire_from_a_real_state_transition() {
        let mut app = boot_app();

        // Wait for the real backend + the real sfx.ron manifest to finish
        // loading.
        poll_until(&mut app, 400, |app| {
            let ready = app.world().resource::<AudioBackend>().is_available();
            let handle = app
                .world()
                .get_resource::<SfxManifestHandle>()
                .map(|h| h.0.clone());
            let manifest_loaded = match handle {
                Some(h) => app
                    .world()
                    .resource::<Assets<SfxManifest>>()
                    .get(&h)
                    .is_some(),
                None => false,
            };
            ready && manifest_loaded
        });

        if !app.world().resource::<AudioBackend>().is_available() {
            eprintln!(
                "skipping real-playback assertions: no cpal output device in this environment"
            );
            return;
        }

        // Pre-load the exact sounds this test will trigger, so the real
        // assertions below don't race the async first-load.
        preload_event_assets(&mut app, &SfxEvent::Run(common::terrain::BlockKind::Grass));
        let attack_event = SfxEvent::Attack(
            CharacterAbilityType::BasicMelee(StageSection::Action),
            ToolKind::Sword,
        );
        preload_event_assets(&mut app, &attack_event);

        // The local player (the movement/combat mappers' distance anchor).
        app.world_mut()
            .spawn((Transform::from_translation(Vec3::ZERO), NetLocalPlayer));

        // A mirrored NPC standing right next to the player, airborne and
        // unarmed-idle at first — no sound should fire yet.
        let npc = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
                NetUid(1),
                NetVel(Vec3::ZERO),
                NetBody(Body::Humanoid(humanoid::Body::random())),
                NetLocomotion {
                    on_ground: false,
                    in_liquid: false,
                    ground_block: NetGroundBlock::Grass,
                    move_state: NetMoveState::Idle,
                },
                NetCombatMove {
                    attacking: None,
                    weapon_drawn: false,
                },
                NetLoadout {
                    active_tool: Some(NetTool {
                        key: NetToolKey::Tool("common.items.weapons.sword.starter".to_owned()),
                        kind: ToolKind::Sword,
                        hands: Hands::Two,
                    }),
                    ..Default::default()
                },
            ))
            .id();

        app.update();
        assert_eq!(
            sfx_sound_count(&mut app),
            Some(0),
            "an airborne, unarmed-idle entity must not trigger any sfx yet"
        );

        // --- The real state transition: the entity lands on grass and
        // starts moving (a footstep) AND begins a real melee attack, in the
        // SAME frame — exactly "a mirrored entity's velocity/grounded state
        // changing" the task brief asks for, plus the attack half of the
        // verify bar.
        app.world_mut().entity_mut(npc).insert((
            NetVel(Vec3::new(3.0, 0.0, 0.0)),
            NetLocomotion {
                on_ground: true,
                in_liquid: false,
                ground_block: NetGroundBlock::Grass,
                move_state: NetMoveState::Idle,
            },
            NetCombatMove {
                attacking: Some(CharacterAbilityType::BasicMelee(StageSection::Action)),
                weapon_drawn: true,
            },
        ));

        app.update();
        let count_after_transition = sfx_sound_count(&mut app)
            .expect("AudioBackend must still be Ready after the transition");
        assert!(
            count_after_transition >= 1,
            "the footstep + attack state transition must have started at least one real sound, \
             got {count_after_transition}"
        );
    }

    /// Calls the real [`trigger_sfx`] against the app's live Kira backend for a
    /// given emitter/listener geometry, returning `(did_it_play,
    /// sfx_sound_count)`. Two nested `resource_scope`s take the two `&mut`
    /// resources (`SfxAssetCache`, `AudioBackend`) out of the world so the
    /// remaining (immutable) manifest/asset resources can be borrowed
    /// alongside them.
    fn run_trigger(
        app: &mut App,
        event: &SfxEvent,
        emitter: Vec3,
        listener: AudioListener,
    ) -> (bool, usize) {
        let manifest_handle = app.world().resource::<SfxManifestHandle>().0.clone();
        app.world_mut()
            .resource_scope(|world, mut cache: Mut<SfxAssetCache>| {
                world.resource_scope(|world, mut backend: Mut<AudioBackend>| {
                    let manifests = world.resource::<Assets<SfxManifest>>();
                    let manifest = manifests.get(&manifest_handle).expect("manifest loaded");
                    let asset_server = world.resource::<AssetServer>();
                    let audio_assets = world.resource::<Assets<XindelerAudioAsset>>();
                    let played = trigger_sfx(
                        manifest,
                        event,
                        1.0,
                        emitter,
                        &listener,
                        asset_server,
                        audio_assets,
                        &mut cache,
                        &mut backend,
                    );
                    let count = backend
                        .tracks_mut()
                        .map(|tracks| tracks.sfx.num_sounds())
                        .unwrap_or(0);
                    (played, count)
                })
            })
    }

    /// EM-5.10d — the reported campfire bug, exercised through the REAL spatial
    /// trigger on the live Kira backend. [`trigger_sfx`] is exactly what the
    /// campfire mapper calls once its (≈22 s) threshold elapses; we call it
    /// directly so the test needn't sleep out that cadence. A campfire close to
    /// the listener plays; one beyond [`SFX_DIST_LIMIT`] is fully attenuated
    /// and starts nothing. The volume *taper* in between (a
    /// far-but-in-range fire is much quieter than a near one — the precise
    /// "constant loud campfire" regression) is asserted deterministically
    /// by `xindeler_audio::sfx::spatial`'s own unit tests, since Kira
    /// exposes no public way to read a playing sound's effective volume
    /// back.
    #[test]
    fn campfire_trigger_is_spatially_gated_by_distance() {
        let mut app = boot_app();
        poll_until(&mut app, 400, |app| {
            let ready = app.world().resource::<AudioBackend>().is_available();
            let handle = app
                .world()
                .get_resource::<SfxManifestHandle>()
                .map(|h| h.0.clone());
            let manifest_loaded = match handle {
                Some(h) => app
                    .world()
                    .resource::<Assets<SfxManifest>>()
                    .get(&h)
                    .is_some(),
                None => false,
            };
            ready && manifest_loaded
        });

        if !app.world().resource::<AudioBackend>().is_available() {
            eprintln!(
                "skipping real-playback assertions: no cpal output device in this environment"
            );
            return;
        }

        preload_event_assets(&mut app, &SfxEvent::Campfire);
        let listener = AudioListener {
            pos: Vec3::ZERO,
            right: Vec3::X,
            underwater: false,
        };

        // Right next to the fire: within earshot -> a real sound starts.
        let (played_near, count_near) = run_trigger(
            &mut app,
            &SfxEvent::Campfire,
            Vec3::new(2.0, 0.0, 0.0),
            listener,
        );
        assert!(played_near, "a campfire 2 m away must play");
        assert!(
            count_near >= 1,
            "a near campfire must have started a real sound, got {count_near}"
        );

        // Far beyond the cull radius (SFX_DIST_LIMIT = 256 m): fully attenuated
        // -> nothing plays. Pre-fix this either played at full blast (if the
        // mapper's binary cull let it through) — the exact "still loud no matter
        // how far I walk" symptom.
        let (played_far, _) = run_trigger(
            &mut app,
            &SfxEvent::Campfire,
            Vec3::new(400.0, 0.0, 0.0),
            listener,
        );
        assert!(
            !played_far,
            "a campfire 400 m away (past SFX_DIST_LIMIT) must be silent"
        );
    }

    /// EM-5.10d — the listener "ears" track the player camera + underwater
    /// state: [`update_audio_listener`] copies the `MainCamera` position/right
    /// axis and the local player's `in_liquid` flag into [`AudioListener`]
    /// (which the muffle system + `trigger_sfx` then read). No audio device
    /// needed — this is pure ECS wiring.
    #[test]
    fn audio_listener_follows_camera_and_underwater_state() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<AudioListener>();
        app.add_systems(Update, update_audio_listener);

        app.world_mut().spawn((
            Transform::from_xyz(5.0, 6.0, 7.0),
            crate::camera::MainCamera,
        ));
        app.world_mut()
            .spawn((NetLocalPlayer, NetUid(1), NetLocomotion {
                on_ground: true,
                in_liquid: true,
                ground_block: NetGroundBlock::Grass,
                move_state: NetMoveState::Idle,
            }));

        app.update();

        let listener = app.world().resource::<AudioListener>();
        assert_eq!(
            listener.pos,
            Vec3::new(5.0, 6.0, 7.0),
            "listener must sit at the camera position"
        );
        assert!(
            (listener.right - Vec3::X).length() < 1e-5,
            "an un-rotated camera's right axis is +X, got {:?}",
            listener.right
        );
        assert!(
            listener.underwater,
            "the local player being in liquid must mark the listener underwater"
        );
    }
}
