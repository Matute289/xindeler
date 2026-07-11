//! `EntityTemplate` — the generic, data-driven entity factory (BL-82 EM-4.7,
//! spec §1.10 / migration spec §5.5).
//!
//! ## Approach (spec §5.5, already locked)
//! `EntityTemplate` RON/JSON assets (`entity_template_id` → a small, fixed set
//! of "component kinds": `body`/`stats`/`faction`/`loot`/
//! `ai_behavior_override`) spawn a matching fixed set of `Pending*` descriptor
//! components via [`spawn_entity_template`]. Bevy 0.19's BSN would be the
//! native answer but its asset loader hasn't shipped (0.19 facts doc) — this
//! is the small template loader the spec asks us to build now, swapped for
//! `.bsn` when it lands.
//!
//! ## Why this is a direct function, not a runtime registry (BL-82 EM-4.10 T48.7)
//! An earlier version of this module routed every kind through a
//! `ComponentSpawnRegistry` (`HashMap<String, fn(&mut EntityCommands,
//! &ron::Value)>`), framed as letting "a future consumer add a brand-new
//! component kind without touching `with_builtins`." That framing didn't
//! hold up: `EntityTemplate::component_values` (the registry's ONLY
//! producer, since removed) always emitted the exact same fixed five kinds
//! straight off
//! this struct's own fields — a genuinely NEW kind is unreachable without
//! first adding a field to [`EntityTemplate`] in Rust anyway, at which point
//! adding a `match` arm here is exactly as easy as registering a closure, and
//! the `HashMap` lookup + RON `Value` round-trip per field bought nothing.
//! Authoring a NEW template (a new `.entity_template.ron` file with a
//! different id, reusing the five existing kinds) still requires **zero
//! Rust changes** — the three sample templates in `assets/xindeler/
//! entity_templates/` (a stalking wolf, a fleeing deer, a stalking owl) prove
//! that with the collapsed direct function just as well as the registry did.
//! A real runtime registry is worth reintroducing only once a SECOND,
//! differently-shaped producer needs to register kinds at runtime — not
//! before.
//!
//! ## Two-crate split (isolation law)
//! This crate never embeds a `specs::World` (rule 4: bridge/shell code only
//! writes into the sim through its PUBLIC APIs). So [`spawn_entity_template`]
//! only spawns a lightweight, transient BEVY entity carrying descriptor
//! components ([`PendingBody`]/[`PendingStats`]/[`PendingFaction`]/
//! [`PendingLoot`]/[`PendingAiBehavior`] + [`PendingEntityTemplateSpawn`]).
//! `xindeler-sim-bridge` (which DOES own the `SimServer`) reads those
//! descriptors and does the actual sim-side spawn through the exact same
//! public event bus (`NpcBuilder` + `CreateNpcEvent` +
//! `State::emit_event_now`) its own `spawn_test_npcs` already uses — see that
//! module's `entity_factory.rs` for the other half. Once spawned there, the
//! EXISTING `mirror_sim_entities` system mirrors the new NPC into
//! `NetBody`/`NetUid`/… with ZERO new client code, and `xindeler-client`'s
//! EM-3.8 figure pipeline assembles its real `.vox` model — "verbatim reuse"
//! is a structural consequence of going through the same public API real
//! test NPCs already use, not a new parallel path.
//!
//! [`AgentPreset`] is the `BehaviorRegistry` half of the checklist: it tunes
//! the SAME `Psyche` fields `/spawn`/`/make_npc` already tune by hand
//! (`server/src/cmd.rs`) — no new behavior-tree interpreter (that's an
//! explicit v2, deferred). An unknown/malformed `ai_behavior_override`
//! string resolves to [`AgentPreset::Passive`] (anti-chaos, same posture as
//! `DmEvent::sanitize` — this module reuses that exact closed set,
//! [`crate::dm_event::bounds::KNOWN_AI_BEHAVIORS`]/`DEFAULT_AI_BEHAVIOR`,
//! rather than re-declaring a second one that could drift).

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    prelude::*,
    reflect::TypePath,
};
use serde::{Deserialize, Serialize};
use xindeler_protocol::DimensionId;

use crate::dm_event::bounds as dm_bounds;

/// One entity-factory template: `entity_template_id` names it; the other
/// fields are the fixed set of "component kinds" [`spawn_entity_template`]
/// knows how to turn into descriptor components. `#[serde(default)]`
/// throughout so partial files keep loading as the schema grows, exactly
/// like [`crate::dm_event::DmEvent`]/[`crate::atmosphere::AtmosphereProfile`].
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EntityTemplate {
    /// Human-readable id for logging/debugging; not used for dispatch (see
    /// the module doc's "why this isn't a switch statement").
    pub entity_template_id: String,
    /// An existing Veloren NPC body keyword (e.g. `"wolf"`, `"pig"`,
    /// `"humanoid"`) — the SAME string vocabulary the in-game `/spawn`
    /// admin command already parses via `common::npc::NpcBody`'s
    /// `FromStr` (`server/src/cmd.rs`'s `handle_spawn`). Never a new,
    /// parallel body-naming scheme.
    pub body: String,
    /// Minimal display stats (v1 deliberately does not expose
    /// balance-affecting numeric fields here — that is game-content tuning,
    /// out of this plumbing task's scope).
    pub stats: EntityTemplateStats,
    /// One of [`bounds::KNOWN_FACTIONS`] (`common::comp::Alignment`'s
    /// spawnable variants, lowercased); unknown falls back to
    /// [`bounds::DEFAULT_FACTION`].
    pub faction: String,
    /// An existing Veloren item/loot-table asset specifier (frozen asset
    /// names, per constraint #2), or `None` for no loot.
    pub loot: Option<String>,
    /// Resolves through [`AgentPreset::resolve`] — one of
    /// [`crate::dm_event::bounds::KNOWN_AI_BEHAVIORS`]; unknown/malformed
    /// falls back to [`AgentPreset::Passive`] rather than panicking.
    pub ai_behavior_override: String,
}

impl Default for EntityTemplate {
    fn default() -> Self {
        Self {
            entity_template_id: String::new(),
            body: "pig".to_owned(),
            stats: EntityTemplateStats::default(),
            faction: bounds::DEFAULT_FACTION.to_owned(),
            loot: None,
            ai_behavior_override: dm_bounds::DEFAULT_AI_BEHAVIOR.to_owned(),
        }
    }
}

/// Minimal display stats a template can specify (the `"stats"` component
/// kind). Deliberately thin — see [`EntityTemplate::stats`]'s doc comment.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EntityTemplateStats {
    /// Display name; `None` lets the sim-side spawn fall back to the body's
    /// own default name (`common::npc` name tables), exactly like
    /// `/make_npc`'s own `get_npc_name` fallback.
    pub name: Option<String>,
}

/// Clamp bounds enforced by [`EntityTemplate::sanitize`] (anti-chaos, same
/// threat model as [`crate::dm_event::bounds`]: template files are content
/// authored by ORACLE's LLM-side tooling as well as by hand, so hostile/buggy
/// values are in scope). Numeric-string-length bounds are reused directly
/// from `dm_event::bounds` (`MAX_STRING_LEN`) rather than re-declared here.
pub mod bounds {
    /// `EntityTemplate::faction` allowlist — the spawnable
    /// `common::comp::Alignment` variants, lowercased (`Owned(Uid)` is
    /// excluded: it needs a runtime owner entity, not expressible in a
    /// static template).
    pub const KNOWN_FACTIONS: &[&str] = &["wild", "enemy", "npc", "tame", "passive"];
    /// Safe fallback for an unrecognized `faction` string.
    pub const DEFAULT_FACTION: &str = "wild";
}

impl EntityTemplate {
    /// Anti-chaos clamps (mirrors [`crate::dm_event::DmEvent::sanitize`]'s
    /// shape exactly): truncates every free-form string, and validates
    /// `faction`/`ai_behavior_override` against their closed allowlists,
    /// falling back to a safe default rather than ever propagating a
    /// hostile value downstream. Idempotent. Runs on every ingestion path
    /// (today: [`EntityTemplateLoader::load`]).
    pub fn sanitize(&mut self) {
        crate::dm_event::truncate_to(&mut self.entity_template_id, dm_bounds::MAX_STRING_LEN);
        crate::dm_event::truncate_to(&mut self.body, dm_bounds::MAX_STRING_LEN);
        if let Some(name) = &mut self.stats.name {
            crate::dm_event::truncate_to(name, dm_bounds::MAX_STRING_LEN);
        }
        if !bounds::KNOWN_FACTIONS.contains(&self.faction.as_str()) {
            self.faction = bounds::DEFAULT_FACTION.to_owned();
        }
        if let Some(loot) = &mut self.loot {
            crate::dm_event::truncate_to(loot, dm_bounds::MAX_STRING_LEN);
        }
        if !dm_bounds::KNOWN_AI_BEHAVIORS.contains(&self.ai_behavior_override.as_str()) {
            self.ai_behavior_override = dm_bounds::DEFAULT_AI_BEHAVIOR.to_owned();
        }
    }
}

// ---------------------------------------------------------------------------
// Descriptor components (the Bevy-side half of a pending factory spawn)
// ---------------------------------------------------------------------------

/// Attached by the `"body"` spawn closure. Carries the SAME NPC body keyword
/// string the template declared — `xindeler-sim-bridge`'s adapter resolves it
/// via `common::npc::NpcBody`'s `FromStr` (the existing `/spawn` mechanism).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct PendingBody(pub String);

/// Attached by the `"stats"` spawn closure.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingStats {
    pub name: Option<String>,
}

/// Attached by the `"faction"` spawn closure. Already-sanitized against
/// [`bounds::KNOWN_FACTIONS`] by the time it reaches here (assuming the
/// template came through [`EntityTemplateLoader`]/[`EntityTemplate::sanitize`]
/// — the adapter falls back defensively regardless, per EM-4.4's "defuse,
/// don't crash" posture, in case a caller constructs a template in-process).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct PendingFaction(pub String);

/// Attached by the `"loot"` spawn closure (only if the template's `loot` is
/// `Some`).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct PendingLoot(pub String);

/// Attached by the `"ai_behavior_override"` spawn closure. Resolved to an
/// [`AgentPreset`] downstream, not here (needs the resolved `Body` from
/// [`PendingBody`] first — see [`AgentPreset::build_agent`]).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct PendingAiBehavior(pub String);

/// Marks a Bevy entity as a not-yet-materialized template spawn request:
/// [`spawn_entity_template`] always attaches this alongside whatever
/// descriptor components the registry produced, so a consumer can query for
/// `With<PendingEntityTemplateSpawn>` regardless of which kinds a given
/// template happened to use.
///
/// `dimension` is carried for forward-compat with EM-4.5/4.9 (spawning a
/// DmEvent's monsters into ITS instanced dimension) but v1 only supports
/// [`DimensionId::DEFAULT`] — a consumer must reject anything else rather
/// than silently mis-spawning into the wrong world (see
/// `xindeler-sim-bridge::entity_factory`'s doc comment).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PendingEntityTemplateSpawn {
    /// Sim/world position (Veloren axes: x-east, y-north, z-up) — same
    /// convention as `xindeler_protocol::TerrainAnchor::wpos`.
    pub pos: [f32; 3],
    pub dimension: DimensionId,
}

/// Spawns a transient Bevy "pending spawn request" entity for `template` at
/// `pos`/`dimension`: attaches [`PendingEntityTemplateSpawn`] plus the
/// [`PendingBody`]/[`PendingStats`]/[`PendingFaction`]/[`PendingAiBehavior`]
/// descriptor components straight off `template`'s own (already-typed)
/// fields — [`PendingLoot`] only when `template.loot` is `Some`. BL-82
/// EM-4.10 T48.7: this used to dispatch through a `ComponentSpawnRegistry`
/// (a runtime `HashMap<String, fn(...)>`) plus a RON `Value` round-trip per
/// field; collapsed to a direct sequence of inserts since every field here is
/// already a concrete typed value (no parsing, and thus no per-field parse
/// failure, is actually possible) — see the module doc's "why this is a
/// direct function" section.
///
/// Returns the pending entity; `xindeler-sim-bridge`'s adapter system reads
/// its descriptor components and despawns it once the real sim NPC has been
/// requested (see that crate's `entity_factory` module).
pub fn spawn_entity_template(
    commands: &mut Commands,
    template: &EntityTemplate,
    pos: [f32; 3],
    dimension: DimensionId,
) -> Entity {
    let mut ec = commands.spawn(PendingEntityTemplateSpawn { pos, dimension });
    ec.insert(PendingBody(template.body.clone()));
    ec.insert(PendingStats {
        name: template.stats.name.clone(),
    });
    ec.insert(PendingFaction(template.faction.clone()));
    if let Some(loot) = &template.loot {
        ec.insert(PendingLoot(loot.clone()));
    }
    ec.insert(PendingAiBehavior(template.ai_behavior_override.clone()));
    ec.id()
}

// ---------------------------------------------------------------------------
// BehaviorRegistry (AgentPreset)
// ---------------------------------------------------------------------------

/// A named preset over `common::comp::Agent`'s existing, battle-tested
/// `Psyche` knobs (BL-82 EM-4.7, spec §5.5's "v1: behavior strings map onto
/// presets of Veloren's existing Agent"). Every variant just tunes the SAME
/// fields `/spawn`/`/make_npc` already tune by hand (`server/src/cmd.rs`) —
/// deliberately NOT a new behavior-tree interpreter (that's v2, deferred).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentPreset {
    /// Never notices anyone (`aggro_range_multiplier = 0.0`) and never
    /// flees. The safe anti-chaos fallback for an unknown/malformed
    /// `ai_behavior_override` string — matches
    /// [`crate::dm_event::bounds::DEFAULT_AI_BEHAVIOR`] (`"passive"`).
    #[default]
    Passive,
    /// The body's own default wander/notice/engage behavior
    /// (`Agent::from_body`, untouched) — patrols/"stalks" around its spawn
    /// point and engages normally once a target comes into range.
    Stalk,
    /// Notices from farther away, skips the aggro warn-up, never flees.
    Aggro,
    /// Always flees rather than fighting (`flee_health = 1.0`).
    Flee,
}

impl AgentPreset {
    /// Multiplier [`Self::Aggro`] applies to `Psyche::aggro_range_multiplier`
    /// — notices targets from twice the body's own default range.
    const AGGRO_RANGE_MULTIPLIER: f32 = 2.0;

    /// Resolves an `ai_behavior_override` string to a preset, falling back
    /// to [`AgentPreset::Passive`] for anything outside
    /// [`crate::dm_event::bounds::KNOWN_AI_BEHAVIORS`] — the SAME closed set
    /// `DmEvent::sanitize`/`EntityTemplate::sanitize` already validate
    /// against, so a string that survived either sanitize pass always
    /// resolves to a real variant here too (never silently "does nothing"
    /// due to a typo slipping past both).
    #[must_use]
    pub fn resolve(name: &str) -> Self {
        match name {
            "stalk" => Self::Stalk,
            "aggro" => Self::Aggro,
            "flee" => Self::Flee,
            // "passive" and any unrecognized string both land here — the
            // fallback and the explicit choice are the same safe preset.
            _ => Self::Passive,
        }
    }

    /// Builds a real `comp::Agent` tuned to this preset, seeded from
    /// `body`'s own species-specific defaults (`Agent::from_body`) and
    /// patrolling around `pos` (sim/world axes, matching
    /// [`PendingEntityTemplateSpawn::pos`]).
    #[must_use]
    pub fn build_agent(self, body: &common::comp::Body, pos: [f32; 3]) -> common::comp::Agent {
        let origin = vek::Vec3::new(pos[0], pos[1], pos[2]);
        let mut agent = common::comp::Agent::from_body(body).with_patrol_origin(origin);
        match self {
            Self::Passive => {
                agent.psyche.aggro_range_multiplier = 0.0;
                agent.psyche.flee_health = 0.0;
            },
            Self::Stalk => {
                // Body-derived defaults, untouched — this preset IS
                // `Agent::from_body`'s baseline behavior.
            },
            Self::Aggro => {
                agent = agent.with_aggro_no_warn();
                agent.psyche.aggro_range_multiplier = Self::AGGRO_RANGE_MULTIPLIER;
                agent.psyche.flee_health = 0.0;
            },
            Self::Flee => {
                agent.psyche.flee_health = 1.0;
            },
        }
        agent
    }
}

// ---------------------------------------------------------------------------
// Asset + loader (mirrors DmEventLoader's shape exactly)
// ---------------------------------------------------------------------------

/// Async [`AssetLoader`] for `.entity_template.ron` / `.entity_template.json`
/// — dual-extension like [`crate::dm_event::DmEventLoader`], for the same
/// reason (ORACLE's LLM-side tooling emits JSON comfortably; RON is this
/// project's own convention).
#[derive(Default, TypePath)]
pub struct EntityTemplateLoader;

impl AssetLoader for EntityTemplateLoader {
    type Asset = EntityTemplate;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let is_json = load_context
            .path()
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            == Some("json");
        match parse_entity_template(&bytes, is_json) {
            Ok(template) => Ok(template),
            Err(err) => {
                warn!(
                    "entity_template: failed to load {} ({err}); ignoring (load failed, host \
                     keeps running)",
                    load_context.path()
                );
                Err(err)
            },
        }
    }

    fn extensions(&self) -> &[&str] { &["entity_template.ron", "entity_template.json"] }
}

fn parse_entity_template(bytes: &[u8], is_json: bool) -> Result<EntityTemplate, BevyError> {
    let mut template: EntityTemplate = if is_json {
        serde_json::from_slice(bytes)?
    } else {
        ron::de::from_bytes(bytes)?
    };
    template.sanitize();
    Ok(template)
}

/// Registers the [`EntityTemplate`] asset + loader. Requires `AssetPlugin`
/// already present, same contract as [`crate::dm_event::DmEventPlugin`].
pub struct EntityTemplatePlugin;

impl Plugin for EntityTemplatePlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<EntityTemplate>()
            .init_asset_loader::<EntityTemplateLoader>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hostile_template() -> EntityTemplate {
        EntityTemplate {
            entity_template_id: "x".repeat(dm_bounds::MAX_STRING_LEN * 4),
            body: "y".repeat(dm_bounds::MAX_STRING_LEN * 4),
            stats: EntityTemplateStats {
                name: Some("z".repeat(dm_bounds::MAX_STRING_LEN * 4)),
            },
            faction: "definitely_not_a_real_faction".to_owned(),
            loot: Some("w".repeat(dm_bounds::MAX_STRING_LEN * 4)),
            ai_behavior_override: "definitely_not_a_real_behavior".to_owned(),
        }
    }

    #[test]
    fn sanitize_defuses_hostile_templates() {
        let mut garbage = hostile_template();
        garbage.sanitize();

        assert!(garbage.entity_template_id.len() <= dm_bounds::MAX_STRING_LEN);
        assert!(garbage.body.len() <= dm_bounds::MAX_STRING_LEN);
        assert!(
            garbage.stats.name.as_ref().expect("still present").len() <= dm_bounds::MAX_STRING_LEN
        );
        assert_eq!(garbage.faction, bounds::DEFAULT_FACTION);
        assert!(garbage.loot.as_ref().expect("still present").len() <= dm_bounds::MAX_STRING_LEN);
        assert_eq!(garbage.ai_behavior_override, dm_bounds::DEFAULT_AI_BEHAVIOR);

        // A non-hostile template is untouched by sanitize (mirrors DmEvent's
        // own "already-sane" no-op check).
        let mut sane = EntityTemplate::default();
        sane.sanitize();
        assert_eq!(sane, EntityTemplate::default());
    }

    #[test]
    fn sanitize_is_idempotent() {
        let mut template = hostile_template();
        template.sanitize();
        let once = template.clone();
        template.sanitize();
        assert_eq!(template, once);
    }

    #[test]
    fn both_extensions_parse_identical_content_to_equal_values() {
        let original = EntityTemplate {
            entity_template_id: "dread_wolf".to_owned(),
            body: "wolf".to_owned(),
            stats: EntityTemplateStats {
                name: Some("Dread Wolf".to_owned()),
            },
            faction: "enemy".to_owned(),
            loot: Some("common.items.crafting_ing.hide.tough".to_owned()),
            ai_behavior_override: "aggro".to_owned(),
        };

        let ron_text = ron::ser::to_string(&original).expect("EntityTemplate serializes to RON");
        let json_text =
            serde_json::to_string(&original).expect("EntityTemplate serializes to JSON");

        let from_ron =
            parse_entity_template(ron_text.as_bytes(), false).expect(".entity_template.ron parses");
        let from_json = parse_entity_template(json_text.as_bytes(), true)
            .expect(".entity_template.json parses");

        assert_eq!(from_ron, original);
        assert_eq!(from_json, original);
    }

    #[test]
    fn malformed_input_fails_without_panic() {
        assert!(
            parse_entity_template(b"not valid ron {{{", false).is_err(),
            "garbage RON must fail the load, not panic"
        );
        assert!(
            parse_entity_template(b"{not valid json", true).is_err(),
            "garbage JSON must fail the load, not panic"
        );
    }

    /// The three shipped sample templates (`assets/xindeler/
    /// entity_templates/*.entity_template.ron`) parse, are already sane
    /// (`sanitize` is a no-op), and each use a DIFFERENT `ai_behavior_override`
    /// preset — the concrete demonstration that authoring a new template
    /// asset (three of them, here) requires zero Rust changes, per the
    /// module doc's acceptance-bar claim.
    #[test]
    fn shipped_sample_templates_parse_and_are_already_sane() {
        let fixtures = [
            (
                "dread_wolf",
                include_str!(
                    "../../../assets/xindeler/entity_templates/dread_wolf.entity_template.ron"
                ),
                "aggro",
            ),
            (
                "forest_deer",
                include_str!(
                    "../../../assets/xindeler/entity_templates/forest_deer.entity_template.ron"
                ),
                "flee",
            ),
            (
                "sentinel_owl",
                include_str!(
                    "../../../assets/xindeler/entity_templates/sentinel_owl.entity_template.ron"
                ),
                "stalk",
            ),
        ];

        for (id, text, expected_behavior) in fixtures {
            let mut parsed: EntityTemplate =
                ron::from_str(text).unwrap_or_else(|e| panic!("{id} parses: {e}"));
            assert_eq!(parsed.entity_template_id, id);
            assert_eq!(parsed.ai_behavior_override, expected_behavior);

            let before = parsed.clone();
            parsed.sanitize();
            assert_eq!(
                parsed, before,
                "{id} should already be sane (sanitize must be a no-op)"
            );
        }
    }

    #[derive(Resource)]
    struct TestTemplate(EntityTemplate);
    #[derive(Resource, Default)]
    struct TestSpawnedEntity(Option<Entity>);

    fn spawn_via_system(template: EntityTemplate, dimension: DimensionId) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(TestTemplate(template));
        app.init_resource::<TestSpawnedEntity>();
        app.add_systems(
            Startup,
            move |mut commands: Commands,
                  template: Res<TestTemplate>,
                  mut out: ResMut<TestSpawnedEntity>| {
                out.0 = Some(spawn_entity_template(
                    &mut commands,
                    &template.0,
                    [1.0, 2.0, 3.0],
                    dimension,
                ));
            },
        );
        app.update();
        let entity = app
            .world()
            .resource::<TestSpawnedEntity>()
            .0
            .expect("spawn_entity_template ran in Startup");
        (app, entity)
    }

    /// EM-4.7 acceptance: a template with every one of the five kinds
    /// populated attaches all five descriptor components, plus the pending
    /// marker carrying pos/dimension.
    #[test]
    fn spawn_entity_template_attaches_all_five_descriptor_components() {
        let template = EntityTemplate {
            entity_template_id: "dread_wolf".to_owned(),
            body: "wolf".to_owned(),
            stats: EntityTemplateStats {
                name: Some("Dread Wolf".to_owned()),
            },
            faction: "enemy".to_owned(),
            loot: Some("common.items.crafting_ing.hide.tough".to_owned()),
            ai_behavior_override: "aggro".to_owned(),
        };
        let (app, entity) = spawn_via_system(template, DimensionId::DEFAULT);
        let world = app.world();

        let pending = world
            .get::<PendingEntityTemplateSpawn>(entity)
            .expect("pending marker attached");
        assert_eq!(pending.pos, [1.0, 2.0, 3.0]);
        assert_eq!(pending.dimension, DimensionId::DEFAULT);

        assert_eq!(
            world.get::<PendingBody>(entity).expect("body attached").0,
            "wolf"
        );
        assert_eq!(
            world
                .get::<PendingStats>(entity)
                .expect("stats attached")
                .name,
            Some("Dread Wolf".to_owned())
        );
        assert_eq!(
            world
                .get::<PendingFaction>(entity)
                .expect("faction attached")
                .0,
            "enemy"
        );
        assert_eq!(
            world.get::<PendingLoot>(entity).expect("loot attached").0,
            "common.items.crafting_ing.hide.tough"
        );
        assert_eq!(
            world
                .get::<PendingAiBehavior>(entity)
                .expect("ai_behavior_override attached")
                .0,
            "aggro"
        );
    }

    /// A template with `loot: None` does NOT attach `PendingLoot` at all —
    /// the closure is a no-op for the absent case, not a component carrying
    /// an empty string.
    #[test]
    fn spawn_entity_template_omits_loot_when_none() {
        let template = EntityTemplate {
            loot: None,
            ..EntityTemplate::default()
        };
        let (app, entity) = spawn_via_system(template, DimensionId::DEFAULT);
        assert!(app.world().get::<PendingLoot>(entity).is_none());
    }

    #[test]
    fn agent_preset_resolve_falls_back_to_passive_for_unknown() {
        assert_eq!(AgentPreset::resolve("stalk"), AgentPreset::Stalk);
        assert_eq!(AgentPreset::resolve("aggro"), AgentPreset::Aggro);
        assert_eq!(AgentPreset::resolve("flee"), AgentPreset::Flee);
        assert_eq!(AgentPreset::resolve("passive"), AgentPreset::Passive);
        assert_eq!(
            AgentPreset::resolve("definitely_not_a_real_behavior"),
            AgentPreset::Passive,
            "an unknown ai_behavior_override string must default to Passive, never panic"
        );
    }

    fn pig_body() -> common::comp::Body {
        common::comp::quadruped_small::Body {
            species: common::comp::quadruped_small::Species::Pig,
            body_type: common::comp::quadruped_small::BodyType::Female,
        }
        .into()
    }

    #[test]
    fn agent_preset_tunings_are_observably_distinct() {
        let body = pig_body();
        let pos = [10.0, 20.0, 30.0];

        let passive = AgentPreset::Passive.build_agent(&body, pos);
        assert_eq!(passive.psyche.aggro_range_multiplier, 0.0);
        assert_eq!(passive.psyche.flee_health, 0.0);

        let stalk = AgentPreset::Stalk.build_agent(&body, pos);
        let baseline = common::comp::Agent::from_body(&body);
        assert_eq!(
            stalk.psyche.aggro_range_multiplier,
            baseline.psyche.aggro_range_multiplier
        );
        assert_eq!(stalk.psyche.flee_health, baseline.psyche.flee_health);

        let aggro = AgentPreset::Aggro.build_agent(&body, pos);
        assert_eq!(
            aggro.psyche.aggro_range_multiplier,
            AgentPreset::AGGRO_RANGE_MULTIPLIER
        );
        assert_eq!(
            aggro.psyche.aggro_dist, None,
            "aggro preset skips the warn-up"
        );
        assert_eq!(aggro.psyche.flee_health, 0.0, "aggro preset never flees");

        let flee = AgentPreset::Flee.build_agent(&body, pos);
        assert_eq!(flee.psyche.flee_health, 1.0);

        // All four presets are pairwise distinct on at least one Psyche
        // field — the smoke-test-observable proof that they're real,
        // different behaviors, not four names for the same tuning.
        assert_ne!(
            passive.psyche.aggro_range_multiplier,
            stalk.psyche.aggro_range_multiplier
        );
        assert_ne!(
            passive.psyche.aggro_range_multiplier,
            aggro.psyche.aggro_range_multiplier
        );
        assert_ne!(flee.psyche.flee_health, passive.psyche.flee_health);
        assert_ne!(flee.psyche.flee_health, aggro.psyche.flee_health);
    }

    #[test]
    fn agent_preset_build_agent_sets_patrol_origin() {
        let body = pig_body();
        let agent = AgentPreset::Stalk.build_agent(&body, [5.0, 6.0, 7.0]);
        assert_eq!(agent.patrol_origin, Some(vek::Vec3::new(5.0, 6.0, 7.0)));
    }
}
