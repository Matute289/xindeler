//! EM-3.8 / EM-3.8b — real `.vox` figures for mirrored entities (listen-server
//! only).
//!
//! Replaces EM-3.7's placeholder capsules with the actual Veloren voxel models.
//! The heavy lifting (meshing, recolour, bone placement, animation) lives in
//! `xindeler-render-voxel::figure`; THIS module is the client-side asset glue.
//!
//! ## Two body paths
//! - **quadruped-small** (EM-3.8): two manifests + raw `.vox` parts → static
//!   rest-pose figure (the sim's test-NPC Pig).
//! - **humanoid** (EM-3.8b): eight manifests (colour + head + six armour slots)
//!   → recoloured 16-bone character (the player + humanoid NPCs), plus
//!   per-frame skeletal ANIMATION (idle vs walk/run from the entity's
//!   replicated velocity) via [`animate_humanoids`].
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
//! Still-unsupported bodies (quadruped-medium, birds, …) are marked
//! [`FigureBuilt`] immediately and keep their capsule. `TODO(EM-3.8c)`: the
//! remaining bodies + quadruped animation + humanoid weapons/equipped gear.
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
use xindeler_protocol::{NetBody, NetVel};
use xindeler_render_voxel::figure::{
    self, FigureBody, LoadedPart, PartSpecRef, QS_CENTRAL_MANIFEST, QS_LATERAL_MANIFEST,
    QsCentralManifest, QsLateralManifest,
    humanoid::{
        self, HUM_ARMOR_BELT_MANIFEST, HUM_ARMOR_CHEST_MANIFEST, HUM_ARMOR_FOOT_MANIFEST,
        HUM_ARMOR_HAND_MANIFEST, HUM_ARMOR_PANTS_MANIFEST, HUM_ARMOR_SHOULDER_MANIFEST,
        HUM_COLOR_MANIFEST, HUM_HEAD_MANIFEST, HumAnim, HumArmorBeltSpec, HumArmorChestSpec,
        HumArmorFootSpec, HumArmorHandSpec, HumArmorPantsSpec, HumArmorShoulderSpec, HumBone,
        HumColorSpec, HumHeadSpec, HumManifests, HumVoxRef, LoadedHumPart,
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
            .init_asset::<HumColorManifestAsset>()
            .init_asset::<HumHeadManifestAsset>()
            .init_asset::<HumChestManifestAsset>()
            .init_asset::<HumBeltManifestAsset>()
            .init_asset::<HumPantsManifestAsset>()
            .init_asset::<HumFootManifestAsset>()
            .init_asset::<HumHandManifestAsset>()
            .init_asset::<HumShoulderManifestAsset>()
            .init_asset_loader::<VoxLoader>()
            .init_asset_loader::<QsCentralManifestLoader>()
            .init_asset_loader::<QsLateralManifestLoader>()
            .init_asset_loader::<HumColorManifestLoader>()
            .init_asset_loader::<HumHeadManifestLoader>()
            .init_asset_loader::<HumChestManifestLoader>()
            .init_asset_loader::<HumBeltManifestLoader>()
            .init_asset_loader::<HumPantsManifestLoader>()
            .init_asset_loader::<HumFootManifestLoader>()
            .init_asset_loader::<HumHandManifestLoader>()
            .init_asset_loader::<HumShoulderManifestLoader>()
            .add_systems(Startup, (load_figure_manifests, load_humanoid_manifests))
            .add_systems(
                Update,
                (
                    classify_bodies,
                    build_pending_figures,
                    build_pending_humanoids,
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

/// Marks an entity that has been finalised: it has a real figure OR is a
/// deliberately-kept capsule (unsupported body). Neither figure nor capsule
/// path re-processes it.
#[derive(Component)]
pub struct FigureBuilt;

/// Every frame, for any mirrored entity not yet finalised: classify its body.
/// Unsupported → mark [`FigureBuilt`] (keeps its capsule). Supported → once the
/// manifests are parsed, start loading its `.vox` parts ([`PendingFigure`]).
/// Runs each frame (not just on `Added`) so it is robust to the manifests
/// finishing loading AFTER the first NPCs are mirrored.
fn classify_bodies(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    manifests: Option<Res<FigureManifests>>,
    central_assets: Res<Assets<QsCentralManifestAsset>>,
    lateral_assets: Res<Assets<QsLateralManifestAsset>>,
    hum: Option<Res<HumanoidManifests>>,
    hum_assets: HumManifestAssets,
    query: Query<
        (Entity, &NetBody),
        (
            With<NetBody>,
            Without<FigureBuilt>,
            Without<PendingFigure>,
            Without<PendingHumanoid>,
        ),
    >,
) {
    for (entity, body) in &query {
        let figure_body = resolve_figure_body(body);
        let species_body_type = match figure_body {
            FigureBody::QuadrupedSmall { species, body_type } => (species, body_type),
            FigureBody::Humanoid(hum_body) => {
                // Humanoid path (EM-3.8b): once the humanoid manifests are all
                // parsed, resolve the part `.vox` list and start loading them.
                let (Some(hum), Some(manifests)) = (&hum, hum_assets.get()) else {
                    continue; // manifests still loading — keep the capsule, retry
                };
                let _ = hum; // handles kept alive by the resource
                let Some(refs) = humanoid::humanoid_vox_refs(&manifests, &hum_body) else {
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
        // Wait until all parts have loaded (or hard-failed).
        let mut all_ready = true;
        let mut any_failed = false;
        for part in &figure.parts {
            match asset_server.get_load_state(&part.handle) {
                Some(LoadState::Loaded) => {},
                Some(LoadState::Failed(_)) => any_failed = true,
                _ => all_ready = false,
            }
        }
        if any_failed {
            warn!("figure: a .vox part failed to load; keeping the placeholder capsule");
            commands
                .entity(entity)
                .remove::<PendingFigure>()
                .insert(FigureBuilt);
            continue;
        }
        if !all_ready {
            continue;
        }

        let FigureBody::QuadrupedSmall { species, body_type } = figure.body else {
            commands
                .entity(entity)
                .remove::<PendingFigure>()
                .insert(FigureBuilt);
            continue;
        };
        let rest = figure::quadruped_small_bone_rest(species, body_type);

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
        let assembled = figure::assemble(&loaded, &rest);

        // One shared matte material for all vertex-coloured parts (bind-group
        // reuse). base_color WHITE so the per-voxel vertex colour shows through.
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.85,
            ..default()
        });

        let mut ec = commands.entity(entity);
        // Drop the placeholder capsule geometry; keep the (interpolated) root
        // Transform + Visibility so the figure moves with the entity.
        ec.remove::<Mesh3d>()
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .remove::<PendingFigure>()
            .insert(FigureBuilt);
        ec.with_children(|root| {
            for part in assembled {
                root.spawn((
                    Mesh3d(meshes.add(part.mesh)),
                    MeshMaterial3d(material.clone()),
                    part.transform,
                    Name::new(part.name),
                ));
            }
        });

        info!("figure: assembled a real .vox model for a quadruped-small NPC");
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
        })
    }
}

/// A humanoid entity whose real figure is still loading its `.vox` parts.
#[derive(Component)]
struct PendingHumanoid {
    parts: Vec<PendingHumPart>,
    body: common::comp::humanoid::Body,
}

struct PendingHumPart {
    spec: HumVoxRef,
    handle: Handle<VoxAsset>,
}

/// A finalised humanoid figure: keeps the `Body` (for per-frame animation) and
/// the bone→child-entity map so [`animate_humanoids`] can update each part's
/// `Transform` every frame. Present ⇒ the entity is also [`FigureBuilt`].
#[derive(Component)]
struct HumanoidFigure {
    body: common::comp::humanoid::Body,
    /// One (bone, child-entity) pair per assembled part.
    parts: Vec<(HumBone, Entity)>,
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
        let mut all_ready = true;
        let mut any_failed = false;
        for part in &figure.parts {
            match asset_server.get_load_state(&part.handle) {
                Some(LoadState::Loaded) => {},
                Some(LoadState::Failed(_)) => any_failed = true,
                _ => all_ready = false,
            }
        }
        if any_failed {
            warn!("humanoid figure: a .vox part failed to load; keeping the placeholder capsule");
            commands
                .entity(entity)
                .remove::<PendingHumanoid>()
                .insert(FigureBuilt);
            continue;
        }
        if !all_ready {
            continue;
        }

        // Pair each loaded `.vox` back with its role, then recolour + assemble.
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
        // Rest pose (idle at t=0) for the initial placement; animation updates
        // it every frame (`animate_humanoids`).
        let rest = humanoid::humanoid_bone_rest(&figure.body);

        // One shared matte material (bind-group reuse); base_color WHITE so the
        // per-voxel vertex colour shows through (same as the quadruped path).
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.85,
            ..default()
        });

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
        ec.insert(HumanoidFigure {
            body: figure.body,
            parts: part_entities,
        });

        info!("humanoid figure: assembled a real .vox character (default loadout)");
    }
}

/// Per-frame skeletal animation for assembled humanoids (EM-3.8b Part B): pick
/// idle vs run from the entity's replicated velocity, recompute the character
/// bone transforms at the current time, and write each part-child's
/// `Transform`.
///
/// The whole figure already faces its heading (the entity root carries the
/// interpolated orientation), so we drive the animation with a generic forward
/// velocity of the right MAGNITUDE (`|NetVel|`), not its world direction — that
/// keeps the stride speed matched without double-applying the heading.
fn animate_humanoids(
    time: Res<Time>,
    figures: Query<(&HumanoidFigure, Option<&NetVel>)>,
    mut transforms: Query<&mut Transform>,
) {
    let t = time.elapsed_secs();
    for (figure, vel) in &figures {
        // Horizontal ground speed (blocks/s). NetVel is in Bevy axes; the
        // ground plane is x/z (y is up), so ignore the vertical component.
        let speed = vel.map_or(0.0, |v| {
            let h = v.0;
            (h.x * h.x + h.z * h.z).sqrt()
        });
        // Small deadzone so idle NPCs don't jitter into a run cycle.
        let anim = if speed > 0.4 {
            HumAnim::Run
        } else {
            HumAnim::Idle
        };
        let bones = humanoid::humanoid_bone_transforms(&figure.body, anim, t, speed);
        for (bone, child) in &figure.parts {
            if let Ok(mut tf) = transforms.get_mut(*child) {
                *tf = bones.get(*bone);
            }
        }
    }
}
