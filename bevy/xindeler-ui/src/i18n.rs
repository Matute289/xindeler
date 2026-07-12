//! BL-82 EM-5.1 T56.3 — the i18n seam: a thin in-house loader over the real
//! `fluent`/`unic-langid` crates (spec §4: "prefer a thin in-house fluent
//! loader over a lagging dep — the `.ftl` parsing crate `fluent` itself is
//! engine-agnostic and stable"), reusing the SAME frozen `.ftl` assets under
//! `assets/voxygen/i18n/` the legacy client uses verbatim — never a new
//! catalog, never a renamed path (isolation law).
//!
//! v1 scope (§9 Q6 gates FULL coverage; the SEAM itself ships regardless,
//! per spec §2 EM-5.1 point 3): loads a caller-chosen set of `.ftl` files
//! into one bundle for the `en` locale, and resolves message/attribute
//! lookups. A missing key resolves visibly to the key itself rather than
//! panicking — "no screen hardcodes English" is the goal, but a missing
//! translation must never crash the HUD.
//!
//! [`Localization`] is stored as a Bevy `NonSend` resource (not a `Resource`):
//! `fluent::FluentBundle`'s default memoizer is not `Sync`, and this crate has
//! no need to fight that — HUD text resolution is a main-thread-only concern
//! anyway, exactly like `xindeler-sim-bridge::SimServer`'s own `NonSend`
//! posture for a similarly non-`Sync` embedded type.

use std::path::PathBuf;

use fluent::{FluentBundle, FluentResource};
use unic_langid::langid;

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

/// A loaded `.ftl` catalog for one locale. v1 is `en`-only (§9 Q6 gates full
/// multi-language coverage); the type itself is locale-agnostic (a future
/// `Localization::load(lang, files)` swaps the `langid!("en")` below for a
/// caller-supplied one without any other change).
pub struct Localization {
    bundle: FluentBundle<FluentResource>,
}

impl Localization {
    /// Parses every file in `ftl_paths` (relative to `assets_root()`) into
    /// one bundle. A file that fails to read/parse is skipped with a logged
    /// warning (degrade clean — never panic the whole HUD over one bad/
    /// missing localization file), so callers always get SOME bundle back,
    /// even if some keys in it resolve to their own fallback.
    #[must_use]
    pub fn load(ftl_paths: &[&str]) -> Self {
        let mut bundle = FluentBundle::new(vec![langid!("en")]);
        let root = assets_root();
        for rel_path in ftl_paths {
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
        Self { bundle }
    }

    /// Resolves `key`'s message VALUE. Falls back to `key` itself if the
    /// message is missing or has no value — never panics.
    #[must_use]
    pub fn tr(&self, key: &str) -> String {
        let Some(message) = self.bundle.get_message(key) else {
            return key.to_owned();
        };
        let Some(pattern) = message.value() else {
            return key.to_owned();
        };
        let mut errors = Vec::new();
        self.bundle
            .format_pattern(pattern, None, &mut errors)
            .into_owned()
    }

    /// Resolves `key`'s `attr` ATTRIBUTE (e.g. `buff-heal`'s `.desc`). Falls
    /// back to `"{key}.{attr}"` if missing — never panics.
    #[must_use]
    pub fn tr_attr(&self, key: &str, attr: &str) -> String {
        let Some(message) = self.bundle.get_message(key) else {
            return format!("{key}.{attr}");
        };
        let Some(pattern) = message.get_attribute(attr).map(|a| a.value()) else {
            return format!("{key}.{attr}");
        };
        let mut errors = Vec::new();
        self.bundle
            .format_pattern(pattern, None, &mut errors)
            .into_owned()
    }
}

/// The frozen HUD-relevant `.ftl` files (relative to the assets root) this
/// crate's plugin loads by default. Extend this list (never rename an
/// existing entry — isolation law) as later screens need more catalogs.
pub const DEFAULT_HUD_FTL_FILES: &[&str] = &[
    "voxygen/i18n/en/buff.ftl",
    "voxygen/i18n/en/hud/misc.ftl",
    "voxygen/i18n/en/hud/sct.ftl",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolvable key returns its real translated value; an unresolvable
    /// key falls back to the key itself — never panics either way (the
    /// T56.3 acceptance bar).
    #[test]
    fn resolves_known_keys_and_falls_back_visibly_for_unknown_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ftl_path = dir.path().join("test.ftl");
        std::fs::write(
            &ftl_path,
            "hello-world = Hello, world!\n    .desc = A greeting.\n",
        )
        .expect("write fixture");

        // SAFETY: this test is the sole reader/writer of `XINDELER_ASSETS`
        // in this crate's test suite, and every set/assert/remove step runs
        // sequentially within this one test function.
        unsafe {
            std::env::set_var("XINDELER_ASSETS", dir.path());
        }
        let localization = Localization::load(&["test.ftl"]);
        unsafe {
            std::env::remove_var("XINDELER_ASSETS");
        }

        assert_eq!(localization.tr("hello-world"), "Hello, world!");
        assert_eq!(localization.tr_attr("hello-world", "desc"), "A greeting.");
        assert_eq!(
            localization.tr("totally-unknown-key"),
            "totally-unknown-key",
            "an unknown key must fall back to itself, never panic"
        );
    }

    /// A missing `.ftl` file degrades to an empty (but usable) bundle rather
    /// than panicking the whole HUD.
    #[test]
    fn missing_ftl_file_degrades_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("XINDELER_ASSETS", dir.path());
        }
        let localization = Localization::load(&["does/not/exist.ftl"]);
        unsafe {
            std::env::remove_var("XINDELER_ASSETS");
        }
        assert_eq!(localization.tr("anything"), "anything");
    }
}
