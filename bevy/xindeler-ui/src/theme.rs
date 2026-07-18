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
    /// The "this slot is EMPTY" stone cue drawn in the CENTRE of an unfilled
    /// hotbar skill slot (`hotbar.rs`'s `SkillSlotBackground`), UNDER the
    /// ornate `skill_slot_border.png` frame ring.
    ///
    /// BL-82 HUD overhaul (this pass) replaces round 7's opaque near-black
    /// full-box `slot_bg` fill. Round 7 filled the WHOLE slot box opaque so
    /// adjacent frames read flush — but Matías found the resulting solid black
    /// boxes too heavy and asked for transparent slots back (the game world
    /// showing through the ring's notches is now acceptable), with only the
    /// specifically-EMPTY slot state carrying a subtle grey stone treatment in
    /// its centre so "is this slot empty?" still reads at a glance without a
    /// heavy flat plate. So this role is now: (a) a mid GREY stone tone, not
    /// near-black — it must read as a deliberate "empty socket" cue, distinct
    /// from the frame's own dark metal (see
    /// `slot_empty_stone_is_a_readable_mid_grey_not_near_black`); (b) only
    /// partially opaque, since `hotbar.rs` insets it to the ring's centre hole
    /// (not the full box) and hides it entirely once the slot holds an
    /// ability — the ring's own notches stay see-through either way.
    pub slot_empty_stone: Color,
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
            slot_empty_stone: Color::srgba(0.34, 0.33, 0.31, 0.72),
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

    /// BL-82 HUD overhaul: `slot_empty_stone` replaces round 7's opaque
    /// near-black full-box `slot_bg`. It is the "empty socket" cue drawn in an
    /// unfilled slot's centre hole (`hotbar.rs`'s `SkillSlotBackground`), so it
    /// must read as a deliberate MID-GREY stone, NOT the frame's own near-black
    /// metal (which is what round 7's dark fill looked like, and which Matías
    /// rejected as too heavy). It is also partially translucent — `hotbar.rs`
    /// insets it to the centre and hides it on filled slots, so it never needs
    /// to fully occlude the game world the way round 7's flush fill did.
    #[test]
    fn slot_empty_stone_is_a_readable_mid_grey_not_near_black() {
        let color = HudPalette::default().slot_empty_stone.to_srgba();
        let luma = 0.299 * color.red + 0.587 * color.green + 0.114 * color.blue;
        assert!(
            luma > 0.2,
            "slot_empty_stone must be a readable mid grey, distinct from the frame's near-black \
             metal, so an empty slot's stone cue is visible at a glance: {color:?} (luma {luma})"
        );
        assert!(
            luma < 0.6,
            "slot_empty_stone should still read as dim STONE, not a bright plate: {color:?} (luma \
             {luma})"
        );
        assert!(
            color.alpha > 0.0 && color.alpha < 1.0,
            "slot_empty_stone is a partial-cover centre cue, not an opaque full-box fill: \
             {color:?}"
        );
    }
}
