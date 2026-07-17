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
//!
//! ## Items / Equipment tab split (BL-82 EM-5.18 Phase 1)
//! The bag grid ([`BagGridRoot`]) and the 18-slot paper-doll
//! ([`PaperdollRoot`]) used to render as SIBLINGS in the same panel row (the
//! Phase 7 "stacked in the same panel" problem Matías flagged live-testing).
//! They are now split into two mutually-exclusive tabs — [`InventoryTab`]
//! (a plain resource, not a component — mirrors `diary.rs`'s `DiaryTab`
//! shape exactly, spec §1.3/§3.1) picks which of [`ItemsTabRoot`] (wraps
//! `BagGridRoot`) / [`EquipmentTabRoot`] (wraps `PaperdollRoot`) is mounted
//! with `Node::display: Flex` at a time — see
//! [`sync_inventory_tab_content_visibility`]'s own doc comment for why
//! `Node::display`, not `Visibility`, is the correct toggle here.
//!
//! **Temporary regression, expected and documented (NOT a bug):** because
//! only one tab's slots are ever mounted with `Node::display: Flex`
//! simultaneously, a bag slot and an equip slot are never BOTH laid out at
//! the same time once the tabs are separate — Bevy cannot drag an item
//! between an entity with a real layout box and one that is `Display::None`.
//! **Between this Phase 1 merging and BL-82 EM-5.18 Phase 2 (the click-slot
//! equip-picker modal) merging, there is NO way to equip or unequip an item
//! via any path** — this is a structural, intentional consequence of the tab
//! split itself (spec §3.1/§3.6), not a regression introduced by mistake.
//! Same-tab dragging (bag-to-bag reordering within Items; weapon-set-to-
//! weapon-set within Equipment) is untouched and keeps working, since both
//! ends of a same-tab drag stay mounted together.
//!
//! ## Click-to-equip picker modal (BL-82 EM-5.18 Phase 2)
//! Closes the P1 gap noted above: clicking an equip slot
//! ([`spawn_equip_slot`]'s new `.observe(On<Pointer<Click>>, ..)`, T58.10)
//! opens [`EquipPickerRoot`] — a SIBLING of [`InventoryWindowRoot`] (not
//! nested inside its `Row` panel), listing every bag item whose (server-
//! computed, T58.7) `NetItemStack::equippable_slots` contains the clicked
//! slot, plus an "Unequip" row when the slot is already occupied
//! ([`rebuild_equip_picker_contents`], T58.12). Picking a row (or Unequip)
//! sends the SAME `InventoryActionRequest(InventoryManip::Swap(..))` drag-
//! drop already sent (spec §3.3) and closes the picker. [`EquipPickerState`]
//! (a plain resource, outside `HudState` — no `HudWindow` variant fits a
//! transient sub-modal of an already-open window) tracks which slot (if any)
//! is open; [`sync_equip_picker_visibility`] toggles the root's
//! `Visibility` (NOT `Node::display` — this root has no `Row`-direction
//! flex siblings of its own, unlike P1's tab toggle, so `Visibility` is safe
//! here). `Escape` closes the picker ([`close_equip_picker_on_escape`],
//! T58.13) — see that system's own doc comment for why this is collision-
//! free with `camera.rs`/`chat.rs`/`map_view.rs`'s own independent `Escape`
//! consumers.

use bevy::{
    ecs::schedule::common_conditions::not,
    picking::events::{Click, Pointer},
    prelude::*,
};
use common::comp::inventory::{
    item::Quality,
    slot::{ArmorSlot, EquipSlot, InvSlotId, Slot},
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    InventoryActionRequest, NetInventory, NetItemStack, NetLocalPlayer, inventory::ALL_EQUIP_SLOTS,
};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    images::{HudImageKey, HudImages},
    panel::{image_panel_bundle, panel_bundle},
    scroll::scroll_view_bundle,
    slot::{
        HudSlot, SlotAddress, SlotContents, SlotDropped, SlotGroup, slot_bundle,
        slot_bundle_with_rarity,
    },
    theme::{HudFonts, HudTheme},
    tooltip::TooltipBackground,
    zlayer,
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
/// BL-82 EM-5.18 Phase 2 — the equip-picker's candidate-item rows (T58.12).
/// A fresh [`SlotGroup`] alongside hotbar (`0`)/bag (`1`)/equip (`2`)/the two
/// trade-offer groups (`3`/`4`)/`diary.rs`'s Abilities tab (`5`) — reusing
/// [`slot_bundle_with_rarity`] for a picker row's icon means it inherits the
/// SAME global drag-drop observers every [`HudSlot`] gets
/// (`xindeler_ui::slot::install_observers`); a drag started FROM a picker row
/// is harmless (this group isn't recognized by [`address_to_slot`], so any
/// resulting `SlotDropped` resolves to `None` and is silently ignored, the
/// same "unknown group" contract this file already tests) — flagged, not a
/// blocker.
const EQUIP_PICKER_GROUP: SlotGroup = SlotGroup(6);

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

/// The two tabs this inventory window splits into (BL-82 EM-5.18 Phase 1,
/// spec §1/§3.1): "Items" = the bag grid ([`BagGridRoot`]), "Equipment" = the
/// paper-doll ([`PaperdollRoot`] + its 3 columns). Mirrors `diary.rs`'s
/// `DiaryTab` shape (a plain [`Resource`], not a component) but simplified —
/// there's no dynamic tab list here (always exactly these 2), so the tab
/// buttons spawn once in [`spawn_inventory_window`] rather than via a
/// `sync_diary_tabs`-style reactive rebuild.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Default)]
enum InventoryTab {
    #[default]
    Items,
    Equipment,
}

/// Marks the tab-button row container (BL-82 EM-5.18 Phase 1) — mirrors
/// `diary.rs`'s `DiaryTabBar`.
#[derive(Component)]
struct InventoryTabBar;
/// Tags a tab button with which [`InventoryTab`] it selects on click —
/// mirrors `diary.rs`'s `DiaryTabButton`.
#[derive(Component, Clone, Copy)]
struct InventoryTabButton(InventoryTab);
/// Wraps [`BagGridRoot`] — the Items tab's content root (BL-82 EM-5.18 Phase
/// 1, spec §3.1). `BagGridRoot`'s own internal spawn logic
/// ([`spawn_bag_grid_once_capacity_known`]) is completely unchanged; this is
/// purely a new parent one level up.
#[derive(Component)]
struct ItemsTabRoot;
/// Wraps [`PaperdollRoot`] (its 3 flanking/center columns) — the Equipment
/// tab's content root (BL-82 EM-5.18 Phase 1, spec §3.1). `PaperdollRoot`'s
/// own internal spawn logic is completely unchanged; this is purely a new
/// parent one level up.
#[derive(Component)]
struct EquipmentTabRoot;

/// BL-82 EM-5.18 Phase 2 (T58.11, spec §3.3) — which [`EquipSlot`] the
/// click-to-equip picker modal currently shows candidates for (`None` =
/// closed). Lives OUTSIDE [`HudState`] — no `HudWindow` variant fits a
/// transient sub-modal of an already-open window (the same posture
/// `social_hud.rs`'s `ActiveDialogue` already uses).
#[derive(Resource, Default)]
struct EquipPickerState {
    open_slot: Option<EquipSlot>,
}

/// Marks the equip-picker modal's top-level root — a SIBLING of
/// [`InventoryWindowRoot`] (not nested in its `Row` panel), spawned once at
/// `Startup` (BL-82 EM-5.18 T58.11).
#[derive(Component)]
struct EquipPickerRoot;
/// Marks the picker's scrollable content container — the parent
/// [`rebuild_equip_picker_contents`] despawns/respawns children of.
#[derive(Component)]
struct EquipPickerContentRoot;
/// Tags a rendered candidate-item row with the bag [`InvSlotId`] it
/// represents — lets tests introspect which items [`rebuild_equip_picker_
/// contents`] actually rendered without parsing the row's `Text` child.
/// `#[allow(dead_code)]`: the `InvSlotId` field is only ever READ by this
/// module's own tests (`picker_filters_items_to_compatible_equip_slots_only`)
/// — no production call site needs to read it back (the click handler
/// captures its own `bag_slot` by value instead) — documenting that as
/// deliberate, not an oversight, so a non-`--all-targets` clippy run doesn't
/// flag a real, tested field as unused.
#[derive(Component)]
#[allow(dead_code)]
struct EquipPickerItemRow(InvSlotId);
/// Tags the picker's "Unequip" row — lets tests assert its presence/absence
/// without parsing button labels.
#[derive(Component)]
struct EquipPickerUnequipRow;

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
            .init_resource::<InventoryTab>()
            .init_resource::<EquipPickerState>()
            .add_systems(
                Startup,
                (
                    spawn_inventory_window.after(xindeler_ui::theme::init_theme),
                    // BL-82 EM-5.18 T58.11 — a top-level SIBLING of
                    // `InventoryWindowRoot`, not nested inside it.
                    spawn_equip_picker_root.after(xindeler_ui::theme::init_theme),
                    force_open_inventory_for_smoke_capture,
                    // BL-82 EM-5.18 T58.15 (P3 parity check) — the
                    // Equipment-tab counterpart to
                    // `force_open_inventory_for_smoke_capture`, so a live
                    // `--smoke-screenshot` can capture the 18-slot
                    // paper-doll specifically instead of only the default
                    // Items tab (`--smoke-screenshot` has no real mouse to
                    // click the Equipment tab button with).
                    force_select_equipment_tab_for_smoke_capture,
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
                    // BL-82 EM-5.18 Phase 1 — the Items/Equipment tab toggle;
                    // see this system's own doc comment for why `Node::
                    // display`, not `Visibility`.
                    sync_inventory_tab_content_visibility,
                    spawn_bag_grid_once_capacity_known,
                    sync_slot_contents.after(spawn_bag_grid_once_capacity_known),
                    // BL-82 EM-5.17 T57.15 — must run after slots exist so a
                    // fresh spawn's initial 2H state applies the same frame
                    // it can (a later `NetInventory` change self-corrects it
                    // either way; see this system's own doc comment).
                    sync_two_handed_offhand_disable.after(spawn_bag_grid_once_capacity_known),
                    handle_slot_drops,
                    push_loot_pickup_notifications,
                    // BL-82 EM-5.18 Phase 2 — the equip-picker modal.
                    sync_equip_picker_visibility,
                    rebuild_equip_picker_contents,
                    close_equip_picker_on_escape,
                ),
            );
    }
}

/// A short display label for an [`InventoryTab`] — no i18n depth needed yet
/// (matches this crate's other placeholder-label posture, e.g. `diary.rs`'s
/// `group_label` for the parts real i18n doesn't cover).
fn inventory_tab_label(tab: InventoryTab) -> &'static str {
    match tab {
        InventoryTab::Items => "Items",
        InventoryTab::Equipment => "Equipment",
    }
}

/// Spawns the (initially hidden) inventory window: a full-screen dim
/// backdrop containing a themed panel with a 2-button tab bar
/// ([`InventoryTabBar`]) followed by [`ItemsTabRoot`] (wraps the bag grid,
/// [`BagGridRoot`] — an EMPTY container; [`spawn_bag_grid_once_capacity_known`]
/// fills it in once the real capacity is known) and [`EquipmentTabRoot`]
/// (wraps the paper-doll, [`PaperdollRoot`] — all 22 equip slots, fixed size,
/// spawned now).
///
/// BL-82 EM-5.18 Phase 1 (spec §3.1): before this change, `PaperdollRoot` and
/// `BagGridRoot` spawned as SIBLINGS directly under this panel row — the
/// "stacked in the same panel" problem Matías flagged live-testing. They are
/// now each wrapped in their own tab-content root, and only ONE of
/// `ItemsTabRoot`/`EquipmentTabRoot` is ever mounted with `Node::display:
/// Flex` at a time (see [`sync_inventory_tab_content_visibility`]). Both
/// tabs' INTERNAL content (the bag grid's later fill-in, the paper-doll's 3
/// columns/`*_INDICES` constants/`spawn_equip_slot`) is byte-identical to
/// Phase 7 — only this new wrapping parent + the tab bar are added. The tab
/// set is fixed (always exactly 2), so — unlike `diary.rs`'s
/// `sync_diary_tabs`, which reactively rebuilds a DYNAMIC tab list — the 2
/// tab buttons spawn once, right here, with no reactive rebuild system
/// needed.
///
/// BL-82 EM-5.17/5.18 click-routing fix: `InventoryWindowRoot` is one of the
/// three consumers the zlayer scheme's own doc comment names for
/// `MODAL_WINDOWS` (diary/inventory/full-map), and [`spawn_equip_picker_root`]
/// below already assumed it carried that tier (its own doc comment says "one
/// tier ABOVE `InventoryWindowRoot`'s own `MODAL_WINDOWS`") — but this spawn
/// tuple never actually applied `GlobalZIndex(MODAL_WINDOWS)`, unlike
/// `diary.rs`'s `DiaryWindowRoot`. Left at the default z-partition (0), this
/// root sat BELOW the always-on ambient chrome once it gained its own higher
/// z-index this phase (hotbar/orbs = `ORBS_ACTION_BAR_PARTY_MINIMAP`=20) —
/// wherever the Inventory window visually overlapped that chrome,
/// `bevy_ui` picking (which resolves the highest z-partition first) routed
/// clicks to the chrome in front instead of the inventory panel underneath.
fn spawn_inventory_window(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    commands
        .spawn((
            InventoryWindowRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::MODAL_WINDOWS),
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
                // BL-82 EM-5.18 Phase 1 — the 2-button tab bar, mirroring
                // `diary.rs::sync_diary_tabs`'s per-button `.observe(On<
                // Activate>)` idiom verbatim (spec §1.3/§3.1), just spawned
                // once here instead of via a reactive rebuild (the tab set
                // never changes).
                panel
                    .spawn((InventoryTabBar, Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        min_width: Val::Px(140.0),
                        ..Default::default()
                    }))
                    .with_children(|tab_bar| {
                        for tab in [InventoryTab::Items, InventoryTab::Equipment] {
                            tab_bar
                                .spawn(button_bundle(&theme, &fonts, inventory_tab_label(tab)))
                                .insert(InventoryTabButton(tab))
                                .observe(
                                    move |activate: On<Activate>,
                                          buttons: Query<&InventoryTabButton>,
                                          mut selected: ResMut<InventoryTab>| {
                                        if let Ok(button) = buttons.get(activate.entity) {
                                            *selected = button.0;
                                        }
                                    },
                                );
                        }
                    });

                // `ItemsTabRoot` starts `Flex` (Items is the default tab);
                // `EquipmentTabRoot` starts `None` — matching `InventoryTab`'s
                // `#[default]` variant. `sync_inventory_tab_content_visibility`
                // is the only system that ever changes either afterward.
                panel
                    .spawn((ItemsTabRoot, Node {
                        display: Display::Flex,
                        ..Default::default()
                    }))
                    .with_children(|items_tab| {
                        items_tab.spawn((BagGridRoot, Node {
                            display: Display::Grid,
                            grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(8, 48.0)],
                            row_gap: Val::Px(4.0),
                            column_gap: Val::Px(4.0),
                            max_width: Val::Px(8.0 * 52.0),
                            ..Default::default()
                        }));
                    });

                panel
                    .spawn((EquipmentTabRoot, Node {
                        display: Display::None,
                        ..Default::default()
                    }))
                    .with_children(|equipment_tab| {
                        // BL-82 EM-5.17 T57.14 — `PaperdollRoot` is a ROW of 3
                        // columns (left weapon set / center armor column /
                        // right weapon set), not a flat 2-col grid of all 22
                        // slots — see the module doc comment's layout
                        // constants for the confirmed arrangement. Unchanged
                        // by this Phase 1 restructure other than its new
                        // `EquipmentTabRoot` parent.
                        equipment_tab
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
                    });
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
///
/// BL-82 EM-5.18 T58.10 — also attaches [`on_equip_slot_click`], opening the
/// equip-picker modal for THIS slot on click. Entirely additive/local to this
/// function; `xindeler_ui::slot`/`trade_ui.rs`/`hotbar.rs` are untouched
/// (spec §3.2). Two-handed-disabled Offhand slots already carry
/// `Pickable::IGNORE` ([`sync_two_handed_offhand_disable`], unchanged) — the
/// click observer simply never fires for them (`bevy_picking`'s own backend
/// excludes `Pickable::IGNORE` entities from hit-testing before any observer
/// runs), no extra guard needed here.
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
    parent
        .spawn((
            slot_bundle(theme, EQUIP_GROUP, address, 48.0),
            bevy::ui::widget::ImageNode::new(images.get(equip_slot_frame(equip_slot))),
            TooltipBackground(HudImageKey::InventoryTooltipBg),
        ))
        .observe(on_equip_slot_click(equip_slot));
}

/// BL-82 EM-5.18 T58.10 — builds the per-entity `.observe(On<Pointer<
/// Click>>, ..)` closure [`spawn_equip_slot`] attaches to every equip slot:
/// clicking it opens the equip-picker modal for THAT slot. Split out into a
/// named function (rather than an inline closure at the call site) so
/// [`equip_slot_click_opens_picker_with_correct_slot`] can attach the EXACT
/// same wiring to a bare test entity without needing a full `HudTheme`/
/// `HudImages`-backed [`spawn_equip_slot`] call (mirrors this file's own
/// `spawn_equip_slot_for_test` convention of building only the shape a test
/// needs).
fn on_equip_slot_click(
    equip_slot: EquipSlot,
) -> impl Fn(On<Pointer<Click>>, ResMut<EquipPickerState>) + Send + Sync + 'static {
    move |_: On<Pointer<Click>>, mut picker: ResMut<EquipPickerState>| {
        picker.open_slot = Some(equip_slot);
    }
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

/// BL-82 EM-5.18 T58.15 (P3 parity check) — forces `InventoryTab::Equipment`
/// once at boot when `XINDELER_SMOKE_INVENTORY_TAB=equipment` is set, the
/// same env-var-gated, smoke-only debug-override convention
/// [`force_open_inventory_for_smoke_capture`] (immediately above) already
/// establishes: `--smoke-screenshot` has no real mouse to click the
/// Equipment tab button with, so this is how a live visual parity check
/// against Phase 7's shipped, Matías-approved 18-slot paper-doll layout
/// (spec/task T58.15) can capture the Equipment tab specifically, rather
/// than only ever capturing the default (`InventoryTab::Items`) tab. A
/// no-op (the tab stays on its `Default` value, `Items`) unless the env var
/// is set to exactly `"equipment"` — harmless in every normal run, and in
/// every OTHER smoke capture that doesn't set it.
fn force_select_equipment_tab_for_smoke_capture(mut tab: ResMut<InventoryTab>) {
    if std::env::var("XINDELER_SMOKE_INVENTORY_TAB").as_deref() == Ok("equipment") {
        *tab = InventoryTab::Equipment;
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

/// Toggles [`ItemsTabRoot`]'s and [`EquipmentTabRoot`]'s content to match the
/// currently-selected [`InventoryTab`] — via `Node::display` (`Flex`/`None`),
/// NOT `Visibility` (BL-82 EM-5.18 Phase 1, spec §3.1).
///
/// ## Why `Display`, not `Visibility` (a real bug this crate already fixed once)
/// `Visibility::Hidden` only skips RENDERING an entity — it does NOT remove
/// it from `taffy`'s layout computation, so a `Row`-direction panel with both
/// tab-content containers as siblings would still lay them out SIDE BY SIDE
/// regardless of which one is "hidden," summing BOTH widths into the row and
/// mis-sizing/off-centering the whole panel. This is the exact same bug
/// `diary.rs::sync_tab_content_visibility`'s own doc comment documents (a
/// live `--smoke-screenshot` of the Diary window caught it there: the
/// darkened backdrop rendered, but no panel content was ever visible
/// anywhere on screen, because all three of Stats/Tree/Abilities summed their
/// widths regardless of which was "selected"). This inventory panel is the
/// IDENTICAL shape (`FlexDirection::Row` with tab-content siblings), so this
/// system copies that fix verbatim: `Node::display = Display::None` removes
/// an entity from layout entirely (zero size, as if it weren't there), so
/// only the ONE currently-selected tab's content ever contributes to the
/// row's width. Do not "fix" this back to `Visibility` — that would
/// reintroduce the exact bug `diary.rs` already root-caused once in this
/// same crate.
fn sync_inventory_tab_content_visibility(
    selected: Res<InventoryTab>,
    mut items: Query<&mut Node, (With<ItemsTabRoot>, Without<EquipmentTabRoot>)>,
    mut equipment: Query<&mut Node, (With<EquipmentTabRoot>, Without<ItemsTabRoot>)>,
) {
    if !selected.is_changed() {
        return;
    }
    fn display_for(is_selected: bool) -> Display {
        if is_selected {
            Display::Flex
        } else {
            Display::None
        }
    }
    if let Ok(mut node) = items.single_mut() {
        node.display = display_for(matches!(*selected, InventoryTab::Items));
    }
    if let Ok(mut node) = equipment.single_mut() {
        node.display = display_for(matches!(*selected, InventoryTab::Equipment));
    }
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

/// BL-82 EM-5.18 Phase 2 (T58.11, spec §3.3) — spawns the (initially hidden)
/// equip-picker modal at `Startup`: a full-screen absolute dim backdrop
/// ([`EquipPickerRoot`], `GlobalZIndex(zlayer::MODAL_WINDOWS_STACKED)` — one
/// tier ABOVE `InventoryWindowRoot`'s own `MODAL_WINDOWS`, so it renders
/// stacked on top of the already-open Inventory window) containing a
/// centered [`image_panel_bundle`] (reusing [`HudImageKey::InventoryBg`] —
/// unused elsewhere today, since [`spawn_inventory_window`] itself still uses
/// the flat [`panel_bundle`]) wrapping a [`scroll_view_bundle`]
/// ([`EquipPickerContentRoot`], the parent [`rebuild_equip_picker_contents`]
/// fills in). A top-level SIBLING of [`InventoryWindowRoot`] — NOT nested in
/// its `Row` panel (spec §3.3) — so opening it never perturbs the
/// Inventory window's own tab layout.
fn spawn_equip_picker_root(mut commands: Commands, theme: Res<HudTheme>, images: Res<HudImages>) {
    commands
        .spawn((
            EquipPickerRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::MODAL_WINDOWS_STACKED),
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
            backdrop
                .spawn(image_panel_bundle(
                    &theme,
                    images.get(HudImageKey::InventoryBg),
                ))
                .with_children(|panel| {
                    panel.spawn((
                        EquipPickerContentRoot,
                        scroll_view_bundle(&theme, 360.0, 420.0),
                    ));
                });
        });
}

/// Toggles [`EquipPickerRoot`]'s **`Visibility`** (NOT `Node::display`, spec
/// §3.3) from [`EquipPickerState::open_slot`] — this root has no
/// `Row`-direction flex siblings of its own (it's the only content under its
/// backdrop, unlike P1's tab-content pair), so `Visibility::Hidden` doesn't
/// hit the layout-summing hazard [`sync_inventory_tab_content_visibility`]'s
/// own doc comment documents; toggling it here is safe and simpler.
fn sync_equip_picker_visibility(
    picker: Res<EquipPickerState>,
    mut root: Query<&mut Visibility, With<EquipPickerRoot>>,
) {
    if !picker.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if picker.open_slot.is_some() {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Despawns every existing child of `root`, then hands the (now-empty)
/// entity's `ChildSpawner` to `spawn_children` — mirrors `diary.rs::
/// rebuild_children`'s exact shape (BL-82 EM-5.18 T58.12); duplicated rather
/// than imported since that copy is private to `diary.rs`'s own module.
fn rebuild_children(
    commands: &mut Commands,
    root: Entity,
    children_query: &Query<&Children>,
    spawn_children: impl FnOnce(&mut ChildSpawnerCommands),
) {
    if let Ok(children) = children_query.get(root) {
        for &child in children {
            commands.entity(child).despawn();
        }
    }
    commands.entity(root).with_children(spawn_children);
}

/// BL-82 EM-5.18 T58.12 (spec §3.3) — rebuilds [`EquipPickerContentRoot`]'s
/// children whenever [`EquipPickerState::open_slot`] changes: filters the
/// local player's [`NetInventory::slots`] to items whose (T58.7)
/// `equippable_slots` contains the open slot, prepending an "Unequip" row
/// when the slot is currently occupied (`NetInventory::equipped`). Empties
/// the content root (no rows) when the picker is closed or no local player
/// is mirrored yet — degrade clean, spec §3.2.
fn rebuild_equip_picker_contents(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
    picker: Res<EquipPickerState>,
    player: Query<Ref<NetInventory>, With<NetLocalPlayer>>,
    content_root: Query<Entity, With<EquipPickerContentRoot>>,
    children_query: Query<&Children>,
) {
    let inventory = player.single().ok();
    // Rebuild when the open slot changes, OR when the local player's bag
    // mutates while the picker is open: a loot pickup / trade / external swap
    // can add or remove a compatible item, or free/occupy the `free_bag_slot`
    // the Unequip row targets. Gating solely on `picker.is_changed()` left the
    // candidate list (and the cached free-slot) stale — flagged by both the
    // bevy-migration and ecs-design reviewers of this PR.
    let inventory_changed = inventory
        .as_ref()
        .map(|inv| inv.is_changed())
        .unwrap_or(false);
    if !picker.is_changed() && !(picker.open_slot.is_some() && inventory_changed) {
        return;
    }
    let Ok(root_entity) = content_root.single() else {
        return;
    };

    let Some(open_slot) = picker.open_slot else {
        rebuild_children(&mut commands, root_entity, &children_query, |_parent| {});
        return;
    };

    let Some(inventory) = inventory else {
        rebuild_children(&mut commands, root_entity, &children_query, |_parent| {});
        return;
    };

    let occupied = inventory
        .equipped
        .iter()
        .any(|equipped| equipped.slot == open_slot && equipped.item.is_some());
    let free_bag_slot = inventory
        .slots
        .iter()
        .find(|slot| slot.item.is_none())
        .map(|slot| slot.slot);
    let candidates: Vec<(InvSlotId, &NetItemStack)> = inventory
        .slots
        .iter()
        .filter_map(|net_slot| {
            let item = net_slot.item.as_ref()?;
            item.equippable_slots
                .contains(&open_slot)
                .then_some((net_slot.slot, item))
        })
        .collect();

    rebuild_children(&mut commands, root_entity, &children_query, |parent| {
        if occupied {
            spawn_unequip_row(parent, &theme, &fonts, open_slot, free_bag_slot);
        }
        for (bag_slot, item) in candidates {
            spawn_candidate_row(parent, &theme, &fonts, &images, bag_slot, item, open_slot);
        }
    });
}

/// BL-82 EM-5.18 T58.12 (spec §3.3 step 2) — the "Unequip" row: a plain
/// themed [`button_bundle`] (no item icon), tagged [`EquipPickerUnequipRow`].
/// Its click computes the first empty bag [`InvSlotId`] (`free_bag_slot`,
/// already resolved by the caller from the local player's own mirrored
/// `NetInventory` — no protocol addition needed) and sends
/// `InventoryManip::Swap(Slot::Equip(open_slot), Slot::Inventory(free_slot))`.
/// If the bag has no free slot, the row renders disabled
/// (`bevy::ui::InteractionDisabled` — a documented, non-blocking edge case,
/// spec §3.3).
fn spawn_unequip_row(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    open_slot: EquipSlot,
    free_bag_slot: Option<InvSlotId>,
) {
    let mut row = parent.spawn((
        button_bundle(theme, fonts, "Unequip"),
        EquipPickerUnequipRow,
    ));
    match free_bag_slot {
        Some(free_bag_slot) => {
            row.observe(on_unequip_row_click(open_slot, free_bag_slot));
        },
        None => {
            row.insert(bevy::ui::InteractionDisabled);
        },
    }
}

/// BL-82 EM-5.18 T58.12 — the "Unequip" row's click handler, split out so
/// [`picking_an_item_sends_swap_and_closes_picker`]'s sibling test can attach
/// the EXACT same wiring directly (mirrors [`on_equip_slot_click`]'s own
/// split for the same reason).
fn on_unequip_row_click(
    open_slot: EquipSlot,
    free_bag_slot: InvSlotId,
) -> impl Fn(On<Activate>, ResMut<EquipPickerState>, MessageWriter<InventoryActionRequest>)
+ Send
+ Sync
+ 'static {
    move |_: On<Activate>,
          mut picker: ResMut<EquipPickerState>,
          mut requests: MessageWriter<InventoryActionRequest>| {
        requests.write(InventoryActionRequest(common::comp::InventoryManip::Swap(
            Slot::Equip(open_slot),
            Slot::Inventory(free_bag_slot),
        )));
        picker.open_slot = None;
    }
}

/// BL-82 EM-5.18 T58.12 (spec §3.3 step 3) — one candidate-item row: reuses
/// [`slot_bundle_with_rarity`] (rarity background + icon/quantity via
/// [`SlotContents`], matching the bag grid's existing rendering — no new
/// visual vocabulary, per spec) + a plain name-label [`Text`] child, both
/// under one clickable row container tagged [`EquipPickerItemRow`]. Clicking
/// anywhere on the row sends `InventoryManip::Swap(Slot::Inventory(bag_slot),
/// Slot::Equip(open_slot))` and closes the picker.
fn spawn_candidate_row(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: &HudImages,
    bag_slot: InvSlotId,
    item: &NetItemStack,
    open_slot: EquipSlot,
) {
    let rarity_bg = images.get(quality_rarity_background(item.quality));
    let contents = net_item_to_slot_contents(Some(item));
    let name = item.name.clone();
    parent
        .spawn((
            EquipPickerItemRow(bag_slot),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(4.0)),
                ..Default::default()
            },
            bevy::picking::Pickable::default(),
        ))
        .with_children(|row| {
            row.spawn(slot_bundle_with_rarity(
                theme,
                EQUIP_PICKER_GROUP,
                SlotAddress::from_inv_slot_idx(bag_slot.idx()),
                40.0,
                rarity_bg,
            ))
            .insert(contents);
            row.spawn((
                Text(name),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(16.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        })
        .observe(on_candidate_row_click(bag_slot, open_slot));
}

/// BL-82 EM-5.18 T58.12 — a candidate row's click handler, split out so
/// [`picking_an_item_sends_swap_and_closes_picker`] can attach the EXACT same
/// wiring directly (mirrors [`on_equip_slot_click`]'s own split).
fn on_candidate_row_click(
    bag_slot: InvSlotId,
    open_slot: EquipSlot,
) -> impl Fn(On<Pointer<Click>>, ResMut<EquipPickerState>, MessageWriter<InventoryActionRequest>)
+ Send
+ Sync
+ 'static {
    move |_: On<Pointer<Click>>,
          mut picker: ResMut<EquipPickerState>,
          mut requests: MessageWriter<InventoryActionRequest>| {
        requests.write(InventoryActionRequest(common::comp::InventoryManip::Swap(
            Slot::Inventory(bag_slot),
            Slot::Equip(open_slot),
        )));
        picker.open_slot = None;
    }
}

/// BL-82 EM-5.18 T58.13 (spec §5) — `Escape` closes the equip-picker modal
/// while it's open. Mirrors `map_view.rs::close_full_map_on_escape`'s own
/// scoping precedent (EM-5.17 Phase 0's root-caused Escape-collision bug for
/// the map view): reads the raw `ButtonInput<KeyCode>` directly (there's no
/// rebindable "close modal" `GameInput` action) and is deliberately NOT
/// gated on `!text_input_focused` — `Escape` is never a typing-collision
/// risk (not a printable character), so gating it on chat focus would wrongly
/// block closing an already-open picker while chat happens to hold focus.
///
/// Collision check against every OTHER existing `KeyCode::Escape` consumer in
/// this crate (verified 2026-07-16, BL-82 EM-5.18): `camera.rs` releases the
/// cursor grab, `chat.rs` blurs the chat input, `map_view.rs` closes the full
/// map — EACH keys off its OWN state (`CursorGrabMode`/`InputFocus`/
/// `HudState::is_open(HudWindow::Map)` respectively), not a shared "generic
/// Escape" dispatcher, so a SINGLE physical Escape press already fires
/// several of these independently and harmlessly today. This system follows
/// the identical shape, scoped purely by `EquipPickerState::open_slot` being
/// `Some` — a fifth independent consumer, additive and collision-free by
/// construction.
fn close_equip_picker_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut picker: ResMut<EquipPickerState>,
) {
    if keys.just_pressed(KeyCode::Escape) && picker.open_slot.is_some() {
        picker.open_slot = None;
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use common::comp::inventory::{
        item::ItemDefinitionIdOwned,
        slot::{ArmorSlot, EquipSlot},
    };
    use xindeler_protocol::{NetEquippedSlot, NetInventorySlot};

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

    /// BL-82 EM-5.17/5.18 click-routing fix regression: `InventoryWindowRoot`
    /// is one of the three consumers the zlayer scheme's own doc comment
    /// names for `MODAL_WINDOWS` (diary/inventory/full-map) — pins that it
    /// now actually carries that `GlobalZIndex`, matching `diary.rs`'s
    /// `spawn_diary_window_uses_skill_tree_bg_and_modal_z_index` test for
    /// `DiaryWindowRoot`. Before this fix `InventoryWindowRoot` had NO
    /// `GlobalZIndex` at all (default z-partition 0) — even though
    /// `spawn_equip_picker_root`'s own doc comment already assumed it did
    /// ("one tier ABOVE `InventoryWindowRoot`'s own `MODAL_WINDOWS`"). Left
    /// unindexed, it sat BELOW the always-on ambient chrome once that chrome
    /// gained its own higher z-index this phase (hotbar/orbs =
    /// `ORBS_ACTION_BAR_PARTY_MINIMAP`=20) — wherever the Inventory
    /// window visually overlapped that chrome, `bevy_ui` picking (highest
    /// z-partition first) routed clicks to the chrome in front instead of
    /// the inventory panel underneath.
    #[test]
    fn inventory_window_root_carries_the_modal_windows_z_index() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });

        app.world_mut()
            .run_system_once(spawn_inventory_window)
            .expect("spawn_inventory_window runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<InventoryWindowRoot>>()
            .single(world)
            .expect("InventoryWindowRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
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
            equippable_slots: vec![EquipSlot::ActiveMainhand, EquipSlot::InactiveMainhand],
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
            equippable_slots: vec![
                EquipSlot::ActiveMainhand,
                EquipSlot::ActiveOffhand,
                EquipSlot::InactiveMainhand,
                EquipSlot::InactiveOffhand,
            ],
        }
    }

    /// A real armor stack usable as an equip-picker candidate fixture (BL-82
    /// EM-5.18 T58.14) — `equippable_slots` restricted to exactly ONE slot,
    /// so filtering tests can assert an item is excluded from every OTHER
    /// slot's candidate list.
    fn feet_armor_stack() -> NetItemStack {
        NetItemStack {
            item_id: ItemDefinitionIdOwned::Simple("common.items.testing.test_boots".to_owned()),
            name: "Testing Boots".to_owned(),
            amount: 1,
            quality: Quality::Low,
            is_two_handed: false,
            equippable_slots: vec![EquipSlot::Armor(ArmorSlot::Feet)],
        }
    }

    /// Fires a synthetic [`Pointer<Click>`] at `entity` — mirrors
    /// `diary.rs`'s own `world.trigger(Activate { entity: node })` test
    /// precedent, generalized to `bevy_picking`'s `Pointer<E>` shape (the
    /// `NormalizedRenderTarget::None` variant + `Entity::PLACEHOLDER` camera
    /// need no real window/camera/AssetServer — this is a headless,
    /// synthetic event, not a real picking-backend dispatch; see
    /// `on_equip_slot_click`'s own doc comment for why testing the OBSERVER
    /// this way, rather than re-exercising `bevy_picking`'s own hit-testing,
    /// is this crate's established convention).
    fn fire_pointer_click(world: &mut World, entity: Entity) {
        use bevy::picking::{
            backend::HitData,
            pointer::{Location, PointerButton, PointerId},
        };

        let location = Location {
            target: bevy::camera::NormalizedRenderTarget::None {
                width: 0,
                height: 0,
            },
            position: Vec2::ZERO,
        };
        let click = Click {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: std::time::Duration::ZERO,
            count: 1,
        };
        world.trigger(Pointer::new_without_propagate(
            PointerId::Mouse,
            location,
            click,
            entity,
        ));
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

    /// BL-82 EM-5.18 Phase 1 (T58.5): selecting `InventoryTab::Equipment`
    /// flips `EquipmentTabRoot`'s `Node::display` to `Flex` and
    /// `ItemsTabRoot`'s to `None` — and the reverse holds for the default
    /// (`Items`) selection. Asserted via `Node::display`, NOT `Visibility` —
    /// that distinction is the entire point of this system (see its own doc
    /// comment for why `Visibility::Hidden` alone would NOT be equivalent
    /// here).
    #[test]
    fn sync_inventory_tab_content_visibility_toggles_node_display_per_selected_tab() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(InventoryTab::default());

        let items_root = app
            .world_mut()
            .spawn((ItemsTabRoot, Node {
                display: Display::Flex,
                ..Default::default()
            }))
            .id();
        let equipment_root = app
            .world_mut()
            .spawn((EquipmentTabRoot, Node {
                display: Display::None,
                ..Default::default()
            }))
            .id();

        // `Res<InventoryTab>::is_changed()` is true on the tick the resource
        // is inserted, so this first run already exercises the default
        // (`Items`) branch.
        app.world_mut()
            .run_system_once(sync_inventory_tab_content_visibility)
            .expect("system runs");

        assert_eq!(
            app.world().get::<Node>(items_root).unwrap().display,
            Display::Flex,
            "Items is the default tab"
        );
        assert_eq!(
            app.world().get::<Node>(equipment_root).unwrap().display,
            Display::None,
            "Equipment tab content stays unmounted while Items is selected"
        );

        *app.world_mut().resource_mut::<InventoryTab>() = InventoryTab::Equipment;
        app.world_mut()
            .run_system_once(sync_inventory_tab_content_visibility)
            .expect("system runs");

        assert_eq!(
            app.world().get::<Node>(items_root).unwrap().display,
            Display::None,
            "Items tab content unmounts once Equipment is selected"
        );
        assert_eq!(
            app.world().get::<Node>(equipment_root).unwrap().display,
            Display::Flex,
            "Equipment tab content mounts once selected"
        );
    }

    /// BL-82 EM-5.18 Phase 1 (T58.5): pins `handle_slot_drops`' existing
    /// (previously untested) same-tab (`BAG_GROUP` -> `BAG_GROUP`) drag
    /// behavior — it produces a real `InventoryActionRequest`
    /// (`InventoryManip::Swap`). `handle_slot_drops`/`address_to_slot`
    /// operate purely on `SlotDropped` message payloads and never read tab
    /// state, so this coverage doesn't actually depend on the tab-split
    /// restructure — it just guards against a FUTURE change to this file
    /// silently breaking same-tab drag (spec §3.6: same-tab drag-drop is
    /// explicitly kept working, only CROSS-tab drag became impossible).
    #[test]
    fn same_tab_bag_to_bag_drag_still_produces_inventory_swap_request() {
        use bevy::ecs::message::Messages;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<SlotDropped>();
        app.add_message::<InventoryActionRequest>();

        let from_inv = InvSlotId::new(0, 0);
        let to_inv = InvSlotId::new(0, 1);
        app.world_mut().write_message(SlotDropped {
            from_group: BAG_GROUP,
            from_address: SlotAddress::from_inv_slot_idx(from_inv.idx()),
            to_group: BAG_GROUP,
            to_address: SlotAddress::from_inv_slot_idx(to_inv.idx()),
        });

        app.world_mut()
            .run_system_once(handle_slot_drops)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<InventoryActionRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![InventoryActionRequest(
            common::comp::InventoryManip::Swap(Slot::Inventory(from_inv), Slot::Inventory(to_inv),)
        )]);
    }

    /// BL-82 EM-5.18 T58.16 (P3 parity check) — the SAME regression coverage
    /// as `same_tab_bag_to_bag_drag_still_produces_inventory_swap_request`,
    /// but for the OTHER same-tab drag spec §3.6 explicitly calls out as kept
    /// working: dragging one weapon-set slot onto its counterpart within the
    /// Equipment tab (`ActiveMainhand` <-> `InactiveMainhand`) to swap
    /// loadouts by hand. Both ends are `EQUIP_GROUP` — `handle_slot_drops`/
    /// `address_to_slot` don't special-case direction, so this is the same
    /// code path, just proven with the OTHER group this redesign's tab split
    /// didn't touch either.
    #[test]
    fn same_tab_weapon_set_to_weapon_set_drag_still_produces_inventory_swap_request() {
        use bevy::ecs::message::Messages;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<SlotDropped>();
        app.add_message::<InventoryActionRequest>();

        let active_mainhand_discriminant = ALL_EQUIP_SLOTS
            .iter()
            .position(|&s| s == EquipSlot::ActiveMainhand)
            .expect("ActiveMainhand is in the canonical list")
            as u32;
        let inactive_mainhand_discriminant = ALL_EQUIP_SLOTS
            .iter()
            .position(|&s| s == EquipSlot::InactiveMainhand)
            .expect("InactiveMainhand is in the canonical list")
            as u32;

        app.world_mut().write_message(SlotDropped {
            from_group: EQUIP_GROUP,
            from_address: SlotAddress::from_equip_slot_discriminant(active_mainhand_discriminant),
            to_group: EQUIP_GROUP,
            to_address: SlotAddress::from_equip_slot_discriminant(inactive_mainhand_discriminant),
        });

        app.world_mut()
            .run_system_once(handle_slot_drops)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<InventoryActionRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![InventoryActionRequest(
            common::comp::InventoryManip::Swap(
                Slot::Equip(EquipSlot::ActiveMainhand),
                Slot::Equip(EquipSlot::InactiveMainhand),
            )
        )]);
    }

    /// A real, headless `App` carrying `HudTheme`/`HudFonts`/a real (test)
    /// `HudImages` (via `AssetPlugin` + `HudImages::load`, mirroring
    /// `social_hud.rs`'s own established "AssetPlugin::default() for a real
    /// AssetServer in a headless test" precedent — `HudImages`'s fields are
    /// private to `xindeler_ui::images`, so `load` is the only public
    /// constructor a downstream crate's test can use) — for tests exercising
    /// [`rebuild_equip_picker_contents`], which needs all three resources.
    fn new_app_with_hud_resources() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::image::Image>();
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.insert_resource(EquipPickerState::default());
        app.add_message::<InventoryActionRequest>();
        app
    }

    /// BL-82 EM-5.18 T58.14 — clicking an equip slot's real observer wiring
    /// ([`on_equip_slot_click`], the SAME closure [`spawn_equip_slot`]
    /// attaches) sets `EquipPickerState::open_slot` to THAT slot.
    #[test]
    fn equip_slot_click_opens_picker_with_correct_slot() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(EquipPickerState::default());

        let slot = EquipSlot::Armor(ArmorSlot::Feet);
        let entity = spawn_equip_slot_for_test(app.world_mut(), slot);
        app.world_mut()
            .entity_mut(entity)
            .observe(on_equip_slot_click(slot));

        fire_pointer_click(app.world_mut(), entity);

        assert_eq!(
            app.world().resource::<EquipPickerState>().open_slot,
            Some(slot)
        );
    }

    /// BL-82 EM-5.18 T58.14 — a two-handed-disabled Offhand slot's click
    /// observer is wired IDENTICALLY to every other equip slot
    /// (`spawn_equip_slot` never special-cases Offhand) — the ONLY thing
    /// that keeps a REAL click from ever opening the picker for it is
    /// `Pickable::IGNORE`, which `sync_two_handed_offhand_disable` already
    /// sets (T57.15). `bevy_picking`'s own backend (`ui_picking`,
    /// `bevy_ui::picking_backend`) excludes `Pickable::IGNORE` entities from
    /// hit-testing before any observer ever runs — that dispatch guarantee
    /// is `bevy_picking`'s own tested contract, not re-proven here (a raw
    /// `World::trigger` bypasses it entirely, so firing one here would prove
    /// the WRONG thing — see `fire_pointer_click`'s own doc comment). This
    /// test instead pins the STATE that guarantee depends on, matching this
    /// crate's own `install_observers_registers_without_panicking`
    /// precedent (`xindeler-ui::slot`) for "prove the wiring is
    /// structurally correct, not the picking backend's own internals".
    #[test]
    fn two_handed_offhand_slot_click_never_opens_picker() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let active_offhand = spawn_equip_slot_for_test(app.world_mut(), EquipSlot::ActiveOffhand);
        app.world_mut()
            .entity_mut(active_offhand)
            .observe(on_equip_slot_click(EquipSlot::ActiveOffhand));

        app.world_mut().spawn((NetLocalPlayer, NetInventory {
            slots: Vec::new(),
            equipped: vec![NetEquippedSlot {
                slot: EquipSlot::ActiveMainhand,
                item: Some(two_handed_weapon_stack()),
            }],
            capacity: 0,
        }));

        app.world_mut()
            .run_system_once(sync_two_handed_offhand_disable)
            .expect("system runs");

        assert_eq!(
            *app.world()
                .get::<bevy::picking::Pickable>(active_offhand)
                .unwrap(),
            bevy::picking::Pickable::IGNORE,
            "sync_two_handed_offhand_disable must mark this slot Pickable::IGNORE — the sole \
             guard preventing its (identically-wired) click observer from ever opening the picker \
             in real play"
        );
    }

    /// BL-82 EM-5.18 T58.14 — [`rebuild_equip_picker_contents`] renders only
    /// the item whose `equippable_slots` contains the currently-open slot;
    /// an item compatible with a DIFFERENT slot is excluded entirely.
    #[test]
    fn picker_filters_items_to_compatible_equip_slots_only() {
        let mut app = new_app_with_hud_resources();

        let feet_slot = InvSlotId::new(0, 0);
        let weapon_slot = InvSlotId::new(0, 1);
        app.world_mut().spawn((NetLocalPlayer, NetInventory {
            slots: vec![
                NetInventorySlot {
                    slot: feet_slot,
                    item: Some(feet_armor_stack()),
                },
                NetInventorySlot {
                    slot: weapon_slot,
                    item: Some(two_handed_weapon_stack()),
                },
            ],
            equipped: Vec::new(),
            capacity: 2,
        }));
        let content_root = app.world_mut().spawn(EquipPickerContentRoot).id();

        *app.world_mut().resource_mut::<EquipPickerState>() = EquipPickerState {
            open_slot: Some(EquipSlot::Armor(ArmorSlot::Feet)),
        };
        app.world_mut()
            .run_system_once(rebuild_equip_picker_contents)
            .expect("system runs");

        let rendered: Vec<InvSlotId> = app
            .world()
            .get::<Children>(content_root)
            .expect("content root has rendered rows")
            .iter()
            .filter_map(|child| {
                app.world()
                    .get::<EquipPickerItemRow>(child)
                    .map(|row| row.0)
            })
            .collect();
        assert_eq!(
            rendered,
            vec![feet_slot],
            "only the Feet-compatible item must render — the two-handed weapon's equippable_slots \
             never contains Armor(Feet)"
        );
    }

    /// BL-82 EM-5.18 T58.14 — the "Unequip" row appears ONLY when the open
    /// slot is currently occupied (`NetInventory::equipped` has a real
    /// item), and is absent when it's empty.
    #[test]
    fn unequip_row_appears_only_when_slot_occupied() {
        let open_slot = EquipSlot::ActiveMainhand;

        let has_unequip_row = |occupied: bool| -> bool {
            let mut app = new_app_with_hud_resources();
            app.world_mut().spawn((NetLocalPlayer, NetInventory {
                slots: Vec::new(),
                equipped: if occupied {
                    vec![NetEquippedSlot {
                        slot: open_slot,
                        item: Some(one_handed_weapon_stack()),
                    }]
                } else {
                    Vec::new()
                },
                capacity: 0,
            }));
            let content_root = app.world_mut().spawn(EquipPickerContentRoot).id();

            *app.world_mut().resource_mut::<EquipPickerState>() = EquipPickerState {
                open_slot: Some(open_slot),
            };
            app.world_mut()
                .run_system_once(rebuild_equip_picker_contents)
                .expect("system runs");

            app.world()
                .get::<Children>(content_root)
                .is_some_and(|children| {
                    children
                        .iter()
                        .any(|child| app.world().get::<EquipPickerUnequipRow>(child).is_some())
                })
        };

        assert!(
            has_unequip_row(true),
            "an occupied slot must render the Unequip row"
        );
        assert!(
            !has_unequip_row(false),
            "an empty slot must NOT render the Unequip row"
        );
    }

    /// BL-82 EM-5.18 T58.14 — clicking a candidate row's real observer
    /// wiring ([`on_candidate_row_click`], the SAME closure
    /// [`spawn_candidate_row`] attaches) sends the real `InventoryManip::
    /// Swap(Slot::Inventory(bag_slot), Slot::Equip(open_slot))` and closes
    /// the picker (`open_slot` back to `None`).
    #[test]
    fn picking_an_item_sends_swap_and_closes_picker() {
        use bevy::ecs::message::Messages;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<InventoryActionRequest>();
        let open_slot = EquipSlot::Armor(ArmorSlot::Feet);
        app.insert_resource(EquipPickerState {
            open_slot: Some(open_slot),
        });

        let bag_slot = InvSlotId::new(0, 3);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(entity)
            .observe(on_candidate_row_click(bag_slot, open_slot));

        fire_pointer_click(app.world_mut(), entity);

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<InventoryActionRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![InventoryActionRequest(
            common::comp::InventoryManip::Swap(Slot::Inventory(bag_slot), Slot::Equip(open_slot))
        )]);
        assert_eq!(
            app.world().resource::<EquipPickerState>().open_slot,
            None,
            "picking an item must close the picker"
        );
    }
}
