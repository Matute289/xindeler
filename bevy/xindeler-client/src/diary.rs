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
//! ## HUD-D4 reskin (BL-82 EM-5.17 Phase 6, spec §3.6/§4.3)
//! The spec's own explicit synthesis point: the Notion HUD-D4 doc's own
//! illustrative `SkillId`/`SkillNode`/`PlayerSkillTree` example (a fixed
//! 15-node teaching example) is NOT ported here — it predates and is
//! materially simpler than the real, already-shipped [`SkillTreeShape`]/
//! tier/prerequisite machinery above; using it would be a regression. Phase
//! 6 reskins ONLY the render layer: [`spawn_diary_window`] swaps the flat
//! panel background for the "Path of Ascension" parchment (T57.11), and
//! [`sync_skill_tree_content`] gained a connector-line pass between the
//! SAME already-computed node positions (T57.12, see [`connector_segment`]/
//! [`spawn_connector_line`]). No new data model, no change to the
//! tier/prerequisite/unlock logic above.
//!
//! Compiled only under `listen-server`/`net-client` — same posture as every
//! other `xindeler_protocol`-consuming module in this crate.

use std::{collections::HashMap, path::PathBuf};

use bevy::{
    ecs::{change_detection::NonSend, schedule::common_conditions::not},
    prelude::*,
};
use common::comp::{
    ClassKind,
    skillset::{
        SkillGroupKind, SkillPrerequisite,
        skills::{ClassPassiveStat, Skill},
    },
    tool::ToolKind,
};
use xindeler_input::{ActionState, GameInput};
use xindeler_protocol::{
    NetAbilityPool, NetBuffs, NetCombo, NetEnergy, NetHealth, NetLocalPlayer, NetPoise,
    NetSkillSet, NetXp, UnlockSkillRequest,
};

use crate::chat::text_input_focused;
use xindeler_ui::{
    button::{Activate, button_bundle},
    hud_state::{HudAction, HudState, HudWindow},
    i18n::{CurrentLocale, Localization, LocalizedLabel},
    images::{HudImageKey, HudImages},
    panel::image_panel_bundle,
    scroll::scroll_view_bundle,
    slot::{SlotAddress, SlotContents, SlotGroup, slot_bundle},
    theme::{HudFonts, HudTheme},
    tooltip::{Tooltip, TooltipBackground},
    zlayer,
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

    /// The DIRECT prerequisite skills for `skill` — the union of `All`'s/
    /// `Any`'s key set, prerequisite LEVEL discarded (BL-82 EM-5.17 Phase 6,
    /// T57.12: the connector-line renderer only needs to know WHICH nodes
    /// are linked; [`Self::prerequisites_met`] already does the
    /// level-aware check separately, for a different purpose). Returns a
    /// `Vec` rather than an iterator since callers need to look each entry
    /// up in a position table; the prerequisite set is always small (a
    /// handful of entries), never a hot-path allocation.
    fn direct_prerequisites(&self, skill: Skill) -> Vec<Skill> {
        match self.prerequisites.get(&skill) {
            Some(SkillPrerequisite::All(map) | SkillPrerequisite::Any(map)) => {
                map.keys().copied().collect()
            },
            None => Vec::new(),
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
                    // BL-82 EM-5.17 Phase 6: reads `Res<HudImages>` (the
                    // "Path of Ascension" parchment background) alongside
                    // `Res<HudTheme>` — ordered after BOTH `Startup`
                    // resource-inserting systems, mirroring the existing
                    // `.after(theme::init_theme)` convention this system
                    // already followed.
                    spawn_diary_window
                        .after(xindeler_ui::theme::init_theme)
                        .after(xindeler_ui::images::init_images),
                    force_open_diary_for_smoke_capture,
                ),
            )
            .add_systems(
                Update,
                (
                    // Reads `ActionState` — must run after the frame's real
                    // input resolution (BL-82 EM-5.17 Phase 0: this system
                    // had no ordering constraint, so the scheduler could run
                    // it BEFORE `InputResolveSet` cleared/rebuilt
                    // `just_pressed`, missing the 'P' press edge on some
                    // frames — the "sometimes doesn't respond" symptom).
                    // Mirrors `controls_screen::toggle_controls_screen` and
                    // `camera`'s own `ActionState`-reading systems. Also
                    // gated on `!text_input_focused` (BL-82 EM-5.17 Phase 0)
                    // so typing "p" in the chat box doesn't ALSO open the
                    // Diary — see `chat::text_input_focused`'s doc comment.
                    toggle_diary_window
                        .after(xindeler_input::InputResolveSet)
                        .run_if(not(text_input_focused)),
                    sync_diary_window_visibility,
                    sync_diary_tabs,
                    force_select_class_tab_for_smoke_capture,
                    force_select_abilities_tab_for_smoke_capture,
                    sync_tab_content_visibility,
                    // BL-82 EM-5.16 (T56.44 follow-up, bevy-migration-reviewer
                    // finding): `sync_stats_panel` reads `NonSend<Localization>`
                    // unconditionally every frame while the Stats tab is open
                    // (self-correcting, worst case one stale frame on a locale
                    // switch), and `sync_skill_tree_content` folds the locale
                    // tag into its own `Local` rebuild-cache key — both need
                    // the SAME `.after(LocaleSyncSet)` edge `settings_window.
                    // rs`'s `refresh_setting_labels` documents as load-bearing
                    // (no ordering guarantee between two systems with
                    // conflicting `NonSend`/`NonSendMut` `Localization` access
                    // absent one); for `sync_skill_tree_content` specifically,
                    // without it a locale switch could read the stale bundle
                    // once and then never rebuild again (the cache-key gate is
                    // now satisfied).
                    sync_stats_panel.after(xindeler_ui::i18n::LocaleSyncSet),
                    sync_skill_tree_content.after(xindeler_ui::i18n::LocaleSyncSet),
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
///
/// BL-82 EM-5.17 Phase 6 (T57.11, spec §3.6): the panel's flat
/// `panel_bundle` background is swapped for [`image_panel_bundle`] rendering
/// [`HudImageKey::SkillTreeBg`] (the "SKILL TREE — PATH OF ASCENSION"
/// parchment) — a render-layer-only change, everything below this point
/// (tab bar, the three content containers, their `Visibility`/`Display`
/// gating) is untouched. **`SkillTreeBg`, not `OtherSkillTreeBg`, for every
/// tab** (Stats/Abilities/every `Group`, not just `Class`): the spec's own
/// §6 Q5 resolution picks `skill_tree_bg.png` as Phase 6's default (the one
/// asset the Notion doc's own reference code uses), and this window has
/// exactly ONE shared panel background across all tabs — swapping it
/// per-tab would need new per-tab-change tracking for a visual-only nuance
/// the spec explicitly left non-blocking; `OtherSkillTreeBg` stays wired
/// into [`HudImageKey`] for a later phase to pick up if Matías wants a
/// per-group-kind variant. The whole window also gets
/// [`zlayer::MODAL_WINDOWS`] (spec §4.4) — the FIRST `GlobalZIndex` applied
/// anywhere in this HUD (spec §1.1's "no `ZIndex`/`GlobalZIndex` anywhere"
/// gap), since the diary is exactly the kind of modal window that scheme
/// exists for.
fn spawn_diary_window(mut commands: Commands, theme: Res<HudTheme>, hud_images: Res<HudImages>) {
    commands
        .spawn((
            DiaryWindowRoot,
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
            let mut panel_entity = backdrop.spawn(image_panel_bundle(
                &theme,
                hud_images.get(HudImageKey::SkillTreeBg),
            ));
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
                // comment) — `Visibility` is `Inherited` on all three, NOT
                // `Visible`. An earlier version of this fix used
                // `Visibility::Visible` here (chasing a real bug:
                // `Visibility::Hidden` gates PAINTING independent of
                // `Node::display`/layout, so a panel spawned Hidden would
                // never render even once its `display` flipped to `Flex`,
                // since nothing in this module ever touches `Visibility`
                // again after spawn — only `Display` does. A live
                // `--smoke-screenshot` of the Warrior tree tab caught this:
                // `sync_skill_tree_content` really did build all 12 real
                // skill nodes and `sync_tab_content_visibility` really did
                // flip `TreeRoot`'s `Node::display` to `Flex`, yet NOTHING
                // painted — the tree area just showed the world through the
                // backdrop). But `Visibility::Visible` FORCE-OVERRIDES the
                // ancestor (`DiaryWindowRoot`)'s `Visibility::Hidden` — it
                // does NOT mean "inherit from parent", that's what
                // `Inherited` means — so these three panels kept painting
                // even while the whole Diary window was supposedly closed
                // (BL-82 EM-5.17 Phase 0: a live `--smoke-screenshot` capture
                // with the Diary never opened showed `StatsPanelRoot`'s
                // "Level 1 / XP 0/250 / …" text floating above the chat
                // panel — the "duplicate Lv.1" bug). `Inherited` still
                // paints once the ancestor becomes `Visible` (the window
                // opens) and does NOT paint while the ancestor is `Hidden`
                // (the window is closed) — exactly the behaviour both fixes
                // were reaching for.
                panel.spawn((StatsPanelRoot, Visibility::Inherited, Node {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    width: Val::Px(TREE_W),
                    ..Default::default()
                }));
                panel.spawn((TreeRoot, Visibility::Inherited, Node {
                    display: Display::None,
                    position_type: PositionType::Relative,
                    width: Val::Px(TREE_W),
                    height: Val::Px(TREE_H),
                    ..Default::default()
                }));
                let mut abilities_entity = panel.spawn((
                    AbilitiesPanelRoot,
                    Visibility::Inherited,
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
    localization: NonSend<Localization>,
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
            let mut button = parent.spawn(xindeler_ui::button::button_bundle(
                &theme,
                &fonts,
                &tab_label(tab, &localization),
            ));
            // BL-82 EM-5.16 (T56.44 follow-up): tag with `LocalizedLabel` when
            // this tab maps to a real `.ftl` key (every fixed tab + every
            // group this screen has a translated name for today) so it
            // re-resolves live on a locale switch via the shared
            // `xindeler_ui::i18n::relocalize_button_labels` system — this
            // system's own rebuild gate (the derived tab-LIST identity) never
            // fires on a bare locale change, so without the tag a language
            // switch would leave an already-open tab bar showing stale text
            // until the local player's group set itself changes. A tab with
            // no mapped key (an as-yet-untranslated `Weapon` kind — see
            // `weapon_group_label_key`) falls back to its raw `Debug` name,
            // which never changes with locale, so it is deliberately left
            // untagged.
            if let Some(key) = tab_label_key(tab) {
                button.insert(LocalizedLabel(key));
            }
            button.insert(DiaryTabButton(tab)).observe(
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

/// The `.ftl` key for a tab's label, when this screen has a real translated
/// name for it — `None` only for a [`SkillGroupKind::Weapon`] tool kind this
/// screen doesn't have a mapped key for yet (see [`weapon_group_label_key`]),
/// which falls back to its raw `Debug` name in [`tab_label`].
fn tab_label_key(tab: DiaryTab) -> Option<&'static str> {
    match tab {
        // Reuses the legacy `DiarySection::Character` key: this tab shows
        // the SAME content (level/XP/health/energy/poise/combo/buffs) that
        // section names "Character", not "Stats" — see this fn's own call
        // site doc comment.
        DiaryTab::Stats => Some("hud-diary-sections-character-title"),
        DiaryTab::Abilities => Some("hud-diary-sections-abilities-title"),
        DiaryTab::Group(SkillGroupKind::General) => Some("hud-skill_tree-general"),
        DiaryTab::Group(SkillGroupKind::Feats) => Some("hud-skill_tree-feats"),
        DiaryTab::Group(SkillGroupKind::Weapon(tool)) => weapon_group_label_key(tool),
        DiaryTab::Group(SkillGroupKind::Class(class)) => Some(class_label_key(class)),
    }
}

/// The `.ftl` key for a weapon skill group's tab label — covers every
/// [`ToolKind`] that actually owns a skill-tree manifest today (`skills.ftl`'s
/// `hud-skill_tree-*` keys); any other tool kind (never actually reached as a
/// real [`SkillGroupKind::Weapon`] in the shipped content, but kept `None`
/// rather than assumed-unreachable so this stays total) falls back to its raw
/// `Debug` name.
fn weapon_group_label_key(tool: ToolKind) -> Option<&'static str> {
    match tool {
        ToolKind::Sword => Some("hud-skill_tree-sword"),
        ToolKind::Axe => Some("hud-skill_tree-axe"),
        ToolKind::Hammer => Some("hud-skill_tree-hammer"),
        ToolKind::Bow => Some("hud-skill_tree-bow"),
        ToolKind::Staff => Some("hud-skill_tree-staff"),
        ToolKind::Sceptre => Some("hud-skill_tree-sceptre"),
        ToolKind::Pick => Some("hud-skill_tree-mining"),
        _ => None,
    }
}

/// The `.ftl` key for a class skill group's tab label — `common-class-*`,
/// already shipped for every [`ClassKind`] variant (the SAME keys
/// `char_select.rs`'s class picker already resolves — see that module for
/// the other consumer of this exact table).
fn class_label_key(class: ClassKind) -> &'static str {
    match class {
        ClassKind::Adventurer => "common-class-adventurer",
        ClassKind::Warrior => "common-class-warrior",
        ClassKind::Mage => "common-class-mage",
        ClassKind::Cleric => "common-class-cleric",
        ClassKind::Rogue => "common-class-rogue",
        ClassKind::Barbarian => "common-class-barbarian",
        ClassKind::Sorcerer => "common-class-sorcerer",
        ClassKind::Warlock => "common-class-warlock",
        ClassKind::Bard => "common-class-bard",
        ClassKind::Paladin => "common-class-paladin",
        ClassKind::Druid => "common-class-druid",
        ClassKind::Ranger => "common-class-ranger",
        ClassKind::Monk => "common-class-monk",
        ClassKind::Artificer => "common-class-artificer",
        ClassKind::BloodSlayer => "common-class-blood_slayer",
    }
}

/// A display label for a diary tab, resolved through the active locale. Falls
/// back to the tab's raw `Debug` name only for a [`SkillGroupKind::Weapon`]
/// tool kind [`tab_label_key`] has no mapped key for yet (never a REAL
/// regression from this screen's earlier fully-generic posture: every kind
/// that tool kind covers today already has a key).
fn tab_label(tab: DiaryTab, localization: &Localization) -> String {
    match tab_label_key(tab) {
        Some(key) => localization.tr(key),
        None => match tab {
            DiaryTab::Group(SkillGroupKind::Weapon(tool)) => format!("{tool:?}"),
            // Unreachable in practice (every other `DiaryTab` variant always
            // resolves `Some` above) — degrade to the group's own `Debug`
            // text rather than panic, matching this module's "never crash
            // the HUD over a display nuance" posture.
            DiaryTab::Group(kind) => format!("{kind:?}"),
            DiaryTab::Stats | DiaryTab::Abilities => {
                unreachable!("tab_label_key always returns Some for Stats/Abilities")
            },
        },
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
    localization: NonSend<Localization>,
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

    // BL-82 EM-5.16 (T56.44 follow-up): each row's LABEL word is resolved
    // through the active locale; the numbers themselves are plain data, not
    // Fluent placeables (`Localization::tr` has no `{ $var }` interpolation
    // support — see that type's own doc comment), so each line is built as
    // "translated label" + a plain Rust-formatted value, the same split
    // `settings_window.rs`'s `value_label`/numeric rows use. This system
    // already rebuilds every frame the Stats tab is selected (see this
    // function's own doc comment), so a locale switch while it's open is
    // picked up on the very next frame with no extra hot-swap wiring needed.
    let mut lines = Vec::new();
    if let Some(xp) = xp {
        lines.push(format!(
            "{} {}",
            localization.tr("character_window-character_level"),
            xp.level
        ));
        lines.push(format!(
            "{} {}/{}",
            localization.tr("character_window-character_xp"),
            xp.xp_into_level,
            xp.xp_for_level
        ));
    }
    if let Some(health) = health {
        lines.push(format!(
            "{} {:.0}/{:.0}",
            localization.tr("character_window-character_health"),
            health.current,
            health.max
        ));
    }
    if let Some(energy) = energy {
        lines.push(format!(
            "{} {:.0}/{:.0}",
            localization.tr("character_window-character_energy"),
            energy.current,
            energy.max
        ));
    }
    if let Some(poise) = poise {
        lines.push(format!(
            "{} {:.0}/{:.0}",
            localization.tr("character_window-character_poise"),
            poise.current,
            poise.max
        ));
    }
    if let Some(combo) = combo
        && combo.counter > 0
    {
        lines.push(format!(
            "{} {}",
            localization.tr("character_window-character_combo"),
            combo.counter
        ));
    }
    if let Some(buffs) = buffs {
        // BL-82 EM-5.16 close-out: resolves through the same
        // `crate::buff_i18n::buff_i18n_key` table `combat_hud.rs`'s buff
        // strip uses, instead of a raw `{:?}` Debug identifier.
        for entry in &buffs.0 {
            lines.push(format!(
                "{} x{}",
                localization.tr(crate::buff_i18n::buff_i18n_key(entry.kind)),
                entry.stacks
            ));
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
    mut requests: MessageWriter<UnlockSkillRequest>,
) {
    if let Ok(target) = targets.get(activate.entity) {
        // BL-82 EM-8.3: the real replicon client message (unified write path),
        // not a listen-server-only `UnlockSkillRequest` — surfaces on the
        // bridge as `FromClient<_>` for a genuinely-remote dedicated-server
        // client AND the listen-server's own `ClientId::Server` local echo.
        requests.write(UnlockSkillRequest(target.0));
    }
}

/// The connector line `Node`'s fixed height in pixels (BL-82 EM-5.17 Phase
/// 6, T57.12) — `skill_line_active.png`/`skill_line_locked.png` are both a
/// horizontal chain motif on a near-square canvas with black padding above/
/// below (verified directly: `skill_line_active.png` is 711×692px,
/// `skill_line_locked.png` 699×606px, chain content only in the vertical
/// middle band); stretching the whole canvas down to this thickness (rather
/// than cropping) keeps the chain-link art intact while collapsing the
/// black padding. Tunable once seen live (matches this module's own
/// `TREE_*` constants' "tunable for visual polish" precedent).
const CONNECTOR_THICKNESS_PX: f32 = 28.0;

/// A connector segment between two node centers — the pure geometry behind
/// a connector `Node` sized to the segment length with a [`UiTransform`]
/// rotation (BL-82 EM-5.17 Phase 6, spec §3.6/§4.3). Factored out as a pure
/// fn (no ECS/asset access) so the geometry itself is unit-testable without
/// a running `App` — the SAME "compute geometry as a pure fn, test it
/// directly" pattern `map_view::heading_from_forward`/`wpos_to_screen_uv`
/// already establish in this crate.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ConnectorSegment {
    /// The segment's midpoint, in the SAME `TreeRoot`-local pixel space the
    /// node-spawn loop's `x`/`y` already use.
    midpoint: Vec2,
    /// The segment's length in pixels — the connector `Node`'s `width`.
    length: f32,
    /// Radians, clockwise from the local +x axis — [`UiTransform::rotation`]'s
    /// documented "rotate clockwise" convention. Screen space is y-down, so
    /// `dy.atan2(dx)` directly yields that clockwise angle — the SAME sign
    /// convention `map_view::sync_minimap`'s own `Rot2::radians(heading)`
    /// already relies on for the player-arrow icon.
    angle: f32,
}

/// Computes the [`ConnectorSegment`] between two node centers `a`/`b`.
fn connector_segment(a: Vec2, b: Vec2) -> ConnectorSegment {
    let delta = b - a;
    ConnectorSegment {
        midpoint: (a + b) * 0.5,
        length: delta.length(),
        angle: delta.y.atan2(delta.x),
    }
}

/// `SkillLineActive` (the "glowing molten chain, unlocked/invested" asset,
/// spec §3.6) if the edge's target (child) skill has at least one level
/// invested, else `SkillLineLocked` ("dull rusted chain, locked"). A pure fn
/// (no [`HudImages`] access) so the glow/dull DECISION is independently
/// unit-testable from the real asset lookup.
fn connector_image_key(target_unlocked: bool) -> HudImageKey {
    if target_unlocked {
        HudImageKey::SkillLineActive
    } else {
        HudImageKey::SkillLineLocked
    }
}

/// Spawns one connector-line `Node`: sized to the segment length, positioned
/// so its (unrotated) box is centred on the segment midpoint, then rotated
/// via [`UiTransform`] to the segment's angle. `bevy_ui`'s layout system
/// rotates a node around its OWN computed center (confirmed against
/// `bevy_ui` 0.19's `ui_layout_system`, which adds the node's local center
/// to the `UiTransform`-derived affine transform) — the SAME "position the
/// box centred on the target point, then rotate in place" idiom
/// `map_view`'s minimap player-arrow already uses for its own `UiTransform`.
fn spawn_connector_line(
    parent: &mut ChildSpawnerCommands,
    hud_images: &HudImages,
    from: Vec2,
    to: Vec2,
    target_unlocked: bool,
) {
    let segment = connector_segment(from, to);
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(segment.midpoint.x - segment.length / 2.0),
            top: Val::Px(segment.midpoint.y - CONNECTOR_THICKNESS_PX / 2.0),
            width: Val::Px(segment.length),
            height: Val::Px(CONNECTOR_THICKNESS_PX),
            ..Default::default()
        },
        UiTransform::from_rotation(Rot2::radians(segment.angle)),
        ImageNode {
            image: hud_images.get(connector_image_key(target_unlocked)),
            image_mode: NodeImageMode::Stretch,
            ..Default::default()
        },
    ));
}

/// Rebuilds the tree grid for the currently-selected [`DiaryTab::Group`]
/// whenever the selection OR the local player's [`NetSkillSet`] changes — the
/// ONE generic renderer BL-06 established, now covering every group instead
/// of just Class. *Verify (T56.23):* spend an SP -> the passive/ability
/// applies server-side and this tab's border colour flips from "available"
/// to a level-appropriate shade next mirror tick.
///
/// BL-82 EM-5.17 Phase 6 (T57.12, spec §3.6/§4.3): also renders a connector
/// line for every prerequisite edge in the CURRENTLY VISIBLE tab (both
/// endpoints must have a node in `positions` — a prerequisite living in a
/// different group, e.g. a `Skill::UnlockGroup` gate, has no node here to
/// draw a line to, and is simply skipped) — see [`connector_segment`] and
/// [`spawn_connector_line`]'s own doc comments for the geometry/asset-key
/// decisions. The tree's own tier/row/prerequisite DATA MODEL is completely
/// unchanged; only this rendering pass gained the connector lines.
fn sync_skill_tree_content(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    hud_images: Res<HudImages>,
    shape: Res<SkillTreeShape>,
    selected: Res<DiaryTab>,
    current_locale: Res<CurrentLocale>,
    localization: NonSend<Localization>,
    player: Query<&NetSkillSet, With<NetLocalPlayer>>,
    root: Query<Entity, With<TreeRoot>>,
    children_query: Query<&Children>,
    mut last_built: Local<Option<(SkillGroupKind, Vec<(Skill, u16)>, String)>>,
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

    // Rebuild only when `(selected group, unlocked-skill snapshot, active
    // locale)` actually differs from last time — a plain value comparison,
    // not `Changed<NetSkillSet>`/`is_changed()` (BL-82 EM-5.7 follow-up: see
    // `sync_diary_tabs`'s own doc comment for why ECS change-detection
    // raced against the listen-server's local-replication re-emit and never
    // fired in a live `--smoke-screenshot`). The locale tag (BL-82 EM-5.16
    // T56.44 follow-up) is what makes a bare language switch, with no
    // skillset change at all, still refresh this tab's tooltip text while
    // it's already open.
    let mut snapshot = skillset.skills.clone();
    snapshot.sort_by_key(|(skill, _)| format!("{skill:?}"));
    let key = (kind, snapshot, current_locale.0.clone());
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

    // BL-82 EM-5.17 Phase 6 (T57.12): each visible skill's node-CENTER
    // position, in the SAME `TreeRoot`-local pixel space the node-spawn loop
    // below computes `x`/`y` in (top-left of the node box; `+
    // TREE_NODE_SIZE / 2.0` gets the center) — computed as its OWN pass, no
    // ECS access, purely so the connector-line pass below can look a
    // prerequisite's position up before that prerequisite's node entity has
    // even been spawned yet this rebuild.
    let mut positions: HashMap<Skill, Vec2> = HashMap::with_capacity(skills.len());
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
            #[expect(
                clippy::cast_precision_loss,
                reason = "column index is a handful per row"
            )]
            let x = row_x_start + col as f32 * TREE_COL_W;
            let center = Vec2::new(x + TREE_NODE_SIZE / 2.0, y + TREE_NODE_SIZE / 2.0);
            positions.insert(skills[skill_idx], center);
        }
    }

    rebuild_children(&mut commands, root_entity, &children_query, |parent| {
        // Connector lines FIRST (BL-82 EM-5.17 Phase 6, spec §3.6: "draw the
        // lines BEHIND the node icons") — spawned before any node button
        // below, so they sit behind every node in this container's
        // paint/pick order (`bevy_ui` paints/picks siblings in child-spawn
        // order at equal z-index; `TreeRoot`'s children carry no per-node
        // `GlobalZIndex`, so plain spawn order is what decides this here).
        for &skill in &skills {
            let Some(&target_pos) = positions.get(&skill) else {
                continue;
            };
            for prereq in shape.direct_prerequisites(skill) {
                let Some(&prereq_pos) = positions.get(&prereq) else {
                    // The prerequisite has no node in THIS tab's visible
                    // list (e.g. a cross-group `UnlockGroup` gate) — nothing
                    // to draw a line to.
                    continue;
                };
                let target_unlocked = unlocked.get(&skill).copied().unwrap_or(0) > 0;
                spawn_connector_line(parent, &hud_images, prereq_pos, target_pos, target_unlocked);
            }
        }

        for row_indices in &rows {
            for &skill_idx in row_indices {
                let skill = skills[skill_idx];
                // Reuse the SAME center this pass's connector-line loop
                // above already computed (`positions`), rather than
                // recomputing `total_row_w`/`row_x_start`/`x`/`y` a second
                // time from scratch — two textually-independent copies of
                // that arithmetic could silently drift apart (e.g. a future
                // centering/margin tweak to one and not the other), which
                // would misalign connector lines from the nodes they
                // connect with no test able to catch it. Deriving both from
                // one source makes that drift structurally impossible.
                let Some(&center) = positions.get(&skill) else {
                    continue;
                };
                let x = center.x - TREE_NODE_SIZE / 2.0;
                let y = center.y - TREE_NODE_SIZE / 2.0;

                let level = unlocked.get(&skill).copied().unwrap_or(0);
                let max = shape.max_level(skill);
                // BL-82 EM-5.16 item D: resolve the node's own name (and, for
                // feats, its description) through the `skill_i18n_key` table.
                // `None` (a non-weapon unlock group, never a real node here)
                // falls back to the Debug name, preserving prior behaviour.
                let name = crate::skill_i18n::skill_i18n_key(skill)
                    .map_or_else(|| format!("{skill:?}"), |k| localization.tr(k));
                // Only the feat messages carry a `.desc` attribute; for every
                // other key `tr_attr` returns its `"{key}.desc"` sentinel, which
                // we drop so non-feat nodes stay name-only (today's behaviour).
                let desc_line = crate::skill_i18n::skill_i18n_key(skill)
                    .map(|k| (k, localization.tr_attr(k, "desc")))
                    .filter(|(k, d)| *d != format!("{k}.desc"))
                    .map(|(_, d)| format!("\n{d}"))
                    .unwrap_or_default();
                let kind_note = if shape.is_passive(skill) {
                    format!(" {}", localization.tr("hud-skill_tree-node_passive"))
                } else {
                    String::new()
                };
                let (border, tooltip) = if level >= max {
                    (
                        theme.palette.buff_good,
                        format!(
                            "{name}{kind_note}{desc_line}\n{} ({level}/{max})",
                            localization.tr("hud-skill_tree-node_maxed")
                        ),
                    )
                } else if shape.prerequisites_met(skill, &unlocked) {
                    let cost = skill.skill_cost(level + 1);
                    (
                        theme.palette.accent,
                        format!(
                            "{name}{kind_note}{desc_line}\n{} {level}/{max}\n{}: {cost} {}",
                            localization.tr("hud-skill_tree-node_level"),
                            localization.tr("hud-skill_tree-node_cost"),
                            localization.tr("hud-sp_arrow_txt"),
                        ),
                    )
                } else {
                    (
                        theme.palette.text_muted,
                        format!(
                            "{name}{kind_note}{desc_line}\n{}",
                            localization.tr("hud-skill_tree-node_locked")
                        ),
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
                    // BL-82 EM-5.17 T57.16 — reskins this skill node's hover
                    // tooltip with the themed `skill_tooltip_bg.png` frame
                    // (see `xindeler_ui::tooltip`'s module doc comment for
                    // the opt-in `TooltipBackground` contract).
                    TooltipBackground(HudImageKey::SkillTooltipBg),
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
    /// [`UnlockSkillRequest`] for THAT node's [`Skill`] — the real
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
        app.add_message::<UnlockSkillRequest>();

        let skill = Skill::Warrior(WarriorSkill::Rally);
        let node = app.world_mut().spawn(SkillNodeTarget(skill)).id();
        app.world_mut()
            .entity_mut(node)
            .observe(handle_skill_node_activate);

        app.world_mut().trigger(Activate { entity: node });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<UnlockSkillRequest>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![UnlockSkillRequest(skill)]);
    }

    /// A click on an entity that does NOT carry [`SkillNodeTarget`] (should
    /// never happen in practice — every tree node spawn inserts it — but the
    /// handler's own `Ok(target)` guard is the only thing preventing a panic
    /// if it ever did) writes nothing.
    #[test]
    fn activating_a_node_without_a_target_writes_nothing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<UnlockSkillRequest>();

        let node = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(node)
            .observe(handle_skill_node_activate);

        app.world_mut().trigger(Activate { entity: node });

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<UnlockSkillRequest>>()
            .drain()
            .collect();
        assert!(sent.is_empty());
    }

    /// BL-82 EM-5.17 Phase 0 regression (the "duplicate Lv.1" bug):
    /// [`StatsPanelRoot`]/[`TreeRoot`]/[`AbilitiesPanelRoot`] must spawn with
    /// `Visibility::Inherited`, NOT `Visibility::Visible`. `Visible`
    /// FORCE-OVERRIDES the ancestor [`DiaryWindowRoot`]'s
    /// `Visibility::Hidden` (it does not mean "inherit from parent" — that's
    /// what `Inherited` means), so these panels kept painting even while the
    /// whole Diary window was supposedly closed. This asserts the `Visibility`
    /// component itself on each of the three content roots right after
    /// spawn — a real computed-`InheritedVisibility`/`ViewVisibility`
    /// assertion would need `bevy_render`'s `VisibilityPlugin` propagation
    /// pass wired into the test app, which no existing test in this crate
    /// does (`combat_hud.rs`'s own `spawn_combat_hud_keeps_every_bars_
    /// sizing_from_spawn_bar_intact` test, the closest precedent, only
    /// checks `Node` after a bare `MinimalPlugins` + `run_system_once`); this
    /// is the documented fallback the task brief allows.
    #[test]
    fn diary_content_panels_spawn_inherited_not_visible() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = diary_window_test_app();

        app.world_mut()
            .run_system_once(spawn_diary_window)
            .expect("spawn_diary_window runs");

        fn visibility_of<T: bevy::ecs::component::Component>(world: &mut World) -> Visibility {
            *world
                .query_filtered::<&Visibility, With<T>>()
                .single(world)
                .expect("the tagged panel root exists")
        }

        assert_eq!(
            visibility_of::<StatsPanelRoot>(app.world_mut()),
            Visibility::Inherited,
            "StatsPanelRoot must inherit the (Hidden) DiaryWindowRoot's visibility, not \
             force-override it"
        );
        assert_eq!(
            visibility_of::<TreeRoot>(app.world_mut()),
            Visibility::Inherited
        );
        assert_eq!(
            visibility_of::<AbilitiesPanelRoot>(app.world_mut()),
            Visibility::Inherited
        );
    }

    /// A headless test `App` with a real (headless) `AssetServer` wired up —
    /// [`spawn_diary_window`] now reads `Res<HudImages>` (BL-82 EM-5.17 Phase
    /// 6, T57.11), and `HudImages::load` needs a real `AssetServer` to build
    /// (its `handles` field is private, so a fake instance can't be
    /// hand-constructed from this crate — see `xindeler-ui::images`'s own
    /// module doc comment). Mirrors `sprite_view.rs`'s established
    /// `MinimalPlugins` + `AssetPlugin::default()` + `init_asset::<T>()`
    /// headless-asset-server recipe (that test seeds `Mesh`; this one needs
    /// `Image`). No file on disk is ever actually read by these tests — a
    /// `Handle<Image>` from `asset_server.load(path)` is real and usable
    /// (comparable, clonable) the instant it's requested, whether or not the
    /// asset loader ever resolves it.
    fn diary_window_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.insert_resource(HudTheme::default());
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.finish();
        app
    }

    /// BL-82 EM-5.17 Phase 6 (T57.11) acceptance: the diary's shared panel
    /// carries an [`ImageNode`] for [`HudImageKey::SkillTreeBg`] (the "Path
    /// of Ascension" parchment — NOT `OtherSkillTreeBg`, see
    /// `spawn_diary_window`'s own doc comment for why this phase always uses
    /// the ONE background regardless of tab), and the whole window carries
    /// [`zlayer::MODAL_WINDOWS`] as its [`GlobalZIndex`] — the first
    /// `GlobalZIndex` anywhere in this HUD (spec §1.1/§4.4).
    #[test]
    fn spawn_diary_window_uses_skill_tree_bg_and_modal_z_index() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = diary_window_test_app();
        let asset_server = app.world().resource::<AssetServer>().clone();
        let expected_bg = HudImages::load(&asset_server).get(HudImageKey::SkillTreeBg);

        app.world_mut()
            .run_system_once(spawn_diary_window)
            .expect("spawn_diary_window runs");

        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, With<DiaryWindowRoot>>()
            .single(world)
            .expect("DiaryWindowRoot exists");
        let z_index = world
            .get::<GlobalZIndex>(root)
            .expect("DiaryWindowRoot carries a GlobalZIndex");
        assert_eq!(z_index.0, zlayer::MODAL_WINDOWS);

        let panel = world
            .query_filtered::<Entity, With<xindeler_ui::panel::HudPanel>>()
            .single(world)
            .expect("the diary's shared HudPanel exists");
        let image_node = world
            .get::<ImageNode>(panel)
            .expect("the panel carries an ImageNode");
        assert_eq!(image_node.image, expected_bg);
    }

    /// [`connector_segment`] between two horizontally-offset centers: the
    /// midpoint is the arithmetic mean, the length is the plain distance,
    /// and a purely horizontal segment (pointing along +x) has a ZERO
    /// rotation angle.
    #[test]
    fn connector_segment_horizontal() {
        let seg = connector_segment(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        assert_eq!(seg.midpoint, Vec2::new(5.0, 0.0));
        assert!((seg.length - 10.0).abs() < 1e-5);
        assert!(seg.angle.abs() < 1e-6, "a horizontal segment has angle 0");
    }

    /// A segment pointing straight "down" the screen (increasing y, `bevy_ui`
    /// screen space is y-down) has a clockwise rotation of +90° (π/2
    /// radians) from the local +x axis — matches [`UiTransform::rotation`]'s
    /// documented "rotate clockwise" convention (the SAME sign
    /// `map_view::sync_minimap`'s `Rot2::radians(heading)` already relies on).
    #[test]
    fn connector_segment_vertical_is_a_quarter_turn() {
        let seg = connector_segment(Vec2::new(0.0, 0.0), Vec2::new(0.0, 10.0));
        assert!((seg.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    /// A 3-4-5-triangle-shaped diagonal segment (scaled ×10): length is the
    /// Euclidean distance (50, not 30+40), midpoint is the arithmetic mean,
    /// and the angle is the plain `atan2` of the offset — a non-axis-aligned
    /// case the two axis-aligned tests above don't cover.
    #[test]
    fn connector_segment_diagonal() {
        let seg = connector_segment(Vec2::new(0.0, 0.0), Vec2::new(30.0, 40.0));
        assert_eq!(seg.midpoint, Vec2::new(15.0, 20.0));
        assert!((seg.length - 50.0).abs() < 1e-3);
        let expected_angle = 40.0_f32.atan2(30.0);
        assert!((seg.angle - expected_angle).abs() < 1e-6);
    }

    /// [`connector_image_key`]: `SkillLineActive` when the edge's target
    /// (child) skill is unlocked, `SkillLineLocked` when it isn't — the
    /// spec §3.6 "glowing molten chain, unlocked/invested" vs "dull rusted
    /// chain, locked" distinction.
    #[test]
    fn connector_image_key_is_active_only_when_target_unlocked() {
        assert_eq!(connector_image_key(true), HudImageKey::SkillLineActive);
        assert_eq!(connector_image_key(false), HudImageKey::SkillLineLocked);
    }

    /// [`SkillTreeShape::direct_prerequisites`] returns the union of an
    /// `All`/`Any` prerequisite's key set (level requirement discarded), and
    /// an empty `Vec` for a skill with no prerequisite entry at all (a tier-0
    /// root).
    #[test]
    fn direct_prerequisites_returns_the_key_set_of_all_or_any() {
        let root = Skill::UnlockGroup(SkillGroupKind::General);
        let leaf_a = Skill::Warrior(WarriorSkill::Rally);
        let leaf_b = Skill::Warrior(WarriorSkill::Onslaught);
        let target = Skill::Warrior(WarriorSkill::BrutalEdge);
        let shape = shape_with(&[], &[(
            target,
            SkillPrerequisite::All(PrereqMap::from([(root, 1), (leaf_a, 1)])),
        )]);

        let mut prereqs = shape.direct_prerequisites(target);
        prereqs.sort_by_key(|s| format!("{s:?}"));
        let mut expected = vec![root, leaf_a];
        expected.sort_by_key(|s| format!("{s:?}"));
        assert_eq!(prereqs, expected);

        assert!(
            shape.direct_prerequisites(leaf_b).is_empty(),
            "a skill absent from the prerequisite manifest has no direct prerequisites"
        );
    }

    /// BL-82 EM-5.16 (T56.44 follow-up): switching the active locale
    /// re-localizes an already-spawned diary TAB button live, using the REAL
    /// repo `.ftl` catalog (not a synthetic fixture) via
    /// `VELOREN_ASSETS`/`XINDELER_ASSETS` — the same real-catalog idiom
    /// `esc_menu.rs`'s own hot-swap test uses, exercised here against
    /// `sync_diary_tabs`'s `LocalizedLabel`-tagged Abilities tab (a plain
    /// fixed-key tab that needs no `SkillTreeShape`/group content, keeping
    /// this test focused on the i18n wiring itself).
    #[test]
    fn switching_locale_relocalizes_a_diary_tab_button_live() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = diary_window_test_app();
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &["common.ftl"],
        ));
        app.init_resource::<CurrentLocale>();
        app.add_systems(Update, xindeler_ui::button::spawn_button_labels);

        app.world_mut()
            .spawn((NetLocalPlayer, NetSkillSet::default()));

        app.world_mut()
            .run_system_once(spawn_diary_window)
            .expect("spawn_diary_window runs");
        app.world_mut()
            .run_system_once(sync_diary_tabs)
            .expect("sync_diary_tabs runs");
        app.update(); // let spawn_button_labels give each tab button its child

        fn abilities_button_text(app: &mut App) -> String {
            let world = app.world_mut();
            let child = world
                .query::<(&LocalizedLabel, &Children)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "hud-diary-sections-abilities-title")
                .map(|(_, children)| children[0])
                .expect("the Abilities tab button was spawned and tagged");
            world
                .get::<Text>(child)
                .expect("label child exists")
                .0
                .clone()
        }

        assert_eq!(
            abilities_button_text(&mut app),
            "Abilities",
            "the Abilities tab must show the real en catalog text at spawn time"
        );

        app.world_mut().resource_mut::<CurrentLocale>().0 = "es".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_button_labels)
            .expect("relocalize runs");
        app.update(); // spawn_button_labels propagates the HudButtonLabel change onto Text

        assert_eq!(
            abilities_button_text(&mut app),
            "Habilidades",
            "must resolve to the REAL es catalog's own hud-diary-sections-abilities-title value, \
             not the en fallback"
        );
    }

    /// BL-82 EM-5.16 (T56.44 follow-up): the Stats tab's per-frame content
    /// rebuild (`sync_stats_panel`) resolves its row labels through the
    /// active locale directly (no tag needed — it rebuilds every frame the
    /// tab is selected), proven here against the REAL repo `.ftl` catalog:
    /// spawn with `en` active, confirm the Level row, reload to `es`, confirm
    /// the SAME row now reads the real Spanish catalog value.
    #[test]
    fn stats_panel_resolves_row_labels_through_the_active_locale() {
        use bevy::ecs::system::RunSystemOnce;
        use xindeler_protocol::NetXp;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_resource(DiaryTab::Stats);
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &["hud/char_window.ftl"],
        ));

        let root = app.world_mut().spawn(StatsPanelRoot).id();
        app.world_mut().spawn((NetLocalPlayer, NetXp {
            level: 3,
            xp_into_level: 10,
            xp_for_level: 100,
        }));

        app.world_mut()
            .run_system_once(sync_stats_panel)
            .expect("sync_stats_panel runs");

        fn first_line(app: &mut App, root: Entity) -> String {
            let world = app.world_mut();
            let children = world
                .get::<Children>(root)
                .expect("StatsPanelRoot has children");
            world
                .get::<Text>(children[0])
                .expect("first line is a Text node")
                .0
                .clone()
        }

        assert_eq!(
            first_line(&mut app, root),
            "Level 3",
            "the Level row must show the real en catalog label at spawn time"
        );

        // Reload to the real es catalog and re-run the same system — no
        // separate hot-swap chain needed here, `sync_stats_panel` just reads
        // whatever `Localization` bundle is current every time it rebuilds.
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::parse_locale("es"),
            &["hud/char_window.ftl"],
        ));
        app.world_mut()
            .run_system_once(sync_stats_panel)
            .expect("sync_stats_panel runs again");

        assert_eq!(
            first_line(&mut app, root),
            "Nivel 3",
            "must resolve to the real es catalog's own character_window-character_level value"
        );
    }

    /// Every skill in every group of the real skill-tree manifest resolves to
    /// real localized text via `skill_i18n_key` (BL-82 EM-5.16 item D). Loads
    /// the same `SkillTreeShape` the Diary renders from, so it covers exactly
    /// the leaves that can appear as nodes — no skill can regress to a raw
    /// Debug name unnoticed.
    #[test]
    fn every_manifest_skill_resolves_to_real_text() {
        use xindeler_ui::i18n::{DEFAULT_HUD_FTL_FILES, Localization, fallback_locale};
        let shape = SkillTreeShape::load();
        let l10n = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);
        assert!(!shape.groups.is_empty(), "skill-groups manifest must load");
        for skills in shape.groups.values() {
            for &skill in skills {
                let key = crate::skill_i18n::skill_i18n_key(skill)
                    .unwrap_or_else(|| panic!("{skill:?} (a rendered node) has no i18n key"));
                assert_ne!(
                    l10n.tr(key),
                    key,
                    "{skill:?} -> {key} must resolve to real text"
                );
            }
        }
    }
}
