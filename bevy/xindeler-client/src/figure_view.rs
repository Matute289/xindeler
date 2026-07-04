//! EM-3.8 — real `.vox` figures for mirrored entities (listen-server only).
//!
//! Replaces EM-3.7's placeholder capsules with the actual Veloren voxel models
//! for the bodies v1 supports (quadruped-small — the sim's test-NPC Pig). The
//! heavy lifting (meshing + rest-pose bone placement) lives in
//! `xindeler-render-voxel::figure`; THIS module is the client-side asset glue.
//!
//! ## Flow (decoupled from load timing)
//! `super::entity_view::add_presentation` still gives EVERY new mirrored entity
//! a placeholder capsule on `Added<NetBody>`. THIS module then, every frame for
//! any entity not yet finalised (`Without<FigureBuilt>`):
//! 1. classifies the body (`resolve_figure_body`);
//! 2. for a SUPPORTED body, once the manifests are parsed, starts loading its
//!    `.vox` parts (attaches [`PendingFigure`]);
//! 3. once every part has loaded, REPLACES the capsule (removes `Mesh3d`/
//!    `MeshMaterial3d`) with one child entity per part — each meshed (`figure`
//!    crate) and parented at its rest-pose bone `Transform` — then marks
//!    [`FigureBuilt`].
//!
//! Unsupported bodies (humanoid, etc.) are marked [`FigureBuilt`] immediately
//! and keep their capsule. `TODO(EM-3.8b)`: humanoid (armour/recolour/16-bone
//! character skeleton) + the remaining bodies + time-based skeletal animation.
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
use xindeler_protocol::NetBody;
use xindeler_render_voxel::figure::{
    self, FigureBody, LoadedPart, PartSpecRef, QS_CENTRAL_MANIFEST, QS_LATERAL_MANIFEST,
    QsCentralManifest, QsLateralManifest,
};

/// Installs the figure asset loaders, the manifest handles, and the two figure
/// systems (classify → build). Added alongside `EntityViewPlugin`.
pub struct FigureViewPlugin;

impl Plugin for FigureViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<VoxAsset>()
            .init_asset::<QsCentralManifestAsset>()
            .init_asset::<QsLateralManifestAsset>()
            .init_asset_loader::<VoxLoader>()
            .init_asset_loader::<QsCentralManifestLoader>()
            .init_asset_loader::<QsLateralManifestLoader>()
            .add_systems(Startup, load_figure_manifests)
            .add_systems(Update, (classify_bodies, build_pending_figures).chain());
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
    query: Query<(Entity, &NetBody), (With<NetBody>, Without<FigureBuilt>, Without<PendingFigure>)>,
) {
    for (entity, body) in &query {
        let figure_body = resolve_figure_body(body);
        let FigureBody::QuadrupedSmall { species, body_type } = figure_body else {
            // Unsupported body: keep the capsule, don't reconsider it.
            commands.entity(entity).insert(FigureBuilt);
            continue;
        };

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
