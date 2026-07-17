//! BL-82 EM-5.17 Phase 5 — the boss/target nameplate (design spec §3.5).
//!
//! ## Research spike (this phase's first job, per the plan) — RESULT: no real
//! ## target-selection source exists; the panel ships HIDDEN
//! The design spec's §6 Q7 confirmed the current Bevy client has no
//! target/selected-entity concept, and named legacy's `HudInfo::
//! target_entity`/`selected_entity` (`voxygen/src/hud/mod.rs:659/665-666`) as
//! the precedent to port. Tracing that precedent all the way down (not just
//! to the struct fields) shows it is **not** a small resource to port
//! in-place:
//! - `target_entity` is set every frame from a **3D raycast under the
//!   crosshair** (`voxygen/src/session/target.rs`'s `targets_under_cursor`,
//!   called from `session/mod.rs:684/785`) — nearby entities sorted by camera
//!   distance, tested against a ray from `cam_pos`/`cam_dir` out to
//!   `MAX_TARGET_RANGE`. This client has **no** 3D picking/raycast-vs-entity
//!   system at all (`bevy::picking` here is used only for `bevy_ui` widgets,
//!   e.g. `map_view.rs`/`combat_hud.rs` — never against world-space entities).
//! - `selected_entity` is only ever set on key-**up** of the dedicated `Select`
//!   action (`session/mod.rs:1316-1320`, default-bound to `X`), snapshotting
//!   whatever `target_entity` currently is. Interestingly,
//!   `xindeler_input::GameInput::Select` (`bevy/xindeler-input/src/
//!   game_input.rs:120`, default-bound to `KeyX` in `keybind.rs:226`) **already
//!   exists** in this client's rebindable-action enum — but nothing anywhere
//!   consumes it (`grep GameInput::Select` outside its own definition returns
//!   nothing). So the keybind is defined but there is nothing for it to
//!   confirm, because step one (the raycast) doesn't exist.
//!
//! `social_hud.rs`'s `handle_talk_key` (nearest OTHER mirrored entity within
//! `TALK_RANGE`) looks superficially similar but is a **one-shot, edge-
//! triggered** resolution (`keys.just_pressed`, fresh distance scan on that
//! single frame, fires a one-off dialogue message) for an unrelated feature
//! (who does a `T` press greet) — it holds no persistent "current target"
//! between frames. Re-purposing that shape into an always-on, continuously
//! re-evaluated "nearest mirrored entity becomes your boss target" resource
//! would be a **new** auto-targeting mechanic (e.g. walking within range of
//! any NPC — including non-hostile ones — silently becomes your nameplate
//! target), not a port of anything that exists in this client, in legacy, or
//! mirrored from the server. Per this phase's explicit brief ("do not invent
//! a new target-selection UX/mechanic without asking first"), that is **not**
//! built here.
//!
//! **Decision: [`SelectedTarget`] ships as a resource with a fully-built,
//! fully-wired render path, but nothing in this crate ever sets it to
//! `Some(_)` during real gameplay** — the panel stays hidden
//! ([`sync_nameplate_visibility`]'s existing `None` → `Visibility::Hidden`
//! branch, unchanged). The only thing that can populate it is
//! [`force_target_for_smoke_capture`], explicitly gated behind
//! `XINDELER_SMOKE_FORCE_TARGET` for visual smoke-testing only.
//!
//! **TODO(EM-5.17 Q7 follow-up) — what a real fix needs, one of:**
//! 1. Port legacy's crosshair-raycast (`targets_under_cursor`) as a genuine new
//!    3D-picking system, then wire the already-defined `GameInput::Select`
//!    action to snapshot it — this is real, scoped feature work (a new gameplay
//!    mechanic), not a "small resource."
//! 2. Or: have the server mirror an authoritative "current target" concept
//!    (e.g. last-attacked / attacked-by) instead of a client-local pick — a
//!    different design choice, needs a decision, not assumed here.
//!
//! Either path is a follow-up phase/spec question for Matías, not an
//! invention made unilaterally in this PR.
//!
//! **UPDATE (BL-82 EM-5.19 Phase 2, 2026-07-16): hard-lock bright/dim.**
//! `crate::targeting::HardLock` + `TargetLockKind` now layer a persistent
//! lock (`GameInput::Select`) on top of the soft scan below;
//! [`sync_nameplate_lock_style`] is the only change this phase makes here —
//! it tints the panel dim while `TargetLockKind::Soft`, and leaves it at its
//! default bright look for `TargetLockKind::Hard`. See `targeting.rs`'s own
//! module doc for the promote/facing/auto-release machinery.
//!
//! **UPDATE (BL-82 EM-5.18 Phase 1, 2026-07-16): resolved via option 2's
//! spirit, but client-local, not raycast-based.** Matías specified a hybrid
//! Diablo-IV-style soft-target: `crate::targeting::update_soft_target`
//! continuously scans a cone in front of the camera and scores `Enemy`-
//! aligned mirrored candidates (spec `docs/design/specs/2026-07-16-bl82-
//! hybrid-target-selection-design.md`), then writes the winner into
//! [`SelectedTarget`] every frame. This is neither of the two options sketched
//! above verbatim — no raycast/picking system, and no new server-mirrored
//! "current target" concept — but it is exactly the kind of new, scoped
//! gameplay mechanic the TODO above called out as needing a real decision
//! before being built; that decision has now been made (see the spec's FD1-3)
//! and implemented in `targeting.rs`. `SelectedTarget` now becomes `Some(_)`
//! in real gameplay whenever an `Enemy` is in range/cone — the panel is no
//! longer permanently hidden. `force_target_for_smoke_capture` below is
//! unchanged and still works; see `targeting.rs`'s module doc comment for the
//! precedence between the two.
//!
//! ## What's mirrored for non-local entities (also verified directly)
//! - Health → [`NetHealth`] (mirrored since EM-3.7, every entity).
//! - Stagger → [`NetPoise`] (BL-82 EM-5.2's `mirror_combat_hud_state` iterates
//!   ALL of `SimMirror`, no local-player filter — confirmed by reading
//!   `xindeler-sim-bridge/src/combat_hud.rs` directly, matching spec §3.5's own
//!   claim).
//! - Level → [`NetXp`] — ALSO already mirrored for every entity carrying a sim
//!   `SkillSet`, which is every NPC (`NpcBuilder`/`create_npc` always attach
//!   one, `server/src/state_ext.rs`). This is a genuine finding beyond what the
//!   spec spike wrote down: the level badge does NOT need to ship hidden, it
//!   reads a real mirrored value.
//! - Name → **NOT mirrored for arbitrary entities.** `mirror_sim_entities`
//!   reads `comp::Stats` in its own tests but never projects it into a `Net*`
//!   component (no `NetStats`/`NetName` exists); the only per-entity name path
//!   in this codebase (`xindeler_protocol::social::
//!   NetPlayerListEntry`/`NetGroupMember`) is bulk data scoped to players/
//!   group members correlated by `NetUid`, not a general "any mirrored entity's
//!   display name" lookup. Per this phase's decision rule, this is NOT built
//!   speculatively — [`placeholder_name`] renders a coarse label derived from
//!   the ALREADY-mirrored [`NetBody`] (e.g. "WOLF", "HUMANOID") falling back to
//!   `"TARGET #<uid>"`, and the real fix (mirror `comp:: Stats.name`,
//!   i18n-flattened the same way `NetPlayerListEntry::name` already is) is
//!   flagged as a follow-up, not silently invented here.
//!
//! Compiled only under the `listen-server`/`net-client` cargo features, same
//! gate as every other `Net*`-reading module in this crate.

use bevy::prelude::*;
use common::comp::Body;
use xindeler_protocol::{NetBody, NetHealth, NetLocalPlayer, NetPoise, NetUid, NetXp};
use xindeler_ui::{
    bar::{BarValue, spawn_bar, spawn_horizontal_image_bar},
    images::{HudImageKey, HudImages},
    theme::{HudFonts, HudTheme},
    zlayer,
};

const NAMEPLATE_WIDTH: f32 = 400.0;
const HEALTH_BAR_WIDTH: f32 = 340.0;
const HEALTH_BAR_HEIGHT: f32 = 24.0;
const STAGGER_BAR_WIDTH: f32 = 340.0;
const STAGGER_BAR_HEIGHT: f32 = 14.0;
const LEVEL_BADGE_SIZE: f32 = 40.0;

/// BL-82 EM-5.19 Phase 2 (spec §3.5): the nameplate background's tint while
/// the selection is only a soft target — a translucent dim, so a hard lock
/// (the default, untinted `Color::WHITE` the panel already spawns with)
/// reads unmistakably brighter/solid by contrast. This is the ONE additive
/// change this phase makes to the nameplate itself (see
/// [`sync_nameplate_lock_style`]).
const NAMEPLATE_SOFT_TINT: Color = Color::srgba(1.0, 1.0, 1.0, 0.55);

/// The currently-selected/targeted mirrored entity, if any (BL-82 EM-5.17
/// Phase 5's port of legacy's `HudInfo::target_entity`/`selected_entity`
/// concept). See the module doc comment: real gameplay never sets this to
/// `Some(_)` yet (no crosshair-raycast/picking system exists to drive it) —
/// it stays `None` outside of [`force_target_for_smoke_capture`]'s
/// smoke-testing override, which keeps the nameplate hidden by default.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct SelectedTarget(pub Option<Entity>);

/// Marks the nameplate's root panel — the ONLY entity whose [`Visibility`]
/// this module ever sets to [`Visibility::Visible`]/[`Visibility::Hidden`].
/// Every content child spawns with [`Visibility::Inherited`] (never
/// `Visible`) — BL-82 EM-5.17 Phase 0's Bug B is the exact regression this
/// guards against (a child's `Visible` force-overrides an ancestor's
/// `Hidden` in Bevy; `Inherited` is the correct "follow the parent" value).
#[derive(Component)]
struct NameplateRoot;
#[derive(Component)]
struct NameplateNameText;
#[derive(Component)]
struct NameplateHealthBarTag;
#[derive(Component)]
struct NameplateStaggerBarTag;
#[derive(Component)]
struct NameplateLevelText;

/// Installs the boss/target nameplate: spawns the (initially hidden) panel
/// at `Startup` and keeps target-resolution + content sync running every
/// frame.
pub struct BossNameplateViewPlugin;

impl Plugin for BossNameplateViewPlugin {
    fn build(&self, app: &mut App) {
        // Same double-add guard `combat_hud::CombatHudViewPlugin` already
        // documents — several view plugins in this crate each want
        // `XindelerUiPlugin` present.
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        app.init_resource::<SelectedTarget>()
            .add_systems(
                Startup,
                spawn_boss_nameplate
                    .after(xindeler_ui::theme::init_theme)
                    .after(xindeler_ui::images::init_images),
            )
            .add_systems(
                Update,
                (
                    force_target_for_smoke_capture,
                    sync_nameplate_visibility,
                    sync_nameplate_content,
                    // BL-82 EM-5.19 Phase 2: dim the panel for a soft target,
                    // bright (default/untinted) for a hard lock. Degrades
                    // clean via `Option<Res<_>>` if `TargetSelectionPlugin`
                    // isn't registered (see the fn's own doc comment) —
                    // never a hard dependency between the two plugins.
                    sync_nameplate_lock_style,
                ),
            );
    }
}

/// Smoke-only override (BL-82 EM-5.17 Phase 5): with no real
/// target-selection source wired (see module doc comment),
/// [`SelectedTarget`] otherwise never becomes `Some(_)`, so the panel would
/// be untestable visually. `XINDELER_SMOKE_FORCE_TARGET`, when set,
/// force-selects WHATEVER mirrored non-local entity exists so a live visual
/// smoke check can confirm the panel actually renders — the same
/// env-var-gated convention `diary.rs`'s `force_open_diary_for_smoke_capture`
/// and `inventory_ui.rs`'s `force_open_inventory_for_smoke_capture` already
/// establish. A no-op unless the env var is set; this is dev/test tooling
/// only, not a gameplay selection mechanism.
///
/// `pub(crate)`: BL-82 EM-5.18 P1's `targeting::update_soft_target` also
/// writes `SelectedTarget`, so it declares an explicit `.ambiguous_with(this)`
/// edge to tell Bevy's ambiguity checker the shared-`ResMut` overlap is
/// intentional and resolved by the env gate (both systems no-op unless the
/// OTHER's precondition is false) — see `targeting.rs`'s module doc comment.
pub(crate) fn force_target_for_smoke_capture(
    any_other: Query<Entity, (With<NetUid>, Without<NetLocalPlayer>)>,
    mut target: ResMut<SelectedTarget>,
) {
    if std::env::var("XINDELER_SMOKE_FORCE_TARGET").is_ok_and(|v| v != "0")
        && let Some(entity) = any_other.iter().next()
    {
        target.0 = Some(entity);
    }
}

/// Coarse, honest placeholder display name derived from the target's
/// ALREADY-mirrored [`NetBody`] — see the module doc comment for why a real
/// per-entity name mirror is a documented follow-up, not built here.
fn placeholder_name(body: &Body) -> String {
    let label = match body {
        Body::Humanoid(_) => "Humanoid",
        Body::QuadrupedSmall(_) => "Small Beast",
        Body::QuadrupedMedium(_) => "Beast",
        Body::QuadrupedLow(_) => "Reptile",
        Body::BirdMedium(_) => "Bird",
        Body::BirdLarge(_) => "Great Bird",
        Body::FishMedium(_) | Body::FishSmall(_) => "Fish",
        Body::Dragon(_) => "Dragon",
        Body::BipedLarge(_) => "Giant",
        Body::BipedSmall(_) => "Small Foe",
        Body::Golem(_) => "Golem",
        Body::Theropod(_) => "Theropod",
        Body::Arthropod(_) => "Arthropod",
        Body::Crustacean(_) => "Crustacean",
        Body::Ship(_) => "Vessel",
        Body::Object(_) | Body::Item(_) => "Object",
        Body::Plugin(_) => "Creature",
    };
    label.to_uppercase()
}

/// Spawns the (initially hidden) nameplate panel: name text, a
/// `boss_bar_frame.png`-framed health bar, a `boss_stagger_bar.png`/
/// `boss_stagger_full_bar.png` stagger bar, and a `boss_level_badge.png`
/// level badge (spec §3.5).
fn spawn_boss_nameplate(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
) {
    let health_bar = spawn_bar(
        &mut commands,
        &theme,
        theme.palette.health,
        theme.palette.health_bg,
        HEALTH_BAR_WIDTH,
        HEALTH_BAR_HEIGHT,
        BarValue::new(1.0, 1.0),
    );
    commands.entity(health_bar).insert(NameplateHealthBarTag);
    commands.entity(health_bar).with_children(|parent| {
        // The frame overlay convention `bar::spawn_orb_bar` already
        // establishes: a full-size sibling `ImageNode` on top, `Pickable::
        // IGNORE` so it never blocks interaction with whatever's beneath.
        parent.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..Default::default()
            },
            ImageNode::new(images.get(HudImageKey::BossBarFrame)),
            bevy::picking::Pickable::IGNORE,
        ));
    });

    let stagger_bar = spawn_horizontal_image_bar(
        &mut commands,
        images.get(HudImageKey::BossStaggerBar),
        images.get(HudImageKey::BossStaggerFullBar),
        None,
        STAGGER_BAR_WIDTH,
        STAGGER_BAR_HEIGHT,
        BarValue::new(1.0, 1.0),
    );
    commands.entity(stagger_bar).insert(NameplateStaggerBarTag);

    // Spawned first (without the bar children — those already exist as their
    // own entities from `spawn_bar`/`spawn_horizontal_image_bar` above) so
    // its `Entity` id is known before reparenting them onto it via
    // `EntityCommands::add_child` below. `RelatedSpawnerCommands` (the type
    // `with_children`'s closure hands back) only supports SPAWNING new
    // children, not reparenting an already-existing entity — so the name
    // text + level badge (genuinely new entities) go through
    // `with_children`, while the two pre-built bars are attached via
    // `add_child` afterward, in the same relative order so the visual
    // column layout (name, health, stagger, badge) is unaffected.
    let root = commands
        .spawn((
            NameplateRoot,
            Visibility::Hidden,
            GlobalZIndex(zlayer::BOSS_NAMEPLATE),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(25.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-(NAMEPLATE_WIDTH / 2.0))),
                width: Val::Px(NAMEPLATE_WIDTH),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::all(theme.spacing.sm_px()),
                row_gap: theme.spacing.xs_px(),
                ..Default::default()
            },
            ImageNode::new(images.get(HudImageKey::BossNamePlateBg)),
        ))
        .with_children(|parent| {
            parent.spawn((
                NameplateNameText,
                Visibility::Inherited,
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.title.clone()),
                    font_size: bevy::text::FontSize::Px(22.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        })
        .id();

    commands.entity(root).add_child(health_bar);
    commands.entity(root).add_child(stagger_bar);

    commands.entity(root).with_children(|parent| {
        parent
            .spawn((
                Visibility::Inherited,
                Node {
                    width: Val::Px(LEVEL_BADGE_SIZE),
                    height: Val::Px(LEVEL_BADGE_SIZE),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..Default::default()
                },
                ImageNode::new(images.get(HudImageKey::BossLevelBadge)),
            ))
            .with_children(|badge| {
                badge.spawn((
                    NameplateLevelText,
                    Visibility::Inherited,
                    Text(String::new()),
                    TextFont {
                        font: bevy::text::FontSource::Handle(fonts.body.clone()),
                        font_size: bevy::text::FontSize::Px(16.0),
                        ..Default::default()
                    },
                    TextColor(theme.palette.text),
                ));
            });
    });
}

/// Shows/hides [`NameplateRoot`] from [`SelectedTarget`] — the SAME
/// root-only `Visible`/`Hidden` toggle convention `diary.rs`'s
/// `sync_diary_window_visibility`/`inventory_ui.rs`'s
/// `sync_inventory_window_visibility` already use (never touching a
/// content child's own `Visibility`, per Bug B's lesson).
fn sync_nameplate_visibility(
    target: Res<SelectedTarget>,
    mut root: Query<&mut Visibility, With<NameplateRoot>>,
) {
    if !target.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if target.0.is_some() {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// BL-82 EM-5.19 Phase 2 (spec §3.5) — the one additive change this phase
/// makes to the nameplate: tints the panel [`NAMEPLATE_SOFT_TINT`] while the
/// selection is a soft target, and back to the default untinted
/// `Color::WHITE` (the panel's own spawn-time color, unchanged since P1) for
/// a hard lock — so a lock reads visibly brighter/solid by contrast, with no
/// new asset. Only runs the write when `TargetLockKind` actually changed
/// (same `is_changed()` early-out convention [`sync_nameplate_visibility`]
/// uses). Takes `Option<Res<_>>` rather than a hard `Res<_>` dependency:
/// [`crate::targeting::TargetLockKind`] is registered by the separate
/// `TargetSelectionPlugin`, so this degrades to a no-op (default bright
/// look) rather than panicking if that plugin is ever not present alongside
/// this one.
fn sync_nameplate_lock_style(
    lock_kind: Option<Res<crate::targeting::TargetLockKind>>,
    mut root: Query<&mut ImageNode, With<NameplateRoot>>,
) {
    let Some(lock_kind) = lock_kind else { return };
    if !lock_kind.is_changed() {
        return;
    }
    let Ok(mut image) = root.single_mut() else {
        return;
    };
    image.color = match *lock_kind {
        crate::targeting::TargetLockKind::Soft => NAMEPLATE_SOFT_TINT,
        crate::targeting::TargetLockKind::Hard => Color::WHITE,
    };
}

/// Reads the selected target's mirrored [`NetHealth`]/[`NetPoise`]/
/// [`NetXp`]/[`NetBody`]/[`NetUid`] and updates the panel's bars/text.
/// Degrades clean (a target with no mirrored health/poise/xp yet, or no
/// target at all) — never panics (spec §3.2).
fn sync_nameplate_content(
    target: Res<SelectedTarget>,
    entities: Query<(
        Option<&NetHealth>,
        Option<&NetPoise>,
        Option<&NetXp>,
        Option<&NetBody>,
        Option<&NetUid>,
    )>,
    mut health_bars: Query<
        &mut BarValue,
        (With<NameplateHealthBarTag>, Without<NameplateStaggerBarTag>),
    >,
    mut stagger_bars: Query<
        &mut BarValue,
        (With<NameplateStaggerBarTag>, Without<NameplateHealthBarTag>),
    >,
    mut name_text: Query<&mut Text, (With<NameplateNameText>, Without<NameplateLevelText>)>,
    mut level_text: Query<&mut Text, (With<NameplateLevelText>, Without<NameplateNameText>)>,
) {
    let Some(target_entity) = target.0 else {
        return;
    };
    let Ok((health, poise, xp, body, uid)) = entities.get(target_entity) else {
        return;
    };

    if let Ok(mut bar) = health_bars.single_mut() {
        *bar = health
            .map(|h| BarValue::new(h.current, h.max))
            .unwrap_or(BarValue::new(0.0, 0.0));
    }
    if let Ok(mut bar) = stagger_bars.single_mut() {
        *bar = poise
            .map(|p| BarValue::new(p.current, p.max))
            .unwrap_or(BarValue::new(0.0, 0.0));
    }
    if let Ok(mut text) = name_text.single_mut() {
        text.0 = body
            .map(|b| placeholder_name(&b.0))
            .unwrap_or_else(|| format!("TARGET #{}", uid.map(|u| u.0).unwrap_or_default()));
    }
    if let Ok(mut text) = level_text.single_mut() {
        text.0 = xp.map(|x| format!("{}", x.level)).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use xindeler_ui::bar::HudImageBarFillClip;

    use super::*;

    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.init_resource::<SelectedTarget>();
        app
    }

    /// With `SelectedTarget = None`, [`sync_nameplate_visibility`] hides the
    /// root — the "nothing selected" acceptance case (spec §3.5).
    #[test]
    fn sync_nameplate_visibility_hides_with_no_target() {
        let mut app = new_app();
        let root = app
            .world_mut()
            .spawn((NameplateRoot, Visibility::Visible))
            .id();

        app.world_mut()
            .run_system_once(sync_nameplate_visibility)
            .expect("system runs");

        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Hidden
        );
    }

    /// With `SelectedTarget = Some(entity)`, the root becomes visible and
    /// [`sync_nameplate_content`] reads that entity's mirrored `NetHealth`/
    /// `NetPoise` into the health/stagger bars — the core acceptance case
    /// this phase's brief asks for.
    #[test]
    fn nameplate_becomes_visible_and_reads_mirrored_health_and_poise() {
        let mut app = new_app();
        let root = app
            .world_mut()
            .spawn((NameplateRoot, Visibility::Hidden))
            .id();
        let health_bar = app
            .world_mut()
            .spawn((NameplateHealthBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let stagger_bar = app
            .world_mut()
            .spawn((NameplateStaggerBarTag, BarValue::new(1.0, 1.0)))
            .id();
        let name_text = app
            .world_mut()
            .spawn((NameplateNameText, Text(String::new())))
            .id();
        let level_text = app
            .world_mut()
            .spawn((NameplateLevelText, Text(String::new())))
            .id();

        let target_entity = app
            .world_mut()
            .spawn((
                NetUid(42),
                NetHealth {
                    current: 40.0,
                    max: 100.0,
                },
                NetPoise {
                    current: 5.0,
                    max: 50.0,
                },
                NetXp {
                    level: 7,
                    xp_into_level: 0,
                    xp_for_level: 1,
                },
                NetBody(Body::default()),
            ))
            .id();
        app.insert_resource(SelectedTarget(Some(target_entity)));

        app.world_mut()
            .run_system_once(sync_nameplate_visibility)
            .expect("visibility system runs");
        app.world_mut()
            .run_system_once(sync_nameplate_content)
            .expect("content system runs");

        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Visible
        );
        assert_eq!(
            *app.world().get::<BarValue>(health_bar).unwrap(),
            BarValue::new(40.0, 100.0)
        );
        assert_eq!(
            *app.world().get::<BarValue>(stagger_bar).unwrap(),
            BarValue::new(5.0, 50.0)
        );
        assert_eq!(app.world().get::<Text>(name_text).unwrap().0, "SMALL BEAST");
        assert_eq!(app.world().get::<Text>(level_text).unwrap().0, "7");
    }

    /// A target with no mirrored `NetBody`/`NetUid` yet still degrades clean
    /// — no panic, name falls back to `"TARGET #0"`.
    #[test]
    fn nameplate_content_degrades_clean_with_no_body_or_uid() {
        let mut app = new_app();
        app.world_mut().spawn(NameplateRoot);
        app.world_mut()
            .spawn((NameplateHealthBarTag, BarValue::new(1.0, 1.0)));
        app.world_mut()
            .spawn((NameplateStaggerBarTag, BarValue::new(1.0, 1.0)));
        let name_text = app
            .world_mut()
            .spawn((NameplateNameText, Text(String::new())))
            .id();
        app.world_mut()
            .spawn((NameplateLevelText, Text(String::new())));

        let target_entity = app
            .world_mut()
            .spawn(NetHealth {
                current: 1.0,
                max: 1.0,
            })
            .id();
        app.insert_resource(SelectedTarget(Some(target_entity)));

        app.world_mut()
            .run_system_once(sync_nameplate_content)
            .expect("content system runs without panicking");

        assert_eq!(app.world().get::<Text>(name_text).unwrap().0, "TARGET #0");
    }

    /// [`placeholder_name`] uppercases a coarse label per top-level `Body`
    /// variant — the documented placeholder for the missing per-entity name
    /// mirror.
    #[test]
    fn placeholder_name_uppercases_a_coarse_body_label() {
        assert_eq!(placeholder_name(&Body::default()), "SMALL BEAST");
    }

    /// The stagger bar is built via
    /// `xindeler_ui::bar::spawn_horizontal_image_bar` — proving it carries
    /// a real fraction-reveal clip window (tagged [`HudImageBarFillClip`])
    /// sized to the initial `BarValue` fraction, the same shape
    /// `bar::image_bar_fill_tracks_value_changes_by_width` already proves the
    /// primitive keeps in sync as the value changes (that crate owns
    /// `update_horizontal_image_bars`, `pub(crate)` there, so this module
    /// only asserts the spawn shape it depends on, not the private update
    /// system itself). Checks the CLIP WINDOW, not the fill image directly —
    /// BL-82 EM-5.17 Phase 0-style follow-up rework: the fill image's own
    /// `Node` is now a fixed `Val::Px` (never resized), only the clip
    /// wrapper reacts to the fraction (see `bar::
    /// image_bar_fill_image_never_resizes_only_the_clip_wrapper_does` for
    /// the regression guard on that split).
    #[test]
    fn stagger_bar_is_built_from_the_shared_image_bar_primitive() {
        let mut app = new_app();

        let mut commands = app.world_mut().commands();
        let container = spawn_horizontal_image_bar(
            &mut commands,
            Handle::default(),
            Handle::default(),
            None,
            STAGGER_BAR_WIDTH,
            STAGGER_BAR_HEIGHT,
            BarValue::new(50.0, 100.0),
        );
        app.world_mut().flush();

        let children: Vec<Entity> = app
            .world()
            .get::<Children>(container)
            .unwrap()
            .iter()
            .collect();
        let clip = children
            .into_iter()
            .find(|&e| app.world().get::<HudImageBarFillClip>(e).is_some())
            .expect("clip-window child exists");
        assert_eq!(
            app.world().get::<Node>(clip).unwrap().width,
            Val::Percent(50.0)
        );
    }

    /// BL-82 EM-5.19 Phase 2: with no [`crate::targeting::TargetLockKind`]
    /// resource registered at all (`TargetSelectionPlugin` absent),
    /// [`sync_nameplate_lock_style`] must degrade clean — no panic, no
    /// color change — rather than assume the other plugin is always present.
    #[test]
    fn sync_nameplate_lock_style_noops_without_target_lock_kind_resource() {
        let mut app = new_app();
        let root = app
            .world_mut()
            .spawn((NameplateRoot, ImageNode::default()))
            .id();

        app.world_mut()
            .run_system_once(sync_nameplate_lock_style)
            .expect("system runs without the resource");

        assert_eq!(
            app.world().get::<ImageNode>(root).unwrap().color,
            Color::default()
        );
    }

    /// A soft target dims the panel to [`NAMEPLATE_SOFT_TINT`].
    #[test]
    fn sync_nameplate_lock_style_dims_panel_for_soft_target() {
        let mut app = new_app();
        app.insert_resource(crate::targeting::TargetLockKind::Soft);
        let root = app
            .world_mut()
            .spawn((NameplateRoot, ImageNode::default()))
            .id();

        app.world_mut()
            .run_system_once(sync_nameplate_lock_style)
            .expect("system runs");

        assert_eq!(
            app.world().get::<ImageNode>(root).unwrap().color,
            NAMEPLATE_SOFT_TINT
        );
    }

    /// A hard lock renders at the panel's default (untinted, bright) color —
    /// even if it was previously dimmed by a soft target the frame before.
    #[test]
    fn sync_nameplate_lock_style_brightens_panel_for_hard_lock() {
        let mut app = new_app();
        app.insert_resource(crate::targeting::TargetLockKind::Hard);
        let root = app
            .world_mut()
            .spawn((NameplateRoot, ImageNode {
                color: NAMEPLATE_SOFT_TINT,
                ..Default::default()
            }))
            .id();

        app.world_mut()
            .run_system_once(sync_nameplate_lock_style)
            .expect("system runs");

        assert_eq!(
            app.world().get::<ImageNode>(root).unwrap().color,
            Color::WHITE
        );
    }
}
