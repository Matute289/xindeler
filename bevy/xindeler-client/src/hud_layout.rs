//! BL-82 EM-5.17 Phase 2 — shared bottom-center HUD cluster geometry.
//!
//! `combat_hud.rs` (the 3 resource orbs) and `hotbar.rs` (the 2-piece action
//! bar background + ability slots) are two INDEPENDENT `Startup`-spawn
//! plugins, but per the design spec (§3.1) they must render as ONE visually
//! contiguous bottom-center row:
//! `[ Health orb ] [ action_bar_bg_left ] [ Stamina orb ] [ action_bar_bg_right
//! ] [ Mana orb ]`. Rather than have each file invent its own magic-number
//! offsets (guaranteed to drift out of alignment the first time either file's
//! constants change), every element's horizontal placement is computed ONCE
//! here, from the same arithmetic, and both files position their own pieces
//! purely via absolute `left: 50% + margin-left: <this module's offset>` (the
//! same percent-plus-negative-margin centring idiom `combat_hud.rs`'s own
//! `Crosshair` node already uses) — no shared parent entity is needed across
//! the two plugins, so spawn-order between them never matters.
//!
//! ## A real asset gap found while wiring this phase — RESOLVED, see below
//! *(Historical note, BL-82 EM-5.17 Phase 2)* This doc comment originally
//! flagged every orb "frame"/"border" PNG as a plain opaque RGB PNG with no
//! alpha channel at all, meaning the frame would fully occlude the liquid
//! fill underneath it. **That is no longer true of the assets on disk** —
//! re-verified directly (BL-82 EM-5.17 Phase 0 follow-up, Matías's HUD
//! art-alignment report): `orb_frame_angel.png`/`orb_frame_cuthulhu.png`/
//! `orb_frame_stamina.png` and the `*_liquid.png` files are all real 8-bit
//! RGBA PNGs today, with a genuine transparent circular cutout in the frame
//! art and a genuine circular liquid blob in the fill art (the art pack was
//! re-cut at some point after this comment was written). The remaining real
//! issue this module now fixes is SIZING, not occlusion: every one of these
//! files is a wide `1408×768`-ish canvas with the actual circular art
//! centred in a narrower sub-region (padding left/right for the gargoyle-
//! wing frame extensions) — see [`ORB_SOURCE_CROP`]'s own doc comment for
//! the exact measured bounding boxes and the crop that fixes it.
use bevy::{math::Rect, ui::Val};

/// Size (px, both axes) of each of the three resource orbs — spec §3.1's
/// "~160×160". The source `orb_frame_*.png`/`*_liquid.png` files are a wider
/// `1408×768`-ish canvas (not literally square, see the module doc comment's
/// asset-gap note) — [`ORB_SOURCE_CROP`] fixes the squash this used to cause
/// by cropping a square sub-region of the source BEFORE it's stretched onto
/// this square box.
pub const ORB_SIZE_PX: f32 = 160.0;

/// BL-82 EM-5.17 Phase 0 follow-up (Matías's HUD art-alignment report) — the
/// SQUARE pixel-space sub-rect of the HUD-D4 orb pack's native canvas that
/// every `spawn_orb_bar` call for the three resource orbs passes as
/// `xindeler_ui::bar::spawn_orb_bar`'s `source_crop` parameter.
///
/// Verified directly against the actual on-disk PNGs (all `1408×768`,
/// except `orb_frame_cuthulhu.png` at `1407×768` — a 1px rounding
/// difference, negligible): every orb frame/liquid file is a WIDE canvas
/// where the real circular art sits centred in a narrower square-ish
/// sub-region, not spanning the full width — the artist left transparent
/// padding left/right for the gargoyle-wing frame extensions. Measured
/// bounding boxes: the liquid art's own opaque content is
/// `x[354,1055] y[28,727]` (≈701×699px); the frame's enclosed circular
/// cutout is `x[523,896] y[197,577]` (≈372×380px). Both are centred within a
/// few px of the canvas's own horizontal centre (`x≈704-710` of ~1408) —
/// close enough (a handful of px on a ~700px-wide circle) that ONE shared
/// crop rect serves the frame, angel/stamina/cuthulhu variants, and all four
/// liquid variants alike (this doesn't need to be pixel-exact — see
/// `spawn_orb_bar`'s own doc comment).
///
/// This crop keeps the FULL canvas height (`0..768`, already tight around
/// the circle) and crops the width down to match it (`768px` wide, centred
/// on `x≈704`, i.e. `[320,1088]`) — producing a genuinely SQUARE sub-rect.
/// Stretching a square crop onto `spawn_orb_bar`'s square
/// [`ORB_SIZE_PX`]×[`ORB_SIZE_PX`] box distorts nothing (uniform scale);
/// stretching the whole non-square canvas onto that same square box (the
/// pre-fix behaviour) squashed the circle into an ellipse.
pub const ORB_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(320.0, 0.0),
    max: bevy::math::Vec2::new(1088.0, 768.0),
};

/// Height (px) of each action-bar-half background. Matches [`ORB_SIZE_PX`]
/// so the whole cluster's bottom edge lines up in one row.
pub const ACTION_BAR_HALF_HEIGHT_PX: f32 = ORB_SIZE_PX;

/// Width (px) of each action-bar-half background, derived from
/// `action_bar_bg_left.png`/`action_bar_bg_right.png`'s REAL on-disk aspect
/// ratio (`1380×752`/`1379×752`, i.e. ≈1.835:1) at
/// [`ACTION_BAR_HALF_HEIGHT_PX`] tall — keeps the ornate frame art undistorted
/// instead of an arbitrary guessed width.
pub const ACTION_BAR_HALF_WIDTH_PX: f32 = ACTION_BAR_HALF_HEIGHT_PX * (1380.0 / 752.0);

/// Horizontal gap (px) between adjacent cluster pieces. NEGATIVE (a small
/// overlap): every piece's source PNG has a sizeable fully-opaque black
/// border padding around its ornate art (see the module doc comment) — a
/// small overlap keeps that padding from reading as a visible seam of double
/// black between pieces. Tuned by eye against the Phase 2 smoke screenshot;
/// revisit if a future asset re-cut changes the padding.
pub const CLUSTER_GAP_PX: f32 = -32.0;

/// Distance (px) from the viewport's bottom edge to the bottom of the whole
/// orb/action-bar row.
pub const CLUSTER_BOTTOM_PX: f32 = 20.0;

/// Gap (px) between the top of the orb/action-bar row and the XP-bar+level
/// cluster sitting just above it (spec §3.1's "between/above the orbs and
/// action bar").
pub const XP_CLUSTER_GAP_PX: f32 = 6.0;

/// Total width (px) of the "core" action-bar span — the two background
/// halves plus the centre Stamina orb, EXCLUDING the two outer (Health/Mana)
/// orbs. The XP bar + level readout are centred on this span, matching the
/// HUD-D4 reference's own narrower XP strip (it sits above the action bar,
/// not edge-to-edge across the full orb-to-orb cluster).
pub const ACTION_BAR_TOTAL_WIDTH_PX: f32 =
    2.0 * ACTION_BAR_HALF_WIDTH_PX + ORB_SIZE_PX + 2.0 * CLUSTER_GAP_PX;

/// Every cluster piece's horizontal offset (px), relative to the viewport's
/// own horizontal centre, of that piece's OWN LEFT edge — i.e. exactly the
/// value to hand to `margin.left` on a `Node` that already has
/// `left: Val::Percent(50.0)` (the project's established centring idiom).
/// Computed once, left-to-right, from the centre Stamina orb outward, so
/// both `combat_hud.rs` and `hotbar.rs` derive their own pieces' placement
/// from the exact same arithmetic.
pub struct ClusterOffsets {
    pub health_orb_left: f32,
    pub action_bar_left_half_left: f32,
    pub stamina_orb_left: f32,
    pub action_bar_right_half_left: f32,
    pub mana_orb_left: f32,
}

/// The single, shared instance of [`ClusterOffsets`] — both HUD plugins read
/// this directly rather than recomputing the arithmetic themselves.
pub const CLUSTER: ClusterOffsets = {
    let stamina_orb_left = -ORB_SIZE_PX / 2.0;
    let stamina_orb_right = ORB_SIZE_PX / 2.0;
    let action_bar_right_half_left = stamina_orb_right + CLUSTER_GAP_PX;
    let action_bar_right_half_right = action_bar_right_half_left + ACTION_BAR_HALF_WIDTH_PX;
    let mana_orb_left = action_bar_right_half_right + CLUSTER_GAP_PX;
    let action_bar_left_half_right = stamina_orb_left - CLUSTER_GAP_PX;
    let action_bar_left_half_left = action_bar_left_half_right - ACTION_BAR_HALF_WIDTH_PX;
    let health_orb_left = action_bar_left_half_left - CLUSTER_GAP_PX - ORB_SIZE_PX;

    ClusterOffsets {
        health_orb_left,
        action_bar_left_half_left,
        stamina_orb_left,
        action_bar_right_half_left,
        mana_orb_left,
    }
};

/// Convenience: `left: Val::Percent(50.0)` is always paired with a
/// `margin.left` offset in this module's idiom — this just names that
/// constant so call sites don't repeat the literal.
pub const CENTER_LEFT: Val = Val::Percent(50.0);

#[cfg(test)]
mod tests {
    use super::*;

    /// The cluster is laid out symmetrically around screen centre: the
    /// Stamina orb (the centrepiece) straddles `x = 0` exactly, and the two
    /// action-bar halves + outer orbs mirror each other in width — a
    /// regression guard against a future constant tweak silently breaking
    /// the "one contiguous row" contract this module exists to enforce.
    #[test]
    fn cluster_is_symmetric_around_centre() {
        assert_eq!(CLUSTER.stamina_orb_left, -ORB_SIZE_PX / 2.0);

        let left_bar_width =
            CLUSTER.stamina_orb_left - CLUSTER_GAP_PX - CLUSTER.action_bar_left_half_left;
        let right_bar_width =
            CLUSTER.mana_orb_left - CLUSTER_GAP_PX - CLUSTER.action_bar_right_half_left;
        assert!((left_bar_width - right_bar_width).abs() < f32::EPSILON);
        assert!((left_bar_width - ACTION_BAR_HALF_WIDTH_PX).abs() < f32::EPSILON);

        // Health orb's right edge must sit exactly `CLUSTER_GAP_PX` before
        // the left bar half's left edge (and symmetrically for Mana/right).
        let health_orb_right = CLUSTER.health_orb_left + ORB_SIZE_PX;
        assert!(
            (health_orb_right + CLUSTER_GAP_PX - CLUSTER.action_bar_left_half_left).abs() < 0.01
        );
    }

    /// The XP/level cluster's width matches the "core" action-bar span
    /// (both halves + the centre orb, not the two outer orbs) — pins the
    /// value both `combat_hud.rs`'s XP-cluster container and any future
    /// caller rely on.
    #[test]
    fn action_bar_total_width_spans_both_halves_and_stamina_orb() {
        let expected = 2.0 * ACTION_BAR_HALF_WIDTH_PX + ORB_SIZE_PX + 2.0 * CLUSTER_GAP_PX;
        assert!((ACTION_BAR_TOTAL_WIDTH_PX - expected).abs() < f32::EPSILON);
    }
}
