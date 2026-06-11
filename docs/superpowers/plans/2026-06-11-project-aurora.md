# PROJECT AURORA Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** NPCs become inhabitants: persistent minds (values/fears/alignment/mood), short/long-term memory with salience-based consolidation, and a typed durable social graph layered over the existing decaying `Sentiments` — then families, dynamic economy, organizations, generated quests, LLM color, and LOD optimization.

**Architecture:** AURORA extends rtsim in place. New persisted state hangs off `Npc`/`Site`/`Data` as `#[serde(default)]` fields (existing migration pattern, `rtsim/src/data/npc.rs:299`), so `CURRENT_VERSION` (10, `rtsim/src/data/mod.rs:37`) is **not** bumped and old `data.dat` files keep loading. Fast-moving affect stays in `Sentiments`; durable relationships are new typed edges that never decay stochastically. New rules register in `RtState::start_default_rules` (`rtsim/src/lib.rs:199-209`) and tick staggered-by-seed like `cleanup`. Every persisted store has a hard cap plus a serialized-size test; every new field gets an old-save fixture assertion.

**Tech Stack:** Rust nightly (2024 edition), `rmp-serde` MessagePack persistence, `enum-map` (serde feature) and `rand_chacha` (both already rtsim deps). Design spec: `docs/superpowers/specs/2026-06-10-project-aurora-design.md`. Crate: `veloren-rtsim` (verified package name in `rtsim/Cargo.toml`).

**Pre-verified baseline facts (2026-06-11, branch `development`):**
- The `Sentiments::cleanup` heap-order bug flagged in the spec is **already fixed**: `rtsim/src/data/sentiment.rs:101` wraps the key in `cmp::Reverse` (min-heap), regression test `cleanup_forgets_weakest_sentiments_first` at `sentiment.rs:224`. All tasks build on the fixed behavior (weakest sentiments forgotten first); do not re-fix.
- `Npc` has a **manual `Clone` impl** (`rtsim/src/data/npc.rs:341-366`). Every task adding an `Npc` field must extend it; the compiler enforces this.
- `Personality` fields are private (`common/src/rtsim.rs:92-98`); AURORA reads personality only via `Personality::is(PersonalityTrait)`.
- `arrayvec`/`arraydeque` are **not** workspace deps. Capped `Vec`/`VecDeque` with `const` caps (enforced + tested) are used instead.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim` (first build compiles `veloren-world`; slow once, then cached).
- Branch: create `feature/aurora-phase1` off `development` before Task 1.
- Invoke the `veloren-aurora` skill for context and `superpowers:test-driven-development` before writing code; `veloren-dev` for ECS patterns.
- Determinism: new simulation logic uses no RNG or `ChaChaRng` seeded from `npc.seed`/world state — never `rand::rng()` (the thread-seeded rng in `rule/cleanup.rs:21` is legacy; do not copy it into new rules).
- Every new persisted field: `#[serde(default)]`, an assertion in the pre-AURORA fixture test (Task 1), and a stated per-NPC persisted-bytes budget.
- Simulation LOD: Phase 1–2 stores update via event handlers (`rule/report.rs`) and staggered tick rules (`rule/cleanup.rs` pattern), which run identically for `SimulationMode::Simulated` and `Loaded` NPCs — uniform by construction. Phases needing divergent LOD (3, 4, 7, 8) say so explicitly.

---

## Phase 1 — Foundations (full TDD)

### Task 1: Branch + pre-AURORA save-compatibility fixture

**Files:**
- Create: `rtsim/tests/save_compat.rs` (integration test; rtsim's `[dependencies]` — `common`, `vek`, `rmp-serde` — are available to test targets), `rtsim/tests/fixtures/npc_pre_aurora.dat` (generated binary, committed)

- [ ] **Step 1: Create the branch**

```bash
git checkout development && git checkout -b feature/aurora-phase1
```

- [ ] **Step 2: Write the fixture generator and loader test**

Create `rtsim/tests/save_compat.rs`:

```rust
//! `npc_pre_aurora.dat` was serialized from `Npc` *before* any AURORA field
//! existed. Every task adding a `#[serde(default)]` field to `Npc` adds an
//! assertion to `pre_aurora_npc_still_loads` proving old bytes still load.
use common::{character::CharacterId, comp, rtsim::{Profession, Role}};
use vek::Vec3;
use veloren_rtsim::data::{Sentiment, npc::Npc};

fn fixture_npc() -> Npc {
    let mut npc = Npc::new(
        12345,
        Vec3::new(100.0, 200.0, 50.0),
        comp::Body::Humanoid(comp::humanoid::Body::random()),
        Role::Civilised(Some(Profession::Farmer)),
    );
    npc.sentiments.toward_mut(CharacterId(42)).change_by(0.5, 1.0);
    npc
}

/// Run once (ignored) to generate the fixture. NEVER re-run after AURORA
/// fields land — the point is that the bytes predate them.
#[test]
#[ignore]
fn generate_pre_aurora_fixture() {
    let bytes = rmp_serde::to_vec_named(&fixture_npc()).expect("serialize fixture npc");
    std::fs::create_dir_all("tests/fixtures").unwrap();
    std::fs::write("tests/fixtures/npc_pre_aurora.dat", bytes).unwrap();
}

#[test]
fn pre_aurora_npc_still_loads() {
    let npc: Npc = rmp_serde::from_slice(include_bytes!("fixtures/npc_pre_aurora.dat"))
        .expect("pre-AURORA Npc bytes must always deserialize");
    assert_eq!(npc.seed, 12345);
    assert!((npc.health_fraction - 1.0).abs() < f32::EPSILON);
    assert!(npc.sentiments.toward(CharacterId(42)).is(Sentiment::ALLY));
    // Task 2 adds: assert!(npc.mind.is_unseeded());
    // Task 4 adds: assert_eq!(npc.name, None);
    // Task 5 adds: assert_eq!(npc.memory.len(), 0);
    // Task 7 adds: assert_eq!(npc.relationships.len(), 0);
}
```

- [ ] **Step 3: Generate the fixture, then run the loader test**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim --test save_compat -- --ignored generate_pre_aurora_fixture`
Expected: PASS; `rtsim/tests/fixtures/npc_pre_aurora.dat` exists.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim --test save_compat pre_aurora_npc_still_loads`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rtsim/tests/save_compat.rs rtsim/tests/fixtures/npc_pre_aurora.dat
git commit -m "test: pre-AURORA Npc save-compat fixture harness"
```

---

### Task 2: `Mind` component — types, seeded generation, `Npc` wiring, migrate re-seed

**Files:**
- Create: `rtsim/src/data/mind.rs`
- Modify: `rtsim/src/data/mod.rs:1-19` (`pub mod mind;` + re-export `mind::Mind`), `rtsim/src/data/npc.rs:299-305` (field), `:341-366` (Clone), `:372-395` (`Npc::new`), `:400-403` (`with_personality`), `rtsim/src/rule/migrate.rs` (re-seed), `rtsim/tests/save_compat.rs`

**Persisted-bytes budget:** `Mind` ≤ 128 B/NPC (named msgpack, 1-char rename keys) — enforced by test. 10k NPCs ⇒ ≤ 1.28 MB.

- [ ] **Step 1: Write the failing tests**

Add `pub mod mind;` after `pub mod faction;` in `rtsim/src/data/mod.rs` and `mind::Mind` to its `pub use self::{...}`. Create `rtsim/src/data/mind.rs` containing ONLY this tests module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::rtsim::Personality;

    #[test]
    fn mind_seeding_is_deterministic_and_marks_seeded() {
        let p = Personality::default();
        assert_eq!(Mind::seeded(7, &p), Mind::seeded(7, &p));
        assert_ne!(Mind::seeded(7, &p), Mind::seeded(8, &p));
        assert!(Mind::default().is_unseeded());
        assert!(!Mind::seeded(7, &p).is_unseeded());
    }

    #[test]
    fn mood_decays_toward_zero_and_event_impacts_scale() {
        let mut mood = Mood { joy: 2, anger: -2, fear: 0, grief: 1, pride: -1, shame: 0 };
        mood.decay_step();
        assert_eq!((mood.joy, mood.anger, mood.grief, mood.pride), (1, -1, 0, 0));
        mood.decay_step();
        mood.decay_step();
        assert_eq!(mood, Mood::default());
        let (mut friend, mut stranger) = (Mood::default(), Mood::default());
        friend.on_witnessed_death(true, true);
        stranger.on_witnessed_death(false, true);
        assert!(friend.grief > stranger.grief && friend.anger > stranger.anger);
    }

    #[test]
    fn mind_fits_byte_budget() {
        let mut mind = Mind::seeded(99, &Personality::default());
        mind.goals = vec![Goal::GetRich { amount: 10_000 }; MAX_GOALS];
        let bytes = rmp_serde::to_vec_named(&mind).unwrap();
        assert!(bytes.len() <= 128, "Mind serialized to {} B (budget 128)", bytes.len());
    }
}
```

In `save_compat.rs`: add `assert!(npc.mind.is_unseeded());` to the fixture test, plus:

```rust
#[test]
fn new_npc_has_seeded_mind_consistent_with_personality() {
    use common::rtsim::Personality;
    let a = fixture_npc().with_personality(Personality::default());
    let b = fixture_npc().with_personality(Personality::default());
    assert!(!a.mind.is_unseeded());
    assert_eq!(a.mind, b.mind);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim mind`
Expected: FAIL to compile with "cannot find struct, variant or union type `Mind`" (and `Mood`, `Goal`).

- [ ] **Step 3: Implement the types**

Above the tests in `rtsim/src/data/mind.rs`:

```rust
use common::rtsim::{Actor, Personality, PersonalityTrait, SiteId};
use enum_map::{Enum, EnumMap, enum_map};
use rand::prelude::*;
use rand_chacha::ChaChaRng;
use serde::{Deserialize, Serialize};

/// Maximum concurrently-active long-term goals per NPC.
pub const MAX_GOALS: usize = 3;
/// Salt decorrelating mind RNG from existing `Npc::rng` permutations.
const MIND_SEED_SALT: u64 = 0x00A1_60_0A;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Enum, Serialize, Deserialize)]
pub enum Value { Tradition, Power, Wealth, Family, Faith, Freedom, Knowledge, Honor }

#[derive(Copy, Clone, Debug, PartialEq, Eq, Enum, Serialize, Deserialize)]
pub enum Fear { Violence, Poverty, Outsiders, Gods }

/// Two-axis morality: positive = lawful / selfless. Drifts from deeds.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alignment {
    #[serde(rename = "l")] pub lawful_chaotic: i8,
    #[serde(rename = "s")] pub selfless_selfish: i8,
}

/// Emotional state; decays stepwise toward zero via `decay_step`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mood {
    #[serde(rename = "j")] pub joy: i8,
    #[serde(rename = "a")] pub anger: i8,
    #[serde(rename = "f")] pub fear: i8,
    #[serde(rename = "g")] pub grief: i8,
    #[serde(rename = "p")] pub pride: i8,
    #[serde(rename = "h")] pub shame: i8,
}

impl Mood {
    /// One step toward zero per component; cleanup rule calls this at
    /// `MOOD_DECAY_TICK_SKIP` cadence (half rate for neurotic NPCs).
    pub fn decay_step(&mut self) {
        for c in [&mut self.joy, &mut self.anger, &mut self.fear,
                  &mut self.grief, &mut self.pride, &mut self.shame] {
            *c -= c.signum();
        }
    }

    pub fn on_witnessed_death(&mut self, victim_was_friend: bool, was_murder: bool) {
        if victim_was_friend { self.grief = self.grief.saturating_add(64); }
        if was_murder {
            self.fear = self.fear.saturating_add(32);
            self.anger = self.anger.saturating_add(if victim_was_friend { 64 } else { 16 });
        }
    }

    pub fn on_witnessed_theft(&mut self, at_home_site: bool) {
        self.anger = self.anger.saturating_add(if at_home_site { 48 } else { 16 });
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Goal {
    FindSpouse,
    GetRich { amount: u32 },
    AvengeDeath { of: Actor, against: Actor },
    SettleAt(SiteId),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mind {
    #[serde(rename = "v")] pub values: EnumMap<Value, u8>,
    #[serde(rename = "f")] pub fears: EnumMap<Fear, u8>,
    #[serde(rename = "a")] pub alignment: Alignment,
    #[serde(rename = "m")] pub mood: Mood,
    /// Hard-capped at [`MAX_GOALS`] by all writers.
    #[serde(rename = "g")] pub goals: Vec<Goal>,
}

impl Mind {
    /// Reproducible mind for seed + personality. Biases use only the public
    /// `Personality::is` API (fields are private).
    pub fn seeded(seed: u32, personality: &Personality) -> Self {
        let mut rng = ChaChaRng::seed_from_u64(seed as u64 ^ MIND_SEED_SALT);
        let mut values: EnumMap<Value, u8> = enum_map! { _ => rng.random_range(16..=112) };
        let mut fears: EnumMap<Fear, u8> = enum_map! { _ => rng.random_range(0..=64) };
        use PersonalityTrait::*;
        if personality.is(Open) {
            values[Value::Knowledge] = values[Value::Knowledge].saturating_add(64);
            values[Value::Freedom] = values[Value::Freedom].saturating_add(32);
        }
        if personality.is(Conscientious) {
            values[Value::Tradition] = values[Value::Tradition].saturating_add(48);
            values[Value::Honor] = values[Value::Honor].saturating_add(48);
        }
        if personality.is(Agreeable) {
            values[Value::Family] = values[Value::Family].saturating_add(64);
        }
        if personality.is(Neurotic) {
            for (_, f) in fears.iter_mut() { *f = f.saturating_add(48); }
        }
        let lawful: i16 = if personality.is(Conscientious) { 64 }
            else if personality.is(Unconscientious) { -64 } else { 0 };
        let selfless: i16 = if personality.is(Agreeable) { 48 }
            else if personality.is(Disagreeable) { -48 } else { 0 };
        Self {
            values,
            fears,
            alignment: Alignment {
                lawful_chaotic: (lawful + rng.random_range(-32..=32)).clamp(-127, 127) as i8,
                selfless_selfish: (selfless + rng.random_range(-32..=32)).clamp(-127, 127) as i8,
            },
            mood: Mood::default(),
            goals: Vec::new(),
        }
    }

    /// True for minds from `Default` (pre-AURORA saves via `serde(default)`).
    /// `seeded` cannot yield all-zero values (range starts at 16).
    pub fn is_unseeded(&self) -> bool { self.values.values().all(|v| *v == 0) }
}
```

- [ ] **Step 4: Wire `Npc` and the migrate rule**

In `rtsim/src/data/npc.rs`: add `mind::Mind` to the `crate::data` use block (line 3); after `sentiments` (line 302) add `#[serde(default)]\n    pub mind: Mind,`; Clone impl gains `mind: self.mind.clone(),`; `Npc::new` gains `mind: Mind::seeded(seed, &Personality::default()),`; the builder re-seeds so personality biases apply:

```rust
    pub fn with_personality(mut self, personality: Personality) -> Self {
        self.personality = personality;
        self.mind = Mind::seeded(self.seed, &personality);
        self
    }
```

In `rtsim/src/rule/migrate.rs`, at the end of the `OnSetup` closure body:

```rust
            // AURORA: pre-AURORA saves load a default (all-zero) mind;
            // re-seed deterministically from seed + personality.
            for (_, npc) in data.npcs.iter_mut() {
                if npc.mind.is_unseeded() {
                    npc.mind = crate::data::mind::Mind::seeded(npc.seed, &npc.personality);
                }
            }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim mind` — 3 PASS (if the budget test fails, shorten rename keys; do not raise the budget).
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim --test save_compat` — PASS (proves `serde(default)` against real pre-change bytes).
Run: `cargo check -p veloren-rtsim` — clean (Clone impl would error on a missed field).

- [ ] **Step 6: Commit**

```bash
git add rtsim/src/data/mind.rs rtsim/src/data/mod.rs rtsim/src/data/npc.rs rtsim/src/rule/migrate.rs rtsim/tests/save_compat.rs
git commit -m "feat(aurora): persisted Mind component with seeded generation and migrate re-seed"
```

---

### Task 3: Short-term memory + perception/mood wiring at event sources

**Files:**
- Create: `rtsim/src/data/memory.rs` (STM half; LTM lands in Task 5)
- Modify: `rtsim/src/data/mod.rs` (`pub mod memory;` + re-export `memory::{Perception, PerceptionKind, ShortTermMemory}`), `rtsim/src/data/npc.rs` (`#[serde(skip)] pub stm` + Clone), `rtsim/src/rule/report.rs:19-48,50-76` (feed perceptions + mood), `rtsim/src/rule/cleanup.rs:5-9,25-30` (mood decay cadence)

**Persisted-bytes budget:** 0 — STM is `#[serde(skip)]` by design; mood lives inside `Mind`'s budget.
**Anchoring decision (verified):** the spec's suggestion to feed STM at the inbox take (`rule/npc_ai/mod.rs:135`) is infeasible without restructuring borrows — `data.npcs.iter_mut()` holds `data` mutably, blocking the `data.reports` read needed to interpret `NpcInput::Report`. Perceptions are pushed **at the source** in `rule/report.rs`, where the `ReportKind` is in hand. Dialogue perceptions follow in Phase 2.

- [ ] **Step 1: Write the failing tests**

Declare `pub mod memory;` in `data/mod.rs`, then create `rtsim/src/data/memory.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::{character::CharacterId, resources::TimeOfDay, rtsim::Actor};

    fn spoke(with: u64, at: f64) -> Perception {
        Perception {
            kind: PerceptionKind::Spoke { with: Actor::Character(CharacterId(with)) },
            at: TimeOfDay(at),
            valence: 10,
        }
    }

    #[test]
    fn stm_is_a_bounded_ring_and_salience_ranks_events() {
        let mut stm = ShortTermMemory::default();
        for i in 0..(STM_CAP as u64 + 4) {
            stm.push(spoke(i, i as f64));
        }
        assert_eq!(stm.len(), STM_CAP);
        // Oldest evicted: first remaining is perception #4
        assert!(matches!(
            stm.iter().next().unwrap().kind,
            PerceptionKind::Spoke { with: Actor::Character(CharacterId(4)) }
        ));
        let murder = Perception {
            kind: PerceptionKind::WitnessedDeath {
                victim: Actor::Character(CharacterId(1)),
                killer: Some(Actor::Character(CharacterId(2))),
            },
            at: TimeOfDay(0.0),
            valence: -100,
        };
        assert!(murder.salience() > spoke(1, 0.0).salience());
        assert!(spoke(1, 0.0).salience() < SALIENCE_RECORD_THRESHOLD);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim memory`
Expected: FAIL to compile ("cannot find type `Perception`" etc.).

- [ ] **Step 3: Implement STM**

Above the tests:

```rust
use common::{resources::TimeOfDay, rtsim::Actor};
use std::collections::VecDeque;

/// Ring capacity for unpersisted short-term perceptions.
pub const STM_CAP: usize = 16;
/// Minimum salience for a perception to consolidate into LTM (Task 5).
pub const SALIENCE_RECORD_THRESHOLD: u8 = 32;

#[derive(Copy, Clone, Debug)]
pub enum PerceptionKind {
    Spoke { with: Actor },
    WitnessedDeath { victim: Actor, killer: Option<Actor> },
    WitnessedTheft { thief: Actor, at_home_site: bool },
    Helped { by: Actor },
}

#[derive(Copy, Clone, Debug)]
pub struct Perception {
    pub kind: PerceptionKind,
    pub at: TimeOfDay,
    /// Emotional valence for the perceiver: -127 (awful) ..= 127 (great).
    pub valence: i8,
}

impl Perception {
    /// Salience = kind base + |valence|/2, saturating. Deterministic.
    pub fn salience(&self) -> u8 {
        let base: u8 = match self.kind {
            PerceptionKind::Spoke { .. } => 8,
            PerceptionKind::WitnessedTheft { .. } => 40,
            PerceptionKind::Helped { .. } => 72,
            PerceptionKind::WitnessedDeath { killer: None, .. } => 56,
            PerceptionKind::WitnessedDeath { killer: Some(_), .. } => 88,
        };
        base.saturating_add(self.valence.unsigned_abs() / 2)
    }
}

/// Unpersisted (`serde(skip)` at the `Npc` field) recent-perception ring.
#[derive(Clone, Debug, Default)]
pub struct ShortTermMemory {
    buf: VecDeque<Perception>,
}

impl ShortTermMemory {
    pub fn push(&mut self, p: Perception) {
        if self.buf.len() >= STM_CAP { self.buf.pop_front(); }
        self.buf.push_back(p);
    }

    pub fn len(&self) -> usize { self.buf.len() }

    pub fn is_empty(&self) -> bool { self.buf.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = &Perception> { self.buf.iter() }

    pub fn drain(&mut self) -> impl Iterator<Item = Perception> + '_ { self.buf.drain(..) }
}
```

In `rtsim/src/data/npc.rs`: add to the **unpersisted** block after `inbox` (~line 316): `#[serde(skip)]\n    pub stm: ShortTermMemory,`; Clone impl `stm: Default::default(),` (session-local, like `inbox`); `Npc::new` likewise.

- [ ] **Step 4: Wire perceptions + mood in the report rule, decay in cleanup**

In `rtsim/src/rule/report.rs`, extend imports to `use crate::data::{Report, Sentiment, memory::{Perception, PerceptionKind}, report::ReportKind};` and copy `let at = data.time_of_day;` before each witness loop. The `on_death` loop (lines ~41-46) becomes:

```rust
            for npc_id in nearby {
                if let Some(npc) = data.npcs.get_mut(npc_id) {
                    npc.inbox.push_back(NpcInput::Report(report));
                    let victim_was_friend =
                        npc.sentiments.toward(ctx.event.actor).is(Sentiment::FRIEND);
                    npc.mind.mood
                        .on_witnessed_death(victim_was_friend, ctx.event.killer.is_some());
                    npc.stm.push(Perception {
                        kind: PerceptionKind::WitnessedDeath {
                            victim: ctx.event.actor,
                            killer: ctx.event.killer,
                        },
                        at,
                        valence: if victim_was_friend { -100 } else { -25 },
                    });
                }
            }
```

The `on_theft` loop (lines ~69-74) gains, after its `push_back`:

```rust
                    let at_home_site = ctx.event.site.is_some() && npc.home == ctx.event.site;
                    npc.mind.mood.on_witnessed_theft(at_home_site);
                    npc.stm.push(Perception {
                        kind: PerceptionKind::WitnessedTheft {
                            thief: ctx.event.actor,
                            at_home_site,
                        },
                        at,
                        valence: if at_home_site { -60 } else { -15 },
                    });
```

In `rtsim/src/rule/cleanup.rs`: add `use common::rtsim::PersonalityTrait;`, a const next to lines 6-9:

```rust
/// Mood steps one unit toward baseline every ~10 s (30 TPS); neurotic NPCs at
/// half rate — full-scale emotion fades in ~21 min vs ~42 min.
const MOOD_DECAY_TICK_SKIP: u64 = 300;
```

and after the sentiment-decay `for_each` (line 30):

```rust
            // Decay NPC moods toward baseline (deterministic, no RNG)
            data.npcs
                .iter_mut()
                .filter(|(_, npc)| {
                    let skip = if npc.personality.is(PersonalityTrait::Neurotic) {
                        MOOD_DECAY_TICK_SKIP * 2
                    } else {
                        MOOD_DECAY_TICK_SKIP
                    };
                    (npc.seed as u64 + ctx.event.tick).is_multiple_of(skip)
                })
                .for_each(|(_, npc)| npc.mind.mood.decay_step());
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim memory` — PASS.
Run: `cargo check -p veloren-rtsim` — clean. Run `--test save_compat` — PASS (STM is skipped; fixture untouched).
Run: `grep -n "stm.push\|on_witnessed\|MOOD_DECAY_TICK_SKIP" rtsim/src/rule/report.rs rtsim/src/rule/cleanup.rs` — Expected ≥ 5 hits (wiring landed; rule closures need a full `RtState`/`World`, so wiring is verified by compile + this audit).

- [ ] **Step 6: Commit**

```bash
git add rtsim/src/data/memory.rs rtsim/src/data/mod.rs rtsim/src/data/npc.rs rtsim/src/rule/report.rs rtsim/src/rule/cleanup.rs
git commit -m "feat(aurora): short-term perception memory + mood wiring from witnessed events"
```

---

### Task 4: Persisted NPC names

**Files:**
- Modify: `rtsim/src/data/npc.rs` (field + Clone + `Npc::new` + builder; `get_name` at `:431-439`), `rtsim/tests/save_compat.rs`

**Persisted-bytes budget:** 0 B for existing NPCs (`None` ≈ 1 B); ≤ 24 B when set (newborns in Phase 3, renames). Resolves the TODO at `npc.rs:431`.

- [ ] **Step 1: Write the failing tests**

In `pre_aurora_npc_still_loads`: `assert_eq!(npc.name, None);`. Add:

```rust
#[test]
fn persisted_name_overrides_seed_generated_name() {
    let generated = fixture_npc().get_name().expect("humanoids have generated names");
    let named = fixture_npc().with_name("Aldric Thornwood");
    assert_eq!(named.get_name().as_deref(), Some("Aldric Thornwood"));
    // Same seed without a persisted name keeps the deterministic generated one
    assert_eq!(fixture_npc().get_name(), Some(generated));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim --test save_compat`
Expected: FAIL to compile ("no field `name`" / "no method named `with_name`").

- [ ] **Step 3: Implement**

After the `mind` field in `npc.rs`:

```rust
    /// Persisted display name; `None` falls back to the seed-generated name.
    /// Set for newborns (Phase 3) so family names survive.
    #[serde(default)]
    pub name: Option<String>,
```

Clone impl: `name: self.name.clone(),`. `Npc::new`: `name: None,`. Builder next to `with_home`:

```rust
    // TODO: have a dedicated `NpcBuilder` type for this.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
```

Replace `get_name` (lines 431-439):

```rust
    pub fn get_name(&self) -> Option<String> {
        if let Some(name) = &self.name {
            Some(name.clone())
        } else if let comp::Body::Humanoid(_) = &self.body {
            Some(name::generate_npc(&mut self.rng(Self::PERM_NAME)))
        } else {
            None
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim --test save_compat` — PASS. `cargo check -p veloren-rtsim` — clean.

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/npc.rs rtsim/tests/save_compat.rs
git commit -m "feat(aurora): persisted NPC names with seed-generated fallback"
```

---

### Task 5: Long-term episodic memory with salience consolidation

**Files:**
- Modify: `rtsim/src/data/memory.rs` (LTM + `consolidate`), `rtsim/src/data/npc.rs` (field + Clone + `Npc::cleanup` at `:456-467`), `rtsim/src/rule/cleanup.rs:56-60` (pass `time_of_day`), `rtsim/tests/save_compat.rs`

**Persisted-bytes budget:** ≤ 1024 B/NPC at the 24-episode cap (enforced; the spec's 672 B assumed tighter `Actor` encoding than `write_named` produces). 10k NPCs worst case ≤ 10 MB.
**Determinism:** no RNG — pure salience thresholds; daily forgetting gates on a persisted `last_decay` timestamp, so it is save/load- and cadence-independent.

- [ ] **Step 1: Write the failing tests**

Add to `rtsim/src/data/memory.rs` tests:

```rust
    fn murder(victim: u64, at: f64) -> Perception {
        Perception {
            kind: PerceptionKind::WitnessedDeath {
                victim: Actor::Character(CharacterId(victim)),
                killer: Some(Actor::Character(CharacterId(99))),
            },
            at: TimeOfDay(at),
            valence: -100,
        }
    }

    #[test]
    fn consolidation_keeps_salient_drops_mundane_dedupes_and_caps() {
        let (mut stm, mut ltm) = (ShortTermMemory::default(), LongTermMemory::default());
        stm.push(spoke(1, 0.0)); // below threshold
        stm.push(murder(7, 0.0));
        stm.push(murder(7, 1.0)); // same (kind, actors): refresh, not duplicate
        consolidate(&mut stm, &mut ltm, TimeOfDay(1.0));
        assert!(stm.is_empty());
        assert_eq!(ltm.len(), 1);
        assert!((ltm.iter().next().unwrap().at.0 - 1.0).abs() < f64::EPSILON);
        for i in 0..(LTM_CAP as u64 + 5) {
            stm.push(murder(100 + i, i as f64));
            consolidate(&mut stm, &mut ltm, TimeOfDay(i as f64));
        }
        assert_eq!(ltm.len(), LTM_CAP);
        assert!(ltm.involving(Actor::Character(CharacterId(99))).next().is_some());
    }

    #[test]
    fn salience_decays_daily_and_forgotten_episodes_are_evicted() {
        const DAY: f64 = 60.0 * 60.0 * 24.0;
        let (mut stm, mut ltm) = (ShortTermMemory::default(), LongTermMemory::default());
        let mut weak = spoke(1, 0.0);
        weak.valence = 90; // salience 8 + 45 = 53, just above threshold
        stm.push(weak);
        stm.push(murder(2, 0.0));
        consolidate(&mut stm, &mut ltm, TimeOfDay(0.0));
        let start = ltm.len();
        for day in 1..=40 {
            consolidate(&mut stm, &mut ltm, TimeOfDay(day as f64 * DAY));
        }
        assert!(ltm.len() < start, "low-salience episodes must be forgotten");
        assert!(ltm.iter().all(|e| e.salience > 0));
    }

    #[test]
    fn full_ltm_fits_byte_budget() {
        let (mut stm, mut ltm) = (ShortTermMemory::default(), LongTermMemory::default());
        for i in 0..LTM_CAP as u64 {
            stm.push(murder(i, i as f64));
            consolidate(&mut stm, &mut ltm, TimeOfDay(i as f64));
        }
        let bytes = rmp_serde::to_vec_named(&ltm).unwrap();
        assert!(bytes.len() <= 1024, "LTM serialized to {} B (budget 1024)", bytes.len());
    }
```

And in `pre_aurora_npc_still_loads`: `assert_eq!(npc.memory.len(), 0);`

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim memory`
Expected: FAIL to compile ("cannot find `LongTermMemory`", "`consolidate`", "`LTM_CAP`").

- [ ] **Step 3: Implement LTM + consolidation**

Add to `rtsim/src/data/memory.rs` (`use serde::{Deserialize, Serialize};` joins imports):

```rust
/// Hard cap on persisted episodes per NPC.
pub const LTM_CAP: usize = 24;
/// Salience lost per in-game day (rehearsal refreshes it).
pub const SALIENCE_DAILY_DECAY: u8 = 2;
const DAY_SECS: f64 = 60.0 * 60.0 * 24.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EpisodeKind { Spoke, WitnessedDeath, Theft, Helped }

impl From<PerceptionKind> for EpisodeKind {
    fn from(kind: PerceptionKind) -> Self {
        match kind {
            PerceptionKind::Spoke { .. } => Self::Spoke,
            PerceptionKind::WitnessedDeath { .. } => Self::WitnessedDeath,
            PerceptionKind::WitnessedTheft { .. } => Self::Theft,
            PerceptionKind::Helped { .. } => Self::Helped,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Episode {
    #[serde(rename = "k")] pub kind: EpisodeKind,
    /// Up to two involved actors (e.g. victim + killer), my perspective.
    #[serde(rename = "a")] pub actors: [Option<Actor>; 2],
    #[serde(rename = "t")] pub at: TimeOfDay,
    #[serde(rename = "s")] pub salience: u8,
    #[serde(rename = "v")] pub valence: i8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LongTermMemory {
    #[serde(rename = "e")] episodes: Vec<Episode>,
    /// Last daily-decay time; persisted so forgetting is deterministic
    /// across save/load and independent of tick cadence.
    #[serde(rename = "d", default)] last_decay: TimeOfDay,
}

impl LongTermMemory {
    pub fn len(&self) -> usize { self.episodes.len() }

    pub fn is_empty(&self) -> bool { self.episodes.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = &Episode> { self.episodes.iter() }

    pub fn involving(&self, actor: Actor) -> impl Iterator<Item = &Episode> {
        self.episodes.iter().filter(move |e| e.actors.contains(&Some(actor)))
    }

    /// Same (kind, actors) refreshes salience/timestamp (rehearsal); else
    /// insert, evicting the weakest episode if full and strictly weaker.
    pub fn record(&mut self, ep: Episode) {
        if let Some(existing) = self.episodes.iter_mut()
            .find(|e| e.kind == ep.kind && e.actors == ep.actors)
        {
            existing.salience = existing.salience.max(ep.salience);
            existing.at = ep.at;
        } else if self.episodes.len() < LTM_CAP {
            self.episodes.push(ep);
        } else if let Some((idx, weakest)) = self.episodes.iter().enumerate()
            .min_by_key(|(_, e)| e.salience)
            .map(|(i, e)| (i, e.salience))
            && weakest < ep.salience
        {
            self.episodes[idx] = ep;
        }
    }

    fn decay(&mut self, now: TimeOfDay) {
        let elapsed_days = ((now.0 - self.last_decay.0) / DAY_SECS) as u32;
        if elapsed_days > 0 {
            let loss = (elapsed_days.min(255) as u8).saturating_mul(SALIENCE_DAILY_DECAY);
            for e in &mut self.episodes {
                e.salience = e.salience.saturating_sub(loss);
            }
            self.episodes.retain(|e| e.salience > 0);
            self.last_decay = now;
        }
    }
}

/// Drain STM into LTM (salience-gated), then apply daily forgetting.
/// Deterministic: no RNG. Called from `Npc::cleanup`.
pub fn consolidate(stm: &mut ShortTermMemory, ltm: &mut LongTermMemory, now: TimeOfDay) {
    for p in stm.drain() {
        let salience = p.salience();
        if salience >= SALIENCE_RECORD_THRESHOLD {
            let actors = match p.kind {
                PerceptionKind::Spoke { with } => [Some(with), None],
                PerceptionKind::WitnessedDeath { victim, killer } => [Some(victim), killer],
                PerceptionKind::WitnessedTheft { thief, .. } => [Some(thief), None],
                PerceptionKind::Helped { by } => [Some(by), None],
            };
            ltm.record(Episode { kind: p.kind.into(), actors, at: p.at, salience, valence: p.valence });
        }
    }
    ltm.decay(now);
}
```

In `npc.rs`: add `#[serde(default)] pub memory: LongTermMemory,` after `name`; Clone `memory: self.memory.clone(),`; `Npc::new` default; add `TimeOfDay` to the `common::resources` import (line 12). Replace `Npc::cleanup` (lines 456-467):

```rust
    pub fn cleanup(&mut self, reports: &Reports, time_of_day: TimeOfDay) {
        self.sentiments
            .cleanup(crate::data::sentiment::NPC_MAX_SENTIMENTS);
        self.known_reports
            .retain(|report| reports.contains_key(*report));
        // Consolidate perceptions into episodic memory; forget stale episodes
        crate::data::memory::consolidate(&mut self.stm, &mut self.memory, time_of_day);
    }
```

In `rtsim/src/rule/cleanup.rs`, the entity-cleanup pass (lines 56-60) becomes:

```rust
            // Clean up entities
            let time_of_day = data.time_of_day;
            data.npcs
                .iter_mut()
                .filter(|(_, npc)| (npc.seed as u64 + ctx.event.tick).is_multiple_of(NPC_CLEANUP_TICK_SKIP))
                .for_each(|(_, npc)| npc.cleanup(&data.reports, time_of_day));
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim memory` — 4 tests PASS (Tasks 3+5).
Run: `--test save_compat` — PASS. `cargo check -p veloren-rtsim` — clean (catches the `cleanup` call-site change).

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/memory.rs rtsim/src/data/npc.rs rtsim/src/rule/cleanup.rs rtsim/tests/save_compat.rs rtsim/src/data/mod.rs
git commit -m "feat(aurora): long-term episodic memory with salience consolidation and forgetting"
```

---

### Task 6: Phase 1 gate — budget enforcement, sentiment re-cap, lint, changelog

**Files:**
- Modify: `rtsim/src/data/sentiment.rs:13` (`NPC_MAX_SENTIMENTS` 128 → 64; typed edges take over durable cases in Phase 2, per spec), `rtsim/tests/save_compat.rs`, `CHANGELOG.md`

- [ ] **Step 1: Write the whole-NPC budget test**

Add to `rtsim/tests/save_compat.rs`:

```rust
#[test]
fn fully_loaded_npc_fits_byte_ceiling() {
    use common::resources::TimeOfDay;
    use veloren_rtsim::data::memory::{LTM_CAP, Perception, PerceptionKind, consolidate};
    use veloren_rtsim::data::mind::{Goal, MAX_GOALS};

    let mut npc = fixture_npc().with_name("Aldric Thornwood");
    npc.mind.goals = vec![Goal::GetRich { amount: 10_000 }; MAX_GOALS];
    for i in 0..64u64 {
        npc.sentiments.toward_mut(CharacterId(i)).change_by(0.9, 1.0);
    }
    for i in 0..LTM_CAP as u64 {
        npc.stm.push(Perception {
            kind: PerceptionKind::WitnessedDeath {
                victim: common::rtsim::Actor::Character(CharacterId(i)),
                killer: Some(common::rtsim::Actor::Character(CharacterId(1000 + i))),
            },
            at: TimeOfDay(i as f64),
            valence: -100,
        });
        consolidate(&mut npc.stm, &mut npc.memory, TimeOfDay(i as f64));
    }
    let bytes = rmp_serde::to_vec_named(&npc).unwrap();
    // Absolute ceiling for an all-caps-full NPC. The 2 KB p95 (spec §Memory
    // size budget) is a *runtime-typical* target tracked in soak telemetry;
    // this test guards the hard ceiling.
    assert!(bytes.len() <= 4096, "capped NPC serialized to {} B (ceiling 4096)", bytes.len());
    println!("capped NPC persisted size: {} B", bytes.len());
}
```

Deliberate divergence from the spec's arithmetic: under `write_named`, `Target`/`Actor` enum keys cost more than the spec's 12–20 B/entry estimate, so enforcement = component caps (Tasks 2/5/7) + this 4 KB ceiling, with 2 KB p95 as the runtime target.

- [ ] **Step 2: Run, then re-cap sentiments**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim --test save_compat fully_loaded`
Expected: PASS. If the ceiling is exceeded, print the size and stop for review — do not raise the ceiling.
Then change `rtsim/src/data/sentiment.rs:13` to `pub const NPC_MAX_SENTIMENTS: usize = 64;`.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim` — all PASS (the existing cleanup test is cap-agnostic).

- [ ] **Step 3: CI-identical lint and format**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
cargo fmt --all -- --check
```
Expected: clean. Fix warnings without `#[allow]` unless justified by a comment.

- [ ] **Step 4: Changelog, commit, finish the phase slice**

Add under the unreleased section of `CHANGELOG.md`:
```markdown
- AURORA Phase 1: rtsim NPCs now have persistent minds (values, fears, alignment, mood), episodic memory of salient events, and persisted names.
```

```bash
git add rtsim/src/data/sentiment.rs rtsim/tests/save_compat.rs CHANGELOG.md
git commit -m "feat(aurora): phase 1 budget gate — sentiment re-cap and NPC byte ceiling"
```

Invoke `superpowers:finishing-a-development-branch` (run `veloren-review` first). Phase 2 continues on this branch if kept open, or on `feature/aurora-phase2` after merge — record the decision in the merge/PR description.

---

## Phase 2 — Social Graph (full TDD)

### Task 7: Typed relationship edges on `Npc`

**Files:**
- Create: `rtsim/src/data/relationship.rs`
- Modify: `rtsim/src/data/mod.rs` (module + re-export `relationship::{Edge, EdgeKind, Relationships}`), `rtsim/src/data/npc.rs` (field + Clone + `Npc::new`), `rtsim/tests/save_compat.rs`

**Persisted-bytes budget:** ≤ 768 B/NPC at the 16-edge cap (enforced; spec's 320 B assumed tighter encoding — see Task 6 note). Edges are durable: **no stochastic decay** — they change only via social-rule consolidation, betrayal/death events, or integrity sweeps.

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/data/relationship.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::{character::CharacterId, resources::TimeOfDay, rtsim::Actor};

    fn actor(i: u64) -> Actor { Actor::Character(CharacterId(i)) }

    #[test]
    fn upsert_accumulates_strength_caps_edges_and_protects_structural() {
        let mut rel = Relationships::default();
        let now = TimeOfDay(0.0);
        assert!(rel.upsert(actor(1), EdgeKind::Friendship, 8, now));
        assert!(rel.upsert(actor(1), EdgeKind::Friendship, 8, now));
        assert_eq!(rel.get(actor(1), EdgeKind::Friendship).unwrap().strength, 16);
        rel.upsert(actor(2), EdgeKind::Marriage, 1, now);
        rel.upsert(actor(3), EdgeKind::Kinship(KinRole::Child), 1, now);
        for i in 10..(10 + MAX_RELATIONSHIPS as u64 + 5) {
            rel.upsert(actor(i), EdgeKind::Friendship, 100, now);
        }
        assert_eq!(rel.len(), MAX_RELATIONSHIPS);
        // Structural edges survive cap pressure from strong friendships
        assert!(rel.get(actor(2), EdgeKind::Marriage).is_some());
        assert!(rel.get(actor(3), EdgeKind::Kinship(KinRole::Child)).is_some());
    }

    #[test]
    fn full_relationships_fit_byte_budget() {
        let mut rel = Relationships::default();
        for i in 0..MAX_RELATIONSHIPS as u64 {
            rel.upsert(actor(i), EdgeKind::Friendship, 100, TimeOfDay(1.0e7));
        }
        let bytes = rmp_serde::to_vec_named(&rel).unwrap();
        assert!(bytes.len() <= 768, "Relationships serialized to {} B (budget 768)", bytes.len());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim relationship`
Expected: FAIL to compile ("cannot find `Relationships`").

- [ ] **Step 3: Implement**

```rust
use common::{resources::TimeOfDay, rtsim::Actor};
use serde::{Deserialize, Serialize};

/// Hard cap on typed edges per NPC.
pub const MAX_RELATIONSHIPS: usize = 16;
/// Strength at/above which an edge counts as durable.
pub const EDGE_DURABLE: i8 = 32;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KinRole { Parent, Child, Sibling }

/// `Professional` arrives in Phase 4; `OrgPeer` in Phase 5 (additive variants).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind { Kinship(KinRole), Friendship, Rivalry, Romance, Marriage }

impl EdgeKind {
    /// Structural edges are exempt from cap-pressure eviction and fading.
    pub fn is_structural(&self) -> bool {
        matches!(self, EdgeKind::Kinship(_) | EdgeKind::Marriage)
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    #[serde(rename = "k")] pub kind: EdgeKind,
    #[serde(rename = "s")] pub strength: i8,
    #[serde(rename = "t")] pub since: TimeOfDay,
}

/// Ego-centric adjacency list: every hot query ("my spouse", "a rival here?")
/// is an O(16) scan; serializes inside the per-NPC payload for free.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Relationships {
    #[serde(rename = "r")] edges: Vec<(Actor, Edge)>,
}

impl Relationships {
    pub fn len(&self) -> usize { self.edges.len() }

    pub fn iter(&self) -> impl Iterator<Item = &(Actor, Edge)> { self.edges.iter() }

    pub fn get(&self, with: Actor, kind: EdgeKind) -> Option<&Edge> {
        self.edges.iter().find(|(a, e)| *a == with && e.kind == kind).map(|(_, e)| e)
    }

    /// Strengthen-or-create. Negative `delta` weakens; non-structural edges
    /// at/below zero strength are removed (structural floor at 1). Returns
    /// false if the cap blocked insertion.
    pub fn upsert(&mut self, with: Actor, kind: EdgeKind, delta: i8, now: TimeOfDay) -> bool {
        if let Some((_, e)) = self.edges.iter_mut().find(|(a, e)| *a == with && e.kind == kind) {
            e.strength = e.strength.saturating_add(delta);
            if e.strength <= 0 {
                if kind.is_structural() {
                    e.strength = 1;
                } else {
                    self.edges.retain(|(a, e)| !(*a == with && e.kind == kind));
                }
            }
            true
        } else if delta > 0 {
            if self.edges.len() >= MAX_RELATIONSHIPS {
                // Evict the weakest non-structural edge if strictly weaker
                let weakest = self.edges.iter().enumerate()
                    .filter(|(_, (_, e))| !e.kind.is_structural())
                    .min_by_key(|(_, (_, e))| e.strength)
                    .map(|(i, (_, e))| (i, e.strength));
                match weakest {
                    Some((idx, s)) if s < delta => { self.edges.remove(idx); },
                    _ => return false,
                }
            }
            self.edges.push((with, Edge { kind, strength: delta, since: now }));
            true
        } else {
            false
        }
    }

    pub fn retain(&mut self, f: impl FnMut(&(Actor, Edge)) -> bool) { self.edges.retain(f); }
}
```

In `npc.rs`: `#[serde(default)] pub relationships: Relationships,` after `memory`; Clone `relationships: self.relationships.clone(),`; `Npc::new` default. In `save_compat.rs` fixture test: `assert_eq!(npc.relationships.len(), 0);`

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim relationship` — 2 PASS. Run `--test save_compat` — PASS.

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/relationship.rs rtsim/src/data/mod.rs rtsim/src/data/npc.rs rtsim/tests/save_compat.rs
git commit -m "feat(aurora): typed durable relationship edges with capped ego-adjacency"
```

---

### Task 8: Sentiment introspection + reputation query

**Files:**
- Modify: `rtsim/src/data/sentiment.rs` (`Sentiment::value` private→pub at `:155`; new `Sentiments::iter`; `reputation_of` free fn + tests), `rtsim/src/data/mod.rs` (`Data::reputation_at_site`)

**Persisted-bytes budget:** 0 — reputation is computed on demand, never stored (spec §Morality: single source of truth).

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` in `sentiment.rs`:

```rust
    #[test]
    fn reputation_is_mean_sentiment_among_population() {
        let target = Target::Character(CharacterId(7));
        let mut a = Sentiments::default();
        a.toward_mut(target).change_by(1.0, 1.0);
        let mut b = Sentiments::default();
        b.toward_mut(target).change_by(-1.0, 1.0);
        let c = Sentiments::default();
        assert!(reputation_of(target, [&a, &b, &c]).abs() < 0.01);
        assert!(reputation_of(target, [&a, &c]) > 0.4);
        assert_eq!(reputation_of(target, []), 0.0);
        assert_eq!(a.iter().count(), 1);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim sentiment`
Expected: FAIL to compile ("cannot find function `reputation_of`", "no method named `iter`").

- [ ] **Step 3: Implement**

In `sentiment.rs`: change `fn value(&self) -> f32` (line 155) to `pub fn value(&self) -> f32`. Add to `impl Sentiments` after `cleanup`:

```rust
    /// Iterate over all non-default sentiment entries.
    pub fn iter(&self) -> impl Iterator<Item = (&Target, &Sentiment)> { self.map.iter() }
```

Free function above the `Sentiment` struct:

```rust
/// Aggregate reputation of `target` among observers' sentiments: mean value
/// in [-1, 1], 0.0 for an empty sample. Never stored — always derived.
pub fn reputation_of<'a>(
    target: impl Into<Target>,
    among: impl IntoIterator<Item = &'a Sentiments>,
) -> f32 {
    let target = target.into();
    let (sum, n) = among.into_iter()
        .fold((0.0f32, 0u32), |(sum, n), s| (sum + s.toward(target).value(), n + 1));
    if n == 0 { 0.0 } else { sum / n as f32 }
}
```

In `rtsim/src/data/mod.rs`, inside `impl Data` (after `prepare`; add `sentiment`/`SiteId` imports as needed):

```rust
    /// Reputation of `target` at a site, sampled over up to `sample`
    /// residents (sorted by NPC uid — `population` is a HashSet, so sorting
    /// keeps the sample deterministic).
    pub fn reputation_at_site(
        &self,
        target: impl Into<sentiment::Target>,
        site: SiteId,
        sample: usize,
    ) -> f32 {
        let target = target.into();
        let Some(site) = self.sites.get(site) else { return 0.0 };
        let mut residents: Vec<&Npc> =
            site.population.iter().filter_map(|id| self.npcs.get(*id)).collect();
        residents.sort_unstable_by_key(|npc| npc.uid);
        sentiment::reputation_of(target, residents.iter().take(sample).map(|n| &n.sentiments))
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim sentiment` — existing + new PASS. `cargo check -p veloren-rtsim` — clean.

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/data/sentiment.rs rtsim/src/data/mod.rs
git commit -m "feat(aurora): derived reputation queries over sentiments"
```

---

### Task 9: `social` rule — edge consolidation, symmetry, integrity

**Files:**
- Create: `rtsim/src/rule/social.rs`
- Modify: `rtsim/src/rule/mod.rs:1-8` (`pub mod social;`), `rtsim/src/lib.rs:201-208` (register between `SimulateNpcs` and `NpcAi`, so brains see fresh edges)

**Determinism:** the consolidation pass uses **no RNG** — pure sentiment/episode thresholds; identical state ⇒ identical edges (tested). **LOD:** runs for all NPCs at stagger regardless of `SimulationMode`.
**Spec mapping:** "sustained sentiment ≥ FRIEND for an in-game week" is realized as accumulation: each qualifying pass adds +8 strength; durable at `EDGE_DURABLE` (32) after 4 consecutive passes; non-qualifying young (non-durable, non-structural) edges decay −4/pass and vanish at 0 — flings fade, sustained affect consolidates.

- [ ] **Step 1: Write the failing tests**

Create `rtsim/src/rule/social.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Sentiments, memory::LongTermMemory, relationship::*};
    use common::{character::CharacterId, resources::TimeOfDay, rtsim::Actor};

    fn actor(i: u64) -> Actor { Actor::Character(CharacterId(i)) }

    #[test]
    fn sustained_friendship_consolidates_deterministically_with_mirror_ops() {
        let mut sentiments = Sentiments::default();
        sentiments.toward_mut(actor(1)).change_by(0.7, 1.0); // ≥ FRIEND
        let memory = LongTermMemory::default();
        let (mut rel_a, mut rel_b) = (Relationships::default(), Relationships::default());
        for pass in 0..4 {
            let ops_a = consolidate_edges(&sentiments, &memory, &mut rel_a, TimeOfDay(pass as f64));
            let ops_b = consolidate_edges(&sentiments, &memory, &mut rel_b, TimeOfDay(pass as f64));
            assert_eq!(ops_a, ops_b, "social consolidation must be deterministic");
            assert!(ops_a.iter().any(|(a, k, d)| *a == actor(1) && *k == EdgeKind::Friendship && *d > 0));
        }
        assert!(rel_a.get(actor(1), EdgeKind::Friendship).unwrap().strength >= EDGE_DURABLE);
    }

    #[test]
    fn rivalry_requires_a_grievance_episode() {
        let mut sentiments = Sentiments::default();
        sentiments.toward_mut(actor(2)).change_by(-0.5, 1.0); // ≤ RIVAL
        let mut rel = Relationships::default();
        consolidate_edges(&sentiments, &LongTermMemory::default(), &mut rel, TimeOfDay(0.0));
        assert!(rel.get(actor(2), EdgeKind::Rivalry).is_none());
    }

    #[test]
    fn unsustained_edges_fade() {
        let sentiments = Sentiments::default(); // affect decayed to neutral
        let mut rel = Relationships::default();
        rel.upsert(actor(3), EdgeKind::Friendship, 8, TimeOfDay(0.0)); // young edge
        for pass in 0..3 {
            consolidate_edges(&sentiments, &LongTermMemory::default(), &mut rel, TimeOfDay(pass as f64));
        }
        assert!(rel.get(actor(3), EdgeKind::Friendship).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim social`
Expected: FAIL to compile ("cannot find function `consolidate_edges`"). Add `pub mod social;` to `rtsim/src/rule/mod.rs` first.

- [ ] **Step 3: Implement the rule**

```rust
use crate::{
    RtState, Rule, RuleError,
    data::{
        Sentiment, Sentiments,
        memory::LongTermMemory,
        relationship::{EDGE_DURABLE, EdgeKind, Relationships},
        sentiment::Target,
    },
    event::OnTick,
};
use common::{resources::TimeOfDay, rtsim::Actor};
use hashbrown::HashSet;

/// Per-NPC social pass every 100 ticks (~3.3 s at 30 TPS), staggered by seed
/// — same pattern as cleanup's NPC_CLEANUP_TICK_SKIP.
const SOCIAL_TICK_SKIP: u64 = 100;
/// Dead-edge integrity sweep cadence per NPC (~100 s at 30 TPS).
const INTEGRITY_TICK_SKIP: u64 = 3000;
const EDGE_GAIN: i8 = 8;
const EDGE_FADE: i8 = -4;

/// One social pass over a single NPC's affective state. Pure and RNG-free.
/// Returns mirror ops `(other, kind, delta)` the caller applies to the other
/// endpoint to maintain edge symmetry.
pub(crate) fn consolidate_edges(
    sentiments: &Sentiments,
    memory: &LongTermMemory,
    relationships: &mut Relationships,
    now: TimeOfDay,
) -> Vec<(Actor, EdgeKind, i8)> {
    let mut ops = Vec::new();
    let mut sustained: Vec<(Actor, EdgeKind)> = Vec::new();
    for (target, s) in sentiments.iter() {
        let actor = match target {
            Target::Character(c) => Actor::Character(*c),
            Target::Npc(n) => Actor::Npc(*n),
            Target::Faction(_) => continue,
        };
        if s.is(Sentiment::FRIEND) {
            relationships.upsert(actor, EdgeKind::Friendship, EDGE_GAIN, now);
            sustained.push((actor, EdgeKind::Friendship));
            ops.push((actor, EdgeKind::Friendship, EDGE_GAIN));
        } else if s.is(Sentiment::RIVAL) && memory.involving(actor).any(|e| e.valence < 0) {
            relationships.upsert(actor, EdgeKind::Rivalry, EDGE_GAIN, now);
            sustained.push((actor, EdgeKind::Rivalry));
            ops.push((actor, EdgeKind::Rivalry, EDGE_GAIN));
        }
    }
    // Young, unsustained, non-structural edges fade (durable ones never do)
    let fading: Vec<(Actor, EdgeKind)> = relationships.iter()
        .filter(|(a, e)| {
            !e.kind.is_structural()
                && e.strength < EDGE_DURABLE
                && !sustained.contains(&(*a, e.kind))
        })
        .map(|(a, e)| (*a, e.kind))
        .collect();
    for (a, k) in fading {
        relationships.upsert(a, k, EDGE_FADE, now);
    }
    ops
}

pub struct Social;

impl Rule for Social {
    fn start(rtstate: &mut RtState) -> Result<Self, RuleError> {
        rtstate.bind::<Self, OnTick>(|ctx| {
            let data = &mut *ctx.state.data_mut();
            let now = data.time_of_day;
            // Pass 1: per-NPC consolidation, collecting mirror ops
            let mut mirror_ops = Vec::new();
            for (npc_id, npc) in data.npcs.iter_mut().filter(|(_, npc)| {
                !npc.is_dead()
                    && (npc.seed as u64 + ctx.event.tick).is_multiple_of(SOCIAL_TICK_SKIP)
            }) {
                for op in consolidate_edges(&npc.sentiments, &npc.memory, &mut npc.relationships, now) {
                    mirror_ops.push((Actor::Npc(npc_id), op));
                }
            }
            // Pass 2: symmetry — apply mirror ops to NPC endpoints
            for (from, (to, kind, delta)) in mirror_ops {
                if let Actor::Npc(to_npc) = to
                    && let Some(other) = data.npcs.get_mut(to_npc)
                {
                    other.relationships.upsert(from, kind, delta, now);
                }
            }
            // Pass 3 (staggered): repair dangling edges of dead/removed NPCs
            // (mirrors `known_reports.retain` in `Npc::cleanup`)
            let live: HashSet<_> = data.npcs.iter().map(|(id, _)| id).collect();
            data.npcs
                .iter_mut()
                .filter(|(_, npc)| {
                    (npc.seed as u64 + ctx.event.tick).is_multiple_of(INTEGRITY_TICK_SKIP)
                })
                .for_each(|(_, npc)| {
                    npc.relationships.retain(|(actor, _)| match actor {
                        Actor::Npc(id) => live.contains(id),
                        Actor::Character(_) => true,
                    });
                });
        });
        Ok(Self)
    }
}
```

(Borrow note for pass 3: `live` is collected via an immutable borrow that ends before `iter_mut`; if this trips the borrow checker, collect `live` at the top of the closure.)

Register in `rtsim/src/lib.rs` `start_default_rules`, between the `SimulateNpcs` and `NpcAi` lines:

```rust
        self.start_rule::<rule::social::Social>();
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim social` — 3 PASS.
Run: `cargo check -p veloren-rtsim` — clean (registration compiles).

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/rule/social.rs rtsim/src/rule/mod.rs rtsim/src/lib.rs
git commit -m "feat(aurora): social rule consolidating sentiments+episodes into symmetric durable edges"
```

---

### Task 10: Memory-aware dialogue — NPCs reference shared episodes

**Files:**
- Modify: `rtsim/src/rule/npc_ai/dialogue.rs:5-36` (`general` gains a "reminisce" response) and near `:446` (new `reminisce` fn beside `sentiments`), `assets/voxygen/i18n/en/dialogue.ftl` (new keys)

- [ ] **Step 1: Write the failing test**

At the end of `rtsim/src/rule/npc_ai/dialogue.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::memory::{Episode, EpisodeKind};
    use common::resources::TimeOfDay;

    fn ep(kind: EpisodeKind, valence: i8) -> Episode {
        Episode { kind, actors: [None, None], at: TimeOfDay(0.0), salience: 50, valence }
    }

    #[test]
    fn reminisce_key_covers_all_episode_kinds() {
        assert_eq!(reminisce_key(&ep(EpisodeKind::Helped, 80)), "npc-dialogue-reminisce_helped");
        assert_eq!(reminisce_key(&ep(EpisodeKind::WitnessedDeath, -90)), "npc-dialogue-reminisce_death");
        assert_eq!(reminisce_key(&ep(EpisodeKind::Theft, -50)), "npc-dialogue-reminisce_theft");
        assert_eq!(reminisce_key(&ep(EpisodeKind::Spoke, 20)), "npc-dialogue-reminisce_good");
        assert_eq!(reminisce_key(&ep(EpisodeKind::Spoke, -20)), "npc-dialogue-reminisce_bad");
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim reminisce`
Expected: FAIL to compile ("cannot find function `reminisce_key`").

- [ ] **Step 2: Implement**

Beside the `sentiments` dialogue fn (~line 446):

```rust
fn reminisce_key(ep: &crate::data::memory::Episode) -> &'static str {
    use crate::data::memory::EpisodeKind;
    match (ep.kind, ep.valence >= 0) {
        (EpisodeKind::Helped, _) => "npc-dialogue-reminisce_helped",
        (EpisodeKind::WitnessedDeath, _) => "npc-dialogue-reminisce_death",
        (EpisodeKind::Theft, _) => "npc-dialogue-reminisce_theft",
        (_, true) => "npc-dialogue-reminisce_good",
        (_, false) => "npc-dialogue-reminisce_bad",
    }
}

fn reminisce<S: State>(tgt: Actor, session: DialogueSession) -> impl Action<S> {
    now(move |ctx, _| {
        // Highest-salience episode involving my interlocutor. Read-only:
        // `ctx.npc` is `&Npc`; rehearsal-refresh happens in consolidation
        // (`LongTermMemory::record`), not here.
        match ctx.npc.memory.involving(tgt).max_by_key(|e| e.salience) {
            Some(ep) => session.say_statement(Content::localized(reminisce_key(ep))).boxed(),
            None => session
                .say_statement(Content::localized("npc-dialogue-reminisce_nothing"))
                .boxed(),
        }
    })
}
```

In `general` (after the job `match` block ending ~line 36):

```rust
        // AURORA: offer to reminisce when we share a memorable episode
        if ctx.npc.memory.involving(tgt).next().is_some() {
            responses.push((
                Response::from(Content::localized("dialogue-question-reminisce")),
                reminisce(tgt, session).boxed(),
            ));
        }
```

Add to `assets/voxygen/i18n/en/dialogue.ftl` (match the file's section style):

```ftl
dialogue-question-reminisce = Do you remember when we last met?
npc-dialogue-reminisce_helped = I haven't forgotten the kindness you showed me. Few would have done the same.
npc-dialogue-reminisce_death = I still see it when I close my eyes... that death. Dark times.
npc-dialogue-reminisce_theft = I remember the thieving that went on around here. Keep your hands where I can see them.
npc-dialogue-reminisce_good = Aye, good memories. It's nice to see a familiar face.
npc-dialogue-reminisce_bad = I remember... though I'd rather not speak of it.
npc-dialogue-reminisce_nothing = Hmm, can't say anything springs to mind.
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim reminisce` — PASS. `cargo check -p veloren-rtsim` — clean.

- [ ] **Step 4: In-game verification**

Use the `veloren-run` skill: spawn near a village, cause a witnessed death near an NPC (e.g. kill a wild-animal NPC in view), wait ≥ ~4 s (`NPC_CLEANUP_TICK_SKIP` consolidation), talk to the witness — the "Do you remember…" option appears and the reply matches the episode; a fresh NPC does **not** show the option. (With the logging-verbose build, `veloren-telemetry` confirms the branch.)

- [ ] **Step 5: Commit**

```bash
git add rtsim/src/rule/npc_ai/dialogue.rs assets/voxygen/i18n/en/dialogue.ftl
git commit -m "feat(aurora): NPCs reference remembered episodes in dialogue"
```

---

### Task 11: Phase 2 gate — full suite, lint, changelog, finish

- [ ] **Step 1: Full test suite**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim -p veloren-common`
Expected: PASS, including the save-compat fixture with all phase-1/2 assertions.

- [ ] **Step 2: CI-identical lint + voxygen publish profile + format**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
cargo fmt --all -- --check
```
Expected: clean.

- [ ] **Step 3: Changelog + commit + finish the branch**

Add under the unreleased section of `CHANGELOG.md`:
```markdown
- AURORA Phase 2: NPCs form durable friendships and rivalries from sustained sentiment and remembered events, and reference shared memories in dialogue.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for AURORA phase 2 social graph"
```

Invoke `superpowers:finishing-a-development-branch` (run `veloren-review` before merging into `development`). This is the spec's first shippable milestone: *NPCs visibly remember players*.

---

## Phases 3–8 (contract-level — re-verify anchors when each phase starts)

Concrete contracts (files, signatures, tests, acceptance) but **not** line-anchored: Phases 1–2 will have moved the files. Each task's first step is the mandatory anchor re-verification grep. Universal constraints carried forward: every persisted field is `#[serde(default)]` + a `save_compat.rs` fixture assertion (generate per-phase fixtures for `Site`/`Data` exactly as Task 1 did for `Npc`); every store states and tests a byte budget; new rules use seeded `ChaChaRng` or no RNG; new behavior defines loaded-vs-simulated semantics.

### Phase 3 — Families (XL)

**Task 3.1: Birth time, age, life stages**
- Re-verify: `grep -n "pub struct Npc {" -A 30 rtsim/src/data/npc.rs` ; `grep -n "wanted_population\|MIN_SPAWN_DELAY" rtsim/src/rule/architect.rs rtsim/src/data/architect.rs` ; `grep -n "pub struct WorldSettings" -A 6 common/src/rtsim.rs`
- Files: `rtsim/src/data/npc.rs` (`#[serde(default)] pub birth_tod: Option<TimeOfDay>` — `None` = pre-AURORA adult); `common/src/rtsim.rs` (`WorldSettings` gains `pub year_secs: f64`, default `18.0 * 3600.0` per spec, server-tunable).
- Interfaces: `impl Npc { pub fn age_years(&self, now: TimeOfDay, year_secs: f64) -> Option<f32>; pub fn life_stage(&self, now: TimeOfDay, year_secs: f64) -> LifeStage }`; `pub enum LifeStage { Child, Adult, Elder }` (Child < 16y, Elder ≥ 60y; `None` ⇒ Adult).
- Tests: stage-boundary unit tests; fixture asserts `birth_tod == None`. Budget ≤ 10 B/NPC. Acceptance: existing saves load; all NPCs report Adult until births occur.

**Task 3.2: `lifecycle` rule — births and old-age death**
- Re-verify: `grep -n "start_rule" rtsim/src/lib.rs` ; `grep -n "OnDeath" rtsim/src/event.rs` ; `grep -n "Npc::new" rtsim/src/rule/architect.rs`
- Files: create `rtsim/src/rule/lifecycle.rs` (register after `social`, before `npc_ai`); extend `rtsim/src/event.rs` with `pub struct OnBirth { pub child: NpcId, pub parents: [NpcId; 2], pub site: Option<SiteId> }` (`type SystemData<'a> = ();`).
- Interfaces: per-NPC daily stagger (`LIFECYCLE_TICK_SKIP` from day length / 30 TPS); `fn try_birth(data: &mut Data, parents: (NpcId, NpcId), rng: &mut ChaChaRng) -> Option<NpcId>` gated on a Marriage edge + home-site population below the `wanted_population` ceiling; old-age death probability ramps after Elder, emitting the **existing** `OnDeath` (no second death path). RNG: `ChaChaRng::seed_from_u64(npc.seed as u64 ^ day_index)`.
- LOD: identical simulated/loaded (per-day rates); loaded NPCs additionally announce via `NpcAction::Say` (presentation only).
- Tests: deterministic decisions for fixed seed+day; population ≤ ceiling over 1000 simulated days on a synthetic site. Acceptance: soak (`veloren-telemetry` `"life"` channel) shows stable population, no architect double-spawn.

**Task 3.3: Genetics and kinship edges**
- Re-verify: `grep -n "fn distributed\|pub fn random" common/src/rtsim.rs` ; `grep -n "with_personality\|Mind::seeded" rtsim/src/data/npc.rs`
- Files: `common/src/rtsim.rs` (`impl Personality { pub fn blend(a: &Self, b: &Self, rng: &mut impl RngExt) -> Self }` — per-trait mean ± `distributed` jitter; lives in `common` because fields are private); `rtsim/src/rule/lifecycle.rs` (`fn child_of(a: &Npc, b: &Npc, seed: u32, now: TimeOfDay) -> Npc` — body-param blend + jitter, `Mind::seeded` then value-inheritance at half strength, surname from a parent via persisted `name`, `birth_tod = Some(now)`); kinship via `EdgeKind::Kinship` upserts on child/parents/siblings (structural — never evicted, Task 7 guarantee).
- Tests: same seed+parents ⇒ identical child; kinship symmetric; structural exemption holds at the 16-edge cap. Acceptance: 3-generation families by soak day 60 (spec metric).

**Task 3.4: Architect ceiling refactor**
- Re-verify: `grep -n "wanted_population\|fn on_death\|Npc::new" rtsim/src/rule/architect.rs` ; `grep -n "TrackedPopulation" rtsim/src/data/architect.rs`
- Files: `rtsim/src/rule/architect.rs` — for `Role::Civilised`, architect respawn becomes a **floor** repair (force-spawn only below 50% of wanted); births are the normal replenishment. `Wild`/`Monster` unchanged.
- Tests: spawn-decision function: no spawn at 100%/75%, spawn at 40%. Acceptance: no unrelated replacement NPCs for civilised deaths in soak; recovery from a simulated 60% cull.

**Task 3.5: Coins, deeds, inheritance**
- Re-verify: `grep -n "ItemResource" common/src/rtsim.rs` ; `grep -n "pub struct Site {" -A 20 rtsim/src/data/site.rs` ; `grep -rn "OnDeath" rtsim/src/rule/ | head`
- Files: `rtsim/src/data/npc.rs` (`#[serde(default)] pub coins: u32`, ≤ 5 B); `rtsim/src/data/site.rs` (`#[serde(default)] pub deeds: Vec<Deed>`; `pub struct Deed { pub plot: Id<world::site::Plot>, pub owner: Actor, pub kind: DeedKind }`, `pub enum DeedKind { Home, Shop, Farm }`; plot ids re-linked at load like `world_site`, orphan cleanup in `migrate`); `rtsim/src/rule/lifecycle.rs` binds `OnDeath`: coins+deeds pass spouse → eldest child → sibling → site treasury (order from kinship edges + `birth_tod`).
- Tests: inheritance conserves total coins (property test over random family shapes); deed orphan cleanup; new `site_pre_phase3.dat` fixture loads with `deeds == []`. Budget: deeds on `Site` (≈ 30 B/deed, ≤ 64/site), not on NPCs.

### Phase 4 — Economy (L)

**Task 4.1: `SiteEconomy` persisted state**
- Re-verify: `grep -rn "enum Good" common/src/trade.rs common/src/comp/inventory/trade_pricing.rs | head` ; `grep -n "Labor" world/src/site/economy/map_types.rs | head` ; `grep -n "rugged_ser_enum_map" rtsim/src/data/mod.rs`
- Files: create `rtsim/src/data/economy.rs`: `pub struct SiteEconomy { pub stock: EnumMap<Good, f32>, pub demand: EnumMap<Good, f32>, pub price_mult: EnumMap<Good, f32> }` using the sparse `rugged_ser_enum_map` serializers (`rtsim/src/data/mod.rs:122-166`) so neutral entries cost 0 bytes; `Site` gains `#[serde(default)] pub economy: SiteEconomy`, seeded from the worldgen snapshot in `rule/migrate.rs` when all-default.
- Budget: ≤ 600 B/site (3 maps × ~16 active goods); ~10² sites ⇒ ≤ 60 KB. Tests: sparse serde round-trip; seeding from a synthetic snapshot; site fixture loads default economy.

**Task 4.2: `economy` rule — production, consumption, pricing**
- Re-verify: `grep -n "population" rtsim/src/data/site.rs` ; `grep -n "start_rule" rtsim/src/lib.rs`
- Files: create `rtsim/src/rule/economy.rs` (register after `npc_ai`): in-game-daily per site (staggered by `site.seed`); production from the profession census of `site.population`; per-capita consumption; `price_mult = clamp((demand/supply).powf(0.5), 0.25, 4.0)` smoothed `0.9*old + 0.1*new`. No RNG.
- LOD: site-level aggregates are already statistical — this rule *is* the far-ring economy, identical for all sites. Tests: price rises under shortage and recovers; bounded after 10k random shocks (proptest); two identical runs ⇒ identical state.

**Task 4.3: Live prices into player trade**
- Re-verify: `grep -n "fn trader_loadout\|SiteInformation" server/src/rtsim/tick.rs | head` ; `grep -n "fn balance" common/src/trade.rs`
- Files: `server/src/rtsim/tick.rs` — merchant stocking consumes `SiteEconomy.stock`/`price_mult` instead of the frozen `SiteInformation` (the `// economy isn't economying sometimes` hack site); thread `price_mult` into the `SitePrices` used by `common/src/trade.rs::balance`. No voxygen change.
- Tests: stocked-merchant `SitePrices` reflect `price_mult`; manual `veloren-run` check that bulk-buying raises the price within one in-game day (spec metric). Anti-exploit acceptance: per-player daily trade-volume cap per site (server const), documented and tested.

**Task 4.4: Merchant cargo and utility routes**
- Re-verify: `grep -n "fn adventure" rtsim/src/rule/npc_ai/mod.rs` ; `grep -n "pub enum Job" rtsim/src/data/npc.rs`
- Files: `rtsim/src/data/npc.rs` (`#[serde(default)] pub cargo: Option<(Good, f32)>`, ≤ 16 B); `rtsim/src/rule/npc_ai/mod.rs` — the merchant branch of `adventure()` picks destination by `max((price_mult_dst − price_mult_src) / distance)` over `nearby_sites_by_size` (deterministic, site-id tiebreak); buy-on-departure / sell-on-arrival transfers stock between `SiteEconomy` maps + profit to `npc.coins`; death drops cargo (existing `OnDeath` hook) — banditry gets macro effects.
- Tests: route choice on synthetic spreads; goods conservation across a completed route; killed merchant loses cargo without duplication.

### Phase 5 — Organizations (XL)

**Task 5.1: `Organization` entity + `Data.organizations`**
- Re-verify: `grep -n "pub struct Faction" -A 10 rtsim/src/data/faction.rs` ; `grep -n "FactionId" common/src/rtsim.rs | head -5` ; `grep -n "pub struct Data" -A 25 rtsim/src/data/mod.rs`
- Files: create `rtsim/src/data/organization.rs`; declare slotmap key `OrgId` in `common/src/rtsim.rs` beside `FactionId`. `pub struct Organization { pub kind: OrgKind, pub name: String, pub governance: Governance, pub members: HashMap<Actor, Membership>, pub treasury: f32, pub home: Option<SiteId>, pub goals: Vec<OrgGoal> /* cap 4 */, pub sentiments: Sentiments, pub charter: Option<String> }`; `OrgKind` = the spec's 8 variants; `Governance { Autocratic { leader: Actor }, Council { seats: Vec<Actor> }, Elective { leader: Actor, term_ends: TimeOfDay } }`; `Membership { rank: u8, standing: i8, joined: TimeOfDay }`; `Data` gains `#[serde(default)] pub organizations: Organizations`.
- Budget: ≤ 4 KB/org (members map dominates; roster capped at 128 listed members, larger orgs track count + sampled roster); ≤ 10³ orgs expected. Tests: serde round-trip; member-cap enforcement; old fixtures unaffected (new top-level map defaults empty).

**Task 5.2: `organizations` rule — founding, ranks, dissolution**
- Re-verify: `grep -n "ReportKind" rtsim/src/data/report.rs` ; `grep -n "start_rule" rtsim/src/lib.rs`
- Files: create `rtsim/src/rule/organizations.rs` (in-game daily per org, staggered by org-id hash; seeded `ChaChaRng`): founding scan per the spec table (≥ N same-profession NPCs in one site + founder with matching `Goal`/`Mind.values` + 2 willing co-founders); rank ascent on contribution; dissolution on treasury ≤ 0, leader death without succession, or members < 3 for an in-game month. All transitions create reports — extend `ReportKind` with `OrgEvent { org: OrgId, kind: OrgEventKind }` (additive variant) so gossip propagates.
- Tests: founding fires for a synthetic 6-blacksmith site, not for 2; each dissolution path; determinism.

**Task 5.3: GOAP planner for org goals only**
- Files: create `rtsim/src/ai/goap.rs` (~300 lines, zero new deps): `pub struct WorldFacts(/* bitset + numeric facts */); pub trait OrgAction { fn preconditions(&self, f: &WorldFacts) -> bool; fn apply(&self, f: &mut WorldFacts); fn cost(&self) -> f32; } pub fn plan(start: WorldFacts, goal: impl Fn(&WorldFacts) -> bool, actions: &[Box<dyn OrgAction>], max_depth: usize) -> Option<Vec<usize>>` — A* over action indices. Used **only** by the organizations rule (per-NPC GOAP explicitly rejected, spec §AI(c)).
- Tests: plans a 3-step `Monopolize(Iron)` toy domain; `None` when unsatisfiable; depth cap respected.

**Task 5.4: Faction → Organization migration (staged behind alias)**
- Re-verify: `grep -rn "\.faction" rtsim/src/rule/architect.rs rtsim/src/rule/npc_ai/ | wc -l` (faction is load-bearing — count call sites before touching anything)
- Files: `rtsim/src/rule/migrate.rs` — for each `Faction`, create a `PoliticalFaction` org mirroring leader/sentiments and record `faction_id → org_id`; `Npc::faction`/`Site::faction` **stay** as deprecated aliases for one release; only new AURORA systems read orgs; architect and existing AI keep reading `faction`. No data deletion.
- Tests: migrated org count == faction count; idempotent on re-run (second setup creates nothing).

**Task 5.5: Governance dynamics — succession and elections**
- Files: `rtsim/src/rule/organizations.rs` (+ hooks in `social.rs`): leader `OnDeath` → primogeniture via kinship edges (noble house) or top rank; `Elective` term expiry → candidacy (members with `Value::Power` ≥ threshold), vote weight `standing × sentiment`, seeded-RNG tiebreak. Coups are **not** initiated here: AURORA publishes tension telemetry; ORACLE sanctions via `OnDirective` (integration contract).
- Tests: deterministic succession on a fixture family/org; election vote counting; contested succession (two similar claimants) emits the quest-seed report consumed by Phase 6.

### Phase 6 — Dynamic Quests (L)

**Task 6.1: New `QuestKind` variants + payload generalization**
- Re-verify: `grep -n "pub enum QuestKind" -A 25 rtsim/src/data/quest.rs` ; `grep -rn "Payload" rtsim/src/data/quest.rs rtsim/src/rule/npc_ai/quest.rs | head` ; `grep -n "compare_exchange\|QuestRes" rtsim/src/data/quest.rs | head`
- Files: `rtsim/src/data/quest.rs`: add `Find { target: Actor, area: SiteId }`, `Procure { good: Good, amount: u32, site: SiteId }`, `Mediate { a: Actor, b: Actor }`, `Investigate { report: ReportId }`; generalize the hardcoded courier `Payload` to carry an `ItemResource`. Additive enum variants — serde-compatible with old saves.
- Tests: serde round-trip per variant; existing quest tests unaffected; the arbiter monotonic-resolution property (`AtomicU8` compare-exchange) holds for new kinds.

**Task 6.2: `quest_gen` rule — needs → seeds → validation → rewards**
- Re-verify: `grep -n "PathingMemory\|Track" rtsim/src/data/npc.rs | head -3` ; `grep -n "fn related_to" rtsim/src/data/quest.rs` ; `ls docs/superpowers/specs/ | grep character-levels`
- Files: create `rtsim/src/rule/quest_gen.rs` (in-game daily per site): `struct QuestSeed { template: Template, urgency: f32, arbiter: NpcId }`; need detection scans `Mind.goals`, moods (grief ⇒ revenge), unresolved reports, `SiteEconomy.demand` spikes; validation — target alive + reachable (site path via `world::civ::Track`), items obtainable, arbiter alive and home; **unsolvable seeds are dropped, never patched**; reward = base(danger × distance) × site wealth, XP band per the character-levels spec (already merged — hard dependency).
- Tests: validation rejects unreachable/dead targets; urgency ordering deterministic; reward monotonic in distance and danger.

**Task 6.3: Ten templates wired to dialogue offers**
- Re-verify: `grep -n "quest_request\|fn quest" rtsim/src/rule/npc_ai/quest.rs | head`
- Files: `rtsim/src/rule/quest_gen.rs` (constructors for the spec's 10-template taxonomy); `rtsim/src/rule/npc_ai/quest.rs` (offer generated quests through the existing dialogue path + escrow deposits); i18n keys in `assets/voxygen/i18n/en/dialogue.ftl`.
- Tests: each template yields a valid `Quest` from a synthetic world state; per-template i18n key test mirroring Task 10's pattern.

**Task 6.4: Anti-exploit guards**
- Files: `rtsim/src/rule/quest_gen.rs` + `rtsim/src/data/quest.rs`: ≤ 3 active generated quests per player; per-template-per-site cooldown (persisted `#[serde(default)]` map on the site, ≤ 200 B/site); expiry via the existing `timeout`; abandonment ⇒ arbiter sentiment penalty (thresholds already gate behavior via the `sentiment.rs` consts).
- Tests: rate-limit property test; cooldown survives save/load; penalty applied on abandon. Reward duplication impossible by construction (existing escrow + monotonic resolution).

### Phase 7 — LLM Integration (M)

**Task 7.1: `TextOracle` trait + `NullOracle`**
- Re-verify: `grep -n "with_resource\|pub mod" rtsim/src/lib.rs | head -15`
- Files: create `rtsim/src/llm.rs`: `pub struct TextRequest { pub template_id: &'static str, pub personality_bucket: u8, pub mood_bucket: u8, pub facts: Vec<String> } pub struct TextTicket(pub u64); pub trait TextOracle: Send + Sync { fn request(&self, req: TextRequest) -> TextTicket; fn poll(&self, ticket: TextTicket) -> Option<String>; } pub struct NullOracle;` (`poll` always `None` ⇒ template fallback). Oracle handed to rtsim as an `RtState` resource via `with_resource`.
- Tests: `NullOracle` contract; a test-only `MockOracle` for 7.3. Phase acceptance: **the game is 100% playable with `NullOracle`**.

**Task 7.2: Server bridge with cache and budget**
- Re-verify: `ls server/src/rtsim/` ; `grep -rn "pub struct Settings" server/src/settings/mod.rs | head -3`
- Files: create `server/src/rtsim/llm_bridge.rs`: worker thread, bounded queue (depth 64, drop-oldest); backend enum `{ Disabled, Local { url }, Remote { model } }` from `server/src/settings/mod.rs`; LRU cache keyed `(template_id, personality_bucket, mood_bucket, fact_hash)`; 2 s timeout; `poll` is a map lookup, `request` enqueue-only — **no blocking call in any tick path** (debug-assert on elapsed time); hit/miss/drop counters exposed to metrics.
- Tests: overflow drops oldest; timeout yields `None`; bucketing collapses similar NPCs to few cache keys (hit-rate test over a synthetic population).

**Task 7.3: Dialogue color + org charters consumption**
- Files: `rtsim/src/rule/npc_ai/dialogue.rs` — **LOD split: loaded NPCs only** (LLM color is player-facing presentation; simulated NPCs always use templates); `rtsim/src/rule/organizations.rs` — founding requests a one-shot charter cached in `Organization.charter`.
- Tests: with `MockOracle`, the colored line is used when ready and the template ships verbatim on `None`; charter generated exactly once per org. Acceptance: outputs length-capped (240 chars); LLM text never feeds back into simulation state besides the `charter` string.

### Phase 8 — Optimization (L)

**Task 8.1: Statistical far ring**
- Re-verify: `grep -n "SimulationMode" rtsim/src/data/npc.rs rtsim/src/rule/simulate_npcs.rs | head` ; `grep -n "SIMULATED_TICK_SKIP" rtsim/src/rule/npc_ai/mod.rs`
- Files: `rtsim/src/rule/simulate_npcs.rs` + the `lifecycle`/`social`/`economy` rules: sites without player presence for > 1 in-game day demote to aggregate updates (birth/death/edge-drift as site-level rates); promotion lazily reconciles individuals. Off-screen whitelist (spec Open Question 6): marriages/births/prices **yes**; deaths of arbiters with active quests **no** (checked via `Quests::related_to`).
- Tests: demote → 30 days → promote yields population/edge counts within ±10% of a fully-simulated reference; no active-quest arbiter dies off-screen (property test).

**Task 8.2: Criterion benches + tick budgets**
- Files: `rtsim/Cargo.toml` (`[dev-dependencies] criterion` + `[[bench]]` entries); create `rtsim/benches/{social_tick_10k,consolidation_10k,economy_50_sites,data_clone_serialize_10k,quest_gen_validation}.rs` over synthetic 10k-NPC `Data`.
- Acceptance thresholds (dev profile, spec §Scale): social ≤ 0.3 ms/tick-slice; consolidation ≤ 0.2 ms; economy ≤ 0.3 ms amortized; full `Data` clone+serialize ≤ 250 ms (guards the 60 s background save — re-verify with `grep -n "save" server/src/rtsim/tick.rs | head`); CI manual-dispatch job alerts at +20%.

**Task 8.3: Save-clone optimization (conditional)**
- Only if 8.2's `data_clone_serialize_10k` exceeds budget: copy-on-write snapshot (`Arc`-wrap large `Data` sub-maps, clone-on-mutate) behind an unchanged `Data::write_to`.
- Tests: serialization byte-identical pre/post; save-thread behavior unchanged. Acceptance: `data.dat` ≤ 30 MB at 10k NPCs (spec metric), measured in soak.

---

**Plan complete.** Execute with superpowers:subagent-driven-development, one task per subagent, in order.
