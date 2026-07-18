//! BL-82 EM-5.2 — the core combat HUD, the proof slice for the EM-5.1
//! widget kit + the new HUD state-mirror pattern (spec §3.2/§6).
//!
//! Reads REAL mirrored sim state off the client's own [`NetLocalPlayer`]
//! entity — `NetHealth` (already mirrored since EM-3.7) plus the FIVE new
//! EM-5.2 components `xindeler-sim-bridge::combat_hud` projects:
//! `NetEnergy`/`NetPoise`/`NetCombo`/`NetXp`/`NetBuffs`. No mocked/hardcoded
//! values — every bar/number on screen tracks the real sim entity through the
//! replicated mirror, exactly like `entity_view`'s figures already do for
//! position/health.
//!
//! v1 scope (this PR): health/energy/poise globes, XP bar + level, combo
//! counter, buff/debuff strip (icons = themed colour swatches + hover
//! tooltip — real `.vox`/icon-atlas art is a follow-up, EM-5.1's own doc
//! comment defers the icon-atlas primitive to whichever screen needs it
//! first), a crosshair, the death/respawn screen + a low-health red vignette,
//! and an in-world overhead health bar over every OTHER mirrored entity
//! (reusing the already-mirrored `NetHealth` — no new mirror needed for
//! that part). Floating combat text is DEFERRED (see module-end note): it
//! needs `common::Outcome` to reach the client, which nothing mirrors yet —
//! flagged as a real follow-up, not silently skipped.
//!
//! Compiled only under `listen-server`/`net-client` (the only modes where
//! `xindeler-protocol`'s Net* components are even linked), matching every
//! other consumer module in this crate (`entity_view`, `hud_toast`, ...).

use std::collections::HashMap;

use bevy::{prelude::*, ui::GlobalZIndex};
use xindeler_protocol::{
    NetBuffs, NetCombo, NetEnergy, NetHealth, NetLocalPlayer, NetPoise, NetXp,
};
use xindeler_ui::{
    bar::{BarValue, spawn_bar, spawn_orb_bar},
    button::button_bundle,
    images::{HudImageKey, HudImages},
    orb_material::OrbLiquidMaterial,
    theme::{HudFonts, HudTheme},
    tooltip::Tooltip,
    zlayer,
};

use crate::hud_layout;

/// Marks the health bar's container entity (so [`sync_local_player_bars`]
/// can update its [`BarValue`] without re-querying by position every frame).
#[derive(Component)]
struct HealthBarTag;
#[derive(Component)]
struct EnergyBarTag;
#[derive(Component)]
struct PoiseBarTag;
#[derive(Component)]
struct XpBarTag;

/// The combo/level readout text nodes.
#[derive(Component)]
struct ComboText;
#[derive(Component)]
struct LevelText;

/// The death/respawn screen root (toggled visible when the local player's
/// `NetHealth::current <= 0.0`).
#[derive(Component)]
struct DeathScreenRoot;

/// The full-screen low-health vignette overlay.
#[derive(Component)]
struct DamageVignette;

/// The crosshair marker (a small always-on centred reticle).
///
/// `pub(crate)` (BL-82 EM-5.12): the settings window's Interface tab drives
/// its live [`Visibility`] from `XindelerSettings::interface.show_crosshair`
/// (`settings_window::sync_crosshair_visibility`) — the reticle's spawn/layout
/// stays owned here; only the toggle reads this marker.
#[derive(Component)]
pub(crate) struct Crosshair;

/// One spawned buff-strip icon slot, tagged with which [`common::comp::buff::
/// BuffKind`] index (`kinds` array position, EM-5.2's `NetBuffs` ordering) it
/// currently displays — [`sync_buff_strip`] reconciles the strip's children
/// against the current `NetBuffs` list each time it changes, reusing
/// existing icon entities where possible instead of despawn/respawn churn.
#[derive(Component)]
struct BuffIconSlot;
#[derive(Component)]
struct BuffStripRoot;

/// Installs the whole core combat HUD: spawns the always-on widgets at
/// `Startup` and keeps them synced to the mirrored [`NetLocalPlayer`] state
/// every frame.
pub struct CombatHudViewPlugin;

impl Plugin for CombatHudViewPlugin {
    fn build(&self, app: &mut App) {
        // BL-82 EM-5.4 (bevy-migration-reviewer BLOCKER finding): `XindelerUiPlugin`
        // doesn't override `is_unique()` (defaults `true`), so adding it twice in
        // the same App panics ("plugin was already added in application"). This
        // crate now has MULTIPLE view plugins that each want `XindelerUiPlugin`
        // present (`chat::ChatViewPlugin` is the other one) — an UNCONDITIONAL add
        // here only "worked" by accident of registration ORDER (this plugin
        // happened to be added first in every real shell); guarding it the same
        // way `chat::ChatViewPlugin` and `XindelerUiPlugin`'s own inner
        // `UiWidgetsPlugins` add already do makes it order-independent instead of
        // a landmine for the next view plugin/reordering.
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        app.add_systems(
            Startup,
            spawn_combat_hud
                .after(xindeler_ui::theme::init_theme)
                .after(xindeler_ui::images::init_images),
        )
        .add_systems(
            Update,
            (
                sync_local_player_bars,
                sync_buff_strip,
                sync_death_screen_and_vignette,
                handle_respawn_button,
                overhead_health_bars,
            ),
        );
    }
}

/// Spawns every always-on combat HUD element: the bottom-centre 3-orb
/// resource cluster (health/stamina/mana) + the 2-piece action bar's flanking
/// XP bar/level readout (spec §3.1, BL-82 EM-5.17 Phase 2 — replaces the
/// former top-left stacked health/energy/poise bars and the former top-right
/// `LevelText`, closing Bug B), a combo readout, an empty buff strip (filled
/// in by [`sync_buff_strip`] once real buffs arrive), a crosshair, the
/// (initially hidden) death screen, and the (initially transparent) damage
/// vignette.
fn spawn_combat_hud(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
    mut orb_materials: ResMut<Assets<OrbLiquidMaterial>>,
) {
    // Bottom-centre resource-orb cluster (spec §3.1), left to right: Health
    // (angel frame) — Stamina (a genuine full orb, dead centre, mirrors
    // `NetPoise`) — Mana (cuthulhu frame, mirrors `NetEnergy`). Horizontal
    // placement comes from `crate::hud_layout::CLUSTER`, the SAME arithmetic
    // `hotbar.rs`'s two action-bar-half backgrounds use, so the two
    // independent plugins render as one contiguous row (see that module's
    // doc comment). `Some(hud_layout::ORB_SOURCE_CROP)` fixes the squashed-
    // ellipse sizing bug (BL-82 EM-5.17 Phase 0 follow-up) by cropping each
    // source PNG's wide canvas down to the square sub-region that actually
    // holds the circular art before it's stretched onto this square orb box.
    // The per-variant `hud_layout::*_FRAME_SOURCE_CROP` (frame crop) +
    // `hud_layout::*_LIQUID_INSET_PX` (liquid inset) constants fix the
    // follow-up "liquid sits smaller than the frame's window" sizing
    // mismatch (BL-82 EM-5.17 Phase 0 SECOND follow-up) AND, since round 2,
    // the opposite "frame crop clips the decorative art" regression a single
    // SHARED crop caused — see those constants' own doc comments for why
    // this needs 3 different values per parameter instead of 1 shared one.
    // BL-82 orb crop round 3: round 2's square crop was STILL clipping the
    // angel/cuthulhu wings (geometrically unavoidable for a square box —
    // see `hud_layout::ANGEL_FRAME_SOURCE_CROP`'s own doc comment), so the
    // per-variant `hud_layout::*_FRAME_WIDTH_PX` constants now size each
    // frame overlay WIDER than `ORB_SIZE_PX` (via `spawn_orb_bar`'s new
    // `frame_width_px` parameter) instead of squeezing it into the orb's
    // own square hit-box.
    let health_orb = spawn_orb_bar(
        &mut commands,
        &theme,
        &mut orb_materials,
        images.get(HudImageKey::HealthLiquid),
        Some(images.get(HudImageKey::OrbFrameAngel)),
        Some(hud_layout::ORB_SOURCE_CROP),
        Some(hud_layout::ANGEL_FRAME_SOURCE_CROP),
        hud_layout::ANGEL_FRAME_WIDTH_PX,
        hud_layout::ANGEL_LIQUID_INSET_PX,
        hud_layout::ORB_SIZE_PX,
        hud_layout::ORB_SIZE_PX,
        BarValue::new(1.0, 1.0),
    );
    // NOTE (regression precedent, see the historical comment this replaced):
    // `.entry::<Node>().and_modify(..)` mutates the EXISTING `Node`
    // `spawn_orb_bar` just inserted, instead of a second `insert(Node { .. })`
    // that would REPLACE it wholesale and silently discard its
    // width/height/overflow/border_radius.
    commands
        .entity(health_orb)
        .insert((
            HealthBarTag,
            GlobalZIndex(zlayer::ORBS_ACTION_BAR_PARTY_MINIMAP),
        ))
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.left = hud_layout::CENTER_LEFT;
            // BL-82 HUD polish round 4 (issue 1): shift the container DOWN
            // by orb_frame_angel.png's own measured transparent bottom
            // margin so the real opaque art (not the bounding box) lands
            // flush with the screen's bottom edge — see
            // `hud_layout::CLUSTER_BOTTOM_PX`'s own doc comment for why
            // `CLUSTER_BOTTOM_PX` alone can't fix this.
            node.bottom =
                Val::Px(hud_layout::CLUSTER_BOTTOM_PX - hud_layout::ANGEL_FRAME_BOTTOM_PAD_PX);
            node.margin = UiRect::left(Val::Px(hud_layout::CLUSTER.health_orb_left));
        });

    let stamina_orb = spawn_orb_bar(
        &mut commands,
        &theme,
        &mut orb_materials,
        images.get(HudImageKey::StaminaLiquid),
        Some(images.get(HudImageKey::OrbFrameStamina)),
        Some(hud_layout::ORB_SOURCE_CROP),
        Some(hud_layout::STAMINA_FRAME_SOURCE_CROP),
        hud_layout::STAMINA_FRAME_WIDTH_PX,
        hud_layout::STAMINA_LIQUID_INSET_PX,
        hud_layout::ORB_SIZE_PX,
        hud_layout::ORB_SIZE_PX,
        BarValue::new(1.0, 1.0),
    );
    commands
        .entity(stamina_orb)
        .insert((
            PoiseBarTag,
            GlobalZIndex(zlayer::ORBS_ACTION_BAR_PARTY_MINIMAP),
        ))
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.left = hud_layout::CENTER_LEFT;
            // BL-82 HUD polish round 4 (issue 1) — see the health orb's own
            // comment above; stamina uses its OWN measured pad
            // (`orb_frame_stamina.png` has a different transparent margin
            // than the angel/cuthulhu frames).
            node.bottom =
                Val::Px(hud_layout::CLUSTER_BOTTOM_PX - hud_layout::STAMINA_FRAME_BOTTOM_PAD_PX);
            node.margin = UiRect::left(Val::Px(hud_layout::CLUSTER.stamina_orb_left));
        });

    let mana_orb = spawn_orb_bar(
        &mut commands,
        &theme,
        &mut orb_materials,
        images.get(HudImageKey::ManaLiquid),
        Some(images.get(HudImageKey::OrbFrameCuthulhu)),
        Some(hud_layout::ORB_SOURCE_CROP),
        Some(hud_layout::CUTHULHU_FRAME_SOURCE_CROP),
        hud_layout::CUTHULHU_FRAME_WIDTH_PX,
        hud_layout::CUTHULHU_LIQUID_INSET_PX,
        hud_layout::ORB_SIZE_PX,
        hud_layout::ORB_SIZE_PX,
        BarValue::new(1.0, 1.0),
    );
    commands
        .entity(mana_orb)
        .insert((
            EnergyBarTag,
            GlobalZIndex(zlayer::ORBS_ACTION_BAR_PARTY_MINIMAP),
        ))
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.left = hud_layout::CENTER_LEFT;
            // BL-82 HUD polish round 4 (issue 1) — see the health orb's own
            // comment above; the mana orb uses cuthulhu's own measured pad.
            node.bottom =
                Val::Px(hud_layout::CLUSTER_BOTTOM_PX - hud_layout::CUTHULHU_FRAME_BOTTOM_PAD_PX);
            node.margin = UiRect::left(Val::Px(hud_layout::CLUSTER.mana_orb_left));
        });

    // XP bar + the SINGLE canonical level readout, centred directly above the
    // "core" row span (both ability-slot rows + the Stamina orb — spec §3.1;
    // Bug B's fix removes the old top-right `LevelText` entirely, this is
    // the only level readout left on screen outside the diary). A column
    // flex container (not two independently-positioned nodes) so the level
    // text centres over the (fixed-width) XP bar via `AlignItems::Center`
    // rather than a guessed text-width offset.
    let xp_cluster_root = commands
        .spawn((GlobalZIndex(zlayer::ORBS_ACTION_BAR_PARTY_MINIMAP), Node {
            position_type: PositionType::Absolute,
            left: hud_layout::CENTER_LEFT,
            bottom: Val::Px(
                hud_layout::CLUSTER_BOTTOM_PX
                    + hud_layout::ORB_SIZE_PX
                    + hud_layout::XP_CLUSTER_GAP_PX,
            ),
            margin: UiRect::left(Val::Px(-hud_layout::CORE_ROW_WIDTH_PX / 2.0)),
            width: Val::Px(hud_layout::CORE_ROW_WIDTH_PX),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: Val::Px(theme.spacing.xs),
            ..Default::default()
        }))
        .with_children(|parent| {
            parent.spawn((
                LevelText,
                Text("Lv. 1".to_owned()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(18.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        })
        .id();

    let xp_bar = spawn_bar(
        &mut commands,
        &theme,
        theme.palette.xp,
        theme.palette.xp_bg,
        hud_layout::CORE_ROW_WIDTH_PX,
        6.0,
        BarValue::new(0.0, 1.0),
    );
    commands.entity(xp_bar).insert(XpBarTag);
    commands.entity(xp_cluster_root).add_child(xp_bar);

    // Combo readout (unaffected by the bottom-centre reskin — stays
    // top-right).
    commands.spawn((
        ComboText,
        Text(String::new()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.title.clone()),
            font_size: bevy::text::FontSize::Px(28.0),
            ..Default::default()
        },
        TextColor(theme.palette.combo),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(16.0),
            right: Val::Px(120.0),
            ..Default::default()
        },
    ));

    // Buff/debuff strip: an empty horizontal row `sync_buff_strip` fills.
    commands.spawn((BuffStripRoot, Node {
        position_type: PositionType::Absolute,
        top: Val::Px(90.0),
        left: Val::Px(16.0),
        column_gap: Val::Px(theme.spacing.xs),
        ..Default::default()
    }));

    // Crosshair: a small centred dot.
    commands.spawn((
        Crosshair,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(50.0),
            left: Val::Percent(50.0),
            width: Val::Px(4.0),
            height: Val::Px(4.0),
            margin: UiRect::all(Val::Px(-2.0)),
            ..Default::default()
        },
        BackgroundColor(theme.palette.text),
    ));

    // Death/respawn screen: hidden until NetHealth hits zero. MODAL_WINDOWS
    // z-tier (not just "spawned after the orbs/action bar," which stopped
    // being a reliable ordering guarantee once those bars gained their own
    // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP) in this same phase — a
    // node with a GlobalZIndex sorts as an independent stack partition, so
    // an un-indexed sibling can end up BELOW it regardless of spawn order).
    let respawn_button = button_bundle(&theme, &fonts, "Respawn");
    commands
        .spawn((
            DeathScreenRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::MODAL_WINDOWS),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(theme.spacing.md),
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text("You have died".to_owned()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.title.clone()),
                    font_size: bevy::text::FontSize::Px(42.0),
                    ..Default::default()
                },
                TextColor(theme.palette.danger),
            ));
            parent.spawn(respawn_button).observe(
                |_activate: On<xindeler_ui::button::Activate>,
                 mut actions: MessageWriter<xindeler_ui::hud_state::HudAction>| {
                    actions.write(xindeler_ui::hud_state::HudAction::Respawn);
                },
            );
        });

    // Low-health damage vignette: a full-screen overlay, alpha driven by
    // `sync_death_screen_and_vignette` from `1.0 - health_fraction`. Same
    // MODAL_WINDOWS tier as the death screen it shares this spawn function
    // with — a near-death warning is a crisis-state overlay, not a normal
    // HUD panel, and must stay visible over the orb cluster/action bar
    // rather than tinting underneath them.
    //
    // `Pickable::IGNORE` is LOAD-BEARING, not cosmetic: this node is spawned
    // `Visibility::Visible` (only its alpha is 0 at full health) and, since
    // the EM-5.17 z scheme, carries `GlobalZIndex(MODAL_WINDOWS)` = 100. Bevy's
    // UI picking backend (`bevy_ui::picking_backend`) hit-tests on geometry
    // ALONE (a transparent background still picks), treats a node WITHOUT a
    // `Pickable` as *blocking* everything below it, and resolves the highest
    // z-partition first. Without this, the vignette sat above the ESC menu and
    // inventory window (both un-indexed → z-partition 0) and silently swallowed
    // EVERY click — no button anywhere in the HUD responded (Matías, live).
    // `IGNORE` (`should_block_lower: false`) makes picks pass straight through
    // to the panel beneath, matching the hotbar/boss-nameplate/social
    // full-screen overlays that already opt out this way.
    commands.spawn((
        DamageVignette,
        GlobalZIndex(zlayer::MODAL_WINDOWS),
        bevy::picking::Pickable::IGNORE,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..Default::default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.0)),
    ));
}

/// Reads the local player's mirrored `NetHealth`/`NetEnergy`/`NetPoise`/
/// `NetXp`/`NetCombo` and writes each into its bar's `BarValue`
/// (`xindeler_ui::bar::update_bars` does the actual fill-width math) plus the
/// combo/level text. Degrades clean (no panic, bars just keep their last
/// value) when the local player's mirror hasn't arrived yet — spec §3.2's
/// "a screen with no mirror data yet renders empty/loading, never panics".
///
/// Diffs before writing each `BarValue`/`Text` — this runs every frame (it's
/// a plain `Update` system, not gated on `Changed<NetHealth>` etc.), and an
/// unconditional `*value = ...`/`text.0 = ...` would mark the component
/// `Changed` even when the value is identical to last frame, defeating
/// `bar.rs`'s downstream `Changed<BarValue>`-gated fill-width update and
/// forcing a text-layout re-measure every single frame regardless of whether
/// the mirrored stats actually ticked — the same bug `boss_nameplate.rs`'s
/// `sync_nameplate_content` had before its BL-82 EM-5.17 Phase 5 fix (this
/// function follows that same diff-before-write discipline), and the same
/// convention `hotbar.rs`'s `sync_cooldown_overlays` (`if text.0 !=
/// countdown_text`) already uses elsewhere in this crate.
#[allow(clippy::too_many_arguments)]
fn sync_local_player_bars(
    player: Query<
        (
            Option<&NetHealth>,
            Option<&NetEnergy>,
            Option<&NetPoise>,
            Option<&NetXp>,
            Option<&NetCombo>,
        ),
        With<NetLocalPlayer>,
    >,
    mut health_bars: Query<
        &mut BarValue,
        (
            With<HealthBarTag>,
            Without<EnergyBarTag>,
            Without<PoiseBarTag>,
            Without<XpBarTag>,
        ),
    >,
    mut energy_bars: Query<
        &mut BarValue,
        (
            With<EnergyBarTag>,
            Without<HealthBarTag>,
            Without<PoiseBarTag>,
            Without<XpBarTag>,
        ),
    >,
    mut poise_bars: Query<
        &mut BarValue,
        (
            With<PoiseBarTag>,
            Without<HealthBarTag>,
            Without<EnergyBarTag>,
            Without<XpBarTag>,
        ),
    >,
    mut xp_bars: Query<
        &mut BarValue,
        (
            With<XpBarTag>,
            Without<HealthBarTag>,
            Without<EnergyBarTag>,
            Without<PoiseBarTag>,
        ),
    >,
    mut combo_texts: Query<&mut Text, (With<ComboText>, Without<LevelText>)>,
    mut level_texts: Query<&mut Text, (With<LevelText>, Without<ComboText>)>,
) {
    let Ok((health, energy, poise, xp, combo)) = player.single() else {
        return;
    };

    if let Some(health) = health
        && let Ok(mut value) = health_bars.single_mut()
    {
        let new_value = BarValue::new(health.current, health.max);
        if *value != new_value {
            *value = new_value;
        }
    }
    if let Some(energy) = energy
        && let Ok(mut value) = energy_bars.single_mut()
    {
        let new_value = BarValue::new(energy.current, energy.max);
        if *value != new_value {
            *value = new_value;
        }
    }
    if let Some(poise) = poise
        && let Ok(mut value) = poise_bars.single_mut()
    {
        let new_value = BarValue::new(poise.current, poise.max);
        if *value != new_value {
            *value = new_value;
        }
    }
    if let Some(xp) = xp {
        if let Ok(mut value) = xp_bars.single_mut() {
            let new_value = BarValue::new(xp.xp_into_level as f32, xp.xp_for_level.max(1) as f32);
            if *value != new_value {
                *value = new_value;
            }
        }
        if let Ok(mut text) = level_texts.single_mut() {
            let new_text = format!("Lv. {}", xp.level);
            if text.0 != new_text {
                text.0 = new_text;
            }
        }
    }
    if let Some(combo) = combo
        && let Ok(mut text) = combo_texts.single_mut()
    {
        let new_text = if combo.counter > 0 {
            format!("{}x combo", combo.counter)
        } else {
            String::new()
        };
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

/// Reconciles the buff strip's icon children against the local player's
/// current `NetBuffs` list: one themed colour-swatch icon per active buff
/// kind (real `.vox`/icon-atlas art is a follow-up — EM-5.1's own doc
/// comment names the icon-atlas primitive as not-yet-built), tagged with a
/// [`Tooltip`] showing the buff kind + strength + remaining seconds. Rebuilds
/// the strip only when `NetBuffs` actually changes (`Changed<NetBuffs>`), not
/// every frame.
fn sync_buff_strip(
    mut commands: Commands,
    theme: Res<HudTheme>,
    player: Query<&NetBuffs, (With<NetLocalPlayer>, Changed<NetBuffs>)>,
    strip: Query<(Entity, Option<&Children>), With<BuffStripRoot>>,
    icons: Query<Entity, With<BuffIconSlot>>,
) {
    let Ok(buffs) = player.single() else {
        return;
    };
    let Ok((strip_entity, children)) = strip.single() else {
        return;
    };

    // Despawn the previous icon set (simple v1: rebuild on every change —
    // buff churn is rare relative to per-frame cost, and the strip is tiny).
    if let Some(children) = children {
        for child in children.iter() {
            if icons.get(child).is_ok() {
                commands.entity(child).despawn();
            }
        }
    }

    commands.entity(strip_entity).with_children(|parent| {
        for entry in &buffs.0 {
            let tint = if entry.kind.is_buff() {
                theme.palette.buff_good
            } else {
                theme.palette.buff_bad
            };
            let remaining = entry
                .remaining_secs
                .map_or_else(|| "∞".to_owned(), |secs| format!("{secs:.0}s"));
            let tooltip_text = format!(
                "{:?} ×{} — {:.1} ({remaining})",
                entry.kind, entry.stacks, entry.strength
            );
            parent.spawn((
                BuffIconSlot,
                Node {
                    width: Val::Px(24.0),
                    height: Val::Px(24.0),
                    border: UiRect::all(Val::Px(2.0)),
                    ..Default::default()
                },
                BackgroundColor(tint),
                bevy::ui::BorderColor::all(theme.palette.panel_border),
                bevy::picking::hover::Hovered(false),
                Tooltip { text: tooltip_text },
            ));
        }
    });
}

/// Shows the death screen + fades the low-health vignette in as the local
/// player's `NetHealth` drops, using only the ALREADY-mirrored `NetHealth`
/// (no new mirror needed for this part).
fn sync_death_screen_and_vignette(
    player: Query<&NetHealth, With<NetLocalPlayer>>,
    mut death_screen: Query<&mut Visibility, With<DeathScreenRoot>>,
    mut vignette: Query<&mut BackgroundColor, With<DamageVignette>>,
) {
    let Ok(health) = player.single() else {
        return;
    };
    let fraction = if health.max > 0.0 {
        (health.current / health.max).clamp(0.0, 1.0)
    } else {
        0.0
    };

    if let Ok(mut visibility) = death_screen.single_mut() {
        *visibility = if health.current <= 0.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    // Vignette ramps in only below 30% health, matching legacy's "hurt"
    // threshold shape (a hard cutoff below full health would be distracting
    // at, say, 95%).
    const VIGNETTE_THRESHOLD: f32 = 0.3;
    if let Ok(mut color) = vignette.single_mut() {
        let severity = ((VIGNETTE_THRESHOLD - fraction) / VIGNETTE_THRESHOLD).clamp(0.0, 1.0);
        color.0 = Color::srgba(0.5, 0.0, 0.0, severity * 0.6);
    }
}

/// Drains [`xindeler_ui::hud_state::HudAction::Respawn`] (currently just
/// logs — routed through the crate's own generic HUD→action flow, per
/// bevy-migration-reviewer feedback, rather than a bespoke one-off message:
/// the client→server respawn WIRE message is a follow-up, see the module doc
/// comment's "Deliberately not implemented" note). Ignores every other
/// `HudAction` variant — a future settings/window-toggle screen owns those.
fn handle_respawn_button(mut events: MessageReader<xindeler_ui::hud_state::HudAction>) {
    for action in events.read() {
        if matches!(action, xindeler_ui::hud_state::HudAction::Respawn) {
            info!("combat_hud: respawn requested (client→server respawn message not yet wired)");
        }
    }
}

/// Marks an in-world overhead health bar's container (spawned/despawned by
/// [`overhead_health_bars`] as remote entities enter/leave the mirror).
#[derive(Component)]
struct OverheadHealthBar;

const OVERHEAD_BAR_WIDTH: f32 = 40.0;
const OVERHEAD_BAR_HEIGHT: f32 = 6.0;
/// How far above the entity's `GlobalTransform` origin (world units) the bar
/// floats — roughly head height for a humanoid capsule.
const OVERHEAD_BAR_WORLD_OFFSET: f32 = 2.2;

/// In-world overhead health bars over every OTHER mirrored entity (never the
/// local player, which has its own HUD globe) — the `ingame` widget analog
/// (spec §2 EM-5.2). Reuses the ALREADY-mirrored [`NetHealth`] (no new
/// mirror needed for this part) and the EM-5.1 [`spawn_bar`]/[`BarValue`]
/// primitive, projected to screen space each frame via
/// `Camera::world_to_viewport`. An entity that's behind the camera or has
/// left the mirror gets its bar hidden/despawned — never a stale floating
/// bar. `Local` state (not a `Resource`): this bookkeeping is private to this
/// one system, matching `xindeler-sim-bridge`'s own per-system scratch-state
/// convention (`MirrorScratch`) for the same "don't allocate a resource for
/// one system's private state" reasoning.
fn overhead_health_bars(
    mut commands: Commands,
    theme: Res<HudTheme>,
    camera: Query<(&Camera, &GlobalTransform), (With<Camera3d>, Without<NetHealth>)>,
    others: Query<(Entity, &NetHealth, &GlobalTransform), Without<NetLocalPlayer>>,
    mut bars: Query<(&mut Node, &mut BarValue, &mut Visibility), With<OverheadHealthBar>>,
    mut bar_of: Local<HashMap<Entity, Entity>>,
) {
    let Ok((cam, cam_transform)) = camera.single() else {
        return;
    };

    // Despawn bars whose source entity is no longer mirrored at all (left
    // view / died — `mirror_sim_entities` already despawns the mirror
    // entity itself in that case, so a plain presence check is enough).
    let mirrored_now: std::collections::HashSet<Entity> =
        others.iter().map(|(entity, ..)| entity).collect();
    bar_of.retain(|source, &mut bar_entity| {
        let still_mirrored = mirrored_now.contains(source);
        if !still_mirrored {
            commands.entity(bar_entity).despawn();
        }
        still_mirrored
    });

    // Update value + re-project screen position every frame — a remote
    // entity's position changes far more often than its health, so both
    // need a fresh look each frame regardless of which one triggered it.
    for (source, health, transform) in &others {
        let bar_entity = *bar_of.entry(source).or_insert_with(|| {
            spawn_bar(
                &mut commands,
                &theme,
                theme.palette.health,
                theme.palette.health_bg,
                OVERHEAD_BAR_WIDTH,
                OVERHEAD_BAR_HEIGHT,
                BarValue::new(health.current, health.max),
            )
        });
        let Ok((mut node, mut value, mut visibility)) = bars.get_mut(bar_entity) else {
            continue;
        };
        *value = BarValue::new(health.current, health.max);

        let world_head = transform.translation() + Vec3::Y * OVERHEAD_BAR_WORLD_OFFSET;
        match cam.world_to_viewport(cam_transform, world_head) {
            Ok(screen_pos) => {
                *visibility = Visibility::Inherited;
                node.position_type = PositionType::Absolute;
                node.left = Val::Px(screen_pos.x - OVERHEAD_BAR_WIDTH / 2.0);
                node.top = Val::Px(screen_pos.y - OVERHEAD_BAR_HEIGHT / 2.0);
            },
            // Behind the camera or otherwise unprojectable — hide rather
            // than leave it pinned at a stale/garbage screen position.
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce, image::ImagePlugin};
    use common::comp::buff::BuffKind;
    use xindeler_protocol::NetBuffEntry;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app
    }

    /// [`spawn_combat_hud`] now needs a real [`HudImages`] (the orb/action-bar
    /// art lookup) — built the same way `xindeler_ui::orb_material`'s own
    /// tests build a headless `AssetServer` (a real asset type + loader
    /// registration, no window/GPU needed just to allocate `Handle<Image>`s)
    /// — plus a real `Assets<OrbLiquidMaterial>` collection (BL-82 EM-5.17
    /// wave/stone-reveal rework: `spawn_orb_bar` now needs `ResMut<Assets<
    /// OrbLiquidMaterial>>` to `add` each orb's own material instance).
    fn new_app_with_images() -> App {
        let mut app = new_app();
        app.add_plugins(AssetPlugin::default());
        app.add_plugins(ImagePlugin::default());
        app.init_asset::<OrbLiquidMaterial>();
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app
    }

    /// Regression test for the bug a real play session hit (Matías, BL-82
    /// Phase 5 follow-up): only "Lv. 1" and one EMPTY gray rounded panel were
    /// visible — no health/energy/poise/XP fill, no crosshair content. Root
    /// cause: `spawn_combat_hud` did
    /// `commands.entity(bar).insert((Tag, Node { position_type, top, left,
    /// ..Default::default() }))` on an entity [`bar::spawn_bar`]/
    /// [`bar::spawn_orb_bar`] had ALREADY given a real `Node` (explicit
    /// width/height/`Overflow::clip()`/`border_radius`) — a second `insert`
    /// of the SAME component type REPLACES it wholesale (`Node` isn't merged
    /// field-by-field), so the `..Default::default()` silently discarded the
    /// bar's sizing. BL-82 EM-5.17 Phase 2 kept the `.entry::<Node>()
    /// .and_modify(..)` fix for the three resource orbs (this test's
    /// original regression target) and extended coverage to the new
    /// bottom-centre cluster's positioning + the XP bar's new width.
    #[test]
    fn spawn_combat_hud_keeps_every_bars_sizing_and_cluster_position_intact() {
        let mut app = new_app_with_images();
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });

        app.world_mut()
            .run_system_once(spawn_combat_hud)
            .expect("spawn_combat_hud runs");

        fn node_of<T: bevy::ecs::component::Component>(world: &mut World) -> Node {
            world
                .query_filtered::<&Node, With<T>>()
                .single(world)
                .expect("the tagged bar entity exists")
                .clone()
        }

        let clip = bevy::ui::Overflow::clip();
        // BL-82 orb crop round 3: the three orb containers now clip only the
        // `y` axis (`x` stays `Visible` so a `frame_width_px` wider than
        // `ORB_SIZE_PX` can spill past the container — see
        // `xindeler_ui::bar::spawn_orb_bar`'s own doc comment). `spawn_bar`'s
        // containers (the XP bar below) are untouched by this and still clip
        // both axes.
        let orb_clip = bevy::ui::Overflow::clip_y();
        let non_zero_radius = bevy::ui::BorderRadius::all(Val::Px(HudTheme::default().radius.sm));

        let health = node_of::<HealthBarTag>(app.world_mut());
        assert_eq!(
            health.width,
            Val::Px(hud_layout::ORB_SIZE_PX),
            "health orb must keep spawn_orb_bar's width, not collapse to Auto"
        );
        assert_eq!(health.height, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(health.overflow, orb_clip);
        assert_eq!(health.border_radius, non_zero_radius);
        assert_eq!(health.position_type, PositionType::Absolute);
        assert_eq!(health.left, hud_layout::CENTER_LEFT);
        // BL-82 HUD polish round 4 (issue 1): no longer bare
        // `CLUSTER_BOTTOM_PX` — each orb is shifted down by its OWN measured
        // transparent-bottom-margin pad so the real art (not the bounding
        // box) sits flush with the screen edge. See
        // `hud_layout::CLUSTER_BOTTOM_PX`'s doc comment for why.
        assert_eq!(
            health.bottom,
            Val::Px(hud_layout::CLUSTER_BOTTOM_PX - hud_layout::ANGEL_FRAME_BOTTOM_PAD_PX)
        );
        assert_eq!(
            health.margin.left,
            Val::Px(hud_layout::CLUSTER.health_orb_left)
        );

        let poise = node_of::<PoiseBarTag>(app.world_mut());
        assert_eq!(poise.width, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(poise.height, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(poise.overflow, orb_clip);
        assert_eq!(poise.border_radius, non_zero_radius);
        assert_eq!(
            poise.bottom,
            Val::Px(hud_layout::CLUSTER_BOTTOM_PX - hud_layout::STAMINA_FRAME_BOTTOM_PAD_PX)
        );
        assert_eq!(
            poise.margin.left,
            Val::Px(hud_layout::CLUSTER.stamina_orb_left)
        );

        let energy = node_of::<EnergyBarTag>(app.world_mut());
        assert_eq!(energy.width, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(energy.height, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(energy.overflow, orb_clip);
        assert_eq!(energy.border_radius, non_zero_radius);
        assert_eq!(
            energy.bottom,
            Val::Px(hud_layout::CLUSTER_BOTTOM_PX - hud_layout::CUTHULHU_FRAME_BOTTOM_PAD_PX)
        );
        assert_eq!(
            energy.margin.left,
            Val::Px(hud_layout::CLUSTER.mana_orb_left)
        );

        let xp = node_of::<XpBarTag>(app.world_mut());
        // The XP bar now spans the action bar's "core" width (both halves +
        // the Stamina orb), not the old full-screen `Percent(100.0)` — but
        // height/overflow/radius must still survive from `spawn_bar`.
        assert_eq!(xp.width, Val::Px(hud_layout::CORE_ROW_WIDTH_PX));
        assert_eq!(xp.height, Val::Px(6.0));
        assert_eq!(xp.overflow, clip);
        assert_eq!(xp.border_radius, non_zero_radius);
    }

    /// BL-82 EM-5.17 Phase 2 (Bug B follow-through): exactly ONE character-
    /// level readout node exists after `spawn_combat_hud` — the old
    /// top-right `LevelText` is gone, replaced by the single readout in the
    /// bottom-centre XP cluster. Guards against Bug B's "two Lv.1 texts on
    /// screen" regression recurring on the code side (the diary-visibility
    /// half of Bug B is Phase 0's fix, in a different file).
    #[test]
    fn spawn_combat_hud_creates_exactly_one_level_readout() {
        let mut app = new_app_with_images();
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });

        app.world_mut()
            .run_system_once(spawn_combat_hud)
            .expect("spawn_combat_hud runs");

        let count = app
            .world_mut()
            .query_filtered::<Entity, With<LevelText>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 1, "exactly one LevelText readout must exist");
    }

    /// Regression test for the total UI-interaction breakage Matías hit live
    /// (BL-82, after the EM-5.17 z-index scheme + EM-5.18 landed): NO button
    /// worked anywhere — the ESC pause menu opened but its options were
    /// unclickable, and inventory slots/buttons were dead too. Root cause: the
    /// always-present, fully-transparent [`DamageVignette`] full-screen overlay
    /// is spawned `Visibility::Visible` (only its alpha is 0) and, in the
    /// EM-5.17 z scheme, carries `GlobalZIndex(MODAL_WINDOWS)` = 100 — yet it
    /// has NO [`bevy::picking::Pickable`] component. Bevy's UI picking backend
    /// (`bevy_ui::picking_backend`) treats a node WITHOUT a `Pickable` as
    /// *blocking* the nodes below it, hit-tests on pure geometry (background
    /// alpha is irrelevant), and processes the highest z-partition first. The
    /// ESC menu and inventory window roots have NO `GlobalZIndex` (default
    /// z-partition 0), so they sit BELOW the vignette at z=100 — the vignette
    /// was the first hit under every click and the picking loop `break`ed on it
    /// before the panel's own buttons were ever considered. The vignette is a
    /// passive visual tint and must NEVER intercept picks: it carries
    /// `Pickable::IGNORE`, exactly like the other full-screen HUD overlays
    /// (hotbar backdrop, boss-nameplate container, social-panel backdrop).
    #[test]
    fn damage_vignette_ignores_picking_so_it_never_blocks_ui_clicks() {
        let mut app = new_app_with_images();
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });

        app.world_mut()
            .run_system_once(spawn_combat_hud)
            .expect("spawn_combat_hud runs");

        let vignette = app
            .world_mut()
            .query_filtered::<Entity, With<DamageVignette>>()
            .single(app.world())
            .expect("the damage vignette exists");
        let pickable = app.world().get::<bevy::picking::Pickable>(vignette);
        assert_eq!(
            pickable,
            Some(&bevy::picking::Pickable::IGNORE),
            "the transparent full-screen damage vignette must carry Pickable::IGNORE so it never \
             intercepts clicks meant for the ESC menu / inventory below it (bevy_ui picking \
             treats a node WITHOUT a Pickable as blocking every node beneath it)"
        );
    }

    /// A resource orb spawned through this screen's own real
    /// `spawn_orb_bar` call site (same image handles/size/crop
    /// `spawn_combat_hud` uses for the health orb) carries the correct
    /// [`BarValue`] and writes that same fraction onto its
    /// [`HudOrbBarFill`](xindeler_ui::bar::HudOrbBarFill) child's
    /// [`OrbLiquidMaterial::fill_fraction`] uniform — mirrors
    /// `xindeler_ui::bar`'s own
    /// `orb_bar_material_fraction_tracks_value_changes` acceptance bar.
    /// Reworked (BL-82 EM-5.17 wave/stone-reveal shader rework) from the
    /// old CPU-clip-window height assertion to a material uniform read,
    /// since the fraction (plus the wave/stone-reveal) is now entirely the
    /// shader's job. This test also pins the liquid layer's `Node` to a
    /// FIXED `Val::Px` matching the orb's full inset size, the regression
    /// this screen's own real call site must never reintroduce
    /// (Matías's original "shrinks instead of drains" report, which this
    /// primitive already fixed once before this shader rework). This tests
    /// the SPAWN-time fraction (not a later `BarValue` mutation): the system
    /// that writes a LATER change (`xindeler_ui::bar::update_orb_bars`) is
    /// `pub(crate)` to `xindeler-ui` and already covered by that crate's own
    /// tests; from `xindeler-client` the observable contract is "the orb
    /// this screen spawns is a real `spawn_orb_bar` at the value/crop it's
    /// given," which this asserts directly.
    #[test]
    fn health_orb_spawns_with_correct_value_and_half_fill_fraction() {
        let mut app = new_app_with_images();
        let theme = HudTheme::default();
        let images = app.world().resource::<HudImages>().clone();

        let health_orb_entity = app
            .world_mut()
            .resource_scope::<Assets<OrbLiquidMaterial>, _>(|world, mut materials| {
                let mut commands = world.commands();
                let id = spawn_orb_bar(
                    &mut commands,
                    &theme,
                    &mut materials,
                    images.get(HudImageKey::HealthLiquid),
                    Some(images.get(HudImageKey::OrbFrameAngel)),
                    Some(hud_layout::ORB_SOURCE_CROP),
                    Some(hud_layout::ANGEL_FRAME_SOURCE_CROP),
                    hud_layout::ANGEL_FRAME_WIDTH_PX,
                    hud_layout::ANGEL_LIQUID_INSET_PX,
                    hud_layout::ORB_SIZE_PX,
                    hud_layout::ORB_SIZE_PX,
                    BarValue::new(50.0, 100.0),
                );
                world.flush();
                id
            });
        app.update();

        assert_eq!(
            *app.world().get::<BarValue>(health_orb_entity).unwrap(),
            BarValue::new(50.0, 100.0)
        );

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(health_orb_entity)
            .expect("the orb has fill/frame children")
            .iter()
            .collect();
        let fill_entity = children
            .into_iter()
            .find(|&e| {
                app.world()
                    .get::<xindeler_ui::bar::HudOrbBarFill>(e)
                    .is_some()
            })
            .expect("a HudOrbBarFill child exists");

        let material_node = app
            .world()
            .get::<bevy::prelude::MaterialNode<OrbLiquidMaterial>>(fill_entity)
            .expect("the fill child carries a MaterialNode<OrbLiquidMaterial>");
        let materials = app.world().resource::<Assets<OrbLiquidMaterial>>();
        assert_eq!(
            materials.get(material_node).unwrap().fill_fraction,
            0.5,
            "the material's fill_fraction must match the spawn-time BarValue fraction"
        );

        let fill_node = app.world().get::<Node>(fill_entity).unwrap();
        let inset_size = hud_layout::ORB_SIZE_PX - 2.0 * hud_layout::ANGEL_LIQUID_INSET_PX;
        assert_eq!(
            (fill_node.width, fill_node.height),
            (Val::Px(inset_size), Val::Px(inset_size)),
            "the liquid image must stay at its FIXED inset size, never the shrinking fraction — \
             this is the regression this screen's real call site must never reintroduce"
        );
        assert_eq!(fill_node.left, Val::Px(hud_layout::ANGEL_LIQUID_INSET_PX));
        assert_eq!(fill_node.bottom, Val::Px(hud_layout::ANGEL_LIQUID_INSET_PX));
    }

    /// The EM-5.2 acceptance bar (spec §6): spawning a `NetLocalPlayer`
    /// mirror with real `NetHealth`/`NetEnergy`/`NetPoise`/`NetXp`/`NetCombo`
    /// and running `sync_local_player_bars` makes every HUD bar/text track
    /// those values — and CHANGING the mirrored health (as a real sim tick
    /// would) updates the bar again, proving this isn't a one-shot read.
    #[test]
    fn hud_bars_track_local_player_mirror_and_its_changes() {
        let mut app = new_app();

        let health_bar = app
            .world_mut()
            .spawn((HealthBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let energy_bar = app
            .world_mut()
            .spawn((EnergyBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let poise_bar = app
            .world_mut()
            .spawn((PoiseBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let xp_bar = app
            .world_mut()
            .spawn((XpBarTag, BarValue::new(0.0, 1.0)))
            .id();
        let combo_text = app.world_mut().spawn((ComboText, Text(String::new()))).id();
        let level_text = app
            .world_mut()
            .spawn((LevelText, Text("Lv. 1".to_owned())))
            .id();

        let player = app
            .world_mut()
            .spawn((
                NetLocalPlayer,
                NetHealth {
                    current: 80.0,
                    max: 100.0,
                },
                NetEnergy {
                    current: 40.0,
                    max: 100.0,
                },
                NetPoise {
                    current: 5.0,
                    max: 10.0,
                },
                NetXp {
                    level: 3,
                    xp_into_level: 50,
                    xp_for_level: 200,
                },
                NetCombo { counter: 4 },
            ))
            .id();

        app.world_mut()
            .run_system_once(sync_local_player_bars)
            .expect("sync_local_player_bars runs");

        assert_eq!(
            *app.world().get::<BarValue>(health_bar).unwrap(),
            BarValue::new(80.0, 100.0)
        );
        assert_eq!(
            *app.world().get::<BarValue>(energy_bar).unwrap(),
            BarValue::new(40.0, 100.0)
        );
        assert_eq!(
            *app.world().get::<BarValue>(poise_bar).unwrap(),
            BarValue::new(5.0, 10.0)
        );
        assert_eq!(
            *app.world().get::<BarValue>(xp_bar).unwrap(),
            BarValue::new(50.0, 200.0)
        );
        assert_eq!(app.world().get::<Text>(level_text).unwrap().0, "Lv. 3");
        assert_eq!(app.world().get::<Text>(combo_text).unwrap().0, "4x combo");

        // A real sim tick would change the mirrored health (e.g. taking
        // damage) — verify the bar tracks the NEW value, not the one it
        // first saw.
        app.world_mut()
            .get_mut::<NetHealth>(player)
            .unwrap()
            .current = 30.0;
        app.world_mut()
            .run_system_once(sync_local_player_bars)
            .expect("sync_local_player_bars runs again");
        assert_eq!(
            *app.world().get::<BarValue>(health_bar).unwrap(),
            BarValue::new(30.0, 100.0),
            "the bar must track the CHANGED health, not the stale first reading"
        );
    }

    /// [`sync_local_player_bars`] must NOT re-mark its output components
    /// `Changed` on a frame where the local player's mirrored stats haven't
    /// actually changed — the same diff-before-write discipline
    /// `boss_nameplate.rs`'s `sync_nameplate_content` establishes (and this
    /// function now follows, per its own doc comment). This is a plain
    /// `Update` system (not gated on `Changed<NetHealth>` etc.), so it runs
    /// every frame regardless of whether the sim tick actually changed
    /// anything; an unconditional `*value = ...`/`text.0 = ...` write would
    /// mark the component `Changed` regardless of whether the value differs
    /// (`Mut::deref_mut`'s documented behaviour), defeating `bar.rs`'s
    /// downstream `Changed<BarValue>`-gated fill-width update and forcing a
    /// text-layout re-measure every single frame.
    ///
    /// Proven via the same `World::clear_trackers` baseline-reset idiom
    /// `boss_nameplate.rs`'s equivalent regression test uses: run once
    /// (establishes real, non-default content — trivially marks `Changed`),
    /// reset the tracking baseline, run again with the EXACT SAME mirrored
    /// data, then assert `is_changed()` reads `false` for every bar/text this
    /// system writes — i.e. the second run performed no redundant write.
    #[test]
    fn sync_local_player_bars_does_not_rewrite_unchanged_bars_or_text() {
        let mut app = new_app();

        let health_bar = app
            .world_mut()
            .spawn((HealthBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let energy_bar = app
            .world_mut()
            .spawn((EnergyBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let poise_bar = app
            .world_mut()
            .spawn((PoiseBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let xp_bar = app
            .world_mut()
            .spawn((XpBarTag, BarValue::new(0.0, 1.0)))
            .id();
        let combo_text = app.world_mut().spawn((ComboText, Text(String::new()))).id();
        let level_text = app
            .world_mut()
            .spawn((LevelText, Text("Lv. 1".to_owned())))
            .id();

        app.world_mut().spawn((
            NetLocalPlayer,
            NetHealth {
                current: 80.0,
                max: 100.0,
            },
            NetEnergy {
                current: 40.0,
                max: 100.0,
            },
            NetPoise {
                current: 5.0,
                max: 10.0,
            },
            NetXp {
                level: 3,
                xp_into_level: 50,
                xp_for_level: 200,
            },
            NetCombo { counter: 4 },
        ));

        // Run 1: establishes real content (definitely differs from the
        // spawn-time defaults above, so this run's writes legitimately mark
        // `Changed` — not the thing under test).
        app.world_mut()
            .run_system_once(sync_local_player_bars)
            .expect("first run succeeds");

        // Reset the tracking baseline so a SUBSEQUENT no-op write (if the
        // bug is present) is the only thing that could show up as `Changed`
        // below.
        app.world_mut().clear_trackers();

        // Run 2: same mirrored player data, nothing should change.
        app.world_mut()
            .run_system_once(sync_local_player_bars)
            .expect("second run succeeds");

        let world = app.world();
        assert!(
            !world
                .entity(health_bar)
                .get_ref::<BarValue>()
                .unwrap()
                .is_changed(),
            "health bar was rewritten even though its value didn't change"
        );
        assert!(
            !world
                .entity(energy_bar)
                .get_ref::<BarValue>()
                .unwrap()
                .is_changed(),
            "energy bar was rewritten even though its value didn't change"
        );
        assert!(
            !world
                .entity(poise_bar)
                .get_ref::<BarValue>()
                .unwrap()
                .is_changed(),
            "poise bar was rewritten even though its value didn't change"
        );
        assert!(
            !world
                .entity(xp_bar)
                .get_ref::<BarValue>()
                .unwrap()
                .is_changed(),
            "xp bar was rewritten even though its value didn't change"
        );
        assert!(
            !world
                .entity(combo_text)
                .get_ref::<Text>()
                .unwrap()
                .is_changed(),
            "combo text was rewritten even though its value didn't change"
        );
        assert!(
            !world
                .entity(level_text)
                .get_ref::<Text>()
                .unwrap()
                .is_changed(),
            "level text was rewritten even though its value didn't change"
        );
    }

    /// The death screen shows and the vignette reddens once the local
    /// player's mirrored `NetHealth` reaches zero — using ONLY the
    /// already-mirrored `NetHealth` (no new mirror needed).
    #[test]
    fn death_screen_shows_and_vignette_reddens_at_zero_health() {
        let mut app = new_app();
        let death_screen = app
            .world_mut()
            .spawn((DeathScreenRoot, Visibility::Hidden))
            .id();
        let vignette = app
            .world_mut()
            .spawn((DamageVignette, BackgroundColor(Color::NONE)))
            .id();
        app.world_mut().spawn((NetLocalPlayer, NetHealth {
            current: 0.0,
            max: 100.0,
        }));

        app.world_mut()
            .run_system_once(sync_death_screen_and_vignette)
            .expect("system runs");

        assert_eq!(
            *app.world().get::<Visibility>(death_screen).unwrap(),
            Visibility::Visible
        );
        let vignette_alpha = app
            .world()
            .get::<BackgroundColor>(vignette)
            .unwrap()
            .0
            .alpha();
        assert!(
            vignette_alpha > 0.0,
            "the vignette must redden at zero health, got alpha {vignette_alpha}"
        );
    }

    /// A full-health player keeps the death screen hidden and the vignette
    /// fully transparent.
    #[test]
    fn full_health_hides_death_screen_and_clears_vignette() {
        let mut app = new_app();
        let death_screen = app
            .world_mut()
            .spawn((DeathScreenRoot, Visibility::Hidden))
            .id();
        let vignette = app
            .world_mut()
            .spawn((DamageVignette, BackgroundColor(Color::NONE)))
            .id();
        app.world_mut().spawn((NetLocalPlayer, NetHealth {
            current: 100.0,
            max: 100.0,
        }));

        app.world_mut()
            .run_system_once(sync_death_screen_and_vignette)
            .expect("system runs");

        assert_eq!(
            *app.world().get::<Visibility>(death_screen).unwrap(),
            Visibility::Hidden
        );
        let vignette_alpha = app
            .world()
            .get::<BackgroundColor>(vignette)
            .unwrap()
            .0
            .alpha();
        assert_eq!(vignette_alpha, 0.0);
    }

    /// The buff strip spawns one icon per distinct active buff kind off the
    /// local player's real `NetBuffs` mirror, tagged with a [`Tooltip`]
    /// carrying the buff's kind/strength/duration.
    #[test]
    fn buff_strip_spawns_an_icon_per_active_buff() {
        let mut app = new_app();
        let strip = app.world_mut().spawn(BuffStripRoot).id();
        app.world_mut().spawn((
            NetLocalPlayer,
            NetBuffs(vec![NetBuffEntry {
                kind: BuffKind::Regeneration,
                strength: 3.0,
                remaining_secs: Some(9.5),
                stacks: 2,
            }]),
        ));

        app.world_mut()
            .run_system_once(sync_buff_strip)
            .expect("system runs");
        app.update();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(strip)
            .expect("the strip got a child icon")
            .iter()
            .collect();
        assert_eq!(children.len(), 1, "one icon per distinct active buff kind");
        assert!(app.world().get::<BuffIconSlot>(children[0]).is_some());
        let tooltip = app
            .world()
            .get::<Tooltip>(children[0])
            .expect("the icon carries a Tooltip");
        assert!(tooltip.text.contains("Regeneration"));
        assert!(tooltip.text.contains("9s") || tooltip.text.contains("10s"));
    }
}
