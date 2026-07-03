---
name: xindeler-bevy
description: Use when working on the Bevy engine migration (BL-82) or any code under bevy/* — Bevy 0.19 APIs and gotchas, the Embedded-Sim+Mirror architecture, the logic/shell isolation law, upstream-sync triage with the Mapper, and the voxel render pipeline (greedy meshing, vertex AO, PBR texture arrays, TAA/volumetric fog).
---

# xindeler-bevy

Xindeler is migrating 100% (client + server shell) from Veloren's bespoke engine to **Bevy ≥ 0.19**
(decision 2026-07-02, resolves BL-50 → Path B+). Canonical docs (private design repo):
- Spec: `docs/design/specs/2026-07-02-bevy-migration-design.md`
- Mapper (rename + concept map + sync triage): `docs/design/specs/2026-07-02-veloren-xindeler-mapper.md`
- Plan: `docs/design/plans/2026-07-02-bevy-migration-plan.md` · Board: `docs/design/tasks/45-engine-migration-tasks.md`

Review every migration PR with the **`bevy-migration-reviewer`** agent.

## The architecture in one paragraph (decisions LOCKED 2026-07-02)

The specs ECS simulation (logic crates: `common*`, `world`, `rtsim`, `client`, `server`,
`network*`, `voxygen/anim`) is **embedded SERVER-side, never rewritten**: `Server::tick`
(`server/src/lib.rs:787`) is a headless function called from the Bevy server shell
(`MinimalPlugins` App — full multithreaded scheduler headless). The server-side
`bevy/xindeler-sim-bridge` mirrors sim state → **replicated** Bevy entities, and **bevy_replicon**
([Q3]=B) carries them to a **100%-pure-Bevy client** (no specs, no `xindeler-client-core` embedded —
logic crates linked as type libraries only). The replication contract lives in
`bevy/xindeler-protocol` (shared comps/channels/events). During the migration the sim's legacy
quinn listener stays up (**dual-stack**) so the old client coexists; it's retired at cutover.
New server systems (ORACLE/AURORA) are native Bevy ECS. UI = **bevy_ui + Feathers** ([Q4]=B),
no egui in the shipped client.

## The isolation law (hard rule — CI-enforced)

1. Logic crates get **zero** `bevy`/`wgpu`/`winit` deps, ever (`scripts/check-engine-isolation.sh`).
2. Logic crates are only edited by upstream merges (+ the idempotent `tools/xindeler-rename.sh`).
3. `assets/**` file names and load paths keep Veloren names — NEVER rename them.
4. Bridge systems are read-mostly; writes into the sim go through its public APIs/events only.
5. Directory names stay upstream-verbatim; only Cargo package names rebrand (Mapper §B).

## Bevy 0.19 facts & gotchas (verified 2026-07)

- **0.19 (2026-06-19), wgpu 29.** Breaking vs 0.18: render graph → **ECS schedules** (custom passes
  are systems, no graph nodes); **resources stored as components** on singleton entities (broad
  `Query<Entity>` now conflicts with resource access → filter `Without<IsResource>`;
  `init_non_send_resource` → `init_non_send`); text cosmic-text → parley (`FontSize`, `FontSource`);
  old `bevy_scene` → `bevy_world_serialization`; bloom luma now linear-space.
- **TAA is stable** (`bevy_anti_alias::TemporalAntiAliasing`): requires `Msaa::Off`; prepasses come
  as required components. It is our fix for distant block flicker (MSAA fights greedy meshing).
- **Volumetrics:** `VolumetricFog` (camera) + `VolumetricLight` (lights) + `FogVolume` (entities);
  `DistanceFog` for cheap far-fog. `Atmosphere` is a **standalone entity** in 0.19
  (`inner_radius`/`outer_radius`). All driven at runtime by our `AtmosphereController` (data-driven).
- **Shadows:** 4-cascade CSM + **contact shadows (new 0.19)**; PCSS behind `experimental_pbr_pcss`.
- **Solari (ray tracing) & DLSS = experimental, Vulkan+RTX only** → optional `GraphicsTier`, never
  the baseline renderer.
- **StandardMaterial does NOT take `Texture2dArray`** (#20134) → block textures bind via
  `ExtendedMaterial<StandardMaterial, VoxelMaterialExt>` (albedo/normal/MRA arrays, layer index =
  custom vertex attribute). Bindless since 0.16 on Vulkan/DX12.
- **Crisp pixels:** `ImagePlugin::default_nearest()` global + `Image.sampler =
  ImageSampler::nearest()` per image (nearest min/mag, linear mip + TAA = no shimmer).
- **Vertex AO:** per-vertex AO from the ported mesher callbacks goes into `ATTRIBUTE_VOXEL_AO`,
  multiplied into **indirect light only** in the material extension's WGSL.
- **Assets:** async `AssetLoader` trait; hot reload = cargo feature `file_watcher`; out-of-tree
  dirs via custom `AssetSource` (ORACLE drop-dir = `oracle://` → `userdata/oracle_events/`).
  `.bsn` scene ASSET loader has NOT shipped (BSN is code-only in 0.19) — we use our own
  `EntityTemplate` loader meanwhile.
- **Headless:** `MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1./30.)))`.
- **Events split (0.17):** buffered = `MessageReader/MessageWriter`; observers use `On<E>`.
- **Netcode: bevy_replicon** ([Q3]=B locked) — replication via `xindeler-protocol` comps/events;
  terrain = compressed chunk payloads (bincode+lz4) on a dedicated channel, NOT per-component
  replication; per-client visibility = interest management (region/distance + `DimensionId`).
  Transport backend chosen by the EM-4.2b spike (renet2 vs quinnet), pinned exact. Veloren's
  `network*`/`client` crates stay in-tree upstream-merged but serve only the transitional old
  client + smoke bot — never link them from the Bevy client.
- **UI: bevy_ui + Feathers** ([Q4]=B locked) — build on the shared Xindeler widget kit (EM-5.1);
  `EditableText` for inputs, `ui_picking` for interaction. Don't add egui to the shipped client.
- Ecosystem lag: third-party crates typically reach a new Bevy ~1–3 months late — pin exact
  versions, upgrade Bevy in one contained PR per release (rehearsed in EM-6.3).

## Meshing pipeline (where things live)

Greedy mesher lift-copied from `voxygen/src/mesh/{greedy,terrain,segment}.rs` →
`bevy/xindeler-render-voxel/src/mesh/` (Mapper C5–C7 — upstream fixes to the originals are
hand-ported each sync). Chunk meshing runs on `AsyncComputeTaskPool`, budgeted uploads/frame,
meshes `RenderAssetUsages::RENDER_WORLD` (no CPU copy — matters for dimension GC). Block palette +
PBR params are data (`block_palette.ron`), never hardcoded.

## Upstream sync (the whole point)

Run `gitlab-master-merger`, then: re-run `tools/xindeler-rename.sh`, then triage with **Mapper §D**
(logic crates merge clean; `voxygen/` merges into the unbuilt in-tree reference; mesh/anim changes
hand-port; scene/render/hud upstream changes usually skip), log decisions in Mapper §E. `voxygen/`
is intentionally in-tree but excluded from `default-members` — don't "clean it up".
