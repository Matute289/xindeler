//! EM-3.5 — async chunk meshing pipeline with budgeted uploads.
//!
//! Flow (spec §4.1): dirty-chunk queue → one [`AsyncComputeTaskPool`] task
//! per chunk (`generate_mesh` + EM-3.2 conversion, entirely off the main
//! thread) → per-frame drain that uploads AT MOST
//! [`ChunkUploadBudget::max_uploads_per_frame`] finished chunks, spawning /
//! replacing their `Mesh3d` entities. Meshes are
//! `RenderAssetUsages::RENDER_WORLD` (the converter sets it — no CPU copy,
//! matters for dimension GC).
//!
//! ## Generic over the volume source
//! The pipeline never generates terrain: a host-installed
//! [`ChunkVolumeProvider`] closure hands it the `VolGrid2d` + mesh range for
//! a key. The EM-3.3-era demo plugs a synthetic generator in; EM-3.6 plugs
//! the replicated `TerrainGrid` in — the pipeline is byte-identical in both.
//!
//! ## Ordering & budget semantics
//! - Uploads are budgeted per `Update` (default 2). Meshing itself runs on the
//!   task pool's worker threads, but the number of OUTSTANDING tasks is capped
//!   at budget × [`IN_FLIGHT_FACTOR`] — a mass re-mesh (palette reload, EM-3.6
//!   teleport) parks the remainder in the dedup queue instead of retaining
//!   hundreds of finished meshes in memory.
//! - Completion order is NOT deterministic (task scheduling + `HashMap` drain
//!   order); callers must not rely on it. Documented per the EM-3.5 acceptance
//!   — determinism is not required.
//! - Re-marking an in-flight key replaces its task (the dropped
//!   `bevy_tasks::Task` is cancelled) — last write wins. A re-mark whose
//!   provider now returns `None` ALSO cancels the in-flight task, so a stale
//!   result can never land after the volume went away.
//! - Unload ([`ChunkMeshQueue::remove_chunk`], the EM-3.6 streaming path):
//!   cancels the pending dirty mark AND the in-flight task, then despawns the
//!   chunk's entities + index entry at the start of the next pipeline run
//!   (removals are processed BEFORE the dirty drain, so `remove` → `mark`
//!   within one frame nets out to a fresh chunk — last write wins here too).
//! - Replacing a chunk despawns the old entities and spawns the new ones in the
//!   SAME command batch, so there is no visible hole.
//!
//! ## BL-82 EM-3.11h fix: first-load placeholder (no more black frames)
//! The "no visible hole" guarantee above only ever covered RE-meshing an
//! already-spawned chunk (old entity stays up until the new one is ready).
//! It said nothing about a chunk's FIRST ever mesh: between a fresh key
//! being marked dirty and its `AsyncComputeTaskPool` task finishing +
//! clearing the upload budget, that key had **no entity at all** — for
//! however many frames the greedy mesher + the budget (default 2/`Update`)
//! took. Bug report: BL-82 EM-3.11h, a real gameplay capture, showed ~2
//! fully black frames (nothing drawn — no sky, no terrain, no character;
//! only the UI overlay) while walking into a cave, immediately followed by
//! the cave popping in fully rendered. Root cause, confirmed by reading
//! `xindeler-client`'s `far_terrain.rs`: its far-mesh cutout hole is
//! DELIBERATELY excluded within `chunk_render_distance` of the live camera
//! (so the coarse LOD sheet never z-fights the block-accurate near terrain)
//! — the near pipeline (this module) was trusted to always cover that
//! band. It didn't, for a never-before-seen chunk: no near mesh (not ready
//! yet) AND no far mesh (deliberately excluded) = the bare `ClearColor`,
//! which reads as a hard black frame whenever the current atmosphere
//! profile's sky colour is dark (dusk/night/cave shadow — exactly the
//! reported moment).
//!
//! [`spawn_chunk_mesh_tasks`] now spawns a cheap, SYNCHRONOUS placeholder
//! entity (a flat-shaded box spanning the chunk's footprint and z-range,
//! `PlaceholderChunkMesh`, sharing a `TerrainChunkMesh` marker so it obeys
//! the same distance culling as real chunks) the instant a never-before-
//! indexed key starts its async task — so there is something solid to draw
//! at that spot from frame 1, not after the mesh finishes. When the real
//! mesh lands, [`apply_chunk_meshes`]'s existing despawn-old+spawn-new
//! atomic swap replaces it exactly like any other re-mesh (zero special-
//! casing needed there — a placeholder is just another `ChunkEntities`
//! entry). Already-indexed keys (re-meshes of a chunk that already has real
//! geometry, e.g. a border re-mesh when a neighbour streams in) are
//! untouched — they keep relying on the pre-existing atomic swap, no
//! placeholder ever inserted for them.
//!
//! ## BL-82 EM-3.11i follow-up: the placeholder could still read as a black
//! ## hole (lighting, not throughput)
//! A later real gameplay capture (Matías, walking cave-adjacent terrain at
//! speed) showed the black-frame bug was gone but replaced by something
//! Matías rated MORE visible: a solid, hard-edged, box-shaped dark region
//! that grew over ~0.5-0.7s then popped away all at once. Two hypotheses
//! were checked against the evidence instead of assumed:
//!
//! 1. **Throughput/backlog** — is mesh generation too slow to keep up with fast
//!    movement, so several placeholders are up at once for an extended time?
//!    Ruled out as the PRIMARY driver: `spawn_chunk_mesh_tasks`'s placeholder
//!    spawn is synchronous and per-key, so it cannot itself be backlogged; a
//!    burst of newly-streamed chunks (`terrain_stream.rs` marks a new key's
//!    full 3×3 neighbourhood dirty every arrival) can genuinely have several
//!    placeholders up simultaneously, and the ALREADY-DOCUMENTED, still-open
//!    sim-tick stutter (EM-3.11c/d/e, `docs/backlog/engine-migration.md`)
//!    stretches however many frames that takes into real wall-clock seconds
//!    when frame time spikes to 30-200+ms — but the budget/pipeline mechanics
//!    themselves are unchanged and not the thing that made the box read as
//!    BLACK.
//! 2. **Lighting** — a `StandardMaterial` is normally lit: with no direct light
//!    reaching a fragment and no usable indirect/ambient term, its physically
//!    correct output is exactly zero, regardless of `base_color`.
//!    [`placeholder_transform`] scales the box to the chunk's FULL
//!    footprint/height, so the reported walking-into-a-cave case routinely puts
//!    the camera INSIDE the box, surrounded by its own inner faces
//!    (intentionally rendered via `cull_mode: None`). A closed box viewed from
//!    its interior self-shadows against the sun from nearly every direction and
//!    starves whatever indirect/SSAO light would otherwise reach it — textbook
//!    conditions for a lit surface to render fully black. Confirmed as the
//!    primary cause: [`placeholder_material`] is now `unlit: true`, so its
//!    fragment output is `base_color` unconditionally — no lighting term, no
//!    self-shadow, no ambient/SSAO dependency, hence no path to black. It stays
//!    a flat, obviously-crude mid-grey box under any scene condition (bright
//!    noon through a pitch cave), which is what "there's a placeholder here"
//!    was always supposed to look like.
//!
//! Net: the "growing region" perception is real and (per the evidence
//! above) tracks the pre-existing, still-open perf issue rather than a new
//! meshing-throughput bug introduced here — that part is a tuning/perf
//! question for EM-3.11c/d/e, not this task. What made it look like a
//! second black-frame bug — the box actually rendering as solid black — is
//! fixed here at the material level, independent of how large or long that
//! backlog ever gets.
//!
//! ## BL-82 EM-3.11 (round 14) — "background disappears for 1-2 frames"
//! (`record11.mov`): the placeholder was invisible, not the terrain
//! Matías reported the distant tree/mountain background periodically
//! vanishing entirely for 1-2 frames, then popping back — described as
//! things "flickering before they finish generating." Root-caused with an
//! offscreen frozen-camera capture harness (mirroring the EM-3.11q
//! methodology: fixed camera, no player movement, so any change between
//! consecutive frames is a genuine content pop, not camera/retile motion):
//! a mountain silhouette (rendered by `xindeler-client::far_terrain`'s
//! coarse mesh, confirmed NOT the cause after exhaustive testing — its
//! despawn+respawn retile swap is genuinely atomic, verified across 30+
//! retiles both by ECS-level entity-presence logging and by frame-by-frame
//! visual capture) was abruptly PARTIALLY OCCLUDED by a flat, pale
//! rectangle for over a dozen consecutive frames, then the real chunk mesh
//! (with real trees) popped in and the mountain silhouette was fully
//! visible again. That rectangle is exactly this module's
//! [`PlaceholderChunkMesh`] box — working as designed (EM-3.11h/i) — but at the
//! render-distance band where new chunks stream in, `bevy_pbr`'s `DistanceFog`
//! is already ~90-99% opaque (BL-82 EM-3.11 Phase B's own tuning target for
//! that exact radius). Fog application is gated ONLY by `fog_enabled` (which
//! defaults `true`), NEVER by `unlit` (`bevy_pbr`'s `pbr.wgsl`:
//! `main_pass_post_lighting_processing` runs after the unlit/lit branch, not
//! inside it — confirmed by reading the shader), so the placeholder's "neutral
//! rock-grey" (chosen in EM-3.11i to read as an obvious placeholder under any
//! LIGHTING condition) washes out toward the pale fog/sky colour at typical
//! viewing distance anyway, flattening it into something visually
//! indistinguishable from "empty sky" rather than "an obviously crude
//! placeholder box" — exactly the reported symptom, and exactly why it reads as
//! the BACKGROUND vanishing rather than a foreground object appearing: the box
//! is farthest, so it is the most fogged, so it is the first thing fog erases.
//! (The already-tracked, still-open EM-3.11c/d/e/p mesh-throughput/stutter
//! question governs HOW LONG a placeholder stays up, not WHETHER it is visible
//! while it's up — that duration question is explicitly out of scope here, same
//! boundary EM-3.11i already drew.) Fix: [`placeholder_material`] now sets
//! `fog_enabled: false` (a first-class `StandardMaterial` field precisely
//! for this: `bevy_pbr::pbr_material`'s
//! `STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT`, checked before fog is applied in
//! `main_pass_post_lighting_processing`) — one line, no effect on
//! timing/throughput/geometry, keeps the placeholder reading as "something is
//! loading here" at every distance instead of dissolving into the horizon.
//!
//! ## BL-82 EM-3.11 follow-up (2026-07-11) — the placeholder/far-mesh seam
//! A `bevy-migration-reviewer` MAJOR (explicitly flagged UNTESTED — a
//! hypothesis, not a confirmed bug) worried that round 14's `fog_enabled:
//! false` and `xindeler-client`'s far-mesh dissolve (PR #60,
//! `far_terrain_material.rs`) — both landed the same session, never tested
//! together — could trade "background disappears" for a NEW artifact: a
//! flat, un-fogged placeholder popping visibly next to the heavily
//! fog/haze-dissolved far mesh right at `chunk_render_distance`.
//!
//! Reproduced and confirmed with the same frozen-camera consecutive-frame
//! technique round 14 used (`XINDELER_SMOKE_FAR_MESH_CAM=1` + a temporary
//! per-frame burst capture): a solid, hard-edged, warm rock-grey box is
//! plainly visible against the horizon for ~10+ consecutive captured frames
//! (`burst_00201.png`–`burst_00210.png` of that run) before resolving into
//! real meshed terrain — exactly the hypothesized artifact, real and
//! visible, though categorically milder than round 14's bug (the box IS
//! visible, just starkly flat next to its surroundings, rather than
//! invisible). Root cause: near `chunk_render_distance`, real terrain and the
//! far mesh's own near edge are BOTH already heavily blended toward the
//! atmosphere's fog/sky colour (real terrain via `DistanceFog`, the far mesh
//! via `DistanceFog` PLUS its own dissolve once beyond `bend_start` — see
//! `far_terrain_material.wgsl`) — round 14 made the placeholder the ONE thing
//! in that band immune to any such blending, so it now reads as a distinctly
//! flat, saturated slab against an otherwise uniformly hazy scene.
//!
//! Fix: [`PlaceholderHazeTint`] — an OPTIONAL, host-installed resource
//! carrying the live atmosphere colour to blend toward, and
//! [`PLACEHOLDER_HAZE_BLEND`] — a capped, DISTANCE-INDEPENDENT blend factor
//! (unlike `DistanceFog`, which ramps toward ~100% with distance — the exact
//! mechanism round 14 had to disable because it erased the placeholder
//! entirely). [`sync_placeholder_haze`] re-checks the shared placeholder
//! material against the live tint every frame one is installed, but only
//! WRITES when the target colour actually differs from what's applied
//! (a `bevy-migration-reviewer` finding: `Assets::get_mut` unconditionally
//! marks an asset modified regardless of whether the value changed, so a
//! naive unconditional write would re-extract this material into the render
//! world every frame forever, settled or not — see that function's own doc
//! comment for why a `resource_changed`-style gate at the registration site,
//! the seemingly obvious fix, is actually wrong instead: a placeholder can
//! first appear long after the tint last changed). Absent a tint (e.g. the
//! synthetic demo, which has no atmosphere), the placeholder stays
//! [`PLACEHOLDER_BASE_COLOR`] verbatim — current, round-14 behaviour,
//! unchanged. This softens the box toward its surroundings' general haze
//! WITHOUT reintroducing per-fragment distance-fog (so it can never wash out
//! completely the way round 14's bug did) and without touching timing, size,
//! or throughput — see
//! `docs/design/specs/2026-07-09-bl82-em311-findings-log.md` for the full
//! investigation, evidence, and before/after screenshots.
//!
//! ## BL-82 EM-3.11 round 16 — the softened box still reads as a "tan/beige
//! ## patch" (simultaneous colour contrast, not a hue defect)
//! Matías's `record12.mov` (~1:46-2:00) showed a flat, light tan/beige
//! rectangular patch popping in and out at the tree-line/sky boundary while
//! terrain streamed in, alongside continued distant-tree-shape flicker.
//! Leading hypothesis going in: the live atmosphere's `fog_color` might
//! itself read as tan under some lighting, making the round-15 haze-tinted
//! placeholder genuinely warm. Checked directly — `AtmosphereProfile::
//! default().fog_color` is `(0.66, 0.73, 0.81)`, a desaturated COOL blue, and
//! nothing in this codebase varies it by time of day (the day/night stub only
//! rotates the sun; `fog_color` is a static profile field absent an explicit
//! DmEvent) — so that specific mechanism was ruled out by reading the code,
//! not assumed.
//!
//! Reproduced live instead (frozen + free-roam offscreen bursts, same
//! methodology as rounds 14/15/9): the flat patch Matías described is,
//! confirmed frame-by-frame, this same round-14/15 placeholder box. Sampled
//! pixel values directly inside it across multiple captures were
//! **essentially neutral** (R≈G≈B, e.g. `(98,98,98)`, `(101,101,100)`,
//! `(102,102,101)`) — matching round 15's own measurement almost exactly, so
//! the round-15 fix has NOT regressed and the box itself is not, in absolute
//! terms, tan. But viewed in context (a screenshot crop, not just sampled
//! pixel values) the SAME patch reads unmistakably as a pale cream/tan slab —
//! confirmed by this investigation's own visual read of the evidence before
//! the numbers were checked. The mechanism is **simultaneous colour
//! contrast**: a genuinely near-neutral (slightly warm-biased,
//! [`PLACEHOLDER_BASE_COLOR`] is `(0.35, 0.33, 0.30)`, R > G > B) flat patch
//! sitting between a cool blue-hazed far-mesh mountain and a dark
//! tree-canopy shadow reads warmer than it measures, by contrast with its
//! neighbours — a well-documented perceptual effect, not a code defect in
//! the strict "wrong RGB value" sense, but a real, reproducible, and fixable
//! visual bug in its effect on the player regardless of mechanism.
//!
//! [`PLACEHOLDER_HAZE_BLEND`]'s round-15 value (0.2) was "a reasoned
//! default... not re-tuned further by eye" per that round's own notes — this
//! round's live evidence shows it under-corrects for exactly the seam
//! scenario it was built for. Retuned to 0.5: still capped and
//! distance-independent (never risks round 14's full-wash failure mode —
//! [`PLACEHOLDER_BASE_COLOR`] always keeps a 50% floor, however far or long
//! the box stays up), but pulls the box's flat colour much closer to the
//! live atmosphere tone it sits against, measurably softening the contrast
//! that reads as an odd-coloured slab. Verified with the same frozen-camera
//! capture technique: the patch's sampled colour moves from the
//! near-neutral-but-visually-warm values above toward a paler blue-grey that
//! reads as atmospheric haze rather than a distinctly different material.
//!
//! Distant-tree-shape flicker (the OTHER symptom Matías reported alongside
//! the patch): traced to the SAME placeholder mechanism, not a separate bug
//! — a distant tree's canopy silhouette is part of the real chunk mesh that
//! this placeholder box stands in for while it streams; a placeholder
//! resolving into (or being replaced by) the real mesh, or a neighbouring
//! chunk's placeholder popping in front of an already-real tree and then
//! clearing, both read as "the tree flickered" from the player's point of
//! view even though no tree geometry itself ever changed. This closes as the
//! same root cause as the patch, not a distinct residual — the still-open,
//! separately-tracked EM-3.11c/d/e/p mesh-throughput/frame-pacing stutter
//! governs HOW LONG any of this stays visible, unchanged by this fix.
//!
//! Instrumentation: `tracing` spans around each mesh task
//! (`chunk_mesh_task`) and each upload (`chunk_mesh_upload`), plus the
//! [`ChunkUploadStats`] resource (uploads last frame / total / in-flight).
//!
//! The upload budget belongs in `GraphicsSettings` (EM-3.5 board note); that
//! struct lives in `xindeler-app`, outside this task's crate set, so v1
//! hosts insert [`ChunkUploadBudget`] directly — the settings hookup is a
//! one-line follow-up there.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use bevy::{
    app::{App, Plugin, Update},
    asset::{Assets, Handle, RenderAssetUsages},
    color::{Color, Mix},
    ecs::{
        component::Component,
        entity::Entity,
        resource::Resource,
        schedule::{
            IntoScheduleConfigs, SystemCondition, SystemSet, common_conditions::resource_exists,
        },
        system::{Commands, Res, ResMut},
    },
    math::Vec3,
    mesh::{Indices, Mesh as BevyMesh, Mesh3d, PrimitiveTopology},
    pbr::{MeshMaterial3d, StandardMaterial},
    tasks::{AsyncComputeTaskPool, Task, block_on},
    transform::components::Transform,
};
use common::{terrain::TerrainChunk, vol::RectRasterableVol, volumes::vol_grid_2d::VolGrid2d};
use vek::{Aabb, Vec2 as VVec2, Vec3 as VVec3};

use crate::{
    convert::{fluid_mesh_to_bevy, terrain_mesh_to_bevy},
    material::{VoxelMaterial, WaterMaterial},
    mesh::terrain::generate_mesh,
};

/// 2D chunk key, upstream convention (`TerrainGrid` keys).
pub type ChunkKey = VVec2<i32>;

/// Upstream's max-texture-size hint for the greedy atlas (same value the
/// EM-3.3 demo used).
const MAX_ATLAS_SIZE: VVec2<u16> = VVec2 { x: 4096, y: 4096 };

/// Everything a mesh task needs for one chunk, as returned by the
/// [`ChunkVolumeProvider`].
pub struct ChunkVolume {
    /// Volume containing the chunk AND its ±1 xy neighbours (the mesher
    /// reads across the border; missing neighbours read as the grid's
    /// default chunk).
    pub grid: Arc<VolGrid2d<TerrainChunk>>,
    /// Mesh range, upstream convention (voxygen scene/terrain/mod.rs):
    /// xy = chunk ± 1 border, z = `[min_z - 2, max_z + 2]`. The mesher emits
    /// xy relative to the CHUNK ORIGIN (`range.min.xy + 1`) and ABSOLUTE z
    /// (terrain.rs `mesh_delta`), which is what [`chunk_transform`] assumes.
    pub range: Aabb<i32>,
}

impl ChunkVolume {
    /// Builds the canonical mesh range for `key` from z bounds (helper so
    /// every provider constructs the same contract — see [`Self::range`]).
    #[must_use]
    pub fn with_z_bounds(
        grid: Arc<VolGrid2d<TerrainChunk>>,
        key: ChunkKey,
        min_z: i32,
        max_z: i32,
    ) -> Self {
        let sz = TerrainChunk::RECT_SIZE.map(|e| e as i32);
        let range = Aabb {
            min: VVec3::new(key.x * sz.x - 1, key.y * sz.y - 1, min_z - 2),
            max: VVec3::new((key.x + 1) * sz.x + 1, (key.y + 1) * sz.y + 1, max_z + 2),
        };
        Self { grid, range }
    }
}

/// Host-installed volume source (see module docs). Returning `None` drops
/// the request (unknown / unloaded chunk) and cancels any in-flight task
/// for the key.
///
/// ## Snapshot contract (EM-3.6)
/// The `Arc<VolGrid2d>` a fetch returns is treated as an IMMUTABLE snapshot:
/// the mesh task reads it on another thread with no further synchronisation.
/// A streaming host must NOT mutate a grid it already handed out — it
/// materialises a fresh `VolGrid2d` per fetch (cheap: chunks are
/// `Arc<TerrainChunk>`, so building a grid is cloning a handful of `Arc`s)
/// and calls [`ChunkMeshQueue::mark_dirty`] again after every terrain edit;
/// the pipeline never re-meshes on its own.
#[derive(Resource, Clone)]
pub struct ChunkVolumeProvider(Arc<dyn Fn(ChunkKey) -> Option<ChunkVolume> + Send + Sync>);

impl ChunkVolumeProvider {
    pub fn new(provider: impl Fn(ChunkKey) -> Option<ChunkVolume> + Send + Sync + 'static) -> Self {
        Self(Arc::new(provider))
    }

    #[must_use]
    pub fn fetch(&self, key: ChunkKey) -> Option<ChunkVolume> { (self.0)(key) }
}

/// `BlockKind as u8` → texture-array layer, snapshotted from the block
/// palette ([`crate::palette::BlockPalette::layer_lut`]). A task captures
/// the `Arc` at spawn time, so a palette hot reload only affects chunks
/// marked dirty AFTER the new map is installed. IMPORTANT for reload hosts:
/// swap the `Arc` IN PLACE through `ResMut` (then re-mark) — a deferred
/// `commands.insert_resource` lands at the end of the frame, so the freshly
/// re-marked chunks could drain first and capture the stale map.
#[derive(Resource, Clone)]
pub struct ChunkLayerMap(pub Arc<[u32; 256]>);

impl Default for ChunkLayerMap {
    fn default() -> Self { Self(Arc::new([0; 256])) }
}

/// Materials for spawned chunk entities. ONE shared terrain material for all
/// chunks (bind-group reuse is what keeps budgeted uploads cheap) + the
/// shared water material (EM-3.9b — animated scroll/ripple, see
/// [`crate::material::WaterMaterialExt`]).
#[derive(Resource, Clone)]
pub struct ChunkMaterials {
    pub terrain: bevy::asset::Handle<VoxelMaterial>,
    pub fluid: bevy::asset::Handle<WaterMaterial>,
}

/// Per-frame upload budget. Default 2 (EM-3.5). Belongs in
/// `GraphicsSettings` eventually — see the module docs for why it is a
/// standalone resource v1.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ChunkUploadBudget {
    /// Max finished chunks applied (mesh assets added + entities swapped)
    /// per `Update`. Clamped to ≥ 1 at use (0 would stall forever).
    pub max_uploads_per_frame: u32,
}

impl Default for ChunkUploadBudget {
    fn default() -> Self {
        Self {
            max_uploads_per_frame: 2,
        }
    }
}

/// Dirty-chunk queue (FIFO, deduplicated) + unload requests. Hosts call
/// [`Self::mark_dirty`] / [`Self::remove_chunk`]; the pipeline drains both
/// every frame (removals first — module docs).
#[derive(Resource, Default)]
pub struct ChunkMeshQueue {
    dirty: VecDeque<ChunkKey>,
    queued: HashSet<ChunkKey>,
    removals: Vec<ChunkKey>,
}

impl ChunkMeshQueue {
    /// Enqueues `key` for (re)meshing; a key already queued is a no-op.
    pub fn mark_dirty(&mut self, key: ChunkKey) {
        if self.queued.insert(key) {
            self.dirty.push_back(key);
        }
    }

    /// Unloads `key` (the EM-3.6 streaming path): cancels a pending dirty
    /// mark and, on the next pipeline run, the in-flight task, the spawned
    /// entities and the [`ChunkMeshIndex`] entry. Last write wins — a later
    /// [`Self::mark_dirty`] re-creates the chunk.
    pub fn remove_chunk(&mut self, key: ChunkKey) {
        self.queued.remove(&key);
        self.removals.push(key);
    }

    fn pop(&mut self) -> Option<ChunkKey> {
        while let Some(key) = self.dirty.pop_front() {
            // Entries no longer in `queued` were cancelled by remove_chunk
            // (or superseded by a mark_dirty re-add later in the deque).
            if self.queued.remove(&key) {
                return Some(key);
            }
        }
        None
    }

    fn take_removals(&mut self) -> Vec<ChunkKey> { core::mem::take(&mut self.removals) }

    /// Chunks waiting to be meshed (excludes cancelled entries).
    #[must_use]
    pub fn len(&self) -> usize { self.queued.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.queued.is_empty() }
}

/// Output of one mesh task.
struct MeshedChunk {
    terrain: Option<BevyMesh>,
    fluid: Option<BevyMesh>,
}

/// In-flight mesh tasks, one per chunk key (re-marking replaces → cancels).
#[derive(Resource, Default)]
struct ChunkMeshTasks(HashMap<ChunkKey, Task<MeshedChunk>>);

/// Spawned entities per chunk key (so re-meshing replaces, not duplicates).
#[derive(Resource, Default)]
pub struct ChunkMeshIndex(HashMap<ChunkKey, ChunkEntities>);

pub struct ChunkEntities {
    pub terrain: Option<Entity>,
    pub fluid: Option<Entity>,
    /// EM-3.11h: `true` while `terrain` is the synchronous first-load
    /// placeholder box (see module docs), not the real greedy-meshed
    /// geometry. Private — only this module ever needs to tell the
    /// difference (the atomic despawn-old+spawn-new swap in
    /// [`apply_chunk_meshes`] treats a placeholder exactly like any other
    /// entry, on purpose).
    is_placeholder: bool,
}

impl ChunkMeshIndex {
    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    #[must_use]
    pub fn get(&self, key: ChunkKey) -> Option<&ChunkEntities> { self.0.get(&key) }

    /// Keys of every currently-spawned chunk. Used by palette hot reload to
    /// re-mark all live chunks dirty (their per-vertex layers changed).
    pub fn keys(&self) -> impl Iterator<Item = ChunkKey> + '_ { self.0.keys().copied() }
}

/// Upload instrumentation (complements the tracing spans).
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ChunkUploadStats {
    /// Chunks applied during the last `Update` (always ≤ the budget).
    pub uploads_last_frame: u32,
    /// Chunks applied since startup.
    pub total_uploads: u64,
    /// Mesh tasks currently in flight.
    pub in_flight: usize,
}

/// Marker on spawned opaque-terrain chunk entities.
#[derive(Component)]
pub struct TerrainChunkMesh {
    pub key: ChunkKey,
}

/// Marker on spawned fluid chunk entities.
#[derive(Component)]
pub struct FluidChunkMesh {
    pub key: ChunkKey,
}

/// Marker on the EM-3.11h synchronous first-load placeholder (see module
/// docs): a coarse box standing in for a chunk's real mesh while its async
/// task runs. Always co-spawned with a `TerrainChunkMesh` (so it obeys
/// whatever chunk-distance culling band the host applies) — this is an
/// additional tag for callers that need to tell it apart from real
/// geometry (debugging, tests), not a replacement for that marker.
#[derive(Component)]
pub struct PlaceholderChunkMesh;

/// Entity transform for a chunk mesh: the mesher emits xy relative to the
/// chunk origin and ABSOLUTE z (see [`ChunkVolume::range`]), so the entity
/// sits at the chunk origin mapped through the converter's z-up → y-up
/// rotation: Veloren `(32·kx, 32·ky, 0)` → Bevy `(32·kx, 0, −32·ky)`.
/// INTEGER translation only (converter contract: world-space texture tiling
/// with period 1 stays chunk-continuous).
#[must_use]
pub fn chunk_transform(key: ChunkKey) -> Transform {
    let sz = TerrainChunk::RECT_SIZE.map(|e| e as i32);
    #[expect(clippy::cast_precision_loss, reason = "chunk coords ≪ 2^24")]
    Transform::from_xyz((key.x * sz.x) as f32, 0.0, -(key.y * sz.y) as f32)
}

/// EM-3.11h — first-load placeholder assets: every placeholder chunk reuses
/// the SAME unit-box mesh (stretched to the chunk's footprint/height via its
/// per-entity `Transform` scale, [`placeholder_transform`]) and the SAME
/// material, so spawning one costs a component insert, not a fresh asset.
///
/// A [`Resource`] (not a [`bevy::ecs::system::Local`] to
/// [`spawn_chunk_mesh_tasks`] as in EM-3.11h originally) as of the
/// EM-3.11-follow-up seam-artifact fix (module docs): [`sync_placeholder_haze`]
/// needs the same material handle to keep its colour live, so the handle can
/// no longer be private per-system state. Behaviour is otherwise identical —
/// still exactly one mesh/material pair for the process's whole lifetime,
/// still created lazily on first use.
#[derive(Resource, Default)]
struct PlaceholderAssets {
    mesh: Option<Handle<BevyMesh>>,
    material: Option<Handle<StandardMaterial>>,
}

/// EM-3.11h — the placeholder's world transform: a unit box (built once by
/// [`placeholder_box_mesh`]) scaled to the chunk's `32×32` footprint and
/// `[z_lo, z_hi]` height range, positioned at the chunk's own origin (same
/// xz convention as [`chunk_transform`] — Veloren `(32·kx, 32·ky)` → Bevy
/// `(32·kx, −32·ky)`, box growing toward −z/+x/+y from there).
fn placeholder_transform(key: ChunkKey, z_lo: f32, z_hi: f32) -> Transform {
    let sz = TerrainChunk::RECT_SIZE.map(|e| e as f32);
    let height = (z_hi - z_lo).max(1.0);
    #[expect(clippy::cast_precision_loss, reason = "chunk coords ≪ 2^24")]
    let origin = Vec3::new(key.x as f32 * sz.x, z_lo, -(key.y as f32 * sz.y) - sz.y);
    Transform::from_translation(origin).with_scale(Vec3::new(sz.x, height, sz.y))
}

/// EM-3.11h — a flat-shaded, axis-aligned unit box (each face gets its own
/// 4 duplicated vertices + normal). Reused for every placeholder via a
/// non-uniform `Transform` scale ([`placeholder_transform`]) rather than
/// rebuilt per chunk. Paired with [`placeholder_material`]'s `cull_mode:
/// None` so a camera standing INSIDE a not-yet-meshed chunk (the exact
/// "walked into a cave" case this fix targets) still sees the box's inner
/// faces instead of nothing.
fn placeholder_box_mesh() -> BevyMesh {
    let corners = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(0.0, 1.0, 1.0),
    ];
    // (corner indices wound for an outward-facing first triangle, outward
    // normal) per face of the unit cube.
    let faces: [([usize; 4], Vec3); 6] = [
        ([0, 1, 2, 3], Vec3::new(0.0, 0.0, -1.0)), // -Z
        ([5, 4, 7, 6], Vec3::new(0.0, 0.0, 1.0)),  // +Z
        ([4, 0, 3, 7], Vec3::new(-1.0, 0.0, 0.0)), // -X
        ([1, 5, 6, 2], Vec3::new(1.0, 0.0, 0.0)),  // +X
        ([4, 5, 1, 0], Vec3::new(0.0, -1.0, 0.0)), // -Y
        ([3, 2, 6, 7], Vec3::new(0.0, 1.0, 0.0)),  // +Y
    ];

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(24);
    let mut indices: Vec<u32> = Vec::with_capacity(36);
    for (face_corners, normal) in faces {
        let base = positions.len() as u32;
        for corner_index in face_corners {
            positions.push(corners[corner_index].to_array());
            normals.push(normal.to_array());
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut mesh = BevyMesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(BevyMesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// The placeholder's neutral "obviously a placeholder" rock-grey, chosen in
/// EM-3.11i to stay legible under any lighting condition. Pulled out as a
/// named const (was inline in [`placeholder_material`]) so
/// [`sync_placeholder_haze`] can blend FROM this exact value rather than
/// duplicating the literal.
const PLACEHOLDER_BASE_COLOR: Color = Color::srgb(0.35, 0.33, 0.30);

/// Capped, distance-independent blend factor [`sync_placeholder_haze`] mixes
/// [`PlaceholderHazeTint`] into the placeholder's `base_color` by (module
/// docs' EM-3.11-follow-up section). Deliberately modest and NOT
/// distance-scaled: round 14 already proved that letting fog ramp toward
/// ~100% opacity with distance (`DistanceFog`) erases the placeholder
/// entirely, which is exactly what `fog_enabled: false` exists to prevent.
/// This is a fixed, one-time tint instead — enough to soften the box toward
/// its surroundings' general haze (confirmed necessary by a live
/// frozen-camera capture, module docs) without ever fully washing it out,
/// however far or long it stays up. Comparable in spirit to
/// `xindeler-client::far_terrain::FAR_HAZE_BLEND` (0.12, a similar "small
/// atmospheric finish, not a mask" role for the far mesh's own real colour);
/// higher here because this box is flat/monochrome (no per-vertex detail of
/// its own to preserve) and sits right at the same render-distance band the
/// far mesh's near edge is already heavily hazed at.
///
/// ## BL-82 EM-3.11 round 16 retune (module docs' round-16 section)
/// Round 15's original value (0.2) left the placeholder's absolute colour
/// genuinely near-neutral (verified: `(0.35, 0.33, 0.30)` blended 20% toward
/// a cool `(0.66, 0.73, 0.81)` fog colour lands around `(0.41, 0.41, 0.40)`,
/// i.e. R≈G≈B) — but a live capture showed that same near-neutral patch
/// reading as a distinctly warm "tan/beige" slab by SIMULTANEOUS CONTRAST
/// against the cooler blue-hazed far mesh/sky it typically sits next to.
/// Bumped to 0.5 (still `< 1.0`, so [`PLACEHOLDER_BASE_COLOR`] always keeps a
/// 50% floor — the box can never fully wash to the tint colour and vanish
/// the way round 14's unconditional `DistanceFog` did): this pulls the box's
/// resting colour much closer to the actual sky/haze tone next to it,
/// verified with the same frozen-camera capture technique to measurably
/// soften the contrast that read as an odd-coloured patch.
const PLACEHOLDER_HAZE_BLEND: f32 = 0.5;

/// Host-installed, OPTIONAL live "haze" colour for the placeholder box
/// (module docs' EM-3.11-follow-up section) — typically the current
/// atmosphere's fog colour. Absent entirely is a valid, honestly-degraded
/// state (e.g. the synthetic voxel-demo, which has no atmosphere concept):
/// the placeholder then stays [`PLACEHOLDER_BASE_COLOR`] verbatim, exactly
/// EM-3.11i/round-14 behaviour, unchanged.
#[derive(Resource, Clone, Copy)]
pub struct PlaceholderHazeTint(pub Color);

/// EM-3.11i — a neutral rock-grey, genuinely `unlit`. See the module docs'
/// EM-3.11i section for the full story: the original EM-3.11h material was
/// a normally-lit `StandardMaterial`, which a real gameplay capture proved
/// can render fully BLACK — indistinguishable from the black-frame bug this
/// placeholder exists to fix — whenever the scene provides it no usable
/// light. That is not a tuning miss, it is what physically-based lighting is
/// SUPPOSED to do (`indirect + direct == 0` ⇒ output `== 0`, whatever the
/// albedo), and this box hits that case squarely: [`placeholder_transform`]
/// scales it to the chunk's FULL footprint/height, so a camera walking into
/// a never-before-meshed chunk (the exact scenario this fix targets) is
/// routinely standing INSIDE the box, surrounded by its own inner faces
/// (`cull_mode: None` renders them on purpose — module docs above). A closed
/// box viewed from its own interior self-shadows against the sun from
/// nearly every angle and starves indirect/SSAO light the same way any
/// fully-enclosed interior does — a real StandardMaterial box in that
/// geometry goes dark regardless of `base_color`. `unlit: true` makes the
/// fragment output `base_color` directly, with NO lighting term at all, so
/// it stays a flat, clearly-a-placeholder mid-grey under every scene
/// condition (bright noon, dusk, night, deep cave) instead of only some of
/// them — the guarantee EM-3.11h was meant to provide in the first place.
/// `perceptual_roughness`/`reflectance` are dropped: both are lit-material
/// knobs with no effect once `unlit` is set.
///
/// ## BL-82 EM-3.11 round 14 — `fog_enabled: false`
/// See the module docs' round-14 section for the full investigation. At the
/// render-distance band where a never-before-meshed chunk typically appears
/// (near `chunk_render_distance`), `DistanceFog` is already ~90-99% opaque,
/// and fog application is gated ONLY by `fog_enabled` (default `true`),
/// NEVER by `unlit`
/// (`bevy_pbr::render::pbr_functions::main_pass_post_lighting_processing`
/// runs after, not inside, the unlit/lit branch) — so the "obviously a
/// placeholder" rock-grey chosen above washed out toward the pale fog/sky
/// colour anyway, reading as empty sky rather than a crude stand-in and
/// reproducing exactly as "the background disappeared for a couple of
/// frames." `fog_enabled` is a first-class `StandardMaterial` field for
/// precisely this case; setting it `false` here has NO effect on the
/// placeholder's timing, size, or the underlying mesh-generation throughput
/// (a separate, already-tracked, still-open question — EM-3.11c/d/e/p) — it
/// only keeps the box visually legible as a placeholder at every distance
/// instead of dissolving into the horizon.
fn placeholder_material() -> StandardMaterial {
    StandardMaterial {
        base_color: PLACEHOLDER_BASE_COLOR,
        unlit: true,
        cull_mode: None,
        fog_enabled: false,
        ..Default::default()
    }
}

/// Keeps the shared placeholder material's `base_color` blended
/// [`PLACEHOLDER_HAZE_BLEND`] toward the live [`PlaceholderHazeTint`], when a
/// host has installed one (module docs' EM-3.11-follow-up section). Runs
/// unconditionally every `Update` (no `run_if`) but only ever WRITES when the
/// freshly-recomputed target colour actually differs from what's already
/// applied — see the comment inline below for why a `resource_changed`-style
/// gate at the registration site would have been the wrong tool here despite
/// looking like the obvious one.
fn sync_placeholder_haze(
    tint: Option<Res<PlaceholderHazeTint>>,
    assets: Res<PlaceholderAssets>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let Some(tint) = tint else { return };
    let Some(mut materials) = materials else {
        return;
    };
    let Some(handle) = assets.material.as_ref() else {
        return;
    };
    let target = PLACEHOLDER_BASE_COLOR.mix(&tint.0, PLACEHOLDER_HAZE_BLEND);
    // Peek IMMUTABLY first (bevy-migration-reviewer finding): `Assets::
    // get_mut` returns a change-detection guard whose `DerefMut`/`Drop`
    // unconditionally mark the asset modified and push `AssetEvent::
    // Modified` (verified against `bevy_asset-0.19.0`'s `assets.rs`),
    // regardless of whether the value written is actually different —
    // driving `bevy_render`'s generic re-extraction path for this material
    // every single frame, forever, once any placeholder had ever spawned.
    // A plain `run_if(resource_changed::<PlaceholderHazeTint>)` at the
    // registration site would dodge that cost but silently break
    // correctness instead: a chunk (and its placeholder) can first stream in
    // long AFTER the tint last changed (the common case once the atmosphere
    // has settled), and that placeholder would then never pick up the
    // current tint at all. Re-deriving `target` from CURRENT state every
    // frame and only taking the mutable path when it actually differs from
    // what's already applied is correct in both the "tint just changed"
    // and "a fresh placeholder just appeared" cases, while still making the
    // steady-state (settled atmosphere, no new placeholders) cost a single
    // cheap immutable lookup + `Color` comparison, no asset-system churn.
    let Some(current) = materials.get(handle) else {
        return;
    };
    if current.base_color == target {
        return;
    }
    let Some(mut material) = materials.get_mut(handle) else {
        return;
    };
    material.base_color = target;
}

/// In-flight meshing cap factor: [`spawn_chunk_mesh_tasks`] stops draining
/// the dirty queue once `budget × IN_FLIGHT_FACTOR` tasks are outstanding.
/// Bounds the finished-but-not-yet-applied meshes retained in memory during
/// mass re-meshes (palette reload, EM-3.6 teleports); the dedup queue holds
/// the remainder at ~24 bytes/key instead of a full mesh each.
const IN_FLIGHT_FACTOR: u32 = 8;

/// BL-82 EM-3.11n — per-frame cap on how many NEW mesh tasks
/// [`spawn_chunk_mesh_tasks`] STARTS in one call (pops off the dirty queue,
/// synchronously fetches a volume snapshot via [`ChunkVolumeProvider::fetch`],
/// and hands off to the task pool) — separate from `in_flight_cap` above
/// (the total OUTSTANDING task ceiling). `fetch` runs on the MAIN thread
/// (only `generate_mesh` itself runs off-thread, inside `pool.spawn`), so
/// popping+fetching many keys in a single frame is a real main-thread cost
/// that scales with how many distinct chunks the dirty queue backlogged
/// since the last frame — previously uncapped whenever `in_flight_cap` had
/// headroom (e.g. right after an idle period, when few tasks are
/// outstanding, a burst could pop+fetch up to `budget × IN_FLIGHT_FACTOR`
/// keys in ONE frame).
///
/// Investigated for the "diagonal movement feels choppier than straight"
/// report (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md` round
/// 8; `xindeler-client`'s `terrain_stream.rs::neighbourhood_3x3` docs): a
/// diagonal streaming frontier backlogs ~1.6× as many distinct chunks per
/// unit distance as a straight one for the same real ground speed (proven
/// deterministically by `terrain_stream.rs`'s
/// `diagonal_streaming_touches_more_distinct_chunks_than_straight` test), so
/// its bursts are structurally bigger — and a live `--smoke-perf-run` A/B
/// (straight vs. diagonal, same fresh world, `xindeler-client`) measured
/// diagonal movement's frame-time distribution consistently skewing toward
/// larger max/stdev than straight's across repeated trials, even though mean
/// frame time was statistically unchanged — consistent with OCCASIONAL
/// bigger spawn bursts causing occasional bigger frame-time spikes, not a
/// sustained per-frame cost increase. Spreading the fetch cost over more
/// frames bounds the single-frame spike regardless of burst size, trading a
/// slightly longer time-to-fully-meshed for a smoother frame time. Smaller
/// than `IN_FLIGHT_FACTOR` (a fetch is far cheaper than a GPU upload, but
/// this cap exists specifically to smooth BURSTS, not to throttle steady
/// throughput).
const SPAWN_BURST_FACTOR: u32 = 4;

// BL-82 EM-3.11p follow-up
// (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md` round 10):
// re-tested after Matías reported the diagonal stutter felt unchanged.
// Confirmed with live `debug!` instrumentation (`spawn_chunk_mesh_tasks`/
// `apply_chunk_meshes`'s timers below) that the cap DOES engage during real
// diagonal movement (30-45 times per 45s) — this mitigation is not a no-op —
// but the main-thread cost it bounds stayed under ~2ms even while engaged, both
// for the fetch loop and for `apply_chunk_meshes`'s upload/spawn/despawn work.
// That rules this pipeline out as the dominant cost behind the 50-800ms
// frame-time spikes Matías experiences: EM-3.11n's fix is real but small,
// addressing a structurally confirmed (~1.6× more distinct chunks touched per
// unit diagonal distance) but practically minor effect. The dominant cost
// remains unidentified as of this round; see the findings log for what else was
// ruled out (sim-side per-system timing via `XINDELER_SLOW_SYS_MS`, a full
// `bevy/trace` + `trace_chrome` capture attempt) and the honest scope of what
// this investigation could and couldn't establish given shared-machine
// measurement noise.

/// BL-82 EM-4.11 Phase D — system-ordering label for the pipeline's
/// removals→spawn→apply chain, so a HOST crate (e.g. `xindeler-client`'s
/// `receive_chunks`, which reads [`ChunkMeshIndex`] to decide which
/// neighbours are genuinely affected by a new arrival) can order itself
/// relative to this pipeline WITHOUT reaching into its private system
/// functions (`process_chunk_removals`/`spawn_chunk_mesh_tasks`/
/// `apply_chunk_meshes` are, and stay, private — this label is the only
/// ordering handle exposed). Runs in `Update` (see
/// [`ChunkMeshPipelinePlugin`]'s doc comment).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkMeshPipelineSet;

/// Registers the queue/tasks/budget/stats resources and the pipeline
/// systems (removals → spawn → apply). Spawn/apply idle until the host
/// inserts a [`ChunkVolumeProvider`], a [`ChunkLayerMap`] and
/// [`ChunkMaterials`]; removal processing is always live.
pub struct ChunkMeshPipelinePlugin;

impl Plugin for ChunkMeshPipelinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkMeshQueue>()
            .init_resource::<ChunkMeshTasks>()
            .init_resource::<ChunkMeshIndex>()
            .init_resource::<ChunkUploadStats>()
            .init_resource::<ChunkUploadBudget>()
            .init_resource::<PlaceholderAssets>()
            .add_systems(
                Update,
                (
                    process_chunk_removals,
                    spawn_chunk_mesh_tasks.run_if(
                        resource_exists::<ChunkVolumeProvider>
                            .and_then(resource_exists::<ChunkLayerMap>),
                    ),
                    apply_chunk_meshes.run_if(resource_exists::<ChunkMaterials>),
                )
                    .chain()
                    .in_set(ChunkMeshPipelineSet),
            )
            // EM-3.11-follow-up (module docs): independent of the removals→
            // spawn→apply chain above — only needs `Assets<StandardMaterial>`
            // + the two placeholder resources, and a one-frame lag on the
            // very first placeholder ever spawned (before its material
            // exists to tint) is negligible. Deliberately NO `run_if` here —
            // see [`sync_placeholder_haze`]'s own doc comment for why a
            // `resource_changed`-gated version (the seemingly obvious
            // optimization, flagged by a `bevy-migration-reviewer` pass) is
            // actually WRONG: it would silently stop tinting any placeholder
            // that first spawns after the tint last changed, which is the
            // common case once the atmosphere settles. The function itself
            // already avoids the real cost (an unconditional write every
            // frame) by peeking before writing.
            .add_systems(Update, sync_placeholder_haze);
    }
}

/// Executes [`ChunkMeshQueue::remove_chunk`] requests: cancels the key's
/// in-flight task (drop = cancel, so a stale mesh can never apply after the
/// remove) and despawns its entities + index entry. Runs BEFORE the dirty
/// drain, so `remove` → `mark_dirty` within one frame nets out to a freshly
/// meshed chunk.
fn process_chunk_removals(
    mut commands: Commands,
    mut queue: ResMut<ChunkMeshQueue>,
    mut tasks: ResMut<ChunkMeshTasks>,
    mut index: ResMut<ChunkMeshIndex>,
) {
    if queue.removals.is_empty() {
        return;
    }
    for key in queue.take_removals() {
        tracing::debug!(key_x = key.x, key_y = key.y, "chunk unloaded");
        tasks.0.remove(&key);
        if let Some(old) = index.0.remove(&key) {
            if let Some(entity) = old.terrain {
                commands.entity(entity).despawn();
            }
            if let Some(entity) = old.fluid {
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Drains the dirty queue into `AsyncComputeTaskPool` tasks (one per chunk:
/// greedy meshing + EM-3.2 conversion, fully off the main thread), holding
/// back once `budget × IN_FLIGHT_FACTOR` tasks are outstanding OR once
/// `budget × SPAWN_BURST_FACTOR` NEW tasks have started THIS frame (BL-82
/// EM-3.11n — see [`SPAWN_BURST_FACTOR`]'s docs).
fn spawn_chunk_mesh_tasks(
    provider: Res<ChunkVolumeProvider>,
    layer_map: Res<ChunkLayerMap>,
    budget: Res<ChunkUploadBudget>,
    mut queue: ResMut<ChunkMeshQueue>,
    mut tasks: ResMut<ChunkMeshTasks>,
    mut commands: Commands,
    mut index: ResMut<ChunkMeshIndex>,
    mut meshes: ResMut<Assets<BevyMesh>>,
    mut placeholder_materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut placeholder_assets: ResMut<PlaceholderAssets>,
) {
    if queue.is_empty() {
        return;
    }
    // BL-82 EM-3.11p: wall-clock this whole system at `debug` level.
    // `provider.fetch` runs synchronously on the main thread (module docs),
    // so THIS is where a diagonal-heavier backlog actually costs a frame —
    // not just in queue depth. EM-3.11n bounded the FETCH COUNT (below) on
    // the theory that a bigger diagonal backlog meant a bigger main-thread
    // cost; this timer answers "how big, actually" without re-instrumenting
    // from scratch next time someone re-opens the diagonal-stutter
    // investigation (`docs/design/specs/2026-07-09-bl82-em311-findings-log.md`
    // round 9 vs. the EM-3.11p follow-up: even with the cap engaging on
    // every diagonal-movement burst, measured cost stayed under ~2ms —
    // nowhere near the 50+ms frame-time spikes Matías reports, so this path
    // was never the dominant cost).
    let fetch_loop_start = std::time::Instant::now();
    let in_flight_cap = (budget.max_uploads_per_frame.max(1) * IN_FLIGHT_FACTOR) as usize;
    let spawn_cap_this_frame = (budget.max_uploads_per_frame.max(1) * SPAWN_BURST_FACTOR) as usize;
    let pool = AsyncComputeTaskPool::get();
    let mut spawned_this_frame = 0usize;
    while tasks.0.len() < in_flight_cap && spawned_this_frame < spawn_cap_this_frame {
        let Some(key) = queue.pop() else {
            break;
        };
        spawned_this_frame += 1;
        let Some(volume) = provider.fetch(key) else {
            // The provider no longer has this chunk: cancel any in-flight
            // task too, so a stale mesh can't land later (last write wins).
            tasks.0.remove(&key);
            // EM-3.11h: also clean up an abandoned first-load placeholder —
            // its real mesh is never coming now (the volume is gone), so
            // nothing should be left behind to linger forever.
            if index
                .0
                .get(&key)
                .is_some_and(|entities| entities.is_placeholder)
                && let Some(entity) = index.0.remove(&key).and_then(|entities| entities.terrain)
            {
                commands.entity(entity).despawn();
            }
            tracing::debug!(?key, "chunk mesh request dropped: provider has no volume");
            continue;
        };

        // EM-3.11h: a key with no entity at all yet — its first ever mesh —
        // gets an instant, synchronous placeholder so there is always
        // SOMETHING to draw at this chunk's footprint while the async task
        // + upload budget catch up (module docs: this is what closes the
        // "black frame" gap the far mesh's camera-proximity hole relied on
        // the near pipeline to cover). A key that already has an entity
        // (real geometry from a previous upload, OR a placeholder already
        // up from an earlier mark of this same key) is left alone — this
        // only ever fires once per chunk, on its very first mark.
        if !index.0.contains_key(&key)
            && let Some(materials) = placeholder_materials.as_deref_mut()
        {
            let mesh = placeholder_assets
                .mesh
                .get_or_insert_with(|| meshes.add(placeholder_box_mesh()))
                .clone();
            let material = placeholder_assets
                .material
                .get_or_insert_with(|| materials.add(placeholder_material()))
                .clone();
            #[expect(
                clippy::cast_precision_loss,
                reason = "world z bounds ≪ 2^24, same contract as chunk_transform"
            )]
            let (z_lo, z_hi) = (volume.range.min.z as f32, volume.range.max.z as f32);
            let entity = commands
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    placeholder_transform(key, z_lo, z_hi),
                    TerrainChunkMesh { key },
                    PlaceholderChunkMesh,
                ))
                .id();
            index.0.insert(key, ChunkEntities {
                terrain: Some(entity),
                fluid: None,
                is_placeholder: true,
            });
        }

        let lut = layer_map.0.clone();
        let task = pool.spawn(async move {
            let _span =
                tracing::info_span!("chunk_mesh_task", key_x = key.x, key_y = key.y).entered();
            let (opaque, fluid, _shadow, (_bounds, atlas, atlas_size, ..)) =
                generate_mesh(&volume.grid, (volume.range, MAX_ATLAS_SIZE, ()));
            MeshedChunk {
                terrain: (!opaque.is_empty()).then(|| {
                    terrain_mesh_to_bevy(&opaque, &atlas, atlas_size, |k| lut[usize::from(k)])
                }),
                fluid: (!fluid.is_empty()).then(|| fluid_mesh_to_bevy(&fluid)),
            }
        });
        // Insert replaces any in-flight task for the key; the dropped Task
        // is cancelled (bevy_tasks/async_task semantics) — last write wins.
        tasks.0.insert(key, task);
    }
    // BL-82 EM-3.11p: confirms `SPAWN_BURST_FACTOR` is actually engaging
    // (the dirty queue backlogs past the cap in a real run) rather than
    // sitting at a value so generous it never binds — verified live during a
    // scripted diagonal-movement `--smoke-perf-run` before this round's
    // findings were written up (30-45 engagements per 45s).
    if spawned_this_frame >= spawn_cap_this_frame && !queue.is_empty() {
        tracing::debug!(
            spawned_this_frame,
            spawn_cap_this_frame,
            queue_remaining = queue.len(),
            "EM-3.11p: spawn-burst cap engaged this frame"
        );
    }
    let fetch_loop_elapsed_ms = fetch_loop_start.elapsed().as_secs_f64() * 1000.0;
    if fetch_loop_elapsed_ms > 0.1 {
        tracing::debug!(
            elapsed_ms = fetch_loop_elapsed_ms,
            spawned_this_frame,
            "EM-3.11p: spawn_chunk_mesh_tasks main-thread cost this frame"
        );
    }
}

/// Applies at most [`ChunkUploadBudget::max_uploads_per_frame`] FINISHED
/// tasks per frame: adds the mesh assets and swaps the chunk's entities
/// (despawn old + spawn new in the same command batch — no visible hole).
fn apply_chunk_meshes(
    mut commands: Commands,
    mut tasks: ResMut<ChunkMeshTasks>,
    mut index: ResMut<ChunkMeshIndex>,
    mut stats: ResMut<ChunkUploadStats>,
    budget: Res<ChunkUploadBudget>,
    materials: Res<ChunkMaterials>,
    mut meshes: ResMut<Assets<BevyMesh>>,
) {
    // Skip entirely while idle so the stats resource's change tick (and the
    // stats themselves) stay quiet — but only once the stats already say
    // idle (tasks can drain without an upload, e.g. a provider-None or
    // remove_chunk cancellation, and `in_flight` must not go stale).
    if tasks.0.is_empty() && stats.uploads_last_frame == 0 && stats.in_flight == 0 {
        return;
    }
    // BL-82 EM-3.11p: wall-clock the upload/despawn/spawn side too, same
    // rationale as `spawn_chunk_mesh_tasks`'s timer above — this budget is
    // separately capped (`max_uploads_per_frame`, default 2/frame), so it was
    // already suspected small; measured live during diagonal movement it
    // never exceeded the threshold below either.
    let apply_start = std::time::Instant::now();
    stats.uploads_last_frame = 0;

    let budget = budget.max_uploads_per_frame.max(1) as usize;
    let ready: Vec<ChunkKey> = tasks
        .0
        .iter()
        .filter(|(_, task)| task.is_finished())
        .map(|(key, _)| *key)
        .take(budget)
        .collect();

    for key in ready {
        let Some(task) = tasks.0.remove(&key) else {
            continue;
        };
        let _span =
            tracing::info_span!("chunk_mesh_upload", key_x = key.x, key_y = key.y).entered();
        let meshed = block_on(task); // finished — returns immediately

        if let Some(old) = index.0.remove(&key) {
            if let Some(entity) = old.terrain {
                commands.entity(entity).despawn();
            }
            if let Some(entity) = old.fluid {
                commands.entity(entity).despawn();
            }
        }

        let transform = chunk_transform(key);
        let terrain = meshed.terrain.map(|mesh| {
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.terrain.clone()),
                    transform,
                    TerrainChunkMesh { key },
                ))
                .id()
        });
        let fluid = meshed.fluid.map(|mesh| {
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.fluid.clone()),
                    transform,
                    FluidChunkMesh { key },
                ))
                .id()
        });
        index.0.insert(key, ChunkEntities {
            terrain,
            fluid,
            is_placeholder: false,
        });
        stats.uploads_last_frame += 1;
        stats.total_uploads += 1;
    }
    stats.in_flight = tasks.0.len();
    let apply_elapsed_ms = apply_start.elapsed().as_secs_f64() * 1000.0;
    if apply_elapsed_ms > 0.5 {
        tracing::debug!(
            elapsed_ms = apply_elapsed_ms,
            uploads = stats.uploads_last_frame,
            "EM-3.11p: apply_chunk_meshes main-thread cost this frame"
        );
    }
}
