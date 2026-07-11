//! EM-4.7 adapter: the OTHER half of the generic entity factory
//! (`xindeler_oracle_host::entity_template`) — turns the Bevy-side
//! `Pending*` descriptor components a factory-spawned staging entity carries
//! into a REAL sim NPC, through the exact same public event bus
//! (`NpcBuilder` + `CreateNpcEvent` + `State::emit_event_now`)
//! [`crate::spawn_test_npcs`] already uses. Lives in THIS crate because it is
//! one of the two sanctioned `specs` consumers under `bevy/` (this crate's
//! own lib.rs module doc — the other is `xindeler-server-app::login`'s
//! narrow, sanctioned exception, BL-82 EM-4.2c) — `xindeler-oracle-host`
//! never embeds a `specs::World` itself (isolation-law rule 4: writes into
//! the sim go through its public APIs only).
//!
//! Once [`apply_pending_entity_template_spawns`] requests the NPC, the
//! EXISTING [`crate::mirror_sim_entities`] system mirrors it into
//! `NetBody`/`NetUid`/… with ZERO new client code, and `xindeler-client`'s
//! EM-3.8 figure pipeline assembles its real `.vox` model — this is what
//! "verbatim reuse of the figure-assembly pipeline" means structurally: the
//! new NPC is indistinguishable, from the mirror's point of view, from one
//! of [`crate::spawn_test_npcs`]'s own test NPCs.
//!
//! ## BL-82 EM-4.9 (Phase C, T51.6): routes into ANY currently-Active dimension
//! There is still only ONE real specs `Server`/`State` (one physical terrain,
//! one physical NPC storage) in this process — `DimensionRegistry` indexes a
//! SEPARATE, standalone procgen `world::World`/chunk-store per dimension
//! (`xindeler-dimensions::registry`), but nothing wires that generated
//! terrain into the sim's own live `TerrainGrid` yet (true per-dimension
//! physics/terrain simulation is out of scope — see the migration spec's
//! deferred-items table). So "routing a spawn into dimension X" means: the
//! real sim NPC is still created in the one physical world (same terrain
//! everyone else's entities stand on), but its MIRROR is tagged with
//! dimension X's `DimensionId` (via [`crate::PendingDimensionAttribution`],
//! consumed by [`crate::mirror_sim_entities`]) so per-client interest
//! management (`xindeler_protocol::visibility`) scopes its visibility
//! separately from the default world's own entities — the concrete,
//! honest meaning of "the minions live in the instanced dimension" for v1.
//!
//! A request naming a `DimensionId` that isn't currently registered+`Active`
//! in the [`DimensionRegistry`] is dropped with a `warn!` (defensive: a
//! never-activated or already-torn-down dimension must never silently
//! mis-spawn an entity nowhere any client will ever see it cleaned up from).
//! [`DimensionId::DEFAULT`] is always accepted (unchanged v1 behavior).
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
//!
//! ## BL-82 EM-4.9: the real producer now exists (`xindeler-server-app::oracle`)
//! `ServerOraclePlugin`'s `spawn_event_minions` system is the real caller
//! `spawn_from_spawning_rules` was waiting on (EM-4.3/4.4's `DmEventPlugin`
//! got its own real caller the same task, `ingest_dm_events`) — it resolves a
//! `DmEvent`'s `spawning_rules` once its target dimension reaches `Active`
//! ([`xindeler_dimensions::DimensionActivated`]) and calls this module's
//! functions exactly like the pre-existing tests already did directly. That
//! system carries the explicit ordering edge
//! (`.before(apply_pending_entity_template_spawns)`) this doc comment used to
//! ask a future caller to remember.

use std::collections::{HashMap, VecDeque};

use bevy::{
    ecs::{
        change_detection::NonSendMut,
        entity::Entity,
        resource::Resource,
        system::{Query, Res, ResMut},
    },
    log::{info, warn},
    prelude::Commands,
};
use common::{
    comp,
    event::{CreateNpcEvent, NpcBuilder},
    lottery::LootSpec,
    npc,
};
use rand::RngExt;
use xindeler_dimensions::{DimensionLifecycle, DimensionRegistry};
use xindeler_oracle_host::{
    dm_event::SpawningRules,
    entity_template::{
        AgentPreset, EntityTemplate, PendingAiBehavior, PendingBody, PendingEntityTemplateSpawn,
        PendingFaction, PendingLoot, PendingStats, spawn_entity_template,
    },
};
use xindeler_protocol::DimensionId;

use crate::SimServer;

/// FIFO queue of dimensions awaiting attribution to the next brand-new,
/// Agent-bearing sim entity [`crate::mirror_sim_entities`] discovers (BL-82
/// EM-4.9, Phase C / T51.6).
///
/// ## Why a queue, not a direct `specs::Entity -> DimensionId` map
/// `State::emit_event_now` only QUEUES a `CreateNpcEvent` onto the sim's
/// event bus — the real specs `Entity` doesn't exist until the sim's OWN
/// next `tick_sim` call processes
/// it (one `FixedUpdate` tick later than this module's own
/// [`apply_pending_entity_template_spawns`] run), so there is no `Entity` to
/// key a map by at the moment this module requests the spawn. Instead, this
/// module pushes the TARGET dimension once per non-default-dimension spawn
/// it successfully requests (in request order); [`crate::mirror_sim_entities`]
/// pops the front entry the next time it discovers a brand-new sim entity
/// that also carries an `Agent` component (real players never do — see that
/// function's own doc comment) and assigns that dimension via
/// [`crate::SimEntityDimension`], defaulting to [`DimensionId::DEFAULT`] when
/// the queue is empty (unchanged v1 behavior for ordinary NPC/test spawns).
///
/// ## Known, documented limitation
/// This is a best-effort correlation, not a guaranteed one: if some OTHER
/// Agent-bearing entity (e.g. `spawn_test_npcs`'s wandering ring, disabled by
/// default) happens to be discovered by the mirror in the exact same tick a
/// factory batch materializes, one queue entry could be consumed by the
/// wrong entity. Acceptable for v1 (a single controlled ORACLE encounter at a
/// time); a fully robust fix needs the sim itself to carry dimension
/// identity on the entity at creation time, which would touch the `common`/
/// `server` logic crates (upstream-merge-sensitive, out of this shell-only
/// task's scope) — tracked as a follow-up, not attempted here.
#[derive(Resource, Debug, Default)]
pub struct PendingDimensionAttribution(pub VecDeque<DimensionId>);

/// Turns a (already-sanitized) `SpawningRules` — e.g. a `DmEvent`'s
/// `spawning_rules` field, the Mist-Bound-example schema's monster-population
/// directive (migration spec §5.1/§5.5) — into up to `spawn_count`
/// individual factory-spawn requests, each going through the exact same
/// [`spawn_entity_template`] staging-entity path a single template spawn
/// uses. This is the piece of "generic entity factory v1" that actually
/// turns an ORACLE event's spawn directive into a BATCH of NPCs;
/// [`apply_pending_entity_template_spawns`] (below) is what resolves each
/// staged request into a real sim NPC once this function has staged it.
///
/// ## Anti-chaos: no clamping happens HERE
/// `rules.spawn_count`/`rules.spawn_radius`/`rules.ai_behavior_override`/
/// `rules.entity_templates` were already clamped once, at ingestion, by
/// [`xindeler_oracle_host::dm_event::DmEvent::sanitize`] /
/// `SpawningRules::sanitize` (EM-4.4's "clamp on the way in, once" posture —
/// see that module's docs). This function only **floors** the still-`f32`
/// `spawn_count` to a whole number of entities; it never re-derives or
/// widens a bound itself, so a caller that skips sanitize first is not
/// protected by this function (documented, not silently patched over here).
///
/// ## `entity_templates` id resolution
/// `rules.entity_templates` names ids (cycling through the list if
/// `spawn_count` exceeds its length); an id with no match in the
/// caller-supplied `templates` lookup is skipped with a `warn!` rather than
/// aborting the whole batch — same "defuse, don't crash" posture as every
/// other anti-chaos path in this module.
///
/// ## `ai_behavior_override` is an OVERRIDE, not a default
/// `rules.ai_behavior_override` replaces every spawned template's OWN
/// `ai_behavior_override` unconditionally — that is the field's literal
/// purpose: an ORACLE event author dialing "everyone spawned by THIS event
/// uses this preset" (e.g. "these 15 wolves all stalk"), regardless of what
/// each template's author picked as ITS OWN default.
///
/// ## Determinism
/// Positions scatter uniformly within `rules.spawn_radius` of `origin`
/// (sim/world axes, matching [`PendingEntityTemplateSpawn::pos`]) via a
/// caller-supplied `rng` — deterministic given a seeded RNG, per the ORACLE
/// determinism invariant (this function itself reads no wall-clock/thread
/// entropy). Caveat: this covers ONLY this function's own scatter. Once a
/// staged request reaches [`apply_pending_entity_template_spawns`]'s
/// [`resolve_body`], `common::npc::NpcBody::from_str` may itself pick a
/// random species/body-type variant internally (pre-existing upstream
/// behavior, identical to `/spawn`'s own body resolution) — so a spawned
/// NPC's cosmetic body variant is not guaranteed reproducible across runs
/// even with the same seed here, only its position/count/behavior preset
/// are.
pub fn spawn_from_spawning_rules(
    commands: &mut Commands,
    templates: &HashMap<String, EntityTemplate>,
    rules: &SpawningRules,
    origin: [f32; 3],
    dimension: DimensionId,
    rng: &mut impl RngExt,
) -> Vec<Entity> {
    if rules.entity_templates.is_empty() {
        warn!("entity_factory: spawning_rules has no entity_templates listed; nothing to spawn");
        return Vec::new();
    }

    let count = rules.spawn_count.max(0.0).floor() as u32;
    let mut spawned = Vec::with_capacity(count as usize);
    for i in 0..count {
        let template_id =
            rules.entity_templates[(i as usize) % rules.entity_templates.len()].as_str();
        let Some(template) = templates.get(template_id) else {
            warn!(
                template_id,
                "entity_factory: spawning_rules named an entity_template id with no matching \
                 template; skipping this spawn"
            );
            continue;
        };

        let mut resolved = template.clone();
        resolved.ai_behavior_override = rules.ai_behavior_override.clone();

        let angle = rng.random_range(0.0..std::f32::consts::TAU);
        let dist = rng.random_range(0.0..=rules.spawn_radius.max(0.0));
        let pos = [
            origin[0] + angle.cos() * dist,
            origin[1] + angle.sin() * dist,
            origin[2],
        ];
        spawned.push(spawn_entity_template(commands, &resolved, pos, dimension));
    }
    spawned
}

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
    registry: Res<DimensionRegistry>,
    mut attribution: ResMut<PendingDimensionAttribution>,
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

        // BL-82 EM-4.9 (Phase C, T51.6): route into any dimension the
        // registry currently reports `Active` — DEFAULT is always accepted
        // (unchanged); a never-activated/already-torn-down/still-Spinup
        // target is a defensive drop, never a mis-spawn. See the module doc
        // comment for what "routed into dimension X" concretely means today
        // (one physical sim, mirror-tagged for interest-management scoping).
        if request.dimension != DimensionId::DEFAULT {
            let accepts = registry
                .get(request.dimension)
                .is_some_and(|state| state.lifecycle() == DimensionLifecycle::Active);
            if !accepts {
                warn!(
                    dimension = request.dimension.0,
                    "entity_template: target dimension is not registered/Active; dropping this \
                     spawn request"
                );
                continue;
            }
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

        // BL-82 EM-4.9: record the target dimension BEFORE queuing the
        // event, in request order, so `mirror_sim_entities` can attribute it
        // to the resulting sim entity once it materializes (see
        // `PendingDimensionAttribution`'s doc comment). Never pushed for
        // DEFAULT — that's the mirror's own unattributed fallback.
        if request.dimension != DimensionId::DEFAULT {
            attribution.0.push_back(request.dimension);
        }

        let wpos = vek::Vec3::new(request.pos[0], request.pos[1], request.pos[2]);
        sim.server.state().emit_event_now(CreateNpcEvent {
            pos: comp::Pos(wpos),
            ori: comp::Ori::default(),
            npc,
        });

        info!(
            body = body_name,
            ?preset,
            dimension = request.dimension.0,
            "entity_template: spawned a factory NPC through the sim's public event bus"
        );
    }
}
