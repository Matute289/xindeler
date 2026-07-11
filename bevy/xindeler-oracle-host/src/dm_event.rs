//! `DmEvent` — the ORACLE "drop a file, spin up an encounter" schema (BL-82
//! EM-4.3 + EM-4.4, spec §5.1 / §0.4 / §1.6 / §1.7).
//!
//! Applies the [`crate::atmosphere`] loader pattern (RON `Asset` +
//! `AssetLoader`, `bounds::` + `sane()` + `sanitize()` anti-chaos, hot reload
//! via `AssetEvent`) to a new asset type. Two pieces genuinely go beyond that
//! template:
//! - a **dual-extension loader** (`.dmevent.ron` / `.dmevent.json`) — ORACLE's
//!   LLM-side tooling emits JSON comfortably, RON is the project's own
//!   convention, so both parse into the identical [`DmEvent`];
//! - a **custom `oracle://` `AssetSource`** rooted at a runtime watch directory
//!   OUTSIDE `assets/` — ORACLE writes files there; the game's shipped asset
//!   tree never sees them.
//!
//! `DmEvent.atmosphere` reuses [`AtmosphereProfile`] directly (not a
//! re-declared parallel schema) so its `bounds::`/`sanitize` clamps are
//! inherited for free.
//!
//! ## v1 scope
//! EM-4.3/4.4 only **load + validate**: nothing in THIS module spins up a
//! dimension, applies the atmosphere override, or spawns monsters — those
//! seams are EM-4.5 (`DimensionRegistry`) / EM-4.7 (entity factory) / EM-4.8
//! (narrative hooks) / EM-4.9 (the end-to-end drill).
//!
//! **Update (BL-82 EM-4.9, 2026-07-11):** the real wiring landed.
//! `xindeler-sim-bridge::oracle::ServerOraclePlugin` calls
//! [`register_oracle_source`] BEFORE `AssetPlugin` in `main.rs` and adds
//! [`DmEventPlugin`] AFTER it, then a producer system set
//! (`ingest_dm_events`/`spawn_event_minions`/`retire_dm_events`) reads
//! `AssetEvent<DmEvent>` to drive a real dimension spinup + factory spawn +
//! narrative-hook registration — see that module's own doc comment for the
//! full chain. Server-authoritative note: only that server-side shell
//! registers [`DmEventPlugin`] — `xindeler-client` (which depends on this
//! crate only for [`atmosphere`](crate::atmosphere)) never adds it, and the
//! client only ever receives the resulting normal net state (replicated
//! entities, `HudToast`, the atmosphere-replication seam), never a raw
//! `DmEvent` or file.

use std::path::PathBuf;

use bevy::{
    asset::{
        AssetLoader, LoadContext,
        io::{AssetSourceBuilder, AssetSourceId, Reader},
    },
    ecs::error::BevyError,
    prelude::*,
};
use serde::{Deserialize, Serialize};

use crate::atmosphere::{AtmosphereProfile, sane};

/// One dungeon-master event: a self-contained "spin up an instanced
/// encounter" spec ORACLE's tooling writes as a file (migration spec §5.1's
/// Mist-Bound example — `dimension_config`, `atmosphere`, `spawning_rules`,
/// `narrative`). `#[serde(default)]` throughout so partial files keep
/// loading as the schema grows, exactly like [`AtmosphereProfile`].
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DmEvent {
    /// World-gen parameters for the dimension this event spins up (EM-4.5
    /// consumes this).
    pub dimension_config: DimensionConfig,
    /// Reuses [`AtmosphereProfile`] DIRECTLY (spec §1.7: "should just BE an
    /// `AtmosphereProfile`, not a re-declared parallel schema") — every
    /// clamp `AtmosphereProfile::sanitize` already enforces is inherited for
    /// free.
    pub atmosphere: AtmosphereProfile,
    /// What (and how many) monsters populate the instance (EM-4.7 consumes
    /// this).
    pub spawning_rules: SpawningRules,
    /// DM-flavour text hooks (EM-4.8 consumes these; this crate only loads +
    /// validates them for now).
    pub narrative: Narrative,
}

impl DmEvent {
    /// Anti-chaos clamps (spec §1.7): delegates the nested `atmosphere`
    /// field to [`AtmosphereProfile::sanitize`] and applies this schema's
    /// own [`bounds`] table to everything else. Runs on EVERY ingestion path
    /// (today: [`DmEventLoader::load`]) so no unclamped value can ever reach
    /// a later consumer (EM-4.5/4.7/4.8). Idempotent, like
    /// `AtmosphereProfile::sanitize`.
    pub fn sanitize(&mut self) {
        self.atmosphere.sanitize();
        self.dimension_config.sanitize();
        self.spawning_rules.sanitize();
        self.narrative.sanitize();
    }
}

/// World-gen parameters for the dimension a [`DmEvent`] spins up (spec §5.3:
/// `world`'s generator runs with `seed_modifier` + an injected biome profile
/// on the async task pool — EM-4.5's job; loading + validating the values is
/// all THIS task does).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DimensionConfig {
    /// Procgen seed offset. Any `u64` is a legal seed — the type itself
    /// already excludes NaN/inf, and no value is "unsafe" (world-gen must
    /// already produce valid terrain for any seed) — so, unlike every float
    /// field in this module, this one is deliberately exempt from a
    /// `bounds::` clamp; documented here rather than silently skipped (spec
    /// §1.7's "no field is exempt … because it's probably fine" is about
    /// numeric fields that COULD be hostile; this one structurally can't).
    pub seed_modifier: u64,
    /// Named biome profile injected into the generator (EM-4.5). Free-form
    /// for now — no `world`-side allowlist exists yet to validate against —
    /// so `sanitize` only enforces a defensive length cap
    /// ([`bounds::MAX_STRING_LEN`]) against a pathologically large hostile
    /// string, not a content allowlist.
    pub biome_profile: String,
}

impl Default for DimensionConfig {
    fn default() -> Self {
        Self {
            seed_modifier: 0,
            biome_profile: "default".to_owned(),
        }
    }
}

impl DimensionConfig {
    fn sanitize(&mut self) { truncate_to(&mut self.biome_profile, bounds::MAX_STRING_LEN); }
}

/// What (and how many) monsters a [`DmEvent`] spawns (EM-4.7 consumes the
/// resolved list; loading + validating it is all THIS task does).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawningRules {
    /// `EntityTemplate` ids (EM-4.7) to draw spawns from.
    pub entity_templates: Vec<String>,
    /// How many entities to spawn. A plain `f32` (not `u32`/`usize`) so a
    /// hostile float value (NaN/inf/huge) is representable in a crafted file
    /// and exercised by [`Self::sanitize`] exactly like every other numeric
    /// field in this module — EM-4.7 rounds this down to a whole count when
    /// it actually spawns.
    pub spawn_count: f32,
    /// Spawn scatter radius, blocks.
    pub spawn_radius: f32,
    /// Resolves through EM-4.7's `BehaviorRegistry` (stalk/aggro/flee
    /// presets over the existing `server-agent` `Agent`). An unknown string
    /// clamps to [`bounds::DEFAULT_AI_BEHAVIOR`] rather than being rejected
    /// — "defuse, don't crash" (spec §1.7).
    pub ai_behavior_override: String,
}

impl Default for SpawningRules {
    fn default() -> Self {
        Self {
            entity_templates: Vec::new(),
            spawn_count: 0.0,
            spawn_radius: 50.0,
            ai_behavior_override: bounds::DEFAULT_AI_BEHAVIOR.to_owned(),
        }
    }
}

impl SpawningRules {
    fn sanitize(&mut self) {
        let defaults = Self::default();
        self.spawn_count = sane(self.spawn_count, bounds::SPAWN_COUNT, defaults.spawn_count);
        self.spawn_radius = sane(
            self.spawn_radius,
            bounds::SPAWN_RADIUS,
            defaults.spawn_radius,
        );
        if !bounds::KNOWN_AI_BEHAVIORS.contains(&self.ai_behavior_override.as_str()) {
            self.ai_behavior_override = bounds::DEFAULT_AI_BEHAVIOR.to_owned();
        }
        if self.entity_templates.len() > bounds::MAX_ENTITY_TEMPLATES {
            self.entity_templates.truncate(bounds::MAX_ENTITY_TEMPLATES);
        }
        for template in &mut self.entity_templates {
            truncate_to(template, bounds::MAX_STRING_LEN);
        }
    }
}

/// DM-flavour text hooks (EM-4.8 consumes these — `world_rumor` into the
/// chronicle-hook log, `on_enter_message` into a `HudToast`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Narrative {
    /// Appended to the chronicle-hook log the moment the event loads.
    pub world_rumor: Option<String>,
    /// Sent to the one client whose mirrored entity enters this event's
    /// dimension.
    pub on_enter_message: Option<String>,
}

impl Narrative {
    fn sanitize(&mut self) {
        if let Some(text) = &mut self.world_rumor {
            truncate_to(text, bounds::MAX_STRING_LEN);
        }
        if let Some(text) = &mut self.on_enter_message {
            truncate_to(text, bounds::MAX_STRING_LEN);
        }
    }
}

/// Truncates `s` to at most `max_bytes` bytes, walking back to the nearest
/// char boundary so a hostile string that splits a multi-byte char exactly
/// at `max_bytes` can never panic.
///
/// `pub(crate)` (EM-4.7): `entity_template.rs` reuses this exact helper for
/// its own free-form string fields (`body`/`loot`/stats name/…) rather than
/// duplicating it — same anti-chaos primitive, one definition, mirroring
/// `atmosphere::sane`'s own `pub(crate)` reuse rationale.
pub(crate) fn truncate_to(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// Clamp bounds enforced by [`DmEvent::sanitize`] (anti-chaos, spec §1.7:
/// `DmEvent` files are a surface ORACLE's LLM-side tooling writes, so
/// hostile/buggy values are the threat model — same reasoning as
/// [`crate::atmosphere::bounds`], extended to this schema's own fields).
/// `DmEvent.atmosphere`'s numeric fields are already covered by
/// [`crate::atmosphere::bounds`] via `AtmosphereProfile::sanitize`; this
/// module only adds the fields that type doesn't have.
pub mod bounds {
    /// `SpawningRules::spawn_count` (EM-4.7 rounds down to a whole number
    /// before actually spawning).
    pub const SPAWN_COUNT: (f32, f32) = (0.0, 200.0);
    /// `SpawningRules::spawn_radius`, blocks.
    pub const SPAWN_RADIUS: (f32, f32) = (0.0, 2000.0);
    /// Ceiling on any free-form string field (`biome_profile`, narrative
    /// text, entity template ids, ...) — a defensive floor against an
    /// unboundedly large hostile value, not a game-design number.
    pub const MAX_STRING_LEN: usize = 4096;
    /// Ceiling on `SpawningRules::entity_templates`'s length.
    pub const MAX_ENTITY_TEMPLATES: usize = 64;
    /// `ai_behavior_override` allowlist: EM-4.7's `BehaviorRegistry` presets
    /// over the existing `server-agent` `Agent` (stalk/aggro/flee) plus the
    /// safe default. Until that registry exists, this is the closed set an
    /// unknown string clamps against ("defuse, don't crash" — spec §1.7).
    pub const KNOWN_AI_BEHAVIORS: &[&str] = &["passive", "stalk", "aggro", "flee"];
    /// Safe fallback preset for an unrecognized `ai_behavior_override`.
    pub const DEFAULT_AI_BEHAVIOR: &str = "passive";
}

/// Async [`AssetLoader`] for `.dmevent.ron` / `.dmevent.json` (EM-4.3):
/// branches parser by extension, sanitizes on the way in (EM-4.4) — mirrors
/// [`crate::atmosphere::AtmosphereProfileLoader`] exactly, plus the
/// extension branch.
#[derive(Default, TypePath)]
pub struct DmEventLoader;

impl AssetLoader for DmEventLoader {
    type Asset = DmEvent;
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
        // `Path::extension()` on `foo.dmevent.ron` / `foo.dmevent.json`
        // yields the LAST dot-segment ("ron"/"json") — enough to
        // disambiguate the two formats this loader's `extensions()` list
        // registers.
        let is_json = load_context
            .path()
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            == Some("json");
        match parse_dm_event(&bytes, is_json) {
            Ok(event) => Ok(event),
            Err(err) => {
                // Anti-chaos (spec §1.6): a malformed/garbage file must
                // never crash the host — surface a `warn!` and let the
                // asset fail to load like any other missing/broken asset.
                warn!(
                    "dm_event: failed to load {} ({err}); ignoring (load failed, host keeps \
                     running)",
                    load_context.path()
                );
                Err(err)
            },
        }
    }

    fn extensions(&self) -> &[&str] { &["dmevent.ron", "dmevent.json"] }
}

/// Parses `bytes` as RON (`is_json == false`) or JSON, then sanitizes.
/// A free function (not inlined into [`DmEventLoader::load`]) so tests can
/// exercise malformed input directly without spinning up a full
/// `App`/`AssetServer`.
fn parse_dm_event(bytes: &[u8], is_json: bool) -> Result<DmEvent, BevyError> {
    let mut event: DmEvent = if is_json {
        serde_json::from_slice(bytes)?
    } else {
        ron::de::from_bytes(bytes)?
    };
    // Anti-chaos (EM-4.4): ORACLE-written files are untrusted input.
    event.sanitize();
    Ok(event)
}

/// Name of the custom `AssetSource` [`DmEvent`] files load from
/// (`oracle://foo.dmevent.ron`) — a runtime directory OUTSIDE `assets/`,
/// never the game's shipped asset tree.
pub const ORACLE_SOURCE: &str = "oracle";

/// Env var overriding the ORACLE event watch directory. Mirrors
/// `xindeler-app::settings`'s local env-var-resolution pattern deliberately,
/// rather than depending on `xindeler-app`/`common-base` for a single path
/// helper — same "keep this shell crate's dependency graph shallow"
/// reasoning that crate's own module doc comment gives for not depending on
/// `common-base`.
pub const ORACLE_EVENTS_DIR_ENV: &str = "XINDELER_ORACLE_EVENTS_DIR";

/// Default runtime directory ORACLE writes `.dmevent.ron`/`.dmevent.json`
/// files into. Named in the style of the existing `<userdata>/server`
/// convention (`xindeler-server-app`'s own `<userdata>/server` root), but
/// resolved independently (its own env var, its own `./userdata` fallback)
/// rather than nested under whatever `common-base::userdata_dir()`/
/// `XINDELER_USERDATA` currently resolve to — so the two do NOT
/// automatically relocate together; a deployment that moves userdata via
/// those still needs `XINDELER_ORACLE_EVENTS_DIR` set explicitly to follow.
#[must_use]
pub fn default_events_dir() -> PathBuf {
    std::env::var_os(ORACLE_EVENTS_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./userdata/oracle_events"))
}

/// Registers ONLY the `oracle://` `AssetSource`, rooted at `events_dir`
/// (created if missing — bevy's default file watcher silently no-ops, with
/// only a `warn!`, if the path doesn't exist yet when the source is built).
///
/// **Must be called BEFORE `DefaultPlugins`/`MinimalPlugins` + `AssetPlugin`
/// are added** — `bevy_asset`'s own `AssetApp::register_asset_source` doc
/// comment: "asset sources must be registered before adding `AssetPlugin`,
/// since registered asset sources are built at that point and not after."
/// This is a free function rather than part of [`DmEventPlugin`] because the
/// two halves of "register `oracle://`" have OPPOSITE ordering
/// requirements against `AssetPlugin` — this one must run BEFORE it,
/// [`DmEventPlugin`] (which needs the `AssetServer` resource `AssetPlugin`
/// inserts) must run AFTER it. No single `Plugin::build` can satisfy both,
/// so callers do:
///
/// ```ignore
/// register_oracle_source(&mut app, &events_dir);
/// app.add_plugins(DefaultPlugins /* or MinimalPlugins + AssetPlugin */);
/// app.add_plugins(DmEventPlugin);
/// ```
pub fn register_oracle_source(app: &mut App, events_dir: &std::path::Path) {
    if let Err(err) = std::fs::create_dir_all(events_dir) {
        warn!(
            "dm_event: could not create the oracle events dir {} ({err}); the file watcher will \
             not activate until it exists",
            events_dir.display()
        );
    }
    let path = events_dir.to_string_lossy().into_owned();
    app.register_asset_source(
        AssetSourceId::Name(ORACLE_SOURCE.into()),
        AssetSourceBuilder::platform_default(&path, None),
    );
}

/// Registers the [`DmEvent`] asset + [`DmEventLoader`]. Requires
/// `AssetPlugin` (part of `DefaultPlugins`/`MinimalPlugins`) to already be
/// present in the `App` — the same contract every `init_asset`/
/// `init_asset_loader` caller has (e.g. `XindelerAtmospherePlugin`, added
/// after `DefaultPlugins` in `xindeler-client`'s `main.rs`). Must be paired
/// with [`register_oracle_source`] (called BEFORE `AssetPlugin`) so
/// `oracle://` paths actually resolve — see that function's doc comment for
/// the full two-phase ordering contract.
pub struct DmEventPlugin;

impl Plugin for DmEventPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<DmEvent>()
            .init_asset_loader::<DmEventLoader>();
    }
}

#[cfg(test)]
mod tests {
    use bevy::{
        app::App,
        asset::{AssetPlugin, AssetServer, Handle},
        prelude::{Messages, MinimalPlugins},
    };

    use super::*;

    /// A [`DmEvent`] with every corner deliberately hostile: NaN/negative/
    /// out-of-range numerics, an unknown behavior string, oversized strings
    /// and an oversized template list. Shared by the "defuses" and
    /// "idempotent" tests below.
    fn hostile_dm_event() -> DmEvent {
        DmEvent {
            dimension_config: DimensionConfig {
                seed_modifier: u64::MAX,
                biome_profile: "x".repeat(bounds::MAX_STRING_LEN * 4),
            },
            atmosphere: AtmosphereProfile {
                fog_density: f32::NAN,
                fog_color: [-3.0, f32::INFINITY, 42.0],
                fog_volume_density: -7.0,
                sun_illuminance: 9.0e9,
                time_lock: Some(37.0),
                transition_secs: -1.0,
                ..Default::default()
            },
            spawning_rules: SpawningRules {
                entity_templates: (0..bounds::MAX_ENTITY_TEMPLATES * 4)
                    .map(|i| "t".repeat(bounds::MAX_STRING_LEN * 2) + &i.to_string())
                    .collect(),
                spawn_count: f32::NAN,
                spawn_radius: -500.0,
                ai_behavior_override: "definitely_not_a_real_behavior".to_owned(),
            },
            narrative: Narrative {
                world_rumor: Some("y".repeat(bounds::MAX_STRING_LEN * 4)),
                on_enter_message: None,
            },
        }
    }

    #[test]
    fn sanitize_defuses_hostile_dm_events() {
        let mut garbage = hostile_dm_event();
        garbage.sanitize();

        let defaults = AtmosphereProfile::default();
        assert!((garbage.atmosphere.fog_density - defaults.fog_density).abs() < f32::EPSILON);
        assert!(garbage.atmosphere.fog_volume_density.is_finite());

        assert!(garbage.dimension_config.biome_profile.len() <= bounds::MAX_STRING_LEN);

        assert!(garbage.spawning_rules.spawn_count.is_finite());
        assert!(
            (bounds::SPAWN_COUNT.0..=bounds::SPAWN_COUNT.1)
                .contains(&garbage.spawning_rules.spawn_count)
        );
        assert!(
            (bounds::SPAWN_RADIUS.0..=bounds::SPAWN_RADIUS.1)
                .contains(&garbage.spawning_rules.spawn_radius)
        );
        assert_eq!(
            garbage.spawning_rules.ai_behavior_override,
            bounds::DEFAULT_AI_BEHAVIOR
        );
        assert!(garbage.spawning_rules.entity_templates.len() <= bounds::MAX_ENTITY_TEMPLATES);
        for template in &garbage.spawning_rules.entity_templates {
            assert!(template.len() <= bounds::MAX_STRING_LEN);
        }

        assert!(
            garbage.narrative.world_rumor.expect("still present").len() <= bounds::MAX_STRING_LEN
        );
        assert_eq!(garbage.narrative.on_enter_message, None);

        // A non-hostile event is untouched by sanitize (mirrors
        // `AtmosphereProfile`'s own "already-sane profile" no-op check).
        let mut sane_event = DmEvent::default();
        sane_event.sanitize();
        assert_eq!(sane_event, DmEvent::default());
    }

    #[test]
    fn sanitize_is_idempotent() {
        let mut event = hostile_dm_event();
        event.sanitize();
        let once = event.clone();
        event.sanitize();
        assert_eq!(event, once);
    }

    #[test]
    fn both_extensions_parse_identical_content_to_equal_values() {
        let original = DmEvent {
            dimension_config: DimensionConfig {
                seed_modifier: 42,
                biome_profile: "mist_bound_mist".to_owned(),
            },
            atmosphere: AtmosphereProfile {
                fog_density: 0.2,
                time_lock: Some(23.5),
                ..Default::default()
            },
            spawning_rules: SpawningRules {
                entity_templates: vec!["ghost_wolf".to_owned(), "banshee".to_owned()],
                spawn_count: 6.0,
                spawn_radius: 120.0,
                ai_behavior_override: "aggro".to_owned(),
            },
            narrative: Narrative {
                world_rumor: Some("A cold mist swallows the village.".to_owned()),
                on_enter_message: Some("The gate to the Mist-Bound creaks open.".to_owned()),
            },
        };

        let ron_text = ron::ser::to_string(&original).expect("DmEvent serializes to RON");
        let json_text = serde_json::to_string(&original).expect("DmEvent serializes to JSON");

        let from_ron = parse_dm_event(ron_text.as_bytes(), false).expect(".dmevent.ron parses");
        let from_json = parse_dm_event(json_text.as_bytes(), true).expect(".dmevent.json parses");

        // `original`'s values are already sane, so `parse_dm_event`'s
        // sanitize pass is a no-op and both extensions must agree exactly.
        assert_eq!(from_ron, original);
        assert_eq!(from_json, original);
    }

    /// BL-82 EM-4.9 (T51.5): the shipped `mist_bound.dmevent.ron` (the
    /// canonical example both the E2E drill test and a real human drop use)
    /// parses AND `sanitize()` is a no-op — every value in it must already
    /// sit inside `dm_event::bounds`/`atmosphere::bounds`, per spec §5.1's
    /// own comments.
    #[test]
    fn shipped_mist_bound_dmevent_parses_and_is_already_sane() {
        use crate::atmosphere::WeatherEffect;

        let text = include_str!("../../../assets/xindeler/oracle_events/mist_bound.dmevent.ron");
        let mut parsed: DmEvent = ron::from_str(text).expect("mist_bound.dmevent.ron parses");

        assert_eq!(parsed.dimension_config.seed_modifier, 1_298_754_643);
        assert_eq!(
            parsed.dimension_config.biome_profile,
            "mist_bound_grey_forest"
        );
        assert_eq!(parsed.atmosphere.time_lock, Some(23.5));
        assert_eq!(parsed.atmosphere.weather_effect, WeatherEffect::Rain);
        assert_eq!(parsed.spawning_rules.entity_templates, vec![
            "mist_bound_shade".to_owned()
        ]);
        assert!((parsed.spawning_rules.spawn_count - 15.0).abs() < f32::EPSILON);
        assert_eq!(parsed.spawning_rules.ai_behavior_override, "aggro");
        assert!(parsed.narrative.world_rumor.is_some());
        assert!(parsed.narrative.on_enter_message.is_some());

        let before = parsed.clone();
        parsed.sanitize();
        assert_eq!(
            parsed, before,
            "mist_bound.dmevent.ron should already be sane (sanitize must be a no-op)"
        );
    }

    #[test]
    fn malformed_input_fails_without_panic() {
        assert!(
            parse_dm_event(b"not valid ron {{{", false).is_err(),
            "garbage RON must fail the load, not panic"
        );
        assert!(
            parse_dm_event(b"{not valid json", true).is_err(),
            "garbage JSON must fail the load, not panic"
        );
    }

    fn drain_dm_events(app: &mut App) -> Vec<AssetEvent<DmEvent>> {
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<DmEvent>>>()
            .drain()
            .collect()
    }

    /// EM-4.3's hot-reload acceptance bar: dropping a `.dmevent.ron` file
    /// into the watched `oracle://` directory fires
    /// `AssetEvent::Added<DmEvent>` within ~1s (loose bound; the atmosphere
    /// precedent this pattern mirrors measured ~0.4s).
    ///
    /// Note the load-then-write ordering below is required, not incidental:
    /// `bevy_asset`'s file watcher only reloads paths that already have an
    /// outstanding handle (verified against its own test suite,
    /// `reloads_asset_after_source_event` in `bevy_asset::lib`) — a brand
    /// new, never-requested path is NOT auto-discovered by watching a
    /// directory. So we request the (not-yet-existing) file first, exactly
    /// like a real caller would (e.g. ORACLE announcing a filename before
    /// writing it, or the host retrying a previously-missing well-known
    /// path), then write it.
    #[test]
    fn dropped_file_triggers_asset_added_within_1s() {
        let dir = tempfile::tempdir().expect("tempdir");
        // macOS gotcha: `$TMPDIR` (what `tempfile` uses) resolves through a
        // `/var` -> `/private/var` symlink; `notify`'s macOS FSEvents backend
        // reports the CANONICAL (symlink-resolved) path, but bevy_asset's
        // `FileWatcher` roots itself at `std::path::absolute(path)` (no
        // symlink resolution — see its own `make_absolute_path` doc comment)
        // and panics stripping the prefix if the two disagree. Canonicalize
        // up front so both sides agree, exactly as a real deployment would
        // want anyway (a real `<userdata>/oracle_events` dir could equally
        // sit behind a symlink).
        let root = dir.path().canonicalize().expect("canonicalize tempdir");

        let mut app = App::new();
        // Two-phase ordering (see `register_oracle_source`'s doc comment):
        // the source must be registered BEFORE `AssetPlugin`, `DmEventPlugin`
        // (which needs `AssetServer` to already exist) AFTER it.
        register_oracle_source(&mut app, &root);
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default());
        app.add_plugins(DmEventPlugin);
        app.finish();
        app.update();

        let asset_server = app.world().resource::<AssetServer>().clone();
        let handle: Handle<DmEvent> = asset_server.load("oracle://dropped.dmevent.ron");
        // Let the (expected-to-fail, file doesn't exist yet) first load
        // attempt settle before writing the file.
        for _ in 0..20 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drain_dm_events(&mut app); // discard the failed-load noise

        let ron_text = ron::ser::to_string(&DmEvent::default()).expect("serializes");
        std::fs::write(dir.path().join("dropped.dmevent.ron"), ron_text).expect("write fixture");

        let mut added = false;
        for _ in 0..1000 {
            app.update();
            if drain_dm_events(&mut app)
                .iter()
                .any(|event| matches!(event, AssetEvent::Added { id } if *id == handle.id()))
            {
                added = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            added,
            "AssetEvent::Added<DmEvent> did not fire within ~1s of dropping the file"
        );
    }
}
