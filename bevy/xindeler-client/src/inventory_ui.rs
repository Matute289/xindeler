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

use bevy::prelude::*;
use common::comp::inventory::slot::{InvSlotId, Slot};
use xindeler_protocol::{
    InventoryActionRequest, NetInventory, NetLocalPlayer, inventory::ALL_EQUIP_SLOTS,
};
use xindeler_ui::{
    hud_state::{HudAction, HudState, HudWindow},
    panel::panel_bundle,
    slot::{SlotAddress, SlotContents, SlotDropped, SlotGroup, slot_bundle},
    theme::HudTheme,
};

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

/// Marks the whole inventory window root (toggled by [`HudState`]).
#[derive(Component)]
struct InventoryWindowRoot;
/// Marks the bag grid container (children are the bag [`xindeler_ui::slot`]
/// entities, spawned once real capacity is known).
#[derive(Component)]
struct BagGridRoot;
/// Marks the paper-doll container (children are the 22 equip slots, spawned
/// at `Startup` — the slot COUNT is fixed/known ahead of time, unlike the
/// bag).
#[derive(Component)]
struct PaperdollRoot;

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
                    toggle_inventory_window,
                    sync_inventory_window_visibility,
                    spawn_bag_grid_once_capacity_known,
                    sync_slot_contents,
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
                panel.spawn((PaperdollRoot, Node {
                    display: Display::Grid,
                    grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(2, 48.0)],
                    row_gap: Val::Px(4.0),
                    column_gap: Val::Px(4.0),
                    ..Default::default()
                }));
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
    mut spawned: ResMut<BagGridSpawned>,
    player: Query<&NetInventory, With<NetLocalPlayer>>,
    bag_root: Query<Entity, With<BagGridRoot>>,
    paperdoll_root: Query<Entity, With<PaperdollRoot>>,
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
            parent.spawn(slot_bundle(
                &theme,
                BAG_GROUP,
                SlotAddress::from_inv_slot_idx(net_slot.slot.idx()),
                48.0,
            ));
        }
    });

    if let Ok(paperdoll_root_entity) = paperdoll_root.single() {
        commands
            .entity(paperdoll_root_entity)
            .with_children(|parent| {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
                )]
                for (idx, _slot) in ALL_EQUIP_SLOTS.iter().enumerate() {
                    parent.spawn(slot_bundle(
                        &theme,
                        EQUIP_GROUP,
                        SlotAddress::from_equip_slot_discriminant(idx as u32),
                        48.0,
                    ));
                }
            });
    }

    spawned.0 = true;
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

/// Toggles [`HudWindow::Inventory`] on the `I` key — the generic
/// `HudAction::ToggleWindow` flow EM-5.1's state machine already provides.
fn toggle_inventory_window(keys: Res<ButtonInput<KeyCode>>, mut actions: MessageWriter<HudAction>) {
    if keys.just_pressed(KeyCode::KeyI) {
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

/// Reconciles every bag/equip slot's [`SlotContents`] against the local
/// player's current `NetInventory` — `Changed<NetInventory>`-gated.
/// Degrades clean (no panic) if the bag grid hasn't been spawned yet (spec
/// §3.2).
fn sync_slot_contents(
    player: Query<&NetInventory, (With<NetLocalPlayer>, Changed<NetInventory>)>,
    mut bag_slots: Query<(&SlotAddress, &mut SlotContents), With<xindeler_ui::slot::HudSlot>>,
) {
    let Ok(inventory) = player.single() else {
        return;
    };

    for net_slot in &inventory.slots {
        let address = SlotAddress::from_inv_slot_idx(net_slot.slot.idx());
        if let Some((_, mut contents)) = bag_slots.iter_mut().find(|(a, _)| **a == address) {
            *contents = net_item_to_slot_contents(net_slot.item.as_ref());
        }
    }
    for (idx, equipped) in inventory.equipped.iter().enumerate() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
        )]
        let address = SlotAddress::from_equip_slot_discriminant(idx as u32);
        if let Some((_, mut contents)) = bag_slots.iter_mut().find(|(a, _)| **a == address) {
            *contents = net_item_to_slot_contents(equipped.item.as_ref());
        }
    }
}

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
    use common::comp::inventory::slot::{ArmorSlot, EquipSlot};

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
}
