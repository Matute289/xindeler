# Engine Improvement Program: Performance, Memory Safety, Continuous Review

**Date:** 2026-06-10
**Scope:** Three parallel workstreams — runtime performance, memory safety/management, and a repeatable detailed-review process — phased over ~6–8 weeks of part-time work.

## Context

This fork is evolving from "Veloren + terrain pipeline" into a heavily-modded RPG/MMO (see companion spec `2026-06-10-rpg-evolution-master-roadmap.md`). That direction multiplies engine load on every axis: more concurrent NPCs (rtsim + server agents), larger streamed worlds (Transvoxel terrain already added in Phase 1–3), and more simultaneous abilities/projectiles per combat encounter. The upstream engine was tuned for vanilla load; our additions (smooth terrain meshing, triplanar normal maps, 6-sink logging with telemetry) add their own costs on top.

Before adding more gameplay weight, the engine needs: (1) measured headroom on the hot paths that will saturate first — terrain meshing, physics, render pipeline recreation; (2) a memory-safety baseline so the small-but-critical unsafe surface doesn't grow unaudited as we mod deeper; (3) a review process that catches performance and ECS-design regressions per-PR instead of per-profiling-session.

## Goals / Non-Goals

**Goals**

- Reduce per-chunk terrain meshing cost (the dominant client-side spike during traversal) without changing visual output.
- Reduce server tick cost of the physics system at high entity counts (target: 500+ active entities without tick budget overrun).
- Eliminate the telemetry layer's lock-on-emit and per-event allocation, so `telemetry!()` stays free enough to leave in hot systems.
- Catalogue and justify every real `unsafe` block; refactor the highest-risk one (`memory_manager.rs`).
- Stand up a per-PR review pipeline (checklists + two specialized review subagents) that runs on every branch before merge.
- Every performance change ships with a before/after measurement (criterion bench or tracy capture).

**Non-Goals**

- No renderer architecture rewrite (the deferred pipeline with 18 pipeline modules in `voxygen/src/render/pipelines/` stays as-is).
- No ECS framework migration away from `specs`.
- No upstream-divergent changes to network protocol or persistence formats.
- No GPU-side optimization beyond what tracy/wgpu-profiler captures justify (shader micro-optimization is out of scope).

## Current State (verified)

| Subsystem | Location | State |
|---|---|---|
| Render pipelines | `voxygen/src/render/pipelines/` (18 modules: shadow, rain_occlusion, bloom, postprocess, lod_terrain, lod_object, clouds, fluid, trail, rope, terrain, sprite, figure, particle, skybox, debug, ui, blit) | Deferred pipeline; async creation via per-recreation rayon pools in `voxygen/src/render/renderer/pipeline_creation.rs` (initial: L982; recreate: L1061) |
| GPU profiling | `voxygen/src/render/renderer/mod.rs` ~L170 (`profiler: wgpu_profiler::GpuProfiler`) | wgpu-profiler wired, surfaced through tracy |
| Terrain meshing | `voxygen/src/mesh/terrain.rs` (628 lines), `voxygen/src/mesh/greedy.rs` (901 lines) | Chunks meshed in parallel via `SlowJobPool` jobs named `TERRAIN_MESHING` (`voxygen/src/scene/terrain/mod.rs:1118-1119`); *within* a chunk everything is serial, including two full light-BFS passes |
| Light propagation | `voxygen/src/mesh/terrain.rs:36-211` (`calc_light`) | Serial BFS over `VecDeque` (L64–183); called twice per chunk — sunlight and glow (L277–278); allocates two full-volume `Vec<u8>` light maps per call (L58, L196) even when the seed iterator is empty |
| Physics | `common/systems/src/phys/mod.rs` (1486 lines), `phys/collision.rs` (954 lines) | `par_join` at L389 (entity↔entity pushback), L724 (velocity update), L861 (entity↔terrain); two spatial grids rebuilt serially every tick (`construct_spatial_grid` L324, `construct_voxel_collider_spatial_grid` L570, both called from `run` at L1477/L1480) |
| Unsafe surface | 269 raw `grep unsafe` lines workspace-wide | Misleading raw count — see Workstream B for breakdown; `#![deny(unsafe_code)]` already set in `voxygen/src/lib.rs:1`, `common/src/lib.rs:1`, `network/src/lib.rs:1`, `client/src/lib.rs:1`, `server/src/lib.rs:1` |
| Highest-risk unsafe | `common/state/src/plugin/memory_manager.rs` (116 lines) | `AtomicRefCell<Option<NonNull<EcsWorld<'static,'static>>>>` (L56) + lifetime-erasing cast (L86) + `std::process::abort()` on misuse (L82); manual `unsafe impl Send/Sync` (L61–62) |
| CPU profiling | `tracy` feature; `prof_span!` in `common/base/src/lib.rs:94`; `SysMetrics`/`PhysicsMetrics` in `common/ecs/src/metrics.rs:5,10`; `tracy-memory` feature wiring a tracy `global_allocator` (`voxygen/Cargo.toml:34`, `voxygen/src/main.rs:14-15`) | Cargo aliases exist: `tracy-server`, `tracy-voxygen`, `tracy-server-debuginfo`, `tracy-server-releasedebuginfo` (`.cargo/config.toml`) |
| Benchmarks | criterion 0.8 workspace dep (`Cargo.toml:160`) | `common/benches/` (chonk, color, loot), `voxygen/benches/meshing_benchmark.rs` (full terrain mesh over 4×4 chunks at world center), `world/benches/` (cave, site, tree), `network/` + `network/protocol/` benches |
| Fork telemetry | `common/frontend/src/` (lib 342, bounded_writer 225, telemetry_layer 90, lifecycle 83 lines); `telemetry!` macro `common/src/lib.rs:22`; `TelemetrySystem` `common/systems/src/telemetry.rs` (98 lines, snapshots every 150 ticks) | `TelemetryLayer::on_event` builds a fresh `String` with per-field `format!` and writes under a global `Arc<Mutex<BufWriter<File>>>` (telemetry_layer.rs L14, L49–51) — lock + allocation on the emitting thread |
| LOD | `voxygen/src/settings/graphics.rs:38,52` (`lod_distance` default 200, `lod_detail` default 250); clamped to 100–2500 in `voxygen/src/scene/lod.rs:67` | Static user settings; no load-adaptive tuning |

## Workstream A — Performance

### A1. Terrain meshing: light precompute + per-call allocation removal

**Problem.** `generate_mesh` (`voxygen/src/mesh/terrain.rs:228`) runs `calc_light` twice per chunk (L277 sunlight, L278 glow). Each call allocates `light_map` over the padded volume (L58) plus a second minimized `light_map2` (L196) and copies between them — even the glow pass with zero glow blocks pays both allocations and the full copy loop. The BFS itself is serial per chunk; cross-chunk parallelism via `TERRAIN_MESHING` slow jobs already exists, so the win is per-job latency and allocator pressure, not throughput.

**Approach.**
1. Early-out in `calc_light` when `lit_blocks` is empty and `default_light == 0`: return a constant closure, skip both allocations (glow pass is empty for most surface chunks — `glow_blocks` collected at L259).
2. Precompute per-column sunlight heights once per chunk (cacheable in chunk meta at generation/load time) so the sunlight BFS seeds only at occlusion boundaries instead of scanning the full top layer.
3. Reuse light-map buffers across meshing jobs via a thread-local pool keyed by volume size (slow-job worker threads are long-lived).
4. In `greedy.rs`, audit per-axis sweep for redundant face checks against fully-opaque neighbor columns (per-axis culling using the column heights from step 2).

**Risk.** Medium — light output must be bit-identical; the existing `voxygen/benches/meshing_benchmark.rs` gives a correctness+perf harness, and a golden-mesh comparison test (hash vertex output for fixed seed chunks) guards regressions.

**Expected gain (estimate).** 20–40% per-chunk mesh time for surface chunks (glow early-out + allocation reuse alone removes two large allocations and one full-volume copy per chunk); measured, not promised.

**Measure.** `VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench meshing` before/after; tracy capture of `calc_light` span (already instrumented via `span!` at terrain.rs:47) during a straight-line flight across fresh terrain.

### A2. Physics: spatial grid rebuild + pushback allocation

**Problem.** Every tick, `run` (`phys/mod.rs:1459`) serially rebuilds two spatial grids (L1477, L1480) by iterating all entities (`construct_spatial_grid` L324–360). The three `par_join` passes (L389, L724, L861) are read-mostly against these grids, so the parallel sections scale, but the serial rebuild is O(entities) on the main system thread every tick and grows linearly with NPC count — exactly the axis the RPG roadmap pushes.

**Approach.**
1. Incremental grid maintenance: only re-insert entities whose cell changed since last tick (compare against `PreviousPhysCache`, maintained at L205); fall back to full rebuild on large dirty ratios.
2. Collect the L861 pass results (`land_on_grounds`, outcomes) into pre-sized buffers reused across ticks instead of fresh collections.
3. Profile `phys/collision.rs` voxel-collider paths under load before touching them — they are only hot near airships/voxel entities.

**Risk.** Medium-high — physics is gameplay-critical and shared client/server; any divergence breaks prediction. Incremental grid must be validated against full rebuild (debug assertion comparing grid contents every N ticks under a test feature).

**Expected gain (estimate).** Grid rebuild drops from O(n) to O(moved); at 500 NPCs mostly idle (rtsim village population) this is the difference between linear and near-constant rebuild cost.

**Measure.** `cargo tracy-server` (alias exists) + `swarm` bot load (`.cargo/config.toml` alias `swarm`, requires `client/bin_bot`); track `PhysicsMetrics` (`common/ecs/src/metrics.rs:10`) and the `phys` system span. Acceptance: tick time at 500 entities improves ≥15% vs baseline capture.

### A3. Shader hot-reload stutter

**Problem.** Pipeline recreation is already fully backgrounded (`pipeline_creation.rs:1061-1137`, triggered by `ReloadWatcher` at `renderer/mod.rs:1241-1242`) — but each recreation builds a **fresh** `rayon::ThreadPool` with default thread count = all logical cores (L1061–1064; same pattern at L982 for initial creation). During a dev-loop shader edit, shaderc compilation saturates every core and starves the render/main threads, which presents as a multi-frame stutter even though nothing blocks.

**Approach.**
1. Cap recreation pool to `max(1, num_cpus/2 - 1)` threads and reuse one persistent pool instead of constructing per recreation.
2. (Dev-only) lower compile thread priority where the platform allows.
3. Keep the existing deferral logic (`recreation_pending`, `renderer/mod.rs:1300-1303`) unchanged.

**Risk.** Low — purely a scheduling change; worst case recreation takes longer in wall time, which is acceptable for a background job.

**Measure.** Tracy frame-time plot while touching a shader file with `cargo tracy-voxygen`; acceptance: no frame >33ms during recreation on the dev machine.

### A4. Telemetry batching with ring buffer

**Problem.** `TelemetryLayer::on_event` (`common/frontend/src/telemetry_layer.rs:36-53`) runs on the emitting thread: it allocates a `String`, does one `format!` per field (JsonVisitor, L55–90), then takes a global `Arc<Mutex<BufWriter<File>>>`. Every `telemetry!()` call site in combat/projectile/etc. systems (`common/systems/src/melee.rs`, `projectile.rs`) pays lock + allocations inside the ECS tick. `TelemetrySystem` (`common/systems/src/telemetry.rs`) additionally emits up to 20 entity snapshots per 150-tick window.

**Approach.**
1. Replace the mutexed writer with a bounded SPSC/MPSC ring buffer (fixed-size byte slots, e.g. `crossbeam_channel::bounded` or a custom ring): `on_event` serializes into a stack/thread-local buffer and pushes; a dedicated drain thread owns the `BufWriter` and flushes batches. Drop-on-full with a dropped-events counter (telemetry must never backpressure the tick).
2. Replace per-field `format!` with `write!` into a reused thread-local `String` (zero allocation steady-state).
3. Keep `bounded_writer.rs` rotation/compression as-is — it already offloads gzip to its own thread (sync_channel(64), bounded_writer.rs L50–60).

**Risk.** Low — telemetry is fork-only, no upstream coupling; failure mode is dropped telemetry lines, which the counter makes visible.

**Expected gain.** Removes a global lock from combat-system hot paths; matters most on the server under swarm load.

**Measure.** Criterion micro-bench of `on_event` (new bench in `common/frontend`); tracy comparison of melee/projectile system spans with telemetry-heavy combat before/after.

### A5. LOD tuning

**Problem.** `lod_distance` (default 200) and `lod_detail` (default 250, clamp 100–2500 at `scene/lod.rs:67`) are static user settings. With larger view ranges planned, a fixed detail level either wastes budget on distant LOD or starves it.

**Approach.** Add a load-adaptive controller: when the meshing backlog (pending `TERRAIN_MESHING` jobs) or frame time exceeds thresholds for N consecutive seconds, step `lod_detail` down within a user-configured band; step back up when headroom returns. Surface the active value in the HUD debug overlay. Pure voxygen change: `voxygen/src/scene/lod.rs`, `voxygen/src/settings/graphics.rs`.

**Risk.** Low — clamped to user band, off by default initially.

**Measure.** Frame-time percentiles (tracy) during fast traversal at max view distance, before/after.

## Workstream B — Memory Safety & Management

### B1. Unsafe audit (catalogue + SAFETY-comment gate)

The raw count (269 `unsafe` grep lines) collapses to a small real surface once classified:

| Category | Count (approx) | Locations | Risk |
|---|---|---|---|
| `unsafe(export_name = ...)` attributes (Rust 2024 requires `unsafe` on these; mechanical, for hot-reload dylib exports) | ~240 | `voxygen/anim/src/**` (197 grep lines), `world/src/site/plot/*` (most of 47) | Trivial |
| `#![deny(unsafe_code)]` / `#[expect(unsafe_code)]` markers | ~8 | voxygen, common, network, client, server lib roots | None (these are the guard rails) |
| Dylib symbol loads (`Library::new`, `lib.get`) | 4 | `common/dynlib/src/lib.rs:74`, `voxygen/anim/src/lib.rs:166,240`, `world/src/site/generation.rs:1690` | Medium (dev-only, hot-reload feature) |
| `create_shader_module_trusted` | 2 | `voxygen/src/render/renderer/compiler.rs:112,172` | Medium (skips wgpu validation) |
| Clipboard FFI | 1 | `voxygen/src/ui/ice/winit.rs:17` | Low |
| Lifetime-erased ECS pointer | 3 | `common/state/src/plugin/memory_manager.rs:61,62,112` | **High** |

Only 3 `// SAFETY:` comments exist workspace-wide (2 of them in `memory_manager.rs`). **Process change:** every real unsafe block (non-attribute) must carry a `// SAFETY:` comment; enforced by a repo lint script (`grep` for `unsafe {`/`unsafe impl` without adjacent SAFETY in the 7 files above — small enough to hand-maintain a whitelist) wired into the Workstream C checklist. New unsafe outside the whitelist fails review.

### B2. `memory_manager.rs` refactor

**Current design** (`common/state/src/plugin/memory_manager.rs`): `EcsAccessManager` smuggles a `&EcsWorld<'a,'b>` to plugin callbacks by casting to `NonNull<EcsWorld<'static,'static>>` (L86), storing it in an `AtomicRefCell` (L56), and `abort()`ing the whole process if a borrow outlives `execute_with` (L82). The `with` accessor re-derefs unsafely (L112). The HRTB closure bound (L99) prevents reference escape, which is sound but subtle, and the abort path turns a plugin bug into a server kill.

**Proposal.**
1. Short term (keep the pattern, shrink the blast radius): replace `abort()` with poisoning — mark the manager failed, return `None` from subsequent `with` calls, and disable the offending plugin. Abort is only justified if a dangling deref is otherwise reachable; with poisoning + the existing borrow guard it is not.
2. Medium term: replace the erased pointer with a scoped-token API — `execute_with` passes an opaque `EcsToken<'scope>` into the plugin dispatch, and `with` becomes a method on the token, making escape a compile error and deleting all three unsafe sites. This requires threading a lifetime through the plugin call chain in `common/state/src/plugin/` — bounded, fork-local change (plugins feature is barely used today, which is exactly why now is the cheap time to fix it).

**Risk.** Low blast radius (plugin feature is optional); validated by existing plugin tests plus a new test that deliberately leaks the reference and asserts poisoning instead of abort.

### B3. Allocation hot spots

Verified per-call allocations in the meshing path (all per chunk meshed, on slow-job worker threads):

| Site | Allocation |
|---|---|
| `voxygen/src/mesh/terrain.rs:58` | `vec![UNKNOWN; outer volume]` light map — twice per chunk (sunlight + glow) |
| `voxygen/src/mesh/terrain.rs:196` | second minimized light map + full-volume copy loop — twice per chunk |
| `voxygen/src/mesh/terrain.rs:299` | `vec![AIR; w*h*d]` flattened block copy |
| `voxygen/src/mesh/terrain.rs:438-440` | three fresh `Vec`s for opaque mesh depth layers |
| `common/frontend/src/telemetry_layer.rs:45` + JsonVisitor | per-event `String` + per-field `format!` temporaries |

Confirm and rank with `tracy-memory` builds (`voxygen/Cargo.toml:34` already wires a tracy global allocator behind that feature) — capture during chunk-streaming flight, sort by allocation rate.

### B4. Arena / pooling opportunities

1. **Meshing scratch pool** (covers B3 rows 1–4): thread-local `MeshScratch { light_a: Vec<u8>, light_b: Vec<u8>, flat: Vec<Block>, opaque_layers: [Vec<…>; 3] }` reused across `TERRAIN_MESHING` jobs; `clear()` + `resize()` instead of fresh `vec![]`. Slow-job worker threads are long-lived so thread-local lifetime is fine.
2. **Telemetry slots**: the A4 ring buffer doubles as the pool — fixed byte slots, no per-event heap.
3. **Physics outcome buffers** (A2.2): per-tick reused `Vec`s held in the system struct.
4. Explicitly *not* pursuing a general arena allocator — the wins above are local pools; a global arena adds complexity without a measured driver.

## Workstream C — Continuous Code Review Process

CI lint gates already exist (`cargo ci-clippy`, `ci-clippy2` aliases; commands mirrored in `CLAUDE.md`). This workstream adds a structured human+agent review layer on top.

### Pipeline (every branch, before merge)

1. **Mechanical gates** — `cargo ci-clippy -- -D warnings`, `cargo ci-clippy2 -- -D warnings`, `cargo fmt --all -- --check`, `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <touched crates>`. Already codified in the `veloren-review` skill; keep as step 1.
2. **Specialized subagent passes** — two reviewer agents, created as part of Phase 1 deliverables:
   - `.claude/agents/rust-perf-reviewer.md` — flags: allocation in per-tick/per-frame loops (`Vec::new`, `format!`, `collect` in systems and mesh/render paths), lock acquisition inside `par_join` closures, missing `prof_span!` on new systems, fresh rayon pool construction.
   - `.claude/agents/ecs-design-reviewer.md` — flags: components added outside `common/src/comp/`, systems not registered through `common-state`, `WriteStorage` where `ReadStorage` suffices, joins missing `.maybe()` patterns the codebase uses, server-only logic leaking into `common-systems`.
3. **Holistic pass** — `/code-review` (high effort) on the branch diff for correctness bugs.
4. **Checklist sign-off** — reviewer (human or agent) confirms the per-PR checklist below; findings either fixed or explicitly waived in the PR description.

### Per-PR review checklist

| Check | What to look for |
|---|---|
| ECS patterns | Comp/resource/system placement matches CLAUDE.md layout; no `World::fetch` in hot loops; system declares accurate `SystemData` |
| Allocation in hot loops | No fresh heap allocation per entity/per block in `run()`/mesh/render paths; reuse buffers per B4 |
| Unsafe justification | New `unsafe` only in whitelisted files (B1 table); every block has `// SAFETY:`; whitelist change requires explicit spec note |
| Network message size | New/changed `common-net` messages: bounded collections, no per-tick full-state sends, compression considered for >1KB payloads |
| Telemetry cost | New `telemetry!()` sites are off the per-entity-per-tick path or sampled (cf. `SNAPSHOT_TICKS` pattern in `common/systems/src/telemetry.rs:7`) |
| Measurement | Perf-motivated PRs link a bench delta or tracy capture pair |

### Cadence

- Per-PR: pipeline above (steps 1–4).
- Monthly: re-run the B1 unsafe census and the A-workstream baseline benches; diff against last month's numbers, file regressions as issues.
- Per upstream merge: run the full pipeline on the staging branch (extends the process in `2026-06-10-upstream-merge-design.md`).

## Phases

### Phase 1 — Measure, gate, and quick wins (complexity M, ~5–8 dev-days)

**Deliverables**
- Baseline tracy captures (voxygen flight path, server swarm at 200/500 entities) archived under `docs/superpowers/baselines/` with capture protocol notes.
- New criterion benches: `calc_light` micro-bench (extracted from meshing bench), telemetry `on_event` bench.
- A4 telemetry ring buffer + zero-alloc serialization shipped.
- A3 shader-recreation pool cap shipped.
- B1 unsafe catalogue committed; SAFETY-comment lint script in repo.
- `.claude/agents/rust-perf-reviewer.md` and `.claude/agents/ecs-design-reviewer.md` written and exercised on one real PR.

**Milestones:** baselines captured → telemetry layer replaced → review agents live.

**Risks:** swarm bot load test may need fixes to run on this fork (verify `client/bin_bot` builds early).

**Tasks:** capture protocol doc (S); calc_light bench extraction (S); telemetry ring buffer (M); shader pool cap (S); unsafe census + lint script (S); two agent definitions + checklist into `veloren-review` skill (M).

### Phase 2 — Meshing and physics core (complexity L, ~10–15 dev-days)

**Deliverables**
- A1 steps 1–3: glow early-out, sunlight column precompute, thread-local mesh scratch pool (B4.1).
- Golden-mesh regression test (fixed-seed chunk hashes).
- A2 steps 1–2: incremental spatial grid + reused outcome buffers, with debug-feature grid-equivalence assertion.
- Before/after bench + tracy deltas published for both.

**Milestones:** meshing bench ≥20% faster on surface chunks → physics tick at 500 entities ≥15% faster → golden tests green.

**Risks:** sunlight precompute touches chunk meta (cross-crate: `common` terrain + `voxygen` mesh) — keep behind a feature flag until golden tests pass; incremental grid correctness is the long pole, hence the equivalence assertion.

**Tasks:** glow early-out (S); buffer pool (M); column sunlight cache + BFS seeding (L); golden-mesh harness (M); incremental grid (L); physics buffer reuse (S); greedy per-axis culling exploration (M, may be dropped if column cache already captures the win).

### Phase 3 — Safety hardening and adaptive tuning (complexity M, ~8–12 dev-days)

**Deliverables**
- B2 memory_manager: poisoning instead of abort (step 1), then scoped-token refactor (step 2) deleting the unsafe sites.
- A5 adaptive LOD controller behind a settings toggle.
- `tracy-memory` allocation-rate capture comparing Phase 2 results; pooling extended to anything still dominating.
- Monthly review cadence executed once end-to-end (census + bench diff) and documented as a repeatable runbook section in the review skill.

**Milestones:** plugin abort path removed → unsafe count in `common/state` reaches 0 → adaptive LOD validated in a max-view-distance flight.

**Risks:** scoped-token refactor may fight the plugin dispatch signatures; fallback is stopping at step 1 (poisoning), which already removes the worst failure mode.

**Tasks:** poisoning change + leak test (S); scoped-token API (L); adaptive LOD controller (M); tracy-memory pass (S); runbook (S).

## Testing / Verification Strategy

- **Benchmarks:** every A/B change lands with criterion before/after on the relevant bench (`meshing_benchmark`, new `calc_light`/telemetry benches, `chonk_benchmark` for terrain access changes). Run with `VELOREN_ASSETS="$(pwd)/assets" cargo bench -p <crate>`; record numbers in the PR.
- **Tracy capture protocol:** fixed scenarios — (1) voxygen: straight flight at fixed speed over fresh terrain, 60s, `cargo tracy-voxygen`; (2) server: `cargo tracy-server` + swarm at 200 and 500 bots, 120s. Captures saved with commit hash; compare `calc_light`, `phys` system, frame-time percentiles.
- **No-regression gates:** golden-mesh hashes (Phase 2) in `cargo test`; grid-equivalence debug assertion under a `phys-verify` feature in CI test job; clippy/fmt gates unchanged; SAFETY lint script in the review pipeline.
- **Functional:** existing test suite per touched crate; manual smoke via `veloren-run` skill (client + local server) after each phase; telemetry output validated with the `veloren-telemetry` skill parsing a session log.
- **Review process verification:** Phase 1 exit requires both subagents having produced at least one accepted finding on a real diff (not a synthetic test).

## Open Questions

1. Should the sunlight column cache live in `TerrainChunk` meta (shared with server, larger blast radius) or a voxygen-side cache keyed by chunk position (duplicated work on remesh, zero common/ changes)? Default: voxygen-side first, promote if profiling justifies.
2. Telemetry ring buffer sizing: fixed 64-byte slots cover current events, but rtsim-scale NPC telemetry (roadmap) may want variable-size records — decide when rtsim telemetry lands.
3. Does the swarm bot harness exercise enough physics variety (collisions, projectiles) to validate A2, or do we need a scripted NPC brawl scenario via the agent system?
4. Adaptive LOD (A5) vs. the roadmap's bigger-world streaming changes — if view-distance architecture changes in the RPG roadmap, A5 may be folded into that work instead of landing standalone.
5. Whether to upstream any of A1/A2 to GitLab Veloren once proven — keeping the diff small helps future merges (cf. upstream-merge spec), but upstreaming costs review round-trips.
