//! BL-82 holistic-review prevention measure — the `*Root` z-index audit
//! (bevy-migration-reviewer's suggested guard against a 5th/6th recurrence
//! of the "missing `GlobalZIndex`" bug class already fixed independently
//! across this session: `EscMenuRoot` (`esc_menu.rs`), `InventoryWindowRoot`
//! (`inventory_ui.rs`), `FullMapRoot` (`map_view.rs`), `InviteRoot`/
//! `TradeWindowRoot` (`trade_ui.rs`), `SocialWindowRoot`/`InviteBannerRoot`/
//! `DialoguePanelRoot` (`social_hud.rs`), and `BuffStripRoot`
//! (`combat_hud.rs`, caught BY this very audit while writing it — see its
//! own regression test in `combat_hud.rs`) — every one a top-level,
//! absolutely-positioned HUD panel spawned with NO `GlobalZIndex` at all,
//! silently defaulting to z-partition 0 and sitting below whichever
//! always-on ambient chrome happened to have one, causing real
//! click-routing/occlusion bugs Matías hit live.
//!
//! ## Why a hand-maintained registry, not reflection
//! Rust has no runtime reflection over "every `Component` type defined in
//! this crate" — `bevy_reflect` can enumerate REGISTERED types, but nothing
//! forces a marker component to register itself, and most of these `*Root`
//! structs are private to their own module, so a crate-level test can't
//! even name their type to query an `App` for one. The next-best guard:
//! statically scan every `.rs` file under `src/` for a `struct <Name>Root`
//! DEFINITION (source text, not the type system) and cross-check the found
//! names against [`ROOT_REGISTRY`] below — a hand-maintained list recording,
//! for every root, whether it's a TOP-LEVEL panel (needs its own
//! `GlobalZIndex`, verified by that file's own dedicated regression test —
//! e.g. `trade_ui.rs`'s `invite_and_trade_window_roots_carry_the_modal_
//! windows_z_index`) or a NESTED child of an already-indexed root (exempt —
//! a child inherits its ancestor's paint order; only a node spawned as a
//! direct, independently-positioned panel needs its own tier).
//!
//! **This test cannot itself verify a `GlobalZIndex` value** (see above) —
//! that's each top-level root's own file-local regression test's job. What
//! it DOES guarantee: adding a new `struct FooRoot` anywhere in this crate
//! without adding a matching [`ROOT_REGISTRY`] entry fails THIS test
//! immediately, forcing a conscious "does this need a `GlobalZIndex`?"
//! decision before the PR merges, rather than a silent recurrence.
//!
//! ## Maintenance note for future authors
//! Adding a new `*Root` marker component? Add ONE entry to
//! [`ROOT_REGISTRY`] below:
//! - **Top-level panel** (spawned directly via `commands.spawn`,
//!   independently/absolutely positioned, NOT nested inside another root's
//!   `with_children`/`parent.spawn`): give it an explicit
//!   `GlobalZIndex(zlayer::…)` at its spawn site AND a regression test in its
//!   own file (copy any existing `*_carries_the_*_z_index` test), then register
//!   it here as [`RootKind::TopLevel`].
//! - **Nested child** of an already-indexed root (spawned via
//!   `parent.spawn`/`.with_children` inside another `*Root`'s own subtree): no
//!   `GlobalZIndex` needed — register it here as [`RootKind::NestedChild`]
//!   naming its parent, so the NEXT auditor can see at a glance why it's exempt
//!   instead of re-litigating it.

use std::{fs, path::Path};

/// Whether a `*Root` marker needs its own [`bevy::ui::GlobalZIndex`]. See
/// the module doc comment's "Maintenance note" for how to pick one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootKind {
    /// Spawned as an independently-positioned top-level panel — must carry
    /// an explicit `GlobalZIndex`, verified by a dedicated regression test
    /// in its own file (not by this audit, which only checks source text).
    TopLevel,
    /// Spawned as a child inside another `*Root`'s own `with_children`
    /// subtree — inherits that ancestor's paint order, so no `GlobalZIndex`
    /// of its own is needed. `parent` names that ancestor for traceability.
    NestedChild { parent: &'static str },
}

/// The full roster of `*Root` marker components in this crate, as of this
/// audit (BL-82 holistic-review pass). See the module doc comment for how
/// to extend this when adding a new one.
const ROOT_REGISTRY: &[(&str, RootKind)] = &[
    // boss_nameplate.rs
    ("NameplateRoot", RootKind::TopLevel), // GlobalZIndex(BOSS_NAMEPLATE)
    // char_preview.rs
    ("PreviewFigureRoot", RootKind::NestedChild {
        parent: "N/A — not a UI node",
    }), // BL-82 EM-5.14 follow-up: this "Root" is a plain 3D `Transform`
    // entity (no `Node`/`GlobalZIndex` at all) — the char-select 3D preview
    // figure, rendered by its OWN dedicated offscreen `Camera3d` into a
    // render-to-texture `Image` (shown elsewhere via an `ImageNode` that
    // lives in `CharSelectRoot`'s own UI subtree, already covered by ITS
    // z-index). It never enters the main window's `bevy_ui` stacking
    // context, so it categorically cannot cause the click-routing/occlusion
    // bug this audit exists to catch; the "Root" suffix here only narrows
    // `spin_preview_figure`'s query to a single entity (see
    // `char_preview.rs`'s own doc comment on the struct). Registered instead
    // of silently exempting it so a future auditor sees the reasoning rather
    // than re-litigating it.
    // char_select.rs
    ("CharSelectRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    // chat.rs
    ("ChatPanelRoot", RootKind::TopLevel), // GlobalZIndex(CHAT)
    // combat_hud.rs
    ("DeathScreenRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    ("BuffStripRoot", RootKind::TopLevel),   // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP)
    // controls_screen.rs
    ("ControlsScreenRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    // crafting_ui.rs
    ("CraftingWindowRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    ("CategoryBarRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("ModularPrimaryListRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("ModularSecondaryListRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("ModularTabRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("RecipeDetailRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("RecipeListRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("RecipesTabRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("RepairListRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("RepairTabRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("SalvageListRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    ("SalvageTabRoot", RootKind::NestedChild {
        parent: "CraftingWindowRoot",
    }),
    // diary.rs
    ("DiaryWindowRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    ("StatsPanelRoot", RootKind::NestedChild {
        parent: "DiaryWindowRoot",
    }),
    ("TreeRoot", RootKind::NestedChild {
        parent: "DiaryWindowRoot",
    }),
    ("AbilitiesPanelRoot", RootKind::NestedChild {
        parent: "DiaryWindowRoot",
    }),
    // esc_menu.rs
    ("EscMenuRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    // inventory_ui.rs
    ("InventoryWindowRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    ("BagGridRoot", RootKind::NestedChild {
        parent: "InventoryWindowRoot",
    }),
    ("PaperdollRoot", RootKind::NestedChild {
        parent: "InventoryWindowRoot",
    }),
    ("EquipPickerRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS_STACKED)
    ("EquipPickerContentRoot", RootKind::NestedChild {
        parent: "EquipPickerRoot",
    }),
    // map_view.rs
    ("MinimapPanelRoot", RootKind::TopLevel), // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP)
    ("ObjectivesRoot", RootKind::TopLevel),   // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP)
    ("FullMapRoot", RootKind::TopLevel),      // GlobalZIndex(MODAL_WINDOWS)
    // menu.rs
    ("ConnectingRoot", RootKind::TopLevel), // GlobalZIndex(TOAST + 100) — above every
    // HUD layer so it fully covers the (still loading) gameplay chrome that
    // spawns behind it.
    ("MenuRoot", RootKind::TopLevel), // GlobalZIndex(TOAST + 100) — same tier;
    // the main menu is the only thing on screen at that point, but stays
    // consistent with its sibling `ConnectingRoot`.
    // tutorial_overlay.rs (BL-82 EM-5.16a): the first-run tutorial overlay's
    // modal backdrop, same shape as `SettingsWindowRoot` below (registered by
    // the EM-5.10a pass that found ITS pre-existing gap).
    ("TutorialOverlayRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    // social_hud.rs
    ("SocialWindowRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS) — participates in
    // HudState's mutually-exclusive window slot + cursor-free rule, same as
    // Inventory/Diary/Map (see `social_hud.rs`'s own spawn-site comment).
    ("PlayerListRoot", RootKind::NestedChild {
        parent: "SocialWindowRoot",
    }),
    ("GroupPanelRoot", RootKind::TopLevel), // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP) —
    // always-on ambient chrome, never claims the HudState window slot.
    ("GroupMembersRoot", RootKind::NestedChild {
        parent: "GroupPanelRoot",
    }),
    ("InviteBannerRoot", RootKind::TopLevel), // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP) —
    // ambient, resource-driven like its sibling GroupPanelRoot.
    ("DialoguePanelRoot", RootKind::TopLevel), // GlobalZIndex(ORBS_ACTION_BAR_PARTY_MINIMAP) —
    // ambient, resource-driven (ActiveDialogue), never claims the HudState
    // window slot either.
    ("DialogueResponsesRoot", RootKind::NestedChild {
        parent: "DialoguePanelRoot",
    }),
    // trade_ui.rs
    ("InviteRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    ("TradeWindowRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
    ("MyOfferGridRoot", RootKind::NestedChild {
        parent: "TradeWindowRoot",
    }),
    ("TheirOfferGridRoot", RootKind::NestedChild {
        parent: "TradeWindowRoot",
    }),
    // settings_window.rs — pre-existing gap found while verifying T56.34
    // (BL-82 EM-5.10a): `SettingsWindowRoot` (EM-5.12, PR #169) was never
    // added here, unrelated to the audio work in this PR. Same
    // full-screen-modal-backdrop shape as `EscMenuRoot`/`InventoryWindowRoot`
    // above (`GlobalZIndex(zlayer::MODAL_WINDOWS)` at its spawn site,
    // `spawn_settings_window`).
    ("SettingsWindowRoot", RootKind::TopLevel), // GlobalZIndex(MODAL_WINDOWS)
];

/// Scans every `.rs` file under `dir` (recursively) for a top-level `struct
/// <Name>Root` DEFINITION (not a reference/query/use of one) and returns
/// every distinct name found. A minimal hand-rolled scan (no `syn`/regex
/// dependency needed) — this crate's `struct Foo;`/`struct Foo(...)`
/// definitions are consistently written as a private (never `pub`) `struct
/// <Ident>` at the very start of a (post-`derive`-attribute) line, which is
/// all this needs to match; confirmed against every current `*Root`
/// definition when this audit was written.
fn find_root_struct_names(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir({dir:?}): {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_root_struct_names(&path));
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read_to_string({path:?}): {e}"));
        for line in contents.lines() {
            let Some(rest) = line.trim_start().strip_prefix("struct ") else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if name.len() > "Root".len() && name.ends_with("Root") {
                found.push(name);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// See the module doc comment: every `*Root` struct actually defined in
    /// this crate's `src/` must have exactly one [`ROOT_REGISTRY`] entry —
    /// catches both a NEW root added without registering it (the failure
    /// mode this test exists to prevent) and a STALE registry entry for a
    /// root that was renamed/removed (keeps the registry itself honest).
    #[test]
    fn every_root_struct_is_registered_exactly_once() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found: BTreeSet<String> = find_root_struct_names(&src_dir).into_iter().collect();

        let registered: BTreeSet<&str> = ROOT_REGISTRY.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            registered.len(),
            ROOT_REGISTRY.len(),
            "ROOT_REGISTRY has a duplicate entry — each *Root name must appear exactly once"
        );

        let found_names: BTreeSet<&str> = found.iter().map(String::as_str).collect();

        let missing: Vec<&str> = found_names.difference(&registered).copied().collect();
        assert!(
            missing.is_empty(),
            "found *Root struct(s) with NO ROOT_REGISTRY entry: {missing:?} — add one to \
             `zlayer_audit.rs`'s ROOT_REGISTRY (see this module's doc comment for how to classify \
             TopLevel vs NestedChild) before merging; a TopLevel root also needs an explicit \
             `GlobalZIndex(zlayer::…)` at its spawn site plus its own regression test"
        );

        let stale: Vec<&str> = registered.difference(&found_names).copied().collect();
        assert!(
            stale.is_empty(),
            "ROOT_REGISTRY has entry/entries for a *Root struct that no longer exists in src/: \
             {stale:?} — remove the stale entry"
        );
    }
}
