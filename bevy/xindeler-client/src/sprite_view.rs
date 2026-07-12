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
//!    whitelisted [`sprite_render_kinds`] (EM-3.9c widens this from just the
//!    outdoor `Plant` category to also cover furniture/dungeon décor — see that
//!    function's docs).
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
//! - **Kind whitelist** ([`sprite_render_kinds`]): EM-3.9b widened this from
//!   ~16 outdoor kinds to the whole `Plant` category (grasses, flowers, cacti,
//!   crops, mushrooms); **EM-3.9c widens it further** to `Furniture`, `Decor`,
//!   `Lamp` and `Container` (furniture, dungeon décor/chests, standalone lights
//!   — placement already works generically: `sprite_z_rot`/ `sprite_mirror_vec`
//!   read the SAME `Ori`/`MirrorX` attributes regardless of category, so these
//!   categories are not a special case, just an unexplored one — verified
//!   against `common::terrain::block`). `Structural` (doors/windows/walls —
//!   several have interactive open/close behaviour in the old client this port
//!   doesn't reimplement) and `Modular` (adjacency- dependent fences, one
//!   variant) stay OUT for now; `Resource`/ `MineableResource` (ore/wood/gem
//!   world nodes) are a separate, gameplay- adjacent widening, not
//!   "furniture/prop/dungeon". The whitelist is now CATEGORY-based rather than
//!   a hand-typed kind list (mirrors the `Plant` widening's own spirit of "the
//!   whole category", scaled to multiple categories without enumerating ~150
//!   kind names by hand).
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
//! - **Wind sway v2 (EM-3.9c) — shipped, normal-consistent by construction.**
//!   EM-3.9b's attempt (a per-vertex sine sway via `ExtendedMaterial<
//!   StandardMaterial, SpriteWindMaterialExt>`) displaced `world_position` but
//!   left `world_normal` at its stock, unperturbed value — for a REAL 3-D
//!   voxel-meshed sprite (many faces at every axis orientation, unlike a flat
//!   billboard card) that mismatch drove the PBR diffuse term toward zero for a
//!   large share of faces/angles, rendering largely black (confirmed by
//!   disabling the effect: an exact match of the pre-EM-3.9b screenshot
//!   returned). v2 instead ROTATES both position and normal by the IDENTICAL
//!   per-vertex angle about the sprite's own world-space base pivot (Rodrigues'
//!   rotation formula — see `xindeler_render_voxel::material:: sprite_wind`'s
//!   WGSL for the full derivation), so the two stay geometrically consistent at
//!   every vertex, not merely at rest. The sway weight
//!   (`xindeler_render_voxel::convert::ATTRIBUTE_SPRITE_SWAY`) is baked
//!   per-mesh from each vertex's own height and the sprite's OWN authored
//!   `sprite_manifest.ron` `wind_sway` value
//!   (`xindeler_render_voxel::sprite::SpriteManifest::sway_strength`) — already
//!   correctly `0.0` for rigid props (furniture/decor/lamp/container, and even
//!   rigid `Plant`-category kinds like cacti) in the SHIPPED asset, so v2
//!   needed no synthetic per-category table of its own.
//!   [`SpriteMeshCache::material`] now holds a
//!   `Handle<xindeler_render_voxel::material::SpriteWindMaterial>` instead of a
//!   plain `StandardMaterial`; batching is unaffected (still one shared
//!   material handle for every sprite instance — EM-3.9b's own instancing
//!   argument above still holds). `XINDELER_SPRITE_WIND=0` disables sway at
//!   runtime (perf/quality escape hatch — see [`wind_strength_from_env`]).
//!
//! ## Purity
//! 100% Bevy + `xindeler-render-voxel` (a shell crate), `common` terrain types
//! and `dot_vox`/`ron` — NO specs. Compiled under EITHER the `listen-server`
//! OR `net-client` feature (both `listen_server.rs` and `net_client.rs` add
//! this plugin alongside `TerrainStreamPlugin`).
//!
//! ## Shared decoded-chunk store (EM-3.9c — resolves the EM-3.9b deferral)
//! EM-3.9b left this module decoding EVERY `CompressedChunk` a second time
//! (its own independent lz4+bincode pass), even though `terrain_stream`'s
//! `receive_chunks` had already decoded the SAME bytes into its own
//! `SharedTerrain` store moments earlier — deferred back then because
//! unifying the two lifecycles (this module's pending/built bookkeeping vs.
//! `terrain_stream`'s store/remesh bookkeeping) looked riskier than a cheap
//! per-chunk decompress. EM-3.9c takes the SAFER slice of that idea instead of
//! merging the lifecycles: [`build_chunk_sprites`] now reads
//! [`crate::terrain_stream::SharedTerrain::get_chunk`] first (a plain
//! `Arc::clone` of the chunk `terrain_stream` already decoded and owns) and
//! falls back to this module's own `CompressedChunk::decode` only on a miss
//! (chunk not resident yet, or the optional resource absent in a minimal test
//! harness) — no shared ownership/eviction contract between the two modules
//! is introduced, `SharedTerrain` stays the sole owner, and correctness never
//! depends on which system happens to run first in a given frame.

use std::{collections::VecDeque, sync::Arc};

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, LoadState, io::Reader},
    platform::collections::HashMap,
    prelude::*,
    reflect::TypePath,
};
use common::terrain::{SpriteKind, sprite::Category};
use vek::{Vec2 as VVec2, Vec3 as VVec3};
use xindeler_protocol::{CompressedChunk, RemoveChunk};
use xindeler_render_voxel::{
    material::SpriteWindMaterial,
    pipeline::ChunkMeshIndex,
    sprite::{
        SPRITE_MANIFEST, SPRITE_SCALE, SpriteManifest, collect_sprite_instances,
        sprite_model_to_bevy,
    },
};

/// Sprite CATEGORIES rendered client-side (EM-3.9c — see module docs for the
/// full reasoning): `Plant` (EM-3.9b's whole-category widening) plus
/// `Furniture`/`Decor`/`Lamp`/`Container` (EM-3.9c's furniture/dungeon-décor
/// widening). `Structural` and `Modular` stay out (module docs); `Resource`/
/// `MineableResource` are a separate, not-yet-attempted widening.
const RENDER_CATEGORIES: &[Category] = &[
    Category::Plant,
    Category::Furniture,
    Category::Decor,
    Category::Lamp,
    Category::Container,
];

/// Whether a sprite kind is one v1 renders — an O(1) category check (5-arm
/// linear scan) rather than a linear scan of a hand-typed kind list, so this
/// is cheap to call PER INSTANCE in the hot per-chunk filter below (unlike the
/// old `SPRITE_KINDS.contains(&kind)`, which scanned a list that has grown
/// past 150 entries now that furniture/décor/lamp/container are included).
#[must_use]
pub fn is_sprite_render_kind(kind: SpriteKind) -> bool {
    RENDER_CATEGORIES.contains(&kind.category())
}

/// All sprite kinds v1 renders, derived from [`RENDER_CATEGORIES`] rather
/// than a hand-typed list — scales to "the whole Furniture/Decor/Lamp/
/// Container categories" (~150+ kinds) without enumerating them by hand and
/// stays correct across an upstream sync that adds a new kind to an
/// already-whitelisted category. Iterated ONCE at startup (`load_sprite_models`
/// kicking off `.vox` loads), never per-frame — see [`is_sprite_render_kind`]
/// for the per-instance hot-path check.
pub fn sprite_render_kinds() -> impl Iterator<Item = SpriteKind> {
    SpriteKind::all()
        .iter()
        .copied()
        .filter(|k| is_sprite_render_kind(*k))
}

/// Legacy alias kept only for the hand-authored regression list below (which
/// still enumerates the original outdoor `Plant` widening by name); NOT used
/// by production code (see [`sprite_render_kinds`]/[`is_sprite_render_kind`]).
#[cfg(test)]
const SPRITE_KINDS: &[SpriteKind] = &[
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
    material: Option<Handle<SpriteWindMaterial>>,
    /// Whether the whitelisted `.vox` loads have been kicked off (once the
    /// manifest parsed).
    started: bool,
    /// Set once every kind has left `Loading` (all `Ready`/`Failed`), so
    /// [`load_sprite_models`] can early-out instead of re-scanning all kinds
    /// forever.
    all_settled: bool,
}

/// Reads `XINDELER_SPRITE_WIND` once (env, not a per-frame poll): unset or any
/// value other than `"0"` keeps the EM-3.9c v2 sway ON at its tuned default
/// strength (`1.0`); `"0"` disables it (`wind_strength: 0.0`), a zero-cost
/// escape hatch if a live perf check (or a future low-end graphics tier)
/// needs sway off without swapping materials.
///
/// **Measured (`--smoke-perf-run`, 15s window each, same dev machine):**
/// wind ON — mean 42.7ms/frame, p50 33.9ms, p95 97.1ms; wind OFF — mean
/// 32.6ms/frame, p50 21.8ms, p95 84.0ms. Directionally consistent with a real
/// (if modest) added GPU/CPU cost, but NOT a clean controlled A/B: the
/// machine was under heavy, variable load from unrelated concurrent
/// processes at measurement time, and the two runs booted fresh worlds with
/// different chunk-mesh counts during warmup (406 vs. 832) — i.e. different
/// scene content, not just the sway toggle. `rust-perf-reviewer`'s
/// independent cost-model analysis of `sprite_wind.wgsl` (Rodrigues rotation
/// is ~20-30 extra ALU ops for a swayable vertex, applied to sprite meshes of
/// a few dozen–low hundreds of vertices each) concluded this is trivial GPU
/// cost, not a bottleneck, at the density this codebase's whitelist/budget
/// caps sprites to (`MAX_SPRITES_PER_CHUNK`). Shipped ON by default on that
/// combined basis; the env var above remains the honest escape hatch if a
/// cleaner future measurement (an idle machine, matched world seeds) shows
/// otherwise.
fn wind_strength_from_env() -> f32 {
    match std::env::var("XINDELER_SPRITE_WIND") {
        Ok(v) if v == "0" => 0.0,
        _ => 1.0,
    }
}

/// The shared matte material for vertex-coloured sprites (base_color WHITE so
/// per-voxel colour shows through; slightly rough, double-sided so thin grass
/// cards are lit from both faces). One material for ALL sprites → batching.
/// EM-3.9c: `ExtendedMaterial<StandardMaterial, SpriteWindMaterialExt>`
/// instead of a plain `StandardMaterial` — same `base`, plus the wind-sway
/// extension (module docs).
fn sprite_material() -> SpriteWindMaterial {
    SpriteWindMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.9,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: xindeler_render_voxel::material::SpriteWindMaterialExt {
            wind_strength: wind_strength_from_env(),
        },
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
    mut materials: ResMut<Assets<SpriteWindMaterial>>,
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
        for kind in sprite_render_kinds() {
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
                && let Some(mesh) =
                    sprite_model_to_bevy(&vox.0, 0, p.offset, manifest.0.sway_strength(kind))
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
    // `Option` (round 18): the real app always has this (`VoxelRenderPlugin`
    // is unconditionally added in `main.rs`, wiring `xindeler-render-voxel`'s
    // `ChunkMeshPipelinePlugin`, before `SpriteViewPlugin`) — but a minimal
    // headless test harness exercising ONLY this module's own lifecycle
    // (this file's `tests` module) never wires the terrain-mesh pipeline at
    // all. Absent entirely degrades honestly to "no terrain-readiness gate"
    // (the same "optional host hook, not a silent no-op that could hide a
    // real production gap" convention xindeler-render-voxel::pipeline uses
    // for its other optional resources, since production always has this
    // one).
    mesh_index: Option<Res<ChunkMeshIndex>>,
    // `Option` (EM-3.9c, same convention as `mesh_index` above): the real app
    // always has this (`TerrainStreamPlugin` is unconditionally added in
    // `main.rs` before `SpriteViewPlugin`), but this file's own minimal
    // headless test harness never wires it. Absent degrades honestly to
    // "always take the own-decode fallback below" — never a silent gap.
    shared_terrain: Option<Res<crate::terrain_stream::SharedTerrain>>,
    mut perf_log: Local<Option<bool>>,
    mut decode_queue: Local<VecDeque<CompressedChunk>>,
) {
    // Read `XINDELER_SPRITE_PERF_LOG` once per run (cached in the `Local`),
    // not every frame.
    let perf_log = *perf_log
        .get_or_insert_with(|| std::env::var("XINDELER_SPRITE_PERF_LOG").is_ok_and(|v| v != "0"));
    let decode_loop_start = std::time::Instant::now();
    let mut chunks_processed_this_frame = 0usize;
    let mut chunks_reused_this_frame = 0usize;

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
    while chunks_processed_this_frame < CHUNK_BUILD_BURST_CAP {
        let Some(msg) = decode_queue.pop_front() else {
            break;
        };
        // BL-82 EM-3.9c: prefer the ALREADY-decoded chunk `terrain_stream`'s
        // own receive system stores in `SharedTerrain` (it processes every
        // arriving message uncapped, while this system's OWN processing is
        // deliberately deferred/capped at `CHUNK_BUILD_BURST_CAP` per frame —
        // so by the time a chunk reaches the front of THIS queue, it has
        // almost always already been decoded once by `terrain_stream` several
        // frames earlier). Falls back to this module's own
        // `CompressedChunk::decode` (a second lz4+bincode pass) ONLY when the
        // shared store doesn't have it yet — no ordering assumption between
        // the two systems is required for correctness, just for the (common)
        // fast path to actually trigger.
        let shared_hit = shared_terrain.as_deref().and_then(|s| s.get_chunk(msg.key));
        let chunk = match shared_hit {
            Some(chunk) => {
                chunks_reused_this_frame += 1;
                chunk
            },
            None => {
                let Some(chunk) = msg.decode() else { continue };
                Arc::new(chunk)
            },
        };
        chunks_processed_this_frame += 1;
        // Despawn FIRST (built parent AND any in-flight marker), unconditionally
        // — a chunk edited to have NO vegetation must drop its old sprites too,
        // so this precedes the empty early-return below.
        despawn_chunk_sprites(&mut commands, &mut index, msg.key);

        let mut instances: Vec<_> = collect_sprite_instances(&chunk)
            .into_iter()
            .filter(|i| is_sprite_render_kind(i.kind))
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
    if perf_log && chunks_processed_this_frame > 0 {
        let elapsed_ms = decode_loop_start.elapsed().as_secs_f64() * 1000.0;
        debug!(
            chunks_processed_this_frame,
            chunks_reused_this_frame,
            elapsed_ms,
            "EM-3.11p round 11 / EM-3.9c: sprite decode+collect main-thread cost this frame \
             (chunks_reused_this_frame came from the shared terrain store, no second decode)"
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
        // BL-82 EM-3.11 round 18: also wait for the chunk's REAL terrain mesh
        // (not `xindeler-render-voxel`'s EM-3.11h first-load placeholder box)
        // before spawning its sprites. This pipeline is otherwise entirely
        // decoupled from the terrain-mesh pipeline (module docs: its own
        // `CompressedChunk` stream, no shared timing) — without this gate, a
        // sprite (rendered with a normally-LIT `sprite_material()`, unlike
        // the terrain pipeline's deliberately `unlit` placeholder) can spawn
        // floating over/inside the flat, dim placeholder box before the real
        // terrain and its lighting exist for that spot, rendering as a solid
        // black silhouette that only "resolves" once the real mesh replaces
        // the placeholder — Matías's reported foliage "titilan cuando se
        // crean" (flicker when created). `xindeler-old` never has this gap:
        // its `mesh_worker` builds a chunk's opaque terrain AND its sprite
        // instances in the SAME async task, applied in ONE atomic swap
        // (`voxygen/src/scene/terrain/mod.rs`) — sprites there are
        // structurally incapable of appearing before their parent chunk's
        // real terrain. No timeout: a `CompressedChunk` and the matching
        // near-pipeline dirty-mark originate from the SAME "chunk arrived"
        // event (`terrain_stream.rs`), so `ChunkMeshIndex` is guaranteed to
        // gain at least a placeholder entry for this key at essentially the
        // same time — this just waits the FEW EXTRA frames until that
        // entry's placeholder resolves to real geometry, the same
        // wait-with-no-timeout style the sprite-kind-readiness check below
        // already uses.
        let terrain_ready = mesh_index
            .as_ref()
            .is_none_or(|idx| idx.has_real_terrain_mesh(VVec2::new(chunk.key[0], chunk.key[1])));
        if !terrain_ready {
            continue; // real terrain not up yet — keep waiting
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

    /// EM-3.9c widening: the CATEGORY-based whitelist covers every kind the
    /// old hand-typed `Plant`-only list did (no regression), grew past it
    /// (furniture/decor/lamp/container are now in), and still excludes the
    /// categories deliberately left out (module docs: `Structural`/`Modular`).
    #[test]
    fn render_kinds_covers_plant_and_new_categories_but_not_structural() {
        let rendered: std::collections::HashSet<_> = sprite_render_kinds().collect();

        for &kind in SPRITE_KINDS {
            assert!(
                rendered.contains(&kind),
                "{kind:?}: EM-3.9b's Plant-category kind dropped by the EM-3.9c category widening \
                 — regression"
            );
        }
        assert!(
            rendered.len() > SPRITE_KINDS.len(),
            "EM-3.9c should have added furniture/decor/lamp/container kinds on top of the {} \
             Plant-category kinds",
            SPRITE_KINDS.len()
        );
        assert!(
            rendered.contains(&SpriteKind::Barrel),
            "Furniture category kind (Barrel) should now render"
        );
        assert!(
            rendered.contains(&SpriteKind::Chest),
            "Container category kind (Chest) should now render"
        );
        assert!(
            rendered.contains(&SpriteKind::Lantern),
            "Lamp category kind (Lantern) should now render"
        );
        assert!(
            rendered.contains(&SpriteKind::Gravestone),
            "Decor category kind (Gravestone) should now render"
        );
        assert!(
            !rendered.contains(&SpriteKind::Door),
            "Structural category (doors have old-client interactive open/close behaviour this \
             port doesn't reimplement) must stay excluded"
        );
        assert!(
            !rendered.contains(&SpriteKind::FenceWoodWoodland),
            "Modular category (adjacency-dependent) must stay excluded"
        );
    }

    /// EM-3.9c: the real manifest's OWN authored `wind_sway` values already
    /// gate correctly — swayable outdoor grass is nonzero, rigid props
    /// (furniture/container/lamp/decor, and even a rigid `Plant`-category
    /// kind like a cactus) are exactly `0.0` — with NO synthetic per-category
    /// table needed on this port's side. `#[ignore]` — reads the real asset;
    /// run with `cargo test -p xindeler-client --features listen-server
    /// sway_strength_reads_the_real_authored_manifest -- --ignored`.
    #[test]
    #[ignore = "reads the real sprite_manifest.ron asset"]
    fn sway_strength_reads_the_real_authored_manifest() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/voxygen/voxel/sprite_manifest.ron");
        let bytes = std::fs::read(&path).expect("read sprite_manifest.ron");
        let manifest: SpriteManifest =
            ron::de::from_bytes(&bytes).expect("parse sprite_manifest.ron");

        assert!(
            manifest.sway_strength(SpriteKind::ShortGrass) > 0.0,
            "grass should sway"
        );
        assert_eq!(manifest.sway_strength(SpriteKind::Barrel), 0.0);
        assert_eq!(manifest.sway_strength(SpriteKind::CrateBlock), 0.0);
        assert_eq!(manifest.sway_strength(SpriteKind::Chest), 0.0);
        assert_eq!(manifest.sway_strength(SpriteKind::Lantern), 0.0);
        assert_eq!(manifest.sway_strength(SpriteKind::Gravestone), 0.0);
        assert_eq!(
            manifest.sway_strength(SpriteKind::BarrelCactus),
            0.0,
            "a rigid Plant-category kind (cactus) is ALSO authored at 0.0 in the real manifest — \
             confirms the manifest-driven approach is strictly better than a Plant-category-wide \
             guess"
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

    // -------------------------------------------------------------------
    // BL-82 EM-3.11 round 18 — sprites wait for the REAL terrain mesh
    // -------------------------------------------------------------------

    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use common::{terrain::MapSizeLg, volumes::vol_grid_2d::VolGrid2d};
    use xindeler_render_voxel::pipeline::{
        ChunkLayerMap, ChunkMaterials, ChunkMeshPipelinePlugin, ChunkMeshQueue, ChunkUploadBudget,
        ChunkVolume, ChunkVolumeProvider,
    };

    /// Headless app running BOTH pipelines together (the REAL
    /// `ChunkMeshPipelinePlugin`, not a fake), so [`build_chunk_sprites`]'s
    /// round-18 [`ChunkMeshIndex::has_real_terrain_mesh`] gate is exercised
    /// against a genuine placeholder-then-real transition, not an assumption.
    fn test_app_with_terrain_pipeline(volume_available: Arc<AtomicBool>) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_message::<CompressedChunk>()
            .add_message::<RemoveChunk>()
            .init_resource::<SpriteChunkIndex>()
            .add_plugins(ChunkMeshPipelinePlugin)
            .add_systems(Update, (build_chunk_sprites, remove_chunk_sprites).chain());

        // A real 3x3-chunk grid (default map size big enough to hold key
        // (3,3) + its ±1 mesher border) with the SAME `chunk(true)` (rock +
        // grass) at (3,3); neighbours stay the grid's default (empty) chunk,
        // which is fine — the mesher already treats a missing neighbour that
        // way (module docs on `ChunkVolume`).
        let map_size_lg = MapSizeLg::new(VVec2::new(3, 3)).expect("valid test map size");
        let default = Arc::new(TerrainChunk::new(
            0,
            Block::empty(),
            Block::empty(),
            TerrainChunkMeta::void(),
        ));
        let mut grid = VolGrid2d::new(map_size_lg, default).expect("chunk size is a power of two");
        grid.insert(VVec2::new(3, 3), Arc::new(chunk(true)));
        let grid = Arc::new(grid);

        app.insert_resource(ChunkVolumeProvider::new(move |key| {
            (volume_available.load(Ordering::Relaxed) && key == VVec2::new(3, 3))
                .then(|| ChunkVolume::with_z_bounds(grid.clone(), key, 0, 2))
        }))
        .insert_resource(ChunkLayerMap::default())
        .insert_resource(ChunkMaterials {
            terrain: Handle::default(),
            fluid: Handle::default(),
        })
        .insert_resource(ChunkUploadBudget::default());

        // Pre-seed: ShortGrass Ready with one dummy variation; material set
        // (same as `test_app`'s own pre-seed — this test is about the
        // TERRAIN gate, not sprite-asset loading).
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

    /// The load-bearing round-18 regression: sprites for a chunk must NOT
    /// spawn while that chunk has NO real terrain mesh at all — even though
    /// their sprite-kind ASSETS are already `Ready` and their
    /// `PendingChunkSprites` marker is already built — and must spawn once
    /// the real terrain mesh appears. The volume provider starts UNAVAILABLE
    /// (not merely "still meshing") so the "must not build yet" window is
    /// fully deterministic (no async-task-timing race): `ChunkMeshIndex` is
    /// guaranteed empty for this key until the test explicitly flips the
    /// provider on, unlike waiting on a real in-flight mesh task, which could
    /// in principle finish before the assertion runs on a slow/loaded CI box.
    ///
    /// Verified non-tautological: reverting the `has_real_terrain_mesh` gate
    /// in `build_chunk_sprites` (temporarily hardcoding `terrain_ready =
    /// true`) makes the FIRST assertion below fail — a parent builds while
    /// the provider is still unavailable and `ChunkMeshIndex` has no entry
    /// for the key at all.
    #[test]
    fn sprites_wait_for_the_real_terrain_mesh_before_spawning() {
        let available = Arc::new(AtomicBool::new(false));
        let mut app = test_app_with_terrain_pipeline(Arc::clone(&available));
        let key = VVec2::new(3, 3);

        // Mirrors production: the SAME "chunk arrived" event marks BOTH the
        // near-terrain pipeline's dirty queue (`terrain_stream.rs::
        // receive_chunks` in the real app) AND sends the sprite pipeline's
        // `CompressedChunk` (this test does both explicitly, since the
        // minimal harness above wires neither `terrain_stream` nor
        // `net_client`/`listen_server`). The provider is unavailable, so the
        // terrain pipeline's own `spawn_chunk_mesh_tasks` drops this mark
        // entirely (module docs: a `None` fetch cancels it, no entry
        // created — not even a placeholder) — deterministically "no real
        // terrain, at all" for as long as `available` stays false.
        app.world_mut()
            .resource_mut::<ChunkMeshQueue>()
            .mark_dirty(key);
        app.world_mut()
            .write_message(CompressedChunk::encode([3, 3], &chunk(true)));

        // Several updates: enough for the sprite pipeline's own two-phase
        // decode-then-build to fully settle (its `PendingChunkSprites`
        // marker needs at least one extra update to become query-visible
        // after the `Commands::spawn` that creates it — same reason the
        // OTHER tests in this module poll `for _ in 0..4`), while the
        // terrain provider stays unavailable throughout.
        for _ in 0..4 {
            app.update();
        }
        assert!(
            app.world()
                .resource::<xindeler_render_voxel::pipeline::ChunkMeshIndex>()
                .get(key)
                .is_none(),
            "sanity: with the provider unavailable, the terrain pipeline must have NO entry at \
             all for this key (not even a placeholder)"
        );
        assert_eq!(
            parent_count(&mut app),
            0,
            "sprites must NOT spawn while the chunk has no real terrain mesh at all, even though \
             their sprite-kind assets are already Ready and their PendingChunkSprites marker is \
             already built — this is round 18's whole point (module docs: xindeler-old ties \
             sprite spawn to the SAME atomic mesh-worker response as the real terrain; this \
             port's decoupled pipeline must wait explicitly instead)"
        );
        assert_eq!(
            marker_count(&mut app),
            1,
            "the pending marker must still be waiting, not consumed"
        );

        // Let the terrain actually arrive: flip the provider on and re-mark
        // the key (the earlier mark was consumed — dropped, per module docs
        // — by the failed fetch above).
        available.store(true, Ordering::Relaxed);
        app.world_mut()
            .resource_mut::<ChunkMeshQueue>()
            .mark_dirty(key);

        // Keep updating until the real mesh replaces the (now-spawned)
        // placeholder (the async task pool needs a little wall-clock time —
        // same polling pattern `xindeler-render-voxel`'s own pipeline tests
        // use).
        let mut settled = false;
        for _ in 0..500 {
            app.update();
            if app
                .world()
                .resource::<xindeler_render_voxel::pipeline::ChunkMeshIndex>()
                .has_real_terrain_mesh(key)
            {
                settled = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            settled,
            "the real terrain mesh must eventually replace the placeholder"
        );

        // One more update so `build_chunk_sprites` can observe the now-real
        // terrain and build the sprite parent.
        app.update();
        assert_eq!(
            parent_count(&mut app),
            1,
            "sprites must spawn once the chunk's real terrain mesh is up"
        );
        assert_eq!(marker_count(&mut app), 0, "marker consumed after build");
    }
}
