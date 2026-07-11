//! EM-3.9 — block sprites on the streamed terrain (listen-server only).
//!
//! Grass, flowers and other block sprites are drawn as many INSTANCES of a
//! small set of shared `.vox` meshes. This module is the client-side asset glue
//! around `xindeler-render-voxel::sprite` (which owns the manifest read,
//! per-chunk instance collection and `.vox` meshing), mirroring how
//! `figure_view` wraps `render_voxel::figure`.
//!
//! ## Flow
//! 1. On startup, load `sprite_manifest.ron` (typed `AssetLoader`, same trick
//!    the figure manifests use) and, once parsed, kick off `.vox` loads for the
//!    whitelisted [`SPRITE_KINDS`] (v1 renders common OUTDOOR sprites — grasses
//!    + flowers — not the ~150 furniture/dungeon kinds).
//! 2. As each kind's `.vox` variations finish loading, mesh them ONCE
//!    ([`sprite_model_to_bevy`]) into shared `Mesh3d` handles cached in
//!    [`SpriteMeshCache`].
//! 3. For every [`CompressedChunk`] that arrives, decode it, collect its sprite
//!    instances ([`collect_sprite_instances`]), keep only whitelisted kinds,
//!    apply a per-chunk density budget, and spawn one child entity per instance
//!    under a per-chunk parent — each sharing a cached `Mesh3d` (+ the shared
//!    sprite material), so Bevy batches the draws (effectively instanced).
//! 4. A [`RemoveChunk`] despawns that chunk's sprite parent (and its children).
//!
//! ## Budget / performance strategy (task requirement)
//! Sprites can be enormous (thousands of grass tufts per chunk). v1 keeps it
//! cheap three ways, all documented and tunable:
//! - **Kind whitelist** ([`SPRITE_KINDS`]): common outdoor vegetation (grasses,
//!   flowers, cacti, crops, mushrooms — EM-3.9b widened this from ~16 to the
//!   whole `Plant` sprite category), so the ~989-entry manifest doesn't spawn
//!   furniture/dungeon décor (`Furniture`/`Decor`/`Lamp`/`Container`/`Modular`
//!   categories — deferred: those need per-kind placement review, not just a
//!   list extension, since some assume interior/wall-adjacent placement the
//!   outdoor density budget below isn't tuned for).
//! - **Per-chunk cap** ([`MAX_SPRITES_PER_CHUNK`]): if a chunk exceeds it, the
//!   instances are thinned by a deterministic stride so density scales down
//!   gracefully rather than spiking the entity count.
//! - **Shared meshes + one material**: N instances of a kind reuse ONE `Mesh3d`
//!   asset + ONE `StandardMaterial` handle. **EM-3.9b verified (not just
//!   assumed) that this is real GPU instancing, not merely "few draw calls"**:
//!   Bevy's automatic batching (`bevy_render::batching` — opt-out via a
//!   `NoAutomaticBatching` marker we never add) merges consecutive phase items
//!   that share `(pipeline id, draw function, material bind group)` into ONE
//!   indirect multi-draw where the backend supports it (`gpu_preprocessing`,
//!   the default path). Because ALL sprite instances of a (kind, variation)
//!   share the SAME mesh + material handle — and the material is IDENTICAL
//!   across chunks (one handle total) — batching merges across chunk/parent
//!   boundaries too: the whole visible world's sprites collapse to at most
//!   ~(kinds × variations) draw calls, not one per chunk and NOT one per
//!   instance. A hand-rolled `SpecializedMeshPipeline` with a manual instance
//!   buffer would reimplement exactly this for no measurable win, so v1 keeps
//!   the shared-handle approach and does not add one.
//! - **Wind sway — attempted in EM-3.9b, REVERTED.** A per-vertex sine sway via
//!   an `ExtendedMaterial<StandardMaterial, SpriteWindMaterialExt>` caused a
//!   real visual regression: sprites lost most of their vertex colour and
//!   rendered largely black once the vertex-stage position perturbation was
//!   live (confirmed by disabling the effect — the render returned to an exact
//!   match of the pre-EM-3.9b screenshot; the vertex shader's `world_normal`
//!   output is the unperturbed stock normal, which no longer matches the swayed
//!   surface for some instances/angles, driving the PBR diffuse term to ~0).
//!   Root-caused but not re-attempted within EM-3.9b's budget — reverted to the
//!   plain vertex-coloured `StandardMaterial` below (bit-identical to
//!   pre-EM-3.9b) rather than ship a known lighting bug. Deferred to
//!   **EM-3.9c**: either also perturb/recompute `world_normal` to stay
//!   consistent with the swayed position, or use a cheaper effect that doesn't
//!   touch geometry (e.g. a per-instance vertex-colour brightness pulse) to
//!   sidestep normal/lighting correctness entirely.
//!
//! ## Purity
//! 100% Bevy + `xindeler-render-voxel` (a shell crate), `common` terrain types
//! and `dot_vox`/`ron` — NO specs. Compiled only under the `listen-server`
//! feature. The decode here is independent of `terrain_stream`'s (a second lz4
//! pass per chunk — accepted v1 cost, keeps the two consumers decoupled). This
//! stayed a v1 cost in EM-3.9b too: a shared decoded-chunk cache would need
//! both this module's pending/built lifecycle AND `terrain_stream`'s
//! store/remesh lifecycle to agree on ownership/eviction timing, and both are
//! subtle, already-correct, and independently tested — not worth the risk for
//! a cost that is one lz4 decompress of a chunk-sized buffer, not a hot loop.

use std::collections::VecDeque;

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, LoadState, io::Reader},
    platform::collections::HashMap,
    prelude::*,
    reflect::TypePath,
};
use common::terrain::SpriteKind;
use vek::Vec3 as VVec3;
use xindeler_protocol::{CompressedChunk, RemoveChunk};
use xindeler_render_voxel::sprite::{
    SPRITE_MANIFEST, SPRITE_SCALE, SpriteManifest, collect_sprite_instances, sprite_model_to_bevy,
};

/// The sprite kinds v1 renders: the whole outdoor-safe `Plant` sprite category
/// (`common::terrain::sprite` — grasses, flowers, cacti, crops, mushrooms).
/// EM-3.9b widened this from the original ~16 (grasses + flowers only) to
/// cover the REST of `Plant` — verified against `sprite_manifest.ron`
/// (read-only) to have real `variations` entries in the SAME shape the
/// original 16 use, so no new mesh-assembly logic was needed (`sprite.rs`
/// already ignores per-kind `custom_indices`/filters generically).
/// Furniture/dungeon décor (`Furniture`/`Decor`/`Lamp`/`Container`/`Modular`
/// categories) is a SEPARATE, deferred widening (module docs) — those aren't
/// simple list additions, they need placement-context review.
pub const SPRITE_KINDS: &[SpriteKind] = &[
    // Cacti
    SpriteKind::BarrelCactus,
    SpriteKind::RoundCactus,
    SpriteKind::ShortCactus,
    SpriteKind::MedFlatCactus,
    SpriteKind::ShortFlatCactus,
    SpriteKind::LargeCactus,
    SpriteKind::TallCactus,
    // Flowers
    SpriteKind::BlueFlower,
    SpriteKind::PinkFlower,
    SpriteKind::PurpleFlower,
    SpriteKind::RedFlower,
    SpriteKind::WhiteFlower,
    SpriteKind::YellowFlower,
    SpriteKind::Sunflower,
    SpriteKind::Moonbell,
    SpriteKind::Pyrebloom,
    SpriteKind::LushFlower,
    SpriteKind::LanternFlower,
    // Grasses, ferns and other "wild" plants
    SpriteKind::LongGrass,
    SpriteKind::MediumGrass,
    SpriteKind::ShortGrass,
    SpriteKind::Fern,
    SpriteKind::LargeGrass,
    SpriteKind::TaigaGrass,
    SpriteKind::GrassBlue,
    SpriteKind::SavannaGrass,
    SpriteKind::TallSavannaGrass,
    SpriteKind::RedSavannaGrass,
    SpriteKind::SavannaBush,
    SpriteKind::Welwitch,
    SpriteKind::LeafyPlant,
    SpriteKind::DeadBush,
    SpriteKind::JungleFern,
    SpriteKind::JungleRedGrass,
    SpriteKind::DeadPlant,
    // Crops, berries and fungi
    SpriteKind::Corn,
    SpriteKind::WheatYellow,
    SpriteKind::WheatGreen,
    SpriteKind::LingonBerry,
    SpriteKind::Blueberry,
    SpriteKind::Lettuce,
    SpriteKind::Pumpkin,
    SpriteKind::Carrot,
    SpriteKind::Tomato,
    SpriteKind::Radish,
    SpriteKind::Turnip,
    SpriteKind::Flax,
    SpriteKind::WildFlax,
    SpriteKind::Mushroom,
    SpriteKind::CaveMushroom,
    SpriteKind::Cotton,
    SpriteKind::SewerMushroom,
    SpriteKind::LushMushroom,
    SpriteKind::RockyMushroom,
    SpriteKind::GlowMushroom,
];

/// Per-chunk sprite cap (v1 budget). A chunk with more whitelisted sprites is
/// thinned by a deterministic stride, so grass-dense chunks scale down rather
/// than spawning tens of thousands of entities. Tunable; belongs in
/// `GraphicsSettings` eventually (→ EM-3.9b, like the chunk upload budget).
pub const MAX_SPRITES_PER_CHUNK: usize = 1500;

/// Per-frame CHUNK budget for [`build_chunk_sprites`] (BL-82 EM-3.11p round
/// 11). Unlike `xindeler-render-voxel::pipeline`'s terrain-mesh path (async
/// meshing off the main thread, capped uploads/spawns per frame via
/// `ChunkUploadBudget` + `SPAWN_BURST_FACTOR`), this system used to do EVERY
/// arriving chunk's lz4 decode + full voxel scan (loop 1) and EVERY pending
/// chunk's up-to-[`MAX_SPRITES_PER_CHUNK`]-entity spawn (loop 2)
/// synchronously, unconditionally, in whatever `Update` they landed in — with
/// no limit on how many CHUNKS could complete both loops in the same frame.
/// A burst of several chunks arriving/becoming-ready in one tick (world boot,
/// a fast teleport, or simply a bad-luck network flush) could cost tens of ms
/// of main-thread time in one frame (measured: ~30ms for a 30-chunk decode
/// burst during boot — see the round-11 findings-log entry). This caps how
/// many chunks each loop processes per frame; excess work is left for LATER
/// frames (loop 1: every arriving `CompressedChunk` message is still drained
/// out of Bevy's `Messages<T>` double-buffer EVERY frame — its 2-update TTL
/// means anything left unread past that is silently dropped, so the cap
/// cannot be applied by leaving messages unread — into an owned, TTL-free
/// `decode_queue`, and it's popping from THAT queue that's capped; loop 2:
/// an unbuilt `PendingChunkSprites` marker is untouched and retried next
/// frame) rather than ever being dropped. Chosen the same order of magnitude
/// as `pipeline.rs`'s `SPAWN_BURST_FACTOR`-derived cap.
const CHUNK_BUILD_BURST_CAP: usize = 4;

/// Client-side sprite plugin (listen-server only). Installs the manifest
/// loader, the mesh cache, and the systems that load models + build per-chunk
/// sprites.
pub struct SpriteViewPlugin;

impl Plugin for SpriteViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SpriteManifestAsset>()
            .init_asset_loader::<SpriteManifestLoader>()
            .init_resource::<SpriteMeshCache>()
            .init_resource::<SpriteChunkIndex>()
            .add_systems(Startup, load_sprite_manifest)
            .add_systems(
                Update,
                (
                    load_sprite_models,
                    build_chunk_sprites,
                    remove_chunk_sprites,
                )
                    .chain(),
            );
    }
}

// ---------------------------------------------------------------------------
// Manifest asset + loader (typed `.ron`, same trick as the figure manifests)
// ---------------------------------------------------------------------------

/// Bevy asset wrapper for the parsed sprite manifest.
#[derive(Asset, TypePath)]
pub struct SpriteManifestAsset(pub SpriteManifest);

#[derive(Default, TypePath)]
struct SpriteManifestLoader;

impl AssetLoader for SpriteManifestLoader {
    type Asset = SpriteManifestAsset;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _ctx: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(SpriteManifestAsset(ron::de::from_bytes(&bytes)?))
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

/// A Veloren dotted asset name (`voxygen.voxel.…`) → the on-disk relative path
/// Bevy's `AssetServer` resolves. Names are frozen (isolation law rule 3): only
/// the `.` separator + the extension are translated.
fn asset_path(dotted: &str, ext: &str) -> String { format!("{}.{ext}", dotted.replace('.', "/")) }

/// Strong handle keeping the sprite manifest (and its file watch) alive.
#[derive(Resource)]
struct SpriteManifestHandle(Handle<SpriteManifestAsset>);

fn load_sprite_manifest(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(SpriteManifestHandle(
        asset_server.load(asset_path(SPRITE_MANIFEST, "ron")),
    ));
}

// ---------------------------------------------------------------------------
// Model loading → shared mesh cache
// ---------------------------------------------------------------------------

/// One loaded, meshed variation of a sprite kind: the shared render mesh
/// (manifest offset already baked in at mesh time).
struct SpriteVariationMesh {
    mesh: Handle<Mesh>,
}

/// Per-kind loading state: the `.vox` handles (while loading) and, once meshed,
/// the shared variation meshes. A kind stays `Loading` until every variation is
/// loaded (or failed), then becomes `Ready` (or `Failed`, skipped).
enum SpriteKindState {
    /// Variation `.vox` handles still loading.
    Loading(Vec<PendingSpriteModel>),
    /// Meshed shared handles, one per usable variation.
    Ready(Vec<SpriteVariationMesh>),
    /// No usable variation (manifest absent / all `.vox` failed) — skip.
    Failed,
}

struct PendingSpriteModel {
    handle: Handle<crate::figure_view::VoxAsset>,
    offset: VVec3<f32>,
}

/// The shared sprite render assets: per-kind meshes + the single sprite
/// material. Filled lazily by [`load_sprite_models`] as models load.
#[derive(Resource, Default)]
struct SpriteMeshCache {
    kinds: HashMap<SpriteKind, SpriteKindState>,
    material: Option<Handle<StandardMaterial>>,
    /// Whether the whitelisted `.vox` loads have been kicked off (once the
    /// manifest parsed).
    started: bool,
    /// Set once every kind has left `Loading` (all `Ready`/`Failed`), so
    /// [`load_sprite_models`] can early-out instead of re-scanning all kinds
    /// forever.
    all_settled: bool,
}

/// The shared matte material for vertex-coloured sprites (base_color WHITE so
/// per-voxel colour shows through; slightly rough, double-sided so thin grass
/// cards are lit from both faces). One material for ALL sprites → batching.
fn sprite_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.9,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

/// Once the manifest is parsed, start loading the whitelisted kinds' `.vox`
/// files; each frame, promote kinds whose models have all resolved into meshed
/// shared handles.
fn load_sprite_models(
    asset_server: Res<AssetServer>,
    manifest_handle: Option<Res<SpriteManifestHandle>>,
    manifests: Res<Assets<SpriteManifestAsset>>,
    vox_assets: Res<Assets<crate::figure_view::VoxAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<SpriteMeshCache>,
) {
    // Every kind has reached its terminal state — nothing left to poll.
    if cache.all_settled {
        return;
    }
    let Some(manifest_handle) = manifest_handle else {
        return;
    };
    let Some(manifest) = manifests.get(&manifest_handle.0) else {
        return; // still loading
    };

    // One-time material + kick off `.vox` loads for the whitelist.
    if !cache.started {
        cache.material = Some(materials.add(sprite_material()));
        for &kind in SPRITE_KINDS {
            let state = match manifest.0.variations(kind) {
                Some(vars) if !vars.is_empty() => {
                    let pending = vars
                        .iter()
                        .map(|v| PendingSpriteModel {
                            handle: asset_server.load(asset_path(&v.model, "vox")),
                            offset: VVec3::from(v.offset),
                        })
                        .collect();
                    SpriteKindState::Loading(pending)
                },
                _ => SpriteKindState::Failed,
            };
            cache.kinds.insert(kind, state);
        }
        cache.started = true;
    }

    // Promote any Loading kind whose variations have all resolved.
    let kinds: Vec<SpriteKind> = cache.kinds.keys().copied().collect();
    for kind in kinds {
        let Some(SpriteKindState::Loading(pending)) = cache.kinds.get(&kind) else {
            continue;
        };
        // Wait until every variation is Loaded or Failed.
        let all_settled = pending.iter().all(|p| {
            matches!(
                asset_server.get_load_state(&p.handle),
                Some(LoadState::Loaded | LoadState::Failed(_))
            )
        });
        if !all_settled {
            continue;
        }
        // Mesh the variations that loaded; drop failures.
        let mut variation_meshes = Vec::new();
        for p in pending {
            if let Some(vox) = vox_assets.get(&p.handle)
                && let Some(mesh) = sprite_model_to_bevy(&vox.0, 0, p.offset)
            {
                variation_meshes.push(SpriteVariationMesh {
                    mesh: meshes.add(mesh),
                });
            }
        }
        let state = if variation_meshes.is_empty() {
            warn!(
                ?kind,
                "sprite: no usable .vox variation; skipping this kind"
            );
            SpriteKindState::Failed
        } else {
            info!(?kind, count = variation_meshes.len(), "sprite: kind ready");
            SpriteKindState::Ready(variation_meshes)
        };
        cache.kinds.insert(kind, state);
    }

    // Once no kind is still Loading, stop polling next frame.
    if cache.started
        && !cache
            .kinds
            .values()
            .any(|s| matches!(s, SpriteKindState::Loading(_)))
    {
        cache.all_settled = true;
    }
}

// ---------------------------------------------------------------------------
// Per-chunk sprite spawning
// ---------------------------------------------------------------------------

/// Sprite entities per chunk key, in BOTH lifecycle phases, so a re-received
/// chunk or a `RemoveChunk` cleans up whichever exists and never leaks or
/// duplicates:
/// - `built`: the finished sprite-parent entity (children = the instances);
/// - `pending`: the in-flight [`PendingChunkSprites`] MARKER entity, which has
///   not been indexed as `built` yet (models still loading). A chunk normally
///   arrives more than once at startup (terrain_stream re-marks neighbours
///   dirty and re-emits `CompressedChunk`), so without tracking the marker the
///   second arrival would spawn a SECOND marker → two parents, the first
///   leaked.
#[derive(Resource, Default)]
struct SpriteChunkIndex {
    built: HashMap<[i32; 2], Entity>,
    pending: HashMap<[i32; 2], Entity>,
}

/// Marker on a built sprite-chunk parent, carrying its world-space vegetation
/// centroid and instance count. `pub` so the EM-3.9 smoke camera can frame the
/// densest sprite patch (the world-centre figure framing sits in a dark
/// interior — this points the capture at open, lit vegetation instead).
///
/// `count` is only READ by `player_input`'s listen-server-only smoke camera
/// (`SmokeSpriteCamPlugin`) and by this module's own `#[cfg(test)]` helpers —
/// under the EM-4.2b `net-client` feature alone (no `listen-server`), nothing
/// reads it, hence the `dead_code` allow below (BL-82 EM-4.2b: this is the
/// first build combination that compiles this module without also compiling
/// `player_input`).
#[derive(Component)]
pub struct SpriteChunkParent {
    pub centroid: Vec3,
    #[cfg_attr(not(feature = "listen-server"), allow(dead_code))]
    pub count: usize,
}

/// A chunk whose sprites still need building once every whitelisted kind it
/// contains is Ready (models finish loading AFTER the first chunks arrive).
#[derive(Component)]
struct PendingChunkSprites {
    key: [i32; 2],
    instances: Vec<xindeler_render_voxel::sprite::SpriteInstance>,
}

/// Decodes arriving chunks, collects + thins their whitelisted sprite
/// instances, and (once the referenced kinds are meshed) spawns them under a
/// per-chunk parent. A chunk waits (holding a [`PendingChunkSprites`] marker
/// entity) until its kinds are Ready.
///
/// ## BL-82 EM-3.11p round 11: unbounded, unbudgeted main-thread cost
/// Unlike `xindeler-render-voxel::pipeline`'s terrain-mesh path (async
/// meshing off the main thread, `ChunkUploadBudget` + `SPAWN_BURST_FACTOR`
/// capping how much upload/fetch work lands in any one frame), this system
/// has NO cap at all: every arriving [`CompressedChunk`] gets its own lz4
/// decode (a SECOND decode of the same bytes `terrain_stream.rs` already
/// decoded once — accepted v1 cost per the module docs) plus a full
/// `collect_sprite_instances` voxel scan of the whole chunk column,
/// synchronously, in loop 1 below; and every chunk whose sprite kinds are
/// already `Ready` (true for all of them, past the first few seconds of any
/// session) gets up to [`MAX_SPRITES_PER_CHUNK`] (1500) entities spawned via
/// `Commands`, synchronously, in loop 2. Measured (BL-82 EM-3.11p round 11):
/// a 30-chunk decode burst during world boot cost ~30ms of main-thread time
/// in one frame with no cap — a real, previously-unbudgeted cost the
/// terrain-mesh pipeline's `ChunkUploadBudget`/`SPAWN_BURST_FACTOR` never had
/// to deal with because meshing itself runs off-thread. Both loops are now
/// capped at [`CHUNK_BUILD_BURST_CAP`] chunks per frame (see its docs); timed
/// here too (gated by `XINDELER_SPRITE_PERF_LOG=1` so the always-on cost is a
/// no-op check) so `--smoke-perf-run` can still correlate any residual
/// frame-time spikes against these events by `epoch_ms` timestamp. Round 11
/// live A/B testing (findings log) did NOT find this path to be the dominant
/// cause of the reported diagonal-vs-straight difference specifically — the
/// measured spikes in an isolated fresh-world trial occurred with ZERO
/// sprite-decode/spawn events in the recorded window — but it is a genuine,
/// independently real bug worth closing regardless, in the same defensive
/// spirit as every other budgeted path in this pipeline.
fn build_chunk_sprites(
    mut commands: Commands,
    mut chunks: MessageReader<CompressedChunk>,
    cache: Res<SpriteMeshCache>,
    mut index: ResMut<SpriteChunkIndex>,
    pending: Query<(Entity, &PendingChunkSprites)>,
    mut perf_log: Local<Option<bool>>,
    mut decode_queue: Local<VecDeque<CompressedChunk>>,
) {
    // Read `XINDELER_SPRITE_PERF_LOG` once per run (cached in the `Local`),
    // not every frame.
    let perf_log = *perf_log
        .get_or_insert_with(|| std::env::var("XINDELER_SPRITE_PERF_LOG").is_ok_and(|v| v != "0"));
    let decode_loop_start = std::time::Instant::now();
    let mut chunks_decoded_this_frame = 0usize;

    // 1. Newly-arrived chunks → collect + thin instances → a pending marker.
    //
    // Bevy's `Messages<T>` double-buffer only guarantees a message survives
    // for 2 `Update`s (bevy_ecs's own doc comment on `Messages`); a
    // `MessageReader` that hasn't caught up by then silently loses whatever
    // it didn't read. So the cap CANNOT be applied by early-`break`ing out of
    // `chunks.read()`'s iterator across MULTIPLE frames (an earlier version
    // of this fix did exactly that and a reviewer caught it: at
    // `CHUNK_BUILD_BURST_CAP` per frame, a real 30-chunk boot burst needs ~8
    // frames to drain, well past the 2-frame TTL — most of the burst would
    // be silently dropped, not deferred). Mirrors `pipeline.rs`'s own
    // pattern instead: `chunks.read()` is drained COMPLETELY, every frame,
    // into an OWNED `decode_queue` (cheap — no decode yet, just a clone of
    // the still-compressed bytes), and the expensive lz4-decode +
    // `collect_sprite_instances` voxel scan is what's actually capped,
    // popping from that TTL-free owned queue instead.
    decode_queue.extend(chunks.read().cloned());
    while chunks_decoded_this_frame < CHUNK_BUILD_BURST_CAP {
        let Some(msg) = decode_queue.pop_front() else {
            break;
        };
        let Some(chunk) = msg.decode() else { continue };
        chunks_decoded_this_frame += 1;
        // Despawn FIRST (built parent AND any in-flight marker), unconditionally
        // — a chunk edited to have NO vegetation must drop its old sprites too,
        // so this precedes the empty early-return below.
        despawn_chunk_sprites(&mut commands, &mut index, msg.key);

        let mut instances: Vec<_> = collect_sprite_instances(&chunk)
            .into_iter()
            .filter(|i| SPRITE_KINDS.contains(&i.kind))
            .collect();
        if instances.is_empty() {
            continue; // cleaned up above; nothing new to spawn
        }
        thin_to_budget(&mut instances, MAX_SPRITES_PER_CHUNK);
        let marker = commands
            .spawn(PendingChunkSprites {
                key: msg.key,
                instances,
            })
            .id();
        // Track the CURRENT marker so a later arrival (or the build pass) can
        // tell it apart from a stale one scheduled for despawn (commands are
        // deferred, so the query below still yields the old marker this frame).
        index.pending.insert(msg.key, marker);
    }
    if perf_log && chunks_decoded_this_frame > 0 {
        let elapsed_ms = decode_loop_start.elapsed().as_secs_f64() * 1000.0;
        debug!(
            chunks_decoded_this_frame,
            elapsed_ms, "EM-3.11p round 11: sprite decode+collect main-thread cost this frame"
        );
    }

    // 2. Pending chunks whose kinds are all Ready → build.
    let build_loop_start = std::time::Instant::now();
    let mut chunks_built_this_frame = 0usize;
    let mut children_spawned_this_frame = 0usize;
    for (marker_entity, chunk) in &pending {
        // Skip a marker that is no longer the current one for its key (a newer
        // arrival replaced it this frame; its despawn is queued).
        if index.pending.get(&chunk.key) != Some(&marker_entity) {
            continue;
        }
        let ready = chunk
            .instances
            .iter()
            .all(|i| matches!(cache.kinds.get(&i.kind), Some(SpriteKindState::Ready(_))));
        if !ready {
            // If any kind is Failed (never becoming Ready), drop those and
            // rebuild the readiness check on the survivors so a chunk isn't
            // stuck forever on a skipped kind.
            let any_failed = chunk
                .instances
                .iter()
                .any(|i| matches!(cache.kinds.get(&i.kind), Some(SpriteKindState::Failed)));
            if !any_failed {
                continue; // still loading — wait
            }
        }

        let Some(material) = &cache.material else {
            continue;
        };

        // This marker is being consumed either way — clear its pending slot and
        // schedule its despawn.
        index.pending.remove(&chunk.key);
        commands.entity(marker_entity).despawn();

        // Only the instances whose kind resolved to Ready will render; if none
        // will (every kind Failed), skip spawning an empty parent entirely.
        let renderable = chunk
            .instances
            .iter()
            .filter(|i| matches!(cache.kinds.get(&i.kind), Some(SpriteKindState::Ready(_))));
        if renderable.clone().next().is_none() {
            debug!(key = ?chunk.key, "sprite: no renderable kinds; skipping parent");
            continue;
        }

        let origin = chunk_origin_bevy(chunk.key);
        // Average sprite height (Bevy y) over the chunk, so the smoke cam can
        // aim at the vegetation layer rather than the chunk floor.
        let mut sum_y = 0.0f32;
        let mut children_spawned = 0usize;
        let parent = commands
            .spawn((
                Transform::from_translation(origin),
                Visibility::Visible,
                Name::new(format!("sprites[{},{}]", chunk.key[0], chunk.key[1])),
            ))
            .id();
        commands.entity(parent).with_children(|p| {
            for inst in renderable {
                let Some(SpriteKindState::Ready(variations)) = cache.kinds.get(&inst.kind) else {
                    continue; // unreachable (filtered), but keeps the match local
                };
                let variation = &variations[inst.variation_index(variations.len())];
                let tf = sprite_transform(inst);
                sum_y += origin.y + tf.translation.y;
                p.spawn((
                    Mesh3d(variation.mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    tf,
                ));
                children_spawned += 1;
            }
        });

        // Tag the parent with its world-space centroid + child count so the
        // EM-3.9 smoke camera can frame the densest, lit vegetation patch.
        let sz = {
            use common::vol::RectRasterableVol;
            common::terrain::TerrainChunk::RECT_SIZE.map(|e| e as f32)
        };
        let centroid = Vec3::new(
            origin.x + sz.x * 0.5,
            if children_spawned > 0 {
                sum_y / children_spawned as f32
            } else {
                origin.y
            },
            origin.z - sz.y * 0.5,
        );
        commands.entity(parent).insert(SpriteChunkParent {
            centroid,
            count: children_spawned,
        });

        index.built.insert(chunk.key, parent);
        chunks_built_this_frame += 1;
        children_spawned_this_frame += children_spawned;
        debug!(
            key = ?chunk.key,
            children_spawned,
            "sprite: built chunk sprites"
        );
    }
    if perf_log && chunks_built_this_frame > 0 {
        let elapsed_ms = build_loop_start.elapsed().as_secs_f64() * 1000.0;
        debug!(
            chunks_built_this_frame,
            children_spawned_this_frame,
            elapsed_ms,
            "EM-3.11p round 11: sprite spawn main-thread cost this frame"
        );
    }
}

/// Despawns a chunk's sprite parent (+ children) on unload.
fn remove_chunk_sprites(
    mut commands: Commands,
    mut removes: MessageReader<RemoveChunk>,
    mut index: ResMut<SpriteChunkIndex>,
) {
    for msg in removes.read() {
        despawn_chunk_sprites(&mut commands, &mut index, msg.key);
    }
}

/// Despawns whatever sprites exist for `key`, in EITHER phase: the built parent
/// (+ its children) and/or the in-flight [`PendingChunkSprites`] marker. Called
/// on unload AND on every re-arrival, so a key never leaks an entity or shows
/// doubled vegetation.
fn despawn_chunk_sprites(commands: &mut Commands, index: &mut SpriteChunkIndex, key: [i32; 2]) {
    if let Some(entity) = index.built.remove(&key) {
        commands.entity(entity).despawn();
    }
    if let Some(marker) = index.pending.remove(&key) {
        commands.entity(marker).despawn();
    }
}

/// The Bevy-space origin of a chunk (mirrors `pipeline::chunk_transform`:
/// Veloren `(32·kx, 32·ky, 0)` → Bevy `(32·kx, 0, −32·ky)`).
fn chunk_origin_bevy(key: [i32; 2]) -> Vec3 {
    use common::{terrain::TerrainChunk, vol::RectRasterableVol};
    let sz = TerrainChunk::RECT_SIZE.map(|e| e as f32);
    Vec3::new(key[0] as f32 * sz.x, 0.0, -(key[1] as f32 * sz.y))
}

/// Builds an instance's local `Transform` relative to the chunk-origin parent:
/// z-up→y-up rotate the chunk-local Veloren position, rotate about the (Bevy)
/// up axis by the sprite's z-orientation, mirror + scale. Mirrors voxygen's
/// per-instance `Mat4` (`scaled_3d(SPRITE_SCALE * mirror)` then place at wpos),
/// with the z-up→y-up map the whole render pipeline uses.
fn sprite_transform(inst: &xindeler_render_voxel::sprite::SpriteInstance) -> Transform {
    // Veloren chunk-local (x, y, z) → Bevy (x, z, −y).
    let translation = Vec3::new(inst.rel_pos.x, inst.rel_pos.z, -inst.rel_pos.y);
    // Veloren z-up rotation about +z maps to a Bevy rotation about +y. The
    // z-up→y-up map (x, y, z)→(x, z, −y) sends +z rotation to a −y rotation.
    let rotation = Quat::from_rotation_y(-inst.z_rot);
    // Mirror per axis (Veloren xyz → Bevy xzy with a sign flip on the mapped
    // −y): mirror.x→x, mirror.z→y, mirror.y→z. Scale by SPRITE_SCALE.
    let scale = Vec3::new(inst.mirror.x, inst.mirror.z, inst.mirror.y) * SPRITE_SCALE;
    Transform {
        translation,
        rotation,
        scale,
    }
}

/// Thins `instances` in place to at most `budget`, keeping a deterministic,
/// evenly-spread subset (stride sampling) so density scales down uniformly
/// rather than clipping a spatial corner.
fn thin_to_budget(
    instances: &mut Vec<xindeler_render_voxel::sprite::SpriteInstance>,
    budget: usize,
) {
    if instances.len() <= budget || budget == 0 {
        return;
    }
    // Keep every `stride`-th instance (ceil so we never exceed the budget).
    let stride = instances.len().div_ceil(budget);
    let kept: Vec<_> = instances
        .iter()
        .copied()
        .step_by(stride)
        .take(budget)
        .collect();
    *instances = kept;
}

#[cfg(test)]
mod tests {
    use xindeler_render_voxel::sprite::SpriteInstance;

    use super::*;

    fn dummy(kind: SpriteKind) -> SpriteInstance {
        SpriteInstance {
            kind,
            rel_pos: VVec3::new(0.5, 0.5, 0.0),
            z_rot: 0.0,
            mirror: VVec3::new(1.0, 1.0, 1.0),
            seed: 0,
        }
    }

    /// EM-3.9b whitelist expansion: no accidental duplicate `SpriteKind`
    /// (would double-load/double-mesh a kind and silently overwrite its cache
    /// slot) and the list actually grew past the original ~16 vegetation-only
    /// set.
    #[test]
    fn sprite_kinds_whitelist_has_no_duplicates_and_grew() {
        let mut seen = std::collections::HashSet::new();
        for &kind in SPRITE_KINDS {
            assert!(
                seen.insert(kind),
                "duplicate SpriteKind in SPRITE_KINDS: {kind:?}"
            );
        }
        assert!(
            SPRITE_KINDS.len() > 16,
            "EM-3.9b widened the whitelist past the original 16 grasses/flowers"
        );
    }

    #[test]
    fn thinning_respects_budget_and_is_stable() {
        let mut v: Vec<_> = (0..10_000).map(|_| dummy(SpriteKind::ShortGrass)).collect();
        thin_to_budget(&mut v, MAX_SPRITES_PER_CHUNK);
        assert!(v.len() <= MAX_SPRITES_PER_CHUNK, "thinned within budget");
        assert!(!v.is_empty(), "thinning keeps a representative subset");

        // Under budget → untouched.
        let mut small: Vec<_> = (0..5).map(|_| dummy(SpriteKind::ShortGrass)).collect();
        thin_to_budget(&mut small, MAX_SPRITES_PER_CHUNK);
        assert_eq!(small.len(), 5);
    }

    #[test]
    fn chunk_origin_maps_z_up_to_y_up() {
        // Chunk (1, 2): Veloren (32, 64, 0) → Bevy (32, 0, −64).
        assert_eq!(chunk_origin_bevy([1, 2]), Vec3::new(32.0, 0.0, -64.0));
    }

    #[test]
    fn sprite_transform_scales_by_sprite_scale() {
        let t = sprite_transform(&dummy(SpriteKind::ShortGrass));
        assert!((t.scale.x - SPRITE_SCALE).abs() < 1e-6);
        assert!((t.scale.y - SPRITE_SCALE).abs() < 1e-6);
        // rel_pos (0.5, 0.5, 0) → Bevy (0.5, 0, −0.5).
        assert_eq!(t.translation, Vec3::new(0.5, 0.0, -0.5));
    }

    // --- Fix #1 / #2: build_chunk_sprites lifecycle (headless, no GPU) ---

    use bevy::{
        app::App,
        asset::{AssetPlugin, Handle},
        prelude::MinimalPlugins,
    };
    use common::{
        terrain::{Block, BlockKind, TerrainChunk, TerrainChunkMeta},
        vol::WriteVol,
    };
    use vek::Rgb;

    /// A chunk with a rock floor and (optionally) a grass sprite layer on top.
    fn chunk(with_grass: bool) -> TerrainChunk {
        let mut c = TerrainChunk::new(0, Block::empty(), Block::empty(), TerrainChunkMeta::void());
        for lx in 0..8 {
            for ly in 0..8 {
                for z in 0..2 {
                    c.set(
                        VVec3::new(lx, ly, z),
                        Block::new(BlockKind::Rock, Rgb::new(120, 120, 120)),
                    )
                    .expect("in-bounds");
                }
                if with_grass {
                    c.set(VVec3::new(lx, ly, 2), Block::air(SpriteKind::ShortGrass))
                        .expect("in-bounds");
                }
            }
        }
        c
    }

    /// Headless app running only the sprite build/remove systems, with the mesh
    /// cache PRE-SEEDED so ShortGrass is `Ready` (a dummy mesh handle — no GPU,
    /// no real asset needed). This exercises the parent/marker bookkeeping.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Mesh>()
            .add_message::<CompressedChunk>()
            .add_message::<RemoveChunk>()
            .init_resource::<SpriteChunkIndex>()
            .add_systems(Update, (build_chunk_sprites, remove_chunk_sprites).chain());

        // Pre-seed: ShortGrass Ready with one dummy variation; material set.
        let mut cache = SpriteMeshCache {
            all_settled: true,
            material: Some(Handle::default()),
            ..Default::default()
        };
        cache.kinds.insert(
            SpriteKind::ShortGrass,
            SpriteKindState::Ready(vec![SpriteVariationMesh {
                mesh: Handle::default(),
            }]),
        );
        app.insert_resource(cache);
        app.finish();
        app
    }

    fn parent_count(app: &mut App) -> usize {
        let world = app.world_mut();
        world
            .query::<&SpriteChunkParent>()
            .iter(world)
            .filter(|p| p.count > 0)
            .count()
    }

    fn marker_count(app: &mut App) -> usize {
        let world = app.world_mut();
        world.query::<&PendingChunkSprites>().iter(world).count()
    }

    /// A key can build to at most ONE parent, and re-sending the same key does
    /// not duplicate it or leak an entity (fix #1).
    #[test]
    fn resending_a_key_does_not_duplicate_or_leak() {
        let mut app = test_app();

        app.world_mut()
            .write_message(CompressedChunk::encode([0, 0], &chunk(true)));
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(parent_count(&mut app), 1, "one built parent for the key");
        assert_eq!(marker_count(&mut app), 0, "marker consumed after build");

        // Re-send the SAME key several times (the normal startup case).
        for _ in 0..3 {
            app.world_mut()
                .write_message(CompressedChunk::encode([0, 0], &chunk(true)));
        }
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            parent_count(&mut app),
            1,
            "re-sending a key must not leak or duplicate parents"
        );
        assert_eq!(marker_count(&mut app), 0, "no stale markers linger");
        assert_eq!(
            app.world().resource::<SpriteChunkIndex>().built.len(),
            1,
            "exactly one indexed parent"
        );
        assert!(
            app.world()
                .resource::<SpriteChunkIndex>()
                .pending
                .is_empty(),
            "pending index drained"
        );
    }

    /// Re-sending a key with NO vegetation removes the old sprites (fix #2:
    /// despawn runs before the empty early-return).
    #[test]
    fn resending_empty_clears_old_sprites() {
        let mut app = test_app();

        app.world_mut()
            .write_message(CompressedChunk::encode([0, 0], &chunk(true)));
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(parent_count(&mut app), 1, "vegetation built first");

        // The chunk is edited to have no sprites and re-sent.
        app.world_mut()
            .write_message(CompressedChunk::encode([0, 0], &chunk(false)));
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            parent_count(&mut app),
            0,
            "an emptied chunk must drop its old sprites"
        );
        assert!(
            app.world().resource::<SpriteChunkIndex>().built.is_empty(),
            "built index cleared for the emptied key"
        );
    }

    /// `RemoveChunk` despawns a built parent (unload path).
    #[test]
    fn remove_chunk_despawns_sprites() {
        let mut app = test_app();

        app.world_mut()
            .write_message(CompressedChunk::encode([0, 0], &chunk(true)));
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(parent_count(&mut app), 1);

        app.world_mut().write_message(RemoveChunk { key: [0, 0] });
        for _ in 0..2 {
            app.update();
        }
        assert_eq!(parent_count(&mut app), 0, "RemoveChunk unloads the sprites");
    }
}
