# PROJECT AURORA: AI-Driven Social Simulation Layer

**Date:** 2026-06-10
**Companion specs:** `2026-06-10-project-oracle-design.md` (world director), `2026-06-10-character-levels-design.md` (progression/reward scaling)
**Layer:** rtsim (primary), server bridge, common types

## Context

This fork's long-term goal is a living-world MMO: NPCs as autonomous inhabitants with memories, relationships, families, property, and institutions — not quest dispensers. AURORA is the *inhabitant* layer. It divides responsibility with PROJECT ORACLE as follows:

- **ORACLE** is the world director: it decides *what happens* at the macro level (crises, wars, festivals, plagues, trade embargoes) by reading world telemetry and issuing directives.
- **AURORA** is *how inhabitants live and react*: per-NPC minds, social graphs, families, organizations, local economies, and the dynamic quests that emerge from NPC state.

ORACLE never manipulates individual NPCs directly; AURORA never invents global events on its own. The contract between them is defined in [ORACLE Integration Contract](#oracle-integration-contract).

Veloren's `rtsim` crate already provides a remarkably strong substrate: persistent NPCs with Big Five personalities, a decaying sentiment system, an information-propagation system (reports), a quest board with escrow deposits, an action-combinator AI framework, and a population-managing architect. AURORA is designed as an *extension* of these systems, not a replacement. Every existing-system claim below was verified against the codebase at commit `b3bd45f467`.

## Goals

1. NPCs form opinions, friendships, rivalries, and romances with players and each other, grounded in observed events.
2. NPCs marry, have children, raise families, age, die, and pass on property and traits.
3. NPCs join, found, and run organizations: guilds, religions, cults, criminal rings, mercenary companies, merchant guilds, noble houses, political factions.
4. Quests are generated from genuine NPC needs and grievances, validated for solvability, and rewarded per the character-levels spec.
5. A dynamic site-level economy (supply/demand, production, merchant routes, price signals) replaces the static worldgen-only snapshot.
6. LLM-generated text gives conversations, charters, and rumors color — without an LLM in any tick path.

## Non-Goals

- **Not** replacing the existing `npc_ai` action combinators, dialogue system, or quest arbiter pattern — we extend them.
- **Not** simulating player characters: AURORA models NPC↔NPC and NPC→player edges; player→NPC state stays in existing server persistence.
- **Not** a multi-shard distributed simulation in v1 (single server first; see [Scale & Performance](#scale--performance)).
- **Not** voice, animation, or rendering work — `voxygen` consumes existing `DialogueKind` messages unchanged.
- **Not** an external database. rtsim's single-file MessagePack persistence remains the source of truth in v1.

## Design Principles

| Principle | Consequence |
|---|---|
| Server authority | All AURORA state lives in `rtsim::data::Data` on the server. Clients see only dialogue, chat, and behavior. |
| Graceful degradation | Every LLM-backed feature has a deterministic template fallback. If the LLM service is down, the world keeps running, just blander. |
| No per-NPC LLM calls in the hot path | Tick decisions are utility scores and combinators. LLM output is generated asynchronously, cached, and consumed as data. |
| Determinism where possible | Simulation rules use seeded `ChaChaRng` (existing pattern, `rtsim/src/rule/cleanup.rs:21`); LLM text affects *presentation*, never *outcomes*. |
| Budgeted memory | Every per-NPC store has a hard cap enforced by the existing `cleanup` rule pattern, with explicit byte budgets (see [Memory Budget](#memory-size-budget)). |
| Extend, don't fork upstream types | New data hangs off new fields with `#[serde(default)]` (existing migration pattern, `rtsim/src/data/npc.rs:299`), so `CURRENT_VERSION` (10) need not be bumped. |

## Current State — Verified Inventory

What exists today, verified by reading the code:

| Capability | Exists? | Where | Gap for AURORA |
|---|---|---|---|
| Persistent NPCs | Yes | `rtsim/src/data/npc.rs` — `Npc { uid, seed, wpos, body, role, home, faction, health_fraction, known_reports, personality, sentiments, job }` | No age, no persisted name (generated from seed, `get_name()`), no family, no property |
| Big Five personality | Yes | `common/src/rtsim.rs:92` — 5×u8 + 16 derived `PersonalityTrait`s | No values/beliefs/fears/goals; no mood; no morality axes |
| Sentiments w/ decay | Yes | `rtsim/src/data/sentiment.rs` — `Target {Character, Npc, Faction}` → i8 positivity; stochastic decay (POSITIVE ≈26 min … HERO/VILLAIN ≈47 h); caps 128/NPC, 1024/faction | Single scalar per target; everything decays to zero — friendships cannot be permanent; no typed relationships |
| Event knowledge (reports) | Yes | `rtsim/src/data/report.rs` — `ReportKind::{Death{actor,killer}, Theft{thief,site,sprite}}`, remembered 1.5–15 in-game days | Only 2 event kinds; no salience, no episodic detail, no per-NPC perspective |
| Factions | Yes (thin) | `rtsim/src/data/faction.rs` — `{ seed, leader: Option<Actor>, good_or_evil: bool, sentiments }` | No membership ranks, treasury, goals, governance; `good_or_evil` flagged `// TODO: Very stupid` in-code |
| Sites | Yes | `rtsim/src/data/site.rs` — `{ uid, seed, wpos, faction, known_reports, population (unpersisted), nearby_sites_by_size }` | No plot ownership, no local market state |
| Quests | Yes | `rtsim/src/data/quest.rs` — `QuestKind::{Escort, Slay, Courier}`; arbiter-gated monotonic resolution (`AtomicU8` compare-exchange); deposit escrow via `ItemResource` | 3 fixed kinds; generation hand-rolled in `rtsim/src/rule/npc_ai/quest.rs` (898 lines); no need-driven generation, no solvability validation |
| Dialogue | Yes | `common/src/rtsim.rs:324` — `DialogueKind::{Start, End, Statement, Ack, Question, Response, Marker}`; `rtsim/src/rule/npc_ai/dialogue.rs` (687 lines): hiring, directions, quest talk, rock-paper-scissors; profession- and personality-aware | All text from static i18n keys; no memory of past conversations; no NPC↔NPC substantive dialogue |
| AI framework | Yes | `rtsim/src/ai/mod.rs` (1,199 lines) — `Action<S, R>` trait with `then`, `repeat`, `stop_if`, `interrupt_with`, `map`, `boxed`; constructors `now`, `until`, `just`, `finish`, `choose`, `watch`, `seq`; priority via `NpcCtx::current_action_priority` | Behavior selection is a profession `match` (`npc_ai/mod.rs:1541`); no need/utility-driven goal selection |
| Population management | Yes | `rtsim/src/rule/architect.rs` + `rtsim/src/data/architect.rs` — tracks `Population` vs `wanted_population` per `TrackedPopulation` category, queues `Death`s, respawns after `MIN_SPAWN_DELAY` | Respawn creates a *brand-new* NPC (`Npc::new(rng.random(), …)`, `rule/architect.rs:313`) — no birth, no continuity, no inheritance |
| Economy | Worldgen-only | `world/src/site/economy/` — full goods/labor sim (`Economy { labors, yields, productivity, limited_by, trade orders/deliveries }`) run **once** at world generation (`world/src/lib.rs:156` → `sim2::simulate`) | Frozen after worldgen. Runtime prices come from a static asset table (`common/src/comp/inventory/trade_pricing.rs:251`, `lazy_static TRADE_PRICING`); merchant stock filled from frozen `SiteInformation` at NPC load (`server/src/rtsim/tick.rs:42–67`) |
| Merchant travel | Yes | `rtsim/src/rule/npc_ai/mod.rs:572` — `adventure()`: Merchants/Adventurers travel between sites that have workshop plots; merchants linger 15 min | Travel is decorative — no goods actually move between site economies |
| Event bus | Yes | `rtsim/src/event.rs` — `OnSetup, OnTick{tick,dt}, OnDeath, OnHelped, OnHealthChange, OnTheft, OnMountVolume`; rules bound in `rtsim/src/lib.rs:201–208` (migrate, architect, replenish_resources, report, sync_npcs, simulate_npcs, npc_ai, cleanup) | Needs new events for social/lifecycle/org/economy domains |
| Server bridge | Yes | `server/src/rtsim/` — rtsim ticks **every server tick** (30 TPS, dispatched after physics, `server/src/rtsim/mod.rs:382`); NPCs flip `SimulationMode::{Simulated, Loaded}` as chunks load (`server/src/rtsim/tick.rs:610–704`); simulated NPC brains run every 10th tick staggered by seed (`SIMULATED_TICK_SKIP = 10`, `rule/npc_ai/mod.rs:100`) | Cadence framework exists; AURORA adds slower lanes |
| Persistence | Yes | MessagePack via `rmp_serde::encode::write_named` to `<data_dir>/rtsim/data.dat` (`rtsim/src/data/mod.rs:111`, `server/src/rtsim/mod.rs:140–147`); saved every 60 s wall-clock on a background thread via full `Data` clone (`server/src/rtsim/tick.rs:553–560`) | Whole-`Data` clone each save makes per-NPC byte budgets a *clone-cost* concern, not just disk |
| Pets | Partial | `common/src/comp/pet.rs` — player taming (`Pet` component, `is_tameable`) | rtsim NPCs do not own pets; no adoption lifecycle |
| Families / marriage / romance | **No** | grep for marriage/spouse/romance/family in `rtsim/src/` returns nothing | Entire system missing |
| Organizations beyond factions | **No** | — | Entire system missing |

**Bug found during verification:** `Sentiments::cleanup` (`rtsim/src/data/sentiment.rs:93–111`) collects `(|positivity|, target)` into a max-heap and `drain_sorted().take(excess)` — which removes the *strongest* sentiments first, the opposite of the in-code comment's intent ("calculate how valuable it is for us to remember"). Should wrap in `cmp::Reverse`. Filed as a separate fix task; AURORA's relationship layer assumes the fixed behavior.

## NPC Personality & Mind

### Extending the Big Five

Keep `Personality` (`common/src/rtsim.rs:92`) untouched — it is shared with agent code and dialogue. Add a new persisted `Mind` struct in a new file `rtsim/src/data/mind.rs`, hung off `Npc` as `#[serde(default)] pub mind: Mind`:

```rust
pub struct Mind {
    pub values: EnumMap<Value, u8>,      // 8 values: Tradition, Power, Wealth, Family,
                                         // Faith, Freedom, Knowledge, Honor
    pub fears: EnumMap<Fear, u8>,        // 4: Violence, Poverty, Outsiders, Gods
    pub alignment: Alignment,            // 2 axes: lawful_chaotic: i8, selfless_selfish: i8
    pub mood: Mood,                      // see below
    pub goals: ArrayVec<Goal, 3>,        // active long-term goals (enum + target + progress)
}
```

`Value`/`Fear` weights are seeded at spawn from `Personality` + culture (home site) + `ChaChaRng(seed)`, so they are reproducible. `Goal` is a small enum (`FindSpouse`, `GetRich(amount)`, `JoinOrg(OrgId)`, `AvengeDeath(Actor)`, `OwnProperty(SiteId)`, …) that the utility layer (see [AI Architecture](#ai-architecture)) feeds on.

### Morality and reputation

- **Alignment** is the 2-axis `i8` pair above; it drifts slowly from committed/witnessed deeds (murder report witnessed → lawful axis of the *witness* unaffected, of the *perpetrator* shifts chaotic).
- **Reputation** is *not* stored on the NPC. It is the aggregate of others' sentiments plus circulating reports — computed on demand: `reputation(actor, site) = mean(sentiment of site.population sample toward actor) + report modifiers`. New helper in `rtsim/src/data/sentiment.rs`. This keeps a single source of truth and zero extra persisted bytes.

### Emotional state (mood)

`Mood` is a 6-component `i8` vector (joy, anger, fear, grief, pride, shame), decaying toward a personality-derived baseline (neurotic NPCs decay slower toward calm). Updated by the same events that touch sentiments, in the existing `cleanup` rule cadence (`NPC_SENTIMENT_TICK_SKIP = 30`, `rtsim/src/rule/cleanup.rs:6`). Mood modulates: dialogue tone selection, utility-AI weights (angry → confrontation actions score higher), and quest-generation triggers (grief → revenge quest seed).

### Short-term vs long-term memory

| Store | Contents | Persistence | Structure | Cap |
|---|---|---|---|---|
| STM | recent perceptions: who spoke to me, what I saw this session | **Unpersisted** (`#[serde(skip)]`) | ring buffer `ArrayDeque<Perception, 16>` on `Npc`, fed from existing `inbox: VecDeque<NpcInput>` | 16 |
| LTM (episodic) | salient events with my perspective: "X saved me", "Y stole from my shop" | Persisted | `ArrayVec<Episode, 24>`; `Episode { kind: u8, actors: [Option<Actor>; 2], at: TimeOfDay, salience: u8, valence: i8 }` ≈ 28 B msgpack | 24 |
| Semantic | relationship/opinion summaries | Persisted | the typed social-graph edges below + existing `Sentiments` | 16 edges + 64 sentiments |

**Consolidation** (new logic in `rtsim/src/data/memory.rs`, driven from the `cleanup` rule): each STM perception scores salience = emotional impact (|sentiment delta| + mood delta) × personal relevance (involves me/kin/friend?) × novelty. Above threshold → written to LTM, possibly evicting the lowest-salience episode. **Forgetting**: salience decays daily; episodes referenced by dialogue or quest generation get salience refreshed (rehearsal). Repeated same-actor positive episodes consolidate into a durable `Friendship` edge — this is how transient sentiments become permanent relationships, fixing the "everything decays to neutral" limitation of `Sentiment::decay`.

### Memory size budget

Hard requirement: **≤ 2 KB persisted per NPC at p95**, so 10,000 NPCs ≤ 20 MB. The math (MessagePack with short `serde(rename)` keys, the existing pattern from `TerrainResource`, `common/src/rtsim.rs:414`):

| Component | Cap | ≈Bytes/entry | Max bytes |
|---|---|---|---|
| Existing `Npc` core fields | — | — | ~120 |
| Existing sentiments (re-capped 128 → 64; edges below take over the durable cases) | 64 | 12 | 768 |
| `Mind` (values 8 + fears 4 + alignment 2 + mood 6 + 3 goals) | fixed | — | ~80 |
| LTM episodes | 24 | 28 | 672 |
| Relationship edges | 16 | 20 | 320 |
| Family links (parents, spouse, children ids) | 8 | 9 | 72 |
| Org memberships (org id + rank + standing) | 4 | 12 | 48 |
| **Total p95** | | | **~2,080 ≈ 2 KB** |

10k NPCs × 2 KB = **20 MB** of NPC payload; with sites/factions/orgs/quests, `data.dat` stays ≈ 25 MB. The 60-second save clones all of `Data` (`server/src/rtsim/mod.rs:37`) — a 25 MB clone every 60 s is ~0.4 MB/s amortized, acceptable; Phase 8 adds a benchmark guarding this (see [Roadmap](#roadmap)).

### Social graph architecture

New file `rtsim/src/data/relationship.rs`:

```rust
pub enum EdgeKind { Kinship(KinRole), Friendship, Rivalry, Romance, Marriage,
                    Professional(ProfRel), OrgPeer(OrgId) }
pub struct Edge { pub kind: EdgeKind, pub strength: i8, pub since: TimeOfDay }
```

**Storage choice: adjacency lists on each `Npc`** (`relationships: ArrayVec<(Actor, Edge), 16>`), *not* a central graph structure. Rationale:

- Every hot query is ego-centric ("my spouse", "my friends nearby", "do I have a rival here?") — O(16) scan, cache-local, and it serializes for free inside the existing per-NPC persistence.
- Global queries (community detection for ORACLE telemetry, org recruitment scans) run at slow cadence (in-game daily) in the new `social` rule, which builds a transient `petgraph`-style index in memory — never persisted.
- Symmetry is maintained by the `social` rule (both endpoints updated in the same handler); a daily integrity pass repairs dangling edges of dead NPCs (mirrors `known_reports.retain` in `Npc::cleanup`, `rtsim/src/data/npc.rs:456`).

Edges are *durable* (no stochastic decay); they weaken only through betrayal/conflict events or death. `Sentiments` remain the fast-moving affective layer on top.

## Life Simulation

rtsim NPCs today do not age — verified: `Npc` has no age/birth field, and "respawn" via the architect creates an unrelated new NPC (`rtsim/src/rule/architect.rs:313`). AURORA replaces category-respawn for `Role::Civilised` NPCs with a lifecycle, leaving `Wild`/`Monster` respawn to the architect unchanged.

### Lifecycle

New rule `rtsim/src/rule/lifecycle.rs`, ticked once per in-game day per NPC (staggered by seed, same pattern as `SIMULATED_TICK_SKIP`):

| Stage | Mechanics |
|---|---|
| Birth | Married couple + home site below housing cap → chance of child. New `Npc` with `birth_tod: TimeOfDay` field, `body` interpolated from parents, kinship edges to parents/siblings. Children use existing humanoid bodies at small scale (no new assets in v1). |
| Aging | Age = `(now - birth_tod)`; in-game year ≈ 18 real-time hours (tunable in `WorldSettings`, `common/src/rtsim.rs:513`). Life stages: child (no profession) → adult (profession assignment) → elder (reduced wages, advisory org roles). |
| Death | Existing `OnDeath` event (`rtsim/src/event.rs:42`) plus new old-age death from the lifecycle rule. Triggers inheritance, grief moods in kin, possible revenge goals if murdered. |
| Population control | The architect's `wanted_population` (`rtsim/src/data/architect.rs:145`) becomes the *ceiling*: births throttled when at cap, architect only force-spawns civilians when population collapses below a floor (e.g. war/plague aftermath). |

### Family trees and genetics

- `rtsim/src/data/family.rs`: kinship is stored as edges (above); a transient genealogy index (built at load in `Data::prepare`, the existing pattern at `rtsim/src/data/mod.rs:119`) answers "ancestors/descendants" queries.
- **Simplified genetics:** child `seed` derives name/appearance via the existing `RandomPerm` machinery (`Npc::rng`, `rtsim/src/data/npc.rs:427`); appearance = parent body-param blend + jitter; `Personality` = per-trait mean of parents ± distributed jitter (reusing `distributed()`, `common/src/rtsim.rs:100`); `Mind.values` inherit with stronger jitter.
- **Cultural inheritance:** children copy home-site culture weights and parents' religion/org sympathies at reduced strength; a cultist's child is *predisposed*, not predestined.
- **Names become persisted** (`name: Option<String>` on `Npc`) — required for family names; resolves the in-code TODO at `rtsim/src/data/npc.rs:431`. Family surname passes patrilineally/matrilineally per culture.

### Property and inheritance

World sites have plots with kinds (`world/src/site/plot/`, queried by NPC AI today, e.g. taverns at `rtsim/src/rule/npc_ai/mod.rs:644`) but **no ownership** — verified, no owner field exists. Ownership therefore lives on the rtsim side, in `rtsim/src/data/site.rs`:

```rust
pub struct Deed { pub plot: Id<Plot>, pub owner: Actor, pub kind: DeedKind } // home/shop/farm
// Site gains: #[serde(default)] pub deeds: Vec<Deed>   (persisted; plot ids re-linked
// at load like `world_site`, with the same orphan-cleanup caveat as site.rs:34–40)
```

NPC wealth = persisted `coins: u32` on `Npc` (rtsim already abstracts money as `ItemResource::Coin`, `common/src/rtsim.rs:452`). On death: deeds and coins pass spouse → eldest child → siblings → home-site treasury. Profession evolution: children apprentice toward a parent's profession with probability weighted by site labor demand (economy layer below), making professions hereditary-but-responsive.

## Economy

### Assessment of what exists

The `world/src/site/economy/` simulation is sophisticated (goods, labor allocation, productivity, inter-site trade orders/deliveries via `NeighborInformation`) but runs **exactly once**, during world generation (`world/src/lib.rs:156`). At runtime: prices come from the static `TradePricing` asset table (`common/src/comp/inventory/trade_pricing.rs`), and merchant inventories are stocked from the frozen per-site `SiteInformation` snapshot when an NPC loads (`server/src/rtsim/tick.rs:42–67` — including the in-code comment `// economy isn't economying sometimes`). Merchants travel between sites (`adventure()`) but carry no goods that affect anything.

### Dynamic layer design

Do **not** re-run the full worldgen economy at runtime (it is O(sites²·goods) and not incremental). Instead, new persisted `rtsim/src/data/economy.rs` keeps a small dynamic state per site, seeded from the worldgen snapshot:

```rust
pub struct SiteEconomy {
    pub stock:  EnumMap<Good, f32>,     // current inventory
    pub demand: EnumMap<Good, f32>,     // smoothed consumption rate
    pub price_mult: EnumMap<Good, f32>, // dynamic multiplier over TradePricing base
}
```

- **Production:** each in-game day (new rule `rtsim/src/rule/economy.rs`), site stock += population-weighted output per profession census (the site already tracks `population: HashSet<NpcId>`, `rtsim/src/data/site.rs:47`). Farmers→Food, Hunters→Meat, Blacksmiths→Tools, etc., reusing the worldgen `Labor`→`Good` mapping (`world/src/site/economy/map_types.rs`).
- **Consumption:** population eats/uses stock; shortfall raises `demand`, starvation lowers site mood and raises emigration pressure.
- **Pricing:** `price_mult = clamp((demand / supply)^k, 0.25, 4.0)`, smoothed. Hook: `server/src/rtsim/tick.rs` merchant-stocking already consumes `SiteInformation` — we substitute live `SiteEconomy` values so player-facing trade prices (computed through `SitePrices::balance`, `common/src/trade.rs:404`) reflect the dynamic multipliers. No change needed in the voxygen trade UI.
- **Merchant routes:** merchants get cargo: on departure they buy surplus goods cheap, on arrival sell into demand (transfer between `SiteEconomy` maps + personal profit to `Npc::coins`). Route choice becomes a utility decision (price spread × distance) instead of `adventure()`'s random pick at `rtsim/src/rule/npc_ai/mod.rs:575`. A killed merchant loses the cargo — banditry now has macro effects.
- **Crises:** inflation, famine, trade wars are **ORACLE-coordinated**: ORACLE issues a directive (e.g. `Embargo { a, b }`, `Blight { site, good }`); AURORA applies it as economy-rule modifiers and lets prices/NPC behavior emerge. AURORA never invents macro crises itself.

## Organizations

### Unified `Organization` entity

The existing `Faction` (`rtsim/src/data/faction.rs`) is too thin to extend in place (3 fields, one flagged "very stupid"), but is load-bearing (NPC field, site field, architect tracking by faction). **Recommendation: supersede.** New first-class entity, with `Faction` retained as a deprecated alias during migration (Phase 5 migrates NPC/site `faction` fields to org ids via the `migrate` rule, `rtsim/src/rule/migrate.rs`):

```rust
// rtsim/src/data/organization.rs
slotmap: OrgId
pub struct Organization {
    pub kind: OrgKind,        // Guild(Profession) | Religion | Cult | Criminal |
                              // Mercenary | MerchantGuild | NobleHouse | PoliticalFaction
    pub name: String,
    pub governance: Governance, // Autocratic { leader } | Council { seats: Vec<Actor> }
                              // | Elective { leader, term_ends: TimeOfDay }
    pub members: HashMap<Actor, Membership>,   // Membership { rank: u8, standing: i8, joined }
    pub treasury: f32,        // ItemResource::Coin units
    pub home: Option<SiteId>,
    pub goals: ArrayVec<OrgGoal, 4>, // Expand(SiteId) | Monopolize(Good) | Convert(SiteId)
                              // | Eliminate(OrgId) | Enrich(target) | SeizePower(SiteId)
    pub sentiments: Sentiments,  // org-level, reusing FACTION_MAX_SENTIMENTS budget
    pub charter: Option<String>, // LLM-generated flavor, cached, never simulated
}
```

| Kind | Founding trigger | Typical goals | Existing hooks |
|---|---|---|---|
| Guild (craft) | ≥N same-profession NPCs in one site | wage floors, monopoly | `Profession` enum (`common/src/rtsim.rs:485`) |
| Religion | high-Faith NPC + receptive population | convert sites, build temples | — |
| Cult | Religion variant with `Mind.values` skew + secrecy flag | infiltrate, ritual quests | `Profession::Cultist` already exists |
| Criminal | high-chaotic NPCs + economic desperation | theft, smuggling, extortion | `OnTheft` event, theft reports, `Profession::Pirate(bool)` + pirate AI (`npc_ai/mod.rs:1542`) |
| Mercenary | veteran Guards/Adventurers | contracts (escort/war), payment | hiring system in `dialogue.rs:28–464` |
| Merchant guild | wealthy merchants on a shared route | price-fixing, caravan protection | merchant travel |
| Noble house | wealthiest landowning family | dynastic marriage, titles, succession | family system (Phase 3) |
| Political faction | site population with grievances | win elections, coups | site governance (Phase 5) |

**Membership and dynamics:** joining = NPC goal (`JoinOrg`) satisfied via dialogue with a recruiter; rank ascends through contribution (org-related quest completions, tithes). **Founding:** an NPC with the matching goal, threshold wealth/standing, and ≥2 willing co-founders registers a new org. **Dissolution:** treasury bankruptcy, leader death without succession, or membership < 3 for an in-game month. All transitions emit reports so news propagates through the existing gossip mechanism.

## Dynamic Quest Generation

The existing quest system is kept as the *execution and escrow* layer (arbiter, monotonic resolution, deposits — `rtsim/src/data/quest.rs:225–242`); AURORA adds a *generation* layer in `rtsim/src/rule/quest_gen.rs`.

### Template taxonomy

| Template | Source NPC state | Underlying `QuestKind` |
|---|---|---|
| Lost pet | NPC pet (Phase 3) wandered/stolen | new `Find { target, area }` |
| Missing child | child NPC abducted (criminal org action) | `Find` + `Escort` chain |
| Escort | travel goal + danger reports on route | existing `Escort` |
| Medical supply | site plague/injury + herb shortage in `SiteEconomy` | existing `Courier` (payload generalized from the hardcoded `Payload::{LegoomLeaf, GnarlingCarving}`) |
| Shortage run | `SiteEconomy.demand` spike | new `Procure { good, amount, site }` |
| Monster attack | architect-spawned monsters near site + death reports | existing `Slay` |
| Family dispute | rivalry edge between kin, inheritance contention | new `Mediate { a, b }` (dialogue-resolved) |
| Political intrigue | org `SeizePower` goal | `Courier`/`Find`/`Mediate` chains |
| Religious conflict | two religions converting same site | chains + `Mediate` |
| Criminal investigation | unresolved theft/murder reports at a site | new `Investigate { report }` → `Slay`/`Mediate` |

### Generation pipeline

1. **Need detection** (in-game daily, per site): scan NPC goals, moods, reports, and `SiteEconomy` for grievances above threshold; emit `QuestSeed`s with urgency scores.
2. **Validation (solvability):** target exists and is reachable (site path exists via `world::civ::Track`, already used by `PathingMemory`, `rtsim/src/data/npc.rs:58`); required items obtainable; no circular dependency; arbiter NPC alive and home. Unsolvable seeds are dropped, not patched.
3. **Reward generation:** deposit = base (template danger × distance) × site wealth multiplier, denominated in `ItemResource::Coin`; difficulty band and XP per the character-levels spec (`2026-06-10-character-levels-design.md`) so a level-8 area emits level-6–10 quests.
4. **Anti-exploit:** per-player rate limit (≤3 active generated quests, cooldown per template per site); deposits use the existing escrow so reward duplication is impossible by construction (monotonic `QuestRes`); generated quests expire via the existing `timeout` field; repeated abandon → arbiter sentiment penalty → reduced offers (sentiment thresholds already gate behavior, `rtsim/src/data/sentiment.rs:127`).

## Social Simulation Dynamics

All rules live in `rtsim/src/rule/social.rs`, cadence: per-NPC social tick every 100 rtsim ticks (staggered), plus event-driven handlers.

- **Friendship/rivalry formation:** `Δedge = interaction_frequency × personality_compat × recent_sentiment`. Compatibility from Big Five (similar openness/values attract; two low-agreeableness NPCs → rivalry-prone). Sustained sentiment ≥ `FRIEND` (0.6) for an in-game week consolidates a `Friendship` edge; sustained ≤ `RIVAL` (−0.3) plus a grievance episode consolidates `Rivalry`.
- **Betrayal:** an NPC with `Rivalry`-masked-by-`OrgPeer` edges and high selfish alignment may defect (steal treasury share, leak org secrets as reports) when offered a better goal payoff; betrayal converts edges to `Rivalry` and slashes org standing.
- **Romance → marriage → divorce:** compatible adults with positive edges escalate Friendship → Romance → Marriage (goal-driven, requires courtship interactions; culture gates from home site). Divorce triggered by sustained negative sentiment, betrayal episodes, or rival romance; splits property, creates rivalry edges.
- **Succession:** noble house / autocratic org leader dies → heir by primogeniture (family tree) or rank; contested succession (two claimants with similar standing) spawns a political-intrigue quest chain and possible org schism.
- **Elections:** `Elective` governance — term expiry triggers candidacy (high-Power-value members), a campaign window of influence interactions, then weighted member vote (standing × sentiment toward candidate).
- **Coups/rebellions:** ORACLE-sanctioned (AURORA reports tension metrics: mean mood, wealth inequality, faction sentiment toward leadership; ORACLE decides if a coup fires; AURORA executes: leadership swap, loyalist/rebel partition along edge lines, casualties as `OnDeath` events).
- **Crime:** the detection substrate exists — `OnTheft` events (`rtsim/src/event.rs:72`), theft/murder reports with witnesses (`rtsim/src/rule/report.rs`), guard profession. AURORA adds: criminal orgs *generate* crime (planned thefts against wealthy deeds), guards *investigate* (new quest template), and conviction consequences (fines to site treasury, exile = home loss, sentiment cascades through the victim's family edges).

## AI Architecture

Layered, cheapest-first:

### (a) Utility-AI need selection (per NPC)

New `rtsim/src/ai/utility.rs`. At brain-tick frequency, score a fixed set of drives (survive, work, socialize, family, ambition, org-duty) from `Mind`, mood, goals, and context; the winner selects which top-level `Action` tree to run. This *replaces only* the profession `match` at `rtsim/src/rule/npc_ai/mod.rs:1541` — everything below the selection stays combinator-based. Scores are deterministic given (npc state, seeded rng).

### (b) Action combinators for execution

Unchanged: the verified `Action<S, R>` framework (`rtsim/src/ai/mod.rs:149`) with `then`/`repeat`/`stop_if`/`interrupt_with`/`choose`/`watch`/`seq` and priority-based interruption is exactly a behavior-tree-with-data-flow and already handles loaded/simulated duality. New behaviors (court spouse, attend sermon, run shop, patrol, smuggle) are new combinator functions in `rtsim/src/rule/npc_ai/`.

### (c) GOAP for org-level planning only

Organizations plan multi-step goals (e.g. `Monopolize(Iron)` → recruit miners → buy deeds → undercut rival → price-fix). This *is* genuine planning over preconditions/effects, where GOAP earns its cost — and there are ~10²–10³ orgs versus 10⁴ NPCs, planning at in-game-daily cadence. A tiny A*-over-actions planner (~300 lines, no dependency) in `rtsim/src/ai/goap.rs`, used *only* by the `organizations` rule. Per-NPC GOAP is explicitly rejected: utility + combinators cover individual behavior at a fraction of the cost and stay debuggable via the existing `Action::backtrace`.

### (d) LLM integration

| Used for | Never used for |
|---|---|
| Dialogue color (paraphrasing template dialogue lines with NPC personality/mood/memory context) | Any tick decision |
| Org charters, religion tenets, cult rituals (one-shot at founding, cached in `Organization.charter`) | Combat/utility scoring |
| Quest flavor text (description from template + seed facts) | Quest *generation* or rewards |
| Rumor phrasing (reports → gossip lines) | Sentiment/relationship math |

Mechanics: trait boundary in `rtsim/src/llm.rs` —

```rust
pub trait TextOracle: Send + Sync {
    fn request(&self, req: TextRequest) -> TextTicket;     // non-blocking enqueue
    fn poll(&self, ticket: TextTicket) -> Option<String>;  // checked next tick
}
```

Implementation in `server/src/rtsim/llm_bridge.rs`: a worker thread with a bounded queue (depth 64, drop-oldest), backed by either a **local model** (llama.cpp/Ollama HTTP endpoint, 7–8B instruct class — adequate for one-line paraphrase) or a **remote API** (Claude Haiku class) selected in server settings. Responses cached in an LRU keyed by `(template_id, personality_bucket, mood_bucket, fact_hash)` — bucketing makes the cache hit-rate high because most NPCs collapse to a few hundred distinct keys. **Cost model:** with caching, a 100-player server generates ≈ 2–5k uncached requests/day ≈ ≤ $1/day on Haiku-class pricing, or $0 local. **Fallback:** on timeout (>2 s), queue-full, or disabled feature → the i18n template line ships verbatim (today's behavior). The game must be 100% playable with `TextOracle = NullOracle`.

### (e) Memory storage

In-process and serialized with `Data`, as designed in the memory section. **Against an external vector DB in v1:** (1) the entire corpus per NPC is ≤ 24 episodes — exact salience-scan beats approximate-nearest-neighbor at that size; (2) rtsim persistence is one atomic file — adding a second store creates dual-write consistency problems for zero retrieval benefit; (3) operational footprint on a small VPS. The retrieval seam is a trait in `rtsim/src/data/memory.rs`:

```rust
pub trait EpisodeRetriever {
    fn relevant(&self, npc: &Npc, cue: &RetrievalCue, k: usize) -> ArrayVec<&Episode, 8>;
}
```

v1 ships `SalienceRetriever` (recency × salience × actor-match). An embedding-backed implementation can plug in later without touching call sites.

### (f) Event bus and ECS sync

Extend `rtsim/src/event.rs` with AURORA events: `OnBirth`, `OnMarriage`, `OnInheritance`, `OnOrgEvent { org, kind }`, `OnEconomyTick`, `OnDirective` (from ORACLE). New rules register in `RtState::start_rules` (`rtsim/src/lib.rs:201`) — order matters: `lifecycle` and `social` before `npc_ai` so brains see fresh state; `economy` and `organizations` after. World↔ECS sync continues through the existing bridge: loaded NPCs act through `comp::Agent`/`RtSimController` and feed perceptions back via `NpcInput` (`common/src/rtsim.rs:369`); no new sync channel needed except dialogue memory writes, which ride the existing validated-dialogue path (`Dialogue<true>`).

### ORACLE Integration Contract

| Direction | Channel | Messages | Cadence |
|---|---|---|---|
| AURORA → ORACLE | telemetry snapshot (serialized summary, *not* full `Data`) | per-site: population, mood mean, wealth Gini, dominant orgs, unresolved-grievance count; per-org: power, goals, conflicts; economy: price indices | per in-game day |
| ORACLE → AURORA | `OnDirective` event | `Crisis(kind, site)`, `Embargo(a, b)`, `Festival(site)`, `SanctionCoup(org, site)`, `SpawnThreat(area, tier)` | sparse (hours–days) |
| Invariants | — | ORACLE never mutates `Data` directly; AURORA may *refuse* a directive that violates invariants (e.g. coup in a site with no org) and reports the refusal | — |

Both specs version this contract; breaking changes require updating `docs/superpowers/specs/2026-06-10-project-oracle-design.md` in the same commit.

## Scale & Performance

Verified baseline: rtsim ticks at full server rate (30 TPS), with per-NPC brain runs at 3 Hz for simulated NPCs (`SIMULATED_TICK_SKIP = 10`) and every tick for loaded NPCs; sentiment decay at 1 Hz equivalent (`NPC_SENTIMENT_TICK_SKIP = 30`); cleanup at `NPC_CLEANUP_TICK_SKIP = 100`.

**Simulation LOD** (extends the existing `SimulationMode` split):

| Ring | NPCs | What runs |
|---|---|---|
| Loaded (near players) | 10²–10³ | full: brain every tick, STM perception, dialogue, LLM color |
| Simulated-near (sites with recent player presence) | 10³ | brain at 3 Hz (today's behavior) + social/lifecycle/economy lanes |
| Simulated-far | 10⁴ | **statistical**: no individual brain ticks; site-level aggregate updates (births/deaths/economy/org membership drift as rates), individual state lazily reconciled when the ring promotes — same philosophy as the architect's population accounting |

**Tick budgets** (target, 10k NPCs, measured on the dev profile):

| Lane | Cadence | Budget/tick |
|---|---|---|
| npc_ai (existing + utility layer) | per tick | ≤ 2.0 ms (today's dominant cost; utility adds ≤ 0.1 ms) |
| social rule | per tick (1% of NPCs staggered) | ≤ 0.3 ms |
| lifecycle | per tick (daily per NPC, staggered) | ≤ 0.1 ms |
| economy + organizations | bursty, in-game daily per site/org, amortized across ticks | ≤ 0.3 ms |
| cleanup (+ memory consolidation) | existing cadence | ≤ 0.2 ms |

**Distributed stance:** single-server first. Shard-ready boundaries are kept clean anyway: AURORA state is entirely inside `rtsim::data::Data`; the only cross-boundary surfaces are the server bridge (`server/src/rtsim/`) and the ORACLE contract — both message-shaped, so a future split of rtsim into its own process is a transport change, not a redesign.

**Benchmarks to add** (`rtsim/benches/`, criterion): `social_tick_10k`, `consolidation_10k`, `economy_50_sites`, `data_clone_serialize_10k` (guards the 60 s save), `quest_gen_validation`. CI threshold alerts at +20% regression.

## Roadmap

Eight phases; one senior dev + AI assistance. Complexity: S ≈ 1–3 days, M ≈ 4–8, L ≈ 9–15, XL ≈ 16–25.

| # | Phase | Deliverables | Key technical tasks (file-level) | Complexity | Risks |
|---|---|---|---|---|---|
| 1 | Foundations | `Mind` (values/fears/alignment/mood), STM/LTM stores, consolidation, persisted names, byte-budget enforcement | new `rtsim/src/data/{mind,memory}.rs`; extend `Npc` (`data/npc.rs`) with `#[serde(default)]` fields; consolidation hooks in `rule/cleanup.rs`; fix `Sentiments::cleanup` heap-order bug (`data/sentiment.rs:104`); budget tests | **L** (9–14 d) | save-size creep; migration of live `data.dat` (mitigated: `serde(default)`, version 10 unchanged) |
| 2 | Social Graph | typed edges, formation/decay rules, reputation queries, dialogue references to shared memories | new `rtsim/src/data/relationship.rs`, `rule/social.rs`; register in `lib.rs:start_rules`; extend `rule/npc_ai/dialogue.rs` with memory-aware branches | **L** (10–15 d) | edge-symmetry bugs; tuning formation rates (needs soak telemetry) |
| 3 | Families | lifecycle (birth/aging/death), genetics, kinship, pets for NPCs, inheritance of coins | new `rule/lifecycle.rs`, `data/family.rs`; architect ceiling refactor (`rule/architect.rs`, `data/architect.rs`); child bodies via body-param scaling | **XL** (16–24 d) | architect interplay (double-spawning); aging pace vs. server uptime expectations |
| 4 | Economy | `SiteEconomy`, production/consumption, dynamic pricing into player trade, merchant cargo | new `rtsim/src/data/economy.rs`, `rule/economy.rs`; rewire merchant stocking (`server/src/rtsim/tick.rs:42–67`); cargo-aware `adventure()` (`rule/npc_ai/mod.rs:572`) | **L** (10–15 d) | price oscillation/exploits (player buy-dump loops) — needs damping + per-player trade caps |
| 5 | Organizations | `Organization` entity, 8 kinds, governance, treasury, founding/dissolution, Faction migration | new `data/organization.rs`, `rule/organizations.rs`, `ai/goap.rs`; migrate `Npc::faction`/`Site::faction` in `rule/migrate.rs` | **XL** (18–25 d) | Faction is load-bearing (architect, AI checks) — migration must be staged behind an alias |
| 6 | Dynamic Quests | seed pipeline, 10 templates, validation, reward scaling, anti-exploit | new `rule/quest_gen.rs`; new `QuestKind` variants (`data/quest.rs`); generalize `Payload`; reward tables per character-levels spec; dialogue offers in `rule/npc_ai/quest.rs` | **L** (10–15 d) | quest spam/repetition; balancing depends on character-levels spec landing first |
| 7 | LLM Integration | `TextOracle` trait, server bridge, cache, local+remote backends, fallback | new `rtsim/src/llm.rs`, `server/src/rtsim/llm_bridge.rs`; settings in `server/src/settings/mod.rs`; cache metrics | **M** (5–8 d) | latency spikes; content safety for remote models (system-prompt constraints + output length caps) |
| 8 | Optimization | statistical far-ring LOD, criterion benches, tick-budget enforcement, save-clone optimization | LOD in `rule/simulate_npcs.rs` + new aggregate site updates; `rtsim/benches/`; consider copy-on-write save snapshot | **L** (9–14 d) | LOD promote/demote fidelity (NPCs "teleporting" through life changes) |

Total: ≈ 87–130 dev-days. Milestone gates: after Phase 2 (NPCs visibly remember players — first shippable slice), Phase 4 (prices move — economy alpha), Phase 6 (generated quests live — content beta).

## Testing Strategy

- **Determinism tests:** all new rules take seeded `ChaChaRng` (existing pattern); golden tests run 10k simulated ticks from a fixture `Data` and assert identical post-state hashes across runs (`rtsim/tests/determinism.rs`). LLM layer excluded by design (presentation only).
- **Unit/property tests:** memory budget never exceeded (proptest over event streams); edge symmetry invariant; inheritance totals conserve coins; quest validation rejects unreachable targets. Run via `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim`.
- **Long-run soak:** headless `veloren-server-cli` with bot clients, 72 h accelerated time; assertions on population stability (no extinction/explosion), price boundedness, org count equilibrium, `data.dat` size ceiling, zero panics. Scripted under `.github/workflows/` as a manual-dispatch job.
- **Telemetry dashboards:** the fork's structured logging system (spec `2026-06-05-logging-system-design.md`, `telemetry!` macro at `common/src/lib.rs:22`, `logging-verbose` feature) gains AURORA channels: `"soc"` (edge formed/broken), `"life"` (birth/death/marriage), `"eco"` (price index per site), `"org"` (founding/succession/coup), `"qst"` (generated/accepted/resolved). The `veloren-telemetry` skill workflow consumes `server_telemetry.jsonl` for session analysis; dashboards aggregate the same JSONL.

## Metrics of Success

| Metric | Target |
|---|---|
| NPC recalls a specific past player interaction in dialogue | ≥ 80% of NPCs with a qualifying episode |
| Stable friendships/marriages emerge in soak | ≥ 30% of adult NPCs have ≥1 durable edge by day 30 |
| Multi-generation families exist | ≥ 3 generations by soak day 60 |
| Prices respond to player action (bulk buying raises price) | observable within one in-game day |
| Organizations founded organically (not seeded) | ≥ 1 per 500 NPCs per in-game month |
| Generated quests completed by playtesters rated "made sense" | ≥ 70% in playtest survey |
| Tick budget at 10k NPCs | AURORA lanes ≤ 1.0 ms/tick combined |
| `data.dat` at 10k NPCs | ≤ 30 MB |

## Open Questions

1. **Aging pace:** one in-game year ≈ 18 real hours makes generational play visible within weeks, but means a beloved NPC dies of old age in ~2 real months. Tune after soak telemetry, possibly per-server setting in `WorldSettings`.
2. **Player marriage to NPCs:** the edge model supports `Actor::Character` spouses, but offline-player handling (does an NPC spouse remarry?) needs a product decision before Phase 3.
3. **Report capacity:** global `Reports` has a `// TODO: Limit global number of reports` (`rtsim/src/data/report.rs:78`); AURORA multiplies report kinds — cap and prioritization policy needed in Phase 1 or 2.
4. **Child NPC safety/combat rules:** children must be excluded from `Slay` targets and combat AI; needs a `Role`-level or age-gate check audit across `npc_ai`.
5. **ORACLE telemetry transport:** in-process call vs. file/socket boundary — decided in the ORACLE spec; AURORA only requires the snapshot/directive types to be `serde`-stable.
6. **Statistical-ring fidelity:** which life events may occur "off-screen" (marriage yes, murder of a named quest arbiter no?) — needs a whitelist before Phase 8.
