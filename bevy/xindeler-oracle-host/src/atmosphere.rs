//! Data-driven atmosphere (BL-82 EM-2.4, spec §5.4).
//!
//! [`AtmosphereProfile`] is a RON asset (`*.atmo.ron`) mirroring the
//! atmospheric slice of the DmEvent schema (spec §5.1): ORACLE writes files,
//! the engine loads assets. [`AtmosphereController`] holds the live
//! `current -> target` interpolation so every profile change animates over
//! `transition_secs` — values never pop.
//!
//! Split of responsibilities (isolation-friendly):
//! - this crate (headless-safe, no render deps): the asset type, its
//!   [`AssetLoader`], the controller resource and the lerp/transition system;
//! - `xindeler-client`: the apply system writing `controller.current` into the
//!   render components (`DistanceFog`, `VolumetricFog`, `FogVolume`,
//!   `DirectionalLight`, the `SunCycle` stub).

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    ecs::error::BevyError,
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// Canonical asset path (relative to the `assets/` source root) of the
/// default profile the client boots with.
pub const DEFAULT_PROFILE_ASSET_PATH: &str = "xindeler/atmosphere/default.atmo.ron";

/// Placeholder weather taxonomy (spec §5.4 `weather: WeatherEffect`).
///
/// Pure data for now — nothing consumes it until the weather particle/audio
/// profiles land (EM-5.x); it exists so `.atmo.ron` / DmEvent files written
/// today stay forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WeatherEffect {
    #[default]
    None,
    Rain,
    Storm,
}

/// Uniform sky ambient light (drives the client's `GlobalAmbientLight`
/// resource): the indirect-light floor the voxel vertex AO acts on (spec
/// §4.3 multiplies AO into indirect light only — without an ambient term the
/// AO would be invisible).
///
/// Engine extension (EM-3.4): not in the canonical DmEvent example; the name
/// `ambient_sky` follows the engine-extension naming convention of
/// `fog_volume_density`/`sun_illuminance` (spec §5.4 amendment tracked in
/// the design repo).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmbientSky {
    /// sRGB triple for the ambient tint.
    pub color: [f32; 3],
    /// Ambient brightness in cd/m² (bevy's stock default is 80; ours is a
    /// sky-lit outdoor value balanced against the EV100 13 camera).
    pub brightness: f32,
}

impl Default for AmbientSky {
    fn default() -> Self {
        Self {
            // The EM-3.3-era values that used to be hardcoded in the client's
            // AtmospherePlugin — now data like every other atmosphere knob.
            color: [0.75, 0.85, 1.0],
            brightness: 6_000.0,
        }
    }
}

/// One atmosphere preset, loadable from `*.atmo.ron` (and, later, carried
/// inside a DmEvent). All fields have defaults so partial files keep loading
/// as the schema grows (`#[serde(default)]`).
///
/// Field names follow the canonical DmEvent `atmosphere` schema (spec §5.1:
/// `fog_density`, `fog_color`, `sky_color`, `ambient_light_intensity`,
/// `weather_effect`, `time_lock`, `transition_secs`);
/// `fog_volume_density`, `sun_illuminance` and `ambient_sky` are engine
/// extensions (they drive render knobs the DmEvent example doesn't name;
/// spec §5.4 amendment tracked in the design repo).
///
/// `AtmosphereProfile::default()`, the client rig spawn values and the
/// shipped `default.atmo.ron` are one single source of truth, so the first
/// applied profile is a visual no-op (no boot pop). Note the defaults
/// deliberately re-tuned EM-2.2's placeholder distance fog from the
/// two-color `FogFalloff::Atmospheric` mode to the profile-drivable
/// single-density `Exponential` mode (same ~350 m visibility, slightly
/// different far-field tint).
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AtmosphereProfile {
    /// Distance-fog extinction density (`FogFalloff::Exponential`). The
    /// default corresponds to ~350 m visibility (Koschmieder,
    /// `-ln(0.05) / 350`).
    pub fog_density: f32,
    /// Fog color, sRGB triple. Drives `DistanceFog.color`.
    pub fog_color: [f32; 3],
    /// Sky/void color, sRGB triple. Drives the world `ClearColor` (what
    /// shows where nothing — including the atmosphere pass — draws).
    /// Default = bevy's stock clear color (`srgb_u8(43, 44, 47)`).
    pub sky_color: [f32; 3],
    /// Density factor of placed `FogVolume`s (volumetric fog patches).
    /// Engine extension (not in the canonical DmEvent example).
    pub fog_volume_density: f32,
    /// `VolumetricFog.ambient_intensity` on the camera.
    pub ambient_light_intensity: f32,
    /// Sun `DirectionalLight.illuminance`, lux. Default = physical raw
    /// sunlight (130 000 lx, `bevy_light::light_consts::lux::RAW_SUNLIGHT`)
    /// — the camera compensates with `Exposure { ev100: 13.0 }`.
    /// Engine extension (not in the canonical DmEvent example).
    pub sun_illuminance: f32,
    /// Uniform sky ambient (`GlobalAmbientLight` on the client). Engine
    /// extension (EM-3.4) — see [`AmbientSky`].
    pub ambient_sky: AmbientSky,
    /// Placeholder weather tag (data only for now).
    pub weather_effect: WeatherEffect,
    /// `Some(hour)` freezes the day/night cycle at that hour (`0.0..24.0`,
    /// 12.0 = noon) — e.g. a dread dimension locked at midnight. `None`
    /// resumes the normal cycle.
    pub time_lock: Option<f32>,
    /// Seconds over which a change TO this profile interpolates. Clamped to
    /// a small minimum so a zero/negative value still applies (as a snap).
    pub transition_secs: f32,
}

impl Default for AtmosphereProfile {
    fn default() -> Self {
        Self {
            // -ln(0.05) / 350.0 — Koschmieder density for ~350 m visibility.
            fog_density: 0.00856,
            fog_color: [0.55, 0.65, 0.75],
            // bevy's stock ClearColor, srgb_u8(43, 44, 47).
            sky_color: [0.168_627, 0.172_549, 0.184_314],
            fog_volume_density: 0.15,
            ambient_light_intensity: 0.1,
            sun_illuminance: 130_000.0,
            ambient_sky: AmbientSky::default(),
            weather_effect: WeatherEffect::None,
            time_lock: None,
            transition_secs: 5.0,
        }
    }
}

/// Clamp bounds enforced by [`AtmosphereProfile::sanitize`] (anti-chaos,
/// spec §5.1: `.atmo.ron` is a surface ORACLE's LLM-side tooling writes, so
/// hostile/buggy values are the threat model, and a single NaN lerped into
/// `AtmosphereController::current` poisons every later frame).
pub mod bounds {
    /// Extinction density: 1.0 ≈ 3 m visibility, already a whiteout.
    pub const FOG_DENSITY: (f32, f32) = (0.0, 1.0);
    /// `FogVolume::density_factor` (bevy default 0.1; >4 is opaque soup).
    pub const FOG_VOLUME_DENSITY: (f32, f32) = (0.0, 4.0);
    /// `VolumetricFog::ambient_intensity` (bevy default 0.1).
    pub const AMBIENT_LIGHT_INTENSITY: (f32, f32) = (0.0, 10.0);
    /// Lux; 200 000 > any physical daylight (raw sunlight = 130 000).
    pub const SUN_ILLUMINANCE: (f32, f32) = (0.0, 200_000.0);
    /// `GlobalAmbientLight.brightness`, cd/m² (bevy default 80, our sky-lit
    /// default 6 000; 100 000 is already well past "everything washed out").
    pub const AMBIENT_SKY_BRIGHTNESS: (f32, f32) = (0.0, 100_000.0);
    /// Transitions longer than an hour are indistinguishable from broken.
    pub const TRANSITION_SECS: (f32, f32) = (0.0, 3600.0);
}

/// `value` clamped into `(min, max)`; non-finite (NaN/±inf) falls back to
/// `default`.
fn sane(value: f32, (min, max): (f32, f32), default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

impl AtmosphereProfile {
    /// Anti-chaos clamps (spec §5.1): every float is forced finite and into
    /// its [`bounds`] range, colors into `0.0..=1.0`, a locked hour onto the
    /// 24 h wheel (`rem_euclid`); non-finite values fall back to the field
    /// default (a non-finite locked hour drops the lock). Runs on EVERY
    /// ingestion path — the loader (files) and [`AtmosphereController::
    /// retarget`] (any future in-process caller, e.g. DmEvent apply) — so no
    /// unclamped value can ever reach the lerp.
    pub fn sanitize(&mut self) {
        let defaults = Self::default();
        self.fog_density = sane(self.fog_density, bounds::FOG_DENSITY, defaults.fog_density);
        for i in 0..3 {
            self.fog_color[i] = sane(self.fog_color[i], (0.0, 1.0), defaults.fog_color[i]);
            self.sky_color[i] = sane(self.sky_color[i], (0.0, 1.0), defaults.sky_color[i]);
        }
        self.fog_volume_density = sane(
            self.fog_volume_density,
            bounds::FOG_VOLUME_DENSITY,
            defaults.fog_volume_density,
        );
        self.ambient_light_intensity = sane(
            self.ambient_light_intensity,
            bounds::AMBIENT_LIGHT_INTENSITY,
            defaults.ambient_light_intensity,
        );
        self.sun_illuminance = sane(
            self.sun_illuminance,
            bounds::SUN_ILLUMINANCE,
            defaults.sun_illuminance,
        );
        for i in 0..3 {
            self.ambient_sky.color[i] = sane(
                self.ambient_sky.color[i],
                (0.0, 1.0),
                defaults.ambient_sky.color[i],
            );
        }
        self.ambient_sky.brightness = sane(
            self.ambient_sky.brightness,
            bounds::AMBIENT_SKY_BRIGHTNESS,
            defaults.ambient_sky.brightness,
        );
        self.time_lock = self
            .time_lock
            .and_then(|hour| hour.is_finite().then(|| hour.rem_euclid(24.0)));
        self.transition_secs = sane(
            self.transition_secs,
            bounds::TRANSITION_SECS,
            defaults.transition_secs,
        );
    }

    /// Moves `self` a fraction `t` (`0.0..=1.0`) toward `target`.
    ///
    /// Continuous fields lerp; discrete fields (`weather_effect`,
    /// `transition_secs`, and the *presence* of `time_lock`) snap to the
    /// target immediately (t > 0) so the client can react at transition
    /// start. A locked hour lerps wrap-aware while both ends are locked.
    pub fn step_toward(&mut self, target: &Self, t: f32) {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.0 {
            return;
        }
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        self.fog_density = lerp(self.fog_density, target.fog_density);
        for i in 0..3 {
            self.fog_color[i] = lerp(self.fog_color[i], target.fog_color[i]);
            self.sky_color[i] = lerp(self.sky_color[i], target.sky_color[i]);
        }
        self.fog_volume_density = lerp(self.fog_volume_density, target.fog_volume_density);
        self.ambient_light_intensity =
            lerp(self.ambient_light_intensity, target.ambient_light_intensity);
        self.sun_illuminance = lerp(self.sun_illuminance, target.sun_illuminance);
        for i in 0..3 {
            self.ambient_sky.color[i] =
                lerp(self.ambient_sky.color[i], target.ambient_sky.color[i]);
        }
        self.ambient_sky.brightness =
            lerp(self.ambient_sky.brightness, target.ambient_sky.brightness);
        self.time_lock = match (self.time_lock, target.time_lock) {
            (Some(a), Some(b)) => {
                // Shortest path around the 24 h wheel (23.0 -> 1.0 goes
                // forward through midnight, not backward through noon).
                let delta = (b - a + 12.0).rem_euclid(24.0) - 12.0;
                Some((a + delta * t).rem_euclid(24.0))
            },
            (_, lock) => lock,
        };
        self.weather_effect = target.weather_effect;
        self.transition_secs = target.transition_secs;
    }
}

/// Async [`AssetLoader`] for `*.atmo.ron` (verified against bevy_asset
/// 0.19.0: `load` is an `async fn` returning the asset; errors convert into
/// `BevyError`).
#[derive(Default, TypePath)]
pub struct AtmosphereProfileLoader;

impl AssetLoader for AtmosphereProfileLoader {
    type Asset = AtmosphereProfile;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let mut profile: AtmosphereProfile = ron::de::from_bytes(&bytes)?;
        // Anti-chaos (spec §5.1): ORACLE-written files are untrusted input.
        profile.sanitize();
        Ok(profile)
    }

    fn extensions(&self) -> &[&str] { &["atmo.ron"] }
}

/// Live atmosphere state (spec §5.4): `current` is what the client applies
/// every frame; `target` is the last loaded profile; changes interpolate over
/// the target's `transition_secs`.
#[derive(Resource, Debug, Clone)]
pub struct AtmosphereController {
    /// Strong handle keeping the active profile (and its file watch) alive.
    pub handle: Handle<AtmosphereProfile>,
    /// The interpolated state to apply this frame.
    pub current: AtmosphereProfile,
    /// Where `current` is heading.
    pub target: AtmosphereProfile,
    /// False until the first asset load lands (`current` then snaps to it —
    /// which is a no-op visually, since the rigs spawn from the same
    /// defaults the default profile ships).
    pub loaded: bool,
    /// Seconds left in the running transition (0 = settled).
    transition_remaining: f32,
}

impl Default for AtmosphereController {
    fn default() -> Self {
        Self {
            handle: Handle::default(),
            current: AtmosphereProfile::default(),
            target: AtmosphereProfile::default(),
            loaded: false,
            transition_remaining: 0.0,
        }
    }
}

impl AtmosphereController {
    /// True while a transition is animating.
    #[must_use]
    pub fn in_transition(&self) -> bool { self.transition_remaining > 0.0 }

    /// Would [`Self::retarget`] with this profile be a no-op? (Settled on an
    /// identical target — e.g. a file rewritten with unchanged content, or
    /// ORACLE re-emitting the active profile.) Callers holding `ResMut`
    /// should check this through `&*controller` BEFORE calling `retarget`,
    /// so the resource change tick stays clean.
    #[must_use]
    pub fn is_settled_at(&self, profile: &AtmosphereProfile) -> bool {
        self.loaded && !self.in_transition() && *profile == self.target
    }

    /// Aims the controller at `profile` (sanitized on the way in — every
    /// ingestion path clamps, not just the file loader). The first profile
    /// ever seen snaps (boot); later ones animate over their
    /// `transition_secs`. Identical retargets while settled are no-ops
    /// (avoids restarting a multi-second "transition" to where we already
    /// are).
    pub fn retarget(&mut self, mut profile: AtmosphereProfile) {
        profile.sanitize();
        if self.is_settled_at(&profile) {
            return;
        }
        if self.loaded {
            self.transition_remaining = profile.transition_secs.max(0.0);
            self.target = profile;
            if self.transition_remaining <= 0.0 {
                self.current = self.target.clone();
            }
        } else {
            self.loaded = true;
            self.current = profile.clone();
            self.target = profile;
            self.transition_remaining = 0.0;
        }
    }

    /// Advances the transition by `dt` seconds (linear in time: each step
    /// covers `dt / remaining` of what's left, converging exactly at the
    /// deadline).
    pub fn advance(&mut self, dt: f32) {
        if self.transition_remaining <= 0.0 {
            return;
        }
        if dt >= self.transition_remaining {
            self.transition_remaining = 0.0;
            self.current = self.target.clone();
        } else {
            let t = dt / self.transition_remaining;
            self.transition_remaining -= dt;
            let target = self.target.clone();
            self.current.step_toward(&target, t);
        }
    }
}

/// Registers the atmosphere asset pipeline + controller and loads
/// `profile_path` at startup. Requires an `AssetPlugin` in the host app
/// (the client's `DefaultPlugins`; a future server shell would add a
/// minimal asset stack).
pub struct XindelerAtmospherePlugin {
    /// Asset path (relative to the asset source root) of the boot profile.
    pub profile_path: String,
}

impl Default for XindelerAtmospherePlugin {
    fn default() -> Self {
        Self {
            profile_path: DEFAULT_PROFILE_ASSET_PATH.to_owned(),
        }
    }
}

impl Plugin for XindelerAtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<AtmosphereProfile>()
            .init_asset_loader::<AtmosphereProfileLoader>()
            .init_resource::<AtmosphereController>()
            .add_systems(Update, update_atmosphere_controller);

        let path = self.profile_path.clone();
        app.add_systems(
            Startup,
            move |asset_server: Res<AssetServer>, mut controller: ResMut<AtmosphereController>| {
                controller.handle = asset_server.load(path.clone());
            },
        );
    }
}

/// Consumes `AssetEvent<AtmosphereProfile>` (Added/Modified — hot reload via
/// the `file_watcher` feature or an explicit `AssetServer::reload`) into new
/// targets, then advances the lerp. Mutates the resource only when something
/// actually changes, so `resource_changed::<AtmosphereController>` gates
/// downstream apply work (and change ticks stay quiet when settled).
pub fn update_atmosphere_controller(
    time: Res<Time>,
    profiles: Res<Assets<AtmosphereProfile>>,
    mut events: MessageReader<AssetEvent<AtmosphereProfile>>,
    mut controller: ResMut<AtmosphereController>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        if *id != controller.handle.id() {
            continue;
        }
        if let Some(profile) = profiles.get(*id) {
            // Read-only pre-check (`&*controller`): an identical rewrite
            // must not dirty the resource change tick at all.
            if controller.is_settled_at(profile) {
                debug!("atmosphere profile reloaded unchanged; ignoring");
                continue;
            }
            info!(
                "atmosphere profile (re)loaded; transitioning over {:.2}s",
                profile.transition_secs.max(0.0)
            );
            controller.retarget(profile.clone());
        }
    }

    if controller.in_transition() {
        controller.advance(time.delta_secs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_default_profile_matches_rust_defaults() {
        // The committed default.atmo.ron must equal `AtmosphereProfile::
        // default()` so the first applied profile is a visual no-op.
        let text = include_str!("../../../assets/xindeler/atmosphere/default.atmo.ron");
        let parsed: AtmosphereProfile = ron::from_str(text).expect("default.atmo.ron parses");
        assert_eq!(parsed, AtmosphereProfile::default());
    }

    #[test]
    fn partial_profile_uses_defaults() {
        let parsed: AtmosphereProfile =
            ron::from_str("(fog_density: 0.5, fog_color: (1.0, 0.0, 0.0))").expect("parses");
        assert!((parsed.fog_density - 0.5).abs() < f32::EPSILON);
        assert_eq!(parsed.fog_color, [1.0, 0.0, 0.0]);
        assert_eq!(parsed.time_lock, None);
        assert!(
            (parsed.transition_secs - AtmosphereProfile::default().transition_secs).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn transition_is_linear_and_converges() {
        let mut controller = AtmosphereController::default();
        controller.retarget(AtmosphereProfile::default()); // boot snap
        let target = AtmosphereProfile {
            fog_density: 1.0,
            fog_color: [1.0, 0.0, 0.0],
            transition_secs: 2.0,
            ..Default::default()
        };
        controller.retarget(target.clone());
        assert!(controller.in_transition());

        controller.advance(1.0); // halfway
        let expected = (AtmosphereProfile::default().fog_density + 1.0) / 2.0;
        assert!((controller.current.fog_density - expected).abs() < 1e-4);

        controller.advance(1.0); // done (exact)
        assert!(!controller.in_transition());
        assert_eq!(controller.current, target);
    }

    #[test]
    fn first_load_snaps_without_transition() {
        let mut controller = AtmosphereController::default();
        let profile = AtmosphereProfile {
            fog_density: 0.9,
            transition_secs: 10.0,
            ..Default::default()
        };
        controller.retarget(profile.clone());
        assert!(!controller.in_transition());
        assert_eq!(controller.current, profile);
    }

    #[test]
    fn sanitize_defuses_hostile_profiles() {
        // Anti-chaos (spec §5.1): NaN/inf/negatives/out-of-range values from
        // buggy or hostile ORACLE output must come out finite and in-bounds
        // — one NaN lerped into the controller poisons every later frame.
        let mut garbage = AtmosphereProfile {
            fog_density: f32::NAN,
            fog_color: [-3.0, f32::INFINITY, 42.0],
            sky_color: [f32::NEG_INFINITY, 2.0, -0.5],
            fog_volume_density: -7.0,
            ambient_light_intensity: f32::INFINITY,
            sun_illuminance: 9.0e9,
            ambient_sky: AmbientSky {
                color: [f32::NAN, 5.0, -1.0],
                brightness: f32::NEG_INFINITY,
            },
            weather_effect: WeatherEffect::Storm,
            time_lock: Some(37.0),
            transition_secs: -1.0,
        };
        garbage.sanitize();

        let defaults = AtmosphereProfile::default();
        assert!((garbage.fog_density - defaults.fog_density).abs() < f32::EPSILON); // NaN -> default
        assert_eq!(garbage.fog_color, [0.0, defaults.fog_color[1], 1.0]);
        assert_eq!(garbage.sky_color, [defaults.sky_color[0], 1.0, 0.0]);
        assert!((garbage.fog_volume_density - 0.0).abs() < f32::EPSILON);
        assert!(
            (garbage.ambient_light_intensity - defaults.ambient_light_intensity).abs()
                < f32::EPSILON
        );
        assert!((garbage.sun_illuminance - bounds::SUN_ILLUMINANCE.1).abs() < f32::EPSILON);
        assert_eq!(garbage.ambient_sky.color, [
            defaults.ambient_sky.color[0],
            1.0,
            0.0
        ]);
        assert!(
            (garbage.ambient_sky.brightness - defaults.ambient_sky.brightness).abs() < f32::EPSILON
        );
        assert_eq!(garbage.time_lock, Some(13.0)); // 37 h -> 13 h on the wheel
        assert!((garbage.transition_secs - 0.0).abs() < f32::EPSILON);

        // A non-finite locked hour drops the lock instead of freezing on NaN.
        let mut nan_lock = AtmosphereProfile {
            time_lock: Some(f32::NAN),
            ..Default::default()
        };
        nan_lock.sanitize();
        assert_eq!(nan_lock.time_lock, None);

        // Sanitizing an already-sane profile is a no-op (loader + retarget
        // both sanitize; double application must not drift).
        let mut sane_profile = AtmosphereProfile::default();
        sane_profile.sanitize();
        assert_eq!(sane_profile, AtmosphereProfile::default());
    }

    #[test]
    fn identical_retarget_while_settled_is_a_noop() {
        let mut controller = AtmosphereController::default();
        controller.retarget(AtmosphereProfile::default()); // boot snap
        let snapshot = controller.clone();

        // Same profile again (e.g. the smoke harness restoring the RON with
        // identical content): nothing changes, no transition starts.
        assert!(controller.is_settled_at(&AtmosphereProfile::default()));
        controller.retarget(AtmosphereProfile::default());
        assert!(!controller.in_transition());
        assert_eq!(controller.current, snapshot.current);
        assert_eq!(controller.target, snapshot.target);
    }

    #[test]
    fn time_lock_lerps_across_midnight() {
        let mut a = AtmosphereProfile {
            time_lock: Some(23.0),
            ..Default::default()
        };
        let b = AtmosphereProfile {
            time_lock: Some(1.0),
            ..Default::default()
        };
        a.step_toward(&b, 0.5);
        let hour = a.time_lock.expect("still locked");
        assert!((hour - 0.0).abs() < 1e-4, "expected midnight, got {hour}");
    }
}
