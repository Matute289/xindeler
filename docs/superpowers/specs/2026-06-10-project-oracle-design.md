# PROJECT ORACLE: Autonomous World Director AI

**Date:** 2026-06-10
**Companion specs:** `2026-06-10-project-aurora-design.md` (NPC minds), `2026-06-10-world-difficulty-zones-design.md` (level bands), `2026-06-10-magic-abilities-design.md` (affix abilities), `2026-06-10-lore-cosmology-design.md` (canon)

## Context

This fork runs a private Veloren server (Rust, specs ECS, server-authoritative) with a custom
telemetry pipeline (`telemetry!` macro at `common/src/lib.rs:22`, JSON Lines sink in
`common/frontend/src/telemetry_layer.rs`). The vanilla world already has a long-running
simulation — rtsim (`rtsim/`) — that persists NPCs, sites, factions, reports, and quests across
restarts, plus a server-side weather simulation (`server/src/weather/`). What it lacks is
*direction*: nothing makes wars start, plagues spread, festivals happen, or stories conclude.
The closest thing is the Architect rule (`rtsim/src/rule/architect.rs`), whose stated job is
"making sure interesting stuff keeps happening" but which today only respawns dead NPCs.

PROJECT ORACLE is the World Director: an autonomous AI layer that generates world events, story
arcs, and global narratives; tracks consequences; and reacts to player and NPC actions while
maintaining coherence. It does **not** roleplay individual NPCs — that is PROJECT AURORA.

**Integration contract:** ORACLE decides *what happens* (a war, a plague, a festival). AURORA
decides *how inhabitants react* (dialogue, fear, profiteering, migration). The boundary is a
typed fact store both read from rtsim `Data`.

```
            +--------------------------------------------------+
            |                 PROJECT ORACLE                    |
            |  World State Graph -> Event Engine -> Narrative   |
            |  (rule core + async LLM proposer, never in tick)  |
            +-----------+--------------------------+-----------+
                        | world events             | reads observations
                        v                          |
            +-----------+-----------+    +---------+----------+
            |  WorldFacts (typed,   |    |  Telemetry stream  |
            |  persisted in rtsim   |    |  + rtsim events    |
            |  Data, append-only    |    |  (OnDeath, OnTheft |
            |  chronicle)           |    |   OnTick, ...)     |
            +-----------+-----------+    +---------+----------+
                        | consumed by               ^
                        v                           |
            +-----------+-----------+               |
            |    PROJECT AURORA     |               |
            |  NPC reactions, talk, |---------------+
            |  individual behavior  |   NPC deeds become observations
            +-----------+-----------+
                        v
                  NPCs <-> Players        (both ORACLE and AURORA read/write
                                           rtsim Data; ORACLE owns facts,
                                           AURORA owns per-NPC state)
```

## Goals

| # | Goal |
|---|---|
| G1 | A living world: wars, disasters, festivals, and arcs emerge from world state, not cron jobs |
| G2 | Consequences persist: every resolved event leaves typed facts and chronicle entries |
| G3 | Players matter: deeds tracked per region; fame/infamy; villains; legacy monuments |
| G4 | Monster ecology: populations grow, migrate, compete, and drift instead of static respawn |
| G5 | Seasons, moon phases, eclipses, and climate anomalies as gameplay-affecting systems |
| G6 | Every ORACLE decision observable and auditable via the fork's telemetry pipeline |

## Non-Goals

- Per-NPC dialogue, personality, or memory (AURORA's domain).
- Multiplayer-scale sharding now; this is a single-server fork (Section 15 keeps the door open).
- Player-built content moderation or UGC pipelines.
- Replacing worldgen: ORACLE mutates the *simulation* layer, never the deterministic seed terrain.

## Design Principles

1. **Server authority.** ORACLE runs inside the server process; clients only see results.
2. **LLM proposes, rules validate, sim executes.** The LLM never has write access to game state.
3. **Bounded autonomy.** Hard invariants (Section 13) cap what any event can do; everything has
   a kill-switch.
4. **Everything observable.** Every proposal, validation verdict, and state transition emits a
   `telemetry!` event; the chronicle is exportable.
5. **Deterministic where possible.** The rule core is seeded-RNG deterministic given the same
   observations; only LLM text/pitches are non-deterministic, and they are stored verbatim so a
   replay can reuse them.
6. **Tick-path purity.** No network or LLM call ever blocks the ECS tick; the pattern is the
   existing rtsim save thread (`server/src/rtsim/mod.rs:318`, crossbeam channel + worker thread).

## Current State Inventory (verified)

| Capability | Exists today? | Where | Gap for ORACLE |
|---|---|---|---|
| Persistent world sim | Yes | `rtsim/src/data/mod.rs:40` — `Data { nature, npcs, sites, factions, reports, architect, quests, tick, time_of_day, airship_sim }` | No event/fact/chronicle storage |
| Sim rules | Yes | `rtsim/src/rule/`: `architect`, `cleanup`, `migrate`, `npc_ai/`, `replenish_resources`, `report`, `simulate_npcs`, `sync_npcs`; server adds `deplete_resources` (`server/src/rtsim/rule/mod.rs`) | No director rule |
| Rtsim event bus | Yes | `rtsim/src/event.rs`: `OnSetup`, `OnTick`, `OnDeath`, `OnHelped`, `OnHealthChange`, `OnTheft`, `OnMountVolume`; server adds `OnBlockChange` (`server/src/rtsim/event.rs`) | No event for player deeds, trades, site capture |
| Sites | Yes | World sites in `world/src/site/` (`SiteKind`, `world/src/site/mod.rs:89`, 24 variants); rtsim mirror `rtsim/src/data/site.rs:15` | rtsim `Site` has no prosperity/stability; pop is transient |
| Factions | Yes | `rtsim/src/data/faction.rs:10` — `{ seed, leader, good_or_evil, sentiments }` | No power, territory, or inter-faction relations |
| Reports (rumors) | Yes | `rtsim/src/data/report.rs:51` — `ReportKind::{Death, Theft}` only | No event-driven reports |
| Quests | Yes | `rtsim/src/data/quest.rs:321` — `QuestKind::{Escort, Slay, Courier}` | No arc structure, no world-event linkage |
| Monster population | Partial | Architect tracks `TrackedPopulation` (19 categories incl. `GigasFrost`, wyverns; `rtsim/src/data/architect.rs:22`), respawns after `MIN_SPAWN_DELAY` = 1 in-game day (`rtsim/src/rule/architect.rs:29`) | Global counts only — no regions, growth, predation, migration, evolution |
| Weather | Yes | `server/src/weather/sim.rs` — cell grid (cloud/rain/wind), humidity from worldgen chunks, `add_zone` override (`sim.rs:70`), lightning cells; `/weather_zone` admin cmd (`server/src/cmd.rs:240`) | Weather is noise-driven; no climate anomalies, no economy coupling |
| Weather rendering | Yes | `voxygen/src/render/pipelines/{clouds,rain_occlusion,skybox}.rs` | — |
| Time & calendar | Partial | `TimeOfDay` resource; day length config (`server/src/settings/mod.rs:341`); `CalendarEvent::{Christmas, Halloween, AprilFools, Easter}` from real-world dates (`common/src/calendar.rs:8`) | No in-game seasons or fictional calendar |
| Moon | Partial | One moon rendered: `get_moon_dir` (`common/src/resources.rs:27`), `MoonPeriod` (`common/src/time.rs:37`), sky shader `assets/voxygen/shaders/include/sky.glsl` | No phases, eclipses, comets; zero gameplay coupling |
| Seasons | No | Only `ItemSpec::Seasonal` keyed to CalendarEvent (`common/src/comp/inventory/loadout_builder.rs:53`) | Everything |
| Dungeons | Static | Generated at worldgen from `world/src/site/plot/` (47 plot types: `gnarling.rs`, `cultist.rs`, `vampire_castle.rs`, ...) | No runtime creation; see Section 7 |
| Site economy | Yes | `world/src/site/economy/` (production/consumption per site) | No external shock inputs |
| Terrain edits | Yes | `server/src/terrain_persistence.rs:25` — per-block overrides applied on chunk load (`apply_changes:64`, `set_block:215`) | Block-level only; fine for monuments/seals |
| Telemetry | Yes (fork) | `telemetry!` macro (`common/src/lib.rs:22`) → `TelemetryLayer` JSONL | Needs ORACLE event-type codes |
| Server game events | Yes | `server/src/events/mod.rs:55` `ServerEvent` trait + dispatch (`register_event_systems:105`) | ORACLE hooks into handlers |
| Downtime catch-up | No | `server/src/rtsim/mod.rs:50` loads `Data` and resumes at saved `time_of_day`; no elapsed-time sim | Section 12 |

## 1. World State Engine

### 1.1 Placement

ORACLE state lives inside rtsim `Data` as a new serialized field, following the `architect`
precedent (`rtsim/src/data/mod.rs:55`):

| New module | Contents |
|---|---|
| `rtsim/src/data/oracle/mod.rs` | `pub struct OracleData { world_graph, facts, chronicle, events, ecosystem, climate, narrative, players }` (all `#[serde(default)]` — no `CURRENT_VERSION` bump needed) |
| `rtsim/src/data/oracle/world_graph.rs` | Region/settlement/faction/resource/religion nodes |
| `rtsim/src/data/oracle/facts.rs` | Typed `WorldFact` store (the AURORA interface) |
| `rtsim/src/data/oracle/chronicle.rs` | Append-only history with compaction |
| `rtsim/src/rule/oracle/` | Director rules registered in `start_default_rules` (`rtsim/src/lib.rs:199`) |
| `server/src/oracle/` | LLM proposer thread, validator, admin commands, telemetry codes |

### 1.2 World State Graph

| Node | Key fields | Source of truth |
|---|---|---|
| `Region` | id, bounds (weather-cell aligned), biome profile (avg `SimChunk.temp/humidity`, `world/src/sim/mod.rs:2510`), tension 0..1, climate state | Derived once from worldgen, mutable sim state on top |
| `Settlement` | `SiteId` link, prosperity 0..1, population (head-count from `Site.population` + persistent census), stability 0..1, garrison strength | Extends `rtsim/src/data/site.rs:15` |
| `FactionNode` | `FactionId` link, power score, territory (site set), relations matrix (-1..1 per faction pair), war state | Extends `rtsim/src/data/faction.rs:10`; replaces `good_or_evil` over time |
| `ResourceNode` | per-region aggregates of `Nature` chunk resources (`rtsim/src/data/nature.rs:14`) + crop-yield modifier | Read from `deplete_resources`/`replenish_resources` rules |
| `Religion` | id, deity (from lore bible), follower estimate per region, fervor | New; seeded from `2026-06-10-lore-cosmology-design.md` |

Edges: `controls(faction, settlement)`, `trades(settlement, settlement)` (from
`world/src/site/economy/` neighbor data), `worships(settlement, religion)`,
`at_war(faction, faction)`.

### 1.3 Chronicle (historical record)

Append-only `Vec<ChronicleEntry>`:

```rust
pub struct ChronicleEntry {
    pub id: u64,                  // monotonic
    pub at: TimeOfDay,
    pub kind: ChronicleKind,      // EventStarted, EventResolved, FactionFell, PlayerDeed, ...
    pub causes: Vec<u64>,         // ids of prior entries — the causal chain
    pub subjects: Vec<Subject>,   // Actor / SiteId / FactionId / RegionId
    pub summary_key: String,      // i18n key; LLM prose stored separately (Section 10)
}
```

The causal chain is what makes "historical records" possible: a library NPC (via AURORA) can
walk `causes` backwards and narrate why the war started. Compaction policy in Section 15.

### 1.4 Change detection

| Signal | Hook point |
|---|---|
| NPC deaths, thefts, heals | Existing rtsim events (`rtsim/src/event.rs`) — already bound by rules via `rtstate.bind` |
| Player combat/death/trade | New `telemetry!` consumers + direct hooks in `server/src/events/entity_manipulation.rs` and `server/src/events/trade.rs` handlers (same place the fork's telemetry macros already sit) |
| Resource depletion | `server/src/rtsim/rule/deplete_resources.rs` emits deltas to `ResourceNode` |
| Block changes (sabotage, building) | `OnBlockChange` (`server/src/rtsim/event.rs`) |
| Site population shifts | `Architect` death ledger (`rtsim/src/data/architect.rs:137`) |

A new `rtsim/src/rule/oracle/world_state.rs` rule folds these into graph updates every N ticks
(N=32, matching `ARCHITECT_TICK_SKIP`, `rtsim/src/rule/architect.rs:27`).

## 2. Event Engine

### 2.1 Taxonomy

| Class | Examples | Mechanical effects |
|---|---|---|
| Military | war, rebellion, invasion, siege | faction relations, garrison spawns, site control flips |
| Political | succession crisis, assassination, coup | leader changes (`Faction.leader`), stability drops |
| Economic | trade collapse, famine, gold rush | economy input shocks (`world/src/site/economy/`), price drift via AURORA merchants |
| Natural | earthquake, flood, drought, wildfire | climate state (Section 9), resource modifiers, block-level scars via `TerrainPersistence` |
| Magical | mana storm, planar breach, curse | global buff modifiers (`common/src/comp/buff.rs:31`), spawn-table overrides, affix monsters |
| Religious | revival, schism, crusade, omen | religion fervor, festival scheduling, AURORA sermon topics |
| Festive | harvest festival, royal wedding, games | safe-zone buffs, merchant discounts, decoration sprites |
| Ecological | migration wave, apex predator, blight | ecosystem pressure injections (Section 6) |

### 2.2 Lifecycle state machine

```
Proposed --validate--> Validated --schedule--> Scheduled --trigger--> Active
   |  (LLM or rule          |  (pacing director       (stage 1..k escalation,
   |   generated)           |   picks start time,      each stage re-validated)
   v                        v   cooldowns)                  |
Rejected (logged)      Expired (preconditions               v
                        broke before start)            Resolving --> Resolved
                                                            |
                                                            v
                                                  Consequences: WorldFacts
                                                  written + chronicle entries
```

Stored as `enum EventState` on `WorldEvent` in `rtsim/src/data/oracle/events.rs`. Transition
authority: only `rtsim/src/rule/oracle/event_engine.rs` mutates state; LLM and admin commands
enqueue *requests*.

### 2.3 Causal model: typed facts + rule engine

`WorldFact` is the lingua franca with AURORA:

```rust
pub enum WorldFact {
    AtWar { a: FactionId, b: FactionId, since: TimeOfDay },
    SiteControlled { site: SiteId, by: FactionId },
    Plague { region: RegionId, severity: f32 },
    FoodShortage { site: SiteId, severity: f32 },
    FestivalActive { site: SiteId, kind: FestivalKind, ends: TimeOfDay },
    BountyOn { actor: Actor, gold: u32, reason_chronicle_id: u64 },
    OmenSighted { region: RegionId, omen: OmenKind },
    // ... one variant per consequence class; versioned enum, serde-tolerant
}
```

Event templates declare preconditions and effects over facts and graph queries, evaluated by a
small forward-chaining rule engine (no external deps; a `Vec<Precondition>` of typed predicates
like `RelationBelow(a, b, -0.5)`, `ProsperityAbove(site, 0.6)`). Effects are fact insertions/
retractions plus mechanical hooks (spawn orders to Architect, weather zones, buff auras).
**AURORA contract:** AURORA reads `OracleData.facts` (read-only) each agent think-tick and
submits `Observation`s (NPC deeds, player conversations flagged as significant) to a bounded
queue ORACLE drains — never direct writes.

### 2.4 Pacing director

Per-region tension score in `Region.tension`, raised by negative events/player heat, decayed
over in-game days. The director fits scheduling to a target tension curve (calm → rising →
climax → cooldown) per region, with: per-class cooldowns (no two wars in a region within 30
in-game days), global event-density cap (Section 13), and "spotlight fairness" — regions with
recent player presence (from telemetry position events) get priority for visible events,
empty regions get cheap simulated-only events.

## 3. Dynamic Monster Ecosystem

Extends the Architect rather than replacing it: Architect remains the *executor* (it already
owns spawn plumbing, `rtsim/src/rule/architect.rs:55`), ORACLE's ecosystem model becomes the
*planner* that writes `wanted_population` instead of the static startup computation
(`rtsim/src/data/architect.rs:144`).

### 3.1 Population model

Per region r and species-group s (extending `TrackedPopulation`, `rtsim/src/data/architect.rs:22`):

```
N[r,s] += dt_days * ( growth[s] * N[r,s] * (1 - N[r,s]/K[r,s])   // logistic growth
                    - sum_p pred[p,s] * N[r,p] * N[r,s] / K[r,s] // predation (discrete L-V)
                    + migration_in[r,s] - migration_out[r,s] )   // pressure-driven
```

- `K[r,s]` (carrying capacity) derived from biome profile: `SimChunk.temp`, `humidity`,
  `tree_density` (`world/src/sim/mod.rs:2510`) aggregated per region, modulated by climate
  state (drought halves herbivore K) and season.
- `pred[p,s]` is a hand-authored sparse predation matrix in
  `assets/common/oracle/predation.ron` (data-driven like entity configs).
- Migration: when `N[r,s] > 0.8*K[r,s]`, surplus flows to the adjacent region with highest
  `K - N`; ORACLE may emit a visible `MigrationWave` event when the flow is large.
- Territorial conflict: two apex groups above threshold in one region triggers an `Ecological`
  event (visible lair fights, temporary danger zone fact for AURORA guards to warn about).
- Update cadence: once per in-game hour inside the ORACLE tick rule — integer counts, cheap.

### 3.2 Mutation / evolution drift

Each region-species carries a `DriftProfile { stat_bias: Vec<(StatKind, f32)>, ability_pool }`.
Survivor-weighted: species frequently killed by players in a region drift toward the defensive
stats of the variants that lived longest (computed from death telemetry). Drift is capped at
±15% per stat and resets if the population collapses (no Lamarckian runaway).

### 3.3 Variant system

Concrete mechanism: entity-config templating. Spawns already go through `EntityConfig`
(`common/src/generation.rs:120`) with body/loadout/loot fields. Add a
`VariantOverlay { name_suffix, stat_multipliers, affixes: Vec<AbilityAffixId>, loot_bonus }`
applied at spawn time by the Architect executor:

| Variant | Trigger | Mechanics |
|---|---|---|
| Elite | random 2–5% of spawns, scaled by region tension | +30–60% HP/damage, 1 affix, loot bonus |
| Regional | permanent per-region drift (3.2) | drift stat biases + cosmetic name ("Ashland Troll") |
| Legendary | unique, chronicle-tracked, respawns only via event | named, 2–3 affixes, map rumor fact for AURORA |
| Seasonal | active season/calendar window | themed affix (frostbound in winter) |
| Event | spawned by an Active event stage | event-themed affixes, despawn on Resolved |

Stat bands respect the level bands in `2026-06-10-world-difficulty-zones-design.md`; affix
abilities draw from the pool defined in `2026-06-10-magic-abilities-design.md`.

## 4. Dynamic Dungeons

Hard constraint, stated honestly: terrain is generated deterministically from the world seed;
sites are placed at worldgen (`world/src/site/`, plots in `world/src/site/plot/`) and there is
no runtime site creation. rtsim explicitly links to worldgen sites and notes the dependency
(`rtsim/src/data/site.rs:30`). Three options:

| Option | How | Cost | Risks | Verdict |
|---|---|---|---|---|
| A. Pre-reserved dormant sites | Worldgen places N extra dungeon sites per region flagged `dormant`: sealed entrance (solid plug blocks), no spawn population. Runtime "discovery" event unseals via `TerrainPersistence::set_block` (`server/src/terrain_persistence.rs:215`) and tells Architect to populate | M — worldgen flag + unseal routine + spawn orders | World map regeneration needed once (seed-compatible additive change is impossible — site placement shifts; requires the planned map regen for difficulty zones anyway); finite supply of dungeons per map | **Recommended**, Phase-gated |
| B. Instanced pocket planes | Portals teleport players to reserved far-off map margins (engine has one world, no instancing); each "plane" is a pre-generated arena reused round-robin, themed at runtime via block overrides + spawns. Reuses the planes design from the lore/magic specs | L — portal mechanic, reservation manager, anti-overlap bookkeeping | Not true instancing: two groups can collide; map edges have biome constraints; travel/respawn edge cases | Phase 2 of dungeons, for event climaxes only |
| C. Runtime terrain modification | Carve whole dungeons via persisted block overrides | XL | `TerrainPersistence` stores per-block diffs — a 60×60×30 dungeon is ~100k overrides applied on every chunk load (`apply_changes`, `terrain_persistence.rs:64`); memory + load-time cost, no structural reuse of plot generators (they run inside worldgen canvas, not on live terrain) | Rejected for dungeons; kept for small scars/monuments |

**Do first (cheapest, no map change): dungeon invasions.** Existing dungeon sites
(`SiteKind::{Gnarling, Cultist, Sahagin, Haniwa, VampireCastle, Myrmidon, ...}`,
`world/src/site/mod.rs:89`) get repopulated with a different faction by an event: Architect
spawn orders swap the entity configs, ORACLE writes `SiteControlled` facts, AURORA NPCs gossip
about it. Zero terrain work, immediate novelty.

**Boss evolution:** a dungeon boss that wipes a party or survives an event gains one affix
(capped at 3) and a chronicle entry; its `Legendary` variant record persists in
`OracleData.ecosystem` so the *same named boss* returns stronger.

## 5. Astronomical Simulation

### 5.1 Seasons — feasibility analysis

Worldgen bakes climate at generation: `SimChunk.temp/humidity` are computed once from noise
(`temp_nz`/`humid_nz`, `world/src/sim/mod.rs:120,122`) and never re-evaluated; chunk meshes are
generated from that. Therefore *terrain-visual* seasons (snow-covered forests) require
re-meshing loaded chunks with a season-dependent sampling offset — possible (the sampler can add
a `season_temp_offset` before block selection) but every season flip forces full chunk
regeneration client- and server-side. Honest phasing:

| Layer | Mechanism | Cost |
|---|---|---|
| S1 Gameplay seasons | `Season` derived from `TimeOfDay` (year = 96 in-game days, 4 seasons; day length configurable via `server/src/settings/mod.rs:341`); modifies weather sim humidity/pressure constants (`server/src/weather/sim.rs:124`), crop yield, ecosystem K, spawn tables | S |
| S2 Sky & ambience | Sun path tilt, color grading, audio sets per season (client reads synced `Season` resource) | M |
| S3 Terrain visuals | Season offset in worldgen sampler + staged re-mesh over several real minutes at season boundaries | XL — deferred to Optimization phase, may stay cut |

### 5.2 Moon, eclipses, comets

A single moon already orbits and lights the night (`get_moon_dir`,
`common/src/resources.rs:27`; `MoonPeriod`, `common/src/time.rs:37`; rendered via
`assets/voxygen/shaders/include/sky.glsl` and `voxygen/src/render/pipelines/skybox.rs`). Add:

- **Phases:** `MoonPhase` computed from `TimeOfDay` over a 8-in-game-day cycle; shader gets a
  phase uniform (crescent masking); full moon raises night-monster spawn weights and fuels
  lunar affixes; new moon boosts stealth (AURORA guards' detection fact).
- **Eclipses:** scheduled by ORACLE (not orbital mechanics — authored rare events that *look*
  orbital): sun-darkening shader state + a global `OmenSighted` fact + magic amplification
  buff window. These are Event-class objects with the full lifecycle.
- **Comets:** skybox object with a multi-day visibility window; pure omen/foreshadowing device
  the Narrative Director uses to telegraph an upcoming arc climax.

Global modifiers travel as a new `CelestialState` resource synced via `common-net` message
(same pattern as the weather grid sync), consumed server-side by spawn/buff systems and
client-side by the sky shader.

## 6. Climate Simulation

Builds directly on `WeatherSim` (`server/src/weather/sim.rs`): today weather is stateless noise
shaped by static per-cell humidity (`CellConsts`, `sim.rs:21`) plus temporary override zones
(`add_zone`, `sim.rs:70` — already exposed as the `/weather_zone` admin command,
`common/src/cmd.rs:463`).

ORACLE adds a persistent `ClimateState` per region in `rtsim/src/data/oracle/climate.rs`:

| Anomaly | Sim effect | World effect |
|---|---|---|
| Drought | humidity const −60%, no rain zones | crop yield −50% → `FoodShortage` facts → AURORA price/migration reactions; wildfire event precondition |
| Flood | sustained rain zones, lightning up | river-adjacent settlement prosperity −, disease event precondition |
| Heatwave | clear skies, temp modifier | ecosystem K shifts; desert species migrate outward |
| Harsh winter (seasonal) | snow/storm zones | trade route slowdowns (AURORA caravans), wolf migration toward settlements |

Anomalies are events (full lifecycle, Section 2.2) whose Active stages drive `add_zone` calls
and whose Consequences write economy input shocks consumed by `world/src/site/economy/`
context and by AURORA's merchant simulation.

## 7. Narrative Director

### 7.1 Arc templates and beats

`rtsim/src/data/oracle/narrative.rs` defines `Arc { template, scope, beats, state }`:

| Scope | Example | Typical length |
|---|---|---|
| Main | "The Sundered Crown" — succession war spanning 3 factions | 30–60 in-game days |
| Regional | bandit warlord rises in one region | 8–15 days |
| Faction | schism inside the merchant guild | 10–20 days |
| Personal | a player's villain arc / nemesis (Section 8) | reactive |

Beats follow setup → rising (2–4 escalations) → climax → aftermath. Each beat binds 1–3 events
from Section 2 plus fact preconditions; the director only advances a beat when its events
resolved and pacing allows.

### 7.2 Spawning and coherence

Arcs spawn from world-state triggers (e.g., two factions below −0.6 relations + contested
border site) weighted by player heat (telemetry presence). **Coherence:** every arc pitch is
checked against canon constraints exported from `2026-06-10-lore-cosmology-design.md` as a
machine-readable file `assets/common/oracle/canon.ron` (deity names, dead characters,
geographic invariants, banned anachronisms). A rule-based consistency checker rejects pitches
that contradict facts or canon (e.g., proposing a faction that fell — chronicle lookup).

### 7.3 LLM vs rules split

| Task | Owner |
|---|---|
| Arc pitch generation (theme, antagonists, stakes) | LLM (async, Section 14) |
| Beat/event selection, scheduling, preconditions | Rules |
| Mechanical effects (spawns, buffs, facts) | Rules/sim only |
| Chronicle prose, proclamations, rumor text | LLM, post-hoc, cached per chronicle id |
| Consistency/canon validation | Rules first; LLM critic pass as advisory second opinion |

## 8. Player Impact

- **Deed ledger:** `OracleData.players` keyed by character id: kills (what/where), quests,
  event participation, thefts (rtsim `OnTheft` already fires), boss kills — sourced from the
  same server event handlers the fork's telemetry instruments
  (`server/src/events/entity_manipulation.rs`).
- **Fame/infamy:** per-region scalar pair, decaying slowly; thresholds unlock facts
  (`BountyOn`, `LocalHero`) that AURORA turns into greetings, discounts, or guard hostility.
- **Villain systems:** players who murder NPCs/players accumulate infamy; ORACLE escalates:
  bounty facts → AURORA bounty-hunter NPC dispatch → regional `Manhunt` event → personal arc
  with a named nemesis hunter (Legendary-variant human).
- **Legacy:** world-first boss kills and arc-deciding deeds produce: a chronicle entry, a
  named-location overlay on the world map (client-side label layer), and a small monument
  (statue plinth + plaque sprite) placed via `TerrainPersistence::set_block` near the relevant
  settlement plaza — bounded to <200 blocks each, the one sanctioned use of Option C.

## 9. Time Simulation

- **Live:** ORACLE rules run inside the rtsim tick (`server/src/rtsim/tick.rs`), strided like
  the Architect (every 32 ticks) with an internal per-system budget; heavy phases (ecosystem
  solve, pacing optimization) are amortized across strides. Target: p95 < 2 ms per ORACLE
  stride at current world size.
- **Downtime (verified gap):** on restart the server loads rtsim data and resumes at the saved
  `time_of_day` (`server/src/rtsim/mod.rs:50`) — no elapsed-time simulation exists. Design:
  **catch-up sim** at boot, after `OnSetup` — compute real downtime, convert via
  `day_cycle_coefficient` (`server/src/settings/mod.rs:341`), and run coarse ORACLE-only ticks
  (events, ecosystem, climate; no NPC pathing) at 1-in-game-hour resolution, capped at 7
  in-game days. Beyond the cap, time is considered "quiet" (chronicled as such). All catch-up
  entries flagged `simulated_offline` for auditability.
- **Acceleration:** the same coarse-tick machinery doubles as an admin tool
  (`/oracle fastforward <days>`) for testing and soak sims (Section 17).

## 10. Anti-Chaos Safeguards

| Invariant | Limit | Enforcement |
|---|---|---|
| Faction map control | no faction > 40% of settlements | validator rejects/auto-resolves wars that would exceed it |
| Settlement destruction | a site can be *occupied* or *depressed*, never deleted | effects API simply has no delete |
| Economy circuit breaker | aggregate prosperity drop > 25%/in-game-week → freeze Economic+Natural event classes | canary metric (10.2) |
| Event density | ≤ 1 Active visible event per region; ≤ 4 world-wide; class cooldowns | pacing director hard caps |
| Player-hostile pile-up | bounty/nemesis pressure on one player capped (1 active nemesis) | validator |
| Narrative kill-switch | `/oracle pause` halts Proposed→Scheduled transitions; Active events run to Resolving | admin cmd in `server/src/cmd.rs` (pattern: `handle_weather_zone`, `server/src/cmd.rs:6126`) |
| LLM output | proposals are data (RON/JSON against a schema), never code; schema-validated, then rule-validated | `server/src/oracle/validate.rs` |

**Canary metrics + auto-rollback:** ORACLE tracks rolling baselines (NPC population, prosperity,
player session length, deaths/hour from telemetry). A breach quarantines the most recent
event's effects: facts are retracted (each effect records its inverse), spawn orders cancelled,
chronicle entry marked `annulled`. Breaches emit `telemetry!("oracle_canary", ...)` and a
server log alert.

## 11. AI Architecture Summary

| Layer | Contents | Sync/async |
|---|---|---|
| World State Graph | regions/settlements/factions/resources/religions + facts | in-tick, owned by rules |
| Event Graph | event instances, lifecycle, causal links to facts and chronicle | in-tick |
| Narrative Graph | arcs → beats → events; canon constraints | in-tick |
| Causal reasoning core | forward-chaining typed-predicate engine; seeded RNG; deterministic | in-tick, budgeted |
| Simulation executors | existing rtsim rules (Architect spawns, weather zones, buffs) | in-tick |
| LLM proposer | arc pitches, event flavor, chronicle prose; consumes a world-state digest, returns schema-validated proposals | **async worker thread** (crossbeam channel, mirrors `server/src/rtsim/mod.rs:318` save thread); batch every 5–15 real minutes |

LLM operations: batched and cached; cost model assumes a small-model tier for prose
(~1k tokens/event) and a stronger tier for arc pitches (~4k tokens, few per day) — at private-
server scale this is dollars per month, not per hour. A local-model option (llama.cpp server
behind the same HTTP trait in `server/src/oracle/llm.rs`) keeps the system functional offline;
with the LLM disabled entirely, ORACLE degrades gracefully to template-text events — the rule
core never depends on the LLM.

## 12. Scalability Stance

Single-server fork; design for hundreds of concurrent players and years of sim time, with
clean seams for more:

- **Event compaction:** Resolved events older than 60 in-game days collapse into their
  consequence facts + one chronicle entry.
- **Chronicle archival:** entries beyond a cap (50k) stream to JSONL files beside the telemetry
  logs (same `BoundedWriter` infra, `common/frontend/src/bounded_writer.rs`); in-memory keeps a
  summary index so historians (AURORA) can still answer about archived eras coarsely.
- **Region sharding boundary (future):** all per-region state is keyed by `RegionId` with no
  cross-region mutable references except the fact store — the eventual shard seam.
- Per-tick cost is independent of player count (observations are queued/sampled), and the
  ecosystem solve is O(regions × species-groups), both small constants.

## 13. Roadmap

One senior dev + AI assistance. Sizes: S ≤ 3 dev-days, M ≤ 8, L ≤ 15, XL > 15.

| Phase | Deliverables | Key tasks (file-level) | Complexity | Risks |
|---|---|---|---|---|
| 1. World State Engine | graph, facts, chronicle, change detection, telemetry codes | `rtsim/src/data/oracle/{mod,world_graph,facts,chronicle}.rs`; rule `rtsim/src/rule/oracle/world_state.rs`; hooks in `server/src/events/{entity_manipulation,trade}.rs`; `/oracle status` in `server/src/cmd.rs` | **L (12d)** | rtsim serde compat — mitigated by `#[serde(default)]` field, no version bump |
| 2. Event Engine | lifecycle, 8 event classes (2 templates each), validator, pacing director, admin inject/veto | `rtsim/src/data/oracle/events.rs`; `rtsim/src/rule/oracle/event_engine.rs`; `server/src/oracle/validate.rs`; templates in `assets/common/oracle/events/*.ron` | **XL (18d)** | effect-inverse bookkeeping for rollback; start with idempotent fact effects |
| 3. Monster Ecosystem | regional populations, L-V dynamics, migration, drift, variant overlay | extend `rtsim/src/data/architect.rs` (regional `Population`); `rtsim/src/rule/oracle/ecosystem.rs`; `VariantOverlay` in `common/src/generation.rs` + Architect spawn path; `assets/common/oracle/predation.ron` | **L (14d)** | balance explosions — clamp + soak-test (Sec. 17); coordinate with difficulty-zones bands |
| 4. Climate | climate states, 4 anomalies as events, economy shock inputs | `rtsim/src/data/oracle/climate.rs`; seasonal/anomaly modifiers in `server/src/weather/sim.rs`; shock inputs into `world/src/site/economy/context.rs` | **M (7d)** | weather sync perf unchanged (grid already synced) |
| 5. Astronomy | Season type (S1+S2), moon phases, eclipse/comet events, `CelestialState` sync | `common/src/calendar.rs` (in-game seasons alongside real-date events); `common/src/time.rs` (`MoonPhase`); sync message in `common-net`; shader uniforms `assets/voxygen/shaders/include/sky.glsl`; `voxygen/src/render/pipelines/skybox.rs` | **L (12d)** | S3 terrain visuals explicitly out; client/server proto bump |
| 6. Narrative Director | arc templates ×6, beat scheduler, canon checker, LLM proposer thread + prose cache | `rtsim/src/data/oracle/narrative.rs`; `rtsim/src/rule/oracle/narrative.rs`; `server/src/oracle/llm.rs`; `assets/common/oracle/canon.ron` | **XL (16d)** | LLM quality variance — schema validation + advisory critic + template fallback |
| 7. Player Impact | deed ledger, fame/infamy, villain pipeline, legacy monuments, dungeon invasions + boss evolution | `rtsim/src/data/oracle/players.rs`; bounty/nemesis events; monument placement via `server/src/terrain_persistence.rs`; invasion templates over `world/src/site/mod.rs` dungeon kinds | **L (13d)** | griefing loops — invariant caps from Sec. 10 first |
| 8. Optimization & Dormant Dungeons | catch-up sim, compaction/archival, perf budget enforcement, pre-reserved dormant sites (with the difficulty-zones map regen), instanced-plane pilot | boot catch-up in `server/src/rtsim/mod.rs`; archival via `common/frontend/`; worldgen dormant flag in `world/src/site/mod.rs` + unseal routine | **XL (18d)** | map regeneration is a world-reset-class operation — schedule with the difficulty-zones rollout |

Total ≈ 110 dev-days. Phases 1–2 are the spine; 3–7 are parallelizable after 2; 8 last.

## 14. Testing & Success Metrics

**Testing**

- **Headless soak sims:** `veloren-server-cli` with `/oracle fastforward` driving 365 in-game
  days nightly in CI; assert invariants (Section 10) never breach, populations stay within
  [0.2K, 1.2K], no panics. Run with `VELOREN_ASSETS` set as in existing tests.
- **Event-injection harness:** `/oracle inject <template> <region>` and `/oracle veto <id>`
  admin commands (registered like `WeatherZone`, `common/src/cmd.rs:463`) drive scripted
  scenario tests; unit tests for the predicate engine live in `rtsim/src/rule/oracle/`.
- **Chronicle audits:** nightly job validates causal-chain integrity (every `causes` id exists,
  no cycles, annulled events have retracted facts).
- **Telemetry dashboards:** ORACLE emits `telemetry!("oracle_*", ...)` events; the fork's
  existing JSONL tooling (veloren-telemetry skill) graphs tension curves, event density,
  canary baselines.

**Success metrics**

| Metric | Target |
|---|---|
| Visible events per region per in-game week | 1–3, never 0 for player-occupied regions |
| Invariant breaches per 365-day soak | 0 |
| ORACLE stride p95 | < 2 ms |
| LLM proposal rejection rate | < 30% (higher means prompt/digest drift) |
| Player-named chronicle entries per active player per week | ≥ 1 after Phase 7 |

## Open Questions

1. Year length and season boundaries (96 in-game days assumed) — needs playtest feel-check
   against the configurable `day_length`.
2. Should facts sync to clients for UI (world-map event overlay), or stay AURORA-mediated only?
   Leaning overlay-after-Phase-2, behind a feature flag.
3. Dormant-site density per region for the Phase 8 map regen — decide jointly with the
   difficulty-zones spec to avoid two map resets.
4. Whether AURORA's observation queue needs back-pressure priorities (player-adjacent
   observations first) — revisit with AURORA's Phase 2 throughput numbers.
