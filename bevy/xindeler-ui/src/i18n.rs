//! BL-82 EM-5.1 T56.3 / EM-5.16 T56.44 — the i18n seam: a thin in-house loader
//! over the real `fluent`/`unic-langid` crates (spec §4: "prefer a thin
//! in-house fluent loader over a lagging dep — the `.ftl` parsing crate
//! `fluent` itself is engine-agnostic and stable"), reusing the SAME frozen
//! `.ftl` assets under `assets/voxygen/i18n/` the legacy client uses verbatim
//! — never a new catalog, never a renamed path (isolation law).
//!
//! ## T56.44 — full multi-locale + reactive hot-swap
//! [`Localization::load`] now takes a real target [`LanguageIdentifier`]
//! instead of hardcoding `en`, and degrades gracefully when the requested
//! locale's `.ftl` files are missing or incomplete: it ALSO loads the same
//! file list from `en` as a fallback bundle (skipped when the target locale
//! already IS `en`), and [`Localization::tr`]/[`Localization::tr_attr`] try
//! the primary bundle, then the `en` fallback, and only fall back to the bare
//! key/`"{key}.{attr}"` if NEITHER resolves it — a missing key (or an
//! entirely missing locale directory) still never panics, exactly the
//! pre-existing "no screen hardcodes English... but a missing translation
//! must never crash the HUD" philosophy, just with one more fallback rung.
//!
//! The REACTIVE half lives here too: [`CurrentLocale`] is a plain `Resource`
//! (a `String` tag is trivially `Send + Sync`, unlike the `FluentBundle`
//! memoizer below) tracking which locale is currently loaded.
//! [`reload_localization_on_locale_change`] rebuilds the `NonSend`
//! [`Localization`] bundle whenever it changes, and
//! [`relocalize_text`]/[`relocalize_button_labels`] re-resolve every tagged
//! [`LocalizedText`]/[`LocalizedLabel`] entity in the SAME frame (chained,
//! `.in_set(LocaleSyncSet)`) — so switching the language setting re-localizes
//! every already-spawned, tagged screen live, with no app restart. Screen
//! code has nothing bespoke to write beyond tagging its localized text/button
//! entities at spawn time (see those two components' own docs); whoever
//! CHANGES [`CurrentLocale`] (`xindeler-client`'s settings bridge) just needs
//! to order itself `.before(LocaleSyncSet)` so the change and the reload/
//! relocalize land in the same frame.
//!
//! [`Localization`] itself is still stored as a Bevy `NonSend` resource (not a
//! `Resource`): `fluent::FluentBundle`'s default memoizer is not `Sync`, and
//! this crate has no need to fight that — HUD text resolution is a
//! main-thread-only concern anyway, exactly like
//! `xindeler-sim-bridge::SimServer`'s own `NonSend` posture for a similarly
//! non-`Sync` embedded type.

use std::path::PathBuf;

use bevy::prelude::*;
pub use fluent::FluentArgs;
use fluent::{FluentBundle, FluentResource};
use unic_langid::{LanguageIdentifier, langid};

/// Resolves the real asset root the same way every other `bevy/*` crate does
/// (`XINDELER_ASSETS` > `VELOREN_ASSETS` > `<cwd>/assets`) — duplicated here
/// (rather than imported from `xindeler-client`) because `xindeler-client`
/// depends on THIS crate, not the other way around.
#[must_use]
pub fn assets_root() -> PathBuf {
    if let Ok(dir) = std::env::var("XINDELER_ASSETS") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("VELOREN_ASSETS") {
        return PathBuf::from(dir);
    }
    PathBuf::from("assets")
}

/// The locale every other locale falls back to when it's missing a key (or
/// missing its `.ftl` directory entirely) — the frozen, always-complete `en`
/// catalog under `assets/voxygen/i18n/en/`.
#[must_use]
pub fn fallback_locale() -> LanguageIdentifier { langid!("en") }

/// Parses a BCP-47-ish tag (e.g. `XindelerSettings::language`'s `"en"`/
/// `"es"`/`"zh-Hans"`) into a real [`LanguageIdentifier`]. An unparseable tag
/// falls back to [`fallback_locale`] — never panics, matching this module's
/// "a missing/unresolvable thing degrades, it never crashes the HUD"
/// philosophy.
#[must_use]
pub fn parse_locale(tag: &str) -> LanguageIdentifier {
    tag.parse().unwrap_or_else(|_| fallback_locale())
}

/// The one field of a locale's `_manifest.ron` this crate reads — its own
/// declared display name (e.g. `assets/voxygen/i18n/es/_manifest.ron`'s
/// `metadata.language_name = "Español de España (Spanish - Spain)"`). A
/// nested struct (not a flat one) because that's the manifest's real shape;
/// every other field (`language_identifier`, `fonts`) is intentionally
/// un-deserialized (`serde` ignores unknown-to-it siblings by default, but
/// here it's the reverse: we only declare the ONE field we read, so adding a
/// new manifest field upstream never breaks this parse).
#[derive(serde::Deserialize)]
struct LocaleManifest {
    metadata: LocaleManifestMetadata,
}

#[derive(serde::Deserialize)]
struct LocaleManifestMetadata {
    language_name: String,
}

/// Reads `lang`'s own declared display name straight from its shipped
/// `_manifest.ron` (`assets/voxygen/i18n/<lang>/_manifest.ron`) — the SAME
/// file the legacy `client/i18n` crate's `LanguageMetadata` already sources
/// this from, rather than hand-copying it into a Rust const (which had
/// already drifted: a hardcoded `"Español"` vs. the manifest's real
/// `"Español de España (Spanish - Spain)"`, a game-architecture-reviewer
/// finding on an earlier revision of this module). Falls back to the bare
/// BCP-47 tag if the manifest is missing/unparseable — never panics, the
/// same degrade-clean posture as everything else in this module.
#[must_use]
pub fn language_name(lang: &LanguageIdentifier) -> String {
    let path = assets_root()
        .join("voxygen/i18n")
        .join(lang.to_string())
        .join("_manifest.ron");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return lang.to_string();
    };
    ron::from_str::<LocaleManifest>(&text).map_or_else(
        |_| lang.to_string(),
        |manifest| manifest.metadata.language_name,
    )
}

/// A loaded `.ftl` catalog for one locale, with an `en` fallback bundle for
/// whatever that locale doesn't cover (see the module doc for the resolve
/// order).
pub struct Localization {
    lang: LanguageIdentifier,
    bundle: FluentBundle<FluentResource>,
    /// `None` when [`Self::lang`] already IS [`fallback_locale`] — the
    /// primary bundle already covers everything it possibly can, so a
    /// separate identical fallback bundle would be pure waste.
    fallback: Option<FluentBundle<FluentResource>>,
}

impl Localization {
    /// Parses every file in `ftl_files` (relative to
    /// `<assets_root>/voxygen/i18n/<lang>/`) into one bundle. A file that
    /// fails to read/parse is skipped with a logged warning (degrade clean —
    /// never panic the whole HUD over one bad/missing localization file).
    fn build_bundle(lang: &LanguageIdentifier, ftl_files: &[&str]) -> FluentBundle<FluentResource> {
        let mut bundle = FluentBundle::new(vec![lang.clone()]);
        // Fluent wraps interpolated argument values in U+2068/U+2069 isolation
        // marks by default; disable it (legacy Veloren's i18n loader does the
        // same) so `tr_args` output is clean, matchable text. No-op for the
        // arg-less `tr`/`tr_attr` paths (nothing is interpolated there).
        bundle.set_use_isolating(false);
        let root = assets_root().join("voxygen/i18n").join(lang.to_string());
        for rel_path in ftl_files {
            let full_path = root.join(rel_path);
            match std::fs::read_to_string(&full_path) {
                Ok(source) => match FluentResource::try_new(source) {
                    Ok(resource) => {
                        if let Err(errors) = bundle.add_resource(resource) {
                            eprintln!(
                                "xindeler-ui i18n: {} had {} duplicate-message error(s), keeping \
                                 the first definition",
                                full_path.display(),
                                errors.len()
                            );
                        }
                    },
                    Err((_, errors)) => {
                        eprintln!(
                            "xindeler-ui i18n: {} failed to parse ({} error(s)) — its keys will \
                             fall back to their own name",
                            full_path.display(),
                            errors.len()
                        );
                    },
                },
                Err(err) => {
                    eprintln!(
                        "xindeler-ui i18n: cannot read {} ({err}) — its keys will fall back to \
                         their own name",
                        full_path.display()
                    );
                },
            }
        }
        bundle
    }

    /// Loads `lang`'s catalog (`ftl_files`, filenames relative to that
    /// locale's own `.ftl` directory — e.g. `"hud/settings.ftl"`, never a
    /// path already carrying a locale segment). If `lang` isn't
    /// [`fallback_locale`], ALSO loads the same file list from `en` as a
    /// fallback bundle, so an incomplete (or entirely missing) locale
    /// directory still resolves via `en` rather than showing raw keys — see
    /// the module doc for the exact resolve order.
    #[must_use]
    pub fn load(lang: &LanguageIdentifier, ftl_files: &[&str]) -> Self {
        let bundle = Self::build_bundle(lang, ftl_files);
        let fallback_lang = fallback_locale();
        let fallback = if *lang == fallback_lang {
            None
        } else {
            Some(Self::build_bundle(&fallback_lang, ftl_files))
        };
        Self {
            lang: lang.clone(),
            bundle,
            fallback,
        }
    }

    /// The locale this catalog was loaded for.
    #[must_use]
    pub fn lang(&self) -> &LanguageIdentifier { &self.lang }

    fn resolve(bundle: &FluentBundle<FluentResource>, key: &str) -> Option<String> {
        let message = bundle.get_message(key)?;
        let pattern = message.value()?;
        let mut errors = Vec::new();
        Some(
            bundle
                .format_pattern(pattern, None, &mut errors)
                .into_owned(),
        )
    }

    fn resolve_attr(
        bundle: &FluentBundle<FluentResource>,
        key: &str,
        attr: &str,
    ) -> Option<String> {
        let message = bundle.get_message(key)?;
        let pattern = message.get_attribute(attr)?.value();
        let mut errors = Vec::new();
        Some(
            bundle
                .format_pattern(pattern, None, &mut errors)
                .into_owned(),
        )
    }

    fn resolve_with_args(
        bundle: &FluentBundle<FluentResource>,
        key: &str,
        args: &FluentArgs,
    ) -> Option<String> {
        let message = bundle.get_message(key)?;
        let pattern = message.value()?;
        let mut errors = Vec::new();
        Some(
            bundle
                .format_pattern(pattern, Some(args), &mut errors)
                .into_owned(),
        )
    }

    /// Resolves `key`'s message VALUE: tries the active locale, then the `en`
    /// fallback bundle (if any), then falls back to `key` itself — never
    /// panics.
    #[must_use]
    pub fn tr(&self, key: &str) -> String {
        if let Some(value) = Self::resolve(&self.bundle, key) {
            return value;
        }
        if let Some(fallback) = &self.fallback
            && let Some(value) = Self::resolve(fallback, key)
        {
            return value;
        }
        key.to_owned()
    }

    /// Resolves `key`'s message VALUE with Fluent argument interpolation — the
    /// argument-carrying sibling of [`Self::tr`], same active-locale-then-`en`-
    /// fallback-then-bare-key order. Never panics. (`Content::Attr` carries no
    /// args, so its client resolves via the existing [`Self::tr_attr`]; there
    /// is deliberately no `tr_attr_args`.)
    #[must_use]
    pub fn tr_args(&self, key: &str, args: &FluentArgs) -> String {
        if let Some(value) = Self::resolve_with_args(&self.bundle, key, args) {
            return value;
        }
        if let Some(fallback) = &self.fallback
            && let Some(value) = Self::resolve_with_args(fallback, key, args)
        {
            return value;
        }
        key.to_owned()
    }

    /// Resolves `key`'s `attr` ATTRIBUTE (e.g. `buff-heal`'s `.desc`), with
    /// the same active-locale-then-`en`-fallback order as [`Self::tr`].
    /// Falls back to `"{key}.{attr}"` if neither resolves it — never panics.
    #[must_use]
    pub fn tr_attr(&self, key: &str, attr: &str) -> String {
        if let Some(value) = Self::resolve_attr(&self.bundle, key, attr) {
            return value;
        }
        if let Some(fallback) = &self.fallback
            && let Some(value) = Self::resolve_attr(fallback, key, attr)
        {
            return value;
        }
        format!("{key}.{attr}")
    }
}

/// The frozen `.ftl` files (relative to each locale's own `.ftl` directory)
/// this crate's plugin loads by default. Extend this list (never rename an
/// existing entry — isolation law) as later screens need more catalogs.
pub const DEFAULT_HUD_FTL_FILES: &[&str] = &[
    "buff.ftl",
    "hud/misc.ftl",
    "hud/sct.ftl",
    // BL-82 EM-5.16 (T56.44): the common/esc-menu/settings/main-menu catalogs
    // `settings_window.rs`/`esc_menu.rs`/`menu.rs` now resolve real keys from.
    "common.ftl",
    "esc_menu.ftl",
    "hud/settings.ftl",
    "main.ftl",
    // BL-82 EM-5.16 (T56.44 follow-up, full i18n screen coverage):
    // `gameinput.ftl` already carries a `gameinput-*` key for every
    // `GameInput` variant (ported from legacy `voxygen` 1:1) — `controls_
    // screen.rs` now resolves real per-action row labels from it instead of
    // its old `display_name()` camelCase-splitting heuristic.
    "gameinput.ftl",
    "hud/controls.ftl",
    // The remaining screens converted this same pass, each reusing (and, where
    // needed, extending) the legacy per-domain catalog already frozen under
    // `assets/voxygen/i18n/**` rather than inventing a parallel one:
    // `inventory_ui.rs` (bag/equipment slots), `diary.rs` (stats tab +
    // skill-tree tooltips — `hud/skills.ftl`'s 236 legacy lines were never
    // actually wired to this screen before this pass), `hud/char_window.rs`'s
    // catalog (diary stats-row labels), `chat.rs` (channel tabs),
    // `crafting_ui.rs`, `hud/map.ftl` + `hud/quest.ftl` (map_view.rs),
    // `hud/social.ftl` + `hud/group.ftl` (social_hud.rs), `trade_ui.rs`, and
    // `char_select.rs` (the character-creation wizard, top-level — not under
    // `hud/`, matching the legacy layout).
    "hud/bag.ftl",
    "hud/char_window.ftl",
    "hud/chat.ftl",
    "hud/crafting.ftl",
    "hud/map.ftl",
    "hud/quest.ftl",
    "hud/skills.ftl",
    // BL-82 EM-5.16 item D: the Diary skill-tree name lookup resolves its 89
    // weapon nodes through this catalog's `common-abilities-*` /
    // `veloren-core-pseudo_abilities-*` keys.
    "hud/ability.ftl",
    "hud/social.ftl",
    "hud/group.ftl",
    "hud/trade.ftl",
    "hud/combat_hud.ftl",
    "char_selection.ftl",
    // BL-82 EM-5.16 Phase 5 close-out: the `subtitle-*` overlay keys
    // `xindeler-client::subtitle_overlay` resolves (the toggle's own row
    // label, `hud-settings-subtitles`, lives in the already-registered
    // `hud/settings.ftl`).
    "hud/subtitles.ftl",
    // BL-82 EM-5.16 chat i18n interpolation: `command.ftl`'s `/command`
    // feedback keys (e.g. `players-list-header`) — the client resolves these
    // client-side (via `Localization::tr_args`) once `xindeler-sim-bridge`
    // projects a `Content::Localized` command-feedback line into a
    // `NetLocalizedContent` payload (see `xindeler-client::chat`).
    "command.ftl",
];

/// Tracks which BCP-47 tag the currently-loaded [`Localization`] catalog is
/// for. A plain `Resource` (not `NonSend` — a `String` is trivially
/// `Send + Sync`), so any system can gate on
/// `resource_changed::<CurrentLocale>()` — the same change-detection idiom
/// `settings_window`'s own `apply_graphics_settings` already uses for
/// `XindelerSettings` — without needing to touch the non-`Sync`
/// `Localization` bundle just to detect a change.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct CurrentLocale(pub String);

impl Default for CurrentLocale {
    /// Matches `XindelerSettings::language`'s own default (`"en"`) — kept as
    /// a literal here rather than a cross-crate import, since `xindeler-ui`
    /// has no dependency on `xindeler-app` (the isolation boundary the module
    /// doc's "who changes `CurrentLocale`" paragraph relies on).
    fn default() -> Self { Self("en".to_owned()) }
}

/// Tags a plain [`Text`] node whose content is a resolved `.ftl` message
/// VALUE — [`relocalize_text`] re-resolves it whenever [`CurrentLocale`]
/// changes. Screens that spawn a heading/note/label as bare `Text` (not a
/// [`crate::button::button_bundle`] label — see [`LocalizedLabel`] for that
/// case) tag it with this at spawn time; the initial spawn should still call
/// [`Localization::tr`] directly for the FIRST paint (this component only
/// handles LATER changes).
#[derive(Component, Debug, Clone, Copy)]
pub struct LocalizedText(pub &'static str);

/// Tags a [`crate::button::HudButtonLabel`]-carrying button whose label is a
/// resolved `.ftl` message VALUE — [`relocalize_button_labels`] re-resolves it
/// whenever [`CurrentLocale`] changes, mutating the button's
/// `HudButtonLabel`; `spawn_button_labels`'s own `Changed<HudButtonLabel>` arm
/// (see that system's doc comment) propagates the new text onto the actual
/// `Text` child.
#[derive(Component, Debug, Clone, Copy)]
pub struct LocalizedLabel(pub &'static str);

/// The ordering handle every system that CHANGES [`CurrentLocale`] (the
/// `xindeler-client` settings bridge, today the only one) must run
/// `.before(..)` so [`reload_localization_on_locale_change`] and the two
/// relocalize systems below see the fully up-to-date value in the SAME frame
/// the change happened, rather than one frame late.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocaleSyncSet;

/// Rebuilds the whole [`Localization`] catalog for [`CurrentLocale`]'s
/// current tag. First in [`LocaleSyncSet`] — the two relocalize systems below
/// read the bundle it just rebuilt, in the same frame.
pub fn reload_localization_on_locale_change(
    current_locale: Res<CurrentLocale>,
    mut localization: NonSendMut<Localization>,
) {
    let lang = parse_locale(&current_locale.0);
    if *localization.lang() == lang {
        return;
    }
    *localization = Localization::load(&lang, DEFAULT_HUD_FTL_FILES);
}

/// Re-resolves every [`LocalizedText`]-tagged `Text` node from the
/// just-reloaded [`Localization`] bundle.
pub fn relocalize_text(
    localization: NonSend<Localization>,
    mut texts: Query<(&LocalizedText, &mut Text)>,
) {
    for (tag, mut text) in &mut texts {
        let resolved = localization.tr(tag.0);
        if text.0 != resolved {
            text.0 = resolved;
        }
    }
}

/// Re-resolves every [`LocalizedLabel`]-tagged button's
/// [`crate::button::HudButtonLabel`] from the just-reloaded [`Localization`]
/// bundle.
pub fn relocalize_button_labels(
    localization: NonSend<Localization>,
    mut labels: Query<(&LocalizedLabel, &mut crate::button::HudButtonLabel)>,
) {
    for (tag, mut label) in &mut labels {
        let resolved = localization.tr(tag.0);
        if label.0 != resolved {
            label.0 = resolved;
        }
    }
}

#[cfg(test)]
mod tests {
    use unic_langid::langid;

    use super::*;

    /// Every `XINDELER_ASSETS`-touching scenario in ONE test function,
    /// deliberately consolidated rather than one `#[test]` per scenario:
    /// `cargo test` runs test functions in parallel by default, and separate
    /// functions each independently mutating the SAME process-wide env var
    /// would race each other the moment more than one existed. Every
    /// set/assert/remove step below runs sequentially within this one
    /// function, so there is no cross-thread data race on the var.
    #[test]
    fn i18n_multi_locale_and_fallback_behaviour() {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: see the function doc comment above.
        unsafe {
            std::env::set_var("XINDELER_ASSETS", dir.path());
        }

        // -- a resolvable key returns its real value; an unresolvable one
        // falls back to the key itself — never panics either way (the T56.3
        // acceptance bar, still true post-T56.44) --
        let en_dir = dir.path().join("voxygen/i18n/en");
        std::fs::create_dir_all(&en_dir).expect("mkdir en");
        std::fs::write(
            en_dir.join("test.ftl"),
            "hello-world = Hello, world!\n    .desc = A greeting.\nbye = Bye\ngreet = Hello, { \
             $name }!\n",
        )
        .expect("write en fixture");

        let en = Localization::load(&langid!("en"), &["test.ftl"]);
        assert_eq!(en.tr("hello-world"), "Hello, world!");
        assert_eq!(en.tr_attr("hello-world", "desc"), "A greeting.");
        assert_eq!(
            en.tr("totally-unknown-key"),
            "totally-unknown-key",
            "an unknown key must fall back to itself, never panic"
        );
        assert!(
            en.fallback.is_none(),
            "the en locale is its own fallback target — no separate bundle needed"
        );

        // -- `tr_args` interpolates Fluent arguments (the arg-less `tr`
        // cannot), with no U+2068/U+2069 isolation marks around the
        // interpolated value, and degrades to the bare key when the message
        // is missing — never panics --
        let mut args = FluentArgs::new();
        args.set("name", "world");
        assert_eq!(
            en.tr_args("greet", &args),
            "Hello, world!",
            "interpolated value must appear verbatim, with no isolation marks"
        );
        let empty_args = FluentArgs::new();
        assert_eq!(
            en.tr_args("totally-unknown-key", &empty_args),
            "totally-unknown-key",
            "a missing key must degrade to the bare key, never panic"
        );

        // -- a missing .ftl FILE degrades to a usable (empty) bundle rather
        // than panicking the whole HUD --
        let missing_file = Localization::load(&langid!("en"), &["does/not/exist.ftl"]);
        assert_eq!(missing_file.tr("anything"), "anything");

        // -- an entirely missing LOCALE directory (no `voxygen/i18n/es/` at
        // all yet) falls back to en wholesale --
        let unknown_locale = Localization::load(&langid!("es"), &["test.ftl"]);
        assert_eq!(
            unknown_locale.tr("hello-world"),
            "Hello, world!",
            "a locale with no directory at all must resolve every key via the en fallback"
        );

        // -- a PARTIAL locale (covers some but not all keys) falls back
        // per-key, not wholesale --
        let es_dir = dir.path().join("voxygen/i18n/es");
        std::fs::create_dir_all(&es_dir).expect("mkdir es");
        std::fs::write(es_dir.join("test.ftl"), "hello-world = ¡Hola, mundo!\n")
            .expect("write es fixture");
        let es = Localization::load(&langid!("es"), &["test.ftl"]);
        assert_eq!(
            es.tr("hello-world"),
            "¡Hola, mundo!",
            "a key the locale DOES cover must resolve to the locale's own text, not the fallback"
        );
        assert_eq!(
            es.tr("bye"),
            "Bye",
            "a key the locale does NOT cover must fall back to the en catalog's text"
        );
        assert_eq!(
            es.tr("totally-unknown-key"),
            "totally-unknown-key",
            "a key NEITHER catalog has must still fall back to the bare key, never panic"
        );

        // -- `language_name` reads the locale's OWN `_manifest.ron`, never a
        // hand-copied Rust const (the game-architecture-reviewer finding this
        // closes) --
        std::fs::write(
            en_dir.join("_manifest.ron"),
            "(metadata: (language_name: \"English\", language_identifier: \"en\"), fonts: {})",
        )
        .expect("write en manifest fixture");
        std::fs::write(
            es_dir.join("_manifest.ron"),
            "(metadata: (language_name: \"Español de España (Spanish - Spain)\", \
             language_identifier: \"es\"), fonts: {})",
        )
        .expect("write es manifest fixture");
        assert_eq!(language_name(&langid!("en")), "English");
        assert_eq!(
            language_name(&langid!("es")),
            "Español de España (Spanish - Spain)",
            "must read the manifest's REAL declared name, not a shortened guess"
        );
        assert_eq!(
            language_name(&langid!("xx")),
            "xx",
            "a locale with no _manifest.ron at all falls back to the bare tag, never panics"
        );

        // SAFETY: see the function doc comment above; leave the environment
        // clean for any later test in this binary.
        unsafe {
            std::env::remove_var("XINDELER_ASSETS");
        }
    }

    /// No shared env-var mutation — safe to run in parallel with the test
    /// above.
    #[test]
    fn parse_locale_falls_back_to_en_on_an_unparseable_tag() {
        assert_eq!(parse_locale("es"), langid!("es"));
        assert_eq!(parse_locale("zh-Hans"), langid!("zh-Hans"));
        assert_eq!(parse_locale("!!!not-a-locale!!!"), langid!("en"));
    }

    /// `hud/ability.ftl` must be in the default catalog: the Diary skill-tree
    /// name lookup (BL-82 EM-5.16 item D) resolves its 89 weapon nodes through
    /// `common-abilities-*` / `veloren-core-pseudo_abilities-*` keys that live
    /// only there.
    #[test]
    fn ability_ftl_is_loaded_and_resolves_a_weapon_key() {
        let l10n = Localization::load(&fallback_locale(), DEFAULT_HUD_FTL_FILES);
        // A representative weapon-ability key + the one naming-quirk key.
        for key in [
            "common-abilities-sword-heavy_sweep",
            "veloren-core-pseudo_abilities-sword-fell_strike",
            "common-abilities-staff-fireshockwave",
        ] {
            assert_ne!(l10n.tr(key), key, "{key} must resolve to real text");
        }
    }
}
