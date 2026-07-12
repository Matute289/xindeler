<!-- Engine-migration (BL-82) program backlog. Human-readable roll-up of EVERY migration task —
     done and pending — so Matías can see status at a glance. Complements (does NOT replace) the
     verbose per-task board in the PRIVATE design repo (docs/design/tasks/45-engine-migration-tasks.md),
     which carries the full acceptance criteria, findings, and code detail. The general project
     backlog lives in docs/backlog/backlog.md; this file is ONLY the Bevy engine migration. -->

# 🛠️ Engine Migration Backlog — Veloren → Bevy (BL-82)

**What this is:** the single human-scannable status list for the **full migration of Xindeler from
Veloren's bespoke engine to [Bevy](https://bevy.org)**. Every task (done + pending) is here. The
general project backlog is [`backlog.md`](backlog.md); this file is *only* BL-82.

**Where the detail lives** (private `docs/design/` repo):
- Spec: `specs/2026-07-02-bevy-migration-design.md` · Mapper: `specs/2026-07-02-veloren-xindeler-mapper.md`
- Plan: `plans/2026-07-02-bevy-migration-plan.md` · **Full task board (source of truth):** `tasks/45-engine-migration-tasks.md`
- Skill `xindeler-bevy` + review agent `bevy-migration-reviewer`.

**Locked decisions (worksheet 2026-07-02):** Q1=A preserved git history · Q2=A `voxygen/` in-tree
unbuilt reference · **Q3=B `bevy_replicon` netcode** · **Q4=B `bevy_ui` + Feathers UI** · Q5=A pin
Bevy `=0.19.0`. Architecture = **Embedded-Sim + Mirror**: the specs sim stays untouched server-side,
mirrored to a pure-Bevy client over replicon; upstream sync with `gitlab/master` preserved via
logic-crate isolation (CI-enforced) + preserved history.

---

## ⚠️ Bevy is pre-1.0 — breaking changes EVERY release (standing rule)

**Bevy is a young, actively-developed engine — NOT production-frozen. Minor releases (0.19 → 0.20 → …)
routinely carry non-backwards-compatible API changes** (0.19 alone moved the render graph to ECS
schedules, made resources components, and swapped the text stack). So this is a **permanent
maintenance obligation, not a one-off:**

> **Every time a new Bevy version ships, we MUST:** (1) read its official **migration guide +
> release notes** (`bevy.org/learn/migration-guides/…` + `bevy.org/news/…`), (2) catalog the breaking
> changes that touch our `bevy/*` crates, (3) create a spec + plan + tasks for the upgrade, (4) bump
> the pin + fix everything that broke, (5) re-run all gates and the smoke screenshots. This is
> tracked as the standing **EM-M1** track below (and rehearsed once at **EM-6.3**).

The churn is contained on purpose: everything engine-touching lives under `bevy/*`, so an upgrade is
one bounded PR — never a project-wide scramble. Pin exact (`=0.19.0`) so a version never moves by
accident; third-party bevy-ecosystem crates (replicon, etc.) typically lag a new Bevy by 1–3 months,
so an upgrade waits until the dep tree catches up.

---

## Progress at a glance

| Phase | Scope | Status |
|---|---|---|
| **0** | Repos & clean environment | ✅ **complete** (PRs #1, #2, #149) |
| **1** | Logic-crate extraction & modularization | ✅ **complete** (PRs #3, #4, #5) |
| **2** | Bevy core + graphics pipeline | ✅ **complete** (PRs #5, #6) |
| **3** | Voxel meshing, terrain & figures | 🔵 **in progress** — EM-3.1→3.10, 3.8d, 3.8e, 3.9b, 3.10b, 3.11-FH (P-A+P-B), 3.12 all done (PRs #7–#20, #26, #28–#30, #57, #60, #64): **real Xindeler terrain + entities + a controllable character + real animated `.vox` figures (quadruped/humanoid/birds) with REAL equipped weapons/armor/lantern/helmets/glider + vegetation sprites & translucent animated water render in Bevy, with frustum + distance-band culling, a real full-horizon far-mesh (real colour + curvature bend + occlusion dissolve) + camera-collision spring-arm**. Open: EM-3.9c (sprite wind-sway v2, unstarted), EM-3.11-FH Phase C (streamed LOD objects, unstarted), EM-3.11 **[M]** (Matías in-game smoke) 🔵 17 rounds in, all merged — round 17 (PR #71) awaiting his live retest; EM-3.11p (diagonal stutter) not formally closed |
| **4** | Server shell, replicon transport & ORACLE foundations | 🔵 **in progress, essentially content-complete** — EM-4.1→4.12 all done (PRs #27, #33, #45, #46, #49, #52, #58, #59, #61, #65, #66): headless server shell, transport/login/interest-mgmt, dimension lifecycle+teardown+GC, entity factory, narrative hooks, AI-gateway/AURORA readiness seams, full E2E ORACLE event drill (EM-4.9) all real and passing. **Only open item: EM-4.2's full 24h soak run** (10-min soak-readiness sanity done; the multi-hour run itself not yet executed/reported) |
| **5** | UI (bevy_ui+Feathers), audio & playable parity | ⚪ pending |
| **6** | Upstream-sync drills & hardening | ⚪ pending |
| **7** | Visual detail & atmosphere polish (voxel color/texture noise, foliage detail, clouds/rain/sun/stars/moon/wind/wet-ground/canopy-rain, calendar & seasons) | 🔒 **research/spec only for now** (2026-07-09, Opus-authored) — implementation **blocked until Phase 6 completes** (Matías's explicit sequencing); EM-7.1→7.7 + EM-7.9→7.13 scaffolded (EM-7.6/7.9 fully designed + locked; EM-7.8 moved to BL-86) |
| **M1** | 🔁 Bevy version-upgrade watch (standing) | ⚪ recurring — fires on each new Bevy release |

**Legend:** ✅ done · 🔵 in progress · ⚪ pending · 🔒 blocked · 🟣 deferred · **[M]** = needs Matías
(GitHub admin / in-game check / decision).

---

## 🔁 Standing tracks (recurring, never "done")

| ID | Track | Status | Notes |
|---|---|---|---|
| **EM-M1** | **Bevy version-upgrade watch** — on each new Bevy release: read migration guide + notes → spec/plan/tasks → bump pin → fix breaking changes → re-run gates + smoke screenshots. Add the concrete tasks to this backlog when it fires. | ⚪ recurring | See the ⚠️ box above. First real exercise doubles as **EM-6.3**. Ecosystem deps (replicon…) must reach the new Bevy first. Current pin: **`=0.19.0`**. |
| **EM-M2** | **Upstream Veloren sync** — after each `gitlab/master` merge: re-run `tools/xindeler-rename.sh` + `cargo fmt`, triage with Mapper §D (logic crates merge clean; `voxygen/` mesh/anim changes hand-port; scene/render/hud usually skip), log in Mapper §E. | ⚪ recurring | Formalized + drilled at **EM-6.1/6.2**. The whole isolation architecture exists to keep this cheap. |

---

## Phase 0 — Repos & clean environment ✅

| Task | What | Status |
|---|---|---|
| EM-0.1 **[M]** | Rename GitHub repo → `xindeler-old` | ✅ Matías |
| EM-0.2 | `xindeler-old` README DISCONTINUED banner | ✅ PR #149 |
| EM-0.3 **[M]** | Create new `xindeler` repo + replicate branch protection | ✅ Matías + us |
| EM-0.4 | Push preserved history (merge-base with GitLab intact) | ✅ |
| EM-0.5 | Working clone re-pointed to the new repo | ✅ |
| EM-0.6 | CI on the new repo (code-quality required check; `VPS_SSH_KEY` re-created by Matías) | ✅ |
| EM-0.7 | "Migration epoch" — 8 `bevy/*` crate skeletons, Bevy pinned | ✅ PR #2 |
| EM-0.8 | BL-82 backlog row + `xindeler-bevy` skill + reviewer agent | ✅ PR #1 |

## Phase 1 — Logic-crate extraction & modularization ✅

| Task | What | Status |
|---|---|---|
| EM-1.1 | Workspace surgery — `voxygen`+`voxygen/egui` out of members (in-tree unbuilt reference) | ✅ PR #3 |
| EM-1.2 | Engine-isolation CI guard (logic crates never depend on bevy/wgpu/winit) | ✅ PR #3 |
| EM-1.3 | `tools/xindeler-rename.sh` — scripted `veloren-*`→`xindeler-*` (executes old BL-40) | ✅ PR #4 |
| EM-1.4 | `XINDELER_ASSETS` env shim (VELOREN_* fallback) | ✅ PR #4 |
| EM-1.5 | `xindeler-sim-bridge` — SimServer embeds the sim (non-send); 100-tick acceptance passed | ✅ PR #5 |
| EM-1.5b | `xindeler-protocol` — replicon replicated comps + PlayerInput + channels | ✅ PR #5 |
| EM-1.6 | `tools/smoke-bot` — full loopback regression (server+client+char+move) | ✅ PR #5 |

## Phase 2 — Bevy core + graphics pipeline ✅

| Task | What | Status |
|---|---|---|
| EM-2.1 | `xindeler-app` — AppState, SystemSets, RON settings, FPS overlay | ✅ PR #5 |
| EM-2.2 | Client camera + graphics stack — TAA, SSAO, bloom, volumetric+distance fog | ✅ PR #5 |
| EM-2.3 | Light rig — CSM, contact shadows, `Atmosphere` entity, day/night stub | ✅ PR #5 |
| EM-2.4 | `AtmosphereController` — data-driven `.atmo.ron` + **real hot reload** (anti-chaos clamps) | ✅ PR #6 |
| EM-2.5 | `GraphicsTier` presets (Low→Ultra) + experimental Solari/DLSS slot | ✅ PR #6 |
| EM-2.6 | Custom post-process slot (`FullscreenMaterial` vignette) | ✅ PR #6 |

## Phase 3 — Voxel meshing, terrain & figures 🔵

| Task | What | Status |
|---|---|---|
| EM-3.1 | Greedy mesher **lift-copy** from voxygen (fidelity contract, golden tests) | ✅ PR #7 |
| EM-3.2 | Custom vertex attrs (`VOXEL_AO`/`BLOCK_LAYER`) + `Mesh<V>`→`bevy::Mesh` conversion | ✅ PR #8 |
| EM-3.3 | `VoxelMaterialExt` — PBR texture arrays, nearest sampling, **vertex AO indirect-only** | ✅ PR #8 |
| EM-3.4 | **Block palette RON** (kind→layer + PBR params) — data-driven + hot reload + real mips | ✅ PR #9 |
| EM-3.5 | Async chunk pipeline — `AsyncComputeTaskPool` + per-frame upload budget + unload path | ✅ PR #9 |
| EM-3.6 | **Listen-server: real Xindeler terrain streams to Bevy via replicon** 🎯 phase gate (visual) | ✅ PR #10 |
| EM-3.7 | Entity mirror + interpolation — sim entities live on the streamed world | ✅ PR #12 |
| EM-3.7b | Controllable character + input — **walk the world yourself** (embedded Client, 3rd-person cam) | ✅ PR #13 |
| EM-3.8 | Figures (`.vox`) — real assembled voxel models replace capsules (quadruped end-to-end; `NetBody`→full `Body`) | ✅ PR #15 |
| EM-3.8b | Humanoid figures + skeletal animation (armour/recolour/16-bone assembly + idle/walk/run bone matrices) | ✅ PR #17 |
| EM-3.8c | Figure completion — animated quadrupeds/birds + more bodies (quadruped-medium, birds) + humanoid weapon + polish minors | ✅ PR #18 |
| EM-3.8d | Figure gear + polish — real equipped gear from inventory + lantern/back/glider + bird fly hysteresis + dedup nits | ✅ PR #26 — real equipped weapon(s) + armor (new `NetLoadout` mirror comp) + lantern replace the EM-3.8c hardcoded sword; helmets/glider/bird-hysteresis/dedup deferred to EM-3.8e |
| EM-3.8e | Figure gear polish v2 — head-slot helmets (species-keyed head-armour manifest), glider (needs `CharacterState` in the mirror), bird fly/run threshold hysteresis, VoxSimple/LoadedPart dedup nits | ✅ PR #29 — all 4 items landed: real head-armor merge (voxygen's hollow/override union rule, ported verbatim) + a real glider (gated on `CharacterState::Glide`/`GlideWield`, a NaN-producing zero-scale-matrix decode bug caught+fixed along the way) + bird fly/run hysteresis + a `VoxSimple`/`LoadedPart` dedup refactor |
| EM-3.9 | Sprites (grass/props) + fluids v1 (`river_velocity`) | ✅ PR #19 — vegetation sprites (shared-mesh + whitelist + density cap) + translucent water render on streamed terrain; `river_velocity` carried for the EM-3.9b water shader |
| EM-3.9b | Sprites/water polish — UV-scroll water shader, GPU-instanced sprites, sprite attr filters/LOD/wind, furniture/prop kinds, shared decoded-chunk store | ✅ PR #28 — animated water shader (UV drift + river-flow scroll + ripples) landed; sprite whitelist widened 16→55 (full outdoor `Plant` category); GPU instancing confirmed already-optimal (Bevy auto-batching, no code needed); wind-sway attempted + reverted (broke sprite lighting, root-caused, deferred to EM-3.9c); furniture kinds + shared decoded-chunk store deferred to EM-3.9c |
| EM-3.9c | Sprites polish v3 — wind-sway v2 (normal-consistent or non-geometric approach), furniture/prop/dungeon sprite kinds, shared decoded-chunk store | ⚪ |
| EM-3.10 | LOD & culling v1 (distance bands + GPU occlusion) | ✅ PR #20 — Bevy auto-frustum-culls all meshes (verified); added `LodCullingPlugin` distance bands (chunk + nearer sprite-parent, Visibility-toggle, data-driven `CullingConfig`) |
| EM-3.10b | LOD & culling v2 — GPU occlusion culling (DepthPrepass+HZB, measure-gated) + lod-alt far-mesh (needs a bridge→client lod_alt/horizon data path) + `CullingConfig`→RON | ✅ PR #30 — occlusion culling measured (no gain in the smoke scene, shipped opt-in default OFF) + real lod-alt far-mesh fills the horizon (new one-shot `NetLodAlt` bridge→protocol→client data path); `CullingConfig`→RON not attempted (small follow-up) |
| EM-3.12 | **Third-person camera collision (spring-arm)** — stop the camera clipping through solid geometry (Matías: looking up from below, the eye keeps its fixed 9 m boom and passes through the floor → "todo se vuelve transparente"; same for trees/walls/mountains). v1 = spring-arm pull-in only (the exact reported bug **and** full parity with `xindeler-old`, which ships only a spring-arm). **Key finding:** the raycast stays entirely in the pure-Bevy `xindeler-client` crate — the client already holds decoded terrain blocks (`terrain_stream.rs` `SharedTerrain`, already uses `common::vol::ReadVol`+`VolGrid2d`), so it reuses the same voxel DDA ray the physics uses (`phys/collision.rs:548`, `is_solid()`) with no sim round-trip and no new isolation surface; one cast covers terrain+mountains+walls+trees (trees are `Wood`/`Leaves` blocks). Composes with the EM-4.11 eased focus by clamping the *arm length* from the already-eased *pivot* (snap-in / ease-out), applied as the final step (mirrors old voxygen `Camera::update`→`compute_dependents`). Occlusion-fade (Phase 2) + over-the-shoulder (Phase 3) deferred. **Spec/plan/tasks:** `docs/design/specs/2026-07-11-bl82-camera-collision-design.md` + `plans/2026-07-11-bl82-camera-collision-plan.md` + `tasks/52-bl82-camera-collision-tasks.md`. | ✅ PR #64 merged — spring-arm pull-in shipped, `XINDELER_CAMERA_COLLISION=0/1` kill-switch available for further live A/B. Occlusion-fade (Phase 2) + over-the-shoulder (Phase 3) remain ⚪, deferred. |
| EM-3.11 **[M]** | In-game visual smoke (AO/TAA/fog/anims/perf vs old client) | 🔵 17 rounds so far, ALL merged (PRs #32/#34/#36/#37/#39/#40/#41/#43 + round 10 = PR #48, #63/#64/#65/#66/#67/#68/#69/#71): camera, tree/terrain color, far-mesh, perf, fog, ghost-hand/TAA, terrain black-frame, lighting, flicker, vsync, quadruped logging, diagonal stutter, NPC mirroring (fixed), distant-sprite shadow flicker (fixed), slope-descent camera flicker (fixed, round 13), background-disappears-for-1-2-frames (fixed, round 14 — a fogged placeholder-chunk box, not the far mesh), placeholder-box-vs-far-mesh seam artifact (fixed, round 15), tan/beige placeholder patch + distant-tree flicker (fixed, round 16 — `PLACEHOLDER_HAZE_BLEND` retuned 0.2→0.5, tuned for a SINGLE isolated box), a beige frontier STRIP + tree-creation flicker (fixed, round 17 — NOT the isolated-plate bug: live instrumentation measured up to 16 SIMULTANEOUS placeholder chunks per burst, recurring every ~220ms during active exploration, a cluster problem the round-16 blend retune couldn't close without risking round 14's regression; fixed with a real per-chunk `PlaceholderColorHint` sourced from the far-mesh's own terrain-colour grid, gated by a two-part cave-safety guard shaped by 2 `bevy-migration-reviewer` rounds) — round 17 not yet Matías-confirmed live (merged, awaiting his retest). **Full findings log:** `docs/design/specs/2026-07-09-bl82-em311-findings-log.md`. Open: EM-3.11p (diagonal-movement stutter, 5 rounds in, root cause still unidentified — needs a call on whether to keep going; Matías reported it much improved by round 10 but not formally closed). |
## Phase 4 — Server shell, replicon transport & ORACLE foundations 🔵

AI coordination note (2026-07-07): Phase 4 must leave the server ready to connect with the AI crown-track work without making Engine Migration responsible for implementing ORACLE/AURORA end-to-end. During server preparation, include the runtime seams needed by BL-83 (AURORA/NPC RAG + memory over the local vLLM node) and BL-85 (ORACLE/Bedrock world-director orchestration). Keep AWS account creation, Bedrock model access, Budgets, and paid setup pending until the server foundation is resolved and ORACLE is ready to consume the AWS startup credits effectively.

| Task | What | Status |
|---|---|---|
| EM-4.1 | `xindeler-server-app` — headless `MinimalPlugins` shell embedding the sim; **dual-stack** (old client keeps connecting) | ✅ PR #27 — real `Server::tick` @ 30Hz + SIGINT/SIGTERM graceful shutdown + `/metrics` Prometheus passthrough; dual-stack verified via a real separate-process client connect+play+logout ([Q?] plugins-on-by-default confirmed, Matías 2026-07-09) |
| EM-4.2 | Persistence / rtsim / agent-dylib verified under the shell; 24h soak | 🔵 correctness done (PR #33) — persistence + rtsim REAL round-trip tests (genuine process stop/SIGTERM/restart, state proven to survive not regenerate); `hot-agent` feature wired (was missing entirely); dylib compile+load+watch confirmed, live reload cycle untestable on macOS (documented pre-existing platform limitation) + would need editing the forbidden `server/agent` crate; soak-readiness sanity (10min, RSS flat/declining, tick time well under budget) — full 24h soak run separately, duration reported honestly when it wraps |
| EM-4.2b | Transport backend spike — `ReplicaTransport` abstraction + `quinnet` impl (real network, not loopback); renet2 evaluated, not adopted (lags our Bevy/replicon pins) | ✅ PR #46 merged, reviewed clean, real dual-transport acceptance test passing |
| EM-4.2c | Login / session handshake bridged to the sim's accounts + persistence | ✅ PR #46 merged, reviewed clean, real login-handshake acceptance test passing (offline + online-mode paths, duplicate-login kick) |
| EM-4.2d | Interest management — per-client visibility (region/distance + `DimensionId`); bandwidth vs old protocol | ✅ PR #46 merged, reviewed clean, ~2x measured bandwidth improvement (party-scale scene) |
| EM-4.2e | AI gateway readiness handoff — `AiExecutionMode` (Offline/LocalOnly/Full) + config/metrics/fallback seams for BL-83 and BL-85; no AWS account or Bedrock setup yet | ✅ PR #46 merged, reviewed clean |
| EM-4.2f | **AURORA/NPC entity-model readiness** — `NetUid` identity + full `AuroraOverlay` schema (memory/intention/mood, neutral-default population outside Offline mode). Scope widened 2026-07-10 (worksheet Q2) | ✅ PR #49 merged (folded in alongside EM-4.5-4.8) |
| EM-4.3 | `DmEventLoader` — `.dmevent.ron/json` AssetLoader + `oracle://` watch dir (ORACLE writes files) | ✅ PR #45 merged, reviewed clean (bundled with EM-4.4) |
| EM-4.4 | Anti-chaos validation layer (clamp tables for injected events) | ✅ PR #45 merged (see EM-4.3, one implementation PR) |
| EM-4.5 | `DimensionRegistry` + `DimensionId` + instanced-dimension generation — full Spinup→Active→Draining→Teardown lifecycle (worksheet Q4=maximalist) | ✅ PR #46 merged, reviewed clean, real lifecycle acceptance test passing |
| EM-4.6 | Dimension teardown & GC (RAM+VRAM leak-free) + heuristic predictive GC (worksheet Q4=maximalist) | ✅ PR #49 merged (folded into the combined Wave-3 PR), reviewed clean (2 blocker/major findings fixed: same-frame sim-delete ordering race, unguarded `DimensionId::DEFAULT` drain-lockout), real RAM-delta + `Assets<Mesh>` VRAM-baseline acceptance tests passing |
| EM-4.7 | Generic entity factory v1 (behavior strings → `Agent` presets) | ✅ PR #49 merged (folded into the combined Wave-3 PR), reviewed clean (bevy-migration-reviewer + ecs-design-reviewer, minor follow-ups only) — `EntityTemplate` RON/JSON asset + `ComponentSpawnRegistry` + `AgentPreset` (stalk/aggro/flee/passive over the existing `server-agent` `Agent`) + `spawn_from_spawning_rules` batch spawn off a `DmEvent.spawning_rules`; real acceptance test spawns 15 clamped stalker minions into the default dimension. Wired into a production `App` by EM-4.9's ingestion→spawn producer system |
| EM-4.8 | Narrative hooks (world_rumor → chronicle; on_enter_message → HUD toast) | ✅ PR #49 merged (folded into the combined Wave-3 PR), reviewed clean (bevy-migration-reviewer + ecs-design-reviewer) |
| EM-4.9 | E2E event drill (full "the Mist-Bound" example, both clients coexisting) | ✅ PR #58 merged. Wires the dormant `DmEvent`→spinup→spawn→atmosphere→toast producer chain into `xindeler-sim-bridge::oracle::ServerOraclePlugin`; ships `mist_bound.dmevent.ron` + a grey-undead `husk` entity template; routes real minions into a non-default `DimensionId` (best-effort correlation-queue attribution, documented limits); adds `SetClientAtmosphere` targeted sync. **Full E2E drill passes**: boots the real server binary, connects both a legacy and a real net-client simultaneously, drops the event file, observes 15 real minions spawn + the event dimension reach `Active`, retires the event, observes clean teardown, both clients stay connected throughout. Caught+fixed a real `bevy_asset` 0.19 gotcha along the way (`AssetEvent::Removed` doesn't fire on file deletion while a handle is outstanding — retire now polls filesystem existence instead). Reviewed clean by all three reviewers (a few majors found+fixed: registry-purge-on-retire, stale spawn-radius doc/bound, ordering-edge follow-up). **Not done** (documented, deferred): no live player-transfer trigger moving a connected player into the event dimension; minions still share the one real specs World/terrain (no true per-dimension physics — pre-existing gap). **EM-4.9b (2026-07-11): the per-dimension-physics gap is now researched + designed** — options survey (A spatial "instance plots" in the one shared world [recommended v1, upstream-idiomatic], B N independent `Server` instances [true multi-world; blocked by hard-coded `ListenAddr::Mpsc(14004)` + per-instance persistence + N× cost], B-lite shared-world query filtering [rejected: needs invasive logic-crate edits vs the isolation law], C hybrid) + recommendation (Option A, since Mist-Bound/near-term events need only atmosphere+NPCs, already met) in `docs/design/specs/2026-07-11-em49b-per-dimension-physics-research.md` (+plan+tasks 55). Still ⚪/🔒 — not scheduled, pending a concrete need + option pick. |
| EM-4.10 **[regression]** | **Wave-2+3 post-merge regression hardening** (PR #49 = merge `5e5c2dc849`): real gameplay regressed — FPS oscillating 28↔160 Hz, black frames, unmasked beige horizon. 9 findings verified against committed HEAD: A `DimensionMembers` `Vec`→`EntityHashSet` (O(n) per-frame churn on `DimensionId::DEFAULT` = primary FPS cause), B dimension-lifecycle systems `Update`→`FixedUpdate`, C mirror/aurora per-tick buffer allocs, D `begin_draining` stuck-`Teardown` on DEFAULT, E unsanctioned `server/` login-widening + silent replicon inventory-drop (isolation-law blocker), F stale "only-specs-consumer" doc, G "Ravenloft" WotC-trademark scrub → "the Mist-Bound", H `predictive_gc` consts → RON, I `ComponentSpawnRegistry` YAGNI. Plan groups **P0**(A+B+C+D perf hotfix)/P1(E+F)/P2(G)/P3(H+I)/P4(beige-horizon diagnosis, gated on P0). Corroborated by `specs/2026-07-10-bevy-performance-guidelines.md`. **Spec/plan/tasks:** `docs/design/specs/2026-07-10-bl82-wave3-regression-fixes-design.md` + `plans/…` + `tasks/48-bl82-wave3-regression-fixes-tasks.md`. | ✅ PR #52 merged — all 9 findings (A–I) fixed |
| EM-4.11 **[root-cause fix]** | **Frame-rate local-player prediction** — the port already embeds a full, correct client-side predictor (`xindeler-client-core::Client`, same predictor old voxygen used) but it ticked at 30Hz `FixedUpdate` and its own prediction was discarded for rendering (the render eased toward the 30Hz-sampled `NetPos` instead) — root cause of the residual FPS oscillation + tick-quantization/landing-lag bug family post-EM-4.10. Fix: `tick_player` moved `FixedUpdate`→`Update` (ticks once per rendered frame; `Server::tick` UNCHANGED, stays `FixedUpdate` @ 30Hz); embedded `Clock` now `target_dt=Duration::ZERO` (pure dt-smoother, no more spin_sleep pacing — Bevy owns frame pacing); new `PredictedLocalTransform` component (non-replicated) written by `mirror_local_player_prediction`; the local player's render now snaps to it directly instead of easing `NetPos`. Retired the now-dead EM-3.11r `LOCAL_PLAYER_POS_LERP_RATE`/landing-gap diagnostic (captured before/after numbers first, per plan). **Spec/plan/tasks:** `docs/design/specs/2026-07-11-bl82-frame-rate-prediction-design.md` + `plans/…` + `tasks/49-bl82-frame-rate-prediction-tasks.md`. | ✅ **Phase A+B done** (T49.1–T49.3), PR #59 merged, reviewed clean (bevy-migration-reviewer + ecs-design-reviewer + rust-perf-reviewer, all three per the plan since `tick_player` is now a per-frame hot path — no blockers/majors from any). Real `--smoke-perf-run` before/after: mean frame time roughly halved (34.5ms→~17-21ms straight leg, 33.9ms→16.7ms diagonal), the rolling FPS envelope went from oscillating 16-145fps to steady 61-82fps, and the literal 30Hz tick-quantization signature (isolated single-frame zero-delta render steps during a straight walk) dropped from 101 occurrences to 0. A caught-and-fixed implementation bug along the way: an explicit cross-plugin `Update` ordering constraint was needed for the write (`mirror_local_player_prediction`) to be visible same-frame to the read (`interpolate_entities`) — without it the fix silently didn't fully apply; also switched that write from `Commands::insert` every frame to an in-place mutation per reviewer feedback. Phase C (sim-pacing hardening) evidence-gated — not needed, B's own acceptance data didn't show residual oscillation warranting it. Phase D (diagonal streaming-churn) shipped separately: PR #56. **Follow-up bug found post-merge (round 13, `docs/design/specs/2026-07-09-bl82-em311-findings-log.md`):** the direct-snap render (both the player's own `Transform` and the third-person camera that follows it) has zero jitter horizontally but not vertically — walking over sloped/stepped voxel terrain produces genuine per-tick ground-contact discontinuities (measured live: up to a 1.00 m single-frame pop) that used to be masked by the retired 60/s ease and, in old voxygen, by the camera's own always-on focus lerp (`THIRD_PERSON_INTERP_TIME`), which this port had dropped entirely. Fixed by giving `third_person_camera` its own decoupled, eased follow-focus (`smoothed_focus`, rate ported 1:1 from old voxygen's `1/0.1s`), scoped to the camera only — the player entity's own rendered `Transform` stays a direct snap, so this does not touch or regress the quantization fix above. Measured 10× reduction in max single-frame delta, 5× lower RMS jitter, same net convergence. **Follow-up (4-reviewer pass, rust-perf-reviewer MAJOR):** the `Update` move itself is correct and NOT reverted/capped back to 30Hz — but the spec's own §3 risk list named a mitigation ("clamp to a max Hz") that was never implemented, leaving no upper safety ceiling for uncapped/very-high-refresh setups. Added a real Hz safety ceiling in `tick_player` (`bevy/xindeler-sim-bridge/src/player.rs`): tracks wall-clock time since the last real `client.tick()` dispatch and skips the redundant dispatch (render still proceeds unaffected) under a minimum interval, default 240Hz (`DEFAULT_MAX_PLAYER_TICK_HZ`, comfortable headroom above any normal 60/120/144Hz monitor), configurable/disableable via `XINDELER_MAX_PLAYER_TICK_HZ` (`<=0` opts out). Added an opt-in `XINDELER_PLAYER_TICK_PERF_LOG=1` diagnostic (`target: "player_tick_perf"`) to measure the real dispatch Hz empirically. Verified via `--smoke-perf-run`: at this machine's natural ~60Hz headless render rate (both default `Fifo` and `XINDELER_PRESENT_MODE=novsync` — the sandboxed macOS environment doesn't composite the window, so `novsync` couldn't push the render loop past ~60Hz here, matching `smoke.rs`'s own documented uncomposited-window caveat) `tick_player` ran at ~60Hz with ~0 skipped frames, i.e. completely unaffected; with the ceiling artificially lowered to 20Hz the measured real-tick rate clamped to ~15-17Hz (skipping the majority of frames) while the render loop itself kept running unaffected (~60fps, no stall) — proving the clamp engages and bounds the dispatch rate without touching rendering. Two new unit tests (`max_player_tick_interval_defaults_and_respects_overrides`, `tick_is_due_gates_on_elapsed_wall_clock_time`) pin the env-parsing and gating logic with real `Instant`/`Duration` math. |
| EM-3.11-FH | **Full-horizon LOD terrain — the structural beige-horizon fix** (follow-up to EM-3.11 round-11 fog retune `114a079664` + EM-4.10 P4, which its own commit message flagged as the real fix). `record9.mov` still shows a flat, undetailed distant plateau with a hard edge = "the map ends here". Root cause: the Bevy far-terrain pipeline renders a strictly weaker world than the old engine (heightmap-only `NetLodAlt` → flat synthetic `height_tint`, masked solely by fog) and leans on fog to hide it. **Key finding: the richer LOD data already exists server-side** in `client::WorldData` (`lod_base` colour + `lod_horizon` occlusion, the same struct the old client used) — `send_lod_alt_once` samples only `alt_at` and discards it; a sampling+transport+rendering task, not worldgen. **Phased:** P-A real colour (sample `lod_base`, send `colors`, bake real vertex colour — closes the symptom); P-B `lod_horizon` occlusion + atmospheric silhouette blend + restore near/mid fog clarity (Path-1 custom material vs Path-2 texture-driven spiral = Matías fork); P-C streamed LOD objects (distant trees/houses). **Spec/plan/tasks:** `docs/design/specs/2026-07-11-bl82-full-horizon-lod-terrain-design.md` + `plans/2026-07-11-bl82-full-horizon-lod-terrain-plan.md` + `tasks/50-bl82-full-horizon-lod-terrain-tasks.md`. | ✅ P-A shipped (PR #57). **P-B shipped (PR #60):** Matías's Path decision resolved to the **A+C synthesis** — `client::WorldData::horizon_at` (T49.5) + server-side `lod_horizon` sampling (index-aligned with heights/colors, verified against a real booted world) + a new `ExtendedMaterial<StandardMaterial, FarTerrainExtension>` (`far_terrain_material.rs`/`.wgsl`, T49.6): a vertex-shader world-curvature bend (`drop = bend_strength·(dist − bend_start)²`, zero across the near band by construction) plus a fragment-shader soft horizon-occlusion + fog-colour dissolve, composing with (not replacing) `DistanceFog`. `fog_density` restored `0.00913→0.00667` (T49.7); the round-11 fog-coverage regression test re-expressed as a looser, explicitly-reasoned invariant now that the material does the edge-hiding. Shipped default `bend_strength = 0.00005` (imperceptible near the seam, ~12 m drop by 500 m beyond it — see `far_terrain_material.rs` doc comments for the full reasoning); `0.0`/`XINDELER_FAR_MESH_BEND_STRENGTH` env override both verified as clean disables. Verified: real-world sim-bridge integration test (horizon index-alignment), unit tests (bend formula, per-quad horizon bake), and `XINDELER_SMOKE_FAR_MESH_CAM=1` live renders (confirmed the bend visibly deforms the mesh with no seam at an exaggerated strength; caught and fixed a real bug live — the fragment dissolve was blending toward the dark `sky_color` void tint instead of the pale `fog_color` haze tone). Since then hardened further by EM-3.11 rounds 14-17 (placeholder-chunk colour/blend fixes, PRs #63/#67/#69/#71). P-C (streamed LOD objects) remains ⚪, unstarted. |
| EM-4.12 **[cleanup]** | **Data-driven-content cleanup** (comprehensive architecture review, 3 MODERATE findings): (1) `xindeler-sim-bridge::oracle`'s `WELL_KNOWN_EVENT_FILENAMES` compiled-in event-name list → `OracleEventManifest` RON asset (`assets/xindeler/oracle_events/manifest.oracle_manifest.ron`, mirrors `PredictiveGcAsset`/T48.6); (2) `far_terrain_material::FAR_MESH_BEND_STRENGTH` + the mesh's `bend_start` → two new `AtmosphereProfile` fields (`far_mesh_bend_strength`/`far_mesh_bend_start_scale`, floored ≥1.0 to preserve the near/far seam invariant) in `default.atmo.ron`, hot-reloadable + per-biome DmEvent-overridable like every sibling atmosphere knob; (3) the `dm_event::bounds::SPAWN_RADIUS` vs. `event_gen_opts()` world-size cross-crate invariant (which broke silently once before) is now pinned by a unit test computing the real half-extent instead of a doc-comment-only cross-reference. | ✅ PR #65 merged, reviewed clean (bevy-migration-reviewer + game-architecture-reviewer, no blockers/majors, only optional nits) |

## Phase 5 — UI (bevy_ui + Feathers), audio & playable parity ⚪

| Task | What | Status |
|---|---|---|
| EM-5.1 | UI foundation — Xindeler widget kit + theme on `bevy_ui`+Feathers | ⚪ |
| EM-5.2→5.8 | HUD screens — health/buffs, hotbar+cooldowns, chat, map, bag/trade, diary, char-select/creation | ⚪ |
| EM-5.9 | Main menu + server browser / login flow | ⚪ |
| EM-5.10 | Audio (`bevy_audio`/`bevy_kira_audio`) — existing `.ogg`/spatial | ⚪ |
| EM-5.11 | Input rebinding (persisted keymap, gamepad) | ⚪ |
| EM-5.12 | Settings menu (graphics tiers, audio, controls, i18n) | ⚪ |
| EM-5.13 **[M]** | Full parity play session → **cutover decision** | ⚪ |

## Phase 6 — Upstream-sync drills & hardening ⚪

| Task | What | Status |
|---|---|---|
| EM-6.1 | Real sync drill #1 (`gitlab-master-merger` + Mapper §D triage), time it, fix friction | ⚪ |
| EM-6.2 | Sync drill #2 → target <1 day routine; write the runbook into the skill | ⚪ |
| EM-6.3 | **Bevy version-bump rehearsal** (first EM-M1 exercise — proves churn is contained to `bevy/*`) | ⚪ |
| EM-6.4 | Retire interim pieces per cutover (legacy quinn listener off, old client archived) | ⚪ |
| EM-6.5 | Post-migration perf review (FPS/frame-time/RAM/VRAM/server-tick vs old client) | ⚪ |

## Phase 7 — Visual detail & atmosphere polish 🔒

**Sequencing decided 2026-07-09 (Matías):** scope = everything Matías flagged as "missing/flat" beyond
the EM-3.11(b) bug fixes — new rendering *content*, not bug fixes. **Research + spec/plan/tasks authoring
starts NOW** (Opus 4.8-authored, per this program's delegation convention — Opus authors specs/plans/
tasks, Sonnet/Haiku implement), but **implementation is explicitly BLOCKED until Phase 6 completes** —
Matías wants the phases done in order (finish the EM-3.11b re-test → Phase 4 → Phase 5 → Phase 6 →
*then* Phase 7), not run in parallel as first floated.

**Worksheet locked 2026-07-09 (maximalist v1 — no cost-cut, "no hay restricción de tiempo"):** every item
below builds its full-fidelity form in v1 (real volumetric clouds, geometric instanced foliage, hybrid
weather grid, wind vertex-displacement, wet-surface PBR, canopy rain interception, deterministic
server-synced day/night). **Core principle (Matías): all graphics processing is CLIENT-SIDE — the server
stays minimal**, holding only what's needed for player-relevant relationships/state and leaving knobs
ORACLE can eventually adjust; it must never become a rendering/graphics workload. Given the **real,
still-unresolved perf/stutter issue found in EM-3.11b** (measured ~29fps, real bug fixed but a residual
"slow tick" root cause not yet nailed down), every one of these systems needs a measured perf gate before
it ships enabled-by-default — this is not optional polish, it directly stacks on an open performance risk.
**EM-7.8 (birds) was extracted out of this phase** — Matías wants real (non-AI) entities for it, which is
gameplay/entity-model scope beyond "atmosphere polish"; tracked as its own epic, **BL-86**, in the general
backlog. Each row below links its spec/plan/task doc once drafted.

| Task | What | Status |
|---|---|---|
| EM-7.1 | Per-voxel color/texture variation — v1 = read the mesher's authored per-voxel `ColLight.col` (currently computed then discarded) into the palette shader, plus position-hash jitter + procedural per-vertex AO at block seams (maximalist v1, worksheet 2026-07-09) | ⚪ |
| EM-7.2 | Geometric foliage — v1 = real instanced 3D grass-blade/leaf meshes (not billboards), shadow-casting, independently wind-reactive (maximalist v1, worksheet 2026-07-09) | ⚪ |
| EM-7.3 | Clouds — v1 = real raymarched volumetric clouds in Bevy's render graph, forward light scattering, physically traversable (maximalist v1, worksheet 2026-07-09) | ⚪ |
| EM-7.4 | Rain (weather) — v1 = hybrid: minimal server-side low-res weather grid (state sync only) + full client GPU rendering (`bevy_hanabi`), wind-coupled diagonal rain, canopy interception/secondary drip, wet-surface dynamic PBR (maximalist v1, worksheet 2026-07-09) | ⚪ |
| EM-7.5 | Visible sun disc — v1 = Bevy 0.19's first-party `SunDisk` component on the existing `Sun` light entity (nearly free once found) | ⚪ |
| EM-7.6 | Night sky — stars — v1 = real astronomical star map + Xindeler-canon constellations, with clean (currently-empty) hooks for ORACLE to later mutate the sky during narrative events. **Design authored** (spec/plan + tasks/53); zodiac **= 13 signs**, 1:1 with the 13-month calendar (13th = the Doorless's unnamed sign, canon `lore/01-calendar.md §VII`). Blocked until Phase 6. | ⚪ |
| EM-7.7 | Moon — v1 = real dynamic lunar phases, deterministically clocked (maximalist v1, worksheet 2026-07-09) | ⚪ |
| ~~EM-7.8~~ | ~~Ambient wildlife — birds~~ **moved to BL-86** (general backlog) — Matías wants real, non-AI entities; that's entity-model/gameplay scope, not atmosphere polish | ➡️ BL-86 |
| EM-7.9 | Calendar & seasons — v1 = looping 364-day calendar (7-day week · 13 months × 28 · 4 seasons × 91), physically-plausible solstice/equinox day-length + gradual (cosine) season transitions, all derived from the synced `TimeOfDay`; empty ORACLE `SeasonOverride` hook. **Design authored + §9 decisions LOCKED** (spec/plan + tasks/54; calendar canon `lore/01-calendar.md`; Q-PHO=A re-canons `DAYS_IN_MONTH` 40→28). Blocked until Phase 6. | ⚪ |
| EM-7.10 | Wind vertex-displacement shaders — v1 = procedural-noise-driven vertex displacement physically bending foliage (trees/plants/flowers) and creature/figure fur, from a shared client-side wind field (sourced from the synced weather-state grid, modulated by noise for gusts); normal-consistent by construction (supersedes the reverted EM-3.9c sprite-sway, whose bug was displacement breaking sprite lighting) (maximalist v1, worksheet 2026-07-09; private spec `2026-07-09-visual-detail-atmosphere-polish.md` §2.9) | ⚪ |
| EM-7.11 | Dynamic wet-ground PBR — v1 = exposed blocks get roughness/metallic modified live during rain (darkened albedo + lowered roughness for wet sheen), driven by the weather-state grid and gated by canopy interception (EM-7.12) so only rain-exposed ground wets; ramps in/out with rain intensity, dries over time (maximalist v1, worksheet 2026-07-09; private spec §2.10) | ⚪ |
| EM-7.12 | Canopy rain interception — v1 = real rain occlusion by tree canopies (a depth/occlusion pass, legacy `rain_occlusion` as the technique reference) producing dry zones under dense foliage, plus a secondary procedural drip effect at canopy edges; couples EM-7.4 (rain), EM-7.2 (canopy geometry) and EM-7.11 (dry = not wet) (maximalist v1, worksheet 2026-07-09; private spec §2.11) | ⚪ |
| EM-7.13 | Weather↔wind physics coupling — v1 = the shared wind field physically drives weather visuals: rain vectors go diagonal under wind, particle drag reacts to storm intensity, clouds advect on the same wind — one wind source feeds foliage (EM-7.10), rain (EM-7.4) and clouds (EM-7.3) coherently (maximalist v1, worksheet 2026-07-09; private spec §2.12) | ⚪ |

**Numbering note:** EM-7.10→7.13 were originally drafted as EM-7.9→7.12 inside the private atmosphere
spec before the public EM-7.9 slot was claimed by Calendar & Seasons (2026-07-11) — renumbered
2026-07-11 (`docs/design` commit `6d8ca5b`) to free it cleanly; these four rows were only just now
added to this public table (they existed in the private spec all along but were never surfaced here).

**Day/night ↔ sim sync:** `SunCycle` (client-local real-time stub) gets a read-only mirror of the sim's
authoritative `TimeOfDay` as part of this phase — cheap correctness win Matías asked for explicitly (was
previously unsynced, clients could each show a different sky).

---

## 🔻 Low-priority / deferred (unscheduled)

| ID | Task | Status |
|---|---|---|
| **EM-L1** | **Asset rebrand (`veloren-*` names → `xindeler-*`) + asset-sync mapper.** Everything in Xindeler's CODE is already rebranded (crates, binaries, identifiers — EM-1.3); **the world, sim and game are Xindeler**. The ONE remaining Veloren-named surface is **assets** — `.vox`/`.png`/`.ogg`/`.ttf` file names + their RON load-path strings (frozen on purpose per the migration spec §0 constraint 2, so upstream `gitlab/master` assets keep flowing in unbroken). This task: **(a)** investigate whether those asset names/paths CAN be renamed to Xindeler given how many RON manifests + code strings reference them (measure scope + risk); **(b)** if feasible, build the **asset-sync mapper** — the asset analog of the code `tools/xindeler-rename.sh` + Mapper §B — that maps an upstream Veloren asset (name/path) to our renamed Xindeler one, so each `gitlab/master` sync can still pull the upstream asset's UPDATED CONTENT into our renamed file. Without that mapper a rename would sever the ability to keep our assets current from upstream, so the mapper is the enabling piece. **Low priority:** the frozen-names approach works fine today; this is consistency polish and it RAISES the upstream-merge conflict surface, so weigh it against the sync cost. Author a spec/plan/tasks in `docs/design/` before touching anything. Relates to EM-M2 (upstream sync) + the code Mapper (§B). Note: some Veloren strings must stay regardless (wire-protocol magic, plugin ABI, DB migrations) — those are NOT assets and out of scope. | ⚪ |

---

## Adding tasks

New migration work (a Bevy upgrade, a discovered gap, an emerged sub-task) gets:
1. its detail in the private board `docs/design/tasks/45-engine-migration-tasks.md` (acceptance criteria),
2. a spec/plan there if it's substantial (e.g. a Bevy-version-upgrade spec when EM-M1 fires),
3. a row **here** so the human roll-up stays complete,
4. and — if it changes the program's shape — a note on the BL-82 row in the general `backlog.md`.

**Timeline estimate:** ~5–7 months solo + Claude to full parity. The Phase-3 visual gate (real terrain
in Bevy) is **met**; the remaining lift is entities/UI/audio/server-shell + the sync/version drills.
