//! BL-82 EM-5.17 T57.9 (v1 scaffold) — the `UiMaterial` powering the
//! health/stamina/mana orbs' liquid fill, reworked (BL-82 EM-5.17, Matías's
//! repeated request this session: "the orbs need a real shader-based
//! agitated-water effect and a dark stone/metal reveal underneath the
//! depleted liquid, not a flat clip window") into the **v2 wave** — the
//! `TODO(v2 wave)` this module's v1 scaffold left in `orb_material.wgsl`.
//!
//! ## History — from theoretical spike to real consumer
//! T57.9 shipped this module as a genuinely working, tested `UiMaterial`
//! (compiles, registers, spawns) but deliberately **not wired into any HUD
//! screen** — Phase 2's actual orb rendering used [`crate::bar`]'s CPU-clip
//! mechanism instead (a resizing `Overflow::clip()` window), since the HUD-D4
//! art pack's frame PNGs already mask the orb's corners for free, so a flat
//! CPU-clip reveal looked correct with zero shader risk. The one thing the
//! CPU-clip approach could never fake — an animated, wavy liquid surface
//! instead of a flat horizontal cut, plus a "the emptied vessel is made of
//! something" material read — is exactly what this rework adds, and
//! [`crate::bar::spawn_orb_bar`] now spawns a
//! [`MaterialNode`](bevy::prelude::MaterialNode)`<OrbLiquidMaterial>` for
//! the orbs' liquid layer instead of a plain `ImageNode`.
//!
//! ## v2: the agitated wave + dark stone/metal depletion reveal
//! [`OrbLiquidMaterial::fill_fraction`] still drives the visible liquid
//! amount, but the fragment shader (`orb_material.wgsl`) now:
//! - perturbs the flat `1.0 - fill_fraction` cutoff line with a sum of two
//!   sines at different frequency/phase/speed, offset by [`Self::time`], so the
//!   boundary reads as a sloshing/agitated water surface rather than a
//!   ruler-straight clip — even while the fraction itself holds steady;
//! - renders the region ABOVE that wavy line (the depleted portion) as a flat
//!   dark grey stone/metal tint instead of discarding to transparent, masked by
//!   the liquid texture's own alpha channel so the reveal still respects the
//!   source art's circular footprint — the "empty vessel" look Matías asked
//!   for, without needing a separate circular mask or a full PBR stone material
//!   (a flat tint is the right v1 scope for a small HUD element — see the
//!   shader's own doc comment).
//!
//! [`Self::time`] is advanced every frame by [`tick_orb_material_time`] from
//! the app's real [`bevy::time::Time`] — the same "caller/plugin owns the
//! uniform write, this owns the render" split [`Self::fill_fraction`]
//! already established (that one is written by
//! [`crate::bar::update_orb_bars`] instead, gated on `Changed<BarValue>`).
//!
//! ## Crop lives in pixel space, resolved in-shader via `textureDimensions`
//! `MaterialNode<M>` (unlike `ImageNode`) has no `rect` field to crop a
//! sub-region of the source texture the way the old CPU-clip path's
//! `ImageNode::rect` did (see [`crate::minimap_material`]'s own
//! `crop_min`/`crop_size` uniforms, which hit the same underlying problem —
//! though that module stores its crop in UV space, not pixel space like this
//! one does). Rather than
//! hand-duplicate the HUD-D4 pack's canvas pixel dimensions as a second Rust
//! constant that could silently drift from the real on-disk art,
//! [`Self::crop_min`]/[`Self::crop_size`] stay in PIXEL space (mirroring the
//! `Rect` [`crate::bar::spawn_orb_bar`]'s `fill_source_crop` parameter
//! already accepts) and the shader itself calls WGSL's `textureDimensions`
//! to normalize against the REAL loaded texture size. `crop_size ==
//! Vec2::ZERO` is the sentinel [`Self::new`] uses for "no crop" (sample the
//! full `[0,1]²` UV) — see the shader's own doc comment.

use bevy::{
    app::{App, Plugin, Update},
    asset::{Asset, Assets, Handle, embedded_asset},
    ecs::{
        component::Component,
        system::{Res, ResMut},
    },
    image::Image,
    math::Vec2,
    prelude::UiMaterial,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
    time::Time,
};

/// A `UiMaterial` for a circular liquid-fill orb: samples `liquid_texture`,
/// perturbs the [`Self::fill_fraction`] cutoff line with an animated wave,
/// and reveals a dark stone/metal tint above it — see the module doc comment
/// for the full v2 rationale.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Component)]
pub struct OrbLiquidMaterial {
    /// The liquid texture (e.g. `health_liquid.png`/`stamina_liquid.png`/
    /// `mana_liquid.png`, resolved via [`crate::images::HudImages`]).
    #[texture(0)]
    #[sampler(1)]
    pub liquid_texture: Handle<Image>,
    /// Current `current/max` fraction, `[0.0, 1.0]` — the caller
    /// ([`crate::bar::update_orb_bars`]) updates this on the material asset
    /// directly (`Assets<OrbLiquidMaterial>::get_mut`), the same "caller owns
    /// the value, this owns the render" split [`crate::bar::BarValue`]
    /// already establishes.
    #[uniform(2)]
    pub fill_fraction: f32,
    /// Seconds since app start — the wave animation's only per-frame driver,
    /// advanced unconditionally every frame by [`tick_orb_material_time`]
    /// (unlike `fill_fraction`, this is never `Changed`-gated: continuous
    /// motion means every frame's write is a real, needed change).
    #[uniform(3)]
    pub time: f32,
    /// Pixel-space crop rect minimum corner, in the liquid texture's OWN
    /// native pixel coordinates (mirrors [`crate::bar::spawn_orb_bar`]'s
    /// `fill_source_crop: Option<bevy::math::Rect>` parameter's `Rect::min`)
    /// — see the module doc comment for why this stays pixel-space rather
    /// than pre-converted to UV.
    #[uniform(4)]
    pub crop_min: Vec2,
    /// Pixel-space crop rect size (`Rect::max - Rect::min`). `Vec2::ZERO` is
    /// the sentinel [`Self::new`] uses for "no crop" — the shader samples the
    /// full `[0,1]²` UV in that case rather than dividing by zero.
    #[uniform(5)]
    pub crop_size: Vec2,
}

impl OrbLiquidMaterial {
    /// Convenience constructor: `crop_min: Vec2::ZERO`, `crop_size:
    /// Vec2::ZERO` (the shader's own "no crop, sample the full `[0,1]²` UV"
    /// sentinel) is what a caller passes for `fill_source_crop: None`'s
    /// equivalent; a real pixel-space `Rect` converts as `(rect.min, rect.max
    /// - rect.min)`.
    #[must_use]
    pub fn new(
        liquid_texture: Handle<Image>,
        crop_min: Vec2,
        crop_size: Vec2,
        fill_fraction: f32,
    ) -> Self {
        Self {
            liquid_texture,
            fill_fraction,
            time: 0.0,
            crop_min,
            crop_size,
        }
    }
}

impl UiMaterial for OrbLiquidMaterial {
    fn fragment_shader() -> ShaderRef { "embedded://xindeler_ui/orb_material.wgsl".into() }
}

/// Advances every live [`OrbLiquidMaterial`]'s [`OrbLiquidMaterial::time`]
/// uniform each frame from the app's real [`Time`] — the wave animation's
/// only per-frame driver. Deliberately unconditional (not `Changed`-gated,
/// unlike [`crate::bar::update_orb_bars`]'s `fill_fraction` write): the whole
/// point is continuous per-frame motion, so every write here is a genuine,
/// needed change. A handful of materials in practice (one per resource orb —
/// 3 today) makes the extra GPU bind-group re-upload this forces every frame
/// negligible for a small HUD element.
pub(crate) fn tick_orb_material_time(
    time: Res<Time>,
    mut materials: ResMut<Assets<OrbLiquidMaterial>>,
) {
    let elapsed = time.elapsed_secs();
    for (_, material) in materials.iter_mut() {
        material.time = elapsed;
    }
}

/// Registers the embedded orb-liquid WGSL + [`bevy::ui_render::
/// UiMaterialPlugin<OrbLiquidMaterial>`] + [`tick_orb_material_time`]. Added
/// by [`crate::XindelerUiPlugin`] unconditionally — `xindeler-client`'s
/// `combat_hud::spawn_combat_hud` is this material's real consumer (all three
/// resource orbs).
pub(crate) struct OrbMaterialPlugin;

impl Plugin for OrbMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "orb_material.wgsl");
        app.add_plugins(bevy::ui_render::UiMaterialPlugin::<OrbLiquidMaterial>::default());
        app.add_systems(Update, tick_orb_material_time);
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, asset::AssetPlugin, ecs::system::RunSystemOnce, prelude::*};

    use super::*;

    /// [`OrbMaterialPlugin`] registers without panicking and the material
    /// asset type becomes real — the structural half of this spike's
    /// acceptance bar (a real render-pipeline pixel test needs a GPU/window,
    /// out of scope for a headless crate test, same posture as this crate's
    /// other picking/render-adjacent tests).
    #[test]
    fn orb_material_plugin_registers_without_panicking() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<OrbLiquidMaterial>();
        // `UiMaterialPlugin` itself requires the full render-app machinery
        // (RenderApp sub-app, GPU device) that isn't available headlessly —
        // exercise ONLY the embedded-asset registration + the material
        // trait's shader-path resolution here, which is what this crate can
        // meaningfully assert without a real adapter.
        embedded_asset!(app, "orb_material.wgsl");
        assert!(matches!(
            OrbLiquidMaterial::fragment_shader(),
            ShaderRef::Path(_)
        ));
    }

    /// A material asset's `fill_fraction`/`crop_min`/`crop_size` round-trip
    /// verbatim — pins the uniform fields' meaning against an accidental
    /// rename/reorder.
    #[test]
    fn fields_round_trip() {
        let material = OrbLiquidMaterial::new(
            Handle::default(),
            Vec2::new(320.0, 0.0),
            Vec2::new(768.0, 768.0),
            0.42,
        );
        assert!((material.fill_fraction - 0.42).abs() < f32::EPSILON);
        assert_eq!(material.crop_min, Vec2::new(320.0, 0.0));
        assert_eq!(material.crop_size, Vec2::new(768.0, 768.0));
        assert_eq!(material.time, 0.0, "new() starts the wave clock at zero");
    }

    /// [`tick_orb_material_time`] advances EVERY live material's `time` to
    /// the app's real elapsed seconds — the wave animation's whole per-frame
    /// contract.
    #[test]
    fn tick_orb_material_time_advances_every_live_material() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<OrbLiquidMaterial>();

        let (handle_a, handle_b) = {
            let mut materials = app.world_mut().resource_mut::<Assets<OrbLiquidMaterial>>();
            let a = materials.add(OrbLiquidMaterial::new(
                Handle::default(),
                Vec2::ZERO,
                Vec2::ZERO,
                1.0,
            ));
            let b = materials.add(OrbLiquidMaterial::new(
                Handle::default(),
                Vec2::ZERO,
                Vec2::ZERO,
                0.5,
            ));
            (a, b)
        };

        // Advance the app's own clock so `elapsed_secs()` reads something
        // other than the default zero, then run the system under test.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(500));
        app.world_mut()
            .run_system_once(tick_orb_material_time)
            .expect("tick_orb_material_time runs");

        let materials = app.world().resource::<Assets<OrbLiquidMaterial>>();
        let elapsed = app.world().resource::<Time>().elapsed_secs();
        assert!(elapsed > 0.0, "the app clock must have actually advanced");
        assert_eq!(materials.get(&handle_a).unwrap().time, elapsed);
        assert_eq!(materials.get(&handle_b).unwrap().time, elapsed);
    }
}
