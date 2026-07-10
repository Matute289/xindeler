//! EM-4.7 adapter: the OTHER half of the generic entity factory
//! (`xindeler_oracle_host::entity_template`) — turns the Bevy-side
//! `Pending*` descriptor components a factory-spawned staging entity carries
//! into a REAL sim NPC, through the exact same public event bus
//! (`NpcBuilder` + `CreateNpcEvent` + `State::emit_event_now`)
//! [`crate::spawn_test_npcs`] already uses. Lives in THIS crate because it is
//! "the ONLY legal `specs` consumer under `bevy/`" (this crate's own lib.rs
//! module doc) — `xindeler-oracle-host` never embeds a `specs::World` itself
//! (isolation-law rule 4: writes into the sim go through its public APIs
//! only).
//!
//! Once [`apply_pending_entity_template_spawns`] requests the NPC, the
//! EXISTING [`crate::mirror_sim_entities`] system mirrors it into
//! `NetBody`/`NetUid`/… with ZERO new client code, and `xindeler-client`'s
//! EM-3.8 figure pipeline assembles its real `.vox` model — this is what
//! "verbatim reuse of the figure-assembly pipeline" means structurally: the
//! new NPC is indistinguishable, from the mirror's point of view, from one
//! of [`crate::spawn_test_npcs`]'s own test NPCs.
//!
//! ## v1 scope: default dimension only (soft dependency on EM-4.5)
//! A [`PendingEntityTemplateSpawn`] naming any `DimensionId` other than
//! [`DimensionId::DEFAULT`] is dropped with a `warn!` — there is no
//! per-dimension `Server`/`State` handle a spawn call could target yet (the
//! `DimensionRegistry` only tags the MIRROR's own entities today). Documented
//! rather than silent; extending this is EM-4.9's job (spawning a DmEvent's
//! monsters into ITS instanced dimension).
//!
//! ## Anti-chaos: a request that can't be resolved is dropped, never panics
//! - No [`PendingBody`] at all, or its string doesn't parse as a
//!   `common::npc::NpcBody` keyword (the SAME vocabulary `/spawn` parses) → the
//!   request can't become an NPC; dropped with a `warn!`.
//! - No [`PendingFaction`], or an unrecognized faction string → falls back to
//!   `Alignment::Wild`.
//! - No [`PendingAiBehavior`], or an unrecognized behavior string → falls back
//!   to [`AgentPreset::Passive`] via [`AgentPreset::resolve`].
//!
//! Either way the staging entity is ALWAYS despawned once processed — it
//! never accumulates across ticks, matching every other one-shot request
//! pattern in this crate (e.g. [`crate::TestNpcState`]'s spawn latch).

use bevy::{
    ecs::{change_detection::NonSendMut, entity::Entity, system::Query},
    log::{info, warn},
    prelude::Commands,
};
use common::{
    comp,
    event::{CreateNpcEvent, NpcBuilder},
    lottery::LootSpec,
    npc,
};
use xindeler_oracle_host::entity_template::{
    AgentPreset, PendingAiBehavior, PendingBody, PendingEntityTemplateSpawn, PendingFaction,
    PendingLoot, PendingStats,
};
use xindeler_protocol::DimensionId;

use crate::SimServer;

/// Resolves an `EntityTemplate.faction` string to an `Alignment`, falling
/// back to `Wild` for anything outside the closed set
/// `xindeler_oracle_host::entity_template::bounds::KNOWN_FACTIONS` — same
/// "defuse, don't crash" posture as [`AgentPreset::resolve`]. `Owned(Uid)`
/// is intentionally unreachable (a template can't name a runtime owner).
fn resolve_faction(name: &str) -> comp::Alignment {
    match name {
        "enemy" => comp::Alignment::Enemy,
        "npc" => comp::Alignment::Npc,
        "tame" => comp::Alignment::Tame,
        "passive" => comp::Alignment::Passive,
        // "wild" and anything unrecognized both land here.
        _ => comp::Alignment::Wild,
    }
}

/// Resolves an NPC body keyword — the SAME vocabulary the in-game `/spawn`
/// admin command already parses via `common::npc::NpcBody`'s `FromStr`
/// (`server/src/cmd.rs`'s `handle_spawn`) — to a concrete `(NpcKind, Body)`
/// pair. `None` for an unrecognized keyword.
fn resolve_body(name: &str) -> Option<(npc::NpcKind, comp::Body)> {
    let npc::NpcBody(kind, mut make_body) = name.parse().ok()?;
    Some((kind, make_body()))
}

/// Reads every [`PendingEntityTemplateSpawn`] staging entity, resolves it
/// into a real sim NPC via the sim's public event bus, and despawns the
/// staging entity — whether or not the request could actually be resolved
/// (see the module doc's anti-chaos section).
pub fn apply_pending_entity_template_spawns(
    sim: Option<NonSendMut<SimServer>>,
    mut commands: Commands,
    pending: Query<(
        Entity,
        &PendingEntityTemplateSpawn,
        Option<&PendingBody>,
        Option<&PendingStats>,
        Option<&PendingFaction>,
        Option<&PendingLoot>,
        Option<&PendingAiBehavior>,
    )>,
) {
    let Some(sim) = sim else { return };

    for (entity, request, body, stats, faction, loot, ai_behavior) in &pending {
        // Always despawn the staging entity once seen — win or lose, it
        // must never accumulate across ticks.
        commands.entity(entity).despawn();

        if request.dimension != DimensionId::DEFAULT {
            warn!(
                dimension = request.dimension.0,
                "entity_template: v1 only spawns into the default dimension; dropping this request"
            );
            continue;
        }

        let Some(body_name) = body.map(|b| b.0.as_str()) else {
            warn!("entity_template: no \"body\" component on this request; cannot spawn");
            continue;
        };
        let Some((kind, resolved_body)) = resolve_body(body_name) else {
            warn!(
                body = body_name,
                "entity_template: not a recognized NPC body keyword; dropping this spawn request"
            );
            continue;
        };

        let name = stats
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| npc::get_npc_name(kind, npc::BodyType::from_body(resolved_body)));
        let alignment = faction.map_or(comp::Alignment::Wild, |f| resolve_faction(&f.0));
        let loot_spec = loot
            .map(|l| LootSpec::Item(l.0.clone()))
            .unwrap_or(LootSpec::Nothing);
        let preset = ai_behavior.map_or(AgentPreset::Passive, |b| AgentPreset::resolve(&b.0));
        let agent = preset.build_agent(&resolved_body, request.pos);

        let mut npc = NpcBuilder::new(
            comp::Stats::new(comp::Content::Plain(name), resolved_body),
            resolved_body,
            alignment,
        )
        .with_health(comp::Health::new(resolved_body))
        .with_agent(agent);
        npc.loot = loot_spec;

        let wpos = vek::Vec3::new(request.pos[0], request.pos[1], request.pos[2]);
        sim.server.state().emit_event_now(CreateNpcEvent {
            pos: comp::Pos(wpos),
            ori: comp::Ori::default(),
            npc,
        });

        info!(
            body = body_name,
            ?preset,
            "entity_template: spawned a factory NPC through the sim's public event bus"
        );
    }
}
