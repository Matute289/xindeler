//! BL-82 EM-5.1 T56.1 — the Progress bar / globe primitive.
//!
//! The shape every always-on combat readout needs (health/energy/poise/XP —
//! legacy's `skillbar.rs` bars): a background track + a fill child whose
//! width tracks a `current/max` value. [`BarValue`] is the single piece of
//! state a caller (EM-5.2's mirror-reading systems) updates; [`update_bars`]
//! is the one system that turns it into a fill-width, `Changed<BarValue>`-
//! gated so it costs nothing on ticks where nothing changed.

use bevy::{
    asset::{Assets, Handle},
    ecs::{
        component::Component,
        hierarchy::Children,
        query::{Changed, With},
        system::{Commands, Query, ResMut},
    },
    image::Image,
    math::{Rect, Vec2},
    picking::Pickable,
    ui::{
        BackgroundColor, GlobalZIndex, Node, PositionType, Val,
        widget::{ImageNode, NodeImageMode},
    },
};

use crate::{orb_material::OrbLiquidMaterial, theme::HudTheme, zlayer};

/// The value a bar displays: `current` / `max`. Any caller (an EM-5.2 HUD
/// system reading `NetHealth`/`NetEnergy`/etc.) writes this; [`update_bars`]
/// is the only thing that reads it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct BarValue {
    pub current: f32,
    pub max: f32,
}

impl BarValue {
    #[must_use]
    pub fn new(current: f32, max: f32) -> Self { Self { current, max } }

    /// Clamped `[0, 1]` fraction — `max <= 0` degrades to `0.0` rather than
    /// NaN/inf (spec §3.2 "degrade clean", applied to widget math too).
    #[must_use]
    pub fn fraction(self) -> f32 {
        if self.max <= 0.0 {
            0.0
        } else {
            (self.current / self.max).clamp(0.0, 1.0)
        }
    }
}

/// Marks a bar's fill child (the node whose width [`update_bars`] resizes).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudBarFill;

/// Marks a bar's container (the node carrying [`BarValue`] + [`Children`]
/// that includes exactly one [`HudBarFill`]).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudBar;

/// Spawns a themed bar: a background-track container (`width`×`height` px,
/// `bg_color`) with one [`HudBarFill`] child (`fill_color`) sized to `value`'s
/// current fraction. Returns the container [`bevy::ecs::entity::Entity`] —
/// callers update the display later purely by mutating [`BarValue`] on it
/// (via `Commands::entity(id).insert(BarValue::new(..))` or a `Query<&mut
/// BarValue>`), never by touching the fill child directly.
pub fn spawn_bar(
    commands: &mut Commands,
    theme: &HudTheme,
    fill_color: bevy::color::Color,
    bg_color: bevy::color::Color,
    width_px: f32,
    height_px: f32,
    value: BarValue,
) -> bevy::ecs::entity::Entity {
    commands
        .spawn((
            HudBar,
            value,
            Node {
                width: Val::Px(width_px),
                height: Val::Px(height_px),
                overflow: bevy::ui::Overflow::clip(),
                // `BorderRadius` is a FIELD of `Node` in Bevy 0.19, not a
                // standalone `Component` — see `panel.rs`'s own note.
                border_radius: bevy::ui::BorderRadius::all(Val::Px(theme.radius.sm)),
                ..Default::default()
            },
            BackgroundColor(bg_color),
        ))
        .with_children(|parent| {
            parent.spawn((
                HudBarFill,
                Node {
                    width: Val::Percent(value.fraction() * 100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                BackgroundColor(fill_color),
            ));
        })
        .id()
}

/// Resizes every bar's fill child to its container's current
/// [`BarValue`] fraction — the only system that ever touches a fill child's
/// `Node`. `Changed<BarValue>`-gated: costs nothing on frames where nothing
/// updated the value (most frames, for a full-health/energy player standing
/// still). `With<HudBar>`-scoped (added alongside [`HudOrbBar`], BL-82
/// EM-5.17) so this doesn't also iterate every orb bar's container — those
/// resize via [`update_orb_bars`] instead.
pub(crate) fn update_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudBar>)>,
    mut fills: Query<&mut Node, With<HudBarFill>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(mut node) = fills.get_mut(child) {
                node.width = Val::Percent(value.fraction() * 100.0);
            }
        }
    }
}

/// BL-82 EM-5.17 T57.8, reworked twice: BL-82 EM-5.17 Phase 0 follow-up
/// (Matías's "the liquid SHRINKS instead of DRAINS" report) fixed the
/// squash bug by splitting this into a fixed-size image nested inside a
/// resizing CPU-clip window; this later rework (BL-82 EM-5.17, Matías's
/// "the orbs need a real shader-based wave + stone-reveal effect" request)
/// REPLACES that CPU-clip window entirely — marks an orb bar's liquid
/// render layer, now a
/// [`bevy::prelude::MaterialNode`]`<`[`OrbLiquidMaterial`]`>` instead of a
/// plain `ImageNode`. **This node's own `Node.width`/ `Node.height` is FIXED**
/// (`Val::Px`, the orb's full size minus `liquid_inset_px` on each side) and
/// NEVER touched by [`update_orb_bars`] — the fraction (AND, new in this
/// rework, the wave-animated surface line plus dark stone/metal depletion
/// reveal) are now expressed entirely inside
/// the shader via [`OrbLiquidMaterial::fill_fraction`]/`time`, computed
/// per-fragment rather than by resizing a CPU clip box. This is strictly
/// MORE capable than the old two-node clip-window nesting (a flat rectangle
/// can only ever reveal a flat line — see [`OrbLiquidMaterial`]'s own module
/// doc comment for the full wave/stone rationale), so the old
/// `HudOrbBarFillClip` wrapper this node used to live inside no longer exists.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudOrbBarFill;

/// Marks an orb bar's container — the vertical, bottom-anchored counterpart
/// to [`HudBar`]. See [`spawn_orb_bar`]'s doc comment for why this is a
/// PARALLEL primitive rather than an `Orientation` parameter grafted onto
/// [`spawn_bar`]/[`HudBar`] itself.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudOrbBar;

/// Spawns a themed VERTICAL, bottom-anchored bar — the liquid-fill-orb shape
/// a later phase needs (spec §3.1): a circular frame with a liquid image
/// inside that rises/falls with a `current/max` fraction. `fill_image` is
/// the liquid texture (e.g. `health_liquid.png`); `frame_image`, if given, is
/// spawned as a full-size sibling `ImageNode` ON TOP of the fill, with
/// [`Pickable::IGNORE`] so it never blocks interaction with whatever's
/// underneath — the frame PNGs in the HUD-D4 pack have an alpha-transparent
/// centre, so the fill (now a real [`crate::orb_material::OrbLiquidMaterial`]
/// shader — see that module's doc comment for the wave/stone-reveal
/// rationale) shows through the frame's circular cutout while the frame's own
/// opaque ring/carving still renders over the fill's square corners; no
/// separate circular-clip mask is needed for either layer.
///
/// ## Why a PARALLEL primitive, not an `Orientation` param on `spawn_bar`
/// [`spawn_bar`]'s signature is a real, already-shipped API with existing
/// call sites (`combat_hud.rs`/`hotbar.rs`/etc.) — adding a required
/// `Orientation` parameter to it would break every one of them (the exact
/// thing this whole phase's "additive, don't touch existing call sites"
/// brief rules out). A second, size-matched function pair
/// (`spawn_orb_bar`/[`HudOrbBar`]/[`update_orb_bars`], mirroring `spawn_bar`/
/// [`HudBar`]/[`update_bars`]'s exact shape one-for-one) costs a little
/// duplication but means zero existing caller needs to change, which this
/// crate's own established precedent favours (see `slot.rs`'s "prefer the
/// wrapper that needs zero existing-callsite changes" posture, applied here
/// to a sibling function instead of a wrapper since the fill AXIS itself
/// differs, not just an extra parameter).
///
/// ## `fill_source_crop`/`frame_source_crop` — fixing the "squashed ellipse"
/// sizing bug AND the follow-up "liquid doesn't fill the frame's window"
/// sizing mismatch
/// The HUD-D4 pack's `orb_frame_*.png`/`*_liquid.png` files are a wide
/// `1408×768`-ish canvas, NOT a square crop of just the circle — the artist
/// left a big transparent margin left/right of a centred circle (room for
/// the gargoyle-wing frame extensions). Verified directly against the actual
/// on-disk files: the liquid art's own opaque bounding box is
/// `x[354,1055] y[28,727]` (≈701×699px) and the frame's enclosed circular
/// cutout is `x[523,896] y[197,577]` (≈372×380px), both centred within a few
/// px of the canvas's own horizontal centre (`x≈704-710` of 1407-1408).
/// Stretching the FULL wide canvas onto a square `width_px`×`height_px` box
/// (the old behaviour) squashes that circle into an ellipse — visibly
/// mismatched against the reference mockup's round orbs. Each `*_source_crop`
/// param, when given, is a pixel-space `Rect` (in that image's OWN SOURCE
/// texture coordinates) applied via `ImageNode::rect` + `NodeImageMode::
/// Stretch` — passing a genuinely SQUARE sub-rect means that square crop
/// stretches onto the square `width_px`×`height_px` box with NO distortion,
/// keeping the circle round. `None` preserves the old "stretch the whole
/// source image" behaviour (existing/mock call sites that don't care about
/// real art alignment).
///
/// The two crops are DELIBERATELY SEPARATE parameters, not one shared
/// `source_crop` (BL-82 EM-5.17 Phase 0 second follow-up, Matías's HUD-D4
/// art-alignment report: "the liquid sits smaller than the frame's circular
/// window, a dark ring of frame material shows between them"). A single
/// shared crop stretched onto the same square box preserves the SOURCE
/// pixel ratio between the liquid's ≈701px circle and the frame's ≈372px
/// hole no matter which square window is chosen — so tuning one shared
/// `source_crop` can never change how big the liquid renders RELATIVE to
/// the frame's own opening, only how much of each image's outer padding is
/// visible. Cropping the frame image TIGHTER than the liquid image (see
/// [`crate::bar`]'s callers in `hud_layout`'s per-variant
/// `*_FRAME_SOURCE_CROP` constants vs the shared `hud_layout::
/// ORB_SOURCE_CROP`) makes the frame's hole occupy more of the shared
/// `width_px`×`height_px` box, independent of the liquid's own scale — BL-82
/// orb crop round 2 went further and made the frame crop (plus the
/// `liquid_inset_px` this function takes) per-variant rather than one shared
/// constant, since a single shared value clipped some variants' decorative
/// art while still under/over-sizing others' liquid; see `hud_layout`'s
/// `ANGEL_FRAME_SOURCE_CROP` doc comment for the full story.
///
/// ## `frame_width_px` — BL-82 orb crop round 3: the frame is NOT forced
/// square any more
/// Round 2's own doc comment on `hud_layout::ANGEL_FRAME_SOURCE_CROP`
/// admitted the square-crop approach was geometrically incomplete: the
/// angel/cuthulhu frame art's decorative wings genuinely span more pixels
/// than the canvas is tall (angel `879px`, cuthulhu `1165px` wide vs a
/// `768px`-tall canvas), so ANY square crop, no matter how loose, still
/// clips real wing pixels — confirmed live (Matías's round-3 report: the
/// angel/cuthulhu orbs are still visibly missing wing/tentacle art after
/// round 2 shipped). The only real fix is to stop rendering the frame into
/// the same square `width_px`×`height_px` box the liquid/hit-box use:
/// `frame_width_px`, when it differs from `width_px`, sizes the frame
/// overlay's OWN `Node` to `frame_width_px`×`height_px` (still undistorted,
/// since the caller derives it from the crop's real aspect ratio at the
/// same scale factor `height_px` uses) and centres it horizontally on the
/// container, so a wider frame spills symmetrically past both the left and
/// right edges instead of being squeezed into them. This deliberately
/// escapes the container's own `overflow: Overflow::clip_y()` (below) on
/// the x axis — see [`zlayer::AMBIENT_CHROME_OVERLAY`] for why the overlay
/// also gets its own explicit `GlobalZIndex` so the spillover always paints
/// over whichever ambient-chrome sibling (e.g. the action bar background)
/// it now visually overlaps. Passing `frame_width_px == width_px` (every
/// pre-round-3 call site that doesn't need the wider box, and every
/// variant whose art already fits, e.g. the stamina orb) reproduces the
/// exact old square-frame behaviour byte-for-byte.
///
/// ## `materials` — the shader now doing the fraction/wave/stone-reveal work
/// BL-82 EM-5.17 (Matías's "the orbs need a real shader-based agitated-water
/// plus dark stone/metal depletion reveal" request): the liquid layer is now a
/// [`bevy::prelude::MaterialNode`]`<`[`OrbLiquidMaterial`]`>` instead of a
/// plain `ImageNode`, so this function needs write access to that material
/// asset collection to `add` the orb's own material instance — `materials`
/// is exactly that, the same "pass the `ResMut<Assets<M>>` the caller already
/// has" pattern `map_view::spawn_map_screens` uses for
/// `MinimapFadeMaterial`. `fill_source_crop`, when given, converts straight
/// to the material's pixel-space `crop_min`/`crop_size` uniforms (see
/// [`OrbLiquidMaterial`]'s own module doc comment for why those stay pixel
/// space rather than being pre-divided into UV here) — `frame_source_crop`
/// is UNCHANGED, still an `ImageNode::rect` crop, since the frame overlay
/// this function spawns is untouched by this rework.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn spawn_orb_bar(
    commands: &mut Commands,
    theme: &HudTheme,
    materials: &mut Assets<OrbLiquidMaterial>,
    fill_image: Handle<Image>,
    frame_image: Option<Handle<Image>>,
    fill_source_crop: Option<Rect>,
    frame_source_crop: Option<Rect>,
    frame_width_px: f32,
    liquid_inset_px: f32,
    width_px: f32,
    height_px: f32,
    value: BarValue,
) -> bevy::ecs::entity::Entity {
    let (crop_min, crop_size) = match fill_source_crop {
        Some(rect) => (rect.min, rect.max - rect.min),
        // `Vec2::ZERO` is `OrbLiquidMaterial`'s own "no crop, sample the
        // full [0,1]^2 UV" sentinel — see its module doc comment.
        None => (Vec2::ZERO, Vec2::ZERO),
    };
    let fill_material = materials.add(OrbLiquidMaterial::new(
        fill_image,
        crop_min,
        crop_size,
        value.fraction(),
    ));

    let container = commands
        .spawn((HudOrbBar, value, Node {
            width: Val::Px(width_px),
            height: Val::Px(height_px),
            // BL-82 orb crop round 3: only `y` clips now — the liquid/
            // hit-box stay contained vertically (unchanged from before),
            // but `x` must stay `Visible` so a `frame_width_px` wider than
            // `width_px` can spill past the container's own left/right
            // edges instead of being clipped back into a square (see
            // `spawn_orb_bar`'s own doc comment on `frame_width_px`). A
            // liquid-fill orb is circular in the final art (the frame
            // PNG's alpha carves the circle) — clipping is enough since
            // the frame overlay masks the corners; see the module doc
            // comment above.
            overflow: bevy::ui::Overflow::clip_y(),
            border_radius: bevy::ui::BorderRadius::all(Val::Px(theme.radius.sm)),
            ..Default::default()
        }))
        .with_children(|parent| {
            // The liquid render layer itself: FIXED `Val::Px` size —
            // deliberately NOT resized as the fraction changes (that was the
            // old CPU-clip bug this whole primitive already fixed once —
            // see [`HudOrbBarFill`]'s doc comment). `liquid_inset_px` insets
            // it EQUALLY on all four sides (BL-82 EM-5.17 Phase 0 second
            // follow-up), centring it a few px inside the orb's full
            // `width_px`×`height_px` box rather than flush with it — see
            // [`spawn_orb_bar`]'s own doc comment for why the liquid needs
            // to render a hair SMALLER than the frame's hole rather than
            // exactly flush: a few px of deliberate slack means sub-pixel
            // rounding at different UI-scale factors can never read as the
            // liquid overlapping the frame's ring. The fraction (and, new in
            // this rework, the wave-animated surface + stone-reveal) is now
            // expressed entirely inside [`OrbLiquidMaterial`]'s shader, not
            // by this `Node`'s own size.
            parent.spawn((
                HudOrbBarFill,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(liquid_inset_px),
                    bottom: Val::Px(liquid_inset_px),
                    width: Val::Px((width_px - 2.0 * liquid_inset_px).max(0.0)),
                    height: Val::Px((height_px - 2.0 * liquid_inset_px).max(0.0)),
                    ..Default::default()
                },
                bevy::prelude::MaterialNode(fill_material),
            ));
        })
        .id();

    if let Some(frame_image) = frame_image {
        let mut frame_image_node = ImageNode::new(frame_image);
        if let Some(rect) = frame_source_crop {
            frame_image_node.rect = Some(rect);
            frame_image_node.image_mode = NodeImageMode::Stretch;
        }
        // BL-82 orb crop round 3: `frame_width_px` may be WIDER than
        // `width_px` (see `spawn_orb_bar`'s own doc comment) — `overhang`
        // is how far the frame spills past the container on EACH side,
        // negative `left` pulling it out symmetrically so it stays centred
        // on the same hole the liquid/hit-box are centred on. When
        // `frame_width_px == width_px` (every variant/call site that
        // doesn't need the wider box) `overhang` is exactly `0.0`,
        // reproducing the old flush square overlay byte-for-byte.
        let overhang = (frame_width_px - width_px) / 2.0;
        commands.entity(container).with_children(|parent| {
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(-overhang),
                    top: Val::Px(0.0),
                    width: Val::Px(frame_width_px),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                frame_image_node,
                // The frame is pure decoration on top of the fill — it must
                // never intercept pointer events meant for whatever's
                // beneath it (the orb's own hover/tooltip, if any).
                Pickable::IGNORE,
                // A wider-than-`width_px` frame now visually spills onto
                // whichever ambient-chrome sibling sits next to this orb
                // (e.g. the action bar background) — an explicit
                // `GlobalZIndex` above the shared ambient-chrome layer
                // makes that spillover deterministically paint on top
                // instead of depending on the two independent `Startup`
                // plugins' unspecified spawn order (see
                // [`zlayer::AMBIENT_CHROME_OVERLAY`]'s own doc comment).
                GlobalZIndex(zlayer::AMBIENT_CHROME_OVERLAY),
            ));
        });
    }

    container
}

/// Writes every orb bar's current [`BarValue`] fraction onto its
/// [`HudOrbBarFill`] child's [`OrbLiquidMaterial::fill_fraction`] uniform —
/// the vertical counterpart to [`update_bars`], same `Changed<BarValue>`
/// gate. Reworked (BL-82 EM-5.17, the wave/stone-reveal shader rework) from
/// the old "resize a CPU-clip window's `Node.height`" mechanism to a direct
/// material-asset write, since the fraction (plus the wave/stone-reveal) is
/// now entirely the shader's job — see [`HudOrbBarFill`]'s doc comment.
/// **Never touches the [`HudOrbBarFill`] entity's own `Node`** — that node's
/// size stays a constant `Val::Px` forever (the fixed liquid-inset box); only
/// the material asset's uniform reacts to the fraction. See
/// `orb_bar_material_fraction_tracks_value_changes` below for the coverage.
pub(crate) fn update_orb_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudOrbBar>)>,
    fills: Query<&bevy::prelude::MaterialNode<OrbLiquidMaterial>, With<HudOrbBarFill>>,
    mut materials: ResMut<Assets<OrbLiquidMaterial>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(material_node) = fills.get(child)
                && let Some(mut material) = materials.get_mut(material_node)
            {
                material.fill_fraction = value.fraction();
            }
        }
    }
}

/// BL-82 EM-5.17 Phase 5, reworked BL-82 EM-5.17 Phase 0's follow-up fix
/// applied to this sibling primitive (Matías's "the liquid SHRINKS instead
/// of DRAINS" report, originally fixed only on [`HudOrbBarFill`]/
/// [`update_orb_bars`] — this horizontal image bar was added in the same
/// session and shipped with the identical squash bug, caught by
/// `bevy-migration-reviewer`) — marks a horizontal, image-filled bar's fill
/// child. **This node's own `Node.width`/`Node.height` is now FIXED**
/// (`Val::Px`, matching the bar's full `width_px`/`height_px`) and NEVER
/// touched by [`update_horizontal_image_bars`] — that was the bug: resizing
/// an `ImageNode`-carrying `Node` directly makes `ImageNode`'s default
/// stretch-to-fit rescale/squash the texture into the shrunk box, which
/// reads as the stagger bar's fill texture visually stretching/squashing
/// horizontally rather than draining. The fraction is now expressed purely
/// by [`HudImageBarFillClip`], the wrapper this node lives inside — see that
/// type's doc comment for the full clip-reveal mechanism. Distinct from
/// [`HudBarFill`] (which paints a flat [`BackgroundColor`]) since the
/// boss/target nameplate's health + stagger bars use the HUD-D4 pack's own
/// dedicated fill textures (a red `ImageNode` fill under `boss_bar_frame.png`;
/// `boss_stagger_full_bar.png` over `boss_stagger_bar.png`'s track), not a
/// themed flat color — a third, parallel primitive following the exact
/// "sibling function, not a parameter grafted onto an existing one"
/// precedent [`spawn_orb_bar`]'s own doc comment already established for
/// vertical orb bars (horizontal fill here instead of `spawn_bar`'s width
/// resize, image instead of color).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudImageBarFill;

/// BL-82 EM-5.17 Phase 5 follow-up — the fraction-reveal CLIP WINDOW wrapped
/// around a horizontal image bar's [`HudImageBarFill`] image (the
/// nameplate health/stagger bars this primitive serves — see
/// [`spawn_horizontal_image_bar`] — are UNCHANGED by the orb liquid's BL-82
/// EM-5.17 wave/stone-reveal shader rework; this CPU-clip mechanism stays the
/// simple, correct choice here since those bars have no "empty vessel"
/// material-reveal requirement). This is the node
/// [`update_horizontal_image_bars`] resizes (`Node.width = value.fraction() *
/// 100%`), left-anchored with `overflow: Overflow::clip()`: as the fraction
/// shrinks, this window's RIGHT edge sweeps toward the left (never resizing
/// the fill image inside it), progressively hiding more of the fixed-size
/// fill graphic from the right side in — a real "meter draining" look, the
/// CSS `clip-path`/`overflow:hidden` idiom applied to `bevy_ui`'s own
/// `Overflow::clip()` primitive (the same CPU-clip idiom the orb liquid used
/// to use too, before this rework moved that one to a shader — see
/// [`HudOrbBarFill`]'s doc comment). This is a SECOND, INNER clip layer,
/// nested inside the container's own outer `overflow: Overflow::clip()`
/// (spawn-time only, never resized) — the two clips serve different jobs and
/// neither can substitute for the other.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudImageBarFillClip;

/// Marks a horizontal image-filled bar's container.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudImageBar;

/// Spawns a themed HORIZONTAL bar whose fill is an `ImageNode` (not a flat
/// color) sized to `value`'s current fraction — the nameplate health/
/// stagger-bar shape (spec §3.5): a background TRACK image (e.g.
/// `boss_stagger_bar.png`) with a left-anchored fill image (e.g.
/// `boss_stagger_full_bar.png`) clipped to the fraction, optionally topped
/// by a separate frame `ImageNode` (`Pickable::IGNORE`, same convention
/// [`spawn_orb_bar`]'s frame overlay already uses) so the frame's own
/// opaque border still reads over the fill's square-clipped edge. Returns
/// the container entity — callers update the display purely by mutating
/// [`BarValue`] on it, exactly like [`spawn_bar`]/[`spawn_orb_bar`].
#[must_use]
pub fn spawn_horizontal_image_bar(
    commands: &mut Commands,
    track_image: Handle<Image>,
    fill_image: Handle<Image>,
    frame_image: Option<Handle<Image>>,
    width_px: f32,
    height_px: f32,
    value: BarValue,
) -> bevy::ecs::entity::Entity {
    let container = commands
        .spawn((
            HudImageBar,
            value,
            Node {
                width: Val::Px(width_px),
                height: Val::Px(height_px),
                overflow: bevy::ui::Overflow::clip(),
                ..Default::default()
            },
            ImageNode::new(track_image),
        ))
        .with_children(|parent| {
            // The fraction-reveal clip window (see [`HudImageBarFillClip`]'s
            // doc comment) — left-anchored, its OWN width is what tracks
            // `value.fraction()`, and it clips (`Overflow::clip()`) whatever
            // of the always-full-size fill image to its right sticks out
            // past it.
            parent
                .spawn((HudImageBarFillClip, Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(value.fraction() * 100.0),
                    height: Val::Percent(100.0),
                    overflow: bevy::ui::Overflow::clip(),
                    ..Default::default()
                }))
                .with_children(|clip_parent| {
                    // The fill graphic itself: FIXED `Val::Px` size —
                    // deliberately NOT `Val::Percent(100.0)` of the
                    // (shrinking) clip window's own box, which would
                    // re-squash the texture right back into the exact bug
                    // this rework fixes.
                    clip_parent.spawn((
                        HudImageBarFill,
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            width: Val::Px(width_px),
                            height: Val::Px(height_px),
                            ..Default::default()
                        },
                        ImageNode::new(fill_image),
                    ));
                });
        })
        .id();

    if let Some(frame_image) = frame_image {
        commands.entity(container).with_children(|parent| {
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                ImageNode::new(frame_image),
                Pickable::IGNORE,
            ));
        });
    }

    container
}

/// Resizes every horizontal image bar's fraction-reveal CLIP WINDOW
/// ([`HudImageBarFillClip`]) to its container's current [`BarValue`]
/// fraction, left-anchored (grows the window's WIDTH) — the horizontal
/// counterpart to [`update_orb_bars`], same `Changed<BarValue>` gate.
/// **Never touches [`HudImageBarFill`]** (the fill image itself, one level
/// deeper) — that split is the whole fix for the "fill squashes instead of
/// drains" bug: only the clip window's box may react to the fraction, the
/// fill graphic's own `Node` must stay a constant `Val::Px` forever. See
/// `image_bar_fill_image_never_resizes_only_the_clip_wrapper_does` below for
/// the regression guard.
pub(crate) fn update_horizontal_image_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudImageBar>)>,
    mut clips: Query<&mut Node, With<HudImageBarFillClip>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(mut node) = clips.get_mut(child) {
                node.width = Val::Percent(value.fraction() * 100.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::{asset::AssetPlugin, prelude::*};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, update_bars);
        app
    }

    /// Spawning a bar at half value gives the fill child a 50% width; a
    /// later `BarValue` mutation resizes it on the next update — the T56.1
    /// acceptance bar for this primitive.
    #[test]
    fn bar_fill_tracks_value_changes() {
        let mut app = new_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_bar(
                &mut commands,
                &theme,
                theme.palette.health,
                theme.palette.health_bg,
                200.0,
                20.0,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let fill_width = |app: &mut App, container: Entity| -> Val {
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let fill_entity = children
                .into_iter()
                .find(|&e| app.world().get::<HudBarFill>(e).is_some())
                .expect("bar has a fill child");
            app.world().get::<Node>(fill_entity).unwrap().width
        };

        assert_eq!(fill_width(&mut app, container), Val::Percent(50.0));

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 25.0;
        app.update();

        assert_eq!(fill_width(&mut app, container), Val::Percent(25.0));
    }

    /// A zero/negative `max` degrades to a `0.0` fraction, never NaN — the
    /// "degrade clean" rule applied to bar math.
    #[test]
    fn zero_max_degrades_to_zero_fraction() {
        let value = BarValue::new(10.0, 0.0);
        assert_eq!(value.fraction(), 0.0);
    }

    fn new_orb_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<OrbLiquidMaterial>();
        app.add_systems(Update, update_orb_bars);
        app
    }

    /// Test-only helper: `spawn_orb_bar` needs simultaneous `&mut Commands` +
    /// `&mut Assets<OrbLiquidMaterial>`, which `App`'s test helpers can't hand
    /// out together directly — `World::resource_scope` is the standard idiom
    /// for exactly this (borrow the resource out, hand back a `&mut World` to
    /// build `Commands` from, same pattern `combat_hud.rs`'s own test uses).
    #[allow(clippy::too_many_arguments)]
    fn spawn_orb_bar_for_test(
        app: &mut App,
        theme: &HudTheme,
        fill_image: Handle<Image>,
        frame_image: Option<Handle<Image>>,
        fill_source_crop: Option<Rect>,
        frame_source_crop: Option<Rect>,
        frame_width_px: f32,
        liquid_inset_px: f32,
        width_px: f32,
        height_px: f32,
        value: BarValue,
    ) -> Entity {
        app.world_mut()
            .resource_scope::<Assets<OrbLiquidMaterial>, _>(|world, mut materials| {
                let mut commands = world.commands();
                let id = spawn_orb_bar(
                    &mut commands,
                    theme,
                    &mut materials,
                    fill_image,
                    frame_image,
                    fill_source_crop,
                    frame_source_crop,
                    frame_width_px,
                    liquid_inset_px,
                    width_px,
                    height_px,
                    value,
                );
                world.flush();
                id
            })
    }

    /// Finds an orb bar container's [`HudOrbBarFill`] child and reads its
    /// [`OrbLiquidMaterial::fill_fraction`] uniform straight off the material
    /// asset — the one place every orb test below needs to look to observe
    /// the fraction, now that it lives in a shader uniform instead of a
    /// resizable `Node`.
    fn fill_fraction_of(app: &mut App, container: Entity) -> f32 {
        let fill_entity = find_fill_child(app, container);
        let material_node = app
            .world()
            .get::<bevy::prelude::MaterialNode<OrbLiquidMaterial>>(fill_entity)
            .expect("the fill child carries a MaterialNode<OrbLiquidMaterial>");
        let materials = app.world().resource::<Assets<OrbLiquidMaterial>>();
        materials.get(material_node).unwrap().fill_fraction
    }

    /// Finds an orb bar container's [`HudOrbBarFill`] child entity — now a
    /// DIRECT child of the container (the CPU-clip wrapper this used to be
    /// nested inside no longer exists, see [`HudOrbBarFill`]'s own doc
    /// comment).
    fn find_fill_child(app: &mut App, container: Entity) -> Entity {
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_some())
            .expect("orb bar has a fill child")
    }

    /// `spawn_orb_bar` at half value writes `0.5` onto the fill child's
    /// [`OrbLiquidMaterial::fill_fraction`] uniform; a later `BarValue`
    /// mutation updates it again on the next [`update_orb_bars`] run — the
    /// T57.8 acceptance bar for the vertical orb-bar primitive, reworked
    /// (BL-82 EM-5.17 wave/stone-reveal shader rework) from the old
    /// CPU-clip-window height resize to this material-uniform write (see
    /// [`orb_bar_fill_node_never_resizes_only_the_material_fraction_does`]
    /// for the companion "the `Node` itself never changes" guard).
    #[test]
    fn orb_bar_material_fraction_tracks_value_changes() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            None,
            None,
            160.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(50.0, 100.0),
        );
        app.update();

        assert_eq!(fill_fraction_of(&mut app, container), 0.5);

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 25.0;
        app.update();

        assert_eq!(fill_fraction_of(&mut app, container), 0.25);
    }

    /// Regression test for the ROOT CAUSE Matías hit live pre-shader-rework
    /// (PR #142): "the health orb's liquid appeared completely
    /// empty/invisible... yet I survived roughly 3 more hits before actually
    /// dying." The OLD `HudOrbBarFillClip` CPU-clip window measured its
    /// height as a flat fraction of the CONTAINER's full height, while the
    /// liquid image lived `liquid_inset_px` above the container's true bottom
    /// — for a real inset (e.g. the health orb's `ANGEL_LIQUID_INSET_PX =
    /// 14.0` at `ORB_SIZE_PX = 160.0`) any `fraction < liquid_inset_px /
    /// height_px` (here, `< 8.75%`) meant the clip window never reached where
    /// the liquid started, so it rendered fully invisible despite real,
    /// nonzero HP. This shader-based rework structurally cannot reintroduce
    /// that class of bug: [`OrbLiquidMaterial::fill_fraction`] maps directly
    /// and proportionally onto the [`HudOrbBarFill`] node's OWN local UV
    /// space (already sized to exactly `height_px - 2.0 * liquid_inset_px`
    /// by `spawn_orb_bar` — see that function's doc comment), with no
    /// separate "measured from the container's true bottom" clip window to
    /// go out of sync with the liquid's own inset — asserted here at the
    /// same low-but-nonzero 5% fraction PR #142's original regression test
    /// used.
    #[test]
    fn health_orb_fill_fraction_has_no_low_fraction_dead_zone_with_inset() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();
        const HEIGHT_PX: f32 = 160.0;
        const LIQUID_INSET_PX: f32 = 14.0;
        const LOW_FRACTION: f32 = 0.05; // 5% HP — below the old 8.75% dead zone.

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            None,
            None,
            HEIGHT_PX,
            LIQUID_INSET_PX,
            HEIGHT_PX,
            HEIGHT_PX,
            BarValue::new(LOW_FRACTION * 100.0, 100.0),
        );
        app.update();

        assert_eq!(
            fill_fraction_of(&mut app, container),
            LOW_FRACTION,
            "the material's fill_fraction must equal the real fraction even at a low value with \
             a nonzero liquid_inset_px — this mechanism has no separate clip-window geometry that \
             could go out of sync with the inset, unlike the old CPU-clip mechanism PR #142 fixed"
        );
    }

    /// Regression guard for the ORIGINAL bug this primitive's CPU-clip
    /// rework once fixed (Matías's report: the health/stamina/mana orbs
    /// visually SHRANK as the resource depleted instead of looking like
    /// liquid draining) — still enforced after the BL-82 EM-5.17 wave/
    /// stone-reveal shader rework: the [`HudOrbBarFill`] node's own
    /// `Node.width`/`Node.height` must stay a FIXED `Val::Px` (the orb's full
    /// size minus `liquid_inset_px`) across a `BarValue` mutation — ONLY the
    /// material's `fill_fraction` uniform (covered above) may react to the
    /// fraction. A future change that goes back to resizing this `Node`
    /// directly must fail this test.
    #[test]
    fn orb_bar_fill_node_never_resizes_only_the_material_fraction_does() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            None,
            None,
            160.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(50.0, 100.0),
        );
        app.update();

        let fill_node_size = |app: &mut App, container: Entity| -> (Val, Val) {
            let fill_entity = find_fill_child(app, container);
            let node = app.world().get::<Node>(fill_entity).unwrap();
            (node.width, node.height)
        };

        let full_size = (Val::Px(160.0), Val::Px(160.0));
        assert_eq!(
            fill_node_size(&mut app, container),
            full_size,
            "the liquid layer must spawn at the orb's FULL fixed size"
        );

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 5.0;
        app.update();

        assert_eq!(
            fill_node_size(&mut app, container),
            full_size,
            "changing BarValue must NEVER resize the liquid layer's Node — only the material's \
             fill_fraction uniform may change"
        );
        assert_eq!(
            fill_fraction_of(&mut app, container),
            0.05,
            "the material's fill_fraction must still track the changed BarValue"
        );
    }

    /// `fill_source_crop`, when given, converts straight to the fill
    /// material's pixel-space `crop_min`/`crop_size` uniforms (`rect.min`/
    /// `rect.max - rect.min`) — the material-based fix for the "squashed
    /// ellipse" sizing bug the old `ImageNode::rect` crop used to handle
    /// (see `spawn_orb_bar`'s own doc comment). `frame_source_crop` is
    /// UNCHANGED — still an `ImageNode::rect` crop on the frame overlay,
    /// since the frame is untouched by the shader rework. `None` leaves the
    /// material's crop at `Vec2::ZERO`/`Vec2::ZERO` (the "no crop, full UV"
    /// sentinel — see [`OrbLiquidMaterial`]'s own doc comment).
    #[test]
    fn orb_bar_source_crop_applies_pixel_rect_to_material_and_frame_image() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();
        let crop = Rect::new(320.0, 0.0, 1088.0, 768.0);

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            Some(crop),
            Some(crop),
            160.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(1.0, 1.0),
        );
        app.update();

        let fill_entity = find_fill_child(&mut app, container);
        let material_node = app
            .world()
            .get::<bevy::prelude::MaterialNode<OrbLiquidMaterial>>(fill_entity)
            .unwrap();
        let materials = app.world().resource::<Assets<OrbLiquidMaterial>>();
        let material = materials.get(material_node).unwrap();
        assert_eq!(material.crop_min, crop.min);
        assert_eq!(material.crop_size, crop.max - crop.min);

        let container_children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let frame_entity = container_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_none())
            .expect("orb bar has a frame overlay child");
        let image_node = app.world().get::<ImageNode>(frame_entity).unwrap();
        assert_eq!(image_node.rect, Some(crop));
        assert_eq!(image_node.image_mode, NodeImageMode::Stretch);
    }

    /// The fix for BL-82 EM-5.17 Phase 0's second follow-up (Matías's HUD-D4
    /// art-alignment report — the liquid sat smaller than the frame's
    /// circular window with a visible dark-ring gap): `fill_source_crop` and
    /// `frame_source_crop` are genuinely INDEPENDENT — a caller may crop the
    /// frame image tighter than the liquid image (making the frame's own
    /// hole occupy more of the shared box) without that choice being forced
    /// onto the liquid's crop too. Still holds after the shader rework: the
    /// fill's crop now lives on the material, the frame's crop still on its
    /// `ImageNode`, and the two remain independent parameters.
    #[test]
    fn orb_bar_fill_and_frame_source_crops_are_independent() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();
        let fill_crop = Rect::new(320.0, 0.0, 1088.0, 768.0);
        let frame_crop = Rect::new(352.0, 29.0, 1068.0, 745.0);

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            Some(fill_crop),
            Some(frame_crop),
            160.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(1.0, 1.0),
        );
        app.update();

        let fill_entity = find_fill_child(&mut app, container);
        let material_node = app
            .world()
            .get::<bevy::prelude::MaterialNode<OrbLiquidMaterial>>(fill_entity)
            .unwrap();
        let materials = app.world().resource::<Assets<OrbLiquidMaterial>>();
        let material = materials.get(material_node).unwrap();
        assert_eq!(material.crop_min, fill_crop.min);
        assert_eq!(material.crop_size, fill_crop.max - fill_crop.min);

        let container_children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let frame_entity = container_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_none())
            .expect("orb bar has a frame overlay child");
        assert_eq!(
            app.world().get::<ImageNode>(frame_entity).unwrap().rect,
            Some(frame_crop)
        );
    }

    /// `liquid_inset_px` shrinks the liquid layer EQUALLY on all four sides,
    /// centring it inside the orb's full `width_px`×`height_px` box instead
    /// of spawning it flush with the box edges — the other half of the
    /// BL-82 EM-5.17 Phase 0 second follow-up fix (a tighter
    /// `frame_source_crop` makes the frame's hole bigger; this inset makes
    /// the liquid a hair smaller, so the liquid's edge sits fully inside
    /// the hole with no overlap even under sub-pixel rounding). Unaffected
    /// by the shader rework — this is still a plain `Node` position/size.
    #[test]
    fn orb_bar_liquid_inset_shrinks_and_centers_fill_node() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            None,
            None,
            160.0,
            8.0,
            160.0,
            160.0,
            BarValue::new(1.0, 1.0),
        );
        app.update();

        let fill_entity = find_fill_child(&mut app, container);
        let node = app.world().get::<Node>(fill_entity).unwrap();
        assert_eq!(node.left, Val::Px(8.0));
        assert_eq!(node.bottom, Val::Px(8.0));
        assert_eq!(node.width, Val::Px(144.0));
        assert_eq!(node.height, Val::Px(144.0));
    }

    /// The optional frame overlay, when given, spawns as a SECOND child
    /// carrying [`Pickable::IGNORE`] — the "decoration never blocks
    /// interaction with what's underneath" contract.
    #[test]
    fn orb_bar_frame_overlay_ignores_picking() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            None,
            None,
            160.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(1.0, 1.0),
        );
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        assert_eq!(children.len(), 2, "fill child + frame overlay child");
        let frame_entity = children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_none())
            .expect("a non-fill (frame) child exists");
        assert_eq!(
            *app.world().get::<Pickable>(frame_entity).unwrap(),
            Pickable::IGNORE
        );
    }

    /// Omitting the frame overlay spawns only the fill child — the frame
    /// stays a genuinely OPTIONAL parameter.
    #[test]
    fn orb_bar_without_frame_spawns_only_fill_child() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            None,
            None,
            None,
            160.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(1.0, 1.0),
        );
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        assert_eq!(children.len(), 1, "only the fill child, no frame overlay");
    }

    /// BL-82 orb crop round 3 — the regression guard for the actual fix:
    /// `frame_width_px` WIDER than `width_px` must render the frame overlay
    /// as its own wider `Node`, centred symmetrically (equal overhang on
    /// both sides) on the container rather than squeezed into it, AND that
    /// overlay must carry its own [`GlobalZIndex`] (so it deterministically
    /// paints over a sibling ambient-chrome element it now visually spills
    /// onto — see [`crate::zlayer::AMBIENT_CHROME_OVERLAY`]'s doc comment).
    /// This is exactly the mechanism `hud_layout::CUTHULHU_FRAME_WIDTH_PX`
    /// relies on to show the mana orb's wing art in full instead of the
    /// square-crop clipping round 2 shipped. Unaffected by the shader
    /// rework — the frame overlay itself is untouched.
    #[test]
    fn orb_bar_wider_frame_overlay_spills_symmetrically_with_its_own_z_index() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = spawn_orb_bar_for_test(
            &mut app,
            &theme,
            Handle::default(),
            Some(Handle::default()),
            None,
            None,
            200.0,
            0.0,
            160.0,
            160.0,
            BarValue::new(1.0, 1.0),
        );
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let frame_entity = children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_none())
            .expect("orb bar has a frame overlay child");

        let node = app.world().get::<Node>(frame_entity).unwrap();
        assert_eq!(node.width, Val::Px(200.0));
        // (200 - 160) / 2 == 20 px overhang on each side.
        assert_eq!(node.left, Val::Px(-20.0));

        assert_eq!(
            *app.world().get::<GlobalZIndex>(frame_entity).unwrap(),
            GlobalZIndex(zlayer::AMBIENT_CHROME_OVERLAY)
        );
    }

    fn new_image_bar_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, update_horizontal_image_bars);
        app
    }

    /// `spawn_horizontal_image_bar` at half value gives the fraction-reveal
    /// CLIP WINDOW ([`HudImageBarFillClip`], the container's direct child) a
    /// 50% WIDTH (left-anchored, same axis as [`spawn_bar`]'s flat-color
    /// fill, but image-backed) — the nameplate health/stagger-bar acceptance
    /// case (BL-82 EM-5.17 Phase 5), updated for the follow-up clip-reveal
    /// rework: the CLIP WINDOW is what tracks the fraction now, not the fill
    /// image itself (see
    /// [`image_bar_fill_image_never_resizes_only_the_clip_wrapper_does`]).
    #[test]
    fn image_bar_fill_tracks_value_changes_by_width() {
        let mut app = new_image_bar_app();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_horizontal_image_bar(
                &mut commands,
                Handle::default(),
                Handle::default(),
                Some(Handle::default()),
                360.0,
                24.0,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let clip_width = |app: &mut App, container: Entity| -> Val {
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let clip_entity = children
                .into_iter()
                .find(|&e| app.world().get::<HudImageBarFillClip>(e).is_some())
                .expect("image bar has a clip-window child");
            app.world().get::<Node>(clip_entity).unwrap().width
        };

        assert_eq!(clip_width(&mut app, container), Val::Percent(50.0));

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 25.0;
        app.update();

        assert_eq!(clip_width(&mut app, container), Val::Percent(25.0));
    }

    /// Regression guard for the exact bug this rework fixes (the boss
    /// stagger bar's `boss_stagger_full_bar.png` fill visually
    /// stretched/squashed horizontally as the meter depleted instead of
    /// looking like it drained — the unfixed sibling of the vertical orb-bar
    /// bug Matías originally reported, caught by `bevy-migration-reviewer`).
    /// Root cause: the old [`update_horizontal_image_bars`] resized the
    /// `ImageNode`-carrying fill child's own `Node.width` directly —
    /// `ImageNode`'s stretch-to-fit then rescales/squashes the fill texture
    /// into the shrunk box. This asserts the fill image's `Node`
    /// ([`HudImageBarFill`], now a GRANDCHILD nested inside
    /// [`HudImageBarFillClip`]) stays a FIXED `Val::Px` matching the bar's
    /// full size across a `BarValue` mutation — only the clip wrapper
    /// (covered above) may react to the fraction. A future change that goes
    /// back to resizing the fill image directly must fail this test.
    #[test]
    fn image_bar_fill_image_never_resizes_only_the_clip_wrapper_does() {
        let mut app = new_image_bar_app();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_horizontal_image_bar(
                &mut commands,
                Handle::default(),
                Handle::default(),
                Some(Handle::default()),
                360.0,
                24.0,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let fill_image_size = |app: &mut App, container: Entity| -> (Val, Val) {
            let container_children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let clip_entity = container_children
                .into_iter()
                .find(|&e| app.world().get::<HudImageBarFillClip>(e).is_some())
                .expect("image bar has a clip-window child");
            let clip_children: Vec<Entity> = app
                .world()
                .get::<Children>(clip_entity)
                .unwrap()
                .iter()
                .collect();
            let fill_entity = clip_children
                .into_iter()
                .find(|&e| app.world().get::<HudImageBarFill>(e).is_some())
                .expect("clip window has a fill-image grandchild");
            let node = app.world().get::<Node>(fill_entity).unwrap();
            (node.width, node.height)
        };

        let full_size = (Val::Px(360.0), Val::Px(24.0));
        assert_eq!(
            fill_image_size(&mut app, container),
            full_size,
            "the fill image must spawn at the bar's FULL fixed size"
        );

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 5.0;
        app.update();

        assert_eq!(
            fill_image_size(&mut app, container),
            full_size,
            "changing BarValue must NEVER resize the fill image itself — only the clip wrapper's \
             width may change"
        );
    }

    /// The optional frame overlay, when given, spawns as a SECOND child
    /// carrying [`Pickable::IGNORE`] — same contract as
    /// [`orb_bar_frame_overlay_ignores_picking`].
    #[test]
    fn image_bar_frame_overlay_ignores_picking() {
        let mut app = new_image_bar_app();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_horizontal_image_bar(
                &mut commands,
                Handle::default(),
                Handle::default(),
                Some(Handle::default()),
                360.0,
                24.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        assert_eq!(children.len(), 2, "clip-window child + frame overlay child");
        let frame_entity = children
            .into_iter()
            .find(|&e| app.world().get::<HudImageBarFillClip>(e).is_none())
            .expect("a non-clip-window (frame) child exists");
        assert_eq!(
            *app.world().get::<Pickable>(frame_entity).unwrap(),
            Pickable::IGNORE
        );
    }

    /// Omitting the frame overlay spawns only the fraction-reveal clip
    /// window — the frame stays a genuinely OPTIONAL parameter.
    #[test]
    fn image_bar_without_frame_spawns_only_fill_child() {
        let mut app = new_image_bar_app();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_horizontal_image_bar(
                &mut commands,
                Handle::default(),
                Handle::default(),
                None,
                360.0,
                24.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        assert_eq!(
            children.len(),
            1,
            "only the clip-window child, no frame overlay"
        );
    }
}
