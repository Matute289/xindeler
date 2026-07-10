//! Custom post-process slot (EM-2.6): a vignette + gamma placeholder pass
//! proving we can insert fullscreen passes under Bevy 0.19's schedule-based
//! render world (no graph nodes).
//!
//! Mechanism (verified against bevy_core_pipeline 0.19.0 source,
//! `fullscreen_material.rs`): the [`FullscreenMaterial`] trait — the
//! pluginified `custom_post_processing` pattern. Our uniform struct is a
//! camera component; `FullscreenMaterialPlugin` extracts it, uploads it as a
//! uniform, and runs the fragment shader over a fullscreen triangle in
//! `Core3dSystems::PostProcess`, before `tonemapping` (the trait's default
//! schedule slot).

use bevy::{
    asset::embedded_asset,
    core_pipeline::{
        Core3dSystems,
        fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin},
        tonemapping::tonemapping,
    },
    ecs::{
        schedule::{IntoScheduleConfigs, ScheduleConfigs},
        system::BoxedSystem,
    },
    post_process::bloom::bloom,
    prelude::*,
    render::{extract_component::ExtractComponent, render_resource::ShaderType},
    shader::ShaderRef,
};

/// Camera component = the pass's uniform data. Present on the camera => the
/// pass runs (gated by `GraphicsSettings.vignette` at camera spawn).
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType)]
pub struct VignettePost {
    /// Vignette darkening at the frame corners, `0.0..=1.0`.
    pub strength: f32,
    /// Placeholder gamma exponent applied as `color^(1/gamma)`. `1.0` =
    /// neutral (the real tonemapping happens later in the chain; this only
    /// proves per-pixel uniform-driven math in the custom pass).
    pub gamma: f32,
}

impl Default for VignettePost {
    fn default() -> Self {
        Self {
            strength: 0.5,
            gamma: 1.0,
        }
    }
}

impl FullscreenMaterial for VignettePost {
    fn fragment_shader() -> ShaderRef {
        // Registered by `embedded_asset!` in `PostProcessPlugin::build`
        // (crate `src/` prefix is trimmed by the embedded source).
        "embedded://xindeler_client/post.wgsl".into()
    }

    /// EM-3.11k (brightness/contrast flicker): the trait's DEFAULT
    /// `schedule_configs` only orders this pass `.before(tonemapping)` —
    /// bevy's own `bloom` system (`bevy_post_process::bloom::bloom`) carries
    /// the SAME constraint (`.before(tonemapping)`) and nothing else, so the
    /// two `Core3dSystems::PostProcess` systems have NO ordering relative to
    /// EACH OTHER. Confirmed by an instrumented offscreen-capture harness
    /// (since reverted) that isolated every camera effect flag one at a
    /// time: enabling ONLY `bloom` + this vignette pass together (all other
    /// effects off) reproduced a one-frame brightness/contrast pop,
    /// alternating almost every other frame during camera rotation (50
    /// occurrences in a 900-frame capture) — while NEITHER effect alone, nor
    /// any other combination tried, reproduced it. Both passes read/write
    /// the camera's ping-ponged `ViewTarget` (`post_process_write`), so an
    /// unconstrained relative order between them is exactly the kind of
    /// ambiguity that can leave a stale/incorrectly-ordered read on some
    /// schedule builds — pin it explicitly: vignette must darken the FINAL
    /// (already-bloomed) image, so it belongs strictly after bloom, not
    /// racing it.
    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system
            .in_set(Core3dSystems::PostProcess)
            .before(tonemapping)
            .after(bloom)
    }
}

pub struct PostProcessPlugin;

impl Plugin for PostProcessPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "post.wgsl");
        app.add_plugins(FullscreenMaterialPlugin::<VignettePost>::default());
    }
}
