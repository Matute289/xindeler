//! BL-82 EM-5.6 — the inventory/bag + paper-doll screen (spec §2/§6, tasks
//! T56.19/T56.20).
//!
//! Reads the local player's real mirrored [`NetInventory`] (EM-5.6's
//! `xindeler-sim-bridge::inventory::mirror_inventory_state`) and renders it
//! on the [`xindeler_ui::slot`] drag-drop primitive: a paper-doll column
//! (every [`xindeler_protocol::inventory::ALL_EQUIP_SLOTS`] slot, occupied or
//! not) + a bag grid (every physical slot up to [`NetInventory::capacity`]).
//! Dragging a slot onto another (via [`xindeler_ui::slot::SlotDropped`])
//! sends a real [`InventoryActionRequest`] (`InventoryManip::Swap`) — the
//! SAME wire message `xindeler-sim-bridge::inventory::
//! apply_inventory_action_requests` re-emits through the sim's public event
//! bus (see that module's doc comment).
//!
//! Toggled by the `I` key (writes `HudAction::ToggleWindow(HudWindow::
//! Inventory)`, the SAME generic HUD→action flow EM-5.2's respawn button
//! uses) — `HudWindow::Inventory` already existed in EM-5.1's state machine
//! (unused until now), so this screen needs no `xindeler-ui` state-machine
//! changes.
//!
//! ## Loot-pickup feed (T56.20)
//! Derived from a REAL diff of `Changed<NetInventory>` against the
//! previously-seen slot contents (no separate pickup EVENT mirror exists yet
//! — `common::Outcome` still doesn't reach the client at all, the same gap
//! EM-5.2's floating-combat-text note already flagged) — a slot whose amount
//! increases, or a newly-occupied slot, pushes a real
//! `xindeler_ui::notification::NotificationQueue` entry ("+N Item"). This is
//! a genuine, functioning v1 (driven by real mirrored data, not mocked), not
//! the more precise future implementation a dedicated pickup event would
//! give (which would also distinguish "picked up" from "received in trade"
//! from "crafted" — out of scope here).
//!
//! ## Known gap (documented, not silently skipped)
//! World **overitem** pick-up prompts (a floating "[E] Pick up" over a
//! dropped item prop in the world) need a mirror of world item-drop entities
//! that does not exist yet (only creatures/players are mirrored today,
//! `NetBody`) — a real follow-up, not attempted in this task.

use bevy::{ecs::schedule::common_conditions::not, prelude::*};
use common::comp::inventory::{
    item::Quality,
    slot::{ArmorSlot, EquipSlot, InvSlotId, Slot},
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    InventoryActionRequest, NetInventory, NetItemStack, NetLocalPlayer, inventory::ALL_EQUIP_SLOTS,
};
use xindeler_ui::{
    hud_state::{HudAction, HudState, HudWindow},
    images::{HudImageKey, HudImages},
    panel::panel_bundle,
    slot::{HudSlot, SlotAddress, SlotContents, SlotDropped, SlotGroup, slot_bundle},
    theme::HudTheme,
    tooltip::TooltipBackground,
};

use crate::chat::text_input_focused;

/// The two [`SlotGroup`]s this screen's slots live in — bag slots can be
/// dragged onto equip slots (to equip) and vice versa (to unequip), so both
/// groups are handled uniformly by [`handle_slot_drops`] rather than
/// rejecting cross-group drops (unlike a hypothetical hotbar/hotbar-only
/// screen, which WOULD reject cross-group).
/// `pub(crate)`: `crate::trade_ui` reuses this exact group to recognize a
/// drag between a bag slot and a trade-offer slot (dragging a bag item INTO
/// an offer, or an offer item back OUT to the bag).
pub(crate) const BAG_GROUP: SlotGroup = SlotGroup(1);
const EQUIP_GROUP: SlotGroup = SlotGroup(2);

/// BL-82 EM-5.17 T57.14 — the confirmed 18-of-22-slot Equipment panel layout
/// (spec §3.7, Matías's direct confirmation): two full weapon SETS
/// (Mainhand+Offhand each) flank a center armor column; the 4 `Bag1`-`Bag4`
/// slots are EXCLUDED (they belong on the Items/Inventory tab, not
/// Equipment — this screen's bag GRID already covers them via physical
/// `InvSlotId` addressing, unrelated to these loadout-provided `EquipSlot`
/// bag slots). All three constants index into [`ALL_EQUIP_SLOTS`].
///
/// Which physical side shows Active vs. Inactive doesn't matter per spec
/// ("doesn't matter visually") — Active-on-the-left is an arbitrary, stable
/// convention.
const LEFT_WEAPON_SET_INDICES: [usize; 2] = [16, 17]; // ActiveMainhand, ActiveOffhand
/// The right-flanking weapon set — see [`LEFT_WEAPON_SET_INDICES`].
const RIGHT_WEAPON_SET_INDICES: [usize; 2] = [18, 19]; // InactiveMainhand, InactiveOffhand
/// The center armor column, top-to-bottom render order. Per spec §3.7:
/// Head/Neck/Shoulders/Chest, then Legs (inserted between Chest and Feet:
/// "Chest → Legs → Feet"), then Lantern+Glider immediately below Feet, then
/// the rest of the Notion doc's original-10 slots (Hands/Ring1/Ring2/Back),
/// then Tabard (placed adjacent to Back — "near Chest/Back" per spec; the
/// EXACT spot is flagged non-blocking, worth a later confirm from Matías —
/// see the Phase 7 report), then Belt.
const CENTER_COLUMN_INDICES: [usize; 14] = [
    0, 1, 2, 3, // Head, Neck, Shoulders, Chest
    9, 10, // Legs, Feet ("Chest -> Legs -> Feet")
    20, 21, // Lantern, Glider ("below Feet")
    4, 5, 6, 7,  // Hands, Ring1, Ring2, Back
    11, // Tabard — placed next to Back; flagged, non-blocking (see module doc comment)
    8,  // Belt
];

/// Marks the whole inventory window root (toggled by [`HudState`]).
#[derive(Component)]
struct InventoryWindowRoot;
/// Marks the bag grid container (children are the bag [`xindeler_ui::slot`]
/// entities, spawned once real capacity is known).
#[derive(Component)]
struct BagGridRoot;
/// Marks the paper-doll ROW container (the 3 flanking/center columns below
/// are its children) — spawned at `Startup` since the slot COUNT (18) is
/// fixed/known ahead of time, unlike the bag.
#[derive(Component)]
struct PaperdollRoot;
/// The left flanking weapon-set column (BL-82 EM-5.17 T57.14) — see
/// [`LEFT_WEAPON_SET_INDICES`].
#[derive(Component)]
struct LeftWeaponColumnRoot;
/// The center armor column — see [`CENTER_COLUMN_INDICES`].
#[derive(Component)]
struct CenterEquipColumnRoot;
/// The right flanking weapon-set column — see [`RIGHT_WEAPON_SET_INDICES`].
#[derive(Component)]
struct RightWeaponColumnRoot;

/// Latches "have we spawned the `capacity`-sized bag grid yet" — the bag
/// grid can't be spawned until the FIRST real `NetInventory` arrives (its
/// capacity isn't known before then); a paper-doll's slot count is fixed
/// (22, `ALL_EQUIP_SLOTS::len()`), so it spawns unconditionally at `Startup`.
#[derive(Resource, Default)]
struct BagGridSpawned(bool);

/// Last-seen bag contents (by [`InvSlotId`]), for the loot-feed diff — see
/// module doc comment. `baseline_established` is tracked SEPARATELY from
/// `slots` being non-empty — a player who starts (or currently sits) with a
/// completely empty bag must still stop suppressing "first observation"
/// after that first tick, not forever (an empty `slots` map is otherwise
/// indistinguishable from "never observed yet").
#[derive(Resource, Default)]
struct LastSeenBag {
    slots: std::collections::HashMap<InvSlotId, u32>,
    baseline_established: bool,
}

pub struct InventoryUiPlugin;

impl Plugin for InventoryUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BagGridSpawned>()
            .init_resource::<LastSeenBag>()
            .add_systems(
                Startup,
                (
                    spawn_inventory_window.after(xindeler_ui::theme::init_theme),
                    force_open_inventory_for_smoke_capture,
                ),
            )
            .add_systems(
                Update,
                (
                    // Reads `ActionState` — must run after the frame's real
                    // input resolution (BL-82 EM-5.17 Phase 0, same fix as
                    // `diary::toggle_diary_window`/`controls_screen::
                    // toggle_controls_screen`). Also gated on
                    // `!text_input_focused` so typing "i" in the chat box
                    // doesn't ALSO open the Inventory.
                    toggle_inventory_window
                        .after(xindeler_input::InputResolveSet)
                        .run_if(not(text_input_focused)),
                    sync_inventory_window_visibility,
                    spawn_bag_grid_once_capacity_known,
                    sync_slot_contents.after(spawn_bag_grid_once_capacity_known),
                    // BL-82 EM-5.17 T57.15 — must run after slots exist so a
                    // fresh spawn's initial 2H state applies the same frame
                    // it can (a later `NetInventory` change self-corrects it
                    // either way; see this system's own doc comment).
                    sync_two_handed_offhand_disable.after(spawn_bag_grid_once_capacity_known),
                    handle_slot_drops,
                    push_loot_pickup_notifications,
                ),
            );
    }
}

/// Spawns the (initially hidden) inventory window: a full-screen dim
/// backdrop containing a themed panel with a paper-doll column (all 22
/// equip slots — fixed size, spawned now) and an EMPTY bag-grid container
/// ([`spawn_bag_grid_once_capacity_known`] fills it in once the real
/// capacity is known).
fn spawn_inventory_window(mut commands: Commands, theme: Res<HudTheme>) {
    commands
        .spawn((
            InventoryWindowRoot,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
        ))
        .with_children(|backdrop| {
            // `panel_bundle` already carries a real `Node` (padding/border/
            // radius) — a SECOND `Node` in the same spawn tuple would
            // REPLACE it wholesale (the exact EM-5.2 regression class; see
            // `xindeler-client::combat_hud`'s own doc comment for the full
            // story), so the row-layout overrides are applied via
            // `.entry::<Node>().and_modify(..)` (in-place field mutation) —
            // on its own statement, since `EntityEntryCommands` doesn't
            // itself expose `with_children`.
            let mut panel_entity = backdrop.spawn(panel_bundle(&theme));
            // `and_modify`'s closure is queued into `Commands` (runs later),
            // so it requires `'static` — an owned `Val` copied out of
            // `theme` BEFORE the closure, not a borrow of `theme` itself
            // (which only lives for this function call).
            let column_gap = Val::Px(theme.spacing.lg);
            panel_entity.entry::<Node>().and_modify(move |mut node| {
                node.flex_direction = FlexDirection::Row;
                node.column_gap = column_gap;
            });
            panel_entity.with_children(|panel| {
                // BL-82 EM-5.17 T57.14 — `PaperdollRoot` is now a ROW of 3
                // columns (left weapon set / center armor column / right
                // weapon set), not a flat 2-col grid of all 22 slots — see
                // the module doc comment's layout constants for the
                // confirmed arrangement.
                panel
                    .spawn((PaperdollRoot, Node {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(8.0),
                        align_items: AlignItems::FlexStart,
                        ..Default::default()
                    }))
                    .with_children(|paperdoll| {
                        paperdoll.spawn((LeftWeaponColumnRoot, Node {
                            display: Display::Flex,
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            ..Default::default()
                        }));
                        paperdoll.spawn((CenterEquipColumnRoot, Node {
                            display: Display::Flex,
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            ..Default::default()
                        }));
                        paperdoll.spawn((RightWeaponColumnRoot, Node {
                            display: Display::Flex,
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            ..Default::default()
                        }));
                    });
                panel.spawn((BagGridRoot, Node {
                    display: Display::Grid,
                    grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(8, 48.0)],
                    row_gap: Val::Px(4.0),
                    column_gap: Val::Px(4.0),
                    max_width: Val::Px(8.0 * 52.0),
                    ..Default::default()
                }));
            });
        });
}

/// Fills the (spawned-empty) paper-doll + bag grid containers with real
/// slots the FIRST time a `NetInventory` arrives: the paper-doll's slot
/// count is fixed (22, `ALL_EQUIP_SLOTS::len()`) but is spawned HERE rather
/// than at `Startup` alongside [`spawn_inventory_window`] purely to keep
/// both grids' spawn logic in one place, keyed off the same one-shot
/// [`BagGridSpawned`] latch — the bag grid genuinely can't be sized before
/// the real `capacity` is known, so this system already has to wait either
/// way. A later capacity CHANGE (rare; not exercised by any content this
/// task ships) is a documented follow-up, not handled by a resize here.
fn spawn_bag_grid_once_capacity_known(
    mut commands: Commands,
    theme: Res<HudTheme>,
    images: Res<HudImages>,
    mut spawned: ResMut<BagGridSpawned>,
    player: Query<&NetInventory, With<NetLocalPlayer>>,
    bag_root: Query<Entity, With<BagGridRoot>>,
    left_weapon_root: Query<Entity, With<LeftWeaponColumnRoot>>,
    center_root: Query<Entity, With<CenterEquipColumnRoot>>,
    right_weapon_root: Query<Entity, With<RightWeaponColumnRoot>>,
) {
    if spawned.0 {
        return;
    }
    let Ok(inventory) = player.single() else {
        return;
    };
    let Ok(bag_root_entity) = bag_root.single() else {
        return;
    };

    commands.entity(bag_root_entity).with_children(|parent| {
        for net_slot in &inventory.slots {
            // BL-82 EM-5.17 T57.13/T57.16 — every bag slot starts with a
            // (currently invisible/transparent, per `ImageNode::default()`)
            // rarity-background image node: `sync_slot_contents` swaps its
            // `image`/tint in once the slot is occupied (see that system's
            // doc comment) rather than this spawn site inserting/removing
            // the component later — a slot with items constantly moving in
            // and out just needs the ONE component mutated in place. Also
            // T57.16: `TooltipBackground(InventoryTooltipBg)` reskins this
            // slot's hover tooltip.
            parent.spawn((
                slot_bundle(
                    &theme,
                    BAG_GROUP,
                    SlotAddress::from_inv_slot_idx(net_slot.slot.idx()),
                    48.0,
                ),
                bevy::ui::widget::ImageNode::default(),
                TooltipBackground(HudImageKey::InventoryTooltipBg),
            ));
        }
    });

    // BL-82 EM-5.17 T57.14 — the 18-of-22-slot Equipment panel: 3 columns,
    // each spawned from its own `*_INDICES` constant (module doc comment).
    if let Ok(left_entity) = left_weapon_root.single() {
        commands.entity(left_entity).with_children(|parent| {
            for &idx in &LEFT_WEAPON_SET_INDICES {
                spawn_equip_slot(parent, &theme, &images, idx);
            }
        });
    }
    if let Ok(center_entity) = center_root.single() {
        commands.entity(center_entity).with_children(|parent| {
            for &idx in &CENTER_COLUMN_INDICES {
                spawn_equip_slot(parent, &theme, &images, idx);
            }
        });
    }
    if let Ok(right_entity) = right_weapon_root.single() {
        commands.entity(right_entity).with_children(|parent| {
            for &idx in &RIGHT_WEAPON_SET_INDICES {
                spawn_equip_slot(parent, &theme, &images, idx);
            }
        });
    }

    spawned.0 = true;
}

/// Spawns one Equipment-panel slot (BL-82 EM-5.17 T57.14) at
/// `ALL_EQUIP_SLOTS[idx]` — a themed [`slot_bundle`] carrying its own bespoke
/// `equip_empty_*.png` frame (via [`equip_slot_frame`]; ALL 18 shown slots have
/// a dedicated frame — see the Phase 7 report for why the originally-flagged
/// `slot_empty.png` interim fallback ended up unnecessary) plus T57.16's
/// `TooltipBackground` reskin. [`sync_two_handed_offhand_disable`] is the only
/// system that later mutates this same [`bevy::ui::widget::ImageNode`]'s tint
/// (never its `image` handle — the frame itself never changes, only whether
/// it's greyed).
fn spawn_equip_slot(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    images: &HudImages,
    idx: usize,
) {
    let equip_slot = ALL_EQUIP_SLOTS[idx];
    #[expect(
        clippy::cast_possible_truncation,
        reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
    )]
    let address = SlotAddress::from_equip_slot_discriminant(idx as u32);
    parent.spawn((
        slot_bundle(theme, EQUIP_GROUP, address, 48.0),
        bevy::ui::widget::ImageNode::new(images.get(equip_slot_frame(equip_slot))),
        TooltipBackground(HudImageKey::InventoryTooltipBg),
    ));
}

/// BL-82 EM-5.17 T57.14 — the per-`EquipSlot` `equip_empty_*.png` frame
/// (spec §3.7's confirmed layout). All 18 slots the Equipment panel shows
/// (see [`LEFT_WEAPON_SET_INDICES`]/[`CENTER_COLUMN_INDICES`]/
/// [`RIGHT_WEAPON_SET_INDICES`]) have a REAL dedicated frame asset —
/// `Ring1`/`Ring2` share the ORIGINAL single `equip_empty_ring.png` (the
/// Notion doc's 9-files-for-10-slots count already folded the two ring
/// slots together); the other 8 newly-added slots (4 weapon slots, Legs,
/// Lantern, Glider, Tabard) each got their OWN bespoke frame in this same
/// phase (the 8 new PNGs this diff adds) — so, contrary to this task's
/// original brief (which anticipated a `slot_empty.png` fallback for
/// whichever new slots shipped without dedicated art), NO fallback is
/// actually needed: every arm below has real, distinct art. `Bag1`-`Bag4`
/// are unreachable here — the Equipment panel never spawns them (excluded
/// per spec; they belong on the Items/Inventory tab).
fn equip_slot_frame(slot: EquipSlot) -> HudImageKey {
    match slot {
        EquipSlot::Armor(ArmorSlot::Head) => HudImageKey::EquipEmptyHelmet,
        EquipSlot::Armor(ArmorSlot::Neck) => HudImageKey::EquipEmptyNecklace,
        EquipSlot::Armor(ArmorSlot::Shoulders) => HudImageKey::EquipEmptyShoulders,
        EquipSlot::Armor(ArmorSlot::Chest) => HudImageKey::EquipEmptyChest,
        EquipSlot::Armor(ArmorSlot::Hands) => HudImageKey::EquipEmptyHands,
        EquipSlot::Armor(ArmorSlot::Ring1 | ArmorSlot::Ring2) => HudImageKey::EquipEmptyRing,
        EquipSlot::Armor(ArmorSlot::Back) => HudImageKey::EquipEmptyBack,
        EquipSlot::Armor(ArmorSlot::Belt) => HudImageKey::EquipEmptyBelt,
        EquipSlot::Armor(ArmorSlot::Legs) => HudImageKey::EquipEmptyLegs,
        EquipSlot::Armor(ArmorSlot::Feet) => HudImageKey::EquipEmptyFeet,
        EquipSlot::Armor(ArmorSlot::Tabard) => HudImageKey::EquipEmptyTabard,
        EquipSlot::ActiveMainhand => HudImageKey::EquipEmptyActiveMainhand,
        EquipSlot::ActiveOffhand => HudImageKey::EquipEmptyActiveOffhand,
        EquipSlot::InactiveMainhand => HudImageKey::EquipEmptyInactiveMainhand,
        EquipSlot::InactiveOffhand => HudImageKey::EquipEmptyInactiveOffhand,
        EquipSlot::Lantern => HudImageKey::EquipEmptyLantern,
        EquipSlot::Glider => HudImageKey::EquipEmptyGlider,
        EquipSlot::Armor(ArmorSlot::Bag1 | ArmorSlot::Bag2 | ArmorSlot::Bag3 | ArmorSlot::Bag4) => {
            unreachable!(
                "Bag1-4 are excluded from the Equipment panel (spec §3.7) and never spawned via \
                 spawn_equip_slot"
            )
        },
    }
}

/// BL-82 EM-5.17 T57.13 — folds Xindeler's real 8-tier [`Quality`]
/// (`common/src/comp/inventory/item/mod.rs:73-82`) down to the rarity asset
/// pack's 6 slot-background textures (spec §3.7: "a mapping decision, not a
/// blocker, but worth a short confirmation" — this is that documented
/// choice, made now rather than left silent):
/// - `Low` + `Common` fold together into the pack's own "Common" tier (both are
///   the two lowest/mundane tiers, and the pack itself has no separate "very
///   common"/"junk" visual).
/// - `Moderate` -> Uncommon, `High` -> Rare, `Epic` -> VeryRare, `Legendary` ->
///   Legendary keep a natural 1:1 step up the remaining tiers.
/// - `Artifact` (the highest REAL player-facing tier) maps to the pack's top
///   "Mythic" visual (there is no dedicated 7th texture).
/// - `Debug` is a dev-only tier that should never reach a player's bag (per the
///   spec's own note) — folded to `Mythic` too, purely so this match stays
///   total without a panic path; it is not expected to ever actually render in
///   play.
fn quality_rarity_background(quality: Quality) -> HudImageKey {
    match quality {
        Quality::Low | Quality::Common => HudImageKey::SlotBgCommon,
        Quality::Moderate => HudImageKey::SlotBgUncommon,
        Quality::High => HudImageKey::SlotBgRare,
        Quality::Epic => HudImageKey::SlotBgVeryRare,
        Quality::Legendary => HudImageKey::SlotBgLegendary,
        Quality::Artifact | Quality::Debug => HudImageKey::SlotBgMythic,
    }
}

/// The bag slot's rarity-background [`bevy::ui::widget::ImageNode`] for the
/// given (possibly absent) occupant — see [`quality_rarity_background`] for
/// the tier mapping. An empty slot gets back a plain
/// [`bevy::ui::widget::ImageNode::default`] (fully transparent — see that
/// type's own doc comment), the SAME state every bag slot starts in at
/// spawn.
fn bag_rarity_image_node(
    item: Option<&NetItemStack>,
    images: &HudImages,
) -> bevy::ui::widget::ImageNode {
    match item {
        Some(item) => {
            bevy::ui::widget::ImageNode::new(images.get(quality_rarity_background(item.quality)))
        },
        None => bevy::ui::widget::ImageNode::default(),
    }
}

/// Force-opens [`HudWindow::Inventory`] once at boot when
/// `XINDELER_SMOKE_OPEN_INVENTORY` is set — the same env-var-gated,
/// smoke-only debug-override convention `player_input.rs` already
/// establishes (`XINDELER_SMOKE_ROTATE`/`XINDELER_SMOKE_MOVE_PATTERN`/etc.):
/// `--smoke-screenshot` has no real keyboard to press `I` with, so this is
/// how a live visual smoke check (the EM-5.2 regression's own lesson: "ECS
/// proof is not a substitute for an actual pixel check") can confirm the bag
/// grid + paper-doll genuinely render, without adding a bespoke input-
/// injection mechanism to the harness itself. A no-op (never opens anything)
/// unless the env var is set — harmless in every normal run.
fn force_open_inventory_for_smoke_capture(mut state: ResMut<HudState>) {
    if std::env::var("XINDELER_SMOKE_OPEN_INVENTORY").is_ok_and(|v| v != "0") {
        state.toggle(HudWindow::Inventory);
    }
}

/// Toggles [`HudWindow::Inventory`] on [`GameInput::Inventory`] (`I` by
/// default, but rebindable — BL-82 EM-5.17 Phase 0: this used to read the
/// raw, non-rebindable `ButtonInput<KeyCode>` with a hardcoded `KeyCode::
/// KeyI`, so rebinding `Inventory` away from `I` silently did nothing here)
/// — the generic `HudAction::ToggleWindow` flow EM-5.1's state machine
/// already provides.
fn toggle_inventory_window(action_state: Res<ActionState>, mut actions: MessageWriter<HudAction>) {
    if action_state.just_pressed(GameInput::Inventory) {
        actions.write(HudAction::ToggleWindow(HudWindow::Inventory));
    }
}

/// Shows/hides the inventory window root from [`HudState`] — the SAME
/// consumption pattern every future `HudWindow`-gated screen follows.
fn sync_inventory_window_visibility(
    state: Res<HudState>,
    mut root: Query<&mut Visibility, With<InventoryWindowRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if state.is_open(HudWindow::Inventory) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Reconciles every bag/equip slot's [`SlotContents`] (+, for BAG slots
/// only, the T57.13 rarity-background [`bevy::ui::widget::ImageNode`] —
/// [`bag_rarity_image_node`]) against the local player's current
/// `NetInventory` — `Changed<NetInventory>`-gated. Degrades clean (no panic)
/// if the bag grid hasn't been spawned yet (spec §3.2).
///
/// Equip (paper-doll) slots intentionally do NOT get their `ImageNode`
/// touched here — their frame (from [`equip_slot_frame`]) is a fixed
/// per-slot-TYPE background set once at spawn, not a per-CONTENT rarity
/// tint (spec §3.7 only calls out the bag grid as rarity-tiered); only
/// [`sync_two_handed_offhand_disable`] ever mutates an equip slot's
/// `ImageNode` afterward (its tint, for the greyed-out state), which is why
/// the query below is scoped by [`SlotGroup`] rather than reusing one
/// generic "any slot with this address" lookup for both loops.
fn sync_slot_contents(
    player: Query<&NetInventory, (With<NetLocalPlayer>, Changed<NetInventory>)>,
    mut slots: Query<
        (
            &SlotAddress,
            &SlotGroup,
            &mut SlotContents,
            &mut bevy::ui::widget::ImageNode,
        ),
        With<HudSlot>,
    >,
    images: Res<HudImages>,
) {
    let Ok(inventory) = player.single() else {
        return;
    };

    for net_slot in &inventory.slots {
        let address = SlotAddress::from_inv_slot_idx(net_slot.slot.idx());
        if let Some((_, _, mut contents, mut image)) = slots
            .iter_mut()
            .find(|(a, g, _, _)| **a == address && **g == BAG_GROUP)
        {
            *contents = net_item_to_slot_contents(net_slot.item.as_ref());
            *image = bag_rarity_image_node(net_slot.item.as_ref(), &images);
        }
    }
    for (idx, equipped) in inventory.equipped.iter().enumerate() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
        )]
        let address = SlotAddress::from_equip_slot_discriminant(idx as u32);
        if let Some((_, _, mut contents, _image)) = slots
            .iter_mut()
            .find(|(a, g, _, _)| **a == address && **g == EQUIP_GROUP)
        {
            *contents = net_item_to_slot_contents(equipped.item.as_ref());
        }
    }
}

/// BL-82 EM-5.17 T57.15 — reflects (never re-implements) `EquipSlot::
/// can_hold`'s existing two-handed-weapon rule (`common/src/comp/inventory/
/// slot.rs`): when a weapon set's Mainhand currently holds a `Hands::Two`
/// item (mirrored client-side via [`NetItemStack::is_two_handed`] — see
/// that field's own doc comment for why this is the chosen data path),
/// that SAME set's Offhand slot renders greyed (its
/// [`bevy::ui::widget::ImageNode`] tint darkens) and stops accepting
/// pointer input (`Pickable::IGNORE` — no drag can target it, matching
/// spec §3.7's "disabled/greyed/**blocked**"). The sim already refuses to
/// let a two-handed Mainhand's paired Offhand hold anything, so the Offhand
/// slot is guaranteed empty in that state; this system only makes that
/// guaranteed-empty, guaranteed-unusable state visually obvious instead of
/// showing what would otherwise look like a normal, equippable empty slot.
/// Independent per set: the Active set's state never affects the Inactive
/// set's Offhand, and vice versa. `Changed<NetInventory>`-gated, same as
/// [`sync_slot_contents`].
fn sync_two_handed_offhand_disable(
    player: Query<&NetInventory, (With<NetLocalPlayer>, Changed<NetInventory>)>,
    mut equip_slots: Query<
        (
            &SlotAddress,
            &mut bevy::ui::widget::ImageNode,
            &mut bevy::picking::Pickable,
        ),
        With<HudSlot>,
    >,
) {
    let Ok(inventory) = player.single() else {
        return;
    };

    let mainhand_is_two_handed = |mainhand: EquipSlot| -> bool {
        inventory
            .equipped
            .iter()
            .find(|equipped| equipped.slot == mainhand)
            .and_then(|equipped| equipped.item.as_ref())
            .is_some_and(|item| item.is_two_handed)
    };

    for (mainhand, offhand) in [
        (EquipSlot::ActiveMainhand, EquipSlot::ActiveOffhand),
        (EquipSlot::InactiveMainhand, EquipSlot::InactiveOffhand),
    ] {
        let disabled = mainhand_is_two_handed(mainhand);
        let Some(discriminant) = ALL_EQUIP_SLOTS.iter().position(|&slot| slot == offhand) else {
            continue;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
        )]
        let address = SlotAddress::from_equip_slot_discriminant(discriminant as u32);
        if let Some((_, mut image, mut pickable)) =
            equip_slots.iter_mut().find(|(a, _, _)| **a == address)
        {
            image.color = if disabled {
                disabled_offhand_tint()
            } else {
                Color::WHITE
            };
            *pickable = if disabled {
                bevy::picking::Pickable::IGNORE
            } else {
                bevy::picking::Pickable::default()
            };
        }
    }
}

/// The greyed-out tint [`sync_two_handed_offhand_disable`] applies to a
/// disabled Offhand slot's frame — a dark, partly-transparent multiply
/// tint (not a new [`HudTheme`] token: this is a one-off per-slot STATE
/// tint, not a reusable palette colour).
fn disabled_offhand_tint() -> Color { Color::srgba(0.32, 0.32, 0.32, 0.75) }

fn net_item_to_slot_contents(item: Option<&xindeler_protocol::NetItemStack>) -> SlotContents {
    match item {
        Some(item) => SlotContents {
            icon_text: item.name.chars().take(3).collect(),
            quantity: Some(item.amount),
            tooltip: format!("{} ({:?}) ×{}", item.name, item.quality, item.amount),
        },
        None => SlotContents::default(),
    }
}

/// Translates a completed [`SlotDropped`] between a bag/equip slot pair into
/// a real [`InventoryActionRequest`] (`InventoryManip::Swap`) — the only
/// place this screen writes toward the sim, and it does so purely by sending
/// the REAL wire message; the sim itself validates the move
/// (`server::events::inventory_manip`).
fn handle_slot_drops(
    mut drops: MessageReader<SlotDropped>,
    mut requests: MessageWriter<InventoryActionRequest>,
) {
    for drop in drops.read() {
        let Some(from_slot) = address_to_slot(drop.from_group, drop.from_address) else {
            continue;
        };
        let Some(to_slot) = address_to_slot(drop.to_group, drop.to_address) else {
            continue;
        };
        requests.write(InventoryActionRequest(common::comp::InventoryManip::Swap(
            from_slot, to_slot,
        )));
    }
}

/// Unpacks a [`BAG_GROUP`] [`SlotAddress`] back into the real `InvSlotId` it
/// encodes (the inverse of `SlotAddress::from_inv_slot_idx`). `pub(crate)`:
/// `crate::trade_ui` reuses this to resolve a dragged bag slot's identity
/// without going through the full `Slot` enum (a trade offer's `AddItem`
/// only ever needs the bare `InvSlotId`).
pub(crate) fn bag_address_to_inv_slot(address: SlotAddress) -> InvSlotId {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "InvSlotId::idx() packs into a u32"
    )]
    let idx = address.raw() as u32;
    InvSlotId::new((idx >> 16) as u16, (idx & 0xFFFF) as u16)
}

/// Unpacks a `(SlotGroup, SlotAddress)` pair back into a real
/// `common::comp::inventory::slot::Slot` — the inverse of the packing this
/// screen does when spawning slots.
fn address_to_slot(group: SlotGroup, address: SlotAddress) -> Option<Slot> {
    if group == BAG_GROUP {
        Some(Slot::Inventory(bag_address_to_inv_slot(address)))
    } else if group == EQUIP_GROUP {
        let discriminant = (address.raw() & 0xFFFF_FFFF) as u32;
        ALL_EQUIP_SLOTS
            .get(discriminant as usize)
            .copied()
            .map(Slot::Equip)
    } else {
        None
    }
}

/// Diffs the local player's bag against [`LastSeenBag`] and pushes a real
/// notification for any slot whose occupied amount increased or that newly
/// became occupied — see the module doc comment's "Loot-pickup feed" note
/// for why this is a diff, not a dedicated pickup event.
fn push_loot_pickup_notifications(
    player: Query<&NetInventory, (With<NetLocalPlayer>, Changed<NetInventory>)>,
    mut last_seen: ResMut<LastSeenBag>,
    mut queue: ResMut<xindeler_ui::notification::NotificationQueue>,
) {
    let Ok(inventory) = player.single() else {
        return;
    };
    // The very FIRST `NetInventory` a session ever sees (freshly logged in,
    // a full starting kit already in the bag) must not flood the toast queue
    // with a "pickup" for every starting item — only real deltas AFTER the
    // baseline is established count as a pickup. Tracked as its own flag
    // (not "was `slots` empty") so a player who starts/currently has a
    // genuinely empty bag doesn't suppress notifications forever.
    let is_first_observation = !last_seen.baseline_established;

    for net_slot in &inventory.slots {
        let Some(item) = &net_slot.item else {
            last_seen.slots.remove(&net_slot.slot);
            continue;
        };
        let previous = last_seen.slots.get(&net_slot.slot).copied().unwrap_or(0);
        if item.amount > previous && !is_first_observation {
            queue.push(format!("+{} {}", item.amount - previous, item.name));
        }
        last_seen.slots.insert(net_slot.slot, item.amount);
    }
    last_seen.baseline_established = true;
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use common::comp::inventory::{
        item::ItemDefinitionIdOwned,
        slot::{ArmorSlot, EquipSlot},
    };
    use xindeler_protocol::NetEquippedSlot;

    use super::*;

    #[test]
    fn bag_address_round_trips_through_inv_slot_idx() {
        let inv = InvSlotId::new(2, 7);
        let address = SlotAddress::from_inv_slot_idx(inv.idx());
        let slot = address_to_slot(BAG_GROUP, address).expect("bag group resolves");
        assert_eq!(slot, Slot::Inventory(inv));
    }

    #[test]
    fn equip_address_round_trips_through_all_equip_slots() {
        let discriminant = ALL_EQUIP_SLOTS
            .iter()
            .position(|&s| s == EquipSlot::Armor(ArmorSlot::Chest))
            .expect("Chest is in the canonical list") as u32;
        let address = SlotAddress::from_equip_slot_discriminant(discriminant);
        let slot = address_to_slot(EQUIP_GROUP, address).expect("equip group resolves");
        assert_eq!(slot, Slot::Equip(EquipSlot::Armor(ArmorSlot::Chest)));
    }

    /// An unrecognized group resolves to `None` rather than a bogus slot —
    /// e.g. a drop involving a slot from a screen this one doesn't own
    /// (future hotbar/crafting groups).
    #[test]
    fn an_unknown_group_resolves_to_none() {
        assert!(address_to_slot(SlotGroup(99), SlotAddress(0)).is_none());
    }

    /// BL-82 EM-5.17 T57.13 — pins the documented 8-tier-to-6-texture rarity
    /// fold (this function's own doc comment) so a future `Quality` variant
    /// addition/reorder can't silently change which pack texture a tier
    /// shows without a test noticing. `Low`/`Common` share one texture;
    /// `Artifact`/`Debug` share the top one; every other tier is a distinct
    /// 1:1 step.
    #[test]
    fn quality_rarity_background_folds_all_eight_tiers_to_the_six_pack_textures() {
        assert_eq!(
            quality_rarity_background(Quality::Low),
            HudImageKey::SlotBgCommon
        );
        assert_eq!(
            quality_rarity_background(Quality::Common),
            HudImageKey::SlotBgCommon
        );
        assert_eq!(
            quality_rarity_background(Quality::Moderate),
            HudImageKey::SlotBgUncommon
        );
        assert_eq!(
            quality_rarity_background(Quality::High),
            HudImageKey::SlotBgRare
        );
        assert_eq!(
            quality_rarity_background(Quality::Epic),
            HudImageKey::SlotBgVeryRare
        );
        assert_eq!(
            quality_rarity_background(Quality::Legendary),
            HudImageKey::SlotBgLegendary
        );
        assert_eq!(
            quality_rarity_background(Quality::Artifact),
            HudImageKey::SlotBgMythic
        );
        assert_eq!(
            quality_rarity_background(Quality::Debug),
            HudImageKey::SlotBgMythic
        );
    }

    fn spawn_equip_slot_for_test(world: &mut World, slot: EquipSlot) -> bevy::ecs::entity::Entity {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
        )]
        let discriminant = ALL_EQUIP_SLOTS
            .iter()
            .position(|&s| s == slot)
            .expect("slot is in the canonical list") as u32;
        let address = SlotAddress::from_equip_slot_discriminant(discriminant);
        world
            .spawn((
                HudSlot,
                address,
                EQUIP_GROUP,
                bevy::ui::widget::ImageNode::default(),
                bevy::picking::Pickable::default(),
            ))
            .id()
    }

    fn two_handed_weapon_stack() -> NetItemStack {
        NetItemStack {
            item_id: ItemDefinitionIdOwned::Simple(
                "common.items.weapons.greatsword.starter".to_owned(),
            ),
            name: "Starter Greatsword".to_owned(),
            amount: 1,
            quality: Quality::Common,
            is_two_handed: true,
        }
    }

    fn one_handed_weapon_stack() -> NetItemStack {
        NetItemStack {
            item_id: ItemDefinitionIdOwned::Simple(
                "common.items.weapons.dagger.starter_dagger".to_owned(),
            ),
            name: "Starter Dagger".to_owned(),
            amount: 1,
            quality: Quality::Common,
            is_two_handed: false,
        }
    }

    /// BL-82 EM-5.17 T57.15 — the core behavioral requirement from spec
    /// §3.7: a two-handed Mainhand greys/blocks its OWN set's Offhand only,
    /// independently per set. Spawns both weapon sets' 4 slots, equips a
    /// two-handed weapon on `ActiveMainhand` and a one-handed weapon on
    /// `InactiveMainhand`, and asserts: `ActiveOffhand` is greyed +
    /// `Pickable::IGNORE`; `InactiveOffhand`, `ActiveMainhand`, and
    /// `InactiveMainhand` are all untouched (`Color::WHITE` +
    /// `Pickable::default()`).
    #[test]
    fn sync_two_handed_offhand_disable_greys_out_only_the_two_handed_sets_paired_offhand() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let active_mainhand = spawn_equip_slot_for_test(app.world_mut(), EquipSlot::ActiveMainhand);
        let active_offhand = spawn_equip_slot_for_test(app.world_mut(), EquipSlot::ActiveOffhand);
        let inactive_mainhand =
            spawn_equip_slot_for_test(app.world_mut(), EquipSlot::InactiveMainhand);
        let inactive_offhand =
            spawn_equip_slot_for_test(app.world_mut(), EquipSlot::InactiveOffhand);

        app.world_mut().spawn((NetLocalPlayer, NetInventory {
            slots: Vec::new(),
            equipped: vec![
                NetEquippedSlot {
                    slot: EquipSlot::ActiveMainhand,
                    item: Some(two_handed_weapon_stack()),
                },
                NetEquippedSlot {
                    slot: EquipSlot::InactiveMainhand,
                    item: Some(one_handed_weapon_stack()),
                },
            ],
            capacity: 0,
        }));

        app.world_mut()
            .run_system_once(sync_two_handed_offhand_disable)
            .expect("system runs");

        let is_disabled = |world: &World, entity: bevy::ecs::entity::Entity| -> bool {
            let image = world
                .get::<bevy::ui::widget::ImageNode>(entity)
                .expect("slot carries an ImageNode");
            let pickable = world
                .get::<bevy::picking::Pickable>(entity)
                .expect("slot carries a Pickable");
            image.color == disabled_offhand_tint() && *pickable == bevy::picking::Pickable::IGNORE
        };

        assert!(
            is_disabled(app.world(), active_offhand),
            "ActiveOffhand must grey out when ActiveMainhand holds a two-handed weapon"
        );
        assert!(
            !is_disabled(app.world(), inactive_offhand),
            "InactiveOffhand must stay usable — InactiveMainhand holds a ONE-handed weapon"
        );
        assert!(
            !is_disabled(app.world(), active_mainhand),
            "the Mainhand slot itself is never disabled by this system"
        );
        assert!(
            !is_disabled(app.world(), inactive_mainhand),
            "the Mainhand slot itself is never disabled by this system"
        );
    }
}
