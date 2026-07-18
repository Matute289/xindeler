//! BL-82 EM-5.6 — the drag-drop item-slot primitive (spec §2 EM-5.1's own
//! "Deferred to the first screen that needs them" list named this: "the
//! drag-drop slot (needed by EM-5.3/5.6/5.7/5.15 — lands with whichever of
//! those is first)" — EM-5.6 is that screen).
//!
//! ## API shape (siblings build on this)
//! A slot is a plain `bevy_ui` node carrying [`HudSlot`] + [`SlotGroup`] +
//! [`SlotAddress`] (+ optionally [`SlotContents`] once it holds something).
//! [`SlotGroup`]/[`SlotAddress`] are OPAQUE `u32`/`u64` keys this primitive
//! never interprets — the caller (a bag screen, a trade offer grid, a future
//! hotbar/crafting screen) picks whatever encoding makes sense for its own
//! domain (e.g. EM-5.6 packs `InvSlotId`/`EquipSlot`/a trade-offer index into
//! [`SlotAddress`] via `SlotAddress::from_inv_slot_idx`/friends; a future
//! hotbar screen would use its own 0..10 slot index). Dragging one slot onto
//! another fires [`SlotDropped`] carrying BOTH ends' `(group, address)` pair
//! — the screen (never this primitive) decides what the move MEANS (an
//! inventory swap, an equip, adding to a trade offer, binding a hotbar
//! ability, …) and issues whatever client→server request follows.
//!
//! ## What this primitive owns
//! - [`slot_bundle`]: spawns a themed square slot (background, border,
//!   hover-highlight, an icon-text label placeholder — see EM-5.1's own note
//!   that the real `.vox`-icon-as-UI-icon path is deferred to whichever screen
//!   needs it first; this v1 uses a short text glyph + a quantity badge
//!   instead, the SAME "themed placeholder, reviewer-approved for v1" posture
//!   EM-5.2's buff-strip colour swatches established) + a
//!   [`crate::tooltip::Tooltip`] hook.
//! - [`SlotContents`]: what a slot currently displays; [`update_slot_visuals`]
//!   is the one system that turns a `Changed<SlotContents>` into the actual
//!   icon-text/quantity-badge/tooltip nodes.
//! - Drag/drop via `bevy_picking`'s stock `Pointer<DragStart>`/`Pointer<Drag>`/
//!   `Pointer<DragEnd>`/`Pointer<DragEnter>`/`Pointer<DragLeave>`/
//!   `Pointer<DragDrop>` events, observed GLOBALLY (`App::add_observer`, not
//!   per-entity `.observe()` — every slot in the app is drag-drop-capable the
//!   moment it carries [`HudSlot`], no per-spawn wiring needed): a slot dims
//!   while being dragged, a valid drop target highlights while
//!   hovered-with-a-drag, and dropping fires [`SlotDropped`].
//!
//! ## v1 simplification (documented, not silently skipped)
//! No floating "ghost" icon follows the cursor during a drag (Bevy 0.19 has
//! no first-party drag-ghost widget) — the dimmed source + highlighted
//! target already gives clear feedback for a grid of same-sized slots; a
//! cursor-following ghost sprite is a pure-polish follow-up once a screen
//! needs finer-grained visual feedback (e.g. dragging between distant
//! windows).

use bevy::{
    ecs::{
        bundle::Bundle,
        component::Component,
        entity::Entity,
        hierarchy::Children,
        message::{Message, MessageWriter},
        observer::On,
        query::{Changed, Has, With},
        system::{Commands, Query, Res},
    },
    picking::{
        events::{DragDrop, DragEnd, DragEnter, DragLeave, DragStart, Pointer},
        hover::Hovered,
    },
    prelude::{
        BackgroundColor, BorderColor, BorderRadius, Node, PositionType, Text, TextColor, TextFont,
        UiRect, Val, Visibility,
    },
    text::{FontSize, FontSource},
};

use crate::theme::{HudFonts, HudTheme};

/// Which drag-drop group a slot belongs to. Drops are reported regardless of
/// whether the two ends share a group — screens that need to REJECT
/// cross-group drops (e.g. "you can't drag a trade-offer slot into your
/// bag") check `from_group == to_group` (or whatever rule they need)
/// themselves when handling [`SlotDropped`]; this primitive stays opinion-free.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotGroup(pub u32);

/// The caller-defined address this slot represents — opaque to this
/// primitive. Two convenience constructors below cover EM-5.6's own two
/// domains (bag slots, equip slots); a screen with a different domain (a
/// future hotbar's 0..10 index, a trade-offer index) just picks its own
/// encoding.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotAddress(pub u64);

impl SlotAddress {
    /// Packs a `common::comp::inventory::slot::InvSlotId` (loadout_idx <<
    /// 16 | slot_idx, already exposed as `InvSlotId::idx() -> u32`) into a
    /// tagged address — high bit clear distinguishes it from
    /// [`Self::from_equip_slot_discriminant`]'s tagged range.
    #[must_use]
    pub fn from_inv_slot_idx(idx: u32) -> Self { Self(u64::from(idx)) }

    /// Packs an equip-slot's small integer discriminant (the caller resolves
    /// `EquipSlot` ↔ discriminant — this primitive doesn't know the
    /// `EquipSlot` enum) into a tagged address in the high half, so bag and
    /// equip addresses never collide.
    #[must_use]
    pub fn from_equip_slot_discriminant(discriminant: u32) -> Self {
        Self((1u64 << 32) | u64::from(discriminant))
    }

    /// The raw packed value, for a caller that needs to unpack it back.
    #[must_use]
    pub fn raw(self) -> u64 { self.0 }
}

/// What (if anything) a slot currently displays. The caller writes this
/// (typically driven by a `Net*` mirror, e.g. EM-5.6's `NetInventory`);
/// [`update_slot_visuals`] is the only system that reads it.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct SlotContents {
    /// Short placeholder glyph text shown in the slot (real `.vox`-icon
    /// rendering is a documented follow-up — see module doc comment).
    pub icon_text: String,
    /// Stack count badge; `None`/`Some(1)` both render as no badge (a
    /// singleton item doesn't need a "×1").
    pub quantity: Option<u32>,
    /// Hover tooltip text (already resolved by the caller — this primitive
    /// does no i18n/lookup itself, matching [`crate::tooltip::Tooltip`]'s own
    /// convention).
    pub tooltip: String,
}

/// Marks a spawned drag-drop slot root.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct HudSlot;

/// Marks a slot whose "resting" `BackgroundColor`/`BorderColor` is fully
/// transparent (`Color::NONE`) by DESIGN, not the flat `panel_bg`/
/// `panel_border` chrome [`slot_bundle`] gives every slot by default — a
/// caller (e.g. `xindeler-client`'s hotbar) that overrides those two render
/// components right after spawning to let its own overlay art (`skill_slot_
/// border.png`) be the slot's only frame ALSO inserts this marker, so
/// [`on_drag_end`]/[`on_drag_leave`]/[`on_drag_drop`] restore the drag/hover
/// cues back to `Color::NONE` instead of the theme's opaque panel colours —
/// without this marker those observers (registered GLOBALLY against every
/// `HudSlot`, see [`install_observers`]) would silently reintroduce the flat
/// chrome the FIRST time a chromeless slot participates in a drag (as either
/// end), recreating the exact "doubled rectangle" visual bug the override
/// was meant to fix, just gated behind a user interaction instead of always
/// on. [`on_drag_start`] additionally skips its dim-while-dragging cue for a
/// chromeless slot (no flat fill to fade to 40% alpha; dimming the overlay
/// art itself is a real asset swap this primitive doesn't own — deferred,
/// same "documented v1 simplification" posture as the module doc comment's
/// no-ghost-sprite note). [`on_drag_enter`]'s accent-coloured hover-target
/// ring is left unchanged for chromeless slots too — it draws a real, thin
/// border in the layout's already-reserved 2px border box regardless of the
/// resting `BorderColor`, so it reads as a legitimate transient highlight
/// (not a second background rectangle) whether or not the slot is
/// chromeless.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ChromelessSlot;

/// Marks a slot's icon-text child (the node [`update_slot_visuals`] retexts).
/// `pub(crate)` (not private): `update_slot_visuals` is itself `pub(crate)`
/// (called from `crate::XindelerUiPlugin` in `lib.rs`), and a query type
/// parameter naming this marker makes it part of that function's effective
/// signature — Rust's privacy check requires the marker be at least as
/// visible as the function that names it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct SlotIconText;
/// Marks a slot's quantity-badge child. `pub(crate)` — see [`SlotIconText`]'s
/// doc comment for why.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct SlotQuantityBadge;

/// Fired when a drag ending over a slot completes — see the module doc
/// comment for the full contract. `from`/`to` are `(group, address)` pairs;
/// this primitive never inspects the semantic meaning of either.
#[derive(Message, Clone, Copy, Debug)]
pub struct SlotDropped {
    pub from_group: SlotGroup,
    pub from_address: SlotAddress,
    pub to_group: SlotGroup,
    pub to_address: SlotAddress,
}

/// The width (px) of a [`slot_bundle`] slot's flat chrome border, factored out
/// as a named constant so a caller that draws its own overlay art edge-to-edge
/// (BL-82 hotbar: `hotbar.rs`'s `SkillSlotBackground` fill) can inset by
/// exactly this to cover the slot's FULL border box, not just the padding box
/// the border leaves inside it — keeping the two in lockstep instead of a
/// second hardcoded `2.0` silently drifting from this one.
pub const SLOT_BORDER_PX: f32 = 2.0;

/// Spawns a themed, empty, drag-drop-capable slot (square, `size_px` on a
/// side) at the given `(group, address)`, carrying a DEFAULT (empty)
/// [`SlotContents`] from the start — so a caller's own `Query<&mut
/// SlotContents>` always matches every spawned slot immediately (no
/// "insert it yourself or nothing renders" trap), and
/// [`update_slot_visuals`]'s `Changed<SlotContents>` gate fires correctly the
/// first time a caller updates it. The caller updates [`SlotContents`] on
/// the returned entity (via `Commands`/a `Query<&mut SlotContents>`)
/// whenever the underlying data changes; never spawn the icon/quantity/
/// tooltip children directly — [`update_slot_visuals`] owns them.
#[must_use]
pub fn slot_bundle(
    theme: &HudTheme,
    group: SlotGroup,
    address: SlotAddress,
    size_px: f32,
) -> impl Bundle {
    (
        HudSlot,
        group,
        address,
        SlotContents::default(),
        Hovered(false),
        bevy::picking::Pickable::default(),
        Node {
            width: Val::Px(size_px),
            height: Val::Px(size_px),
            border: UiRect::all(Val::Px(SLOT_BORDER_PX)),
            border_radius: BorderRadius::all(Val::Px(theme.radius.sm)),
            // BL-82 EM-5.17/5.18 legacy-inventory rebuild (STEP 4, "the icon
            // never renders" bug): centers both the icon-text glyph and the
            // quantity badge's flow position within the slot — the badge
            // itself is absolutely positioned (unaffected by this), but the
            // icon-text child (see `update_slot_visuals`) has no positioning
            // of its own, so without this it fell back to flex's default
            // top-left alignment and rendered nearly invisible in a corner.
            justify_content: bevy::ui::JustifyContent::Center,
            align_items: bevy::ui::AlignItems::Center,
            ..Default::default()
        },
        BackgroundColor(theme.palette.panel_bg),
        BorderColor::all(theme.palette.panel_border),
    )
}

/// BL-82 EM-5.17 T57.8 — a rarity-tiered variant of [`slot_bundle`]: the SAME
/// slot (identical group/address/contents/drag-drop wiring), with the flat
/// [`BackgroundColor`] fill overlaid by a rarity-texture [`ImageNode`] (e.g.
/// `slot_bg_common.png`.. `slot_bg_mythic.png` via [`crate::images::
/// HudImages`], keyed off Xindeler's existing item-quality tiers — a future
/// phase's own mapping, not this primitive's concern). A wrapper around
/// [`slot_bundle`] rather than an extra parameter on it, per this phase's own
/// "prefer whichever additive shape needs zero existing-callsite changes"
/// brief — every current `slot_bundle(..)` call site (bag/trade/hotbar
/// screens) keeps compiling completely unchanged; a screen that wants a
/// rarity background switches to this function instead, one call site at a
/// time, whenever it's ready.
#[must_use]
pub fn slot_bundle_with_rarity(
    theme: &HudTheme,
    group: SlotGroup,
    address: SlotAddress,
    size_px: f32,
    rarity_background: bevy::asset::Handle<bevy::image::Image>,
) -> impl Bundle {
    (
        slot_bundle(theme, group, address, size_px),
        bevy::ui::widget::ImageNode::new(rarity_background),
    )
}

/// Reconciles every [`HudSlot`]'s icon-text/quantity-badge children against
/// its current [`SlotContents`] — `Changed<SlotContents>`-gated, and further
/// gated on the CONTENT actually needing a (re)spawn (children are reused,
/// not despawned/respawned every change, unlike [`crate::notification`]'s
/// deliberately-simple rebuild-every-time strategy — a bag can have dozens
/// of slots changing per network tick, so this one is worth the extra care).
pub(crate) fn update_slot_visuals(
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    mut commands: Commands,
    slots: Query<(Entity, &SlotContents, Option<&Children>), Changed<SlotContents>>,
    mut icon_texts: Query<
        &mut Text,
        (
            With<SlotIconText>,
            bevy::ecs::query::Without<SlotQuantityBadge>,
        ),
    >,
    mut quantity_badges: Query<
        (&mut Text, &mut Visibility),
        (
            With<SlotQuantityBadge>,
            bevy::ecs::query::Without<SlotIconText>,
        ),
    >,
) {
    for (slot_entity, contents, children) in &slots {
        let existing_icon = children.and_then(|kids| {
            kids.iter()
                .find(|&&child| icon_texts.get(child).is_ok())
                .copied()
        });
        let existing_badge = children.and_then(|kids| {
            kids.iter()
                .find(|&&child| quantity_badges.get(child).is_ok())
                .copied()
        });

        if let Some(icon_entity) = existing_icon {
            if let Ok(mut text) = icon_texts.get_mut(icon_entity) {
                text.0.clone_from(&contents.icon_text);
            }
        } else {
            commands.entity(slot_entity).with_children(|parent| {
                parent.spawn((
                    SlotIconText,
                    Text(contents.icon_text.clone()),
                    TextFont {
                        font: FontSource::Handle(fonts.body.clone()),
                        font_size: FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                    // BL-82 EM-5.17/5.18 legacy-inventory rebuild (STEP 4,
                    // "the icon glyph never renders" bug): a PLAIN, auto-sized
                    // flex child — the slot's own `Node` (see `slot_bundle`)
                    // carries `justify_content: Center`/`align_items: Center`,
                    // which centers this child over the rarity-background
                    // `ImageNode`. It deliberately does NOT get an
                    // absolute-fill `Node` of its own: `justify_content`/
                    // `align_items` on a leaf `Text` node are no-ops (glyph
                    // placement inside a text box is governed by `TextLayout`,
                    // not flex align), so stretching the text node to fill the
                    // slot would just move the glyph back to the box's
                    // top-left — the exact bug this fixes (matches
                    // `crate::button`'s label-centering idiom). The quantity
                    // badge (spawned below) is the ONLY absolutely-positioned
                    // child, so it stays out of flow in its own corner and
                    // never fights this centered glyph.
                ));
            });
        }

        let badge_text = match contents.quantity {
            Some(qty) if qty > 1 => format!("×{qty}"),
            _ => String::new(),
        };
        let badge_visible = !badge_text.is_empty();
        if let Some(badge_entity) = existing_badge {
            if let Ok((mut text, mut visibility)) = quantity_badges.get_mut(badge_entity) {
                text.0.clone_from(&badge_text);
                *visibility = if badge_visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
            }
        } else {
            commands.entity(slot_entity).with_children(|parent| {
                parent.spawn((
                    SlotQuantityBadge,
                    Text(badge_text),
                    TextFont {
                        font: FontSource::Handle(fonts.body.clone()),
                        font_size: FontSize::Px(11.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text_muted),
                    Node {
                        position_type: PositionType::Absolute,
                        bottom: Val::Px(1.0),
                        right: Val::Px(2.0),
                        ..Default::default()
                    },
                    if badge_visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                ));
            });
        }

        commands
            .entity(slot_entity)
            .insert(crate::tooltip::Tooltip {
                text: contents.tooltip.clone(),
            });
    }
}

/// Dims a slot while it's being dragged (the "this is the thing you're
/// moving" cue) — restored by [`on_drag_end`]. A [`ChromelessSlot`] has no
/// flat fill to dim (see that marker's doc comment) — its background stays
/// untouched during the drag rather than manufacturing a fill that isn't
/// otherwise part of its resting state.
fn on_drag_start(
    trigger: On<Pointer<DragStart>>,
    mut backgrounds: Query<(&mut BackgroundColor, Has<ChromelessSlot>), With<HudSlot>>,
    theme: Res<HudTheme>,
) {
    if let Ok((mut bg, chromeless)) = backgrounds.get_mut(trigger.entity) {
        if chromeless {
            return;
        }
        let mut faded = theme.palette.panel_bg.to_srgba();
        faded.alpha *= 0.4;
        bg.0 = bevy::color::Color::Srgba(faded);
    }
}

/// Restores a slot's normal background once the drag ends (whether or not it
/// landed on a valid target — [`on_drag_drop`] handles the actual move). A
/// [`ChromelessSlot`]'s "normal" background is `Color::NONE`, not the theme's
/// opaque `panel_bg` — see that marker's doc comment for why restoring the
/// wrong one here would silently reintroduce the doubled-rectangle bug.
fn on_drag_end(
    trigger: On<Pointer<DragEnd>>,
    mut backgrounds: Query<(&mut BackgroundColor, Has<ChromelessSlot>), With<HudSlot>>,
    theme: Res<HudTheme>,
) {
    if let Ok((mut bg, chromeless)) = backgrounds.get_mut(trigger.entity) {
        bg.0 = if chromeless {
            bevy::color::Color::NONE
        } else {
            theme.palette.panel_bg
        };
    }
}

/// Highlights a slot while a drag is hovering over it (a valid drop target
/// cue).
fn on_drag_enter(
    trigger: On<Pointer<DragEnter>>,
    mut borders: Query<&mut BorderColor, With<HudSlot>>,
    theme: Res<HudTheme>,
) {
    if let Ok(mut border) = borders.get_mut(trigger.entity) {
        *border = BorderColor::all(theme.palette.accent);
    }
}

/// Restores a slot's normal border once a drag leaves it without dropping. A
/// [`ChromelessSlot`]'s "normal" border is `Color::NONE`, not the theme's
/// opaque `panel_border` — see that marker's doc comment for why restoring
/// the wrong one here would silently reintroduce the doubled-rectangle bug.
fn on_drag_leave(
    trigger: On<Pointer<DragLeave>>,
    mut borders: Query<(&mut BorderColor, Has<ChromelessSlot>), With<HudSlot>>,
    theme: Res<HudTheme>,
) {
    if let Ok((mut border, chromeless)) = borders.get_mut(trigger.entity) {
        *border = BorderColor::all(if chromeless {
            bevy::color::Color::NONE
        } else {
            theme.palette.panel_border
        });
    }
}

/// The pure "did a real drop between two slots happen, and what does it
/// mean" logic, split out from [`on_drag_drop`] so it's unit-testable
/// without constructing a real `bevy_picking` event (the observer plumbing
/// itself is exercised structurally by [`install_observers`] being callable
/// at all + registered against the real `Pointer<DragDrop>` type; the
/// SEMANTIC contract — "both ends must be real slots, else ignore" — is what
/// this function isolates for a direct test).
fn resolve_drop(
    to: Option<(&SlotGroup, &SlotAddress)>,
    from: Option<(&SlotGroup, &SlotAddress)>,
) -> Option<SlotDropped> {
    let (to_group, to_address) = to?;
    let (from_group, from_address) = from?;
    Some(SlotDropped {
        from_group: *from_group,
        from_address: *from_address,
        to_group: *to_group,
        to_address: *to_address,
    })
}

/// The one place a completed drag turns into [`SlotDropped`]: reads BOTH
/// ends' `(SlotGroup, SlotAddress)` (via [`resolve_drop`]) and writes the
/// message — see the module doc comment for the full contract. A drop onto
/// (or from) an entity missing either component (not a real slot) is
/// silently ignored.
fn on_drag_drop(
    trigger: On<Pointer<DragDrop>>,
    slots: Query<(&SlotGroup, &SlotAddress), With<HudSlot>>,
    mut borders: Query<(&mut BorderColor, Has<ChromelessSlot>), With<HudSlot>>,
    theme: Res<HudTheme>,
    mut writer: MessageWriter<SlotDropped>,
) {
    let to_entity = trigger.entity;
    let dropped_entity = trigger.event.dropped;

    // Same `ChromelessSlot` resting-border rule as `on_drag_leave` — see
    // that marker's doc comment.
    if let Ok((mut border, chromeless)) = borders.get_mut(to_entity) {
        *border = BorderColor::all(if chromeless {
            bevy::color::Color::NONE
        } else {
            theme.palette.panel_border
        });
    }

    if let Some(dropped) = resolve_drop(slots.get(to_entity).ok(), slots.get(dropped_entity).ok()) {
        writer.write(dropped);
    }
}

/// Registers the slot's global drag/drop observers + [`update_slot_visuals`].
/// Added by [`crate::XindelerUiPlugin`].
pub(crate) fn install_observers(app: &mut bevy::app::App) {
    app.add_message::<SlotDropped>();
    app.add_observer(on_drag_start);
    app.add_observer(on_drag_end);
    app.add_observer(on_drag_enter);
    app.add_observer(on_drag_leave);
    app.add_observer(on_drag_drop);
}

#[cfg(test)]
mod tests {
    use bevy::{app::App, prelude::*};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app
    }

    /// `slot_bundle` spawns a real `HudSlot` carrying the given group/address
    /// and the theme's background — the T56.19 "widget primitive exists"
    /// acceptance bar.
    #[test]
    fn slot_bundle_spawns_with_group_and_address() {
        let mut app = new_app();
        let theme = HudTheme::default();
        let entity = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(1), SlotAddress(42), 48.0))
            .id();

        assert!(app.world().get::<HudSlot>(entity).is_some());
        assert_eq!(*app.world().get::<SlotGroup>(entity).unwrap(), SlotGroup(1));
        assert_eq!(
            *app.world().get::<SlotAddress>(entity).unwrap(),
            SlotAddress(42)
        );
    }

    /// `slot_bundle_with_rarity` spawns a real `HudSlot` (same as plain
    /// `slot_bundle`) that ALSO carries an `ImageNode` for the given rarity
    /// texture — the T57.8 additive acceptance bar for the rarity-background
    /// wrapper.
    #[test]
    fn slot_bundle_with_rarity_carries_image_node() {
        let mut app = new_app();
        let theme = HudTheme::default();
        let rarity: Handle<Image> = Handle::default();
        let entity = app
            .world_mut()
            .spawn(slot_bundle_with_rarity(
                &theme,
                SlotGroup(1),
                SlotAddress(42),
                48.0,
                rarity.clone(),
            ))
            .id();

        assert!(app.world().get::<HudSlot>(entity).is_some());
        let image_node = app
            .world()
            .get::<bevy::ui::widget::ImageNode>(entity)
            .expect("slot carries an ImageNode");
        assert_eq!(image_node.image, rarity);
    }

    /// `SlotAddress`'s two packing helpers never collide (bag vs. equip
    /// addressing stay in disjoint ranges) — the invariant `xindeler-client`'s
    /// screens rely on to tell "was this an equip slot or a bag slot" apart
    /// purely from the raw value if ever needed.
    #[test]
    fn inv_slot_and_equip_slot_addresses_never_collide() {
        for idx in 0..64u32 {
            assert_ne!(
                SlotAddress::from_inv_slot_idx(idx).raw(),
                SlotAddress::from_equip_slot_discriminant(idx).raw()
            );
        }
    }

    /// Setting a slot's [`SlotContents`] spawns real icon-text/quantity-badge
    /// children reflecting it; a LATER content change updates the SAME
    /// children in place (not a fresh despawn/respawn pair) — the T56.19
    /// acceptance bar for the reconciliation half of this primitive.
    #[test]
    fn slot_contents_drive_icon_and_quantity_children() {
        let mut app = new_app();
        app.add_systems(Update, update_slot_visuals);
        let theme = HudTheme::default();

        // `slot_bundle` already carries a DEFAULT `SlotContents` — set the
        // real one via a follow-up `insert` (the production usage pattern),
        // not by double-including the component in one spawn tuple.
        let slot = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(1), 48.0))
            .id();
        app.world_mut().entity_mut(slot).insert(SlotContents {
            icon_text: "Pot".to_owned(),
            quantity: Some(5),
            tooltip: "Minor Potion".to_owned(),
        });
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(slot)
            .expect("icon/badge children spawned")
            .iter()
            .collect();
        let icon = children
            .iter()
            .copied()
            .find(|&e| app.world().get::<SlotIconText>(e).is_some())
            .expect("icon child exists");
        let badge = children
            .iter()
            .copied()
            .find(|&e| app.world().get::<SlotQuantityBadge>(e).is_some())
            .expect("badge child exists");
        assert_eq!(app.world().get::<Text>(icon).unwrap().0, "Pot");
        assert_eq!(app.world().get::<Text>(badge).unwrap().0, "×5");

        // Change the contents; the SAME children update, no new ones spawn.
        app.world_mut().entity_mut(slot).insert(SlotContents {
            icon_text: "Ax".to_owned(),
            quantity: None,
            tooltip: "Axe".to_owned(),
        });
        app.update();

        let children_after: Vec<Entity> =
            app.world().get::<Children>(slot).unwrap().iter().collect();
        assert_eq!(
            children_after.len(),
            children.len(),
            "content changes must reuse existing children, not grow the child list"
        );
        assert_eq!(app.world().get::<Text>(icon).unwrap().0, "Ax");
        assert_eq!(app.world().get::<Text>(badge).unwrap().0, "");
    }

    /// [`resolve_drop`] (the semantic core [`on_drag_drop`] delegates to)
    /// builds a [`SlotDropped`] from two real slots' group/address — the
    /// T56.19 end-to-end acceptance bar for "what a completed drag/drop
    /// MEANS", independent of `bevy_picking`'s own event-dispatch plumbing
    /// (which is registered, not reimplemented, by [`install_observers`]).
    #[test]
    fn resolve_drop_builds_slot_dropped_from_both_ends() {
        let from = (SlotGroup(0), SlotAddress(1));
        let to = (SlotGroup(0), SlotAddress(2));
        let dropped = resolve_drop(Some((&to.0, &to.1)), Some((&from.0, &from.1)))
            .expect("both ends are real slots");
        assert_eq!(dropped.from_address, SlotAddress(1));
        assert_eq!(dropped.to_address, SlotAddress(2));
    }

    /// A drop where either end isn't a real slot (missing group/address —
    /// e.g. dropped outside any slot) is silently ignored, not a panic.
    #[test]
    fn resolve_drop_ignores_a_non_slot_end() {
        let to = (SlotGroup(0), SlotAddress(2));
        assert!(resolve_drop(Some((&to.0, &to.1)), None).is_none());
        assert!(resolve_drop(None, Some((&to.0, &to.1))).is_none());
    }

    /// [`install_observers`] registers the drag/drop observer set + the
    /// [`SlotDropped`] message type without panicking — the structural half
    /// of the acceptance bar (the real `bevy_picking` dispatch machinery
    /// that would actually fire these observers is `bevy_picking`'s own
    /// tested responsibility, not reimplemented here).
    #[test]
    fn install_observers_registers_without_panicking() {
        let mut app = new_app();
        app.add_plugins(bevy::picking::PickingPlugin);
        install_observers(&mut app);
        app.update();
    }

    /// A synthetic pointer location shared by every `fire_drag_*` helper
    /// below — mirrors `xindeler-client`'s own `inventory_ui.rs::
    /// fire_pointer_click` precedent (`NormalizedRenderTarget::None` needs no
    /// real window/camera, so this is a headless, synthetic event, not a
    /// real `bevy_picking` backend dispatch).
    fn synthetic_location() -> bevy::picking::pointer::Location {
        bevy::picking::pointer::Location {
            target: bevy::camera::NormalizedRenderTarget::None {
                width: 0,
                height: 0,
            },
            position: bevy::math::Vec2::ZERO,
        }
    }

    /// Fires a synthetic [`Pointer<DragEnd>`] at `entity`.
    fn fire_drag_end(world: &mut bevy::ecs::world::World, entity: Entity) {
        use bevy::picking::pointer::PointerId;

        world.trigger(Pointer::new_without_propagate(
            PointerId::Mouse,
            synthetic_location(),
            DragEnd {
                button: bevy::picking::pointer::PointerButton::Primary,
                distance: bevy::math::Vec2::ZERO,
            },
            entity,
        ));
    }

    /// Fires a synthetic [`Pointer<DragLeave>`] at `entity`, reporting
    /// `dragged` as the entity that was being dragged when the pointer left.
    fn fire_drag_leave(world: &mut bevy::ecs::world::World, entity: Entity, dragged: Entity) {
        use bevy::picking::{backend::HitData, pointer::PointerId};

        world.trigger(Pointer::new_without_propagate(
            PointerId::Mouse,
            synthetic_location(),
            DragLeave {
                button: bevy::picking::pointer::PointerButton::Primary,
                dragged,
                hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            },
            entity,
        ));
    }

    /// Fires a synthetic [`Pointer<DragDrop>`] at `entity`, reporting
    /// `dropped` as the entity dropped onto it.
    fn fire_drag_drop(world: &mut bevy::ecs::world::World, entity: Entity, dropped: Entity) {
        use bevy::picking::{backend::HitData, pointer::PointerId};

        world.trigger(Pointer::new_without_propagate(
            PointerId::Mouse,
            synthetic_location(),
            DragDrop {
                button: bevy::picking::pointer::PointerButton::Primary,
                dropped,
                hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            },
            entity,
        ));
    }

    /// Regression test for the drag-restore half of the "doubled rectangle"
    /// bug (see [`ChromelessSlot`]'s own doc comment): a [`ChromelessSlot`]'s
    /// `BackgroundColor` must come back `Color::NONE` — NOT the theme's
    /// opaque `panel_bg` — once [`on_drag_end`] fires, while a plain slot
    /// (no marker) keeps restoring the theme colour exactly as before.
    #[test]
    fn on_drag_end_restores_chromeless_slots_to_transparent() {
        use bevy::color::Alpha;

        let mut app = new_app();
        app.add_plugins(bevy::picking::PickingPlugin);
        install_observers(&mut app);
        let theme = HudTheme::default();

        let chromeless = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(0), 48.0))
            .insert((BackgroundColor(bevy::color::Color::NONE), ChromelessSlot))
            .id();
        let plain = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(1), 48.0))
            .id();

        fire_drag_end(app.world_mut(), chromeless);
        fire_drag_end(app.world_mut(), plain);
        app.update();

        let world = app.world();
        assert!(
            world
                .get::<BackgroundColor>(chromeless)
                .unwrap()
                .0
                .is_fully_transparent(),
            "on_drag_end must restore a ChromelessSlot's background to Color::NONE, not the \
             opaque theme panel colour — else the FIRST drag on a hotbar slot reintroduces the \
             doubled-rectangle bug"
        );
        assert_eq!(
            world.get::<BackgroundColor>(plain).unwrap().0,
            theme.palette.panel_bg,
            "a plain (non-chromeless) slot's on_drag_end behaviour must stay unchanged"
        );
    }

    /// Same contract as
    /// [`on_drag_end_restores_chromeless_slots_to_transparent`]
    /// for [`on_drag_leave`]'s border restore.
    #[test]
    fn on_drag_leave_restores_chromeless_slots_to_transparent() {
        use bevy::color::Alpha;

        let mut app = new_app();
        app.add_plugins(bevy::picking::PickingPlugin);
        install_observers(&mut app);
        let theme = HudTheme::default();

        let chromeless = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(0), 48.0))
            .insert((BorderColor::all(bevy::color::Color::NONE), ChromelessSlot))
            .id();
        let plain = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(1), 48.0))
            .id();
        let dragged = app.world_mut().spawn_empty().id();

        fire_drag_leave(app.world_mut(), chromeless, dragged);
        fire_drag_leave(app.world_mut(), plain, dragged);
        app.update();

        let world = app.world();
        assert!(
            world
                .get::<BorderColor>(chromeless)
                .unwrap()
                .top
                .is_fully_transparent(),
            "on_drag_leave must restore a ChromelessSlot's border to Color::NONE, not the opaque \
             theme panel colour"
        );
        assert_eq!(
            world.get::<BorderColor>(plain).unwrap().top,
            theme.palette.panel_border,
            "a plain (non-chromeless) slot's on_drag_leave behaviour must stay unchanged"
        );
    }

    /// Same contract as
    /// [`on_drag_end_restores_chromeless_slots_to_transparent`]
    /// for [`on_drag_drop`]'s border restore.
    #[test]
    fn on_drag_drop_restores_chromeless_slots_to_transparent() {
        use bevy::color::Alpha;

        let mut app = new_app();
        app.add_plugins(bevy::picking::PickingPlugin);
        install_observers(&mut app);
        let theme = HudTheme::default();

        let chromeless = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(0), 48.0))
            .insert((BorderColor::all(bevy::color::Color::NONE), ChromelessSlot))
            .id();
        let dropped = app
            .world_mut()
            .spawn(slot_bundle(&theme, SlotGroup(0), SlotAddress(1), 48.0))
            .id();

        fire_drag_drop(app.world_mut(), chromeless, dropped);
        app.update();

        let world = app.world();
        assert!(
            world
                .get::<BorderColor>(chromeless)
                .unwrap()
                .top
                .is_fully_transparent(),
            "on_drag_drop must restore a ChromelessSlot's border to Color::NONE, not the opaque \
             theme panel colour"
        );
    }
}
