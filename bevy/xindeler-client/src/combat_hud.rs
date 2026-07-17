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
#[derive(Component)]
struct Crosshair;

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
    let health_orb = spawn_orb_bar(
        &mut commands,
        &theme,
        images.get(HudImageKey::HealthLiquid),
        Some(images.get(HudImageKey::OrbFrameAngel)),
        Some(hud_layout::ORB_SOURCE_CROP),
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
            node.bottom = Val::Px(hud_layout::CLUSTER_BOTTOM_PX);
            node.margin = UiRect::left(Val::Px(hud_layout::CLUSTER.health_orb_left));
        });

    let stamina_orb = spawn_orb_bar(
        &mut commands,
        &theme,
        images.get(HudImageKey::StaminaLiquid),
        Some(images.get(HudImageKey::OrbFrameStamina)),
        Some(hud_layout::ORB_SOURCE_CROP),
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
            node.bottom = Val::Px(hud_layout::CLUSTER_BOTTOM_PX);
            node.margin = UiRect::left(Val::Px(hud_layout::CLUSTER.stamina_orb_left));
        });

    let mana_orb = spawn_orb_bar(
        &mut commands,
        &theme,
        images.get(HudImageKey::ManaLiquid),
        Some(images.get(HudImageKey::OrbFrameCuthulhu)),
        Some(hud_layout::ORB_SOURCE_CROP),
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
            node.bottom = Val::Px(hud_layout::CLUSTER_BOTTOM_PX);
            node.margin = UiRect::left(Val::Px(hud_layout::CLUSTER.mana_orb_left));
        });

    // XP bar + the SINGLE canonical level readout, centred directly above the
    // action bar's "core" span (both halves + the Stamina orb — spec §3.1;
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
            margin: UiRect::left(Val::Px(-hud_layout::ACTION_BAR_TOTAL_WIDTH_PX / 2.0)),
            width: Val::Px(hud_layout::ACTION_BAR_TOTAL_WIDTH_PX),
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
        hud_layout::ACTION_BAR_TOTAL_WIDTH_PX,
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
    commands.spawn((
        DamageVignette,
        GlobalZIndex(zlayer::MODAL_WINDOWS),
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
        *value = BarValue::new(health.current, health.max);
    }
    if let Some(energy) = energy
        && let Ok(mut value) = energy_bars.single_mut()
    {
        *value = BarValue::new(energy.current, energy.max);
    }
    if let Some(poise) = poise
        && let Ok(mut value) = poise_bars.single_mut()
    {
        *value = BarValue::new(poise.current, poise.max);
    }
    if let Some(xp) = xp {
        if let Ok(mut value) = xp_bars.single_mut() {
            *value = BarValue::new(xp.xp_into_level as f32, xp.xp_for_level.max(1) as f32);
        }
        if let Ok(mut text) = level_texts.single_mut() {
            text.0 = format!("Lv. {}", xp.level);
        }
    }
    if let Some(combo) = combo
        && let Ok(mut text) = combo_texts.single_mut()
    {
        text.0 = if combo.counter > 0 {
            format!("{}x combo", combo.counter)
        } else {
            String::new()
        };
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
    /// registration, no window/GPU needed just to allocate `Handle<Image>`s).
    fn new_app_with_images() -> App {
        let mut app = new_app();
        app.add_plugins(AssetPlugin::default());
        app.add_plugins(ImagePlugin::default());
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
        let non_zero_radius = bevy::ui::BorderRadius::all(Val::Px(HudTheme::default().radius.sm));

        let health = node_of::<HealthBarTag>(app.world_mut());
        assert_eq!(
            health.width,
            Val::Px(hud_layout::ORB_SIZE_PX),
            "health orb must keep spawn_orb_bar's width, not collapse to Auto"
        );
        assert_eq!(health.height, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(health.overflow, clip);
        assert_eq!(health.border_radius, non_zero_radius);
        assert_eq!(health.position_type, PositionType::Absolute);
        assert_eq!(health.left, hud_layout::CENTER_LEFT);
        assert_eq!(health.bottom, Val::Px(hud_layout::CLUSTER_BOTTOM_PX));
        assert_eq!(
            health.margin.left,
            Val::Px(hud_layout::CLUSTER.health_orb_left)
        );

        let poise = node_of::<PoiseBarTag>(app.world_mut());
        assert_eq!(poise.width, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(poise.height, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(poise.overflow, clip);
        assert_eq!(poise.border_radius, non_zero_radius);
        assert_eq!(
            poise.margin.left,
            Val::Px(hud_layout::CLUSTER.stamina_orb_left)
        );

        let energy = node_of::<EnergyBarTag>(app.world_mut());
        assert_eq!(energy.width, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(energy.height, Val::Px(hud_layout::ORB_SIZE_PX));
        assert_eq!(energy.overflow, clip);
        assert_eq!(energy.border_radius, non_zero_radius);
        assert_eq!(
            energy.margin.left,
            Val::Px(hud_layout::CLUSTER.mana_orb_left)
        );

        let xp = node_of::<XpBarTag>(app.world_mut());
        // The XP bar now spans the action bar's "core" width (both halves +
        // the Stamina orb), not the old full-screen `Percent(100.0)` — but
        // height/overflow/radius must still survive from `spawn_bar`.
        assert_eq!(xp.width, Val::Px(hud_layout::ACTION_BAR_TOTAL_WIDTH_PX));
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

    /// A resource orb spawned through this screen's own real
    /// `spawn_orb_bar` call site (same image handles/size/crop
    /// `spawn_combat_hud` uses for the health orb) carries the correct
    /// [`BarValue`] and a real fraction-reveal CLIP WINDOW
    /// (`HudOrbBarFillClip`) whose height reflects the fraction it was
    /// spawned with — mirrors `xindeler_ui::bar`'s own
    /// `orb_bar_fill_tracks_value_changes_by_height` acceptance bar.
    /// Updated for the BL-82 EM-5.17 Phase 0 follow-up clip-reveal rework:
    /// the fraction now lives on the clip WRAPPER (a direct child of the
    /// orb), not the liquid image itself (`HudOrbBarFill`, now nested one
    /// level deeper inside that wrapper) — this test also pins the liquid
    /// image's `Node` to a FIXED `Val::Px` matching the orb's full size, the
    /// regression this screen's own real call site must never reintroduce
    /// (Matías's "shrinks instead of drains" report). This tests the
    /// SPAWN-time fraction (not a later `BarValue` mutation): the system
    /// that resizes the clip window on a LATER change
    /// (`xindeler_ui::bar::update_orb_bars`) is `pub(crate)` to
    /// `xindeler-ui` and already covered by that crate's own tests; from
    /// `xindeler-client` the observable contract is "the orb this screen
    /// spawns is a real `spawn_orb_bar` at the value/crop it's given," which
    /// this asserts directly.
    #[test]
    fn health_orb_spawns_with_correct_value_and_half_height_fill() {
        let mut app = new_app_with_images();
        let theme = HudTheme::default();
        let images = app.world().resource::<HudImages>().clone();

        let health_orb_entity = {
            let mut commands = app.world_mut().commands();
            let id = spawn_orb_bar(
                &mut commands,
                &theme,
                images.get(HudImageKey::HealthLiquid),
                Some(images.get(HudImageKey::OrbFrameAngel)),
                Some(hud_layout::ORB_SOURCE_CROP),
                hud_layout::ORB_SIZE_PX,
                hud_layout::ORB_SIZE_PX,
                BarValue::new(50.0, 100.0),
            );
            app.world_mut().flush();
            id
        };
        app.update();

        assert_eq!(
            *app.world().get::<BarValue>(health_orb_entity).unwrap(),
            BarValue::new(50.0, 100.0)
        );

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(health_orb_entity)
            .expect("the orb has clip-window/frame children")
            .iter()
            .collect();
        let clip_entity = children
            .into_iter()
            .find(|&e| {
                app.world()
                    .get::<xindeler_ui::bar::HudOrbBarFillClip>(e)
                    .is_some()
            })
            .expect("a HudOrbBarFillClip child exists");
        assert_eq!(
            app.world().get::<Node>(clip_entity).unwrap().height,
            Val::Percent(50.0)
        );

        let clip_children: Vec<Entity> = app
            .world()
            .get::<Children>(clip_entity)
            .expect("the clip window has a fill-image grandchild")
            .iter()
            .collect();
        let fill_entity = clip_children
            .into_iter()
            .find(|&e| {
                app.world()
                    .get::<xindeler_ui::bar::HudOrbBarFill>(e)
                    .is_some()
            })
            .expect("a HudOrbBarFill grandchild exists");
        let fill_node = app.world().get::<Node>(fill_entity).unwrap();
        assert_eq!(
            (fill_node.width, fill_node.height),
            (
                Val::Px(hud_layout::ORB_SIZE_PX),
                Val::Px(hud_layout::ORB_SIZE_PX)
            ),
            "the liquid image must stay at the orb's FULL fixed size, never the shrinking \
             fraction — this is the regression this screen's real call site must never reintroduce"
        );
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
