//! BL-82 EM-8.4 — structural guard against the recurring `NetOwnerOnly`/
//! `Scope` bug class (see [`crate::owner_visibility`]'s own doc comment on
//! [`crate::owner_visibility::NetOwnerOnly`]'s `VisibilityFilter` impl for
//! the full history: `NetSkillSet`/`NetAbilityPool`, EM-5.7, PR #96;
//! `NetCrafting`, EM-5.15, PR #172 — the SAME bug, found twice independently
//! after the first fix's own doc comment already warned it would recur).
//!
//! ## The bug class, one more time
//! A mirror system in `xindeler-sim-bridge` (`inventory.rs`/`trade.rs`/
//! `skillset.rs`/`crafting.rs`) tags a mirrored entity with
//! `NetOwnerOnly(owner_uid)` and separately inserts a private `Net*`
//! component (`NetInventory`/`NetCrafting`/…), often with a doc comment
//! claiming "self-scoped" or "owner-scoped via `NetOwnerOnly`" — but
//! `bevy_replicon`'s `VisibilityFilter::Scope` only actually hides a
//! component that is LITERALLY NAMED in `NetOwnerOnly`'s `Scope` tuple
//! (`owner_visibility.rs`). Tagging the entity does nothing on its own; the
//! private component still replicates to every client that can see the
//! entity at all unless its type is ALSO listed in `Scope`. This has shipped
//! as a real cross-client info-disclosure bug once (`NetCrafting`, PR #172)
//! after already being caught once before by review (`NetSkillSet`/
//! `NetAbilityPool`, PR #96/EM-5.7) — a 3rd occurrence of the identical
//! mismatch.
//!
//! ## Why a static source-scanner, not a compile-time guard
//! `NetOwnerOnly` is a single, non-generic marker component (it carries only
//! the owner `Uid`, never the protected component's type) inserted via a
//! bare `ec.insert(NetOwnerOnly(owner))` call, textually separate — often
//! tens of lines away, past its own dedup-cache branch — from the
//! `ec.insert(net_thing.clone())` call for the component it's meant to
//! protect (see any of the four `mirror_*_state` functions in
//! `xindeler-sim-bridge`). Making this a compile error would require either
//! (a) a proc macro that inspects `Scope`'s tuple contents at every
//! `NetOwnerOnly` call site — no stable Rust mechanism reads a sibling
//! `impl`'s associated-type tuple from a macro, so this would still need its
//! own hand-maintained call-site annotation, which is just as forgettable as
//! the bug it's meant to prevent — or (b) redesigning `NetOwnerOnly` to be
//! generic over the protected component type and funneling every insertion
//! through one generic helper bounded by a marker trait that only the
//! `Scope`-registered types implement. (b) is architecturally tidier but
//! does not actually close the hole: the marker-trait impls and the `Scope`
//! tuple would still be two independently-editable lists that can drift
//! apart exactly like today's doc-comment-vs-`Scope` mismatch does, AND it
//! would not catch a call site that (like all four real ones today) tags
//! `NetOwnerOnly` and inserts the private component as two separate
//! `ec.insert` calls rather than through the helper — nothing stops a
//! future author from skipping the generic helper entirely, and a skipped
//! helper produces no compiler diagnostic at all. Neither path is a real
//! compile-time guarantee without a proc macro that would itself become a
//! new maintenance burden; a static scanner test, mirroring
//! `xindeler-client::zlayer_audit`'s already-proven `ROOT_REGISTRY` pattern
//! for the analogous "missing `GlobalZIndex`" bug class, closes the SAME
//! shape of gap with no new dependency and a pattern this codebase already
//! understands and trusts.
//!
//! ## How the scan works
//! 1. Recursively read every `.rs` file under `xindeler-sim-bridge/src` (mirror
//!    systems live there exclusively today — confirmed by grepping the whole
//!    `bevy/` workspace for `NetOwnerOnly` at the time this audit was written;
//!    every OTHER file that mentions it does so only in a doc comment, never an
//!    actual `.insert` call).
//! 2. Strip comments and string/char-literal CONTENTS first (a naive brace
//!    counter would misfire on a `{`/`}` living inside a doc comment or a
//!    `format!("...{}...")` string) — see [`strip_comments_and_strings`].
//! 3. Extract every function BODY (via a comment-safe brace counter) that
//!    contains the literal text `NetOwnerOnly(` — i.e. every mirror system that
//!    tags an entity as owner-scoped.
//! 4. Within each such body, collect every `remove::<NetXxx>()` turbofish
//!    target starting with `Net` — `EntityCommands::remove::<T>()` requires `T:
//!    Bundle`, so this can only ever name a real top-level component (never a
//!    nested field type like `NetSkillGroup`/`NetHotbarSlot`, which aren't
//!    components and could not compile there). Every one of the four current
//!    mirror functions has exactly this shape: an early-out/absent branch that
//!    calls `ec.remove::<TheSameTypeItInserts>()` — this is the reliable "which
//!    component does this tagging protect" signal, chosen over a line-proximity
//!    guess precisely because the tagged component is usually inserted many
//!    lines away from the `NetOwnerOnly` tag itself (see the module doc comment
//!    above).
//! 5. Cross-check the resulting set against [`crate::owner_visibility`]'s own
//!    `Scope` tuple, parsed the same source-text way (no runtime reflection
//!    over an associated type's tuple contents is possible in Rust — the same
//!    limitation `zlayer_audit`'s own doc comment notes for `bevy_reflect` and
//!    "every `Component` type defined in this crate").
//! 6. Fail loudly, naming the missing type(s), if anything tagged is absent
//!    from `Scope`.
//!
//! ## Maintenance note for future authors
//! Adding a 6th (or Nth) `NetOwnerOnly`-tagged mirror? You don't need to
//! touch this file at all — the scan is fully automatic, unlike
//! `zlayer_audit`'s hand-maintained `ROOT_REGISTRY` (that one needs a
//! TopLevel/NestedChild judgment call this bug class doesn't have; every
//! owner-scoped type needs the exact same treatment: add it to `Scope`).
//! Just make sure your new `mirror_*_state` function's "component now
//! absent" branch calls `ec.remove::<YourNewNetType>()` as its OWN,
//! standalone turbofish call (the existing pattern every current mirror
//! already follows) — if it does, this test catches a forgotten `Scope`
//! entry automatically. **Do not** fold it into a tuple/batch removal like
//! `ec.remove::<(YourNewNetType, SomethingElse)>()` — [`removed_net_types`]
//! only extracts a single leading type name up to the next `>`/`,`, so a
//! tuple bundle's first char would be `(`, never matching `Net`, and your
//! new type would silently vanish from the scan (a false negative this
//! scanner cannot currently catch — keep every owner-scoped removal its own
//! statement). If your new mirror has no removal branch (the component is
//! never un-derived once present), add a one-line
//! `ec.remove::<YourNewNetType>()` call somewhere in its body purely so this
//! scanner has a signal to find. Either way, ALSO add a
//! `tests::your_type_is_owner_scoped_across_two_real_clients` regression
//! test in `owner_visibility.rs` alongside `skillset_is_owner_scoped_
//! across_two_real_clients`/`crafting_is_owner_scoped_across_two_real_
//! clients` — that is the ONLY thing that verifies a `Scope` entry actually
//! hides the component end to end; this scanner only verifies the `Scope`
//! *tuple* names it (see that module's own doc comment for why both checks
//! exist).

use std::{collections::BTreeSet, fs, path::Path};

/// Strips `//`/`/* */` comments (nested block comments handled) and the
/// CONTENTS of string literals (plain `"..."` and raw `r#"..."#`/`r"..."`,
/// escapes honored) from Rust source, replacing everything removed with a
/// space (newlines preserved) so a later brace/line count still lines up.
/// Necessary so the brace-counting function-body extractor below never
/// misfires on a `{`/`}` living inside a doc comment or a
/// `format!("...{}...")` string, and so the `remove::<Net...>()` search below
/// never matches inside a comment (e.g. a stray "// also
/// remove::<NetFoo>()" note). Deliberately does NOT special-case char
/// literals (`'x'`) — they're indistinguishable from a lifetime (`'a`)
/// without a much fuller tokenizer, and none of this codebase's mirror
/// functions contain one; only a `'{'`/`'}'` char literal could ever make
/// that gap matter here.
fn strip_comments_and_strings(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    let mut block_depth: u32 = 0;

    fn blank(out: &mut String, c: char) { out.push(if c == '\n' { '\n' } else { ' ' }); }

    while i < chars.len() {
        if block_depth > 0 {
            if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                block_depth += 1;
                out.push(' ');
                out.push(' ');
                i += 2;
            } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                block_depth -= 1;
                out.push(' ');
                out.push(' ');
                i += 2;
            } else {
                blank(&mut out, chars[i]);
                i += 1;
            }
            continue;
        }
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            block_depth = 1;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if chars[i] == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        // Raw string literal: r"...", r#"..."#, r##"..."##, ...
        if chars[i] == 'r' {
            let mut j = i + 1;
            let mut hashes = 0usize;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                for _ in i..=j {
                    out.push(' ');
                }
                i = j + 1;
                loop {
                    if i >= chars.len() {
                        break;
                    }
                    if chars[i] == '"' {
                        let close_start = i;
                        let mut k = i + 1;
                        let mut closing_hashes = 0usize;
                        while chars.get(k) == Some(&'#') && closing_hashes < hashes {
                            closing_hashes += 1;
                            k += 1;
                        }
                        if closing_hashes == hashes {
                            for _ in close_start..k {
                                out.push(' ');
                            }
                            i = k;
                            break;
                        }
                    }
                    blank(&mut out, chars[i]);
                    i += 1;
                }
                continue;
            }
        }
        if chars[i] == '"' {
            out.push(' ');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                blank(&mut out, chars[i]);
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Returns the source text of every `{ ... }` function body in `cleaned`
/// (already run through [`strip_comments_and_strings`]) whose opening brace
/// is the first `{` following an `fn ` keyword — i.e. every function
/// definition's own body, with any nested functions/closures/match arms
/// inside it included verbatim (this only needs to delimit the OUTER body so
/// a `remove::<...>()` call several match arms deep is still captured).
fn function_bodies(cleaned: &str) -> Vec<&str> {
    let mut bodies = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = cleaned[search_from..].find("fn ") {
        let fn_kw = search_from + rel;
        let Some(brace_rel) = cleaned[fn_kw..].find('{') else {
            break;
        };
        let open = fn_kw + brace_rel;
        let mut depth: i32 = 0;
        let mut end = None;
        for (offset, ch) in cleaned[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset + 1);
                        break;
                    }
                },
                _ => {},
            }
        }
        let Some(end) = end else { break };
        bodies.push(&cleaned[open..end]);
        search_from = end;
    }
    bodies
}

/// Every `remove::<NetXxx>()` turbofish target inside `body` — see the
/// module doc comment's "How the scan works" step 4 for why this specific
/// call shape is the reliable signal.
fn removed_net_types(body: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut search_from = 0;
    while let Some(rel) = body[search_from..].find("remove::<") {
        let start = search_from + rel + "remove::<".len();
        let Some(end) = body[start..].find(['>', ',']).map(|e| start + e) else {
            break;
        };
        let name = body[start..end].trim();
        if name.starts_with("Net") {
            found.insert(name.to_owned());
        }
        search_from = end.max(start + 1);
    }
    found
}

/// Scans every `.rs` file under `dir` (recursively) for a function body
/// containing a `NetOwnerOnly(` insertion, and returns the union of every
/// `remove::<NetXxx>()` type found inside such a body — see the module doc
/// comment's "How the scan works" for the full rationale.
fn find_owner_scoped_types(dir: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir({dir:?}): {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_owner_scoped_types(&path));
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read_to_string({path:?}): {e}"));
        let cleaned = strip_comments_and_strings(&contents);
        for body in function_bodies(&cleaned) {
            if body.contains("NetOwnerOnly(") {
                found.extend(removed_net_types(body));
            }
        }
    }
    found
}

/// Parses the literal `type Scope = ( ... );` tuple out of already-cleaned
/// (comments/strings stripped) `owner_visibility.rs` source text — the same
/// "read the source, don't try to reflect over the type system" move
/// `zlayer_audit`'s own doc comment explains for `ROOT_REGISTRY`; nothing in
/// Rust lets a test enumerate an associated type's tuple element types at
/// runtime.
fn scope_tuple_types(cleaned_source: &str) -> BTreeSet<String> {
    let marker = "type Scope = (";
    let start = cleaned_source
        .find(marker)
        .unwrap_or_else(|| panic!("no `{marker}` found in source"))
        + marker.len();
    let end = cleaned_source[start..]
        .find(");")
        .unwrap_or_else(|| panic!("no closing `);` found for the Scope tuple"))
        + start;
    cleaned_source[start..end]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real, end-to-end guard: every `Net*` component tagged
    /// `NetOwnerOnly` by a `xindeler-sim-bridge` mirror system (found via
    /// [`find_owner_scoped_types`]) must be named in `owner_visibility.rs`'s
    /// own `Scope` tuple. This is the automated version of the review catch
    /// that closed EM-5.7/EM-5.15 (see the module doc comment) — it must
    /// keep passing against today's correct code and fail the instant a 4th
    /// occurrence is introduced.
    #[test]
    fn every_owner_scoped_mirror_type_is_registered_in_scope() {
        let sim_bridge_src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../xindeler-sim-bridge/src");
        let found = find_owner_scoped_types(&sim_bridge_src);
        // Sanity: the scan itself must find something, or this test would
        // pass vacuously if the heuristic ever silently stopped matching
        // anything (e.g. after an unrelated refactor of the mirror files'
        // shape changed away from the `remove::<T>()` signal it looks for).
        assert!(
            !found.is_empty(),
            "found NO NetOwnerOnly-tagged type in xindeler-sim-bridge/src — the scan heuristic in \
             `owner_scope_audit.rs` may have broken (see its module doc comment for the exact \
             signal it looks for: a `remove::<NetXxx>()` call inside a function body that also \
             contains `NetOwnerOnly(`)"
        );

        let owner_visibility_src = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/owner_visibility.rs"),
        )
        .expect("owner_visibility.rs exists in this crate");
        let cleaned = strip_comments_and_strings(&owner_visibility_src);
        let registered = scope_tuple_types(&cleaned);

        let missing: Vec<&String> = found.iter().filter(|t| !registered.contains(*t)).collect();
        assert!(
            missing.is_empty(),
            "found xindeler-sim-bridge mirror type(s) tagged `NetOwnerOnly` with NO matching \
             entry in `owner_visibility.rs`'s `VisibilityFilter::Scope` tuple: {missing:?} — this \
             is EXACTLY the EM-5.7/EM-5.15 bug class (see that file's doc comment): tagging an \
             entity with `NetOwnerOnly` does nothing on its own, only listing the type in `Scope` \
             actually hides it from non-owning clients. Add the missing type(s) to `Scope` before \
             merging."
        );
    }

    /// Proves the scanner is a real tripwire, not just "current code
    /// passes": builds a synthetic mirror file (same shape as the real
    /// `mirror_crafting_state`, renamed so it can never collide with a real
    /// registered type) in a throwaway temp directory, tags a fake type with
    /// `NetOwnerOnly`, and confirms [`find_owner_scoped_types`] finds it
    /// while a `Scope`-tuple text that DOESN'T list it fails the same
    /// missing-entry check the real test above performs — i.e. reintroducing
    /// the exact PR #172 mistake (tag without registering) is caught.
    #[test]
    fn reintroducing_the_tag_without_scope_registration_is_caught() {
        let dir = std::env::temp_dir().join(format!(
            "xindeler_owner_scope_audit_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp scan dir");

        // A deliberate reintroduction of the EM-5.15/PR #172 scenario:
        // `NetFakeCrafting` is tagged `NetOwnerOnly` in the exact shape
        // `mirror_crafting_state` uses (tag first, insert/remove the private
        // component many lines later in the same function).
        let buggy_mirror_source = "
            pub fn mirror_fake_state(mut commands: Commands) {
                for (&sim_entity, &bevy_entity) in mirror.0.iter() {
                    let mut ec = commands.entity(bevy_entity);
                    if cache.owner.get(&sim_entity) != Some(&owner) {
                        ec.insert(NetOwnerOnly(owner));
                        cache.owner.insert(sim_entity, owner);
                    }
                    match presences.contains(sim_entity) {
                        true => {
                            let net_fake = build_net_fake(inventory);
                            ec.insert(net_fake.clone());
                        },
                        false => {
                            ec.remove::<NetFakeCrafting>();
                        },
                    }
                }
            }
        ";
        fs::write(dir.join("fake_mirror.rs"), buggy_mirror_source).expect("write fake mirror file");

        let found = find_owner_scoped_types(&dir);
        fs::remove_dir_all(&dir).ok();
        assert!(
            found.contains("NetFakeCrafting"),
            "scanner failed to find the deliberately-tagged NetFakeCrafting in the synthetic \
             mirror file — the heuristic itself is broken, not just the (expected-empty) real \
             Scope check below"
        );

        // A `Scope` tuple text that reproduces the bug: it lists every OTHER
        // real type but forgets the new one — exactly PR #172's mistake.
        let buggy_scope_source = "
            type Scope = (
                NetInventory,
                NetTrade,
            );
        ";
        let registered = scope_tuple_types(buggy_scope_source);

        let missing: Vec<&String> = found.iter().filter(|t| !registered.contains(*t)).collect();
        assert!(
            !missing.is_empty() && missing.iter().any(|t| t.as_str() == "NetFakeCrafting"),
            "the guard did not trip on a deliberately reintroduced missing-Scope-entry bug — this \
             test is supposed to PROVE the tripwire fires, not just that today's real code is \
             clean"
        );
    }

    /// [`strip_comments_and_strings`] must not let a brace hiding inside a
    /// comment or string confuse the function-body extractor — the exact
    /// failure mode a naive brace counter (like an early draft of this
    /// audit) would hit against real doc comments in this crate (e.g.
    /// `owner_visibility.rs`'s own module doc comment contains stray `{`/`}`
    /// via inline code-spans and Markdown link syntax).
    #[test]
    fn comment_and_string_braces_do_not_confuse_the_body_extractor() {
        let source = r#"
            /// a doc comment with a stray { brace and a "quoted { thing }"
            fn confusing() {
                let _ = "a string with { braces } inside";
                let _ = format!("{}", 1);
                ec.insert(NetOwnerOnly(owner));
                ec.remove::<NetWeird>();
            }
        "#;
        let cleaned = strip_comments_and_strings(source);
        let bodies = function_bodies(&cleaned);
        assert_eq!(
            bodies.len(),
            1,
            "expected exactly one function body: {bodies:?}"
        );
        assert!(bodies[0].contains("NetOwnerOnly("));
        assert_eq!(
            removed_net_types(bodies[0]),
            BTreeSet::from(["NetWeird".to_owned()])
        );
    }
}
