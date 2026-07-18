//! BL-82 EM-5.15 — the crafting screen (spec §2/§6, tasks T56.40-42).
//!
//! Reads the local player's real mirrored [`NetCrafting`] (EM-5.15's
//! `xindeler-sim-bridge::crafting::mirror_crafting_state`) — cross-referenced
//! against the already-mirrored [`NetInventory`] for item display — and renders
//! a FOUR-TAB window (spec §Q4=A: recipes / salvage / repair / modular-weapon,
//! full parity, not a smaller v1). Every tab's action button sends a REAL
//! [`InventoryActionRequest`] wrapping a `common::comp::controller::CraftEvent`
//! inside `InventoryManip::CraftRecipe` — the SAME wire message
//! `xindeler-sim-bridge::inventory::apply_inventory_action_requests` re-emits
//! through the sim's public event bus (there is no new client→sim message for
//! crafting — see [`xindeler_protocol::crafting`]'s module doc comment).
//!
//! Toggled by the `C` key ([`xindeler_input::GameInput::Crafting`], writing
//! `HudAction::ToggleWindow(HudWindow::Crafting)`) — `HudWindow::Crafting`
//! already existed in EM-5.1's state machine, unused until now, so this screen
//! needs no `xindeler-ui` state-machine changes. Built on the same primitives
//! `inventory_ui`/`diary` use (`image_panel_bundle`, `button_bundle`,
//! `scroll_view_bundle`, the rarity-tiered `slot_bundle_with_rarity`, the
//! `zlayer::MODAL_WINDOWS` z-band), so it reads as one system with the other
//! HUD windows, not a different look.
//!
//! ## Per-tab reality (honest scope, spec §Q4=A)
//! - **Recipes** — fully real end-to-end for STATION-FREE recipes (`craft_
//!   sprite: None`, e.g. `craftsman_hammer`): the Craft button consumes the
//!   real ingredients and the crafted item appears in the bag (proven by
//!   `xindeler-sim-bridge::crafting`'s
//!   `crafting_a_recipe_round_trips_through_a_real_sim_tick_into_net_inventory`
//!   test). Station-tagged recipes are LISTED + requirement-highlighted and
//!   their craft request is real, but the sim rejects them until the player is
//!   at the required station — see the station-gate note below.
//! - **Salvage / Repair / Modular-weapon** — the UI is fully built and each
//!   action button sends the REAL
//!   `CraftEvent::Salvage`/`Repair`/`ModularWeapon` against candidate lists the
//!   sim itself computed (not stubs). BUT all three are gated SERVER-SIDE on
//!   standing at a specific crafting station
//!   (`DismantlingBench`/`RepairBench`/`CraftingBench`), supplied as the
//!   request's `craft_sprite: Option<VolumePos>`. The Bevy client has no
//!   crafting-station interaction / `VolumePos` plumbing yet (a distinct
//!   engine-migration task, unrelated to the crafting BACKEND, which is fully
//!   real — `common::recipe::{try_salvage, modular_weapon}` /
//!   `Inventory::repair_item_at_slot`), so these actions cannot COMPLETE
//!   end-to-end today; they degrade cleanly (the sim no-ops the request). Each
//!   tab shows its station requirement so the gap is visible, not silent.
//!
//! ## Deferred: free-text recipe search
//! The recipe list filters by CATEGORY (derived from each output item's asset
//! path — the "filter the recipe list" need for v1). A free-text keyboard
//! search box is deliberately NOT built here: the HUD has no generic text-input
//! focus arbitration (only `chat.rs` has a bespoke keyboard-capture path, and
//! the window-toggle hotkeys `I`/`P`/`C` are gated only on CHAT's focus), so a
//! second bespoke capture would type into the search box AND fire those
//! toggles. Categories deliver the filtering; free-text search waits for the
//! HUD-wide focus system a later epic will add.

use bevy::{ecs::schedule::common_conditions::not, prelude::*};
use common::comp::{
    InventoryManip,
    controller::CraftEvent,
    inventory::{
        item::{ItemDefinitionIdOwned, Quality},
        slot::{InvSlotId, Slot},
    },
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    InventoryActionRequest, NetCrafting, NetInventory, NetItemStack, NetLocalPlayer, NetRecipe,
    NetRepairableSlot,
};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    images::{HudImageKey, HudImages},
    panel::image_panel_bundle,
    scroll::scroll_view_bundle,
    slot::{SlotAddress, SlotContents, SlotGroup, slot_bundle_with_rarity},
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::chat::text_input_focused;

/// The crafting window's four tabs (spec §Q4=A). `Recipes` is the default.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CraftingTab {
    #[default]
    Recipes,
    Salvage,
    Repair,
    Modular,
}

impl CraftingTab {
    const ALL: [CraftingTab; 4] = [
        CraftingTab::Recipes,
        CraftingTab::Salvage,
        CraftingTab::Repair,
        CraftingTab::Modular,
    ];

    fn label(self) -> &'static str {
        match self {
            CraftingTab::Recipes => "Recipes",
            CraftingTab::Salvage => "Salvage",
            CraftingTab::Repair => "Repair",
            CraftingTab::Modular => "Modular",
        }
    }
}

/// The player's current selections across the four tabs (independent per tab).
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct CraftingSelection {
    /// Selected recipe key (recipes tab).
    pub recipe: Option<String>,
    /// Selected bag slot to salvage (salvage tab).
    pub salvage: Option<InvSlotId>,
    /// Selected slot to repair (repair tab).
    pub repair: Option<Slot>,
    /// Selected primary/secondary modular components (modular tab).
    pub modular_primary: Option<InvSlotId>,
    pub modular_secondary: Option<InvSlotId>,
}

/// The selected recipe-list category filter (`None` = show all recipes).
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct RecipeCategory(pub Option<String>);

/// The slot [`SlotGroup`] the crafting screen's icon slots live in — a fresh
/// group alongside hotbar (`0`)/bag (`1`)/equip (`2`)/trade (`3`/`4`)/diary
/// abilities (`5`)/equip-picker (`6`). These slots are DISPLAY-ONLY (their
/// drag-drop `SlotDropped` resolves to `None` against `inventory_ui`'s
/// `address_to_slot`, the same harmless "unknown group" contract that file
/// already documents) — the crafting screen drives all its actions through
/// explicit Craft/Salvage/Repair/Forge buttons, never drag-drop.
const CRAFTING_SLOT_GROUP: SlotGroup = SlotGroup(7);

const CRAFT_WINDOW_W: f32 = 620.0;
const CRAFT_WINDOW_H: f32 = 560.0;
const CONTENT_H: f32 = 430.0;
const SLOT_PX: f32 = 40.0;

// ---------------------------------------------------------------------------
// Marker components
// ---------------------------------------------------------------------------

#[derive(Component)]
struct CraftingWindowRoot;
#[derive(Component)]
struct CraftingTabBar;
#[derive(Component)]
struct CraftingTabButton(CraftingTab);
#[derive(Component)]
struct RecipesTabRoot;
#[derive(Component)]
struct SalvageTabRoot;
#[derive(Component)]
struct RepairTabRoot;
#[derive(Component)]
struct ModularTabRoot;
#[derive(Component)]
struct CategoryBarRoot;
#[derive(Component)]
struct RecipeListRoot;
#[derive(Component)]
struct RecipeDetailRoot;
#[derive(Component)]
struct SalvageListRoot;
#[derive(Component)]
struct RepairListRoot;
#[derive(Component)]
struct ModularPrimaryListRoot;
#[derive(Component)]
struct ModularSecondaryListRoot;

/// A recipe-list row button, tagged with the recipe key it selects.
#[derive(Component)]
struct RecipeRowButton(String);
/// A category-bar button, tagged with the category it filters to (`None` =
/// all).
#[derive(Component)]
struct CategoryButton(Option<String>);
/// A salvage/repair/modular candidate-row button, tagged with the slot it
/// selects.
#[derive(Component)]
struct SalvageRowButton(InvSlotId);
#[derive(Component)]
struct RepairRowButton(Slot);
#[derive(Component)]
struct ModularPrimaryRowButton(InvSlotId);
#[derive(Component)]
struct ModularSecondaryRowButton(InvSlotId);

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CraftingUiPlugin;

impl Plugin for CraftingUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CraftingTab>()
            .init_resource::<CraftingSelection>()
            .init_resource::<RecipeCategory>()
            .add_systems(
                Startup,
                spawn_crafting_window
                    .after(xindeler_ui::theme::init_theme)
                    .after(xindeler_ui::images::init_images),
            )
            .add_systems(
                Update,
                (
                    // Reads `ActionState` — after the frame's input resolution
                    // and gated on `!text_input_focused`, the SAME shape
                    // `inventory_ui::toggle_inventory_window`/
                    // `diary::toggle_diary_window` use.
                    toggle_crafting_window
                        .after(xindeler_input::InputResolveSet)
                        .run_if(not(text_input_focused)),
                    sync_crafting_window_visibility,
                    sync_crafting_tab_content_visibility,
                    rebuild_recipes_tab,
                    rebuild_salvage_tab,
                    rebuild_repair_tab,
                    rebuild_modular_tab,
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Window skeleton
// ---------------------------------------------------------------------------

/// Spawns the (initially hidden) crafting window: a dim backdrop → one themed
/// panel → title row (with close button) → a static 4-button tab bar → the four
/// tab content roots (all present in the layout, gated to one visible at a time
/// by `Display` in [`sync_crafting_tab_content_visibility`], mirroring
/// `diary`'s own `Display`-not-`Visibility` tab gating and its documented
/// layout-width reasoning).
fn spawn_crafting_window(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
) {
    commands
        .spawn((
            CraftingWindowRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::MODAL_WINDOWS),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
        ))
        .with_children(|backdrop| {
            let mut panel = backdrop.spawn(image_panel_bundle(
                &theme,
                images.get(HudImageKey::SkillTreeBg),
            ));
            panel.entry::<Node>().and_modify(|mut node| {
                node.flex_direction = FlexDirection::Column;
                node.width = Val::Px(CRAFT_WINDOW_W);
                node.height = Val::Px(CRAFT_WINDOW_H);
                node.row_gap = Val::Px(8.0);
                node.padding = UiRect::all(Val::Px(12.0));
            });
            panel.with_children(|panel| {
                // Title row: heading + close button pinned right.
                panel
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn(label_bundle(
                            &fonts,
                            &theme,
                            "Crafting",
                            22.0,
                            theme.palette.text,
                        ));
                        row.spawn(button_bundle(&theme, &fonts, "X")).observe(
                            |_: On<Activate>, mut actions: MessageWriter<HudAction>| {
                                actions.write(HudAction::CloseWindow);
                            },
                        );
                    });

                // Static tab bar (4 fixed tabs — unlike diary's data-derived
                // groups, the crafting tabs never change).
                panel
                    .spawn((CraftingTabBar, Node {
                        width: Val::Percent(100.0),
                        column_gap: Val::Px(4.0),
                        ..default()
                    }))
                    .with_children(|bar| {
                        for tab in CraftingTab::ALL {
                            bar.spawn(button_bundle(&theme, &fonts, tab.label()))
                                .insert(CraftingTabButton(tab))
                                .observe(
                                    |activate: On<Activate>,
                                     buttons: Query<&CraftingTabButton>,
                                     mut selected: ResMut<CraftingTab>| {
                                        if let Ok(button) = buttons.get(activate.entity) {
                                            *selected = button.0;
                                        }
                                    },
                                );
                        }
                    });

                // Recipes tab: category bar + recipe list (left) + detail (right).
                panel
                    .spawn((RecipesTabRoot, Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(CONTENT_H),
                        column_gap: Val::Px(10.0),
                        ..default()
                    }))
                    .with_children(|tab| {
                        tab.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|left| {
                            left.spawn((CategoryBarRoot, Node {
                                width: Val::Px(260.0),
                                flex_wrap: FlexWrap::Wrap,
                                column_gap: Val::Px(4.0),
                                row_gap: Val::Px(4.0),
                                ..default()
                            }));
                            left.spawn((
                                RecipeListRoot,
                                scroll_view_bundle(&theme, 260.0, CONTENT_H - 40.0),
                            ));
                        });
                        tab.spawn((RecipeDetailRoot, Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(6.0),
                            flex_grow: 1.0,
                            ..default()
                        }));
                    });

                // Salvage tab.
                panel
                    .spawn((SalvageTabRoot, Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(CONTENT_H),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(8.0),
                        display: Display::None,
                        ..default()
                    }))
                    .with_children(|tab| {
                        tab.spawn(label_bundle(
                            &fonts,
                            &theme,
                            "Break items down into materials. Requires a Dismantling Bench.",
                            14.0,
                            theme.palette.text_muted,
                        ));
                        tab.spawn((
                            SalvageListRoot,
                            scroll_view_bundle(&theme, CRAFT_WINDOW_W - 40.0, CONTENT_H - 90.0),
                        ));
                        tab.spawn(button_bundle(&theme, &fonts, "Salvage selected")).observe(
                            |_: On<Activate>,
                             selection: Res<CraftingSelection>,
                             mut requests: MessageWriter<InventoryActionRequest>| {
                                if let Some(slot) = selection.salvage {
                                    requests.write(InventoryActionRequest(salvage_manip(slot)));
                                }
                            },
                        );
                    });

                // Repair tab.
                panel
                    .spawn((RepairTabRoot, Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(CONTENT_H),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(8.0),
                        display: Display::None,
                        ..default()
                    }))
                    .with_children(|tab| {
                        tab.spawn(label_bundle(
                            &fonts,
                            &theme,
                            "Restore a damaged item's durability. Requires a Repair Bench.",
                            14.0,
                            theme.palette.text_muted,
                        ));
                        tab.spawn((
                            RepairListRoot,
                            scroll_view_bundle(&theme, CRAFT_WINDOW_W - 40.0, CONTENT_H - 90.0),
                        ));
                        tab.spawn(button_bundle(&theme, &fonts, "Repair selected")).observe(
                            |_: On<Activate>,
                             selection: Res<CraftingSelection>,
                             mut requests: MessageWriter<InventoryActionRequest>| {
                                if let Some(slot) = selection.repair {
                                    requests.write(InventoryActionRequest(repair_manip(slot)));
                                }
                            },
                        );
                    });

                // Modular tab.
                panel
                    .spawn((ModularTabRoot, Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(CONTENT_H),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(8.0),
                        display: Display::None,
                        ..default()
                    }))
                    .with_children(|tab| {
                        tab.spawn(label_bundle(
                            &fonts,
                            &theme,
                            "Forge a weapon from a primary + secondary component of the same tool \
                             kind. Requires a Crafting Bench.",
                            14.0,
                            theme.palette.text_muted,
                        ));
                        tab.spawn(Node {
                            width: Val::Percent(100.0),
                            column_gap: Val::Px(12.0),
                            flex_grow: 1.0,
                            ..default()
                        })
                        .with_children(|cols| {
                            cols.spawn(Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(4.0),
                                ..default()
                            })
                            .with_children(|col| {
                                col.spawn(label_bundle(
                                    &fonts,
                                    &theme,
                                    "Primary",
                                    16.0,
                                    theme.palette.text,
                                ));
                                col.spawn((
                                    ModularPrimaryListRoot,
                                    scroll_view_bundle(&theme, 280.0, CONTENT_H - 130.0),
                                ));
                            });
                            cols.spawn(Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(4.0),
                                ..default()
                            })
                            .with_children(|col| {
                                col.spawn(label_bundle(
                                    &fonts,
                                    &theme,
                                    "Secondary",
                                    16.0,
                                    theme.palette.text,
                                ));
                                col.spawn((
                                    ModularSecondaryListRoot,
                                    scroll_view_bundle(&theme, 280.0, CONTENT_H - 130.0),
                                ));
                            });
                        });
                        tab.spawn(button_bundle(&theme, &fonts, "Forge weapon")).observe(
                            |_: On<Activate>,
                             selection: Res<CraftingSelection>,
                             mut requests: MessageWriter<InventoryActionRequest>| {
                                if let (Some(primary), Some(secondary)) =
                                    (selection.modular_primary, selection.modular_secondary)
                                {
                                    requests.write(InventoryActionRequest(forge_manip(
                                        primary, secondary,
                                    )));
                                }
                            },
                        );
                    });
            });
        });
}

// ---------------------------------------------------------------------------
// Toggle + visibility
// ---------------------------------------------------------------------------

fn toggle_crafting_window(action_state: Res<ActionState>, mut actions: MessageWriter<HudAction>) {
    if action_state.just_pressed(GameInput::Crafting) {
        actions.write(HudAction::ToggleWindow(HudWindow::Crafting));
    }
}

fn sync_crafting_window_visibility(
    state: Res<HudState>,
    mut root: Query<&mut Visibility, With<CraftingWindowRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if state.is_open(HudWindow::Crafting) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Gates which tab content root is laid out, via `Display` (not `Visibility`) —
/// the SAME reasoning `diary::sync_tab_content_visibility` documents (hidden-
/// but-still-laid-out siblings would sum wider than the panel).
#[allow(clippy::type_complexity)]
fn sync_crafting_tab_content_visibility(
    tab: Res<CraftingTab>,
    mut recipes: Query<
        &mut Node,
        (
            With<RecipesTabRoot>,
            Without<SalvageTabRoot>,
            Without<RepairTabRoot>,
            Without<ModularTabRoot>,
        ),
    >,
    mut salvage: Query<
        &mut Node,
        (
            With<SalvageTabRoot>,
            Without<RecipesTabRoot>,
            Without<RepairTabRoot>,
            Without<ModularTabRoot>,
        ),
    >,
    mut repair: Query<
        &mut Node,
        (
            With<RepairTabRoot>,
            Without<RecipesTabRoot>,
            Without<SalvageTabRoot>,
            Without<ModularTabRoot>,
        ),
    >,
    mut modular: Query<
        &mut Node,
        (
            With<ModularTabRoot>,
            Without<RecipesTabRoot>,
            Without<SalvageTabRoot>,
            Without<RepairTabRoot>,
        ),
    >,
) {
    if !tab.is_changed() {
        return;
    }
    let display = |shown: bool| if shown { Display::Flex } else { Display::None };
    if let Ok(mut node) = recipes.single_mut() {
        node.display = display(*tab == CraftingTab::Recipes);
    }
    if let Ok(mut node) = salvage.single_mut() {
        node.display = display(*tab == CraftingTab::Salvage);
    }
    if let Ok(mut node) = repair.single_mut() {
        node.display = display(*tab == CraftingTab::Repair);
    }
    if let Ok(mut node) = modular.single_mut() {
        node.display = display(*tab == CraftingTab::Modular);
    }
}

// ---------------------------------------------------------------------------
// Request construction (pure helpers — unit-tested)
// ---------------------------------------------------------------------------

/// The `InventoryManip` a Craft click sends: `CraftEvent::Simple` echoing the
/// recipe key + the server-resolved `craft_slots`, `craft_sprite: None` (see
/// the module doc comment's station-gate note).
pub(crate) fn craft_recipe_manip(recipe: &NetRecipe) -> InventoryManip {
    InventoryManip::CraftRecipe {
        craft_event: CraftEvent::Simple {
            recipe: recipe.key.clone(),
            slots: recipe.craft_slots.clone(),
            amount: 1,
        },
        craft_sprite: None,
    }
}

/// The `InventoryManip` a Salvage click sends.
pub(crate) fn salvage_manip(slot: InvSlotId) -> InventoryManip {
    InventoryManip::CraftRecipe {
        craft_event: CraftEvent::Salvage(slot),
        craft_sprite: None,
    }
}

/// The `InventoryManip` a Repair click sends.
pub(crate) fn repair_manip(slot: Slot) -> InventoryManip {
    InventoryManip::CraftRecipe {
        craft_event: CraftEvent::Repair(slot),
        craft_sprite: None,
    }
}

/// The `InventoryManip` a Forge (modular-weapon) click sends.
pub(crate) fn forge_manip(primary: InvSlotId, secondary: InvSlotId) -> InventoryManip {
    InventoryManip::CraftRecipe {
        craft_event: CraftEvent::ModularWeapon {
            primary_component: primary,
            secondary_component: secondary,
        },
        craft_sprite: None,
    }
}

// ---------------------------------------------------------------------------
// Content rebuilds
// ---------------------------------------------------------------------------

/// Rebuilds the recipes tab (category bar + recipe list + detail) when the
/// mirrored crafting state, the selected tab, category, or selection changes —
/// the "cheap value/change-diff, not per-frame rebuild" posture
/// `xindeler_ui::notification` sanctions for infrequently-open widgets.
#[allow(clippy::too_many_arguments)]
fn rebuild_recipes_tab(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
    player: Query<Ref<NetCrafting>, With<NetLocalPlayer>>,
    tab: Res<CraftingTab>,
    category: Res<RecipeCategory>,
    selection: Res<CraftingSelection>,
    category_bar: Query<Entity, With<CategoryBarRoot>>,
    list_root: Query<Entity, With<RecipeListRoot>>,
    detail_root: Query<Entity, With<RecipeDetailRoot>>,
    children_query: Query<&Children>,
) {
    let Ok(crafting) = player.single() else {
        return;
    };
    if !(crafting.is_changed()
        || tab.is_changed()
        || category.is_changed()
        || selection.is_changed())
    {
        return;
    }

    // Category bar: "All" + every distinct output category, sorted.
    if let Ok(bar) = category_bar.single() {
        let mut categories: Vec<String> = crafting
            .recipes
            .iter()
            .map(recipe_category)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        rebuild_children(&mut commands, bar, &children_query, |parent| {
            parent
                .spawn(button_bundle(&theme, &fonts, "All"))
                .insert(CategoryButton(None))
                .observe(on_category_click);
            for cat in categories.drain(..) {
                parent
                    .spawn(button_bundle(&theme, &fonts, &cat))
                    .insert(CategoryButton(Some(cat.clone())))
                    .observe(on_category_click);
            }
        });
    }

    // Recipe list: filtered by category, sorted by name.
    if let Ok(list) = list_root.single() {
        let mut rows: Vec<&NetRecipe> = crafting
            .recipes
            .iter()
            .filter(|r| match &category.0 {
                None => true,
                Some(cat) => &recipe_category(r) == cat,
            })
            .collect();
        rows.sort_by(|a, b| a.output_name.cmp(&b.output_name));
        rebuild_children(&mut commands, list, &children_query, |parent| {
            for recipe in rows {
                let marker = if recipe.craftable { "* " } else { "  " };
                let label = format!("{marker}{}", recipe.output_name);
                parent
                    .spawn(button_bundle(&theme, &fonts, &label))
                    .insert(RecipeRowButton(recipe.key.clone()))
                    .observe(
                        |activate: On<Activate>,
                         buttons: Query<&RecipeRowButton>,
                         mut selection: ResMut<CraftingSelection>| {
                            if let Ok(button) = buttons.get(activate.entity) {
                                selection.recipe = Some(button.0.clone());
                            }
                        },
                    );
            }
        });
    }

    // Detail: the selected recipe's output slot + ingredient rows + Craft button.
    if let Ok(detail) = detail_root.single() {
        let selected = selection
            .recipe
            .as_ref()
            .and_then(|key| crafting.recipes.iter().find(|r| &r.key == key));
        rebuild_children(&mut commands, detail, &children_query, |parent| {
            let Some(recipe) = selected else {
                parent.spawn(label_bundle(
                    &fonts,
                    &theme,
                    "Select a recipe.",
                    16.0,
                    theme.palette.text_muted,
                ));
                return;
            };
            // Output row: rarity slot + "Name xN".
            parent
                .spawn(Node {
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_display_slot(
                        row,
                        &theme,
                        &images,
                        0,
                        &recipe.output_name,
                        recipe.output_quality,
                        Some(recipe.output_amount),
                    );
                    row.spawn(label_bundle(
                        &fonts,
                        &theme,
                        &format!("{} x{}", recipe.output_name, recipe.output_amount),
                        18.0,
                        theme.palette.text,
                    ));
                });
            if let Some(sprite) = recipe.craft_sprite {
                parent.spawn(label_bundle(
                    &fonts,
                    &theme,
                    &format!("Station: {sprite:?}"),
                    13.0,
                    theme.palette.text_muted,
                ));
            }
            parent.spawn(label_bundle(
                &fonts,
                &theme,
                "Ingredients:",
                15.0,
                theme.palette.text,
            ));
            for input in &recipe.inputs {
                let color = if input.satisfied() {
                    theme.palette.text
                } else {
                    // Requirement highlight: missing/insufficient in red.
                    Color::srgb(0.9, 0.3, 0.3)
                };
                parent.spawn(label_bundle(
                    &fonts,
                    &theme,
                    &format!("{}  {}/{}", input.name, input.available, input.required),
                    14.0,
                    color,
                ));
            }
            // Craft button (real end-to-end for station-free recipes).
            parent
                .spawn(button_bundle(&theme, &fonts, "Craft"))
                .observe(on_craft_click());
        });
    }
}

/// The Craft button's [`Activate`] handler — split out (like
/// `inventory_ui::on_candidate_row_click`) so a test can attach the EXACT same
/// wiring to a bare entity. Sends the real craft request ONLY for the
/// currently-selected recipe when it is actually craftable (the sim would
/// reject an un-craftable one anyway; this avoids a pointless round-trip).
fn on_craft_click() -> impl Fn(
    On<Activate>,
    Res<CraftingSelection>,
    Query<&NetCrafting, With<NetLocalPlayer>>,
    MessageWriter<InventoryActionRequest>,
) + Send
+ Sync
+ 'static {
    move |_: On<Activate>,
          selection: Res<CraftingSelection>,
          player: Query<&NetCrafting, With<NetLocalPlayer>>,
          mut requests: MessageWriter<InventoryActionRequest>| {
        let Ok(crafting) = player.single() else {
            return;
        };
        if let Some(recipe) = selection
            .recipe
            .as_ref()
            .and_then(|key| crafting.recipes.iter().find(|r| &r.key == key))
            .filter(|r| r.craftable)
        {
            requests.write(InventoryActionRequest(craft_recipe_manip(recipe)));
        }
    }
}

fn on_category_click(
    activate: On<Activate>,
    buttons: Query<&CategoryButton>,
    mut category: ResMut<RecipeCategory>,
) {
    if let Ok(button) = buttons.get(activate.entity) {
        category.0 = button.0.clone();
    }
}

/// Rebuilds the salvage candidate list (one row per salvageable bag slot).
fn rebuild_salvage_tab(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    player: Query<(Ref<NetCrafting>, &NetInventory), With<NetLocalPlayer>>,
    selection: Res<CraftingSelection>,
    list_root: Query<Entity, With<SalvageListRoot>>,
    children_query: Query<&Children>,
) {
    let Ok((crafting, inventory)) = player.single() else {
        return;
    };
    if !(crafting.is_changed() || selection.is_changed()) {
        return;
    }
    let Ok(list) = list_root.single() else {
        return;
    };
    rebuild_children(&mut commands, list, &children_query, |parent| {
        if crafting.salvageable.is_empty() {
            parent.spawn(label_bundle(
                &fonts,
                &theme,
                "No salvageable items.",
                15.0,
                theme.palette.text_muted,
            ));
            return;
        }
        for &slot in &crafting.salvageable {
            let name = item_at_inv(inventory, slot).map_or("(item)", |it| it.name.as_str());
            let selected = selection.salvage == Some(slot);
            spawn_candidate_row(parent, &theme, &fonts, name, selected)
                .insert(SalvageRowButton(slot))
                .observe(
                    |activate: On<Activate>,
                     buttons: Query<&SalvageRowButton>,
                     mut selection: ResMut<CraftingSelection>| {
                        if let Ok(button) = buttons.get(activate.entity) {
                            selection.salvage = Some(button.0);
                        }
                    },
                );
        }
    });
}

/// Rebuilds the repair candidate list (one row per damaged item).
fn rebuild_repair_tab(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    player: Query<(Ref<NetCrafting>, &NetInventory), With<NetLocalPlayer>>,
    selection: Res<CraftingSelection>,
    list_root: Query<Entity, With<RepairListRoot>>,
    children_query: Query<&Children>,
) {
    let Ok((crafting, inventory)) = player.single() else {
        return;
    };
    if !(crafting.is_changed() || selection.is_changed()) {
        return;
    }
    let Ok(list) = list_root.single() else {
        return;
    };
    rebuild_children(&mut commands, list, &children_query, |parent| {
        if crafting.repairable.is_empty() {
            parent.spawn(label_bundle(
                &fonts,
                &theme,
                "No damaged items.",
                15.0,
                theme.palette.text_muted,
            ));
            return;
        }
        for repairable in &crafting.repairable {
            let NetRepairableSlot {
                slot,
                durability_lost,
                max_durability,
            } = *repairable;
            let name = item_at_slot(inventory, slot).map_or("(item)", |it| it.name.as_str());
            let remaining = max_durability.saturating_sub(durability_lost);
            let selected = selection.repair == Some(slot);
            spawn_candidate_row(
                parent,
                &theme,
                &fonts,
                &format!("{name}  ({remaining}/{max_durability})"),
                selected,
            )
            .insert(RepairRowButton(slot))
            .observe(
                |activate: On<Activate>,
                 buttons: Query<&RepairRowButton>,
                 mut selection: ResMut<CraftingSelection>| {
                    if let Ok(button) = buttons.get(activate.entity) {
                        selection.repair = Some(button.0);
                    }
                },
            );
        }
    });
}

/// Rebuilds the modular tab's two candidate lists (primary / secondary).
fn rebuild_modular_tab(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    player: Query<(Ref<NetCrafting>, &NetInventory), With<NetLocalPlayer>>,
    selection: Res<CraftingSelection>,
    primary_root: Query<Entity, With<ModularPrimaryListRoot>>,
    secondary_root: Query<Entity, With<ModularSecondaryListRoot>>,
    children_query: Query<&Children>,
) {
    let Ok((crafting, inventory)) = player.single() else {
        return;
    };
    if !(crafting.is_changed() || selection.is_changed()) {
        return;
    }

    if let Ok(list) = primary_root.single() {
        rebuild_children(&mut commands, list, &children_query, |parent| {
            let mut any = false;
            for comp in crafting.components.iter().filter(|c| c.is_primary) {
                any = true;
                let name =
                    item_at_inv(inventory, comp.slot).map_or("(component)", |it| it.name.as_str());
                let selected = selection.modular_primary == Some(comp.slot);
                spawn_candidate_row(parent, &theme, &fonts, name, selected)
                    .insert(ModularPrimaryRowButton(comp.slot))
                    .observe(
                        |activate: On<Activate>,
                         buttons: Query<&ModularPrimaryRowButton>,
                         mut selection: ResMut<CraftingSelection>| {
                            if let Ok(button) = buttons.get(activate.entity) {
                                selection.modular_primary = Some(button.0);
                            }
                        },
                    );
            }
            if !any {
                parent.spawn(label_bundle(
                    &fonts,
                    &theme,
                    "No primary components.",
                    15.0,
                    theme.palette.text_muted,
                ));
            }
        });
    }
    if let Ok(list) = secondary_root.single() {
        rebuild_children(&mut commands, list, &children_query, |parent| {
            let mut any = false;
            for comp in crafting.components.iter().filter(|c| c.is_secondary) {
                any = true;
                let name =
                    item_at_inv(inventory, comp.slot).map_or("(component)", |it| it.name.as_str());
                let selected = selection.modular_secondary == Some(comp.slot);
                spawn_candidate_row(parent, &theme, &fonts, name, selected)
                    .insert(ModularSecondaryRowButton(comp.slot))
                    .observe(
                        |activate: On<Activate>,
                         buttons: Query<&ModularSecondaryRowButton>,
                         mut selection: ResMut<CraftingSelection>| {
                            if let Ok(button) = buttons.get(activate.entity) {
                                selection.modular_secondary = Some(button.0);
                            }
                        },
                    );
            }
            if !any {
                parent.spawn(label_bundle(
                    &fonts,
                    &theme,
                    "No secondary components.",
                    15.0,
                    theme.palette.text_muted,
                ));
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// A plain themed text label bundle (the idiom `diary`'s content-sync systems
/// use).
fn label_bundle(
    fonts: &HudFonts,
    _theme: &HudTheme,
    text: &str,
    size: f32,
    color: Color,
) -> impl Bundle {
    (
        Text(text.to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(size),
            ..default()
        },
        TextColor(color),
    )
}

/// The rarity-background [`HudImageKey`] for an item quality — the SAME mapping
/// `inventory_ui::quality_rarity_background` uses for the bag grid, so crafting
/// slots read identically.
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

/// Spawns a display-only rarity slot (icon glyph + optional quantity) into a
/// parent — reuses the exact `slot_bundle_with_rarity` + `SlotContents`
/// convention `inventory_ui` established.
fn spawn_display_slot(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    images: &HudImages,
    address: u64,
    name: &str,
    quality: Quality,
    quantity: Option<u32>,
) {
    let rarity = images.get(quality_rarity_background(quality));
    parent
        .spawn(slot_bundle_with_rarity(
            theme,
            CRAFTING_SLOT_GROUP,
            SlotAddress(address),
            SLOT_PX,
            rarity,
        ))
        .insert(SlotContents {
            icon_text: name.chars().next().map(String::from).unwrap_or_default(),
            quantity,
            tooltip: name.to_owned(),
        });
}

/// Spawns a candidate-list row as a single selectable button labeled with the
/// item name (a leading `> ` marks the current selection). Returns the button's
/// `EntityCommands` so the caller can tag it + attach the click observer.
///
/// Kept deliberately button-only (no separate rarity slot icon) so it stays a
/// direct child of `parent` — a nested "slot + button" row would need to hand
/// back a grandchild's `EntityCommands`, which the `ChildSpawnerCommands`
/// borrow shape makes awkward. The rarity-tiered `slot_bundle_with_rarity` icon
/// IS used where it matters most (the recipe detail's output slot,
/// [`spawn_display_slot`]); candidate lists are text rows, a reasonable v1.
fn spawn_candidate_row<'a>(
    parent: &'a mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    label: &str,
    selected: bool,
) -> bevy::ecs::system::EntityCommands<'a> {
    let marker = if selected { "> " } else { "  " };
    parent.spawn(button_bundle(theme, fonts, &format!("{marker}{label}")))
}

/// Despawns every child of `root`, then repopulates it — the shared "simple
/// rebuild" helper (a local copy of `diary`/`inventory_ui`'s own private
/// `rebuild_children`).
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

/// The item (if any) in a bag slot of a mirrored [`NetInventory`].
fn item_at_inv(inventory: &NetInventory, slot: InvSlotId) -> Option<&NetItemStack> {
    inventory
        .slots
        .iter()
        .find(|s| s.slot == slot)
        .and_then(|s| s.item.as_ref())
}

/// The item (if any) in a bag OR equipped slot of a mirrored [`NetInventory`].
fn item_at_slot(inventory: &NetInventory, slot: Slot) -> Option<&NetItemStack> {
    match slot {
        Slot::Inventory(id) => item_at_inv(inventory, id),
        Slot::Equip(equip) => inventory
            .equipped
            .iter()
            .find(|s| s.slot == equip)
            .and_then(|s| s.item.as_ref()),
        Slot::Overflow(_) => None,
    }
}

/// A coarse recipe category derived from the output item's asset-path segment
/// after `items` (e.g. `common.items.tool.craftsman_hammer` → `tool`,
/// `common.items.weapons.sword.x` → `weapons`) — the data-driven filter the
/// recipe list groups by (no hardcoded per-recipe category table). `misc` for
/// non-`Simple` ids or paths with no `items` segment.
fn recipe_category(recipe: &NetRecipe) -> String {
    let ItemDefinitionIdOwned::Simple(id) = &recipe.output_id else {
        return "misc".to_owned();
    };
    let parts: Vec<&str> = id.split('.').collect();
    parts
        .iter()
        .position(|p| *p == "items")
        .and_then(|pos| parts.get(pos + 1))
        .map_or_else(|| "misc".to_owned(), |seg| (*seg).to_owned())
}

#[cfg(test)]
mod tests {
    use xindeler_protocol::NetRecipe;

    use super::*;

    fn sample_recipe() -> NetRecipe {
        NetRecipe {
            key: "craftsman_hammer".to_owned(),
            output_id: ItemDefinitionIdOwned::Simple(
                "common.items.tool.craftsman_hammer".to_owned(),
            ),
            output_name: "Craftsman Hammer".to_owned(),
            output_amount: 1,
            output_quality: Quality::Common,
            inputs: vec![],
            craftable: true,
            craft_sprite: None,
            craft_slots: vec![(0, InvSlotId::new(0, 3)), (1, InvSlotId::new(0, 4))],
        }
    }

    /// The Craft click builds a real `CraftEvent::Simple` echoing the recipe
    /// key + the server-resolved `craft_slots`, with `craft_sprite: None` — the
    /// exact wire payload `xindeler-sim-bridge`'s round-trip test then proves
    /// the sim actually crafts from.
    #[test]
    fn craft_manip_echoes_key_and_resolved_slots() {
        let recipe = sample_recipe();
        let manip = craft_recipe_manip(&recipe);
        match manip {
            InventoryManip::CraftRecipe {
                craft_event:
                    CraftEvent::Simple {
                        recipe: key,
                        slots,
                        amount,
                    },
                craft_sprite,
            } => {
                assert_eq!(key, "craftsman_hammer");
                assert_eq!(slots, recipe.craft_slots);
                assert_eq!(amount, 1);
                assert_eq!(craft_sprite, None);
            },
            other => panic!("expected CraftEvent::Simple, got {other:?}"),
        }
    }

    #[test]
    fn salvage_manip_targets_the_slot() {
        let slot = InvSlotId::new(0, 5);
        match salvage_manip(slot) {
            InventoryManip::CraftRecipe {
                craft_event: CraftEvent::Salvage(s),
                craft_sprite,
            } => {
                assert_eq!(s, slot);
                assert_eq!(craft_sprite, None);
            },
            other => panic!("expected CraftEvent::Salvage, got {other:?}"),
        }
    }

    #[test]
    fn repair_manip_targets_the_slot() {
        let slot = Slot::Inventory(InvSlotId::new(0, 6));
        match repair_manip(slot) {
            InventoryManip::CraftRecipe {
                craft_event: CraftEvent::Repair(s),
                ..
            } => assert_eq!(s, slot),
            other => panic!("expected CraftEvent::Repair, got {other:?}"),
        }
    }

    #[test]
    fn forge_manip_pairs_primary_and_secondary() {
        let primary = InvSlotId::new(0, 1);
        let secondary = InvSlotId::new(0, 2);
        match forge_manip(primary, secondary) {
            InventoryManip::CraftRecipe {
                craft_event:
                    CraftEvent::ModularWeapon {
                        primary_component,
                        secondary_component,
                    },
                ..
            } => {
                assert_eq!(primary_component, primary);
                assert_eq!(secondary_component, secondary);
            },
            other => panic!("expected CraftEvent::ModularWeapon, got {other:?}"),
        }
    }

    /// The real Craft observer, attached to a bare entity and fired with a real
    /// `Activate` (the SAME `world.trigger(Activate { entity })` idiom
    /// `inventory_ui`/`diary`'s own observer tests use), sends exactly one
    /// `InventoryActionRequest` carrying the selected recipe's `CraftEvent::
    /// Simple` — proving the click→request wiring end-to-end, not just the pure
    /// manip helper.
    #[test]
    fn craft_click_sends_the_real_request() {
        use bevy::ecs::message::Messages;
        use xindeler_protocol::NetCrafting;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<InventoryActionRequest>();
        app.insert_resource(CraftingSelection {
            recipe: Some("craftsman_hammer".to_owned()),
            ..default()
        });
        app.world_mut().spawn((NetLocalPlayer, NetCrafting {
            recipes: vec![sample_recipe()],
            ..default()
        }));
        let button = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(button).observe(on_craft_click());

        app.world_mut().trigger(Activate { entity: button });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<InventoryActionRequest>>()
            .drain()
            .collect();
        assert_eq!(sent.len(), 1, "exactly one craft request is sent");
        match &sent[0].0 {
            InventoryManip::CraftRecipe {
                craft_event: CraftEvent::Simple { recipe, .. },
                ..
            } => assert_eq!(recipe, "craftsman_hammer"),
            other => panic!("expected CraftEvent::Simple, got {other:?}"),
        }
    }

    /// A Craft click for an UN-craftable recipe sends nothing (the observer
    /// filters on `craftable`).
    #[test]
    fn craft_click_sends_nothing_for_an_uncraftable_recipe() {
        use bevy::ecs::message::Messages;
        use xindeler_protocol::NetCrafting;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<InventoryActionRequest>();
        app.insert_resource(CraftingSelection {
            recipe: Some("craftsman_hammer".to_owned()),
            ..default()
        });
        let uncraftable = NetRecipe {
            craftable: false,
            craft_slots: vec![],
            ..sample_recipe()
        };
        app.world_mut().spawn((NetLocalPlayer, NetCrafting {
            recipes: vec![uncraftable],
            ..default()
        }));
        let button = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(button).observe(on_craft_click());

        app.world_mut().trigger(Activate { entity: button });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<InventoryActionRequest>>()
            .drain()
            .collect();
        assert!(sent.is_empty(), "no request for an un-craftable recipe");
    }

    /// Categories are derived from the output item's asset path, data-driven —
    /// not a hardcoded per-recipe table.
    #[test]
    fn recipe_category_reads_the_asset_path() {
        assert_eq!(recipe_category(&sample_recipe()), "tool");
        let weapon = NetRecipe {
            output_id: ItemDefinitionIdOwned::Simple(
                "common.items.weapons.sword.long_sword".to_owned(),
            ),
            ..sample_recipe()
        };
        assert_eq!(recipe_category(&weapon), "weapons");
    }
}
