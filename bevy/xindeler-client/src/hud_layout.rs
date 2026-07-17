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
/// `xindeler_ui::bar::spawn_orb_bar`'s `fill_source_crop` parameter.
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
///
/// This is the crop for the LIQUID (`*_liquid.png`) images specifically —
/// see [`ANGEL_FRAME_SOURCE_CROP`]/[`CUTHULHU_FRAME_SOURCE_CROP`]/
/// [`STAMINA_FRAME_SOURCE_CROP`] for the frame's own PER-VARIANT crops
/// (BL-82 orb crop round 2 — see those constants' doc comments for why a
/// single shared frame crop stopped working). This liquid crop stays SHARED
/// across all four liquid variants (health/mana/mana2/stamina) — re-verified
/// round 2: all four `*_liquid.png` files measure an IDENTICAL opaque bbox
/// (`x[353,1057] y[28,728]`), so unlike the frame crop there is no per-
/// variant difference to tune here. It is already as tight as it can be: the
/// liquid's own opaque content spans the full `699`–`700px` of the canvas's
/// `768px` height (`y[28,727]`, re-verified directly against the on-disk
/// PNGs), so there is no further square sub-region available within this
/// canvas that both stays square AND keeps the whole liquid circle — any
/// tighter crop would clip real liquid pixels. Making the liquid render
/// SMALLER (BL-82 EM-5.17 Phase 0 second follow-up, see
/// [`ANGEL_LIQUID_INSET_PX`]'s doc comment) therefore has to come from the
/// per-variant `*_LIQUID_INSET_PX` constants instead of a tighter crop here.
pub const ORB_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(320.0, 0.0),
    max: bevy::math::Vec2::new(1088.0, 768.0),
};

/// BL-82 EM-5.17 Phase 0 SECOND follow-up (Matías's live in-game report on
/// `record20.mov`: the liquid circles sit visibly SMALLER than the frame's
/// own circular window, leaving a dark ring of frame material between the
/// liquid's edge and the frame — even though [`ORB_SOURCE_CROP`] already
/// fixed the earlier "squashed ellipse" bug). Root cause, re-measured
/// directly against the on-disk `orb_frame_angel.png`/`orb_frame_cuthulhu.png`/
/// `orb_frame_stamina.png` (via a connected-component labelling of each
/// PNG's alpha channel — a single-scanline probe gets fooled by the
/// cuthulhu frame's asymmetric wing/tentacle art, which has its own internal
/// opaque/transparent transitions on the same row/column as the real hole):
/// [`spawn_orb_bar`](xindeler_ui::bar::spawn_orb_bar) used to stretch ONE
/// SHARED crop onto both the liquid and the frame images. Since both then
/// scale onto the exact same `width_px`×`height_px` box by the exact same
/// factor, a shared crop can only ever preserve the two images' SOURCE pixel
/// ratio — it can never change how big the liquid renders RELATIVE to the
/// frame's own hole, no matter which square sub-region is chosen. A crop for
/// the FRAME ALONE, tighter around the hole than [`ORB_SOURCE_CROP`], makes
/// the hole occupy more of the shared box independent of the liquid's own
/// scale.
///
/// ## BL-82 orb crop round 2 (per-variant tuning) — this went TOO tight
/// The Phase 0 second-follow-up fix above originally landed as ONE shared
/// `ORB_FRAME_SOURCE_CROP` (`716×716`, centred `(710,387)`) tight enough to
/// markedly enlarge the hole — but Matías's round-2 report (`foto1.png`)
/// showed the angel/cuthulhu frames' own decorative statue/tentacle art now
/// visibly CUT OFF by that square window (opposite-direction regression from
/// the original "liquid smaller than the hole" bug: the shared crop had
/// swung from too loose to too tight). Re-measured directly (connected-
/// component analysis of each frame PNG's alpha channel, `scipy.ndimage.
/// label`, `alpha>10` threshold) two things per variant: the ENCLOSED hole
/// (the transparent component that does NOT touch the canvas border — the
/// real liquid window, immune to the wing art's own internal transparent
/// gaps) and the full decorative-art extent. The art extent
/// (angel `879px` wide, cuthulhu `1165px` wide — genuinely spans most of the
/// `1408px`-ish canvas, not a thin bleed of stray anti-aliasing: column-
/// density profiling confirms substantial opaque coverage all the way out)
/// physically CANNOT fit in a square crop bounded by the canvas's own `768px`
/// height, so zero clipping into every last decorative pixel is geometrically
/// impossible for a square orb — the widest available square is exactly
/// `768×768` (the full canvas height). The fix: use that FULL `768×768`
/// square (not the narrower `716×716`) for each variant, centred
/// horizontally on that VARIANT'S OWN measured hole centre (not the art's
/// centre, and not one shared centre) so the hole still lines up with
/// [`ORB_SOURCE_CROP`]'s own centred liquid circle:
/// - angel: hole `(525,197)-(897,578)`, centre `x=711.0` → `x[327.0,1095.0]`
/// - cuthulhu: hole `(529,199)-(894,577)`, centre `x=711.5` → `x[327.5,1095.5]`
/// - stamina: hole `(524,202)-(895,569)`, centre `x=709.5` → `x[325.5,1093.5]`
///
/// (all `y[0,768]`, the full canvas height, same as [`ORB_SOURCE_CROP`]).
/// This maximizes how much of each variant's own decorative art survives the
/// square crop while keeping hole/liquid alignment exact — verified via
/// direct pixel-level compositing (crop+resize+alpha-overlay, mirroring
/// `spawn_orb_bar`'s own crop→`NodeImageMode::Stretch` pipeline) that the
/// full statue/tentacle silhouettes render uncut. See
/// [`ANGEL_LIQUID_INSET_PX`]/[`CUTHULHU_LIQUID_INSET_PX`]/
/// [`STAMINA_LIQUID_INSET_PX`] for the companion per-variant inset fix this
/// widened crop requires (a looser frame crop shrinks the hole's SHARE of the
/// rendered box, so the liquid needs a bigger inset to still fit inside it
/// without visibly spilling past the ring).
pub const ANGEL_FRAME_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(327.0, 0.0),
    max: bevy::math::Vec2::new(1095.0, 768.0),
};

/// Cuthulhu (mana orb) variant of [`ANGEL_FRAME_SOURCE_CROP`] — see that
/// constant's doc comment for the round-2 per-variant crop rationale. Centred
/// on cuthulhu's own measured hole centre (`x=711.5`); `cuthulhu`'s canvas is
/// `1407px` wide (1px narrower than the other two, a negligible artist-export
/// rounding difference), still comfortably wide enough for this crop's
/// `x1=1095.5`.
pub const CUTHULHU_FRAME_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(327.5, 0.0),
    max: bevy::math::Vec2::new(1095.5, 768.0),
};

/// Stamina (centre orb) variant of [`ANGEL_FRAME_SOURCE_CROP`] — see that
/// constant's doc comment for the round-2 per-variant crop rationale. Centred
/// on stamina's own measured hole centre (`x=709.5`).
pub const STAMINA_FRAME_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(325.5, 0.0),
    max: bevy::math::Vec2::new(1093.5, 768.0),
};

/// BL-82 EM-5.17 Phase 0 second follow-up — companion to
/// [`ANGEL_FRAME_SOURCE_CROP`]: [`xindeler_ui::bar::spawn_orb_bar`]'s
/// `liquid_inset_px` parameter for the health (angel) orb. [`ORB_SOURCE_CROP`]
/// is already as tight as the liquid PNGs' own canvas allows (see its doc
/// comment), so the liquid can't be shrunk any further via cropping — this
/// insets the liquid's RENDERED box a few px on every side instead, centred
/// within the orb's full [`ORB_SIZE_PX`]×[`ORB_SIZE_PX`] box, so its circular
/// edge sits comfortably inside the frame's hole with deliberate slack rather
/// than exactly flush with it — flush-fit only holds at one exact scale; a
/// few px of margin means sub-pixel rounding at a different UI-scale factor
/// can never read as the liquid overlapping the frame's ring.
///
/// ## BL-82 orb crop round 2 — re-tuned per variant, no longer one shared value
/// The original Phase 0 second follow-up used ONE shared `LIQUID_INSET_PX =
/// 6.0`, tuned against the since-widened-away `716×716` frame crop. Widening
/// the frame crop to `768×768` (see [`ANGEL_FRAME_SOURCE_CROP`]'s doc
/// comment) shrinks the hole's share of the rendered box, so a `6px` inset
/// now lets the liquid visibly spill past the ring's outer silhouette on
/// stamina especially (Matías's round-2 report: "the center stamina orb's
/// liquid circle is still visibly larger than its frame"). Re-tuned directly
/// via pixel-mask overlap analysis (not eyeballing): for each variant,
/// composited the liquid's own alpha mask against the frame's own alpha mask
/// at the real render resolution (mirroring `spawn_orb_bar`'s crop→resize→
/// composite pipeline) and swept `inset` to find the smallest value at which
/// the VISIBLE liquid (the part not hidden under the frame's own opaque ring
/// material, which the frame draws on top of and therefore occludes) no
/// longer has any pixels outside the enclosed hole — i.e. the smallest inset
/// with zero genuine spill visible past the ring's own silhouette — then
/// added a `+3px` safety margin (same "deliberate slack" reasoning as the
/// original Phase 0 tuning). Zero-spill insets measured: angel `11px`,
/// cuthulhu `17px` (a couple stray anti-aliasing px persist past this due to
/// the wing art's irregular ring edge — negligible), stamina `17px` — angel's
/// hole is proportionally larger (relative to its own frame crop) than the
/// other two, hence its lower value; this is exactly why round 2 goes
/// per-variant instead of trying a second shared constant.
pub const ANGEL_LIQUID_INSET_PX: f32 = 14.0;

/// Cuthulhu (mana orb) variant of [`ANGEL_LIQUID_INSET_PX`] — see that
/// constant's doc comment for the round-2 re-tuning methodology.
pub const CUTHULHU_LIQUID_INSET_PX: f32 = 20.0;

/// Stamina (centre orb) variant of [`ANGEL_LIQUID_INSET_PX`] — see that
/// constant's doc comment for the round-2 re-tuning methodology. This is the
/// orb Matías's round-2 report called out by name ("the center stamina orb's
/// liquid circle is still visibly larger than its frame").
pub const STAMINA_LIQUID_INSET_PX: f32 = 20.0;

/// Height (px) of each action-bar-half background. Matches [`ORB_SIZE_PX`]
/// so the whole cluster's bottom edge lines up in one row.
pub const ACTION_BAR_HALF_HEIGHT_PX: f32 = ORB_SIZE_PX;

/// Width (px) of each action-bar-half background, derived from
/// `action_bar_bg_left.png`/`action_bar_bg_right.png`'s REAL on-disk aspect
/// ratio (`1380×752`/`1379×752`, i.e. ≈1.835:1) at
/// [`ACTION_BAR_HALF_HEIGHT_PX`] tall — keeps the ornate frame art undistorted
/// instead of an arbitrary guessed width.
///
/// BL-82 EM-5.17 Phase 0 review follow-up (Matías: the bronze-framed action-
/// bar pieces themselves should shrink a bit, distinct from the orb/gap
/// tuning PR #126 already did — "more breathing room in the overall HUD
/// row"). [`ACTION_BAR_WIDTH_TRIM`] scales the pure aspect-derived width down
/// a modest 12% — a "small trim, not a redesign" per Matías's own framing,
/// tuned by eye against `--smoke-screenshot` output. The two pieces' 3+2
/// ability slots (`SLOT_SIZE_PX` each, `hotbar.rs`) stay comfortably clear of
/// the trimmed width, so nothing overflows. A pure width-only scale (height
/// untouched, since [`ACTION_BAR_HALF_HEIGHT_PX`] still has to match
/// [`ORB_SIZE_PX`] for the row's shared bottom edge) does very slightly
/// squash the art off its native aspect ratio — imperceptible at this modest
/// a trim, and the far smaller evil compared to either shrinking the row's
/// height (breaking the orb alignment) or leaving no breathing room at all.
pub const ACTION_BAR_WIDTH_TRIM: f32 = 0.88;
pub const ACTION_BAR_HALF_WIDTH_PX: f32 =
    ACTION_BAR_HALF_HEIGHT_PX * (1380.0 / 752.0) * ACTION_BAR_WIDTH_TRIM;

/// Horizontal gap (px) between adjacent cluster pieces — health orb / left
/// action-bar half / stamina orb / right action-bar half / mana orb. Was a
/// NEGATIVE (`-32.0`) small overlap: every piece's source PNG has a sizeable
/// fully-opaque black border padding around its ornate art (see the module
/// doc comment), so pieces were pulled together to keep that padding from
/// reading as a visible seam of double black between pieces. BL-82 EM-5.17
/// Phase 0 THIRD follow-up (Matías's `record20.mov` review: "the whole row
/// has zero breathing room, everything is pressed together") re-tunes this
/// to a small POSITIVE gap instead — re-verified by eye against a live smoke
/// screenshot that the exposed sliver of each piece's own opaque padding
/// reads as a clean dark seam against the row's already-dark backdrop, not
/// as a mismatched notch. Kept modest (a few px, not a wide gap) per
/// Matías's own "small gap/margin... not a large redesign" framing — this is
/// still THE single shared gap every adjacent pair in the row uses, so the
/// row stays visually contiguous, just no longer touching. Revisit if a
/// future asset re-cut changes the padding.
pub const CLUSTER_GAP_PX: f32 = 4.0;

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

    /// Regression guard for [`ACTION_BAR_WIDTH_TRIM`] (BL-82 EM-5.17 Phase 0
    /// review follow-up): the module doc comment on
    /// [`ACTION_BAR_HALF_WIDTH_PX`] claims "the two pieces' 3+2 ability
    /// slots stay comfortably clear of the trimmed width" — this pins that
    /// claim as a real assertion instead of a comment that could silently
    /// go stale if `SLOT_SIZE_PX`, `HudSpacing::xs`, or
    /// `ACTION_BAR_WIDTH_TRIM` ever change independently.
    /// `hotbar.rs::sync_slot_half_parenting` gives the LEFT half
    /// `total.div_ceil(2)` slots — the worst case for a 5-slot hotbar (the
    /// sim's current default `ActiveAbilities::limit`) is 3 slots + 2 gaps.
    #[test]
    fn trimmed_action_bar_half_width_still_fits_the_worst_case_slot_row() {
        let xs_gap = xindeler_ui::theme::HudSpacing::default().xs;
        let worst_case_slots_per_half = 3.0;
        let worst_case_row_width =
            worst_case_slots_per_half * crate::hotbar::SLOT_SIZE_PX + 2.0 * xs_gap;
        assert!(
            ACTION_BAR_HALF_WIDTH_PX >= worst_case_row_width,
            "ACTION_BAR_HALF_WIDTH_PX ({ACTION_BAR_HALF_WIDTH_PX}) must fit 3 hotbar slots + 2 \
             gaps ({worst_case_row_width}) — a future change to ACTION_BAR_WIDTH_TRIM, \
             SLOT_SIZE_PX, or HudSpacing::xs shrank this below that floor"
        );
    }
}
