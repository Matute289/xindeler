//! BL-82 EM-5.1 T56.1 — the Progress bar / globe primitive.
//!
//! The shape every always-on combat readout needs (health/energy/poise/XP —
//! legacy's `skillbar.rs` bars): a background track + a fill child whose
//! width tracks a `current/max` value. [`BarValue`] is the single piece of
//! state a caller (EM-5.2's mirror-reading systems) updates; [`update_bars`]
//! is the one system that turns it into a fill-width, `Changed<BarValue>`-
//! gated so it costs nothing on ticks where nothing changed.

use bevy::{
    asset::Handle,
    ecs::{
        component::Component,
        hierarchy::Children,
        query::{Changed, With},
        system::{Commands, Query},
    },
    image::Image,
    math::Rect,
    picking::Pickable,
    ui::{
        BackgroundColor, Node, PositionType, Val,
        widget::{ImageNode, NodeImageMode},
    },
};

use crate::theme::HudTheme;

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

/// BL-82 EM-5.17 T57.8, reworked BL-82 EM-5.17 Phase 0 follow-up (Matías's
/// "the liquid SHRINKS instead of DRAINS" report) — marks an orb bar's
/// liquid-image child. **This node's own `Node.width`/`Node.height` is now
/// FIXED** (`Val::Px`, matching the orb's full `width_px`/`height_px`) and
/// NEVER touched by [`update_orb_bars`] — that was the bug: resizing an
/// `ImageNode`-carrying `Node` directly makes `ImageNode`'s default
/// stretch-to-fit rescale/squash the texture into the shrunk box, which
/// reads as the liquid shrinking rather than draining. The fraction is now
/// expressed purely by [`HudOrbBarFillClip`], the wrapper this node lives
/// inside — see that type's doc comment for the full clip-reveal mechanism.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudOrbBarFill;

/// BL-82 EM-5.17 Phase 0 follow-up — the fraction-reveal CLIP WINDOW wrapped
/// around an orb bar's [`HudOrbBarFill`] image. This is the node
/// [`update_orb_bars`] resizes (`Node.height = value.fraction() * 100%`),
/// bottom-anchored with `overflow: Overflow::clip()`: as the fraction
/// shrinks, this window's TOP edge sinks toward the bottom (never resizing
/// the liquid image inside it), progressively hiding more of the fixed-size
/// liquid graphic from the top down — a real "liquid level draining inside a
/// fixed-size glass" look, the CSS `clip-path`/`overflow:hidden` idiom
/// applied to `bevy_ui`'s own `Overflow::clip()` primitive. This is a SECOND,
/// INNER clip layer, nested inside the `container`'s own outer
/// `overflow: Overflow::clip()` (which only masks the square box's corners
/// against the circular frame art, spawn-time only, never resized) — the two
/// clips serve different jobs and neither can substitute for the other.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct HudOrbBarFillClip;

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
/// centre, so the fill shows through the frame's circular cutout while the
/// frame's own opaque ring/carving still renders over the fill's square
/// corners (no separate circular-clip shader needed for this v1 — see
/// `crate::orb_material`'s module doc comment for the full `UiMaterial`
/// spike + why v1 deliberately stays with this CPU-clip mechanism).
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
/// [`crate::bar`]'s callers in `hud_layout::ORB_FRAME_SOURCE_CROP` vs
/// `hud_layout::ORB_SOURCE_CROP`) makes the frame's hole occupy more of the
/// shared `width_px`×`height_px` box, independent of the liquid's own scale.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn spawn_orb_bar(
    commands: &mut Commands,
    theme: &HudTheme,
    fill_image: Handle<Image>,
    frame_image: Option<Handle<Image>>,
    fill_source_crop: Option<Rect>,
    frame_source_crop: Option<Rect>,
    liquid_inset_px: f32,
    width_px: f32,
    height_px: f32,
    value: BarValue,
) -> bevy::ecs::entity::Entity {
    let mut fill_image_node = ImageNode::new(fill_image);
    if let Some(rect) = fill_source_crop {
        fill_image_node.rect = Some(rect);
        fill_image_node.image_mode = NodeImageMode::Stretch;
    }

    let container = commands
        .spawn((HudOrbBar, value, Node {
            width: Val::Px(width_px),
            height: Val::Px(height_px),
            overflow: bevy::ui::Overflow::clip(),
            // A liquid-fill orb is circular in the final art (the frame
            // PNG's alpha carves the circle) — a plain square clip
            // region is enough since the frame overlay masks the
            // corners; see the module doc comment above.
            border_radius: bevy::ui::BorderRadius::all(Val::Px(theme.radius.sm)),
            ..Default::default()
        }))
        .with_children(|parent| {
            // The fraction-reveal clip window (see [`HudOrbBarFillClip`]'s
            // doc comment) — bottom-anchored, its OWN height is what tracks
            // `value.fraction()`, and it clips (`Overflow::clip()`) whatever
            // of the always-full-size liquid image below sticks out above
            // it.
            parent
                .spawn((HudOrbBarFillClip, Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(value.fraction() * 100.0),
                    overflow: bevy::ui::Overflow::clip(),
                    ..Default::default()
                }))
                .with_children(|clip_parent| {
                    // The liquid graphic itself: FIXED `Val::Px` size —
                    // deliberately NOT `Val::Percent(100.0)` of the
                    // (shrinking) clip window's own box, which would
                    // re-squash the texture right back into the exact bug
                    // this rework fixes. `liquid_inset_px` insets it
                    // EQUALLY on all four sides (BL-82 EM-5.17 Phase 0
                    // second follow-up), centring it a few px inside the
                    // orb's full `width_px`×`height_px` box rather than
                    // flush with it — see [`spawn_orb_bar`]'s own doc
                    // comment for why the liquid needs to render a hair
                    // SMALLER than the frame's hole rather than exactly
                    // flush: a few px of deliberate slack means sub-pixel
                    // rounding at different UI-scale factors can never read
                    // as the liquid overlapping the frame's ring. Still
                    // bottom-anchored (via the `bottom: Val::Px(liquid_inset_px)`
                    // offset) inside the clip window so the window's
                    // fraction-driven reveal still tracks a real "liquid
                    // level," just measured from `liquid_inset_px` above the
                    // container's true bottom instead of the container's
                    // bottom exactly.
                    clip_parent.spawn((
                        HudOrbBarFill,
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(liquid_inset_px),
                            bottom: Val::Px(liquid_inset_px),
                            width: Val::Px((width_px - 2.0 * liquid_inset_px).max(0.0)),
                            height: Val::Px((height_px - 2.0 * liquid_inset_px).max(0.0)),
                            ..Default::default()
                        },
                        fill_image_node,
                    ));
                });
        })
        .id();

    if let Some(frame_image) = frame_image {
        let mut frame_image_node = ImageNode::new(frame_image);
        if let Some(rect) = frame_source_crop {
            frame_image_node.rect = Some(rect);
            frame_image_node.image_mode = NodeImageMode::Stretch;
        }
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
                frame_image_node,
                // The frame is pure decoration on top of the fill — it must
                // never intercept pointer events meant for whatever's
                // beneath it (the orb's own hover/tooltip, if any).
                Pickable::IGNORE,
            ));
        });
    }

    container
}

/// Resizes every orb bar's fraction-reveal CLIP WINDOW ([`HudOrbBarFillClip`])
/// to its container's current [`BarValue`] fraction, bottom-anchored (grows
/// the window's HEIGHT, unlike [`update_bars`]'s width resize) — the
/// vertical counterpart to [`update_bars`], same `Changed<BarValue>` gate.
/// **Never touches [`HudOrbBarFill`]** (the liquid image itself, one level
/// deeper) — that split is the whole fix for the "liquid shrinks instead of
/// drains" bug: only the clip window's box may react to the fraction, the
/// liquid graphic's own `Node` must stay a constant `Val::Px` forever. See
/// `orb_bar_fill_image_never_resizes_only_the_clip_wrapper_does` below for
/// the regression guard.
pub(crate) fn update_orb_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudOrbBar>)>,
    mut clips: Query<&mut Node, With<HudOrbBarFillClip>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(mut node) = clips.get_mut(child) {
                node.height = Val::Percent(value.fraction() * 100.0);
            }
        }
    }
}

/// BL-82 EM-5.17 Phase 5 — marks a horizontal, image-filled bar's fill child
/// (the node [`update_horizontal_image_bars`] resizes by WIDTH). Distinct
/// from [`HudBarFill`] (which paints a flat [`BackgroundColor`]) since the
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
            parent.spawn((
                HudImageBarFill,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(value.fraction() * 100.0),
                    height: Val::Percent(100.0),
                    overflow: bevy::ui::Overflow::clip(),
                    ..Default::default()
                },
                ImageNode::new(fill_image),
            ));
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

/// Resizes every horizontal image bar's fill child to its container's
/// current [`BarValue`] fraction (WIDTH, left-anchored) — the image-fill
/// counterpart to [`update_bars`], same `Changed<BarValue>` gate.
pub(crate) fn update_horizontal_image_bars(
    bars: Query<(&BarValue, &Children), (Changed<BarValue>, With<HudImageBar>)>,
    mut fills: Query<&mut Node, With<HudImageBarFill>>,
) {
    for (value, children) in &bars {
        for &child in children.iter() {
            if let Ok(mut node) = fills.get_mut(child) {
                node.width = Val::Percent(value.fraction() * 100.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

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
        app.add_systems(Update, update_orb_bars);
        app
    }

    /// `spawn_orb_bar` at half value gives the fraction-reveal CLIP WINDOW
    /// ([`HudOrbBarFillClip`], the container's direct child) a 50% HEIGHT
    /// (bottom-anchored, unlike the horizontal bar's width fill); a later
    /// `BarValue` mutation resizes it on the next update — the T57.8
    /// acceptance bar for the vertical orb-bar primitive, updated for the
    /// BL-82 EM-5.17 Phase 0 follow-up clip-reveal rework: the CLIP WINDOW
    /// is what tracks the fraction now, not the liquid image itself (see
    /// [`orb_bar_fill_image_never_resizes_only_the_clip_wrapper_does`]).
    #[test]
    fn orb_bar_fill_tracks_value_changes_by_height() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                None,
                None,
                0.0,
                160.0,
                160.0,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let clip_height = |app: &mut App, container: Entity| -> Val {
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let clip_entity = children
                .into_iter()
                .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_some())
                .expect("orb bar has a clip-window child");
            app.world().get::<Node>(clip_entity).unwrap().height
        };

        assert_eq!(clip_height(&mut app, container), Val::Percent(50.0));

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 25.0;
        app.update();

        assert_eq!(clip_height(&mut app, container), Val::Percent(25.0));
    }

    /// Regression guard for the exact bug this rework fixes (Matías's
    /// report: the health/stamina/mana orbs visually SHRANK as the resource
    /// depleted instead of looking like liquid draining). Root cause: the
    /// old [`update_orb_bars`] resized the `ImageNode`-carrying fill child's
    /// own `Node.height` directly — `ImageNode`'s stretch-to-fit then
    /// rescales/squashes the liquid texture into the shrunk box. This
    /// asserts the liquid image's `Node` ([`HudOrbBarFill`], now a
    /// GRANDCHILD nested inside [`HudOrbBarFillClip`]) stays a FIXED
    /// `Val::Px` matching the orb's full size across a `BarValue` mutation
    /// — only the clip wrapper (covered above) may react to the fraction. A
    /// future change that goes back to resizing the fill image directly
    /// must fail this test.
    #[test]
    fn orb_bar_fill_image_never_resizes_only_the_clip_wrapper_does() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                None,
                None,
                0.0,
                160.0,
                160.0,
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
                .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_some())
                .expect("orb bar has a clip-window child");
            let clip_children: Vec<Entity> = app
                .world()
                .get::<Children>(clip_entity)
                .unwrap()
                .iter()
                .collect();
            let fill_entity = clip_children
                .into_iter()
                .find(|&e| app.world().get::<HudOrbBarFill>(e).is_some())
                .expect("clip window has a fill-image grandchild");
            let node = app.world().get::<Node>(fill_entity).unwrap();
            (node.width, node.height)
        };

        let full_size = (Val::Px(160.0), Val::Px(160.0));
        assert_eq!(
            fill_image_size(&mut app, container),
            full_size,
            "the liquid image must spawn at the orb's FULL fixed size"
        );

        app.world_mut()
            .get_mut::<BarValue>(container)
            .unwrap()
            .current = 5.0;
        app.update();

        assert_eq!(
            fill_image_size(&mut app, container),
            full_size,
            "changing BarValue must NEVER resize the liquid image itself — only the clip \
             wrapper's height may change"
        );
    }

    /// `fill_source_crop`/`frame_source_crop`, when given, are each applied
    /// to their OWN `ImageNode` as a pixel-space source `rect` +
    /// `NodeImageMode::Stretch` — the fix for the "squashed ellipse" sizing
    /// bug (see `spawn_orb_bar`'s own doc comment on the parameters). `None`
    /// leaves an `ImageNode` at its default `rect`/`image_mode` (whole-image
    /// stretch), preserving the pre-fix behaviour for callers that don't
    /// pass real HUD-D4 art.
    #[test]
    fn orb_bar_source_crop_applies_rect_and_stretch_to_fill_and_frame() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();
        let crop = Rect::new(320.0, 0.0, 1088.0, 768.0);

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                Some(crop),
                Some(crop),
                0.0,
                160.0,
                160.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let container_children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let clip_entity = container_children
            .iter()
            .copied()
            .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_some())
            .expect("orb bar has a clip-window child");
        let frame_entity = container_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_none())
            .expect("orb bar has a frame overlay child");
        let clip_children: Vec<Entity> = app
            .world()
            .get::<Children>(clip_entity)
            .unwrap()
            .iter()
            .collect();
        let fill_entity = clip_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_some())
            .expect("clip window has a fill-image grandchild");

        for entity in [fill_entity, frame_entity] {
            let image_node = app.world().get::<ImageNode>(entity).unwrap();
            assert_eq!(image_node.rect, Some(crop));
            assert_eq!(image_node.image_mode, NodeImageMode::Stretch);
        }
    }

    /// The fix for BL-82 EM-5.17 Phase 0's second follow-up (Matías's HUD-D4
    /// art-alignment report — the liquid sat smaller than the frame's
    /// circular window with a visible dark-ring gap): `fill_source_crop` and
    /// `frame_source_crop` are genuinely INDEPENDENT — a caller may crop the
    /// frame image tighter than the liquid image (making the frame's own
    /// hole occupy more of the shared box) without that choice being forced
    /// onto the liquid's crop too, unlike the old single shared `source_crop`
    /// parameter this replaced (which could only scale both images by the
    /// exact same factor — see [`spawn_orb_bar`]'s own doc comment on why
    /// that made the frame/liquid RATIO untunable).
    #[test]
    fn orb_bar_fill_and_frame_source_crops_are_independent() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();
        let fill_crop = Rect::new(320.0, 0.0, 1088.0, 768.0);
        let frame_crop = Rect::new(352.0, 29.0, 1068.0, 745.0);

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                Some(fill_crop),
                Some(frame_crop),
                0.0,
                160.0,
                160.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let container_children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let clip_entity = container_children
            .iter()
            .copied()
            .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_some())
            .expect("orb bar has a clip-window child");
        let frame_entity = container_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_none())
            .expect("orb bar has a frame overlay child");
        let clip_children: Vec<Entity> = app
            .world()
            .get::<Children>(clip_entity)
            .unwrap()
            .iter()
            .collect();
        let fill_entity = clip_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_some())
            .expect("clip window has a fill-image grandchild");

        assert_eq!(
            app.world().get::<ImageNode>(fill_entity).unwrap().rect,
            Some(fill_crop)
        );
        assert_eq!(
            app.world().get::<ImageNode>(frame_entity).unwrap().rect,
            Some(frame_crop)
        );
    }

    /// `liquid_inset_px` shrinks the liquid image EQUALLY on all four sides,
    /// centring it inside the orb's full `width_px`×`height_px` box instead
    /// of spawning it flush with the box edges — the other half of the
    /// BL-82 EM-5.17 Phase 0 second follow-up fix (a tighter
    /// `frame_source_crop` makes the frame's hole bigger; this inset makes
    /// the liquid a hair smaller, so the liquid's edge sits fully inside
    /// the hole with no overlap even under sub-pixel rounding).
    #[test]
    fn orb_bar_liquid_inset_shrinks_and_centers_fill_image() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                None,
                None,
                8.0,
                160.0,
                160.0,
                BarValue::new(1.0, 1.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        let container_children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let clip_entity = container_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_some())
            .expect("orb bar has a clip-window child");
        let clip_children: Vec<Entity> = app
            .world()
            .get::<Children>(clip_entity)
            .unwrap()
            .iter()
            .collect();
        let fill_entity = clip_children
            .into_iter()
            .find(|&e| app.world().get::<HudOrbBarFill>(e).is_some())
            .expect("clip window has a fill-image grandchild");

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

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                Some(Handle::default()),
                None,
                None,
                0.0,
                160.0,
                160.0,
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
            .find(|&e| app.world().get::<HudOrbBarFillClip>(e).is_none())
            .expect("a non-clip-window (frame) child exists");
        assert_eq!(
            *app.world().get::<Pickable>(frame_entity).unwrap(),
            Pickable::IGNORE
        );
    }

    /// Omitting the frame overlay spawns only the fraction-reveal clip
    /// window — the frame stays a genuinely OPTIONAL parameter.
    #[test]
    fn orb_bar_without_frame_spawns_only_fill_child() {
        let mut app = new_orb_app();
        let theme = HudTheme::default();

        let container = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                Handle::default(),
                None,
                None,
                None,
                0.0,
                160.0,
                160.0,
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

    fn new_image_bar_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, update_horizontal_image_bars);
        app
    }

    /// `spawn_horizontal_image_bar` at half value gives the fill child a 50%
    /// WIDTH (left-anchored, same axis as [`spawn_bar`]'s flat-color fill,
    /// but image-backed) — the nameplate health/stagger-bar acceptance case
    /// (BL-82 EM-5.17 Phase 5).
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

        let fill_width = |app: &mut App, container: Entity| -> Val {
            let children: Vec<Entity> = app
                .world()
                .get::<Children>(container)
                .unwrap()
                .iter()
                .collect();
            let fill_entity = children
                .into_iter()
                .find(|&e| app.world().get::<HudImageBarFill>(e).is_some())
                .expect("image bar has a fill child");
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
        assert_eq!(children.len(), 2, "fill child + frame overlay child");
        let frame_entity = children
            .into_iter()
            .find(|&e| app.world().get::<HudImageBarFill>(e).is_none())
            .expect("a non-fill (frame) child exists");
        assert_eq!(
            *app.world().get::<Pickable>(frame_entity).unwrap(),
            Pickable::IGNORE
        );
    }

    /// Omitting the frame overlay spawns only the fill child.
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
        assert_eq!(children.len(), 1, "only the fill child, no frame overlay");
    }
}
