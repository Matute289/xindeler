//! BL-82 EM-5.10b (T56.35) — mirrors the sim's `PhysicsState`/
//! `CharacterState` down to [`xindeler_protocol::NetLocomotion`]/
//! [`xindeler_protocol::NetCombatMove`], the exact fields
//! `xindeler-client::sfx`'s movement/combat event mappers need, following the
//! SAME `NetHealth`/`NetLoadout` pattern [`crate::mirror_sim_entities`]
//! established — a NEW, separate system (like [`crate::combat_hud`]), not
//! folded into that already-huge function. Extended by BL-82 EM-5.10e
//! (T56.37) to also mirror [`xindeler_protocol::NetInstrumentMove`] — the
//! same `CharacterState`/`Inventory` read, one more UPSERT.
//!
//! Project, don't dump (spec §3.2): [`xindeler_protocol::NetMoveState`] is
//! the `CharacterState` CLASSIFICATION (Roll/RollCancel/Sneak/Climb/Glide/
//! Idle), computed server-side exactly the way `crate::is_gliding` already
//! pre-flattens `CharacterState` for `NetLoadout::gliding` — the client never
//! sees a raw `CharacterState`. `on_ground`/`in_liquid`/`ground_block` come
//! straight off `PhysicsState` (no terrain lookup needed: `PhysicsState.
//! on_ground` already IS the exact `Block` the entity is standing on).

use bevy::{
    app::{App, FixedUpdate, Plugin, Update},
    ecs::{
        change_detection::NonSendMut,
        message::MessageWriter,
        schedule::IntoScheduleConfigs,
        system::{Commands, Res},
    },
};
use bevy_replicon::prelude::{SendTargets, ToClients};
use common::comp::{
    CharacterAbilityType, CharacterState, Inventory, PhysicsState,
    inventory::{item::tool::AbilitySpec, slot::EquipSlot},
};
use specs::WorldExt;
use xindeler_protocol::{
    NetCombatMove, NetGroundBlock, NetInstrumentMove, NetLocomotion, NetMoveState, NetOutcome,
};

use crate::{
    EmbeddedPlayer, SimMirror, SimServer, mirror_sim_entities, player::tick_player, tick_sim,
};

/// Ported verbatim from the old combat mapper's own `weapon_drawn` helper
/// (`voxygen/src/audio/sfx/event_mapper/combat/mod.rs`): a wielded OR
/// mid-equip state counts as "weapon drawn".
fn weapon_drawn(character_state: &CharacterState) -> bool {
    character_state.is_wield() || matches!(character_state, CharacterState::Equipping { .. })
}

/// Classifies `character_state` into the [`NetMoveState`] category the
/// client-side movement mapper recombines with `on_ground`/`in_liquid`/
/// `Vel` every frame (see [`xindeler_protocol::NetMoveState`]'s doc comment
/// for why physics stays a separate field). Ported from the old client's
/// `MovementEventMapper::map_movement_event`'s own `Roll`/`Sneak`/`Climb`/
/// `Glide` arms (the `Run(BlockKind)`/`Swim` arms are physics-gated, so they
/// live in the CLIENT's own recombination, not here).
fn classify_move_state(character_state: &CharacterState) -> NetMoveState {
    if let CharacterState::Roll(data) = character_state {
        return if data.static_data.was_cancel {
            NetMoveState::RollCancel
        } else {
            NetMoveState::Roll
        };
    }
    if character_state.is_stealthy() {
        return NetMoveState::Sneak;
    }
    if matches!(character_state, CharacterState::Climb(_)) {
        return NetMoveState::Climb;
    }
    // Reuses the SAME `is_glide_wielded` predicate `crate::is_gliding` already
    // applies for `NetLoadout::gliding` — one classification, two consumers.
    if character_state.is_glide_wielded() {
        return NetMoveState::Glide;
    }
    NetMoveState::Idle
}

/// Resolves the [`AbilitySpec`] the instrument note-bank sub-mapper
/// (BL-82 EM-5.10e, T56.37) needs, if `character_state` is
/// `character_state.is_music()` — ported verbatim from the old combat
/// mapper's own `Music(ToolKind, AbilitySpec)` construction
/// (`voxygen/src/audio/sfx/event_mapper/combat/mod.rs::map_event`'s
/// `is_music()` arm): resolve the equip slot from
/// `character_state.ability_info().and_then(|info| info.hand)`, falling back
/// to `ActiveMainhand` (matching the old code's own `map_or`), then read that
/// item's `ability_spec()` off `inventory`. `None` while not playing, or if
/// the resolved item carries no `ability_spec` — both are a normal "stay
/// silent" case, never a panic/default.
fn playing_instrument(
    character_state: &CharacterState,
    inventory: Option<&Inventory>,
) -> Option<AbilitySpec> {
    if !character_state.is_music() {
        return None;
    }
    let equip_slot = character_state
        .ability_info()
        .and_then(|info| info.hand)
        .map_or(EquipSlot::ActiveMainhand, |hand| hand.to_equip_slot());
    inventory
        .and_then(|inventory| inventory.equipped(equip_slot))
        .and_then(|item| item.ability_spec())
        .map(|spec| spec.into_owned())
}

/// Reads the sim's `PhysicsState`/`CharacterState`/`Inventory` for every
/// currently-mirrored entity ([`SimMirror`]) and UPSERTs [`NetLocomotion`]/
/// [`NetCombatMove`]/[`NetInstrumentMove`], removing them when the sim entity
/// no longer carries the underlying components — mirrors
/// `mirror_sim_entities`'s own `Some(h) => insert / None =>
/// remove::<NetHealth>()` shape.
///
/// A no-op (returns immediately) if no [`SimServer`] is booted yet — same
/// early-out every sibling mirror system uses.
pub fn mirror_locomotion_and_combat_state(
    sim: Option<NonSendMut<SimServer>>,
    mirror: Res<SimMirror>,
    mut commands: Commands,
) {
    let Some(sim) = sim else { return };

    let ecs = sim.server.state().ecs();
    let physics_states = ecs.read_storage::<PhysicsState>();
    let character_states = ecs.read_storage::<CharacterState>();
    let inventories = ecs.read_storage::<Inventory>();

    for (&sim_entity, &bevy_entity) in mirror.0.iter() {
        let mut ec = commands.entity(bevy_entity);

        match physics_states.get(sim_entity) {
            Some(physics) => {
                let ground_block = physics
                    .on_ground
                    .map(|block| NetGroundBlock::from_block_kind(block.kind()))
                    .unwrap_or_default();
                // `character_states.get(..)` may legitimately be absent (some
                // mirrored entities, e.g. simple scenery objects, carry
                // `PhysicsState` but no `CharacterState`) — default to
                // `Idle`, matching the old client's own "no character state"
                // fallthrough.
                let move_state = character_states
                    .get(sim_entity)
                    .map(classify_move_state)
                    .unwrap_or_default();
                ec.insert(NetLocomotion {
                    on_ground: physics.on_ground.is_some(),
                    in_liquid: physics.in_liquid().is_some(),
                    ground_block,
                    move_state,
                });
            },
            None => {
                ec.remove::<NetLocomotion>();
            },
        }

        match character_states.get(sim_entity) {
            Some(character_state) => {
                let attacking = character_state
                    .is_attack()
                    .then(|| CharacterAbilityType::from(character_state));
                ec.insert(NetCombatMove {
                    attacking,
                    weapon_drawn: weapon_drawn(character_state),
                });
                ec.insert(NetInstrumentMove {
                    playing: playing_instrument(character_state, inventories.get(sim_entity)),
                });
            },
            None => {
                ec.remove::<NetCombatMove>();
                ec.remove::<NetInstrumentMove>();
            },
        }
    }
}

/// Registers [`mirror_locomotion_and_combat_state`] in `FixedUpdate`, after
/// `tick_sim`/`mirror_sim_entities` — the exact ordering
/// [`crate::CombatHudMirrorPlugin`] already establishes for its own sibling
/// mirror system.
pub struct SfxLocomotionMirrorPlugin;

impl Plugin for SfxLocomotionMirrorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            mirror_locomotion_and_combat_state
                .after(tick_sim)
                .after(mirror_sim_entities),
        );
    }
}

// ---------------------------------------------------------------------------
// `handle_outcome` port: captures the embedded player's own `Outcome` stream
// and broadcasts a partial `NetOutcome` projection (BL-82 EM-5.10b, T56.35).
// ---------------------------------------------------------------------------

/// Projects one sim [`common::outcome::Outcome`] onto the wire
/// [`NetOutcome`] shape, if it's one of the five variants this phase covers
/// (see [`xindeler_protocol::sfx`]'s module doc comment for the "why only
/// 5 of ~40" reasoning) — `None` for everything else, a normal, silent
/// "not covered yet" case, not an error.
///
/// Positions are converted through [`crate::sim_pos_to_bevy`] — the SAME
/// vek→glam axis conversion every other `Net*` position field uses.
fn project_outcome(outcome: &common::outcome::Outcome) -> Option<NetOutcome> {
    use common::{combat::DamageSource, comp::Health, outcome::Outcome};

    match outcome {
        Outcome::Explosion { pos, power, .. } => Some(NetOutcome::Explosion {
            pos: crate::sim_pos_to_bevy(*pos),
            power: power.abs(),
        }),
        Outcome::HealthChange { pos, info } => {
            // Old client's own filter: negative (damage, not healing) AND not
            // buff-sourced (e.g. a poison tick stays silent).
            let is_damage = info.amount < Health::HEALTH_EPSILON
                && !matches!(info.cause, Some(DamageSource::Buff(_)));
            is_damage.then(|| NetOutcome::Damage {
                pos: crate::sim_pos_to_bevy(*pos),
            })
        },
        Outcome::Death { pos } => Some(NetOutcome::Death {
            pos: crate::sim_pos_to_bevy(*pos),
        }),
        Outcome::Block { pos, parry, .. } => Some(NetOutcome::Block {
            pos: crate::sim_pos_to_bevy(*pos),
            parry: *parry,
        }),
        Outcome::PoiseChange { pos, state } => {
            // Old client's own no-op arm: `Normal` never produces a sound.
            (*state != common::comp::poise::PoiseState::Normal).then(|| NetOutcome::PoiseChange {
                pos: crate::sim_pos_to_bevy(*pos),
                state: *state,
            })
        },
        _ => None,
    }
}

/// Drains every `Outcome` the embedded player's Client received this frame
/// (via [`EmbeddedPlayer::drain_pending_outcomes`]) and broadcasts each
/// COVERED one (see [`project_outcome`]) as a [`ToClients<NetOutcome>`]
/// (`SendTargets::All` — same posture [`crate::chat::broadcast_embedded_chat`]
/// already documents: safe today because a listen-server has exactly one
/// real participant). A no-op (and the pending queue simply never
/// accumulates) if either the sim or the embedded player doesn't exist yet.
///
/// ## ⚠️ Known gap: not wired into `xindeler-server-app` yet (disclosed, not
/// silently narrowed — same class of gap `ChatBridgePlugin`'s own module doc
/// comment discloses)
/// [`SfxOutcomeBridgePlugin`] is added ONLY by `xindeler-client::listen_server`
/// today. `xindeler-server-app` (the real dedicated multiplayer server) has no
/// [`crate::EmbeddedPlayer`] at all, so there is nothing for THIS mechanism to
/// read there — a genuinely-remote client's own `Outcome` stream would need a
/// different capture point (e.g. sniffing the sim's own outgoing
/// `ServerGeneral::Outcomes` per real connection), not a copy of this
/// listen-server-only shape. Tracked as a required follow-up before the
/// EM-5.13 cutover checklist, same class as EM-5.4/5.6/5.7/5.8's own disclosed
/// gaps.
pub fn broadcast_embedded_outcomes(
    player: Option<NonSendMut<EmbeddedPlayer>>,
    mut writer: MessageWriter<ToClients<NetOutcome>>,
) {
    let Some(mut player) = player else { return };
    for outcome in player.drain_pending_outcomes() {
        if let Some(net_outcome) = project_outcome(&outcome) {
            writer.write(ToClients {
                targets: SendTargets::All,
                message: net_outcome,
            });
        }
    }
}

/// Registers [`broadcast_embedded_outcomes`] in `Update` (frame rate,
/// matching [`tick_player`]'s own schedule — outcomes ride the embedded
/// Client's tick, not the sim's 30 Hz `FixedUpdate`, exactly like
/// [`crate::ChatBridgePlugin`]), `.after(tick_player)` so an outcome
/// captured THIS frame broadcasts the SAME frame.
pub struct SfxOutcomeBridgePlugin;

impl Plugin for SfxOutcomeBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, broadcast_embedded_outcomes.after(tick_player));
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, ecs::system::RunSystemOnce, prelude::MinimalPlugins};
    use common::states::utils::StageSection;
    use specs::Builder;

    use super::*;
    use crate::{SimServer, boot_test_server};

    fn new_app_with_sim(data_dir: &std::path::Path) -> App {
        let sim = boot_test_server(data_dir).expect("test server boots");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<SimMirror>();
        app.insert_non_send(sim);
        app
    }

    /// [`project_outcome`]: the 5 covered variants project correctly
    /// (including the two conditional ones — a healing/buff `HealthChange`
    /// and a `Normal` `PoiseChange` must NOT project), and an uncovered
    /// variant (e.g. `Lightning`) returns `None` — the documented v1 cut,
    /// not a silent panic/default.
    #[test]
    fn project_outcome_covers_exactly_the_documented_five_with_their_real_filters() {
        use common::{
            combat::DamageSource,
            comp::poise::PoiseState,
            outcome::{HealthChangeInfo, Outcome},
            uid::Uid,
        };
        use vek::Vec3 as SimVec3;

        let pos = SimVec3::new(1.0, 2.0, 3.0);
        let bevy_pos = crate::sim_pos_to_bevy(pos);

        assert_eq!(
            project_outcome(&Outcome::Explosion {
                pos,
                power: -2.0,
                radius: 1.0,
                is_attack: true,
                reagent: None,
            }),
            Some(NetOutcome::Explosion {
                pos: bevy_pos,
                power: 2.0,
            }),
            "power must be absolute-valued, matching the old client"
        );

        let damage_info = HealthChangeInfo {
            amount: -10.0,
            precise: false,
            target: Uid(std::num::NonZeroU64::new(1).unwrap()),
            by: None,
            cause: None,
            instance: 0,
        };
        assert_eq!(
            project_outcome(&Outcome::HealthChange {
                pos,
                info: damage_info,
            }),
            Some(NetOutcome::Damage { pos: bevy_pos })
        );

        let healing_info = HealthChangeInfo {
            amount: 10.0,
            ..damage_info
        };
        assert_eq!(
            project_outcome(&Outcome::HealthChange {
                pos,
                info: healing_info,
            }),
            None,
            "positive amount (healing) must not project a Damage sound"
        );

        let buff_damage_info = HealthChangeInfo {
            cause: Some(DamageSource::Buff(common::comp::buff::BuffKind::Burning)),
            ..damage_info
        };
        assert_eq!(
            project_outcome(&Outcome::HealthChange {
                pos,
                info: buff_damage_info,
            }),
            None,
            "buff-sourced damage (e.g. a poison tick) must stay silent, matching the old client"
        );

        assert_eq!(
            project_outcome(&Outcome::Death { pos }),
            Some(NetOutcome::Death { pos: bevy_pos })
        );

        assert_eq!(
            project_outcome(&Outcome::Block {
                pos,
                parry: true,
                uid: Uid(std::num::NonZeroU64::new(1).unwrap()),
            }),
            Some(NetOutcome::Block {
                pos: bevy_pos,
                parry: true,
            })
        );

        assert_eq!(
            project_outcome(&Outcome::PoiseChange {
                pos,
                state: PoiseState::Stunned,
            }),
            Some(NetOutcome::PoiseChange {
                pos: bevy_pos,
                state: PoiseState::Stunned,
            })
        );
        assert_eq!(
            project_outcome(&Outcome::PoiseChange {
                pos,
                state: PoiseState::Normal,
            }),
            None,
            "PoiseState::Normal must not project a sound, matching the old client's no-op arm"
        );

        assert_eq!(
            project_outcome(&Outcome::Lightning { pos }),
            None,
            "an uncovered variant must return None, not panic or fabricate a sound"
        );
    }

    #[test]
    fn no_mirrored_entities_is_a_harmless_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        app.world_mut()
            .run_system_once(mirror_locomotion_and_combat_state)
            .expect("system runs without a mirrored entity");
    }

    /// A grounded, non-attacking, non-wielding entity mirrors as `Idle`/
    /// grounded/no-attack — the baseline case the client's own footstep
    /// distance accumulator relies on.
    #[test]
    fn mirrors_a_grounded_idle_entity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let physics = PhysicsState {
                on_ground: Some(common::terrain::Block::new(
                    common::terrain::BlockKind::Grass,
                    Default::default(),
                )),
                ..Default::default()
            };
            ecs.create_entity()
                .with(physics)
                .with(CharacterState::default())
                .build()
        };

        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_locomotion_and_combat_state)
            .expect("system runs");
        app.update();

        let locomotion = app
            .world()
            .get::<NetLocomotion>(bevy_entity)
            .expect("NetLocomotion must be mirrored");
        assert!(locomotion.on_ground);
        assert!(!locomotion.in_liquid);
        assert_eq!(locomotion.ground_block, NetGroundBlock::Grass);
        assert_eq!(locomotion.move_state, NetMoveState::Idle);

        let combat_move = app
            .world()
            .get::<NetCombatMove>(bevy_entity)
            .expect("NetCombatMove must be mirrored");
        assert!(combat_move.attacking.is_none());
        assert!(!combat_move.weapon_drawn);
    }

    /// A `CharacterState::Roll` (not cancelled) mirrors as `NetMoveState::
    /// Roll`; a cancelled roll mirrors as `RollCancel` — the distinction the
    /// old client's own movement mapper draws.
    #[test]
    fn roll_and_roll_cancel_classify_distinctly() {
        use common::states::roll;

        let base_static = roll::StaticData {
            buildup_duration: Default::default(),
            movement_duration: Default::default(),
            recover_duration: Default::default(),
            roll_strength: 1.0,
            attack_immunities: dummy_attack_filters(),
            ability_info: dummy_ability_info(),
            was_cancel: false,
        };
        let normal_roll = CharacterState::Roll(roll::Data {
            static_data: base_static,
            timer: Default::default(),
            stage_section: StageSection::Movement,
            was_wielded: false,
            prev_aimed_dir: None,
            is_sneaking: false,
        });
        assert_eq!(classify_move_state(&normal_roll), NetMoveState::Roll);

        let cancelled_roll = CharacterState::Roll(roll::Data {
            static_data: roll::StaticData {
                was_cancel: true,
                ..base_static
            },
            timer: Default::default(),
            stage_section: StageSection::Movement,
            was_wielded: false,
            prev_aimed_dir: None,
            is_sneaking: false,
        });
        assert_eq!(
            classify_move_state(&cancelled_roll),
            NetMoveState::RollCancel
        );
    }

    /// An attacking `CharacterState` mirrors `NetCombatMove::attacking` as
    /// `Some(ability_type)`, matching the old combat mapper's own
    /// `character_state.is_attack()` → `CharacterAbilityType::from(..)` path.
    /// Uses `SelfBuff` (an `is_attack()`-true state per `CharacterState::
    /// is_attack`) as the simplest real attack-shaped state to construct —
    /// the exact ability TYPE tested is incidental, only that a real
    /// `is_attack()` state mirrors `Some(_)`, not `None`.
    #[test]
    fn attacking_character_state_mirrors_the_ability_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity()
                .with(PhysicsState::default())
                .with(dummy_self_buff_state())
                .build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_locomotion_and_combat_state)
            .expect("system runs");
        app.update();

        let combat_move = app
            .world()
            .get::<NetCombatMove>(bevy_entity)
            .expect("NetCombatMove must be mirrored");
        assert_eq!(combat_move.attacking, Some(CharacterAbilityType::SelfBuff));
    }

    // -----------------------------------------------------------------
    // Instrument note-bank sub-mapper (BL-82 EM-5.10e, T56.37).
    // -----------------------------------------------------------------

    /// A non-`Music` `CharacterState` never resolves an [`AbilitySpec`],
    /// regardless of what's equipped — the pure-function guard against
    /// firing music notes off an unrelated state.
    #[test]
    fn playing_instrument_is_none_outside_the_music_state() {
        assert_eq!(playing_instrument(&CharacterState::default(), None), None);
    }

    /// A real `CharacterState::Music` with a `Flute` equipped in
    /// `ActiveMainhand` resolves `Some(AbilitySpec::Custom("Flute"))` —
    /// ported behaviour from the old combat mapper's own `Music(ToolKind,
    /// AbilitySpec)` construction.
    #[test]
    fn playing_instrument_resolves_the_equipped_flutes_ability_spec() {
        let mut inventory = Inventory::with_empty();
        let flute =
            common::comp::Item::new_from_asset_expect("common.items.tool.instruments.flute");
        inventory.replace_loadout_item(
            EquipSlot::ActiveMainhand,
            Some(flute),
            common::resources::Time(0.0),
        );

        assert_eq!(
            playing_instrument(&dummy_music_state(), Some(&inventory)),
            Some(AbilitySpec::Custom("Flute".to_owned()))
        );
    }

    /// No `Inventory` at all (e.g. a scenery entity somehow in `Music`, which
    /// never really happens, but the function must stay total) resolves
    /// `None`, not a panic.
    #[test]
    fn playing_instrument_with_no_inventory_is_none() {
        assert_eq!(playing_instrument(&dummy_music_state(), None), None);
    }

    /// The full mirror system: a sim entity in `CharacterState::Music` with a
    /// `Flute` equipped mirrors `NetInstrumentMove::playing ==
    /// Some(Custom("Flute"))` — the task's own headline verify ("the
    /// instrument notes load + play" starts here, at the mirror that tells
    /// the client-side mapper WHICH note-bank to draw from).
    #[test]
    fn mirrors_a_playing_instrument_entity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            let mut inventory = Inventory::with_empty();
            let flute =
                common::comp::Item::new_from_asset_expect("common.items.tool.instruments.flute");
            inventory.replace_loadout_item(
                EquipSlot::ActiveMainhand,
                Some(flute),
                common::resources::Time(0.0),
            );
            ecs.create_entity()
                .with(PhysicsState::default())
                .with(dummy_music_state())
                .with(inventory)
                .build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_locomotion_and_combat_state)
            .expect("system runs");
        app.update();

        let instrument_move = app
            .world()
            .get::<NetInstrumentMove>(bevy_entity)
            .expect("NetInstrumentMove must be mirrored");
        assert_eq!(
            instrument_move.playing,
            Some(AbilitySpec::Custom("Flute".to_owned()))
        );
    }

    /// Builds a minimal `CharacterState::Music` — `is_music()`-true by
    /// construction, `ability_info.hand: None` so [`playing_instrument`]'s
    /// own `ActiveMainhand` fallback is what's under test.
    fn dummy_music_state() -> CharacterState {
        use common::states::music;
        CharacterState::Music(music::Data {
            static_data: music::StaticData {
                play_duration: Default::default(),
                ori_modifier: 1.0,
                ability_info: dummy_ability_info(),
            },
            timer: Default::default(),
            stage_section: StageSection::Action,
            exhausted: false,
        })
    }

    /// Losing the sim-side `PhysicsState`/`CharacterState` components
    /// removes the corresponding `Net*` mirrors too.
    #[test]
    fn removing_sim_components_removes_the_net_mirrors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = new_app_with_sim(dir.path());

        let sim_entity = {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.create_entity()
                .with(PhysicsState::default())
                .with(CharacterState::default())
                .build()
        };
        let bevy_entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SimMirror>()
            .0
            .insert(sim_entity, bevy_entity);

        app.world_mut()
            .run_system_once(mirror_locomotion_and_combat_state)
            .expect("first run mirrors both");
        app.update();
        assert!(app.world().get::<NetLocomotion>(bevy_entity).is_some());
        assert!(app.world().get::<NetCombatMove>(bevy_entity).is_some());
        assert!(app.world().get::<NetInstrumentMove>(bevy_entity).is_some());

        {
            let mut sim = app.world_mut().non_send_mut::<SimServer>();
            let ecs = sim.server.state_mut().ecs_mut();
            ecs.write_storage::<PhysicsState>().remove(sim_entity);
            ecs.write_storage::<CharacterState>().remove(sim_entity);
        }

        app.world_mut()
            .run_system_once(mirror_locomotion_and_combat_state)
            .expect("second run removes both");
        app.update();
        assert!(app.world().get::<NetLocomotion>(bevy_entity).is_none());
        assert!(app.world().get::<NetCombatMove>(bevy_entity).is_none());
        assert!(app.world().get::<NetInstrumentMove>(bevy_entity).is_none());
    }

    /// Builds a minimal `SelfBuff` `CharacterState` — `is_attack()`-true per
    /// `CharacterState::is_attack`'s own match, and by far the shallowest
    /// attack-shaped state's fields to construct by hand for a test fixture.
    fn dummy_self_buff_state() -> CharacterState {
        use common::states::self_buff;
        CharacterState::SelfBuff(self_buff::Data {
            static_data: self_buff::StaticData {
                buildup_duration: Default::default(),
                cast_duration: Default::default(),
                recover_duration: Default::default(),
                buffs: Vec::new(),
                buff_cat: None,
                combo_cost: 0,
                combo_scaling: None,
                combo_on_use: 0,
                enforced_limit: false,
                ability_info: dummy_ability_info(),
                specifier: None,
            },
            timer: Default::default(),
            stage_section: StageSection::Buildup,
        })
    }

    fn dummy_attack_filters() -> common::comp::character_state::AttackFilters {
        common::comp::character_state::AttackFilters {
            melee: false,
            projectiles: false,
            beams: false,
            ground_shockwaves: false,
            air_shockwaves: false,
            explosions: false,
            arcs: false,
            pools: false,
        }
    }

    fn dummy_ability_info() -> common::states::utils::AbilityInfo {
        common::states::utils::AbilityInfo {
            tool: None,
            hand: None,
            input: common::comp::InputKind::Primary,
            input_attr: None,
            ability_meta: Default::default(),
            ability: None,
        }
    }
}
