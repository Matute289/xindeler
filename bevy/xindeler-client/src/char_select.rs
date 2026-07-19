//! BL-82 EM-5.14 (T56.33) — the character-select screen + 6-step creation
//! wizard, gated on [`AppState::CharSelect`].
//!
//! This is the pure-Bevy client side of the EM-5.14 seam: it reads the
//! server-broadcast [`NetCharList`] into a resource, renders the roster
//! (select / delete / "create new"), drives the creation wizard
//! (Body → Appearance → Class → Alignment → Background → Finish, matching
//! `docs/design/specs/2026-06-12-char-creation-wizard-design.md` 1:1, extended
//! with the Alignment + Background steps EM-5.14 adds), and hands the player's
//! intents to the sim-bridge as in-process [`LocalCharCreate`]/
//! [`LocalCharDelete`]/[`LocalCharSelect`] messages
//! (`xindeler_sim_bridge::charlist::apply_char_requests` applies them to the
//! embedded `client::Client`). On spawn into the world it flips
//! [`AppState`] to [`AppState::InGame`].
//!
//! ## Wizard invariants (spec §Navigation + EM-5.14 acceptance)
//! - The **name** field is visible/editable in every step (owned-buffer input,
//!   the same pattern `chat.rs` uses — NOT `EditableText`, which mis-behaves on
//!   macOS; see that module's own doc comment).
//! - Every step has **valid defaults from step 1** (`WizardState::new_default`:
//!   a random Human body, Warrior + its starter sword, True-Neutral ethos, no
//!   background, hardcore off, a non-empty placeholder name) so the player
//!   could hit **Create** immediately.
//! - The **Create** action exists **only on the last step** (Finish); the class
//!   step shows **only the selected class's valid weapons** (a genuinely
//!   filtered list — the other classes' weapons are never spawned as buttons,
//!   so they are unselectable, not merely greyed).
//! - The class↔weapon table [`class_weapon_options`] mirrors the
//!   server-authoritative whitelist `server::character_creator::
//!   valid_starter_items` (the server re-validates every create — this table is
//!   a display convenience, never the source of truth). Unifying it into
//!   `common` is the pre-existing separately-approved "M2 whitelist → common"
//!   backlog item; until then this mirrors it the same way the legacy voxygen
//!   UI (`voxygen/src/menu/char_selection/ui.rs`) kept its own copy.
//!
//! The core wizard logic ([`WizardStep`], [`WizardState`],
//! [`class_weapon_options`]) is deliberately pure (no ECS/asset access) so the
//! step order, defaults, weapon filtering and create-only-on-last-step rules
//! are unit-testable without a running `App` — the same "pure fn, test it
//! directly" posture the rest of this crate uses.

use bevy::{
    ecs::{
        message::{MessageReader, MessageWriter},
        schedule::IntoScheduleConfigs,
    },
    prelude::*,
};
use common::comp::{
    Background, Body, ClassKind, Ethos, Moral, Order,
    background::BackgroundKind,
    humanoid::{self, BodyType, Species},
};
use xindeler_app::AppState;
use xindeler_protocol::{
    CharCreateParams, LocalCharCreate, LocalCharDelete, LocalCharSelect, NetCharList,
    NetCharListEntry, NetLocalPlayer,
};
use xindeler_ui::{
    button::{Activate, button_bundle},
    i18n::{CurrentLocale, Localization, LocalizedLabel, LocalizedText},
    theme::{HudFonts, HudTheme},
};

use crate::char_preview::{CharPreview, CharPreviewPlugin};

// ---------------------------------------------------------------------------
// Pure wizard core (unit-testable without an App)
// ---------------------------------------------------------------------------

/// The six wizard steps, in order (spec §Step structure + EM-5.14's added
/// Alignment/Background steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Body,
    Appearance,
    Class,
    Alignment,
    Background,
    Finish,
}

impl WizardStep {
    /// Every step in navigation order.
    pub const ORDER: [WizardStep; 6] = [
        WizardStep::Body,
        WizardStep::Appearance,
        WizardStep::Class,
        WizardStep::Alignment,
        WizardStep::Background,
        WizardStep::Finish,
    ];

    /// 0-based index of this step in [`Self::ORDER`].
    pub fn index(self) -> usize { Self::ORDER.iter().position(|s| *s == self).unwrap_or(0) }

    /// The next step, clamped at [`Self::Finish`].
    pub fn next(self) -> WizardStep {
        Self::ORDER
            .get(self.index() + 1)
            .copied()
            .unwrap_or(WizardStep::Finish)
    }

    /// The previous step, clamped at [`Self::Body`].
    pub fn prev(self) -> WizardStep {
        Self::ORDER
            .get(self.index().saturating_sub(1))
            .copied()
            .unwrap_or(WizardStep::Body)
    }

    /// The last step, where the Create action lives.
    pub fn is_last(self) -> bool { self == WizardStep::Finish }

    /// The `.ftl` key for this step's title LABEL (BL-82 EM-5.16 T56.44 full
    /// i18n coverage). [`sync_step_title`] composes the actual on-screen
    /// "`N`/6  Label" string by Rust-formatting this step's own 1-based
    /// [`Self::index`] around the resolved value — [`Localization::tr`] has
    /// no Fluent placeable/argument support (see that method's own doc
    /// comment), so the numeral can never be baked into the `.ftl` message
    /// itself the way the legacy client's `char_selection-wizard_step`
    /// (`"Step { $step } of 6"`) does.
    pub fn title_key(self) -> &'static str {
        match self {
            WizardStep::Body => "char_selection-step_body",
            WizardStep::Appearance => "char_selection-step_appearance",
            WizardStep::Class => "char_selection-step_class",
            WizardStep::Alignment => "char_selection-step_alignment",
            WizardStep::Background => "char_selection-step_background",
            WizardStep::Finish => "char_selection-step_finish",
        }
    }
}

/// One selectable starter-weapon option for a class: the mainhand/offhand
/// asset paths handed to `create_character`, plus the `.ftl` key for its
/// display label (BL-82 EM-5.16 T56.44: `label` used to be the literal
/// display text — it is now an i18n key resolved via [`Localization::tr`] at
/// render time, reusing the `common-weapons-*` catalog the legacy
/// `voxygen/src/menu/char_selection/ui.rs` resolves these exact same weapon
/// choices from).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponOption {
    pub label: &'static str,
    pub mainhand: Option<&'static str>,
    pub offhand: Option<&'static str>,
}

/// The class → valid starter-weapon options, mirroring the server-authoritative
/// `server::character_creator::valid_starter_items` (see this module's doc
/// comment). The FIRST entry is the class's default weapon. Only these options
/// are ever shown for a chosen class — nothing else is selectable.
pub fn class_weapon_options(class: ClassKind) -> &'static [WeaponOption] {
    // Starter asset paths (verbatim from `valid_starter_items`).
    const SWORD: &str = "common.items.weapons.sword.starter";
    const AXE: &str = "common.items.weapons.axe.starter_axe";
    const HAMMER: &str = "common.items.weapons.hammer.starter_hammer";
    const STAFF: &str = "common.items.weapons.staff.starter_staff";
    const SCEPTRE: &str = "common.items.weapons.sceptre.starter_sceptre";
    const SWORD_1H: &str = "common.items.weapons.sword_1h.starter";
    const BOW: &str = "common.items.weapons.bow.starter";

    // BL-82 EM-5.16 (T56.44): every `label` below is now a `common-weapons-*`
    // i18n key (the same catalog + the same per-weapon key the legacy
    // `voxygen/src/menu/char_selection/ui.rs` resolves these exact starter
    // choices from), not literal display text — `class_weapon_options`'s one
    // call site (`build_wizard_step`'s Class step) resolves it via
    // `Localization::tr` at spawn time. Rogue's dual-wield and Monk's single
    // starter both use the SAME asset (`SWORD_1H`) and so both reuse
    // `common-weapons-shortswords` (plural in the catalog even for Monk's
    // singular case) — there is no separate singular key upstream either.
    const WARRIOR: &[WeaponOption] = &[
        WeaponOption {
            label: "common-weapons-greatsword",
            mainhand: Some(SWORD),
            offhand: None,
        },
        WeaponOption {
            label: "common-weapons-axe",
            mainhand: Some(AXE),
            offhand: None,
        },
        WeaponOption {
            label: "common-weapons-hammer",
            mainhand: Some(HAMMER),
            offhand: None,
        },
    ];
    const MAGE: &[WeaponOption] = &[WeaponOption {
        label: "common-weapons-staff",
        mainhand: Some(STAFF),
        offhand: None,
    }];
    const CLERIC: &[WeaponOption] = &[WeaponOption {
        label: "common-weapons-sceptre",
        mainhand: Some(SCEPTRE),
        offhand: None,
    }];
    const ROGUE: &[WeaponOption] = &[
        WeaponOption {
            label: "common-weapons-shortswords",
            mainhand: Some(SWORD_1H),
            offhand: Some(SWORD_1H),
        },
        WeaponOption {
            label: "common-weapons-bow",
            mainhand: Some(BOW),
            offhand: None,
        },
    ];
    const BARBARIAN: &[WeaponOption] = &[
        WeaponOption {
            label: "common-weapons-axe",
            mainhand: Some(AXE),
            offhand: None,
        },
        WeaponOption {
            label: "common-weapons-hammer",
            mainhand: Some(HAMMER),
            offhand: None,
        },
    ];
    const CASTER_STAFF: &[WeaponOption] = &[WeaponOption {
        label: "common-weapons-staff",
        mainhand: Some(STAFF),
        offhand: None,
    }];
    const SWORD_ONLY: &[WeaponOption] = &[WeaponOption {
        label: "common-weapons-sword",
        mainhand: Some(SWORD),
        offhand: None,
    }];
    const BOW_ONLY: &[WeaponOption] = &[WeaponOption {
        label: "common-weapons-bow",
        mainhand: Some(BOW),
        offhand: None,
    }];
    const MONK: &[WeaponOption] = &[WeaponOption {
        label: "common-weapons-shortswords",
        mainhand: Some(SWORD_1H),
        offhand: None,
    }];
    const NONE: &[WeaponOption] = &[];

    match class {
        ClassKind::Warrior => WARRIOR,
        ClassKind::Mage => MAGE,
        ClassKind::Cleric => CLERIC,
        ClassKind::Rogue => ROGUE,
        ClassKind::Barbarian => BARBARIAN,
        ClassKind::Sorcerer
        | ClassKind::Warlock
        | ClassKind::Bard
        | ClassKind::Druid
        | ClassKind::Artificer => CASTER_STAFF,
        ClassKind::Paladin | ClassKind::BloodSlayer => SWORD_ONLY,
        ClassKind::Ranger => BOW_ONLY,
        ClassKind::Monk => MONK,
        ClassKind::Adventurer => NONE,
    }
}

/// The classes offered in the wizard's class step. The spec's proof slice is
/// Warrior/Mage/Cleric/Rogue (shown first); the machinery is generic over
/// every `ClassKind::PLAYABLE`.
pub fn wizard_classes() -> Vec<ClassKind> {
    let mut classes = vec![
        ClassKind::Warrior,
        ClassKind::Mage,
        ClassKind::Cleric,
        ClassKind::Rogue,
    ];
    for class in ClassKind::PLAYABLE {
        if !classes.contains(&class) {
            classes.push(class);
        }
    }
    classes
}

/// Which appearance field a cycle action targets (step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceField {
    HairStyle,
    HairColor,
    Skin,
    Eyes,
    EyeColor,
    Beard,
    Accessory,
}

/// The full mutable creation state (spec's `Mode::CreateOrEdit` analogue). All
/// choices persist across step navigation — the step only selects what renders.
#[derive(Resource, Debug, Clone)]
pub struct WizardState {
    pub step: WizardStep,
    pub name: String,
    pub species: Species,
    pub body_type: BodyType,
    pub hair_style: u8,
    pub hair_color: u8,
    pub skin: u8,
    pub eyes: u8,
    pub eye_color: u8,
    pub beard: u8,
    pub accessory: u8,
    pub class: ClassKind,
    /// Index into [`class_weapon_options`] for [`Self::class`].
    pub weapon_choice: usize,
    pub order: Order,
    pub moral: Moral,
    pub background: Option<BackgroundKind>,
    pub hardcore: bool,
}

impl Default for WizardState {
    fn default() -> Self { Self::new_default() }
}

impl WizardState {
    /// A fully-valid starting state (spec: "defaults make every step valid from
    /// the start"): a Human Warrior with the starter sword, True-Neutral ethos,
    /// no background, hardcore off, and a non-empty placeholder name so Create
    /// is immediately valid.
    pub fn new_default() -> Self {
        Self {
            step: WizardStep::Body,
            name: "Wanderer".to_owned(),
            species: Species::Human,
            body_type: BodyType::Female,
            hair_style: 0,
            hair_color: 0,
            skin: 0,
            eyes: 0,
            eye_color: 0,
            beard: 0,
            accessory: 0,
            class: ClassKind::Warrior,
            weapon_choice: 0,
            order: Order::Neutral,
            moral: Moral::Neutral,
            background: None,
            hardcore: false,
        }
    }

    /// The chosen humanoid body, clamped to valid ranges for its species /
    /// body-type (`humanoid::Body::validate`).
    pub fn humanoid_body(&self) -> humanoid::Body {
        let mut body = humanoid::Body {
            species: self.species,
            body_type: self.body_type,
            hair_style: self.hair_style,
            beard: self.beard,
            eyes: self.eyes,
            accessory: self.accessory,
            hair_color: self.hair_color,
            skin: self.skin,
            eye_color: self.eye_color,
        };
        body.validate();
        body
    }

    /// The full `common::comp::Body` for the figure pipeline / create request.
    pub fn body(&self) -> Body { Body::Humanoid(self.humanoid_body()) }

    /// The selected weapon option (falls back to the class's first option if
    /// the index is stale, which [`Self::set_class`] prevents anyway).
    pub fn selected_weapon(&self) -> Option<WeaponOption> {
        let options = class_weapon_options(self.class);
        options
            .get(self.weapon_choice)
            .or_else(|| options.first())
            .copied()
    }

    /// Whether Create is currently allowed: only on the last step, and only
    /// with a non-empty (trimmed) name (an empty name is warned + blocked,
    /// matching the legacy UI — the server's `Player::is_valid` rejects it
    /// too).
    pub fn can_create(&self) -> bool { self.step.is_last() && !self.name.trim().is_empty() }

    /// Build the create request payload from the current state.
    pub fn create_params(&self) -> CharCreateParams {
        let weapon = self.selected_weapon();
        CharCreateParams {
            alias: self.name.trim().to_owned(),
            mainhand: weapon.and_then(|w| w.mainhand).map(str::to_owned),
            offhand: weapon.and_then(|w| w.offhand).map(str::to_owned),
            body: self.body(),
            hardcore: self.hardcore,
            class: self.class,
            ethos: Ethos::from_box(self.order, self.moral),
            background: Background(self.background),
        }
    }

    /// Advance one step (clamped at Finish).
    pub fn advance(&mut self) { self.step = self.step.next(); }

    /// Go back one step (clamped at Body).
    pub fn retreat(&mut self) { self.step = self.step.prev(); }

    /// Switch class and reset the weapon choice to that class's default (index
    /// 0) so [`Self::weapon_choice`] is never stale/out of range.
    pub fn set_class(&mut self, class: ClassKind) {
        self.class = class;
        self.weapon_choice = 0;
    }

    /// Switch species and clamp appearance indices to the new species' ranges
    /// (via [`Self::humanoid_body`]'s `validate`, folded back onto the fields).
    pub fn set_species(&mut self, species: Species) {
        self.species = species;
        let clamped = self.humanoid_body();
        self.copy_appearance_from(&clamped);
    }

    /// Switch body-type and re-clamp appearance the same way as
    /// [`Self::set_species`].
    pub fn set_body_type(&mut self, body_type: BodyType) {
        self.body_type = body_type;
        let clamped = self.humanoid_body();
        self.copy_appearance_from(&clamped);
    }

    fn copy_appearance_from(&mut self, body: &humanoid::Body) {
        self.hair_style = body.hair_style;
        self.beard = body.beard;
        self.eyes = body.eyes;
        self.accessory = body.accessory;
        self.hair_color = body.hair_color;
        self.skin = body.skin;
        self.eye_color = body.eye_color;
    }

    /// Cycle an appearance field by `delta` (wrapping within its species/
    /// body-type range).
    pub fn cycle_appearance(&mut self, field: AppearanceField, delta: i32) {
        let count = self.appearance_count(field).max(1);
        let current = self.appearance_value(field) as i32;
        let next = (current + delta).rem_euclid(count as i32) as u8;
        match field {
            AppearanceField::HairStyle => self.hair_style = next,
            AppearanceField::HairColor => self.hair_color = next,
            AppearanceField::Skin => self.skin = next,
            AppearanceField::Eyes => self.eyes = next,
            AppearanceField::EyeColor => self.eye_color = next,
            AppearanceField::Beard => self.beard = next,
            AppearanceField::Accessory => self.accessory = next,
        }
    }

    fn appearance_value(&self, field: AppearanceField) -> u8 {
        match field {
            AppearanceField::HairStyle => self.hair_style,
            AppearanceField::HairColor => self.hair_color,
            AppearanceField::Skin => self.skin,
            AppearanceField::Eyes => self.eyes,
            AppearanceField::EyeColor => self.eye_color,
            AppearanceField::Beard => self.beard,
            AppearanceField::Accessory => self.accessory,
        }
    }

    fn appearance_count(&self, field: AppearanceField) -> u8 {
        match field {
            AppearanceField::HairStyle => self.species.num_hair_styles(self.body_type),
            AppearanceField::HairColor => self.species.num_hair_colors(),
            AppearanceField::Skin => self.species.num_skin_colors(),
            AppearanceField::Eyes => self.species.num_eyes(self.body_type),
            AppearanceField::EyeColor => self.species.num_eye_colors(),
            AppearanceField::Beard => self.species.num_beards(self.body_type),
            AppearanceField::Accessory => self.species.num_accessories(self.body_type),
        }
    }
}

/// Every playable alignment box (Order × Moral) for the 3×3 alignment picker.
pub const ALIGNMENT_ORDERS: [Order; 3] = [Order::Lawful, Order::Neutral, Order::Chaotic];
pub const ALIGNMENT_MORALS: [Moral; 3] = [Moral::Good, Moral::Neutral, Moral::Evil];

// ---------------------------------------------------------------------------
// UI state / resources
// ---------------------------------------------------------------------------

/// Which screen the char-select flow currently shows.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharSelectScreen {
    /// The roster: pick / delete an existing character, or start creation.
    #[default]
    Roster,
    /// The 6-step creation wizard.
    Wizard,
}

/// Latest server-broadcast roster (the [`NetCharList`] mirror).
#[derive(Resource, Debug, Default)]
pub struct CharListData(pub NetCharList);

/// A single, coarse player intent, applied by [`apply_char_actions`]. Kept as
/// one message enum (instead of many observer closures) so the whole flow is
/// driven from one place — see this module's doc comment.
#[derive(Message, Debug, Clone)]
pub enum CharAction {
    OpenWizard,
    CancelWizard,
    Next,
    Back,
    SetSpecies(Species),
    SetBodyType(BodyType),
    CycleAppearance(AppearanceField, i32),
    SetClass(ClassKind),
    SetWeapon(usize),
    SetAlignment(Order, Moral),
    SetBackground(Option<BackgroundKind>),
    ToggleHardcore,
    Create,
    SelectCharacter(common::character::CharacterId),
    DeleteCharacter(common::character::CharacterId),
}

// ---------------------------------------------------------------------------
// UI plugin + systems
// ---------------------------------------------------------------------------

use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    text::{FontSize, FontSource},
};
use xindeler_ui::{XindelerUiPlugin, zlayer};

/// Full-screen char-select root (despawned on leaving
/// [`AppState::CharSelect`]).
#[derive(Component)]
struct CharSelectRoot;

/// The rebuilt content column (roster list, or the current wizard step).
#[derive(Component)]
struct ContentColumn;

/// The 3D preview image node.
#[derive(Component)]
struct PreviewImage;

/// The step-title / screen-title text.
#[derive(Component)]
struct StepTitleText;

/// The always-visible name row (shown only in the wizard).
#[derive(Component)]
struct NameRow;

/// The editable name value text.
#[derive(Component)]
struct NameValueText;

/// The nav/action button row (shown only in the wizard).
#[derive(Component)]
struct NavRow;

/// The "Next >" button (shown on every wizard step except the last).
#[derive(Component)]
struct NextButton;

/// The "Create" button (shown only on the last wizard step).
#[derive(Component)]
struct CreateButton;

/// The char-select screen plugin. Add in whichever shell hosts the screen
/// (the listen-server client, when launched into char-select). Registering it
/// is cheap on any App that never enters [`AppState::CharSelect`].
pub struct CharSelectViewPlugin;

impl Plugin for CharSelectViewPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<XindelerUiPlugin>() {
            app.add_plugins(XindelerUiPlugin);
        }
        app.add_plugins(CharPreviewPlugin)
            .init_resource::<CharListData>()
            .init_resource::<WizardState>()
            .init_resource::<CharSelectScreen>()
            .add_message::<CharAction>()
            .add_systems(OnEnter(AppState::CharSelect), reset_state)
            .add_systems(OnExit(AppState::CharSelect), despawn_screen)
            .add_systems(
                Update,
                (
                    // BL-82 EM-5.14 startup-panic fix (Matías, live-testing
                    // 2026-07-18): `spawn_screen` used to run chained after
                    // `reset_state` on `OnEnter(AppState::CharSelect)`. That
                    // panicked whenever `CharSelect` is the process's INITIAL
                    // `AppState` (exactly the `--char-select` launch path) —
                    // Bevy's `Main` schedule runs the initial state's
                    // `OnEnter` in a special `StateTransition` pass that
                    // fires BEFORE `PreStartup`/`Startup`/`PostStartup` (see
                    // `bevy_app::main_schedule`'s own doc comment: "This
                    // means that `OnEnter(MyState::Foo)` will be called
                    // *before* `PreStartup` ... if `MyState` was added to the
                    // app with `MyState::Foo` as the initial state"), so
                    // `HudTheme`/`HudFonts` (seeded by `XindelerUiPlugin`'s
                    // `theme::init_theme`, a `Startup` system) did not exist
                    // yet. `spawn_screen` now runs as an `Update` system,
                    // guarded the same way `menu.rs`'s `build_menu`/
                    // `enter_connecting` already dodge this exact scenario
                    // (`Option<Res<HudTheme>>`/`Option<Res<HudFonts>>`, no-op
                    // until ready) plus a `CharSelectRoot`-exists check so it
                    // spawns exactly once. `Update` still runs AFTER
                    // `Startup` even on the very first frame, so the screen
                    // is up within that same first frame once the resources
                    // land — no visible delay, just no more panic.
                    spawn_screen,
                    ingest_char_list,
                    read_name_input,
                    apply_char_actions,
                    // BL-82 EM-5.16 (T56.44 follow-up, bevy-migration-reviewer
                    // finding): both fold the active locale into their own
                    // rebuild-gate (`rebuild_content`'s `Local` cache key,
                    // `sync_step_title`'s `current_locale.is_changed()`), so —
                    // exactly like `settings_window.rs`'s `refresh_setting_
                    // labels` documents — they must run `.after(LocaleSyncSet)`:
                    // without this edge, on the exact frame the locale changes
                    // either system could read the STALE `Localization` bundle
                    // (Bevy gives no ordering guarantee between two systems
                    // with conflicting `NonSend`/`NonSendMut` access absent an
                    // explicit one), and because the gate is now satisfied,
                    // never retry until an unrelated screen/state change
                    // happens to force a rebuild anyway.
                    rebuild_content.after(xindeler_ui::i18n::LocaleSyncSet),
                    sync_step_title.after(xindeler_ui::i18n::LocaleSyncSet),
                    sync_name_value,
                    sync_nav_visibility,
                    sync_preview_image,
                    feed_preview_body,
                    enter_world_when_ready,
                )
                    .run_if(in_state(AppState::CharSelect)),
            );
    }
}

// -- small text helper -------------------------------------------------------

fn text_bundle(fonts: &HudFonts, theme: &HudTheme, size: f32, s: impl Into<String>) -> impl Bundle {
    (
        Text(s.into()),
        TextFont {
            font: FontSource::Handle(fonts.body.clone()),
            font_size: FontSize::Px(size),
            ..Default::default()
        },
        TextColor(theme.palette.text),
    )
}

/// Spawns a labeled button that writes `action` on activation.
fn spawn_action_button(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    label: &str,
    action: CharAction,
) {
    parent.spawn(button_bundle(theme, fonts, label)).observe(
        move |_: On<Activate>, mut w: MessageWriter<CharAction>| {
            w.write(action.clone());
        },
    );
}

// -- lifecycle ---------------------------------------------------------------

/// Fresh wizard/roster state each time the screen opens.
fn reset_state(mut state: ResMut<WizardState>, mut screen: ResMut<CharSelectScreen>) {
    *state = WizardState::new_default();
    *screen = CharSelectScreen::Roster;
}

/// Spawns the char-select screen scaffold exactly once (see the module doc
/// comment / `CharSelectViewPlugin::build`'s registration comment for why
/// this is an `Update` system, not `OnEnter`): tolerates `HudTheme`/
/// `HudFonts` not existing yet (retries next frame, same posture as
/// `menu.rs`'s `build_menu`/`enter_connecting`) and no-ops once
/// [`CharSelectRoot`] already exists (`despawn_screen` on `OnExit` clears it,
/// so re-entering the state naturally re-spawns).
fn spawn_screen(
    mut commands: Commands,
    theme: Option<Res<HudTheme>>,
    fonts: Option<Res<HudFonts>>,
    // BL-82 EM-5.16 (T56.44): `Localization` is inserted `NonSend` directly in
    // `XindelerUiPlugin::build` (not deferred to a `Startup` system the way
    // `HudTheme`/`HudFonts` are — see `theme::init_theme`'s own doc comment),
    // so by the time `CharSelectViewPlugin::build`'s
    // `if !app.is_plugin_added::<XindelerUiPlugin>() { .. }` has run, it
    // already exists — a hard `NonSend<Localization>` here needs no
    // `Option`/retry dance, matching `settings_window.rs`/`esc_menu.rs`'s own
    // `Startup`-scheduled spawn functions.
    localization: NonSend<Localization>,
    roots: Query<Entity, With<CharSelectRoot>>,
) {
    if !roots.is_empty() {
        return;
    }
    let (Some(theme), Some(fonts)) = (theme, fonts) else {
        return;
    };
    commands
        .spawn((
            CharSelectRoot,
            GlobalZIndex(zlayer::MODAL_WINDOWS),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(theme.spacing.lg),
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.96)),
        ))
        .with_children(|root| {
            // NOT tagged `LocalizedText`: unlike a fixed heading, this text's
            // resolved value depends on BOTH the locale AND which screen/step
            // is showing (`sync_step_title` recomputes both — the same
            // "state-dependent value, not a plain tag" posture
            // `settings_window.rs`'s `SettingValueLabel`/`refresh_setting_
            // labels` uses instead of `LocalizedLabel` for its cycle-row
            // values). `sync_step_title` seeds/updates it every frame either
            // one changes.
            root.spawn((
                StepTitleText,
                text_bundle(
                    &fonts,
                    &theme,
                    30.0,
                    localization.tr("char_selection-select_character"),
                ),
            ));

            // Middle row: content column + 3D preview.
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.lg),
                align_items: AlignItems::FlexStart,
                ..Default::default()
            })
            .with_children(|row| {
                row.spawn((ContentColumn, Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(theme.spacing.sm),
                    min_width: Val::Px(320.0),
                    ..Default::default()
                }));
                // Preview image (handle wired by `sync_preview_image`).
                row.spawn((PreviewImage, ImageNode::default(), Node {
                    width: Val::Px(220.0),
                    height: Val::Px(300.0),
                    ..Default::default()
                }));
            });

            // Name row (wizard only).
            root.spawn((NameRow, Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.sm),
                align_items: AlignItems::Center,
                display: Display::None,
                ..Default::default()
            }))
            .with_children(|nr| {
                // No colon: `LocalizedText`'s `relocalize_text` sets `Text.0`
                // straight from the resolved `.ftl` VALUE with no room for
                // Rust-side wrapping/punctuation (see that component's doc
                // comment) — matches `settings_window.rs`'s `row_label`
                // convention (a bare label, gapped from its value by layout,
                // never a baked-in colon).
                nr.spawn((
                    LocalizedText("char_selection-summary_label_name"),
                    text_bundle(
                        &fonts,
                        &theme,
                        20.0,
                        localization.tr("char_selection-summary_label_name"),
                    ),
                ));
                nr.spawn((NameValueText, text_bundle(&fonts, &theme, 20.0, "")));
            });

            // Nav row (wizard only): Back | Cancel | Next/Create. Every
            // button here is PERSISTENT (spawned once, never despawned while
            // the screen is up), so each is tagged `LocalizedLabel` +
            // resolved via `localization.tr` at spawn — the global
            // `relocalize_button_labels`/`spawn_button_labels` chain
            // (registered by `XindelerUiPlugin`, already added above) then
            // keeps them live on a locale change with no extra wiring here.
            // The legacy `<`/`>` chrome arrows this button pair used to carry
            // are dropped: `Localization::tr` cannot interpolate a Rust-side
            // wrapper around its resolved value, and the legacy client's own
            // `char_selection-wizard_back`/`wizard_next` buttons show the
            // bare word ("Back"/"Next") with no arrow either — see
            // `voxygen/src/menu/char_selection/ui.rs`.
            root.spawn((NavRow, Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(theme.spacing.md),
                display: Display::None,
                ..Default::default()
            }))
            .with_children(|nav| {
                nav.spawn(button_bundle(
                    &theme,
                    &fonts,
                    &localization.tr("char_selection-wizard_back"),
                ))
                .insert(LocalizedLabel("char_selection-wizard_back"))
                .observe(|_: On<Activate>, mut w: MessageWriter<CharAction>| {
                    w.write(CharAction::Back);
                });
                nav.spawn(button_bundle(
                    &theme,
                    &fonts,
                    &localization.tr("common-cancel"),
                ))
                .insert(LocalizedLabel("common-cancel"))
                .observe(|_: On<Activate>, mut w: MessageWriter<CharAction>| {
                    w.write(CharAction::CancelWizard);
                });
                // Next and Create are separate buttons toggled by `Display`
                // (a persistent `button_bundle` owns its own label, so we
                // never mutate one button's text — `sync_nav_visibility` shows
                // exactly one of these per step). Create exists ONLY on the
                // last step (spec §Navigation).
                nav.spawn(button_bundle(
                    &theme,
                    &fonts,
                    &localization.tr("char_selection-wizard_next"),
                ))
                .insert((NextButton, LocalizedLabel("char_selection-wizard_next")))
                .observe(|_: On<Activate>, mut w: MessageWriter<CharAction>| {
                    w.write(CharAction::Next);
                });
                nav.spawn(button_bundle(
                    &theme,
                    &fonts,
                    &localization.tr("common-create"),
                ))
                .insert((CreateButton, LocalizedLabel("common-create")))
                .observe(|_: On<Activate>, mut w: MessageWriter<CharAction>| {
                    w.write(CharAction::Create);
                });
            });
        });
}

fn despawn_screen(mut commands: Commands, roots: Query<Entity, With<CharSelectRoot>>) {
    for e in &roots {
        commands.entity(e).despawn();
    }
}

// -- data ingest -------------------------------------------------------------

/// Folds every arriving [`NetCharList`] broadcast into [`CharListData`].
fn ingest_char_list(mut reader: MessageReader<NetCharList>, mut data: ResMut<CharListData>) {
    for msg in reader.read() {
        data.0 = msg.clone();
    }
}

// -- name input (owned buffer, chat.rs pattern) ------------------------------

fn read_name_input(
    screen: Res<CharSelectScreen>,
    mut keyboard: MessageReader<KeyboardInput>,
    mut state: ResMut<WizardState>,
) {
    if *screen != CharSelectScreen::Wizard {
        keyboard.read().for_each(drop);
        return;
    }
    for ev in keyboard.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Backspace => {
                state.name.pop();
            },
            Key::Space => {
                if state.name.len() < common::character::MAX_NAME_LENGTH {
                    state.name.push(' ');
                }
            },
            Key::Character(s) => {
                for c in s.chars() {
                    if !c.is_control() && state.name.len() < common::character::MAX_NAME_LENGTH {
                        state.name.push(c);
                    }
                }
            },
            _ => {},
        }
    }
}

// -- action application ------------------------------------------------------

fn apply_char_actions(
    mut actions: MessageReader<CharAction>,
    mut state: ResMut<WizardState>,
    mut screen: ResMut<CharSelectScreen>,
    mut creates: MessageWriter<LocalCharCreate>,
    mut deletes: MessageWriter<LocalCharDelete>,
    mut selects: MessageWriter<LocalCharSelect>,
) {
    for action in actions.read() {
        match action.clone() {
            CharAction::OpenWizard => {
                *state = WizardState::new_default();
                *screen = CharSelectScreen::Wizard;
            },
            CharAction::CancelWizard => {
                *screen = CharSelectScreen::Roster;
            },
            CharAction::Next => state.advance(),
            CharAction::Back => {
                if state.step == WizardStep::Body {
                    *screen = CharSelectScreen::Roster;
                } else {
                    state.retreat();
                }
            },
            CharAction::SetSpecies(species) => state.set_species(species),
            CharAction::SetBodyType(body_type) => state.set_body_type(body_type),
            CharAction::CycleAppearance(field, delta) => state.cycle_appearance(field, delta),
            CharAction::SetClass(class) => state.set_class(class),
            CharAction::SetWeapon(i) => state.weapon_choice = i,
            CharAction::SetAlignment(order, moral) => {
                state.order = order;
                state.moral = moral;
            },
            CharAction::SetBackground(bg) => state.background = bg,
            CharAction::ToggleHardcore => state.hardcore = !state.hardcore,
            CharAction::Create => {
                if state.can_create() {
                    creates.write(LocalCharCreate(state.create_params()));
                }
            },
            CharAction::SelectCharacter(id) => {
                selects.write(LocalCharSelect(id));
            },
            CharAction::DeleteCharacter(id) => {
                deletes.write(LocalCharDelete(id));
            },
        }
    }
}

// -- content rebuild (change-gated) ------------------------------------------

#[allow(clippy::too_many_arguments)]
/// The subset of [`WizardState`] that actually changes what [`rebuild_content`]
/// spawns — everything EXCEPT [`WizardState::name`]. Typing a name must not
/// tear down and respawn the whole step's buttons/observers on every
/// keystroke (bevy-migration-reviewer follow-up: `state.is_changed()` fires
/// on any field write, including name edits); [`sync_name_value`] already
/// reflects the live name into its own dedicated text node without touching
/// this column at all.
#[derive(Debug, Clone, PartialEq)]
struct WizardContentKey {
    step: WizardStep,
    species: Species,
    body_type: BodyType,
    hair_style: u8,
    hair_color: u8,
    skin: u8,
    eyes: u8,
    eye_color: u8,
    beard: u8,
    accessory: u8,
    class: ClassKind,
    weapon_choice: usize,
    order: Order,
    moral: Moral,
    background: Option<BackgroundKind>,
    hardcore: bool,
}

impl From<&WizardState> for WizardContentKey {
    fn from(s: &WizardState) -> Self {
        Self {
            step: s.step,
            species: s.species,
            body_type: s.body_type,
            hair_style: s.hair_style,
            hair_color: s.hair_color,
            skin: s.skin,
            eyes: s.eyes,
            eye_color: s.eye_color,
            beard: s.beard,
            accessory: s.accessory,
            class: s.class,
            weapon_choice: s.weapon_choice,
            order: s.order,
            moral: s.moral,
            background: s.background,
            hardcore: s.hardcore,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild_content(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    state: Res<WizardState>,
    screen: Res<CharSelectScreen>,
    list: Res<CharListData>,
    current_locale: Res<CurrentLocale>,
    localization: NonSend<Localization>,
    column: Query<Entity, With<ContentColumn>>,
    children_query: Query<&Children>,
    // BL-82 EM-5.16 (T56.44): the cache key now also carries the CURRENT
    // locale tag. This whole column is torn down and rebuilt from scratch on
    // any key change (never persistent, per-node `LocalizedText`/
    // `LocalizedLabel` tagging inside `build_roster`/`build_wizard_step`), so
    // folding the locale into the key is what makes a language switch
    // actually re-resolve every roster/wizard string live — without it, a
    // locale flip while sitting on an unchanged screen/state would leave
    // this whole column showing the OLD language until some unrelated
    // roster/wizard change happened to force a rebuild anyway.
    mut last_key: Local<Option<(CharSelectScreen, WizardContentKey, NetCharList, String)>>,
) {
    let key = (
        *screen,
        WizardContentKey::from(&*state),
        list.0.clone(),
        current_locale.0.clone(),
    );
    if last_key.as_ref() == Some(&key) {
        return;
    }
    *last_key = Some(key);
    let Ok(root) = column.single() else { return };

    if let Ok(children) = children_query.get(root) {
        for &child in children {
            commands.entity(child).despawn();
        }
    }

    commands.entity(root).with_children(|parent| match *screen {
        CharSelectScreen::Roster => build_roster(parent, &theme, &fonts, &list, &localization),
        CharSelectScreen::Wizard => {
            build_wizard_step(parent, &theme, &fonts, &state, &localization)
        },
    });
}

fn build_roster(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    list: &CharListData,
    localization: &Localization,
) {
    if list.0.loading {
        parent.spawn(text_bundle(
            fonts,
            theme,
            20.0,
            localization.tr("char_selection-loading_characters"),
        ));
    } else if list.0.characters.is_empty() {
        parent.spawn(text_bundle(
            fonts,
            theme,
            20.0,
            localization.tr("char_selection-no_characters_yet"),
        ));
    } else {
        for entry in &list.0.characters {
            let NetCharListEntry {
                id,
                alias,
                hardcore,
                location,
                ..
            } = entry;
            // `alias`/`location` are player/world DATA, never translatable;
            // only the "[Hardcore]" tag word itself resolves through the
            // active locale — the surrounding brackets/`@` are left as plain
            // chrome (no dedicated key exists for either, the same posture
            // `numeric_row`'s bare `-`/`+` glyphs take).
            let hardcore_tag = if *hardcore {
                format!("  [{}]", localization.tr("char_selection-hardcore"))
            } else {
                String::new()
            };
            let label = format!(
                "{alias}{hardcore_tag}{}",
                location
                    .as_ref()
                    .map(|l| format!("  @ {l}"))
                    .unwrap_or_default()
            );
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(theme.spacing.sm),
                    align_items: AlignItems::Center,
                    ..Default::default()
                })
                .with_children(|row| {
                    row.spawn(text_bundle(fonts, theme, 20.0, label));
                    spawn_action_button(
                        row,
                        theme,
                        fonts,
                        &localization.tr("char_selection-enter_world"),
                        CharAction::SelectCharacter(*id),
                    );
                    spawn_action_button(
                        row,
                        theme,
                        fonts,
                        &localization.tr("char_selection-delete"),
                        CharAction::DeleteCharacter(*id),
                    );
                });
        }
    }
    // Legacy's own button for this exact action shows the bare phrase with no
    // "+" prefix (`voxygen/src/menu/char_selection/ui.rs`'s
    // `char_selection-create_new_character` use) — dropped here for the same
    // reason the wizard nav's `<`/`>` arrows were dropped (see
    // `spawn_screen`'s doc comment on the Nav row).
    spawn_action_button(
        parent,
        theme,
        fonts,
        &localization.tr("char_selection-create_new_character"),
        CharAction::OpenWizard,
    );
}

fn build_wizard_step(
    parent: &mut ChildSpawnerCommands,
    theme: &HudTheme,
    fonts: &HudFonts,
    state: &WizardState,
    localization: &Localization,
) {
    match state.step {
        WizardStep::Body => {
            // "Body type" (Female/Male) reuses `char_selection-sex` — the
            // exact key the legacy `voxygen/src/menu/char_selection/ui.rs`
            // resolves for this same Male/Female picker.
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                localization.tr("char_selection-sex"),
            ));
            for bt in [BodyType::Female, BodyType::Male] {
                spawn_action_button(
                    parent,
                    theme,
                    fonts,
                    &localization.tr(body_type_label_key(bt)),
                    CharAction::SetBodyType(bt),
                );
            }
            // "Species" section header reuses `char_selection-step_body`
            // ("Race") — the same key this step's own title (`title_key`)
            // resolves; no dedicated "Species" header key exists (the legacy
            // UI shows this picker as icon-only tooltips, with no visible
            // text header at all — see that file's `species` `Column`).
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                localization.tr("char_selection-step_body"),
            ));
            for species in humanoid::ALL_SPECIES {
                spawn_action_button(
                    parent,
                    theme,
                    fonts,
                    &localization.tr(species_label_key(species)),
                    CharAction::SetSpecies(species),
                );
            }
        },
        WizardStep::Appearance => {
            for (label_key, field) in [
                ("char_selection-hair_style", AppearanceField::HairStyle),
                ("char_selection-hair_color", AppearanceField::HairColor),
                ("char_selection-skin", AppearanceField::Skin),
                // "Eyes" (eye SHAPE, not color) matches the legacy
                // `char_selection-eyeshape` ("Eye Details") concept, not a
                // generic "eyes" noun.
                ("char_selection-eyeshape", AppearanceField::Eyes),
                ("char_selection-eye_color", AppearanceField::EyeColor),
                ("char_selection-beard", AppearanceField::Beard),
                ("char_selection-accessories", AppearanceField::Accessory),
            ] {
                parent
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(theme.spacing.sm),
                        align_items: AlignItems::Center,
                        ..Default::default()
                    })
                    .with_children(|row| {
                        row.spawn(text_bundle(fonts, theme, 18.0, localization.tr(label_key)));
                        // `<`/`>` are bare mathematical/directional glyphs,
                        // deliberately left unlocalized — the same posture
                        // `settings_window.rs`'s `numeric_row` documents for
                        // its own `-`/`+` glyphs.
                        spawn_action_button(
                            row,
                            theme,
                            fonts,
                            "<",
                            CharAction::CycleAppearance(field, -1),
                        );
                        spawn_action_button(
                            row,
                            theme,
                            fonts,
                            ">",
                            CharAction::CycleAppearance(field, 1),
                        );
                    });
            }
        },
        WizardStep::Class => {
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                localization.tr("char_selection-class"),
            ));
            for class in wizard_classes() {
                spawn_action_button(
                    parent,
                    theme,
                    fonts,
                    &localization.tr(class_label_key(class)),
                    CharAction::SetClass(class),
                );
            }
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                format!(
                    "{} ({})",
                    localization.tr("char_selection-weapon_for_class"),
                    localization.tr(class_label_key(state.class))
                ),
            ));
            // ONLY the selected class's valid weapons are spawned — the other
            // classes' weapons are never rendered, so they are unselectable.
            for (i, opt) in class_weapon_options(state.class).iter().enumerate() {
                spawn_action_button(
                    parent,
                    theme,
                    fonts,
                    &localization.tr(opt.label),
                    CharAction::SetWeapon(i),
                );
            }
        },
        WizardStep::Alignment => {
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                localization.tr("char_selection-alignment"),
            ));
            for order in ALIGNMENT_ORDERS {
                for moral in ALIGNMENT_MORALS {
                    spawn_action_button(
                        parent,
                        theme,
                        fonts,
                        &alignment_label(order, moral, localization),
                        CharAction::SetAlignment(order, moral),
                    );
                }
            }
        },
        WizardStep::Background => {
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                localization.tr("char_selection-background"),
            ));
            spawn_action_button(
                parent,
                theme,
                fonts,
                &localization.tr("char_selection-background_uncommitted"),
                CharAction::SetBackground(None),
            );
            // `BackgroundKind::display_name()` is a documented P1 stand-in
            // (see that method's own doc comment: BL-31 P3 is the separately
            // tracked content phase that will author real
            // `background-<name>-title` i18n keys) — genuinely left English
            // here, not a literal in THIS file, and out of this task's scope.
            for bg in BackgroundKind::ALL {
                spawn_action_button(
                    parent,
                    theme,
                    fonts,
                    &bg.display_name(),
                    CharAction::SetBackground(Some(bg)),
                );
            }
        },
        WizardStep::Finish => {
            // BL-82 EM-5.16 (T56.44): the old single interpolated sentence
            // ("{name} the {class} — {alignment}") is split into one row per
            // field, reusing the `char_selection-summary_label_*` keys the
            // catalog already ships for exactly this — `Localization::tr` has
            // no placeable/argument support (see its own doc comment), so a
            // translated word order can never be baked into a single
            // Fluent-interpolated message the way the old literal was.
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                format!(
                    "{}: {}",
                    localization.tr("char_selection-summary_label_name"),
                    state.name.trim()
                ),
            ));
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                format!(
                    "{}: {}",
                    localization.tr("char_selection-summary_label_class"),
                    localization.tr(class_label_key(state.class))
                ),
            ));
            parent.spawn(text_bundle(
                fonts,
                theme,
                18.0,
                format!(
                    "{}: {}",
                    localization.tr("char_selection-summary_label_alignment"),
                    alignment_label(state.order, state.moral, localization)
                ),
            ));
            spawn_action_button(
                parent,
                theme,
                fonts,
                &format!(
                    "{}: {}",
                    localization.tr("char_selection-hardcore"),
                    localization.tr(if state.hardcore {
                        "common-on"
                    } else {
                        "common-off"
                    })
                ),
                CharAction::ToggleHardcore,
            );
            if state.name.trim().is_empty() {
                parent.spawn(text_bundle(
                    fonts,
                    theme,
                    16.0,
                    localization.tr("char_selection-create_info_name"),
                ));
            }
        },
    }
}

// -- sync systems ------------------------------------------------------------

/// Recomputes the step/screen title from the current screen, step AND
/// locale — a state-dependent value (not a plain `LocalizedText` tag),
/// mirroring `settings_window.rs`'s `refresh_setting_labels` posture (see
/// `spawn_screen`'s doc comment on `StepTitleText`). Numeral formatting
/// happens here in Rust, around [`WizardStep::title_key`]'s resolved value —
/// `Localization::tr` has no placeable support.
fn sync_step_title(
    screen: Res<CharSelectScreen>,
    state: Res<WizardState>,
    current_locale: Res<CurrentLocale>,
    localization: NonSend<Localization>,
    mut title: Query<&mut Text, With<StepTitleText>>,
) {
    if !screen.is_changed() && !state.is_changed() && !current_locale.is_changed() {
        return;
    }
    let Ok(mut text) = title.single_mut() else {
        return;
    };
    let new = match *screen {
        CharSelectScreen::Roster => localization.tr("char_selection-select_character"),
        CharSelectScreen::Wizard => format!(
            "{}/6  {}",
            state.step.index() + 1,
            localization.tr(state.step.title_key())
        ),
    };
    if text.0 != new {
        text.0 = new;
    }
}

fn sync_name_value(state: Res<WizardState>, mut value: Query<&mut Text, With<NameValueText>>) {
    if !state.is_changed() {
        return;
    }
    if let Ok(mut text) = value.single_mut() {
        text.0 = state.name.clone();
    }
}

#[allow(clippy::type_complexity)]
fn sync_nav_visibility(
    screen: Res<CharSelectScreen>,
    state: Res<WizardState>,
    mut name_row: Query<
        &mut Node,
        (
            With<NameRow>,
            Without<NavRow>,
            Without<NextButton>,
            Without<CreateButton>,
        ),
    >,
    mut nav_row: Query<
        &mut Node,
        (
            With<NavRow>,
            Without<NameRow>,
            Without<NextButton>,
            Without<CreateButton>,
        ),
    >,
    mut next_btn: Query<
        &mut Node,
        (
            With<NextButton>,
            Without<CreateButton>,
            Without<NameRow>,
            Without<NavRow>,
        ),
    >,
    mut create_btn: Query<
        &mut Node,
        (
            With<CreateButton>,
            Without<NextButton>,
            Without<NameRow>,
            Without<NavRow>,
        ),
    >,
) {
    if !screen.is_changed() && !state.is_changed() {
        return;
    }
    let wizard = *screen == CharSelectScreen::Wizard;
    let show = if wizard { Display::Flex } else { Display::None };
    if let Ok(mut node) = name_row.single_mut() {
        node.display = show;
    }
    if let Ok(mut node) = nav_row.single_mut() {
        node.display = show;
    }
    // Exactly one of Next / Create per step; Create only on the last step.
    if let Ok(mut node) = next_btn.single_mut() {
        node.display = if wizard && !state.step.is_last() {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let Ok(mut node) = create_btn.single_mut() {
        node.display = if wizard && state.step.is_last() {
            Display::Flex
        } else {
            Display::None
        };
    }
}

fn sync_preview_image(
    preview: Option<Res<CharPreview>>,
    mut nodes: Query<&mut ImageNode, With<PreviewImage>>,
) {
    let Some(preview) = preview else { return };
    if let Ok(mut node) = nodes.single_mut()
        && node.image != preview.image
    {
        node.image = preview.image.clone();
    }
}

/// Feeds the 3D preview the body it should show: the wizard's live body while
/// creating, or the first roster character's body on the roster screen.
fn feed_preview_body(
    screen: Res<CharSelectScreen>,
    state: Res<WizardState>,
    list: Res<CharListData>,
    preview: Option<ResMut<CharPreview>>,
) {
    let Some(mut preview) = preview else { return };
    let body = match *screen {
        CharSelectScreen::Wizard => Some(state.body()),
        CharSelectScreen::Roster => list.0.characters.first().map(|c| c.body),
    };
    if preview.desired_body != body {
        preview.desired_body = body;
    }
}

/// Once the mirrored local player entity appears (spawn succeeded), leave the
/// char-select screen for [`AppState::InGame`].
fn enter_world_when_ready(
    player: Query<(), With<NetLocalPlayer>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if !player.is_empty() {
        next.set(AppState::InGame);
    }
}

// -- display labels (BL-82 EM-5.16 T56.44: real i18n) ------------------------
//
// Every function below returns an `.ftl` KEY (never literal display text) —
// callers resolve it via `Localization::tr` at spawn time. New keys
// (`char_selection-body_female`/`body_male`) were added only where NEITHER
// `char_selection.ftl` nor `common.ftl` already had a matching concept (see
// this module's own doc comment / the task's ground-truth check against the
// legacy `voxygen/src/menu/char_selection/ui.rs`, which this crate's Bevy
// port otherwise mirrors key-for-key).

fn body_type_label_key(bt: BodyType) -> &'static str {
    match bt {
        BodyType::Female => "char_selection-body_female",
        BodyType::Male => "char_selection-body_male",
    }
}

/// Reuses the `common-species-*` catalog — the same keys the legacy
/// species-picker tooltips resolve (`voxygen/src/menu/char_selection/ui.rs`'s
/// `icon_button_tooltip(.., "common-species-human")` etc).
fn species_label_key(species: Species) -> &'static str {
    match species {
        Species::Danari => "common-species-danari",
        Species::Dwarf => "common-species-dwarf",
        Species::Elf => "common-species-elf",
        Species::Human => "common-species-human",
        Species::Orc => "common-species-orc",
        Species::Draugr => "common-species-draugr",
    }
}

/// The proof-slice four reuse `char_selection-class_*` — the exact keys the
/// legacy class-step buttons resolve for these same four classes
/// (`voxygen/src/menu/char_selection/ui.rs`). Every OTHER playable class (this
/// port's wizard offers all of `ClassKind::PLAYABLE`, not just the proof
/// slice) falls back to the fuller `common-class-*` catalog, which has no
/// gap across any `ClassKind` variant.
fn class_label_key(class: ClassKind) -> &'static str {
    match class {
        ClassKind::Warrior => "char_selection-class_warrior",
        ClassKind::Mage => "char_selection-class_mage",
        ClassKind::Cleric => "char_selection-class_cleric",
        ClassKind::Rogue => "char_selection-class_rogue",
        ClassKind::Adventurer => "common-class-adventurer",
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

fn order_label_key(order: Order) -> &'static str {
    match order {
        Order::Lawful => "char_selection-ethos_lawful",
        Order::Neutral => "char_selection-ethos_neutral",
        Order::Chaotic => "char_selection-ethos_chaotic",
    }
}

fn moral_label_key(moral: Moral) -> &'static str {
    match moral {
        Moral::Good => "char_selection-ethos_good",
        Moral::Neutral => "char_selection-ethos_neutral",
        Moral::Evil => "char_selection-ethos_evil",
    }
}

/// True Neutral gets its own single-word key (`char_selection-ethos_true_
/// neutral`, matching legacy); every other box composes `"{Order} {Moral}"`
/// from the two resolved words — this can't be one Fluent-interpolated
/// message (`Localization::tr` has no placeable support), so the composition
/// happens here in Rust around two already-`tr`'d words.
fn alignment_label(order: Order, moral: Moral, localization: &Localization) -> String {
    if order == Order::Neutral && moral == Moral::Neutral {
        localization.tr("char_selection-ethos_true_neutral")
    } else {
        format!(
            "{} {}",
            localization.tr(order_label_key(order)),
            localization.tr(moral_label_key(moral))
        )
    }
}

/// Test-only: an empty-catalog `Localization` — every `.tr(key)` call
/// resolves to `key` itself (the documented, never-panic fallback), which is
/// all the structural tests below need (matching `settings_window.rs`/
/// `esc_menu.rs`'s own `test_localization` helper) — see
/// `switching_locale_relocalizes_the_cancel_button_live` for the one test
/// that DOES need the real repo catalogs.
#[cfg(test)]
fn test_localization() -> Localization {
    Localization::load(&xindeler_ui::i18n::fallback_locale(), &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step order is exactly the spec's Body→Appearance→Class→Alignment→
    /// Background→Finish, and navigation clamps at both ends.
    #[test]
    fn wizard_step_order_and_clamping() {
        assert_eq!(WizardStep::ORDER, [
            WizardStep::Body,
            WizardStep::Appearance,
            WizardStep::Class,
            WizardStep::Alignment,
            WizardStep::Background,
            WizardStep::Finish,
        ]);
        // Walking Next from Body reaches Finish in 5 steps and clamps there.
        let mut s = WizardStep::Body;
        for _ in 0..5 {
            s = s.next();
        }
        assert_eq!(s, WizardStep::Finish);
        assert_eq!(s.next(), WizardStep::Finish, "Next clamps at the last step");
        // Walking Back clamps at Body.
        let mut b = WizardStep::Finish;
        for _ in 0..5 {
            b = b.prev();
        }
        assert_eq!(b, WizardStep::Body);
        assert_eq!(b.prev(), WizardStep::Body, "Back clamps at the first step");
    }

    /// The default state is valid from step 1: a real body, a selected valid
    /// weapon, and — on the last step — Create is allowed (non-empty default
    /// name), so a player could hit Create without touching anything.
    #[test]
    fn defaults_are_valid_from_step_one() {
        let mut s = WizardState::new_default();
        assert_eq!(s.step, WizardStep::Body);
        assert!(matches!(s.body(), Body::Humanoid(_)));
        assert!(
            s.selected_weapon().is_some(),
            "a default weapon is selected"
        );
        assert!(!s.name.trim().is_empty(), "a non-empty default name");
        // Not creatable until the last step...
        assert!(!s.can_create());
        // ...but immediately creatable there, untouched.
        s.step = WizardStep::Finish;
        assert!(s.can_create());
        let params = s.create_params();
        assert_eq!(params.class, ClassKind::Warrior);
        assert_eq!(
            params.mainhand.as_deref(),
            Some("common.items.weapons.sword.starter"),
            "the default Warrior weapon is the starter sword"
        );
    }

    /// Create is allowed ONLY on the last step (and only with a name).
    #[test]
    fn create_only_on_last_step() {
        let mut s = WizardState::new_default();
        for step in [
            WizardStep::Body,
            WizardStep::Appearance,
            WizardStep::Class,
            WizardStep::Alignment,
            WizardStep::Background,
        ] {
            s.step = step;
            assert!(!s.can_create(), "no Create before the last step ({step:?})");
        }
        s.step = WizardStep::Finish;
        assert!(s.can_create());
        // An empty name blocks Create even on the last step.
        s.name = "   ".to_owned();
        assert!(!s.can_create(), "empty name blocks Create");
    }

    /// Only the selected class's valid weapons are offered; switching class
    /// resets the weapon choice; dual-wield carries an offhand.
    #[test]
    fn class_weapons_are_filtered() {
        assert_eq!(class_weapon_options(ClassKind::Warrior).len(), 3);
        assert_eq!(class_weapon_options(ClassKind::Mage).len(), 1);
        assert_eq!(class_weapon_options(ClassKind::Cleric).len(), 1);
        assert_eq!(class_weapon_options(ClassKind::Rogue).len(), 2);

        // Mage sees only the staff — never a sword/bow.
        let mage = class_weapon_options(ClassKind::Mage);
        assert_eq!(
            mage[0].mainhand,
            Some("common.items.weapons.staff.starter_staff")
        );

        // Rogue's first option is dual-wield (an offhand present).
        let rogue = class_weapon_options(ClassKind::Rogue);
        assert!(
            rogue[0].offhand.is_some(),
            "Rogue dual-swords has an offhand"
        );
        assert!(rogue[1].offhand.is_none(), "Rogue bow has no offhand");

        // Switching class resets the (possibly out-of-range) weapon choice.
        let mut s = WizardState::new_default();
        s.weapon_choice = 2; // valid for Warrior
        s.set_class(ClassKind::Mage);
        assert_eq!(
            s.weapon_choice, 0,
            "class switch resets weapon to the default"
        );
        assert_eq!(
            s.selected_weapon().and_then(|w| w.mainhand),
            Some("common.items.weapons.staff.starter_staff")
        );
    }

    /// `create_params` reflects the current choices (class, weapon, alignment,
    /// background, trimmed name).
    #[test]
    fn create_params_reflect_choices() {
        let mut s = WizardState::new_default();
        s.name = "  Aria  ".to_owned();
        s.set_class(ClassKind::Rogue);
        s.weapon_choice = 0; // dual swords
        s.order = Order::Chaotic;
        s.moral = Moral::Good;
        s.background = Some(BackgroundKind::Outlander);

        let p = s.create_params();
        assert_eq!(p.alias, "Aria", "name is trimmed");
        assert_eq!(p.class, ClassKind::Rogue);
        assert_eq!(
            p.mainhand.as_deref(),
            Some("common.items.weapons.sword_1h.starter")
        );
        assert_eq!(
            p.offhand.as_deref(),
            Some("common.items.weapons.sword_1h.starter")
        );
        assert_eq!(p.background, Background(Some(BackgroundKind::Outlander)));
        assert_eq!(p.ethos, Ethos::from_box(Order::Chaotic, Moral::Good));
    }

    /// Switching species never leaves an out-of-range appearance index (it is
    /// clamped via `humanoid::Body::validate`).
    #[test]
    fn set_species_clamps_appearance() {
        let mut s = WizardState::new_default();
        s.hair_style = 250;
        s.hair_color = 250;
        s.skin = 250;
        s.set_species(Species::Orc);
        // After clamping, every index is < its species/body-type count.
        let body = s.humanoid_body();
        assert!(body.hair_style < s.species.num_hair_styles(s.body_type).max(1));
        assert!(body.hair_color < s.species.num_hair_colors().max(1));
        assert!(body.skin < s.species.num_skin_colors().max(1));
    }

    /// `wizard_classes` puts the proof-slice four first and covers every
    /// playable class exactly once.
    #[test]
    fn wizard_classes_cover_all_playable() {
        let classes = wizard_classes();
        assert_eq!(&classes[..4], &[
            ClassKind::Warrior,
            ClassKind::Mage,
            ClassKind::Cleric,
            ClassKind::Rogue,
        ]);
        assert_eq!(classes.len(), ClassKind::PLAYABLE.len());
        assert!(
            !classes.contains(&ClassKind::Adventurer),
            "Adventurer is not playable"
        );
    }

    /// BL-82 EM-5.14 follow-up (zlayer audit): [`CharSelectRoot`] is a
    /// full-screen, independently-positioned window (spec/EM-5.17 §4.4
    /// `MODAL_WINDOWS` tier — it must draw over always-on ambient HUD chrome
    /// exactly like `DiaryWindowRoot`/`InventoryWindowRoot`/`FullMapRoot`).
    /// Mirrors `trade_ui.rs`'s `invite_and_trade_window_roots_carry_the_
    /// modal_windows_z_index` pattern: pins that `spawn_screen` actually
    /// attaches the `GlobalZIndex`, not just that the doc comment claims it.
    #[test]
    fn char_select_root_carries_the_modal_windows_z_index() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(test_localization());

        app.world_mut()
            .run_system_once(spawn_screen)
            .expect("spawn_screen runs");

        let world = app.world_mut();
        let z_index = world
            .query_filtered::<&GlobalZIndex, With<CharSelectRoot>>()
            .single(world)
            .expect("CharSelectRoot exists")
            .0;
        assert_eq!(z_index, zlayer::MODAL_WINDOWS);
    }

    /// Regression test for the real startup panic (Matías, live-testing
    /// 2026-07-18): `spawn_screen` used to take hard `Res<HudTheme>`/
    /// `Res<HudFonts>`, which panicked ("Resource does not exist") the first
    /// time `CharSelect` was the process's INITIAL `AppState` (the
    /// `--char-select` launch path) — see `CharSelectViewPlugin::build`'s
    /// registration comment for the root cause (Bevy's initial-state
    /// `OnEnter` fires before `Startup`, which is what seeds those
    /// resources). `spawn_screen` must run cleanly with neither resource
    /// present and must NOT spawn a half-built [`CharSelectRoot`].
    #[test]
    fn spawn_screen_does_not_panic_before_hud_theme_exists() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Deliberately NO `HudTheme`/`HudFonts` inserted — reproduces the
        // pre-`Startup` window `OnEnter(initial_state)` actually runs in.
        // `Localization` IS inserted (it's orthogonal to this specific
        // regression — see `spawn_screen`'s own doc comment on why it's a
        // hard `NonSend` param, not `Option`-wrapped like Theme/Fonts).
        app.insert_non_send(test_localization());

        app.world_mut()
            .run_system_once(spawn_screen)
            .expect("spawn_screen must not panic when HudTheme/HudFonts are missing");

        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<CharSelectRoot>>()
                .iter(world)
                .count(),
            0,
            "no screen should be spawned until HudTheme/HudFonts exist"
        );
    }

    /// Companion to the test above: once `HudTheme`/`HudFonts` become
    /// available (mirroring `theme::init_theme` finally running at
    /// `Startup`, one frame after the pre-`Startup` `OnEnter` that used to
    /// panic), a later run of the SAME `Update`-scheduled `spawn_screen`
    /// picks them up and builds the screen exactly once — proving the
    /// "retry next frame" fix actually converges, not just that it avoids
    /// panicking forever.
    #[test]
    fn spawn_screen_spawns_once_hud_theme_becomes_available() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_non_send(test_localization());

        // First frame: resources not ready yet — no-op (see the test above).
        app.world_mut()
            .run_system_once(spawn_screen)
            .expect("spawn_screen runs with no resources");

        // "Startup" finally ran: seed HudTheme/HudFonts.
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });

        // Second frame: now it builds the screen...
        app.world_mut()
            .run_system_once(spawn_screen)
            .expect("spawn_screen runs once resources exist");
        {
            let world = app.world_mut();
            assert_eq!(
                world
                    .query_filtered::<Entity, With<CharSelectRoot>>()
                    .iter(world)
                    .count(),
                1,
                "the screen spawns exactly once resources are ready"
            );
        }

        // ...and a THIRD frame must not spawn a second root on top of it.
        app.world_mut()
            .run_system_once(spawn_screen)
            .expect("spawn_screen runs again");
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<CharSelectRoot>>()
                .iter(world)
                .count(),
            1,
            "spawn_screen must not double-spawn the root on a later frame"
        );
    }

    /// End-to-end regression test exercising the REAL Bevy scheduling quirk
    /// that caused the live panic, not just a hand-simulated approximation:
    /// `CharSelect` set as the process's INITIAL `AppState` via
    /// `App::insert_state` (exactly what `main.rs` does for
    /// `--listen-server --char-select`), with `HudTheme`/`HudFonts` seeded by
    /// a REAL `Startup` system (standing in for `XindelerUiPlugin`'s
    /// `theme::init_theme`, minus the real `AssetServer` round-trip this
    /// test doesn't need). Before the fix, `OnEnter(AppState::CharSelect)`
    /// ran `spawn_screen` directly and this panicked on the very first
    /// `app.update()` — see `bevy_app::main_schedule`'s own doc comment: the
    /// initial state's `OnEnter` fires in a `StateTransition` pass that
    /// precedes `PreStartup`/`Startup`/`PostStartup`. After the fix,
    /// `spawn_screen` is `Update`-scheduled (which still runs AFTER
    /// `Startup`, even on this same first frame), so the screen is up with
    /// no panic by the time `update()` returns.
    #[test]
    fn char_select_survives_being_the_initial_app_state() {
        use bevy::state::app::StatesPlugin;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(StatesPlugin);
        // Stand-in for `XindelerUiPlugin`'s `theme::init_theme`.
        app.add_systems(Startup, |mut commands: Commands| {
            commands.insert_resource(HudTheme::default());
            commands.insert_resource(HudFonts {
                title: Handle::default(),
                body: Handle::default(),
            });
        });
        // `Localization` is inserted directly by `XindelerUiPlugin::build`
        // (not deferred to `Startup`) — this hand-rolled stand-in app mirrors
        // that by inserting it up front too.
        app.insert_non_send(test_localization());
        app.init_resource::<WizardState>()
            .init_resource::<CharSelectScreen>()
            .add_systems(OnEnter(AppState::CharSelect), reset_state)
            .add_systems(OnExit(AppState::CharSelect), despawn_screen)
            .add_systems(Update, spawn_screen.run_if(in_state(AppState::CharSelect)));
        // Exactly like `main.rs`'s `--char-select` bypass: `CharSelect` is
        // the INITIAL state, not entered via a later transition.
        app.insert_state(AppState::CharSelect);

        // Must not panic — this is the actual bug.
        app.update();

        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<CharSelectRoot>>()
                .iter(world)
                .count(),
            1,
            "the char-select screen must be up by the end of the very first frame, even though \
             its OnEnter ran before Startup"
        );
    }

    /// BL-82 EM-5.16 (T56.44): switching the active locale re-localizes the
    /// already-spawned, persistent `NavRow` Cancel button live, using the
    /// REAL repo `.ftl` catalogs (`char_selection.ftl` + `common.ftl`) via
    /// `VELOREN_ASSETS`/`XINDELER_ASSETS` — the same real-catalog proof
    /// `esc_menu.rs`'s own hot-swap test uses, exercised here against a
    /// `char_select.rs`-spawned `LocalizedLabel`-tagged button (`common-
    /// cancel`, not one of this screen's own `char_selection-*` keys, since
    /// that key is genuinely translated in the real `es` catalog — see this
    /// module's own key-mapping notes).
    #[test]
    fn switching_locale_relocalizes_the_cancel_button_live() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &["char_selection.ftl", "common.ftl"],
        ));
        app.init_resource::<xindeler_ui::i18n::CurrentLocale>();
        // `button::spawn_button_labels` is what turns `HudButtonLabel` into a
        // real `Text` child — needed for BOTH the initial spawn and the
        // post-hot-swap relabel this test drives.
        app.add_systems(Update, xindeler_ui::button::spawn_button_labels);

        app.world_mut()
            .run_system_once(spawn_screen)
            .expect("spawn_screen runs");
        app.update(); // let spawn_button_labels give the Cancel button its child

        fn cancel_button_text(app: &mut App) -> String {
            let world = app.world_mut();
            let button = world
                .query::<(&LocalizedLabel, &Children)>()
                .iter(world)
                .find(|(tag, _)| tag.0 == "common-cancel")
                .map(|(_, children)| children[0])
                .expect("the Cancel button was spawned and tagged");
            world
                .get::<Text>(button)
                .expect("label child exists")
                .0
                .clone()
        }

        assert_eq!(
            cancel_button_text(&mut app),
            "Cancel",
            "the Cancel button must show the real en catalog text at spawn time"
        );

        app.world_mut()
            .resource_mut::<xindeler_ui::i18n::CurrentLocale>()
            .0 = "es".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::relocalize_button_labels)
            .expect("relocalize runs");
        app.update(); // spawn_button_labels propagates the HudButtonLabel change onto Text

        let after = cancel_button_text(&mut app);
        assert_eq!(
            after, "Cancelar",
            "must resolve to the REAL es catalog's own common-cancel value, not the en fallback"
        );
    }
}
