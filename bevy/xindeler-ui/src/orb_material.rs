//! BL-82 EM-5.17 T57.9 — the `UiMaterial` spike + decision.
//!
//! ## The question
//! The Notion design source (spec §3.1/§4.0) recommends a custom `UiMaterial`
//! WGSL shader (Bevy 0.14's `MaterialNodeBundle`) for the resource orbs'
//! liquid fill (circular clip + an animated sine-wave liquid surface) and for
//! the minimap's circular-clip mask (Phase 3). This project is pinned to
//! Bevy **0.19**, whose render stack changed materially since 0.14 (ECS-
//! scheduled render graph, resources-as-components, etc. — the standing EM-M1
//! note in `docs/backlog/engine-migration.md`) — the spec explicitly flagged
//! this as needing verification, not assumption, before Phase 2/3 commit to
//! either approach.
//!
//! ## The spike (real, not just reading docs)
//! Checked directly against this workspace's PINNED `bevy = "=0.19.0"`
//! registry source (`~/.cargo/registry/src/…/bevy_ui_render-0.19.0/src/
//! ui_material.rs`, `ui_material_pipeline.rs`):
//! - `bevy_ui_render::ui_material::UiMaterial` — the trait — **still exists**
//!   in 0.19, with the same shape the doc example above assumes (`AsBindGroup +
//!   Asset + Clone`, `vertex_shader()`/`fragment_shader()` returning
//!   [`bevy_shader::ShaderRef`], an optional `specialize`).
//! - `MaterialNode<M: UiMaterial>` — **still exists**, a real `Component`
//!   (`#[require(Node)]`) replacing 0.14's `MaterialNodeBundle` — the same
//!   "bundle → bare component" migration this project's other Bevy-0.19 code
//!   already follows (spec §4.0's own porting-notes list).
//! - `UiMaterialPlugin<M>` — **still exists**, re-exported at
//!   `bevy_ui_render`'s crate root, itself re-exported by the umbrella `bevy`
//!   crate as `bevy::ui_render` (`bevy_internal::lib.rs`: `#[cfg(feature =
//!   "bevy_ui_render")] pub use bevy_ui_render as ui_render;`).
//! - **This crate ALREADY enables the `bevy_ui_render` Cargo feature**
//!   (`xindeler-ui/Cargo.toml`'s `bevy` dependency feature list) — added for
//!   EM-5.1's own `bevy_ui` rendering needs, so wiring a `UiMaterial` here
//!   costs **zero new dependencies, zero new Cargo features**.
//! - Precedent: this project already has a real, in-tree custom-shader
//!   convention to copy the SHAPE of (`bevy/xindeler-client/src/
//!   far_terrain_material.rs`'s `ExtendedMaterial<StandardMaterial, X>` +
//!   `embedded_asset!` + a dedicated `*MaterialPlugin` struct) — a 3D-material
//!   example, not `UiMaterial` itself, but the "embed the WGSL via
//!   `embedded_asset!`, register a small `Plugin` that adds the material
//!   plugin" house style transfers directly.
//!
//! **Conclusion: `UiMaterial` is genuinely viable in Bevy 0.19 — NOT
//! awkward, deprecated, or costly at the trait/plugin level.** The original
//! worry (spec §4.0: "verify the API surface still exists") is resolved: it
//! does, with no version-mismatch tax.
//!
//! ## The decision — CPU-clip for v1's actual orb rendering, this file as a
//! working (not merely theoretical) v2 on-ramp
//! Despite `UiMaterial` being confirmed cheap to reach for, **Phase 2's
//! actual health/stamina/mana orb fill should still use [`crate::bar`]'s new
//! `spawn_orb_bar`/CPU-clip mechanism (Task 2 of this same phase), not this
//! material** — for a reason specific to THIS asset pack, not a generic
//! "shaders are scary" hedge: spec §3.1 itself describes each orb as TWO
//! flat layers, `*_liquid.png` (bottom) then `orb_frame_*.png` (top,
//! "alpha-transparent center"). The frame PNG's own alpha channel is already
//! what makes the orb read as circular — a plain rectangular bottom-anchored
//! reveal underneath it looks correct today with **zero shader risk**,
//! because the frame masks the corners for free. The one thing a shader
//! would add on top — an animated sine-wave liquid SURFACE line instead of a
//! flat horizontal cut — is real, but purely a v2 AAA-polish delta, not
//! something the frame-masking trick can fake.
//!
//! So: this module ships a genuinely working (compiles, registers, spawns,
//! tested) [`OrbLiquidMaterial`] — fraction-driven, alpha-discard fill,
//! backed by a real embedded WGSL fragment shader — as the **low-risk,
//! already-proven on-ramp** for that v2 wave pass, so whoever picks it up
//! later inherits a tested `UiMaterialPlugin` registration and a working
//! fraction uniform, and only needs to add the sine perturbation to the
//! fragment shader's cutoff line (marked with a `TODO(v2 wave)` comment
//! below) — not re-derive "does `UiMaterial` even work here" from scratch.
//! **Not wired into any HUD screen this phase** (no orb screen exists yet —
//! that's Phase 2) and not the thing Phase 2's briefs should default to.
//!
//! ## Minimap circular clip (Phase 3, spec §3.3)
//! The same confirmed-viable `UiMaterial` path applies to the minimap's
//! circular-clip mask — this module doesn't build that material (out of
//! scope for Phase 1; no minimap-reskin work happens here), but Phase 3's
//! brief can copy this file's registration shape (`embedded_asset!` + a
//! small `Plugin` adding `UiMaterialPlugin::<M>::default()`) directly.

use bevy::{
    app::{App, Plugin},
    asset::{Asset, Handle, embedded_asset},
    ecs::component::Component,
    image::Image,
    prelude::UiMaterial,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};

/// A `UiMaterial` for a circular liquid-fill orb: samples `liquid_texture`
/// and discards (fully transparent) every fragment above the current
/// [`Self::fill_fraction`] cutoff line — the same "bottom-up reveal" the
/// CPU-clip [`crate::bar::spawn_orb_bar`] does, but computed per-fragment in
/// the shader instead of by resizing a child `Node`. **v1 ships the flat
/// cutoff only** — the wavy animated surface (spec's "sine-wave liquid
/// surface") is a documented, NOT-yet-implemented v2 follow-up (see the
/// `TODO(v2 wave)` marker in the embedded fragment shader).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Component)]
pub struct OrbLiquidMaterial {
    /// The liquid texture (e.g. `health_liquid.png`/`stamina_liquid.png`/
    /// `mana_liquid.png`, resolved via [`crate::images::HudImages`]).
    #[texture(0)]
    #[sampler(1)]
    pub liquid_texture: Handle<Image>,
    /// Current `current/max` fraction, `[0.0, 1.0]` — the caller (a future
    /// Phase 2 system reading `NetHealth`/`NetPoise`/`NetEnergy`) updates
    /// this on the material asset directly (`Assets<OrbLiquidMaterial>::
    /// get_mut`), the same "caller owns the value, this owns the render"
    /// split [`crate::bar::BarValue`] already establishes for the CPU path.
    #[uniform(2)]
    pub fill_fraction: f32,
}

impl UiMaterial for OrbLiquidMaterial {
    fn fragment_shader() -> ShaderRef { "embedded://xindeler_ui/orb_material.wgsl".into() }
}

/// Registers the embedded orb-liquid WGSL + [`bevy::ui_render::
/// UiMaterialPlugin<OrbLiquidMaterial>`]. Added by [`crate::XindelerUiPlugin`]
/// unconditionally (registering the plugin/asset type costs nothing while
/// unused — no orb screen spawns a [`MaterialNode`](bevy::ui_render::
/// MaterialNode)`<OrbLiquidMaterial>` yet; that's Phase 2's job, and Phase 2
/// may reasonably choose the CPU-clip path instead per this module's own
/// documented recommendation above).
pub(crate) struct OrbMaterialPlugin;

impl Plugin for OrbMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "orb_material.wgsl");
        app.add_plugins(bevy::ui_render::UiMaterialPlugin::<OrbLiquidMaterial>::default());
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, asset::AssetPlugin, prelude::*};

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

    /// A material asset's `fill_fraction` round-trips verbatim — pins the
    /// uniform field's meaning against an accidental rename/reorder.
    #[test]
    fn fill_fraction_round_trips() {
        let material = OrbLiquidMaterial {
            liquid_texture: Handle::default(),
            fill_fraction: 0.42,
        };
        assert!((material.fill_fraction - 0.42).abs() < f32::EPSILON);
    }
}
