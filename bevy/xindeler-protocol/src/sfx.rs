//! BL-82 EM-5.10b (T56.35) — the replication contract the SFX event mappers
//! (`xindeler-client::sfx`) read/write. Three small additions, all following
//! the spec §3.2 mirror pattern already established by
//! [`crate::NetLoadout::gliding`]/[`crate::skillset`]:
//!
//! - [`NetLocomotion`]/[`NetCombatMove`]: per-entity `Net*` components,
//!   pre-flattened from the sim's `PhysicsState`/`CharacterState` the SAME way
//!   `xindeler-sim-bridge::is_gliding` already pre-flattens `CharacterState`
//!   down to one bool for [`crate::NetLoadout::gliding`] — "project, don't
//!   dump": the client never sees a raw `CharacterState`.
//! - [`NetOutcome`]: a server → client **message** (bulk/discrete data, spec
//!   §3.2's "bulk data = messages, not per-component replication" rule, same
//!   class as [`crate::chat::NetChatMsg`]/[`crate::narrative::HudToast`]),
//!   carrying a DELIBERATELY PARTIAL projection of `common::outcome::Outcome`
//!   (~40 variants total) — just the five the old client's own
//!   `SfxMgr::handle_outcome` mapped to a genuinely distinct, commonly-heard
//!   combat sound (explosion/damage/death/block-or-parry/poise-break). The
//!   other ~35 variants (arrow/beam/summon/environmental one-offs) are a
//!   documented v1 cut, not a silent gap — see [`NetOutcome`]'s own doc
//!   comment.
use bevy::{ecs::component::Component, math::Vec3};
use common::comp::{CharacterAbilityType, inventory::item::tool::AbilitySpec, poise::PoiseState};
use serde::{Deserialize, Serialize};

/// Coarse ground-material grouping under an entity's feet, mirroring the old
/// client's own `Run(BlockKind)` simplification EXACTLY
/// (`voxygen/src/audio/sfx/event_mapper/movement/mod.rs::map_movement_event`'s
/// `match block.kind() { Snow | ArtSnow => .., Rock | WeakRock | GlowingRock |
/// GlowingWeakRock | Ice => .., Earth => .., Air => .., _ => Grass }`) — a
/// handful of footstep-sound BUCKETS, not the full `BlockKind` enum (which
/// the client has no other use for and would be a far wider wire type for no
/// extra fidelity the sfx.ron manifest even distinguishes).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetGroundBlock {
    #[default]
    Grass,
    Rock,
    Earth,
    Snow,
    Air,
}

impl NetGroundBlock {
    /// Ported 1:1 from the old client's `match block.kind() { .. }` arms in
    /// both `map_movement_event` and `map_non_humanoid_movement_event` (the
    /// two agree on this grouping).
    #[must_use]
    pub fn from_block_kind(kind: common::terrain::BlockKind) -> Self {
        use common::terrain::BlockKind;
        match kind {
            BlockKind::Snow | BlockKind::ArtSnow => Self::Snow,
            BlockKind::Rock
            | BlockKind::WeakRock
            | BlockKind::GlowingRock
            | BlockKind::GlowingWeakRock
            | BlockKind::Ice => Self::Rock,
            BlockKind::Earth => Self::Earth,
            BlockKind::Air => Self::Air,
            _ => Self::Grass,
        }
    }
}

/// The `CharacterState`-derived movement CATEGORY (spec §3.2 pre-flattening
/// — the client never sees a raw `CharacterState`), pre-computed server-side
/// exactly the way `xindeler-sim-bridge::is_gliding` already pre-flattens
/// `CharacterState` for [`crate::NetLoadout::gliding`].
///
/// Deliberately does NOT fold in `PhysicsState.on_ground`/`in_liquid`/`Vel`
/// (those stay their own already-mirrored fields —
/// [`NetLocomotion::on_ground`]/ [`NetLocomotion::in_liquid`]/
/// [`crate::NetVel`]) — the client-side event
/// mapper (`xindeler-client::sfx::movement`) recombines them every frame with
/// its OWN kept-client-side per-entity history (previous on_ground/in_liquid/
/// elapsed-since-last-play), the exact same split the old client's own
/// `MovementEventMapper` drew between "this frame's `CharacterState`
/// classification" (computed fresh) and "history-based emission decision"
/// (kept in its own `event_history` map).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetMoveState {
    #[default]
    Idle,
    /// `CharacterState::Roll` with `static_data.was_cancel == false`.
    Roll,
    /// `CharacterState::Roll` with `static_data.was_cancel == true` (an
    /// aborted roll — old client plays a distinct "cancel" sound).
    RollCancel,
    /// `character_state.is_stealthy()` (sneaking).
    Sneak,
    /// `CharacterState::Climb`.
    Climb,
    /// A glide-shaped `CharacterState` (reuses the SAME `is_glide_wielded`
    /// predicate `xindeler-sim-bridge::is_gliding` already applies for
    /// [`crate::NetLoadout::gliding`] — one classification, two consumers).
    Glide,
}

/// Replicated per-entity locomotion snapshot (BL-82 EM-5.10b, T56.35).
///
/// Everything the client-side movement sfx mapper needs beyond the
/// already-mirrored [`crate::NetVel`]/[`crate::NetBody`] (body gives
/// `stride_length()`/the per-body-type volume table, both pure functions of
/// the already-replicated type). `Scale` is NOT mirrored here (a documented
/// v1 simplification — footstep timing assumes scale 1.0; a giant NPC's
/// footsteps will time slightly wrong until a future task adds it, same
/// "acceptable, documented" posture as other Phase-5 mirrors' own v1 cuts).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetLocomotion {
    /// `PhysicsState.on_ground.is_some()`.
    pub on_ground: bool,
    /// `PhysicsState.in_liquid().is_some()`.
    pub in_liquid: bool,
    /// The block kind under the entity's feet, if grounded — meaningless
    /// (left at its `Default`) while `on_ground` is `false`; the client only
    /// reads it when `on_ground` is `true` anyway (matches old client: the
    /// `Run(BlockKind)` branch is only reached inside the `on_ground` arm).
    pub ground_block: NetGroundBlock,
    /// The `CharacterState`-derived movement category (see
    /// [`NetMoveState`]'s doc comment for why physics stays separate).
    pub move_state: NetMoveState,
}

impl Default for NetLocomotion {
    fn default() -> Self {
        Self {
            on_ground: true,
            in_liquid: false,
            ground_block: NetGroundBlock::default(),
            move_state: NetMoveState::default(),
        }
    }
}

/// Replicated per-entity combat-move snapshot (BL-82 EM-5.10b, T56.35): the
/// two signals the old client's combat sfx mapper's `map_event` needed off
/// `CharacterState` (`character_state.is_attack()` →
/// `CharacterAbilityType::from(character_state)`, and its own
/// `weapon_drawn` helper), pre-computed server-side. Tool KIND for the
/// Attack/Wield/Unwield sfx key comes from the ALREADY-mirrored
/// [`crate::NetLoadout::active_tool`] — no new field needed for it.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NetCombatMove {
    /// `Some(ability_type)` while `character_state.is_attack()`; `None`
    /// otherwise. `CharacterAbilityType` is the sim's own lightweight
    /// frontend-facing enum (`common::comp::ability`) — reused verbatim on
    /// the wire, the same "reuse the sim's own small enum directly" choice
    /// [`crate::NetBuffEntry::kind`] already made for `BuffKind`.
    pub attacking: Option<CharacterAbilityType>,
    /// `character_state.is_wield() || matches!(character_state,
    /// CharacterState::Equipping { .. })` — ported verbatim from the old
    /// combat mapper's own `weapon_drawn` helper.
    pub weapon_drawn: bool,
}

/// Replicated per-entity instrument-playing snapshot (BL-82 EM-5.10e,
/// T56.37 — the 252-file bard instrument note-bank). Kept as its OWN
/// component rather than folded into [`NetCombatMove`] because
/// [`AbilitySpec::Custom`] carries a `String`, which would cost that struct
/// its `Copy` derive (relied on by call sites that construct a `NetCombatMove`
/// value and then still use it after spawning, e.g. this module's own
/// round-trip test) — a clean "one extra component" split, the same shape
/// [`crate::narrative::HudToast`] takes alongside [`crate::chat::NetChatMsg`]
/// rather than merging unrelated shapes into one payload.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct NetInstrumentMove {
    /// `Some(ability_spec)` while `character_state.is_music()` AND the
    /// equipped instrument (the hand `character_state.ability_info()` names,
    /// falling back to `ActiveMainhand`) resolves an
    /// `common::comp::inventory::item::ItemDesc::ability_spec()` — ported
    /// verbatim from the old combat mapper's own `Music(ToolKind,
    /// AbilitySpec)` construction (`voxygen/src/audio/sfx/event_mapper/
    /// combat/mod.rs::map_event`'s `is_music()` arm). `None` while not
    /// playing, or if the equipped item carries no `ability_spec` (both
    /// cases are the client's cue to stay silent). `ToolKind` is NOT
    /// mirrored here — every music-playing item is `ToolKind::Instrument`
    /// (verified against every `assets/common/items/tool/instruments/*.ron`),
    /// so the client-side mapper hardcodes it the same way `sfx.ron`'s own
    /// `Music(Instrument, Custom(..))` keys do.
    pub playing: Option<AbilitySpec>,
}

/// Server → client: a DELIBERATELY PARTIAL projection of
/// `common::outcome::Outcome` (BL-82 EM-5.10b, T56.35's `handle_outcome`
/// port) — see this module's doc comment for the full "why only 5 of ~40"
/// reasoning. Travels as a discrete one-shot message (spec §3.2's "bulk data
/// = messages" rule — an `Outcome` is a transient per-tick happening, not
/// per-entity state to keep replicated), `make_message_independent` like
/// [`crate::chat::NetChatMsg`]/[`crate::narrative::HudToast`] (no entity
/// references, must not queue behind entity replication).
///
/// Positions are already axis-converted to Bevy space (matches every other
/// `Net*` position field) — the bridge does the `vek → glam` conversion once,
/// server-side, so the client never touches sim axes.
///
/// ## Documented v1 cut (not a silent gap)
/// The old `SfxMgr::handle_outcome` maps ~40 `Outcome` variants (arrows,
/// beams, environmental one-offs like `Steam`/`IceCrack`/`Lightning`,
/// per-species `SummonedCreature` roars, `Utterance` creature vocalizations,
/// `Splash`...). This message carries only the five that are (a) genuinely
/// combat-central and (b) don't need extra mirrored state to resolve a sound
/// key (no per-species `Body` lookup, no `VoiceKind` table, no projectile
/// `object::Body` classification): `Explosion`/`HealthChange`(→`Damage`)/
/// `Death`/`Block`/`PoiseChange`. Extending coverage is a straightforward,
/// additive follow-up (each new variant is one more `NetOutcome` case + one
/// more `handle_outcome` match arm) — tracked in
/// `docs/backlog/engine-migration.md`'s EM-5.10 row, not silently narrowed.
#[derive(bevy::ecs::message::Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum NetOutcome {
    /// `Outcome::Explosion { pos, power, .. }` (old code passes
    /// `power.abs()` as the extra volume multiplier).
    Explosion { pos: Vec3, power: f32 },
    /// `Outcome::HealthChange { pos, info }` where `info.amount <
    /// Health::HEALTH_EPSILON` (i.e. damage, not healing) AND
    /// `info.cause` is not `DamageSource::Buff(_)` (old code's exact filter
    /// — buff-sourced damage, e.g. poison ticks, is intentionally silent).
    Damage { pos: Vec3 },
    /// `Outcome::Death { pos }`.
    Death { pos: Vec3 },
    /// `Outcome::Block { pos, parry, .. }`.
    Block { pos: Vec3, parry: bool },
    /// `Outcome::PoiseChange { pos, state }` — `state == PoiseState::Normal`
    /// is filtered out server-side before this is ever constructed (the old
    /// code's own `match poise_state { Normal => {}, .. }` no-op arm), so a
    /// received `NetOutcome::PoiseChange` always carries a real break state.
    PoiseChange { pos: Vec3, state: PoiseState },
}

#[cfg(test)]
mod tests {
    use bevy::{
        app::{App, PluginGroup, PostUpdate},
        ecs::message::Messages,
        prelude::MinimalPlugins,
        state::app::StatesPlugin,
    };
    use bevy_replicon::{
        prelude::{Replicated, RepliconPlugins, SendTargets, ServerPlugin, ToClients},
        test_app::ServerTestAppExt,
    };
    use common::terrain::BlockKind;

    use super::*;
    use crate::XindelerProtocolPlugin;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
            XindelerProtocolPlugin,
        ))
        .finish();
        app
    }

    /// [`NetLocomotion`]/[`NetCombatMove`] round-trip over the real replicon
    /// test loopback — the same acceptance bar `NetLoadout`/`combat_hud`'s
    /// own component mirrors already carry.
    #[test]
    fn locomotion_and_combat_move_replicate_to_the_client() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let locomotion = NetLocomotion {
            on_ground: true,
            in_liquid: false,
            ground_block: NetGroundBlock::Rock,
            move_state: NetMoveState::Sneak,
        };
        let combat_move = NetCombatMove {
            attacking: Some(CharacterAbilityType::BasicBlock),
            weapon_drawn: true,
        };

        server_app
            .world_mut()
            .spawn((Replicated, locomotion, combat_move));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app
            .world_mut()
            .query::<(&NetLocomotion, &NetCombatMove)>();
        let (got_locomotion, got_combat_move) = q
            .single(client_app.world())
            .expect("exactly one mirrored entity");
        assert_eq!(*got_locomotion, locomotion);
        assert_eq!(*got_combat_move, combat_move);
    }

    /// [`NetInstrumentMove`] round-trip (BL-82 EM-5.10e, T56.37) — the same
    /// acceptance bar as the sibling [`NetLocomotion`]/[`NetCombatMove`] test,
    /// exercising the `Custom(String)` payload specifically (the whole reason
    /// this is its own component rather than a `NetCombatMove` field).
    #[test]
    fn instrument_move_replicates_to_the_client() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let instrument_move = NetInstrumentMove {
            playing: Some(AbilitySpec::Custom("Flute".to_owned())),
        };

        server_app
            .world_mut()
            .spawn((Replicated, instrument_move.clone()));

        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let mut q = client_app.world_mut().query::<&NetInstrumentMove>();
        let got = q
            .single(client_app.world())
            .expect("exactly one mirrored entity");
        assert_eq!(*got, instrument_move);
    }

    #[test]
    fn instrument_move_default_is_silent() {
        assert_eq!(NetInstrumentMove::default().playing, None);
    }

    /// [`NetOutcome`] travels as a broadcast server message (`SendTargets::
    /// All`), same shape [`crate::chat::NetChatMsg`]'s own round-trip test
    /// exercises.
    #[test]
    fn net_outcome_broadcasts_to_the_client() {
        let mut server_app = new_app();
        let mut client_app = new_app();
        server_app.connect_client(&mut client_app);

        let payload = NetOutcome::Death {
            pos: Vec3::new(1.0, 2.0, 3.0),
        };
        server_app.world_mut().write_message(ToClients {
            targets: SendTargets::All,
            message: payload,
        });
        server_app.update();
        server_app.exchange_with_client(&mut client_app);
        client_app.update();

        let received: Vec<_> = client_app
            .world_mut()
            .resource_mut::<Messages<NetOutcome>>()
            .drain()
            .collect();
        assert_eq!(received, vec![payload]);
    }

    #[test]
    fn ground_block_grouping_matches_the_old_client_exactly() {
        assert_eq!(
            NetGroundBlock::from_block_kind(BlockKind::Snow),
            NetGroundBlock::Snow
        );
        assert_eq!(
            NetGroundBlock::from_block_kind(BlockKind::ArtSnow),
            NetGroundBlock::Snow
        );
        for rocky in [
            BlockKind::Rock,
            BlockKind::WeakRock,
            BlockKind::GlowingRock,
            BlockKind::GlowingWeakRock,
            BlockKind::Ice,
        ] {
            assert_eq!(NetGroundBlock::from_block_kind(rocky), NetGroundBlock::Rock);
        }
        assert_eq!(
            NetGroundBlock::from_block_kind(BlockKind::Earth),
            NetGroundBlock::Earth
        );
        assert_eq!(
            NetGroundBlock::from_block_kind(BlockKind::Air),
            NetGroundBlock::Air
        );
        // Everything else (Grass, Sand, Wood, Leaves, Misc, ...) buckets to
        // Grass, matching the old client's `_ => SfxEvent::Run(BlockKind::Grass)`
        // catch-all.
        for other in [
            BlockKind::Grass,
            BlockKind::Sand,
            BlockKind::Wood,
            BlockKind::Misc,
        ] {
            assert_eq!(
                NetGroundBlock::from_block_kind(other),
                NetGroundBlock::Grass
            );
        }
    }

    #[test]
    fn net_locomotion_default_is_grounded_idle() {
        let loco = NetLocomotion::default();
        assert!(loco.on_ground);
        assert!(!loco.in_liquid);
        assert_eq!(loco.move_state, NetMoveState::Idle);
    }
}
