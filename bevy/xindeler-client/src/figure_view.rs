//! EM-3.8 / EM-3.8b — real `.vox` figures for mirrored entities (listen-server
//! only).
//!
//! Replaces EM-3.7's placeholder capsules with the actual Veloren voxel models.
//! The heavy lifting (meshing, recolour, bone placement, animation) lives in
//! `xindeler-render-voxel::figure`; THIS module is the client-side asset glue.
//!
//! ## Body paths (EM-3.8 → EM-3.8c)
//! - **quadruped-small** (EM-3.8, animated in EM-3.8c): two manifests + raw
//!   `.vox` parts → the sim's test-NPC Pig.
//! - **quadruped-medium** (EM-3.8c): two manifests → Wolf/Bear/etc.
//! - **bird-medium** (EM-3.8c): two manifests → Owl/Duck/etc (idle/run/fly).
//! - **humanoid** (EM-3.8b + EM-3.8c weapon): eight manifests (colour + head +
//!   six armour slots) → recoloured 16-bone character + a TEST main-hand
//!   weapon.
//!
//! Every body is animated per frame by the single [`animate_figures`] system
//! (idle vs run from the replicated velocity), each figure carrying a
//! [`FigureAnimState`] (phase accumulator + idle/run hysteresis).
//!
//! ## Flow (decoupled from load timing)
//! `super::entity_view::add_presentation` still gives EVERY new mirrored entity
//! a placeholder capsule on `Added<NetBody>`. THIS module then, every frame for
//! any entity not yet finalised (`Without<FigureBuilt>`):
//! 1. classifies the body (`resolve_figure_body`);
//! 2. for a SUPPORTED body, once its manifests are parsed, starts loading its
//!    `.vox` parts ([`PendingFigure`] / [`PendingHumanoid`]);
//! 3. once every part has loaded, REPLACES the capsule (removes `Mesh3d`/
//!    `MeshMaterial3d`) with one child entity per part — each meshed (`figure`
//!    crate) and parented at its bone `Transform` — then marks [`FigureBuilt`]
//!    (humanoids additionally get a [`HumanoidFigure`] the animation drives).
//!
//! Still-unsupported bodies (quadruped-low, bipeds, dragons, …) are marked
//! [`FigureBuilt`] immediately and keep their capsule.
//!
//! EM-3.8d: humanoids now assemble their REAL equipped gear — the mirror sends
//! each humanoid's [`NetLoadout`] (weapon(s) + armour + lantern), which this
//! module translates to a `FigureLoadout` and feeds to the humanoid assembly,
//! so equipping different items on the sim character changes the Bevy figure.
//! `TODO(EM-3.8e)`: head-slot helmets + the glider (glide-state-gated) + the
//! remaining bodies.
//!
//! ## Purity
//! 100% Bevy + `xindeler-render-voxel` (a shell crate) + `dot_vox`/`ron` — NO
//! specs. The replicated [`NetBody`] carries the plain `common::comp::Body`
//! data enum (EM-3.8 protocol enrichment); we pattern-match it, never touch the
//! ECS. Compiled only under the `listen-server` feature.

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, LoadState, io::Reader},
    prelude::*,
    reflect::TypePath,
};
use dot_vox::DotVoxData;
use xindeler_protocol::{NetBody, NetLoadout, NetTool, NetToolKey, NetVel};
use xindeler_render_voxel::figure::{
    self, FigureAnim, FigureBody, LoadedPart, PartSpecRef, QS_CENTRAL_MANIFEST,
    QS_LATERAL_MANIFEST, QsCentralManifest, QsLateralManifest,
    bird_medium::{
        self, BM_CENTRAL_MANIFEST, BM_LATERAL_MANIFEST, BmBone, BmBoneTransforms,
        BmCentralManifest, BmLateralManifest, BmPartSpecRef, LoadedBmPart,
    },
    humanoid::{
        self, FigureLoadout, FigureTool, FigureToolKinds, HUM_ARMOR_BACK_MANIFEST,
        HUM_ARMOR_BELT_MANIFEST, HUM_ARMOR_CHEST_MANIFEST, HUM_ARMOR_FOOT_MANIFEST,
        HUM_ARMOR_HAND_MANIFEST, HUM_ARMOR_PANTS_MANIFEST, HUM_ARMOR_SHOULDER_MANIFEST,
        HUM_COLOR_MANIFEST, HUM_HEAD_MANIFEST, HUM_LANTERN_MANIFEST, HUM_MAIN_WEAPON_MANIFEST,
        HumAnim, HumArmorBackSpec, HumArmorBeltSpec, HumArmorChestSpec, HumArmorFootSpec,
        HumArmorHandSpec, HumArmorPantsSpec, HumArmorShoulderSpec, HumBone, HumColorSpec,
        HumHeadSpec, HumLanternSpec, HumMainWeaponSpec, HumManifests, HumVoxRef, LoadedHumPart,
        WeaponKey,
    },
    quadruped_medium::{
        self, LoadedQmPart, QM_CENTRAL_MANIFEST, QM_LATERAL_MANIFEST, QmBone, QmBoneTransforms,
        QmCentralManifest, QmLateralManifest, QmPartSpecRef,
    },
};

/// Installs the figure asset loaders, the manifest handles, and the figure
/// systems (classify → build → animate). Added alongside `EntityViewPlugin`.
pub struct FigureViewPlugin;

impl Plugin for FigureViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<VoxAsset>()
            .init_asset::<QsCentralManifestAsset>()
            .init_asset::<QsLateralManifestAsset>()
            .init_asset::<QmCentralManifestAsset>()
            .init_asset::<QmLateralManifestAsset>()
            .init_asset::<BmCentralManifestAsset>()
            .init_asset::<BmLateralManifestAsset>()
            .init_asset::<HumColorManifestAsset>()
            .init_asset::<HumHeadManifestAsset>()
            .init_asset::<HumChestManifestAsset>()
            .init_asset::<HumBeltManifestAsset>()
            .init_asset::<HumPantsManifestAsset>()
            .init_asset::<HumFootManifestAsset>()
            .init_asset::<HumHandManifestAsset>()
            .init_asset::<HumShoulderManifestAsset>()
            .init_asset::<HumBackManifestAsset>()
            .init_asset::<HumWeaponManifestAsset>()
            .init_asset::<HumLanternManifestAsset>()
            .init_asset_loader::<VoxLoader>()
            .init_asset_loader::<QsCentralManifestLoader>()
            .init_asset_loader::<QsLateralManifestLoader>()
            .init_asset_loader::<QmCentralManifestLoader>()
            .init_asset_loader::<QmLateralManifestLoader>()
            .init_asset_loader::<BmCentralManifestLoader>()
            .init_asset_loader::<BmLateralManifestLoader>()
            .init_asset_loader::<HumColorManifestLoader>()
            .init_asset_loader::<HumHeadManifestLoader>()
            .init_asset_loader::<HumChestManifestLoader>()
            .init_asset_loader::<HumBeltManifestLoader>()
            .init_asset_loader::<HumPantsManifestLoader>()
            .init_asset_loader::<HumFootManifestLoader>()
            .init_asset_loader::<HumHandManifestLoader>()
            .init_asset_loader::<HumShoulderManifestLoader>()
            .init_asset_loader::<HumBackManifestLoader>()
            .init_asset_loader::<HumWeaponManifestLoader>()
            .init_asset_loader::<HumLanternManifestLoader>()
            .add_systems(
                Startup,
                (
                    load_figure_manifests,
                    load_quadruped_medium_manifests,
                    load_bird_medium_manifests,
                    load_humanoid_manifests,
                ),
            )
            .add_systems(
                Update,
                (
                    classify_bodies,
                    build_pending_figures,
                    build_pending_quadruped_mediums,
                    build_pending_bird_mediums,
                    build_pending_humanoids,
                    animate_quadruped_smalls,
                    animate_quadruped_mediums,
                    animate_bird_mediums,
                    animate_humanoids,
                )
                    .chain(),
            );
    }
}

// ---------------------------------------------------------------------------
// Assets + loaders
// ---------------------------------------------------------------------------

/// A parsed `.vox` model file (Bevy asset wrapper around `dot_vox`).
#[derive(Asset, TypePath)]
pub struct VoxAsset(pub DotVoxData);

/// Loads a `.vox` file into [`VoxAsset`] (claims the `vox` extension).
#[derive(Default, TypePath)]
struct VoxLoader;

impl AssetLoader for VoxLoader {
    type Asset = VoxAsset;
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
        let data = dot_vox::load_bytes(&bytes)
            .map_err(|e| BevyError::from(std::io::Error::other(e.to_string())))?;
        Ok(VoxAsset(data))
    }

    fn extensions(&self) -> &[&str] { &["vox"] }
}

/// Bevy asset wrapper for the quadruped-small CENTRAL figure manifest.
#[derive(Asset, TypePath)]
pub struct QsCentralManifestAsset(pub QsCentralManifest);

/// Bevy asset wrapper for the quadruped-small LATERAL figure manifest.
#[derive(Asset, TypePath)]
pub struct QsLateralManifestAsset(pub QsLateralManifest);

/// Loads the central manifest RON. Typed load disambiguates the plain `.ron`
/// extension by asset type (same trick the block-palette loader uses).
#[derive(Default, TypePath)]
struct QsCentralManifestLoader;

impl AssetLoader for QsCentralManifestLoader {
    type Asset = QsCentralManifestAsset;
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
        Ok(QsCentralManifestAsset(ron::de::from_bytes(&bytes)?))
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

/// Loads the lateral manifest RON.
#[derive(Default, TypePath)]
struct QsLateralManifestLoader;

impl AssetLoader for QsLateralManifestLoader {
    type Asset = QsLateralManifestAsset;
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
        Ok(QsLateralManifestAsset(ron::de::from_bytes(&bytes)?))
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

/// A fully-qualified Veloren dotted asset name (`voxygen.voxel.…`) → the
/// on-disk relative path Bevy's `AssetServer` resolves (`voxygen/voxel/…`).
/// Names are frozen (isolation law rule 3): we only translate the `.`
/// separator and append the extension.
fn asset_path(dotted: &str, ext: &str) -> String { format!("{}.{ext}", dotted.replace('.', "/")) }

/// The Veloren namespace figure `.vox` names in the manifests are relative to
/// (`graceful_load_vox` prepends it): the manifest `("npc.pig.male.head")`
/// really means `voxygen.voxel.npc.pig.male.head`. Prepend it before resolving.
fn vox_path(vox_name: &str) -> String { asset_path(&format!("voxygen.voxel.{vox_name}"), "vox") }

/// Strong handles to the loaded figure manifests (kept alive + polled).
#[derive(Resource)]
struct FigureManifests {
    central: Handle<QsCentralManifestAsset>,
    lateral: Handle<QsLateralManifestAsset>,
}

fn load_figure_manifests(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(FigureManifests {
        central: asset_server.load(asset_path(QS_CENTRAL_MANIFEST, "ron")),
        lateral: asset_server.load(asset_path(QS_LATERAL_MANIFEST, "ron")),
    });
}

// --- EM-3.8c: quadruped-medium + bird-medium manifest assets/loaders ---
//
// Reuse the same typed-`.ron`-by-asset-type trick the humanoid manifests use.
// One Asset + Loader per manifest wrapper type; the loader parses the RON.

macro_rules! ron_manifest_asset {
    ($asset:ident, $loader:ident, $inner:ty) => {
        #[derive(Asset, TypePath)]
        pub struct $asset(pub $inner);

        #[derive(Default, TypePath)]
        struct $loader;

        impl AssetLoader for $loader {
            type Asset = $asset;
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
                Ok($asset(ron::de::from_bytes(&bytes)?))
            }

            fn extensions(&self) -> &[&str] { &["ron"] }
        }
    };
}

ron_manifest_asset!(
    QmCentralManifestAsset,
    QmCentralManifestLoader,
    QmCentralManifest
);
ron_manifest_asset!(
    QmLateralManifestAsset,
    QmLateralManifestLoader,
    QmLateralManifest
);
ron_manifest_asset!(
    BmCentralManifestAsset,
    BmCentralManifestLoader,
    BmCentralManifest
);
ron_manifest_asset!(
    BmLateralManifestAsset,
    BmLateralManifestLoader,
    BmLateralManifest
);

/// Strong handles to the quadruped-medium manifests.
#[derive(Resource)]
struct QmFigureManifests {
    central: Handle<QmCentralManifestAsset>,
    lateral: Handle<QmLateralManifestAsset>,
}

fn load_quadruped_medium_manifests(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(QmFigureManifests {
        central: asset_server.load(asset_path(QM_CENTRAL_MANIFEST, "ron")),
        lateral: asset_server.load(asset_path(QM_LATERAL_MANIFEST, "ron")),
    });
}

/// Strong handles to the bird-medium manifests.
#[derive(Resource)]
struct BmFigureManifests {
    central: Handle<BmCentralManifestAsset>,
    lateral: Handle<BmLateralManifestAsset>,
}

fn load_bird_medium_manifests(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(BmFigureManifests {
        central: asset_server.load(asset_path(BM_CENTRAL_MANIFEST, "ron")),
        lateral: asset_server.load(asset_path(BM_LATERAL_MANIFEST, "ron")),
    });
}

// ---------------------------------------------------------------------------
// Classify → load → build
// ---------------------------------------------------------------------------

/// Resolves a replicated [`NetBody`] to the render crate's [`FigureBody`]. This
/// is the ONE place the client maps a sim body to a figure; everything past it
/// is engine-only.
fn resolve_figure_body(body: &NetBody) -> FigureBody {
    match body.0 {
        common::comp::Body::QuadrupedSmall(b) => FigureBody::QuadrupedSmall {
            species: b.species,
            body_type: b.body_type,
        },
        common::comp::Body::QuadrupedMedium(b) => FigureBody::QuadrupedMedium {
            species: b.species,
            body_type: b.body_type,
        },
        common::comp::Body::BirdMedium(b) => FigureBody::BirdMedium {
            species: b.species,
            body_type: b.body_type,
        },
        common::comp::Body::Humanoid(b) => FigureBody::Humanoid(b),
        _ => FigureBody::Unsupported,
    }
}

/// An entity whose real figure is still loading its `.vox` parts.
#[derive(Component)]
struct PendingFigure {
    parts: Vec<PendingPart>,
    body: FigureBody,
}

struct PendingPart {
    spec: PartSpecRef,
    handle: Handle<VoxAsset>,
}

/// A quadruped-medium entity whose real figure is still loading its parts.
#[derive(Component)]
struct PendingQuadrupedMedium {
    parts: Vec<PendingQmPart>,
    species: common::comp::quadruped_medium::Species,
    body_type: common::comp::quadruped_medium::BodyType,
}

struct PendingQmPart {
    spec: QmPartSpecRef,
    handle: Handle<VoxAsset>,
}

/// A bird-medium entity whose real figure is still loading its parts.
#[derive(Component)]
struct PendingBirdMedium {
    parts: Vec<PendingBmPart>,
    species: common::comp::bird_medium::Species,
    body_type: common::comp::bird_medium::BodyType,
}

struct PendingBmPart {
    spec: BmPartSpecRef,
    handle: Handle<VoxAsset>,
}

/// Marks an entity that has been finalised: it has a real figure OR is a
/// deliberately-kept capsule (unsupported body). Neither figure nor capsule
/// path re-processes it.
#[derive(Component)]
pub struct FigureBuilt;

/// Per-figure locomotion animation state shared by every animated body
/// (EM-3.8c). `acc` is the integrated distance (`speed * dt`) that drives the
/// foot/flap cycle phase continuously across speed changes (polish minor a);
/// `running` latches the idle↔run choice through a hysteresis band so an NPC
/// hovering near the threshold doesn't flicker (polish minor b).
#[derive(Component, Default)]
struct FigureAnimState {
    acc: f32,
    running: bool,
}

/// Idle→run enter / run→idle exit speeds (blocks/s). The gap is the hysteresis
/// band: below `EXIT` we idle, above `ENTER` we run, in-between we hold the
/// current state (polish minor b — replaces the old single 0.4 threshold).
const RUN_ENTER_SPEED: f32 = 0.5;
const RUN_EXIT_SPEED: f32 = 0.3;

/// Updates a figure's [`FigureAnimState`] from the current ground `speed` and
/// frame `dt`, returning whether it should animate as running. Advances the
/// phase accumulator and applies idle/run hysteresis.
fn step_anim_state(state: &mut FigureAnimState, speed: f32, dt: f32) -> bool {
    // Wrap the phase accumulator so f32 resolution can't coarsen over long
    // sessions (acc in the 1e5–1e6 range → visible cycle stepping). 1024 is a
    // multiple of 2π larger than every anim's phase multiplier, so cycle
    // continuity is preserved across the wrap (reviewer minor, EM-3.8c).
    state.acc = (state.acc + speed * dt).rem_euclid(1024.0);
    if state.running {
        if speed < RUN_EXIT_SPEED {
            state.running = false;
        }
    } else if speed > RUN_ENTER_SPEED {
        state.running = true;
    }
    state.running
}

/// Horizontal ground speed (blocks/s) from a replicated [`NetVel`] (Bevy axes;
/// the ground plane is x/z, y is up).
fn ground_speed(vel: Option<&NetVel>) -> f32 {
    vel.map_or(0.0, |v| {
        let h = v.0;
        (h.x * h.x + h.z * h.z).sqrt()
    })
}

/// Every frame, for any mirrored entity not yet finalised: classify its body.
/// Unsupported → mark [`FigureBuilt`] (keeps its capsule). Supported → once the
/// manifests are parsed, start loading its `.vox` parts ([`PendingFigure`]).
/// Runs each frame (not just on `Added`) so it is robust to the manifests
/// finishing loading AFTER the first NPCs are mirrored.
#[allow(clippy::too_many_arguments)]
fn classify_bodies(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    manifests: Option<Res<FigureManifests>>,
    central_assets: Res<Assets<QsCentralManifestAsset>>,
    lateral_assets: Res<Assets<QsLateralManifestAsset>>,
    qm_manifests: Option<Res<QmFigureManifests>>,
    qm_central_assets: Res<Assets<QmCentralManifestAsset>>,
    qm_lateral_assets: Res<Assets<QmLateralManifestAsset>>,
    bm_manifests: Option<Res<BmFigureManifests>>,
    bm_central_assets: Res<Assets<BmCentralManifestAsset>>,
    bm_lateral_assets: Res<Assets<BmLateralManifestAsset>>,
    hum: Option<Res<HumanoidManifests>>,
    hum_assets: HumManifestAssets,
    query: Query<
        (Entity, &NetBody, Option<&NetLoadout>),
        (
            With<NetBody>,
            Without<FigureBuilt>,
            Without<PendingFigure>,
            Without<PendingQuadrupedMedium>,
            Without<PendingBirdMedium>,
            Without<PendingHumanoid>,
        ),
    >,
) {
    for (entity, body, net_loadout) in &query {
        let figure_body = resolve_figure_body(body);
        let species_body_type = match figure_body {
            FigureBody::QuadrupedSmall { species, body_type } => (species, body_type),
            FigureBody::QuadrupedMedium { species, body_type } => {
                // QM path (EM-3.8c): once both QM manifests are parsed, resolve
                // the part list and start loading each part's `.vox`.
                let Some(qm_manifests) = &qm_manifests else {
                    continue;
                };
                let (Some(central), Some(lateral)) = (
                    qm_central_assets.get(&qm_manifests.central),
                    qm_lateral_assets.get(&qm_manifests.lateral),
                ) else {
                    continue;
                };
                let Some(specs) = quadruped_medium::quadruped_medium_part_specs(
                    &central.0, &lateral.0, species, body_type,
                ) else {
                    commands.entity(entity).insert(FigureBuilt);
                    continue;
                };
                let parts: Vec<PendingQmPart> = specs
                    .into_iter()
                    .map(|spec| {
                        let handle = asset_server.load(vox_path(&spec.vox_name));
                        PendingQmPart { spec, handle }
                    })
                    .collect();
                commands.entity(entity).insert(PendingQuadrupedMedium {
                    parts,
                    species,
                    body_type,
                });
                continue;
            },
            FigureBody::BirdMedium { species, body_type } => {
                let Some(bm_manifests) = &bm_manifests else {
                    continue;
                };
                let (Some(central), Some(lateral)) = (
                    bm_central_assets.get(&bm_manifests.central),
                    bm_lateral_assets.get(&bm_manifests.lateral),
                ) else {
                    continue;
                };
                let Some(specs) =
                    bird_medium::bird_medium_part_specs(&central.0, &lateral.0, species, body_type)
                else {
                    commands.entity(entity).insert(FigureBuilt);
                    continue;
                };
                let parts: Vec<PendingBmPart> = specs
                    .into_iter()
                    .map(|spec| {
                        let handle = asset_server.load(vox_path(&spec.vox_name));
                        PendingBmPart { spec, handle }
                    })
                    .collect();
                commands.entity(entity).insert(PendingBirdMedium {
                    parts,
                    species,
                    body_type,
                });
                continue;
            },
            FigureBody::Humanoid(hum_body) => {
                // Humanoid path (EM-3.8b/d): once the humanoid manifests are all
                // parsed AND the entity's loadout has arrived, resolve the part
                // `.vox` list (real equipped gear) and start loading them.
                let (Some(hum), Some(manifests)) = (&hum, hum_assets.get()) else {
                    continue; // manifests still loading — keep the capsule, retry
                };
                let _ = hum; // handles kept alive by the resource
                // EM-3.8d: the mirror gives every humanoid a `NetLoadout` at
                // spawn; wait for it so we assemble the REAL gear (not a default
                // stand-in). An empty loadout is still `Some(default)` → default
                // clothing, so this only waits out the one-frame replication gap.
                let Some(net_loadout) = net_loadout else {
                    continue;
                };
                let loadout = figure_loadout_from_net(net_loadout);
                let Some(refs) = humanoid::humanoid_vox_refs(&manifests, &hum_body, &loadout)
                else {
                    // No head-manifest entry for this species: keep the capsule.
                    commands.entity(entity).insert(FigureBuilt);
                    continue;
                };
                let parts: Vec<PendingHumPart> = refs
                    .into_iter()
                    .map(|r| {
                        let handle = asset_server.load(hum_vox_path(&r.vox_name));
                        PendingHumPart { spec: r, handle }
                    })
                    .collect();
                commands.entity(entity).insert(PendingHumanoid {
                    parts,
                    body: hum_body,
                    loadout,
                });
                continue;
            },
            FigureBody::Unsupported => {
                // Unsupported body: keep the capsule, don't reconsider it.
                commands.entity(entity).insert(FigureBuilt);
                continue;
            },
        };
        let (species, body_type) = species_body_type;

        // Supported body but the manifests aren't parsed yet: leave it (capsule
        // stays visible); we retry next frame.
        let Some(manifests) = &manifests else {
            continue;
        };
        let (Some(central), Some(lateral)) = (
            central_assets.get(&manifests.central),
            lateral_assets.get(&manifests.lateral),
        ) else {
            continue;
        };

        let Some(specs) =
            figure::quadruped_small_part_specs(&central.0, &lateral.0, species, body_type)
        else {
            // No manifest entry for this species: keep the capsule permanently.
            commands.entity(entity).insert(FigureBuilt);
            continue;
        };

        let parts: Vec<PendingPart> = specs
            .into_iter()
            .map(|spec| {
                let handle = asset_server.load(vox_path(&spec.vox_name));
                PendingPart { spec, handle }
            })
            .collect();
        commands.entity(entity).insert(PendingFigure {
            parts,
            body: figure_body,
        });
    }
}

/// Once every `.vox` handle of a [`PendingFigure`] has loaded, mesh the parts,
/// REPLACE the placeholder capsule (remove its `Mesh3d`/`MeshMaterial3d`) with
/// one child entity per part at its rest-pose bone transform, and mark
/// [`FigureBuilt`]. On a load failure, keep the capsule and stop retrying.
fn build_pending_figures(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vox_assets: Res<Assets<VoxAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pending: Query<(Entity, &PendingFigure)>,
) {
    for (entity, figure) in &pending {
        // Per-part fallback (polish minor c): an OPTIONAL part that hard-fails
        // is dropped and the figure still builds; an ESSENTIAL part that fails
        // keeps the capsule. We wait until every part is either loaded or
        // finished (failed).
        let ready = poll_parts(
            &asset_server,
            figure
                .parts
                .iter()
                .map(|p| (&p.handle, qs_part_essential(p.spec.bone))),
        );
        match ready {
            PartsReady::Waiting => continue,
            PartsReady::EssentialFailed => {
                warn!("figure: an essential .vox part failed to load; keeping the capsule");
                commands
                    .entity(entity)
                    .remove::<PendingFigure>()
                    .insert(FigureBuilt);
                continue;
            },
            PartsReady::Ready => {},
        }

        let FigureBody::QuadrupedSmall { species, body_type } = figure.body else {
            commands
                .entity(entity)
                .remove::<PendingFigure>()
                .insert(FigureBuilt);
            continue;
        };
        let rest = figure::quadruped_small_bone_rest(species, body_type);

        // Skip any optional part whose `.vox` failed (its handle isn't Loaded).
        let loaded: Vec<LoadedPart> = figure
            .parts
            .iter()
            .filter_map(|part| {
                vox_assets.get(&part.handle).map(|vox| LoadedPart {
                    vox: &vox.0,
                    model_index: part.spec.model_index,
                    offset: part.spec.offset,
                    flipped: part.spec.flipped,
                    bone: part.spec.bone,
                })
            })
            .collect();
        let assembled = figure::assemble_with_bones(&loaded, &rest);

        let material = figure_material(&mut materials);

        let mut part_entities: Vec<(figure::FigureBoneName, Entity)> =
            Vec::with_capacity(assembled.len());
        let mut ec = commands.entity(entity);
        // Drop the placeholder capsule geometry; keep the (interpolated) root
        // Transform + Visibility so the figure moves with the entity.
        ec.remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<PendingFigure>()
            .insert(FigureBuilt);
        ec.with_children(|root| {
            for (bone, part) in assembled {
                let child = root
                    .spawn((
                        Mesh3d(meshes.add(part.mesh)),
                        MeshMaterial3d(material.clone()),
                        part.transform,
                        Name::new(part.name),
                    ))
                    .id();
                part_entities.push((bone, child));
            }
        });
        ec.insert((
            QuadrupedSmallFigure {
                species,
                body_type,
                parts: part_entities,
            },
            FigureAnimState::default(),
        ));

        info!("figure: assembled a real .vox model for a quadruped-small NPC");
    }
}

/// A finalised quadruped-small figure (EM-3.8c: now animated). Keeps the
/// species/body-type + the bone→child map so [`animate_figures`] updates each
/// part's `Transform` per frame.
#[derive(Component)]
struct QuadrupedSmallFigure {
    species: common::comp::quadruped_small::Species,
    body_type: common::comp::quadruped_small::BodyType,
    parts: Vec<(figure::FigureBoneName, Entity)>,
}

/// A finalised quadruped-medium figure.
#[derive(Component)]
struct QuadrupedMediumFigure {
    species: common::comp::quadruped_medium::Species,
    body_type: common::comp::quadruped_medium::BodyType,
    parts: Vec<(QmBone, Entity)>,
}

/// A finalised bird-medium figure.
#[derive(Component)]
struct BirdMediumFigure {
    species: common::comp::bird_medium::Species,
    body_type: common::comp::bird_medium::BodyType,
    parts: Vec<(BmBone, Entity)>,
}

/// Is a quadruped-small bone essential? Everything but the tail is core; a
/// missing tail still gives a recognisable animal (polish minor c).
fn qs_part_essential(bone: figure::FigureBoneName) -> bool {
    !matches!(bone, figure::FigureBoneName::Tail)
}

/// Is a quadruped-medium bone essential? Jaw/ears/tail are cosmetic extras.
fn qm_part_essential(bone: QmBone) -> bool {
    !matches!(bone, QmBone::Jaw | QmBone::Ears | QmBone::Tail)
}

/// Is a bird-medium bone essential? The tail is the only skippable part.
fn bm_part_essential(bone: BmBone) -> bool { !matches!(bone, BmBone::Tail) }

/// Outcome of polling a figure's parts (per-part fallback, polish minor c).
enum PartsReady {
    /// Some part is still loading — retry next frame.
    Waiting,
    /// An ESSENTIAL part hard-failed — the figure can't build.
    EssentialFailed,
    /// Every part is loaded, or the only failures were OPTIONAL parts.
    Ready,
}

/// Polls `(handle, essential)` pairs: `Waiting` if any is still in flight,
/// `EssentialFailed` if an essential part failed, else `Ready` (optional
/// failures are tolerated and simply skipped at assembly).
fn poll_parts<'a>(
    asset_server: &AssetServer,
    parts: impl Iterator<Item = (&'a Handle<VoxAsset>, bool)>,
) -> PartsReady {
    let mut essential_failed = false;
    for (handle, essential) in parts {
        match asset_server.get_load_state(handle) {
            Some(LoadState::Loaded) => {},
            Some(LoadState::Failed(_)) => {
                if essential {
                    essential_failed = true;
                }
            },
            _ => return PartsReady::Waiting,
        }
    }
    if essential_failed {
        PartsReady::EssentialFailed
    } else {
        PartsReady::Ready
    }
}

/// The shared matte material for vertex-coloured figure parts (bind-group
/// reuse; base_color WHITE so per-voxel vertex colour shows through).
fn figure_material(materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.85,
        ..default()
    })
}

/// Once every part of a [`PendingQuadrupedMedium`] resolves (per-part
/// fallback), assemble + place the QM figure and attach
/// [`QuadrupedMediumFigure`].
fn build_pending_quadruped_mediums(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vox_assets: Res<Assets<VoxAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pending: Query<(Entity, &PendingQuadrupedMedium)>,
) {
    for (entity, figure) in &pending {
        let ready = poll_parts(
            &asset_server,
            figure
                .parts
                .iter()
                .map(|p| (&p.handle, qm_part_essential(p.spec.bone))),
        );
        match ready {
            PartsReady::Waiting => continue,
            PartsReady::EssentialFailed => {
                warn!("qm figure: an essential .vox part failed; keeping the capsule");
                commands
                    .entity(entity)
                    .remove::<PendingQuadrupedMedium>()
                    .insert(FigureBuilt);
                continue;
            },
            PartsReady::Ready => {},
        }

        let rest: QmBoneTransforms =
            quadruped_medium::quadruped_medium_bone_rest(figure.species, figure.body_type);
        let loaded: Vec<LoadedQmPart> = figure
            .parts
            .iter()
            .filter_map(|part| {
                vox_assets.get(&part.handle).map(|vox| LoadedQmPart {
                    vox: &vox.0,
                    model_index: part.spec.model_index,
                    offset: part.spec.offset,
                    flipped: part.spec.flipped,
                    bone: part.spec.bone,
                })
            })
            .collect();
        let assembled = quadruped_medium::assemble(&loaded, &rest);
        let material = figure_material(&mut materials);

        let mut part_entities: Vec<(QmBone, Entity)> = Vec::with_capacity(assembled.len());
        let mut ec = commands.entity(entity);
        ec.remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<PendingQuadrupedMedium>()
            .insert(FigureBuilt);
        ec.with_children(|root| {
            for (bone, part) in assembled {
                let child = root
                    .spawn((
                        Mesh3d(meshes.add(part.mesh)),
                        MeshMaterial3d(material.clone()),
                        part.transform,
                        Name::new(part.name),
                    ))
                    .id();
                part_entities.push((bone, child));
            }
        });
        ec.insert((
            QuadrupedMediumFigure {
                species: figure.species,
                body_type: figure.body_type,
                parts: part_entities,
            },
            FigureAnimState::default(),
        ));

        info!("figure: assembled a real .vox model for a quadruped-medium NPC");
    }
}

/// Once every part of a [`PendingBirdMedium`] resolves, assemble + place the
/// bird figure and attach [`BirdMediumFigure`].
fn build_pending_bird_mediums(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vox_assets: Res<Assets<VoxAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pending: Query<(Entity, &PendingBirdMedium)>,
) {
    for (entity, figure) in &pending {
        let ready = poll_parts(
            &asset_server,
            figure
                .parts
                .iter()
                .map(|p| (&p.handle, bm_part_essential(p.spec.bone))),
        );
        match ready {
            PartsReady::Waiting => continue,
            PartsReady::EssentialFailed => {
                warn!("bird figure: an essential .vox part failed; keeping the capsule");
                commands
                    .entity(entity)
                    .remove::<PendingBirdMedium>()
                    .insert(FigureBuilt);
                continue;
            },
            PartsReady::Ready => {},
        }

        let rest: BmBoneTransforms =
            bird_medium::bird_medium_bone_rest(figure.species, figure.body_type);
        let loaded: Vec<LoadedBmPart> = figure
            .parts
            .iter()
            .filter_map(|part| {
                vox_assets.get(&part.handle).map(|vox| LoadedBmPart {
                    vox: &vox.0,
                    model_index: part.spec.model_index,
                    offset: part.spec.offset,
                    flipped: part.spec.flipped,
                    bone: part.spec.bone,
                })
            })
            .collect();
        let assembled = bird_medium::assemble(&loaded, &rest);
        let material = figure_material(&mut materials);

        let mut part_entities: Vec<(BmBone, Entity)> = Vec::with_capacity(assembled.len());
        let mut ec = commands.entity(entity);
        ec.remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<PendingBirdMedium>()
            .insert(FigureBuilt);
        ec.with_children(|root| {
            for (bone, part) in assembled {
                let child = root
                    .spawn((
                        Mesh3d(meshes.add(part.mesh)),
                        MeshMaterial3d(material.clone()),
                        part.transform,
                        Name::new(part.name),
                    ))
                    .id();
                part_entities.push((bone, child));
            }
        });
        ec.insert((
            BirdMediumFigure {
                species: figure.species,
                body_type: figure.body_type,
                parts: part_entities,
            },
            FigureAnimState::default(),
        ));

        info!("figure: assembled a real .vox model for a bird-medium NPC");
    }
}

// ===========================================================================
// EM-3.8b — Humanoid figures (the player + humanoid NPCs) + skeletal animation
// ===========================================================================
//
// The quadruped path above loads two manifests and raw `.vox` files. The
// humanoid path is heavier: EIGHT manifests (colour + head + six armour slots)
// plus the recolour/assembly in `xindeler-render-voxel::figure::humanoid`, and
// a per-frame animation system that drives the child part transforms from the
// entity's replicated velocity (idle when still, run when moving).

/// A Veloren dotted humanoid `.vox` name (relative to `voxygen.voxel`, e.g.
/// `figure.head.human.male`) → the on-disk path Bevy resolves. Same freeze rule
/// as [`vox_path`]: only the `.` separator + extension are translated.
fn hum_vox_path(vox_name: &str) -> String {
    asset_path(&format!("{}.{vox_name}", humanoid::VOX_NAMESPACE), "vox")
}

// --- Manifest assets + loaders (one Asset+Loader per manifest type) ---
//
// bevy's typed-asset load disambiguates the shared `.ron` extension BY ASSET
// TYPE (each loader claims `ron`; the typed `asset_server.load::<T>()` picks
// the matching loader). Same trick the quadruped manifests + block palette use.

/// Generates a `#[derive(Asset)]` newtype wrapper + an `AssetLoader` that
/// parses the RON into it, for one humanoid manifest type.
macro_rules! hum_manifest_asset {
    ($asset:ident, $loader:ident, $inner:ty) => {
        #[derive(Asset, TypePath)]
        pub struct $asset(pub $inner);

        #[derive(Default, TypePath)]
        struct $loader;

        impl AssetLoader for $loader {
            type Asset = $asset;
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
                Ok($asset(ron::de::from_bytes(&bytes)?))
            }

            fn extensions(&self) -> &[&str] { &["ron"] }
        }
    };
}

hum_manifest_asset!(HumColorManifestAsset, HumColorManifestLoader, HumColorSpec);
hum_manifest_asset!(HumHeadManifestAsset, HumHeadManifestLoader, HumHeadSpec);
hum_manifest_asset!(
    HumChestManifestAsset,
    HumChestManifestLoader,
    HumArmorChestSpec
);
hum_manifest_asset!(
    HumBeltManifestAsset,
    HumBeltManifestLoader,
    HumArmorBeltSpec
);
hum_manifest_asset!(
    HumPantsManifestAsset,
    HumPantsManifestLoader,
    HumArmorPantsSpec
);
hum_manifest_asset!(
    HumFootManifestAsset,
    HumFootManifestLoader,
    HumArmorFootSpec
);
hum_manifest_asset!(
    HumHandManifestAsset,
    HumHandManifestLoader,
    HumArmorHandSpec
);
hum_manifest_asset!(
    HumShoulderManifestAsset,
    HumShoulderManifestLoader,
    HumArmorShoulderSpec
);
hum_manifest_asset!(
    HumBackManifestAsset,
    HumBackManifestLoader,
    HumArmorBackSpec
);
hum_manifest_asset!(
    HumWeaponManifestAsset,
    HumWeaponManifestLoader,
    HumMainWeaponSpec
);
hum_manifest_asset!(
    HumLanternManifestAsset,
    HumLanternManifestLoader,
    HumLanternSpec
);

/// Strong handles to the eight parsed humanoid manifests (kept alive + polled).
#[derive(Resource)]
struct HumanoidManifests {
    color: Handle<HumColorManifestAsset>,
    head: Handle<HumHeadManifestAsset>,
    chest: Handle<HumChestManifestAsset>,
    belt: Handle<HumBeltManifestAsset>,
    pants: Handle<HumPantsManifestAsset>,
    foot: Handle<HumFootManifestAsset>,
    hand: Handle<HumHandManifestAsset>,
    shoulder: Handle<HumShoulderManifestAsset>,
    back: Handle<HumBackManifestAsset>,
    main_weapon: Handle<HumWeaponManifestAsset>,
    lantern: Handle<HumLanternManifestAsset>,
}

fn load_humanoid_manifests(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(HumanoidManifests {
        color: asset_server.load(asset_path(HUM_COLOR_MANIFEST, "ron")),
        head: asset_server.load(asset_path(HUM_HEAD_MANIFEST, "ron")),
        chest: asset_server.load(asset_path(HUM_ARMOR_CHEST_MANIFEST, "ron")),
        belt: asset_server.load(asset_path(HUM_ARMOR_BELT_MANIFEST, "ron")),
        pants: asset_server.load(asset_path(HUM_ARMOR_PANTS_MANIFEST, "ron")),
        foot: asset_server.load(asset_path(HUM_ARMOR_FOOT_MANIFEST, "ron")),
        hand: asset_server.load(asset_path(HUM_ARMOR_HAND_MANIFEST, "ron")),
        shoulder: asset_server.load(asset_path(HUM_ARMOR_SHOULDER_MANIFEST, "ron")),
        back: asset_server.load(asset_path(HUM_ARMOR_BACK_MANIFEST, "ron")),
        main_weapon: asset_server.load(asset_path(HUM_MAIN_WEAPON_MANIFEST, "ron")),
        lantern: asset_server.load(asset_path(HUM_LANTERN_MANIFEST, "ron")),
    });
}

/// A `SystemParam` bundling the eight humanoid manifest asset stores so the
/// classify system can, in one call, check they're all parsed and borrow a
/// [`HumManifests`] view built from them.
#[derive(bevy::ecs::system::SystemParam)]
struct HumManifestAssets<'w> {
    manifests: Option<Res<'w, HumanoidManifests>>,
    color: Res<'w, Assets<HumColorManifestAsset>>,
    head: Res<'w, Assets<HumHeadManifestAsset>>,
    chest: Res<'w, Assets<HumChestManifestAsset>>,
    belt: Res<'w, Assets<HumBeltManifestAsset>>,
    pants: Res<'w, Assets<HumPantsManifestAsset>>,
    foot: Res<'w, Assets<HumFootManifestAsset>>,
    hand: Res<'w, Assets<HumHandManifestAsset>>,
    shoulder: Res<'w, Assets<HumShoulderManifestAsset>>,
    back: Res<'w, Assets<HumBackManifestAsset>>,
    main_weapon: Res<'w, Assets<HumWeaponManifestAsset>>,
    lantern: Res<'w, Assets<HumLanternManifestAsset>>,
}
impl<'w> HumManifestAssets<'w> {
    /// Build a borrowed [`HumManifests`] once EVERY manifest has parsed; `None`
    /// while any is still loading (the entity keeps its capsule and retries
    /// next frame). The returned bundle borrows the asset stores (`&self`),
    /// so it is used only within the calling system — no cloning of the
    /// non-`Clone` colour spec.
    fn get(&self) -> Option<HumManifests<'_>> {
        let m = self.manifests.as_ref()?;
        Some(HumManifests {
            color: &self.color.get(&m.color)?.0,
            head: &self.head.get(&m.head)?.0,
            chest: &self.chest.get(&m.chest)?.0,
            belt: &self.belt.get(&m.belt)?.0,
            pants: &self.pants.get(&m.pants)?.0,
            foot: &self.foot.get(&m.foot)?.0,
            hand: &self.hand.get(&m.hand)?.0,
            shoulder: &self.shoulder.get(&m.shoulder)?.0,
            back: &self.back.get(&m.back)?.0,
            main_weapon: &self.main_weapon.get(&m.main_weapon)?.0,
            lantern: &self.lantern.get(&m.lantern)?.0,
        })
    }
}

/// Translates a replicated [`NetLoadout`] into the render crate's native
/// [`FigureLoadout`] (EM-3.8d) — the client's ONE place that maps protocol data
/// to figure input, keeping `xindeler-render-voxel` protocol-free. Armour keys
/// pass straight through; the tool key's `NetToolKey` becomes the manifest
/// [`WeaponKey`].
fn figure_loadout_from_net(net: &NetLoadout) -> FigureLoadout {
    FigureLoadout {
        active_tool: net.active_tool.as_ref().map(figure_tool_from_net),
        second_tool: net.second_tool.as_ref().map(figure_tool_from_net),
        chest: net.chest.clone(),
        belt: net.belt.clone(),
        back: net.back.clone(),
        pants: net.pants.clone(),
        shoulder: net.shoulder.clone(),
        hand: net.hand.clone(),
        foot: net.foot.clone(),
        lantern: net.lantern.clone(),
    }
}

/// Maps a replicated [`NetTool`] to a render-crate [`FigureTool`].
fn figure_tool_from_net(tool: &NetTool) -> FigureTool {
    let key = match &tool.key {
        NetToolKey::Tool(id) => WeaponKey::Tool(id.clone()),
        NetToolKey::Modular {
            primary,
            secondary,
            hands,
        } => WeaponKey::Modular((primary.clone(), secondary.clone(), *hands)),
    };
    FigureTool {
        key,
        kind: tool.kind,
        hands: tool.hands,
    }
}

/// A humanoid entity whose real figure is still loading its `.vox` parts.
#[derive(Component)]
struct PendingHumanoid {
    parts: Vec<PendingHumPart>,
    body: common::comp::humanoid::Body,
    /// The resolved equipped gear (EM-3.8d) — drives the weapon sheathe pose at
    /// assembly + is distilled to [`FigureToolKinds`] for per-frame animation.
    loadout: FigureLoadout,
}

struct PendingHumPart {
    spec: HumVoxRef,
    handle: Handle<VoxAsset>,
}

/// A finalised humanoid figure: keeps the `Body` (for per-frame animation) and
/// the bone→child-entity map so [`animate_humanoids`] can update each part's
/// `Transform` every frame. Present ⇒ the entity is also [`FigureBuilt`].
/// `pub` so the smoke figure cam can prefer framing a humanoid (the richest
/// figure — head recolour + clothing + weapon) when one is present.
#[derive(Component)]
pub struct HumanoidFigure {
    body: common::comp::humanoid::Body,
    /// One (bone, child-entity) pair per assembled part.
    parts: Vec<(HumBone, Entity)>,
    /// The equipped tool kinds/hands (EM-3.8d) so per-frame animation sheathes
    /// the real weapon(s) on the back with the right pose.
    tools: FigureToolKinds,
}

/// Once every `.vox` handle of a [`PendingHumanoid`] has loaded, RECOLOUR +
/// assemble the parts (in `xindeler-render-voxel::figure::humanoid`), replace
/// the placeholder capsule with one child entity per part at its REST-pose bone
/// transform, and attach [`HumanoidFigure`] (+ [`FigureBuilt`]). On a load
/// failure, keep the capsule and stop retrying.
fn build_pending_humanoids(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    vox_assets: Res<Assets<VoxAsset>>,
    hum_assets: HumManifestAssets,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    pending: Query<(Entity, &PendingHumanoid)>,
) {
    // The humanoid manifests must still be resident (they are — the resource
    // keeps strong handles); if not, retry next frame.
    let Some(manifests) = hum_assets.get() else {
        return;
    };

    for (entity, figure) in &pending {
        // Per-part fallback (polish minor c): a missing accessory/weapon/eye
        // etc. is dropped; a missing core body part keeps the capsule.
        let ready = poll_parts(
            &asset_server,
            figure
                .parts
                .iter()
                .map(|p| (&p.handle, hum_role_essential(&p.spec.role))),
        );
        match ready {
            PartsReady::Waiting => continue,
            PartsReady::EssentialFailed => {
                warn!("humanoid figure: an essential .vox part failed; keeping the capsule");
                commands
                    .entity(entity)
                    .remove::<PendingHumanoid>()
                    .insert(FigureBuilt);
                continue;
            },
            PartsReady::Ready => {},
        }

        // Pair each loaded `.vox` back with its role, then recolour + assemble.
        // A failed OPTIONAL part isn't in the store, so `filter_map` drops it.
        let loaded: Vec<LoadedHumPart> = figure
            .parts
            .iter()
            .filter_map(|part| {
                vox_assets.get(&part.handle).map(|vox| LoadedHumPart {
                    role: part.spec.role.clone(),
                    vox: &vox.0,
                    model_index: part.spec.model_index,
                })
            })
            .collect();
        let assembled = humanoid::assemble_humanoid(&figure.body, &manifests, &loaded);
        // Rest pose (idle at t=0) for the initial placement, with the REAL
        // equipped tools so the weapon sheathes correctly from frame 0;
        // animation updates it every frame (`animate_humanoids`).
        let tools = figure.loadout.tool_kinds();
        let rest = humanoid::humanoid_bone_rest(&figure.body, tools);

        let material = figure_material(&mut materials);

        let mut part_entities: Vec<(HumBone, Entity)> = Vec::with_capacity(assembled.len());
        let mut ec = commands.entity(entity);
        ec.remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<PendingHumanoid>()
            .insert(FigureBuilt);
        ec.with_children(|root| {
            for part in assembled {
                let bone = part.bone;
                let child = root
                    .spawn((
                        Mesh3d(meshes.add(part.mesh)),
                        MeshMaterial3d(material.clone()),
                        rest.get(bone),
                        Name::new(part.name),
                    ))
                    .id();
                part_entities.push((bone, child));
            }
        });
        ec.insert((
            HumanoidFigure {
                body: figure.body,
                parts: part_entities,
                tools,
            },
            FigureAnimState::default(),
        ));

        info!("humanoid figure: assembled a real .vox character (equipped gear from the sim)");
    }
}

/// Is a humanoid part role essential? The bare head + torso + limbs (chest,
/// pants, hands, feet) are core; hair/beard/eyes/accessory/belt/shoulders and
/// the test weapon are cosmetic and may be dropped (polish minor c).
fn hum_role_essential(role: &humanoid::HumVoxRole) -> bool {
    use humanoid::HumVoxRole::*;
    match role {
        HeadBare { .. } => true,
        // Chest + legs (the naked-torso/legs base) are core; belt/back/lantern
        // are optional body-slot parts.
        Body { bone, .. } => matches!(bone, HumBone::Chest | HumBone::Shorts),
        Sided { bone, .. } => matches!(
            bone,
            HumBone::HandL | HumBone::HandR | HumBone::FootL | HumBone::FootR
        ),
        // Eyes, hair, beard, accessory, belt, back, lantern, shoulders, weapon —
        // all optional (a missing one just drops that part).
        _ => false,
    }
}

// ===========================================================================
// EM-3.8c — per-frame animation for every assembled figure
// ===========================================================================
//
// One (chained) system per animated body. They're split rather than merged
// because they all mutably borrow `FigureAnimState`: Bevy proves two queries in
// ONE system disjoint only via `With`/`Without` filters, and it can't see that
// the four figure-marker components are mutually exclusive — so a single system
// holding four `&mut FigureAnimState` queries panics (B0001). Separate systems
// each hold one figure query + their own `Query<&mut Transform>` (the figure
// query doesn't touch `Transform`, so no intra-system conflict).
//
// Each figure carries a `FigureAnimState` (phase accumulator + hysteresis
// latch) advanced by `speed * dt`, so the run cycle stays phase-continuous
// across speed changes (minor a) and idle↔run doesn't flicker (minor b).
//
// The whole figure already faces its heading (the entity root carries the
// interpolated orientation), so we drive the animations with a generic forward
// velocity of the right MAGNITUDE (`|NetVel|`), never its world direction.

/// Writes the given bone transforms onto each part child.
fn apply_bones<B: Copy>(
    parts: &[(B, Entity)],
    transforms: &mut Query<&mut Transform>,
    get: impl Fn(B) -> Transform,
) {
    for (bone, child) in parts {
        if let Ok(mut tf) = transforms.get_mut(*child) {
            *tf = get(*bone);
        }
    }
}

/// Animate quadruped-small figures (idle/run).
fn animate_quadruped_smalls(
    time: Res<Time>,
    mut figures: Query<(&QuadrupedSmallFigure, &mut FigureAnimState, Option<&NetVel>)>,
    mut transforms: Query<&mut Transform>,
) {
    let (t, dt) = (time.elapsed_secs(), time.delta_secs());
    for (figure, mut state, vel) in &mut figures {
        let speed = ground_speed(vel);
        let anim = if step_anim_state(&mut state, speed, dt) {
            FigureAnim::Run
        } else {
            FigureAnim::Idle
        };
        let bones = figure::quadruped_small_bone_transforms(
            figure.species,
            figure.body_type,
            anim,
            state.acc,
            t,
            speed,
        );
        apply_bones(&figure.parts, &mut transforms, |b| bones.get(b));
    }
}

/// Animate quadruped-medium figures (idle/run).
fn animate_quadruped_mediums(
    time: Res<Time>,
    mut figures: Query<(
        &QuadrupedMediumFigure,
        &mut FigureAnimState,
        Option<&NetVel>,
    )>,
    mut transforms: Query<&mut Transform>,
) {
    let (t, dt) = (time.elapsed_secs(), time.delta_secs());
    for (figure, mut state, vel) in &mut figures {
        let speed = ground_speed(vel);
        let anim = if step_anim_state(&mut state, speed, dt) {
            FigureAnim::Run
        } else {
            FigureAnim::Idle
        };
        let bones = quadruped_medium::quadruped_medium_bone_transforms(
            figure.species,
            figure.body_type,
            anim,
            state.acc,
            t,
            speed,
        );
        apply_bones(&figure.parts, &mut transforms, |b| bones.get(b));
    }
}

/// Animate bird-medium figures (idle/run/fly). A meaningful vertical velocity
/// component → flight; grounded motion → run.
fn animate_bird_mediums(
    time: Res<Time>,
    mut figures: Query<(&BirdMediumFigure, &mut FigureAnimState, Option<&NetVel>)>,
    mut transforms: Query<&mut Transform>,
) {
    let (t, dt) = (time.elapsed_secs(), time.delta_secs());
    for (figure, mut state, vel) in &mut figures {
        let horiz = ground_speed(vel);
        let vert = vel.map_or(0.0, |v| v.0.y.abs());
        let running = step_anim_state(&mut state, horiz.max(vert), dt);
        let anim = if vert > 1.0 {
            FigureAnim::Fly
        } else if running {
            FigureAnim::Run
        } else {
            FigureAnim::Idle
        };
        let bones = bird_medium::bird_medium_bone_transforms(
            figure.species,
            figure.body_type,
            anim,
            state.acc,
            t,
            horiz,
        );
        apply_bones(&figure.parts, &mut transforms, |b| bones.get(b));
    }
}

/// Animate humanoid figures (idle/run).
fn animate_humanoids(
    time: Res<Time>,
    mut figures: Query<(&HumanoidFigure, &mut FigureAnimState, Option<&NetVel>)>,
    mut transforms: Query<&mut Transform>,
) {
    let (t, dt) = (time.elapsed_secs(), time.delta_secs());
    for (figure, mut state, vel) in &mut figures {
        let speed = ground_speed(vel);
        let anim = if step_anim_state(&mut state, speed, dt) {
            HumAnim::Run
        } else {
            HumAnim::Idle
        };
        let bones = humanoid::humanoid_bone_transforms(
            &figure.body,
            anim,
            state.acc,
            t,
            speed,
            figure.tools,
        );
        apply_bones(&figure.parts, &mut transforms, |b| bones.get(b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The idle/run hysteresis (polish minor b): entering run needs speed >
    /// `RUN_ENTER_SPEED`, exiting needs speed < `RUN_EXIT_SPEED`; in the band
    /// the state is held — no flicker.
    #[test]
    fn hysteresis_holds_state_in_the_band() {
        let mut s = FigureAnimState::default();
        // Below exit → idle.
        assert!(!step_anim_state(&mut s, 0.1, 0.016));
        // In the band (between exit and enter) → stays idle.
        assert!(!step_anim_state(&mut s, 0.4, 0.016));
        // Above enter → run.
        assert!(step_anim_state(&mut s, 0.9, 0.016));
        // Back into the band → stays running (hysteresis, no flicker).
        assert!(step_anim_state(&mut s, 0.4, 0.016));
        // Below exit → idle again.
        assert!(!step_anim_state(&mut s, 0.2, 0.016));
    }

    /// The phase accumulator (polish minor a) integrates `speed * dt`, so it
    /// advances even when the frame time is constant and reflects speed changes
    /// continuously.
    #[test]
    fn acc_integrates_speed_over_time() {
        let mut s = FigureAnimState::default();
        step_anim_state(&mut s, 2.0, 0.5); // +1.0
        step_anim_state(&mut s, 4.0, 0.25); // +1.0
        assert!(
            (s.acc - 2.0).abs() < 1e-5,
            "acc must integrate speed*dt, got {}",
            s.acc
        );
    }

    /// Ground speed ignores the vertical (y) component — it's the horizontal
    /// locomotion magnitude.
    #[test]
    fn ground_speed_is_horizontal() {
        let vel = NetVel(Vec3::new(3.0, 100.0, 4.0));
        assert!((ground_speed(Some(&vel)) - 5.0).abs() < 1e-5);
        assert_eq!(ground_speed(None), 0.0);
    }
}
