# Engine Improvements Phase 1 (Baselines + Quick Wins) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish measured performance baselines, land the Phase 1 quick wins (telemetry ring buffer, calc_light glow early-out, shader-recreation pool cap), put the unsafe surface under a SAFETY-comment gate, and wire the per-PR review pipeline — every perf change shipping with before/after numbers.

**Architecture:** Measurement-first: each perf task opens with a baseline capture (exact command) and closes by re-measuring, recording numbers in `docs/superpowers/specs/perf-baselines.md` and ultimately in the PR description. The telemetry layer moves from a global `Mutex<BufWriter>` to a bounded channel + dedicated drain thread with buffer recycling (drop-on-full; never backpressures the tick). `calc_light` gets a provably bit-identical early-out for the empty-glow case. Pipeline recreation reuses one capped persistent rayon pool. The physics spatial-grid optimization is **deferred to Phase 2**; Phase 1 only adds its bench (rationale in Task 6). Workstream C lands as a PR template + review-skill extension exercising the two existing reviewer agents.

**Tech Stack:** Rust nightly (2024 edition), criterion 0.8 (workspace dep), tracy (cargo aliases verified in `.cargo/config.toml`), crossbeam-channel (workspace dep), specs ECS. Design spec: `docs/superpowers/specs/2026-06-10-engine-improvements-design.md` (Phase 1: §A1 step 1, §A3, §A4, §B1, §C).

**Conventions for every task:**
- Branch: create `feature/engine-phase1` off `development` before Task 1.
- Tests/benches need the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test|bench -p <crate>`.
- Invoke the `veloren-engine-perf` skill before any perf task and `superpowers:test-driven-development` before writing code.
- **Measurement-first rule:** no perf commit without a baseline captured *before* the change and the delta recorded in `perf-baselines.md`.

---

### Task 1: Baselines file + tracy capture protocol + initial captures

**Files:**
- Create: `docs/superpowers/specs/perf-baselines.md`

- [ ] **Step 1: Verify the measurement toolchain**

```bash
grep -n "tracy-server\|tracy-voxygen\|swarm" .cargo/config.toml
```
Expected: aliases `tracy-server` (L34), `tracy-voxygen` (L40), `swarm` (L43) — verified present on `development`.

```bash
which tracy-capture || brew install tracy
cargo check --bin swarm --features client/bin_bot,client/tick_network
```
Expected: `tracy-capture` on PATH; swarm builds. The spec flags "swarm may not build on this fork" as a Phase 1 risk — if the check fails, fix the swarm bin first (it blocks the server baseline) and note the fix in the PR.

- [ ] **Step 2: Create the baselines file**

Create `docs/superpowers/specs/perf-baselines.md` with exactly:

```markdown
# Engine Performance Baselines

Numbers for the engine-improvements program. Every row records the exact
command, commit hash, and machine. Fill the After column in the same PR
that lands the optimization.

**Machine:** <CPU, cores, RAM, OS — `sysctl -n machdep.cpu.brand_string`>

## Capture protocol

### P1 — Voxygen flight (meshing + frame times)
1. `cargo tracy-voxygen`
2. Second terminal: `tracy-capture -o docs/superpowers/baselines/voxygen-flight-<commit>.tracy -a 127.0.0.1`
3. Singleplayer, default seed, `/site` to the same town each run.
4. `/fly` + hold forward at fixed speed over fresh terrain, 60s, no turning.
5. Record from tracy: `calc_light` span count/mean/p99, `generate_mesh` mean,
   frame time p50/p99.

### P1b — Shader touch (recreation stutter)
As P1, but instead of flying: `touch assets/voxygen/shaders/terrain-frag.glsl`
and capture until recreation completes. Record the worst frame time.

### P2 — Server swarm (tick + physics)
1. `cargo tracy-server` + `tracy-capture -o docs/superpowers/baselines/server-swarm<N>-<commit>.tracy -a 127.0.0.1`
2. `cargo swarm -- -s <N>` with N = 200, then 500; 120s each.
3. Record: tick p50/p99, `phys` span mean, "Construct spatial grid" span mean
   (instrumented at common/systems/src/phys/mod.rs:325).

`.tracy` files live in `docs/superpowers/baselines/` (gitignore if >50MB —
the numbers here are the durable artifact).

## Numbers

### Criterion benches
| Bench | Command | Baseline (commit) | After (commit) | Delta |
|---|---|---|---|---|
| meshing: Terrain mesh 1,1 | `VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench meshing_benchmark` | | | |
| light: sunlight / glow_empty / glow_seeded | `VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench light_benchmark` | | | |
| telemetry_on_event | `cargo bench -p veloren-common-frontend --features logging-verbose` | | | |
| spatial_grid_rebuild/200,500,2000 | `VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-common --bench spatial_grid_benchmark` | | | deferred to Phase 2 |

### Tracy captures
| Scenario | Metric | Baseline (commit) | After (commit) |
|---|---|---|---|
| P1 flight | calc_light p99 / frame p99 | | |
| P1b shader touch | worst frame during recreation | | |
| P2 swarm 200 | tick p99 / phys mean | | |
| P2 swarm 500 | tick p99 / phys mean | | |

### Unsafe census
| Date | Real unsafe sites | With SAFETY comment |
|---|---|---|
| 2026-06-11 | 13 | 4 (pre-gate) |
```

- [ ] **Step 3: Run P1, P1b, and P2 on the unmodified branch**

Fill the tracy Baseline cells and the `meshing_benchmark` row (that bench exists today). The other bench rows are filled by Tasks 2/4/6 when those benches land. Use the current commit hash in every cell.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/perf-baselines.md
git commit -m "docs: perf baselines file with tracy capture protocol and initial numbers"
```

---

### Task 2: `calc_light` micro-bench

**Files:**
- Modify: `voxygen/src/mesh/terrain.rs:36` (export `calc_light`)
- Create: `voxygen/benches/light_benchmark.rs`
- Modify: `voxygen/Cargo.toml` (after the `[[bench]]` block at L170–172)

- [ ] **Step 1: Export `calc_light`**

At `voxygen/src/mesh/terrain.rs:36`, change `fn calc_light<` to:

```rust
/// Public for criterion benches (`voxygen/benches/light_benchmark.rs`); not
/// intended as API — call through `generate_mesh`.
pub fn calc_light<
```

- [ ] **Step 2: Add the bench**

Create `voxygen/benches/light_benchmark.rs`. The world/terrain setup is copied **verbatim** from the existing harness, `voxygen/benches/meshing_benchmark.rs` lines 14–49 (rayon pool, `World::generate` with `DEFAULT_WORLD_SEED`/`DEFAULT_WORLD_MAP`, `TerrainGrid::new`, generate+insert the `GEN_SIZE×GEN_SIZE` chunks at `CENTER`), with `const GEN_SIZE: i32 = 3;`. Then:

```rust
use common::{terrain::TerrainGrid, vol::SampleVol};
use criterion::{Criterion, criterion_group, criterion_main};
use std::{hint::black_box, sync::Arc};
use vek::*;
use veloren_voxygen::mesh::terrain::{MAX_LIGHT_DIST, SUNLIGHT, calc_light};
use world::{World, sim};

const CENTER: Vec2<i32> = Vec2 { x: 512, y: 512 };
const GEN_SIZE: i32 = 3;

pub fn criterion_benchmark(c: &mut Criterion) {
    // ... setup copied verbatim from meshing_benchmark.rs L14-49 ...

    // Sample chunk (1,1) + 1-block borders, same math as meshing_benchmark L51-79
    let chunk_pos = Vec2::new(1, 1) + CENTER;
    let aabr = Aabr {
        min: chunk_pos.map2(TerrainGrid::chunk_size(), |e, sz| e * sz as i32 - 1),
        max: chunk_pos.map2(TerrainGrid::chunk_size(), |e, sz| (e + 1) * sz as i32 + 1),
    };
    let volume = terrain.sample(aabr).unwrap();
    let min_z = volume
        .iter()
        .fold(i32::MAX, |min, (_, chunk)| chunk.get_min_z().min(min));
    let max_z = volume
        .iter()
        .fold(i32::MIN, |max, (_, chunk)| chunk.get_max_z().max(max));
    let range = Aabb {
        min: Vec3::from(aabr.min) + Vec3::unit_z() * (min_z - 1),
        max: Vec3::from(aabr.max) + Vec3::unit_z() * (max_z + 1),
    };

    // 16 synthetic glow seeds spread through the chunk interior
    let glow_seeds: Vec<(Vec3<i32>, u8)> = (0..16)
        .map(|i| {
            let off = Vec3::new(
                MAX_LIGHT_DIST + (i % 4) * 6,
                MAX_LIGHT_DIST + (i / 4) * 6,
                range.size().d / 2,
            );
            (range.min + off, 10)
        })
        .collect();

    let mut group = c.benchmark_group("light");
    group.sample_size(20);
    group.bench_function("sunlight", |b| {
        b.iter(|| {
            black_box(calc_light(true, SUNLIGHT, black_box(range), &volume, core::iter::empty()))
        })
    });
    group.bench_function("glow_empty", |b| {
        b.iter(|| {
            black_box(calc_light(false, 0, black_box(range), &volume, core::iter::empty()))
        })
    });
    group.bench_function("glow_seeded", |b| {
        b.iter(|| {
            black_box(calc_light(false, 0, black_box(range), &volume, glow_seeds.iter().copied()))
        })
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
```

In `voxygen/Cargo.toml`, directly after the `meshing_benchmark` block (L170–172):

```toml
[[bench]]
harness = false
name = "light_benchmark"
```

- [ ] **Step 3: Run and record the baseline**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench light_benchmark
```
Expected: three results; `glow_empty` is non-trivial today (hundreds of µs — it pays two full-volume allocations plus the minimization copy loop at `terrain.rs:58` and `:196`). Record all three in `perf-baselines.md` with the commit hash.

- [ ] **Step 4: Commit**

```bash
git add voxygen/benches/light_benchmark.rs voxygen/Cargo.toml voxygen/src/mesh/terrain.rs docs/superpowers/specs/perf-baselines.md
git commit -m "bench: calc_light micro-bench (sunlight, empty glow, seeded glow)"
```

---

### Task 3: A1 quick win — `calc_light` empty-glow early-out

**Files:**
- Modify: `voxygen/src/mesh/terrain.rs:36-225` (`calc_light`)

**Verified claim:** the glow pass (`generate_mesh` at terrain.rs:278, `is_sunlight == false`, `default_light == 0`) with an empty `glow_blocks` still allocates the full-volume `light_map` (L58), the minimized `light_map2` (L196), and runs the copy loop (L201–208). With no seeds and no sun rays every cell stays `UNKNOWN`, so the closure returns `0.0` for every in-bounds query, and `default_light == 0 → 0.0` out-of-bounds. An **empty** `light_map2` (`.get()` always `None → default_light == 0 → 0.0`) is therefore bit-identical for every input. Guards: `!is_sunlight` (the sunlight pass seeds itself from sun rays even with an empty iterator) and `default_light == 0` (in-bounds `UNKNOWN` mapped to `0.0`; only coincides with `default_light` when it is 0).

- [ ] **Step 1: Restructure `calc_light`**

Keep a single returned closure (the return type is one `impl Fn`; two closure types would not compile). Hoist `min_bounds`/`lm_idx2`, wrap the existing body in an `else`. Only marked lines are new; the BFS (current L56–187) and minimization loop (current L201–208) move inside the `else` **unchanged**:

```rust
    span!(_guard, "calc_light");
    const UNKNOWN: u8 = 255;
    const OPAQUE: u8 = 254;

    let outer = Aabb {
        min: bounds.min - Vec3::new(SUNLIGHT as i32, SUNLIGHT as i32, 1),
        max: bounds.max + Vec3::new(SUNLIGHT as i32, SUNLIGHT as i32, 1),
    };

    // NEW: moved up from below the BFS (was at current L189-200)
    let min_bounds = Aabb {
        min: bounds.min - 1,
        max: bounds.max + 1,
    };
    let lm_idx2 = {
        let (w, h, _) = min_bounds.clone().size().into_tuple();
        move |x, y, z| (w * h * z + h * x + y) as usize
    };

    // NEW: early-out. A non-sunlight pass with no seed blocks can never light
    // anything: every cell would stay UNKNOWN, which the closure below maps
    // to 0.0 — exactly what an empty light map yields via
    // `.get(..) == None → default_light (== 0) → 0.0`. Bit-identical output;
    // skips both full-volume allocations, the BFS, and the minimization copy.
    let mut lit_blocks = lit_blocks.peekable();
    let light_map2 = if !is_sunlight && default_light == 0 && lit_blocks.peek().is_none() {
        Vec::new()
    } else {
        let mut vol_cached = vol.cached();

        let mut light_map = vec![UNKNOWN; outer.size().product() as usize];
        let lm_idx = {
            let (w, h, _) = outer.clone().size().into_tuple();
            move |x, y, z| (w * h * z + h * x + y) as usize
        };
        // ... existing code UNCHANGED from current L63 (`// Light propagation
        // queue`) through L187 (end of the BFS `while let` loop) ...

        let mut light_map2 = vec![UNKNOWN; min_bounds.size().product() as usize];
        for z in 0..min_bounds.size().d {
            for x in 0..min_bounds.size().w {
                for y in 0..min_bounds.size().h {
                    let off = min_bounds.min - outer.min;
                    light_map2[lm_idx2(x, y, z)] =
                        light_map[lm_idx(x + off.x, y + off.y, z + off.z)];
                }
            }
        }
        light_map2
    };

    move |wpos| {
        let pos = wpos - min_bounds.min;
        let l = light_map2
            .get(lm_idx2(pos.x, pos.y, pos.z))
            .copied()
            .unwrap_or(default_light);

        if l != OPAQUE && l != UNKNOWN {
            l as f32 * SUNLIGHT_INV
        } else {
            0.0
        }
    }
```

Delete the now-redundant `drop(light_map);` (it dies at the end of the `else`) and the old `min_bounds`/`lm_idx2` definitions.

- [ ] **Step 2: Verify build and tests**

```bash
cargo check -p veloren-voxygen
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-voxygen
```
Expected: clean / PASS.

- [ ] **Step 3: Re-measure and record**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench light_benchmark
VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench meshing_benchmark
```
Expected: `glow_empty` drops to nanoseconds; `sunlight`/`glow_seeded` within noise (their path is unchanged code); the meshing bench improves for chunks without glow blocks. Record After numbers in `perf-baselines.md`. The golden-mesh hash harness is Phase 2; for Phase 1 the equivalence argument lives in the code comment, plus a manual flight smoke (`veloren-run` skill — verify caves/lava still glow, surface lighting unchanged).

- [ ] **Step 4: Commit**

```bash
git add voxygen/src/mesh/terrain.rs docs/superpowers/specs/perf-baselines.md
git commit -m "perf: skip light-map allocations in calc_light when glow pass has no seeds"
```

---

### Task 4: A4 — telemetry ring buffer + zero-alloc serialization

**Files:**
- Create: `common/frontend/benches/telemetry_benchmark.rs` (baseline FIRST, on the old mutex implementation)
- Rewrite: `common/frontend/src/telemetry_layer.rs` (currently 108 lines: `Arc<Mutex<BufWriter<File>>>` locked on the emitting thread at L51, per-field `format!` temporaries in `JsonVisitor` L70–107)
- Modify: `common/frontend/src/lib.rs:288-289` (layer construction) and `:41-46` (`LogGuards`)
- Modify: `common/frontend/Cargo.toml`

**Design:** `on_event` serializes into a recycled `String` (escape-on-the-fly, no temporaries) and `try_send`s over a bounded channel — drop-on-full with an `AtomicU64` counter, so telemetry can never backpressure the tick. A `telemetry-drain` thread owns the `BufWriter`, flushes on a 250ms idle tick, returns drained buffers through a recycle channel (zero allocation steady-state), and writes a `telemetry_dropped` line whenever the counter advanced.

- [ ] **Step 1: Add the bench and capture the baseline on the OLD code**

Append to `common/frontend/Cargo.toml`:

```toml
[dev-dependencies]
criterion = { workspace = true }

[[bench]]
name = "telemetry_benchmark"
harness = false
required-features = ["logging-verbose"]
```

Create `common/frontend/benches/telemetry_benchmark.rs`:

```rust
use criterion::{Criterion, criterion_group, criterion_main};
use tracing_subscriber::prelude::*;
use veloren_common_frontend::TelemetryLayer;

pub fn criterion_benchmark(c: &mut Criterion) {
    let dir = std::env::temp_dir().join(format!("veloren-telemetry-bench-{}", std::process::id()));
    let layer = TelemetryLayer::new(&dir, "bench").expect("create telemetry file");
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    // Mirrors a combat telemetry!() call site (cf. common/systems/src/melee.rs)
    c.bench_function("telemetry_on_event", |b| {
        b.iter(|| {
            tracing::info!(
                target: "telemetry",
                event = "melee_hit",
                attacker = 42_u64,
                damage = 23.456_f64,
                ability = "sword_basic \"combo\"",
            );
        })
    });
    drop(_guard);
    let _ = std::fs::remove_dir_all(&dir);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
```

Run and record in `perf-baselines.md` (MUST happen before the rewrite below):

```bash
cargo bench -p veloren-common-frontend --features logging-verbose
git add common/frontend/benches/telemetry_benchmark.rs common/frontend/Cargo.toml docs/superpowers/specs/perf-baselines.md
git commit -m "bench: telemetry on_event criterion bench (baseline on mutex implementation)"
```

- [ ] **Step 2: Write the failing test**

At the end of `common/frontend/src/telemetry_layer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;

    #[test]
    fn events_reach_disk_in_order_with_escaping() {
        let dir = std::env::temp_dir()
            .join(format!("veloren-telemetry-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let layer = TelemetryLayer::new(&dir, "test").expect("layer");
        let flush = layer.flush_handle();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            for i in 0..100u64 {
                tracing::info!(
                    target: "telemetry",
                    event = "test_event",
                    seq = i,
                    label = "with \"quotes\" and\nnewline",
                );
            }
            // Different target: must NOT be written
            tracing::info!(other = true, "ordinary log line");
        });
        flush.flush();

        let path = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .expect("telemetry .jsonl exists");
        let content = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 100, "all telemetry events written, nothing else");
        for (i, line) in lines.iter().enumerate() {
            assert!(line.starts_with("{\"ts\":\""), "bad start: {line}");
            assert!(line.ends_with('}'), "bad end: {line}");
            assert!(line.contains(&format!("\"seq\":{i}")), "order: {line}");
            assert!(line.contains(r#"with \"quotes\" and\nnewline"#), "escaping: {line}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

Run: `cargo test -p veloren-common-frontend --features logging-verbose telemetry`
Expected: FAIL to compile with "no method named `flush_handle`".

- [ ] **Step 3: Implement the rewrite**

Add to `common/frontend/Cargo.toml` `[dependencies]`:

```toml
crossbeam-channel = { workspace = true }
```

Replace everything above the test module in `common/frontend/src/telemetry_layer.rs` with:

```rust
use chrono::Utc;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use std::{
    fmt::Write as _,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

/// Max queued lines; beyond this events are dropped (and counted) rather
/// than blocking the emitting thread. 1024 × ~100B lines ≈ 100KB worst case.
const QUEUE_CAPACITY: usize = 1024;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

enum Msg {
    Line(String),
    /// Flush the writer and ack — used by tests and the shutdown path.
    Flush(Sender<()>),
}

pub struct TelemetryLayer {
    tx: Sender<Msg>,
    recycle_rx: Receiver<String>,
    dropped: Arc<AtomicU64>,
}

/// Cloneable handle to flush the drain thread (held by `LogGuards` so the
/// last batch is written on shutdown).
#[derive(Clone)]
pub struct TelemetryFlushHandle {
    tx: Sender<Msg>,
}

impl TelemetryFlushHandle {
    /// Blocks until everything queued before this call is on disk (5s
    /// timeout so a dead drain thread cannot hang process exit).
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = bounded(1);
        if self.tx.send(Msg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

impl TelemetryLayer {
    /// Returns `None` if the file cannot be created (logs a warning, doesn't
    /// panic).
    pub fn new(logs_dir: &Path, prefix: &str) -> Option<Self> {
        let _ = fs::create_dir_all(logs_dir);
        let bucket = Utc::now().format("%Y-%m-%d_%Hh").to_string();
        let name = format!("{bucket}_{prefix}_telemetry.jsonl");
        let file = match File::create(logs_dir.join(&name)) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[log] Failed to create telemetry file {name}: {e}");
                return None;
            },
        };

        let (tx, rx) = bounded::<Msg>(QUEUE_CAPACITY);
        let (recycle_tx, recycle_rx) = bounded::<String>(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_for_drain = Arc::clone(&dropped);
        if let Err(e) = thread::Builder::new()
            .name("telemetry-drain".into())
            .spawn(move || drain(rx, recycle_tx, BufWriter::new(file), dropped_for_drain))
        {
            eprintln!("[log] Failed to spawn telemetry drain thread: {e}");
            return None;
        }

        Some(Self { tx, recycle_rx, dropped })
    }

    pub fn flush_handle(&self) -> TelemetryFlushHandle {
        TelemetryFlushHandle { tx: self.tx.clone() }
    }

    /// Events dropped because the queue was full (visible failure mode).
    pub fn dropped_events(&self) -> u64 { self.dropped.load(Ordering::Relaxed) }
}

fn drain(
    rx: Receiver<Msg>,
    recycle_tx: Sender<String>,
    mut writer: BufWriter<File>,
    dropped: Arc<AtomicU64>,
) {
    let mut reported_dropped = 0u64;
    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(Msg::Line(line)) => {
                let _ = writer.write_all(line.as_bytes());
                // Return the buffer for reuse; drop it if the pool is full.
                let _ = recycle_tx.try_send(line);
            },
            Ok(Msg::Flush(ack)) => {
                let _ = writer.flush();
                let _ = ack.send(());
            },
            Err(RecvTimeoutError::Timeout) => {
                let total = dropped.load(Ordering::Relaxed);
                if total > reported_dropped {
                    let ts = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
                    let _ = writeln!(
                        writer,
                        "{{\"ts\":\"{ts}\",\"event\":\"telemetry_dropped\",\"total_dropped\":{total}}}"
                    );
                    reported_dropped = total;
                }
                let _ = writer.flush();
            },
            Err(RecvTimeoutError::Disconnected) => {
                let _ = writer.flush();
                return;
            },
        }
    }
}

impl<S: Subscriber> Layer<S> for TelemetryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "telemetry" {
            return;
        }

        // Reuse a drained buffer when available (zero allocation steady
        // state); only the first QUEUE_CAPACITY events allocate.
        let mut line = self.recycle_rx.try_recv().unwrap_or_default();
        line.clear();

        let ts = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let _ = write!(line, "{{\"ts\":\"{ts}\"");
        event.record(&mut JsonVisitor { line: &mut line });
        line.push_str("}\n");

        // Never block the emitting (tick) thread: drop on full and count.
        if self.tx.try_send(Msg::Line(line)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct JsonVisitor<'a> {
    line: &'a mut String,
}

/// `fmt::Write` adapter that JSON-escapes as it writes — lets `record_debug`
/// format directly into the line buffer with no intermediate `String`.
struct EscapingWriter<'a>(&'a mut String);

impl std::fmt::Write for EscapingWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for c in s.chars() {
            match c {
                '"' => self.0.push_str("\\\""),
                '\\' => self.0.push_str("\\\\"),
                '\n' => self.0.push_str("\\n"),
                c if c.is_control() => write!(self.0, "\\u{:04x}", c as u32)?,
                c => self.0.push(c),
            }
        }
        Ok(())
    }
}

impl Visit for JsonVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let _ = write!(self.line, ",\"{}\":\"", field.name());
        let _ = EscapingWriter(self.line).write_str(value);
        self.line.push('"');
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.line, ",\"{}\":\"", field.name());
        let _ = write!(EscapingWriter(self.line), "{value:?}");
        self.line.push('"');
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        let _ = write!(self.line, ",\"{}\":{}", field.name(), value);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let _ = write!(self.line, ",\"{}\":{}", field.name(), value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        let _ = write!(self.line, ",\"{}\":{:.3}", field.name(), value);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        let _ = write!(self.line, ",\"{}\":{}", field.name(), value);
    }
}
```

(PR note: control chars other than `\n` are now `\uXXXX`-escaped — strictly more valid JSON; `\n` keeps `\\n` so `veloren-telemetry` skill parsing is unaffected.)

- [ ] **Step 4: Flush on shutdown via `LogGuards`**

In `common/frontend/src/lib.rs`, line 10 becomes:

```rust
pub use telemetry_layer::{TelemetryFlushHandle, TelemetryLayer};
```

Add to `LogGuards` (after `_compress_guards`, L45) and below the struct:

```rust
    #[cfg(feature = "logging-verbose")]
    telemetry_flush: Option<TelemetryFlushHandle>,
}

impl Drop for LogGuards {
    fn drop(&mut self) {
        #[cfg(feature = "logging-verbose")]
        if let Some(h) = &self.telemetry_flush {
            h.flush();
        }
    }
}
```

In `init_split_logs`, replace the telemetry construction (L287–289):

```rust
    #[cfg(feature = "logging-verbose")]
    let (telemetry_layer, telemetry_flush): (
        Option<Box<dyn tracing_subscriber::Layer<_> + Send + Sync>>,
        Option<TelemetryFlushHandle>,
    ) = match TelemetryLayer::new(logs_dir, prefix) {
        Some(t) => {
            let h = t.flush_handle();
            (Some(t.boxed()), Some(h))
        },
        None => (None, None),
    };
```

and add `#[cfg(feature = "logging-verbose")] telemetry_flush,` to the `LogGuards { .. }` literal at the end of the function.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p veloren-common-frontend --features logging-verbose telemetry
cargo check -p veloren-common-frontend
cargo check -p veloren-common-frontend --features logging-verbose,tracy
```
Expected: 1 test PASS; all feature combinations compile.

- [ ] **Step 6: Re-measure and record**

```bash
cargo bench -p veloren-common-frontend --features logging-verbose
```
Expected: `telemetry_on_event` improves vs the Step 1 baseline (no lock, no per-field temporaries; remaining cost is timestamp formatting + channel send). Record After in `perf-baselines.md`. Functional smoke: run a short logging-verbose session and parse the `.jsonl` with the `veloren-telemetry` skill.

- [ ] **Step 7: Commit**

```bash
git add common/frontend docs/superpowers/specs/perf-baselines.md
git commit -m "perf: telemetry layer drains via bounded channel, zero-alloc serialization"
```

---

### Task 5: A3 — shader recreation pool: persistent + capped

**Files:**
- Modify: `voxygen/src/render/renderer/pipeline_creation.rs:1060-1065` — verified: `recreate_pipelines` builds a **fresh** all-cores rayon pool per recreation. Initial creation (L982) stays as-is: startup wants all cores.

- [ ] **Step 1: Baseline capture**

Run protocol **P1b** from `perf-baselines.md` (tracy-voxygen + `touch assets/voxygen/shaders/terrain-frag.glsl`). Record the worst frame time during recreation (expected today: multi-frame stutter — shaderc saturates every core).

- [ ] **Step 2: Implement**

In `pipeline_creation.rs` add `use std::sync::OnceLock;` to the imports and, above `recreate_pipelines`:

```rust
/// Persistent thread pool for background pipeline recreation. Capped to
/// roughly half the logical cores so shaderc compilation does not starve
/// the render/main threads during dev-loop shader edits (recreation is a
/// background job — longer wall time is fine, dropped frames are not).
/// Built once and reused: recreations are frequent in dev and pool
/// construction itself spawns threads.
fn recreation_pool() -> &'static Arc<rayon::ThreadPool> {
    static POOL: OnceLock<Arc<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| (n.get() / 2).saturating_sub(1))
            .max(1);
        Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|n| format!("pipeline-recreation-{}", n))
                .build()
                .expect("failed to build pipeline recreation thread pool"),
        )
    })
}
```

In `recreate_pipelines`, replace L1060–1065:

```rust
    // Create threadpool for parallel portion
    let pool = rayon::ThreadPoolBuilder::new()
        .thread_name(|n| format!("pipeline-recreation-{}", n))
        .build()
        .unwrap();
    let pool = Arc::new(pool);
```

with:

```rust
    // Reuse the persistent capped threadpool for the parallel portion
    let pool = Arc::clone(recreation_pool());
```

The existing deferral logic (`recreation_pending`, `renderer/mod.rs:1300-1303`) is untouched; two recreations sharing one pool serialize naturally.

- [ ] **Step 3: Verify and re-measure**

```bash
cargo check -p veloren-voxygen
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
```
Expected: clean (the function is not hot-reload-gated; it must compile in publish builds).

Repeat P1b. Acceptance (spec §A3): no frame >33ms during recreation on the dev machine. Record before/after in `perf-baselines.md`.

- [ ] **Step 4: Commit**

```bash
git add voxygen/src/render/renderer/pipeline_creation.rs docs/superpowers/specs/perf-baselines.md
git commit -m "perf: persistent capped rayon pool for shader pipeline recreation"
```

---

### Task 6: A2 physics — bench only; optimization explicitly deferred

**Honesty note (why deferred):** the real fix — incremental grid maintenance keyed off `PreviousPhysCache` with deletion handling and a dirty-ratio fallback — is well over 50 lines of correctness-critical code shared by client and server (prediction-divergence risk), and the spec schedules it for Phase 2 behind a grid-equivalence debug assertion. Phase 1 ships only the measurement harness. Tracy spans already exist on **both** rebuilds (`span!` at `common/systems/src/phys/mod.rs:325` "Construct spatial grid" and `:571` "Construct voxel collider spatial grid" — verified, no new spans needed; the spec's implication that instrumentation was missing is corrected).

**Files:**
- Create: `common/benches/spatial_grid_benchmark.rs`
- Modify: `common/Cargo.toml` (after the `loot_benchmark` `[[bench]]` block, L111–113)

- [ ] **Step 1: Add the bench**

Create `common/benches/spatial_grid_benchmark.rs` (`specs` and `vek` are regular deps of `veloren-common`, so benches may use them; import style matches `chonk_benchmark.rs`):

```rust
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use specs::{Builder, World, WorldExt};
use std::hint::black_box;
use vek::*;
use veloren_common::util::SpatialGrid;

/// Mirrors phys's per-tick full rebuild (`construct_spatial_grid`,
/// common/systems/src/phys/mod.rs:324): same cell parameters, one insert per
/// entity. Baseline for the Phase 2 incremental-grid work.
pub fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("spatial_grid_rebuild");
    for &n in &[200usize, 500, 2000] {
        let mut world = World::new();
        let entities: Vec<specs::Entity> =
            (0..n).map(|_| world.create_entity().build()).collect();
        // Deterministic pseudo-random positions in a 1024x1024 region (a
        // busy town); radius 2 ≈ humanoid scaled_radius + truncation error.
        let positions: Vec<Vec2<i32>> = (0..n as i32)
            .map(|i| Vec2::new((i * 7919) % 1024, (i * 104729) % 1024))
            .collect();

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // Parameters from phys/mod.rs:340-342
                let mut grid = SpatialGrid::new(5, 6, 8);
                for (entity, pos) in entities.iter().zip(positions.iter()) {
                    grid.insert(*pos, 2, *entity);
                }
                black_box(&grid);
            })
        });
    }
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
```

In `common/Cargo.toml` after L111–113:

```toml
[[bench]]
name = "spatial_grid_benchmark"
harness = false
```

- [ ] **Step 2: Run and record baseline**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-common --bench spatial_grid_benchmark
```
Expected: three results scaling ~linearly with N. Record in `perf-baselines.md` marked "deferred to Phase 2". Confirm the Task 1 P2 captures include the "Construct spatial grid" span mean at 200/500 bots — the end-to-end number Phase 2 must beat by ≥15%.

- [ ] **Step 3: Commit**

```bash
git add common/benches/spatial_grid_benchmark.rs common/Cargo.toml docs/superpowers/specs/perf-baselines.md
git commit -m "bench: spatial grid full-rebuild baseline (phys optimization deferred to phase 2)"
```

---

### Task 7: B1 — unsafe census, SAFETY comments, lint script

**Verified census (2026-06-11, `grep -rnE 'unsafe \{|unsafe impl|unsafe fn'` minus `unsafe(...)` attributes and `voxygen/anim` generated exports):** 13 real unsafe sites in 9 files — the spec's "~10 in 7 files" missed `voxygen/egui/src/lib.rs`, `server/agent/src/action_nodes.rs`, and `common/base/src/lib.rs`. 4 sites already carry SAFETY comments (`memory_manager.rs:61/62/112` via L59/L103; `common/base/src/lib.rs:370` via L362). **9 sites in 7 files need comments:**

| File:line | What |
|---|---|
| `common/dynlib/src/lib.rs:74` | `Library::new` (hot-reload dylib load) |
| `voxygen/anim/src/lib.rs:166` and `:240` | `lib.get` symbol loads |
| `voxygen/egui/src/lib.rs:196` | `lib.get` symbol load |
| `server/agent/src/action_nodes.rs:1220` | `lib.get` symbol load |
| `world/src/site/generation.rs:1690` | `lib.get` symbol load |
| `voxygen/src/render/renderer/compiler.rs:112` and `:172` | `create_shader_module_trusted` |
| `voxygen/src/ui/ice/winit.rs:17` | `window_clipboard::Clipboard::connect` |

- [ ] **Step 1: Add SAFETY comments to all 9 sites**

Comment style: state the *invariant* and why it holds here, not a restatement of the operation. Two worked examples. `common/dynlib/src/lib.rs:74`:

```rust
        // SAFETY: `Library::new` runs the dylib's load-time initializers.
        // The path comes from `LoadedLib::determine_path`, which only ever
        // points at an artifact this same workspace just compiled (dev-only
        // hot-reload flow) — never an untrusted path. Layout soundness of
        // symbols later fetched from `lib` relies on host and dylib being
        // built by the same rustc, which the hot-reload watcher guarantees
        // by rebuilding on the same toolchain.
        let lib = match unsafe { Library::new(lib_path.clone()) } {
```

`voxygen/src/render/renderer/compiler.rs:112` (note: line 110 is `ShaderRuntimeChecks::unchecked()`, i.e. wgpu runtime bounds checks are **off** — the comment must own that):

```rust
        // SAFETY: the SPIR-V in `descriptor` was produced and validated by
        // shaderc from our own shader sources just above;
        // `create_shader_module_trusted` + `ShaderRuntimeChecks::unchecked()`
        // skips wgpu's re-validation AND runtime bounds checks, trusting that
        // output. Invariant: only shaderc-emitted (never user-supplied or
        // disk-cached) binaries reach this call. A shaderc miscompile could
        // cause GPU-side OOB — acceptable for our own shaders, and why
        // arbitrary sources must never be routed here.
        Ok(unsafe { device.create_shader_module_trusted(descriptor, runtimechecks) })
```

The five `lib.get` sites state: "symbol name and type signature match the `#[unsafe(export_name)]` declaration in the dylib crate, compiled from this same workspace" (cite the exporting crate, e.g. `voxygen-anim`'s generated exports). `compiler.rs:172` mirrors `:112`. `winit.rs:17`: "the raw window handle outlives the clipboard connection because `Clipboard` is owned by the window's UI state and dropped with it (upstream iced_winit pattern)".

- [ ] **Step 2: Add the lint script**

Create `scripts/check-safety-comments.sh` (new `scripts/` dir at repo root), `chmod +x`:

```bash
#!/usr/bin/env bash
# Fails if any real unsafe site lacks a `// SAFETY:` comment within the 8
# lines above it. "Real" = unsafe block/impl/fn, excluding the mechanical
# `unsafe(export_name)`/`unsafe(no_mangle)` attributes Rust 2024 requires on
# hot-reload dylib exports (voxygen/anim, world plots, ...).
# Policy: docs/superpowers/specs/2026-06-10-engine-improvements-design.md §B1
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
while IFS=: read -r file line _; do
    start=$(( line > 8 ? line - 8 : 1 ))
    if ! sed -n "${start},${line}p" "$file" | grep -q 'SAFETY'; then
        echo "MISSING SAFETY comment: ${file}:${line}"
        fail=1
    fi
done < <(grep -rnE 'unsafe \{|unsafe impl|unsafe fn' \
        --include='*.rs' --exclude-dir=target \
        client common network plugin rtsim server server-cli voxygen world \
        2>/dev/null \
    | grep -v 'unsafe(')

if [ "$fail" -ne 0 ]; then
    echo "FAIL: every real unsafe site needs a '// SAFETY:' comment (B1 policy)."
    exit 1
fi
echo "OK: all real unsafe sites carry SAFETY comments."
```

(`network/protocol` lives inside `network/`, so it is covered; new unsafe in any workspace crate is caught.)

- [ ] **Step 3: Verify**

```bash
./scripts/check-safety-comments.sh
```
Expected: `OK: all real unsafe sites carry SAFETY comments.`, exit 0. Negative check: temporarily delete one comment → expect `MISSING SAFETY comment: <file>:<line>` + exit 1; restore.

```bash
cargo check -p veloren-common-dynlib -p veloren-voxygen-anim -p veloren-voxygen-egui -p veloren-server-agent -p veloren-world -p veloren-voxygen
```
Expected: clean (comments only; proves no stray edits). Update the census table in `perf-baselines.md`: 13 real sites, 13 with SAFETY.

- [ ] **Step 4: Commit**

```bash
git add scripts/check-safety-comments.sh common/dynlib voxygen/anim/src/lib.rs voxygen/egui/src/lib.rs server/agent/src/action_nodes.rs world/src/site/generation.rs voxygen/src/render/renderer/compiler.rs voxygen/src/ui/ice/winit.rs docs/superpowers/specs/perf-baselines.md
git commit -m "safety: SAFETY comments on all real unsafe sites + census lint script"
```

---

### Task 8: Workstream C — PR template + review pipeline wiring

**Verified:** `.claude/agents/rust-perf-reviewer.md` and `.claude/agents/ecs-design-reviewer.md` both exist with read-only frontmatter (`tools: Read, Grep, Glob, Bash`). No PR/MR template exists anywhere (`.github/` holds only `CODEOWNERS`, `ISSUE_TEMPLATE/bug_report.md`, `scripts/`, `workflows/`; no `.gitlab/` templates) — create one. The `veloren-review` skill has Steps 1–6 but no subagent dispatch and no SAFETY gate.

**Files:**
- Create: `.github/PULL_REQUEST_TEMPLATE.md`
- Modify: `.claude/skills/veloren-review/SKILL.md` (new step between current Step 4 "ECS Pattern Checklist" and Step 5 "Invoke Code Review"; renumber 5→6, 6→7)

- [ ] **Step 1: Create the PR template**

`.github/PULL_REQUEST_TEMPLATE.md`:

```markdown
## Summary

<!-- What and why. Link the spec/plan if this implements one. -->

## Measurement (required for perf-motivated PRs)

<!-- Before/after from docs/superpowers/specs/perf-baselines.md. Delete only
     if the PR makes no performance claim. -->

| Metric | Before (commit) | After (commit) | Command |
|---|---|---|---|
| | | | |

## Review checklist (engine-improvements §C)

- [ ] `cargo ci-clippy -- -D warnings` and `cargo ci-clippy2 -- -D warnings` clean
- [ ] `cargo fmt --all -- --check` clean
- [ ] `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <touched crates>` pass
- [ ] `./scripts/check-safety-comments.sh` passes; any NEW unsafe site is
      justified here (whitelist policy: engine-improvements spec §B1)
- [ ] ECS placement per CLAUDE.md (comps in `common/src/comp/`, shared systems
      registered via `common-state`, server-only in `server/src/sys/`)
- [ ] No fresh heap allocation per entity/block/frame in `run()`/mesh/render
      paths (reuse buffers — spec §B4)
- [ ] New `telemetry!()` sites off the per-entity-per-tick path or sampled
      (cf. `SNAPSHOT_TICKS`, `common/systems/src/telemetry.rs`)
- [ ] New/changed `common-net` messages: bounded collections, no per-tick
      full-state sends

## Agent review sign-off

<!-- Run from Claude Code on this branch; paste each agent's 3-line verdict. -->

- [ ] `rust-perf-reviewer` dispatched on the diff — verdict:
- [ ] `ecs-design-reviewer` dispatched (required when the diff touches comps,
      systems, resources, or synced state; otherwise write N/A) — verdict:

Findings either fixed or explicitly waived above.
```

- [ ] **Step 2: Wire the subagent passes into the review skill**

Insert into `.claude/skills/veloren-review/SKILL.md` after Step 4:

```markdown
## Step 5: Specialized Reviewer Subagents + Safety Gate

Run the SAFETY-comment gate first:

    ./scripts/check-safety-comments.sh

Must exit 0. Any new unsafe site needs a `// SAFETY:` comment and a PR
justification (policy: engine-improvements spec §B1).

Then dispatch both reviewer agents on the branch diff (Task tool,
`subagent_type` = the agent name):

1. **rust-perf-reviewer** — always. Prompt: "Review `git diff
   development...HEAD` for performance and memory issues." Perf-motivated
   diffs must also show numbers in `docs/superpowers/specs/perf-baselines.md`.
2. **ecs-design-reviewer** — when the diff touches `common/src/comp/`,
   `common/systems/`, `server/src/sys/`, resources, or net-synced state.
   Prompt: "Review `git diff development...HEAD` for ECS architectural fit."

Paste each verdict into the PR description (the template has slots).
Blockers must be fixed; minors fixed or explicitly waived.
```

- [ ] **Step 3: Exercise the pipeline on a real diff (Phase 1 exit criterion)**

Per spec §Testing, both agents must produce at least one accepted finding on a real diff. Dispatch both on this very branch:

```
Task(subagent_type=rust-perf-reviewer): "Review `git diff development...HEAD` for performance and memory issues."
Task(subagent_type=ecs-design-reviewer): "Review `git diff development...HEAD` for ECS architectural fit."
```

Expected: severity-tagged findings + a 3-line verdict each. Fix accepted findings (loop back to the relevant task's verify step) and keep both verdicts for the PR description. If a dispatch fails, fixing the agent definition is in scope for this task.

- [ ] **Step 4: Commit**

```bash
git add .github/PULL_REQUEST_TEMPLATE.md .claude/skills/veloren-review/SKILL.md
git commit -m "process: PR template with perf/safety checklist; reviewer subagents wired into review skill"
```

---

### Task 9: Lint, format, changelog, and branch finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo ci-clippy -- -D warnings
cargo ci-clippy2 -- -D warnings
```
Expected: clean. Fix warnings (no `#[allow]` without a justifying comment).

- [ ] **Step 2: Format**

```bash
cargo fmt --all -- --check
```
If it fails: `cargo fmt --all`, re-check.

- [ ] **Step 3: Tests + safety gate**

```bash
VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-voxygen
cargo test -p veloren-common-frontend --features logging-verbose
./scripts/check-safety-comments.sh
```
Expected: all PASS / exit 0.

- [ ] **Step 4: Changelog**

Under `## [Unreleased]` → `### Changed` in `CHANGELOG.md`:

```markdown
- Telemetry logging no longer takes a global lock per event (batched drain thread, drop-on-full).
- Terrain glow lighting skips all allocations for chunks without glowing blocks.
- Shader pipeline recreation reuses a capped thread pool, removing the dev hot-reload stutter.
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entries for engine phase 1"
```

- [ ] **Step 5: Assemble the PR measurement table**

Copy every Before/After pair from `perf-baselines.md` into the PR description's Measurement table — numbers travel with the PR; that is the program's contract. Include both agent verdicts from Task 8 Step 3 and tick the checklist.

- [ ] **Step 6: Finish the branch**

Invoke `veloren-review` (now including its new Step 5), then `superpowers:finishing-a-development-branch` to merge `feature/engine-phase1` into `development`. Phase 2 (meshing scratch pool, sunlight column precompute, golden-mesh harness, incremental spatial grid — entry point: Task 6's bench baseline) and Phase 3 (memory_manager poisoning + scoped tokens, adaptive LOD) remain tracked in the design spec's phase table.
