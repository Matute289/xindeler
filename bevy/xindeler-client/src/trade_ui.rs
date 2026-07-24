//! BL-82 EM-5.6 — the full two-party trade window (spec §2/§6/§9 Q7=A "not
//! sub-scoped"; task T56.21).
//!
//! Reads the local player's real mirrored [`NetIncomingTradeInvite`]/
//! [`NetTrade`] (`xindeler-sim-bridge::trade`) and drives the WHOLE trade
//! flow through real client→sim messages:
//! - **Initiating**: since no target-picker UI exists yet anywhere in this
//!   phase (EM-5.8's player list is the natural future home for "right-click a
//!   name → trade"), `T` sends [`TradeInviteRequest`] to the nearest OTHER
//!   mirrored entity with a [`NetUid`] — a real, working, if minimal, v1
//!   (documented, not a stub); a proper target-picker is EM-5.8's job.
//! - **Responding to an invite**: [`NetIncomingTradeInvite`] shows an Accept/
//!   Decline prompt, sending [`TradeInviteResponseRequest`].
//! - **Once a trade is active** ([`NetTrade`] present): both offer grids render
//!   on the [`xindeler_ui::slot`] primitive (read from
//!   `my_offer`/`their_offer`, already resolved server-side — see
//!   `xindeler_protocol::trade`'s module doc comment for why the counterparty's
//!   raw inventory is never needed). Dragging a BAG slot (from
//!   [`crate::inventory_ui::BAG_GROUP`]) onto the "my offer" grid sends
//!   `TradeAction::AddItem`; dragging a "my offer" slot back onto the bag grid
//!   sends `TradeAction::RemoveItem` (the whole offered amount —
//!   partial-quantity stepping is a follow-up, not attempted here). Accept/
//!   Decline buttons send `TradeAction::Accept`/`Decline`.
//!
//! All of this travels over REAL replicated state end-to-end (§9 Q7=A) —
//! `NetTrade`/`NetIncomingTradeInvite` are genuine `.replicate::<>()`
//! components (`xindeler_protocol::owner_visibility`-scoped), and every
//! action is a genuine `add_client_message` request the bridge re-emits
//! through the sim's OWN `Trades`/invite event handling
//! (`server::events::trade`/`invite`) — nothing here is client-only mocked.

use bevy::{ecs::change_detection::NonSend, prelude::*};
use common::trade::{TradeAction, TradePhase};
use xindeler_protocol::{
    NetIncomingTradeInvite, NetLocalPlayer, NetTrade, NetTradeOfferEntry, NetUid,
    TradeActionRequest, TradeInviteRequest, TradeInviteResponseRequest,
};
use xindeler_ui::{
    button::{Activate, button_bundle},
    i18n::{Localization, LocalizedLabel, LocalizedText},
    panel::panel_bundle,
    slot::{HudSlot, SlotAddress, SlotContents, SlotDropped, SlotGroup, slot_bundle},
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::inventory_ui::{BAG_GROUP, bag_address_to_inv_slot};

/// The two offer-grid [`SlotGroup`]s this screen owns — distinct from
/// [`crate::inventory_ui::BAG_GROUP`]/its equip group so a drop between a
/// bag slot and an offer slot is unambiguous.
const MY_OFFER_GROUP: SlotGroup = SlotGroup(3);
const THEIR_OFFER_GROUP: SlotGroup = SlotGroup(4);

/// How many offer slots this v1 renders per side — a real trade offer is
/// bounded only by inventory size, but a fixed-size grid (no scroll) is the
/// simplest v1 (matches this epic's other fixed-size-grid choices); a trade
/// with more simultaneous distinct items than this needs a follow-up
/// (scroll/paginate), not attempted here.
const OFFER_SLOT_COUNT: u64 = 12;

#[derive(Component)]
struct InviteRoot;
#[derive(Component)]
struct TradeWindowRoot;
#[derive(Component)]
struct TradeStatusText;
#[derive(Component)]
struct InviteStatusText;
#[derive(Component)]
struct MyOfferGridRoot;
#[derive(Component)]
struct TheirOfferGridRoot;

pub struct TradeUiPlugin;

impl Plugin for TradeUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            spawn_trade_ui.after(xindeler_ui::theme::init_theme),
        )
        .add_systems(
            Update,
            (
                send_trade_invite_to_nearest,
                sync_invite_prompt,
                // BL-82 EM-5.16 (T56.44 follow-up): reads `NonSend<
                // Localization>` (for the trade-phase label) — ordered after
                // `LocaleSyncSet` so a locale switch is reflected the SAME
                // frame while a trade is active, matching `settings_window
                // .rs`'s `refresh_setting_labels` ordering rationale (no
                // ordering guarantee otherwise between two systems with a
                // conflicting `NonSend` access).
                sync_trade_window.after(xindeler_ui::i18n::LocaleSyncSet),
                handle_offer_slot_drops,
            ),
        );
    }
}

/// Spawns the (initially hidden) invite prompt + the (initially hidden)
/// trade window, INCLUDING its fixed-size offer-grid slots (unlike the bag
/// grid, an offer grid's slot count never depends on server data —
/// `OFFER_SLOT_COUNT` is a client-side v1 constant, so there's no need to
/// wait for a `NetTrade` to arrive before spawning them, unlike
/// `crate::inventory_ui`'s capacity-dependent bag grid).
///
/// BL-82 holistic-review z-index fix: both [`InviteRoot`] and
/// [`TradeWindowRoot`] are visually modal — real, centered/near-centered,
/// interactive windows that temporarily own the player's attention, the same
/// paint-order category `zlayer::MODAL_WINDOWS`'s own doc comment names for
/// the diary/inventory/full-map trio — but `panel_bundle` (the bundle both
/// spawn from) never inserts a `GlobalZIndex`, so without an explicit one
/// here they'd default to z-partition 0, sitting BELOW the always-on ambient
/// chrome (`ORBS_ACTION_BAR_PARTY_MINIMAP`=20) the same way
/// `InventoryWindowRoot`/`EscMenuRoot`/`FullMapRoot` did before their own
/// fixes this session — `bevy_ui` picking (highest z-partition first) would
/// route clicks to the chrome in front instead of these windows underneath.
/// Note this is purely a paint-order/z-tier fix: unlike the diary/inventory/
/// social trio, the trade window doesn't participate in `HudState`'s
/// mutually-exclusive window slot (no `HudWindow::Trade` variant exists) and
/// so does NOT free the OS cursor via `cursor.rs`'s
/// `HudState::any_window_open` — that's a pre-existing gap this patch
/// neither introduces nor fixes.
fn spawn_trade_ui(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    localization: NonSend<Localization>,
) {
    // --- Invite prompt ---
    let mut invite_entity = commands.spawn((
        InviteRoot,
        Visibility::Hidden,
        GlobalZIndex(zlayer::MODAL_WINDOWS),
        panel_bundle(&theme),
    ));
    invite_entity.entry::<Node>().and_modify(|mut node| {
        node.position_type = PositionType::Absolute;
        node.top = Val::Px(90.0);
        node.left = Val::Percent(38.0);
        node.flex_direction = FlexDirection::Column;
    });
    invite_entity.with_children(|parent| {
        // BL-82 EM-5.16 (T56.44 follow-up): the invite text embeds a raw
        // player UID (no player-name mirror exists yet — see the module doc
        // comment's "Initiating" note), so the static "Trade request from
        // player" label and the dynamic "#<uid>" value are two SIBLING `Text`
        // nodes rather than one `format!`-ed string — the static half stays
        // reactively localized (tagged `LocalizedText`, no code in
        // `sync_invite_prompt` needs to re-resolve it); only the number is
        // written at runtime.
        parent
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(4.0),
                ..Default::default()
            })
            .with_children(|row| {
                row.spawn((
                    LocalizedText("hud-trade-invite_from_player"),
                    Text(localization.tr("hud-trade-invite_from_player")),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
                row.spawn((
                    InviteStatusText,
                    Text(String::new()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
            });
        parent
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.sm),
                ..Default::default()
            })
            .with_children(|row| {
                labeled_button(row, &theme, &fonts, &localization, "common-accept").observe(
                    |_activate: On<Activate>,
                     mut requests: MessageWriter<TradeInviteResponseRequest>| {
                        requests.write(TradeInviteResponseRequest { accept: true });
                    },
                );
                labeled_button(row, &theme, &fonts, &localization, "common-decline").observe(
                    |_activate: On<Activate>,
                     mut requests: MessageWriter<TradeInviteResponseRequest>| {
                        requests.write(TradeInviteResponseRequest { accept: false });
                    },
                );
            });
    });

    // --- Trade window ---
    let mut window_entity = commands.spawn((
        TradeWindowRoot,
        Visibility::Hidden,
        GlobalZIndex(zlayer::MODAL_WINDOWS),
        panel_bundle(&theme),
    ));
    window_entity.entry::<Node>().and_modify(|mut node| {
        node.position_type = PositionType::Absolute;
        node.top = Val::Percent(20.0);
        node.left = Val::Percent(20.0);
        node.flex_direction = FlexDirection::Column;
    });
    window_entity.with_children(|parent| {
        // BL-82 EM-5.16 (T56.44 follow-up): same static-label/dynamic-value
        // split as the invite prompt above — the counterparty's raw UID and
        // the `TradePhase`'s per-variant label (see [`trade_phase_key`]) are
        // genuinely dynamic; the leading "Trading with player" phrase is not.
        parent
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(4.0),
                ..Default::default()
            })
            .with_children(|row| {
                row.spawn((
                    LocalizedText("hud-trade-trading_with_player"),
                    Text(localization.tr("hud-trade-trading_with_player")),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.title.clone()),
                        font_size: bevy::text::FontSize::Px(20.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
                row.spawn((
                    TradeStatusText,
                    Text(String::new()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.title.clone()),
                        font_size: bevy::text::FontSize::Px(20.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
            });
        parent
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.lg),
                ..Default::default()
            })
            .with_children(|columns| {
                columns
                    .spawn((MyOfferGridRoot, Node {
                        display: Display::Grid,
                        grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(4, 48.0)],
                        row_gap: Val::Px(4.0),
                        column_gap: Val::Px(4.0),
                        ..Default::default()
                    }))
                    .with_children(|grid| {
                        for i in 0..OFFER_SLOT_COUNT {
                            grid.spawn(slot_bundle(&theme, MY_OFFER_GROUP, SlotAddress(i), 48.0));
                        }
                    });
                columns
                    .spawn((TheirOfferGridRoot, Node {
                        display: Display::Grid,
                        grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(4, 48.0)],
                        row_gap: Val::Px(4.0),
                        column_gap: Val::Px(4.0),
                        ..Default::default()
                    }))
                    .with_children(|grid| {
                        for i in 0..OFFER_SLOT_COUNT {
                            grid.spawn(slot_bundle(
                                &theme,
                                THEIR_OFFER_GROUP,
                                SlotAddress(i),
                                48.0,
                            ));
                        }
                    });
            });
        parent
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.sm),
                ..Default::default()
            })
            .with_children(|row| {
                // Reuses `hud-trade-accept`/`hud-trade-decline` — the SAME
                // keys legacy `voxygen/src/hud/trade.rs::accept_decline_
                // buttons` resolves for this exact pair (finalizing the
                // active trade, `TradeAction::Accept`/`Decline`), whose real
                // catalog value is the plain "Accept"/"Decline" (not the
                // hardcoded "Accept Trade"/"Decline Trade" wording this
                // screen used to show) — preferred over inventing new keys
                // for a near-duplicate meaning.
                labeled_button(row, &theme, &fonts, &localization, "hud-trade-accept").observe(
                    |_activate: On<Activate>,
                     trade: Query<&NetTrade, With<NetLocalPlayer>>,
                     mut requests: MessageWriter<TradeActionRequest>| {
                        if let Ok(trade) = trade.single() {
                            requests.write(TradeActionRequest {
                                trade_id: trade.trade_id,
                                action: TradeAction::Accept(trade.phase),
                            });
                        }
                    },
                );
                labeled_button(row, &theme, &fonts, &localization, "hud-trade-decline").observe(
                    |_activate: On<Activate>,
                     trade: Query<&NetTrade, With<NetLocalPlayer>>,
                     mut requests: MessageWriter<TradeActionRequest>| {
                        if let Ok(trade) = trade.single() {
                            requests.write(TradeActionRequest {
                                trade_id: trade.trade_id,
                                action: TradeAction::Decline,
                            });
                        }
                    },
                );
            });
    });
}

/// Spawns a themed button whose label is a resolved `.ftl` message value,
/// tagged [`LocalizedLabel`] so it re-resolves live on a locale change (the
/// same small helper `settings_window.rs`/`esc_menu.rs` both already use).
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

/// The `.ftl` key for a [`TradePhase`]'s short display label — used in the
/// trade window's "Trading with player #N — <phase>" header (see
/// [`sync_trade_window`]); distinct from `hud-trade-phase1_description`/
/// `phase2_description`/`phase3_description` (already existing, but those are
/// the longer explanatory paragraphs the module doc references, not a header
/// word).
fn trade_phase_key(phase: TradePhase) -> &'static str {
    match phase {
        TradePhase::Mutate => "hud-trade-phase_mutate",
        TradePhase::Review => "hud-trade-phase_review",
        TradePhase::Complete => "hud-trade-phase_complete",
    }
}

/// Sends [`TradeInviteRequest`] on `T` to the nearest OTHER mirrored entity
/// carrying a [`NetUid`] — see the module doc comment's "Initiating" note
/// for why this is the v1 target-selection story.
fn send_trade_invite_to_nearest(
    keys: Res<ButtonInput<KeyCode>>,
    player: Query<&GlobalTransform, With<NetLocalPlayer>>,
    others: Query<(&NetUid, &GlobalTransform), Without<NetLocalPlayer>>,
    mut requests: MessageWriter<TradeInviteRequest>,
) {
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    let Ok(player_transform) = player.single() else {
        return;
    };
    let nearest = others.iter().min_by(|(_, a), (_, b)| {
        let da = a
            .translation()
            .distance_squared(player_transform.translation());
        let db = b
            .translation()
            .distance_squared(player_transform.translation());
        da.total_cmp(&db)
    });
    if let Some((uid, _)) = nearest {
        requests.write(TradeInviteRequest { target_uid: uid.0 });
    }
}

/// Shows/hides the invite prompt from the local player's
/// [`NetIncomingTradeInvite`] mirror, filling in the "from" text. Degrades
/// clean (hides, never panics) when the local player's mirror hasn't
/// arrived yet — spec §3.2.
fn sync_invite_prompt(
    player: Query<Option<&NetIncomingTradeInvite>, With<NetLocalPlayer>>,
    mut root: Query<&mut Visibility, With<InviteRoot>>,
    mut text: Query<&mut Text, With<InviteStatusText>>,
) {
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    match player.single().ok().flatten() {
        Some(invite) => {
            *visibility = Visibility::Visible;
            if let Ok(mut text) = text.single_mut() {
                // The static "Trade request from player" label is a SIBLING
                // `Text` node (tagged `LocalizedText`, spawned in
                // `spawn_trade_ui`) — this one only ever holds the raw,
                // never-translated UID.
                text.0 = format!("#{}", invite.from_uid);
            }
        },
        None => *visibility = Visibility::Hidden,
    }
}

/// Shows/hides the trade window from the local player's [`NetTrade`] mirror
/// and reconciles every offer-grid slot's [`SlotContents`] against
/// `my_offer`/`their_offer` — a slot beyond the current offer's length shows
/// empty (a valid drop target for adding a new item). Degrades clean.
fn sync_trade_window(
    player: Query<Option<&NetTrade>, With<NetLocalPlayer>>,
    mut window_visibility: Query<&mut Visibility, (With<TradeWindowRoot>, Without<InviteRoot>)>,
    mut status_text: Query<&mut Text, (With<TradeStatusText>, Without<InviteStatusText>)>,
    mut offer_slots: Query<(&SlotGroup, &SlotAddress, &mut SlotContents), With<HudSlot>>,
    localization: NonSend<Localization>,
) {
    let Ok(mut visibility) = window_visibility.single_mut() else {
        return;
    };
    let Some(trade) = player.single().ok().flatten() else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Visible;
    if let Ok(mut text) = status_text.single_mut() {
        // The static "Trading with player" label is a SIBLING `Text` node
        // (tagged `LocalizedText`, spawned in `spawn_trade_ui`) — this one
        // only holds the raw UID plus the `TradePhase`'s own resolved label
        // ([`trade_phase_key`]), so it must re-resolve on a locale change too
        // (this system reads `NonSend<Localization>` every frame and is
        // ordered `.after(xindeler_ui::i18n::LocaleSyncSet)` in
        // `TradeUiPlugin::build` for exactly that reason).
        text.0 = format!(
            "#{} — {}",
            trade.counterparty_uid,
            localization.tr(trade_phase_key(trade.phase))
        );
    }

    for (group, address, mut contents) in &mut offer_slots {
        let list = if *group == MY_OFFER_GROUP {
            &trade.my_offer
        } else if *group == THEIR_OFFER_GROUP {
            &trade.their_offer
        } else {
            continue;
        };
        let idx = address.raw() as usize;
        *contents = list
            .get(idx)
            .map(offer_entry_to_slot_contents)
            .unwrap_or_default();
    }
}

fn offer_entry_to_slot_contents(entry: &NetTradeOfferEntry) -> SlotContents {
    SlotContents {
        icon_text: entry.item.name.chars().take(3).collect(),
        // Real icons aren't wired into the trade screen yet — untagged
        // slots (no `SlotIconKey`) are simply invisible to
        // `inventory_ui::apply_item_icons`, so this stays `icon_text`-only.
        icon: None,
        quantity: Some(entry.offered),
        tooltip: format!(
            "{} ({:?}) — offering {} of {}",
            entry.item.name, entry.item.quality, entry.offered, entry.owned
        ),
    }
}

/// Translates a completed [`SlotDropped`] between the bag and "my offer"
/// grid into a real [`TradeActionRequest`] — dragging a bag slot IN adds it
/// (`AddItem`, quantity 1 — a stepper for partial quantities is a
/// follow-up); dragging a "my offer" slot back OUT removes the whole offered
/// amount (`RemoveItem`). Drops elsewhere (their offer, unrelated groups)
/// are ignored — the "their offer" grid is read-only in v1 (see module doc
/// comment).
fn handle_offer_slot_drops(
    mut drops: MessageReader<SlotDropped>,
    trade: Query<&NetTrade, With<NetLocalPlayer>>,
    mut requests: MessageWriter<TradeActionRequest>,
) {
    let Ok(trade) = trade.single() else {
        // No active trade — nothing these drops could mean; drop them
        // rather than buffering them until a LATER trade begins (spec
        // §3.2 "degrade clean", mirroring `xindeler-sim-bridge`'s own
        // "drop pending requests" posture for a similarly gated system).
        drops.clear();
        return;
    };

    for drop in drops.read() {
        if drop.from_group == BAG_GROUP && drop.to_group == MY_OFFER_GROUP {
            let inv_slot = bag_address_to_inv_slot(drop.from_address);
            requests.write(TradeActionRequest {
                trade_id: trade.trade_id,
                action: TradeAction::AddItem {
                    item: inv_slot,
                    quantity: 1,
                    ours: true,
                },
            });
        } else if drop.from_group == MY_OFFER_GROUP && drop.to_group == BAG_GROUP {
            let idx = drop.from_address.raw() as usize;
            if let Some(entry) = trade.my_offer.get(idx) {
                requests.write(TradeActionRequest {
                    trade_id: trade.trade_id,
                    action: TradeAction::RemoveItem {
                        item: entry.slot,
                        quantity: entry.offered,
                        ours: true,
                    },
                });
            }
        }
    }
}

/// Test-only: an empty-catalog `Localization` — every `.tr(key)` call
/// resolves to `key` itself (the documented, never-panic fallback), which is
/// all these structural tests need (mirrors `settings_window.rs`/
/// `esc_menu.rs`'s own identically-named test helper).
#[cfg(test)]
fn test_localization() -> Localization {
    Localization::load(&xindeler_ui::i18n::fallback_locale(), &[])
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use common::comp::inventory::{
        item::{ItemDefinitionIdOwned, Quality},
        slot::InvSlotId,
    };
    use xindeler_protocol::NetItemStack;

    use super::*;

    fn new_app_with_hud_resources() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(test_localization());
        app
    }

    /// BL-82 holistic-review z-index fix regression: [`InviteRoot`] and
    /// [`TradeWindowRoot`] are both genuinely modal windows (see
    /// [`spawn_trade_ui`]'s own doc comment for why) but `panel_bundle`
    /// never inserts a `GlobalZIndex` — before this fix, neither carried
    /// one at all (default z-partition 0), sitting BELOW the always-on
    /// ambient chrome the same way `InventoryWindowRoot`/`EscMenuRoot`/
    /// `FullMapRoot` did before their own prior fixes this session. Pins
    /// that both now carry `GlobalZIndex(zlayer::MODAL_WINDOWS)`.
    #[test]
    fn invite_and_trade_window_roots_carry_the_modal_windows_z_index() {
        let mut app = new_app_with_hud_resources();
        app.world_mut()
            .run_system_once(spawn_trade_ui)
            .expect("spawn_trade_ui runs");

        let world = app.world_mut();
        let invite_z = world
            .query_filtered::<&GlobalZIndex, With<InviteRoot>>()
            .single(world)
            .expect("InviteRoot exists")
            .0;
        assert_eq!(invite_z, zlayer::MODAL_WINDOWS);

        let trade_z = world
            .query_filtered::<&GlobalZIndex, With<TradeWindowRoot>>()
            .single(world)
            .expect("TradeWindowRoot exists")
            .0;
        assert_eq!(trade_z, zlayer::MODAL_WINDOWS);
    }

    #[test]
    fn offer_entry_renders_a_short_icon_and_quantity() {
        let entry = NetTradeOfferEntry {
            slot: InvSlotId::new(0, 1),
            item: NetItemStack {
                item_id: ItemDefinitionIdOwned::Simple(
                    "common.items.consumable.potion_minor".to_owned(),
                ),
                name: "Minor Potion".to_owned(),
                amount: 10,
                quality: Quality::Common,
                is_two_handed: false,
                // A potion is non-equippable, matching the field's own
                // documented `[]` default for that case.
                equippable_slots: Vec::new(),
                icon_key: common::comp::inventory::item::item_key::ItemKey::Simple(
                    "common.items.consumable.potion_minor".to_owned(),
                ),
            },
            offered: 3,
            owned: 10,
        };
        let contents = offer_entry_to_slot_contents(&entry);
        assert_eq!(contents.icon_text, "Min");
        assert_eq!(contents.quantity, Some(3));
        assert!(contents.tooltip.contains("Minor Potion"));
        assert!(contents.tooltip.contains("3 of 10"));
    }

    /// BL-82 EM-5.16 (T56.44 follow-up): switching the active locale
    /// re-localizes the already-spawned "Accept"/"Decline" trade-window
    /// buttons AND the static "Trading with player" header label live, using
    /// the REAL repo `.ftl` catalogs (not a synthetic fixture) via
    /// `VELOREN_ASSETS`/`XINDELER_ASSETS` — the same idiom
    /// `esc_menu.rs`'s `switching_locale_relocalizes_the_quit_button_live`
    /// test uses, covering both halves of the reactive chain
    /// (`LocalizedLabel`-tagged buttons via `relocalize_button_labels` +
    /// `button::spawn_button_labels`, and a bare `LocalizedText` node via
    /// `relocalize_text`) for this screen.
    #[test]
    fn switching_locale_relocalizes_the_trade_window_live() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &["common.ftl", "hud/trade.ftl"],
        ));
        app.init_resource::<xindeler_ui::i18n::CurrentLocale>();
        app.add_systems(Update, xindeler_ui::button::spawn_button_labels);

        app.world_mut()
            .run_system_once(spawn_trade_ui)
            .expect("spawn_trade_ui runs");
        app.update(); // let spawn_button_labels give each button its child

        fn accept_trade_button_text(app: &mut App) -> String {
            let world = app.world_mut();
            let button = world
                .query::<(&LocalizedLabel, &Children)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "hud-trade-accept")
                .map(|(_, children)| children[0])
                .expect("the Accept Trade button was spawned and tagged");
            world
                .get::<Text>(button)
                .expect("label child exists")
                .0
                .clone()
        }

        fn trading_with_player_label_text(app: &mut App) -> String {
            let world = app.world_mut();
            world
                .query::<(&LocalizedText, &Text)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "hud-trade-trading_with_player")
                .map(|(_, text)| text.0.clone())
                .expect("the trade-window header label was spawned and tagged")
        }

        assert_eq!(
            accept_trade_button_text(&mut app),
            "Accept",
            "the Accept Trade button must show the real en catalog text at spawn time"
        );
        assert_eq!(
            trading_with_player_label_text(&mut app),
            "Trading with player",
            "the header's static label must show the real en catalog text at spawn time"
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
            accept_trade_button_text(&mut app),
            "Aceptar",
            "must resolve to the REAL es-419 catalog's own hud-trade-accept value, not the en \
             fallback"
        );
        assert_eq!(
            trading_with_player_label_text(&mut app),
            "Comerciando con un jugador",
            "must resolve to the REAL es-419 catalog's own hud-trade-trading_with_player value, \
             not the en fallback"
        );
    }
}
