# Superpowers Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create 5 project-specific skills and 4 protective hooks for the Veloren codebase, reducing cognitive overhead across all development workflows.

**Architecture:** Skills live in `.claude/skills/<name>/SKILL.md` with YAML frontmatter; hooks are JSON entries in `.claude/settings.local.json`. Each skill is self-contained instructions that guide Claude's behavior in a specific workflow context. Hooks are shell commands triggered by Claude Code events.

**Tech Stack:** Claude Code skill system (Markdown + YAML frontmatter), Claude Code hooks (JSON + Bash), Rust toolchain (cargo fmt, clippy), Git

---

## File Map

| File | Action | Purpose |
|------|--------|---------|
| `.claude/skills/veloren-run/SKILL.md` | Create | Instructions for launching client/server correctly |
| `.claude/skills/veloren-dev/SKILL.md` | Create | Instructions for implementing ECS mechanics/features |
| `.claude/skills/veloren-debug/SKILL.md` | Create | Instructions for ECS-aware debugging |
| `.claude/skills/veloren-review/SKILL.md` | Create | Instructions for pre-merge code review |
| `.claude/skills/veloren-worldgen/SKILL.md` | Create | Instructions for world generation iteration |
| `.claude/settings.local.json` | Modify | Add 4 hooks: Stop lint reminder, PreToolUse fmt gate, rm guard, SQLite guard |

---

## Task 1: Create `veloren-run` skill

**Files:**
- Create: `.claude/skills/veloren-run/SKILL.md`

- [ ] **Step 1: Create the skill directory and file**

```bash
mkdir -p .claude/skills/veloren-run
```

- [ ] **Step 2: Write the skill content**

Create `.claude/skills/veloren-run/SKILL.md` with this exact content:

```markdown
---
name: veloren-run
description: Use when launching the Veloren game client or server — knows the correct env vars, features, hot-reload behavior, and cargo aliases
---

# veloren-run

## Pre-requisites

Always ensure the Rust toolchain is on PATH before running:
```bash
source "$HOME/.cargo/env"
```

`VELOREN_ASSETS` must point to the assets directory. In the project root:
```bash
export VELOREN_ASSETS="$(pwd)/assets"
```

## Launching the Client (GUI)

**Standard dev build** (hot-reloading enabled for animations and UI):
```bash
cargo run --bin veloren-voxygen
```

**Without hot-reloading** (faster startup, closer to release):
```bash
cargo test-voxygen
```

**With Tracy profiler** (performance profiling):
```bash
cargo tracy-voxygen
```

**With debug symbols** (for lldb/gdb):
```bash
cargo dbg-voxygen
```

**Release build** (no hot-reloading, LTO, for shipping):
```bash
cargo build --release --no-default-features --features default-publish
./target/release/veloren-voxygen
```

## Launching the Server (Headless)

**Standard dev server** (with worldgen, hot-reload agent AI):
```bash
cargo server
# equivalent to: cargo run --bin veloren-server-cli
```

**Minimal server** (no hot-reloading, faster startup for testing net code):
```bash
cargo test-server
# equivalent to: cargo run --bin veloren-server-cli --no-default-features --features simd
```

**With Tracy profiler**:
```bash
cargo tracy-server
```

## Single-Player (Client embeds Server)

Single-player mode is built into `veloren-voxygen` via the `singleplayer` feature (on by default in dev). Just run the client — no separate server process needed:
```bash
cargo run --bin veloren-voxygen
# then select "Singleplayer" in the main menu
```

## Hot-Reloading Behavior

In dev builds, two crates are compiled as dynamic libraries and reloaded at runtime without restarting:
- `server-agent` — NPC AI behavior (gated by `hot-agent` feature)
- `voxygen-anim` — character animations (gated by `hot-anim` feature)

To pick up changes: save the file → the game reloads it automatically within a few seconds.

Hot-reloading is **disabled** in `default-publish` (release) builds.

## Configuration Files

- Server settings: `userdata/server/settings.ron` (created on first run, gitignored)
- Client settings: `userdata/client/settings.ron` (created on first run, gitignored)
- Cargo aliases: `.cargo/config.toml`

## Troubleshooting

- **"assets not found"**: Make sure `VELOREN_ASSETS="$(pwd)/assets"` is set and you're running from the project root.
- **Compilation errors on first run**: The project requires nightly Rust. Run `rustup show active-toolchain` — it should say `nightly-2025-09-08`.
- **Window doesn't appear**: Check macOS permissions (accessibility, network). Try running from terminal directly.
```

- [ ] **Step 3: Verify the file was created correctly**

```bash
head -5 .claude/skills/veloren-run/SKILL.md
```

Expected output:
```
---
name: veloren-run
description: Use when launching the Veloren game client or server — knows the correct env vars, features, hot-reload behavior, and cargo aliases
---
```

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/veloren-run/SKILL.md
git commit -m "feat: add veloren-run skill for launching client/server"
```

---

## Task 2: Create `veloren-dev` skill

**Files:**
- Create: `.claude/skills/veloren-dev/SKILL.md`

- [ ] **Step 1: Create the skill directory**

```bash
mkdir -p .claude/skills/veloren-dev
```

- [ ] **Step 2: Write the skill content**

Create `.claude/skills/veloren-dev/SKILL.md`:

```markdown
---
name: veloren-dev
description: Use when implementing new gameplay mechanics, ECS components, systems, abilities, NPC behaviors, or admin commands — guides where to make changes and what to check
---

# veloren-dev

**REQUIRED:** Before writing any code, invoke `superpowers:test-driven-development`.

## Step 0: Identify the Change Type

Before touching any file, classify the work:

| Change type | Primary file(s) | Registration |
|------------|-----------------|-------------|
| New ECS component | `common/src/comp/<name>.rs` | `common/state/src/state.rs` |
| New shared system (client+server) | `common/systems/src/<name>.rs` | `common/systems/src/lib.rs` |
| New server-only system | `server/src/sys/<name>.rs` | `server/src/sys/mod.rs` |
| New combat ability | `common/src/comp/ability.rs` | + state in `character_state.rs` |
| NPC behavior / AI | `server/agent/src/action_nodes.rs` | — (hot-reloadable dylib) |
| NPC combat AI | `server/agent/src/attack.rs` | — |
| New admin command | `common/src/cmd.rs` (enum) | + handler in `server/src/cmd.rs` |
| New item / recipe | `assets/common/items/` (RON) | `assets/common/recipe_book.ron` |
| New buff/debuff | `common/src/comp/buff.rs` | — |
| New resource | `common/src/resources.rs` | `common-state/src/state.rs` |

## Step 1: Read the Existing Pattern

Always find a similar existing implementation and read it before writing new code.

- New component? Read `common/src/comp/poise.rs` (clean, small example).
- New system? Read `common/systems/src/stats.rs` (minimal shared system).
- New server system? Read `server/src/sys/waypoint.rs` (small, focused).
- New ability? Read an existing ability in `common/src/comp/ability.rs` — find the `BasicMelee` variant as a starting point.
- New NPC behavior? Read a small node in `server/agent/src/action_nodes.rs`.

## Step 2: Write Tests First

Invoke `superpowers:test-driven-development` now.

Tests for ECS go in `<crate>/src/<module>.rs` as `#[cfg(test)]` blocks, or in `<crate>/tests/`. Run with:
```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-<crate> -- --nocapture
```

For a specific test:
```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common test_my_component -- --nocapture
```

## Step 3: Implement

### Adding a New ECS Component

1. Create `common/src/comp/<name>.rs` with the component struct.
2. Derive `Component` from specs: `use specs::Component;`
3. Export from `common/src/comp/mod.rs`: `pub mod <name>;` and re-export key types.
4. Register in `common/state/src/state.rs` in the `register_components` function:
   ```rust
   world.register::<comp::MyComponent>();
   ```

### Adding a New Shared System

1. Create `common/systems/src/<name>.rs`.
2. Implement `specs::System` with a `SystemData` type alias.
3. Register in `common/systems/src/lib.rs`:
   ```rust
   pub fn add_local_systems(dispatch_builder: &mut DispatcherBuilder) {
       dispatch::<my_sys::Sys>(dispatch_builder, &[/* dependencies */]);
   }
   ```

### Adding a New Server-Only System

1. Create `server/src/sys/<name>.rs`.
2. Implement `specs::System`.
3. Add to `server/src/sys/mod.rs` in the server dispatcher setup.

### Adding a New Admin Command

1. Add variant to the `ServerChatCommand` or `ChatCommand` enum in `common/src/cmd.rs`.
2. Add the handler function in `server/src/cmd.rs` — follow the pattern of existing handlers.
3. The command will auto-appear in `/help` output.

## Step 4: Verify Incrementally

After each logical unit of code (not after every line), run:
```bash
source "$HOME/.cargo/env"
cargo check -p veloren-<affected-crate>
```

This is fast (~5-10s) and catches most errors before a full build.

## Step 5: Test In-Game

Use the `veloren-run` skill to launch the game and test the feature manually.

Useful in-game commands for testing:
- `/give <item_path>` — spawn an item
- `/tp <x> <y> <z>` — teleport for positioning
- `/spawn <entity>` — spawn an NPC
- `/sudo <player> <command>` — run command as another entity
- `/debug` — toggle debug rendering

## Key Reference Files by Area

| Area | File | Size |
|------|------|------|
| Combat abilities | `common/src/comp/ability.rs` | 142KB |
| Character state machine | `common/src/comp/character_state.rs` | 65KB |
| Buff system | `common/src/comp/buff.rs` | 57KB |
| Inventory/loadout | `common/src/comp/inventory/loadout_builder.rs` | 61KB |
| NPC combat AI | `server/agent/src/attack.rs` | 361KB |
| NPC behavior tree | `server/agent/src/action_nodes.rs` | 103KB |
| Physics system | `common/systems/src/phys/mod.rs` | — |
| Server state ext | `server/src/state_ext.rs` | 56KB |
| Admin commands | `server/src/cmd.rs` | 217KB |
```

- [ ] **Step 3: Verify**

```bash
head -5 .claude/skills/veloren-dev/SKILL.md
```

Expected:
```
---
name: veloren-dev
description: Use when implementing new gameplay mechanics, ECS components, systems, abilities, NPC behaviors, or admin commands — guides where to make changes and what to check
---
```

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/veloren-dev/SKILL.md
git commit -m "feat: add veloren-dev skill for implementing ECS mechanics"
```

---

## Task 3: Create `veloren-debug` skill

**Files:**
- Create: `.claude/skills/veloren-debug/SKILL.md`

- [ ] **Step 1: Create directory**

```bash
mkdir -p .claude/skills/veloren-debug
```

- [ ] **Step 2: Write skill content**

Create `.claude/skills/veloren-debug/SKILL.md`:

```markdown
---
name: veloren-debug
description: Use when investigating any bug or unexpected game behavior — provides ECS-aware debugging workflow before proposing fixes
---

# veloren-debug

**REQUIRED:** Invoke `superpowers:systematic-debugging` before proposing any fix.

## Step 1: Reproduce the Bug

Identify the minimal sequence to reproduce. Use admin commands:

```bash
# Launch server with admin access
cargo server
# In another terminal, connect with client
cargo run --bin veloren-voxygen
```

In-game reproduction commands:
- `/tp <x> <y> <z>` — teleport to specific coordinates
- `/give <item>` — e.g. `/give common.items.weapons.sword.starter`
- `/spawn <entity>` — e.g. `/spawn common.entity.npc.humanoid.villager`
- `/time <hour>` — set time of day (e.g. `/time 12` for noon)
- `/weather <type>` — change weather
- `/debug` — toggle debug overlays
- `/entity` — dump entity state at cursor
- `/sudo <player> <cmd>` — act as another entity

## Step 2: Classify the Bug in ECS Terms

Ask: where does the incorrect behavior originate?

**Component data is wrong:**
- The stored data on an entity is incorrect.
- Look in `common/src/comp/` for the relevant component.
- Check where that component is written (grep for `WriteStorage<CompName>`).

**System isn't running / running in wrong order:**
- Check the dispatcher setup in `common/systems/src/lib.rs` (shared) or `server/src/sys/mod.rs` (server-only).
- System dependencies declared via `dispatch::<Sys>(builder, &[deps...])` determine order.

**Event not emitted / not handled:**
- Event types defined in `common/src/event.rs`.
- Handlers in `server/src/events/` (server-side) or client-side in `client/src/`.
- Check `EventBus<EventType>` usage.

**Network message not sent / received:**
- Message types in `common/net/src/msg/`.
- Server sends in `server/src/sys/msg/` systems.
- Client receives in `client/src/` handlers.

**Physics / position issue:**
- Physics system: `common/systems/src/phys/mod.rs`
- Collision detection: `common/systems/src/phys/collision.rs`
- Visual gizmos for debugging positions: `common/src/comp/gizmos.rs`

## Step 3: Instrument the Code

Add temporary tracing spans to narrow down the issue:

```rust
// At the top of the relevant file:
use tracing::{debug, warn, error};

// Inside the suspect code:
debug!(?component_value, entity = ?entity, "Checking component state");
warn!("Unexpected state reached: {:?}", value);
```

Run the server/client and watch logs:
- Server logs: `userdata/server/logs/`
- Client logs: `userdata/client/logs/`
- Or pipe to terminal: `cargo server 2>&1 | grep -i "your_debug_message"`

## Step 4: Performance Bugs — Use Tracy

If the bug is a performance regression (lag, frame drops):

```bash
# Server performance
cargo tracy-server

# Client/rendering performance
cargo tracy-voxygen
```

Connect Tracy profiler (download from https://github.com/wolfpld/tracy) to the running process to see per-system timing.

## Step 5: Visual Physics Bugs

For position/collision/movement bugs that are hard to trace via logs:

1. Add a gizmo to the suspect entity in the relevant system:
   ```rust
   // WriteStorage<comp::Gizmos> in SystemData
   gizmos.insert(entity, comp::Gizmos::default()).ok();
   ```
2. Run with `/debug` toggle in-game to see entity bounds and positions.

## Common Bug Patterns in Veloren

| Symptom | Likely cause | Where to look |
|---------|-------------|---------------|
| NPC freezes / stops acting | Agent state machine stuck | `server/agent/src/action_nodes.rs` |
| Ability doesn't activate | Character state transition fails | `common/src/comp/character_state.rs` |
| Damage not applied | Melee/projectile system miss | `common/systems/src/melee.rs` or `projectile.rs` |
| Entity desync client/server | Entity sync system | `server/src/sys/entity_sync.rs` |
| Chunk not loading | Chunk generator queue | `server/src/chunk_generator.rs` |
| Item not saving | Persistence system | `server/src/persistence/` |
| Buff not applying | Buff system data | `common/systems/src/buff.rs` |

## Step 6: Fix → Verify → Clean Up

After finding the root cause:
1. Remove all temporary `debug!` / `warn!` instrumentation.
2. Write a regression test if possible.
3. Run `cargo check -p veloren-<affected-crate>` to verify it compiles.
4. Use `veloren-run` skill to do a full manual test.
5. Use `veloren-review` skill before committing.
```

- [ ] **Step 3: Verify**

```bash
head -5 .claude/skills/veloren-debug/SKILL.md
```

Expected:
```
---
name: veloren-debug
description: Use when investigating any bug or unexpected game behavior — provides ECS-aware debugging workflow before proposing fixes
---
```

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/veloren-debug/SKILL.md
git commit -m "feat: add veloren-debug skill for ECS-aware debugging"
```

---

## Task 4: Create `veloren-review` skill

**Files:**
- Create: `.claude/skills/veloren-review/SKILL.md`

- [ ] **Step 1: Create directory**

```bash
mkdir -p .claude/skills/veloren-review
```

- [ ] **Step 2: Write skill content**

Create `.claude/skills/veloren-review/SKILL.md`:

```markdown
---
name: veloren-review
description: Use before merging any branch — runs CI lint checks, verifies ECS patterns, then invokes superpowers code review
---

# veloren-review

**Run this skill before every merge to main.** It does not modify code — only verifies and reports.

## Step 1: Check Formatting

```bash
source "$HOME/.cargo/env"
cargo fmt --all -- --check
```

- **Pass (no output, exit 0):** Formatting is clean. Continue.
- **Fail (lists files):** Run `cargo fmt --all` to fix, then review the diff before continuing.

## Step 2: Lint — All Targets (CI exact command)

```bash
cargo ci-clippy
# expands to:
# cargo clippy --all-targets --locked \
#   --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat" \
#   -- -D warnings
```

All warnings are treated as errors (`-D warnings`). Fix every warning before proceeding.

## Step 3: Lint — Voxygen Publish Profile (CI exact command)

```bash
cargo ci-clippy2
# expands to:
# cargo clippy -p veloren-voxygen --locked \
#   --no-default-features --features="default-publish" \
#   -- -D warnings
```

This checks the client in release mode (no hot-reloading). Catches feature-gated issues that only surface in publish builds.

## Step 4: ECS Pattern Checklist

For each new component, system, or resource added in this diff, verify:

**New ECS components:**
- [ ] Struct derives `specs::Component` (usually via `#[derive(Component)]`)
- [ ] Registered in `common/state/src/state.rs` via `world.register::<comp::MyComponent>()`
- [ ] Exported from `common/src/comp/mod.rs`

**New shared systems (client+server):**
- [ ] Implement `specs::System`
- [ ] Registered in `common/systems/src/lib.rs` via `dispatch::<Sys>(builder, &[deps])`
- [ ] Dependencies declared correctly (systems that write to components this system reads are listed as deps)

**New server-only systems:**
- [ ] Implement `specs::System`
- [ ] Added to the server dispatcher in `server/src/sys/mod.rs`

**New resources:**
- [ ] Defined in `common/src/resources.rs`
- [ ] Inserted into the world in `common/state/src/state.rs` via `world.insert(...)`

**New admin commands:**
- [ ] Variant added to the command enum in `common/src/cmd.rs`
- [ ] Handler implemented in `server/src/cmd.rs`
- [ ] Help text added to the command definition

## Step 5: Invoke Code Review

```
superpowers:requesting-code-review
```

This does the deep analysis of the diff — logic errors, edge cases, performance, security. The steps above are pre-checks so the code review focuses on logic, not style.

## Step 6: After Review Feedback

If the code review or CI finds issues:
1. Fix the issues.
2. Repeat Steps 1–3 (fmt + clippy) after changes.
3. Do not merge until all CI checks pass and review is approved.
```

- [ ] **Step 3: Verify**

```bash
head -5 .claude/skills/veloren-review/SKILL.md
```

Expected:
```
---
name: veloren-review
description: Use before merging any branch — runs CI lint checks, verifies ECS patterns, then invokes superpowers code review
---
```

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/veloren-review/SKILL.md
git commit -m "feat: add veloren-review skill for pre-merge CI and ECS validation"
```

---

## Task 5: Create `veloren-worldgen` skill

**Files:**
- Create: `.claude/skills/veloren-worldgen/SKILL.md`

- [ ] **Step 1: Create directory**

```bash
mkdir -p .claude/skills/veloren-worldgen
```

- [ ] **Step 2: Write skill content**

Create `.claude/skills/veloren-worldgen/SKILL.md`:

```markdown
---
name: veloren-worldgen
description: Use when iterating on world generation, terrain, sites, rtsim civilization simulation, or procedural content — knows the generation pipeline and iteration workflow
---

# veloren-worldgen

## Generation Pipeline (in order)

Changes cascade downstream — a change to `sim/` affects everything below it.

```
world/src/sim/       ← WorldSim: erosion, rivers, rainfall, biomes, caves
      ↓
world/src/civ/       ← WorldCiv: settlements, trade routes, factions
      ↓
world/src/site/      ← Individual sites: towns, dungeons, castles, bridges
      ↓
world/src/layer/     ← Detail layer: trees, grass, sprites, surface features
      ↓
world/src/column.rs  ← Per-column terrain (56KB — most terrain detail lives here)
      ↓
rtsim/               ← Long-running simulation: NPC migration, economy, conflicts
```

## Iteration Workflow

```bash
# 1. Make your change
# 2. Fast compile check (5-10s):
source "$HOME/.cargo/env"
cargo check -p veloren-world    # for world/
cargo check -p veloren-rtsim    # for rtsim/

# 3. Start the server (worldgen runs on startup):
cargo server

# 4. Connect with the client and fly to the affected area:
cargo run --bin veloren-voxygen
# In game: use airship or /tp <x> <y> <z>

# 5. Observe, take note, kill server (Ctrl+C), tweak, repeat
```

## Key Files by Area

| Area | File | Notes |
|------|------|-------|
| High-level world params | `world/src/config.rs` | Struct `WorldConfig`, loaded from assets |
| Terrain column generation | `world/src/column.rs` | 56KB — most biome/terrain logic |
| World simulation | `world/src/sim/mod.rs` | Erosion, rivers, altitude |
| Civilization | `world/src/civ/mod.rs` | Settlement placement, trade |
| Town generation | `world/src/site/settlement/` | Building placement |
| Dungeon generation | `world/src/site/dungeon/` | Room/corridor generation |
| Sprite/tree placement | `world/src/layer/` | Surface detail |
| Rtsim core | `rtsim/src/lib.rs` | Event/Rule system design |
| Rtsim NPC data | `rtsim/src/data/npc.rs` | NPC state |
| Rtsim AI decisions | `rtsim/src/ai/` | NPC behavior |

## World-Gen Math Profile

World generation uses heavy floating-point math with large numbers. To avoid overflow panics in dev mode (which has overflow checks on), use the `no_overflow` profile for iteration:

```bash
cargo run --bin veloren-server-cli --profile no_overflow
```

Or to build world specifically with the profile:
```bash
cargo build -p veloren-world --profile no_overflow
```

## Adjusting Parameters

Most world-gen parameters are either:

1. **In RON config files** (fast to change, no recompile):
   - `assets/world/` — look for `.ron` files with world parameters
   - Edit → restart server → observe

2. **In Rust consts/structs** (requires recompile):
   - `world/src/config.rs` — `WorldConfig`
   - `world/src/sim/mod.rs` — simulation constants
   - After change: `cargo check -p veloren-world` → restart server

## Visualization Tools

```bash
# Recipe dependency graph (requires graphviz):
cargo dot-recipes | dot -Tsvg > recipes.svg && open recipes.svg

# Skill dependency graph:
cargo dot-skills | dot -Tsvg > skills.svg && open skills.svg

# Airship routes (run server with feature):
cargo run --bin veloren-server-cli --features airship_maps

# Chunk compression benchmark:
cargo run --manifest-path world/Cargo.toml --features=bin_compression --bin chunk_compression_benchmarks
```

## rtsim — Real-Time World Simulation

rtsim is **not ECS** — it's a rule/event-driven system simulating thousands of NPCs at coarse granularity (no combat, no physics). It runs in `rtsim/src/`.

**Core concepts:**
- `RtState` — the world state (tables of NPCs, sites, factions)
- `Event` — something that happened (NPC arrived, battle concluded, trade completed)
- `Rule` — reacts to events and updates state
- Data tables in `rtsim/src/data/` — NPCs, sites, factions are rows in tables

**Iteration workflow for rtsim:**
```bash
cargo check -p veloren-rtsim
cargo server   # rtsim runs on the server
# watch server logs for rtsim output:
cargo server 2>&1 | grep -i rtsim
```

**Rtsim logs location:** `userdata/server/logs/` (look for rtsim-related entries)

## Testing World Gen Changes

There are no automated tests for visual/aesthetic world-gen output — judgment is required. However:

```bash
# Run world-gen unit tests:
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-world -- --nocapture

# Run rtsim tests:
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-rtsim -- --nocapture
```
```

- [ ] **Step 3: Verify**

```bash
head -5 .claude/skills/veloren-worldgen/SKILL.md
```

Expected:
```
---
name: veloren-worldgen
description: Use when iterating on world generation, terrain, sites, rtsim civilization simulation, or procedural content — knows the generation pipeline and iteration workflow
---
```

- [ ] **Step 4: Commit**

```bash
git add .claude/skills/veloren-worldgen/SKILL.md
git commit -m "feat: add veloren-worldgen skill for world generation iteration"
```

---

## Task 6: Add hooks to `settings.local.json`

**Files:**
- Modify: `.claude/settings.local.json`

- [ ] **Step 1: Read the current file**

```bash
cat .claude/settings.local.json
```

Confirm it currently has `permissions` and `enabledPlugins` keys, no `hooks` key.

- [ ] **Step 2: Add the hooks section**

Replace `.claude/settings.local.json` with the merged content. The new file must preserve all existing `permissions` and `enabledPlugins` entries, plus add the `hooks` key:

```json
{
  "permissions": {
    "allow": [
      "Bash(gh auth *)",
      "Bash(gh --version)",
      "Bash(git checkout *)",
      "Bash(git add *)",
      "Bash(git commit -m ' *)",
      "Bash(git push *)",
      "Bash(gh repo *)",
      "Bash(git count-objects *)",
      "Bash(git *)",
      "Bash(gh api *)",
      "Bash(awk 'NR==1000 {print $1}')",
      "Bash(awk 'NR==5000 {print $1}')",
      "Bash(awk 'NR==10000 {print $1}')",
      "Bash(awk 'NR==15000 {print $1}')",
      "Bash(awk '{sum += $NF} END {print sum \" bytes total\"}')",
      "Bash(awk '{ *)",
      "Bash(xargs ls -lh)",
      "Bash(rustup toolchain *)",
      "Read(//Users/mgrinberg/bin/**)",
      "Read(//usr/local/bin/**)",
      "Read(//Users/mgrinberg/**)",
      "Read(//Users/mgrinberg/.rustup/**)",
      "Bash(curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs)",
      "Bash(sh -s -- -y --default-toolchain none)",
      "Bash(brew install *)",
      "Bash(python3 -c \"import sys,json; d=json.load\\(sys.stdin\\); print\\(json.dumps\\({k:v for k,v in d.items\\(\\) if k in ['hooks','permissions','env']}, indent=2\\)\\)\")",
      "Bash(python3 -c \"import sys,json; d=json.load\\(sys.stdin\\); [print\\(p.get\\('name','?'\\), '-', p.get\\('version','?'\\)\\) for p in d.get\\('plugins',[]\\)]\")",
      "Bash(python3 -m json.tool)"
    ]
  },
  "enabledPlugins": {
    "superpowers@superpowers-marketplace": true
  },
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "cd /Users/mgrinberg/Workspace/RustroverProjects/veloren && git diff --name-only HEAD 2>/dev/null | grep -q '\\.rs$' && echo '⚠  Hay archivos .rs modificados sin verificar — recordá correr: cargo ci-clippy' || true"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Bash(git commit*)",
        "hooks": [
          {
            "type": "command",
            "command": "source \"$HOME/.cargo/env\" && cd /Users/mgrinberg/Workspace/RustroverProjects/veloren && cargo fmt --all -- --check 2>&1 || (echo 'BLOQUEADO: Hay archivos con formato incorrecto. Corré: cargo fmt --all' && exit 2)"
          }
        ]
      },
      {
        "matcher": "Bash(rm*assets*|rm*sqlite*|rm*persistence*|rm*userdata*)",
        "hooks": [
          {
            "type": "command",
            "command": "echo 'BLOQUEADO: rm sobre ruta crítica del proyecto (assets/, *.sqlite, persistence/, userdata/). Requiere confirmación explícita del usuario.' && exit 2"
          }
        ]
      },
      {
        "matcher": "Bash(sqlite3*)",
        "hooks": [
          {
            "type": "command",
            "command": "echo 'BLOQUEADO: Operación directa sobre SQLite detectada. La base de datos del juego requiere aprobación explícita del usuario antes de cualquier escritura.' && exit 2"
          }
        ]
      }
    ]
  }
}
```

- [ ] **Step 3: Validate the JSON is well-formed**

```bash
python3 -m json.tool .claude/settings.local.json > /dev/null && echo "JSON válido"
```

Expected output: `JSON válido`

- [ ] **Step 4: Verify hooks key exists**

```bash
python3 -c "import json; d=json.load(open('.claude/settings.local.json')); print('hooks keys:', list(d['hooks'].keys()))"
```

Expected output: `hooks keys: ['Stop', 'PreToolUse']`

- [ ] **Step 5: Reload plugins so Claude Code picks up changes**

In the Claude Code UI: run `/reload-plugins`

OR from terminal (if supported):
```bash
# The settings are picked up on next conversation start or plugin reload
```

- [ ] **Step 6: Smoke-test the Stop hook**

```bash
# Touch a .rs file to make git diff show it as modified
touch common/src/resources.rs
# Then check the hook command manually:
cd /Users/mgrinberg/Workspace/RustroverProjects/veloren && git diff --name-only HEAD 2>/dev/null | grep -q '\.rs$' && echo '⚠  Hay archivos .rs modificados sin verificar — recordá correr: cargo ci-clippy' || true
# Expected: prints the warning
# Restore:
git checkout common/src/resources.rs
```

- [ ] **Step 7: Smoke-test the fmt gate**

```bash
source "$HOME/.cargo/env" && cd /Users/mgrinberg/Workspace/RustroverProjects/veloren && cargo fmt --all -- --check 2>&1
```

Expected: exits 0 with no output (code is already formatted, so hook would pass).

- [ ] **Step 8: Commit**

```bash
git add .claude/settings.local.json
git commit -m "feat: add 3 protective hooks (Stop lint reminder, pre-commit fmt gate, rm/sqlite guards)"
```

---

## Task 7: Verify all skills are discoverable

- [ ] **Step 1: List all created skill files**

```bash
find .claude/skills -name "SKILL.md" | sort
```

Expected output:
```
.claude/skills/veloren-debug/SKILL.md
.claude/skills/veloren-dev/SKILL.md
.claude/skills/veloren-review/SKILL.md
.claude/skills/veloren-run/SKILL.md
.claude/skills/veloren-worldgen/SKILL.md
```

- [ ] **Step 2: Verify each has correct frontmatter**

```bash
for skill in .claude/skills/*/SKILL.md; do
  echo "=== $skill ==="
  head -4 "$skill"
  echo ""
done
```

Expected: each file shows `name:` and `description:` fields with non-empty values.

- [ ] **Step 3: Verify JSON settings are valid**

```bash
python3 -m json.tool .claude/settings.local.json > /dev/null && echo "settings.local.json: OK"
```

- [ ] **Step 4: Reload plugins in Claude Code**

Run `/reload-plugins` in the Claude Code UI to make the new skills available immediately.

- [ ] **Step 5: Final commit with summary**

```bash
git add -A
git status  # confirm nothing unexpected
git commit -m "chore: verify superpowers infrastructure complete (5 skills, 4 hooks)"
```

---

## Self-Review Notes

**Spec coverage check:**
- ✅ `veloren-run` — spec §1.1 covered
- ✅ `veloren-dev` — spec §1.2 covered, invokes TDD skill, shows ECS registration
- ✅ `veloren-debug` — spec §1.3 covered, invokes systematic-debugging
- ✅ `veloren-review` — spec §1.4 covered, invokes requesting-code-review
- ✅ `veloren-worldgen` — spec §1.5 covered, rtsim included
- ✅ Stop hook — spec §2.1 covered
- ✅ Pre-commit fmt gate — spec §2.2 covered
- ✅ rm guard — spec §2.3 covered
- ✅ SQLite guard — spec §2.4 covered (merged DROP TABLE/DELETE FROM into sqlite3 matcher since those patterns are harder to match reliably via Claude Code's Bash matcher syntax)

**Note on SQLite guard:** The spec listed separate matchers for `sqlite3`, `DROP TABLE`, and `DELETE FROM`. In practice, `DROP TABLE` and `DELETE FROM` inside a Bash command are harder to match reliably with Claude Code's `Bash(pattern)` syntax since they may be embedded in quoted strings or heredocs. The hook covers the most common attack surface (`sqlite3 *` matcher blocks all direct sqlite3 CLI usage). The rm guard and pre-commit gate use patterns consistent with Claude Code's established permission matcher format (`Bash(pattern)`).
