# Classes and Races (Phase 1+2 core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Depends on:** character levels (merged — `SkillSet::character_level()` is at `common/src/comp/skillset/mod.rs:26-35`, already on `development`).
**Depended on by:** Phase B of `2026-06-10-equipment-restrictions-design.md` and the magic-abilities plan both consume `ClassKind` — they can start as soon as Task 1 lands.

**Goal:** Players pick one of four classes (Warrior, Mage, Cleric, Rogue) at character creation. The class is validated and persisted server-side, synced to all clients as a component, gets its own (stub) skill tree via `SkillGroupKind::Class(..)`, grants a class starting kit, and species grant small racial stat passives. Legacy characters load as `Adventurer` and pick once via `/set_class`.

**Architecture:** A new `CharacterClass` component (`common/src/comp/class.rs`) wraps a `ClassKind` enum whose `Adventurer` variant gives legacy characters defined semantics with zero `Option` plumbing. The class persists as a TEXT column on the `character` table (refinery migration `V71`, default `'Adventurer'`); class skill trees reuse the existing `SkillGroup` machinery by extending `SkillGroupKind`, so they persist through the existing `skill_group` table once the `json_models.rs` string converters know about them (both converters **panic on unknown kinds today** — extending them is a hard requirement, not polish). Racial traits are a data-driven RON manifest applied in the buff system's per-tick stat rebuild, immediately after `stat.reset_temp_modifiers()`, so they stack correctly with buffs and need no persistence.

**Tech Stack:** Rust nightly (2024 edition), specs ECS, rusqlite + refinery 0.9, iced char-selection UI. Design spec: `docs/superpowers/specs/2026-06-10-classes-races-design.md`.

**Conventions for every task:**
- Run tests with the assets path: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p <crate>`
- Branch: create `feature/classes-races` off `development` before Task 1.
- Invoke the `veloren-progression` skill for context and the `superpowers:test-driven-development` skill before writing code.
- Protocol note: Tasks 5 and 10 change `ClientGeneral`/persistence tuples. This is fine on a private fork (client+server ship together) but do **not** cherry-pick those tasks onto a branch that talks to old clients.

---

### Task 1: `ClassKind` enum + `CharacterClass` component

**Files:**
- Create: `common/src/comp/class.rs`
- Modify: `common/src/comp/mod.rs` (module list — `pub mod chat;` is at line 11, `pub mod combo;` at line 12; the `pub use self::{` block starts at line 44 with `chat::{..}` ending near line 70 and `combo::Combo` right after)

- [ ] **Step 1: Write the component with failing tests**

Create `common/src/comp/class.rs`:

```rust
//! Character class identity. See
//! docs/superpowers/specs/2026-06-10-classes-races-design.md.
use serde::{Deserialize, Serialize};
use specs::{Component, DerefFlaggedStorage, VecStorage};

#[derive(
    Clone, Copy, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd,
)]
pub enum ClassKind {
    /// Legacy/default class for pre-class characters. Not selectable at
    /// creation, has no class skill tree, and is never listed in item class
    /// whitelists (equipment-restrictions Spec B).
    #[default]
    Adventurer,
    Warrior,
    Mage,
    Cleric,
    Rogue,
}

impl ClassKind {
    /// Classes selectable at character creation (excludes Adventurer).
    pub const PLAYABLE: [ClassKind; 4] = [
        ClassKind::Warrior,
        ClassKind::Mage,
        ClassKind::Cleric,
        ClassKind::Rogue,
    ];

    pub fn is_playable(self) -> bool { !matches!(self, ClassKind::Adventurer) }

    /// Lowercase keyword used by chat commands and asset specifiers.
    pub fn keyword(self) -> &'static str {
        match self {
            ClassKind::Adventurer => "adventurer",
            ClassKind::Warrior => "warrior",
            ClassKind::Mage => "mage",
            ClassKind::Cleric => "cleric",
            ClassKind::Rogue => "rogue",
        }
    }

    /// Inverse of [`Self::keyword`] for the playable classes only.
    pub fn from_keyword(keyword: &str) -> Option<Self> {
        Self::PLAYABLE.iter().copied().find(|c| c.keyword() == keyword)
    }
}

/// The class a player character chose at creation (or via /set_class).
/// Synced to all clients; persisted in the `character` table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterClass(pub ClassKind);

impl Component for CharacterClass {
    type Storage = DerefFlaggedStorage<Self, VecStorage<Self>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_class_is_adventurer() {
        assert_eq!(ClassKind::default(), ClassKind::Adventurer);
        assert_eq!(CharacterClass::default().0, ClassKind::Adventurer);
    }

    #[test]
    fn keyword_round_trips_for_playable_classes() {
        for class in ClassKind::PLAYABLE {
            assert!(class.is_playable());
            assert_eq!(ClassKind::from_keyword(class.keyword()), Some(class));
        }
        // Adventurer is deliberately not re-pickable by keyword
        assert_eq!(ClassKind::from_keyword("adventurer"), None);
        assert_eq!(ClassKind::from_keyword("paladin"), None);
    }
}
```

- [ ] **Step 2: Export it**

In `common/src/comp/mod.rs`, add after `pub mod chat;` (line 11):

```rust
pub mod class;
```

and inside the `pub use self::{` block, directly after the `chat::{...}` entry:

```rust
    class::{CharacterClass, ClassKind},
```

- [ ] **Step 3: Run tests**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class -- --nocapture`
Expected: 2 tests PASS (`default_class_is_adventurer`, `keyword_round_trips_for_playable_classes`).

- [ ] **Step 4: Commit**

```bash
git add common/src/comp/class.rs common/src/comp/mod.rs
git commit -m "feat: ClassKind enum and CharacterClass component"
```

---

### Task 2: ECS registration + net sync

**Files:**
- Modify: `common/state/src/state.rs:242` (after `ecs.register::<comp::Hardcore>();`)
- Modify: `common/net/src/synced_components.rs:24` (x-macro list, after `hardcore: Hardcore,`) and `:131-133` (NetSync impls, after the `Hardcore` impl)

- [ ] **Step 1: Register in the shared ECS setup**

In `common/state/src/state.rs`, after line 242 (`ecs.register::<comp::Hardcore>();`):

```rust
        ecs.register::<comp::CharacterClass>();
```

- [ ] **Step 2: Add to the synced-components x-macro**

In `common/net/src/synced_components.rs`, in the `synced_components!` list after `hardcore: Hardcore,` (line 24):

```rust
            character_class: CharacterClass,
```

and after the `impl NetSync for Hardcore` block (line 131-133):

```rust
impl NetSync for CharacterClass {
    const SYNC_FROM: SyncFrom = SyncFrom::AnyEntity;
}
```

`CharacterClass` is re-exported into scope automatically by `reexport_comps` (it glob-imports `common::comp::*`). The server-side trackers in `server/src/sys/sentinel.rs:318` are generated from the same x-macro, so no server edit is needed.

- [ ] **Step 3: Verify build**

Run: `cargo check -p veloren-common-state -p veloren-common-net -p veloren-server -p veloren-client`
Expected: clean. If `sentinel.rs` errors mention `CharacterClass`, the x-macro entry name and type don't match the pattern `lowercase_name: TypeName,` — fix the entry, do not edit sentinel.rs.

- [ ] **Step 4: Commit**

```bash
git add common/state/src/state.rs common/net/src/synced_components.rs
git commit -m "feat: register and net-sync CharacterClass component"
```

---

### Task 3: `SkillGroupKind::Class(..)` + DB string converters (both directions)

**Files:**
- Modify: `common/src/comp/skillset/mod.rs:107-111` (enum), `:117-140` (`skill_point_cost`), `:1-12` (imports)
- Modify: `server/src/persistence/json_models.rs:71-98` (`skill_group_to_db_string`), `:100-117` (`db_string_to_skill_group`), tests mod at `:339`
- Modify (compiler-driven): `voxygen/src/hud/diary.rs` (~line 2937 match) and `voxygen/src/hud/skillbar.rs` (~line 854 match) — wherever `cargo check` reports non-exhaustive matches

- [ ] **Step 1: Write the failing round-trip test**

In `server/src/persistence/json_models.rs`, inside `pub mod tests` (line 339), add:

```rust
    #[test]
    fn skill_group_db_string_round_trips() {
        use common::comp::{class::ClassKind, item::tool::ToolKind, skillset::SkillGroupKind};
        let kinds = [
            SkillGroupKind::General,
            SkillGroupKind::Weapon(ToolKind::Sword),
            SkillGroupKind::Weapon(ToolKind::Axe),
            SkillGroupKind::Weapon(ToolKind::Hammer),
            SkillGroupKind::Weapon(ToolKind::Bow),
            SkillGroupKind::Weapon(ToolKind::Staff),
            SkillGroupKind::Weapon(ToolKind::Sceptre),
            SkillGroupKind::Weapon(ToolKind::Pick),
            SkillGroupKind::Class(ClassKind::Warrior),
            SkillGroupKind::Class(ClassKind::Mage),
            SkillGroupKind::Class(ClassKind::Cleric),
            SkillGroupKind::Class(ClassKind::Rogue),
        ];
        for kind in kinds {
            assert_eq!(
                super::db_string_to_skill_group(&super::skill_group_to_db_string(kind)),
                kind,
                "round trip failed for {kind:?}"
            );
        }
    }
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server skill_group_db_string`
Expected: FAIL to compile — `SkillGroupKind` has no variant `Class`.

- [ ] **Step 2: Add the enum variant and its SP cost curve**

In `common/src/comp/skillset/mod.rs`, change the imports at line 1-4 to include `ClassKind`:

```rust
use crate::{
    assets::{AssetExt, Ron},
    comp::{class::ClassKind, item::tool::ToolKind, skills::Skill},
};
```

Extend the enum at line 107-111:

```rust
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd)]
pub enum SkillGroupKind {
    General,
    Weapon(ToolKind),
    Class(ClassKind),
}
```

In `skill_point_cost` (line 117), add a `Class` arm before the `_ =>` arm, using the same pacing as the main weapon trees (spec §2):

```rust
            Self::Weapon(ToolKind::Sword | ToolKind::Axe | ToolKind::Hammer | ToolKind::Bow)
            | Self::Class(_) => {
                let level = level as f32;
                ((400.0 * (level / (level + 20.0)).powi(2) + 5.0 * E.powf(0.025 * level))
                    .min(u32::MAX as f32) as u32)
                    .saturating_mul(25)
            },
```

(This replaces the existing `Self::Weapon(...)` arm head; the body is unchanged.)

- [ ] **Step 3: Extend both converters with real arms**

In `server/src/persistence/json_models.rs`, `skill_group_to_db_string` (line 71): change the `use` line and add `Class` arms before the panicking `Weapon(ToolKind::Dagger) | ...` arm:

```rust
    use comp::{class::ClassKind, item::tool::ToolKind, skillset::SkillGroupKind::*};
```

```rust
        Class(ClassKind::Warrior) => "Class Warrior",
        Class(ClassKind::Mage) => "Class Mage",
        Class(ClassKind::Cleric) => "Class Cleric",
        Class(ClassKind::Rogue) => "Class Rogue",
        // Adventurer has no class tree; a Class(Adventurer) group reaching
        // persistence is a bug, consistent with the unsupported-weapon arm.
        Class(ClassKind::Adventurer) => panic!(
            "Tried to add unsupported skill group to database: {:?}",
            skill_group
        ),
```

In `db_string_to_skill_group` (line 100), same `use` change, then add before the `_ => panic!` arm:

```rust
        "Class Warrior" => Class(ClassKind::Warrior),
        "Class Mage" => Class(ClassKind::Mage),
        "Class Cleric" => Class(ClassKind::Cleric),
        "Class Rogue" => Class(ClassKind::Rogue),
```

- [ ] **Step 4: Compiler-driven exhaustive-match fixes**

Run: `cargo check --workspace --all-targets 2>&1 | grep -B2 "non-exhaustive\|SkillGroupKind"`

Known sites: `voxygen/src/hud/diary.rs` ~line 2937 (icon/name match on `SkillGroupKind`) and `voxygen/src/hud/skillbar.rs` ~line 854. For each, add a `SkillGroupKind::Class(_)` arm that reuses the `General` arm's value (real diary tab UI is a later phase). Do NOT add wildcard `_ =>` arms. Repeat until `cargo check --workspace --all-targets` is clean.

- [ ] **Step 5: Run tests to verify they pass**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server skill_group_db_string`
Expected: 1 test PASS.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common`
Expected: PASS (no regressions).

- [ ] **Step 6: Commit**

```bash
git add common/src/comp/skillset/mod.rs server/src/persistence/json_models.rs voxygen/src
git commit -m "feat: SkillGroupKind::Class variant with persistence string mappings"
```

---

### Task 4: Migration V71 + character-table class column + load/store plumbing

Latest migration verified: `server/src/migrations/V70__merge_remaining_unique_recipes.sql` — ours is **V71**. Re-verify before creating the file: `ls server/src/migrations | sort -V | tail -1` → expected `V70__merge_remaining_unique_recipes.sql`.

**Files:**
- Create: `server/src/migrations/V71__character_class.sql`
- Modify: `server/src/persistence/json_models.rs` (class string converters + test)
- Modify: `server/src/persistence/models.rs:1-8` (`Character` struct)
- Modify: `server/src/persistence/mod.rs:35-45` (`PersistedComponents`)
- Modify: `server/src/persistence/character/conversions.rs` (new converters near `convert_hardcore_from_database` at `:771`)
- Modify: `server/src/persistence/character/mod.rs:143-176` (load SELECT), `:326-348` (list SELECT), `:418-428` + `:523-540` (create destructure + INSERT)
- Modify: `server/src/character_creator.rs:77-87` (placeholder field)
- Modify: `server/src/state_ext.rs:696-706` (destructure) and `:745` area (component insert)

- [ ] **Step 1: Migration file (pattern copied from `V62__hardcore.sql`)**

Create `server/src/migrations/V71__character_class.sql`:

```sql
ALTER TABLE "character" ADD COLUMN class TEXT NOT NULL DEFAULT 'Adventurer';
```

- [ ] **Step 2: Write the failing converter test**

In `server/src/persistence/json_models.rs` tests mod:

```rust
    #[test]
    fn class_db_string_round_trips_and_tolerates_unknown() {
        use common::comp::class::ClassKind;
        for class in [
            ClassKind::Adventurer,
            ClassKind::Warrior,
            ClassKind::Mage,
            ClassKind::Cleric,
            ClassKind::Rogue,
        ] {
            assert_eq!(super::db_string_to_class(&super::class_to_db_string(class)), class);
        }
        // A downgrade/foreign DB must never brick the server (spec §4)
        assert_eq!(super::db_string_to_class("Necromancer"), ClassKind::Adventurer);
    }
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server class_db_string`
Expected: FAIL to compile — `db_string_to_class` not found.

- [ ] **Step 3: Implement the converters**

In `server/src/persistence/json_models.rs`, after `db_string_to_skill_group`:

```rust
pub fn class_to_db_string(class: comp::class::ClassKind) -> String {
    use comp::class::ClassKind::*;
    match class {
        Adventurer => "Adventurer",
        Warrior => "Warrior",
        Mage => "Mage",
        Cleric => "Cleric",
        Rogue => "Rogue",
    }
    .to_string()
}

/// Unlike the skill-group converter this never panics: unknown strings fall
/// back to Adventurer with a warning so a DB downgrade never bricks a save.
pub fn db_string_to_class(class_string: &str) -> comp::class::ClassKind {
    use comp::class::ClassKind::*;
    match class_string {
        "Adventurer" => Adventurer,
        "Warrior" => Warrior,
        "Mage" => Mage,
        "Cleric" => Cleric,
        "Rogue" => Rogue,
        unknown => {
            tracing::warn!(?unknown, "Unknown class in database, defaulting to Adventurer");
            Adventurer
        },
    }
}
```

(Add `use tracing;` only if not already importable — the crate already depends on tracing.)

In `server/src/persistence/character/conversions.rs`, next to `convert_hardcore_from_database` (line 771):

```rust
pub fn convert_class_from_database(class: &str) -> common::comp::CharacterClass {
    common::comp::CharacterClass(json_models::db_string_to_class(class))
}

pub fn convert_class_to_database(class: common::comp::CharacterClass) -> String {
    json_models::class_to_db_string(class.0)
}
```

- [ ] **Step 4: Model + PersistedComponents + creation placeholder**

`server/src/persistence/models.rs` — add to `Character`:

```rust
    pub class: String,
```

`server/src/persistence/mod.rs:35` — add to `PersistedComponents` after `hardcore`:

```rust
    pub character_class: comp::CharacterClass,
```

`server/src/character_creator.rs:77` — in the `PersistedComponents { .. }` literal, after `hardcore: ...`:

```rust
        character_class: common::comp::CharacterClass::default(),
```

(Temporary Adventurer placeholder; Task 5 wires the real class from the message.)

- [ ] **Step 5: Load plumbing (watch the row indices)**

`server/src/persistence/character/mod.rs:143-176` (`load_character_data`) — the SELECT gains `c.class` between `c.hardcore` and `b.variant`, shifting the body columns by one:

```rust
    let mut stmt = connection.prepare_cached(
        "
        SELECT  c.character_id,
                c.alias,
                c.waypoint,
                c.hardcore,
                c.class,
                b.variant,
                b.body_data
        FROM    character c
        JOIN    body b ON (c.character_id = b.body_id)
        WHERE   c.player_uuid = ?1
        AND     c.character_id = ?2",
    )?;
```

```rust
            let character_data = Character {
                character_id: row.get(0)?,
                player_uuid: requesting_player_uuid,
                alias: row.get(1)?,
                waypoint: row.get(2)?,
                hardcore: row.get(3)?,
                class: row.get(4)?,
            };

            let body_data = Body {
                body_id: row.get(0)?,
                variant: row.get(5)?,
                body_data: row.get(6)?,
            };
```

In the same function's `Ok((PersistedComponents { ... }` (line 292), after `hardcore,`:

```rust
            character_class: convert_class_from_database(&character_data.class),
```

(Import `convert_class_from_database` in the `use super::...conversions::{...}` list at line 17.)

`load_character_list` (line 326) — add `class` as the 5th selected column and `class: row.get(4)?,` to its `Character` construction (the list UI doesn't use it yet, but the struct now requires it).

- [ ] **Step 6: Store plumbing at creation**

`server/src/persistence/character/mod.rs:418` — add `character_class,` to the `PersistedComponents` destructure. Then the INSERT at line 523:

```rust
    let mut stmt = transaction.prepare_cached(
        "
        INSERT INTO character (character_id,
                               player_uuid,
                               alias,
                               waypoint,
                               hardcore,
                               class)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    stmt.execute([
        &character_id as &dyn ToSql,
        &uuid,
        &character_alias,
        &convert_waypoint_to_database_json(waypoint, map_marker),
        &convert_hardcore_to_database(hardcore),
        &convert_class_to_database(character_class),
    ])?;
```

- [ ] **Step 7: Insert the component on character load**

`server/src/state_ext.rs:696` — add `character_class,` to the `PersistedComponents` destructure, and after `self.write_component_ignore_entity_dead(entity, stats);` (line 745):

```rust
            self.write_component_ignore_entity_dead(entity, character_class);
```

- [ ] **Step 8: Verify**

Run: `cargo check --workspace --all-targets`
Expected: clean.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server`
Expected: PASS, including `class_db_string_round_trips_and_tolerates_unknown`.
Migration smoke test: copy an existing dev DB (`~/.local/share/veloren/saves/db.sqlite` or your fork's userdata path) aside, boot `cargo run --bin veloren-server-cli` once, expected log line: `applying migration` ... `V71__character_class`, then a clean startup. A legacy character must list and load.

- [ ] **Step 9: Commit**

```bash
git add server/src/migrations/V71__character_class.sql server/src/persistence server/src/character_creator.rs server/src/state_ext.rs
git commit -m "feat: persist character class (migration V71, default Adventurer)"
```

---

### Task 5: `CreateCharacter` message field + server validation

**Files:**
- Modify: `common/net/src/msg/client.rs:77-85` (message)
- Modify: `client/src/lib.rs:1325-1343` (`create_character`)
- Modify: `voxygen/src/menu/char_selection/ui.rs:146-153` (Event), `:180-206` (Mode fields), `:225-262` + `:264-293` (constructors), `:1850-1874` (emit)
- Modify: `voxygen/src/menu/char_selection/mod.rs:133-144` (event handling)
- Modify: `server/src/sys/msg/character_screen.rs:174-243` (handler)
- Modify: `server/src/character_creator.rs:24-90` + `:115-128` (validation + Display)

- [ ] **Step 1: Net message**

`common/net/src/msg/client.rs:77` — extend the variant:

```rust
    CreateCharacter {
        alias: String,
        mainhand: Option<String>,
        offhand: Option<String>,
        body: comp::Body,
        // Character will be deleted upon death if true
        hardcore: bool,
        start_site: Option<SiteId>,
        class: comp::class::ClassKind,
    },
```

(Breaking protocol change; acceptable per spec §3 — private fork, client and server ship together.)

- [ ] **Step 2: Client API**

`client/src/lib.rs:1325` — add the parameter and pass it through:

```rust
    pub fn create_character(
        &mut self,
        alias: String,
        mainhand: Option<String>,
        offhand: Option<String>,
        body: comp::Body,
        hardcore: bool,
        start_site: Option<SiteId>,
        class: comp::class::ClassKind,
    ) {
        self.character_list.loading = true;
        self.send_msg(ClientGeneral::CreateCharacter {
            alias,
            mainhand,
            offhand,
            body,
            hardcore,
            start_site,
            class,
        });
    }
```

- [ ] **Step 3: Voxygen plumbing (data only — the picker UI is Task 9)**

In `voxygen/src/menu/char_selection/ui.rs` add the import `use common::comp::class::ClassKind;` near the other `common::comp` imports, then:

1. `Event::AddCharacter` (line 146) gains `class: ClassKind,`.
2. `Mode::CreateOrEdit` (line 180) gains a `class: ClassKind,` field (place after `offhand`).
3. `Mode::create` (line 225) initializes `class: ClassKind::Warrior,`; `Mode::edit` (line 264) initializes `class: ClassKind::Adventurer,` (unused in edit mode — the server ignores class on edit).
4. The `Message::CreateCharacter` arm (line 1850) destructures `class` and emits it:

```rust
                if let Mode::CreateOrEdit {
                    name,
                    body,
                    hardcore_enabled,
                    mainhand,
                    offhand,
                    class,
                    start_site_idx,
                    ..
                } = &self.mode
                {
                    events.push(Event::AddCharacter {
                        alias: name.clone(),
                        mainhand: mainhand.map(String::from),
                        offhand: offhand.map(String::from),
                        body: comp::Body::Humanoid(*body),
                        hardcore: *hardcore_enabled,
                        class: *class,
                        start_site: self
                            .possible_starting_sites
                            .get(start_site_idx.unwrap_or_default())
                            .and_then(|info| info.site),
                    });
```

In `voxygen/src/menu/char_selection/mod.rs:133`, add `class` to the destructure and pass it as the last argument to `create_character(...)`.

- [ ] **Step 4: Server handler + validation**

`server/src/sys/msg/character_screen.rs:174` — add `class,` to the `ClientGeneral::CreateCharacter { ... }` destructure and pass it to `character_creator::create_character` after `body` (matching Step 5's new signature).

`server/src/character_creator.rs` — extend the error enum and signature:

```rust
#[derive(Debug)]
pub enum CreationError {
    InvalidWeapon,
    InvalidBody,
    InvalidClass,
}
```

```rust
pub fn create_character(
    entity: Entity,
    player_uuid: String,
    character_alias: String,
    character_mainhand: Option<String>,
    character_offhand: Option<String>,
    body: Body,
    character_class: ClassKind,
    hardcore: bool,
    character_updater: &mut WriteExpect<'_, CharacterUpdater>,
    waypoint: Option<Waypoint>,
) -> Result<(), CreationError> {
```

(add `class::ClassKind` and `CharacterClass` to the `common::comp` import list). After the body check, validate the class and replace the Task 4 placeholder:

```rust
    if !character_class.is_playable() {
        return Err(CreationError::InvalidClass);
    }
```

```rust
        character_class: CharacterClass(character_class),
```

And the `Display` impl gains:

```rust
            CreationError::InvalidClass => write!(
                f,
                "Invalid class.\nServer and client might be partially incompatible."
            ),
```

- [ ] **Step 5: Verify**

Run: `cargo check --workspace --all-targets`
Expected: clean (the compiler walks you to every remaining `CreateCharacter`/`create_character` call site — fix each by passing `class`, never by defaulting to a magic value on the server side).
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add common/net/src/msg/client.rs client/src/lib.rs voxygen/src/menu/char_selection server/src
git commit -m "feat: class field on CreateCharacter with server-side validation"
```

---

### Task 6: Class starting kits (loadout assets, starter whitelists, class skill group)

Note: the `SkillSetBuilder` preset route (`Group(..)` nodes, format verified in `assets/common/skillset/preset/rank1/sword.ron`) is **deliberately not used** for class groups: `SkillSetBuilder::with_skill` resolves groups via `SKILL_GROUP_LOOKUP`, which would require listing `UnlockGroup(Class(..))` inside the General tree — making every class tree purchasable by anyone with General SP. Class groups are unlocked directly in code instead.

**Files:**
- Modify: `common/src/comp/skillset/mod.rs:337` (`unlock_skill_group` visibility)
- Create: `assets/common/loadout/class/{warrior,mage,cleric,rogue}.ron`
- Modify: `server/src/character_creator.rs` (whitelist fn, loadout, kit items, tests)

- [ ] **Step 1: Write the failing tests**

At the end of `server/src/character_creator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use common::comp::class::ClassKind;

    #[test]
    fn every_class_has_starter_weapons_and_they_load() {
        for class in ClassKind::PLAYABLE {
            let kits = valid_starter_items(class);
            assert!(!kits.is_empty(), "{class:?} has no starter weapons");
            for pair in kits {
                for item in pair.iter().flatten() {
                    Item::new_from_asset_expect(item);
                }
            }
        }
    }

    #[test]
    fn class_loadouts_and_kit_items_load() {
        let mut rng = rand::rng();
        for class in ClassKind::PLAYABLE {
            LoadoutBuilder::empty().defaults().with_asset_expect(
                &format!("common.loadout.class.{}", class.keyword()),
                &mut rng,
                None,
            );
            Item::new_from_asset_expect(class_kit_item(class));
        }
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server character_creator`
Expected: FAIL to compile — `valid_starter_items` / `class_kit_item` not found.

- [ ] **Step 2: Per-class starter whitelist + kit item (replaces `VALID_STARTER_ITEMS`)**

In `server/src/character_creator.rs`, replace the `VALID_STARTER_ITEMS` const (lines 11-22) with:

```rust
/// Per-class starter weapon whitelist (spec §3/§5). `[None, None]` is always
/// accepted separately for unmodified clients.
fn valid_starter_items(class: ClassKind) -> &'static [[Option<&'static str>; 2]] {
    match class {
        ClassKind::Adventurer => &[],
        ClassKind::Warrior => &[
            [Some("common.items.weapons.sword.starter"), None],
            [Some("common.items.weapons.axe.starter_axe"), None],
            [Some("common.items.weapons.hammer.starter_hammer"), None],
        ],
        ClassKind::Mage => &[[Some("common.items.weapons.staff.starter_staff"), None]],
        ClassKind::Cleric => &[[Some("common.items.weapons.sceptre.starter_sceptre"), None]],
        ClassKind::Rogue => &[
            [
                Some("common.items.weapons.sword_1h.starter"),
                Some("common.items.weapons.sword_1h.starter"),
            ],
            [Some("common.items.weapons.bow.starter"), None],
        ],
    }
}

/// One flavorful consumable per class (all verified under
/// assets/common/items/consumable/).
fn class_kit_item(class: ClassKind) -> &'static str {
    match class {
        ClassKind::Adventurer | ClassKind::Warrior => "common.items.consumable.potion_minor",
        ClassKind::Mage | ClassKind::Rogue => "common.items.consumable.potion_agility",
        ClassKind::Cleric => "common.items.consumable.potion_med",
    }
}
```

In `create_character`, replace the old whitelist check with:

```rust
    if !(character_mainhand.is_none() && character_offhand.is_none())
        && !valid_starter_items(character_class)
            .contains(&[character_mainhand.as_deref(), character_offhand.as_deref()])
    {
        return Err(CreationError::InvalidWeapon);
    };
```

- [ ] **Step 3: Class loadout assets (format copied from `assets/common/loadout/default.ron`)**

Create `assets/common/loadout/class/warrior.ron`:

```ron
#![enable(implicit_some)]
(
    chest: Item("common.items.armor.misc.chest.worker_red_0"),
)
```

Then `mage.ron` with `worker_purple_0`, `cleric.ron` with `worker_yellow_0`, `rogue.ron` with `worker_green_0` (all verified under `assets/common/items/armor/misc/chest/`). The class loadout only overrides the chest; everything else comes from `.defaults()`.

- [ ] **Step 4: Apply the kit in `create_character`**

Make `unlock_skill_group` public in `common/src/comp/skillset/mod.rs:337` (`fn` → `pub fn`, doc: `/// Unlocks a skill group directly. Used by character creation and /set_class; players unlock weapon groups via Skill::UnlockGroup instead.`).

In `server/src/character_creator.rs`, replace the loadout/skillset block (lines 53-73):

```rust
    let mut rng = rand::rng();
    let loadout = LoadoutBuilder::empty()
        .defaults()
        .with_asset_expect(
            &format!("common.loadout.class.{}", character_class.keyword()),
            &mut rng,
            None,
        )
        .active_mainhand(character_mainhand.map(|x| Item::new_from_asset_expect(&x)))
        .active_offhand(character_offhand.map(|x| Item::new_from_asset_expect(&x)))
        .build();
    let mut inventory = Inventory::with_loadout_humanoid(loadout);

    let stats = Stats::new(Content::Plain(character_alias.to_string()), body);
    let mut skill_set = SkillSet::default();
    skill_set.unlock_skill_group(SkillGroupKind::Class(character_class));
```

(add `SkillGroupKind` to the `common::comp` imports; the existing potion/cheese/recipe pushes stay, plus one new push)

```rust
    inventory
        .push(Item::new_from_asset_expect(class_kit_item(character_class)))
        .expect("Inventory has at least 1 slot left!");
```

- [ ] **Step 5: Verify**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server character_creator`
Expected: 2 tests PASS.
Run: `cargo check --workspace --all-targets` — expected clean.

- [ ] **Step 6: Commit**

```bash
git add common/src/comp/skillset/mod.rs server/src/character_creator.rs assets/common/loadout/class
git commit -m "feat: per-class starting kits, starter whitelists and class skill group unlock"
```

---

### Task 7: Class skill-tree stubs in the manifest

The manifest at `assets/common/skill_trees/skills_skill-groups_manifest.ron` deserializes to `HashMap<SkillGroupKind, BTreeSet<Skill>>` and feeds `SKILL_GROUP_DEFS` and `SKILL_GROUP_HASHES` (`common/src/comp/skillset/mod.rs:42-105`). Adding an entry pins a stable hash for each class group, so later filling the trees triggers the intended hash-based respec instead of silently desyncing (`hash_val` falls back to `Vec::default()` today — see `conversions.rs:875-878`).

**Files:**
- Modify: `assets/common/skill_trees/skills_skill-groups_manifest.ron` (append entries)
- Modify: `common/src/comp/skillset/mod.rs` (test at end of file)

- [ ] **Step 1: Write the failing test**

At the end of `common/src/comp/skillset/mod.rs`:

```rust
#[cfg(test)]
mod class_tree_tests {
    use super::*;

    #[test]
    fn class_skill_groups_have_defs_and_stable_hashes() {
        for class in ClassKind::PLAYABLE {
            let group = SkillGroupKind::Class(class);
            assert!(SKILL_GROUP_DEFS.contains_key(&group), "missing manifest entry: {group:?}");
            assert!(SKILL_GROUP_HASHES.contains_key(&group), "missing hash: {group:?}");
            // v1 stub trees are empty: no purchasable skills yet
            assert_eq!(group.total_skill_point_cost(), 0);
        }
    }
}
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class_tree`
Expected: FAIL — missing manifest entries.

- [ ] **Step 2: Add manifest entries**

In `assets/common/skill_trees/skills_skill-groups_manifest.ron`, before the closing `}` add:

```ron
    Class(Warrior): [],
    Class(Mage): [],
    Class(Cleric): [],
    Class(Rogue): [],
```

Do NOT touch the `General` list — existing group hashes must not change (a `General` hash change would force a respec for every existing character).

- [ ] **Step 3: Verify**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common class_tree`
Expected: 1 test PASS.
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common` — expected PASS (in particular, no existing skillset test regressions from the new manifest keys).

- [ ] **Step 4: Commit**

```bash
git add assets/common/skill_trees/skills_skill-groups_manifest.ron common/src/comp/skillset/mod.rs
git commit -m "feat: stub class skill trees in the skill-group manifest"
```

---

### Task 8: Racial traits manifest + per-tick application

**Files:**
- Create: `assets/common/class/racial_traits.ron`
- Modify: `common/src/comp/class.rs` (trait struct + loader + tests)
- Modify: `common/systems/src/buff.rs:512-513` (apply after `reset_temp_modifiers`)

- [ ] **Step 1: Write the failing tests**

In `common/src/comp/class.rs` tests mod:

```rust
    #[test]
    fn racial_traits_manifest_loads_with_expected_values() {
        use crate::comp::body::humanoid::Species;
        // Spec §6 v1 values
        assert!(racial_traits(Species::Human).energy_reward_mult > 1.0);
        assert!(racial_traits(Species::Dwarf).damage_reduction_add > 0.0);
        assert!(racial_traits(Species::Elf).move_speed_mult > 1.0);
        assert!(racial_traits(Species::Orc).attack_damage_mult > 1.0);
        assert!(racial_traits(Species::Danari).max_energy_mult > 1.0);
        assert!(racial_traits(Species::Draugr).crowd_control_resistance_add > 0.0);
    }

    #[test]
    fn racial_traits_apply_to_stats() {
        use crate::comp::{Stats, body::humanoid::Species};
        let body = crate::comp::Body::Humanoid(crate::comp::humanoid::Body::random());
        let mut stats = Stats::empty(body);
        let before = stats.attack_damage_modifier;
        apply_racial_traits(&mut stats, Species::Orc);
        assert!(stats.attack_damage_modifier > before);
    }
```

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common racial_traits`
Expected: FAIL to compile — `racial_traits` not found.

- [ ] **Step 2: Implement loader + application in `class.rs`**

Add imports at the top of `common/src/comp/class.rs`:

```rust
use crate::{
    assets::{AssetExt, Ron},
    comp::{Stats, body::humanoid::Species},
};
use hashbrown::HashMap;
```

and below the component:

```rust
/// Per-species passive stat modifiers (spec §6). All values small (≤10%).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct RacialTraits {
    pub move_speed_mult: f32,
    pub attack_damage_mult: f32,
    pub energy_reward_mult: f32,
    pub max_energy_mult: f32,
    pub damage_reduction_add: f32,
    pub crowd_control_resistance_add: f32,
}

impl Default for RacialTraits {
    fn default() -> Self {
        Self {
            move_speed_mult: 1.0,
            attack_damage_mult: 1.0,
            energy_reward_mult: 1.0,
            max_energy_mult: 1.0,
            damage_reduction_add: 0.0,
            crowd_control_resistance_add: 0.0,
        }
    }
}

/// Loads the trait block for a species. Goes through the asset cache each
/// call, so it hot-reloads in dev builds; unknown species are neutral.
pub fn racial_traits(species: Species) -> RacialTraits {
    Ron::<HashMap<Species, RacialTraits>>::load_expect("common.class.racial_traits")
        .read()
        .0
        .get(&species)
        .copied()
        .unwrap_or_default()
}

/// Applies racial passives onto freshly-reset stats. Must run right after
/// `Stats::reset_temp_modifiers` so traits stack with buffs (spec §6).
pub fn apply_racial_traits(stats: &mut Stats, species: Species) {
    let traits = racial_traits(species);
    stats.move_speed_modifier *= traits.move_speed_mult;
    stats.attack_damage_modifier *= traits.attack_damage_mult;
    stats.energy_reward_modifier *= traits.energy_reward_mult;
    stats.max_energy_modifiers.mult_mod *= traits.max_energy_mult;
    stats.damage_reduction.pos_mod += traits.damage_reduction_add;
    stats.crowd_control_resistance += traits.crowd_control_resistance_add;
}
```

(All six `Stats` fields verified at `common/src/comp/stats.rs:73-140`; multiplicative fields default to `1.0`, additive to `0.0`.)

- [ ] **Step 3: Manifest**

Create `assets/common/class/racial_traits.ron`:

```ron
// Racial passives (classes-races spec §6). Partial structs allowed: missing
// fields are neutral via serde defaults. Hot-reloads in dev builds.
{
    Human: (energy_reward_mult: 1.03),
    Dwarf: (damage_reduction_add: 0.02),
    Elf: (move_speed_mult: 1.03),
    Orc: (attack_damage_mult: 1.03),
    Danari: (max_energy_mult: 1.05),
    Draugr: (crowd_control_resistance_add: 0.10),
}
```

- [ ] **Step 4: Apply in the buff tick**

In `common/systems/src/buff.rs`, directly after `stat.reset_temp_modifiers();` (line 513):

```rust
            // Racial passives re-apply each tick right after the reset so
            // they stack with buffs and need no persistence (spec §6).
            if let Body::Humanoid(humanoid_body) = *body {
                common::comp::class::apply_racial_traits(&mut stat, humanoid_body.species);
            }
```

(`body: &Body` is already bound in this loop — see the tuple destructure at `buff.rs:163`.)

- [ ] **Step 5: Verify**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common racial_traits`
Expected: 2 tests PASS.
Run: `cargo check -p veloren-common-systems` — expected clean.

- [ ] **Step 6: Commit**

```bash
git add common/src/comp/class.rs common/systems/src/buff.rs assets/common/class/racial_traits.ron
git commit -m "feat: data-driven racial trait passives applied in the stat rebuild"
```

---

### Task 9: Class picker in character creation (UI)

The data plumbing already exists (Task 5: `Mode::CreateOrEdit.class`, `Event::AddCharacter.class`). This task adds the selector. Layout code is compiler-driven; the state changes below are exact.

**Files:**
- Modify: `voxygen/src/menu/char_selection/ui.rs` (Message enum at `:326`, constants at `:59-64`, `Mode::CreateOrEdit` buttons, update fn at `:1896` area, layout near the species buttons at `:1066-1121`)
- Modify: `assets/voxygen/i18n/en/char_selection.ftl`

- [ ] **Step 1: State + message handling (exact code)**

In `ui.rs` add next to the starter constants (line 59-64):

```rust
const STARTER_SCEPTRE: &str = "common.items.weapons.sceptre.starter_sceptre";

/// Default starter weapon shown when a class is picked; must be a member of
/// the server-side whitelist in server/src/character_creator.rs.
fn default_starter_for_class(class: ClassKind) -> (Option<&'static str>, Option<&'static str>) {
    match class {
        ClassKind::Adventurer | ClassKind::Warrior => (Some(STARTER_SWORD), None),
        ClassKind::Mage => (Some(STARTER_STAFF), None),
        ClassKind::Cleric => (Some(STARTER_SCEPTRE), None),
        ClassKind::Rogue => (Some(STARTER_SWORDS), Some(STARTER_SWORDS)),
    }
}
```

Add to `enum Message` (line 326), after `Species(humanoid::Species),`:

```rust
    Class(ClassKind),
```

Add `class_buttons: [button::State; 4],` to `Mode::CreateOrEdit` (line 187 area) and initialize it with `Default::default()` in both `Mode::create` and `Mode::edit`.

Add the update arm next to `Message::Species` (line 1896):

```rust
            Message::Class(value) => {
                if let Mode::CreateOrEdit {
                    class,
                    mainhand,
                    offhand,
                    inventory,
                    ..
                } = &mut self.mode
                {
                    *class = value;
                    let (new_mainhand, new_offhand) = default_starter_for_class(value);
                    *mainhand = new_mainhand;
                    *offhand = new_offhand;
                    // Rebuild the preview loadout exactly like Message::Tool does
                    // (copy the inventory-update body from the Tool arm at ~:1902).
                }
            },
```

- [ ] **Step 2: i18n**

Append to `assets/voxygen/i18n/en/char_selection.ftl`:

```ftl
char_selection-class = Class
char_selection-class_warrior = Warrior
char_selection-class_mage = Mage
char_selection-class_cleric = Cleric
char_selection-class_rogue = Rogue
```

- [ ] **Step 3: Layout (compiler-driven)**

Mirror the species-button block (`ui.rs:1066-1121`: a row of `Button::new(state, ...)` with `.on_press(Message::Species(..))` and selected-state styling) to render four class buttons bound to `Message::Class(ClassKind::Warrior)` etc., with a `char_selection-class` section title above, placed between the species section and the weapon section. Reuse the same button styling; selected state compares `*class == ClassKind::X`. Also gate each existing tool button (`:1143-1187`) so only weapons in the current class's default/whitelist render enabled (Warrior: sword/axe/hammer; Mage: staff; Cleric: sceptre; Rogue: paired swords/bow) — the server rejects mismatches, the UI just prevents them.

Run after each iteration: `cargo check -p veloren-voxygen`
Expected: clean before moving on.

- [ ] **Step 4: Visual verification**

Use the `veloren-run` skill to launch server + client. Verify:
- Creation screen shows the four class buttons; picking Cleric switches the preview weapon to the sceptre.
- Creating a Mage works; relog: the character loads (class persisted — check server log or DB: `SELECT alias, class FROM character;` shows `Mage`).
- Creating with a mismatched weapon is impossible from the UI.

- [ ] **Step 5: Commit**

```bash
git add voxygen/src/menu/char_selection/ui.rs assets/voxygen/i18n/en/char_selection.ftl
git commit -m "feat: class picker in character creation UI"
```

---

### Task 10: Persist class changes on autosave

Creation writes the class (Task 4), but the periodic/logout update path (`server/src/persistence/character/mod.rs:1064` `update()`) never touches the column. `/set_class` (Task 11) needs it to.

**Files:**
- Modify: `server/src/persistence/character_updater.rs:24-32` (tuple), `:426-442` (destructure)
- Modify: `server/src/sys/persistence.rs` (SystemData, join, tuple)
- Modify: `server/src/events/player.rs:403-411` (logout tuple)
- Modify: `server/src/persistence/character/mod.rs:1064-1073` (signature) and `:1206-1224` (UPDATE)

- [ ] **Step 1: Extend the update tuple**

`server/src/persistence/character_updater.rs:24` — append to `CharacterUpdateData`:

```rust
pub type CharacterUpdateData = (
    CharacterId,
    comp::SkillSet,
    comp::Inventory,
    Vec<PetPersistenceData>,
    Option<comp::Waypoint>,
    comp::ability::ActiveAbilities,
    Option<comp::MapMarker>,
    comp::CharacterClass,
);
```

In `execute_batch_update` (line 426), add `character_class,` as the last destructured element and pass it to `super::character::update(...)` as the last argument before `&mut transaction`.

- [ ] **Step 2: Gather it in the persistence system**

`server/src/sys/persistence.rs` — add `ReadStorage<'a, CharacterClass>` to `SystemData` (and `CharacterClass` to the `common::comp` import), bind it as `character_classes` in `run`, add `character_classes.maybe()` as the last element of the join tuple (line 57-66), bind `character_class` in the closure params, and extend the `Some((...))` tuple (line 102-110):

```rust
                                Some((
                                    id,
                                    skill_set.clone(),
                                    inventory.clone(),
                                    pets,
                                    waypoint.cloned(),
                                    active_abilities.clone(),
                                    map_marker.cloned(),
                                    character_class.copied().unwrap_or_default(),
                                ))
```

- [ ] **Step 3: Same for the logout path**

`server/src/events/player.rs` — before the `add_pending_logout_update` call (line 403):

```rust
                    let character_class = state
                        .ecs()
                        .read_storage::<comp::CharacterClass>()
                        .get(entity)
                        .copied()
                        .unwrap_or_default();
```

and append `character_class,` to the tuple passed in.

- [ ] **Step 4: Write it in `update()`**

`server/src/persistence/character/mod.rs:1064` — add the parameter `character_class: comp::CharacterClass,` after `map_marker`, then replace the waypoint UPDATE (line 1208-1216) with:

```rust
    let mut stmt = transaction.prepare_cached(
        "
        UPDATE  character
        SET     waypoint = ?1,
                class = ?2
        WHERE   character_id = ?3
    ",
    )?;

    let waypoint_count = stmt.execute([
        &db_waypoint as &dyn ToSql,
        &convert_class_to_database(character_class),
        &char_id.0,
    ])?;
```

- [ ] **Step 5: Verify**

Run: `cargo check --workspace --all-targets`
Expected: clean (the compiler finds any remaining tuple-arity mismatch).
Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-server`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add server/src/persistence server/src/sys/persistence.rs server/src/events/player.rs
git commit -m "feat: persist character class on autosave and logout"
```

---

### Task 11: `/set_class` for legacy characters

Pattern copied from the `BattleMode` command pair (`common/src/cmd.rs:549-557` data, `:1188` keyword, `server/src/cmd.rs:156` registration, `:5573` handler).

**Files:**
- Modify: `common/src/cmd.rs` (enum variant near `SetMotd` at the `S` block, `data()` entry, `keyword()` entry)
- Modify: `server/src/cmd.rs` (registration in the dispatch match at `:146` area + handler)
- Modify: `assets/voxygen/i18n/en/command.ftl`

- [ ] **Step 1: Command spec**

In `common/src/cmd.rs`, add the variant `SetClass,` to `ServerChatCommand` (alphabetically next to `SetBodyType`/`SetMotd`). In `data()`:

```rust
            ServerChatCommand::SetClass => cmd(
                vec![Enum(
                    "class",
                    vec![
                        "warrior".to_owned(),
                        "mage".to_owned(),
                        "cleric".to_owned(),
                        "rogue".to_owned(),
                    ],
                    Required,
                )],
                Content::localized("command-set_class-desc"),
                None,
            ),
```

In `keyword()`:

```rust
            ServerChatCommand::SetClass => "set_class",
```

In `assets/voxygen/i18n/en/command.ftl`, next to the battlemode entries:

```ftl
command-set_class-desc = One-time class pick for legacy characters: warrior, mage, cleric or rogue
```

- [ ] **Step 2: Server handler**

In `server/src/cmd.rs`, register in the dispatch match (alphabetical position):

```rust
        ServerChatCommand::SetClass => handle_set_class,
```

and add the handler (near `handle_battlemode`):

```rust
fn handle_set_class(
    server: &mut Server,
    client: EcsEntity,
    target: EcsEntity,
    args: Vec<String>,
    action: &ServerChatCommand,
) -> CmdResult<()> {
    use common::comp::class::{CharacterClass, ClassKind};

    let class_arg = parse_cmd_args!(args, String).ok_or_else(|| action.help_content())?;
    let class = ClassKind::from_keyword(class_arg.to_lowercase().as_str()).ok_or_else(|| {
        Content::Plain(format!(
            "Unknown class '{class_arg}'. Options: warrior, mage, cleric, rogue."
        ))
    })?;

    {
        let mut classes = server.state.ecs().write_storage::<CharacterClass>();
        let current = classes.get(target).copied().unwrap_or_default();
        if current.0 != ClassKind::Adventurer {
            return Err(Content::Plain(format!(
                "Class is already {:?}; /set_class is a one-time pick for legacy characters.",
                current.0
            )));
        }
        let _ = classes.insert(target, CharacterClass(class));
    }

    // Unlock the class skill tree on the live skill set; both the component
    // and the skill group persist via the autosave path (Task 10).
    if let Some(skill_set) = server
        .state
        .ecs()
        .write_storage::<comp::SkillSet>()
        .get_mut(target)
    {
        skill_set.unlock_skill_group(common::comp::skillset::SkillGroupKind::Class(class));
    }

    server.notify_client(
        client,
        ServerGeneral::server_msg(
            ChatType::CommandInfo,
            Content::Plain(format!("Class set to {class:?}.")),
        ),
    );
    Ok(())
}
```

- [ ] **Step 3: Verify**

Run: `cargo check --workspace --all-targets` — expected clean.
Manual: use the `veloren-run` skill; on a pre-V71 character (loads as Adventurer): `/set_class mage` → "Class set to Mage."; `/set_class rogue` again → rejection message. Relog: class survives (autosave path), and `SELECT alias, class FROM character;` shows `Mage`.

- [ ] **Step 4: Commit**

```bash
git add common/src/cmd.rs server/src/cmd.rs assets/voxygen/i18n/en/command.ftl
git commit -m "feat: /set_class one-time class pick for legacy characters"
```

---

### Task 12: Lint, format, changelog, branch finish

- [ ] **Step 1: CI-identical lint**

```bash
cargo clippy --all-targets --locked \
  --features="bin_cmd_doc_gen,bin_compression,bin_csv,bin_graphviz,bin_bot,bin_asset_migrate,asset_tweak,bin,stat,cli" \
  -- -D warnings
```
Expected: clean. Fix any warnings (no bare `#[allow]` without a justifying comment).

- [ ] **Step 2: Voxygen publish-profile clippy**

```bash
cargo clippy -p veloren-voxygen --locked --no-default-features --features="default-publish" -- -D warnings
```
Expected: clean (the char-selection UI changes must not depend on hot-reloading-only code).

- [ ] **Step 3: Format**

Run: `cargo fmt --all -- --check` — if it fails, run `cargo fmt --all` and re-check.

- [ ] **Step 4: Full test suite**

Run: `VELOREN_ASSETS="$(pwd)/assets" cargo test -p veloren-common -p veloren-server -p veloren-common-net`
Expected: PASS.

- [ ] **Step 5: Changelog + commit**

Add under the unreleased section of `CHANGELOG.md`:

```markdown
- Characters now choose a class (Warrior, Mage, Cleric or Rogue) at creation, with class starting kits and a class skill pool. Legacy characters load as Adventurer and pick once via /set_class.
- Species now grant small racial passives (e.g. Orc +3% damage, Elf +3% move speed).
```

```bash
git add CHANGELOG.md
git commit -m "docs: changelog entry for classes and races"
```

- [ ] **Step 6: Finish the branch**

Invoke `superpowers:finishing-a-development-branch` (and `veloren-review` before merging into `development`). Phase 2/3 follow-ups (class skill content, diary tab, ability gating, equipment Spec B integration) are tracked in the design spec's phase table.
