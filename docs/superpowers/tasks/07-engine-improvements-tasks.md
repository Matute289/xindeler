# Engine Improvements Phase 1 — Task Board

**Source plan:** [../plans/2026-06-11-engine-improvements.md](../plans/2026-06-11-engine-improvements.md)
**Execute with:** superpowers:subagent-driven-development, one task per subagent, in plan order.

> If acceptance fails twice, escalate one model tier and leave a note in the task file.

Conventions (apply to every task): branch `feature/engine-phase1` off `development` (created in ENG-1); benches/tests need `VELOREN_ASSETS="$(pwd)/assets"`; invoke `veloren-engine-perf` before any perf task and `superpowers:test-driven-development` before writing code; **measurement-first rule** — no perf commit without a baseline captured *before* the change and the delta recorded in `docs/superpowers/specs/perf-baselines.md`.

## ENG-1 — Baselines file + tracy capture protocol + initial captures

- **Model:** haiku — runs a fully documented capture/bench protocol and pastes numbers; no design judgment.
- **Depends on:** none (first task; create `feature/engine-phase1` off `development` before starting).
- **Branch / commit:** `feature/engine-phase1`; commit `docs: perf baselines file with tracy capture protocol and initial numbers`.
- **Files:** Create: `docs/superpowers/specs/perf-baselines.md` (exact content in plan Step 2). Generated artifacts: `docs/superpowers/baselines/*.tracy` (gitignore if >50MB — the numbers in the md are the durable artifact).
- **Assets:** none.
- **Downloads/tools:** Tracy capture client required: `which tracy-capture || brew install tracy` (plan Task 1 Step 1). Cargo aliases `tracy-server` (L34), `tracy-voxygen` (L40), `swarm` (L43) verified present in `.cargo/config.toml`.
- **Steps:** Follow plan section '### Task 1' steps 1–4 verbatim. Trap: if `cargo check --bin swarm --features client/bin_bot,client/tick_network` fails, fixing the swarm bin is **in scope** (it blocks the P2 server baseline) — note the fix in the PR; if non-trivial, escalate per the rule. Fill only the tracy cells and the `meshing_benchmark` row now; the other bench rows are filled by ENG-2/4/6.
- **Acceptance:** `tracy-capture` on PATH; swarm builds; P1, P1b, P2 (200 and 500 bots) Baseline cells and the meshing bench row filled with the current commit hash; file committed.
- **Size:** M

## ENG-2 — `calc_light` micro-bench

- **Model:** sonnet — test-harness addition; bench code given in plan but setup must be copied verbatim from `voxygen/benches/meshing_benchmark.rs` L14–49.
- **Depends on:** ENG-1 (baselines file must exist to record numbers).
- **Branch / commit:** `feature/engine-phase1`; commit `bench: calc_light micro-bench (sunlight, empty glow, seeded glow)`.
- **Files:** Modify: `voxygen/src/mesh/terrain.rs:36` (export `calc_light` as `pub` with doc comment), `voxygen/Cargo.toml` (new `[[bench]]` after L170–172). Create: `voxygen/benches/light_benchmark.rs`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 2' steps 1–4 verbatim. Use `const GEN_SIZE: i32 = 3;`, chunk (1,1) + 1-block borders sampling math from meshing_benchmark L51–79, 16 synthetic glow seeds as specified.
- **Acceptance:** `VELOREN_ASSETS="$(pwd)/assets" cargo bench -p veloren-voxygen --bench light_benchmark` produces three results; `glow_empty` is non-trivial today (hundreds of µs). All three recorded in `perf-baselines.md` with commit hash.
- **Size:** M

## ENG-3 — A1 quick win: `calc_light` empty-glow early-out

- **Model:** opus — meshing hot-path optimization whose correctness rests on a bit-identical equivalence argument; restructuring must not perturb the BFS.
- **Depends on:** ENG-2 (light bench baseline MUST be recorded before this change — measurement-first rule).
- **Branch / commit:** `feature/engine-phase1`; commit `perf: skip light-map allocations in calc_light when glow pass has no seeds`.
- **Files:** Modify: `voxygen/src/mesh/terrain.rs:36-225` (`calc_light`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 3' steps 1–4 verbatim. Traps: keep a single returned closure (two closure types will not compile); hoist `min_bounds`/`lm_idx2`, wrap existing body in `else`; BFS (current L56–187) and minimization loop (L201–208) move inside the `else` **unchanged**; delete the now-redundant `drop(light_map)` and old `min_bounds`/`lm_idx2` definitions; early-out guards are `!is_sunlight && default_light == 0 && lit_blocks.peek().is_none()` — do not relax them.
- **Acceptance:** `cargo check -p veloren-voxygen` clean; `VELOREN_ASSETS=... cargo test -p veloren-voxygen` PASS. Re-run light + meshing benches: `glow_empty` drops to nanoseconds; `sunlight`/`glow_seeded` within noise; meshing improves for chunks without glow blocks. After numbers in `perf-baselines.md`. Manual flight smoke via `veloren-run` skill: caves/lava still glow, surface lighting unchanged (golden-mesh harness is Phase 2).
- **Size:** M

## ENG-4 — A4: telemetry ring buffer + zero-alloc serialization

- **Model:** sonnet — telemetry plumbing with full TDD code in plan (channel + drain thread + visitor rewrite).
- **Depends on:** ENG-1 (baselines file). Hard sequencing: the bench baseline MUST be run and **committed on the OLD mutex implementation** (Step 1) before any rewrite code is written.
- **Branch / commit:** `feature/engine-phase1`; two commits: `bench: telemetry on_event criterion bench (baseline on mutex implementation)` then `perf: telemetry layer drains via bounded channel, zero-alloc serialization`.
- **Files:** Create: `common/frontend/benches/telemetry_benchmark.rs`. Rewrite: `common/frontend/src/telemetry_layer.rs` (currently 108 lines). Modify: `common/frontend/src/lib.rs:288-289` and `:41-46` (`LogGuards` + flush handle), `common/frontend/Cargo.toml` (dev-deps criterion, `[[bench]]` with `required-features = ["logging-verbose"]`, dependency `crossbeam-channel`).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 4' steps 1–7 verbatim (TDD: failing test `events_reach_disk_in_order_with_escaping` first). Traps: `try_send` drop-on-full with `AtomicU64` counter — telemetry must never backpressure the tick; recycle channel for zero-alloc steady state; `Flush(Sender<()>)` ack with 5s timeout; PR note — control chars other than `\n` now `\uXXXX`-escaped, `\n` keeps `\\n` so `veloren-telemetry` skill parsing is unaffected.
- **Acceptance:** `cargo test -p veloren-common-frontend --features logging-verbose telemetry` 1 PASS; `cargo check -p veloren-common-frontend` and `--features logging-verbose,tracy` compile. `cargo bench -p veloren-common-frontend --features logging-verbose`: `telemetry_on_event` improves vs Step-1 baseline; After recorded in `perf-baselines.md`. Functional smoke: short logging-verbose session, parse `.jsonl` with `veloren-telemetry` skill.
- **Size:** L

## ENG-5 — A3: shader recreation pool, persistent + capped

- **Model:** haiku — fully-specified mechanical edit plus the documented P1b capture protocol.
- **Depends on:** ENG-1 (P1b baseline protocol and Before number).
- **Branch / commit:** `feature/engine-phase1`; commit `perf: persistent capped rayon pool for shader pipeline recreation`.
- **Files:** Modify: `voxygen/src/render/renderer/pipeline_creation.rs:1060-1065` (replace fresh all-cores pool with `recreation_pool()` `OnceLock`; initial creation at L982 stays as-is).
- **Assets:** none.
- **Downloads/tools:** Tracy (already installed in ENG-1) for the P1b capture.
- **Steps:** Follow plan section '### Task 5' steps 1–4 verbatim. Capture P1b baseline FIRST (`touch assets/voxygen/shaders/terrain-frag.glsl` during tracy-voxygen). Pool capped to ~half logical cores; existing deferral logic (`recreation_pending`, `renderer/mod.rs:1300-1303`) untouched.
- **Acceptance:** `cargo check -p veloren-voxygen` clean; `cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings` clean (function is not hot-reload-gated). Repeat P1b: **no frame >33ms during recreation** (spec §A3). Before/after in `perf-baselines.md`.
- **Size:** S

## ENG-6 — A2 physics: bench only (optimization explicitly deferred)

- **Model:** haiku — bench code given verbatim; run and record. Do not attempt the optimization.
- **Depends on:** ENG-1 (P2 captures must include the "Construct spatial grid" span mean at 200/500 bots — the end-to-end number Phase 2 must beat by ≥15%).
- **Branch / commit:** `feature/engine-phase1`; commit `bench: spatial grid full-rebuild baseline (phys optimization deferred to phase 2)`.
- **Files:** Create: `common/benches/spatial_grid_benchmark.rs`. Modify: `common/Cargo.toml` (`[[bench]]` after the `loot_benchmark` block, L111–113).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 6' steps 1–3 verbatim. Trap: the incremental-grid fix is **deferred to Phase 2** (50+ lines of correctness-critical code, prediction-divergence risk) — Phase 1 ships only this harness. Tracy spans already exist at `common/systems/src/phys/mod.rs:325` and `:571`; add no new spans.
- **Acceptance:** `VELOREN_ASSETS=... cargo bench -p veloren-common --bench spatial_grid_benchmark`: three results (200/500/2000) scaling ~linearly with N; recorded in `perf-baselines.md` marked "deferred to Phase 2".
- **Size:** S

## ENG-7 — B1: unsafe census, SAFETY comments, lint script

- **Model:** sonnet — comments must state the actual soundness invariant per site (two worked examples + per-site guidance given in plan).
- **Depends on:** ENG-1 (census table in `perf-baselines.md` to update). Plan order: after ENG-6.
- **Branch / commit:** `feature/engine-phase1`; commit `safety: SAFETY comments on all real unsafe sites + census lint script`.
- **Files:** Create: `scripts/check-safety-comments.sh` (new `scripts/` dir at repo root, `chmod +x`; exact script in plan). Modify (comments only): `common/dynlib/src/lib.rs:74`, `voxygen/anim/src/lib.rs:166` and `:240`, `voxygen/egui/src/lib.rs:196`, `server/agent/src/action_nodes.rs:1220`, `world/src/site/generation.rs:1690`, `voxygen/src/render/renderer/compiler.rs:112` and `:172`, `voxygen/src/ui/ice/winit.rs:17`; plus the census table in `docs/superpowers/specs/perf-baselines.md`.
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 7' steps 1–4 verbatim. Verified census: 13 real sites in 9 files, 4 already commented, **9 need comments**. Use the two worked examples (`dynlib` load, `compiler.rs:112` — must own that `ShaderRuntimeChecks::unchecked()` disables runtime bounds checks); the five `lib.get` sites state symbol/signature match with the dylib's `#[unsafe(export_name)]`; `winit.rs:17` states the raw-window-handle lifetime argument. Comment style: state the *invariant* and why it holds, not the operation.
- **Acceptance:** `./scripts/check-safety-comments.sh` → `OK: all real unsafe sites carry SAFETY comments.`, exit 0. Negative check: temporarily delete one comment → `MISSING SAFETY comment: <file>:<line>` + exit 1; restore. `cargo check -p veloren-common-dynlib -p veloren-voxygen-anim -p veloren-voxygen-egui -p veloren-server-agent -p veloren-world -p veloren-voxygen` clean. Census table updated to 13/13.
- **Size:** M

## ENG-8 — Workstream C: PR template + review pipeline wiring

- **Model:** sonnet — process wiring plus live dispatch of both reviewer agents; fixing an agent definition is in scope if dispatch fails.
- **Depends on:** ENG-2 through ENG-7 (Step 3 exercises the pipeline on this very branch's real diff — Phase 1 exit criterion). ENG-7's safety script must exist (the new review Step 5 runs it).
- **Branch / commit:** `feature/engine-phase1`; commit `process: PR template with perf/safety checklist; reviewer subagents wired into review skill`.
- **Files:** Create: `.github/PULL_REQUEST_TEMPLATE.md` (exact content in plan; verified none exists). Modify: `.claude/skills/veloren-review/SKILL.md` (insert new Step 5 "Specialized Reviewer Subagents + Safety Gate" between current Step 4 and Step 5; renumber 5→6, 6→7).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 8' steps 1–4 verbatim. Step 3: dispatch `rust-perf-reviewer` and `ecs-design-reviewer` (Task tool, `subagent_type` = agent name) on `git diff development...HEAD`; fix accepted findings by looping back to the relevant task's verify step; keep both 3-line verdicts for the PR description.
- **Acceptance:** Both agents produce severity-tagged findings + a 3-line verdict each; per spec §Testing, each produces **at least one accepted finding** on the real diff; blockers fixed, minors fixed or explicitly waived.
- **Size:** M

## ENG-9 — Lint, format, changelog, branch finish (phase gate)

- **Model:** fable — phase-gate review and merge decision (`finishing-a-development-branch`), plus owning any cross-task fix fallout from the final lint/test sweep.
- **Depends on:** ENG-1 through ENG-8 (all).
- **Branch / commit:** `feature/engine-phase1`; commit `docs: changelog entries for engine phase 1`; then merge into `development`.
- **Files:** Modify: `CHANGELOG.md` (three `### Changed` entries verbatim from plan Step 4).
- **Assets:** none.
- **Downloads/tools:** none.
- **Steps:** Follow plan section '### Task 9' steps 1–6 verbatim: `cargo ci-clippy -- -D warnings` + `cargo ci-clippy2 -- -D warnings`; `cargo fmt --all -- --check`; tests (`-p veloren-common -p veloren-voxygen`, `-p veloren-common-frontend --features logging-verbose`) + `./scripts/check-safety-comments.sh`; changelog; assemble the PR Measurement table from every Before/After pair in `perf-baselines.md` plus both ENG-8 agent verdicts; invoke `veloren-review` (including its new Step 5) then `superpowers:finishing-a-development-branch`. No `#[allow]` without a justifying comment.
- **Acceptance:** All lints/format/tests/safety gate clean (exit 0 / PASS); PR description carries the full measurement table and both agent verdicts with the checklist ticked; branch merged into `development`. Phase 2/3 follow-ups remain tracked in the design spec's phase table — do not start them.
- **Size:** M
