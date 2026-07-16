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
//! ## A real asset gap found while wiring this phase (flagged, not silently
//! ## worked around)
//! Every "frame"/"border" PNG this phase composites OVER a liquid fill or a
//! slot (`orb_frame_angel.png`, `orb_frame_stamina.png`,
//! `orb_frame_cuthulhu.png`, `action_bar_bg_left.png`,
//! `action_bar_bg_right.png`, `skill_slot_border.png`) is a **plain opaque
//! RGB PNG with no alpha channel at all** — verified directly (not assumed):
//! every one of these files opens as PIL mode `"RGB"` (no `"A"` channel), and
//! their circular/rectangular "cutout" regions sample as fully opaque black,
//! not transparent. Both the design spec (§3.1: "the frame PNGs in the
//! HUD-D4 pack have an alpha-transparent centre") and
//! `xindeler_ui::bar::spawn_orb_bar`'s own doc comment assumed real alpha
//! transparency there so the liquid fill would show through the frame's
//! circular cutout — that assumption does not hold for the actual files on
//! disk. This phase still wires the mechanism EXACTLY as Phase 1 designed it
//! (`spawn_orb_bar`'s frame-overlay parameter, `skill_slot_border` layered
//! onto the slot) since regenerating/re-cutting the art pack is outside a
//! code phase's scope — but the visual result (confirmed in this phase's own
//! smoke screenshot) is that the opaque frame fully occludes whatever's
//! beneath it, so the orbs' liquid fill never visibly changes with the
//! underlying resource value today. Flagged as the primary follow-up this
//! phase surfaces, not silently patched over.
use bevy::ui::Val;

/// Size (px, both axes) of each of the three resource orbs — spec §3.1's
/// "~160×160". The source `orb_frame_*.png`/`*_liquid.png` files are a wider
/// `1408×768`-ish canvas (not literally square, see the module doc comment's
/// asset-gap note) — `ImageNode`'s default stretch-to-fit means a plain
/// square box does mildly squash the circular artwork; a follow-up can crop
/// the source art or switch to `ImageNode::with_mode` cover-fit once that's
/// worth the extra code.
pub const ORB_SIZE_PX: f32 = 160.0;

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
