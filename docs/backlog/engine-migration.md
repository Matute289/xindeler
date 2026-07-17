<!-- Engine-migration (BL-82) program backlog. Human-readable roll-up of EVERY migration task —
     done and pending — so Matías can see status at a glance. Complements (does NOT replace) the
     verbose per-task board in the PRIVATE design repo (docs/design/tasks/45-engine-migration-tasks.md),
     which carries the full acceptance criteria, findings, and code detail. The general project
     backlog lives in docs/backlog/backlog.md; this file is ONLY the Bevy engine migration.

     Row shape (2026-07-15 compression pass): ID | short name/scope | Status (+ PR when done) | Docs.
     Every row's "Docs" column is a terse pointer to the private-repo doc(s) that carry the full
     acceptance criteria / root-cause narrative / measurements for that row — keep rows short here,
     put the essay there. -->

# 🛠️ Engine Migration Backlog — Veloren → Bevy (BL-82)

**What this is:** the single human-scannable status list for the **full migration of Xindeler from
Veloren's bespoke engine to [Bevy](https://bevy.org)**. Every task (done + pending) is here. The
general project backlog is [`backlog.md`](backlog.md); this file is *only* BL-82.

**Where the detail lives** (private `docs/design/` repo):
- Spec: `specs/2026-07-02-bevy-migration-design.md` · Mapper: `specs/2026-07-02-veloren-xindeler-mapper.md`
- Plan: `plans/2026-07-02-bevy-migration-plan.md` · **Full task board (source of truth):** `tasks/45-engine-migration-tasks.md`
- Skill `xindeler-bevy` + review agent `bevy-migration-reviewer`.
- Every row below also carries its own **Docs** column pointing at the dedicated spec/plan/tasks doc(s)
  for that row, where one exists — that's where root causes, measurements, and file-by-file detail live.

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
| **3** | Voxel meshing, terrain & figures | 🔵 **in progress** — EM-3.1→3.12 all shipped: real terrain, entities, a controllable character, animated `.vox` figures with real gear, vegetation/water, LOD+culling, camera collision, and full-horizon LOD terrain. Open: **EM-3.11 [M]** in-game smoke, 25 rounds in — round 25 fixed the round-21 sun-rotation throttle stepping visibly instead of sweeping smoothly, awaiting live retest (see its own row). |
| **4** | Server shell, replicon transport & ORACLE foundations | ✅ **complete** — EM-4.1→4.12 all shipped (headless server shell, transport/login/interest-mgmt, dimension lifecycle, entity factory, narrative hooks, full E2E ORACLE event drill + live player-dimension transfer); the full 24h soak run (EM-4.2) passed 2026-07-15 (RSS 706MB→994MB over 86400s, tick time within bounds, no leak signature). |
| **5** | UI (bevy_ui + widget kit), audio & playable parity | 🔵 **in progress** — EM-5.1→5.3, 5.4–5.8 all shipped; worksheet locked 2026-07-11 (maximalist v1 — full parity, "reemplazo total"); Wave C (menu/audio/settings/crafting/char-select/accessibility) + the EM-5.13 cutover gate still pending; **EM-5.17 (Notion-documented HUD replacement, HUD-D4 art) all 8 phases PR'd, none merged yet; EM-5.18 (equipment panel redesign — tab split + D4 click-to-equip modal) follow-up in progress, P1 dispatched.** |
| **6** | Upstream-sync drills & hardening | ⚪ pending |
| **7** | Visual detail & atmosphere polish (voxel color/texture noise, foliage detail, clouds/rain/sun/stars/moon/wind/wet-ground/canopy-rain, calendar & seasons) | 🔒 **research/spec only for now** — implementation **blocked until Phase 6 completes** (Matías's explicit sequencing); EM-7.1→7.13 scaffolded (EM-7.6/7.9 fully designed + locked; EM-7.8 moved to BL-86). |
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

| Task | What | Status | Docs |
|---|---|---|---|
| EM-0.1 **[M]** | Rename GitHub repo → `xindeler-old` | ✅ Matías | `tasks/45-engine-migration-tasks.md` |
| EM-0.2 | `xindeler-old` README DISCONTINUED banner | ✅ PR #149 | `tasks/45-engine-migration-tasks.md` |
| EM-0.3 **[M]** | Create new `xindeler` repo + replicate branch protection | ✅ Matías + us | `tasks/45-engine-migration-tasks.md` |
| EM-0.4 | Push preserved history (merge-base with GitLab intact) | ✅ | `tasks/45-engine-migration-tasks.md` |
| EM-0.5 | Working clone re-pointed to the new repo | ✅ | `tasks/45-engine-migration-tasks.md` |
| EM-0.6 | CI on the new repo (code-quality required check; `VPS_SSH_KEY` re-created by Matías) | ✅ | `tasks/45-engine-migration-tasks.md` |
| EM-0.7 | "Migration epoch" — 8 `bevy/*` crate skeletons, Bevy pinned | ✅ PR #2 | `tasks/45-engine-migration-tasks.md` |
| EM-0.8 | BL-82 backlog row + `xindeler-bevy` skill + reviewer agent | ✅ PR #1 | `tasks/45-engine-migration-tasks.md` |

## Phase 1 — Logic-crate extraction & modularization ✅

| Task | What | Status | Docs |
|---|---|---|---|
| EM-1.1 | Workspace surgery — `voxygen`+`voxygen/egui` out of members (in-tree unbuilt reference) | ✅ PR #3 | `tasks/45-engine-migration-tasks.md` |
| EM-1.2 | Engine-isolation CI guard (logic crates never depend on bevy/wgpu/winit) | ✅ PR #3 | `tasks/45-engine-migration-tasks.md` |
| EM-1.3 | `tools/xindeler-rename.sh` — scripted `veloren-*`→`xindeler-*` (executes old BL-40) | ✅ PR #4 | `tasks/45-engine-migration-tasks.md` |
| EM-1.4 | `XINDELER_ASSETS` env shim (VELOREN_* fallback) | ✅ PR #4 | `tasks/45-engine-migration-tasks.md` |
| EM-1.5 | `xindeler-sim-bridge` — SimServer embeds the sim (non-send); 100-tick acceptance passed | ✅ PR #5 | `tasks/45-engine-migration-tasks.md` |
| EM-1.5b | `xindeler-protocol` — replicon replicated comps + PlayerInput + channels | ✅ PR #5 | `tasks/45-engine-migration-tasks.md` |
| EM-1.6 | `tools/smoke-bot` — full loopback regression (server+client+char+move) | ✅ PR #5 | `tasks/45-engine-migration-tasks.md` |

## Phase 2 — Bevy core + graphics pipeline ✅

| Task | What | Status | Docs |
|---|---|---|---|
| EM-2.1 | `xindeler-app` — AppState, SystemSets, RON settings, FPS overlay | ✅ PR #5 | `tasks/45-engine-migration-tasks.md` |
| EM-2.2 | Client camera + graphics stack — TAA, SSAO, bloom, volumetric+distance fog | ✅ PR #5 | `tasks/45-engine-migration-tasks.md` |
| EM-2.3 | Light rig — CSM, contact shadows, `Atmosphere` entity, day/night stub | ✅ PR #5 | `tasks/45-engine-migration-tasks.md` |
| EM-2.4 | `AtmosphereController` — data-driven `.atmo.ron` + **real hot reload** (anti-chaos clamps) | ✅ PR #6 | `tasks/45-engine-migration-tasks.md` |
| EM-2.5 | `GraphicsTier` presets (Low→Ultra) + experimental Solari/DLSS slot | ✅ PR #6 | `tasks/45-engine-migration-tasks.md` |
| EM-2.6 | Custom post-process slot (`FullscreenMaterial` vignette) | ✅ PR #6 | `tasks/45-engine-migration-tasks.md` |

## Phase 3 — Voxel meshing, terrain & figures 🔵

| Task | What | Status | Docs |
|---|---|---|---|
| EM-3.1 | Greedy mesher **lift-copy** from voxygen (fidelity contract, golden tests) | ✅ PR #7 | `tasks/45-engine-migration-tasks.md` |
| EM-3.2 | Custom vertex attrs (`VOXEL_AO`/`BLOCK_LAYER`) + `Mesh<V>`→`bevy::Mesh` conversion | ✅ PR #8 | `tasks/45-engine-migration-tasks.md` |
| EM-3.3 | `VoxelMaterialExt` — PBR texture arrays, nearest sampling, **vertex AO indirect-only** | ✅ PR #8 | `tasks/45-engine-migration-tasks.md` |
| EM-3.4 | **Block palette RON** (kind→layer + PBR params) — data-driven + hot reload + real mips | ✅ PR #9 | `tasks/45-engine-migration-tasks.md` |
| EM-3.5 | Async chunk pipeline — `AsyncComputeTaskPool` + per-frame upload budget + unload path | ✅ PR #9 | `tasks/45-engine-migration-tasks.md` |
| EM-3.6 | **Listen-server: real Xindeler terrain streams to Bevy via replicon** 🎯 phase gate (visual) | ✅ PR #10 | `tasks/45-engine-migration-tasks.md` |
| EM-3.7 | Entity mirror + interpolation — sim entities live on the streamed world | ✅ PR #12 | `tasks/45-engine-migration-tasks.md` |
| EM-3.7b | Controllable character + input — **walk the world yourself** (embedded Client, 3rd-person cam) | ✅ PR #13 | `tasks/45-engine-migration-tasks.md` |
| EM-3.8 | Figures (`.vox`) — real assembled voxel models replace capsules (quadruped end-to-end) | ✅ PR #15 | `tasks/45-engine-migration-tasks.md` |
| EM-3.8b | Humanoid figures + skeletal animation (armour/recolour + idle/walk/run) | ✅ PR #17 | `tasks/45-engine-migration-tasks.md` |
| EM-3.8c | Figure completion — animated quadrupeds/birds + more bodies + humanoid weapon + polish | ✅ PR #18 | `tasks/45-engine-migration-tasks.md` |
| EM-3.8d | Figure gear v1 — real equipped weapon/armor/lantern from inventory (new `NetLoadout` mirror) | ✅ PR #26 | `tasks/45-engine-migration-tasks.md` |
| EM-3.8e | Figure gear polish v2 — head-armor merge, real glider, bird fly/run hysteresis, dedup | ✅ PR #29 | `tasks/45-engine-migration-tasks.md` |
| EM-3.9 | Sprites (grass/props) + fluids v1 (translucent water, `river_velocity`) | ✅ PR #19 | `tasks/45-engine-migration-tasks.md` |
| EM-3.9b | Sprites/water polish — UV-scroll water shader, sprite whitelist widened, wind-sway attempted+reverted | ✅ PR #28 | `tasks/45-engine-migration-tasks.md` |
| EM-3.9c | Sprites polish v3 — wind-sway v2 (normal-consistent), furniture/dungeon sprite kinds, shared decoded-chunk store | ✅ PR #92 | `tasks/45-engine-migration-tasks.md` |
| EM-3.10 | LOD & culling v1 (distance bands + frustum culling) | ✅ PR #20 | `tasks/45-engine-migration-tasks.md` |
| EM-3.10b | LOD & culling v2 — occlusion culling (measured, shipped off by default) + real lod-alt far-mesh | ✅ PR #30 | `tasks/45-engine-migration-tasks.md` |
| EM-3.12 | **Third-person camera collision (spring-arm)** — stop the camera clipping through solid geometry when looking up from below or near trees/walls | ✅ PR #64 | `2026-07-11-bl82-camera-collision-design.md`; `tasks/52-bl82-camera-collision-tasks.md` |
| EM-3.11 **[M]** | In-game visual smoke (AO/TAA/fog/anims/perf vs old client) | 🔵 25 rounds so far (PRs #32–#99 + round-24/25 across rounds; round 19 removed the placeholder-mesh mechanism, round 20 fixed LOD-culling frontier flicker with hysteresis, round 21 fixed a distinct shadow-flicker bug — CSM regen + sprite-wind/shadow desync, round 24 re-diagnosed a lingering "constant flicker" as unrelated LOD-proxy z-fighting near the camera, fixed via near-band discard). **Round 25 fixed round-21's own regression**: its 0.0025 rad sun-rotation throttle made the sun (and its shadows — same `Transform`) visibly step every ~0.48s instead of sweeping; `build_directional_light_cascades` (0.19.0) turns out to have no change-detection gate at all, so the throttle never saved the CSM-regen cost it was meant to. Replaced with a bit-exact `!=` no-op guard (an angle-based epsilon was tried first and rejected: `Quat::angle_between`'s `acos` is ill-conditioned near-identity, amplifying f32 rounding noise into a spurious "changed" reading) — commits every frame the rotation genuinely differs, only skipping the write while paused. Awaiting Matías's live retest. EM-3.11p (diagonal stutter) not formally closed. | `2026-07-09-bl82-em311-findings-log.md` |
## Phase 4 — Server shell, replicon transport & ORACLE foundations 🔵

AI coordination note (2026-07-07): Phase 4 must leave the server ready to connect with the AI crown-track work without making Engine Migration responsible for implementing ORACLE/AURORA end-to-end. During server preparation, include the runtime seams needed by BL-83 (AURORA/NPC RAG + memory over the local vLLM node) and BL-85 (ORACLE/Bedrock world-director orchestration). Keep AWS account creation, Bedrock model access, Budgets, and paid setup pending until the server foundation is resolved and ORACLE is ready to consume the AWS startup credits effectively.

| Task | What | Status | Docs |
|---|---|---|---|
| EM-4.1 | `xindeler-server-app` — headless `MinimalPlugins` shell embedding the sim; **dual-stack** (old client keeps connecting) | ✅ PR #27 | `tasks/45-engine-migration-tasks.md` |
| EM-4.2 | Persistence / rtsim / agent-dylib verified under the shell; 24h soak | 🔵 correctness done (PR #33); soak-readiness sanity (10min) done; the full 24h run is the only open item | `tasks/45-engine-migration-tasks.md` |
| EM-4.2b | Transport backend spike — `ReplicaTransport` abstraction + `quinnet` impl; `renet2` evaluated, not adopted | ✅ PR #46 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.2c | Login / session handshake bridged to the sim's accounts + persistence | ✅ PR #46 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.2d | Interest management — per-client visibility (region/distance + `DimensionId`); ~2x measured bandwidth improvement vs old protocol | ✅ PR #46 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.2e | AI gateway readiness handoff — `AiExecutionMode` + config/metrics/fallback seams for BL-83/BL-85 | ✅ PR #46 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.2f | AURORA/NPC entity-model readiness — `NetUid` identity + full `AuroraOverlay` schema | ✅ PR #49 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.3 | `DmEventLoader` — `.dmevent.ron/json` `AssetLoader` + `oracle://` watch dir (ORACLE writes files) | ✅ PR #45 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.4 | Anti-chaos validation layer (clamp tables for injected events) | ✅ PR #45 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.5 | `DimensionRegistry` + `DimensionId` + instanced-dimension generation — full lifecycle | ✅ PR #46 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.6 | Dimension teardown & GC (RAM+VRAM leak-free) + heuristic predictive GC | ✅ PR #49 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.7 | Generic entity factory v1 (behavior strings → `Agent` presets) | ✅ PR #49 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.8 | Narrative hooks (`world_rumor` → chronicle; `on_enter_message` → HUD toast) | ✅ PR #49 | `tasks/47-bl82-phase4-remaining-tasks.md` |
| EM-4.9 | E2E event drill (full "the Mist-Bound" example, both clients coexisting) + live player-dimension transfer | ✅ PR #58 (drill) + PR #77 (player transfer). Per-dimension physics remains a separately-tracked, unscheduled research item. | `2026-07-11-bl82-em49-e2e-drill-design.md`; `tasks/51-bl82-em49-e2e-drill-tasks.md`; `2026-07-11-em49b-per-dimension-physics-research.md`; `2026-07-12-bl82-em49-player-dimension-transfer-design.md` |
| EM-4.10 **[regression]** | Wave-2+3 post-merge regression hardening (9 findings A–I: perf hotfix, schedule cadence, isolation-law process gap, trademark scrub, data-driven cleanup) | ✅ PR #52 | `2026-07-10-bl82-wave3-regression-fixes-design.md`; `tasks/48-bl82-wave3-regression-fixes-tasks.md` |
| EM-4.11 **[root-cause fix]** | Frame-rate local-player prediction — render now uses the client's own per-frame predictor instead of easing a 30Hz-sampled position (fixes the residual FPS oscillation / tick-quantization bug family) | ✅ PR #59 (Phase A+B) + 2 follow-up fixes (camera-focus decoupling on slopes; max-Hz safety ceiling) | `2026-07-11-bl82-frame-rate-prediction-design.md`; `tasks/49-bl82-frame-rate-prediction-tasks.md` |
| EM-3.11-FH | **Full-horizon LOD terrain** — the structural beige-horizon fix: P-A real far-terrain colour, P-B horizon-occlusion + atmosphere/curvature blend, P-C streamed distant LOD objects (trees/structures) | ✅ P-A PR #57 + P-B PR #60 + P-C PR #94, all merged | `2026-07-11-bl82-full-horizon-lod-terrain-design.md`; `tasks/50-bl82-full-horizon-lod-terrain-tasks.md` |
| EM-4.12 **[cleanup]** | Data-driven-content cleanup (3 moderate findings from an architecture review: hardcoded event-name list, far-mesh bend constants, and a cross-crate spawn-radius invariant → RON/tests) | ✅ PR #65 | `tasks/45-engine-migration-tasks.md` |

## Phase 5 — UI (bevy_ui + widget kit), audio & playable parity ⚪

**Design authored 2026-07-11 (Opus).** Full spec/plan/tasks in the private design repo:
`specs/2026-07-11-bl82-phase5-ui-audio-parity-design.md` + `plans/…-plan.md` +
`tasks/56-bl82-phase5-ui-audio-parity-tasks.md`.

**Worksheet locked 2026-07-11 (maximalist v1 — full parity, "reemplazo total", no lean cutover):**
Matías chose the full-fidelity option on every fork. The old client is retired only under a
**total-replacement scheme** — every secondary/advanced system (two-way trade, full 4-tab crafting,
gamepad, full i18n, the complete 252-file instrument/audio bank) ships **before** the EM-5.13
cutover; nothing defers to post-cutover polish. Key answers: UI = our own theme over `bevy_ui` +
`bevy_ui_widgets` + `EditableText` (zero new UI deps, Feathers copied-from not depended-on); audio =
**direct Kira in our own `bevy/*` crate**, not `bevy_kira_audio`; full server browser; full 4-tab
crafting; gamepad in v1; full multi-language i18n + hot-swap.

**Two facts shape the phase:** (1) Feathers is editor-tooling-only/experimental per Bevy's own docs,
so `[Q4]=B` is honoured as our own theme, not a runtime dep. (2) the phase's real spine is a new HUD
state-mirror replication layer — the Bevy client mirrored almost no gameplay state before this phase,
so nearly every screen adds a small read-only `Net*` protocol comp + a `xindeler-sim-bridge`
projection — roughly as much work as the UI itself.

| Task | What | Status | Docs |
|---|---|---|---|
| EM-5.1 | **UI foundation** — Xindeler widget kit (Panel/Bar-globe/Button/Tooltip/Notification) + theme tokens + i18n seam + `HudState`/`HudAction` state machine + `UiScale` setting | ✅ PR #81 | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.2 | **Core combat HUD** — HP/energy/poise globes, XP bar, combo counter, buff/debuff strip, crosshair, death/respawn, overhead health bars. The proof slice for the mirror pattern. | ✅ PR #81, 🐛 fixed 2026-07-12 (HUD render bug + smoke-harness gap, both root-caused) | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.3 | **Skillbar / hotbar + cooldowns** — drag-to-assign, live keybind labels, sim-driven slot count, cooldown wipe/countdown | ✅ PR #90, 🐛 fixed PR #95 (client-identity resolution, replacing an embedded-player-only shortcut) | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.4 | **Chat** — scrollback + channel tabs + `/command` support | ✅ PR #84, 🐛 fixed PR #91 (minimize control). Known gap: not yet wired into the real dedicated server (listen-server only). | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.5 | **Map** — one-shot world image + always-on minimap + toggle-able full map | ✅ PR #87, 🐛 2 follow-up fixes PR #91 + #98 (resolution, then zoom) | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.6 | **Inventory / bag / trade / loot** — paper-doll, bag grid, drag-drop, full two-party trade | ✅ PR #88 | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.7 | **Diary / skill-trees** — stats/weapon/class trees (reuse BL-06) + abilities tab + SP spend, one generic renderer for every `SkillGroupKind` | ✅ PR #96 (found + fixed 3 real bugs in inherited WIP, incl. a cross-client privacy leak) | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.8 | **Social / group / dialogue** — player list, party frames, invites, NPC dialogue (AURORA seam) | ✅ PR #86 | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.9 | **Main menu + connect flow** — menu, login, connecting/loading, credits, full server browser | ⚪ | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.10 | **Audio** — direct in-house Kira integration (largest epic, splits 5.10a–e); music/SFX/ambience/spatial + full 252-file instrument bank | ⚪ | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.11 | **Input rebinding** — keymap + `settings.ron` controls section + full gamepad support | ✅ PR #85 | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.12 | **Settings + esc menu** — pause menu + all tabs | ⚪ | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.13 **[M]** | **Full parity play session → cutover decision** — total-replacement gate; retires legacy client into Phase 6 | ⚪ terminal gate | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.14 | **Character select + creation** — char list + 3D preview + creation wizard | ⚪ | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.15 | **Crafting** — recipes/search/categories + full salvage/repair/modular-weapon tabs | ⚪ | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.16 | **Accessibility, UI scaling & i18n depth** — subtitles, reduced-flashing, scaling, full multi-language i18n | ⚪ | `tasks/56-bl82-phase5-ui-audio-parity-tasks.md` |
| EM-5.17 | **HUD-D4 dark-gothic ARPG visual replacement** — replaces the current flat-`bevy_ui` HUD with Matías's own Notion-designed, PNG-asset-backed Diablo-IV-style HUD (resource orbs, ornate action bar, boss nameplate, party frames, rarity-tiered loot, "Path of Ascension" skill tree); also roots out 4 real input/logic bugs found in a live play session (chat can't hide, duplicate "Lv.1", top-left bar clutter, dead P/I hotkeys). 8-phase rollout, phases sequenced independently. | 🔵 **Phase 0 shipped** (PR #102) — 6 root-caused HUD input/logic bugfixes (chat-hide + F5, duplicate "Lv.1" `Visibility` leak, dead P/M/F1 hotkeys, raw-`KeyCode`->`ActionState` for Inventory/Social/Map, typing-focus guard, `InputFocus` clear-on-Escape). **Phase 1 done** (PR #101 merged) — asset pipeline: 55 real PNGs at `assets/voxygen/element/ui/hud_d4/`, `HudImages`/`HudImageKey` lookup (53 wired), additive image-backed widget-kit variants, `UiMaterial` viable in Bevy 0.19 but v1 uses CPU-clip fill (shader scaffolded, v2 on-ramp), shared `GlobalZIndex`. **All 8 phases implemented + PR'd (PRs #101-#107, #109), ALL MERGED.** | `specs/2026-07-15-bl82-em517-hud-redesign-design.md`, `plans/2026-07-15-bl82-em517-hud-redesign-plan.md`, `tasks/57-bl82-hud-redesign-tasks.md` |
| EM-5.18 | **Equipment panel redesign** — UX correction to EM-5.17 Phase 7 (PR #109): Matías live-tested it and found the 18 equip slots stacked in the same panel as the bag/item grid with drag-and-drop the only equip path. Redesign: an Inventory-vs-Equipment tab split (reusing the Diary window's exact tab-bar/`Node::display` pattern) + a Diablo-4-style click-slot-to-open-a-filtered-modal equip flow (backed by a new `NetItemStack::equippable_slots` mirror field computed via the real `EquipSlot::can_hold` authority, never re-implemented client-side). Drag-and-drop kept for same-tab rearranging; the modal becomes the only cross-tab equip/unequip path once tabs are separate. 3-phase rollout (tab split / modal / parity-polish). | 🔵 **P1 (tab split) merged (#113); P2 (modal + protocol extension) merged (#117); P3 (parity check + polish) in PR #121** — confirmed the 18-slot layout (2/14/2 weapon-set/armor-column/weapon-set split) renders unchanged inside the new `EquipmentTabRoot` via a live `--smoke-screenshot`, added a weapon-set-to-weapon-set same-tab drag regression test, and closed the P1→P2 gap with an end-to-end sim-bridge test (real `Server::tick()`, not just an EventBus-queued assertion) proving a picker equip request actually mutates `NetInventory`. Awaiting Matías's merge. | `specs/2026-07-16-bl82-equipment-panel-redesign.md`, `plans/2026-07-16-bl82-equipment-panel-redesign-plan.md`, `tasks/58-bl82-equipment-panel-redesign-tasks.md` |
| EM-5.19 | **Hybrid target selection** — soft-target (D4-style camera-cone scan, priority `P=α/d+β·cosθ`, Enemy-filtered) + hard-lock (WoW-style, rebindable `GameInput::Select`/KeyX; character faces target via the existing `look_dir`→`Ori` channel) + directional Tab-flick cycle + auto-release. Client-side only; attacks stay spatial, no new attack-target protocol field. Unblocks EM-5.17 Phase 5's boss/target nameplate (was permanently hidden). | 🔵 **All phases merged (P1 #115, P2 #118, P3 #120).** P2: `HardLock`/`TargetLockKind` + `Select` promote/clear, `apply_hard_lock_facing` (listen-server-only) overrides `LocalPlayerInput.look` toward the lock, `release_invalid_hard_lock` auto-release, bright/dim nameplate+marker. P3: replaces P2's "second `Select` press clears" with a real flick-direction cycle (`AccumulatedMouseMotion`-driven, pure unit-tested selector), and finalizes the Escape-vs-pause-menu ordering P2 deferred (`hard_lock_active` run condition + `clear_hard_lock_on_escape`). P4 (console-orbit cam) deferred/optional. | `specs/2026-07-16-bl82-hybrid-target-selection-design.md`, `plans/2026-07-16-bl82-hybrid-target-selection-plan.md`, `tasks/59-bl82-hybrid-target-selection-tasks.md` |

## Phase 6 — Upstream-sync drills & hardening ⚪

| Task | What | Status | Docs |
|---|---|---|---|
| EM-6.1 | Real sync drill #1 (`gitlab-master-merger` + Mapper §D triage), time it, fix friction | ⚪ | `tasks/45-engine-migration-tasks.md` |
| EM-6.2 | Sync drill #2 → target <1 day routine; write the runbook into the skill | ⚪ | `tasks/45-engine-migration-tasks.md` |
| EM-6.3 | **Bevy version-bump rehearsal** (first EM-M1 exercise — proves churn is contained to `bevy/*`) | ⚪ | `tasks/45-engine-migration-tasks.md` |
| EM-6.4 | Retire interim pieces per cutover (legacy quinn listener off, old client archived) | ⚪ | `tasks/45-engine-migration-tasks.md` |
| EM-6.5 | Post-migration perf review (FPS/frame-time/RAM/VRAM/server-tick vs old client) | ⚪ | `tasks/45-engine-migration-tasks.md` |

## Phase 7 — Visual detail & atmosphere polish 🔒

**Sequencing decided 2026-07-09 (Matías):** scope = everything Matías flagged as "missing/flat" beyond
the EM-3.11(b) bug fixes — new rendering *content*, not bug fixes. Research + spec/plan/tasks authoring
is done; **implementation is explicitly BLOCKED until Phase 6 completes** (Matías wants the phases in
order: EM-3.11b re-test → Phase 4 → Phase 5 → Phase 6 → *then* Phase 7).

**Worksheet locked 2026-07-09 (maximalist v1 — no cost-cut, "no hay restricción de tiempo"):** every
item below builds its full-fidelity form in v1 (real volumetric clouds, geometric instanced foliage,
hybrid weather grid, wind vertex-displacement, wet-surface PBR, canopy rain interception, deterministic
server-synced day/night). **Core principle (Matías): all graphics processing is CLIENT-SIDE** — the
server stays minimal. Given the real, still-unresolved perf/stutter issue found in EM-3.11b, every
system here needs a measured perf gate before shipping enabled-by-default. **EM-7.8 (birds)** was
extracted out of this phase into its own epic, **BL-86** (general backlog) — Matías wants real,
non-AI entities for it, which is gameplay/entity-model scope beyond atmosphere polish.

| Task | What | Status | Docs |
|---|---|---|---|
| EM-7.1 | Per-voxel color/texture variation — real per-voxel `ColLight.col` into the palette shader, position-hash jitter, procedural per-vertex AO at seams | ⚪ | `2026-07-09-visual-detail-atmosphere-polish.md`; `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.2 | Geometric foliage — real instanced 3D grass-blade/leaf meshes, shadow-casting, independently wind-reactive | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.3 | Clouds — real raymarched volumetric clouds, forward light scattering, physically traversable | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.4 | Rain (weather) — hybrid server weather grid + full client GPU rendering (`bevy_hanabi`), wind-coupled, canopy interception, wet-surface PBR | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.5 | Visible sun disc — Bevy 0.19's first-party `SunDisk` component | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.6 | Night sky — stars — real astronomical star map + Xindeler-canon constellations (13-sign zodiac), ORACLE narrative hooks | ⚪ design locked; blocked until Phase 6 | `2026-07-11-em76-night-sky-stars-constellations.md`; `tasks/53-em76-night-sky-stars-constellations-tasks.md` |
| EM-7.7 | Moon — real dynamic lunar phases, deterministically clocked | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| ~~EM-7.8~~ | ~~Ambient wildlife — birds~~ **moved to BL-86** (general backlog) — real, non-AI entities, gameplay scope not atmosphere polish | ➡️ BL-86 | `docs/backlog/backlog.md` |
| EM-7.9 | Calendar & seasons — 364-day loop (13×28 months, 4 seasons), solstice/equinox day-length, ORACLE `SeasonOverride` hook | ⚪ design locked; blocked until Phase 6 | `2026-07-11-em79-calendar-seasons-system.md`; `tasks/54-em79-calendar-seasons-system-tasks.md` |
| EM-7.10 | Wind vertex-displacement shaders — client-side wind field bends foliage/fur, normal-consistent by construction | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.11 | Dynamic wet-ground PBR — rain-driven roughness/albedo, gated by canopy interception | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.12 | Canopy rain interception — occlusion pass for dry zones under foliage + edge drip | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |
| EM-7.13 | Weather↔wind physics coupling — one wind source drives foliage, rain, and clouds coherently | ⚪ | `tasks/46-visual-detail-atmosphere-tasks.md` |

**Numbering note:** EM-7.10→7.13 were originally drafted as EM-7.9→7.12 inside the private atmosphere
spec before the public EM-7.9 slot was claimed by Calendar & Seasons (2026-07-11) — renumbered to free
it cleanly; see `tasks/46-visual-detail-atmosphere-tasks.md` for the full history.

**Day/night ↔ sim sync:** `SunCycle` (client-local real-time stub) gets a read-only mirror of the sim's
authoritative `TimeOfDay` as part of this phase — cheap correctness win Matías asked for explicitly (was
previously unsynced, clients could each show a different sky).

---

## 🔻 Low-priority / deferred (unscheduled)

| ID | Task | Status | Docs |
|---|---|---|---|
| **EM-L1** | **Asset rebrand (`veloren-*` names → `xindeler-*`) + asset-sync mapper.** Code is already fully rebranded (EM-1.3); the ONE remaining Veloren-named surface is **assets** (`.vox`/`.png`/`.ogg`/`.ttf` file names + RON load-path strings, frozen on purpose per the migration spec §0 to keep upstream syncing). This task: investigate whether asset names/paths can be renamed given how many RON manifests reference them, and if feasible build the asset-sync mapper (the asset analog of `tools/xindeler-rename.sh`) so a rename doesn't sever the ability to pull upstream asset updates. **Low priority** — the frozen-names approach works fine today; this raises upstream-merge conflict surface, so weigh it against the sync cost. Author a spec/plan/tasks before touching anything. | ⚪ | `tasks/45-engine-migration-tasks.md` |

---

## Adding tasks

New migration work (a Bevy upgrade, a discovered gap, an emerged sub-task) gets:
1. its detail in the private board `docs/design/tasks/45-engine-migration-tasks.md` (acceptance criteria),
2. a spec/plan there if it's substantial (e.g. a Bevy-version-upgrade spec when EM-M1 fires),
3. a row **here** so the human roll-up stays complete — short description + status + a Docs pointer,
4. and — if it changes the program's shape — a note on the BL-82 row in the general `backlog.md`.

**Timeline estimate:** ~5–7 months solo + Claude to full parity. The Phase-3 visual gate (real terrain
in Bevy) is **met**; the remaining lift is entities/UI/audio/server-shell + the sync/version drills.
