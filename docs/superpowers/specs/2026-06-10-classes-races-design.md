# Classes and Races: D&D-Inspired Class System

**Date:** 2026-06-10
**Companion specs:** `2026-06-10-character-levels-design.md` (character Level from total earned XP), `2026-06-10-equipment-restrictions-design.md` (class/level/race-gated items), `2026-06-10-magic-abilities-design.md` (new spell abilities)

## Context

Vanilla Veloren has no class concept. Progression is purely weapon-driven: a character earns
XP into per-weapon skill groups and unlocks skills in those trees. Races exist only as cosmetic
humanoid species with no gameplay differences. This fork wants D&D-style identity: a class chosen
at character creation that shapes skill access, starting equipment, ability unlocks, and (via the
companion spec) which items can be equipped — plus light racial passives so species choice matters.

This spec covers the class/race data model, character creation flow, persistence, racial traits,
and class skill trees. Equipment gating is specified separately in
`2026-06-10-equipment-restrictions-design.md` (Spec B), which depends on the `ClassKind` type
defined here.

## Goals

| Goal | Detail |
|---|---|
| 4 launch classes | Warrior, Mage, Cleric, Rogue |
| Extensible roster | Adding Ranger, Paladin, Warlock, Druid, Bard, Barbarian later = enum variant + assets, no schema change |
| Class chosen at creation | New picker in the character creation screen, validated and persisted server-side |
| Class skill trees | Each class gets its own XP pool and skill tree using the existing `SkillGroup` machinery |
| Racial passives | Per-species stat modifiers (small, flavorful, not build-defining) |
| Starting kits | Per-class loadout + skillset presets, data-driven in `assets/common/` |
| Legacy safety | Existing characters keep working with a default legacy class; opt-in re-pick |

## Non-Goals

- Multiclassing (explicitly out of scope for v1; see Open Questions).
- Class change/respec UI (a one-time admin/self-service command covers legacy characters).
- NPC classes — rtsim/agent NPCs are untouched; classes apply to player characters only.
- Copying any WotC-protected expression. Generic class names (Warrior, Mage, Cleric, Rogue,
  Paladin, Bard, ...) are fine; no SRD text, no D&D proper nouns (no beholders, no Faerûn),
  all skill/ability names and mechanics are original.

## Current State (verified)

### Races

`common/src/comp/body/humanoid.rs:116` defines the six playable species:

```rust
pub enum Species {
    Danari = 0,
    Dwarf = 1,
    Elf = 2,
    Human = 3,
    Orc = 4,
    Draugr = 5,
}
```

(Note: there is no `Undead`/`Demon` species — the undead-flavored race is `Draugr`.)
Species today affects body model, voice, and minor body-derived attributes only.

### Progression

- `common/src/comp/skillset/mod.rs:89` — `SkillGroupKind { General, Weapon(ToolKind) }`.
- `common/src/comp/skillset/mod.rs:217` — `SkillSet` holds `skill_groups: HashMap<SkillGroupKind, SkillGroup>` and a `skills: HashMap<Skill, u16>` map.
- `common/src/comp/skillset/mod.rs:142` — `SkillGroup` tracks `earned_exp`, `available_exp`, `earned_sp`, `available_sp` per group; per-group SP cost curves live in `SkillGroupKind::skill_point_cost` (`mod.rs:98`).
- `common/src/comp/skillset/skills.rs:13` — `Skill` enum: `Sword(..)`, `Axe(..)`, `Hammer(..)`, `Bow(..)`, `Staff(..)`, `Sceptre(..)`, `Climb(..)`, `Swim(..)`, `Pick(MiningSkill)`, `UnlockGroup(SkillGroupKind)`.
- Skill-tree topology is data-driven: `assets/common/skill_trees/skills_skill-groups_manifest.ron`, `skill_max_levels.ron`, `skill_prerequisites.ron`, loaded into `SKILL_GROUP_DEFS` / `SKILL_GROUP_HASHES` (`skillset/mod.rs:28-86`). Group hashes force a respec when a tree changes.
- `SkillSet::default()` (`skillset/mod.rs:237`) unlocks `General` and `Weapon(Pick)` for every new character.
- XP routing: `server/src/events/entity_manipulation.rs:505` `handle_exp_gain` splits kill XP evenly across `General` + the skill groups of equipped weapons.

### Abilities

`common/src/comp/inventory/item/tool.rs:320` — `AbilityKind::Simple(Option<Skill>, T)`: an
ability inside an ability set can be gated behind a `Skill`; the unlock check is at
`tool.rs:380`. Ability sets are declared in `assets/common/abilities/ability_set_manifest.ron`.
This is the existing hook that lets class skill trees unlock abilities with zero new machinery.

### Persistence

- SQLite via `rusqlite`; migrations via **refinery 0.9** (`server/Cargo.toml:78`), embedded from `server/src/migrations/` (`embed_migrations!("./src/migrations")` at `server/src/persistence/mod.rs:53-54`), run at startup by `run_migrations` (`persistence/mod.rs:169`). Files are named `V{n}__{name}.sql`; latest is `V70__merge_remaining_unique_recipes.sql`. (Diesel is gone; `diesel_to_rusqlite.rs` only converts pre-refinery DBs.)
- `character` table columns: `character_id, player_uuid, alias, waypoint, hardcore` (`server/src/persistence/models.rs:1-8`, INSERT at `server/src/persistence/character/mod.rs:525`).
- `skill_group` table: `entity_id, skill_group_kind (TEXT), earned_exp, spent_exp, skills, hash_val` (`models.rs:27-34`). `SkillGroupKind` ⇄ DB string mapping lives in `server/src/persistence/json_models.rs:71` (`skill_group_to_db_string`) and `:100` (`db_string_to_skill_group`); both **panic on unknown kinds**, so new groups must be added there.
- `PersistedComponents` (`server/src/persistence/mod.rs:35`) is the bundle written on creation: body, hardcore, stats, skill_set, inventory, waypoint, pets, active_abilities, map_marker.

### Character creation flow

1. UI: `voxygen/src/menu/char_selection/ui.rs` (species buttons ~line 1023-1112, starter weapon picker) emits `ui::Event::AddCharacter` handled at `voxygen/src/menu/char_selection/mod.rs:133-143`.
2. Client: `client/src/lib.rs:1318` `Client::create_character` sends `ClientGeneral::CreateCharacter { alias, mainhand, offhand, body, hardcore, start_site }` (`common/net/src/msg/client.rs:77`).
3. Server: handler at `server/src/sys/msg/character_screen.rs:174` calls `server/src/character_creator.rs:30` `create_character`, which validates against the `VALID_STARTER_ITEMS` whitelist (`character_creator.rs:11`), builds `LoadoutBuilder::empty().defaults()` + `SkillSet::default()`, and enqueues `PersistedComponents` via `CharacterUpdater`.

### Stats / racial-trait hook

`common/src/comp/stats.rs:73` — `Stats` already carries every modifier we need:
`move_speed_modifier`, `attack_damage_modifier`, `max_health_modifiers: StatsModifier`,
`max_energy_modifiers: StatsModifier`, `damage_reduction: StatsSplit`,
`crowd_control_resistance`, `recovery_speed_modifier`, `swim_speed_modifier`, etc.
`Stats::reset_temp_modifiers` (`stats.rs:147`) wipes them each tick and they are re-applied in
`common/systems/src/buff.rs:513` — racial passives plug in right after that reset.

### Component sync

New components replicate to clients by registration in the x-macro at
`common/net/src/synced_components.rs` (cf. `Stats` at line 25, `SkillSet` at line 68, with
`NetSync` impls below).

## Design

### 1. `ClassKind` and the `CharacterClass` component

New file `common/src/comp/class.rs`, exported from `common/src/comp/mod.rs`:

```rust
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
pub enum ClassKind {
    /// Legacy/default class for pre-class characters. No class tree, no gates.
    Adventurer,
    Warrior,
    Mage,
    Cleric,
    Rogue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterClass(pub ClassKind);

impl Component for CharacterClass {
    type Storage = DerefFlaggedStorage<Self, specs::storage::VecStorage<Self>>;
}
```

- Registered in `common-state` ECS setup alongside other comps, and added to the
  `synced_components!` x-macro in `common/net/src/synced_components.rs` so all clients can render
  class badges/tooltips (`NetSync` with full sync like `Stats`).
- Future classes (Ranger, Paladin, Warlock, Druid, Bard, Barbarian) are new variants; everything
  downstream (skill groups, DB strings, kits, gates) is keyed off the variant.
- `Adventurer` is a real variant, not an `Option`: it removes `Option<ClassKind>` plumbing
  everywhere and gives legacy characters defined semantics (access to all current weapon trees,
  no class tree, never gated by class requirements in Spec B).

### 2. Class skill trees: extend `SkillGroupKind` vs. parallel system

| | Option 1: `SkillGroupKind::Class(ClassKind)` | Option 2: separate class-tree system |
|---|---|---|
| XP/SP bookkeeping | Free — `SkillGroup` already does exp/sp/respec hashes | Re-implement pools, costs, refunds |
| Persistence | Reuse `skill_group` table; add string mappings in `json_models.rs` | New tables + loader + updater paths |
| Diary UI | Extends existing skill-tree window (`voxygen/src/hud/diary.rs`) | New window from scratch |
| XP routing | One insert in `handle_exp_gain` xp_pools | New award path |
| Ability gating | `AbilityKind::Simple(Option<Skill>, _)` works as-is | Needs a second unlock predicate |
| Risk | Touches a widely-used enum; every exhaustive `match` must be updated (compiler finds them) | Parallel system drifts from the real one |
| Cost | ~M | ~XL |

**Recommendation: Option 1.** Concretely:

- `common/src/comp/skillset/mod.rs:89` — add `Class(ClassKind)` variant to `SkillGroupKind`; give it its own SP cost curve in `skill_point_cost` (start with the weapon curve at `mod.rs:101`).
- `common/src/comp/skillset/skills.rs:13` — add `Skill::ClassSkill(ClassSkill)` with one sub-enum per class (e.g. `ClassSkill::Warrior(WarriorSkill)`), mirroring how `SwordSkill` et al. work. Skill *effects* are wired where existing skills are consumed (character states / ability params); v1 class skills are primarily ability unlocks + small passives.
- `assets/common/skill_trees/skills_skill-groups_manifest.ron` — add the four class groups and their skills; `skill_max_levels.ron` / `skill_prerequisites.ron` as needed. Hash-based respec protection comes for free.
- `server/src/persistence/json_models.rs:71,100` — add `Class(Warrior) ⇄ "Class Warrior"` etc. mappings (both functions panic on unknown strings today, so this is a hard requirement, not polish).
- `server/src/events/entity_manipulation.rs:505` `handle_exp_gain` — insert the character's `SkillGroupKind::Class(class)` into `xp_pools` when accessible, so class XP accrues alongside General + weapon XP.
- On creation, `SkillSet` gets `unlock_skill_group(SkillGroupKind::Class(class))` next to the existing General/Pick unlocks (done via the starting-kit builder, §5).

### 3. Class selection at character creation

| Layer | Change | File |
|---|---|---|
| UI | Class picker (4 buttons + description panel) next to the species picker; new `Message::Class(ClassKind)` in the iced UI state; emit class in `ui::Event::AddCharacter` | `voxygen/src/menu/char_selection/ui.rs`, `mod.rs:133` |
| i18n | Class names/descriptions | `assets/voxygen/i18n/en/char_selection.ftl` |
| Client API | `class: ClassKind` parameter | `client/src/lib.rs:1318` `create_character` |
| Net | `class: ClassKind` field on `ClientGeneral::CreateCharacter` | `common/net/src/msg/client.rs:77` |
| Server msg | Pass class through | `server/src/sys/msg/character_screen.rs:174` |
| Creation | Validate class, build class starting kit (loadout + skillset), attach `CharacterClass` to `PersistedComponents` | `server/src/character_creator.rs:30`, `server/src/persistence/mod.rs:35` |

Protocol note: this is a breaking change to `ClientGeneral`; acceptable on a private fork where
client and server ship together. The server rejects out-of-range/unknown class values with the
existing `CharacterActionError` path (like `CreationError::InvalidBody`).

The starter-weapon whitelist (`VALID_STARTER_ITEMS`, `character_creator.rs:11`) becomes per-class
(a `HashMap<ClassKind, &[[Option<&str>; 2]]>` or a RON manifest, §5), so a Mage cannot create
with a starter greatsword.

### 4. Data model and persistence migration

**Migration `server/src/migrations/V71__character_class.sql`:**

```sql
ALTER TABLE "character" ADD COLUMN class TEXT NOT NULL DEFAULT 'Adventurer';
```

- Pattern copied from `V62__hardcore.sql` (also an `ALTER TABLE "character" ADD COLUMN ... DEFAULT`).
- **Existing characters default to `Adventurer`** — they log in unchanged and keep all weapon
  trees. Spec B grants Adventurer no exemption: items that declare a class whitelist simply never
  list Adventurer, so legacy characters cannot equip class-exclusive gear until they pick a class
  (everything without a whitelist remains fully usable).
- One-time pick: new chat command `/select_class <class>` (registered in `server/src/cmd.rs`,
  enum in `common/src/cmd.rs`) that only succeeds while `class == Adventurer`. No UI flow needed
  for v1; the command is self-service, not admin-gated.
- Conversion code: `server/src/persistence/models.rs` `Character` gains `pub class: String`;
  `server/src/persistence/character/mod.rs` SELECT/INSERT statements add the column (INSERT at
  `:525`); string ⇄ enum conversion in `server/src/persistence/json_models.rs` next to the
  skill-group converters, returning `Adventurer` (with a warning log) instead of panicking on
  unknown strings, so a downgrade never bricks a DB.
- Class skill groups need **no schema change** — they persist through the existing `skill_group`
  table once the `json_models.rs` string mappings exist.

### 5. Class starting kits

Data-driven presets under `assets/common/class/`:

```
assets/common/class/
  starting_kits/warrior.ron   # loadout items + starter weapon whitelist
  starting_kits/mage.ron
  starting_kits/cleric.ron
  starting_kits/rogue.ron
  skillsets/warrior.ron       # SkillSetBuilder preset, e.g. [ Group(Class(Warrior)) ]
  ...
```

- Skillset presets reuse the exact format of `assets/common/skillset/preset/rank1/*.ron`
  (`Group(..)`, `Skill((.., lvl))`, `Tree("..")`), consumed by `common/src/skillset_builder.rs`.
- Loadouts built with `LoadoutBuilder` (`common/src/comp/inventory/loadout_builder.rs`), replacing
  the hardcoded `.defaults()` call in `character_creator.rs:53` for classed characters.
- Kit content v1: Warrior = starter sword/axe/hammer choice + extra armor; Mage = starter staff +
  minor energy potion; Cleric = starter sceptre + healing potions; Rogue = paired starter 1h
  swords + extra mobility consumable. All items already exist under `assets/common/items/`.

### 6. Racial traits

Data-driven manifest `assets/common/class/racial_traits.ron`, mapping `Species` → stat modifiers,
applied in `common/systems/src/buff.rs` immediately after `stat.reset_temp_modifiers()`
(`buff.rs:513`), reading the entity's `Body::Humanoid(body)` species. All values are deliberately
small (≤5%):

| Species | Trait (v1) | `Stats` field |
|---|---|---|
| Human | +3% energy reward | `energy_reward_modifier` |
| Dwarf | +2% damage reduction | `damage_reduction.pos_mod` |
| Elf | +3% move speed | `move_speed_modifier` |
| Orc | +3% attack damage | `attack_damage_modifier` |
| Danari | +5% max energy | `max_energy_modifiers.mult_mod` |
| Draugr | +10% crowd-control resistance | `crowd_control_resistance` |

Because traits are applied in the per-tick stat rebuild, they stack correctly with buffs, need no
persistence, and hot-reload with the asset in dev builds.

### 7. Multiclassing

**Out of scope for v1.** The data model does not preclude it: `SkillSet` already holds multiple
skill groups, so a future multiclass = unlocking a second `SkillGroupKind::Class(..)` plus a rule
for splitting XP in `handle_exp_gain`. The `CharacterClass` component would become
`CharacterClass { primary: ClassKind, secondary: Option<ClassKind> }` — a component/DB change, not
a redesign. Revisit after Level system (companion spec) lands, since multiclass gating wants
min-level rules.

## Interaction with abilities

- Class skills gate abilities through the existing `AbilityKind::Simple(Option<Skill>, _)` hook
  (`common/src/comp/inventory/item/tool.rs:320`): a class ability set entry in
  `assets/common/abilities/ability_set_manifest.ron` lists
  `Simple(Some(ClassSkill(Mage(Fireball))), "common.abilities.mage.fireball")`-style gates.
- New spell ability *content* (states, RON params) is specified in
  `2026-06-10-magic-abilities-design.md`; this spec only guarantees the unlock plumbing.
- Min-level gates on abilities are deferred to the Level companion spec
  (`2026-06-10-character-levels-design.md`); class trees alone gate v1 abilities.

## Interaction with equipment gating (Spec B)

`2026-06-10-equipment-restrictions-design.md` consumes `ClassKind` (class whitelists on
`ItemDef`), `Species` (race gates), and the companion Level. Ordering: Spec A Phase 1 must land
before Spec B Phase 1 (it needs `ClassKind` + `CharacterClass` synced to clients). Class starting
kits should declare requirements consistent with their own class so the kit never self-conflicts.

## Phases

### Phase 1 — Core class identity (M, ~5 dev-days)

| Task | Files | Size |
|---|---|---|
| `ClassKind` + `CharacterClass` comp + ECS registration + net sync | `common/src/comp/class.rs`, `common/src/comp/mod.rs`, `common/net/src/synced_components.rs` | S |
| Net message + client API + server handler | `common/net/src/msg/client.rs`, `client/src/lib.rs`, `server/src/sys/msg/character_screen.rs` | S |
| Creation validation + `PersistedComponents.class` | `server/src/character_creator.rs`, `server/src/persistence/mod.rs` | S |
| Migration V71 + models + character load/save + json_models converters | `server/src/migrations/V71__character_class.sql`, `server/src/persistence/{models.rs,character/mod.rs,json_models.rs}` | M |
| Char-creation class picker + i18n | `voxygen/src/menu/char_selection/ui.rs`, `assets/voxygen/i18n/en/char_selection.ftl` | M |
| `/select_class` command for legacy characters | `common/src/cmd.rs`, `server/src/cmd.rs` | S |

**Milestone:** create a Warrior, relog, class persists; legacy character loads as Adventurer and can `/select_class` once.

### Phase 2 — Class trees, kits, racial traits (L, ~8 dev-days)

| Task | Files | Size |
|---|---|---|
| `SkillGroupKind::Class(..)` + `Skill::ClassSkill(..)` + exhaustive-match fallout | `common/src/comp/skillset/{mod.rs,skills.rs}` + compiler-driven | M |
| Skill-tree manifests for 4 classes | `assets/common/skill_trees/*.ron` | M |
| DB string mappings for class groups | `server/src/persistence/json_models.rs` | S |
| XP routing to class pool | `server/src/events/entity_manipulation.rs:505` | S |
| Diary UI tab for class tree | `voxygen/src/hud/diary.rs`, `assets/voxygen/i18n/en/hud/skills.ftl` | L |
| Starting kits (loadout + skillset RON, per-class starter whitelist) | `assets/common/class/`, `server/src/character_creator.rs` | M |
| Racial traits manifest + application in buff tick | `assets/common/class/racial_traits.ron`, `common/systems/src/buff.rs` | S |

**Milestone:** Mage earns class XP on kills, spends SP in the Mage tree in the diary, and an Orc visibly hits harder than an identical Human.

### Phase 3 — Ability + equipment integration, balance (M, ~5 dev-days)

| Task | Files | Size |
|---|---|---|
| Class ability sets with skill gates | `assets/common/abilities/ability_set_manifest.ron` + ability RONs (per magic-abilities spec) | M |
| Spec B integration: class/race fields honored by item gates | per `2026-06-10-equipment-restrictions-design.md` | S (interface only) |
| Balance pass on trait values, SP curves, kit contents | assets only | M |
| i18n completion (class names in HUD, tooltips) | `assets/voxygen/i18n/en/` | S |

**Milestone:** a Rogue cannot use the Mage-gated blink ability, can use rogue-gated gear, and all four classes have a playable identity loop.

**Total estimate:** ~18 dev-days (one senior dev + AI assistance), XL overall.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| `SkillGroupKind` variant ripples through many exhaustive matches | Compile churn | All breakages are compile-time; do the enum change in one focused commit |
| `json_models.rs` converters panic on unknown strings | Server crash on bad data | Add class mappings in the same commit as the enum; class column converter falls back to Adventurer |
| Protocol break on `CreateCharacter` | Old clients can't create characters | Private fork; client+server ship together; bump network version constant |
| Upstream merges (gitlab/master) conflict with skillset/persistence edits | Merge pain | Keep class code in new files where possible; additive edits to shared files |
| Hash-based respec wipes class SP when tree RON changes | Player-visible respec | This is upstream's intended behavior; acceptable during fork ramp-up |

## Testing strategy

- **Unit (common):** `SkillGroupKind::Class` cost curve, unlock/refund round-trip, `Skill::ClassSkill` prerequisites — alongside existing skillset tests; run `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`.
- **Persistence:** extend converter tests in `server/src/persistence/json_models.rs` (existing test at `:345` shows the pattern) for class-group strings and class-column round-trip; migration smoke test = boot server against a copy of a pre-V71 DB and load a legacy character.
- **Integration:** create-character flow per class via a headless client (`bin_bot` feature) asserting the synced `CharacterClass` and starting kit contents.
- **Manual:** char-creation UI pass for all 6 species × 4 classes; diary tree spend/refund; `/select_class` happy path and double-pick rejection.
- **CI:** existing clippy/fmt commands from `CLAUDE.md` must stay green, including the `default-publish` voxygen check.

## Open questions

1. Should `Adventurer` be selectable at creation (a "classless veteran" option) or reserved for legacy characters only? Current design: reserved; the picker shows only the four classes.
2. Do racial traits need a UI surface (character sheet line) at launch, or is the changelog/wiki enough until the diary gets a "Traits" panel in a later pass?
3. Class XP share: equal split with General+weapons (current design) vs. weighted toward the class pool — needs playtest data after Phase 2.
4. When the Level spec lands, should class skill *tiers* additionally require min character level (D&D-style level gates inside the tree), or is SP cost alone enough pacing?
