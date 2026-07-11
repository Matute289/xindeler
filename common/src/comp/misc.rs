use crate::{
    combat::CombatEffect,
    comp::{PidController, ability::Dodgeable, beam},
    resources::{Secs, Time},
    states::basic_summon::BeamPillarIndicatorSpecifier,
    uid::Uid,
};
use serde::{Deserialize, Serialize};
use specs::{Component, FlaggedStorage, HashMapStorage, VecStorage};
use std::time::Duration;
use vek::Vec3;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Object {
    DeleteAfter {
        spawned_at: Time,
        timeout: Duration,
    },
    Portal {
        target: Vec3<f32>,
        requires_no_aggro: bool,
        buildup_time: Secs,
    },
    BeamPillar {
        spawned_at: Time,
        buildup_duration: Duration,
        attack_duration: Duration,
        beam_duration: Duration,
        radius: f32,
        height: f32,
        damage: f32,
        damage_effect: Option<CombatEffect>,
        dodgeable: Dodgeable,
        tick_rate: f32,
        specifier: beam::FrontendSpecifier,
        indicator_specifier: BeamPillarIndicatorSpecifier,
    },
    Crux {
        owner: Uid,
        scale: f32,
        range: f32,
        strength: f32,
        duration: Secs,
        #[serde(skip)]
        pid_controller: Option<PidController<fn(f32, f32) -> f32, 8>>,
    },
}

impl Component for Object {
    type Storage = FlaggedStorage<Self, VecStorage<Self>>;
}

#[derive(Clone, Debug)]
pub struct PortalData {
    pub target: Vec3<f32>,
    pub requires_no_aggro: bool,
    pub buildup_time: Secs,
}

impl From<PortalData> for Object {
    fn from(
        PortalData {
            target,
            requires_no_aggro,
            buildup_time,
        }: PortalData,
    ) -> Self {
        Self::Portal {
            target,
            requires_no_aggro,
            buildup_time,
        }
    }
}

/// A one-shot, opaque correlation tag a `CreateNpcEvent` caller can attach to
/// an [`crate::event::NpcBuilder`] (via `NpcBuilder::with_spawn_correlation`)
/// so it can recognize the REAL entity that request produced once it
/// materializes, independent of creation order relative to any other entity
/// created the same tick.
///
/// ## BL-82: why this exists (fixes a real misattribution race)
/// `bevy/xindeler-sim-bridge`'s entity mirror needs to know which
/// `xindeler_dimensions::DimensionId` a freshly-discovered NPC belongs to,
/// but `State::emit_event_now` only queues the `CreateNpcEvent` — the real
/// `specs::Entity` doesn't exist until the NEXT tick, so the bridge can't key
/// a `specs::Entity -> DimensionId` map at request time. An earlier design
/// correlated by FIFO queue order ("the next Agent-bearing entity discovered
/// this tick") instead of by identity — but ordinary wildlife spawns
/// (`server/src/sys/terrain.rs`, always-on, not a dev-only ring) are ALSO
/// Agent-bearing and could steal the queue slot meant for an ORACLE-event
/// minion on the same tick, mis-tagging either entity's dimension. This
/// component makes the correlation exact instead of order-dependent: the
/// caller mints a fresh id, stashes `id -> DimensionId` in its own
/// side-channel map, and tags the builder with it; only the entity that
/// ACTUALLY carries this exact id is ever looked up, so an untagged wildlife
/// spawn (or any other CreateNpcEvent caller that doesn't opt in) can never
/// collide with it regardless of discovery order.
///
/// Not persisted (no `Serialize`/`Deserialize`) and not net-synced — this is
/// a same-process, single-session bridge-internal bookkeeping tag, not game
/// state. Left on the entity permanently once assigned (nothing in the
/// isolation-law-respecting bridge is allowed to write into the sim to strip
/// it back off — see `bevy/xindeler-sim-bridge`'s "bridge systems are
/// read-mostly" rule); harmless, since it is never read for anything except
/// this one-shot correlation, cached by the reader after the first read.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpawnCorrelation(pub u64);

impl Component for SpawnCorrelation {
    type Storage = HashMapStorage<Self>;
}
