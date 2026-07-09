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
| **3** | Voxel meshing, terrain & figures | 🔵 **in progress** — EM-3.1→3.10 + 3.8d + 3.8e + 3.9b + 3.10b done (PRs #7–#20, #26, #28, #29, #30): **real Xindeler terrain + entities + a controllable character + real animated `.vox` figures (quadruped/humanoid/birds) with REAL equipped weapons/armor/lantern/helmets/glider + vegetation sprites & translucent animated water render in Bevy, with frustum + distance-band culling + a real far-mesh horizon**; EM-3.11 **[M]** (Matías in-game smoke) 🔵 in progress — PR #32 (camera/tree/terrain-color/far-mesh fixes) landed, follow-up PR (perf/jump/fog) up next, re-test pending |
| **4** | Server shell, replicon transport & ORACLE foundations | 🔵 **in progress** — EM-4.1 done (PR #27): headless `xindeler-server-app` shell, dual-stack verified. EM-4.2 correctness done (PR #33): persistence + rtsim REAL round-trip, `hot-agent` wired; full 24h soak + EM-4.2b+ pending |
| **5** | UI (bevy_ui+Feathers), audio & playable parity | ⚪ pending |
| **6** | Upstream-sync drills & hardening | ⚪ pending |
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
| EM-3.11 **[M]** | In-game visual smoke (AO/TAA/fog/anims/perf vs old client) | 🔵 PR #32 landed — Matías's first live playthrough found: 3rd-person camera pitch dead (mouse Y did nothing), trees/mushrooms/cave-flora rendering with zero color (palette had no entries → fell to Rock-gray default), ordinary terrain (grass/sand/snow/cave-rock/ice) ALSO zero-color for the same reason, and a walkable-through "green wall" at the horizon (root cause: the far-mesh's cutout hole was anchored at boot instead of following the camera). All 4 fixed + `block_palette.ron` extended with 10 new kind entries + water alpha/color retuned. **EM-3.11b follow-up (this PR):** re-test surfaced 3 more findings — flat ~29fps identical in dev AND `--release` w/ full LTO (real bug: `tick_sim`/`tick_player` ran in `Update` at display rate instead of `FixedUpdate` @ 30Hz, feeding the sim a jittery non-30Hz `dt` every frame; fixed + a `Slow server tick` root cause (50–200ms individual ticks, ~62-entity smoke scene, likely agent-AI/rtsim pathfinding cost) found and documented but NOT chased down — flagged as a follow-up profiling task), jump reported "not working" (sim-side plumbing proven correct end-to-end via a real embedded-sim integration test — `on_ground` gate, impulse, landing all fire correctly; most likely explained by the same stutter making a <1s jump arc hard to perceive, not a real jump bug), and the far-mesh horizon still reading as a hard "wall" (root cause: fog color didn't match the physically-based sky color at the mesh's silhouette edge, not missing fog — retuned fog density/color + far-mesh height-tint to reduce the mismatch, measured via pixel sampling). Remaining before EM-3.11 can close: Matías re-test of this PR + the slow-server-tick profiling follow-up. |

## Phase 4 — Server shell, replicon transport & ORACLE foundations ⚪

AI coordination note (2026-07-07): Phase 4 must leave the server ready to connect with the AI crown-track work without making Engine Migration responsible for implementing ORACLE/AURORA end-to-end. During server preparation, include the runtime seams needed by BL-83 (AURORA/NPC RAG + memory over the local vLLM node) and BL-85 (ORACLE/Bedrock world-director orchestration). Keep AWS account creation, Bedrock model access, Budgets, and paid setup pending until the server foundation is resolved and ORACLE is ready to consume the AWS startup credits effectively.

| Task | What | Status |
|---|---|---|
| EM-4.1 | `xindeler-server-app` — headless `MinimalPlugins` shell embedding the sim; **dual-stack** (old client keeps connecting) | ✅ PR #27 — real `Server::tick` @ 30Hz + SIGINT/SIGTERM graceful shutdown + `/metrics` Prometheus passthrough; dual-stack verified via a real separate-process client connect+play+logout ([Q?] plugins-on-by-default confirmed, Matías 2026-07-09) |
| EM-4.2 | Persistence / rtsim / agent-dylib verified under the shell; 24h soak | 🔵 correctness done (PR #33) — persistence + rtsim REAL round-trip tests (genuine process stop/SIGTERM/restart, state proven to survive not regenerate); `hot-agent` feature wired (was missing entirely); dylib compile+load+watch confirmed, live reload cycle untestable on macOS (documented pre-existing platform limitation) + would need editing the forbidden `server/agent` crate; soak-readiness sanity (10min, RSS flat/declining, tick time well under budget) — full 24h soak run separately, duration reported honestly when it wraps |
| EM-4.2b | Transport backend spike — `renet2` vs `quinnet` (real network, not loopback) | ⚪ |
| EM-4.2c | Login / session handshake bridged to the sim's accounts + persistence | ⚪ |
| EM-4.2d | Interest management — per-client visibility (region/distance + `DimensionId`); bandwidth vs old protocol | ⚪ |
| EM-4.2e | AI gateway readiness handoff — server shell/config/metrics/fallback seams for BL-83 and BL-85; no AWS account or Bedrock setup yet | ⚪ |
| EM-4.3 | `DmEventLoader` — `.dmevent.ron/json` AssetLoader + `oracle://` watch dir (ORACLE writes files) | ⚪ |
| EM-4.4 | Anti-chaos validation layer (clamp tables for injected events) | ⚪ |
| EM-4.5 | `DimensionRegistry` + `DimensionId` + instanced-dimension generation | ⚪ |
| EM-4.6 | Dimension teardown & GC (RAM+VRAM leak-free) | ⚪ |
| EM-4.7 | Generic entity factory v1 (behavior strings → `Agent` presets) | ⚪ |
| EM-4.8 | Narrative hooks (world_rumor → chronicle; on_enter_message → HUD toast) | ⚪ |
| EM-4.9 | E2E event drill (full Ravenloft example, both clients coexisting) | ⚪ |

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
