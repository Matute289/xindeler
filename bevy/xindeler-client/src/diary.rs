//! BL-82 EM-5.7 — the character diary / skill-tree screen (spec §2/§6, tasks
//! T56.22-.24).
//!
//! ## Reusing BL-06's data-driven philosophy (the dispatch's own framing)
//! Legacy `voxygen`'s BL-06 class-tree renderer (`handle_class_skills_window`,
//! `voxygen/src/hud/diary.rs`) proved a GENERIC algorithm: read the skill
//! membership of a group, compute each skill's TIER by recursing over its
//! prerequisite DAG (`SKILL_PREREQUISITES`, tier 0 = no prerequisites),
//! auto-layout tier rows into a grid, and stop hand-laying nodes per class.
//! This screen generalizes that ONE step further — the SAME renderer draws
//! EVERY [`common::comp::skillset::SkillGroupKind`] (General, every weapon,
//! every class, Feats), not just Class — reading whichever groups the local
//! player's real [`NetSkillSet`] mirror reports, so a new class or weapon
//! needs zero new UI code, exactly the "generic over hand-laid" principle
//! BL-06 established (Matías's explicit decision, carried into the Bevy
//! port per the EM-5.7 dispatch).
//!
//! ## Why the static tree shape is parsed HERE, not read from `common`
//! `bevy/xindeler-client` links `common` with the `no-assets` feature
//! (`Cargo.toml`) — a codebase-wide convention (`xindeler-render-voxel`'s
//! figure manifests, `xindeler-ui::i18n`'s `.ftl` loader) that Bevy client
//! crates never touch `common`'s `assets_manager`-backed `lazy_static`s
//! (`SKILL_GROUP_DEFS`/`SKILL_PREREQUISITES`/`SKILL_MAX_LEVEL`/
//! `CLASS_SKILL_MODIFIERS`/`FEAT_MODIFIERS`), even though `common` is linked
//! for its TYPES (`Skill`/`SkillGroupKind`/`SkillPrerequisite`/
//! `ClassPassiveStat` — the same isolation-law "type library" carve-out
//! `xindeler_protocol::NetBuffEntry` uses for `BuffKind`). [`SkillTreeShape`]
//! parses the SAME frozen RON manifests directly (`assets/common/
//! skill_trees/*.ron`, never renamed) via a synchronous `std::fs` read at
//! `Startup` — the exact "small, rarely-changing config text, no Bevy
//! `AssetServer`/typed-loader machinery needed" shape `xindeler_ui::i18n::
//! Localization::load` already established, reusing its
//! `xindeler_ui::i18n::assets_root` resolver. Only the DYNAMIC per-player
//! state (which skills are actually unlocked, at what level, with how much
//! SP) travels the wire as [`xindeler_protocol::NetSkillSet`].
//!
//! ## The Abilities tab + hotbar drag (T56.24)
//! [`NetAbilityPool`] (mirrored by `xindeler-sim-bridge::skillset`) already
//! carries EVERY currently-qualifying ability the sim resolves
//! (`ActiveAbilities::all_available_abilities`), so the Abilities tab needs no
//! extra bookkeeping beyond rendering it as [`xindeler_ui::slot`] drag
//! SOURCES — reusing the SAME drag-drop primitive EM-5.3/5.6 already
//! established (`DIARY_ABILITY_GROUP`, a fresh [`SlotGroup`] alongside the
//! hotbar/bag/equip/trade groups those screens already claimed).
//! `xindeler-client::hotbar`'s existing drop handler is extended (in that
//! module, not here) to accept a drop whose `from_group` is
//! [`DIARY_ABILITY_GROUP`] — no new drop-consumption code lives in this
//! module, only the drag SOURCE.
//!
//! Compiled only under `listen-server`/`net-client` — same posture as every
//! other `xindeler_protocol`-consuming module in this crate.

use std::{collections::HashMap, path::PathBuf};

use bevy::prelude::*;
use common::comp::skillset::{
    SkillGroupKind, SkillPrerequisite,
    skills::{ClassPassiveStat, Skill},
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    LocalUnlockSkillRequest, NetAbilityPool, NetBuffs, NetCombo, NetEnergy, NetHealth,
    NetLocalPlayer, NetPoise, NetSkillSet, NetXp,
};
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    panel::panel_bundle,
    scroll::scroll_view_bundle,
    slot::{SlotAddress, SlotContents, SlotGroup, slot_bundle},
    theme::{HudFonts, HudTheme},
    tooltip::Tooltip,
};

/// The drag-drop group the Abilities tab's slots live in (BL-82 EM-5.7) — a
/// fresh [`SlotGroup`] alongside the hotbar (`0`), bag (`1`), equip (`2`), and
/// the two trade-offer groups (`3`/`4`) `xindeler-client`'s other screens
/// already claimed. `pub(crate)`: `crate::hotbar`'s drop handler needs it to
/// recognize a drag FROM this screen.
pub(crate) const DIARY_ABILITY_GROUP: SlotGroup = SlotGroup(5);

const ABILITY_SLOT_SIZE_PX: f32 = 44.0;

/// The tree grid's fixed layout budget (BL-82 EM-5.7) — mirrors BL-06's own
/// `GRID_W`/`GRID_H`/`TOP_MARGIN`/`ROW_H`/`COL_W`/node-size constants
/// (`voxygen/src/hud/diary.rs::handle_class_skills_window`), tunable for
/// visual polish once seen live (per that function's own doc comment).
const TREE_W: f32 = 720.0;
const TREE_H: f32 = 480.0;
const TREE_TOP_MARGIN: f32 = 20.0;
const TREE_ROW_H: f32 = 90.0;
const TREE_COL_W: f32 = 84.0;
const TREE_NODE_SIZE: f32 = 56.0;

/// The currently-selected diary tab (BL-82 EM-5.7). `Group` covers every
/// weapon/class/general/feats tree via the ONE generic renderer; `Stats`/
/// `Abilities` are the two non-tree tabs.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Default)]
enum DiaryTab {
    #[default]
    Stats,
    Abilities,
    Group(SkillGroupKind),
}

/// Resolves the real asset root — reused verbatim from
/// `xindeler_ui::i18n::assets_root` rather than a third duplicate (this
/// crate already depends on `xindeler-ui`, unlike that crate's own doc
/// comment explaining why IT can't reuse `xindeler-client`'s copy).
fn assets_root() -> PathBuf { xindeler_ui::i18n::assets_root() }

fn read_ron_manifest<T: serde::de::DeserializeOwned + Default>(
    root: &std::path::Path,
    dotted: &str,
) -> T {
    let path = root.join(format!("{}.ron", dotted.replace('.', "/")));
    match std::fs::read_to_string(&path) {
        Ok(text) => ron::de::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(?path, error = ?e, "diary: failed to parse skill-tree manifest, using empty default");
            T::default()
        }),
        Err(e) => {
            tracing::warn!(?path, error = ?e, "diary: failed to read skill-tree manifest, using empty default");
            T::default()
        },
    }
}

/// The STATIC skill-tree shape (BL-06's own data, ported to a client-side
/// synchronous parse — see this module's doc comment for why). Loaded once
/// at [`Startup`]; the RON manifests themselves change only with a content
/// patch, never per-session.
#[derive(Resource, Default, Debug)]
pub struct SkillTreeShape {
    groups: HashMap<SkillGroupKind, Vec<Skill>>,
    prerequisites: HashMap<Skill, SkillPrerequisite>,
    max_levels: HashMap<Skill, u16>,
    class_modifiers: HashMap<Skill, Vec<(ClassPassiveStat, f32)>>,
    feat_modifiers: HashMap<Skill, Vec<(ClassPassiveStat, f32)>>,
}

impl SkillTreeShape {
    fn load() -> Self {
        let root = assets_root();
        let shape = Self {
            groups: read_ron_manifest(&root, "common.skill_trees.skills_skill-groups_manifest"),
            prerequisites: read_ron_manifest(&root, "common.skill_trees.skill_prerequisites"),
            max_levels: read_ron_manifest(&root, "common.skill_trees.skill_max_levels"),
            class_modifiers: read_ron_manifest(&root, "common.skill_trees.class_skill_modifiers"),
            feat_modifiers: read_ron_manifest(&root, "common.skill_trees.feat_modifiers"),
        };
        tracing::debug!(
            root = %root.display(),
            groups_len = shape.groups.len(),
            prereqs_len = shape.prerequisites.len(),
            "diary: SkillTreeShape loaded"
        );
        shape
    }

    fn skills_in_group(&self, kind: SkillGroupKind) -> &[Skill] {
        self.groups.get(&kind).map_or(&[], Vec::as_slice)
    }

    /// The skill's max level, defaulting to 1 for a skill absent from the
    /// manifest — mirrors `common::comp::skillset::skills::Skill::max_level`'s
    /// own fallback exactly.
    fn max_level(&self, skill: Skill) -> u16 { self.max_levels.get(&skill).copied().unwrap_or(1) }

    /// Whether `skill` boosts a `Stats` field passively (has a
    /// `CLASS_SKILL_MODIFIERS`/`FEAT_MODIFIERS` entry) as opposed to unlocking
    /// an active ability or a weapon/class group.
    fn is_passive(&self, skill: Skill) -> bool {
        self.class_modifiers.contains_key(&skill) || self.feat_modifiers.contains_key(&skill)
    }

    /// Tier (depth) of `skill` in the prerequisite DAG — tier 0 = no
    /// prerequisites, tier N = 1 + the deepest direct prerequisite's tier.
    /// Mirrors `voxygen::hud::diary::Diary::class_skill_tier` exactly, just
    /// generalized to any group (the underlying algorithm was never
    /// class-specific — only ITS call site was).
    fn tier(&self, skill: Skill, depth: u8) -> u8 {
        if depth > 8 {
            return 0;
        }
        match self.prerequisites.get(&skill) {
            None => 0,
            Some(SkillPrerequisite::All(map) | SkillPrerequisite::Any(map)) => {
                map.keys()
                    .map(|&p| self.tier(p, depth + 1))
                    .max()
                    .unwrap_or(0)
                    + 1
            },
        }
    }

    /// Whether every (or any) prerequisite for `skill` is met by `unlocked`
    /// (skill -> level). Mirrors `SkillSet::prerequisites_met` exactly,
    /// reading the client's OWN projected [`NetSkillSet::skills`] map instead
    /// of a live sim-side `SkillSet`.
    fn prerequisites_met(&self, skill: Skill, unlocked: &HashMap<Skill, u16>) -> bool {
        match self.prerequisites.get(&skill) {
            Some(SkillPrerequisite::All(reqs)) => reqs
                .iter()
                .all(|(s, l)| unlocked.get(s).is_some_and(|have| have >= l)),
            Some(SkillPrerequisite::Any(reqs)) => reqs
                .iter()
                .any(|(s, l)| unlocked.get(s).is_some_and(|have| have >= l)),
            None => true,
        }
    }
}

fn load_skill_tree_shape(mut commands: Commands) {
    commands.insert_resource(SkillTreeShape::load());
}

/// Marks the diary window root (toggled by [`HudState`]).
#[derive(Component)]
struct DiaryWindowRoot;
/// Marks the tab-bar container (children are rebuilt whenever the local
/// player's group set changes).
#[derive(Component)]
struct DiaryTabBar;
/// Marks the Stats tab's text container.
#[derive(Component)]
struct StatsPanelRoot;
/// Marks the tree tab's fixed-size grid container (children = tree nodes).
#[derive(Component)]
struct TreeRoot;
/// Marks the Abilities tab's scroll container (children = ability slots).
#[derive(Component)]
struct AbilitiesPanelRoot;

/// Tags a tab button with which [`DiaryTab`] it selects.
#[derive(Component, Clone, Copy)]
struct DiaryTabButton(DiaryTab);
/// Tags a tree node with the [`Skill`] it spends a point on when clicked.
#[derive(Component, Clone, Copy)]
struct SkillNodeTarget(Skill);

pub struct DiaryUiPlugin;

impl Plugin for DiaryUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillTreeShape>()
            .insert_resource(DiaryTab::default())
            .add_systems(
                Startup,
                (
                    load_skill_tree_shape,
                    spawn_diary_window.after(xindeler_ui::theme::init_theme),
                    force_open_diary_for_smoke_capture,
                ),
            )
            .add_systems(
                Update,
                (
                    toggle_diary_window,
                    sync_diary_window_visibility,
                    sync_diary_tabs,
                    force_select_class_tab_for_smoke_capture,
                    force_select_abilities_tab_for_smoke_capture,
                    sync_tab_content_visibility,
                    sync_stats_panel,
                    sync_skill_tree_content,
                    sync_abilities_tab,
                ),
            );
    }
}

/// Force-opens [`HudWindow::Diary`] once at boot when
/// `XINDELER_SMOKE_OPEN_DIARY` is set — the same env-var-gated, smoke-only
/// debug-override convention `inventory_ui`'s own
/// `force_open_inventory_for_smoke_capture` already establishes:
/// `--smoke-screenshot` has no real keyboard to press `P` with, so this is
/// how a live visual smoke check can confirm the tab bar/tree/abilities grid
/// genuinely render, without adding a bespoke input-injection mechanism to
/// the harness itself. A no-op (never opens anything) unless the env var is
/// set — harmless in every normal run.
fn force_open_diary_for_smoke_capture(mut state: ResMut<HudState>) {
    if std::env::var("XINDELER_SMOKE_OPEN_DIARY").is_ok_and(|v| v != "0") {
        state.toggle(HudWindow::Diary);
    }
}

/// Force-selects the local player's Class tree tab once `NetSkillSet`
/// arrives, when `XINDELER_SMOKE_DIARY_CLASS_TAB` is set — the same
/// smoke-only debug-override convention as
/// [`force_open_diary_for_smoke_capture`], used to visually confirm the generic
/// tree renderer (not just the tab bar/ Stats snapshot) live via
/// `--smoke-screenshot` (no real mouse to click a tab button with). A no-op
/// unless the env var is set.
fn force_select_class_tab_for_smoke_capture(
    player: Query<&NetSkillSet, With<NetLocalPlayer>>,
    mut selected: ResMut<DiaryTab>,
) {
    if std::env::var("XINDELER_SMOKE_DIARY_CLASS_TAB").is_ok_and(|v| v != "0")
        && let Ok(skillset) = player.single()
        && let Some(class_group) = skillset
            .groups
            .iter()
            .find(|g| matches!(g.kind, SkillGroupKind::Class(_)))
    {
        *selected = DiaryTab::Group(class_group.kind);
    }
}

/// Force-selects the Abilities tab when `XINDELER_SMOKE_DIARY_ABILITIES_TAB`
/// is set — the same smoke-only debug-override convention as
/// [`force_select_class_tab_for_smoke_capture`], used to visually confirm the
/// Abilities tab's real [`NetAbilityPool`]-driven slot grid live via
/// `--smoke-screenshot`. A no-op unless the env var is set.
fn force_select_abilities_tab_for_smoke_capture(mut selected: ResMut<DiaryTab>) {
    if std::env::var("XINDELER_SMOKE_DIARY_ABILITIES_TAB").is_ok_and(|v| v != "0") {
        selected.set_if_neq(DiaryTab::Abilities);
    }
}

/// Spawns the (initially hidden) diary window skeleton: a tab-bar column +
/// three content containers (Stats/Tree/Abilities), all empty — the sync
/// systems below fill them in once real mirrored data exists.
fn spawn_diary_window(mut commands: Commands, theme: Res<HudTheme>) {
    commands
        .spawn((
            DiaryWindowRoot,
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
            let mut panel_entity = backdrop.spawn(panel_bundle(&theme));
            let column_gap = Val::Px(theme.spacing.lg);
            panel_entity.entry::<Node>().and_modify(move |mut node| {
                node.flex_direction = FlexDirection::Row;
                node.column_gap = column_gap;
            });
            panel_entity.with_children(|panel| {
                panel.spawn((DiaryTabBar, Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    min_width: Val::Px(140.0),
                    ..Default::default()
                }));
                // `display` starts `Flex` (Stats, the default tab) / `None`
                // (Tree/Abilities) is the ONLY thing that gates which content
                // panel is shown (`sync_tab_content_visibility`'s own doc
                // comment) — `Visibility` stays `Visible` on all three (a
                // real bug this fixed: `Visibility::Hidden` gates PAINTING
                // independent of `Node::display`/layout, so a panel spawned
                // Hidden here would never render even once its `display`
                // flipped to `Flex`, since nothing in this module ever
                // touches `Visibility` again after spawn — only `Display`
                // does. A live `--smoke-screenshot` of the Warrior tree tab
                // caught this: `sync_skill_tree_content` really did build all
                // 12 real skill nodes and `sync_tab_content_visibility` really
                // did flip `TreeRoot`'s `Node::display` to `Flex`, yet NOTHING
                // painted — the tree area just showed the world through the
                // backdrop, because `TreeRoot`/`AbilitiesPanelRoot` were
                // spawned `Visibility::Hidden` and stayed that way forever).
                panel.spawn((StatsPanelRoot, Visibility::Visible, Node {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    width: Val::Px(TREE_W),
                    ..Default::default()
                }));
                panel.spawn((TreeRoot, Visibility::Visible, Node {
                    display: Display::None,
                    position_type: PositionType::Relative,
                    width: Val::Px(TREE_W),
                    height: Val::Px(TREE_H),
                    ..Default::default()
                }));
                let mut abilities_entity = panel.spawn((
                    AbilitiesPanelRoot,
                    Visibility::Visible,
                    scroll_view_bundle(&theme, TREE_W, TREE_H),
                ));
                abilities_entity.entry::<Node>().and_modify(|mut node| {
                    node.display = Display::None;
                    node.flex_wrap = FlexWrap::Wrap;
                });
            });
        });
}

/// Toggles [`HudWindow::Diary`] on [`GameInput::Diary`] (`P` by default) —
/// read through [`ActionState`], not a hardcoded `KeyCode` (the current,
/// rebind-aware convention `controls_screen`'s own doc comment establishes;
/// `HudWindow::Diary` already existed in EM-5.1's state machine, unused until
/// now).
fn toggle_diary_window(action_state: Res<ActionState>, mut actions: MessageWriter<HudAction>) {
    if action_state.just_pressed(GameInput::Diary) {
        actions.write(HudAction::ToggleWindow(HudWindow::Diary));
    }
}

/// Shows/hides the diary window root from [`HudState`].
fn sync_diary_window_visibility(
    state: Res<HudState>,
    mut root: Query<&mut Visibility, With<DiaryWindowRoot>>,
) {
    if !state.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    *visibility = if state.is_open(HudWindow::Diary) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
}

/// Despawns every existing child of `root`, then hands the (now-empty)
/// entity's `ChildSpawner` to `spawn_children` — the shared "simple rebuild"
/// helper every content-sync system below uses (tab bar / tree / abilities
/// all change infrequently, matching `xindeler_ui::notification`'s own
/// documented "rebuild every time" posture for non-hot-path widgets).
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

/// Rebuilds the tab bar from the local player's real [`NetSkillSet`] groups +
/// the two fixed tabs (Stats/Abilities) whenever it changes — the T56.23
/// "class tab resolves the live class group at render time" acceptance bar,
/// generalized to every group.
fn sync_diary_tabs(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    player: Query<&NetSkillSet, With<NetLocalPlayer>>,
    tab_bar: Query<Entity, With<DiaryTabBar>>,
    children_query: Query<&Children>,
    mut last_built: Local<Option<Vec<DiaryTab>>>,
) {
    let Ok(skillset) = player.single() else {
        return;
    };
    let Ok(bar_entity) = tab_bar.single() else {
        return;
    };

    let mut tabs = vec![DiaryTab::Stats];
    tabs.extend(skillset.groups.iter().map(|g| DiaryTab::Group(g.kind)));
    tabs.push(DiaryTab::Abilities);

    // Rebuild only when the DERIVED tab list actually differs from last time
    // — a plain value comparison, not `Changed<NetSkillSet>` (BL-82 EM-5.7
    // follow-up: a live `--smoke-screenshot` showed the tab bar never
    // populating; `Changed<>` compares against THIS system's own last-run
    // tick, which raced against the listen-server's own local-replication
    // re-emit of the mirrored component and never lined up in practice).
    // This is the same "cheap value-diff, not ECS change-detection" posture
    // `xindeler_ui::notification`'s own doc comment already sanctions for
    // infrequently-open, non-hot-path widgets.
    if last_built.as_ref() == Some(&tabs) {
        return;
    }
    *last_built = Some(tabs.clone());

    rebuild_children(&mut commands, bar_entity, &children_query, |parent| {
        for tab in tabs {
            parent
                .spawn(xindeler_ui::button::button_bundle(
                    &theme,
                    &fonts,
                    &tab_label(tab),
                ))
                .insert(DiaryTabButton(tab))
                .observe(
                    move |activate: On<xindeler_ui::button::Activate>,
                          buttons: Query<&DiaryTabButton>,
                          mut selected: ResMut<DiaryTab>| {
                        if let Ok(button) = buttons.get(activate.entity) {
                            *selected = button.0;
                        }
                    },
                );
        }
    });
}

fn tab_label(tab: DiaryTab) -> String {
    match tab {
        DiaryTab::Stats => "Stats".to_owned(),
        DiaryTab::Abilities => "Abilities".to_owned(),
        DiaryTab::Group(kind) => group_label(kind),
    }
}

/// A short display label for a skill group — generic across every
/// [`SkillGroupKind`] variant (no per-class/per-weapon hardcoded table),
/// using `Debug` for the parts i18n doesn't cover yet (real i18n depth is
/// EM-5.16's job, matching the SAME "themed placeholder, deferred to the
/// epic that owns real i18n depth" posture EM-5.2's buff strip established).
fn group_label(kind: SkillGroupKind) -> String {
    match kind {
        SkillGroupKind::General => "General".to_owned(),
        SkillGroupKind::Feats => "Feats".to_owned(),
        SkillGroupKind::Weapon(tool) => format!("{tool:?}"),
        SkillGroupKind::Class(class) => format!("{class:?}"),
    }
}

/// Toggles the three content containers' [`Visibility`] to match the
/// currently-selected [`DiaryTab`].
/// Toggles the three content containers to match the currently-selected
/// [`DiaryTab`] — via `Node::display` (`Flex`/`None`), NOT `Visibility`.
///
/// ## Why `Display`, not `Visibility` (a real bug this fixed)
/// `Visibility::Hidden` only skips RENDERING an entity — it does NOT remove
/// it from `taffy`'s layout computation, so a `Row`-direction panel with all
/// three content containers as siblings (tab bar + Stats(720px) +
/// Tree(720px) + Abilities(720px)) laid out SIDE BY SIDE regardless of which
/// one is "hidden" summed to ~2300px wide — wider than the whole 1280px
/// viewport — which pushed the panel mostly/fully off-screen when centered
/// (`justify_content: Center` on the backdrop). A live `--smoke-screenshot`
/// caught this: the darkened backdrop rendered, but no panel content was
/// ever visible anywhere on screen. `Display::None` removes an entity from
/// layout entirely (zero size, as if it weren't there), so only the ONE
/// selected content container ever contributes to the row's width.
fn sync_tab_content_visibility(
    selected: Res<DiaryTab>,
    mut stats: Query<
        &mut Node,
        (
            With<StatsPanelRoot>,
            Without<TreeRoot>,
            Without<AbilitiesPanelRoot>,
        ),
    >,
    mut tree: Query<
        &mut Node,
        (
            With<TreeRoot>,
            Without<StatsPanelRoot>,
            Without<AbilitiesPanelRoot>,
        ),
    >,
    mut abilities: Query<
        &mut Node,
        (
            With<AbilitiesPanelRoot>,
            Without<StatsPanelRoot>,
            Without<TreeRoot>,
        ),
    >,
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
    if let Ok(mut node) = stats.single_mut() {
        node.display = display_for(matches!(*selected, DiaryTab::Stats));
    }
    if let Ok(mut node) = tree.single_mut() {
        node.display = display_for(matches!(*selected, DiaryTab::Group(_)));
    }
    if let Ok(mut node) = abilities.single_mut() {
        node.display = display_for(matches!(*selected, DiaryTab::Abilities));
    }
}

/// Rebuilds the Stats tab's text rows from the local player's already-real
/// combat-HUD mirrors (EM-5.2's `NetHealth`/`NetEnergy`/`NetPoise`/`NetXp`/
/// `NetCombo`/`NetBuffs`) — no NEW mirror needed for this tab (spec §2 EM-5.7
/// row: "+ stats from 5.2").
fn sync_stats_panel(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    selected: Res<DiaryTab>,
    player: Query<
        (
            Option<&NetHealth>,
            Option<&NetEnergy>,
            Option<&NetPoise>,
            Option<&NetXp>,
            Option<&NetCombo>,
            Option<&NetBuffs>,
        ),
        With<NetLocalPlayer>,
    >,
    root: Query<Entity, With<StatsPanelRoot>>,
    children_query: Query<&Children>,
) {
    if !matches!(*selected, DiaryTab::Stats) {
        // Only relevant while the Stats tab is actually selected. Rebuilds
        // every frame it IS selected (cheap: a handful of text rows) rather
        // than gating on `selected.is_changed()` — a live `--smoke-screenshot`
        // showed that gate racing against `DiaryTab`'s own resource-insertion
        // tick and never firing in practice (same class of bug
        // `sync_diary_tabs`'s own doc comment explains for `Changed<
        // NetSkillSet>`). This also gives the Stats tab a live-updating
        // snapshot while open, a strict improvement over the originally
        // intended "static, read on open" framing.
        return;
    }
    let Ok((health, energy, poise, xp, combo, buffs)) = player.single() else {
        return;
    };
    let Ok(root_entity) = root.single() else {
        return;
    };

    let mut lines = Vec::new();
    if let Some(xp) = xp {
        lines.push(format!("Level {}", xp.level));
        lines.push(format!("XP {}/{}", xp.xp_into_level, xp.xp_for_level));
    }
    if let Some(health) = health {
        lines.push(format!("Health {:.0}/{:.0}", health.current, health.max));
    }
    if let Some(energy) = energy {
        lines.push(format!("Energy {:.0}/{:.0}", energy.current, energy.max));
    }
    if let Some(poise) = poise {
        lines.push(format!("Poise {:.0}/{:.0}", poise.current, poise.max));
    }
    if let Some(combo) = combo
        && combo.counter > 0
    {
        lines.push(format!("Combo {}", combo.counter));
    }
    if let Some(buffs) = buffs {
        for entry in &buffs.0 {
            lines.push(format!("{:?} x{}", entry.kind, entry.stacks));
        }
    }

    rebuild_children(&mut commands, root_entity, &children_query, |parent| {
        for line in lines {
            parent.spawn((
                Text(line),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(18.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
            ));
        }
    });
}

/// Fires on a skill-tree node's [`Activate`] (click/keyboard-activate, via
/// `bevy_ui_widgets`' headless button behaviour — same primitive the tab bar
/// buttons above use): the real SP-spend action (T56.23). The sim is the
/// SOLE source of truth on whether this spend is legal (prerequisites/cost/
/// availability) — see `EmbeddedPlayer::unlock_skill`'s own doc comment —
/// so this always fires the request, never gates client-side; a stale/racy
/// mirror can never desync the button from what the server actually allows.
fn handle_skill_node_activate(
    activate: On<Activate>,
    targets: Query<&SkillNodeTarget>,
    mut requests: MessageWriter<LocalUnlockSkillRequest>,
) {
    if let Ok(target) = targets.get(activate.entity) {
        requests.write(LocalUnlockSkillRequest(target.0));
    }
}

/// Rebuilds the tree grid for the currently-selected [`DiaryTab::Group`]
/// whenever the selection OR the local player's [`NetSkillSet`] changes — the
/// ONE generic renderer BL-06 established, now covering every group instead
/// of just Class. *Verify (T56.23):* spend an SP -> the passive/ability
/// applies server-side and this tab's border colour flips from "available"
/// to a level-appropriate shade next mirror tick.
fn sync_skill_tree_content(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    shape: Res<SkillTreeShape>,
    selected: Res<DiaryTab>,
    player: Query<&NetSkillSet, With<NetLocalPlayer>>,
    root: Query<Entity, With<TreeRoot>>,
    children_query: Query<&Children>,
    mut last_built: Local<Option<(SkillGroupKind, Vec<(Skill, u16)>)>>,
) {
    let DiaryTab::Group(kind) = *selected else {
        return;
    };
    let Ok(skillset) = player.single() else {
        return;
    };
    let Ok(root_entity) = root.single() else {
        return;
    };

    // Rebuild only when `(selected group, unlocked-skill snapshot)` actually
    // differs from last time — a plain value comparison, not `Changed<
    // NetSkillSet>`/`is_changed()` (BL-82 EM-5.7 follow-up: see
    // `sync_diary_tabs`'s own doc comment for why ECS change-detection
    // raced against the listen-server's local-replication re-emit and never
    // fired in a live `--smoke-screenshot`).
    let mut snapshot = skillset.skills.clone();
    snapshot.sort_by_key(|(skill, _)| format!("{skill:?}"));
    let key = (kind, snapshot);
    if last_built.as_ref() == Some(&key) {
        return;
    }
    *last_built = Some(key);

    let unlocked: HashMap<Skill, u16> = skillset.skills.iter().copied().collect();
    let skills = shape.skills_in_group(kind).to_vec();
    if skills.is_empty() {
        rebuild_children(&mut commands, root_entity, &children_query, |_parent| {});
        return;
    }

    let tiers: Vec<u8> = skills.iter().map(|&s| shape.tier(s, 0)).collect();
    let max_tier = tiers.iter().copied().max().unwrap_or(0);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); usize::from(max_tier) + 1];
    for (idx, &tier) in tiers.iter().enumerate() {
        rows[usize::from(tier)].push(idx);
    }

    rebuild_children(&mut commands, root_entity, &children_query, |parent| {
        for (tier_idx, row_indices) in rows.iter().enumerate() {
            let row_count = row_indices.len();
            let total_row_w = (row_count as f32 - 1.0).max(0.0) * TREE_COL_W;
            let row_x_start = ((TREE_W - total_row_w) / 2.0).max(0.0);
            #[expect(
                clippy::cast_precision_loss,
                reason = "tier index is a handful of rows, never near f32's precision limit"
            )]
            let y = TREE_TOP_MARGIN + tier_idx as f32 * TREE_ROW_H;

            for (col, &skill_idx) in row_indices.iter().enumerate() {
                let skill = skills[skill_idx];
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "column index is a handful per row"
                )]
                let x = row_x_start + col as f32 * TREE_COL_W;

                let level = unlocked.get(&skill).copied().unwrap_or(0);
                let max = shape.max_level(skill);
                let kind_note = if shape.is_passive(skill) {
                    " (passive)"
                } else {
                    ""
                };
                let (border, tooltip) = if level >= max {
                    (
                        theme.palette.buff_good,
                        format!("{skill:?}{kind_note}\nMaxed ({level}/{max})"),
                    )
                } else if shape.prerequisites_met(skill, &unlocked) {
                    let cost = skill.skill_cost(level + 1);
                    (
                        theme.palette.accent,
                        format!("{skill:?}{kind_note}\nLevel {level}/{max}\nCost: {cost} SP"),
                    )
                } else {
                    (
                        theme.palette.text_muted,
                        format!("{skill:?}{kind_note}\nLocked (prerequisites not met)"),
                    )
                };

                let mut node_entity =
                    parent.spawn(button_bundle(&theme, &fonts, &skill_glyph(skill)));
                node_entity.entry::<Node>().and_modify(move |mut node| {
                    node.position_type = PositionType::Absolute;
                    node.top = Val::Px(y);
                    node.left = Val::Px(x);
                    node.width = Val::Px(TREE_NODE_SIZE);
                    node.height = Val::Px(TREE_NODE_SIZE);
                    node.padding = UiRect::ZERO;
                });
                node_entity.insert((
                    BorderColor::all(border),
                    Tooltip { text: tooltip },
                    SkillNodeTarget(skill),
                ));
                node_entity.observe(handle_skill_node_activate);
            }
        }
    });
}

/// A short, generic glyph for any [`Skill`] variant, derived from its
/// `Debug` text (no per-skill hardcoded table — matches
/// `xindeler-client::hotbar::short_glyph`'s own "derive from a string, don't
/// enumerate every case" philosophy, applied to `Skill` instead of an
/// ability-id string).
fn skill_glyph(skill: Skill) -> String {
    let debug = format!("{skill:?}");
    let inner = debug.rsplit('(').next().unwrap_or(&debug);
    let inner = inner.trim_end_matches(')');
    let ident = inner
        .split(|c: char| !c.is_alphanumeric())
        .find(|s| !s.is_empty())
        .unwrap_or(inner);
    let mut glyph: String = ident.chars().take(4).collect();
    glyph.make_ascii_uppercase();
    if glyph.is_empty() {
        "SKL".to_owned()
    } else {
        glyph
    }
}

/// Rebuilds the Abilities tab's slot grid from the local player's real
/// [`NetAbilityPool`] whenever it changes — each entry is a real
/// [`xindeler_ui::slot`] drag SOURCE (`DIARY_ABILITY_GROUP`); dragging one
/// onto a hotbar slot is handled by `crate::hotbar`'s existing drop handler
/// (extended to accept this group — see this module's own doc comment).
fn sync_abilities_tab(
    mut commands: Commands,
    theme: Res<HudTheme>,
    player: Query<&NetAbilityPool, With<NetLocalPlayer>>,
    root: Query<Entity, With<AbilitiesPanelRoot>>,
    children_query: Query<&Children>,
    mut last_built: Local<Option<NetAbilityPool>>,
) {
    let Ok(pool) = player.single() else {
        return;
    };
    let Ok(root_entity) = root.single() else {
        return;
    };

    // Value-diff, not `Changed<NetAbilityPool>` — see `sync_diary_tabs`'s own
    // doc comment for why ECS change-detection races against the
    // listen-server's local-replication re-emit for a replicated component.
    if last_built.as_ref() == Some(pool) {
        return;
    }
    *last_built = Some(pool.clone());

    rebuild_children(&mut commands, root_entity, &children_query, |parent| {
        for slot in &pool.0 {
            let address = SlotAddress(slot.aux.to_slot_address_raw());
            let icon_text = slot
                .ability_id
                .as_deref()
                .map(ability_glyph)
                .unwrap_or_default();
            let tooltip = slot.ability_id.clone().unwrap_or_default();
            parent
                .spawn(slot_bundle(
                    &theme,
                    DIARY_ABILITY_GROUP,
                    address,
                    ABILITY_SLOT_SIZE_PX,
                ))
                .insert(SlotContents {
                    icon_text,
                    quantity: None,
                    tooltip,
                });
        }
    });
}

/// A short glyph for an ability id string (last dotted segment, uppercased) —
/// the SAME derivation `xindeler-client::hotbar::short_glyph` uses,
/// duplicated (not imported across modules) to keep each screen's icon-glyph
/// logic self-contained, matching how `xindeler-render-voxel`'s manifest
/// readers are duplicated per figure rather than shared.
fn ability_glyph(ability_id: &str) -> String {
    let segment = ability_id.rsplit('.').next().unwrap_or(ability_id);
    let mut glyph: String = segment.chars().take(4).collect();
    glyph.make_ascii_uppercase();
    glyph
}

#[cfg(test)]
mod tests {
    // `SkillPrerequisite::All`/`Any`'s inner map is `hashbrown::HashMap`
    // (`common`'s own internal choice, `common/src/comp/skillset/mod.rs`),
    // NOT `std::collections::HashMap` (this module's own `HashMap`, imported
    // above for `SkillTreeShape`'s fields and `prerequisites_met`'s
    // parameter) — aliased so both are usable in this module without one
    // shadowing the other.
    use common::comp::skillset::skills::WarriorSkill;
    use hashbrown::HashMap as PrereqMap;

    use super::*;

    fn shape_with(
        groups: &[(SkillGroupKind, Vec<Skill>)],
        prereqs: &[(Skill, SkillPrerequisite)],
    ) -> SkillTreeShape {
        SkillTreeShape {
            groups: groups.iter().cloned().collect(),
            prerequisites: prereqs.iter().cloned().collect(),
            max_levels: HashMap::new(),
            class_modifiers: HashMap::new(),
            feat_modifiers: HashMap::new(),
        }
    }

    /// A skill with no prerequisite entry is tier 0 — the root of every tree.
    #[test]
    fn a_skill_with_no_prerequisites_is_tier_zero() {
        let shape = shape_with(&[], &[]);
        assert_eq!(shape.tier(Skill::Warrior(WarriorSkill::Rally), 0), 0);
    }

    /// A skill whose prerequisite is itself tier 0 is tier 1, and so on —
    /// mirrors BL-06's own `class_skill_tier` recursion exactly.
    #[test]
    fn tier_is_one_plus_the_deepest_prerequisite() {
        let root = Skill::UnlockGroup(SkillGroupKind::General);
        let mid = Skill::Warrior(WarriorSkill::Rally);
        let leaf = Skill::Warrior(WarriorSkill::Onslaught);
        let shape = shape_with(&[], &[
            (mid, SkillPrerequisite::All(PrereqMap::from([(root, 1)]))),
            (leaf, SkillPrerequisite::All(PrereqMap::from([(mid, 1)]))),
        ]);
        assert_eq!(shape.tier(root, 0), 0);
        assert_eq!(shape.tier(mid, 0), 1);
        assert_eq!(shape.tier(leaf, 0), 2);
    }

    /// `prerequisites_met` mirrors `SkillSet::prerequisites_met`'s `All`/`Any`
    /// semantics against a plain unlocked-skill map (no live sim `SkillSet`
    /// needed client-side).
    #[test]
    fn prerequisites_met_respects_all_and_any() {
        let a = Skill::Warrior(WarriorSkill::Rally);
        let b = Skill::Warrior(WarriorSkill::Onslaught);
        let target_all = Skill::UnlockGroup(SkillGroupKind::General);
        let shape = shape_with(&[], &[(
            target_all,
            SkillPrerequisite::All(PrereqMap::from([(a, 1), (b, 1)])),
        )]);

        let mut unlocked = HashMap::new();
        assert!(!shape.prerequisites_met(target_all, &unlocked));
        unlocked.insert(a, 1);
        assert!(
            !shape.prerequisites_met(target_all, &unlocked),
            "All requires BOTH"
        );
        unlocked.insert(b, 1);
        assert!(shape.prerequisites_met(target_all, &unlocked));
    }

    /// A skill absent from the max-levels manifest defaults to max level 1 —
    /// mirrors `Skill::max_level`'s own fallback.
    #[test]
    fn max_level_defaults_to_one_when_absent() {
        let shape = shape_with(&[], &[]);
        assert_eq!(shape.max_level(Skill::Warrior(WarriorSkill::Rally)), 1);
    }

    /// [`skill_glyph`] derives a short, non-empty glyph from any `Skill`
    /// variant's `Debug` text without a hardcoded per-skill table.
    #[test]
    fn skill_glyph_is_short_and_nonempty() {
        let glyph = skill_glyph(Skill::Warrior(WarriorSkill::Rally));
        assert!(!glyph.is_empty());
        assert!(glyph.len() <= 4);
        assert_eq!(glyph, glyph.to_uppercase());
    }

    /// [`ability_glyph`] takes the last dotted segment, uppercased — same
    /// derivation as `hotbar::short_glyph`.
    #[test]
    fn ability_glyph_takes_last_dotted_segment_uppercased() {
        assert_eq!(ability_glyph("class.warrior.rally"), "RALL");
    }

    /// T56.23 acceptance, exercised directly (no real pointer/click needed):
    /// activating a skill-tree node writes exactly one
    /// [`LocalUnlockSkillRequest`] for THAT node's [`Skill`] — the real
    /// SP-spend action a live `--smoke-screenshot` capture alone could never
    /// prove (it only shows the tree renders; this is the click's actual
    /// effect). `World::trigger` fires the SAME [`Activate`] `EntityEvent`
    /// `bevy_ui_widgets`' real click/keyboard-activate path fires, so this
    /// exercises the observer exactly as spawned, not a re-implementation of
    /// it.
    #[test]
    fn activating_a_skill_node_writes_an_unlock_request_for_that_skill() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<LocalUnlockSkillRequest>();

        let skill = Skill::Warrior(WarriorSkill::Rally);
        let node = app.world_mut().spawn(SkillNodeTarget(skill)).id();
        app.world_mut()
            .entity_mut(node)
            .observe(handle_skill_node_activate);

        app.world_mut().trigger(Activate { entity: node });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<LocalUnlockSkillRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![LocalUnlockSkillRequest(skill)]);
    }

    /// A click on an entity that does NOT carry [`SkillNodeTarget`] (should
    /// never happen in practice — every tree node spawn inserts it — but the
    /// handler's own `Ok(target)` guard is the only thing preventing a panic
    /// if it ever did) writes nothing.
    #[test]
    fn activating_a_node_without_a_target_writes_nothing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<LocalUnlockSkillRequest>();

        let node = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(node)
            .observe(handle_skill_node_activate);

        app.world_mut().trigger(Activate { entity: node });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<LocalUnlockSkillRequest>>()
            .drain()
            .collect();
        assert!(sent.is_empty());
    }
}
