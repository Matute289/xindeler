# Logging Coverage Implementation Plan (Plan B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Populate all six log sinks established in Plan A with comprehensive game state events. After this plan, Claude can invoke the `veloren-telemetry` skill to fully observe both client and server side after any test session — without needing screenshots or user descriptions.

**Architecture:** A `telemetry!` helper macro wraps `trace!(target: "telemetry", ...)` for brevity. A new `TelemetrySystem` ECS system emits periodic snapshots (player state, world context, entity context) every N ticks. Additional calls are added directly at existing event handling sites across `server/`, `common/systems/`, `client/`, and `voxygen/`. In singleplayer mode, all telemetry from all crates lands in a single `client_telemetry.jsonl`.

**Prerequisite:** Plan A (logging infrastructure) must be complete. The sinks must exist before we can instrument code.

**Tech Stack:** `tracing` crate (already a workspace dep for all crates), ECS (`specs`).

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Modify | `common/src/lib.rs` | Add `telemetry!` macro |
| Create | `common/systems/src/telemetry.rs` | `TelemetrySystem` — periodic ps/wc/ec snapshots |
| Modify | `common/systems/src/lib.rs` | Register `TelemetrySystem` |
| Modify | `server/src/lib.rs` | Session start/end, connect/disconnect, slow tick, entity count |
| Modify | `server/src/events/entity_manipulation.rs` | Player death (pd), damage hits (ch) |
| Modify | `server/src/events/inventory_manip.rs` | Inventory changes (inv) |
| Modify | `server/src/events/trade.rs` | Trade completion (trade) |
| Modify | `server/src/persistence/character_loader.rs` | Persistence errors (err_ctx) |
| Modify | `common/systems/src/melee.rs` | Melee combat hits (ch) |
| Modify | `common/systems/src/projectile.rs` | Ranged combat hits (ch) |
| Modify | `server/src/sys/agent/mod.rs` | NPC state changes (npc) |
| Modify | `client/src/lib.rs` | Network connect/disconnect/error (net) |
| Modify | `voxygen/src/main.rs` | Session start/end (ss/se) |
| Modify | `voxygen/src/scene/terrain.rs` | Chunk load/unload (co) |
| Modify | `voxygen/src/hud/esc_menu.rs` | UI events (ui) for Plan A's `telemetry!` target |

---

### Task 1: `telemetry!` macro

**Files:**
- Modify: `common/src/lib.rs`

A macro that emits `trace!(target: "telemetry", ...)` events. All game code depends on `common`, so the macro is universally available.

- [ ] **Step 1: Add macro to `common/src/lib.rs`**

Open `common/src/lib.rs`. Near the top, after the `#![...]` attributes and before the `pub mod` declarations, add:

```rust
/// Emit a structured telemetry event captured by TelemetryLayer (JSON Lines).
/// Only active when the `logging-verbose` feature is enabled in the entry-point crate.
/// In release builds this compiles to a no-op trace event with no overhead.
///
/// Usage: `telemetry!("ch", attacker = ?uid, damage = dmg, ...)`
/// The first argument is the event type code (see veloren-telemetry skill for schema).
#[macro_export]
macro_rules! telemetry {
    ($t:expr, $($field:tt)*) => {
        ::tracing::trace!(target: "telemetry", t = $t, $($field)*)
    };
}
```

- [ ] **Step 2: Verify it compiles**
```bash
source "$HOME/.cargo/env"
cargo check -p veloren-common 2>&1 | grep "^error" | head -10
```
Expected: no errors.

- [ ] **Step 3: Commit**
```bash
git add common/src/lib.rs
git commit -m "feat(logging): add telemetry! macro to common"
```

---

### Task 2: `TelemetrySystem` — periodic game state snapshots

**Files:**
- Create: `common/systems/src/telemetry.rs`
- Modify: `common/systems/src/lib.rs`

This ECS system runs every 150 ticks (~5 seconds at 30 TPS) and emits:
- `ps` — full player state snapshot
- `wc` — world context (time of day, weather, biome, altitude)
- `ec` — entity context (nearby entities, their state and distance)

It runs on both client (in singleplayer) and server, giving unified visibility.

- [ ] **Step 1: Create `common/systems/src/telemetry.rs`**

```rust
use common::{
    comp::{
        Body, BuffKind, CharacterState, Energy, Health, Ori, PhysicsState, Player, Pos, Stats, Vel,
    },
    resources::{DeltaTime, Time, TimeOfDay},
    terrain::TerrainChunk,
};
use common_ecs::{Job, Origin, Phase, System};
use specs::{Join, Read, ReadExpect, ReadStorage, WriteExpect};
use std::sync::atomic::{AtomicU32, Ordering};

// Emit a snapshot every SNAPSHOT_TICKS game ticks (~5s at 30 TPS)
const SNAPSHOT_TICKS: u32 = 150;
// Emit entity context for entities within this block-radius
const EC_RADIUS_SQ: f32 = 40.0 * 40.0;
// Maximum entities in ec event
const EC_MAX_ENTITIES: usize = 20;

static TICK_COUNTER: AtomicU32 = AtomicU32::new(0);

#[derive(Default)]
pub struct Sys;

#[derive(specs::SystemData)]
pub struct ReadData<'a> {
    time_of_day: Read<'a, TimeOfDay>,
    positions: ReadStorage<'a, Pos>,
    velocities: ReadStorage<'a, Vel>,
    healths: ReadStorage<'a, Health>,
    energies: ReadStorage<'a, Energy>,
    bodies: ReadStorage<'a, Body>,
    players: ReadStorage<'a, Player>,
    char_states: ReadStorage<'a, CharacterState>,
    stats: ReadStorage<'a, Stats>,
}

impl<'a> System<'a> for Sys {
    type SystemData = ReadData<'a>;

    const NAME: &'static str = "telemetry";
    const ORIGIN: Origin = Origin::Common;
    const PHASE: Phase = Phase::Create;

    fn run(_job: &mut Job<Self>, data: Self::SystemData) {
        let tick = TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
        if tick % SNAPSHOT_TICKS != 0 {
            return;
        }

        let tod = data.time_of_day.0;

        // World context snapshot (wc) — emitted once per snapshot interval
        common::telemetry!("wc", tod = tod);

        // Player snapshots (ps) — one per connected player
        for (pos, vel, health, energy, player, char_state) in (
            &data.positions,
            &data.velocities,
            &data.healths,
            &data.energies,
            &data.players,
            &data.char_states,
        )
            .join()
        {
            let hp = health.current() as u32;
            let hp_max = health.maximum() as u32;
            let en = energy.current() as u32;
            let en_max = energy.maximum() as u32;
            let px = pos.0.x;
            let py = pos.0.y;
            let pz = pos.0.z;
            let state = format!("{:?}", char_state);
            let alias = &player.alias;
            common::telemetry!(
                "ps",
                player = alias,
                hp, hp_max, en, en_max,
                px, py, pz,
                state = state
            );

            // Entity context (ec) — entities near this player
            let mut nearby: Vec<(f32, String, u32, u32, String)> = Vec::new();
            for (other_pos, other_health, other_body, other_char_state) in (
                &data.positions,
                &data.healths,
                &data.bodies,
                &data.char_states,
            )
                .join()
            {
                let dist_sq = (other_pos.0 - pos.0).magnitude_squared();
                if dist_sq < EC_RADIUS_SQ && dist_sq > 0.1 {
                    nearby.push((
                        dist_sq.sqrt(),
                        format!("{:?}", other_body.variant()),
                        other_health.current() as u32,
                        other_health.maximum() as u32,
                        format!("{:?}", other_char_state),
                    ));
                }
            }
            nearby.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            nearby.truncate(EC_MAX_ENTITIES);

            // Emit entity context as a JSON-compatible string
            // Format: "dist:kind:hp/hp_max:state" entries joined by "|"
            let ec_str = nearby
                .iter()
                .map(|(d, k, h, hm, s)| format!("{d:.1}:{k}:{h}/{hm}:{s}"))
                .collect::<Vec<_>>()
                .join("|");

            common::telemetry!("ec", player = alias, entities = ec_str);
        }
    }
}
```

Note: `Body::variant()` may not exist — use `format!("{:?}", other_body)` if needed. Adjust field names to match what the compiler reports. The key goal is to emit the events; exact field extraction may need minor adjustment for the actual API.

- [ ] **Step 2: Register `TelemetrySystem` in `common/systems/src/lib.rs`**

Open `common/systems/src/lib.rs`. Find where other systems are declared and added:
```bash
grep -n "pub mod\|add_system\|dispatcher" common/systems/src/lib.rs | head -20
```

Add module declaration:
```rust
pub mod telemetry;
```

Find the `add_systems` or dispatcher builder function and add the telemetry system alongside existing systems:
```rust
.with(telemetry::Sys::default(), telemetry::Sys::NAME, &[])
```

The telemetry system has no dependencies so the dependency list is empty.

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-common-systems 2>&1 | grep "^error" | head -20
```
Fix any missing import errors by adding the appropriate `use` statements. `common::telemetry!` is available since `common` is already a dep of `common-systems`.

- [ ] **Step 4: Commit**
```bash
git add common/systems/src/telemetry.rs common/systems/src/lib.rs
git commit -m "feat(logging): add TelemetrySystem for periodic ps/wc/ec snapshots"
```

---

### Task 3: Server — session, connect/disconnect, slow tick

**Files:**
- Modify: `server/src/lib.rs`

Key lines:
- Line 779: `pub fn tick(...)` — slow tick detection with `TickStart`
- Line 1192: `let end_of_server_tick = ...`
- Line 1291: `"Emitting client disconnect event"` — disconnect site

- [ ] **Step 1: Add session start telemetry**

In `server/src/lib.rs`, find the `pub fn new(...)` or server startup function. After the server is initialized, add:

```rust
common::telemetry!("ss", side = "server", ver = env!("CARGO_PKG_VERSION"));
```

- [ ] **Step 2: Add player connect/disconnect telemetry**

Search for the disconnect log at line ~1291:
```bash
grep -n "client disconnect\|ClientDisconnect\|player.*alias\|alias.*player" server/src/lib.rs | head -20
```

When a player disconnects, add:
```rust
info!(player = %alias, entity = ?uid, reason = ?reason, "Player disconnected");
common::telemetry!("pd_conn", event = "disconnect", player = alias, reason = ?reason);
```

When a player connects (find `PlayerListUpdate::Add` or similar):
```rust
info!(player = %alias, "Player connected");
common::telemetry!("pd_conn", event = "connect", player = alias);
```

- [ ] **Step 3: Add slow tick detection**

Find `end_of_server_tick` at line ~1192 and the existing tick timing code. Add:
```rust
let tick_ms = before_state_tick.elapsed().as_millis() as u64;
let entity_count = {
    use specs::WorldExt;
    self.state.ecs().entities().join().count() as u32
};
// Every 50 ticks emit a tick perf event
if self.tick_count % 50 == 0 {
    common::telemetry!("tick", ms = tick_ms, entities = entity_count);
}
// Warn on slow ticks (>50ms)
if tick_ms > 50 {
    warn!(tick_ms, entity_count, "Slow server tick");
}
```

Find where `tick_count` is incremented (or add a counter if it doesn't exist).

- [ ] **Step 4: Check compilation**
```bash
cargo check -p veloren-server 2>&1 | grep "^error" | head -20
```

- [ ] **Step 5: Commit**
```bash
git add server/src/lib.rs
git commit -m "feat(logging): add server session/connect/tick telemetry"
```

---

### Task 4: Server — player death, damage, inventory, trade

**Files:**
- Modify: `server/src/events/entity_manipulation.rs`
- Modify: `server/src/events/inventory_manip.rs`
- Modify: `server/src/events/trade.rs`

- [ ] **Step 1: Add player death telemetry in `entity_manipulation.rs`**

Search for the death handling code around line 1036:
```bash
grep -n "player.*died\|DestroyEvent\|health.*0\|death.*player" server/src/events/entity_manipulation.rs | head -10
```

Find the block that handles player death. After the existing logic, add:
```rust
info!(
    player = %player_alias,
    cause = ?death_cause,
    pos = ?position,
    "Player died"
);
common::telemetry!(
    "pd",
    player = player_alias,
    cause = ?death_cause,
    px = position.x, py = position.y, pz = position.z
);
```

- [ ] **Step 2: Add damage telemetry (ch event)**

Search for `HealthChangeEvent` handling:
```bash
grep -n "HealthChangeEvent\|health_change\|fn.*health" server/src/events/entity_manipulation.rs | head -10
```

In the health change handler, add after determining attacker/target:
```rust
common::telemetry!(
    "ch",
    src = ?attacker_uid,
    dst = ?target_uid,
    dmg = change.amount.abs() as u32,
    hp_after = target_health.current() as u32
);
```

- [ ] **Step 3: Add inventory change telemetry in `inventory_manip.rs`**

Find where items are added to inventory (pickup, equip events). Add:
```rust
info!(
    player = %alias,
    item = %item_name,
    op = "pickup",
    "Inventory change"
);
common::telemetry!("inv", player = alias, op = "pickup", item = item_name);
```

For equip events, add:
```rust
common::telemetry!("inv", player = alias, op = "equip", item = item_name);
```

- [ ] **Step 4: Add trade telemetry in `trade.rs`**

Find `TradeResult::Completed` handling:
```bash
grep -n "Completed\|TradeResult\|trade.*result\|finish" server/src/events/trade.rs | head -10
```

After trade completion:
```rust
info!(player = %alias, "Trade completed");
common::telemetry!("trade", player = alias, result = "completed");
```

- [ ] **Step 5: Check compilation**
```bash
cargo check -p veloren-server 2>&1 | grep "^error" | head -20
```

- [ ] **Step 6: Commit**
```bash
git add server/src/events/entity_manipulation.rs server/src/events/inventory_manip.rs server/src/events/trade.rs
git commit -m "feat(logging): add player death, damage, inventory, trade telemetry"
```

---

### Task 5: Combat system — melee and projectile hits

**Files:**
- Modify: `common/systems/src/melee.rs`
- Modify: `common/systems/src/projectile.rs`

Both files are in `common/systems/src/` — path: `/Users/mgrinberg/Workspace/RustroverProjects/veloren/common/systems/src/`.

- [ ] **Step 1: Add combat hit telemetry in `melee.rs`**

Open `common/systems/src/melee.rs`. Find the `fn run(...)` at line 87. Inside the attack processing loop, where damage is applied and `AttackEvent` is emitted, add:

```bash
grep -n "AttackEvent\|emit\|damage\|health.*change\|hit" common/systems/src/melee.rs | head -20
```

After each successful hit:
```rust
common::telemetry!(
    "ch",
    src = ?attacker_uid,
    dst = ?target_uid,
    skill = "melee",
    dmg = damage as u32,
    crit = is_crit
);
debug!(
    attacker = ?attacker_uid,
    target = ?target_uid,
    damage,
    "Melee hit"
);
```

- [ ] **Step 2: Add projectile hit telemetry in `projectile.rs`**

Open `common/systems/src/projectile.rs`. Find where the projectile hits an entity:
```bash
grep -n "AttackEvent\|hit\|damage\|entity.*hit" common/systems/src/projectile.rs | head -10
```

Add after projectile impact:
```rust
common::telemetry!(
    "ch",
    src = ?shooter_uid,
    dst = ?target_uid,
    skill = "projectile",
    dmg = damage as u32
);
```

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-common-systems 2>&1 | grep "^error" | head -20
```

- [ ] **Step 4: Commit**
```bash
git add common/systems/src/melee.rs common/systems/src/projectile.rs
git commit -m "feat(logging): add combat hit telemetry (melee + projectile)"
```

---

### Task 6: NPC AI — behavior state changes

**Files:**
- Modify: `server/src/sys/agent/mod.rs`
- Modify: `server/src/sys/agent/behavior_tree/mod.rs`

The agent system processes NPC behavior each tick. When an NPC transitions between states (Idle → Chase, Chase → Attack, etc.), that's the key event to capture.

- [ ] **Step 1: Identify state transition sites**
```bash
grep -n "hostile\|flee\|patrol\|idle\|chase\|attack\|retreat\|activity\|action\b" server/src/sys/agent/behavior_tree/mod.rs | head -20
grep -n "fn hostile\|fn patrol\|fn flee\|fn idle\|AgentAction\|controller" server/src/sys/agent/behavior_tree/mod.rs | head -20
```

- [ ] **Step 2: Add NPC state change telemetry**

In `server/src/sys/agent/behavior_tree/mod.rs`, at the points where behavior changes (find the `hostile()`, `patrol()`, or equivalent functions), add:

```rust
// When NPC becomes hostile (starts chasing/attacking)
common::telemetry!(
    "npc",
    kind = ?npc_body_kind,
    prev = "idle",
    next = "hostile",
    target = ?target_uid,
    dist = target_distance
);
debug!(npc = ?entity_uid, target = ?target_uid, "NPC became hostile");
```

Look for patterns where the agent's `controller` is updated and wrap those with telemetry calls. The exact field names depend on the actual struct — adjust to compile.

- [ ] **Step 3: Add NPC death/spawn telemetry in agent mod.rs**
```bash
grep -n "despawn\|spawn\|death\|Dead\|remove.*entity" server/src/sys/agent/mod.rs | head -10
```

When NPC is removed or dies:
```rust
common::telemetry!("npc", event = "death", kind = ?npc_body_kind, pos = ?position);
```

- [ ] **Step 4: Check compilation**
```bash
cargo check -p veloren-server 2>&1 | grep "^error" | head -20
```

- [ ] **Step 5: Commit**
```bash
git add server/src/sys/agent/mod.rs server/src/sys/agent/behavior_tree/mod.rs
git commit -m "feat(logging): add NPC AI state change telemetry"
```

---

### Task 7: Persistence errors and performance warnings

**Files:**
- Modify: `server/src/persistence/character_loader.rs`
- Check: `server/src/persistence/` (find save/load error sites)

- [ ] **Step 1: Find persistence error sites**
```bash
grep -rn "Err\b\|error\|failed\|Error::\|unwrap_or_else" server/src/persistence/ --include="*.rs" | grep -v "//\|test" | head -20
```

- [ ] **Step 2: Add error logging to persistence failures**

For each `Err(e)` or `unwrap_or_else` in the persistence layer that represents a save/load failure:
```rust
error!(?e, char_id = ?character_id, "Character save failed");
common::telemetry!("err_ctx", msg = "char_save_failed", char_id = ?character_id);
```

For successful saves (add `info!` at least):
```rust
info!(char_id = ?character_id, "Character saved successfully");
```

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-server 2>&1 | grep "^error" | head -20
```

- [ ] **Step 4: Commit**
```bash
git add server/src/persistence/
git commit -m "feat(logging): add persistence error and success logging"
```

---

### Task 8: Client-side — network events and session start/end

**Files:**
- Modify: `client/src/lib.rs`
- Modify: `voxygen/src/main.rs`

- [ ] **Step 1: Add network event telemetry in `client/src/lib.rs`**

Find where the client connects to the server:
```bash
grep -n "fn connect\|PostBox\|connected\|connection.*success\|error.*connect" client/src/lib.rs | head -10
```

On successful connection:
```rust
info!(server = %server_addr, ping_ms, "Connected to server");
common::telemetry!("net", event = "connect", server = server_addr, ping_ms);
```

On disconnect:
```rust
warn!(reason = ?reason, "Disconnected from server");
common::telemetry!("net", event = "disconnect", reason = ?reason);
```

On network error:
```rust
error!(?e, "Network error");
common::telemetry!("err_ctx", msg = "network_error", error = ?e);
```

- [ ] **Step 2: Add session start/end in `voxygen/src/main.rs`**

After `init_split_logs` and just before the game loop runs, add:
```rust
let game_version = common::util::DISPLAY_VERSION.as_str();
common::telemetry!("ss", ver = game_version, platform = std::env::consts::OS);
info!("Session start: version={game_version}");
```

Near the end of `main()`, just before `cleanup_on_exit`, add:
```rust
common::telemetry!("se", reason = "clean");
info!("Session end");
```

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-client -p veloren-voxygen 2>&1 | grep "^error" | head -20
```

- [ ] **Step 4: Commit**
```bash
git add client/src/lib.rs voxygen/src/main.rs
git commit -m "feat(logging): add client network events and session start/end telemetry"
```

---

### Task 9: Client-side — chunk operations and performance snapshots

**Files:**
- Modify: `voxygen/src/scene/terrain.rs`
- Modify: `voxygen/src/render/mod.rs` or `voxygen/src/lib.rs` (wherever frame time is tracked)

- [ ] **Step 1: Add chunk load/unload telemetry in `voxygen/src/scene/terrain.rs`**

Find where chunks are added and removed from the scene:
```bash
grep -n "insert.*chunk\|remove.*chunk\|chunk.*load\|chunk.*unload\|mesh.*complete" voxygen/src/scene/terrain.rs | head -10
```

On chunk loaded:
```rust
let load_ms = chunk_mesh_start.elapsed().as_millis() as u32;
debug!(chunk = ?chunk_pos, ms = load_ms, "Chunk loaded");
common::telemetry!("co", op = "load", cx = chunk_pos.x, cy = chunk_pos.y, ms = load_ms);
```

On chunk removed:
```rust
debug!(chunk = ?chunk_pos, "Chunk unloaded");
common::telemetry!("co", op = "unload", cx = chunk_pos.x, cy = chunk_pos.y);
```

- [ ] **Step 2: Add performance snapshot telemetry**

Find where FPS and frame time are computed (look for existing FPS display logic):
```bash
grep -rn "fps\|frame_time\|frame.*duration\|dt\b" voxygen/src/lib.rs voxygen/src/run.rs 2>/dev/null | grep -i "fps\|frame" | head -15
```

Every ~30 frames (use a counter mod 30), emit:
```rust
static PERF_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
if PERF_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 30 == 0 {
    let fps = (1.0 / dt.as_secs_f64()) as u32;
    let frame_ms = dt.as_millis() as u32;
    common::telemetry!("perf", fps, frame_ms);
    if frame_ms > 33 {
        debug!(fps, frame_ms, "Low FPS frame");
    }
}
```

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -20
```

- [ ] **Step 4: Commit**
```bash
git add voxygen/src/scene/terrain.rs
git commit -m "feat(logging): add chunk op and performance telemetry"
```

---

### Task 10: UI events

**Files:**
- Modify: `voxygen/src/hud/esc_menu.rs`
- Modify: `voxygen/src/hud/mod.rs` (for map, inventory, crafting opens)

The HUD widgets return events from their `update()` functions. Add telemetry at the call sites in `mod.rs` where those events are handled.

- [ ] **Step 1: Add esc menu telemetry in `voxygen/src/hud/esc_menu.rs`**

Find where each button press is detected (lines ~85–176 have the `Button::image` blocks). After each `if Button::... { ... .set(state.ids.menu_button_N, ui)` block, the return value indicates a press. At each press handling site in the parent `mod.rs`, add:

```bash
grep -n "esc_menu\|EscMenu\|menu_button\|MainMenu\|Logout\|Quit\|Settings" voxygen/src/hud/mod.rs | head -20
```

When a button press is handled:
```rust
common::telemetry!("ui", action = "click", widget = "EscMenu", btn = "Settings");
common::telemetry!("ui", action = "click", widget = "EscMenu", btn = "Logout");
```

- [ ] **Step 2: Add inventory/map open telemetry in `voxygen/src/hud/mod.rs`**

Find where inventory, map, and crafting are toggled:
```bash
grep -n "show.*inventory\|toggle.*map\|open.*bag\|show_bag\|show_map" voxygen/src/hud/mod.rs | head -10
```

On toggle:
```rust
common::telemetry!("ui", action = "open", widget = "Inventory");
common::telemetry!("ui", action = "open", widget = "Map");
```

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -20
```

- [ ] **Step 4: Commit**
```bash
git add voxygen/src/hud/esc_menu.rs voxygen/src/hud/mod.rs
git commit -m "feat(logging): add UI event telemetry"
```

---

### Task 11: Integration verification

- [ ] **Step 1: Build with logging-verbose (singleplayer)**
```bash
source "$HOME/.cargo/env"
cargo build --bin veloren-voxygen \
  --features "veloren-voxygen/terrain-hires,veloren-voxygen/logging-verbose" 2>&1 | tail -5
```
Expected: `Finished dev profile`.

- [ ] **Step 2: Run singleplayer for 60 seconds**

Launch the game, enter singleplayer, walk around, attack an NPC, open inventory, open map, then quit.

- [ ] **Step 3: Verify telemetry file has events from all categories**
```bash
# Session start/end
grep '"t":"ss"\|"t":"se"' userdata/voxygen/logs/*telemetry*.jsonl

# Player snapshots
grep -c '"t":"ps"' userdata/voxygen/logs/*telemetry*.jsonl

# Entity context
grep -c '"t":"ec"' userdata/voxygen/logs/*telemetry*.jsonl

# Performance
grep -c '"t":"perf"' userdata/voxygen/logs/*telemetry*.jsonl

# Chunk ops
grep -c '"t":"co"' userdata/voxygen/logs/*telemetry*.jsonl

# UI events
grep '"t":"ui"' userdata/voxygen/logs/*telemetry*.jsonl | head -5

# NPC events (if NPCs were near)
grep '"t":"npc"' userdata/voxygen/logs/*telemetry*.jsonl | head -5
```

Expected: each category has at least 1 event.

- [ ] **Step 4: Validate all telemetry lines are valid JSON**
```bash
python3 -c "
import json, glob, sys
files = glob.glob('userdata/voxygen/logs/*telemetry*.jsonl')
total, errors = 0, 0
for f in files:
    with open(f) as fh:
        for i, line in enumerate(fh):
            total += 1
            try:
                json.loads(line)
            except json.JSONDecodeError as e:
                print(f'ERROR in {f} line {i+1}: {e}')
                errors += 1
print(f'Validated {total} events, {errors} errors')
"
```
Expected: `0 errors`.

- [ ] **Step 5: Run clippy**
```bash
cargo clippy -p veloren-common -p veloren-common-systems -p veloren-server -p veloren-client -p veloren-voxygen -- -D warnings 2>&1 | grep "^error" | head -20
```

- [ ] **Step 6: Push**
```bash
git push origin main
```

---

## Notes for implementer

- If a field name doesn't exist in the actual struct, use the closest equivalent. The telemetry event schema is flexible — just keep the `t` field accurate.
- Some `common::telemetry!` calls in server code will work in singleplayer (same process) and in dedicated server mode (server_telemetry.jsonl). The same macro works everywhere.
- The `TelemetrySystem` in Task 2 runs in both client and server ECS when singleplayer — this is intentional and gives the unified view the user requested.
- After Plan B is complete, invoking the `veloren-telemetry` skill will give Claude full observability of a test session.
