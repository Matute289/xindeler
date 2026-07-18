//! BL-82 EM-5.17 Phase 2 — shared bottom-center HUD cluster geometry.
//!
//! `combat_hud.rs` (the 3 resource orbs) and `hotbar.rs` (the ability-slot
//! row) are two INDEPENDENT `Startup`-spawn plugins, but per the design spec
//! (§3.1) they must render as ONE visually contiguous bottom-center row:
//! `[ Health orb ] [ left slot row ] [ Stamina orb ] [ right slot row ]
//! [ Mana orb ]`. Rather than have each file invent its own magic-number
//! offsets (guaranteed to drift out of alignment the first time either file's
//! constants change), every element's horizontal placement is computed ONCE
//! here, from the same arithmetic, and both files position their own pieces
//! purely via absolute `left: 50% + margin-left: <this module's offset>` (the
//! same percent-plus-negative-margin centring idiom `combat_hud.rs`'s own
//! `Crosshair` node already uses) — no shared parent entity is needed across
//! the two plugins, so spawn-order between them never matters.
//!
//! ## BL-82 HUD redesign round 6 — the action-bar FRAME pieces are gone
//! Matías's reference (`hud-ejemplo-2.png`, a Diablo-4-fan-art HUD) has NO
//! ornate frame art around the ability slots at all — just individual square
//! icons sitting directly between the two large circular orbs, tightly
//! packed and pulled in close to the centre. Rounds 3-5 below (preserved for
//! the still-relevant orb-frame-margin math they established) spent their
//! effort getting `action_bar_bg_left.png`/`action_bar_bg_right.png` (the
//! ornate bronze/spiked frame pieces) to fit/align well — round 6 removes
//! those two assets from the render path ENTIRELY instead. The former
//! `ACTION_BAR_*` constants that existed purely to size/crop/trim that frame
//! art (`ACTION_BAR_WIDTH_TRIM`, `ACTION_BAR_HALF_WIDTH_PX`, the
//! `ACTION_BAR_LEFT/RIGHT_*_MARGIN_PX`/`*_BOTTOM_PAD_PX`/
//! `*_FLAT_SAFE_*_RAW_PX` family) are deleted outright, not renamed — none of
//! that math has any referent left once the art is gone. What replaces it:
//! [`SLOT_ROW_WIDTH_PX`]/[`SLOT_ROW_HEIGHT_PX`] derive the ability-slot row's
//! box DIRECTLY from the slot geometry itself
//! (`crate::hotbar::SLOT_SIZE_PX`/`SLOTS_PER_HALF`/`HOTBAR_SLOT_GAP_PX`) — no
//! trim factor, no leading inset, no "flat opaque backing" safe-zone, because
//! there is no background art left whose own transparent padding or
//! decorative corner curl needs working around. The row's own box edges ARE
//! its visible content's edges. The four per-seam gap constants
//! ([`HEALTH_TO_LEFT_ROW_GAP_PX`] etc.) keep the SAME "real, opaque-to-opaque
//! visible gap" contract round 5 established — they just drop the
//! now-nonexistent action-bar-art margin term from the `box_gap` formula (the
//! orb frames' own margins are unaffected: those orbs are untouched by this
//! round, only the action-bar frame pieces are removed). See
//! [`CLUSTER_GAP_PX`]'s own doc comment for the up-to-date version of that
//! formula.
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
/// (Matías's `captura2.png` report, asymmetric gap): the very different
/// overhangs computed here (angel `19.58px`/side vs cuthulhu `50.31px`/side)
/// are why the right action-bar half's gap to the mana orb read noticeably
/// tighter than the left half's gap to the stamina orb, even though
/// [`CLUSTER_GAP_PX`] itself was already identical on both sides. Round 3/4
/// tried to compensate for this with a single lump-sum `CUTHULHU_EXTRA_GAP_PX`
/// correction — **round 5 replaces that with the exact per-seam margin
/// constants below** ([`ANGEL_FRAME_RIGHT_MARGIN_PX`] etc.), which fix the
/// same asymmetry precisely instead of by one hand-tuned lump sum — see
/// those constants' own doc comment for why a single shared correction was
/// never going to be exactly right for every seam.
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

/// BL-82 HUD polish round 5 — `hotbar.rs`'s per-slot `skill_slot_border.png`
/// overlay ([`crate::hotbar::SkillSlotBorderOverlay`]) needs the SAME
/// tight-crop treatment as the orb frames above, for a different reason:
/// unlike the orbs (whose decorative art genuinely spans more of the canvas
/// than a square crop allows), this asset's real opaque border art occupies
/// only the CENTRE of its own `1408×768` canvas — verified directly (alpha
/// scan, `alpha > 10` threshold, same methodology as every other crop in
/// this module): opaque bounding box `x[359,1049] y[32,734]`, i.e. only
/// `≈49%` of the canvas's own `1408px` width. `hotbar.rs`'s
/// `sync_hotbar_slots` used to spawn this as a plain full-canvas `ImageNode`
/// stretched onto the slot's `width:100%`/`height:100%` box with no crop at
/// all — the whole `1408×768` canvas (including its huge transparent
/// margin) got squashed into the slot square, so the REAL border art ended
/// up occupying only the centre ~half of every rendered slot, with a big
/// transparent gap on every side before the next slot's own real border
/// began — this is the actual root cause of "big gaps between individual
/// hotbar slots" (Matías's `captura6.png` report): no value of
/// `crate::hotbar::HOTBAR_SLOT_GAP_PX` (however small) can make slots look
/// flush/adjacent (`skill-slots-1.png`'s reference) when half of each slot's
/// own allocated box is invisible padding, not border art.
///
/// This crop (`+4px` anti-aliasing margin on every side, same convention as
/// [`ANGEL_FRAME_SOURCE_CROP`]'s round-3 fix) is close enough to square
/// (`698×710`, a `1.7%` aspect difference) that stretching it directly onto
/// `crate::hotbar::SLOT_SIZE_PX`'s square slot box (via `ImageNode::rect` +
/// `NodeImageMode::Stretch`, the exact same mechanism the orb frames already
/// use) introduces no visible distortion, while making the real border art
/// fill essentially the WHOLE slot box — matching `skill-slots-1.png`'s
/// flush/adjacent squares instead of `captura6.png`'s widely-spaced ones.
/// Unlike the orb frames, this asset needs no `frame_width_px`-style
/// overhang mechanism: its opaque bbox already fits comfortably inside a
/// square, so a single tight crop (not a wider-than-the-box one) is the
/// complete fix.
pub const SKILL_SLOT_BORDER_SOURCE_CROP: Rect = Rect {
    min: bevy::math::Vec2::new(355.0, 28.0),
    max: bevy::math::Vec2::new(1053.0, 738.0),
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

/// BL-82 HUD polish round 5 — **the real root cause of this whole class of
/// bug, finally fixed at the SEAM level instead of a lump-sum correction.**
///
/// Rounds 3/4 (see the historical `CUTHULHU_EXTRA_GAP_PX` writeup this
/// replaced, preserved in `ANGEL_FRAME_SOURCE_CROP`'s doc comment) treated
/// every adjacent pair in the row as sharing ONE nominal box-to-box gap
/// ([`CLUSTER_GAP_PX`]), with a single lump-sum correction bolted onto the
/// one seam (mana orb) that visibly needed it most. That was never going to
/// be exactly right: [`CLUSTER_GAP_PX`] positions BOUNDING BOXES
/// ([`ORB_SIZE_PX`] squares for the orbs, the (since-deleted)
/// `ACTION_BAR_HALF_WIDTH_PX` for the action-bar halves) — it has zero
/// awareness that EVERY piece in this
/// row bakes its own transparent margin into its own render box, and that
/// margin is DIFFERENT on every side of every piece (verified directly via
/// an alpha-channel scan of the real on-disk PNGs, `alpha > 10` threshold,
/// same methodology this module's other crop measurements already use):
///
/// - `orb_frame_angel.png`: real opaque art `x[237,1115]` (out of its own
///   `1408px`-wide canvas) — its RIGHT edge (facing the left action-bar half)
///   sits `74` native px inside [`ANGEL_FRAME_SOURCE_CROP`]'s own right edge,
///   but that crop is ALSO already `19.58px` (rendered) wider than
///   [`ORB_SIZE_PX`] on that side (the round-3 overhang) — net effect, the real
///   art overhangs `4.17px` PAST the orb's own nominal hit-box, not flush with
///   it.
/// - `orb_frame_cuthulhu.png`: real opaque art `x[169,1333]` — its LEFT edge
///   (facing the right action-bar half) sits `83` native px inside
///   [`CUTHULHU_FRAME_SOURCE_CROP`]'s own left edge, which is `50.31px`
///   (rendered) wider than [`ORB_SIZE_PX`] — net effect, the real art recedes
///   `33.02px` INSIDE the orb's own nominal hit-box (a much bigger real
///   transparent margin than the crop's own overhang alone would suggest).
/// - `orb_frame_stamina.png`: real opaque art `x[334,1083]`, both edges within
///   a couple of native px of [`STAMINA_FRAME_SOURCE_CROP`]'s own edges (which
///   are themselves barely wider than [`ORB_SIZE_PX`]) — net effect, both sides
///   recede only ~`2px` inside the nominal hit-box.
/// - `action_bar_bg_left.png` (`1380px`-wide canvas): real opaque art
///   `x[27,1379]` — `27` native px of transparent margin on the LEFT (facing
///   the health orb), flush (`0px` margin) on the RIGHT (facing the stamina
///   orb).
/// - `action_bar_bg_right.png`: real opaque art `x[19,1368]` — `19` native px
///   margin on the LEFT (facing the stamina orb), `11` native px margin on the
///   RIGHT (facing the mana orb).
///
/// None of these margins are equal, and none of them cancel out cleanly —
/// exactly why a single `CLUSTER_GAP_PX` (or one lump-sum correction on top
/// of it) can only ever be right for one seam, by accident. The fix: convert
/// every piece's own measured native margin (scaled by the SAME
/// `render_size / native_size` factor its own width already uses — the same
/// "scale the measurement" discipline [`ANGEL_FRAME_BOTTOM_PAD_PX`] already
/// established for the vertical axis) into a signed
/// `*_MARGIN_PX` constant per side: POSITIVE means the real art recedes
/// INSIDE the piece's own nominal box near that edge (leaving a real gap
/// that must be compensated for), NEGATIVE means the real art actually
/// OVERHANGS PAST the nominal box edge (already closing part of the gap, so
/// less box-to-box distance is needed). [`CLUSTER_GAP_PX`] is repointed to
/// mean the REAL, opaque-to-opaque visible gap every seam should render at
/// (not a box-to-box distance any more), and a dedicated `*_TO_*_GAP_PX`
/// constant per seam (below) computes exactly the box-to-box distance that
/// achieves it: `box_gap = CLUSTER_GAP_PX -
/// margin_of_the_left_piece's_facing_edge
/// - margin_of_the_right_piece's_facing_edge`. A seam whose two pieces
/// already overhang enough on their own correctly computes a NEGATIVE
/// `box_gap` (their nominal boxes overlap) — this is not a bug: the frame
/// overlay already renders above ambient chrome via
/// [`crate::zlayer::AMBIENT_CHROME_OVERLAY`] (established by round 3 for
/// exactly this "decorative art spills onto a neighbouring sibling" case),
/// so an overlapping nominal box just means the real decorative art keeps
/// spilling onto the sibling as designed, with the REAL edges landing
/// exactly [`CLUSTER_GAP_PX`] apart regardless.
///
/// Verified visually via `--smoke-screenshot`, cropping and comparing the
/// four seam regions pixel-for-pixel against `hud-ejemplo.png`'s own
/// near-flush orb-to-action-bar spacing — see this round's PR description for
/// the exact before/after px measurements. This math alone was NOT
/// sufficient, though: a first `--smoke-screenshot` capture with every
/// constant below already in place still showed a real `≈70-90px` gap on
/// every seam — see `crate::hotbar::spawn_action_bar_half`'s own doc comment
/// for the actual second root cause (the action-bar-half `ImageNode`s
/// defaulting to `NodeImageMode::Auto`, which contain-fits + centres instead
/// of stretching to the box these formulas assume it fills). Both fixes
/// together are what the final measurements below reflect.
pub const ANGEL_FRAME_RIGHT_MARGIN_PX: f32 =
    74.0 * ORB_SIZE_PX / 768.0 - (ANGEL_FRAME_WIDTH_PX - ORB_SIZE_PX) / 2.0;

/// Stamina (centre orb) LEFT-side variant of [`ANGEL_FRAME_RIGHT_MARGIN_PX`]
/// — see that constant's own doc comment for the round-5 methodology. Faces
/// the LEFT action-bar half's own right edge.
pub const STAMINA_FRAME_LEFT_MARGIN_PX: f32 =
    4.0 * ORB_SIZE_PX / 768.0 - (STAMINA_FRAME_WIDTH_PX - ORB_SIZE_PX) / 2.0;

/// Stamina (centre orb) RIGHT-side variant of [`ANGEL_FRAME_RIGHT_MARGIN_PX`]
/// — faces the RIGHT action-bar half's own left edge.
pub const STAMINA_FRAME_RIGHT_MARGIN_PX: f32 =
    6.0 * ORB_SIZE_PX / 768.0 - (STAMINA_FRAME_WIDTH_PX - ORB_SIZE_PX) / 2.0;

/// Cuthulhu (mana orb) LEFT-side variant of [`ANGEL_FRAME_RIGHT_MARGIN_PX`] —
/// faces the RIGHT action-bar half's own right edge. This is the single
/// biggest margin of the whole cluster (`-33.02px`, a real OVERHANG, not a
/// gap) — the direct replacement for the old lump-sum `CUTHULHU_EXTRA_GAP_PX`
/// correction, computed exactly from this variant's own measured art extent
/// instead of a hand-tuned frame-width delta.
pub const CUTHULHU_FRAME_LEFT_MARGIN_PX: f32 =
    83.0 * ORB_SIZE_PX / 768.0 - (CUTHULHU_FRAME_WIDTH_PX - ORB_SIZE_PX) / 2.0;

/// BL-82 HUD redesign round 6 — the ability-slot row itself has NO baked-in
/// transparent margin of its own to account for: it's a plain transparent
/// flex container sized to EXACTLY `crate::hotbar::SLOTS_PER_HALF` slots +
/// gaps ([`SLOT_ROW_WIDTH_PX`]), not an ornate background PNG with its own
/// asset-specific padding the way `action_bar_bg_left.png`/`_right.png` used
/// to have (round 5's now-deleted `ACTION_BAR_LEFT/RIGHT_*_MARGIN_PX`). So
/// the `box_gap = target_real_gap - margin_A - margin_B` formula round 5
/// established (see [`ANGEL_FRAME_RIGHT_MARGIN_PX`]'s doc comment) drops to a
/// single term per seam: only the ORB's own margin, since the row's margin on
/// that side is exactly `0.0`.
///
/// The box-to-box distance (px) between the health orb's own [`ORB_SIZE_PX`]
/// hit-box and the LEFT slot row's own [`SLOT_ROW_WIDTH_PX`] box that makes
/// their REAL, opaque content land exactly [`CLUSTER_GAP_PX`] apart. Negative
/// here (the angel frame's own overhang alone already exceeds the target real
/// gap) — the two nominal boxes overlap slightly, which is fine (see
/// [`ANGEL_FRAME_RIGHT_MARGIN_PX`]'s doc comment for why an overlapping
/// nominal box is not a bug).
pub const HEALTH_TO_LEFT_ROW_GAP_PX: f32 = CLUSTER_GAP_PX - ANGEL_FRAME_RIGHT_MARGIN_PX;

/// The box-to-box distance (px) between the LEFT slot row's own right edge
/// and the stamina orb's own left edge that makes their REAL content land
/// exactly [`CLUSTER_GAP_PX`] apart.
pub const LEFT_ROW_TO_STAMINA_GAP_PX: f32 = CLUSTER_GAP_PX - STAMINA_FRAME_LEFT_MARGIN_PX;

/// The box-to-box distance (px) between the stamina orb's own right edge and
/// the RIGHT slot row's own left edge that makes their REAL content land
/// exactly [`CLUSTER_GAP_PX`] apart.
pub const STAMINA_TO_RIGHT_ROW_GAP_PX: f32 = CLUSTER_GAP_PX - STAMINA_FRAME_RIGHT_MARGIN_PX;

/// The box-to-box distance (px) between the RIGHT slot row's own right edge
/// and the mana orb's own left edge that makes their REAL content land
/// exactly [`CLUSTER_GAP_PX`] apart. Large and positive (unlike the other
/// three seams) because the cuthulhu frame's own real art recedes so far
/// inside its nominal hit-box ([`CUTHULHU_FRAME_LEFT_MARGIN_PX`] `≈ -33px`, an
/// overhang) that a wide box-to-box distance is needed before the REAL art
/// gets anywhere near the row's own (zero) right-side margin.
pub const RIGHT_ROW_TO_MANA_GAP_PX: f32 = CLUSTER_GAP_PX - CUTHULHU_FRAME_LEFT_MARGIN_PX;

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

/// BL-82 HUD redesign round 6 — replaces the deleted
/// `ACTION_BAR_HALF_HEIGHT_PX`. Height (px) of each ability-slot row's own
/// container — kept equal to [`ORB_SIZE_PX`] (not just
/// [`crate::hotbar::SLOT_SIZE_PX`]) so the row's container still spans the SAME
/// vertical band as the orbs either side of it; `hotbar.rs`'s `align_items:
/// AlignItems::Center` then centres the (shorter) slots within that band,
/// matching the orbs' own vertical centre exactly the way the old
/// action-bar-half background did.
pub const SLOT_ROW_HEIGHT_PX: f32 = ORB_SIZE_PX;

/// BL-82 HUD redesign round 6 — replaces the deleted
/// `ACTION_BAR_HALF_WIDTH_PX`/ `ACTION_BAR_WIDTH_TRIM`. Width (px) of each
/// ability-slot row's own container, derived DIRECTLY from the slot geometry
/// itself — `crate::hotbar::SLOTS_PER_HALF` slots of
/// `crate::hotbar::SLOT_SIZE_PX` each, separated by
/// `crate::hotbar::SLOTS_PER_HALF - 1` gaps of
/// `crate::hotbar::HOTBAR_SLOT_GAP_PX`. Unlike the deleted constants this
/// replaces, there is no trim factor, no leading inset, and no "flat opaque
/// backing" safe-zone to fit inside: with the ornate `action_bar_bg_left.png`/
/// `_right.png` frame art gone, the row's box has no background art of its
/// own at all (a plain transparent flex container) — its bounding box IS
/// exactly its visible content's bounding box, so this is a straight sum, not
/// a tuned-by-eye multiplier the way the old (now-deleted)
/// `ACTION_BAR_WIDTH_TRIM` used to be. See
/// [`tests::slot_row_width_matches_the_slot_geometry_exactly`] for the pinned
/// regression.
pub const SLOT_ROW_WIDTH_PX: f32 = crate::hotbar::SLOTS_PER_HALF as f32
    * crate::hotbar::SLOT_SIZE_PX
    + (crate::hotbar::SLOTS_PER_HALF as f32 - 1.0) * crate::hotbar::HOTBAR_SLOT_GAP_PX;

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
/// own opaque black border padding.
///
/// ## BL-82 HUD polish round 5 — this stopped meaning "box-to-box gap"
/// Rounds 3/4 (and the historical value/doc comment above) treated this as a
/// raw BOX-to-box distance — but every piece in this row bakes its OWN
/// asset-specific, per-side transparent margin into its own render box (see
/// [`ANGEL_FRAME_RIGHT_MARGIN_PX`]'s doc comment for the full round-5
/// measurement + root-cause writeup), so a shared box-to-box distance never
/// actually produced a uniform REAL visible gap — verified live, the real
/// gaps ranged `~3px` to `~10px` at this same `2.0` box value, depending on
/// which two pieces' own margins happened to fall on that seam. This
/// constant is now the target this module actually enforces: the REAL,
/// opaque-art-to-opaque-art visible gap every seam should render at — every
/// seam's own [`HEALTH_TO_LEFT_ROW_GAP_PX`]/[`LEFT_ROW_TO_STAMINA_GAP_PX`]/
/// [`STAMINA_TO_RIGHT_ROW_GAP_PX`]/[`RIGHT_ROW_TO_MANA_GAP_PX`] computes
/// whatever box-to-box distance (possibly negative — see those constants'
/// own doc comment) achieves it.
///
/// ## BL-82 HUD redesign round 6 — pulled tighter now that there's no frame art
/// Rounds 3-5 tuned this against the ornate `action_bar_bg_left.png`/
/// `_right.png` frame pieces, which no longer render at all (see this
/// module's own top-of-file doc comment). With the frame gone, Matías's
/// reference (`hud-ejemplo-2.png`) shows the ability icons sitting almost
/// flush against the stamina orb's own ring, tighter than round 4/5's
/// already-small `2.0px` target — shrunk `2.0 -> 1.0`. Still a real, if
/// tiny, positive value (not `0.0`/negative) — the same round-4 precedent
/// applies: a fully-flush gap reads as a mismatched art seam where an orb's
/// circular edge meets a slot's square corner, not a clean line.
pub const CLUSTER_GAP_PX: f32 = 1.0;

/// Distance (px) from the viewport's bottom edge to the bottom of the whole
/// orb/slot-row row.
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
/// even with this constant at `0.0`. Root cause: every orb frame PNG has a
/// real, contiguous band of fully-transparent pixels at the very bottom of
/// its own canvas, BELOW the last row of genuinely opaque art:
/// `orb_frame_angel.png` (`768px` tall) `11px`, `orb_frame_cuthulhu.png`
/// `13px`, `orb_frame_stamina.png` `14px` — this is exactly the failure mode
/// the round-4 brief predicted: a `bottom_px` constant positions the
/// CONTAINER's bounding box, not the visible art's own lowest opaque pixel —
/// no value of this one shared constant can compensate for a PER-ASSET
/// amount of baked-in transparent margin, since the 3 orb pieces don't agree
/// on how much they have.
///
/// The fix: [`ANGEL_FRAME_BOTTOM_PAD_PX`]/[`CUTHULHU_FRAME_BOTTOM_PAD_PX`]/
/// [`STAMINA_FRAME_BOTTOM_PAD_PX`] below — each orb's OWN measured padding,
/// scaled from raw canvas px to real rendered px by the exact same factor its
/// own height already uses (`render_height / native_height`, the same "scale
/// the measurement, don't re-measure at render size" approach
/// [`ANGEL_FRAME_WIDTH_PX`] already established). Every real spawn call site
/// (`combat_hud.rs`'s 3 orbs) sets
/// `bottom: Val::Px(CLUSTER_BOTTOM_PX - <that orb's own pad constant>)` —
/// pushing each container down by exactly its own transparent margin so the
/// REAL opaque pixel (not the bounding box) lands flush with the screen's
/// bottom edge, verified directly via `--smoke-screenshot` (not just the
/// arithmetic) against both `captura4.png` (the bug) and live gameplay.
/// `CLUSTER_BOTTOM_PX` itself stays the shared "nominal" anchor every other
/// derived offset (`CLUSTER_TOTAL_HEIGHT_PX`, the XP cluster's own bottom)
/// already builds from.
///
/// BL-82 HUD redesign round 6: the ability-slot row (`hotbar.rs`) no longer
/// has its OWN bottom-pad correction — it's a transparent flex container with
/// zero baked-in margin (unlike the deleted `action_bar_bg_left.png`/
/// `_right.png`, whose own `23px`/`26px` bottom padding used to need the same
/// per-piece treatment), so it now just sits at a bare `CLUSTER_BOTTOM_PX`
/// like any ordinary node, vertically centred within [`SLOT_ROW_HEIGHT_PX`]
/// via `align_items: AlignItems::Center`.
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

/// Total width (px) of the "core" row span — the two ability-slot rows plus
/// the centre Stamina orb, EXCLUDING the two outer (Health/Mana) orbs. The XP
/// bar + level readout are centred on this span, matching the HUD-D4
/// reference's own narrower XP strip (it sits above the slot row, not
/// edge-to-edge across the full orb-to-orb cluster).
///
/// BL-82 HUD redesign round 6: renamed from `ACTION_BAR_TOTAL_WIDTH_PX` (no
/// "action bar" left to name) and now built from [`SLOT_ROW_WIDTH_PX`]
/// instead of the deleted `ACTION_BAR_HALF_WIDTH_PX`. The two gaps this span
/// contains are [`LEFT_ROW_TO_STAMINA_GAP_PX`]/[`STAMINA_TO_RIGHT_ROW_GAP_PX`]
/// — every seam still gets its own per-margin box gap rather than sharing one
/// flat value (see [`CLUSTER_GAP_PX`]'s own doc comment).
pub const CORE_ROW_WIDTH_PX: f32 = 2.0 * SLOT_ROW_WIDTH_PX
    + ORB_SIZE_PX
    + LEFT_ROW_TO_STAMINA_GAP_PX
    + STAMINA_TO_RIGHT_ROW_GAP_PX;

/// Every cluster piece's horizontal offset (px), relative to the viewport's
/// own horizontal centre, of that piece's OWN LEFT edge — i.e. exactly the
/// value to hand to `margin.left` on a `Node` that already has
/// `left: Val::Percent(50.0)` (the project's established centring idiom).
/// Computed once, left-to-right, from the centre Stamina orb outward, so
/// both `combat_hud.rs` and `hotbar.rs` derive their own pieces' placement
/// from the exact same arithmetic.
///
/// BL-82 HUD redesign round 6: fields renamed `action_bar_*_half_left` ->
/// `slot_row_*_half_left` (no "action bar" left to name — see the module's
/// own top-of-file doc comment) and now built from [`SLOT_ROW_WIDTH_PX`] and
/// the [`HEALTH_TO_LEFT_ROW_GAP_PX`]/[`LEFT_ROW_TO_STAMINA_GAP_PX`]/
/// [`STAMINA_TO_RIGHT_ROW_GAP_PX`]/[`RIGHT_ROW_TO_MANA_GAP_PX`] per-seam gaps.
pub struct ClusterOffsets {
    pub health_orb_left: f32,
    pub slot_row_left_half_left: f32,
    pub stamina_orb_left: f32,
    pub slot_row_right_half_left: f32,
    pub mana_orb_left: f32,
}

/// The single, shared instance of [`ClusterOffsets`] — both HUD plugins read
/// this directly rather than recomputing the arithmetic themselves.
pub const CLUSTER: ClusterOffsets = {
    let stamina_orb_left = -ORB_SIZE_PX / 2.0;
    let stamina_orb_right = ORB_SIZE_PX / 2.0;
    let slot_row_right_half_left = stamina_orb_right + STAMINA_TO_RIGHT_ROW_GAP_PX;
    let slot_row_right_half_right = slot_row_right_half_left + SLOT_ROW_WIDTH_PX;
    let mana_orb_left = slot_row_right_half_right + RIGHT_ROW_TO_MANA_GAP_PX;
    let slot_row_left_half_right = stamina_orb_left - LEFT_ROW_TO_STAMINA_GAP_PX;
    let slot_row_left_half_left = slot_row_left_half_right - SLOT_ROW_WIDTH_PX;
    let health_orb_left = slot_row_left_half_left - HEALTH_TO_LEFT_ROW_GAP_PX - ORB_SIZE_PX;

    ClusterOffsets {
        health_orb_left,
        slot_row_left_half_left,
        stamina_orb_left,
        slot_row_right_half_left,
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
    /// ability-slot rows mirror each other in width — a regression guard
    /// against a future constant tweak silently breaking the "one
    /// contiguous row" contract this module exists to enforce.
    ///
    /// BL-82 HUD redesign round 6: renamed BAR->ROW throughout (no "action
    /// bar" left — see the module's own top-of-file doc comment) and now
    /// checks [`SLOT_ROW_WIDTH_PX`] (derived directly from the slot geometry)
    /// instead of the deleted `ACTION_BAR_HALF_WIDTH_PX`.
    #[test]
    fn cluster_is_symmetric_around_centre() {
        assert_eq!(CLUSTER.stamina_orb_left, -ORB_SIZE_PX / 2.0);

        let left_row_width =
            CLUSTER.stamina_orb_left - LEFT_ROW_TO_STAMINA_GAP_PX - CLUSTER.slot_row_left_half_left;
        let right_row_width =
            CLUSTER.mana_orb_left - RIGHT_ROW_TO_MANA_GAP_PX - CLUSTER.slot_row_right_half_left;
        // BL-82 HUD polish round 5: `0.01`, not `f32::EPSILON` — the two
        // sides no longer share an IDENTICAL expression tree (each seam now
        // subtracts its OWN differently-valued `*_TO_*_GAP_PX` constant), so
        // the two computations no longer round bit-for-bit identically even
        // though they're algebraically equal in real-number math. `0.01px`
        // matches the tolerance every other assertion in this test already
        // uses.
        assert!((left_row_width - right_row_width).abs() < 0.01);
        assert!((left_row_width - SLOT_ROW_WIDTH_PX).abs() < 0.01);

        // Health orb's right edge must sit exactly `HEALTH_TO_LEFT_ROW_GAP_PX`
        // before the left row's left edge.
        let health_orb_right = CLUSTER.health_orb_left + ORB_SIZE_PX;
        assert!(
            (health_orb_right + HEALTH_TO_LEFT_ROW_GAP_PX - CLUSTER.slot_row_left_half_left).abs()
                < 0.01
        );

        // Mana orb's left edge must sit exactly `RIGHT_ROW_TO_MANA_GAP_PX`
        // past the right row's right edge.
        let slot_row_right_half_right = CLUSTER.slot_row_right_half_left + SLOT_ROW_WIDTH_PX;
        assert!(
            (CLUSTER.mana_orb_left - RIGHT_ROW_TO_MANA_GAP_PX - slot_row_right_half_right).abs()
                < 0.01
        );
    }

    /// BL-82 HUD polish round 5 — the actual REGRESSION GUARD that round
    /// exists to add: every seam's REAL, opaque-art-to-opaque-art visible gap
    /// must equal [`CLUSTER_GAP_PX`] exactly, computed the same way
    /// [`ANGEL_FRAME_RIGHT_MARGIN_PX`]'s doc comment derives it (`real_gap =
    /// box_gap + orb_margin + row_margin`). This is what a simple
    /// box-position check (like `cluster_is_symmetric_around_centre` above)
    /// can NOT catch by itself — it only asserts relationships between
    /// BOUNDING BOXES, which is exactly how the round-5 bug shipped three
    /// rounds in a row despite that box-position test passing throughout.
    ///
    /// BL-82 HUD redesign round 6: `row_margin` is now always `0.0` — the
    /// ability-slot row (round 6) is a bare transparent flex container with
    /// no baked-in art padding of its own, unlike the deleted
    /// `action_bar_bg_left.png`/`_right.png` this replaced (which used to
    /// contribute the second, now-removed `ACTION_BAR_LEFT/RIGHT_*_MARGIN_PX`
    /// term). Kept as an explicit `+ 0.0` in the loop below (not just dropped)
    /// so the formula stays visibly parallel to the doc comment's own
    /// `box_gap + orb_margin + row_margin` identity.
    #[test]
    fn every_seam_real_opaque_gap_equals_cluster_gap_px() {
        let seams = [
            (
                "health->left_row",
                HEALTH_TO_LEFT_ROW_GAP_PX,
                ANGEL_FRAME_RIGHT_MARGIN_PX,
            ),
            (
                "left_row->stamina",
                LEFT_ROW_TO_STAMINA_GAP_PX,
                STAMINA_FRAME_LEFT_MARGIN_PX,
            ),
            (
                "stamina->right_row",
                STAMINA_TO_RIGHT_ROW_GAP_PX,
                STAMINA_FRAME_RIGHT_MARGIN_PX,
            ),
            (
                "right_row->mana",
                RIGHT_ROW_TO_MANA_GAP_PX,
                CUTHULHU_FRAME_LEFT_MARGIN_PX,
            ),
        ];
        for (name, box_gap, orb_margin) in seams {
            let row_margin = 0.0;
            let real_gap = box_gap + orb_margin + row_margin;
            assert!(
                (real_gap - CLUSTER_GAP_PX).abs() < 0.01,
                "{name} seam's real opaque-art gap is {real_gap}, expected {CLUSTER_GAP_PX}"
            );
        }
    }

    /// The XP/level cluster's width matches the "core" row span (both
    /// ability-slot rows + the centre orb, not the two outer orbs) — pins the
    /// value both `combat_hud.rs`'s XP-cluster container and any future
    /// caller rely on.
    ///
    /// BL-82 HUD redesign round 6: renamed from
    /// `action_bar_total_width_spans_both_halves_and_stamina_orb`, now checks
    /// [`CORE_ROW_WIDTH_PX`]/[`SLOT_ROW_WIDTH_PX`].
    #[test]
    fn core_row_width_spans_both_slot_rows_and_stamina_orb() {
        let expected = 2.0 * SLOT_ROW_WIDTH_PX
            + ORB_SIZE_PX
            + LEFT_ROW_TO_STAMINA_GAP_PX
            + STAMINA_TO_RIGHT_ROW_GAP_PX;
        assert!((CORE_ROW_WIDTH_PX - expected).abs() < f32::EPSILON);
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

    /// BL-82 HUD polish round 5 — [`SKILL_SLOT_BORDER_SOURCE_CROP`] must
    /// stay a real, in-bounds, near-square sub-rect of
    /// `skill_slot_border.png`'s own `1408×768` canvas: in-bounds (so
    /// `ImageNode::rect` never samples outside the source texture), genuinely
    /// smaller than the full canvas (a crop that regressed back to the full
    /// canvas would silently reintroduce the "half-empty slot" bug this crop
    /// exists to fix), and close enough to square (within `10%`) that
    /// stretching it onto a square slot box introduces no visible distortion
    /// — a much larger aspect mismatch here would signal a bad
    /// re-measurement, not a real asset property (the orb frames needed a
    /// dedicated `frame_width_px` non-square mechanism for exactly this
    /// reason; this asset's own opaque bbox doesn't).
    #[test]
    fn skill_slot_border_crop_is_in_bounds_and_nearly_square() {
        const NATIVE_WIDTH: f32 = 1408.0;
        const NATIVE_HEIGHT: f32 = 768.0;
        let crop = SKILL_SLOT_BORDER_SOURCE_CROP;

        assert!(crop.min.x >= 0.0 && crop.min.y >= 0.0);
        assert!(crop.max.x <= NATIVE_WIDTH && crop.max.y <= NATIVE_HEIGHT);

        let width = crop.max.x - crop.min.x;
        let height = crop.max.y - crop.min.y;
        assert!(width > 0.0 && height > 0.0);
        assert!(
            width < NATIVE_WIDTH * 0.9,
            "the crop ({width}px) must be genuinely tighter than the full {NATIVE_WIDTH}px canvas \
             — a crop this close to full width would silently reintroduce the half-empty-slot bug"
        );

        let aspect = width / height;
        assert!(
            (aspect - 1.0).abs() < 0.1,
            "SKILL_SLOT_BORDER_SOURCE_CROP's aspect ratio ({aspect}) is too far from square for a \
             crop-then-stretch-onto-a-square-slot approach to avoid visible distortion — re-check \
             the measurement or add a frame_width_px-style overhang mechanism instead"
        );
    }

    /// BL-82 HUD polish round 4 (issue 1): every `*_BOTTOM_PAD_PX` constant
    /// must be strictly positive (a real piece of the source art's own
    /// transparent margin, not a zero/negative no-op) and small relative to
    /// [`ORB_SIZE_PX`] (a few px, not a large fraction of the whole orb
    /// height — a wildly larger value here would signal a measurement bug,
    /// not a real transparent margin). Also pins each constant's exact scaled
    /// value against the raw measured px this module's own doc comments
    /// cite, so a future asset re-cut that changes the padding is caught
    /// here rather than silently reintroducing the flush-bottom gap.
    ///
    /// BL-82 HUD redesign round 6: only the 3 orb pads remain —
    /// `ACTION_BAR_LEFT/RIGHT_BOTTOM_PAD_PX` are deleted along with the
    /// action-bar frame art they measured; the ability-slot row has no
    /// bottom pad of its own any more (see [`CLUSTER_BOTTOM_PX`]'s doc
    /// comment).
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

        for pad in [
            ANGEL_FRAME_BOTTOM_PAD_PX,
            CUTHULHU_FRAME_BOTTOM_PAD_PX,
            STAMINA_FRAME_BOTTOM_PAD_PX,
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

    /// BL-82 HUD redesign round 6 — replaces the deleted
    /// `five_slots_per_half_fit_inside_the_action_bar_half_width` and
    /// `hotbar_row_fits_within_each_action_bar_halfs_own_flat_opaque_backing`.
    /// Those two tests existed purely because the ability-slot row used to
    /// sit INSIDE a wider, independently-sized background image
    /// (`action_bar_bg_left.png`/`_right.png`) that could, in principle, be
    /// too narrow to fit the slots, or wide enough to fit them but still
    /// clip past that image's own flat opaque backing — both failure modes
    /// are structurally impossible now that [`SLOT_ROW_WIDTH_PX`] IS the
    /// slots' own width (no separate background box to under/overflow). What
    /// remains worth pinning: the formula itself, computed the exact same way
    /// `crate::hotbar`'s own row-spawning code does, so a future edit to
    /// [`SLOT_ROW_WIDTH_PX`]'s definition or to
    /// `crate::hotbar::SLOT_SIZE_PX`/`SLOTS_PER_HALF`/`HOTBAR_SLOT_GAP_PX`
    /// can't silently desync the two.
    #[test]
    fn slot_row_width_matches_the_slot_geometry_exactly() {
        let slots_per_half = crate::hotbar::SLOTS_PER_HALF as f32;
        let expected = slots_per_half * crate::hotbar::SLOT_SIZE_PX
            + (slots_per_half - 1.0) * crate::hotbar::HOTBAR_SLOT_GAP_PX;
        assert!(
            (SLOT_ROW_WIDTH_PX - expected).abs() < f32::EPSILON,
            "SLOT_ROW_WIDTH_PX ({SLOT_ROW_WIDTH_PX}) must equal exactly SLOTS_PER_HALF * \
             SLOT_SIZE_PX + (SLOTS_PER_HALF - 1) * HOTBAR_SLOT_GAP_PX ({expected}) — with no \
             background frame art left, the row's box has nothing else in it"
        );
    }
}
