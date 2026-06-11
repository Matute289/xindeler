# PROJECT ORACLE (Autonomous World Director) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A world-director layer inside rtsim that generates, validates, schedules, and resolves world events with persistent typed consequences (`WorldFact`s) and an append-only causal chronicle — observable end-to-end through the fork's telemetry pipeline, with anti-chaos invariants enforced in code from day one.

**Architecture:** ORACLE state is a new `#[serde(default)]` field `oracle: OracleData` on rtsim `Data` (`rtsim/src/data/mod.rs:40`), following the `architect` precedent — no `CURRENT_VERSION` bump, old saves load with empty ORACLE state. Two new rtsim rules mirror the Architect pattern (`rtsim/src/rule/architect.rs`, strided every 32 ticks): `OracleWorldState` folds rtsim events into the chronicle; `OracleEventEngine` advances the `Proposed → Validated → Scheduled → Active → Resolving → Resolved` lifecycle as pure data transitions. Only the event engine mutates event state; the admin command (and the future LLM proposer) merely enqueue `Proposed` events. Every transition appends a chronicle entry and emits `common::telemetry!` (macro at `common/src/lib.rs:24`). Density caps and class cooldowns are checked at validation *and* re-checked at trigger time. All engine logic is pure over `OracleData`, so it unit-tests without an `RtState`.

**Tech Stack:** Rust nightly (2024 edition), specs ECS, rtsim rule/event system, rmp-serde persistence, slotmap ids. Design spec: `docs/superpowers/specs/2026-06-10-project-oracle-design.md`. Companion plan: `docs/superpowers/plans/2026-06-11-project-aurora.md` (NPC minds — reads ORACLE's fact store).

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim` (and `-p veloren-server` / `-p veloren-common` where stated). Crate names verified in `rtsim/Cargo.toml:2`, `server/Cargo.toml:2`.
- Branch: create `feature/oracle-phase1` off `development` before Task 1.
- Invoke the `veloren-oracle` skill for context and the `superpowers:test-driven-development` skill before writing code.
- Every new field reachable from rtsim `Data` gets `#[serde(default)]` and joins the old-save fixture test (Task 2) — extend that test when you add fields.
- Every lifecycle transition emits `common::telemetry!("oracle_event", ...)`; vetoes emit `"oracle_veto"`; chronicle appends emit `"oracle_chronicle"`. Never remove these — the soak harness (Phase 8) and the veloren-telemetry skill depend on them.
- No wildcard `_ =>` arms on `WorldFact`, `EventState`, or `ChronicleKind` matches outside tests — exhaustiveness keeps future variants safe.

---

## Phase 1: World State Engine (full TDD)

### Task 1: `WorldFact` typed fact store

**Files:**
- Create: `rtsim/src/data/oracle/mod.rs` (skeleton: `pub mod facts;`), `rtsim/src/data/oracle/facts.rs`
- Modify: `rtsim/src/data/mod.rs:1-9` (insert `pub mod oracle;` between `pub mod npc;` and `pub mod quest;`)

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/data/oracle/facts.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::{resources::TimeOfDay, rtsim::{FactionId, SiteId}};
    use slotmap::KeyData;

    fn faction(n: u64) -> FactionId { KeyData::from_ffi(n).into() }
    fn site(n: u64) -> SiteId { KeyData::from_ffi(n).into() }

    #[test]
    fn same_key_replaces_and_at_war_is_symmetric() {
        let mut facts = WorldFacts::default();
        let first = facts.assert(WorldFact::SiteControlled { site: site(1), by: faction(1) });
        let second = facts.assert(WorldFact::SiteControlled { site: site(1), by: faction(2) });
        assert!(second > first, "fact ids must be monotonic");
        assert_eq!(facts.len(), 1, "same-key fact must replace, not duplicate");
        assert!(facts.get(first).is_none());
        assert!(matches!(facts.get(second),
            Some(WorldFact::SiteControlled { by, .. }) if *by == faction(2)));
        facts.assert(WorldFact::AtWar { a: faction(1), b: faction(2), since: TimeOfDay(0.0) });
        facts.assert(WorldFact::AtWar { a: faction(2), b: faction(1), since: TimeOfDay(5.0) });
        assert_eq!(facts.len(), 2, "AtWar(a,b) and AtWar(b,a) are the same fact");
    }

    #[test]
    fn retract_removes_and_store_roundtrips_msgpack() {
        let mut facts = WorldFacts::default();
        let id = facts.assert(WorldFact::OmenSighted { region: RegionId(3), omen: OmenKind::Comet });
        facts.assert(WorldFact::Plague { region: RegionId(7), severity: 0.8 });
        assert!(facts.retract(id).is_some());
        assert!(facts.get(id).is_none());
        let bytes = rmp_serde::to_vec_named(&facts).expect("serialize");
        let de: WorldFacts = rmp_serde::from_slice(&bytes).expect("deserialize");
        assert_eq!(de.len(), 1);
    }
}
```

Create `rtsim/src/data/oracle/mod.rs` with just `pub mod facts;` and add `pub mod oracle;` to `rtsim/src/data/mod.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim oracle`
Expected: FAIL to compile with "cannot find type `WorldFacts`" (and `WorldFact`, `RegionId`).

- [ ] **Step 3: Implement the fact store**

Fill in `rtsim/src/data/oracle/facts.rs` above the test module:

```rust
use common::{resources::TimeOfDay, rtsim::{Actor, FactionId, SiteId}};
use serde::{Deserialize, Serialize};
use slotmap::Key;
use std::collections::BTreeMap;

/// Region id for ORACLE's world model. Phase 1 keeps this an opaque index;
/// Phase 3 derives regions from weather-cell-aligned tiles.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u16);

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FactId(pub u64);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FestivalKind { Harvest, Wedding, Games }

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OmenKind { BloodMoon, Comet, DarkSun, ManaStorm }

/// Typed world facts — the lingua franca between ORACLE (sole writer) and
/// AURORA (reader). One variant per consequence class; add variants, never
/// repurpose them (rmp-serde names variants, renames break old saves).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WorldFact {
    AtWar { a: FactionId, b: FactionId, since: TimeOfDay },
    SiteControlled { site: SiteId, by: FactionId },
    Plague { region: RegionId, severity: f32 },
    Drought { region: RegionId, severity: f32 },
    FoodShortage { site: SiteId, severity: f32 },
    FestivalActive { site: SiteId, kind: FestivalKind, ends: TimeOfDay },
    BountyOn { actor: Actor, gold: u32, reason_chronicle_id: u64 },
    OmenSighted { region: RegionId, omen: OmenKind },
}

impl WorldFact {
    /// Identity excluding payload: (variant tag, subject a, subject b).
    /// Asserting a fact whose identity already exists replaces it.
    fn key(&self) -> (u8, u64, u64) {
        match self {
            Self::AtWar { a, b, .. } => {
                let (x, y) = (a.data().as_ffi(), b.data().as_ffi());
                (0, x.min(y), x.max(y)) // symmetric: (a,b) == (b,a)
            },
            Self::SiteControlled { site, .. } => (1, site.data().as_ffi(), 0),
            Self::Plague { region, .. } => (2, region.0.into(), 0),
            Self::Drought { region, .. } => (3, region.0.into(), 0),
            Self::FoodShortage { site, .. } => (4, site.data().as_ffi(), 0),
            Self::FestivalActive { site, .. } => (5, site.data().as_ffi(), 0),
            Self::BountyOn { actor: Actor::Npc(id), .. } => (6, id.data().as_ffi(), 0),
            Self::BountyOn { actor: Actor::Character(id), .. } => (6, id.0 as u64, 1),
            Self::OmenSighted { region, .. } => (7, region.0.into(), 0),
        }
    }
}

/// The typed fact store. ORACLE writes (assert/retract), AURORA reads.
/// BTreeMap keeps iteration deterministic (design principle 5).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WorldFacts {
    next_id: u64,
    facts: BTreeMap<FactId, WorldFact>,
}

impl WorldFacts {
    /// Assert a fact, replacing any existing fact with the same identity.
    /// O(n) over live facts — fine at world-event scale (tens).
    pub fn assert(&mut self, fact: WorldFact) -> FactId {
        let key = fact.key();
        self.facts.retain(|_, f| f.key() != key);
        let id = FactId(self.next_id);
        self.next_id += 1;
        self.facts.insert(id, fact);
        id
    }
    pub fn retract(&mut self, id: FactId) -> Option<WorldFact> { self.facts.remove(&id) }
    pub fn get(&self, id: FactId) -> Option<&WorldFact> { self.facts.get(&id) }
    pub fn iter(&self) -> impl Iterator<Item = (FactId, &WorldFact)> {
        self.facts.iter().map(|(id, f)| (*id, f))
    }
    pub fn len(&self) -> usize { self.facts.len() }
    pub fn is_empty(&self) -> bool { self.facts.is_empty() }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim oracle`
Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/oracle/ rtsim/src/data/mod.rs
git commit -m "feat(oracle): typed WorldFact store with replace-on-assert semantics"
```

---

### Task 2: Chronicle + `OracleData` field on rtsim `Data` (old-save fixture)

**Files:**
- Create: `rtsim/src/data/oracle/chronicle.rs`
- Modify: `rtsim/src/data/oracle/mod.rs` (the `OracleData` struct)
- Modify: `rtsim/src/data/mod.rs:39-70` (new `Data` field after `quests`; fixture test at end of file)

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/data/oracle/chronicle.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::resources::TimeOfDay;

    fn entry(c: &mut Chronicle, causes: Vec<u64>) -> u64 {
        c.append(TimeOfDay(0.0), ChronicleKind::EventStarted, causes, vec![], "test-key")
    }

    #[test]
    fn ids_monotonic_forward_causes_dropped_and_chain_walks_to_root() {
        let mut c = Chronicle::default();
        let root = entry(&mut c, vec![5]); // entry 5 does not exist yet
        assert_eq!(root, 0);
        assert!(c.get(root).unwrap().causes.is_empty(), "forward refs must be dropped");
        let mid = entry(&mut c, vec![root]);
        let leaf = entry(&mut c, vec![mid]);
        // Walk causes backwards (the "library NPC" query from the spec).
        let mid_id = c.get(leaf).unwrap().causes[0];
        assert_eq!(c.get(mid_id).unwrap().causes[0], root);
        assert_eq!(c.validate_causal_chain(), Ok(()));
    }

    #[test]
    fn missing_cause_detected_after_bad_compaction() {
        let mut c = Chronicle::default();
        let a = entry(&mut c, vec![]);
        let _b = entry(&mut c, vec![a]);
        c.entries.remove(0); // simulate a buggy compaction dropping a referenced entry
        assert_eq!(
            c.validate_causal_chain(),
            Err(CausalChainError::MissingCause { entry: 1, cause: 0 })
        );
    }
}
```

At the end of `rtsim/src/data/mod.rs`, add the old-save fixture test:

```rust
#[cfg(test)]
mod oracle_serde_tests {
    use super::*;
    use crate::data::nature::Chunk;
    use common::grid::Grid;
    use vek::Vec2;

    /// Mirrors the on-disk shape of `Data` *before* the `oracle` field
    /// existed. rmp-serde writes named fields (`write_named`), so a missing
    /// `oracle` key must fall back to `#[serde(default)]`. `nature` is the
    /// only non-defaulted field, so it is all the fixture needs.
    #[derive(Serialize)]
    struct LegacyData {
        version: u32,
        nature: Nature,
    }

    #[test]
    fn old_save_without_oracle_field_loads_and_new_state_roundtrips() {
        let legacy = LegacyData {
            version: CURRENT_VERSION,
            nature: Nature {
                chunks: Grid::populate_from(Vec2::new(1, 1), |_| Chunk {
                    res: EnumMap::default().map(|_, _| 1.0),
                }),
            },
        };
        let mut buf = Vec::new();
        rmp_serde::encode::write_named(&mut buf, &legacy).expect("serialize legacy save");
        let mut data = Data::from_reader(&buf[..]).expect("old save must load");
        assert!(data.oracle.facts.is_empty());
        assert!(data.oracle.chronicle.is_empty());
        // New ORACLE state must survive a save/load cycle.
        data.oracle.facts.assert(oracle::WorldFact::Plague {
            region: oracle::RegionId(1),
            severity: 0.4,
        });
        let mut buf2 = Vec::new();
        data.write_to(&mut buf2).expect("serialize");
        assert_eq!(Data::from_reader(&buf2[..]).expect("reload").oracle.facts.len(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim oracle`
Expected: FAIL to compile with "cannot find type `Chronicle`" and "no field named `oracle`".

- [ ] **Step 3: Implement the chronicle**

Above the test module in `rtsim/src/data/oracle/chronicle.rs`:

```rust
use super::facts::RegionId;
use common::{resources::TimeOfDay, rtsim::{Actor, FactionId, SiteId}};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum Subject { Actor(Actor), Site(SiteId), Faction(FactionId), Region(RegionId) }

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChronicleKind {
    // Event lifecycle (written only by the event engine, Task 6)
    EventProposed, EventValidated, EventRejected, EventScheduled, EventExpired,
    EventStarted, EventStageAdvanced, EventResolved, EventAnnulled,
    // Fact bookkeeping
    FactAsserted, FactRetracted,
    // World observations (Task 3)
    PlayerDeed, Theft,
    // Downtime catch-up marker (Phase 8)
    SimulatedOffline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChronicleEntry {
    /// Monotonic, never reused.
    pub id: u64,
    pub at: TimeOfDay,
    pub kind: ChronicleKind,
    /// Ids of prior entries that caused this one — the causal chain.
    pub causes: Vec<u64>,
    pub subjects: Vec<Subject>,
    /// i18n key for template text; LLM prose is cached separately (Phase 6).
    pub summary_key: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CausalChainError {
    MissingCause { entry: u64, cause: u64 },
    NonCausalOrder { entry: u64, cause: u64 },
}

/// Append-only historical record. Entries are stored in id order (push-only),
/// so `get` is a binary search. Compaction/archival arrives in Phase 8.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Chronicle {
    next_id: u64,
    entries: Vec<ChronicleEntry>,
}

impl Chronicle {
    /// Append an entry and emit telemetry. Causes referencing entries that do
    /// not (yet) exist are dropped: the chronicle can never hold a forward
    /// reference, which makes `validate_causal_chain` a hard invariant.
    pub fn append(&mut self, at: TimeOfDay, kind: ChronicleKind, mut causes: Vec<u64>,
                  subjects: Vec<Subject>, summary_key: impl Into<String>) -> u64 {
        let id = self.next_id;
        causes.retain(|&c| c < id);
        common::telemetry!("oracle_chronicle", id = id, kind = ?kind, causes = ?causes);
        self.next_id += 1;
        self.entries.push(ChronicleEntry {
            id, at, kind, causes, subjects, summary_key: summary_key.into(),
        });
        id
    }
    pub fn get(&self, id: u64) -> Option<&ChronicleEntry> {
        self.entries.binary_search_by_key(&id, |e| e.id).ok().map(|i| &self.entries[i])
    }
    pub fn iter(&self) -> impl Iterator<Item = &ChronicleEntry> { self.entries.iter() }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Audit invariant (nightly chronicle audit, spec Section 14): every
    /// cause exists and strictly precedes its entry — no cycles possible.
    pub fn validate_causal_chain(&self) -> Result<(), CausalChainError> {
        for entry in &self.entries {
            for &cause in &entry.causes {
                if cause >= entry.id {
                    return Err(CausalChainError::NonCausalOrder { entry: entry.id, cause });
                }
                if self.get(cause).is_none() {
                    return Err(CausalChainError::MissingCause { entry: entry.id, cause });
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Implement `OracleData` and the `Data` field**

Replace the contents of `rtsim/src/data/oracle/mod.rs` with:

```rust
pub mod chronicle;
pub mod facts;

pub use self::{
    chronicle::{Chronicle, ChronicleEntry, ChronicleKind, Subject},
    facts::{FactId, RegionId, WorldFact, WorldFacts},
};
use serde::{Deserialize, Serialize};

/// PROJECT ORACLE world-director state. Lives inside rtsim [`crate::data::Data`]
/// behind `#[serde(default)]` — old saves load with empty ORACLE state, no
/// `CURRENT_VERSION` bump (see `rtsim/src/data/mod.rs:32` for the policy).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct OracleData {
    #[serde(default)]
    pub facts: WorldFacts,
    #[serde(default)]
    pub chronicle: Chronicle,
}
```

In `rtsim/src/data/mod.rs`, inside `struct Data` directly after the `quests` field (line ~57):

```rust
    #[serde(default)]
    pub oracle: oracle::OracleData,
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim oracle`
Expected: PASS (chronicle + fixture + Task 1 tests).
Run: `cargo check -p veloren-rtsim -p veloren-server`
Expected: clean — `Data::generate` (`rtsim/src/generate/mod.rs`) and the server load path (`server/src/rtsim/mod.rs:41`) need no changes because `OracleData: Default`.

- [ ] **Step 6: Commit**

```bash
git add rtsim/src/data/oracle/ rtsim/src/data/mod.rs
git commit -m "feat(oracle): chronicle + OracleData field on rtsim Data with old-save fixture"
```

---

### Task 3: `OracleWorldState` rule — change detection into the chronicle

**Files:**
- Create: `rtsim/src/rule/oracle/mod.rs` (`pub mod world_state;`), `rtsim/src/rule/oracle/world_state.rs`
- Modify: `rtsim/src/rule/mod.rs:1-8` (add `pub mod oracle;` after `pub mod npc_ai;`)
- Modify: `rtsim/src/lib.rs:199-209` (`start_default_rules` — register the rule)
- Modify: `rtsim/src/data/oracle/mod.rs` (observation methods + tests)

- [ ] **Step 1: Write the failing tests**

At the end of `rtsim/src/data/oracle/mod.rs`, add:

```rust
#[cfg(test)]
mod observe_tests {
    use super::*;
    use common::{character::CharacterId, resources::TimeOfDay, rtsim::Actor};
    use slotmap::KeyData;

    fn npc(n: u64) -> Actor { Actor::Npc(KeyData::from_ffi(n).into()) }

    #[test]
    fn player_kill_chronicled_ambient_death_skipped_theft_chronicled() {
        let mut oracle = OracleData::default();
        assert!(oracle.observe_death(TimeOfDay(10.0), npc(1), None).is_none());
        assert!(oracle.observe_death(TimeOfDay(10.0), npc(1), Some(npc(2))).is_none());
        assert!(oracle.chronicle.is_empty(), "ambient NPC churn is not chronicled");
        let player = Actor::Character(CharacterId(42));
        let id = oracle
            .observe_death(TimeOfDay(10.0), npc(1), Some(player))
            .expect("player kills are deeds");
        let entry = oracle.chronicle.get(id).unwrap();
        assert_eq!(entry.kind, ChronicleKind::PlayerDeed);
        assert!(matches!(entry.subjects[0], Subject::Actor(Actor::Character(_))));
        let site = KeyData::from_ffi(3).into();
        let id = oracle.observe_theft(TimeOfDay(20.0), npc(1), Some(site));
        assert_eq!(oracle.chronicle.get(id).unwrap().kind, ChronicleKind::Theft);
        assert!(matches!(oracle.chronicle.get(id).unwrap().subjects[1], Subject::Site(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim observe`
Expected: FAIL to compile with "no method named `observe_death`".

- [ ] **Step 3: Implement observation methods**

In `rtsim/src/data/oracle/mod.rs`, add `use common::{resources::TimeOfDay, rtsim::{Actor, SiteId}};` and:

```rust
impl OracleData {
    /// Fold a death into the chronicle. Phase 1 records only player-caused
    /// deaths (player deeds, spec Section 8); ambient NPC churn already lives
    /// in `Architect::deaths` (`rtsim/src/data/architect.rs:137`) and would
    /// grow the chronicle without bound.
    pub fn observe_death(&mut self, at: TimeOfDay, victim: Actor, killer: Option<Actor>)
    -> Option<u64> {
        let killer = match killer {
            Some(k @ Actor::Character(_)) => k,
            Some(Actor::Npc(_)) | None => return None,
        };
        Some(self.chronicle.append(
            at,
            ChronicleKind::PlayerDeed,
            Vec::new(),
            vec![Subject::Actor(killer), Subject::Actor(victim)],
            "oracle-deed-slain",
        ))
    }

    /// Thefts are rare and player-driven (`OnTheft`, `rtsim/src/event.rs:72`),
    /// so all of them are chronicled.
    pub fn observe_theft(&mut self, at: TimeOfDay, thief: Actor, site: Option<SiteId>) -> u64 {
        let mut subjects = vec![Subject::Actor(thief)];
        if let Some(site) = site {
            subjects.push(Subject::Site(site));
        }
        self.chronicle.append(at, ChronicleKind::Theft, Vec::new(), subjects, "oracle-deed-theft")
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim observe` — expected: PASS.

- [ ] **Step 5: Wire the rule**

Create `rtsim/src/rule/oracle/mod.rs` containing `pub mod world_state;`, then `rtsim/src/rule/oracle/world_state.rs`:

```rust
use crate::{
    RtState, Rule, RuleError,
    event::{EventCtx, OnDeath, OnTheft, OnTick},
};

/// How many ticks the ORACLE world-state fold skips. Matches
/// `ARCHITECT_TICK_SKIP` (`rtsim/src/rule/architect.rs:27`).
const ORACLE_TICK_SKIP: u64 = 32;

/// Change detection: folds rtsim events into ORACLE's chronicle and emits a
/// strided telemetry heartbeat. This rule only records — mechanical effects
/// belong to the event engine.
pub struct OracleWorldState;

impl Rule for OracleWorldState {
    fn start(rtstate: &mut RtState) -> Result<Self, RuleError> {
        rtstate.bind::<Self, OnDeath>(on_death);
        rtstate.bind::<Self, OnTheft>(on_theft);
        rtstate.bind::<Self, OnTick>(on_tick);
        Ok(Self)
    }
}

fn on_death(ctx: EventCtx<OracleWorldState, OnDeath>) {
    let data = &mut *ctx.state.data_mut();
    let at = data.time_of_day;
    data.oracle.observe_death(at, ctx.event.actor, ctx.event.killer);
}

fn on_theft(ctx: EventCtx<OracleWorldState, OnTheft>) {
    let data = &mut *ctx.state.data_mut();
    let at = data.time_of_day;
    data.oracle.observe_theft(at, ctx.event.actor, ctx.event.site);
}

fn on_tick(ctx: EventCtx<OracleWorldState, OnTick>) {
    if !ctx.event.tick.is_multiple_of(ORACLE_TICK_SKIP) {
        return;
    }
    let data = &*ctx.state.data();
    common::telemetry!(
        "oracle_tick",
        tick = ctx.event.tick,
        facts = data.oracle.facts.len(),
        chronicle = data.oracle.chronicle.len(),
    );
}
```

Add `pub mod oracle;` to `rtsim/src/rule/mod.rs` (after `pub mod npc_ai;`). In `rtsim/src/lib.rs` `start_default_rules` (line ~199), after the `rule::report::ReportEvents` line, add:

```rust
        self.start_rule::<rule::oracle::world_state::OracleWorldState>();
```

- [ ] **Step 6: Verify build and full crate tests**

Run: `cargo check -p veloren-rtsim -p veloren-server` — expected clean.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim` — expected PASS (no regressions in `ai`/`sentiment` tests).

- [ ] **Step 7: Commit**

```bash
git add rtsim/src/rule/ rtsim/src/lib.rs rtsim/src/data/oracle/mod.rs
git commit -m "feat(oracle): OracleWorldState rule folds deaths/thefts into the chronicle"
```

---

## Phase 2: Event Engine (full TDD)

### Task 4: Event taxonomy + lifecycle state machine as data

**Files:**
- Create: `rtsim/src/data/oracle/events.rs`
- Modify: `rtsim/src/data/oracle/mod.rs` (add `pub mod events;`, re-exports, `events` field on `OracleData`)

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/data/oracle/events.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::resources::TimeOfDay;

    fn event() -> WorldEvent {
        WorldEvent {
            class: EventClass::Festive,
            template: "harvest_festival".into(),
            region: Some(RegionId(0)),
            site: None,
            visible: true,
            proposed_at: TimeOfDay(0.0),
            state: EventState::Proposed,
            asserted_facts: Vec::new(),
            chronicle_root: None,
        }
    }

    #[test]
    fn full_lifecycle_is_legal_and_terminal_is_final() {
        let mut e = event();
        e.transition(EventState::Validated).unwrap();
        e.transition(EventState::Scheduled { start_at: TimeOfDay(10.0) }).unwrap();
        e.transition(EventState::Active { stage: 1, until: TimeOfDay(20.0) }).unwrap();
        e.transition(EventState::Active { stage: 2, until: TimeOfDay(30.0) }).unwrap();
        e.transition(EventState::Resolving).unwrap();
        e.transition(EventState::Resolved { at: TimeOfDay(30.0) }).unwrap();
        assert!(e.state.is_terminal());
        assert!(e.transition(EventState::Validated).is_err(), "terminal accepts nothing");
    }

    #[test]
    fn illegal_jumps_rejected_without_mutation_and_stages_escalate_by_one() {
        let mut e = event();
        let err = e.transition(EventState::Active { stage: 1, until: TimeOfDay(1.0) }).unwrap_err();
        assert_eq!(err, IllegalTransition { from: "Proposed", to: "Active" });
        assert!(matches!(e.state, EventState::Proposed), "failed transition must not mutate");
        e.transition(EventState::Validated).unwrap();
        e.transition(EventState::Scheduled { start_at: TimeOfDay(0.0) }).unwrap();
        assert!(e.transition(EventState::Active { stage: 2, until: TimeOfDay(1.0) }).is_err());
        e.transition(EventState::Active { stage: 1, until: TimeOfDay(1.0) }).unwrap();
        assert!(e.transition(EventState::Active { stage: 3, until: TimeOfDay(2.0) }).is_err());
        assert!(e.transition(EventState::Active { stage: 1, until: TimeOfDay(2.0) }).is_err());
        e.transition(EventState::Active { stage: 2, until: TimeOfDay(2.0) }).unwrap();
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim events`
Expected: FAIL to compile with "cannot find type `WorldEvent`".

- [ ] **Step 3: Implement taxonomy, lifecycle, and store**

Above the tests in `rtsim/src/data/oracle/events.rs`:

```rust
use super::facts::{FactId, RegionId};
use common::{resources::TimeOfDay, rtsim::SiteId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The eight event classes from the design spec (Section 2.1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventClass {
    Military, Political, Economic, Natural, Magical, Religious, Festive, Ecological,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub u64);

/// Lifecycle (spec Section 2.2), stored as data. `TimeOfDay` has no
/// `PartialEq`, so neither does this enum — compare with `matches!`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventState {
    Proposed,
    Validated,
    Scheduled { start_at: TimeOfDay },
    Active { stage: u8, until: TimeOfDay },
    Resolving,
    Resolved { at: TimeOfDay },
    Rejected,
    Expired,
}

impl EventState {
    /// Variant name for telemetry and `IllegalTransition` — an exhaustive
    /// match over all 8 variants returning e.g. "Scheduled" for
    /// `Scheduled { .. }`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Proposed => "Proposed",
            Self::Validated => "Validated",
            Self::Scheduled { .. } => "Scheduled",
            Self::Active { .. } => "Active",
            Self::Resolving => "Resolving",
            Self::Resolved { .. } => "Resolved",
            Self::Rejected => "Rejected",
            Self::Expired => "Expired",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Resolved { .. } | Self::Rejected | Self::Expired)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: &'static str,
    pub to: &'static str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldEvent {
    pub class: EventClass,
    /// Template name (built-in registry in Task 6; RON assets in Phase 6).
    pub template: String,
    pub region: Option<RegionId>,
    pub site: Option<SiteId>,
    /// Visible events count against the density caps; invisible ones are
    /// cheap background sim ("spotlight fairness", spec Section 2.4).
    pub visible: bool,
    pub proposed_at: TimeOfDay,
    pub state: EventState,
    /// Facts asserted by this event's activation effects, retracted on
    /// resolution/annulment — effect-inverse bookkeeping (spec Section 10).
    pub asserted_facts: Vec<FactId>,
    /// Chronicle entry recording the proposal — causal root for all later
    /// transition entries.
    pub chronicle_root: Option<u64>,
}

impl WorldEvent {
    /// The only legal edges of the lifecycle. On error the state is left
    /// untouched — an illegal transition is an engine bug, never a data
    /// condition; callers log and move on.
    pub fn transition(&mut self, to: EventState) -> Result<(), IllegalTransition> {
        use EventState::*;
        let legal = matches!(
            (&self.state, &to),
            (Proposed, Validated | Rejected)
                | (Validated, Scheduled { .. } | Expired)
                | (Scheduled { .. }, Active { stage: 1, .. } | Expired)
                | (Active { .. }, Resolving)
                | (Resolving, Resolved { .. })
        ) || matches!(
            (&self.state, &to),
            (Active { stage: a, .. }, Active { stage: b, .. }) if *b == a + 1
        );
        if legal {
            self.state = to;
            Ok(())
        } else {
            Err(IllegalTransition { from: self.state.name(), to: to.name() })
        }
    }
}

/// Deterministic id-ordered event store (BTreeMap iteration order is the
/// engine's processing order — design principle 5). Same shape as
/// `WorldFacts`: `next_id: u64` + `events: BTreeMap<EventId, WorldEvent>`.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WorldEvents {
    next_id: u64,
    events: BTreeMap<EventId, WorldEvent>,
}

impl WorldEvents {
    pub fn insert(&mut self, event: WorldEvent) -> EventId {
        let id = EventId(self.next_id);
        self.next_id += 1;
        self.events.insert(id, event);
        id
    }
    pub fn get(&self, id: EventId) -> Option<&WorldEvent> { self.events.get(&id) }
    pub fn get_mut(&mut self, id: EventId) -> Option<&mut WorldEvent> { self.events.get_mut(&id) }
    pub fn iter(&self) -> impl Iterator<Item = (EventId, &WorldEvent)> {
        self.events.iter().map(|(id, e)| (*id, e))
    }
    pub fn len(&self) -> usize { self.events.len() }
    pub fn is_empty(&self) -> bool { self.events.is_empty() }
    /// Active *visible* events — the quantity the density caps bound.
    pub fn active_visible(&self) -> impl Iterator<Item = (EventId, &WorldEvent)> {
        self.iter().filter(|(_, e)| e.visible && matches!(e.state, EventState::Active { .. }))
    }
}
```

In `rtsim/src/data/oracle/mod.rs`: add `pub mod events;`, extend the re-exports with `events::{EventClass, EventId, EventState, WorldEvent, WorldEvents}`, and add `#[serde(default)] pub events: WorldEvents,` to `OracleData`. Per convention, extend the Task 2 fixture test with `assert!(data.oracle.events.is_empty());`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim events` then `... cargo test -p veloren-rtsim oracle`
Expected: 2 new tests PASS; fixture still PASS.

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/oracle/ rtsim/src/data/mod.rs
git commit -m "feat(oracle): event taxonomy and lifecycle state machine as data"
```

---

### Task 5: Validation layer — density caps and class cooldowns

**Files:**
- Create: `rtsim/src/data/oracle/validate.rs`
- Modify: `rtsim/src/data/oracle/mod.rs` (add `pub mod validate;`, `pacing` field)

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/data/oracle/validate.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::oracle::{OracleData, events::{EventState, WorldEvent}};

    const DAY: f64 = 60.0 * 60.0 * 24.0;

    fn proposed(region: u16, class: EventClass, visible: bool) -> WorldEvent {
        WorldEvent {
            class,
            template: "test".into(),
            region: Some(RegionId(region)),
            site: None,
            visible,
            proposed_at: TimeOfDay(0.0),
            state: EventState::Proposed,
            asserted_facts: Vec::new(),
            chronicle_root: None,
        }
    }

    fn with_active(oracle: &mut OracleData, region: u16) {
        let mut e = proposed(region, EventClass::Military, true);
        e.state = EventState::Active { stage: 1, until: TimeOfDay(99.0 * DAY) };
        oracle.events.insert(e);
    }

    #[test]
    fn density_caps_veto_visible_but_not_invisible_or_elsewhere() {
        let mut oracle = OracleData::default();
        with_active(&mut oracle, 0);
        let candidate = proposed(0, EventClass::Festive, true);
        assert_eq!(validate(&oracle, &candidate, TimeOfDay(0.0)), Err(Veto::RegionDensityCap));
        let invisible = proposed(0, EventClass::Festive, false);
        assert_eq!(validate(&oracle, &invisible, TimeOfDay(0.0)), Ok(()));
        let elsewhere = proposed(1, EventClass::Festive, true);
        assert_eq!(validate(&oracle, &elsewhere, TimeOfDay(0.0)), Ok(()));
        for region in 1..4 {
            with_active(&mut oracle, region);
        }
        let fifth = proposed(9, EventClass::Festive, true);
        assert_eq!(validate(&oracle, &fifth, TimeOfDay(0.0)), Err(Veto::GlobalDensityCap));
    }

    #[test]
    fn class_cooldown_blocks_then_expires() {
        let mut oracle = OracleData::default();
        oracle.pacing.record_activation(Some(RegionId(0)), EventClass::Military, TimeOfDay(0.0));
        let candidate = proposed(0, EventClass::Military, true);
        assert_eq!(validate(&oracle, &candidate, TimeOfDay(1.0 * DAY)), Err(Veto::ClassCooldown));
        assert_eq!(validate(&oracle, &candidate, TimeOfDay(31.0 * DAY)), Ok(()));
        let other_class = proposed(0, EventClass::Festive, true);
        assert_eq!(validate(&oracle, &other_class, TimeOfDay(1.0 * DAY)), Ok(()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim validate`
Expected: FAIL to compile with "cannot find function `validate`" / "cannot find type `Veto`".

- [ ] **Step 3: Implement validation and pacing**

Above the tests in `rtsim/src/data/oracle/validate.rs`:

```rust
use super::{OracleData, events::{EventClass, WorldEvent}, facts::RegionId};
use common::resources::TimeOfDay;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Hard anti-chaos invariants (design spec Section 10). These are code, not
/// configuration: relaxing them is a reviewed change, not a tuning knob.
pub const MAX_ACTIVE_VISIBLE_PER_REGION: usize = 1;
pub const MAX_ACTIVE_VISIBLE_GLOBAL: usize = 4;
/// Min in-game days between two events of the same class in the same region.
pub const CLASS_COOLDOWN_DAYS: f64 = 30.0;

const DAY_SECS: f64 = 60.0 * 60.0 * 24.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Veto { RegionDensityCap, GlobalDensityCap, ClassCooldown }

/// Per-(region, class) activation times backing the cooldown rule. A `None`
/// region keys global (region-less) events.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Pacing {
    last_activation: BTreeMap<(Option<RegionId>, EventClass), TimeOfDay>,
}

impl Pacing {
    pub fn record_activation(&mut self, region: Option<RegionId>, class: EventClass, at: TimeOfDay) {
        self.last_activation.insert((region, class), at);
    }
    pub fn cooldown_active(&self, region: Option<RegionId>, class: EventClass, now: TimeOfDay) -> bool {
        self.last_activation
            .get(&(region, class))
            .is_some_and(|last| now.0 - last.0 < CLASS_COOLDOWN_DAYS * DAY_SECS)
    }
}

/// Validate a candidate against density caps and cooldowns. Called twice per
/// event: at Proposed→Validated and again at trigger time (the world may have
/// changed in between). Invisible events bypass density caps but still
/// respect class cooldowns.
pub fn validate(oracle: &OracleData, event: &WorldEvent, now: TimeOfDay) -> Result<(), Veto> {
    if event.visible {
        let active: Vec<_> = oracle.events.active_visible().collect();
        if active.len() >= MAX_ACTIVE_VISIBLE_GLOBAL {
            return Err(Veto::GlobalDensityCap);
        }
        if let Some(region) = event.region
            && active.iter().filter(|(_, e)| e.region == Some(region)).count()
                >= MAX_ACTIVE_VISIBLE_PER_REGION
        {
            return Err(Veto::RegionDensityCap);
        }
    }
    if oracle.pacing.cooldown_active(event.region, event.class, now) {
        return Err(Veto::ClassCooldown);
    }
    Ok(())
}
```

In `rtsim/src/data/oracle/mod.rs`: add `pub mod validate;`, re-export `validate::{Pacing, Veto}`, add `#[serde(default)] pub pacing: Pacing,` to `OracleData`, and extend the fixture test with `assert!(!data.oracle.pacing.cooldown_active(None, oracle::EventClass::Military, TimeOfDay(0.0)));`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim validate` then `... cargo test -p veloren-rtsim oracle`
Expected: 2 new tests PASS; fixture still PASS.

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/oracle/ rtsim/src/data/mod.rs
git commit -m "feat(oracle): validation layer with density caps and class cooldowns"
```

---

### Task 6: Event engine rule — templates, transitions, effects, telemetry

**Files:**
- Create: `rtsim/src/data/oracle/templates.rs`, `rtsim/src/rule/oracle/event_engine.rs`
- Modify: `rtsim/src/data/oracle/mod.rs` (add `pub mod templates;`, `propose_from_template`)
- Modify: `rtsim/src/rule/oracle/mod.rs` (add `pub mod event_engine;`)
- Modify: `rtsim/src/lib.rs` (`start_default_rules` — register after `OracleWorldState`)

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/rule/oracle/event_engine.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::oracle::{OracleData, events::EventState, facts::RegionId};
    use common::{resources::TimeOfDay, rtsim::SiteId};

    const HOUR: f64 = 60.0 * 60.0;

    fn site(n: u64) -> SiteId { slotmap::KeyData::from_ffi(n).into() }

    #[test]
    fn festival_runs_full_lifecycle_and_cleans_up_its_facts() {
        let mut oracle = OracleData::default();
        assert!(
            oracle.propose_from_template("not_a_template", None, None, TimeOfDay(0.0)).is_none(),
            "unknown templates are refused at the door"
        );
        let id = oracle
            .propose_from_template("harvest_festival", Some(RegionId(0)), Some(site(1)), TimeOfDay(0.0))
            .expect("known template");
        step_events(&mut oracle, TimeOfDay(0.0)); // Proposed -> Validated
        assert!(matches!(oracle.events.get(id).unwrap().state, EventState::Validated));
        step_events(&mut oracle, TimeOfDay(0.0)); // Validated -> Scheduled (now + 1h)
        assert!(matches!(oracle.events.get(id).unwrap().state, EventState::Scheduled { .. }));
        step_events(&mut oracle, TimeOfDay(2.0 * HOUR)); // Scheduled -> Active
        assert!(matches!(oracle.events.get(id).unwrap().state, EventState::Active { stage: 1, .. }));
        assert_eq!(oracle.facts.len(), 1, "activation must assert the festival fact");
        step_events(&mut oracle, TimeOfDay(2.0 * HOUR + 25.0 * HOUR)); // past 1-day duration
        assert!(matches!(oracle.events.get(id).unwrap().state, EventState::Resolved { .. }));
        assert_eq!(oracle.facts.len(), 0, "resolution must retract asserted facts");
        assert_eq!(oracle.chronicle.validate_causal_chain(), Ok(()));
        assert!(oracle.chronicle.len() >= 5,
            "proposal + transitions + fact bookkeeping must all be chronicled");
    }

    #[test]
    fn density_cap_rejects_proposal_while_region_event_is_active() {
        let mut oracle = OracleData::default();
        oracle.propose_from_template("bandit_raid", Some(RegionId(0)), None, TimeOfDay(0.0)).unwrap();
        step_events(&mut oracle, TimeOfDay(0.0));
        step_events(&mut oracle, TimeOfDay(0.0));
        step_events(&mut oracle, TimeOfDay(2.0 * HOUR)); // raid Active
        let second = oracle
            .propose_from_template("dark_portent", Some(RegionId(0)), None, TimeOfDay(2.0 * HOUR))
            .unwrap();
        step_events(&mut oracle, TimeOfDay(2.0 * HOUR));
        assert!(matches!(oracle.events.get(second).unwrap().state, EventState::Rejected));
    }

    #[test]
    fn class_cooldown_blocks_back_to_back_same_class_events() {
        let mut oracle = OracleData::default();
        oracle.propose_from_template("bandit_raid", Some(RegionId(0)), None, TimeOfDay(0.0)).unwrap();
        for step in 0..4 {
            step_events(&mut oracle, TimeOfDay(step as f64 * 3.0 * 24.0 * HOUR));
        } // raid proposed -> ... -> Resolved (2-day duration)
        let again = oracle
            .propose_from_template("bandit_raid", Some(RegionId(0)), None, TimeOfDay(10.0 * 24.0 * HOUR))
            .unwrap();
        step_events(&mut oracle, TimeOfDay(10.0 * 24.0 * HOUR));
        assert!(matches!(oracle.events.get(again).unwrap().state, EventState::Rejected),
            "30-day class cooldown must reject a raid 10 days after the last one");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim event_engine`
Expected: FAIL to compile with "no method named `propose_from_template`" / "cannot find function `step_events`".

- [ ] **Step 3: Implement the template registry and proposal entry point**

Create `rtsim/src/data/oracle/templates.rs`:

```rust
use super::{events::EventClass, facts::{FestivalKind, OmenKind}};

/// What an Active event does to the fact store. Applied on activation; the
/// asserted `FactId`s are recorded on the event and retracted on resolution —
/// every effect carries its inverse.
#[derive(Copy, Clone, Debug)]
pub enum TemplateEffect {
    // Region-scoped (skipped if the event has no region)
    Plague { severity: f32 },
    Drought { severity: f32 },
    Omen { omen: OmenKind },
    // Site-scoped (skipped if the event has no site)
    FoodShortage { severity: f32 },
    Festival { kind: FestivalKind },
}

#[derive(Copy, Clone, Debug)]
pub struct EventTemplate {
    pub name: &'static str,
    pub class: EventClass,
    pub visible: bool,
    /// In-game seconds an Active stage lasts.
    pub duration: f64,
    pub effect: Option<TemplateEffect>,
}

const DAY: f64 = 60.0 * 60.0 * 24.0;

/// Built-in registry: one template per event class for Phase 2. Phase 6
/// externalizes these to `assets/common/oracle/events/*.ron` and fills out
/// the spec's two-per-class target.
const BUILTINS: &[EventTemplate] = &[
    EventTemplate { name: "bandit_raid", class: EventClass::Military, visible: true, duration: 2.0 * DAY, effect: None },
    EventTemplate { name: "succession_crisis", class: EventClass::Political, visible: true, duration: 5.0 * DAY, effect: None },
    EventTemplate { name: "famine", class: EventClass::Economic, visible: true, duration: 6.0 * DAY, effect: Some(TemplateEffect::FoodShortage { severity: 0.5 }) },
    EventTemplate { name: "drought", class: EventClass::Natural, visible: true, duration: 7.0 * DAY, effect: Some(TemplateEffect::Drought { severity: 0.6 }) },
    EventTemplate { name: "mana_storm", class: EventClass::Magical, visible: true, duration: 1.0 * DAY, effect: Some(TemplateEffect::Omen { omen: OmenKind::ManaStorm }) },
    EventTemplate { name: "dark_portent", class: EventClass::Religious, visible: true, duration: 3.0 * DAY, effect: Some(TemplateEffect::Omen { omen: OmenKind::BloodMoon }) },
    EventTemplate { name: "harvest_festival", class: EventClass::Festive, visible: true, duration: 1.0 * DAY, effect: Some(TemplateEffect::Festival { kind: FestivalKind::Harvest }) },
    EventTemplate { name: "plague_outbreak", class: EventClass::Ecological, visible: true, duration: 10.0 * DAY, effect: Some(TemplateEffect::Plague { severity: 0.5 }) },
];

pub fn builtin(name: &str) -> Option<EventTemplate> {
    BUILTINS.iter().find(|t| t.name == name).copied()
}
pub fn builtin_names() -> impl Iterator<Item = &'static str> { BUILTINS.iter().map(|t| t.name) }
```

Add `pub mod templates;` to `rtsim/src/data/oracle/mod.rs` and extend the `impl OracleData` block:

```rust
    /// Enqueue a Proposed event from a built-in template. The admin command
    /// and (in Phase 6) the LLM proposer both come through here — neither has
    /// any other write path into the event store. None = unknown template.
    pub fn propose_from_template(&mut self, template: &str, region: Option<RegionId>,
                                 site: Option<SiteId>, now: TimeOfDay) -> Option<EventId> {
        let t = templates::builtin(template)?;
        let mut subjects = Vec::new();
        if let Some(region) = region { subjects.push(Subject::Region(region)); }
        if let Some(site) = site { subjects.push(Subject::Site(site)); }
        let root = self.chronicle.append(
            now, ChronicleKind::EventProposed, Vec::new(), subjects, "oracle-event-proposed",
        );
        let id = self.events.insert(WorldEvent {
            class: t.class,
            template: t.name.to_string(),
            region,
            site,
            visible: t.visible,
            proposed_at: now,
            state: EventState::Proposed,
            asserted_facts: Vec::new(),
            chronicle_root: Some(root),
        });
        common::telemetry!("oracle_event", id = id.0, state = "Proposed", template = template);
        Some(id)
    }
```

- [ ] **Step 4: Implement the engine**

Above the tests in `rtsim/src/rule/oracle/event_engine.rs`:

```rust
use crate::{
    RtState, Rule, RuleError,
    data::oracle::{
        OracleData,
        chronicle::{ChronicleKind, Subject},
        events::{EventId, EventState},
        facts::WorldFact,
        templates::{self, TemplateEffect},
        validate,
    },
    event::{EventCtx, OnTick},
};
use common::resources::TimeOfDay;

/// Strided like the Architect (`ARCHITECT_TICK_SKIP`, `rtsim/src/rule/architect.rs:27`).
const EVENT_ENGINE_TICK_SKIP: u64 = 32;
/// Validated events start one in-game hour after scheduling. The Phase 2
/// scheduler is intentionally dumb; the tension-curve pacing director is a
/// Phase 6 deliverable.
const SCHEDULE_DELAY: f64 = 60.0 * 60.0;
const FALLBACK_DURATION: f64 = 60.0 * 60.0 * 24.0;

/// Transition authority for the event lifecycle: only this rule (via
/// `step_events`) mutates `EventState`. Everything else enqueues proposals.
pub struct OracleEventEngine;

impl Rule for OracleEventEngine {
    fn start(rtstate: &mut RtState) -> Result<Self, RuleError> {
        rtstate.bind::<Self, OnTick>(on_tick);
        Ok(Self)
    }
}

fn on_tick(ctx: EventCtx<OracleEventEngine, OnTick>) {
    if !ctx.event.tick.is_multiple_of(EVENT_ENGINE_TICK_SKIP) {
        return;
    }
    let data = &mut *ctx.state.data_mut();
    let now = data.time_of_day;
    step_events(&mut data.oracle, now);
}

/// Advance every non-terminal event by at most one lifecycle transition.
/// Pure over `OracleData` (unit-testable without an `RtState`); iteration is
/// id-ordered and therefore deterministic.
pub fn step_events(oracle: &mut OracleData, now: TimeOfDay) {
    let ids: Vec<EventId> = oracle.events.iter()
        .filter(|(_, e)| !e.state.is_terminal())
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        let (state, template) = {
            let e = oracle.events.get(id).expect("collected above");
            (e.state.clone(), e.template.clone())
        };
        match state {
            EventState::Proposed => {
                match validate::validate(oracle, oracle.events.get(id).expect("known id"), now) {
                    Ok(()) => transition(oracle, id, EventState::Validated, now,
                                         ChronicleKind::EventValidated),
                    Err(veto) => {
                        common::telemetry!("oracle_veto", id = id.0, at = "validate", veto = ?veto);
                        transition(oracle, id, EventState::Rejected, now,
                                   ChronicleKind::EventRejected);
                    },
                }
            },
            EventState::Validated => {
                let start_at = TimeOfDay(now.0 + SCHEDULE_DELAY);
                transition(oracle, id, EventState::Scheduled { start_at }, now,
                           ChronicleKind::EventScheduled);
            },
            EventState::Scheduled { start_at } if start_at.0 <= now.0 => {
                // Re-validate at trigger time: the world may have changed.
                match validate::validate(oracle, oracle.events.get(id).expect("known id"), now) {
                    Ok(()) => {
                        let duration = templates::builtin(&template)
                            .map_or(FALLBACK_DURATION, |t| t.duration);
                        let until = TimeOfDay(now.0 + duration);
                        transition(oracle, id, EventState::Active { stage: 1, until }, now,
                                   ChronicleKind::EventStarted);
                        let (region, class) = {
                            let e = oracle.events.get(id).expect("known id");
                            (e.region, e.class)
                        };
                        oracle.pacing.record_activation(region, class, now);
                        apply_activation_effects(oracle, id, now);
                    },
                    Err(veto) => {
                        common::telemetry!("oracle_veto", id = id.0, at = "trigger", veto = ?veto);
                        transition(oracle, id, EventState::Expired, now,
                                   ChronicleKind::EventExpired);
                    },
                }
            },
            EventState::Active { until, .. } if until.0 <= now.0 => {
                // Resolution pipeline runs atomically within one step.
                if let Some(e) = oracle.events.get_mut(id)
                    && e.transition(EventState::Resolving).is_ok()
                {
                    retract_effects(oracle, id, now);
                    transition(oracle, id, EventState::Resolved { at: now }, now,
                               ChronicleKind::EventResolved);
                }
            },
            // Scheduled in the future / Active still running / terminal.
            _ => {},
        }
    }
}

fn transition(oracle: &mut OracleData, id: EventId, to: EventState, now: TimeOfDay,
              kind: ChronicleKind) {
    let to_name = to.name();
    let (causes, subjects) = {
        let event = oracle.events.get_mut(id).expect("step_events only visits known ids");
        if let Err(err) = event.transition(to) {
            // Engine bug, not a data condition: log loudly, leave the event alone.
            tracing::error!(?err, event = id.0, "illegal ORACLE event transition");
            return;
        }
        let mut subjects = Vec::new();
        if let Some(region) = event.region { subjects.push(Subject::Region(region)); }
        if let Some(site) = event.site { subjects.push(Subject::Site(site)); }
        (event.chronicle_root.into_iter().collect::<Vec<_>>(), subjects)
    };
    let entry = oracle.chronicle.append(now, kind, causes, subjects, "oracle-event-transition");
    common::telemetry!("oracle_event", id = id.0, state = to_name, chronicle = entry);
}

fn apply_activation_effects(oracle: &mut OracleData, id: EventId, now: TimeOfDay) {
    let Some(event) = oracle.events.get(id) else { return };
    let Some(template) = templates::builtin(&event.template) else { return };
    let (region, site) = (event.region, event.site);
    let causes: Vec<u64> = event.chronicle_root.into_iter().collect();
    let fact = match template.effect {
        Some(TemplateEffect::Plague { severity }) =>
            region.map(|region| WorldFact::Plague { region, severity }),
        Some(TemplateEffect::Drought { severity }) =>
            region.map(|region| WorldFact::Drought { region, severity }),
        Some(TemplateEffect::Omen { omen }) =>
            region.map(|region| WorldFact::OmenSighted { region, omen }),
        Some(TemplateEffect::FoodShortage { severity }) =>
            site.map(|site| WorldFact::FoodShortage { site, severity }),
        Some(TemplateEffect::Festival { kind }) => site.map(|site| WorldFact::FestivalActive {
            site, kind, ends: TimeOfDay(now.0 + template.duration),
        }),
        None => None,
    };
    if let Some(fact) = fact {
        let fact_id = oracle.facts.assert(fact);
        oracle.events.get_mut(id).expect("known id").asserted_facts.push(fact_id);
        oracle.chronicle.append(now, ChronicleKind::FactAsserted, causes, Vec::new(),
                                "oracle-fact-asserted");
    }
}

fn retract_effects(oracle: &mut OracleData, id: EventId, now: TimeOfDay) {
    let (facts, causes) = match oracle.events.get_mut(id) {
        Some(e) => (std::mem::take(&mut e.asserted_facts),
                    e.chronicle_root.into_iter().collect::<Vec<_>>()),
        None => return,
    };
    for fact_id in facts {
        if oracle.facts.retract(fact_id).is_some() {
            oracle.chronicle.append(now, ChronicleKind::FactRetracted, causes.clone(),
                                    Vec::new(), "oracle-fact-retracted");
        }
    }
}
```

Add `pub mod event_engine;` to `rtsim/src/rule/oracle/mod.rs`. In `rtsim/src/lib.rs` `start_default_rules`, after the `OracleWorldState` registration:

```rust
        self.start_rule::<rule::oracle::event_engine::OracleEventEngine>();
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim event_engine` then `... cargo test -p veloren-rtsim`
Expected: 3 tests PASS; full crate PASS.
Note: if `density_cap_rejects_proposal_while_region_event_is_active` fails with the second event *Expired* instead of *Rejected*, you validated at the wrong lifecycle stage — `Rejected` is the Proposed-stage verdict, `Expired` is the trigger-time verdict. Fix the engine, not the test.

- [ ] **Step 6: Commit**

```bash
git add rtsim/src/data/oracle/ rtsim/src/rule/oracle/ rtsim/src/lib.rs
git commit -m "feat(oracle): event engine rule with lifecycle transitions, effects, telemetry"
```

---

### Task 7: `/oracle_event` admin command (manual event injection)

**Files:**
- Modify: `common/src/cmd.rs:422` (enum variant between `Object,` and `Outcome,` — the `verify_cmd_list_sorted` test at `common/src/cmd.rs:1597` enforces keyword-sorted enum order), `:834` area (`data()` arm next to `ServerChatCommand::Object`), `:1217` area (`keyword()` arm after `"object"`)
- Modify: `server/src/cmd.rs:229` (dispatch table, after the `RtsimChunk` line) and a new handler next to `handle_rtsim_purge` (`server/src/cmd.rs:2113`)
- Modify: `assets/voxygen/i18n/en/command.ftl:79` (description key, after `command-object-desc`)

- [ ] **Step 1: Add the command variant and metadata**

In `common/src/cmd.rs`, insert into the `ServerChatCommand` enum between `Object,` (line ~422) and `Outcome,`:

```rust
    OracleEvent,
```

In the `data()` match (next to the `ServerChatCommand::Object` arm at line ~834), add:

```rust
            ServerChatCommand::OracleEvent => cmd(
                vec![
                    Any("template", Required),
                    Integer("region index", 0, Optional),
                ],
                Content::localized("command-oracle_event-desc"),
                Some(Admin),
            ),
```

In the `keyword()` match (after `ServerChatCommand::Object => "object",` at line ~1217), add:

```rust
            ServerChatCommand::OracleEvent => "oracle_event",
```

In `assets/voxygen/i18n/en/command.ftl`, after `command-object-desc` (line 79), add:

```
command-oracle_event-desc = Propose an ORACLE world event from a template (it still passes validation)
```

- [ ] **Step 2: Verify the sorted-command invariant**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common cmd`
Expected: PASS, including `verify_cmd_list_sorted` ("object" < "oracle_event" < "outcome").

- [ ] **Step 3: Implement the server handler**

In `server/src/cmd.rs`, add to the dispatch table after `ServerChatCommand::RtsimChunk => handle_rtsim_chunk,` (line ~229):

```rust
        ServerChatCommand::OracleEvent => handle_oracle_event,
```

Then add the handler directly after `handle_rtsim_purge` (line ~2113), mirroring its resource-access pattern:

```rust
fn handle_oracle_event(
    server: &mut Server,
    client: EcsEntity,
    _target: EcsEntity,
    args: Vec<String>,
    action: &ServerChatCommand,
) -> CmdResult<()> {
    use crate::rtsim::RtSim;
    use rtsim::data::oracle::{facts::RegionId, templates};

    if let (Some(template), region) = parse_cmd_args!(args, String, u16) {
        // The command only *proposes*: the event engine remains the sole
        // transition authority, so injected events still pass validation
        // (density caps, cooldowns) like any other proposal.
        let proposed = {
            let rtsim = server.state.ecs().read_resource::<RtSim>();
            let mut data = rtsim.state().data_mut();
            let now = data.time_of_day;
            data.oracle.propose_from_template(&template, region.map(RegionId), None, now)
        };
        match proposed {
            Some(id) => {
                server.notify_client(
                    client,
                    ServerGeneral::server_msg(
                        ChatType::CommandInfo,
                        Content::Plain(format!(
                            "Proposed ORACLE event {} from template '{}' (validation pending)",
                            id.0, template,
                        )),
                    ),
                );
                Ok(())
            },
            None => Err(Content::Plain(format!(
                "Unknown ORACLE event template '{}'. Available: {}",
                template,
                templates::builtin_names().collect::<Vec<_>>().join(", "),
            ))),
        }
    } else {
        Err(action.help_content())
    }
}
```

- [ ] **Step 4: Verify build across affected crates**

Run: `cargo check -p veloren-common -p veloren-server -p veloren-voxygen`
Expected: clean. If voxygen reports a non-exhaustive match on `ServerChatCommand`, add the `OracleEvent` arm explicitly — do not add a wildcard.

- [ ] **Step 5: Smoke test in game**

Use the `veloren-run` skill to launch server + client with an admin account. In chat:
- `/oracle_event harvest_festival 0` → "Proposed ORACLE event 0 ... (validation pending)".
- `/oracle_event bogus` → error listing the eight built-in template names.
- Wait a few strides, then check telemetry (veloren-telemetry skill, logging-verbose build): `oracle_event` entries with `state` walking `Proposed → Validated → Scheduled → Active`, plus `oracle_chronicle` appends and the periodic `oracle_tick` heartbeat.
- Restart the server and confirm the rtsim load log shows no purge — the event survives the round trip (persistence proof for all Phase 1–2 fields).

- [ ] **Step 6: Commit**

```bash
git add common/src/cmd.rs server/src/cmd.rs assets/voxygen/i18n/en/command.ftl
git commit -m "feat(oracle): /oracle_event admin command for manual event injection"
```

---

## Phases 3–8 (contract-level — re-verify anchors when each phase starts)

The tasks below are contracts, not line-anchored diffs: file paths, signatures, and acceptance criteria are fixed, but every task **starts by re-verifying its anchors** — Phases 1–2 and parallel workstreams (difficulty zones, AURORA) will have shifted line numbers by the time these run. Each phase gets its own branch (`feature/oracle-phaseN`) off `development`, follows the same TDD discipline as Tasks 1–7 (failing test → implement → pass → commit), and ends with the Task 17 checklist.

### Task 8: AURORA interface contract — typed WorldFact read API + observation queue

**Files:** modify `rtsim/src/data/oracle/facts.rs` (query API); create `rtsim/src/data/oracle/observations.rs`; modify `rtsim/src/data/oracle/mod.rs` (`observations` field, `#[serde(skip)]` — transient between strides).

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "pub struct WorldFacts" rtsim/src/data/oracle/facts.rs
grep -n "Observation\|oracle.facts\|WorldFact" docs/superpowers/plans/2026-06-11-project-aurora.md | head -20
grep -rn "oracle.facts" rtsim/src server/src | head
```

Confirm the AURORA plan (`docs/superpowers/plans/2026-06-11-project-aurora.md`) still expects read-only fact access from NPC think-ticks plus a bounded observation submit queue (spec Section 2.3: "AURORA reads `OracleData.facts` read-only; submits `Observation`s to a bounded queue ORACLE drains — never direct writes"). If its contract drifted, reconcile **here first** — this task is the integration boundary.

- [ ] **Step 2: Typed read API** (TDD, tests in `facts.rs`):

```rust
impl WorldFacts {
    pub fn at_war(&self, a: FactionId, b: FactionId) -> bool;
    pub fn controlling_faction(&self, site: SiteId) -> Option<FactionId>;
    pub fn active_festival(&self, site: SiteId) -> Option<(FestivalKind, TimeOfDay)>;
    pub fn bounty_on(&self, actor: Actor) -> Option<u32>;
    /// All facts scoped to a region (Plague, Drought, OmenSighted, ...).
    pub fn region_facts(&self, region: RegionId) -> impl Iterator<Item = &WorldFact>;
}
```

- [ ] **Step 3: Observation queue** (TDD, tests in `observations.rs`):

```rust
pub enum ObservationKind { NpcDeed, PlayerConversation, SignificantTrade }
pub struct Observation { pub at: TimeOfDay, pub actor: Actor, pub kind: ObservationKind }
/// Bounded ring (cap 1024): submit drops the oldest when full and emits
/// telemetry!("oracle_obs_dropped", ...). Backed by a VecDeque.
pub struct ObservationQueue { /* ... */ }
impl ObservationQueue {
    pub fn submit(&mut self, obs: Observation);
    pub fn drain(&mut self) -> impl Iterator<Item = Observation> + '_;
}
```

`OracleWorldState::on_tick` drains the queue into chronicle entries each stride.

**Acceptance criteria:** AURORA-side code compiles against `&data.oracle.facts` only (no `&mut` leaks through the API); queue never exceeds its cap under a 10k-submit unit test; all five accessors unit-tested; `at_war(a,b) == at_war(b,a)`.

- [ ] **Step 4:** `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim oracle` PASS → commit `feat(oracle): AURORA interface — typed fact read API + bounded observation queue`

### Task 9 (Phase 3): Regions + ecosystem data model

**Files:** create `rtsim/src/data/oracle/ecosystem.rs`, `assets/common/oracle/predation.ron`; modify `rtsim/src/data/oracle/mod.rs` (`ecosystem` field, `#[serde(default)]`, fixture-test extension).

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "pub enum TrackedPopulation" rtsim/src/data/architect.rs
grep -n "temp\b\|humidity\|tree_density" world/src/sim/mod.rs | head
grep -n "CELL_SIZE" common/src/weather.rs
grep -n "fn wanted_population" rtsim/src/generate/mod.rs
```

- [ ] **Step 2: Implement** (TDD):
  - `RegionMap::derive(world: &World) -> RegionMap` — partitions the map into weather-cell-aligned regions (`common::weather::CELL_SIZE`); per region: aggregated biome profile (mean `SimChunk` temp/humidity/tree_density), `tension: f32`, adjacency list.
  - `Ecosystem { populations: BTreeMap<(RegionId, SpeciesGroup), f32>, carrying: BTreeMap<(RegionId, SpeciesGroup), f32>, drift: BTreeMap<(RegionId, SpeciesGroup), DriftProfile> }` where `SpeciesGroup` wraps `TrackedPopulation`.
  - `pub fn step_ecosystem(eco: &mut Ecosystem, dt_days: f64, predation: &PredationMatrix)` — logistic + Lotka-Volterra + pressure-driven migration (spec Section 3.1), hard clamps `N in [0.0, 1.2 * K]`.
  - `predation.ron`: sparse matrix `[(predator: "Wolf", prey: "Deer", rate: 0.08), ...]`, loaded via `common::assets` like entity configs.

**Acceptance criteria:** populations stay in `[0.2K, 1.2K]` over a 365-day simulated unit test; never negative; migration conserves total population; solver deterministic (no RNG); update is O(regions × species).

- [ ] **Step 3:** tests → commit `feat(oracle): regional ecosystem model with logistic/L-V dynamics`

### Task 10 (Phase 3): Ecosystem planner drives Architect spawns

**Files:** create `rtsim/src/rule/oracle/ecosystem.rs`; modify `rtsim/src/rule/migrate.rs` (the `wanted_population` recompute) and `rtsim/src/generate/mod.rs` (startup seed).

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "wanted_population" rtsim/src/rule/migrate.rs rtsim/src/generate/mod.rs rtsim/src/rule/architect.rs
grep -n "MIN_SPAWN_DELAY\|count_to_spawn" rtsim/src/rule/architect.rs
```

- [ ] **Step 2: Implement:** new rule `OracleEcosystem` (strided, once per in-game hour): runs `step_ecosystem`, then writes summed per-species targets into `data.architect.wanted_population` — Architect stays the *executor*, ORACLE becomes the *planner* (spec Section 3). Startup seeds `Ecosystem` carrying capacities from the same inputs `wanted_population()` uses; the legacy static computation remains the fallback for empty ecosystem state (old saves). Migration flows > 20% of a region's population call `propose_from_template("migration_wave", ...)` — a new invisible Ecological template.

**Acceptance criteria:** `architect.wanted_population.total()` within ±30% of the legacy value on a fresh world (regression test comparing both paths); old saves load and converge (fixture test); a forced drought (K halved) measurably reduces herbivore targets within 7 in-game days in a unit test.

- [ ] **Step 3:** tests → commit `feat(oracle): ecosystem planner writes Architect wanted_population`

### Task 11 (Phase 3): `VariantOverlay` spawn modifiers

**Files:** modify `common/src/generation.rs` (`EntityConfig` overlay application), `rtsim/src/rule/architect.rs` (spawn path applies overlays), `rtsim/src/data/oracle/ecosystem.rs` (`DriftProfile`, legendary records).

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "pub struct EntityConfig" common/src/generation.rs
grep -n "fn architect_tick" rtsim/src/rule/architect.rs
ls docs/superpowers/specs/2026-06-10-world-difficulty-zones-design.md
```

- [ ] **Step 2: Implement:**

```rust
pub struct VariantOverlay {
    pub name_suffix: Option<String>,
    pub health_mult: f32,   // Elite: 1.3..1.6
    pub damage_mult: f32,
    pub loot_mult: f32,
    /// Affix ability ids land with the magic-abilities plan; empty until then.
    pub affixes: Vec<String>,
}
pub fn roll_variant(region_tension: f32, drift: &DriftProfile, rng: &mut impl Rng) -> Option<VariantOverlay>
```

Elite chance 2–5% scaled by region tension; Regional variants apply `DriftProfile` stat biases (capped ±15%, reset on population collapse); Legendary records persist in `OracleData.ecosystem` keyed by name with kill/respawn history.

**Acceptance criteria:** stat multipliers respect the level bands in `docs/superpowers/specs/2026-06-10-world-difficulty-zones-design.md`; `roll_variant` deterministic under a seeded RNG; spawning a Legendary writes a chronicle entry and a rumor fact for AURORA.

- [ ] **Step 3:** tests → commit `feat(oracle): variant overlay system (elite/regional/legendary)`

### Task 12 (Phase 4): Climate states and anomaly events

**Files:** create `rtsim/src/data/oracle/climate.rs`; modify `server/src/weather/sim.rs` (anomaly modifiers), `rtsim/src/data/oracle/templates.rs` (`flood`, `heatwave`, `harsh_winter` join `drought`), `world/src/site/economy/context.rs` (shock inputs).

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "struct CellConsts\|pub fn add_zone" server/src/weather/sim.rs
grep -n "WeatherJob\|queue_zone" server/src/cmd.rs | head
ls world/src/site/economy/
```

- [ ] **Step 2: Implement:** `ClimateState { anomaly: Option<ClimateAnomaly>, crop_yield_mod: f32, humidity_mod: f32 }` per region in `OracleData`. Anomaly templates' Active stages set the region's `ClimateState`; a server-side bridge in `server/src/rtsim/tick.rs` reads climate states each weather tick and drives `WeatherSim::add_zone` over the region's cells; resolution clears state (inverse bookkeeping, as with facts). Consequence chain as the acceptance test: Active drought → `crop_yield_mod = 0.5` → engine asserts `WorldFact::FoodShortage` for the region's sites → economy context consumes the shock.

**Acceptance criteria:** no new network messages (the weather grid is already synced); anomaly lifecycle round-trips save/load; `/oracle_event drought <region>` visibly stops rain in a smoke test; economy shock input covered by a `veloren-world` unit test.

- [ ] **Step 3:** `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim -p veloren-server` → commit `feat(oracle): climate anomalies as lifecycle events driving the weather sim`

### Task 13 (Phase 5): Seasons, moon phases, `CelestialState` sync

**Files:** modify `common/src/calendar.rs` (in-game `Season` alongside real-date `CalendarEvent` at `common/src/calendar.rs:8`), `common/src/time.rs` (`MoonPhase` next to `MoonPeriod` at `:37`), `common/src/resources.rs` (`CelestialState` resource near `get_moon_dir` at `:27`); add a `common-net` sync message; shader uniforms in `assets/voxygen/shaders/include/sky.glsl` + `voxygen/src/render/pipelines/skybox.rs`.

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "pub enum CalendarEvent" common/src/calendar.rs
grep -n "MoonPeriod" common/src/time.rs; grep -n "get_moon_dir" common/src/resources.rs
grep -rn "Weather" common/net/src/msg/server.rs | head   # the grid-sync message to mirror
grep -n "moon" assets/voxygen/shaders/include/sky.glsl | head
```

- [ ] **Step 2: Implement:**

```rust
pub enum Season { Spring, Summer, Autumn, Winter }
pub fn season_at(tod: TimeOfDay) -> Season       // year = 96 in-game days (open question 1)
pub enum MoonPhase { New, WaxingCrescent, FirstQuarter, WaxingGibbous, Full, WaningGibbous, LastQuarter, WaningCrescent }
pub fn moon_phase_at(tod: TimeOfDay) -> MoonPhase // 8-in-game-day cycle
pub struct CelestialState { pub season: Season, pub moon_phase: MoonPhase, pub eclipse: Option<f32>, pub comet: Option<f32> }
```

Server-side couplings this task: full moon raises night-monster spawn weight in the ecosystem planner; season modulates carrying capacities and weather-sim humidity constants. Eclipses/comets are ORACLE events (full lifecycle) whose Active stage sets `CelestialState` fields. S3 terrain visuals are **explicitly out** (spec Section 5.1).

**Acceptance criteria:** `season_at`/`moon_phase_at` pure, total, unit-tested at cycle boundaries; client renders moon phase (shader uniform) and season color grading; protocol bump documented in the changelog.

- [ ] **Step 3:** tests (`-p veloren-common -p veloren-rtsim`) → commit `feat(oracle): seasons, moon phases, and CelestialState sync`

### Task 14 (Phase 6): Narrative director + LLM proposer thread

**Files:** create `rtsim/src/data/oracle/narrative.rs`, `rtsim/src/rule/oracle/narrative.rs`, `server/src/oracle/{mod,llm,validate}.rs`, `assets/common/oracle/canon.ron`, `assets/common/oracle/events/*.ron` (externalize Task 6's built-ins; fill out two templates per class).

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "save_thread\|get_or_insert_with" server/src/rtsim/mod.rs   # async-worker pattern to mirror
grep -n "propose_from_template" rtsim/src/data/oracle/mod.rs
grep -rn "deities\|canon" docs/superpowers/specs/2026-06-10-lore-cosmology-design.md | head
```

- [ ] **Step 2: Implement:**
  - `Arc { template: ArcTemplate, scope: ArcScope, beats: Vec<Beat>, state: BeatCursor }`; beats bind 1–3 event templates plus fact preconditions; the narrative rule advances a beat only when its events resolved and pacing allows.
  - `server/src/oracle/llm.rs`: worker thread + crossbeam channel **mirroring the rtsim save-thread pattern** — never on the tick path. `trait LlmBackend { fn propose(&self, digest: WorldDigest) -> Result<Vec<ProposalJson>, LlmError> }` with an HTTP impl and a `Disabled` impl (template-text fallback — the rule core never depends on the LLM).
  - `server/src/oracle/validate.rs`: schema-validates proposal JSON, then maps to `propose_from_template` calls — the LLM never gets a richer write path than the admin command.
  - `canon.ron`: deity names, dead characters, geographic invariants from the lore spec; rule-based consistency checker rejects pitches contradicting canon or chronicle facts.

**Acceptance criteria:** with `LlmBackend::Disabled` the full arc machinery runs on template text (CI-soakable, no network); malformed LLM output rejected with `telemetry!("oracle_llm_rejected", ...)`, never panics; canon checker unit-tested with deliberately contradictory pitches; beat advancement covered by pure-data tests like Task 6's.

- [ ] **Step 3:** tests → commit `feat(oracle): narrative director with arcs, canon checker, async LLM proposer`

### Task 15 (Phase 7): Player impact — deeds, fame/infamy, villains, legacy

**Files:** create `rtsim/src/data/oracle/players.rs`; modify `server/src/events/entity_manipulation.rs` and `server/src/events/trade.rs` (hooks beside the existing telemetry call sites); monument placement via `server/src/terrain_persistence.rs` callers.

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "fn handle_exp_gain\|fn handle_destroy" server/src/events/entity_manipulation.rs
grep -n "telemetry!" server/src/events/trade.rs
grep -n "pub fn set_block" server/src/terrain_persistence.rs
grep -n "pub enum SiteKind" world/src/site/mod.rs
```

- [ ] **Step 2: Implement:** `PlayerLedger { deeds: Vec<DeedRecord>, fame: BTreeMap<RegionId, f32>, infamy: BTreeMap<RegionId, f32> }` keyed by `CharacterId`, fed through the Task 8 observation queue from the server event handlers; fame/infamy decay per in-game day in the world-state rule. Villain thresholds: infamy > 0.5 → `WorldFact::BountyOn`; > 0.8 → regional `manhunt` event template; **one active nemesis per player** (validator invariant, spec Section 10). Legacy: world-first boss kills append a `PlayerDeed` chronicle entry and place a monument (≤ 200 blocks via `TerrainPersistence::set_block` — the one sanctioned runtime terrain edit). Dungeon invasions: an event template flips a dungeon site's spawn faction via Architect orders and writes `SiteControlled` facts — zero terrain work.

**Acceptance criteria:** fame/infamy decay unit-tested; bounty cap test stacks three bounties and gets one; monument plan asserted ≤ 200 blocks in a unit test; griefing-loop check — infamy earned inside an active `manhunt` cannot re-trigger a second manhunt (reuses the class-cooldown machinery).

- [ ] **Step 3:** tests (`-p veloren-rtsim -p veloren-server`) → commit `feat(oracle): player deed ledger, fame/infamy, villain pipeline, legacy monuments`

### Task 16 (Phase 8): Catch-up sim, compaction, soak harness

**Files:** modify `server/src/rtsim/mod.rs` (boot catch-up after the `OnSetup` emit), `rtsim/src/data/oracle/chronicle.rs` (compaction), `common/src/cmd.rs` + `server/src/cmd.rs` (`/oracle_fastforward`); create a soak script for CI.

- [ ] **Step 1: Re-verify anchors**

```bash
grep -n "OnSetup" server/src/rtsim/mod.rs
grep -n "day_cycle\|day_length" server/src/settings/mod.rs | head
ls common/frontend/src/   # bounded_writer.rs for chronicle archival
grep -n "verify_cmd_list_sorted" common/src/cmd.rs
```

- [ ] **Step 2: Implement:**
  - **Catch-up:** on boot compute real downtime, convert via the day-cycle coefficient, run coarse ORACLE-only steps (`step_events`, ecosystem, climate — no NPC pathing) at 1-in-game-hour resolution, capped at 7 in-game days; every entry flagged `ChronicleKind::SimulatedOffline`.
  - **`/oracle_fastforward <days>`:** admin command reusing the coarse stepper (keyword `oracle_fastforward` sorts between `oracle_event` and `outcome` — the sorted test enforces enum placement).
  - **Compaction:** Resolved events older than 60 in-game days collapse to their consequence facts + one summary chronicle entry; chronicle beyond 50k entries streams to JSONL beside the telemetry logs via `common/frontend/src/bounded_writer.rs`; `validate_causal_chain` must still pass post-compaction (exactly the corruption case the Task 2 test guards).
  - **Soak harness:** headless `veloren-server-cli` driving `/oracle_fastforward 365` nightly in CI; asserts zero invariant breaches, populations in `[0.2K, 1.2K]`, `validate_causal_chain() == Ok(())`, no panics. Dormant dungeon sites stay **deferred** until the difficulty-zones map regen is scheduled (spec Section 13 risk).

**Acceptance criteria:** catch-up of 30-day downtime completes < 10 s wall-clock and stops at the 7-day cap; fastforward deterministic for a fixed seed and start state; ORACLE stride p95 < 2 ms measured via tracy (`veloren-engine-perf` skill) at current world size.

- [ ] **Step 3:** soak run → commit `feat(oracle): downtime catch-up, chronicle compaction, soak harness`

---

### Task 17: Lint, format, changelog, and branch finish (run at the end of every phase branch)

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. Fix any warnings (no `#[allow]` without a justifying comment). Also run the voxygen publish-profile check from CLAUDE.md:

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
```

- [ ] **Step 2: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check.

- [ ] **Step 3: Full test suite**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim -p veloren-server -p veloren-common`
Expected: PASS.

- [ ] **Step 4: Update CHANGELOG and commit**

Add under the unreleased section of `CHANGELOG.md` (Phase 1–2 wording; adjust per phase):

```markdown
- PROJECT ORACLE phase 1-2: world-director event engine with typed world facts, causal chronicle, anti-chaos density caps, and the /oracle_event admin command.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for ORACLE world-director phase 1-2"
```

- [ ] **Step 5: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). Phases 3–8 each branch off `development` after the previous phase merges; re-verify this plan's contract anchors at that point (the grep commands in each task's Step 1).
