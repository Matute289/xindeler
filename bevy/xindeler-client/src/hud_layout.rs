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
///
/// ## BL-82 orb crop round 3 — round 2 was STILL cropped, by its own math
/// Round 2's `768×768` crop was correctly the WIDEST possible SQUARE, but its
/// own doc comment above already admitted this can't be a full fix: the
/// angel/cuthulhu art is wider (`879px`/`1165px`) than the `768px` a square
/// crop is bounded by, so a square crop geometrically MUST still clip real
/// wing/tentacle pixels no matter how it's centred — round 2 only shrank the
/// clipping versus the original `716×716` crop, it never eliminated it. This
/// held up live: Matías's round-3 report confirmed the angel/cuthulhu orbs
/// still visibly lose wing/tentacle art after round 2 shipped. Independently
/// re-verified directly against the on-disk PNGs (alpha-channel connected-
/// component analysis, matching round 2's own method): rendering round 2's
/// `768×768` crop at the real `160×160` orb size and comparing to the full
/// source art confirms angel loses both wingtips + the right-side fence/gate
/// spikes, and cuthulhu loses almost its entire right wing — a severe, boldly
/// visible crop, not a few stray anti-aliased px.
///
/// The real fix: stop forcing the frame into a SQUARE crop at all. A crop
/// symmetric around each variant's own measured hole centre, wide enough to
/// contain the FULL measured opaque art extent (`+4px` anti-aliasing margin,
/// mirrored equally on the shorter side too so the hole stays exactly
/// centred) plus a rendered box whose WIDTH is scaled from that crop by the
/// exact same factor `height_px` already uses (see
/// [`xindeler_ui::bar::spawn_orb_bar`]'s own doc comment on its new
/// `frame_width_px` parameter) shows every decorative pixel with zero
/// distortion — verified via the same pixel-level compositing method as
/// round 2, at the real render scale, that the full statue/wing/tentacle
/// silhouettes now render uncut for every variant. Crucially this does NOT
/// change how big the hole itself renders (same scale factor as round 2's
/// `768`-wide crop → `160px` box, just applied to a wider crop → wider box),
/// so [`ANGEL_LIQUID_INSET_PX`]/[`CUTHULHU_LIQUID_INSET_PX`]/
/// [`STAMINA_LIQUID_INSET_PX`] stay valid UNCHANGED from round 2 — this is
/// purely an additive fix for the decorative art, not a re-tune of the
/// liquid/hole fit.
///
/// Measured (hole centres re-used unchanged from round 2; opaque-art extent
/// independently re-verified): half-width = `max(hole_centre - art_min,
/// art_max - hole_centre) + 4px margin`, crop = `[hole_centre - half_width,
/// hole_centre + half_width]`.
/// - angel: art `x[237,1115]`, hole centre `711.0` → half-width `478.0` →
///   `x[233.0,1189.0]` (`956px` wide) → renders `199.17px` wide (`19.58px`
///   overhang per side past the orb's own `160px` box).
/// - cuthulhu: art `x[169,1333]`, hole centre `711.5` → half-width `625.5` →
///   `x[86.0,1337.0]` (`1251px` wide) → renders `260.63px` wide (`50.31px`
///   overhang per side — the mana orb's own right side has nothing next to it
///   in the cluster, so only its LEFT overhang visually spills onto the right
///   action-bar half).
/// - stamina: art `x[334,1083]`, hole centre `709.5` → half-width `379.5` →
///   `x[330.0,1089.0]` (`759px` wide) → renders `158.13px` wide, i.e.
///   marginally NARROWER than the orb's own `160px` box (a `-0.94px`
///   "overhang," meaning stamina's art already fit — matching the earlier
///   finding that stamina was never the variant losing decorative art; Matías's
///   round-2 report about stamina was about the LIQUID sizing, not this frame
///   crop).
///
/// See [`ANGEL_FRAME_WIDTH_PX`]/[`CUTHULHU_FRAME_WIDTH_PX`]/
/// [`STAMINA_FRAME_WIDTH_PX`] for the corresponding rendered-box widths, and
/// [`crate::zlayer::AMBIENT_CHROME_OVERLAY`] for why the now-overlapping
/// frame needs its own explicit `GlobalZIndex`.
///
/// ## HUD polish round 3 — this overhang is also the root cause of issue 1
/// (Matías's `captura2.png` report, asymmetric gap): see
/// [`CUTHULHU_EXTRA_GAP_PX`]'s own doc comment for how the very different
/// overhangs computed here (angel `19.58px`/side vs cuthulhu `50.31px`/side)
/// explain why the right action-bar half's gap to the mana orb read
/// noticeably tighter than the left half's gap to the stamina orb, even
/// though [`CLUSTER_GAP_PX`] itself was already identical on both sides.
pub const ANGEL_FRAME_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(233.0, 0.0),
    max: bevy::math::Vec2::new(1189.0, 768.0),
};

/// Cuthulhu (mana orb) variant of [`ANGEL_FRAME_SOURCE_CROP`] — see that
/// constant's doc comment for the round-3 per-variant crop rationale. Centred
/// on cuthulhu's own measured hole centre (`x=711.5`, unchanged from round 2).
pub const CUTHULHU_FRAME_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(86.0, 0.0),
    max: bevy::math::Vec2::new(1337.0, 768.0),
};

/// Stamina (centre orb) variant of [`ANGEL_FRAME_SOURCE_CROP`] — see that
/// constant's doc comment for the round-3 per-variant crop rationale. Centred
/// on stamina's own measured hole centre (`x=709.5`, unchanged from round 2).
/// Barely different from round 2's `768`-wide square crop (`759px` vs
/// `768px`) since stamina's art already fit within a square — kept per-variant
/// anyway rather than special-cased, so all three variants go through the
/// exact same [`xindeler_ui::bar::spawn_orb_bar`] `frame_width_px` mechanism.
pub const STAMINA_FRAME_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(330.0, 0.0),
    max: bevy::math::Vec2::new(1089.0, 768.0),
};

/// The rendered WIDTH (px) of the health (angel) orb's frame overlay —
/// [`ANGEL_FRAME_SOURCE_CROP`]'s own width (`956px`) scaled by the exact same
/// factor [`ORB_SIZE_PX`]`/768.0` that `height_px` already uses for every
/// orb, so the wider crop stretches onto a proportionally wider box with NO
/// distortion (see that constant's doc comment for the full round-3
/// rationale). Passed as `spawn_orb_bar`'s `frame_width_px` argument —
/// wider than [`ORB_SIZE_PX`], so the frame overlay spills a few px past the
/// orb's own square hit-box on each side instead of clipping the wingtips.
pub const ANGEL_FRAME_WIDTH_PX: f32 = (1189.0 - 233.0) * ORB_SIZE_PX / 768.0;

/// Cuthulhu (mana orb) variant of [`ANGEL_FRAME_WIDTH_PX`] — by far the
/// widest overhang of the three (the cuthulhu wing is the single widest
/// piece of decorative art in the whole pack).
pub const CUTHULHU_FRAME_WIDTH_PX: f32 = (1337.0 - 86.0) * ORB_SIZE_PX / 768.0;

/// Stamina (centre orb) variant of [`ANGEL_FRAME_WIDTH_PX`] — renders
/// marginally NARROWER than [`ORB_SIZE_PX`] (stamina's art already fit a
/// square crop); still routed through the same `frame_width_px` mechanism as
/// the other two variants rather than special-cased to `ORB_SIZE_PX`.
pub const STAMINA_FRAME_WIDTH_PX: f32 = (1089.0 - 330.0) * ORB_SIZE_PX / 768.0;

/// BL-82 HUD polish round 3 (Matías's `captura2.png` report, issue 1): extra
/// clearance (px) added ONLY between the right action-bar half and the mana
/// orb, on top of the normal [`CLUSTER_GAP_PX`] every other adjacent pair in
/// the row shares.
///
/// Root cause: [`CLUSTER_GAP_PX`] is genuinely the SAME `4.0px` on every
/// adjacent pair in the row (`cluster_is_symmetric_around_centre` already
/// pinned this) — the visible asymmetry Matías reported (the right half's gap
/// to the cuthulhu/mana orb reads noticeably tighter than the left half's gap
/// to the stamina orb) comes entirely from [`CUTHULHU_FRAME_WIDTH_PX`]'s own
/// overhang being far bigger than [`ANGEL_FRAME_WIDTH_PX`]'s: the cuthulhu
/// frame spills `(260.63 - 160.0) / 2 ≈ 50.31px` past the orb's own hit-box on
/// each side, vs the angel frame's `(199.17 - 160.0) / 2 ≈ 19.58px` — both
/// bigger than the `4px` gap, so BOTH overhangs already paint over some of
/// their neighbouring half's background art (by design, see
/// `ANGEL_FRAME_SOURCE_CROP`'s round-3 doc comment — that overlap is an
/// accepted trade-off of letting the wide decorative wing art render
/// uncropped), but the cuthulhu side overlaps roughly `2.5×` further into the
/// right half than the angel side does into the left half, which is what
/// reads as "no breathing room" in the screenshot.
///
/// This constant equalizes that overlap rather than eliminating it outright
/// (a full elimination would need `~50px` of extra clearance, visibly
/// breaking the "one contiguous row" look this cluster exists to keep — see
/// `hud_layout`'s own module doc comment): it is exactly the DELTA between
/// the two overhangs, so after shifting the mana orb this far right (see
/// [`CLUSTER`]'s `mana_orb_left` computation), the cuthulhu frame overlaps
/// the right half by the SAME amount the angel frame already overlaps the
/// left half — "both gaps match," Matías's own framing, applied as "a little
/// breathing room" rather than a full redesign.
pub const CUTHULHU_EXTRA_GAP_PX: f32 = (CUTHULHU_FRAME_WIDTH_PX - ANGEL_FRAME_WIDTH_PX) / 2.0;

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
/// tuned by eye against `--smoke-screenshot` output. A pure width-only scale
/// (height untouched, since [`ACTION_BAR_HALF_HEIGHT_PX`] still has to match
/// [`ORB_SIZE_PX`] for the row's shared bottom edge) does very slightly
/// squash the art off its native aspect ratio — imperceptible at this modest
/// a trim, and the far smaller evil compared to either shrinking the row's
/// height (breaking the orb alignment) or leaving no breathing room at all.
///
/// BL-82 EM-5.17 "5+5 slot-holders" follow-up: each piece now ALWAYS renders
/// [`crate::hotbar::SLOTS_PER_HALF`] (5) ability-slot holders (previously the
/// entity count tracked the sim's real, usually-shorter slot count — see
/// `hotbar.rs`'s own module doc comment) — `hotbar::SLOT_SIZE_PX` was bumped
/// from `44.0` to `46.0` to match, the largest size 5 slots + 4 gaps still
/// fit inside this trimmed width (see that constant's own doc comment for the
/// exact arithmetic);
/// [`tests::five_slots_per_half_fit_inside_the_action_bar_half_width`]
/// pins this so a future change to either constant can't silently overflow.
///
/// BL-82 HUD polish round 3 (Matías's `captura2.png` report, issues 2/3):
/// nudged `0.88 -> 0.95` — a modest re-widening, NOT a reversion of the
/// original "more breathing room" trim (still noticeably below `1.0`, the
/// pure aspect-derived width) — to make room for flush/adjacent slots at a
/// legibly bigger [`crate::hotbar::SLOT_SIZE_PX`] (`46.0 -> 52.0`) without
/// spreading them across the piece.
///
/// Same round, POST-REVIEW follow-up: bumped again, `0.95 -> 1.14` —
/// this one genuinely does cross `1.0` (the pure aspect-derived width),
/// stretching the art slightly wider than its native aspect ratio rather
/// than squashing it, the first time this constant has gone that direction.
/// Necessary to make room for `crate::hotbar::HOTBAR_ROW_LEADING_INSET_PX`
/// (a fix for a SEPARATE bug that same live `--smoke-screenshot` check
/// caught: the row's first slot rendering under the background art's own
/// corner curl — see that constant's own doc comment for the pixel-measured
/// proof, and for why the inset itself is a full slot pitch rather than a
/// smaller value) without shrinking `SLOT_SIZE_PX` back down and undoing
/// issue 3's fix. At this still-modest a stretch (14% over native aspect,
/// similar order of magnitude to the original 12% squash) the distortion
/// reads the same as the original trim's own squash did: imperceptible
/// against the ornate, already-irregular frame art. See
/// [`crate::hotbar::SLOT_SIZE_PX`]'s doc comment for the exact fit
/// arithmetic this trim now supports and
/// [`tests::five_slots_per_half_fit_inside_the_action_bar_half_width`] for
/// the pinned regression.
///
/// BL-82 HUD polish round 4 (issue 3, Matías's `skill-slots-1.png`
/// reference): bumped again, `1.14 -> 1.40` — makes room for
/// `crate::hotbar::SLOT_SIZE_PX`'s further bump (`52.0 -> 58.0`) at the
/// reference's noticeably larger, more flush/adjacent slot squares.
///
/// ## A first attempt at `1.30` looked right on paper, was WRONG on screen
/// The [`tests::five_slots_per_half_fit_inside_the_action_bar_half_width`]
/// arithmetic only proves the 5-slot row fits inside
/// [`ACTION_BAR_HALF_WIDTH_PX`]'s own BOUNDING BOX — it says nothing about
/// whether that box's ART is actually OPAQUE all the way out to that width.
/// A first pass at `1.30` (paired with `SLOT_SIZE_PX = 60.0`) passed that
/// test but, verified via a real `--smoke-screenshot`, visibly showed the
/// RIGHT half's last 1-2 holders (the mouse-click-icon slots) floating over
/// bare green terrain — `action_bar_bg_right.png` has a prominent decorative
/// ROUNDED END-CAP on its right side (the side away from the stamina orb,
/// mirroring the ornate spiked caps the orb frames also have) whose opaque
/// "flat plate" backing runs out well before the box's own right edge.
/// Measured directly (Python + Pillow/NumPy, alpha-channel scan, `alpha >
/// 10` threshold — same methodology as this module's other crop
/// measurements — restricted to the ROW's own vertical band, i.e. the
/// `crate::hotbar::SLOT_SIZE_PX`-tall strip a slot square actually occupies,
/// not the whole `752px` canvas height, since a spike tip far above/below
/// that band is irrelevant to whether a SLOT SQUARE has real backing): the
/// right half's flat, ≥98%-opaque backing (within that band) holds from
/// native `x=98` to `x≈1249` of its `1380px`-wide canvas, then collapses
/// fast (`97.9%` at `x=1250` down to `21%` by `x=1290`). The row's content
/// must end at or before that `x≈1249` boundary (scaled to whatever
/// `ACTION_BAR_HALF_WIDTH_PX` renders at), which is a MUCH tighter
/// constraint than "fits inside the box" — see
/// [`ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX`]/
/// [`ACTION_BAR_LEFT_FLAT_SAFE_START_RAW_PX`] and
/// [`tests::hotbar_row_fits_within_each_action_bar_halfs_own_flat_opaque_backing`]
/// for the constants/regression test this discovery adds. `1.40` (rather
/// than `1.30`) is the smallest re-trim (given `SLOT_SIZE_PX = 58.0`,
/// slightly pulled back from the first `60.0` attempt) that clears this
/// tighter constraint with a real margin on BOTH halves, re-verified against
/// a fresh `--smoke-screenshot` showing every one of the 10 holders — 1-8
/// numbered plus the 2 mouse-click icons — sitting fully on the dark plate
/// art, no green terrain visible through any of them.
pub const ACTION_BAR_WIDTH_TRIM: f32 = 1.40;
pub const ACTION_BAR_HALF_WIDTH_PX: f32 =
    ACTION_BAR_HALF_HEIGHT_PX * (1380.0 / 752.0) * ACTION_BAR_WIDTH_TRIM;

/// BL-82 HUD polish round 4 (issue 3) — the native-canvas x-coordinate (out
/// of `action_bar_bg_right.png`'s `1380px` width) past which the RIGHT
/// action-bar half's own flat plate art stops being reliably opaque within
/// the hotbar row's own vertical band — see [`ACTION_BAR_WIDTH_TRIM`]'s doc
/// comment for the full measurement methodology. A conservative reading of
/// the measured data (opacity is still a genuine `100%` through `x=1240`,
/// only starting its fast collapse at `x=1250`), not the absolute measured
/// edge (`x≈1249`) — a few native px of margin here costs nothing and
/// absorbs any small re-measurement error.
///
/// `#[cfg(test)]`: this constant is only ever read by
/// [`tests::hotbar_row_fits_within_each_action_bar_halfs_own_flat_opaque_backing`]
/// — the real fix is [`ACTION_BAR_WIDTH_TRIM`]'s tuned value itself, this is
/// the measured boundary that value must stay under. Ungated, it trips
/// `-D dead-code` in the non-test build (this crate is a binary, so a `pub`
/// item with no caller outside `#[cfg(test)]` is exactly as dead as a
/// private one) — same pattern [`health_orb_screen_x`] above already uses.
#[cfg(test)]
pub const ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX: f32 = 1240.0;

/// BL-82 HUD polish round 4 (issue 3) — the native-canvas x-coordinate (out
/// of `action_bar_bg_left.png`'s `1380px` width) before which the LEFT
/// action-bar half's own flat plate art is NOT yet reliably opaque within the
/// hotbar row's own vertical band (the mirrored decorative corner curl this
/// piece's own `HOTBAR_ROW_LEADING_INSET_PX` fix already targets — that fix
/// predates this round's measurement but this pins the SAME real constraint
/// numerically instead of by eye). Measured: opacity reaches a genuine
/// `100%` at `x=150`, ramping up from `0%` at `x=20` — same conservative
/// "round to where it's unambiguously flat" reading as
/// [`ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX`].
///
/// `#[cfg(test)]`: same reasoning as
/// [`ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX`]'s own doc comment — test-only
/// regression value, not read at runtime.
#[cfg(test)]
pub const ACTION_BAR_LEFT_FLAT_SAFE_START_RAW_PX: f32 = 150.0;

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
///
/// BL-82 HUD polish round 4 (issue 2, Matías's `captura4.png` report: "bring
/// the action-bar halves + their orbs closer to the centre stamina orb —
/// close, but not touching"): shrunk `4.0 -> 2.0`. Halved rather than zeroed
/// out — Matías's own framing was "noticeably closer," not "flush" (that
/// would read as one solid slab and lose the "5 distinct pieces in one row"
/// look this whole cluster is built around), and round 3's own doc comment
/// already established the precedent that a fully-touching (`0`/negative)
/// gap reads as a mismatched art seam, not a clean line, given each piece's
/// own opaque black border padding. [`cluster_is_symmetric_around_centre`]
/// pins the new, smaller value — a shrink here is the whole point of this
/// round's fix, not a regression.
pub const CLUSTER_GAP_PX: f32 = 2.0;

/// Distance (px) from the viewport's bottom edge to the bottom of the whole
/// orb/action-bar row.
///
/// BL-82 HUD polish round 3 (Matías's `captura2.png` report, issue 5): was
/// `20.0`, leaving a visible gap down to the floor texture in live gameplay
/// frames. Every orb container's own frame-crop already spans the FULL
/// vertical extent of its source canvas (`y[0, 768]` — see
/// `ANGEL_FRAME_SOURCE_CROP`'s/`STAMINA_FRAME_SOURCE_CROP`'s own doc
/// comments), so whatever sits at the very bottom row of each frame's art
/// (including the stamina orb's decorative pointed bottom spike) already
/// renders flush with the orb container's OWN bottom edge — there is no
/// separate vertical inset/overhang mechanism (unlike
/// [`ANGEL_FRAME_WIDTH_PX`]'s horizontal one) that would need a matching
/// change here. Setting this to `0.0` therefore puts every orb's true bottom
/// pixel (angel/cuthulhu/stamina alike) flush with the screen's own bottom
/// edge — exactly Matías's ask, composing correctly with `HudScalePlugin`'s
/// `UiScale` (a uniform multiplier over every `Val::Px` in this module,
/// `0.0 * scale` staying `0.0` at any window size).
///
/// ## BL-82 HUD polish round 4 (issue 1) — round 3's `0.0` was STILL not flush
/// Matías's round-4 report (`captura4.png` + `record23.mov`, independently
/// re-confirmed by extracting frames rather than trusting a single still):
/// green ground/road is still visibly rendering BELOW the orb/action-bar row
/// even with this constant at `0.0`. Round 3's own reasoning above was only
/// half right: it correctly ruled out a SEPARATE vertical inset/overhang
/// mechanism (there isn't one — `ORB_SOURCE_CROP`/`*_FRAME_SOURCE_CROP` really
/// do span the full `y[0,768]` canvas height), but it never checked whether
/// that FULL canvas height is itself free of transparent padding at its own
/// bottom edge. Re-measured directly (Python + Pillow, alpha-channel
/// column/row scan, `alpha > 10` threshold — same methodology as this
/// module's own crop-measurement comments above) against the real on-disk
/// PNGs: every one of the 5 cluster pieces has a real, contiguous band of
/// fully-transparent pixels at the very bottom of its own canvas, BELOW the
/// last row of genuinely opaque art:
/// - `orb_frame_angel.png` (`768px` tall): last opaque row `y=756` → `11px`
///   transparent below it.
/// - `orb_frame_cuthulhu.png` (`768px` tall): last opaque row `y=754` → `13px`.
/// - `orb_frame_stamina.png` (`768px` tall): last opaque row `y=753` → `14px`.
/// - `action_bar_bg_left.png` (`752px` tall): last opaque row `y=728` → `23px`.
/// - `action_bar_bg_right.png` (`752px` tall): last opaque row `y=725` →
///   `26px`.
///
/// This is exactly the failure mode the round-4 brief predicted: a
/// `bottom_px` constant positions the CONTAINER's bounding box, not the
/// visible art's own lowest opaque pixel — no value of this one shared
/// constant can compensate for a PER-ASSET amount of baked-in transparent
/// margin, since the 5 pieces don't agree on how much they have (`11`-`26`
/// raw px, not even close to uniform). At `CLUSTER_BOTTOM_PX == 0.0` every
/// container's bottom edge sits exactly at the screen's bottom edge, but each
/// container's real art ends `padding_px` ABOVE that line — precisely the gap
/// Matías keeps reporting, and (composing with [`HudScalePlugin`]'s
/// `UiScale`) one that GROWS at a larger window/UI scale rather than staying
/// a fixed few px, matching why a still screenshot at a good size already
/// shows it clearly.
///
/// The fix: [`ANGEL_FRAME_BOTTOM_PAD_PX`]/[`CUTHULHU_FRAME_BOTTOM_PAD_PX`]/
/// [`STAMINA_FRAME_BOTTOM_PAD_PX`]/[`ACTION_BAR_LEFT_BOTTOM_PAD_PX`]/
/// [`ACTION_BAR_RIGHT_BOTTOM_PAD_PX`] below — each piece's OWN measured
/// padding, scaled from raw canvas px to real rendered px by the exact same
/// factor its own height already uses (`render_height / native_height`, the
/// same "scale the measurement, don't re-measure at render size" approach
/// [`ANGEL_FRAME_WIDTH_PX`] already established). Every real spawn call site
/// (`combat_hud.rs`'s 3 orbs, `hotbar.rs`'s 2 action-bar halves) now sets
/// `bottom: Val::Px(CLUSTER_BOTTOM_PX - <that piece's own pad constant>)` —
/// pushing each container down by exactly its own transparent margin so the
/// REAL opaque pixel (not the bounding box) lands flush with the screen's
/// bottom edge, verified directly via `--smoke-screenshot` (not just the
/// arithmetic) against both `captura4.png` (the bug) and live gameplay.
/// `CLUSTER_BOTTOM_PX` itself stays the shared "nominal" anchor every other
/// derived offset (`CLUSTER_TOTAL_HEIGHT_PX`, the XP cluster's own bottom)
/// already builds from — only the 5 real spawn sites apply the extra
/// per-piece correction, so nothing downstream needs to change.
pub const CLUSTER_BOTTOM_PX: f32 = 0.0;

/// BL-82 HUD polish round 4 (issue 1) — [`orb_frame_angel.png`]'s own
/// measured transparent bottom margin (`11` raw px out of its `768px`-tall
/// canvas), scaled to real rendered px by the same `ORB_SIZE_PX / 768.0`
/// factor every other orb constant in this module uses. See
/// [`CLUSTER_BOTTOM_PX`]'s own doc comment for the full measurement
/// methodology and why a single shared constant can't fix this.
pub const ANGEL_FRAME_BOTTOM_PAD_PX: f32 = 11.0 * ORB_SIZE_PX / 768.0;

/// Cuthulhu (mana orb) variant of [`ANGEL_FRAME_BOTTOM_PAD_PX`] — `13` raw px
/// out of the same `768px`-tall canvas.
pub const CUTHULHU_FRAME_BOTTOM_PAD_PX: f32 = 13.0 * ORB_SIZE_PX / 768.0;

/// Stamina (centre orb) variant of [`ANGEL_FRAME_BOTTOM_PAD_PX`] — `14` raw
/// px out of the same `768px`-tall canvas.
pub const STAMINA_FRAME_BOTTOM_PAD_PX: f32 = 14.0 * ORB_SIZE_PX / 768.0;

/// `action_bar_bg_left.png`'s own measured transparent bottom margin (`23`
/// raw px out of its `752px`-tall canvas), scaled to real rendered px by the
/// `ACTION_BAR_HALF_HEIGHT_PX / 752.0` factor (the action-bar halves' own
/// native-height scale factor, distinct from the orbs' `768.0` canvas). See
/// [`CLUSTER_BOTTOM_PX`]'s own doc comment for the measurement methodology.
pub const ACTION_BAR_LEFT_BOTTOM_PAD_PX: f32 = 23.0 * ACTION_BAR_HALF_HEIGHT_PX / 752.0;

/// `action_bar_bg_right.png` variant of [`ACTION_BAR_LEFT_BOTTOM_PAD_PX`] —
/// `26` raw px out of the same `752px`-tall canvas (the two halves are
/// mirrored art, not byte-identical, hence the slightly different measured
/// value).
pub const ACTION_BAR_RIGHT_BOTTOM_PAD_PX: f32 = 26.0 * ACTION_BAR_HALF_HEIGHT_PX / 752.0;

/// Gap (px) between the top of the orb/action-bar row and the XP-bar+level
/// cluster sitting just above it (spec §3.1's "between/above the orbs and
/// action bar").
pub const XP_CLUSTER_GAP_PX: f32 = 6.0;

/// Approximate height (px) of the XP-bar+level readout sitting above the orb
/// row — `combat_hud.rs::spawn_combat_hud`'s `xp_cluster_root` stacks
/// `LevelText` (18px font) + a `theme.spacing.xs` (4px, the stock
/// [`xindeler_ui::theme::HudSpacing`] default) row gap + the 6px XP bar, with
/// no extra container padding. Deliberately a generous OVERESTIMATE (real
/// text line-height/leading isn't accounted for exactly) —
/// [`CLUSTER_TOTAL_HEIGHT_PX`] exists so other screens can clear the WHOLE row
/// without re-measuring it themselves, and erring tall here only ever gives
/// them a little MORE clearance than strictly required, never less.
pub const XP_CLUSTER_CONTENT_HEIGHT_PX: f32 = 40.0;

/// Total height (px), from the viewport's bottom edge, of the entire
/// bottom-centre HUD row — the 3 resource orbs plus the XP/level cluster
/// sitting above them ([`XP_CLUSTER_GAP_PX`] +
/// [`XP_CLUSTER_CONTENT_HEIGHT_PX`]). BL-82 HUD-responsive-scaling pass: other
/// screens that need to sit ABOVE this row without vertically overlapping it
/// (`chat.rs`'s panel — Matías's "chat and the health orb overlap at a reduced
/// window size" report) read this rather than re-deriving or guessing the row's
/// real height. Computed from the SAME public constants `combat_hud.rs` itself
/// builds the row from, so it can never silently drift out of sync with the
/// real layout.
pub const CLUSTER_TOTAL_HEIGHT_PX: f32 =
    CLUSTER_BOTTOM_PX + ORB_SIZE_PX + XP_CLUSTER_GAP_PX + XP_CLUSTER_CONTENT_HEIGHT_PX;

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
///
/// BL-82 HUD polish round 3: `mana_orb_left` is the one deliberate exception
/// to the otherwise-mirrored left/right arithmetic — see
/// [`CUTHULHU_EXTRA_GAP_PX`]'s own doc comment for why the mana orb needs
/// its own extra clearance the health orb doesn't.
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
    // BL-82 HUD polish round 3 (issue 1): the mana orb gets
    // CUTHULHU_EXTRA_GAP_PX of clearance ON TOP of the shared CLUSTER_GAP_PX
    // every other adjacent pair uses — see that constant's own doc comment
    // for why only this one pairing needs it.
    let mana_orb_left = action_bar_right_half_right + CLUSTER_GAP_PX + CUTHULHU_EXTRA_GAP_PX;
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

/// The health orb's screen-space `(left, right)` x-edges for a given window
/// width — `width / 2.0` (the `CENTER_LEFT` anchor resolves against the REAL
/// window, `Val::Percent` is untouched by `UiScale`) plus
/// [`CLUSTER::health_orb_left`]'s margin offset. Exists purely so the
/// overlap-avoidance test suite (this module's own + `chat.rs`'s, BL-82
/// HUD-responsive-scaling pass) can reason about exactly where the health
/// orb sits without duplicating this arithmetic by hand — `chat.rs`'s actual
/// production fix (`PANEL_BOTTOM_PX`) guarantees zero overlap via a purely
/// VERTICAL separation instead (see that constant's own doc comment for why
/// a width-based avoidance can't work across every window size), so nothing
/// in non-test code needs this at runtime — `#[cfg(test)]` rather than
/// `#[allow(dead_code)]`, since it's genuinely only ever called from tests.
/// Can go negative (or spill past `width`) for a window narrower than the
/// cluster's own ~1196px total span (BL-82 HUD polish round 3 widened this
/// from the earlier ~1013px) — the orb is simply partially or fully
/// off-screen at that point, a real but SEPARATE known limitation of this
/// cluster's fixed-width design, not something this function hides.
#[cfg(test)]
#[must_use]
pub fn health_orb_screen_x(window_width: f32) -> (f32, f32) {
    let left = window_width / 2.0 + CLUSTER.health_orb_left;
    (left, left + ORB_SIZE_PX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cluster is laid out symmetrically around screen centre: the
    /// Stamina orb (the centrepiece) straddles `x = 0` exactly, and the two
    /// action-bar halves mirror each other in width — a regression guard
    /// against a future constant tweak silently breaking the "one
    /// contiguous row" contract this module exists to enforce.
    ///
    /// BL-82 HUD polish round 3: the outer-orb gaps are DELIBERATELY no
    /// longer identical raw numbers — [`CUTHULHU_EXTRA_GAP_PX`]'s own doc
    /// comment explains why the mana side needs extra clearance the health
    /// side doesn't (the cuthulhu frame's overhang is far bigger than the
    /// angel frame's) — so this test now asserts the mana-side gap is
    /// exactly [`CLUSTER_GAP_PX`] `+ CUTHULHU_EXTRA_GAP_PX` bigger than the
    /// health-side gap, not that the two are equal.
    ///
    /// BL-82 HUD polish round 4 (issue 2): every assertion below reads
    /// [`CLUSTER_GAP_PX`] directly rather than a hardcoded literal, so this
    /// test keeps passing unchanged now that the constant shrank `4.0 ->
    /// 2.0` — a SMALLER gap is this round's whole point, not a regression to
    /// investigate.
    #[test]
    fn cluster_is_symmetric_around_centre() {
        assert_eq!(CLUSTER.stamina_orb_left, -ORB_SIZE_PX / 2.0);

        let left_bar_width =
            CLUSTER.stamina_orb_left - CLUSTER_GAP_PX - CLUSTER.action_bar_left_half_left;
        let right_bar_width = CLUSTER.mana_orb_left
            - CLUSTER_GAP_PX
            - CUTHULHU_EXTRA_GAP_PX
            - CLUSTER.action_bar_right_half_left;
        assert!((left_bar_width - right_bar_width).abs() < f32::EPSILON);
        assert!((left_bar_width - ACTION_BAR_HALF_WIDTH_PX).abs() < f32::EPSILON);

        // Health orb's right edge must sit exactly `CLUSTER_GAP_PX` before
        // the left bar half's left edge.
        let health_orb_right = CLUSTER.health_orb_left + ORB_SIZE_PX;
        assert!(
            (health_orb_right + CLUSTER_GAP_PX - CLUSTER.action_bar_left_half_left).abs() < 0.01
        );

        // Mana orb's left edge must sit exactly `CLUSTER_GAP_PX +
        // CUTHULHU_EXTRA_GAP_PX` past the right bar half's right edge — the
        // one intentionally-asymmetric gap in the whole cluster (issue 1).
        let action_bar_right_half_right =
            CLUSTER.action_bar_right_half_left + ACTION_BAR_HALF_WIDTH_PX;
        assert!(
            (CLUSTER.mana_orb_left
                - CLUSTER_GAP_PX
                - CUTHULHU_EXTRA_GAP_PX
                - action_bar_right_half_right)
                .abs()
                < 0.01
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

    /// [`CLUSTER_TOTAL_HEIGHT_PX`] is exactly the sum of its own named parts
    /// — pins the arithmetic so a future edit to any one constant can't
    /// silently desync the combined value from what the row actually spawns.
    #[test]
    fn cluster_total_height_sums_its_named_parts() {
        let expected =
            CLUSTER_BOTTOM_PX + ORB_SIZE_PX + XP_CLUSTER_GAP_PX + XP_CLUSTER_CONTENT_HEIGHT_PX;
        assert!((CLUSTER_TOTAL_HEIGHT_PX - expected).abs() < f32::EPSILON);
    }

    /// BL-82 HUD polish round 4 (issue 1): every `*_BOTTOM_PAD_PX` constant
    /// must be strictly positive (a real piece of the source art's own
    /// transparent margin, not a zero/negative no-op) and small relative to
    /// [`ORB_SIZE_PX`]/[`ACTION_BAR_HALF_HEIGHT_PX`] (a few px, not a large
    /// fraction of the whole orb/action-bar height — a wildly larger value
    /// here would signal a measurement bug, not a real transparent margin).
    /// Also pins each constant's exact scaled value against the raw
    /// measured px this module's own doc comments cite, so a future asset
    /// re-cut that changes the padding is caught here rather than silently
    /// reintroducing the flush-bottom gap.
    #[test]
    fn bottom_pad_constants_are_small_positive_fractions_of_their_own_render_height() {
        // Computed with the EXACT SAME expression shape/order as each real
        // `const` definition (`raw_px * ORB_SIZE_PX / 768.0`, not
        // `raw_px * (ORB_SIZE_PX / 768.0)`) — f32 multiplication/division
        // isn't associative, so reassociating would compare against a
        // slightly different rounding and could spuriously fail even at a
        // loose tolerance despite both sides being "the same formula"
        // mathematically.
        assert_eq!(ANGEL_FRAME_BOTTOM_PAD_PX, 11.0 * ORB_SIZE_PX / 768.0);
        assert_eq!(CUTHULHU_FRAME_BOTTOM_PAD_PX, 13.0 * ORB_SIZE_PX / 768.0);
        assert_eq!(STAMINA_FRAME_BOTTOM_PAD_PX, 14.0 * ORB_SIZE_PX / 768.0);
        assert_eq!(
            ACTION_BAR_LEFT_BOTTOM_PAD_PX,
            23.0 * ACTION_BAR_HALF_HEIGHT_PX / 752.0
        );
        assert_eq!(
            ACTION_BAR_RIGHT_BOTTOM_PAD_PX,
            26.0 * ACTION_BAR_HALF_HEIGHT_PX / 752.0
        );

        for pad in [
            ANGEL_FRAME_BOTTOM_PAD_PX,
            CUTHULHU_FRAME_BOTTOM_PAD_PX,
            STAMINA_FRAME_BOTTOM_PAD_PX,
            ACTION_BAR_LEFT_BOTTOM_PAD_PX,
            ACTION_BAR_RIGHT_BOTTOM_PAD_PX,
        ] {
            assert!(pad > 0.0, "a bottom pad must be a real positive margin");
            assert!(
                pad < ORB_SIZE_PX * 0.1,
                "a bottom pad of {pad} is implausibly large relative to ORB_SIZE_PX \
                 ({ORB_SIZE_PX}) — re-check the measurement"
            );
        }
    }

    /// [`health_orb_screen_x`]: the orb's edges sit exactly
    /// `width / 2.0 + CLUSTER.health_orb_left` .. `+ ORB_SIZE_PX` — and, for
    /// a window narrower than the cluster's own ~1196px total span, the left
    /// edge genuinely goes negative (the orb spills off-screen) rather than
    /// being silently clamped — callers must handle that themselves.
    #[test]
    fn health_orb_screen_x_matches_the_raw_arithmetic_and_can_go_negative() {
        let (left, right) = health_orb_screen_x(1280.0);
        assert!((left - (640.0 + CLUSTER.health_orb_left)).abs() < f32::EPSILON);
        assert!((right - (left + ORB_SIZE_PX)).abs() < f32::EPSILON);

        let (narrow_left, _) = health_orb_screen_x(400.0);
        assert!(
            narrow_left < 0.0,
            "a window narrower than the cluster's own span must report a genuinely off-screen \
             (negative) left edge, not a clamped one"
        );
    }

    /// Regression guard for [`ACTION_BAR_WIDTH_TRIM`] (BL-82 EM-5.17 "5+5
    /// slot-holders" follow-up, superseding the old "worst case 3 slots"
    /// version of this test; updated BL-82 HUD polish round 3 for the
    /// flush/adjacent slot layout — issues 2/3 of Matías's `captura2.png`
    /// report): each action-bar half ALWAYS renders
    /// [`crate::hotbar::SLOTS_PER_HALF`] (5) holders now (not a sim-driven,
    /// usually-shorter count) — this pins that the re-trimmed
    /// [`ACTION_BAR_HALF_WIDTH_PX`] still fits [`crate::hotbar::
    /// HOTBAR_ROW_LEADING_INSET_PX`] (round-3 post-review follow-up — the
    /// corner-clearing left inset, see that constant's own doc comment) plus
    /// exactly 5 `hotbar::SLOT_SIZE_PX` slots + 4 of `hotbar::
    /// HOTBAR_SLOT_GAP_PX` gaps (no longer the theme's generic
    /// `HudSpacing::xs` — round 3 gave the hotbar its own dedicated, tighter
    /// gap constant, matching `xindeler-old`'s own `slot_offset = 3.0`
    /// reference layout), so a future change to any of
    /// `ACTION_BAR_WIDTH_TRIM`/`HOTBAR_ROW_LEADING_INSET_PX`/`SLOT_SIZE_PX`/
    /// `HOTBAR_SLOT_GAP_PX` can't silently make the row overflow the
    /// background art's own width.
    #[test]
    fn five_slots_per_half_fit_inside_the_action_bar_half_width() {
        let leading_inset = crate::hotbar::HOTBAR_ROW_LEADING_INSET_PX;
        let slot_gap = crate::hotbar::HOTBAR_SLOT_GAP_PX;
        let slots_per_half = crate::hotbar::SLOTS_PER_HALF as f32;
        let row_width = leading_inset
            + slots_per_half * crate::hotbar::SLOT_SIZE_PX
            + (slots_per_half - 1.0) * slot_gap;
        assert!(
            ACTION_BAR_HALF_WIDTH_PX >= row_width,
            "ACTION_BAR_HALF_WIDTH_PX ({ACTION_BAR_HALF_WIDTH_PX}) must fit the \
             HOTBAR_ROW_LEADING_INSET_PX-shifted {slots_per_half} hotbar slots + gaps \
             ({row_width}) — a future change to ACTION_BAR_WIDTH_TRIM, \
             HOTBAR_ROW_LEADING_INSET_PX, SLOT_SIZE_PX, or HOTBAR_SLOT_GAP_PX shrank this below \
             that floor"
        );
    }

    /// BL-82 HUD polish round 4 (issue 3) — the REAL regression guard the
    /// bounding-box-only check above can't provide: the row's content must
    /// land within each action-bar half's own FLAT, OPAQUE plate art (see
    /// [`ACTION_BAR_WIDTH_TRIM`]'s doc comment for the full story of how a
    /// trim that passed the box-fit test above still visibly overflowed onto
    /// green terrain on the right half). Converts the row's rendered-px
    /// start/end back to the SAME `1380px`-native-width coordinate space
    /// [`ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX`]/
    /// [`ACTION_BAR_LEFT_FLAT_SAFE_START_RAW_PX`] were measured against
    /// (both source PNGs share that native width) and checks both real
    /// constraints: the row must END at or before the RIGHT half's flat
    /// backing runs out, and must START at or after the LEFT half's own
    /// corner-curl clears.
    #[test]
    fn hotbar_row_fits_within_each_action_bar_halfs_own_flat_opaque_backing() {
        let leading_inset = crate::hotbar::HOTBAR_ROW_LEADING_INSET_PX;
        let slot_gap = crate::hotbar::HOTBAR_SLOT_GAP_PX;
        let slot_size = crate::hotbar::SLOT_SIZE_PX;
        let slots_per_half = crate::hotbar::SLOTS_PER_HALF as f32;
        let row_end_rendered =
            leading_inset + slots_per_half * slot_size + (slots_per_half - 1.0) * slot_gap;

        // Both `action_bar_bg_left.png`/`_right.png` are `1380px` wide
        // natively — this is the SAME scale factor `ACTION_BAR_HALF_WIDTH_PX`
        // itself is derived from (`ACTION_BAR_HALF_HEIGHT_PX * (1380/752) *
        // ACTION_BAR_WIDTH_TRIM`), just inverted back to native px.
        let native_scale = ACTION_BAR_HALF_WIDTH_PX / 1380.0;
        let row_end_native = row_end_rendered / native_scale;
        let leading_inset_native = leading_inset / native_scale;

        assert!(
            row_end_native <= ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX,
            "the hotbar row's content ends at native x={row_end_native:.1} on the RIGHT half, \
             past the flat plate's own safe opaque boundary \
             ({ACTION_BAR_RIGHT_FLAT_SAFE_END_RAW_PX}) — the last holder(s) will render over bare \
             terrain instead of the plate art; increase ACTION_BAR_WIDTH_TRIM or shrink \
             SLOT_SIZE_PX/HOTBAR_SLOT_GAP_PX"
        );
        assert!(
            leading_inset_native >= ACTION_BAR_LEFT_FLAT_SAFE_START_RAW_PX,
            "the hotbar row's content starts at native x={leading_inset_native:.1} on the LEFT \
             half, before the flat plate's own corner-curl clears \
             ({ACTION_BAR_LEFT_FLAT_SAFE_START_RAW_PX}) — the first holder will render under the \
             piece's decorative corner"
        );
    }
}
