//! BL-82 EM-5.14 (T56.33) — the char-select 3D character preview.
//!
//! Reuses the EXISTING Bevy figure pipeline (EM-3.8x, `figure_view.rs`) rather
//! than re-implementing any meshing: it spawns a plain local entity carrying
//! [`NetBody`] + [`NetLoadout`] — the exact components `figure_view`'s
//! `classify_bodies`/`build_pending_humanoids` react to — so the same
//! humanoid-assembly, recolour and idle-animation code that renders a live
//! mirrored player renders the preview. No sim entity is required (a
//! `humanoid::Body`'s fields are the whole appearance payload).
//!
//! ## Why a far "stage" instead of `RenderLayers`
//! The figure is spawned far from the world (`STAGE_POS`) and viewed by a
//! dedicated [`Camera3d`] that renders into an offscreen `Image` (the
//! `smoke.rs` render-to-texture primitive), shown in the wizard's preview panel
//! via an `ImageNode`. Because the stage is nowhere near the streamed world,
//! the main window camera never has it in frustum and the preview camera never
//! has the world in frustum — so the two never see each other's content
//! without the extra `RenderLayers` child-propagation the figure pipeline would
//! otherwise need (the figure spawns its parts as child entities). A local
//! [`PointLight`] on the stage guarantees the figure is lit regardless of the
//! world's time-of-day.
//!
//! Framing/lighting values here are analytic (this screen is not yet driven
//! into by a live state machine); they are tunable during the first in-game
//! smoke, matching this crate's established "wired now, visual-tune on first
//! sight" posture for EM-5.x screens.

use bevy::{camera::RenderTarget, prelude::*, render::render_resource::TextureFormat};
use common::comp::Body;
use xindeler_app::AppState;
use xindeler_protocol::{NetBody, NetLoadout};

/// Where the preview figure stands — far from any streamed world chunk so the
/// two cameras' frusta never overlap (see the module doc comment).
const STAGE_POS: Vec3 = Vec3::new(6000.0, 1000.0, 6000.0);

/// Offscreen preview render-target size (portrait).
const PREVIEW_W: u32 = 420;
const PREVIEW_H: u32 = 560;

/// Idle spin rate of the preview figure (radians/sec).
const SPIN_RATE: f32 = 0.6;

/// Resource holding the preview's render-target image + the current figure
/// entity + the body it was last built for (so the figure is rebuilt only when
/// the chosen appearance actually changes — the figure pipeline marks an entity
/// `FigureBuilt` and never rebuilds it in place).
#[derive(Resource)]
pub struct CharPreview {
    pub image: Handle<Image>,
    figure: Option<Entity>,
    last_body: Option<Body>,
    /// The body the rest of the UI wants previewed right now — written by the
    /// char-select screen each frame (the wizard's live body, or the selected
    /// roster character's body) via [`CharPreview::desired_body`]. `None`
    /// hides/clears the figure.
    pub desired_body: Option<Body>,
}

/// Marker for the offscreen preview camera (despawned with the screen).
#[derive(Component)]
struct PreviewCamera;

/// Marker for the preview figure root + its stage light.
#[derive(Component)]
struct PreviewStage;

/// Marker for the preview figure root ONLY (not the stage light) — narrows
/// [`spin_preview_figure`]'s query so it matches a single entity instead of
/// every `Transform` in the world (bevy-migration-reviewer follow-up).
#[derive(Component)]
struct PreviewFigureRoot;

/// Sets up the char-select 3D preview: on entering [`AppState::CharSelect`] it
/// creates the offscreen render target, the preview camera and a stage light;
/// on exit it tears them down. Add in the shell that hosts the char-select
/// screen. Registering the plugin is cheap on any App that never enters
/// `CharSelect`.
pub struct CharPreviewPlugin;

impl Plugin for CharPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::CharSelect), setup_preview)
            .add_systems(OnExit(AppState::CharSelect), teardown_preview)
            .add_systems(
                Update,
                (rebuild_preview_figure, spin_preview_figure)
                    .run_if(in_state(AppState::CharSelect)),
            );
    }
}

fn setup_preview(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = images.add(Image::new_target_texture(
        PREVIEW_W,
        PREVIEW_H,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));

    // Camera looking at the (empty for now) stage, rendering into the image.
    // `RenderTarget` is its OWN component in this Bevy version (not a `Camera`
    // field — see `smoke.rs`'s `retarget_camera_to_image`).
    commands.spawn((
        PreviewCamera,
        Camera3d::default(),
        Camera {
            // A dark, opaque backdrop so the panel reads cleanly.
            clear_color: ClearColorConfig::Custom(Color::srgb(0.06, 0.06, 0.08)),
            // Render before the main window camera; irrelevant to correctness
            // (different targets) but keeps a deterministic order.
            order: -1,
            ..Default::default()
        },
        RenderTarget::Image(image.clone().into()),
        Transform::from_translation(STAGE_POS + Vec3::new(0.0, 1.15, 2.7))
            .looking_at(STAGE_POS + Vec3::new(0.0, 0.9, 0.0), Vec3::Y),
    ));

    // A local light so the figure is lit regardless of world time-of-day.
    commands.spawn((
        PreviewStage,
        PointLight {
            intensity: 2_000_000.0,
            range: 20.0,
            ..Default::default()
        },
        Transform::from_translation(STAGE_POS + Vec3::new(1.5, 2.5, 2.5)),
    ));

    commands.insert_resource(CharPreview {
        image,
        figure: None,
        last_body: None,
        desired_body: None,
    });
}

fn teardown_preview(
    mut commands: Commands,
    cameras: Query<Entity, With<PreviewCamera>>,
    stage: Query<Entity, With<PreviewStage>>,
    preview: Option<Res<CharPreview>>,
) {
    for e in &cameras {
        commands.entity(e).despawn();
    }
    for e in &stage {
        commands.entity(e).despawn();
    }
    if let Some(preview) = preview
        && let Some(figure) = preview.figure
    {
        commands.entity(figure).despawn();
    }
    commands.remove_resource::<CharPreview>();
}

/// Rebuilds the preview figure whenever [`CharPreview::desired_body`] differs
/// from the body currently built. Spawns a plain `NetBody` + `NetLoadout`
/// entity at the stage — the existing `figure_view` pipeline turns it into a
/// real humanoid figure.
fn rebuild_preview_figure(mut commands: Commands, preview: Option<ResMut<CharPreview>>) {
    let Some(mut preview) = preview else { return };
    if preview.desired_body == preview.last_body {
        return;
    }

    // Despawn the previous figure (recursively removes its part children).
    if let Some(figure) = preview.figure.take() {
        commands.entity(figure).despawn();
    }

    if let Some(body) = preview.desired_body {
        // No `PreviewStage` marker here: the figure is torn down explicitly via
        // `CharPreview::figure` (both in `rebuild` and `teardown`), so it must
        // not ALSO be caught by `teardown_preview`'s `PreviewStage` query — that
        // would double-despawn the same entity.
        let figure = commands
            .spawn((
                PreviewFigureRoot,
                NetBody(body),
                NetLoadout::default(),
                Transform::from_translation(STAGE_POS),
                Visibility::Visible,
            ))
            .id();
        preview.figure = Some(figure);
    }
    preview.last_body = preview.desired_body;
}

/// Slowly rotates the preview figure so the player can see the whole model.
fn spin_preview_figure(
    time: Res<Time>,
    preview: Option<Res<CharPreview>>,
    mut figures: Query<&mut Transform, With<PreviewFigureRoot>>,
) {
    let Some(preview) = preview else { return };
    let Some(figure) = preview.figure else { return };
    if let Ok(mut transform) = figures.get_mut(figure) {
        transform.rotate_y(SPIN_RATE * time.delta_secs());
    }
}
