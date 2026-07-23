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
//! ## Legacy single-window layout (BL-82 EM-5.17/5.18 legacy-inventory rebuild)
//! The bag grid ([`BagGridRoot`]) and the paper-doll ([`PaperdollRoot`]) used
//! to render as two mutually-exclusive TABS (`InventoryTab::Items`/
//! `InventoryTab::Equipment`, a Diablo4-style split) — Matías flagged that
//! this "looks nothing like ours". This screen now mirrors the legacy
//! "xindeler-old" boxy pixel-art panel instead: ONE combined window (a
//! title, a left stat-icon column, a fixed-size paper-doll silhouette, a
//! rarity-colored bag grid, and a footer), built from the legacy `bag/`
//! asset set (see [`xindeler_ui::images::HudImageKey`]'s `BAG_*_DIR`/
//! `GENERIC_BUTTONS_DIR` variants) rather than the reserved high-res
//! `hud_d4/` art. Because the paper-doll and the bag grid are mounted
//! SIMULTANEOUSLY again (no `Node::display` toggle hides either), drag-to-
//! equip (a bag slot dragged directly onto an equip slot, or vice versa)
//! works exactly as it did before any tab split ever existed — there is no
//! structural drag-drop gap in this layout.
//!
//! ## Click-to-equip picker modal (BL-82 EM-5.18 Phase 2)
//! A convenience alternative to drag-drop, unaffected by the tab removal
//! above: clicking an equip slot ([`spawn_equip_slot`]'s
//! `.observe(On<Pointer<Click>>, ..)`, T58.10) opens [`EquipPickerRoot`] — a
//! SIBLING of [`InventoryWindowRoot`] (not nested inside its panel), listing
//! every bag item whose (server-computed, T58.7) `NetItemStack::
//! equippable_slots` contains the clicked slot, plus an "Unequip" row when
//! the slot is already occupied ([`rebuild_equip_picker_contents`], T58.12).
//! Picking a row (or Unequip) sends the SAME `InventoryActionRequest(
//! InventoryManip::Swap(..))` drag-drop already sends (spec §3.3) and closes
//! the picker. [`EquipPickerState`] (a plain resource, outside `HudState` —
//! no `HudWindow` variant fits a transient sub-modal of an already-open
//! window) tracks which slot (if any) is open;
//! [`sync_equip_picker_visibility`] toggles the root's `Visibility` (this
//! root has no `Row`-direction flex siblings of its own, so `Visibility` is
//! safe here — see that system's own doc comment). `Escape` closes the
//! picker ([`close_equip_picker_on_escape`], T58.13) — see that system's own
//! doc comment for why this is collision-free with `camera.rs`/`chat.rs`/
//! `map_view.rs`'s own independent `Escape` consumers.

use bevy::{
    ecs::{change_detection::NonSend, schedule::common_conditions::not},
    picking::events::{Click, Pointer},
    prelude::*,
};
use common::comp::inventory::{
    item::Quality,
    slot::{ArmorSlot, EquipSlot, InvSlotId, Slot},
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    InventoryActionRequest, NetEnergy, NetHealth, NetInventory, NetItemStack, NetLocalPlayer,
    NetPoise, inventory::ALL_EQUIP_SLOTS,
};
use xindeler_ui::{
    button::{Activate, HudButtonImages, button_bundle, image_button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    i18n::{CurrentLocale, Localization, LocalizedLabel, LocalizedText},
    images::{HudImageKey, HudImages},
    panel::image_panel_bundle,
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

/// BL-82 EM-5.17/5.18 legacy-inventory rebuild — the same 18-of-22-slot
/// Equipment layout (spec §3.7, `Bag1`-`Bag4` still excluded — this screen's
/// bag GRID already covers physical `InvSlotId` addressing, unrelated to
/// these loadout-provided `EquipSlot` bag slots), now positioned as an
/// ABSOLUTE cross/silhouette inside the fixed-size [`PaperdollRoot`] instead
/// of three flex columns — approximating legacy "xindeler-old"'s paper-doll
/// art (tunable later during smoke: these `(left_px, top_px, size_px)`
/// triples are a first pass, not pixel-perfect against the reference art).
/// [`spawn_bag_grid_once_capacity_known`] walks this table directly (no more
/// per-column `*_INDICES` split); [`spawn_equip_slot`] resolves each
/// `EquipSlot`'s [`ALL_EQUIP_SLOTS`] discriminant itself.
const PAPERDOLL_SLOT_LAYOUT: &[(EquipSlot, f32, f32, f32)] = &[
    (EquipSlot::Armor(ArmorSlot::Head), 108.0, 4.0, 44.0),
    (EquipSlot::Armor(ArmorSlot::Neck), 108.0, 52.0, 40.0),
    (EquipSlot::Armor(ArmorSlot::Shoulders), 44.0, 96.0, 44.0),
    (EquipSlot::Armor(ArmorSlot::Chest), 100.0, 96.0, 52.0),
    (EquipSlot::Armor(ArmorSlot::Hands), 162.0, 96.0, 44.0),
    (EquipSlot::Armor(ArmorSlot::Belt), 108.0, 150.0, 40.0),
    (EquipSlot::Armor(ArmorSlot::Legs), 104.0, 196.0, 46.0),
    (EquipSlot::Armor(ArmorSlot::Ring2), 48.0, 148.0, 34.0),
    (EquipSlot::Armor(ArmorSlot::Ring1), 170.0, 148.0, 34.0),
    (EquipSlot::Armor(ArmorSlot::Back), 44.0, 190.0, 44.0),
    (EquipSlot::Armor(ArmorSlot::Feet), 166.0, 190.0, 44.0),
    (EquipSlot::Lantern, 210.0, 8.0, 36.0),
    (EquipSlot::Glider, 210.0, 50.0, 36.0),
    (EquipSlot::Armor(ArmorSlot::Tabard), 210.0, 92.0, 36.0),
    (EquipSlot::ActiveMainhand, 18.0, 250.0, 54.0),
    (EquipSlot::ActiveOffhand, 178.0, 250.0, 54.0),
    (EquipSlot::InactiveMainhand, 78.0, 262.0, 40.0),
    (EquipSlot::InactiveOffhand, 140.0, 262.0, 40.0),
];

/// Marks the whole inventory window root (toggled by [`HudState`]).
#[derive(Component)]
struct InventoryWindowRoot;
/// BL-82 EM-5.18 legacy-inventory round 2 — the title-bar red-X close button
/// (top-right), reusing [`image_button_bundle`] so it inherits the SAME
/// hover/press texture-swap
/// ([`xindeler_ui::button::update_image_button_visuals`]) every image-backed
/// HUD button gets. Its [`Activate`] observer writes [`HudAction::CloseWindow`]
/// — the SAME generic action flow the `I` keybind and `HudState::close` already
/// use, so clicking it closes the window through the one real state machine, no
/// bespoke close path.
#[derive(Component)]
struct InventoryCloseButton;
/// Marks the bag grid container (children are the bag [`xindeler_ui::slot`]
/// entities, spawned once real capacity is known).
#[derive(Component)]
struct BagGridRoot;
/// Marks the left stat-icon column container (BL-82 EM-5.17/5.18 legacy-
/// inventory rebuild) — 6 icon+value rows, one per [`StatKind`].
#[derive(Component)]
struct StatColumn;

/// Marks the fixed-size (250x330px) paper-doll silhouette container — every
/// [`PAPERDOLL_SLOT_LAYOUT`] slot is spawned directly into this entity with
/// an absolute position (BL-82 EM-5.17/5.18 legacy-inventory rebuild;
/// previously a `Row` of 3 flex columns, now a single `Relative`-positioned
/// canvas). Spawned at `Startup` since the slot COUNT (18) is fixed/known
/// ahead of time, unlike the bag.
#[derive(Component)]
struct PaperdollRoot;

/// One of the 6 left-column stat readouts (BL-82 EM-5.17/5.18 legacy-
/// inventory rebuild) — see [`StatKind`] and [`sync_inventory_stats`].
#[derive(Component, Clone, Copy)]
struct StatValueText(StatKind);

/// Which stat a [`StatValueText`] displays — mirrors legacy "xindeler-old"'s
/// stat-icon column (spec: health/energy/protection/stun-resist/combat-
/// rating/stealth). Only Health/Energy/StunRes are backed by a REAL mirrored
/// value today ([`sync_inventory_stats`]); the other three are documented
/// "0" placeholders pending a protocol mirror field (see that system's own
/// doc comment).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum StatKind {
    Health,
    Energy,
    Protection,
    CombatRating,
    StunRes,
    Stealth,
}

/// Marks the footer's coin-count [`Text`] (BL-82 EM-5.17/5.18 legacy-
/// inventory rebuild). Coin/currency isn't mirrored to the client yet — this
/// stays a documented "0" placeholder (see [`spawn_inventory_window`]'s own
/// doc comment).
#[derive(Component)]
struct CoinText;

/// Marks the footer's `occupied/total` bag-slot-count [`Text`] — kept live by
/// [`sync_slot_count`].
#[derive(Component)]
struct SlotCountText;

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
            .init_resource::<EquipPickerState>()
            .add_systems(
                Startup,
                (
                    // BL-82 EM-5.17/5.18 legacy-inventory rebuild — this
                    // spawn now reads `HudImages` directly (stat icons, the
                    // gold-coin readout), unlike the earlier tab-split
                    // version, so it also needs `.after(images::
                    // init_images)`, not just `.after(theme::init_theme)`.
                    spawn_inventory_window
                        .after(xindeler_ui::theme::init_theme)
                        .after(xindeler_ui::images::init_images),
                    // BL-82 EM-5.18 T58.11 — a top-level SIBLING of
                    // `InventoryWindowRoot`, not nested inside it.
                    spawn_equip_picker_root.after(xindeler_ui::theme::init_theme),
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
                    // BL-82 EM-5.18 Phase 2 — the equip-picker modal.
                    sync_equip_picker_visibility,
                    // BL-82 EM-5.16 (T56.44 follow-up): now also gates its
                    // rebuild on a locale change (see the function's own
                    // updated doc comment) — needs the same `.after(
                    // LocaleSyncSet)` edge `settings_window.rs`'s `refresh_
                    // setting_labels` documents as load-bearing.
                    rebuild_equip_picker_contents.after(xindeler_ui::i18n::LocaleSyncSet),
                    close_equip_picker_on_escape,
                    // BL-82 EM-5.17/5.18 legacy-inventory rebuild — the
                    // left stat column + footer readouts.
                    sync_inventory_stats,
                    sync_slot_count,
                ),
            );
    }
}

/// The 6 stat rows the left column shows, in render order, paired with the
/// icon this v1 layout gives each one — see [`StatKind`]'s own doc comment
/// for which of these are backed by a real mirrored value today.
const STAT_ROWS: [(StatKind, HudImageKey); 6] = [
    (StatKind::Health, HudImageKey::StatHealth),
    (StatKind::Energy, HudImageKey::StatEnergy),
    (StatKind::Protection, HudImageKey::StatProtection),
    (StatKind::CombatRating, HudImageKey::StatCombatRating),
    (StatKind::StunRes, HudImageKey::StatStunRes),
    (StatKind::Stealth, HudImageKey::StatStealth),
];

/// BL-82 EM-5.18 legacy-inventory round 2 — the window is sized to the NATIVE
/// pixel dimensions of its chrome plate ([`HudImageKey::InventoryChrome`] =
/// `bag/inv_bg_0.png`, 424x708) so the plate's painted golden border, corner
/// filigree and its two carved region dividers render at their true
/// proportions (the `ImageNode` is `NodeImageMode::Stretch`, so a node of the
/// SAME aspect stretches 1:1 → no distortion — the concern the old
/// "intentionally NOT used" note raised only bites a MISMATCHED box). The four
/// content bands below get FIXED heights measured from the plate's horizontal
/// divider rows (a flood-scan of the asset found rules at y=46 [title], y=468
/// [bag] and y=688 [footer]); they sum EXACTLY to [`INV_WINDOW_H`] so every
/// band aligns to its painted compartment.
///
/// TODO(BL-82 hud-scale): these are FIXED logical pixels — the Bevy HUD has no
/// hud-scale/DPI system yet (consistent with every sibling window today), so
/// the 708px-tall plate leaves only ~6px margin on a 720p client and will clip
/// on a shorter window or under OS/UI downscaling. When a HUD-scale resource
/// lands, multiply `INV_WINDOW_*`/`INV_*_BAND_H` by it uniformly (all bands
/// scale together, preserving the divider alignment).
const INV_WINDOW_W: f32 = 424.0;
const INV_WINDOW_H: f32 = 708.0;
/// Title-bar band: portrait + centred title + close button (plate y 0..46).
const INV_TITLE_BAND_H: f32 = 46.0;
/// Main band: stat column + paper-doll (plate y 46..468).
const INV_MAIN_BAND_H: f32 = 422.0;
/// Bag-grid band (plate y 468..688).
const INV_BAG_BAND_H: f32 = 220.0;
/// Footer band: coin readout + slot count (plate y 688..708).
const INV_FOOTER_BAND_H: f32 = 20.0;
/// Horizontal inset so each band's content clears the plate's painted border.
const INV_SIDE_INSET: f32 = 16.0;

/// Spawns the (initially hidden) inventory window: a full-screen dim backdrop
/// containing ONE panel (BL-82 EM-5.17/5.18 legacy-inventory rebuild — see the
/// module doc comment's "Legacy single-window layout" section for why this
/// replaced the earlier Items/Equipment tab split). Round 2 (EM-5.18) backs
/// the panel with the ornate legacy chrome plate
/// ([`HudImageKey::InventoryChrome`] = `inv_bg_0.png`) at its native 424x708
/// size and lays the content into four FIXED-height bands that butt against
/// the plate's painted divider rows (`INV_*_BAND_H`), top-to-bottom as:
/// 1. A full-width title BAR (BL-82 EM-5.18 legacy-inventory round 2, matching
///    legacy "xindeler-old"'s `bag.rs`): the flat 2D character portrait
///    ([`HudImageKey::CharacterPortrait`], `char_art`) pinned top-left, the
///    centred title ("Inventario" — the player's real character name isn't
///    mirrored to the client today, so this can't yet render "Inventario de
///    <name>"; `// TODO` below flags the missing protocol mirror field), and
///    the red-X close button ([`spawn_close_button`]) pinned top-right.
/// 2. A `Row` of the left [`StatColumn`] (6 icon+value rows, kept live by
///    [`sync_inventory_stats`]) and the fixed-size (250x330px) center
///    [`PaperdollRoot`] (all 18 shown equip slots, absolutely positioned per
///    [`PAPERDOLL_SLOT_LAYOUT`] — spawned empty here, filled by
///    [`spawn_bag_grid_once_capacity_known`] once real inventory data exists,
///    same latch the bag grid already used).
/// 3. The rarity-colored [`BagGridRoot`] (a 9-column `Display::Grid`, 40px
///    slots — spawned empty, same latch).
/// 4. A footer `Row`: a gold-coin readout (left, currency isn't mirrored either
///    — `// TODO`) and the `occupied/total` [`SlotCountText`] (right, kept live
///    by [`sync_slot_count`]).
///
/// BL-82 EM-5.17/5.18 click-routing fix (kept from the prior tab-split
/// version, unrelated to this restructure): `InventoryWindowRoot` is one of
/// the three consumers the zlayer scheme's own doc comment names for
/// `MODAL_WINDOWS` (diary/inventory/full-map) — without this z-index, the
/// window would sit BELOW the always-on ambient chrome (hotbar/orbs =
/// `ORBS_ACTION_BAR_PARTY_MINIMAP`=20), and `bevy_ui` picking (which
/// resolves the highest z-partition first) would route clicks to the chrome
/// in front instead of the inventory panel underneath.
fn spawn_inventory_window(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
    localization: NonSend<Localization>,
) {
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
            // Round 2 (BL-82 EM-5.18): the window is now the ornate legacy
            // chrome plate ([`HudImageKey::InventoryChrome`] = `inv_bg_0.png`)
            // via `image_panel_bundle`, sized to the plate's native 424x708 so
            // its golden border/filigree/dividers render undistorted (see the
            // `INV_WINDOW_*`/`INV_*_BAND_H` constants). `image_panel_bundle`
            // already carries a real `Node` (its own padding + the `ImageNode`)
            // — a SECOND `Node` in the same spawn tuple would REPLACE it
            // wholesale (the EM-5.2 regression class; see
            // `xindeler-client::combat_hud`'s own doc comment), so the
            // fixed-size + column-layout overrides are applied via
            // `.entry::<Node>().and_modify(..)` (in-place field mutation) — on
            // its own statement, since `EntityEntryCommands` doesn't itself
            // expose `with_children`. Padding is zeroed here: the plate paints
            // its own border and each band insets its OWN content
            // (`INV_SIDE_INSET`), and `row_gap` is zeroed because the bands are
            // fixed-height and butt directly against the plate's painted
            // divider rows.
            let mut panel_entity = backdrop.spawn(image_panel_bundle(
                &theme,
                images.get(HudImageKey::InventoryChrome),
            ));
            panel_entity.entry::<Node>().and_modify(move |mut node| {
                node.flex_direction = FlexDirection::Column;
                node.width = Val::Px(INV_WINDOW_W);
                node.height = Val::Px(INV_WINDOW_H);
                node.padding = UiRect::all(Val::Px(0.0));
                node.row_gap = Val::Px(0.0);
            });
            panel_entity.with_children(|panel| {
                // 1. Title bar — a full-width row matching legacy
                // "xindeler-old"'s `bag.rs`: the character portrait pinned
                // top-left (`char_art`), the centred title, and the red-X
                // close button pinned top-right (`close_btn`). The title node's
                // `flex_grow: 1` makes it consume all free space between the
                // portrait and close button, and its `Justify::Center` centres
                // the text within THAT gap. Because the portrait (30px) and
                // close (24px) differ slightly in width, the title's optical
                // centre sits a few px off true window-centre — negligible at
                // this scale, and matching the reference's own eyeballed
                // placement. (`SpaceBetween` is belt-and-braces here: the
                // title's `flex_grow` already eats all slack, so the portrait
                // and close land at the two insets regardless.)
                panel
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(INV_TITLE_BAND_H),
                        flex_shrink: 0.0,
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        padding: UiRect::horizontal(Val::Px(INV_SIDE_INSET)),
                        ..Default::default()
                    })
                    .with_children(|title_row| {
                        // Top-left: character portrait (flat 2D pixel-art
                        // bust — see `HudImageKey::CharacterPortrait`).
                        title_row.spawn((
                            bevy::ui::widget::ImageNode::new(
                                images.get(HudImageKey::CharacterPortrait),
                            ),
                            Node {
                                width: Val::Px(30.0),
                                height: Val::Px(28.0),
                                flex_shrink: 0.0,
                                ..Default::default()
                            },
                        ));
                        // Centre: title (centred within the remaining space).
                        title_row.spawn((
                            // BL-82 EM-5.16 (T56.44 follow-up): this used to
                            // hardcode the Spanish word "Inventario" directly
                            // in the English-titled code path (a real bug,
                            // not a stylistic choice) — now a real, reactively
                            // localized key. `hud-bag-inventory` already
                            // exists but interpolates `{ $playername }`
                            // (`Localization::tr` has no Fluent-argument
                            // support), and the player's real character name
                            // isn't mirrored to the client yet anyway (the
                            // gap the removed TODO flagged) — so this is its
                            // own new, non-interpolated static key until that
                            // mirror field exists and the title can grow a
                            // genuinely dynamic name suffix.
                            LocalizedText("hud-bag-title"),
                            Text(localization.tr("hud-bag-title")),
                            TextFont {
                                font: bevy::text::FontSource::Handle(fonts.title.clone()),
                                font_size: bevy::text::FontSize::Px(22.0),
                                ..Default::default()
                            },
                            TextColor(theme.palette.text),
                            bevy::text::TextLayout {
                                justify: bevy::text::Justify::Center,
                                ..Default::default()
                            },
                            Node {
                                flex_grow: 1.0,
                                ..Default::default()
                            },
                        ));
                        // Top-right: red-X close button.
                        spawn_close_button(title_row, &theme, &fonts, &images);
                    });

                // 2. Main band — stat column (far left) + paper-doll. Fixed to
                // the plate's middle compartment (`INV_MAIN_BAND_H`);
                // `SpaceBetween` pins the stat column to the left inset and
                // pushes the paper-doll toward the plate's centre columns,
                // matching the reference `captura10.png` composition.
                panel
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(INV_MAIN_BAND_H),
                        flex_shrink: 0.0,
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::FlexStart,
                        column_gap: Val::Px(12.0),
                        padding: UiRect {
                            left: Val::Px(INV_SIDE_INSET),
                            right: Val::Px(INV_SIDE_INSET),
                            top: Val::Px(10.0),
                            bottom: Val::Px(0.0),
                        },
                        ..Default::default()
                    })
                    .with_children(|main_row| {
                        main_row
                            .spawn((StatColumn, Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(6.0),
                                min_width: Val::Px(92.0),
                                ..Default::default()
                            }))
                            .with_children(|stat_column| {
                                for (kind, icon) in STAT_ROWS {
                                    stat_column
                                        .spawn(Node {
                                            flex_direction: FlexDirection::Row,
                                            column_gap: Val::Px(6.0),
                                            align_items: AlignItems::Center,
                                            ..Default::default()
                                        })
                                        .with_children(|stat_row| {
                                            stat_row.spawn((
                                                bevy::ui::widget::ImageNode::new(images.get(icon)),
                                                Node {
                                                    width: Val::Px(20.0),
                                                    height: Val::Px(20.0),
                                                    ..Default::default()
                                                },
                                            ));
                                            stat_row.spawn((
                                                StatValueText(kind),
                                                Text("0".to_owned()),
                                                TextFont {
                                                    font: bevy::text::FontSource::Handle(
                                                        fonts.body.clone(),
                                                    ),
                                                    font_size: bevy::text::FontSize::Px(14.0),
                                                    ..Default::default()
                                                },
                                                TextColor(theme.palette.text),
                                            ));
                                        });
                                }
                            });

                        main_row.spawn((PaperdollRoot, Node {
                            width: Val::Px(250.0),
                            height: Val::Px(330.0),
                            position_type: PositionType::Relative,
                            ..Default::default()
                        }));
                    });

                // 3. Bag-grid band — the grid centred inside the plate's
                // lower compartment (`INV_BAG_BAND_H`, below the carved
                // divider at plate y=468).
                panel
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(INV_BAG_BAND_H),
                        flex_shrink: 0.0,
                        flex_direction: FlexDirection::Column,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        padding: UiRect::horizontal(Val::Px(INV_SIDE_INSET)),
                        ..Default::default()
                    })
                    .with_children(|bag_band| {
                        bag_band.spawn((BagGridRoot, Node {
                            display: Display::Grid,
                            grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(9, 40.0)],
                            row_gap: Val::Px(2.0),
                            column_gap: Val::Px(2.0),
                            ..Default::default()
                        }));
                    });

                // 4. Footer band — coin readout (left) + slot count (right) in
                // the plate's thin bottom strip (`INV_FOOTER_BAND_H`, below the
                // divider at plate y=688).
                panel
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(INV_FOOTER_BAND_H),
                        flex_shrink: 0.0,
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        padding: UiRect::horizontal(Val::Px(INV_SIDE_INSET)),
                        ..Default::default()
                    })
                    .with_children(|footer| {
                        footer
                            .spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(4.0),
                                align_items: AlignItems::Center,
                                ..Default::default()
                            })
                            .with_children(|coin_row| {
                                coin_row.spawn((
                                    bevy::ui::widget::ImageNode::new(
                                        images.get(HudImageKey::GoldCoin),
                                    ),
                                    Node {
                                        width: Val::Px(16.0),
                                        height: Val::Px(16.0),
                                        ..Default::default()
                                    },
                                ));
                                coin_row.spawn((
                                    CoinText,
                                    // TODO(BL-82 follow-up): currency isn't
                                    // mirrored to the client yet — this stays
                                    // a documented "0" placeholder.
                                    Text("0".to_owned()),
                                    TextFont {
                                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                                        font_size: bevy::text::FontSize::Px(14.0),
                                        ..Default::default()
                                    },
                                    TextColor(theme.palette.text),
                                ));
                            });
                        footer.spawn((
                            SlotCountText,
                            Text("0/0".to_owned()),
                            TextFont {
                                font: bevy::text::FontSource::Handle(fonts.body.clone()),
                                font_size: bevy::text::FontSize::Px(14.0),
                                ..Default::default()
                            },
                            TextColor(theme.palette.text_muted),
                        ));
                    });
            });
        });
}

/// BL-82 EM-5.18 legacy-inventory round 2 — spawns the title-bar red-X close
/// button as an [`image_button_bundle`] (so it inherits the shared
/// hover/press texture-swap
/// [`xindeler_ui::button::update_image_button_visuals`] every image-backed HUD
/// button already gets), sized to the `close_btn.png` art with an empty
/// (icon-only) label and zero padding. `Activate` fires
/// [`on_close_button_click`]. `image_button_bundle` already carries a real
/// `Node` — a SECOND `Node` in the same spawn tuple would REPLACE it wholesale
/// (the EM-5.2 regression class; see [`spawn_inventory_window`]'s own doc
/// comment), so the size/padding override is applied via
/// `.entry::<Node>().and_modify(..)`, the same pattern [`spawn_equip_slot`]
/// uses.
fn spawn_close_button(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    images: &HudImages,
) {
    let mut button = parent.spawn((
        InventoryCloseButton,
        image_button_bundle(theme, fonts, "", HudButtonImages {
            normal: images.get(HudImageKey::CloseBtn),
            hover: images.get(HudImageKey::CloseBtnHover),
            pressed: images.get(HudImageKey::CloseBtnPress),
        }),
    ));
    button.entry::<Node>().and_modify(move |mut node| {
        node.width = Val::Px(24.0);
        node.height = Val::Px(25.0);
        node.flex_shrink = 0.0;
        node.padding = UiRect::all(Val::Px(0.0));
    });
    button.observe(on_close_button_click());
}

/// Spawns a themed button whose label is a resolved `.ftl` message value,
/// tagged [`LocalizedLabel`] so it re-resolves live on a locale change (the
/// same small helper `settings_window.rs`/`esc_menu.rs`/`trade_ui.rs` all
/// already use).
fn labeled_button<'a>(
    parent: &'a mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    localization: &Localization,
    key: &'static str,
) -> EntityCommands<'a> {
    let mut button = parent.spawn(button_bundle(theme, fonts, &localization.tr(key)));
    button.insert(LocalizedLabel(key));
    button
}

/// BL-82 EM-5.18 legacy-inventory round 2 — the close button's [`Activate`]
/// handler, split out (like [`on_equip_slot_click`]) so a sibling test can
/// attach the EXACT same wiring to a bare entity. Writes
/// [`HudAction::CloseWindow`] — the SAME generic action the ESC/close control
/// already routes through [`xindeler_ui::hud_state::apply_hud_actions`]
/// (`HudState::close`), so the button closes the window through the one real
/// state machine rather than mutating `HudState` behind its back.
fn on_close_button_click() -> impl Fn(On<Activate>, MessageWriter<HudAction>) + Send + Sync + 'static
{
    move |_: On<Activate>, mut actions: MessageWriter<HudAction>| {
        actions.write(HudAction::CloseWindow);
    }
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
            // BL-82 EM-5.17 T57.13/T57.16 — every bag slot starts with the
            // legacy empty-slot art (`InvSlot`): `sync_slot_contents` swaps
            // its `image`/tint in once the slot is occupied (see that
            // system's doc comment) rather than this spawn site inserting/
            // removing the component later — a slot with items constantly
            // moving in and out just needs the ONE component mutated in
            // place. Also T57.16: `TooltipBackground(InventoryTooltipBg)`
            // reskins this slot's hover tooltip.
            parent.spawn((
                slot_bundle(
                    &theme,
                    BAG_GROUP,
                    SlotAddress::from_inv_slot_idx(net_slot.slot.idx()),
                    40.0,
                ),
                bevy::ui::widget::ImageNode::new(images.get(HudImageKey::InvSlot)),
                TooltipBackground(HudImageKey::InventoryTooltipBg),
            ));
        }
    });

    // BL-82 EM-5.17/5.18 legacy-inventory rebuild — the same 18-of-22-slot
    // Equipment panel, now spawned directly into the single `PaperdollRoot`
    // canvas with an absolute position per `PAPERDOLL_SLOT_LAYOUT`.
    if let Ok(paperdoll_entity) = paperdoll_root.single() {
        commands.entity(paperdoll_entity).with_children(|parent| {
            for &(equip_slot, left, top, size) in PAPERDOLL_SLOT_LAYOUT {
                spawn_equip_slot(parent, &theme, &images, equip_slot, left, top, size);
            }
        });
    }

    spawned.0 = true;
}

/// Spawns one Equipment-panel slot (BL-82 EM-5.17/5.18 legacy-inventory
/// rebuild) for the given `equip_slot`, absolutely positioned at
/// `(left, top)` (px, within the 250x330 [`PaperdollRoot`] canvas), `size`
/// px square — a themed [`slot_bundle`] carrying its own ghost/silhouette
/// placeholder (via [`equip_slot_frame`]) plus T57.16's `TooltipBackground`
/// reskin. `slot_bundle` already carries a real `Node` (width/height/
/// border/…) — a SECOND `Node` in the same spawn tuple would REPLACE it
/// wholesale (the exact EM-5.2 regression class; see `spawn_inventory_
/// window`'s own doc comment for the full story), so the absolute-position
/// override is applied via `.entry::<Node>().and_modify(..)` AFTER spawn,
/// the same pattern this file already uses for the window panel's row-
/// layout override. [`sync_two_handed_offhand_disable`] is the only system
/// that later mutates this same [`bevy::ui::widget::ImageNode`]'s tint
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
    equip_slot: EquipSlot,
    left: f32,
    top: f32,
    size: f32,
) {
    let discriminant = ALL_EQUIP_SLOTS
        .iter()
        .position(|&s| s == equip_slot)
        .expect("PAPERDOLL_SLOT_LAYOUT only lists slots present in ALL_EQUIP_SLOTS");
    #[expect(
        clippy::cast_possible_truncation,
        reason = "ALL_EQUIP_SLOTS has 22 entries, far below u32::MAX"
    )]
    let address = SlotAddress::from_equip_slot_discriminant(discriminant as u32);
    let mut slot_entity = parent.spawn((
        slot_bundle(theme, EQUIP_GROUP, address, size),
        bevy::ui::widget::ImageNode::new(images.get(equip_slot_frame(equip_slot))),
        TooltipBackground(HudImageKey::InventoryTooltipBg),
    ));
    slot_entity.entry::<Node>().and_modify(move |mut node| {
        node.position_type = PositionType::Absolute;
        node.left = Val::Px(left);
        node.top = Val::Px(top);
        node.width = Val::Px(size);
        node.height = Val::Px(size);
    });
    slot_entity.observe(on_equip_slot_click(equip_slot));
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

/// BL-82 EM-5.17/5.18 legacy-inventory rebuild — the per-`EquipSlot` ghost/
/// silhouette placeholder shown when that slot is empty, matching legacy
/// "xindeler-old"'s `bag.rs` (the `bag/backgrounds/*.png` ghost art). All 18
/// slots [`PAPERDOLL_SLOT_LAYOUT`] shows have a REAL dedicated ghost asset;
/// `Ring1`/`Ring2` share the single `ring.png` ghost (legacy has no separate
/// left/right ring silhouette). `Bag1`-`Bag4` are unreachable here — the
/// paper-doll never spawns them (excluded per spec; they belong on the bag
/// grid, addressed via physical `InvSlotId`, not this table).
fn equip_slot_frame(slot: EquipSlot) -> HudImageKey {
    match slot {
        EquipSlot::Armor(ArmorSlot::Head) => HudImageKey::GhostHead,
        EquipSlot::Armor(ArmorSlot::Neck) => HudImageKey::GhostNecklace,
        EquipSlot::Armor(ArmorSlot::Shoulders) => HudImageKey::GhostShoulders,
        EquipSlot::Armor(ArmorSlot::Chest) => HudImageKey::GhostChest,
        EquipSlot::Armor(ArmorSlot::Hands) => HudImageKey::GhostHands,
        EquipSlot::Armor(ArmorSlot::Ring1 | ArmorSlot::Ring2) => HudImageKey::GhostRing,
        EquipSlot::Armor(ArmorSlot::Back) => HudImageKey::GhostBack,
        EquipSlot::Armor(ArmorSlot::Belt) => HudImageKey::GhostBelt,
        EquipSlot::Armor(ArmorSlot::Legs) => HudImageKey::GhostLegs,
        EquipSlot::Armor(ArmorSlot::Feet) => HudImageKey::GhostFeet,
        EquipSlot::Armor(ArmorSlot::Tabard) => HudImageKey::GhostTabard,
        EquipSlot::ActiveMainhand | EquipSlot::InactiveMainhand => HudImageKey::GhostMainhand,
        EquipSlot::ActiveOffhand | EquipSlot::InactiveOffhand => HudImageKey::GhostOffhand,
        EquipSlot::Lantern => HudImageKey::GhostLantern,
        EquipSlot::Glider => HudImageKey::GhostGlider,
        EquipSlot::Armor(ArmorSlot::Bag1 | ArmorSlot::Bag2 | ArmorSlot::Bag3 | ArmorSlot::Bag4) => {
            unreachable!(
                "Bag1-4 are excluded from the paper-doll (spec §3.7) and never spawned via \
                 spawn_equip_slot"
            )
        },
    }
}

/// BL-82 EM-5.17/5.18 legacy-inventory rebuild — folds Xindeler's real
/// 8-tier [`Quality`] (`common/src/comp/inventory/item/mod.rs:73-82`) down to
/// the legacy `bag/buttons/inv_slot_*.png` rarity set (matching legacy
/// "xindeler-old"'s `bag.rs` colour mapping), replacing the earlier
/// `hud_d4/slot_bg_*.png` 6-texture fold:
/// - `Low` -> Grey, `Common` -> Common (legacy has a distinct "junk" grey tier
///   the earlier `hud_d4` pack didn't).
/// - `Moderate` -> Green, `High` -> Blue, `Epic` -> Purple, `Legendary` -> Gold
///   — a natural 1:1 step up the remaining named tiers.
/// - `Artifact` (the highest REAL player-facing tier) maps to Orange, the
///   legacy pack's top named colour.
/// - `Debug` is a dev-only tier that should never reach a player's bag (per the
///   earlier spec's own note) — folded to Red, purely so this match stays total
///   without a panic path; not expected to ever actually render in play.
fn quality_rarity_background(quality: Quality) -> HudImageKey {
    match quality {
        Quality::Low => HudImageKey::InvSlotGrey,
        Quality::Common => HudImageKey::InvSlotCommon,
        Quality::Moderate => HudImageKey::InvSlotGreen,
        Quality::High => HudImageKey::InvSlotBlue,
        Quality::Epic => HudImageKey::InvSlotPurple,
        Quality::Legendary => HudImageKey::InvSlotGold,
        Quality::Artifact => HudImageKey::InvSlotOrange,
        Quality::Debug => HudImageKey::InvSlotRed,
    }
}

/// The bag slot's rarity-background [`bevy::ui::widget::ImageNode`] for the
/// given (possibly absent) occupant — see [`quality_rarity_background`] for
/// the tier mapping. An empty slot gets back the legacy empty-slot art
/// (`HudImageKey::InvSlot`), the SAME state every bag slot starts in at
/// spawn ([`spawn_bag_grid_once_capacity_known`]).
fn bag_rarity_image_node(
    item: Option<&NetItemStack>,
    images: &HudImages,
) -> bevy::ui::widget::ImageNode {
    match item {
        Some(item) => {
            bevy::ui::widget::ImageNode::new(images.get(quality_rarity_background(item.quality)))
        },
        None => bevy::ui::widget::ImageNode::new(images.get(HudImageKey::InvSlot)),
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

/// BL-82 EM-5.17/5.18 legacy-inventory rebuild — writes the left [`StatColumn`]
/// readouts from the local player's already-mirrored [`NetHealth`]/
/// [`NetEnergy`]/[`NetPoise`] (the SAME `Net*` components `combat_hud.rs::
/// sync_local_player_bars` already reads off `NetLocalPlayer` — no new
/// mirror). Health/Energy show the pool's `max` (rounded); StunRes uses
/// `poise.max` as a v1 stand-in (poise IS the sim's stun-resistance pool
/// today; a dedicated "stun resistance" stat doesn't exist separately).
/// Protection/CombatRating/Stealth have NO mirrored source yet — they stay
/// the "0" placeholder [`spawn_inventory_window`] already writes.
/// `// TODO(BL-82 follow-up)`: those three need a protocol mirror field
/// before they can show a real number; this system deliberately does NOT
/// invent a value for them. Runs every frame (not `Changed<>`-gated) since
/// the write is 3 tiny `String` diffs on a handful of `Text` components —
/// negligible relative to this crate's other per-frame systems, and the
/// window is hidden most of the time anyway.
fn sync_inventory_stats(
    player: Query<
        (Option<&NetHealth>, Option<&NetEnergy>, Option<&NetPoise>),
        With<NetLocalPlayer>,
    >,
    mut values: Query<(&StatValueText, &mut Text)>,
) {
    let Ok((health, energy, poise)) = player.single() else {
        return;
    };
    for (stat, mut text) in &mut values {
        let new_text = match stat.0 {
            StatKind::Health => health.map(|h| format!("{}", h.max.round())),
            StatKind::Energy => energy.map(|e| format!("{}", e.max.round())),
            StatKind::StunRes => poise.map(|p| format!("{}", p.max.round())),
            // TODO(BL-82 follow-up): Protection/CombatRating/Stealth need a
            // protocol mirror field — no real value exists to show yet.
            StatKind::Protection | StatKind::CombatRating | StatKind::Stealth => None,
        };
        if let Some(new_text) = new_text
            && text.0 != new_text
        {
            text.0 = new_text;
        }
    }
}

/// BL-82 EM-5.17/5.18 legacy-inventory rebuild — keeps the footer's
/// [`SlotCountText`] ("occupied/total") in sync with the local player's
/// [`NetInventory`]. Runs unconditionally (NOT `Changed<NetInventory>`-gated,
/// same posture as [`sync_inventory_stats`]): the mirror can settle the
/// player's real bag capacity a frame or two AFTER the first (possibly
/// still-empty) `NetInventory` insertion, and a `Changed`-gate latched that
/// early empty snapshot as a permanent "0/0" (caught in the EM-5.18 rebuild
/// smoke — the grid rendered its real ~36 empty slots but the footer stayed
/// "0/0"). Recomputing every frame is a single ~36-element `Vec` count —
/// negligible, and always correct.
fn sync_slot_count(
    player: Query<&NetInventory, With<NetLocalPlayer>>,
    mut counts: Query<&mut Text, With<SlotCountText>>,
) {
    let Ok(inventory) = player.single() else {
        return;
    };
    let Ok(mut text) = counts.single_mut() else {
        return;
    };
    let occupied = inventory
        .slots
        .iter()
        .filter(|slot| slot.item.is_some())
        .count();
    let new_text = format!("{}/{}", occupied, inventory.slots.len());
    if text.0 != new_text {
        text.0 = new_text;
    }
}

// TODO(BL-82 follow-up — its OWN EM task, NOT round 2): real `.vox` item icons
// are a substantial subsystem, not a tweak. `NetItemStack` already carries the
// `item_id: ItemDefinitionIdOwned` that would key them, but rendering them the
// way legacy "xindeler-old" does needs an OFFSCREEN voxel→2D-icon render
// pipeline: xindeler-old's `voxygen/src/hud/item_imgs.rs` maps each `ItemKey`
// through `item_image_manifest.ron` (~5.7k lines, ~1400 `VoxTrans` entries —
// nearly every icon is a `.vox` model with a per-item ortho rotation/zoom/
// offset), which conrod renders via its built-in `Graphic::Voxel` offscreen
// cache. Porting that to Bevy = a render-to-texture target + `.vox` segment
// meshing + the manifest + `ItemKey` resolution (which crosses the logic/shell
// isolation boundary this module deliberately keeps closed — see
// `xindeler_protocol::inventory::NetItemStack`'s own doc comment). The 3-char
// `icon_text` glyph below stays the documented v1 placeholder (see
// `xindeler_ui::slot`'s own module doc comment) until that task lands.
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
/// distinct from the legacy chrome plate [`HudImageKey::InventoryChrome`] that
/// [`spawn_inventory_window`] now renders) wrapping a [`scroll_view_bundle`]
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
/// backdrop), so `Visibility::Hidden` doesn't hit the layout-summing hazard
/// `diary.rs`'s own `sync_tab_content_visibility` doc comment documents
/// (a `Row`-direction panel with hidden-but-still-laid-out siblings summing
/// their widths regardless of which is "selected"); toggling it here is
/// safe and simpler.
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
    localization: NonSend<Localization>,
    current_locale: Res<CurrentLocale>,
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
    // bevy-migration and ecs-design reviewers of this PR. BL-82 EM-5.16
    // (T56.44 follow-up, bevy-migration-reviewer finding): ALSO rebuild on a
    // locale change while open — the "Unequip" row's label is resolved via
    // `Localization::tr` (`spawn_unequip_row`) but this gate previously had
    // no locale term at all, so a language switch left it stale until an
    // unrelated inventory/picker change happened to force a rebuild.
    let inventory_changed = inventory
        .as_ref()
        .map(|inv| inv.is_changed())
        .unwrap_or(false);
    let picker_open = picker.open_slot.is_some();
    if !picker.is_changed()
        && !(picker_open && inventory_changed)
        && !(picker_open && current_locale.is_changed())
    {
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
            spawn_unequip_row(
                parent,
                &theme,
                &fonts,
                &localization,
                open_slot,
                free_bag_slot,
            );
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
    localization: &Localization,
    open_slot: EquipSlot,
    free_bag_slot: Option<InvSlotId>,
) {
    let mut row = labeled_button(parent, theme, fonts, localization, "hud-bag-unequip");
    row.insert(EquipPickerUnequipRow);
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

/// Test-only: an empty-catalog `Localization` — every `.tr(key)` call
/// resolves to `key` itself (the documented, never-panic fallback), which is
/// all these structural tests need (mirrors `settings_window.rs`/
/// `esc_menu.rs`/`trade_ui.rs`'s own identically-named test helper).
#[cfg(test)]
fn test_localization() -> Localization {
    Localization::load(&xindeler_ui::i18n::fallback_locale(), &[])
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
        // BL-82 EM-5.17/5.18 legacy-inventory rebuild — `spawn_inventory_
        // window` now reads `HudImages` directly (stat icons, gold coin),
        // so this test needs a real (test) `HudImages`, same as this file's
        // other `new_app_with_hud_resources`-style fixtures.
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::image::Image>();
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.insert_non_send(test_localization());

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

    /// BL-82 EM-5.17/5.18 legacy-inventory rebuild — pins the documented
    /// 8-tier-to-legacy-rarity-set fold (this function's own doc comment) so
    /// a future `Quality` variant addition/reorder can't silently change
    /// which legacy `inv_slot_*.png` colour a tier shows without a test
    /// noticing. Every tier now maps to a DISTINCT legacy colour (unlike the
    /// earlier `hud_d4`-set fold, which shared two textures across pairs of
    /// tiers).
    #[test]
    fn quality_rarity_background_maps_every_tier_to_a_distinct_legacy_colour() {
        assert_eq!(
            quality_rarity_background(Quality::Low),
            HudImageKey::InvSlotGrey
        );
        assert_eq!(
            quality_rarity_background(Quality::Common),
            HudImageKey::InvSlotCommon
        );
        assert_eq!(
            quality_rarity_background(Quality::Moderate),
            HudImageKey::InvSlotGreen
        );
        assert_eq!(
            quality_rarity_background(Quality::High),
            HudImageKey::InvSlotBlue
        );
        assert_eq!(
            quality_rarity_background(Quality::Epic),
            HudImageKey::InvSlotPurple
        );
        assert_eq!(
            quality_rarity_background(Quality::Legendary),
            HudImageKey::InvSlotGold
        );
        assert_eq!(
            quality_rarity_background(Quality::Artifact),
            HudImageKey::InvSlotOrange
        );
        assert_eq!(
            quality_rarity_background(Quality::Debug),
            HudImageKey::InvSlotRed
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

    /// BL-82 EM-5.17/5.18 legacy-inventory rebuild — `spawn_inventory_window`
    /// produces the single combined legacy layout: exactly one [`StatColumn`]
    /// (with 6 [`StatValueText`] rows), one fixed-size [`PaperdollRoot`], one
    /// [`BagGridRoot`], and one [`SlotCountText`] — all mounted
    /// SIMULTANEOUSLY (no tab machinery hides any of them), replacing the
    /// earlier tab-split's `ItemsTabRoot`/`EquipmentTabRoot` pair.
    #[test]
    fn spawn_inventory_window_produces_stat_column_paperdoll_and_slot_count() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::image::Image>();
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.insert_non_send(test_localization());

        app.world_mut()
            .run_system_once(spawn_inventory_window)
            .expect("spawn_inventory_window runs");

        let world = app.world_mut();
        assert_eq!(
            world.query::<&StatColumn>().iter(world).count(),
            1,
            "exactly one stat column"
        );
        assert_eq!(
            world.query::<&StatValueText>().iter(world).count(),
            6,
            "one row per StatKind"
        );
        assert_eq!(
            world.query::<&PaperdollRoot>().iter(world).count(),
            1,
            "exactly one paper-doll root, mounted unconditionally (no tab display toggle)"
        );
        assert_eq!(
            world.query::<&BagGridRoot>().iter(world).count(),
            1,
            "exactly one bag grid root, mounted unconditionally"
        );
        assert_eq!(
            world.query::<&SlotCountText>().iter(world).count(),
            1,
            "exactly one footer slot-count readout"
        );
    }

    /// BL-82 EM-5.18 legacy-inventory round 2 — `spawn_inventory_window`
    /// produces the title-bar chrome legacy "xindeler-old" shows: exactly one
    /// [`InventoryCloseButton`] (top-right red-X), and at least one `ImageNode`
    /// wired to [`HudImageKey::CharacterPortrait`] (top-left bust). Guards the
    /// two most visible gaps Matías flagged against the `captura10.png`
    /// reference from silently regressing.
    #[test]
    fn spawn_inventory_window_produces_portrait_and_close_button() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::image::Image>();
        let asset_server = app.world().resource::<AssetServer>().clone();
        let images = HudImages::load(&asset_server);
        let portrait_handle = images.get(HudImageKey::CharacterPortrait);
        let chrome_handle = images.get(HudImageKey::InventoryChrome);
        app.insert_resource(images);
        app.insert_non_send(test_localization());

        app.world_mut()
            .run_system_once(spawn_inventory_window)
            .expect("spawn_inventory_window runs");

        let world = app.world_mut();
        assert_eq!(
            world.query::<&InventoryCloseButton>().iter(world).count(),
            1,
            "exactly one title-bar close button"
        );
        let has_portrait = world
            .query::<&bevy::ui::widget::ImageNode>()
            .iter(world)
            .any(|node| node.image == portrait_handle);
        assert!(
            has_portrait,
            "the title bar must render the CharacterPortrait bust top-left"
        );
        // Round 2 gap 4: the window panel must be backed by the ornate legacy
        // chrome plate (`inv_bg_0.png`), not the earlier flat themed panel —
        // guards the frame from silently regressing to `panel_bundle`.
        let has_chrome = world
            .query::<&bevy::ui::widget::ImageNode>()
            .iter(world)
            .any(|node| node.image == chrome_handle);
        assert!(
            has_chrome,
            "the window must render the InventoryChrome plate as its background"
        );
    }

    /// BL-82 EM-5.18 legacy-inventory round 2 — clicking the close button's
    /// real observer wiring ([`on_close_button_click`], the SAME closure
    /// [`spawn_close_button`] attaches) writes [`HudAction::CloseWindow`], the
    /// generic action `apply_hud_actions` turns into `HudState::close`.
    #[test]
    fn close_button_click_writes_close_window_action() {
        use bevy::ecs::message::Messages;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<HudAction>();

        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(entity)
            .observe(on_close_button_click());

        app.world_mut().trigger(Activate { entity });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<HudAction>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![HudAction::CloseWindow]);
    }

    /// BL-82 EM-5.17/5.18 legacy-inventory rebuild — `PaperdollRoot`'s own
    /// `Node` is the fixed-size (250x330px) `Relative`-positioned canvas the
    /// module doc comment describes, NOT the earlier tab-split's flex `Row`.
    #[test]
    fn paperdoll_root_is_a_fixed_size_relative_canvas() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::image::Image>();
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.insert_non_send(test_localization());

        app.world_mut()
            .run_system_once(spawn_inventory_window)
            .expect("spawn_inventory_window runs");

        let world = app.world_mut();
        let node = world
            .query_filtered::<&Node, With<PaperdollRoot>>()
            .single(world)
            .expect("PaperdollRoot exists");
        assert_eq!(node.width, Val::Px(250.0));
        assert_eq!(node.height, Val::Px(330.0));
        assert_eq!(node.position_type, PositionType::Relative);
    }

    /// BL-82 EM-5.17/5.18 legacy-inventory rebuild — every
    /// [`PAPERDOLL_SLOT_LAYOUT`] entry names a real [`ALL_EQUIP_SLOTS`] slot
    /// (so `spawn_equip_slot`'s `.position(..).expect(..)` can never panic at
    /// runtime), the table is the confirmed 18 shown slots, and it excludes
    /// the four `Bag1-4` slots (which belong on the bag grid, not the
    /// paper-doll — and whose `equip_slot_frame` arm is `unreachable!`).
    #[test]
    fn paperdoll_layout_is_the_eighteen_shown_equip_slots() {
        assert_eq!(PAPERDOLL_SLOT_LAYOUT.len(), 18);
        for &(slot, ..) in PAPERDOLL_SLOT_LAYOUT {
            assert!(
                ALL_EQUIP_SLOTS.contains(&slot),
                "{slot:?} is not a canonical EquipSlot — spawn_equip_slot would panic"
            );
            assert!(
                !matches!(
                    slot,
                    EquipSlot::Armor(
                        ArmorSlot::Bag1 | ArmorSlot::Bag2 | ArmorSlot::Bag3 | ArmorSlot::Bag4
                    )
                ),
                "Bag1-4 must never appear on the paper-doll (spec §3.7)"
            );
        }
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
        app.insert_non_send(test_localization());
        // BL-82 EM-5.16 (T56.44 follow-up): `rebuild_equip_picker_contents`
        // now reads `Res<CurrentLocale>` as part of its rebuild gate.
        app.init_resource::<CurrentLocale>();
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

    /// BL-82 EM-5.16 (T56.44 follow-up): switching the active locale
    /// re-localizes the already-spawned title-bar heading AND the equip
    /// picker's "Unequip" button live, using the REAL repo `.ftl` catalogs
    /// (not a synthetic fixture) via `VELOREN_ASSETS`/`XINDELER_ASSETS` — the
    /// same idiom `esc_menu.rs`'s `switching_locale_relocalizes_the_quit_
    /// button_live` test uses, covering both halves of the reactive chain
    /// (a bare `LocalizedText` node via `relocalize_text`, and a
    /// `LocalizedLabel`-tagged button via `relocalize_button_labels` +
    /// `button::spawn_button_labels`) for this screen.
    #[test]
    fn switching_locale_relocalizes_the_inventory_title_and_unequip_button_live() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::image::Image>();
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.insert_resource(EquipPickerState::default());
        app.add_message::<InventoryActionRequest>();
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &["hud/bag.ftl"],
        ));
        app.init_resource::<xindeler_ui::i18n::CurrentLocale>();
        app.add_systems(Update, xindeler_ui::button::spawn_button_labels);

        app.world_mut()
            .run_system_once(spawn_inventory_window)
            .expect("spawn_inventory_window runs");
        // `EquipPickerContentRoot` is spawned by `spawn_equip_picker_root`
        // (a SEPARATE top-level system — see its own doc comment: the picker
        // is a sibling root, not nested under `InventoryWindowRoot`), not by
        // `spawn_inventory_window` — without this, `rebuild_equip_picker_
        // contents`'s `content_root.single()` silently finds nothing and the
        // whole rebuild below is a no-op (caught by this test itself).
        app.world_mut()
            .run_system_once(spawn_equip_picker_root)
            .expect("spawn_equip_picker_root runs");

        let open_slot = EquipSlot::Armor(ArmorSlot::Feet);
        app.world_mut().spawn((NetLocalPlayer, NetInventory {
            slots: Vec::new(),
            equipped: vec![NetEquippedSlot {
                slot: open_slot,
                item: Some(one_handed_weapon_stack()),
            }],
            capacity: 0,
        }));
        *app.world_mut().resource_mut::<EquipPickerState>() = EquipPickerState {
            open_slot: Some(open_slot),
        };
        app.world_mut()
            .run_system_once(rebuild_equip_picker_contents)
            .expect("system runs");
        app.update(); // let spawn_button_labels give the Unequip button its child

        fn title_text(app: &mut App) -> String {
            let world = app.world_mut();
            world
                .query::<(&LocalizedText, &Text)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "hud-bag-title")
                .map(|(_, text)| text.0.clone())
                .expect("the title bar heading was spawned and tagged")
        }

        fn unequip_button_text(app: &mut App) -> String {
            let world = app.world_mut();
            let button = world
                .query::<(&LocalizedLabel, &Children)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "hud-bag-unequip")
                .map(|(_, children)| children[0])
                .expect("the Unequip button was spawned and tagged");
            world
                .get::<Text>(button)
                .expect("label child exists")
                .0
                .clone()
        }

        assert_eq!(
            title_text(&mut app),
            "Inventory",
            "the title bar must show the real en catalog text at spawn time"
        );
        assert_eq!(
            unequip_button_text(&mut app),
            "Unequip",
            "the Unequip button must show the real en catalog text at spawn time"
        );

        app.world_mut()
            .resource_mut::<xindeler_ui::i18n::CurrentLocale>()
            .0 = "es-419".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_text)
            .expect("relocalize_text runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_button_labels)
            .expect("relocalize_button_labels runs");
        app.update(); // spawn_button_labels propagates the HudButtonLabel change

        assert_eq!(
            title_text(&mut app),
            "Inventario",
            "must resolve to the REAL es-419 catalog's own hud-bag-title value, not the en \
             fallback"
        );
        assert_eq!(
            unequip_button_text(&mut app),
            "Desequipar",
            "must resolve to the REAL es-419 catalog's own hud-bag-unequip value, not the en \
             fallback"
        );
    }
}
