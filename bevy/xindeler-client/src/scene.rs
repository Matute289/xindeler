//! Demo scene (EM-2.2 acceptance): everything is generated in code — no
//! files under `assets/` (isolation law: asset names stay Veloren-verbatim,
//! and Phase 2 adds none).
//!
//! Contents, each validating part of the pipeline:
//! - big ground plane with a procedurally generated high-frequency checkerboard
//!   (nearest sampler) -> TAA on/off shimmer comparison,
//! - grid of spheres/cubes sweeping metallic x roughness -> PBR response,
//! - tall columns -> long cascaded + contact shadows,
//! - an emissive sphere -> bloom,
//! - a `FogVolume` over one corner -> volumetric fog / god rays.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    light::FogVolume,
    math::Affine2,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use xindeler_app::AppState;

/// BL-82 EM-5.9 (T56.29): now that `MainMenu` is the default interactive state
/// (not `Demo`), the demo scene is scoped to `AppState::Demo` and despawned on
/// exit — closing the `TODO(EM-5.9)` that named this task as the fix point. It
/// used to spawn unconditionally at `Startup`, which — once the menu →
/// Connecting → in-game path lands in a REAL world — would have left the demo
/// ground plane / PBR grid / columns / fog volume coexisting with gameplay in
/// the same Bevy world. Only the `--smoke-atmosphere`/`--smoke-screenshot`/
/// no-feature paths keep `AppState::Demo` as their initial state, so only they
/// spawn it.
pub struct DemoScenePlugin;

impl Plugin for DemoScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Demo), spawn_demo_scene)
            .add_systems(OnExit(AppState::Demo), despawn_demo_scene);
    }
}

/// Tags every entity [`spawn_demo_scene`] creates so [`despawn_demo_scene`] can
/// tear the whole demo scene down when leaving [`AppState::Demo`].
#[derive(Component)]
struct DemoSceneEntity;

/// Removes the demo scene on `OnExit(AppState::Demo)`.
fn despawn_demo_scene(mut commands: Commands, entities: Query<Entity, With<DemoSceneEntity>>) {
    for entity in &entities {
        commands.entity(entity).despawn();
    }
}

/// Procedural checkerboard: 64x64 px, 8 px cells, nearest-filtered and
/// repeat-addressed so it can tile a huge plane at high frequency.
fn checkerboard_image() -> Image {
    const SIZE: u32 = 64;
    const CELL: u32 = 8;
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let light = ((x / CELL) + (y / CELL)).is_multiple_of(2);
            let rgba: [u8; 4] = if light {
                [225, 225, 220, 255]
            } else {
                [30, 30, 35, 255]
            };
            data.extend_from_slice(&rgba);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    image
}

#[expect(
    clippy::cast_precision_loss,
    reason = "tiny grid indices -> f32 positions"
)]
fn spawn_demo_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Ground: 400 m plane, checker tiled 200x -> 0.25 m cells (high
    // frequency in the distance, where TAA-vs-shimmer is most visible).
    let checker = images.add(checkerboard_image());
    commands.spawn((
        DemoSceneEntity,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(400.0, 400.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(checker),
            perceptual_roughness: 0.9,
            uv_transform: Affine2::from_scale(Vec2::splat(200.0)),
            ..Default::default()
        })),
    ));

    // 6x6 metallic (x) by roughness (z) sweep, alternating spheres/cubes.
    // Shifted east since EM-3.3: the voxel demo chunk (voxel_demo.rs) occupies
    // the scene center ([-16, 17] x [-17, 16]).
    const SWEEP_OFFSET_X: f32 = 28.0;
    let sphere = meshes.add(Sphere::new(0.6));
    let cube = meshes.add(Cuboid::from_length(1.1));
    for x in 0..6u32 {
        for z in 0..6u32 {
            let material = materials.add(StandardMaterial {
                base_color: Color::srgb(0.8, 0.35, 0.25),
                metallic: x as f32 / 5.0,
                perceptual_roughness: (z as f32 / 5.0).max(0.05),
                ..Default::default()
            });
            let mesh = if (x + z) % 2 == 0 {
                sphere.clone()
            } else {
                cube.clone()
            };
            commands.spawn((
                DemoSceneEntity,
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_xyz(
                    x as f32 * 2.5 - 6.25 + SWEEP_OFFSET_X,
                    0.6,
                    z as f32 * 2.5 - 6.25,
                ),
            ));
        }
    }

    // Tall columns for long shadows across the checker plane.
    let column = meshes.add(Cuboid::new(1.6, 14.0, 1.6));
    let column_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.55, 0.6),
        perceptual_roughness: 0.7,
        ..Default::default()
    });
    // Kept clear of the voxel chunk footprint (EM-3.3).
    for (x, z) in [(24.0, -14.0), (36.0, 12.0), (-24.0, -20.0), (-24.0, 14.0)] {
        commands.spawn((
            DemoSceneEntity,
            Mesh3d(column.clone()),
            MeshMaterial3d(column_material.clone()),
            Transform::from_xyz(x, 7.0, z),
        ));
    }

    // Emissive sphere: bloom check (south of the voxel chunk since EM-3.3).
    commands.spawn((
        DemoSceneEntity,
        Mesh3d(meshes.add(Sphere::new(0.8))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::BLACK,
            emissive: LinearRgba::rgb(60.0, 25.0, 6.0),
            ..Default::default()
        })),
        Transform::from_xyz(10.0, 3.5, 22.0),
    ));

    // Test fog volume over one corner of the scene (unit cube scaled up).
    // Density comes from the default atmosphere profile (single source of
    // truth); the AtmosphereController re-drives it at runtime (EM-2.4).
    commands.spawn((
        DemoSceneEntity,
        FogVolume {
            density_factor: xindeler_oracle_host::AtmosphereProfile::default().fog_volume_density,
            ..Default::default()
        },
        Transform::from_xyz(-22.0, 5.0, -12.0).with_scale(Vec3::new(30.0, 10.0, 30.0)),
    ));
}
