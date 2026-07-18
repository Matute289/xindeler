//! BL-82 EM-5.1 T56.2 — the theme/token layer.
//!
//! `voxygen`'s conrod HUD has NO central theme (§1.2 of the design doc: "conrod
//! has no central theme — colours are scattered `const`s"). This module is the
//! genuine improvement over that legacy gap: one [`HudTheme`] resource holding
//! every colour/spacing/radius/font-role token every widget in this crate
//! reads, so a single value change restyles the whole widget gallery.
//!
//! Per the locked [Q1] decision (spec §4/§9): this COPIES `bevy_feathers`'
//! token/theme APPROACH (one `Resource` holding named design tokens, roles
//! resolved by name rather than scattered literals) — it does not depend on
//! `bevy_feathers` itself, which stays an experimental editor-tooling crate we
//! never link.
//!
//! The concrete colour values below are a v1 Xindeler dark-fantasy palette —
//! deliberately a single tunable `Resource`, not scattered literals, so
//! retuning later (once real concept-art-derived hex values are locked) is a
//! one-file change, not a grep-and-replace across every screen.

use bevy::{
    asset::{AssetServer, Handle},
    color::Color,
    ecs::{resource::Resource, system::Res},
    text::Font,
    ui::Val,
};

/// Named colour roles every widget resolves against, instead of literal
/// `Color` values scattered through widget code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudPalette {
    /// Panel/window background fill (dark, slightly translucent).
    pub panel_bg: Color,
    /// Panel border/frame line.
    pub panel_border: Color,
    /// Primary body text.
    pub text: Color,
    /// De-emphasised/secondary text (tooltips subtitles, disabled labels).
    pub text_muted: Color,
    /// Interactive accent (button highlight, focus ring).
    pub accent: Color,
    /// Health bar fill.
    pub health: Color,
    /// Health bar background (the "missing" portion).
    pub health_bg: Color,
    /// Energy bar fill.
    pub energy: Color,
    pub energy_bg: Color,
    /// Poise bar fill.
    pub poise: Color,
    pub poise_bg: Color,
    /// XP bar fill.
    pub xp: Color,
    pub xp_bg: Color,
    /// Combo counter text/highlight.
    pub combo: Color,
    /// A beneficial buff's icon border/tint.
    pub buff_good: Color,
    /// A harmful debuff's icon border/tint.
    pub buff_bad: Color,
    /// Low-health danger tint (vignette, globe flash).
    pub danger: Color,
    /// The hotbar's cooldown "sweep" veil (`hotbar.rs`'s
    /// `HotbarCooldownOverlay`). BL-82 EM-5.17 Phase 2 follow-up bugfix: this
    /// used to be a raw `Color::srgba(0.0, 0.0, 0.0, 0.7)` literal in
    /// `hotbar.rs` itself, which read fine in isolation but became genuinely
    /// invisible ("no se ven") once that same phase layered
    /// `skill_slot_border.png` underneath it in every slot — that PNG's
    /// "cutout" centre (meant to show the icon through, per the design spec)
    /// is actually fully opaque near-black on disk (sampled average RGB
    /// ~(19, 18, 16)/255, confirmed directly against the real asset, not
    /// assumed), so a black veil composited on top of it stays
    /// indistinguishably black at any alpha or sweep height — not a z-order
    /// bug (the overlay already spawns and draws correctly ABOVE that
    /// border art). This role is deliberately non-black/higher-luminance so
    /// it keeps reading over that near-black slot art regardless (see
    /// `cooldown_overlay_colour_has_enough_luminance_to_read_over_a_near_black_background`
    /// below, which pins that property) and deliberately a cool steel-blue
    /// rather than reusing `accent`'s near-identical warm amber/gold — a
    /// future selection/focus highlight on a hotbar slot using `accent`
    /// would otherwise visually blend with an active cooldown sweep.
    pub cooldown_overlay: Color,
    /// Opaque dark fill drawn as the BOTTOM layer of every hotbar skill slot
    /// (`hotbar.rs`'s `SkillSlotBackground`), UNDER the ornate
    /// `skill_slot_border.png` frame art. BL-82 HUD polish round 7: the frame
    /// asset is an ornate gothic ring whose opaque silhouette fills only
    /// ~50% of its own bounding box and reaches the box edge on merely ~2% of
    /// each edge (median 31px inset) — so two adjacent slots' frames NEVER
    /// touch no matter how tight the crop or how small
    /// (`hotbar.rs::HOTBAR_SLOT_GAP_PX = 0.0`) the gap, and the game world
    /// showed through every concave notch AND the transparent centre, reading
    /// as a wide gap between slots (Matías's `captura11.png`; rounds 5/6
    /// couldn't fix it by crop/gap tuning because the gap is intrinsic to the
    /// asset's shape, not a measurement error). Filling the whole square box
    /// with this OPAQUE dark first makes adjacent slot boxes touch flush and
    /// turns every notch/centre into continuous dark rather than grass —
    /// exactly how `hud-ejemplo-2.png`'s reference slots (solid dark squares
    /// with a thin frame) read flush. Kept a near-black warm tone matching
    /// the frame art's own darkest metal (sampled ~(19,18,16)/255) so the
    /// fill and frame read as one piece; opaque (`alpha = 1.0`) so no game
    /// world bleeds through — see
    /// `slot_bg_is_opaque_and_dark_enough_to_hide_the_game_world_behind_a_slot`.
    pub slot_bg: Color,
}

impl Default for HudPalette {
    fn default() -> Self {
        Self {
            panel_bg: Color::srgba(0.06, 0.05, 0.08, 0.82),
            panel_border: Color::srgba(0.55, 0.45, 0.25, 0.9),
            text: Color::srgba(0.92, 0.90, 0.85, 1.0),
            text_muted: Color::srgba(0.65, 0.62, 0.58, 1.0),
            accent: Color::srgba(0.85, 0.65, 0.25, 1.0),
            health: Color::srgba(0.78, 0.15, 0.15, 1.0),
            health_bg: Color::srgba(0.25, 0.05, 0.05, 0.9),
            energy: Color::srgba(0.20, 0.55, 0.85, 1.0),
            energy_bg: Color::srgba(0.05, 0.15, 0.25, 0.9),
            poise: Color::srgba(0.85, 0.75, 0.25, 1.0),
            poise_bg: Color::srgba(0.25, 0.20, 0.05, 0.9),
            xp: Color::srgba(0.55, 0.30, 0.75, 1.0),
            xp_bg: Color::srgba(0.15, 0.08, 0.20, 0.9),
            combo: Color::srgba(0.95, 0.55, 0.15, 1.0),
            buff_good: Color::srgba(0.30, 0.80, 0.35, 1.0),
            buff_bad: Color::srgba(0.80, 0.25, 0.25, 1.0),
            danger: Color::srgba(0.85, 0.10, 0.10, 0.55),
            cooldown_overlay: Color::srgba(0.35, 0.55, 0.75, 0.6),
            slot_bg: Color::srgba(0.075, 0.070, 0.060, 1.0),
        }
    }
}

/// Named spacing roles (px), replacing per-widget magic numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudSpacing {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
}

impl Default for HudSpacing {
    fn default() -> Self {
        Self {
            xs: 4.0,
            sm: 8.0,
            md: 16.0,
            lg: 24.0,
        }
    }
}

impl HudSpacing {
    #[must_use]
    pub fn xs_px(self) -> Val { Val::Px(self.xs) }

    #[must_use]
    pub fn sm_px(self) -> Val { Val::Px(self.sm) }

    #[must_use]
    pub fn md_px(self) -> Val { Val::Px(self.md) }

    #[must_use]
    pub fn lg_px(self) -> Val { Val::Px(self.lg) }
}

/// Named corner-radius roles (px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudRadius {
    pub sm: f32,
    pub md: f32,
}

impl Default for HudRadius {
    fn default() -> Self { Self { sm: 3.0, md: 6.0 } }
}

/// The one theme resource every widget in this crate reads.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct HudTheme {
    pub palette: HudPalette,
    pub spacing: HudSpacing,
    pub radius: HudRadius,
}

/// The two real, frozen shared font families this v1 theme wires up (BL-82
/// EM-5.1 T56.2/T56.3): `Alkhemikal.ttf` for display/title text (the legacy
/// "alkhemi" family) and `OpenSans-Regular.ttf` for body copy (the legacy
/// "universal" family). Both already ship under `assets/voxygen/font/` — no
/// new asset. Legacy also names a third "cyri" body family; this v1 theme
/// intentionally ships with two real, correctly-attributed font roles rather
/// than guessing which frozen `.ttf` legacy's in-house "cyri" codename maps
/// to — a follow-up can add the third role once that mapping is confirmed.
#[derive(Resource, Debug, Clone)]
pub struct HudFonts {
    /// Display/title role (headers, level-up flourish).
    pub title: Handle<Font>,
    /// Body role (buttons, labels, tooltips, chat — most HUD text).
    pub body: Handle<Font>,
}

impl HudFonts {
    /// Loads the two font handles via the [`AssetServer`] (hot-reloadable in
    /// dev, same as every other asset this workspace loads).
    pub fn load(asset_server: &AssetServer) -> Self {
        Self {
            title: asset_server.load("voxygen/font/Alkhemikal.ttf"),
            body: asset_server.load("voxygen/font/OpenSans-Regular.ttf"),
        }
    }
}

/// `Startup` system inserting [`HudTheme::default()`] and [`HudFonts`]
/// (loaded via the real [`AssetServer`]). `pub` (not `pub(crate)`): a
/// downstream screen plugin (e.g. EM-5.2's `CombatHudViewPlugin`) that also
/// spawns widgets at `Startup` needs to `.after(init_theme)` its own spawn
/// system, so the theme/font resources are guaranteed to exist first.
pub fn init_theme(mut commands: bevy::ecs::system::Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(HudTheme::default());
    commands.insert_resource(HudFonts::load(&asset_server));
}

#[cfg(test)]
mod tests {
    use bevy::color::Alpha as _;

    use super::*;

    /// The default palette must not be a see-through no-op (every alpha > 0)
    /// — a cheap regression guard against an accidental `Color::NONE`-shaped
    /// default that would render an invisible HUD.
    #[test]
    fn default_palette_colours_are_visible() {
        let palette = HudPalette::default();
        for color in [
            palette.panel_bg,
            palette.health,
            palette.energy,
            palette.poise,
            palette.xp,
        ] {
            assert!(
                color.alpha() > 0.0,
                "theme colour must have non-zero alpha: {color:?}"
            );
        }
    }

    /// Spacing tokens resolve to the expected pixel values (a change-once,
    /// break-everywhere-visibly guard for T56.2's "one token change restyles
    /// the gallery" acceptance bar).
    #[test]
    fn spacing_tokens_resolve_to_px() {
        let spacing = HudSpacing::default();
        assert_eq!(spacing.md_px(), Val::Px(spacing.md));
    }

    /// BL-82 EM-5.17 Phase 2 follow-up (the reported "cooldown sweep doesn't
    /// show" bug): `cooldown_overlay` must remain visible once ACTUALLY
    /// composited over a near-black background — the real on-disk
    /// `skill_slot_border.png` this colour is layered on top of in
    /// `hotbar.rs` samples at an average RGB of roughly (19, 18, 16)/255,
    /// luma ~0.075 (confirmed directly, not assumed). This test composites
    /// the palette colour over that same near-black luma via the standard
    /// alpha-over formula, not just a raw luma check on the foreground
    /// colour alone — a colour with bright RGB channels but a near-zero
    /// alpha would be exactly as invisible as the original bug's
    /// `Color::srgba(0.0, 0.0, 0.0, 0.7)` literal (alpha was never the
    /// problem there, but a future retune could make it one), so both
    /// channels must be covered for this guard to be worth anything.
    #[test]
    fn cooldown_overlay_colour_has_enough_luminance_to_read_over_a_near_black_background() {
        let color = HudPalette::default().cooldown_overlay.to_srgba();
        // Rec. 601 luma approximation is plenty precise for a UI-contrast guard.
        let fg_luma = 0.299 * color.red + 0.587 * color.green + 0.114 * color.blue;
        // The real background this colour composites over in-game (see the
        // field's own doc comment for the sampled value this approximates).
        const NEAR_BLACK_BG_LUMA: f32 = 0.075;
        let composited_luma = color.alpha * fg_luma + (1.0 - color.alpha) * NEAR_BLACK_BG_LUMA;
        assert!(
            composited_luma > 0.2,
            "cooldown overlay colour is too dark/transparent to read over the hotbar's near-black \
             slot-border art once actually composited: {color:?} (composited luma \
             {composited_luma})"
        );
    }

    /// BL-82 HUD polish round 7 (Matías's `captura11.png` "visible gap between
    /// skill slots" report, 7th round): `slot_bg` is the flush-look fix —
    /// `skill_slot_border.png` is an ornate ring that fills only ~50% of its
    /// own bounding box and reaches its box edge on merely ~2% of each edge,
    /// so no crop or `HOTBAR_SLOT_GAP_PX` value can make two adjacent frames
    /// touch; `hotbar.rs` fills the whole slot box with this OPAQUE dark first
    /// so the boxes touch flush and the game world stops showing through the
    /// frame's concave notches and transparent centre. For that to work the
    /// role MUST be (a) fully opaque — any translucency lets the grass bleed
    /// straight back through, reintroducing the exact bug — and (b) genuinely
    /// dark, so it reads as one piece with the near-black frame metal rather
    /// than as a bright plate behind it.
    #[test]
    fn slot_bg_is_opaque_and_dark_enough_to_hide_the_game_world_behind_a_slot() {
        let color = HudPalette::default().slot_bg.to_srgba();
        assert_eq!(
            color.alpha, 1.0,
            "slot_bg must be fully opaque, else the game world bleeds through the frame's concave \
             notches/centre and the round-7 flush fix regresses: {color:?}"
        );
        let luma = 0.299 * color.red + 0.587 * color.green + 0.114 * color.blue;
        assert!(
            luma < 0.15,
            "slot_bg should be near-black to read as one piece with the frame metal: {color:?} \
             (luma {luma})"
        );
    }
}
