# World Difficulty Zones: Region Levels, Leveled NPCs, Map Scale, and Multi-Plane Worlds

**Date:** 2026-06-10
**Companion specs:** `2026-06-10-character-levels-design.md` (player Level), `2026-06-10-classes-races-design.md` (ClassKind), `2026-06-10-lore-cosmology-design.md` (planes), `2026-06-10-project-oracle-design.md` (world director)

## Context

Vanilla Veloren has no level system for entities and no spatial difficulty structure: a wolf
next to the starting town hits as hard as a wolf at the far edge of the map. Danger comes
only from *which* species spawns where (biome-weighted) and from dungeon tiers being
hand-placed. There is no progression loop of "outlevel this zone, travel to the next one" —
the core loop of WoW/Diablo-style world design.

This spec introduces **region-based difficulty zones**: every world region gets a difficulty
rating computed at worldgen, every combat entity gets a **Level** derived from the region it
spawns in, stats/skillsets/loot scale with that level, and XP rewards scale with the
level differential against the player. It also covers the two expansion axes that interact
with zone design: **map enlargement** (more horizontal room for difficulty banding) and
**multi-plane worlds** (vertical expansion via portals, per the lore cosmology spec).

## Goals

- Deterministic per-region difficulty (1–10) computed at worldgen, visible on the map.
- Every combat-relevant entity (wildlife, dungeon mobs, rtsim NPCs) spawns with a Level.
- Level drives HP/damage scaling, skillset rank, and loot tier — data-driven where possible.
- XP scaled by level differential with a gray-mob cutoff and anti-farm dampening.
- Combat-capable rtsim NPCs (guards, adventurers, cultists, pirates) get levels and classes.
- A credible phased path to multi-plane worlds without blocking the difficulty work on it.

## Non-Goals

- Player level mechanics (XP curve, level cap, respec) — owned by the character-levels spec.
- Class ability kits — owned by the classes-races spec; this spec only maps NPCs onto classes.
- Dynamic/adaptive difficulty (mob scaling to player) — explicitly rejected; static zones are
  the point. Project Oracle may later layer dynamic *events* on top, not stat scaling.
- Upstream compatibility for the new fields — this is fork-only; we accept merge friction.

## Current State (Verified 2026-06-10)

| Mechanism | Location | Detail |
|---|---|---|
| Biome difficulty | `common/src/terrain/biome.rs:22` | `BiomeKind::difficulty() -> i32`, range 1–5 |
| Its only consumer | `world/src/lib.rs:279-284` | Spawn-site scoring: `chunk_difficulty = 20.0 / (20.0 + biome.difficulty().pow(4) as f32 / 5.0)` |
| Entity definition | `common/src/generation.rs:120` (`EntityConfig`), `:212` (`EntityInfo`) | Fields: `loot: LootSpec<String>`, `scale: f32`, `skillset_asset: Option<String>`. **No level field.** |
| Skillset ranks | `assets/common/skillset/preset/rank{1..5}/` | Per-weapon RONs (`sword.ron`, `axe.ron`, …, `fullskill.ron`) — the closest thing to a level today |
| Entity RON | `assets/common/entity/**` | e.g. `wild/aggressive/wolf.ron`: `loot: LootTable("common.loot_tables.creature.quad_medium.wolf")`, `meta: []` (meta can carry `SkillSetAsset`) |
| Wildlife spawning | `world/src/layer/wildlife.rs` | `spawn_manifest() -> Vec<(&str, DensityFn)>` where `DensityFn = fn(&SimChunk, &ColumnSample) -> f32`; entities built via `EntityInfo::at(pos).with_asset_expect(...)` (line 130) |
| Dungeon spawning | `world/src/site/plot/{gnarling,adlet,cultist,haniwa,myrmidon_*,…}.rs` | Hardcoded asset paths per plot, e.g. `gnarling.rs:2025` `with_asset_expect("common.entity.dungeon.gnarling.chieftain", …)` |
| Per-chunk sim data | `world/src/sim/mod.rs:2503` `SimChunk` | `chaos, alt, temp, humidity, sites: Vec<Id<Site>>, place, poi, spot, …` — **no difficulty field**. Map file (`WorldFile` in `world/src/sim/map.rs`) persists only `alt`/`basement`; everything else is recomputed deterministically at load |
| Map size | `world/src/sim/mod.rs:148-160` | `GenOpts { x_lg: 10, y_lg: 10, scale: 2.0 }` → 1024×1024 chunks default |
| Town site kinds | `world/src/civ/mod.rs:477-482` | `SiteKind::{Refactor, CliffTown, SavannahTown, CoastalTown, DesertCity}` are the inhabited towns |
| Portals | `common/src/comp/misc.rs:19,54` | `Object::Portal`, `PortalData { target: Vec3<f32>, requires_no_aggro, buildup_time }`; `Teleporting` component in `common/src/comp/teleport.rs:6`. One-way, fixed coords, **same world only** |
| rtsim NPCs | `rtsim/src/data/npc.rs:281` | `Npc { role: Role, personality, sentiments, faction, … }`; `Profession` in `common/src/rtsim.rs:485`: Farmer, Hunter, Merchant, Guard, Adventurer(u32 tier 0–3), Blacksmith, Chef, Alchemist, Pirate(bool), Cultist, Herbalist, Captain |
| rtsim population | `rtsim/src/rule/architect.rs:55` | `architect_tick` reconciles `population` vs `wanted_population`, spawns replacements |
| Stat modifiers | `common/src/comp/stats.rs:78-95` | `max_health_modifiers: StatsModifier`, `attack_damage_modifier: f32`, `move_speed_modifier`, etc. — **reset every tick** by `reset_temp_modifiers()` (line 147), so they carry buffs, not permanent scaling |
| XP today | `server/src/events/entity_manipulation.rs:1122` | `exp_reward = combat::combat_rating(...)`-based, split by damage contribution, group share `/ sqrt(members)`. No level differential |
| Nameplates | `voxygen/src/hud/overhead.rs:48-49` | Widget ids `level` and `level_skull` already exist (legacy from the pre-0.9 level system) — revivable |
| Admin commands | `common/src/cmd.rs:459,440` | `ServerChatCommand::Tp` and `RtsimTp` exist for playtest teleportation |

## Design

### 1. Region Difficulty Model

A `difficulty: u8` in `1..=10` is computed per chunk during worldgen, after civ/site
placement (towns must exist first), and stored as a new field on `SimChunk`
(`world/src/sim/mod.rs:2503`).

```
fn compute_difficulty(chunk: &SimChunk, towns: &[Vec2<i32>]) -> u8 {
    let d_town  = dist_to_nearest_town_chunks(chunk_pos, towns) as f32; // chunks
    let base    = (d_town / 64.0).powf(0.8);            // +1 tier per ~64 chunks, sublinear
    let biome   = (chunk.get_biome().difficulty() - 1) as f32 * 0.75;  // 0.0..=3.0
    let alt     = ((chunk.alt - 1200.0).max(0.0) / 800.0).min(2.0);    // high mountains +0..2
    (1.0 + base + biome + alt).round().clamp(1.0, 10.0) as u8
}
```

- "Town" = any site of kind `Refactor | CliffTown | SavannahTown | CoastalTown | DesertCity`
  (`world/src/civ/mod.rs:477-482`). Every town projects a difficulty-1 safe disc; difficulty
  rises with distance, biome hostility, and altitude.
- **Spatial smoothing:** after the per-chunk pass, run a 5×5 box blur and re-clamp so zone
  borders are gradients, not single-chunk cliffs.
- **Persistence:** none needed. `WorldFile` only stores `alt`/`basement`; `SimChunk` is
  rebuilt deterministically at load, and `compute_difficulty` is deterministic given sites.
  Same map seed ⇒ same difficulty layout, no map-format migration.
- **Exposure:** `World::get_chunk_difficulty(chunk_pos, index) -> u8` next to the existing
  sim accessors in `world/src/lib.rs`; the server reads it through the `World`/`IndexRef`
  it already holds. The client gets it via the world map message (Section 5).
- Replace the spawn-scoring heuristic at `world/src/lib.rs:279-284` to read the new field
  instead of recomputing from biome, keeping one source of truth.

### 2. Entity Level

**Representation:** a new synced component `Level(pub u16)` in `common/src/comp/` (registered
per the ECS pattern in `common-state/`), shared with players (character-levels spec defines
the player side). Not a `Stats` field: `Stats` modifiers are tick-reset buffs
(`stats.rs:147`), while Level is immutable after spawn.

**Assignment band:** region difficulty `d` maps to spawn levels `3d-2 ..= 3d` (uniform roll):

| Region difficulty | Entity levels | Skillset rank (`ceil(level/6)`) |
|---|---|---|
| 1 | 1–3 | rank1 |
| 2–3 | 4–9 | rank1–2 |
| 4–5 | 10–15 | rank2–3 |
| 6–7 | 16–21 | rank3–4 |
| 8–9 | 22–27 | rank4–5 |
| 10 | 28–30 | rank5 |

Entity level cap is 30 for the initial release (player cap per character-levels spec).

**Stat scaling.** Applied once at spawn when the server builds components from `EntityInfo`
(the `NpcData`/`CreateNpc` path in `server/src/`), not via the tick-reset modifiers:

```
hp_mult(level)  = 1.0 + 0.12 * (level - 1)      // L1 ×1.0 → L30 ×4.48
dmg_mult(level) = 1.0 + 0.06 * (level - 1)      // L1 ×1.0 → L30 ×2.74
```

- HP: scale `Health` base max at construction (linear; intentionally steeper than damage so
  high-zone fights are longer, not just lethal).
- Damage: new non-reset field `Stats::level_damage_multiplier: f32` (default 1.0), set at
  spawn, excluded from `reset_temp_modifiers()`, multiplied into outgoing damage alongside
  `attack_damage_modifier` in `common/src/combat.rs`.
- Entity RON baselines stay untouched: a wolf RON still describes a level-1 wolf; species
  identity (a troll outclasses a wolf at equal level) is preserved because scaling is
  multiplicative on the RON baseline.

**Skillset rank:** when `EntityInfo` carries a level and the entity config requests a preset
skillset, select `common.skillset.preset.rank{N}.{weapon}` with `N = ceil(level/6)`
(table above), overriding any rank baked into `meta`. Configs with bespoke skillsets
(bosses) keep them.

**Loot tier:** add `loot_tiered: Option<[LootSpec<String>; 5]>` to `EntityConfig`
(`common/src/generation.rs:120`), one entry per rank band; fall back to the existing
`loot: LootSpec<String>` when absent. Rollout is incremental per-RON — untouched entity
files behave exactly as today.

### 3. Spawn Pipeline Changes

| Path | Change |
|---|---|
| `common/src/generation.rs` | `EntityInfo.level: Option<u16>` + `with_level(u16)` builder; level resolved in `with_asset_expect` flow so skillset/loot selection sees it |
| `world/src/layer/wildlife.rs` | `apply_wildlife_supplement` reads `SimChunk.difficulty` (the `DensityFn` signature already receives `&SimChunk`), rolls the band level, calls `.with_level(l)` after `with_asset_expect` (line 130). Optionally gate apex species behind `difficulty >= n` in manifest density functions |
| `world/src/site/plot/*.rs` | Dungeon plots stop being uniformly hardcoded: each plot derives a tier from the difficulty of its host chunk and passes `with_level(tier_band(d) + role_offset)` — trash +0, elites +2, boss +4. Asset paths stay hardcoded (species identity per dungeon is fine); only levels are parametrized |
| `rtsim/src/data/npc.rs` | `Npc.level: u16` with `#[serde(default = "default_level")]` (=1) so existing rtsim saves load unchanged |
| `rtsim/src/rule/architect.rs` | On spawn in `architect_tick`, assign level from home-site context: guards = site-region band midpoint +2; `Adventurer(tier)` = `1 + tier * 8` (tiers 0–3 → 1/9/17/25); cultists/pirates = their site's band roll. Non-combat professions stay level 1 |
| Server spawn glue | Wherever rtsim NPCs and `EntityInfo` become ECS entities, propagate level into the `Level` component and apply Section 2 scaling |

### 4. NPC Classes

Combat NPCs map `Profession` (`common/src/rtsim.rs:485`) to `ClassKind` (classes-races
spec) for kit selection and nameplate flavor:

| Profession | ClassKind | Notes |
|---|---|---|
| Guard, Captain | Warrior | Sword/shield kits, taunt-adjacent abilities |
| Hunter | Ranger | Bow skillsets |
| Adventurer(t) | tier 0–1 Rogue, tier 2–3 Warrior | Mirrors gear progression |
| Cultist | Warlock | Matches existing cultist staff/sceptre assets |
| Pirate(_) | Rogue | Leader (`Pirate(true)`) +3 levels |
| Herbalist, Alchemist | Druid (non-hostile) | Flavor only; no combat kit change in Phase 2 |
| Farmer, Merchant, Blacksmith, Chef | None | Civilians remain classless |

The mapping is a pure function in `common/src/rtsim.rs`; consumed where loadouts/skillsets
are picked for rtsim NPC bodies.

### 5. UI

- **Nameplates:** revive the dormant `level`/`level_skull` widget ids in
  `voxygen/src/hud/overhead.rs:48-49`. Show numeric level next to the name; show the skull
  icon when target level ≥ player level + 8. Data source: the synced `Level` component.
- **Map overlay:** new toggle in the map HUD rendering a difficulty heat tint (green 1–3,
  yellow 4–6, orange 7–8, red 9–10). The server includes a downsampled difficulty grid
  (1 byte per 4×4-chunk cell, ~64 KiB at default map size) in the world-map message sent
  on connect; no live streaming needed since difficulty is static.

### 6. XP Rules

Current: `exp_reward` comes from `combat::combat_rating` of the victim
(`server/src/events/entity_manipulation.rs:1122`), split by damage contribution and group
size. Levels add a differential multiplier at that call site:

```
delta = victim_level - player_level
xp_mult(delta) = 0                          if delta <= -10   // gray mob
               = clamp(1.0 + 0.10 * delta, 0.25, 2.0) otherwise
```

- Gray cutoff at −10 makes outleveled zones worthless, pushing travel.
- +100% cap at +10 keeps "boosting" in red zones bounded.
- **Anti-farm:** per-player ring buffer (server-side, in-memory) of the last 50 kills with
  entity-config id and timestamp; each repeat kill of the same config within 10 minutes
  applies ×0.9 cumulative (floor ×0.2), decaying back after the window. Counters are not
  persisted — a relog resets them, which is acceptable friction for the gain.
- Group split keeps the existing `/ sqrt(members)` rule; the differential uses the
  highest-level group member to prevent low-level mules inflating rewards.

### 7. Map Enlargement

`GenOpts` (`world/src/sim/mod.rs:148-160`) already supports it — this is a cost decision,
not an engineering one:

| Size | Chunks | Relative gen time | Peak gen RAM | Map file | Verdict |
|---|---|---|---|---|---|
| 2^10 (default) | 1.05 M | 1× (hours) | ~1× (baseline) | ~1× | Current |
| 2^11 | 4.19 M | ~5–6× (erosion is superlinear) | ~4× | ~4× | Viable on a beefy gen box, days of compute |
| 2^12 | 16.8 M | ~25–30× | ~16× | ~16× | Not viable this year |

**Recommendation: defer.** At 1024² chunks (~32×32 km) the difficulty formula in Section 1
yields ~8–10 distinguishable bands with the `/64.0` distance constant — ample for a 1–30
level game. Difficulty layering extracts far more gameplay per chunk than raw acreage;
revisit 2^11 only if zone density feels cramped after Phase 3 playtesting, and treat it as
an offline asset-bake task (generate once, ship the map file).

### 8. Multi-Plane Worlds (XL, long-term sketch)

Per the lore-cosmology spec, the setting has multiple planes. Architecture sketch, in
recommended order:

**Phase A — Pocket planes (intra-world, ships with Phase 4).** Reserve a margin band of the
existing world grid (e.g. chunks beyond a "plane boundary" rect) for themed pocket-plane
sites generated like dungeons but skinned as other planes. Portals reuse the existing
machinery unchanged: `Object::Portal` + `PortalData { target }`
(`common/src/comp/misc.rs:19,54`) already teleports within one world. Cheap, shippable,
validates the *content* of planes before the infrastructure.

**Phase B — True multi-world.**
- `PlaneId(u16)` newtype in `common/src/rtsim.rs`-adjacent shared code; positions in
  cross-plane contexts become `(PlaneId, Vec3<f32>)`.
- Server hosts N `World` instances, each with its own `IndexOwned`, terrain/chunk state,
  and its own rtsim `Data` file (`data/rtsim/plane_{id}/`). One ECS `specs::World` per plane
  is the honest model — entities never co-exist across planes, so cross-plane systems
  reduce to a transfer queue.
- `PortalData` gains `target_plane: Option<PlaneId>` (`None` = same plane, fully
  backward-compatible serde). Cross-plane teleport = serialize the player's persisted
  components, despawn in plane A, enqueue spawn in plane B — structurally identical to the
  existing login flow, which is why it is tractable.
- Persistence: character location in the DB gains a plane column with default 0; rtsim data
  is already per-file so per-plane files are a directory layout change.
- Out of scope even for Phase B: cross-plane chat/trade federation and multi-process
  sharding (one process, N worlds first; shard only if tick budget demands it).

## Phases

| Phase | Scope | Complexity | Est. (1 senior dev + AI) |
|---|---|---|---|
| 1 | Region difficulty + Level component + stat scaling | M | 5–7 days |
| 2 | Spawn pipeline: wildlife, dungeons, rtsim, classes | L | 8–12 days |
| 3 | XP differential, anti-farm, UI (nameplates + map) | M | 5–7 days |
| 4 | Pocket planes + multi-plane groundwork | XL | 15–25 days |

### Phase 1 — Difficulty field and entity levels (M)

- **Deliverables:** `SimChunk.difficulty` + smoothing pass; `Level` component synced to
  clients; HP/damage scaling at spawn; `level_damage_multiplier` in combat path.
- **Milestone:** `/tp` to a far corner of the map, kill a wolf, observe ×4 HP vs a
  starting-zone wolf; difficulty values dumpable via a debug command.
- **Tasks:** worldgen pass after civ placement; component + registration + sync; spawn-time
  scaling in server NPC construction; replace `world/src/lib.rs:279-284` consumer; unit
  tests for the formula (monotonic in distance, clamped, deterministic).
- **Risks:** difficulty formula constants need map-specific tuning (mitigate: constants in a
  RON config under `asset_tweak`); accidental double-scaling on entities that already carry
  health overrides (audit `EntityConfig` health-related fields during implementation).

### Phase 2 — Leveled spawns everywhere (L)

- **Deliverables:** `EntityInfo.level` + builder; wildlife levels from chunk difficulty;
  dungeon plot role offsets; rtsim `Npc.level` + Architect assignment; Profession→ClassKind
  mapping; rank-based skillset selection; `loot_tiered` plumbing (assets migrated for the
  top ~20 creature tables only).
- **Milestone:** new world: starting-zone mobs L1–3, far-zone mobs L25+, dungeon boss +4
  over trash, town guards leveled by region; rank5 mobs visibly use upgraded abilities.
- **Risks:** dungeon plots are many files with repetitive edits (mitigate: helper on the
  plot trait, mechanical sweep); rtsim serde default must be tested against a pre-change
  save before merging; skillset override may surprise bespoke-boss configs (skip any config
  whose `meta` already sets `SkillSetAsset`).

### Phase 3 — XP economy and visibility (M)

- **Deliverables:** differential multiplier + gray cutoff at the `entity_manipulation.rs`
  XP site; anti-farm ring buffer; nameplate levels + skull; map difficulty overlay +
  world-map message extension.
- **Milestone:** leveling a fresh character to ~10 by following the difficulty gradient
  feels directed; farming gray mobs yields 0 XP; map overlay matches in-world danger.
- **Risks:** balance constants wrong on first pass (mitigate: Section "Testing" balance
  sims before playtest); world-map message size on huge maps (downsampling already bounds
  it).

### Phase 4 — Planes (XL)

- **Deliverables:** 1–2 pocket-plane site kinds with portal entrances/exits and difficulty
  9–10 interiors; `PlaneId` type + `PortalData.target_plane: Option<PlaneId>` (serde-default
  `None`, unused at runtime yet); design-validated transfer-queue prototype behind a feature
  flag.
- **Milestone:** walk into a portal in the overworld, arrive in a themed pocket plane,
  fight L28–30 mobs, portal back.
- **Risks:** scope creep toward full multi-world (mitigate: Phase B explicitly out of this
  phase); rtsim NPCs pathing into pocket-plane reserved chunks (exclude the margin band from
  rtsim nav).

## Testing Strategy

- **Worldgen unit tests** (`world/`): difficulty deterministic for a fixed seed; every town
  chunk is difficulty 1–2; values clamp to 1..=10 (mirror the existing
  `test_biome_difficulty` pattern in `common/src/terrain/biome.rs:43`).
- **Spawn distribution tests:** extend the existing wildlife manifest tests in
  `world/src/layer/wildlife.rs` (the test scaffolding around `spawn_manifest()` at lines
  698–741 already instantiates every entity asset): sample N chunks per difficulty band,
  assert spawned levels fall inside the band and skillset rank matches `ceil(level/6)`.
- **Balance simulations:** headless harness (new `bin_*`-style utility, matching the
  existing `bin_csv` pattern) that pits a reference player loadout per level against band
  mobs and reports time-to-kill / time-to-die curves; run before each constants change.
- **Playtest protocol via admin commands** (`common/src/cmd.rs` — `Tp`, `RtsimTp` verified):
  scripted sweep `/tp` to 8 compass-point coordinates at increasing radii, at each stop
  record mob levels, nameplates, XP per kill, and gray-cutoff behavior; cross-check against
  the map overlay. Run with `VELOREN_ASSETS` set per CLAUDE.md.
- **Save-compat tests:** load a pre-Phase-2 rtsim data file and a pre-Phase-1 character DB,
  assert defaults (`level = 1`, plane column 0) apply cleanly.

## Open Questions

1. Should water/cave layers (`world/src/layer/cave.rs`) use surface chunk difficulty or get
   a depth-based bonus? Leaning +1 tier per ~60 m below surface, decide in Phase 2.
2. Do tamed/pet creatures keep their spawn level or inherit owner level? Interacts with the
   character-levels spec; default to keeping spawn level until that spec lands.
3. Whether the difficulty-1 safe disc should also suppress hostile *spawns* entirely (true
   safe zones) or only their levels. Suppression is a one-line density gate in
   `wildlife.rs` manifests if playtests want it.
4. Project Oracle integration point: it should consume `World::get_chunk_difficulty` as
   read-only context first; whether it may ever *mutate* zone difficulty stays open until
   that spec is implemented.
