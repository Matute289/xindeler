//! Item-icon cache + async generation.
//!
//! Turns an [`ItemKey`] into a real rasterized icon [`Handle<Image>`], lazily
//! and off the main thread. Three async stages, each polled once per frame,
//! mirroring `xindeler-render-voxel::pipeline`'s chunk-mesh task pattern:
//!
//! 1. [`ItemIconManifestLoader`] loads `voxygen/item_image_manifest.ron` (an
//!    [`ItemImageManifest`] of ~1400 hand-tuned per-item render specs) through
//!    Bevy's own `AssetServer` — this crate deliberately never touches
//!    `common_assets`' specifier-based loading (same boundary `figure_view`'s
//!    manifest loaders already respect), so it's a small RON `AssetLoader`, not
//!    a call into `item_image::load_manifest`.
//! 2. A request for `ItemKey` with no cache entry yet queues the `.vox`/ `.png`
//!    sub-asset load (reusing `figure_view::VoxAsset`/`VoxLoader` — already
//!    registered by `FigureViewPlugin`, added in both game modes before this
//!    plugin).
//! 3. Once that sub-asset is loaded, an `AsyncComputeTaskPool` task runs
//!    `xindeler_render_voxel::item_icon::render_item_icon` off-thread; the
//!    finished [`image::RgbaImage`] is wrapped as a `bevy::Image` and inserted
//!    into `Assets<Image>`.
//!
//! **Single canonical resolution, not one entry per slot size.** The UI
//! actually uses ~9 distinct slot pixel sizes (34-54px, across bag/hotbar/
//! trade/paperdoll/diary/crafting) — caching a rasterized copy per size would
//! multiply memory ~9x for no visible benefit, since a single [`ICON_PX`]
//! render downscales cleanly through the exact same `ImageNode`-sizing path
//! every other UI image already goes through (`map_view.rs`'s icons, slot
//! rarity backgrounds, ...). So [`ItemIconCache`] keys on [`ItemKey`] alone.

use std::collections::{HashMap, HashSet};

use bevy::{
    app::{App, Plugin, Update},
    asset::{
        Asset, AssetApp, AssetLoader, AssetServer, Assets, Handle, LoadContext, LoadState,
        io::Reader,
    },
    ecs::{resource::Resource, schedule::IntoScheduleConfigs, system::ResMut},
    image::Image,
    reflect::TypePath,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    tasks::{AsyncComputeTaskPool, Task, block_on},
};
use common::comp::inventory::item::{
    item_image::{ImageSpec, ItemImageManifest},
    item_key::ItemKey,
};
use vek::Vec2;
use xindeler_render_voxel::item_icon::{IconTransform, load_icon_segment, render_item_icon};

use crate::figure_view::VoxAsset;

/// The one resolution every icon is rasterized at; the UI downscales as
/// needed per slot (see the module doc's "single canonical resolution"
/// section). Comfortably above the largest real slot size (54px,
/// `EquipSlot::ActiveMainhand`/`ActiveOffhand`) for a clean downscale.
const ICON_PX: u16 = 64;

/// Installs the item-icon manifest loader, the [`ItemIconCache`], and the
/// generation pipeline systems. Requires
/// [`crate::figure_view::FigureViewPlugin`] to already be added (reuses its
/// [`VoxAsset`]/`VoxLoader` registration).
pub struct ItemIconPlugin;

impl Plugin for ItemIconPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<ItemIconManifestAsset>()
            .init_asset_loader::<ItemIconManifestLoader>()
            .init_resource::<ItemIconCache>()
            .init_resource::<PendingIconRequests>()
            .init_resource::<PendingIconVox>()
            .init_resource::<IconRasterTasks>()
            .add_systems(bevy::app::Startup, load_manifest)
            .add_systems(
                Update,
                (
                    start_pending_icon_loads,
                    poll_pending_icon_vox,
                    apply_finished_icon_tasks,
                )
                    .chain(),
            );
    }
}

// ---------------------------------------------------------------------------
// Manifest loading (Bevy-native RON asset — see module doc §1)
// ---------------------------------------------------------------------------

/// Bevy asset wrapper for the item-icon manifest ([`ItemImageManifest`]).
#[derive(Asset, TypePath)]
pub struct ItemIconManifestAsset(pub ItemImageManifest);

#[derive(Default, TypePath)]
struct ItemIconManifestLoader;

impl AssetLoader for ItemIconManifestLoader {
    type Asset = ItemIconManifestAsset;
    type Error = bevy::ecs::error::BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _ctx: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ItemIconManifestAsset(ron::de::from_bytes(&bytes)?))
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

/// The manifest's load handle — kept alive for the app's lifetime so the
/// asset is never dropped once loaded. Load state is polled directly via
/// `AssetServer`/`Assets<ItemIconManifestAsset>` rather than mirrored into
/// this resource, so there's exactly one source of truth.
#[derive(Resource)]
struct ItemIconManifestHandle(Handle<ItemIconManifestAsset>);

fn load_manifest(
    mut commands: bevy::ecs::system::Commands,
    asset_server: bevy::ecs::system::Res<AssetServer>,
) {
    let handle = asset_server.load("voxygen/item_image_manifest.ron");
    commands.insert_resource(ItemIconManifestHandle(handle));
}

// ---------------------------------------------------------------------------
// The cache + request API
// ---------------------------------------------------------------------------

/// `ItemKey -> Handle<Image>` for every icon generated so far this session.
/// Never evicted (the catalogue is a few thousand items at most).
#[derive(Resource, Default)]
pub struct ItemIconCache {
    pub(crate) map: HashMap<ItemKey, Handle<Image>>,
}

impl ItemIconCache {
    /// The icon for `key`, if it's already been generated. Consumed by each
    /// slot-rendering screen (bag/paperdoll/hotbar/trade/...) to swap its
    /// `icon_text` placeholder for the real image once one exists.
    pub fn get(&self, key: &ItemKey) -> Option<Handle<Image>> { self.map.get(key).cloned() }
}

/// Keys requested but not yet resolved against the manifest (waiting on
/// [`ItemIconManifestAsset`] to finish loading, or waiting to be drained on
/// the frame they were requested).
#[derive(Resource, Default)]
pub struct PendingIconRequests(pub(crate) HashSet<ItemKey>);

/// Request an icon for `key`. Returns the cached handle if one already
/// exists; otherwise queues generation (a no-op if already in flight) and
/// returns `None` for this frame — callers should keep using their fallback
/// (`icon_text`) until a later frame's cache lookup succeeds.
pub fn request_icon(
    cache: &ItemIconCache,
    pending: &mut PendingIconRequests,
    vox_pending: &PendingIconVox,
    tasks: &IconRasterTasks,
    key: &ItemKey,
) -> Option<Handle<Image>> {
    if let Some(handle) = cache.get(key) {
        return Some(handle);
    }
    let in_flight = pending.0.contains(key)
        || vox_pending.0.iter().any(|p| &p.key == key)
        || tasks.0.contains_key(key);
    if !in_flight {
        pending.0.insert(key.clone());
    }
    None
}

// ---------------------------------------------------------------------------
// Stage 2: manifest -> sub-asset load
// ---------------------------------------------------------------------------

struct PendingIconVoxEntry {
    key: ItemKey,
    spec: ImageSpec,
    vox_handle: Handle<VoxAsset>,
}

/// Keys whose `.vox` sub-asset is loading (Png specs skip this stage
/// entirely — see [`start_pending_icon_loads`]).
#[derive(Resource, Default)]
pub struct PendingIconVox(Vec<PendingIconVoxEntry>);

/// Drains [`PendingIconRequests`] once the manifest is ready: looks up each
/// key's [`ImageSpec`], and either loads the `.vox` sub-asset (queuing it in
/// [`PendingIconVox`] for the next stage) or, for a flat PNG, loads it
/// directly as an `Image` and inserts it straight into the cache (no
/// rasterization needed). An unmapped key is dropped silently (no manifest
/// entry — the caller's `icon_text` fallback covers it visually).
fn start_pending_icon_loads(
    manifest_handle: Option<bevy::ecs::system::Res<ItemIconManifestHandle>>,
    manifests: bevy::ecs::system::Res<Assets<ItemIconManifestAsset>>,
    asset_server: bevy::ecs::system::Res<AssetServer>,
    mut pending: ResMut<PendingIconRequests>,
    mut vox_pending: ResMut<PendingIconVox>,
    mut cache: ResMut<ItemIconCache>,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(manifest_handle) = manifest_handle else {
        return;
    };
    let Some(ItemIconManifestAsset(manifest)) = manifests.get(&manifest_handle.0) else {
        return; // still loading — retry next frame, requests stay queued
    };

    for key in std::mem::take(&mut pending.0) {
        let Some(spec) = manifest.get(&key) else {
            tracing::debug!(
                ?key,
                "item-icon: no manifest entry, falling back to icon_text"
            );
            continue;
        };
        match spec {
            ImageSpec::Png(_) => {
                let handle: Handle<Image> = asset_server.load(crate::figure_view::asset_path(
                    &spec.full_specifier(),
                    "png",
                ));
                cache.map.insert(key, handle);
            },
            ImageSpec::Vox(..) | ImageSpec::VoxTrans(..) => {
                let vox_handle = asset_server.load(crate::figure_view::asset_path(
                    &spec.full_specifier(),
                    "vox",
                ));
                vox_pending.0.push(PendingIconVoxEntry {
                    key,
                    spec: spec.clone(),
                    vox_handle,
                });
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 3: sub-asset ready -> rasterize off-thread
// ---------------------------------------------------------------------------

/// In-flight rasterization tasks, one per key.
#[derive(Resource, Default)]
pub struct IconRasterTasks(HashMap<ItemKey, Task<image::RgbaImage>>);

/// Polls [`PendingIconVox`] for finished `.vox` loads and spawns the
/// off-thread rasterization task for each. A failed load drops the key
/// (falls back to `icon_text`, same as an unmapped manifest entry).
fn poll_pending_icon_vox(
    asset_server: bevy::ecs::system::Res<AssetServer>,
    vox_assets: bevy::ecs::system::Res<Assets<VoxAsset>>,
    mut vox_pending: ResMut<PendingIconVox>,
    mut tasks: ResMut<IconRasterTasks>,
) {
    if vox_pending.0.is_empty() {
        return;
    }
    let pool = AsyncComputeTaskPool::get();
    let mut still_pending = Vec::new();

    for entry in std::mem::take(&mut vox_pending.0) {
        match asset_server.get_load_state(&entry.vox_handle) {
            Some(LoadState::Loaded) => {
                let Some(VoxAsset(vox)) = vox_assets.get(&entry.vox_handle) else {
                    continue; // asset event/state raced the Assets store — retry never needed, drop
                };
                // `DotVoxData` isn't `Clone` (and shouldn't need to be — it's
                // borrowed from the `Assets<VoxAsset>` store), so the cheap
                // parse-into-`Segment` step runs here, synchronously, on the
                // main thread; only the expensive per-voxel-AO meshing +
                // software rasterization moves to the owned, `Clone`
                // `Segment` in the async task below.
                let (model_index, color, transform) = spec_render_params(&entry.spec);
                let segment = load_icon_segment(vox, model_index, color);
                let task = pool.spawn(async move {
                    render_item_icon(&segment, transform, Vec2::new(ICON_PX, ICON_PX))
                });
                tasks.0.insert(entry.key, task);
            },
            Some(LoadState::Failed(_)) => {
                tracing::debug!(key = ?entry.key, "item-icon: .vox load failed, falling back to icon_text");
            },
            _ => still_pending.push(entry),
        }
    }
    vox_pending.0 = still_pending;
}

/// Extracts `(model_index, color, IconTransform)` from an [`ImageSpec`] —
/// `Png` never reaches here (handled in [`start_pending_icon_loads`]).
fn spec_render_params(spec: &ImageSpec) -> (u32, Option<[u8; 3]>, IconTransform) {
    match spec {
        ImageSpec::Png(_) => {
            unreachable!("Png specs are resolved directly in start_pending_icon_loads")
        },
        ImageSpec::Vox(_, model_index, color) => (*model_index, *color, IconTransform::default()),
        ImageSpec::VoxTrans(_, offset, [rx, ry, rz], zoom, model_index, color) => {
            let ori = vek::Quaternion::rotation_x(rx.to_radians())
                .rotated_y(ry.to_radians())
                .rotated_z(rz.to_radians());
            (*model_index, *color, IconTransform {
                ori,
                offset: vek::Vec3::from(*offset),
                zoom: *zoom,
            })
        },
    }
}

/// Applies finished rasterization tasks: wraps the `RgbaImage` as a
/// `bevy::Image`, inserts it into `Assets<Image>`, and stores the handle in
/// [`ItemIconCache`].
fn apply_finished_icon_tasks(
    mut tasks: ResMut<IconRasterTasks>,
    mut images: ResMut<Assets<Image>>,
    mut cache: ResMut<ItemIconCache>,
) {
    if tasks.0.is_empty() {
        return;
    }
    let ready: Vec<ItemKey> = tasks
        .0
        .iter()
        .filter(|(_, task)| task.is_finished())
        .map(|(key, _)| key.clone())
        .collect();

    for key in ready {
        let Some(task) = tasks.0.remove(&key) else {
            continue;
        };
        let rgba = block_on(task);
        let image = Image::new(
            Extent3d {
                width: rgba.width(),
                height: rgba.height(),
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            rgba.into_raw(),
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        );
        let handle = images.add(image);
        cache.map.insert(key, handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::comp::inventory::item::item_image::ImageSpec;

    #[test]
    fn request_icon_returns_cached_handle_without_requeueing() {
        let mut cache = ItemIconCache::default();
        let mut pending = PendingIconRequests::default();
        let vox_pending = PendingIconVox::default();
        let tasks = IconRasterTasks::default();
        let key = ItemKey::Simple("Anvil".to_owned());

        let mut images = Assets::<Image>::default();
        let handle = images.add(Image::default());
        cache.map.insert(key.clone(), handle.clone());

        let result = request_icon(&cache, &mut pending, &vox_pending, &tasks, &key);
        assert_eq!(result, Some(handle));
        assert!(
            pending.0.is_empty(),
            "an already-cached key must not be queued for (re)generation"
        );
    }

    #[test]
    fn request_icon_queues_an_uncached_key_exactly_once() {
        let cache = ItemIconCache::default();
        let mut pending = PendingIconRequests::default();
        let vox_pending = PendingIconVox::default();
        let tasks = IconRasterTasks::default();
        let key = ItemKey::Simple("Anvil".to_owned());

        assert_eq!(
            request_icon(&cache, &mut pending, &vox_pending, &tasks, &key),
            None
        );
        assert!(pending.0.contains(&key));

        // A second request for the same still-uncached key must not add a
        // duplicate queue entry (it's already pending).
        request_icon(&cache, &mut pending, &vox_pending, &tasks, &key);
        assert_eq!(pending.0.len(), 1);
    }

    #[test]
    fn request_icon_does_not_requeue_a_key_already_in_the_vox_or_task_stage() {
        let cache = ItemIconCache::default();
        let mut pending = PendingIconRequests::default();
        let mut vox_pending = PendingIconVox::default();
        let tasks = IconRasterTasks::default();
        let key = ItemKey::Simple("Anvil".to_owned());

        vox_pending.0.push(PendingIconVoxEntry {
            key: key.clone(),
            spec: ImageSpec::Vox("voxel.sprite.crafting_station.anvil".to_owned(), 0, None),
            vox_handle: Handle::default(),
        });

        request_icon(&cache, &mut pending, &vox_pending, &tasks, &key);
        assert!(
            pending.0.is_empty(),
            "a key already resolving its .vox sub-asset must not be re-queued"
        );
    }

    #[test]
    fn spec_render_params_converts_degrees_to_the_matching_quaternion() {
        let spec = ImageSpec::VoxTrans(
            "voxel.sprite.crafting_station.anvil".to_owned(),
            [0.5, 0.5, 0.0],
            [0.0, 60.0, 90.0],
            1.0,
            0,
            None,
        );
        let (model_index, color, transform) = spec_render_params(&spec);
        assert_eq!(model_index, 0);
        assert_eq!(color, None);
        assert_eq!(transform.zoom, 1.0);
        assert_eq!(transform.offset, vek::Vec3::new(0.5, 0.5, 0.0));
        // Sanity: a non-identity rotation was actually produced (exact
        // component comparison is brittle across quaternion conventions —
        // the render-side rotation math itself is covered by
        // `xindeler-render-voxel::item_icon`'s own tests).
        assert_ne!(transform.ori, vek::Quaternion::identity());
    }

    /// End-to-end against the real asset tree: request `Simple("Anvil")`,
    /// tick the app enough frames for the manifest load, the `.vox`
    /// sub-asset load, and the off-thread rasterization task to all
    /// complete, and confirm the cache actually ends up holding a real
    /// generated icon handle — not just that the pipeline compiles.
    #[test]
    #[ignore = "needs the real asset tree checked out (LFS); run locally"]
    fn the_full_pipeline_eventually_populates_the_cache_for_a_real_item() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::MinimalPlugins);
        // `AssetPlugin::default()`'s `file_path` resolves relative to
        // `CARGO_MANIFEST_DIR` (this crate's own dir), not the workspace
        // root — same gotcha `main.rs` documents on `atmosphere::assets_root`.
        // Point it at the real asset tree so the manifest + `.vox` loads
        // actually resolve.
        app.add_plugins(bevy::asset::AssetPlugin {
            file_path: crate::atmosphere::assets_root()
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        });
        app.init_asset::<Image>();
        // `FigureViewPlugin`'s own systems (unrelated to icon rendering)
        // also spawn real figures off `VoxAsset` loads, and pull
        // `ResMut<Assets<Mesh>>`/`ResMut<Assets<StandardMaterial>>` to do it
        // — register both asset containers so those systems don't fail
        // their parameter validation on a minimal, renderless app.
        app.init_asset::<bevy::mesh::Mesh>();
        app.init_asset::<bevy::pbr::StandardMaterial>();
        app.add_plugins(crate::figure_view::FigureViewPlugin);
        app.add_plugins(ItemIconPlugin);

        let key = ItemKey::Simple("Anvil".to_owned());
        app.world_mut()
            .resource_mut::<PendingIconRequests>()
            .0
            .insert(key.clone());

        // Up to ~200 frames: manifest RON + a real .vox file are small, but
        // `AssetServer` loading and `AsyncComputeTaskPool` scheduling are
        // both genuinely async — this just needs enough ticks for both to
        // settle, not a tight bound. `request_icon`'s own dedup/re-queue
        // behaviour is covered separately above; this test only needs the
        // ONE initial queue entry to prove the rest of the pipeline (asset
        // load -> vox load -> off-thread rasterize -> cache) actually runs.
        let mut resolved = None;
        for _ in 0..200 {
            app.update();
            if let Some(handle) = app.world().resource::<ItemIconCache>().get(&key) {
                resolved = Some(handle);
                break;
            }
        }
        let handle = resolved.expect(
            "the cache must hold a real icon handle for Simple(\"Anvil\") within 200 frames",
        );
        let images = app.world().resource::<Assets<Image>>();
        let image = images
            .get(&handle)
            .expect("the cached handle must resolve to a real Image asset");
        assert_eq!(image.texture_descriptor.size.width, u32::from(ICON_PX));
        assert_eq!(image.texture_descriptor.size.height, u32::from(ICON_PX));
    }
}
