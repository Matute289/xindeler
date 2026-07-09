//! Client half of EM-2.4: applies [`AtmosphereController::current`] (owned
//! and lerped by `xindeler-oracle-host`, spec §5.4) to the render
//! components each time it changes:
//!
//! - `DistanceFog` (camera): color + exponential falloff density,
//! - `VolumetricFog` (camera): ambient intensity,
//! - `FogVolume` (scene entities): density factor,
//! - `DirectionalLight` (the [`Sun`]): illuminance,
//! - `ClearColor` (world): sky/void color,
//! - [`SunCycle`]: `time_lock` freezes/unfreezes the day/night stub.
//!
//! The controller only dirties its change tick while a transition animates
//! (or a profile [re]loads), so this system is quiet — and the sun's cascade
//! change ticks stay clean — when the atmosphere is settled.

use std::path::PathBuf;

use bevy::{light::VolumetricFog, prelude::*};
use xindeler_app::PresentationSet;
use xindeler_oracle_host::{
    AtmosphereController, AtmosphereProfile, XindelerAtmospherePlugin,
    atmosphere::DEFAULT_PROFILE_ASSET_PATH,
};

use crate::light::{Sun, SunCycle};

/// Asset path of the boot profile (see `assets/xindeler/atmosphere/`).
pub const PROFILE_ASSET_PATH: &str = DEFAULT_PROFILE_ASSET_PATH;

/// The game asset directory: `XINDELER_ASSETS` first, `VELOREN_ASSETS` as the
/// transition fallback (same precedence as the EM-1.4 shim in
/// `common-assets`), then `<cwd>/assets` for dev runs. Asset NAMES under the
/// root stay Veloren-verbatim (isolation law #3) — only the env var rebrands.
///
/// This MUST be handed to `AssetPlugin.file_path` explicitly: bevy's default
/// resolves relative to `CARGO_MANIFEST_DIR` (= `bevy/xindeler-client/` under
/// `cargo run`), not the workspace root, so the default would miss `assets/`
/// entirely (verified empirically — "Path not found" + no file watcher).
#[must_use]
pub fn assets_root() -> PathBuf {
    std::env::var_os("XINDELER_ASSETS")
        .or_else(|| std::env::var_os("VELOREN_ASSETS"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map_or_else(|_| PathBuf::from("assets"), |cwd| cwd.join("assets"))
        })
}

pub struct AtmospherePlugin;

impl Plugin for AtmospherePlugin {
    fn build(&self, app: &mut App) {
        // Sky ambient so the vertex AO has indirect light to act on (spec §4.3
        // multiplies AO into indirect ONLY; bevy's default 80 cd/m² is invisible
        // next to the 130k-lux sun at EV100 13). 0.19: `GlobalAmbientLight` is the
        // resource (`AmbientLight` became per-camera). EM-3.4: the value is DATA
        // (`AtmosphereProfile.ambient_sky`) — boot from the same defaults the
        // shipped default.atmo.ron carries (single source of truth, no boot pop);
        // apply_atmosphere lerps/applies it like every other atmosphere knob.
        app.insert_resource(ambient_light_from(&AtmosphereProfile::default()));
        app.add_plugins(XindelerAtmospherePlugin {
            profile_path: PROFILE_ASSET_PATH.to_owned(),
        })
        .add_systems(
            PostUpdate,
            apply_atmosphere
                .in_set(PresentationSet)
                .run_if(resource_changed::<AtmosphereController>),
        );
    }
}

/// `DistanceFog` built from a profile. Used both by the camera rig at spawn
/// (so boot state == `AtmosphereProfile::default()` == the shipped
/// `default.atmo.ron` — no first-apply pop) and by [`apply_atmosphere`].
///
/// The directional-light glow parameters stay code-side for now (not yet
/// atmosphere data); they are constant across profiles.
pub fn distance_fog_from(profile: &AtmosphereProfile) -> DistanceFog {
    DistanceFog {
        color: profile_fog_color(profile),
        directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.5),
        directional_light_exponent: 30.0,
        // EM-3.11f: `ExponentialSquared`, not `Exponential` — plain exponential
        // fog has NO near-field grace period (transmittance already visibly
        // drops within the first ~30-50m, per Matías's "feels foggy all the
        // time, not just far away" report), because `1 - exp(-d·density)`
        // rises fast right from distance 0. `ExponentialSquared`'s
        // `1 - exp(-(d·density)²)` rises much more slowly near the camera
        // (quadratic in the exponent) and accelerates further out, so nearby
        // terrain/trees stay clear while the far-mesh horizon (the thing this
        // fog exists to mask, EM-3.11b) is still fully hidden by ~200-250m.
        // Same profile field (`fog_density`), same schema — just a curve swap
        // + retuned constant (see `default.atmo.ron`'s comment for the numbers).
        falloff: FogFalloff::ExponentialSquared {
            density: profile.fog_density,
        },
    }
}

/// `VolumetricFog` built from a profile (camera component; see
/// [`distance_fog_from`] for the spawn/apply sharing rationale).
pub fn volumetric_fog_from(profile: &AtmosphereProfile) -> VolumetricFog {
    VolumetricFog {
        ambient_intensity: profile.ambient_light_intensity,
        ..Default::default()
    }
}

/// `GlobalAmbientLight` built from a profile (see [`distance_fog_from`] for
/// the spawn/apply sharing rationale). `affects_lightmapped_meshes` is a
/// code-side constant (terrain is never lightmapped).
pub fn ambient_light_from(profile: &AtmosphereProfile) -> bevy::light::GlobalAmbientLight {
    let [r, g, b] = profile.ambient_sky.color;
    bevy::light::GlobalAmbientLight {
        color: Color::srgb(r, g, b),
        brightness: profile.ambient_sky.brightness,
        affects_lightmapped_meshes: true,
    }
}

fn profile_fog_color(profile: &AtmosphereProfile) -> Color {
    let [r, g, b] = profile.fog_color;
    Color::srgb(r, g, b)
}

fn profile_sky_color(profile: &AtmosphereProfile) -> Color {
    let [r, g, b] = profile.sky_color;
    Color::srgb(r, g, b)
}

fn apply_atmosphere(
    controller: Res<AtmosphereController>,
    mut distance_fogs: Query<&mut DistanceFog, With<Camera3d>>,
    mut volumetric_fogs: Query<&mut VolumetricFog, With<Camera3d>>,
    mut fog_volumes: Query<&mut bevy::light::FogVolume>,
    mut suns: Query<&mut DirectionalLight, With<Sun>>,
    mut ambient: ResMut<bevy::light::GlobalAmbientLight>,
    mut clear_color: ResMut<ClearColor>,
    mut cycle: ResMut<SunCycle>,
) {
    let profile = &controller.current;

    for mut fog in &mut distance_fogs {
        fog.color = profile_fog_color(profile);
        // EM-3.11f: keep in sync with `distance_fog_from`'s curve choice.
        fog.falloff = FogFalloff::ExponentialSquared {
            density: profile.fog_density,
        };
    }

    for mut fog in &mut volumetric_fogs {
        fog.ambient_intensity = profile.ambient_light_intensity;
    }

    for mut volume in &mut fog_volumes {
        volume.density_factor = profile.fog_volume_density;
    }

    for mut sun in &mut suns {
        // Guard the write: dirtying DirectionalLight forces light re-prep, so
        // only touch it while illuminance actually animates.
        if (sun.illuminance - profile.sun_illuminance).abs() > f32::EPSILON {
            sun.illuminance = profile.sun_illuminance;
        }
    }

    // Sky ambient -> GlobalAmbientLight (EM-3.4). Guard the write like the
    // sun: only dirty the resource while the value actually animates.
    let ambient_target = ambient_light_from(profile);
    if ambient.color != ambient_target.color
        || (ambient.brightness - ambient_target.brightness).abs() > f32::EPSILON
    {
        *ambient = ambient_target;
    }

    // Sky/void color -> world ClearColor (lerped upstream like the rest).
    // Default profile value == bevy's stock ClearColor, so boot is a no-op.
    let sky = profile_sky_color(profile);
    if clear_color.0 != sky {
        clear_color.0 = sky;
    }

    // time_lock: Some(hour) freezes the day/night stub at that hour (the
    // controller lerps the locked hour itself, wrap-aware); None resumes.
    let (paused, hour) = match profile.time_lock {
        Some(hour) => (true, hour),
        None => (false, cycle.hour),
    };
    if cycle.paused != paused || (cycle.hour - hour).abs() > f32::EPSILON {
        cycle.paused = paused;
        cycle.hour = hour;
    }
}
