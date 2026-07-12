//! BL-82 EM-5.3 — the skillbar/hotbar screen, extending EM-5.2's core combat
//! HUD to full parity: real drag-to-assign, real keybind labels sourced from
//! EM-5.11's `xindeler-input` keymap, and cooldown greying/wipe reading the
//! `xindeler-sim-bridge::hotbar` mirror.
//!
//! ## Slot count (not a hardcoded 10)
//! `xindeler_protocol::NetAbilities::slots` is exactly however many
//! auxiliary-ability slots the sim currently supports for the local
//! player's weapon context (`ActiveAbilities::limit`, `Some(5)` by default —
//! see that mirror's own doc comment). This screen renders exactly that
//! many real, currently-usable slots, keybind-labelled from
//! `GameInput::Slot{n}` for `n <= 10` — it does NOT pad to a fake 10-slot
//! row, so a future skill/perk that raises the limit shows up automatically.
//! M1/M2 (primary/secondary) are separate, non-draggable indicators — the
//! sim's `PrimaryAbility`/`SecondaryAbility` are fixed to "whatever's
//! wielded", not user-rebindable, so there is no slot address for them.
//!
//! ## Real drag-to-assign, todays scope
//! The `xindeler-ui::slot` drag-drop primitive is wired end-to-end: dragging
//! one hotbar slot onto another swaps their bindings via TWO
//! `LocalAssignHotbarSlot` messages (drained by `xindeler-sim-bridge::
//! hotbar::apply_local_hotbar_assignment`, which calls the embedded player's
//! REAL `client::Client::change_ability` — a genuine network send, never a
//! direct ECS write). Today the only drag SOURCE is another hotbar slot
//! (EM-5.6's inventory/EM-5.7's diary — the item/ability sources the spec
//! names — haven't landed yet); a drop whose `from`/`to` groups don't both
//! equal [`HOTBAR_GROUP`] is ignored, not silently mis-applied. Once those
//! screens exist, they reuse the SAME [`xindeler_ui::slot::SlotDropped`]
//! event with their own [`xindeler_ui::slot::SlotGroup`] — no rework needed
//! here.
//!
//! ## Cooldown "sweep" (a documented v1 simplification)
//! The sim's `AbilityCooldowns` only carries the absolute ready-at time, not
//! the original cooldown duration (`xindeler_protocol::NetCooldownEntry`'s
//! own doc comment) — [`sync_cooldown_overlays`]'s own `Local<HashMap<String,
//! f32>>` infers a per-ability "total" as the largest `remaining_secs`
//! observed since it last read as ready, so the
//! overlay height (a linear top-down wipe, not a radial one — Bevy 0.19
//! `bevy_ui` has no circular-clip primitive without a custom material, out
//! of scope for v1) is a genuine, if self-correcting-on-first-use,
//! proportion rather than a guess.
//!
//! Compiled only under `listen-server`/`net-client` — same posture as every
//! other `xindeler_protocol`-consuming module in this crate.
//!
//! ## `xindeler_ui::slot` reuse note
//! The drag-drop slot primitive this screen depends on (`xindeler_ui::slot`)
//! shipped as part of EM-5.6 (PR #88, `feat/bl82-em56-inventory-trade`) while
//! this epic was in progress on a separate branch based off EM-5.11 — this
//! branch adopts that same, already-committed `xindeler-ui/src/slot.rs`
//! verbatim (not a second, competing implementation) so the two epics don't
//! diverge; whichever of the two PRs merges first keeps the file, the other
//! rebases onto it with a trivial (likely no-op) conflict.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use xindeler_input::{GameInput, KeyMap};
use xindeler_protocol::{
    LocalAssignHotbarSlot, NetAbilities, NetAuxiliaryAbility, NetCooldowns, NetLocalPlayer,
};
use xindeler_ui::{
    slot::{SlotAddress, SlotContents, SlotDropped, SlotGroup, slot_bundle},
    theme::{HudFonts, HudTheme},
};

use crate::controls_screen::key_label;

/// The one drag-drop group this screen's slots live in — an internal detail
/// (never interpreted by `xindeler_ui::slot`, which stays opinion-free about
/// what a group number means).
const HOTBAR_GROUP: SlotGroup = SlotGroup(0);

const SLOT_SIZE_PX: f32 = 44.0;

/// The keybind each rendered slot index (0-based) is labelled with, for
/// indices `< 10` — beyond that a slot still works (drag/cooldown/fire all
/// function), it just shows no keybind glyph (documented, not a crash: the
/// sim's `ActiveAbilities::limit` could in principle exceed 10, though
/// nothing does today).
const SLOT_INPUTS: [GameInput; 10] = [
    GameInput::Slot1,
    GameInput::Slot2,
    GameInput::Slot3,
    GameInput::Slot4,
    GameInput::Slot5,
    GameInput::Slot6,
    GameInput::Slot7,
    GameInput::Slot8,
    GameInput::Slot9,
    GameInput::Slot10,
];

/// Installs the hotbar: spawns the (initially empty) slot row + M1/M2
/// indicators at `Startup`, then keeps slot count/content, keybind labels,
/// and cooldown overlays synced every frame, and applies real drag-drop
/// reassignment.
pub struct HotbarViewPlugin;

impl Plugin for HotbarViewPlugin {
    fn build(&self, app: &mut App) {
        // BL-82 EM-5.4 already hit this exact bug (chat.rs's own doc
        // comment): `XindelerUiPlugin` doesn't override `is_unique()`
        // (defaults `true`), so a SECOND `add_plugins` call — e.g. after
        // `CombatHudViewPlugin` already added it — panics ("plugin was
        // already added") instead of silently no-opping. Guard it the same
        // way that fix did, rather than relying on registration ORDER
        // (`ControlsScreenPlugin`'s posture, which happens to work only
        // because it's always added after `CombatHudViewPlugin` today).
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        app.init_resource::<HotbarSlotEntities>()
            .add_systems(Startup, spawn_hotbar.after(xindeler_ui::theme::init_theme))
            .add_systems(
                Update,
                (
                    sync_hotbar_slots,
                    sync_primary_secondary_indicators,
                    sync_keybind_labels,
                    sync_cooldown_overlays.after(sync_hotbar_slots),
                    handle_hotbar_drag_drop,
                ),
            );
    }
}

/// index -> spawned slot entity, resized by [`sync_hotbar_slots`] to match
/// [`NetAbilities::slots`]'s real (sim-driven) length.
#[derive(Resource, Default)]
struct HotbarSlotEntities(Vec<Entity>);

#[derive(Component)]
struct HotbarSlotRow;
#[derive(Component)]
struct HotbarPrimaryText;
#[derive(Component)]
struct HotbarSecondaryText;
/// Which [`GameInput`] this keybind-label child displays — resolved fresh
/// every frame from the live [`KeyMap`] (a rebind updates it immediately,
/// same acceptance bar EM-5.11's own controls screen established).
#[derive(Component)]
struct HotbarKeybindLabel(GameInput);
#[derive(Component)]
struct HotbarCooldownOverlay;
#[derive(Component)]
struct HotbarCooldownText;

/// A short, uppercase placeholder glyph for a dotted ability id (e.g.
/// `"class.warrior.rally"` -> `"RALL"`) — the SAME "themed placeholder,
/// reviewer-approved for v1" posture EM-5.2's buff-strip colour swatches and
/// `xindeler_ui::slot`'s own icon-text established; real `.vox`/icon-atlas
/// art is a documented follow-up (EM-5.1's own deferred `.vox`-icon path).
fn short_glyph(ability_id: &str) -> String {
    let segment = ability_id.rsplit('.').next().unwrap_or(ability_id);
    let mut glyph: String = segment.chars().take(4).collect();
    glyph.make_ascii_uppercase();
    glyph
}

fn spawn_hotbar(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    commands.spawn((HotbarSlotRow, Node {
        position_type: PositionType::Absolute,
        bottom: Val::Px(24.0),
        left: Val::Px(0.0),
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Row,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        column_gap: Val::Px(theme.spacing.xs),
        ..Default::default()
    }));

    let text_font = |font: Handle<bevy::text::Font>| TextFont {
        font: bevy::text::FontSource::Handle(font),
        font_size: bevy::text::FontSize::Px(14.0),
        ..Default::default()
    };
    commands.spawn((
        HotbarPrimaryText,
        Text(String::new()),
        text_font(fonts.body.clone()),
        TextColor(theme.palette.text),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(28.0),
            left: Val::Px(16.0),
            ..Default::default()
        },
    ));
    commands.spawn((
        HotbarSecondaryText,
        Text(String::new()),
        text_font(fonts.body.clone()),
        TextColor(theme.palette.text),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(28.0),
            right: Val::Px(16.0),
            ..Default::default()
        },
    ));
}

/// Resizes [`HotbarSlotEntities`] to match [`NetAbilities::slots`]'s real
/// length and writes each slot's [`SlotContents`].
///
/// ## Why this reads `NetAbilities` unconditionally every frame, not gated
/// ## on `Changed<NetAbilities>` (a real bug this fixed)
/// `xindeler-sim-bridge::hotbar::mirror_hotbar_state` dedups server-side
/// (`HotbarMirrorCache`) — it only re-inserts `NetAbilities` when the VALUE
/// actually differs, which for the local player typically happens once,
/// very early (often before this plugin's own systems get their first
/// `Update` execution at all). A `Changed<NetAbilities>` filter on THIS
/// system would compare against ITS OWN "last observed" tick, established
/// the first time it runs — if the one-and-only real change already
/// happened before that baseline was captured, `Changed` reads `false`
/// FOREVER for that entity, even though the data is genuinely present and
/// correct (confirmed live: a direct probe query with no `Changed` filter
/// saw the real 5-slot `NetAbilities` the whole time `Changed<NetAbilities>`
/// never fired once). `combat_hud.rs`'s own `sync_local_player_bars` never
/// gated on `Changed` for exactly this class of reason — it just re-reads
/// every frame (cheap: a handful of small components) and writes into
/// `SlotContents` only when the computed value actually differs (the
/// `if *slot_contents != new_contents` check below), which is the correct
/// place to avoid redundant work, not the query filter.
fn sync_hotbar_slots(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    row: Query<Entity, With<HotbarSlotRow>>,
    mut slot_entities: ResMut<HotbarSlotEntities>,
    mut contents: Query<&mut SlotContents>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    let Ok(row_entity) = row.single() else {
        return;
    };

    // Entities at indices `< old_len` already existed BEFORE this call and
    // are visible to the `contents` `Query` below; entities spawned by the
    // resize loop just below are only queued via `Commands` — they do NOT
    // exist in `contents`' view of the world until the command queue is
    // flushed (the next sync point / `app.update()`), so this system must
    // NOT try to update them through that `Query` in the SAME call (it
    // would silently no-op, leaving a freshly-spawned slot's `SlotContents`
    // at `slot_bundle`'s empty default forever, since `NetAbilities` may not
    // change again for a long time). See the content-write loop at the
    // bottom of this function for the split this requires.
    let old_len = slot_entities.0.len();

    while slot_entities.0.len() < abilities.slots.len() {
        let index = slot_entities.0.len();
        let slot_entity = commands
            .spawn(slot_bundle(
                &theme,
                HOTBAR_GROUP,
                SlotAddress(index as u64),
                SLOT_SIZE_PX,
            ))
            .id();
        commands.entity(slot_entity).with_children(|parent| {
            if let Some(&input) = SLOT_INPUTS.get(index) {
                parent.spawn((
                    HotbarKeybindLabel(input),
                    Text(String::new()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(11.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text_muted),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(1.0),
                        left: Val::Px(2.0),
                        ..Default::default()
                    },
                ));
            }
            parent.spawn((
                HotbarCooldownOverlay,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(0.0),
                    ..Default::default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            ));
            parent.spawn((
                HotbarCooldownText,
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(12.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(1.0),
                    right: Val::Px(2.0),
                    ..Default::default()
                },
            ));
        });
        commands.entity(row_entity).add_child(slot_entity);
        slot_entities.0.push(slot_entity);
    }
    while slot_entities.0.len() > abilities.slots.len() {
        if let Some(extra) = slot_entities.0.pop() {
            commands.entity(extra).despawn();
        }
    }

    for (index, slot) in abilities.slots.iter().enumerate() {
        let Some(&entity) = slot_entities.0.get(index) else {
            continue;
        };
        let new_contents = SlotContents {
            icon_text: slot
                .ability_id
                .as_deref()
                .map(short_glyph)
                .unwrap_or_default(),
            // Hotbar slots hold abilities, not stackable items — no
            // quantity badge (`xindeler_ui::slot`'s own field for EM-5.6's
            // bag/trade screens).
            quantity: None,
            tooltip: slot
                .ability_id
                .clone()
                .unwrap_or_else(|| "Empty".to_owned()),
        };
        if index < old_len {
            // Pre-existing entity: a live `Query` sees it right now, so
            // updating in place (and skipping a no-op write) is both
            // correct and cheap.
            if let Ok(mut slot_contents) = contents.get_mut(entity)
                && *slot_contents != new_contents
            {
                *slot_contents = new_contents;
            }
        } else {
            // Freshly spawned this call (see the doc comment above `old_len`)
            // — `contents` cannot see it yet, so queue the real content as a
            // command chained onto the same entity instead of relying on
            // the `Query` to catch it next frame (which would leave the
            // slot showing nothing until `NetAbilities` happens to change
            // again).
            commands.entity(entity).insert(new_contents);
        }
    }
}

/// Refreshes the M1/M2 read-only indicators from the local player's
/// [`NetAbilities::primary`]/`secondary`. Reads unconditionally every frame
/// (NOT gated on `Changed<NetAbilities>` — see [`sync_hotbar_slots`]'s own
/// doc comment for why that filter is unsafe to use against a server-side
/// deduped mirror), diffing before writing `Text` to avoid a pointless
/// per-frame mutation once the value settles.
fn sync_primary_secondary_indicators(
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    mut primary_text: Query<&mut Text, (With<HotbarPrimaryText>, Without<HotbarSecondaryText>)>,
    mut secondary_text: Query<&mut Text, (With<HotbarSecondaryText>, Without<HotbarPrimaryText>)>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    if let Ok(mut text) = primary_text.single_mut() {
        let new_text = format!(
            "M1: {}",
            abilities
                .primary
                .as_deref()
                .map(short_glyph)
                .unwrap_or_else(|| "-".to_owned())
        );
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
    if let Ok(mut text) = secondary_text.single_mut() {
        let new_text = format!(
            "M2: {}",
            abilities
                .secondary
                .as_deref()
                .map(short_glyph)
                .unwrap_or_else(|| "-".to_owned())
        );
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

/// Recomputes every keybind-label child's text from the LIVE [`KeyMap`]
/// every frame — cheap at this screen's slot count, and the same "no
/// `is_changed` gate needed at this scale" call `controls_screen.rs`'s own
/// `refresh_binding_labels` already made. A rebind of `Slot1..Slot10`
/// updates the hotbar's own labels immediately, no extra wiring needed.
fn sync_keybind_labels(keymap: Res<KeyMap>, mut labels: Query<(&HotbarKeybindLabel, &mut Text)>) {
    for (label, mut text) in &mut labels {
        let new_label = keymap
            .keyboard
            .get_binding(label.0)
            .map_or_else(String::new, key_label);
        if text.0 != new_label {
            text.0 = new_label;
        }
    }
}

/// Reads the local player's [`NetCooldowns`] and updates every slot's
/// cooldown overlay height + countdown text — see the module doc comment
/// for the "sweep total" heuristic (a `Local<HashMap<String, f32>>` of
/// inferred totals, pruned each frame for abilities that finished cooling
/// down).
#[allow(clippy::too_many_arguments)]
fn sync_cooldown_overlays(
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    cooldowns: Query<&NetCooldowns, With<NetLocalPlayer>>,
    slot_entities: Res<HotbarSlotEntities>,
    children_of: Query<&Children>,
    mut overlays: Query<&mut Node, With<HotbarCooldownOverlay>>,
    mut texts: Query<&mut Text, With<HotbarCooldownText>>,
    mut known_totals: Local<HashMap<String, f32>>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    let cooldowns = cooldowns.single().ok();

    let cooling_ids: HashSet<&str> = cooldowns
        .map(|c| c.0.iter().map(|e| e.ability_id.as_str()).collect())
        .unwrap_or_default();
    known_totals.retain(|id, _| cooling_ids.contains(id.as_str()));

    let remaining_of = |ability_id: &str| -> f32 {
        cooldowns
            .and_then(|c| c.0.iter().find(|e| e.ability_id == ability_id))
            .map_or(0.0, |e| e.remaining_secs)
    };

    for (index, slot) in abilities.slots.iter().enumerate() {
        let Some(&entity) = slot_entities.0.get(index) else {
            continue;
        };
        let Ok(children) = children_of.get(entity) else {
            continue;
        };

        let remaining = slot.ability_id.as_deref().map_or(0.0, remaining_of);
        let fraction = if remaining > 0.0 {
            let ability_id = slot.ability_id.clone().unwrap_or_default();
            let total = known_totals.entry(ability_id).or_insert(remaining);
            *total = total.max(remaining);
            (remaining / *total).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let countdown_text = if remaining > 0.0 {
            format!("{:.0}", remaining.ceil())
        } else {
            String::new()
        };

        for child in children.iter() {
            if let Ok(mut node) = overlays.get_mut(child) {
                node.height = Val::Percent(fraction * 100.0);
            }
            if let Ok(mut text) = texts.get_mut(child)
                && text.0 != countdown_text
            {
                text.0 = countdown_text.clone();
            }
        }
    }
}

/// Drains [`SlotDropped`] and applies whichever of the two SOURCEs this
/// screen currently supports:
/// - a drag ENTIRELY within the hotbar (`from`/`to` both [`HOTBAR_GROUP`])
///   swaps the two slots' bindings via TWO [`LocalAssignHotbarSlot`] messages —
///   the module doc comment's original "real drag-to-assign" contract;
/// - BL-82 EM-5.7: a drag FROM the diary's Abilities tab
///   (`crate::diary::DIARY_ABILITY_GROUP`) INTO a hotbar slot binds that
///   ability into the target slot via ONE [`LocalAssignHotbarSlot`] — the
///   dragged ability is decoded straight from the [`SlotDropped::from_address`]
///   (packed via `NetAuxiliaryAbility::to_slot_address_raw`, see that type's
///   own doc comment), no `abilities` lookup needed for the source side.
///
/// A drop involving any OTHER group is silently ignored — not mis-applied —
/// exactly the module doc comment's original posture, just narrowed to the
/// groups that don't yet have a handler.
fn handle_hotbar_drag_drop(
    mut drops: MessageReader<SlotDropped>,
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    mut assign: MessageWriter<LocalAssignHotbarSlot>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    for drop in drops.read() {
        if drop.to_group != HOTBAR_GROUP {
            continue;
        }

        if drop.from_group == crate::diary::DIARY_ABILITY_GROUP {
            let to_index = drop.to_address.raw() as usize;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "hotbar slot indices are a handful, never near u32::MAX"
            )]
            assign.write(LocalAssignHotbarSlot {
                slot: to_index as u32,
                ability: NetAuxiliaryAbility::from_slot_address_raw(drop.from_address.raw()),
            });
            continue;
        }

        if drop.from_group != HOTBAR_GROUP {
            continue;
        }
        let from_index = drop.from_address.raw() as usize;
        let to_index = drop.to_address.raw() as usize;
        if from_index == to_index {
            continue;
        }
        let (Some(from_slot), Some(to_slot)) = (
            abilities.slots.get(from_index),
            abilities.slots.get(to_index),
        ) else {
            continue;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "hotbar slot indices are a handful, never near u32::MAX"
        )]
        {
            assign.write(LocalAssignHotbarSlot {
                slot: to_index as u32,
                ability: from_slot.aux,
            });
            assign.write(LocalAssignHotbarSlot {
                slot: from_index as u32,
                ability: to_slot.aux,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use xindeler_protocol::{NetCooldownEntry, NetHotbarSlot};

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.init_resource::<HotbarSlotEntities>();
        app
    }

    #[test]
    fn short_glyph_takes_the_last_dotted_segment_uppercased() {
        assert_eq!(short_glyph("class.warrior.rally"), "RALL");
        assert_eq!(short_glyph("m1"), "M1");
    }

    /// The T56.15 acceptance bar (spec: "10 slots show bound abilities +
    /// keybind labels ... persist"): a real `NetAbilities` with N slots
    /// drives exactly N spawned slot entities, each with the right
    /// [`SlotContents`] — and the slot count is NOT hardcoded to 10, it
    /// tracks whatever the mirror reports.
    #[test]
    fn sync_hotbar_slots_spawns_exactly_as_many_slots_as_the_mirror_reports() {
        let mut app = new_app();
        app.world_mut().spawn(HotbarSlotRow);
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: Some("common.abilities.sword.primary".to_owned()),
            secondary: None,
            slots: vec![
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::MainWeapon(0),
                    ability_id: Some("common.abilities.sword.m1".to_owned()),
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Empty,
                    ability_id: None,
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Innate(0),
                    ability_id: Some("class.warrior.rally".to_owned()),
                },
            ],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("sync_hotbar_slots runs");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>();
        assert_eq!(
            slot_entities.0.len(),
            3,
            "must match the mirror's real slot count"
        );

        let world = app.world();
        let first_contents = world.get::<SlotContents>(slot_entities.0[0]).unwrap();
        assert_eq!(first_contents.icon_text, "M1");
        assert_eq!(first_contents.tooltip, "common.abilities.sword.m1");

        let empty_contents = world.get::<SlotContents>(slot_entities.0[1]).unwrap();
        assert_eq!(empty_contents.icon_text, "");
        assert_eq!(empty_contents.tooltip, "Empty");
    }

    /// A LATER change to `NetAbilities` (fewer slots — e.g. a weapon swap to
    /// a context with a shorter aux set) despawns the extra slot entities
    /// rather than leaving stale ones behind.
    #[test]
    fn sync_hotbar_slots_shrinks_when_the_mirror_reports_fewer_slots() {
        let mut app = new_app();
        app.world_mut().spawn(HotbarSlotRow);
        let player = app
            .world_mut()
            .spawn((NetLocalPlayer, NetAbilities {
                primary: None,
                secondary: None,
                slots: vec![
                    NetHotbarSlot::default(),
                    NetHotbarSlot::default(),
                    NetHotbarSlot::default(),
                ],
            }))
            .id();
        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("first run");
        app.update();
        assert_eq!(app.world().resource::<HotbarSlotEntities>().0.len(), 3);

        app.world_mut()
            .get_mut::<NetAbilities>(player)
            .unwrap()
            .slots = vec![NetHotbarSlot::default()];
        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("second run shrinks");
        app.update();
        assert_eq!(
            app.world().resource::<HotbarSlotEntities>().0.len(),
            1,
            "extra slot entities must be despawned, not left stale"
        );
    }

    /// A [`SlotDropped`] entirely within the hotbar group swaps the two
    /// slots' `aux` values via two `LocalAssignHotbarSlot` messages — the
    /// literal EM-5.3 drag-to-assign acceptance bar.
    #[test]
    fn hotbar_internal_drag_drop_swaps_via_two_local_assign_messages() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<LocalAssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::MainWeapon(0),
                    ability_id: Some("a".to_owned()),
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Innate(2),
                    ability_id: Some("b".to_owned()),
                },
            ],
        }));
        app.world_mut().write_message(SlotDropped {
            from_group: HOTBAR_GROUP,
            from_address: SlotAddress(0),
            to_group: HOTBAR_GROUP,
            to_address: SlotAddress(1),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalAssignHotbarSlot>>()
            .drain()
            .collect();
        assert_eq!(sent.len(), 2);
        assert!(sent.contains(&LocalAssignHotbarSlot {
            slot: 1,
            ability: NetAuxiliaryAbility::MainWeapon(0),
        }));
        assert!(sent.contains(&LocalAssignHotbarSlot {
            slot: 0,
            ability: NetAuxiliaryAbility::Innate(2),
        }));
    }

    /// BL-82 EM-5.7 (T56.24): a drag FROM the diary's Abilities tab
    /// (`crate::diary::DIARY_ABILITY_GROUP`) INTO a hotbar slot binds the
    /// packed ability into that slot via ONE `LocalAssignHotbarSlot` — no
    /// `abilities` lookup needed for the source side, the ability is decoded
    /// straight from the dragged address.
    #[test]
    fn diary_ability_drag_binds_the_dragged_ability_into_the_target_slot() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<LocalAssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default(), NetHotbarSlot::default()],
        }));
        let dragged = NetAuxiliaryAbility::Innate(3);
        app.world_mut().write_message(SlotDropped {
            from_group: crate::diary::DIARY_ABILITY_GROUP,
            from_address: SlotAddress(dragged.to_slot_address_raw()),
            to_group: HOTBAR_GROUP,
            to_address: SlotAddress(1),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalAssignHotbarSlot>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![LocalAssignHotbarSlot {
            slot: 1,
            ability: dragged,
        }]);
    }

    /// A drop where either end is NOT in the hotbar group is ignored — no
    /// message is written (the "silently ignored, not mis-applied"
    /// contract for a future cross-screen drag source).
    #[test]
    fn cross_group_drop_is_ignored() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<LocalAssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default(), NetHotbarSlot::default()],
        }));
        app.world_mut().write_message(SlotDropped {
            from_group: SlotGroup(99),
            from_address: SlotAddress(0),
            to_group: HOTBAR_GROUP,
            to_address: SlotAddress(1),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<LocalAssignHotbarSlot>>()
            .drain()
            .collect();
        assert!(sent.is_empty());
    }

    /// The cooldown overlay's height fraction is derived from
    /// `remaining/total` where `total` is the FIRST-observed remaining value
    /// for that ability id — and clears back to 0% once the cooldown entry
    /// disappears (ability ready).
    #[test]
    fn cooldown_overlay_tracks_remaining_over_inferred_total() {
        let mut app = new_app();
        app.world_mut().spawn(HotbarSlotRow);
        let player = app
            .world_mut()
            .spawn((
                NetLocalPlayer,
                NetAbilities {
                    primary: None,
                    secondary: None,
                    slots: vec![NetHotbarSlot {
                        aux: NetAuxiliaryAbility::Innate(0),
                        ability_id: Some("class.warrior.rally".to_owned()),
                    }],
                },
                NetCooldowns(vec![NetCooldownEntry {
                    ability_id: "class.warrior.rally".to_owned(),
                    remaining_secs: 8.0,
                }]),
            ))
            .id();

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn the slot entity first");
        app.update();

        // `sync_cooldown_overlays` is registered as a REAL, persistent
        // system (not called via `run_system_once` a second time) — its
        // `Local<HashMap<String, f32>>` "inferred total" state must survive
        // across ticks the same way it does in the real app (`Update`
        // schedule); `run_system_once` would re-register a fresh one-shot
        // system EVERY call, silently resetting that `Local` to empty each
        // time and defeating the whole "infer the total from the first
        // observation" heuristic this test exists to verify.
        app.add_systems(Update, sync_cooldown_overlays);
        app.update();

        let slot_entity = app.world().resource::<HotbarSlotEntities>().0[0];
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(slot_entity)
            .unwrap()
            .iter()
            .collect();
        let overlay = children
            .iter()
            .copied()
            .find(|&e| app.world().get::<HotbarCooldownOverlay>(e).is_some())
            .expect("overlay child exists");
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().height,
            Val::Percent(100.0),
            "at first observation, remaining == inferred total -> full overlay"
        );

        // Halfway through the cooldown: remaining is 4.0 of the inferred 8.0
        // total -> 50%.
        app.world_mut().get_mut::<NetCooldowns>(player).unwrap().0[0].remaining_secs = 4.0;
        app.update();
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().height,
            Val::Percent(50.0)
        );

        // Ability ready: the cooldown entry disappears -> overlay clears.
        app.world_mut()
            .get_mut::<NetCooldowns>(player)
            .unwrap()
            .0
            .clear();
        app.update();
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().height,
            Val::Percent(0.0)
        );
    }
}
