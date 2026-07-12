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

use bevy::prelude::*;
use xindeler_protocol::{
    NetBuffs, NetCombo, NetEnergy, NetHealth, NetLocalPlayer, NetPoise, NetXp,
};
use xindeler_ui::{
    bar::{BarValue, spawn_bar},
    button::button_bundle,
    theme::{HudFonts, HudTheme},
    tooltip::Tooltip,
};

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
            spawn_combat_hud.after(xindeler_ui::theme::init_theme),
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

/// Spawns every always-on combat HUD element: health/energy/poise/XP bars
/// (top-left, stacked), a combo/level readout, an empty buff strip (filled
/// in by [`sync_buff_strip`] once real buffs arrive), a crosshair, the
/// (initially hidden) death screen, and the (initially transparent) damage
/// vignette.
fn spawn_combat_hud(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    // Health/energy/poise/XP bars, stacked top-left.
    let health_bar = spawn_bar(
        &mut commands,
        &theme,
        theme.palette.health,
        theme.palette.health_bg,
        220.0,
        22.0,
        BarValue::new(1.0, 1.0),
    );
    // NOTE (regression fix): `.entry::<Node>().and_modify(..)` mutates the
    // EXISTING `Node` `spawn_bar` just inserted, instead of a second
    // `insert(Node { .. })` that would REPLACE it wholesale and silently
    // discard its width/height/overflow/border_radius — see this module's
    // `spawn_combat_hud_keeps_every_bars_sizing_from_spawn_bar_intact` test
    // for the full story and the exact symptom this regressed to.
    commands
        .entity(health_bar)
        .insert(HealthBarTag)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.top = Val::Px(16.0);
            node.left = Val::Px(16.0);
        });

    let energy_bar = spawn_bar(
        &mut commands,
        &theme,
        theme.palette.energy,
        theme.palette.energy_bg,
        220.0,
        14.0,
        BarValue::new(1.0, 1.0),
    );
    commands
        .entity(energy_bar)
        .insert(EnergyBarTag)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.top = Val::Px(42.0);
            node.left = Val::Px(16.0);
        });

    let poise_bar = spawn_bar(
        &mut commands,
        &theme,
        theme.palette.poise,
        theme.palette.poise_bg,
        220.0,
        8.0,
        BarValue::new(1.0, 1.0),
    );
    commands
        .entity(poise_bar)
        .insert(PoiseBarTag)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.top = Val::Px(60.0);
            node.left = Val::Px(16.0);
        });

    let xp_bar = spawn_bar(
        &mut commands,
        &theme,
        theme.palette.xp,
        theme.palette.xp_bg,
        220.0,
        6.0,
        BarValue::new(0.0, 1.0),
    );
    commands
        .entity(xp_bar)
        .insert(XpBarTag)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.position_type = PositionType::Absolute;
            node.bottom = Val::Px(0.0);
            node.left = Val::Px(0.0);
            node.width = Val::Percent(100.0);
        });

    // Combo counter + level readout (top-right).
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
    commands.spawn((
        LevelText,
        Text("Lv. 1".to_owned()),
        TextFont {
            font: bevy::text::FontSource::Handle(fonts.body.clone()),
            font_size: bevy::text::FontSize::Px(18.0),
            ..Default::default()
        },
        TextColor(theme.palette.text),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(16.0),
            right: Val::Px(16.0),
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

    // Death/respawn screen: hidden until NetHealth hits zero.
    let respawn_button = button_bundle(&theme, &fonts, "Respawn");
    commands
        .spawn((
            DeathScreenRoot,
            Visibility::Hidden,
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
    // `sync_death_screen_and_vignette` from `1.0 - health_fraction`.
    commands.spawn((
        DamageVignette,
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
    use bevy::ecs::system::RunSystemOnce;
    use common::comp::buff::BuffKind;
    use xindeler_protocol::NetBuffEntry;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app
    }

    /// Regression test for the bug a real play session hit (Matías, BL-82
    /// Phase 5 follow-up): only "Lv. 1" and one EMPTY gray rounded panel were
    /// visible — no health/energy/poise/XP fill, no crosshair content. Root
    /// cause: `spawn_combat_hud` did
    /// `commands.entity(bar).insert((Tag, Node { position_type, top, left,
    /// ..Default::default() }))` on an entity [`bar::spawn_bar`] had ALREADY
    /// given a real `Node` (explicit width/height/`Overflow::clip()`/
    /// `border_radius`) — a second `insert` of the SAME component type
    /// REPLACES it wholesale (`Node` isn't merged field-by-field), so the
    /// `..Default::default()` silently discarded the bar's sizing, collapsing
    /// every stat bar to a zero/auto-sized, invisible box while its sibling
    /// `BackgroundColor` panel-ish container was the only thing left visibly
    /// standing. This is exactly the "spawned but never actually renders"
    /// class of bug the EM-5.1/5.2 PR's own reviewers already caught ONCE
    /// (tooltip/notification widgets) — this was a second, unnoticed instance
    /// in the very next module, because NONE of this module's other tests
    /// call the real [`spawn_combat_hud`] (they all hand-build their own
    /// fixture entities, bypassing the buggy code path entirely). This test
    /// closes that gap: it calls `spawn_combat_hud` itself and asserts every
    /// stat bar kept `spawn_bar`'s sizing/overflow/radius intact alongside
    /// its position override.
    #[test]
    fn spawn_combat_hud_keeps_every_bars_sizing_from_spawn_bar_intact() {
        let mut app = new_app();
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
            Val::Px(220.0),
            "health bar must keep spawn_bar's width, not collapse to Auto"
        );
        assert_eq!(health.height, Val::Px(22.0));
        assert_eq!(health.overflow, clip);
        assert_eq!(health.border_radius, non_zero_radius);
        assert_eq!(health.position_type, PositionType::Absolute);
        assert_eq!(health.top, Val::Px(16.0));
        assert_eq!(health.left, Val::Px(16.0));

        let energy = node_of::<EnergyBarTag>(app.world_mut());
        assert_eq!(energy.width, Val::Px(220.0));
        assert_eq!(energy.height, Val::Px(14.0));
        assert_eq!(energy.overflow, clip);
        assert_eq!(energy.border_radius, non_zero_radius);
        assert_eq!(energy.top, Val::Px(42.0));
        assert_eq!(energy.left, Val::Px(16.0));

        let poise = node_of::<PoiseBarTag>(app.world_mut());
        assert_eq!(poise.width, Val::Px(220.0));
        assert_eq!(poise.height, Val::Px(8.0));
        assert_eq!(poise.overflow, clip);
        assert_eq!(poise.border_radius, non_zero_radius);
        assert_eq!(poise.top, Val::Px(60.0));
        assert_eq!(poise.left, Val::Px(16.0));

        let xp = node_of::<XpBarTag>(app.world_mut());
        // The XP bar's width is DELIBERATELY overridden to fill the screen
        // (unlike the other three) — but its height/overflow/radius must
        // still survive from `spawn_bar`.
        assert_eq!(xp.width, Val::Percent(100.0));
        assert_eq!(xp.height, Val::Px(6.0));
        assert_eq!(xp.overflow, clip);
        assert_eq!(xp.border_radius, non_zero_radius);
        assert_eq!(xp.bottom, Val::Px(0.0));
        assert_eq!(xp.left, Val::Px(0.0));
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
