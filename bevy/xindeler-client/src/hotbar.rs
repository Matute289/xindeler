//! BL-82 EM-5.3 — the skillbar/hotbar screen, extending EM-5.2's core combat
//! HUD to full parity: real drag-to-assign, real keybind labels sourced from
//! EM-5.11's `xindeler-input` keymap, and cooldown greying/wipe reading the
//! `xindeler-sim-bridge::hotbar` mirror.
//!
//! ## BL-82 HUD redesign round 6 — no more ornate action-bar frame art
//! `action_bar_bg_left.png`/`action_bar_bg_right.png` (the ornate
//! bronze/spiked frame pieces this screen used to render behind each slot
//! row) are gone entirely, per Matías's `hud-ejemplo-2.png` reference (a
//! Diablo-4-fan-art HUD with no frame art around the ability icons at all —
//! just individual square icons sitting directly between the two large
//! circular orbs). See `hud_layout.rs`'s own top-of-file doc comment for the
//! full rationale; [`spawn_slot_row_half`] is now a bare transparent flex
//! container instead of an `ImageNode`-backed piece.
//!
//! ## BL-82 HUD overhaul (this pass) — 2×3 grid per side, 12 slots, shrunk
//! Three coupled changes on Matías's ask (he explicitly wanted an Opus-level
//! plan given the scope), superseding the "5+5 flat row" scheme documented
//! below (kept for the still-relevant machinery it explains):
//! 1. **Empty-slot stone cue, not a black box.** Round 7's opaque near-black
//!    full-box [`SkillSlotBackground`] plate is gone (Matías found it too
//!    heavy); slots are transparent again (terrain through the ring notches is
//!    now acceptable), and only the EMPTY state carries a grey-stone centre cue
//!    — see [`SkillSlotBackground`]/[`sync_empty_slot_backgrounds`].
//! 2. **Substantially smaller cluster.** [`SLOT_SIZE_PX`] `58 -> 42` and
//!    `hud_layout::ORB_SIZE_PX` `160 -> 112`, bringing the whole bottom-centre
//!    cluster closer to Veloren's thin bottom strip (the minimap is untouched).
//! 3. **2×3 grid per side, 12 slots.** Each half is a
//!    [`SLOT_COLS_PER_HALF`]×[`SLOT_ROWS_PER_HALF`] grid (6 slots), 12 total,
//!    keyed per Matías's exact map — see [`SLOT_INPUTS`].
//!
//! ## Slot count — HOLDERS always rendered, split evenly (BL-82 EM-5.17
//! ## Phase 3: "5+5 slot-holders" follow-up; now 6+6 per the overhaul above)
//! Matías's original ask (under-scoped by the Phase 0/2 bugfixes above, which
//! only fixed rendering bugs without changing the *count*): the hotbar must
//! show a full grid of skill-holder slots on each half — now
//! [`SLOTS_PER_HALF`] on the LEFT and [`SLOTS_PER_HALF`] on the RIGHT
//! ([`HOTBAR_SLOT_COUNT`] total), one per drag-drop ability-slot address, each
//! independently rebindable regardless of which `GameInput` labels it shows
//! (see the "Keybind LABELS" section below; `Primary`/`Secondary` i.e. M1/M2
//! as a SEPARATE pair of fixed, non-draggable indicators are unrelated — see
//! below).
//!
//! [`HOTBAR_SLOT_COUNT`] slot HOLDERS are now spawned UNCONDITIONALLY,
//! independent of `xindeler_protocol::NetAbilities::slots.len()` — which is
//! only however many auxiliary-ability slots the sim currently grants the
//! local player (`ActiveAbilities::limit`, hardcoded to
//! `common::comp::ability::BASE_ABILITY_LIMIT == 5` for every player
//! character at creation today, `server/src/character_creator.rs` +
//! `server/src/state_ext.rs` — there is no skill/level/perk path that raises
//! it yet). Indices `< abilities.slots.len()` show that slot's real content
//! (icon glyph/tooltip/cooldown/drag-drop, unchanged from before); indices
//! `>= abilities.slots.len()` render as an EMPTY placeholder holder — same
//! chromeless frame + border art, no icon/tooltip, and (already, for free)
//! harmless as a drag-drop target: `xindeler-sim-bridge`'s
//! `ChangeAbilityEvent` handler (`common::comp::ability::ActiveAbilities::
//! change_ability`, via `Vec::get_mut`) silently no-ops for any slot index
//! `>= limit`, so a drop onto a placeholder simply does nothing server-side,
//! not a crash or a mis-bind. This reserves the full 12-slot visual budget
//! today and needs no further client change the day a game-design lever
//! (skill tree, class perk, etc.) raises `ActiveAbilities::limit` past 5 —
//! the newly-real slots just start showing content, the layout already fits
//! them.
//!
//! [`sync_slot_half_parenting`] splits the FIXED [`HOTBAR_SLOT_COUNT`] (not
//! the sim-reported count) via `div_ceil(2)`, so it is always an exact even
//! split ([`SLOTS_PER_HALF`] each side) — previously, with only
//! `abilities.slots.len()` entities existing, the same `div_ceil` math gave an
//! uneven split, which is the literal bug the "5+5 slot-holders" phase fixed.
//!
//! M1/M2 (primary/secondary) remain separate, non-draggable indicators — the
//! sim's `PrimaryAbility`/`SecondaryAbility` are fixed to "whatever's
//! wielded", not user-rebindable, so there is no slot address for them; this
//! phase does not touch [`sync_primary_secondary_indicators`].
//!
//! ## Keybind LABELS — Matías's exact 2×3 map (BL-82 HUD overhaul)
//! [`SLOT_INPUTS`] maps each slot index (row-major within each half) to the
//! `GameInput` whose live binding its badge shows. The overhaul (this pass)
//! fixes the map to Matías's exact spec — left block `1,2,3 / 4,5,6`, right
//! block `7,8,LMB / 9,0,RMB` — so the two mouse buttons
//! (`GameInput::Primary`/`Secondary`) now sit at indices `8`/`11` (rightmost
//! column of each right-block row), not the trailing two. The numbers come
//! from the live [`KeyMap`] (`Slot1..Slot9` -> `1..9`, `Slot10` -> `0`, which
//! this pass rebinds onto `Digit0` in `keybind.rs` so the badge is truthful).
//! This stays a pure LABEL map: nothing today triggers a hotbar slot FROM a
//! `GameInput` press (documented future step — [`SLOT_INPUTS`] only feeds
//! [`sync_keybind_labels`]'s keymap lookup), so reusing `Primary`/`Secondary`
//! here can't double-fire against the separate M1/M2 indicators (those read
//! the WIELDED weapon's ability id, not this array).
//!
//! The two mouse-button holders render Matías's dedicated
//! `mouse_click_left.png`/`mouse_click_right.png` art (a real
//! [`HotbarKeybindIcon`] `ImageNode`, not `"LMB"`/`"RMB"` text) — the
//! `Primary`/`Secondary` entries select WHICH icon to show
//! ([`sync_hotbar_slots`]'s spawn-time `match`), never resynced from a live
//! rebind (see [`HotbarKeybindIcon`]'s own doc comment).
//!
//! ## Real drag-to-assign, todays scope
//! The `xindeler-ui::slot` drag-drop primitive is wired end-to-end: dragging
//! one hotbar slot onto another swaps their bindings via TWO
//! `AssignHotbarSlot` client messages (the real replicon wire message,
//! consumed server-side by `xindeler-sim-bridge::hotbar::
//! apply_hotbar_assignment_requests` — BL-82 EM-5.3 follow-up: this used to
//! write a listen-server-only `LocalAssignHotbarSlot` shortcut instead,
//! which silently dropped every real remote client's rebind request on a
//! dedicated server; writing the real client message here works
//! identically for both a listen-server's own embedded player, via
//! `bevy_replicon`'s local echo, AND a genuinely-remote client). Today the
//! only drag SOURCE is another hotbar slot
//! (EM-5.6's inventory/EM-5.7's diary — the item/ability sources the spec
//! names — haven't landed yet); a drop whose `from`/`to` groups don't both
//! equal [`HOTBAR_GROUP`] is ignored, not silently mis-applied. Once those
//! screens exist, they reuse the SAME [`xindeler_ui::slot::SlotDropped`]
//! event with their own [`xindeler_ui::slot::SlotGroup`] — no rework needed
//! here.
//!
//! ## Cooldown "sweep" (a documented v1 simplification)
//! The sim's `AbilityCooldowns` only carries the absolute ready-at time, not
//! the original cooldown duration (`xindeler_protocol::NetCooldownEntry`'s
//! own doc comment) — [`sync_cooldown_overlays`]'s own `Local<HashMap<String,
//! f32>>` infers a per-ability "total" as the largest `remaining_secs`
//! observed since it last read as ready, so the
//! overlay height (a linear top-down wipe, not a radial one — Bevy 0.19
//! `bevy_ui` has no circular-clip primitive without a custom material, out
//! of scope for v1) is a genuine, if self-correcting-on-first-use,
//! proportion rather than a guess.
//!
//! ### Bugfix: the veil was invisible against the Phase 2 slot art
//! Matías's own in-game smoke of EM-5.17 Phase 2 reported the sweep never
//! visibly appears at all. The sweep LOGIC itself (the height/fraction math
//! above) was already correct and covered by
//! [`tests::cooldown_overlay_tracks_remaining_over_inferred_total`] — and the
//! draw ORDER was already correct too ([`SkillSlotBorderOverlay`] spawns
//! first/under, [`HotbarCooldownOverlay`] spawns after/above it). The actual
//! bug was colour: [`HotbarCooldownOverlay`] used to fill with a raw
//! `Color::srgba(0.0, 0.0, 0.0, 0.7)` literal, but Phase 2's own
//! `skill_slot_border.png` (spawned as [`SkillSlotBorderOverlay`], directly
//! underneath it in the same slot) is fully opaque near-black across its
//! entire area on disk — including the "cutout" centre that was meant to
//! stay alpha-transparent (`hud_layout`'s own module doc comment already
//! flagged this exact asset gap for the orb frames; confirmed here too by
//! directly sampling `skill_slot_border.png`, average RGB ~(19, 18,
//! 16)/255). A black veil composited on top of an already near-black
//! background stays indistinguishably black at ANY alpha or sweep height —
//! so the overlay was always drawing, just never visibly. The fix routes
//! this fill through [`HudTheme::palette`]'s new `cooldown_overlay` role (a
//! deliberately non-black, higher-luminance colour) instead of a hardcoded
//! literal — see that field's own doc comment in `xindeler-ui::theme` for the
//! luminance-floor regression test that pins this.
//!
//! ### Bugfix: every slot showed two overlapping rectangles
//! Matías's screenshot after the Phase 2 art landed showed each numbered
//! slot with what looked like two stacked rectangle graphics. Root cause:
//! [`xindeler_ui::slot::slot_bundle`] gives every slot a generic flat
//! `BackgroundColor`/`BorderColor` panel (a bright gold 2px border) by
//! default, and [`SkillSlotBorderOverlay`] then spawns as a CHILD sized to
//! the slot's padding box (inside that border, not covering it) — so the
//! flat border ring stayed visible as its own square, nested around the
//! ornate `skill_slot_border.png` art. Fixed in [`sync_hotbar_slots`] by
//! overriding both render components to `Color::NONE` right after spawning
//! `slot_bundle`, the same "drop the flat chrome, let the art be the only
//! frame" treatment PR #112 (`map_view.rs`'s `MinimapPanelRoot`) used for
//! the minimap's analogous square-frame bug. Scoped to the hotbar only —
//! bag/equip/trade slots have no overlay art of their own, so their flat
//! chrome is their only frame and stays unchanged.
//!
//! Reviewing this fix surfaced a second, latent instance of the same bug:
//! `xindeler_ui::slot`'s global drag observers hardcoded the theme's opaque
//! panel colours as the "resting" state to restore once a drag ends/leaves/
//! drops, so the FIRST drag touching a hotbar slot (it supports real
//! rearranging — see [`apply_hotbar_drop`]) would silently re-opaque it.
//! Fixed at the source via the [`xindeler_ui::slot::ChromelessSlot`] marker
//! (inserted alongside the `Color::NONE` override below) — see that
//! marker's own doc comment for the observer-by-observer detail.
//!
//! Compiled only under `listen-server`/`net-client` — same posture as every
//! other `xindeler_protocol`-consuming module in this crate.
//!
//! ## `xindeler_ui::slot` reuse note
//! The drag-drop slot primitive this screen depends on (`xindeler_ui::slot`)
//! shipped as part of EM-5.6 (PR #88, `feat/bl82-em56-inventory-trade`) while
//! this epic was in progress on a separate branch based off EM-5.11 — this
//! branch adopts that same, already-committed `xindeler-ui/src/slot.rs`
//! verbatim (not a second, competing implementation) so the two epics don't
//! diverge; whichever of the two PRs merges first keeps the file, the other
//! rebases onto it with a trivial (likely no-op) conflict.

use std::collections::{HashMap, HashSet};

use bevy::{ecs::change_detection::NonSend, prelude::*, ui::GlobalZIndex};
use xindeler_input::{GameInput, KeyMap};
use xindeler_protocol::{
    AssignHotbarSlot, NetAbilities, NetAuxiliaryAbility, NetCooldowns, NetLocalPlayer,
};
use xindeler_ui::{
    i18n::Localization,
    images::{HudImageKey, HudImages},
    slot::{
        ChromelessSlot, SLOT_BORDER_PX, SlotAddress, SlotContents, SlotDropped, SlotGroup,
        slot_bundle,
    },
    theme::{HudFonts, HudTheme},
    zlayer,
};

use crate::{controls_screen::key_label, hud_layout};

/// The one drag-drop group this screen's slots live in — an internal detail
/// (never interpreted by `xindeler_ui::slot`, which stays opinion-free about
/// what a group number means).
const HOTBAR_GROUP: SlotGroup = SlotGroup(0);

/// `pub(crate)` (not private) so `hud_layout.rs`'s own [`hud_layout::
/// SLOT_ROW_WIDTH_PX`] can derive its row-container width DIRECTLY from this
/// (and [`SLOTS_PER_HALF`]/[`HOTBAR_SLOT_GAP_PX`]), instead of a second
/// hardcoded literal silently drifting out of sync with this one.
///
/// History: `44.0` (Phase 2) -> `46.0` (EM-5.17 "5+5 slot-holders") ->
/// `52.0` (HUD polish round 3, legibility) -> `58.0` (HUD polish round 4,
/// Matías's `skill-slots-1.png` reference) — every one of those rounds also
/// re-trimmed the (now-deleted) `hud_layout::ACTION_BAR_WIDTH_TRIM` in
/// lockstep, since the slots used to have to fit inside an independently-sized
/// ornate background image.
///
/// BL-82 HUD redesign round 6: that background image
/// (`action_bar_bg_left.png`/`_right.png`) is gone — see `hud_layout.rs`'s own
/// top-of-file doc comment — so there is no longer any fit-inside-the-frame
/// constraint driving this value.
///
/// BL-82 HUD overhaul (this pass): shrunk `58.0 -> 42.0` as part of Matías's
/// "make the whole bottom-centre cluster substantially smaller, closer to
/// Veloren's thin bottom strip" ask (goal #2) — the orbs shrink in lockstep
/// (`hud_layout::ORB_SIZE_PX` `160 -> 112`). Going from a 1×5 row per side to a
/// 2×3 grid per side (goal #3) already narrows each side's footprint on its
/// own; the smaller per-slot size compounds that. [`hud_layout::
/// SLOT_ROW_WIDTH_PX`] derives the grid's own width straight from this
/// constant and [`SLOT_COLS_PER_HALF`].
pub(crate) const SLOT_SIZE_PX: f32 = 42.0;

/// BL-82 HUD redesign round 6: the gap (px) between adjacent hotbar slots
/// within one slot row. History: `4.0` (theme's generic `HudSpacing::xs`) ->
/// `3.0` (HUD polish round 3, a dedicated tighter constant matching
/// `xindeler-old`'s own `skillbar.rs` reference `slot_offset = 3.0`) -> `2.0`
/// (HUD polish round 4, Matías's `skill-slots-1.png` reference).
///
/// Round 6 shrinks this further, `2.0 -> 0.0` — Matías's `hud-ejemplo-2.png`
/// reference shows the ability icons sitting essentially flush/adjacent, no
/// visible gap between them (each icon's own border art supplies the visual
/// separation, the way `skill_slot_border.png`'s per-slot border already
/// does). Previous rounds kept a small positive gap specifically to avoid a
/// mismatched-art seam where two ORNATE frame pieces touched — that concern
/// doesn't apply here: there is no frame art dictating a minimum spacing any
/// more (see `hud_layout.rs`'s own top-of-file doc comment), so a flush `0.0`
/// is the natural tightest packing, matching the reference exactly.
pub(crate) const HOTBAR_SLOT_GAP_PX: f32 = 0.0;

/// BL-82 HUD overhaul (this pass): the ability-slot grid is now a 2-ROW × 3-
/// COLUMN block per side (Matías's goal #3), up from the previous flat 1×5 row.
/// [`SLOT_COLS_PER_HALF`] columns wide, [`SLOT_ROWS_PER_HALF`] rows tall.
pub(crate) const SLOT_COLS_PER_HALF: usize = 3;
/// See [`SLOT_COLS_PER_HALF`].
pub(crate) const SLOT_ROWS_PER_HALF: usize = 2;

/// Number of ability-slot HOLDERS rendered per action-bar half — now a
/// [`SLOT_COLS_PER_HALF`]×[`SLOT_ROWS_PER_HALF`] grid (`3*2 == 6`), up from the
/// old flat `5`. 6 on the left block, 6 on the right, 12 total, regardless of
/// how many of them the sim currently populates with real content (see the
/// module doc comment).
pub(crate) const SLOTS_PER_HALF: usize = SLOT_COLS_PER_HALF * SLOT_ROWS_PER_HALF;

/// Total slot holders always rendered — matches [`SLOT_INPUTS`]'s length and
/// is asserted equal to it in
/// [`tests::hotbar_slot_count_matches_the_keybind_table`].
const HOTBAR_SLOT_COUNT: usize = SLOTS_PER_HALF * 2;

/// The keybind each rendered slot index (0-based) is labelled with — every
/// one of the [`HOTBAR_SLOT_COUNT`] holders has a real entry here, so every
/// holder shows a keybind glyph, not just the ones the sim currently
/// populates with content.
///
/// BL-82 HUD overhaul (this pass): with the grid now 2×3 per side and 12 slots
/// total, Matías specified this EXACT keybind-to-position map. Slot indices run
/// row-major within each half (the same order the grid auto-flows its
/// children), so index -> grid cell is:
///
/// ```text
///   LEFT half (indices 0..6)      RIGHT half (indices 6..12)
///   row 1:  [1] [2] [3]           row 1:  [7] [8] [LMB]
///   row 2:  [4] [5] [6]           row 2:  [9] [0] [RMB]
/// ```
///
/// So the two mouse buttons ([`GameInput::Primary`]/[`GameInput::Secondary`],
/// rendered as the `mouse_click_left`/`_right` icons — see the match in
/// [`sync_hotbar_slots`]) sit at indices `8` and `11` (right block, rightmost
/// column of each row), NOT the trailing two indices as in the old flat-row
/// scheme. The number labels come straight from the live [`KeyMap`]: `Slot1..
/// Slot9` -> `1..9` and `Slot10` -> `0` (this pass rebound `Slot10` from `KeyQ`
/// onto `Digit0`, `keybind.rs`, so the `0` badge is truthful). This stays a
/// pure LABEL/position map — nothing today triggers a slot FROM a `GameInput`
/// press (documented future step), so reusing `Primary`/`Secondary` here can't
/// double-fire against the separate M1/M2 indicators.
const SLOT_INPUTS: [GameInput; 12] = [
    // Left block, row 1: keys 1, 2, 3.
    GameInput::Slot1,
    GameInput::Slot2,
    GameInput::Slot3,
    // Left block, row 2: keys 4, 5, 6.
    GameInput::Slot4,
    GameInput::Slot5,
    GameInput::Slot6,
    // Right block, row 1: keys 7, 8, LMB.
    GameInput::Slot7,
    GameInput::Slot8,
    GameInput::Primary,
    // Right block, row 2: keys 9, 0, RMB.
    GameInput::Slot9,
    GameInput::Slot10,
    GameInput::Secondary,
];

/// Installs the hotbar: spawns the (initially empty) slot row + M1/M2
/// indicators at `Startup`, then keeps slot count/content, keybind labels,
/// and cooldown overlays synced every frame, and applies real drag-drop
/// reassignment.
pub struct HotbarViewPlugin;

impl Plugin for HotbarViewPlugin {
    fn build(&self, app: &mut App) {
        // BL-82 EM-5.4 already hit this exact bug (chat.rs's own doc
        // comment): `XindelerUiPlugin` doesn't override `is_unique()`
        // (defaults `true`), so a SECOND `add_plugins` call — e.g. after
        // `CombatHudViewPlugin` already added it — panics ("plugin was
        // already added") instead of silently no-opping. Guard it the same
        // way that fix did, rather than relying on registration ORDER
        // (`ControlsScreenPlugin`'s posture, which happens to work only
        // because it's always added after `CombatHudViewPlugin` today).
        if !app.is_plugin_added::<xindeler_ui::XindelerUiPlugin>() {
            app.add_plugins(xindeler_ui::XindelerUiPlugin);
        }
        // Registered here too (idempotent alongside `XindelerProtocolPlugin`'s
        // own `add_client_message` registration) so this plugin's own tests
        // don't need the whole protocol plugin — the same convention
        // `chat.rs`'s `ChatViewPlugin` already follows for `ChatSendRequest`.
        app.add_message::<AssignHotbarSlot>();
        app.init_resource::<HotbarSlotEntities>()
            .add_systems(
                Startup,
                spawn_hotbar
                    .after(xindeler_ui::theme::init_theme)
                    .after(xindeler_ui::images::init_images),
            )
            .add_systems(
                Update,
                (
                    // BL-82 EM-5.16 (T56.44 follow-up): reads
                    // `NonSend<Localization>` for a placeholder slot's
                    // "Empty" tooltip — ordered after `LocaleSyncSet`, same
                    // reasoning as `settings_window.rs`'s
                    // `refresh_setting_labels` (Bevy gives no ordering
                    // guarantee between two systems with conflicting
                    // `NonSend`/`NonSendMut` access absent an explicit edge).
                    // This system already re-reads `NetAbilities`
                    // unconditionally every frame (see its own doc comment),
                    // so a missed edge would only cost one stale frame, not a
                    // permanently-missed relocalize.
                    sync_hotbar_slots.after(xindeler_ui::i18n::LocaleSyncSet),
                    sync_slot_half_parenting.after(sync_hotbar_slots),
                    sync_empty_slot_backgrounds.after(sync_hotbar_slots),
                    sync_primary_secondary_indicators,
                    sync_keybind_labels,
                    sync_cooldown_overlays.after(sync_hotbar_slots),
                    handle_hotbar_drag_drop,
                ),
            );
    }
}

/// index -> spawned slot entity, resized by [`sync_hotbar_slots`] to the
/// FIXED [`HOTBAR_SLOT_COUNT`] (BL-82 EM-5.17 "5+5 slot-holders" follow-up —
/// no longer [`NetAbilities::slots`]'s real, sim-driven length; see the
/// module doc comment).
#[derive(Resource, Default)]
struct HotbarSlotEntities(Vec<Entity>);

/// The LEFT half of the ability-slot row, the parent for the first half of
/// the ability slots (spec §3.1). Originally the parent of the
/// `action_bar_bg_left.png` background piece (BL-82 EM-5.17 Phase 2); BL-82
/// HUD redesign round 6 removed that background image — this is now a bare
/// transparent flex container (see [`spawn_slot_row_half`]'s own doc
/// comment).
#[derive(Component)]
struct HotbarLeftHalf;
/// The RIGHT half — the parent for the remaining ability slots. See
/// [`HotbarLeftHalf`]'s own doc comment.
#[derive(Component)]
struct HotbarRightHalf;
/// Marks a per-slot `skill_slot_border.png` overlay child (BL-82 EM-5.17
/// Phase 2) — spawned FIRST among a slot's children (i.e. UNDER the keybind
/// label/cooldown veil/countdown text in draw order) so those still read
/// correctly; see `hud_layout`'s module doc comment for why this pack's
/// "overlay" art is actually fully opaque on disk, not alpha-cut, which is
/// what forces this ordering choice.
#[derive(Component)]
struct SkillSlotBorderOverlay;
/// Marks a per-slot "empty socket" stone cue child (BL-82 HUD overhaul, goal
/// #1) — spawned as the VERY FIRST child of a slot (under
/// [`SkillSlotBorderOverlay`] and everything else in draw order) so it reads
/// UNDER the ornate frame ring.
///
/// This REPLACES round 7's opaque near-black full-box `slot_bg` plate. Round 7
/// filled the whole slot box opaque so adjacent frames read flush against the
/// terrain — but Matías found the resulting solid black boxes too heavy and
/// asked (goal #1) for transparent slots back (terrain showing through the
/// ring's concave notches is now acceptable), keeping only a grey STONE
/// treatment on the specifically-EMPTY slot state so "is this slot empty?"
/// still reads at a glance. So this child now:
/// - fills only the ring's CENTRE hole ([`EMPTY_SLOT_STONE_INSET_PX`] inset on
///   every side), NOT the full box — the outer band stays clear so the frame's
///   own notches remain see-through;
/// - uses [`HudTheme::palette`]'s partially-translucent mid-grey
///   `slot_empty_stone` (not near-black), with rounded corners echoing the
///   ring's inner circle;
/// - is TOGGLED by [`sync_empty_slot_backgrounds`] — shown (`Display::Flex`)
///   only while the slot holds no ability, hidden (`Display::None`) once it is
///   filled (the icon then occupies the centre instead). It spawns hidden and
///   the toggle system reveals it for empty slots, so a filled slot never
///   flashes a stone cue on the first frame.
#[derive(Component)]
struct SkillSlotBackground;
#[derive(Component)]
struct HotbarPrimaryText;
#[derive(Component)]
struct HotbarSecondaryText;
/// Which [`GameInput`] this keybind-label child displays — resolved fresh
/// every frame from the live [`KeyMap`] (a rebind updates it immediately,
/// same acceptance bar EM-5.11's own controls screen established).
#[derive(Component)]
struct HotbarKeybindLabel(GameInput);
/// BL-82 HUD polish round 4 (issue 4) — marks the small `ImageNode` child a
/// mouse-click-icon holder (indices 8/11, `GameInput::Primary`/`Secondary`)
/// carries INSTEAD OF a [`HotbarKeybindLabel`]. Unlike the numbered text
/// labels, this icon is a STATIC replacement for the old `"LMB"`/`"RMB"`
/// glyph (`key_label`'s own rendering of those two bindings) — it never
/// needs to be resynced from a live [`KeyMap`] rebind the way
/// [`sync_keybind_labels`] resyncs the numbered text labels, since the icon
/// depicts "the mouse button", not the CURRENT binding for `Primary`/
/// `Secondary` (which stays fixed to the mouse in practice — see the module
/// doc comment's "Keybind LABELS" section). Kept as a marker (not just an
/// anonymous `ImageNode`) purely so tests can find it unambiguously.
#[derive(Component)]
struct HotbarKeybindIcon;
#[derive(Component)]
struct HotbarCooldownOverlay;
#[derive(Component)]
struct HotbarCooldownText;

/// BL-82 HUD polish round 4 (issue 3): diameter (px) of the small circular
/// keybind badge sitting in each slot's bottom-left corner — Matías's
/// `skill-slots-1.png` reference shows each slot's keybind indicator as a
/// distinct chip rather than bare unadorned text floating over the slot art.
///
/// BL-82 HUD overhaul: shrunk `20.0 -> 16.0` in step with the smaller
/// [`SLOT_SIZE_PX`] (`58 -> 42`), so the badge keeps roughly the same share of
/// the slot it labels.
pub(crate) const KEYBIND_BADGE_SIZE_PX: f32 = 16.0;

/// BL-82 HUD overhaul (goal #1): inset (px) from each edge of a slot's box to
/// the [`SkillSlotBackground`] "empty" stone cue, so the cue sits only in the
/// ring's CENTRE hole rather than filling the whole box. Keeping the box's
/// outer band clear leaves the ornate `skill_slot_border.png` frame's own
/// concave notches see-through (terrain shows through — Matías now accepts
/// this), reverting round 7's heavy full-box black plate while still giving
/// empty slots a clear grey-stone "socket" read. `~29%` of [`SLOT_SIZE_PX`] on
/// each side leaves a centred square roughly matching the ring's inner opening.
pub(crate) const EMPTY_SLOT_STONE_INSET_PX: f32 = 12.0;

/// Distance (px) from the slot's own bottom/left edges to the badge's
/// bottom/left edges (BL-82 HUD polish round 4, issue 3).
pub(crate) const KEYBIND_BADGE_MARGIN_PX: f32 = 2.0;

/// The keybind badge's own chip chrome — a small, circular
/// (`border_radius == size / 2`) panel using the SAME `panel_bg`/
/// `panel_border` theme roles [`xindeler_ui::slot::slot_bundle`] uses for its
/// flat chrome, just sized down and rounded into a circle instead of a
/// square. Centres its one child (either a [`HotbarKeybindLabel`] text node
/// or a [`HotbarKeybindIcon`] image node) via flex `Center`/`Center`.
/// `Pickable::IGNORE` — this is a passive decorative badge, never a drag-drop
/// or click target of its own (unlike the slot it sits inside, which already
/// carries real `Pickable` state for hotbar drag-and-drop).
fn keybind_badge_bundle(theme: &HudTheme) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(KEYBIND_BADGE_MARGIN_PX),
            left: Val::Px(KEYBIND_BADGE_MARGIN_PX),
            width: Val::Px(KEYBIND_BADGE_SIZE_PX),
            height: Val::Px(KEYBIND_BADGE_SIZE_PX),
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(KEYBIND_BADGE_SIZE_PX / 2.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..Default::default()
        },
        BackgroundColor(theme.palette.panel_bg),
        BorderColor::all(theme.palette.panel_border),
        bevy::picking::Pickable::IGNORE,
    )
}

/// BL-82 HUD polish round 4 (issue 4): the mouse-click-icon bundle a keybind
/// badge's child carries for indices 8/11 (`GameInput::Primary`/`Secondary`)
/// instead of a [`HotbarKeybindLabel`] text node — a small square `ImageNode`
/// (a few px smaller than the badge itself, so the badge's own circular
/// border/background still reads as a rim around it) showing the real
/// `mouse_click_left.png`/`mouse_click_right.png` art (Matías's dedicated
/// dark-mouse-silhouette-with-gold-highlighted-button icons, matching the
/// HUD-D4 pack's own art style) rather than the `"LMB"`/`"RMB"` text glyph
/// this replaces.
///
/// `Pickable::IGNORE` — same reasoning as [`SkillSlotBorderOverlay`]'s own
/// doc comment (bevy_picking's default `Pickable` BLOCKS whatever's beneath
/// an entity that doesn't carry one): without this, hovering exactly over
/// this small icon would make IT (not the parent `HudSlot`) the hit entity,
/// silently defeating the slot's own `Hovered` state under this ~20px
/// corner. No hotbar feature reads `Hovered` today (no tooltip/hover-ring
/// wired for this screen yet), so this is a latent-not-yet-visible gap
/// fixed proactively — bevy-migration-reviewer flagged it during round 4's
/// review — rather than a regression this round introduces.
fn keybind_icon_bundle(icon: Handle<Image>) -> impl Bundle {
    const ICON_INSET_PX: f32 = 4.0;
    (
        HotbarKeybindIcon,
        ImageNode::new(icon),
        Node {
            width: Val::Px(KEYBIND_BADGE_SIZE_PX - ICON_INSET_PX),
            height: Val::Px(KEYBIND_BADGE_SIZE_PX - ICON_INSET_PX),
            ..Default::default()
        },
        bevy::picking::Pickable::IGNORE,
    )
}

/// A short, uppercase placeholder glyph for a dotted ability id (e.g.
/// `"class.warrior.rally"` -> `"RALL"`) — the SAME "themed placeholder,
/// reviewer-approved for v1" posture EM-5.2's buff-strip colour swatches and
/// `xindeler_ui::slot`'s own icon-text established; real `.vox`/icon-atlas
/// art is a documented follow-up (EM-5.1's own deferred `.vox`-icon path).
fn short_glyph(ability_id: &str) -> String {
    let segment = ability_id.rsplit('.').next().unwrap_or(ability_id);
    let mut glyph: String = segment.chars().take(4).collect();
    glyph.make_ascii_uppercase();
    glyph
}

/// Spawns one of the two ability-slot row halves flanking the centre Stamina
/// orb (spec §3.1). Each half is itself the flex-row PARENT its own share of
/// ability slots get `add_child`ed into (by
/// [`sync_hotbar_slots`]/[`sync_slot_half_parenting`]), positioned per
/// `crate::hud_layout::CLUSTER` — the SAME arithmetic `combat_hud.rs`'s orbs
/// use, so the two independently-`Startup`-spawned plugins line up into one
/// contiguous row.
///
/// ## BL-82 HUD redesign round 6 — no more background art
/// Rounds 3-5 (preserved in git history/this module's changelog-style doc
/// comments elsewhere) spent significant effort getting the ornate
/// `action_bar_bg_left.png`/`action_bar_bg_right.png` background pieces to
/// size/crop/align well around the slots — that round removed that art from
/// the render path entirely (Matías's `hud-ejemplo-2.png` reference has no
/// frame art around its ability icons at all), so this function takes no image
/// handle/`bottom_pad_px`/leading inset: it's a plain transparent
/// absolutely-positioned container, sized to EXACTLY [`hud_layout::
/// SLOT_ROW_WIDTH_PX`]×[`hud_layout::SLOT_ROW_HEIGHT_PX`] (derived straight
/// from the slot geometry — see that constant's own doc comment).
///
/// ## BL-82 HUD overhaul — 2×3 grid, not a flat 1×5 row (goal #3)
/// Each half is now a [`Display::Grid`] of [`SLOT_COLS_PER_HALF`] columns ×
/// [`SLOT_ROWS_PER_HALF`] rows of [`SLOT_SIZE_PX`] cells,
/// [`HOTBAR_SLOT_GAP_PX`] apart. The [`SLOTS_PER_HALF`] slot entities this
/// half receives (via [`sync_slot_half_parenting`]) auto-flow into those cells
/// row-major in child order — which is exactly the index order [`SLOT_INPUTS`]
/// assigns keybinds in, so index `0,1,2` land in the top row and `3,4,5` in the
/// bottom row (left block: keys `1,2,3 / 4,5,6`; right block: `7,8,LMB /
/// 9,0,RMB`). The container's own box is `SLOT_ROW_HEIGHT_PX` (`==
/// ORB_SIZE_PX`) tall — taller than the 2 slot rows — so `align_content:
/// Center` centres the grid vertically within the orbs' own band, the same way
/// the old `align_items: Center` centred the single row.
fn spawn_slot_row_half(commands: &mut Commands, left_offset_px: f32) -> Entity {
    commands
        .spawn((GlobalZIndex(zlayer::ORBS_ACTION_BAR_PARTY_MINIMAP), Node {
            position_type: PositionType::Absolute,
            left: hud_layout::CENTER_LEFT,
            bottom: Val::Px(hud_layout::CLUSTER_BOTTOM_PX),
            margin: UiRect::left(Val::Px(left_offset_px)),
            width: Val::Px(hud_layout::SLOT_ROW_WIDTH_PX),
            height: Val::Px(hud_layout::SLOT_ROW_HEIGHT_PX),
            display: Display::Grid,
            grid_template_columns: vec![bevy::ui::RepeatedGridTrack::px(
                SLOT_COLS_PER_HALF,
                SLOT_SIZE_PX,
            )],
            grid_template_rows: vec![bevy::ui::RepeatedGridTrack::px(
                SLOT_ROWS_PER_HALF,
                SLOT_SIZE_PX,
            )],
            align_content: AlignContent::Center,
            justify_content: JustifyContent::Center,
            column_gap: Val::Px(HOTBAR_SLOT_GAP_PX),
            row_gap: Val::Px(HOTBAR_SLOT_GAP_PX),
            ..Default::default()
        }))
        .id()
}

fn spawn_hotbar(mut commands: Commands, theme: Res<HudTheme>, fonts: Res<HudFonts>) {
    let left_half = spawn_slot_row_half(&mut commands, hud_layout::CLUSTER.slot_row_left_half_left);
    commands.entity(left_half).insert(HotbarLeftHalf);

    let right_half =
        spawn_slot_row_half(&mut commands, hud_layout::CLUSTER.slot_row_right_half_left);
    commands.entity(right_half).insert(HotbarRightHalf);

    let text_font = |font: Handle<bevy::text::Font>| TextFont {
        font: bevy::text::FontSource::Handle(font),
        font_size: bevy::text::FontSize::Px(14.0),
        ..Default::default()
    };
    commands.spawn((
        HotbarPrimaryText,
        Text(String::new()),
        text_font(fonts.body.clone()),
        TextColor(theme.palette.text),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(28.0),
            left: Val::Px(16.0),
            ..Default::default()
        },
    ));
    commands.spawn((
        HotbarSecondaryText,
        Text(String::new()),
        text_font(fonts.body.clone()),
        TextColor(theme.palette.text),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(28.0),
            right: Val::Px(16.0),
            ..Default::default()
        },
    ));
}

/// Resizes [`HotbarSlotEntities`] to the FIXED [`HOTBAR_SLOT_COUNT`] (BL-82
/// EM-5.17 "5+5 slot-holders" follow-up — no longer
/// [`NetAbilities::slots`]'s real, sim-driven length; see the module doc
/// comment) and writes each slot's [`SlotContents`] — real content for
/// indices `< abilities.slots.len()`, an empty placeholder for the rest.
///
/// ## Why this reads `NetAbilities` unconditionally every frame, not gated
/// ## on `Changed<NetAbilities>` (a real bug this fixed)
/// `xindeler-sim-bridge::hotbar::mirror_hotbar_state` dedups server-side
/// (`HotbarMirrorCache`) — it only re-inserts `NetAbilities` when the VALUE
/// actually differs, which for the local player typically happens once,
/// very early (often before this plugin's own systems get their first
/// `Update` execution at all). A `Changed<NetAbilities>` filter on THIS
/// system would compare against ITS OWN "last observed" tick, established
/// the first time it runs — if the one-and-only real change already
/// happened before that baseline was captured, `Changed` reads `false`
/// FOREVER for that entity, even though the data is genuinely present and
/// correct (confirmed live: a direct probe query with no `Changed` filter
/// saw the real 5-slot `NetAbilities` the whole time `Changed<NetAbilities>`
/// never fired once). `combat_hud.rs`'s own `sync_local_player_bars` never
/// gated on `Changed` for exactly this class of reason — it just re-reads
/// every frame (cheap: a handful of small components) and writes into
/// `SlotContents` only when the computed value actually differs (the
/// `if *slot_contents != new_contents` check below), which is the correct
/// place to avoid redundant work, not the query filter.
#[allow(clippy::too_many_arguments)]
fn sync_hotbar_slots(
    mut commands: Commands,
    theme: Res<HudTheme>,
    fonts: Res<HudFonts>,
    images: Res<HudImages>,
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    mut slot_entities: ResMut<HotbarSlotEntities>,
    mut contents: Query<&mut SlotContents>,
    localization: NonSend<Localization>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };

    // Entities at indices `< old_len` already existed BEFORE this call and
    // are visible to the `contents` `Query` below; entities spawned by the
    // resize loop just below are only queued via `Commands` — they do NOT
    // exist in `contents`' view of the world until the command queue is
    // flushed (the next sync point / `app.update()`), so this system must
    // NOT try to update them through that `Query` in the SAME call (it
    // would silently no-op, leaving a freshly-spawned slot's `SlotContents`
    // at `slot_bundle`'s empty default forever, since `NetAbilities` may not
    // change again for a long time). See the content-write loop at the
    // bottom of this function for the split this requires.
    let old_len = slot_entities.0.len();

    while slot_entities.0.len() < HOTBAR_SLOT_COUNT {
        let index = slot_entities.0.len();
        let slot_entity = commands
            .spawn(slot_bundle(
                &theme,
                HOTBAR_GROUP,
                SlotAddress(index as u64),
                SLOT_SIZE_PX,
            ))
            .id();
        // BL-82 EM-5.17 Phase 0 review follow-up (Matías's screenshot: every
        // numbered slot showed two overlapping rectangles). Root cause:
        // `slot_bundle`'s generic flat `BackgroundColor(panel_bg)`/
        // `BorderColor(panel_border)` chrome (a bright gold 2px square
        // outline drawn by the slot's OWN Node) sat directly underneath
        // `SkillSlotBorderOverlay`'s `skill_slot_border.png` child spawned
        // just below — that child is absolutely positioned/sized to the
        // slot's PADDING box (inside the 2px border, per bevy_ui's
        // CSS-like absolute-positioning containing block), so the ornate art
        // never covers the border ring; both rendered at once as two nested
        // squares. Same "drop the flat chrome, let the art be the only
        // frame" treatment PR #112 (`map_view.rs`'s `MinimapPanelRoot`) used
        // for the minimap's square-frame bug: override both render
        // components to `Color::NONE` right after spawning `slot_bundle`,
        // rather than inventing a chromeless bundle variant. Only the hotbar
        // does this — bag/equip/trade slots (`inventory_ui.rs`/
        // `trade_ui.rs`/`diary.rs`) call plain `slot_bundle` with no overlay
        // art of their own, so they keep the flat chrome unchanged (it's
        // their ONLY frame; scoped to what was actually reported).
        //
        // The hotbar also supports real drag-and-drop rearrangement
        // (`apply_hotbar_drop` below) — `xindeler_ui::slot`'s drag observers
        // are registered GLOBALLY against every `HudSlot`, and originally
        // hardcoded the theme's OPAQUE `panel_bg`/`panel_border` as the
        // "resting" colour to restore once a drag ends/leaves/drops. Without
        // more, the FIRST drag touching a hotbar slot (as either end) would
        // silently re-opaque it, reintroducing this exact doubled-rectangle
        // bug from then on — caught while reviewing this fix, not in the
        // original report. Fixed at the source: `ChromelessSlot` is a marker
        // `xindeler_ui::slot`'s observers check to restore `Color::NONE`
        // instead for a slot that opted out of the flat chrome, so it's
        // inserted here alongside the one-off `Color::NONE` override (see
        // that marker's own doc comment for the full before/after per
        // observer).
        commands.entity(slot_entity).insert((
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
            ChromelessSlot,
        ));
        commands.entity(slot_entity).with_children(|parent| {
            // BL-82 HUD overhaul (goal #1): the "empty socket" stone cue — a
            // grey, partially-translucent, rounded fill inset to the ring's
            // CENTRE hole ([`EMPTY_SLOT_STONE_INSET_PX`] on every side), spawned
            // as the VERY FIRST child so it renders under the ornate frame, the
            // keybind badge, the cooldown veil and the icon glyph. This
            // REPLACES round 7's opaque near-black full-box fill (Matías found
            // those solid black boxes too heavy; he now accepts terrain showing
            // through the frame's own notches). Spawned `Display::None` and
            // revealed only for EMPTY slots by `sync_empty_slot_backgrounds`
            // (so a filled slot never flashes a stone cue) — see
            // `SkillSlotBackground`'s own doc comment. Rounded corners echo the
            // ring's inner circle; the outer band stays clear so the frame's
            // concave notches remain see-through.
            parent.spawn((
                SkillSlotBackground,
                Node {
                    position_type: PositionType::Absolute,
                    display: Display::None,
                    top: Val::Px(EMPTY_SLOT_STONE_INSET_PX - SLOT_BORDER_PX),
                    left: Val::Px(EMPTY_SLOT_STONE_INSET_PX - SLOT_BORDER_PX),
                    width: Val::Px(SLOT_SIZE_PX - 2.0 * EMPTY_SLOT_STONE_INSET_PX),
                    height: Val::Px(SLOT_SIZE_PX - 2.0 * EMPTY_SLOT_STONE_INSET_PX),
                    // Rounded corners echo the ring's inner circle.
                    border_radius: BorderRadius::all(Val::Px(
                        (SLOT_SIZE_PX - 2.0 * EMPTY_SLOT_STONE_INSET_PX) / 4.0,
                    )),
                    ..Default::default()
                },
                BackgroundColor(theme.palette.slot_empty_stone),
                bevy::picking::Pickable::IGNORE,
            ));
            // BL-82 EM-5.17 Phase 2: `skill_slot_border.png` overlay — spawned
            // right after the background fill above (i.e. rendered UNDER the
            // keybind label/cooldown veil/countdown text below, but ON TOP of
            // that fill) so those stay legible; see `SkillSlotBorderOverlay`'s
            // own doc comment for why this pack's "overlay" art can't safely
            // go on TOP of everything without hiding it.
            //
            // BL-82 HUD polish round 5: `hud_layout::SKILL_SLOT_BORDER_SOURCE_CROP`
            // crops the source PNG down to its own tight opaque bounding box
            // before stretching it onto this slot's square box — see that
            // constant's own doc comment for why the un-cropped full canvas
            // (this used to be a plain `ImageNode::new` with no crop at all)
            // made the real border art occupy only ~half of every rendered
            // slot, which was the actual root cause of "big gaps between
            // individual hotbar slots" no `HOTBAR_SLOT_GAP_PX` tuning could
            // fix.
            parent.spawn((
                SkillSlotBorderOverlay,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..Default::default()
                },
                ImageNode {
                    rect: Some(hud_layout::SKILL_SLOT_BORDER_SOURCE_CROP),
                    image_mode: bevy::ui::widget::NodeImageMode::Stretch,
                    ..ImageNode::new(images.get(HudImageKey::SkillSlotBorder))
                },
                bevy::picking::Pickable::IGNORE,
            ));
            // BL-82 HUD polish round 4 (issue 3): bottom-left circular
            // keybind badge, matching Matías's `skill-slots-1.png` reference
            // (previously a bare top-left text label with no background
            // chip) — see `keybind_badge_bundle`'s own doc comment.
            //
            // BL-82 HUD polish round 4 (issue 4): the two mouse-button holders
            // (`GameInput::Primary`/`Secondary`, indices 8/11 in the 2×3 map)
            // show a dedicated mouse-click ICON inside that same badge instead
            // of the `"LMB"`/`"RMB"` text `key_label` used to render — see
            // `HotbarKeybindIcon`'s own doc comment.
            match SLOT_INPUTS.get(index).copied() {
                Some(GameInput::Primary) => {
                    parent
                        .spawn(keybind_badge_bundle(&theme))
                        .with_children(|badge| {
                            badge.spawn(keybind_icon_bundle(
                                images.get(HudImageKey::MouseClickLeft),
                            ));
                        });
                },
                Some(GameInput::Secondary) => {
                    parent
                        .spawn(keybind_badge_bundle(&theme))
                        .with_children(|badge| {
                            badge.spawn(keybind_icon_bundle(
                                images.get(HudImageKey::MouseClickRight),
                            ));
                        });
                },
                Some(input) => {
                    parent
                        .spawn(keybind_badge_bundle(&theme))
                        .with_children(|badge| {
                            badge.spawn((
                                HotbarKeybindLabel(input),
                                Text(String::new()),
                                TextFont {
                                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                                    font_size: bevy::text::FontSize::Px(11.0),
                                    ..Default::default()
                                },
                                // BL-82 HUD polish round 4 (issue 3): bumped
                                // from `text_muted` to full-contrast `text` —
                                // the old muted colour was tuned for a bare
                                // number floating directly over the busy slot
                                // art; now it sits inside its own solid
                                // `keybind_badge_bundle` chip, so it needs
                                // full contrast to stay legible against that
                                // opaque background (matches the crisp,
                                // clearly-numbered badges in Matías's
                                // `skill-slots-1.png` reference).
                                TextColor(theme.palette.text),
                                // See `keybind_icon_bundle`'s own doc comment
                                // for why this needs `Pickable::IGNORE` too —
                                // same latent hover-blocking gap, same fix.
                                bevy::picking::Pickable::IGNORE,
                            ));
                        });
                },
                None => {},
            }
            parent.spawn((
                HotbarCooldownOverlay,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(0.0),
                    ..Default::default()
                },
                // BL-82 EM-5.17 Phase 2 bugfix: was a raw
                // `Color::srgba(0.0, 0.0, 0.0, 0.7)` literal — invisible once
                // composited over `SkillSlotBorderOverlay`'s near-black
                // `skill_slot_border.png` (see the module doc comment's
                // "Bugfix" section for the full root-cause writeup).
                BackgroundColor(theme.palette.cooldown_overlay),
            ));
            parent.spawn((
                HotbarCooldownText,
                Text(String::new()),
                TextFont {
                    font: bevy::text::FontSource::Handle(fonts.body.clone()),
                    font_size: bevy::text::FontSize::Px(12.0),
                    ..Default::default()
                },
                TextColor(theme.palette.text),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(1.0),
                    right: Val::Px(2.0),
                    ..Default::default()
                },
            ));
        });
        // Parenting into the correct action-bar HALF is `sync_slot_half_parenting`'s
        // job (ordered right after this system) — see its own doc comment
        // for why a freshly-spawned slot isn't parented here directly.
        slot_entities.0.push(slot_entity);
    }
    // Defensive only — `HOTBAR_SLOT_COUNT` is a fixed constant today, so this
    // never actually pops anything; kept symmetric with the grow loop above
    // in case a future change makes the holder count itself dynamic again.
    while slot_entities.0.len() > HOTBAR_SLOT_COUNT {
        if let Some(extra) = slot_entities.0.pop() {
            commands.entity(extra).despawn();
        }
    }

    for index in 0..HOTBAR_SLOT_COUNT {
        let Some(&entity) = slot_entities.0.get(index) else {
            continue;
        };
        // `abilities.slots.get(index)` is `None` for every index at/beyond
        // the sim's real, currently-granted slot count (`ActiveAbilities::
        // limit`, `Some(5)` today — see the module doc comment) — those
        // holders render as an empty placeholder: no icon glyph, a neutral
        // "Empty" tooltip (same text an in-range-but-unassigned slot already
        // shows), same chromeless frame/border art as every other holder.
        //
        // BL-82 EM-5.16 (T56.44 follow-up): reuses the already-loaded
        // `common-empty` key (`common.ftl`, already in `DEFAULT_HUD_FTL_FILES`)
        // rather than a new duplicate key — this system re-reads
        // `NetAbilities`/`Localization` unconditionally every frame (see this
        // function's own doc comment above), so a locale flip re-resolves
        // this tooltip the very next frame with no extra wiring.
        let new_contents = match abilities.slots.get(index) {
            Some(slot) => SlotContents {
                icon_text: slot
                    .ability_id
                    .as_deref()
                    .map(short_glyph)
                    .unwrap_or_default(),
                // Hotbar slots hold abilities, not stackable items — no
                // quantity badge (`xindeler_ui::slot`'s own field for EM-5.6's
                // bag/trade screens).
                quantity: None,
                tooltip: slot
                    .ability_id
                    .clone()
                    .unwrap_or_else(|| localization.tr("common-empty")),
            },
            None => SlotContents {
                icon_text: String::new(),
                quantity: None,
                tooltip: localization.tr("common-empty"),
            },
        };
        if index < old_len {
            // Pre-existing entity: a live `Query` sees it right now, so
            // updating in place (and skipping a no-op write) is both
            // correct and cheap.
            if let Ok(mut slot_contents) = contents.get_mut(entity)
                && *slot_contents != new_contents
            {
                *slot_contents = new_contents;
            }
        } else {
            // Freshly spawned this call (see the doc comment above `old_len`)
            // — `contents` cannot see it yet, so queue the real content as a
            // command chained onto the same entity instead of relying on
            // the `Query` to catch it next frame (which would leave the
            // slot showing nothing until `NetAbilities` happens to change
            // again).
            commands.entity(entity).insert(new_contents);
        }
    }
}

/// BL-82 EM-5.17 Phase 2: (re-)parents every current hotbar slot entity into
/// whichever slot-row HALF it belongs to — the first `ceil(n/2)` slots go
/// into the LEFT half ([`HotbarLeftHalf`]), the rest into the RIGHT half
/// (spec §3.1's "first half of slots in left, rest in right"). Runs
/// AFTER [`sync_hotbar_slots`] so it always sees that system's up-to-date
/// [`HotbarSlotEntities`] for the current frame.
///
/// BL-82 EM-5.17 "5+5 slot-holders" follow-up: `n` is now the FIXED
/// [`HOTBAR_SLOT_COUNT`] (10), not `NetAbilities::slots`'s real, sim-driven
/// length — so `ceil(n/2)` is always an exact 5/5 split and this system no
/// longer actually RE-splits anything in practice (the entity count never
/// changes after the initial spawn; only `SlotContents` does when the
/// mirror's real length changes — see [`sync_hotbar_slots`]'s own doc
/// comment). The `is_changed()` gate below still only fires once, at the
/// initial spawn.
///
/// Gated on `slot_entities.is_changed()` — `Res<T>::is_changed` is true the
/// frame `HotbarSlotEntities` itself is replaced/mutated (i.e. exactly when
/// `sync_hotbar_slots` resizes it), NOT every frame. This matters because
/// `add_child` is NOT a no-op when the entity is already parented to that
/// same target: Bevy 0.19's `ChildOf` relationship hooks unconditionally
/// remove-then-reinsert on every call, moving the entity to the end of the
/// parent's `Children` and marking `Children` `Changed` even when nothing
/// actually moved. Re-running this every frame would therefore make every
/// slot's `ChildOf` (and both halves' `Children`) tick "changed" on every
/// single frame forever — harmless today only because nothing is gated on
/// `Changed<Children>` downstream, but real, avoidable churn this fixes.
fn sync_slot_half_parenting(
    mut commands: Commands,
    slot_entities: Res<HotbarSlotEntities>,
    left_half: Query<Entity, With<HotbarLeftHalf>>,
    right_half: Query<Entity, With<HotbarRightHalf>>,
) {
    if !slot_entities.is_changed() {
        return;
    }

    let Ok(left_half) = left_half.single() else {
        return;
    };
    let Ok(right_half) = right_half.single() else {
        return;
    };

    let total = slot_entities.0.len();
    let mid = total.div_ceil(2);
    for (index, &slot_entity) in slot_entities.0.iter().enumerate() {
        let target = if index < mid { left_half } else { right_half };
        commands.entity(target).add_child(slot_entity);
    }
}

/// BL-82 HUD overhaul (goal #1): shows each slot's [`SkillSlotBackground`]
/// grey-stone "empty socket" cue (`Display::Flex`) only while the slot holds no
/// ability, and hides it (`Display::None`) once the slot is filled — the icon
/// glyph then occupies the centre instead. "Empty" is read straight off the
/// slot's own [`SlotContents::icon_text`] (blank for both an out-of-range
/// placeholder holder AND an in-range-but-unassigned slot — see
/// [`sync_hotbar_slots`]), the same signal every other empty-slot behaviour in
/// this module keys off. Runs AFTER [`sync_hotbar_slots`] so it sees the
/// current frame's content; diffs before writing so a settled slot doesn't
/// churn its `Node` every frame. A freshly-spawned slot isn't visible to the
/// `contents`/`children_of` queries until the next frame (the same command-
/// queue lag [`sync_hotbar_slots`]'s own `old_len` split documents) — it stays
/// at the spawned `Display::None` default for that one frame, which is correct:
/// a filled slot must never flash a stone cue, and an empty slot appearing one
/// frame late at startup is imperceptible.
fn sync_empty_slot_backgrounds(
    slot_entities: Res<HotbarSlotEntities>,
    contents: Query<&SlotContents>,
    children_of: Query<&Children>,
    mut backgrounds: Query<&mut Node, With<SkillSlotBackground>>,
) {
    for &entity in &slot_entities.0 {
        let Ok(slot_contents) = contents.get(entity) else {
            continue;
        };
        let target = if slot_contents.icon_text.is_empty() {
            Display::Flex
        } else {
            Display::None
        };
        let Ok(children) = children_of.get(entity) else {
            continue;
        };
        for child in children.iter() {
            if let Ok(mut node) = backgrounds.get_mut(child)
                && node.display != target
            {
                node.display = target;
            }
        }
    }
}

/// Refreshes the M1/M2 read-only indicators from the local player's
/// [`NetAbilities::primary`]/`secondary`. Reads unconditionally every frame
/// (NOT gated on `Changed<NetAbilities>` — see [`sync_hotbar_slots`]'s own
/// doc comment for why that filter is unsafe to use against a server-side
/// deduped mirror), diffing before writing `Text` to avoid a pointless
/// per-frame mutation once the value settles.
///
/// BL-82 EM-5.16 (T56.44 follow-up): the `"M1"`/`"M2"` prefixes below are
/// deliberately NOT routed through [`Localization::tr`] — like the numbered
/// slots' key-glyph labels (`crate::controls_screen::key_label`, this
/// module's own doc comment) and `settings_window.rs`'s bare `"-"`/`"+"`
/// numeric-row glyphs, `"M1"`/`"M2"` are short mouse-button abbreviations,
/// not language-dependent prose — they read the same in `es` (and every other
/// locale this screen might one day support) as they do in `en`.
fn sync_primary_secondary_indicators(
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    mut primary_text: Query<&mut Text, (With<HotbarPrimaryText>, Without<HotbarSecondaryText>)>,
    mut secondary_text: Query<&mut Text, (With<HotbarSecondaryText>, Without<HotbarPrimaryText>)>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    if let Ok(mut text) = primary_text.single_mut() {
        let new_text = format!(
            "M1: {}",
            abilities
                .primary
                .as_deref()
                .map(short_glyph)
                .unwrap_or_else(|| "-".to_owned())
        );
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
    if let Ok(mut text) = secondary_text.single_mut() {
        let new_text = format!(
            "M2: {}",
            abilities
                .secondary
                .as_deref()
                .map(short_glyph)
                .unwrap_or_else(|| "-".to_owned())
        );
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

/// Recomputes every keybind-label child's text from the LIVE [`KeyMap`]
/// every frame — cheap at this screen's slot count, and the same "no
/// `is_changed` gate needed at this scale" call `controls_screen.rs`'s own
/// `refresh_binding_labels` already made. A rebind of `Slot1..Slot10`
/// updates the hotbar's own labels immediately, no extra wiring needed.
fn sync_keybind_labels(keymap: Res<KeyMap>, mut labels: Query<(&HotbarKeybindLabel, &mut Text)>) {
    for (label, mut text) in &mut labels {
        let new_label = keymap
            .keyboard
            .get_binding(label.0)
            .map_or_else(String::new, key_label);
        if text.0 != new_label {
            text.0 = new_label;
        }
    }
}

/// Reads the local player's [`NetCooldowns`] and updates every slot's
/// cooldown overlay height + countdown text — see the module doc comment
/// for the "sweep total" heuristic (a `Local<HashMap<String, f32>>` of
/// inferred totals, pruned each frame for abilities that finished cooling
/// down).
#[allow(clippy::too_many_arguments)]
fn sync_cooldown_overlays(
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    cooldowns: Query<&NetCooldowns, With<NetLocalPlayer>>,
    slot_entities: Res<HotbarSlotEntities>,
    children_of: Query<&Children>,
    mut overlays: Query<&mut Node, With<HotbarCooldownOverlay>>,
    mut texts: Query<&mut Text, With<HotbarCooldownText>>,
    mut known_totals: Local<HashMap<String, f32>>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    let cooldowns = cooldowns.single().ok();

    let cooling_ids: HashSet<&str> = cooldowns
        .map(|c| c.0.iter().map(|e| e.ability_id.as_str()).collect())
        .unwrap_or_default();
    known_totals.retain(|id, _| cooling_ids.contains(id.as_str()));

    let remaining_of = |ability_id: &str| -> f32 {
        cooldowns
            .and_then(|c| c.0.iter().find(|e| e.ability_id == ability_id))
            .map_or(0.0, |e| e.remaining_secs)
    };

    for (index, slot) in abilities.slots.iter().enumerate() {
        let Some(&entity) = slot_entities.0.get(index) else {
            continue;
        };
        let Ok(children) = children_of.get(entity) else {
            continue;
        };

        let remaining = slot.ability_id.as_deref().map_or(0.0, remaining_of);
        let fraction = if remaining > 0.0 {
            let ability_id = slot.ability_id.clone().unwrap_or_default();
            let total = known_totals.entry(ability_id).or_insert(remaining);
            *total = total.max(remaining);
            (remaining / *total).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let countdown_text = if remaining > 0.0 {
            format!("{:.0}", remaining.ceil())
        } else {
            String::new()
        };

        for child in children.iter() {
            if let Ok(mut node) = overlays.get_mut(child) {
                node.height = Val::Percent(fraction * 100.0);
            }
            if let Ok(mut text) = texts.get_mut(child)
                && text.0 != countdown_text
            {
                text.0 = countdown_text.clone();
            }
        }
    }
}

/// Drains [`SlotDropped`] and applies whichever of the two SOURCEs this
/// screen currently supports — both now go through the SAME real
/// `AssignHotbarSlot` client message (converged onto EM-5.3's follow-up
/// client-identity fix, which retired the old listen-server-only
/// `LocalAssignHotbarSlot` shortcut):
/// - a drag ENTIRELY within the hotbar (`from`/`to` both [`HOTBAR_GROUP`])
///   swaps the two slots' bindings via TWO [`AssignHotbarSlot`] messages — the
///   module doc comment's original "real drag-to-assign" contract;
/// - BL-82 EM-5.7: a drag FROM the diary's Abilities tab
///   (`crate::diary::DIARY_ABILITY_GROUP`) INTO a hotbar slot binds that
///   ability into the target slot via ONE [`AssignHotbarSlot`] — the dragged
///   ability is decoded straight from the [`SlotDropped::from_address`] (packed
///   via `NetAuxiliaryAbility::to_slot_address_raw`, see that type's own doc
///   comment), no `abilities` lookup needed for the source side.
///
/// A drop involving any OTHER group is silently ignored — not mis-applied —
/// exactly the module doc comment's original posture, just narrowed to the
/// groups that don't yet have a handler.
///
/// BL-82 EM-5.17 "5+5 slot-holders" follow-up: a diary-ability drop targeting
/// a PLACEHOLDER holder (`to_index >= abilities.slots.len()`, i.e. beyond the
/// sim's current `ActiveAbilities::limit`) is now also skipped here, client
/// side — `xindeler-sim-bridge`'s `ChangeAbilityEvent` handler already
/// no-ops that same request server-side (`Vec::get_mut` returns `None`
/// beyond the vec's real length), so this guard changes no OBSERVABLE
/// behaviour; it just avoids sending a request the server would silently
/// discard anyway.
fn handle_hotbar_drag_drop(
    mut drops: MessageReader<SlotDropped>,
    abilities: Query<&NetAbilities, With<NetLocalPlayer>>,
    mut assign: MessageWriter<AssignHotbarSlot>,
) {
    let Ok(abilities) = abilities.single() else {
        return;
    };
    for drop in drops.read() {
        if drop.to_group != HOTBAR_GROUP {
            continue;
        }

        if drop.from_group == crate::diary::DIARY_ABILITY_GROUP {
            let to_index = drop.to_address.raw() as usize;
            if abilities.slots.get(to_index).is_none() {
                continue;
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "hotbar slot indices are a handful, never near u32::MAX"
            )]
            assign.write(AssignHotbarSlot {
                slot: to_index as u32,
                ability: NetAuxiliaryAbility::from_slot_address_raw(drop.from_address.raw()),
            });
            continue;
        }

        if drop.from_group != HOTBAR_GROUP {
            continue;
        }
        let from_index = drop.from_address.raw() as usize;
        let to_index = drop.to_address.raw() as usize;
        if from_index == to_index {
            continue;
        }
        let (Some(from_slot), Some(to_slot)) = (
            abilities.slots.get(from_index),
            abilities.slots.get(to_index),
        ) else {
            continue;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "hotbar slot indices are a handful, never near u32::MAX"
        )]
        {
            assign.write(AssignHotbarSlot {
                slot: to_index as u32,
                ability: from_slot.aux,
            });
            assign.write(AssignHotbarSlot {
                slot: from_index as u32,
                ability: to_slot.aux,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::{asset::AssetPlugin, ecs::system::RunSystemOnce, image::ImagePlugin};
    use xindeler_protocol::{NetCooldownEntry, NetHotbarSlot};

    use super::*;

    /// `sync_hotbar_slots` now needs a real [`HudImages`] (the per-slot
    /// `skill_slot_border.png` overlay) — built the same headless-`AssetServer`
    /// way `combat_hud.rs`'s own `new_app_with_images` does.
    fn new_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.add_plugins(ImagePlugin::default());
        app.insert_resource(HudTheme::default());
        app.insert_resource(HudFonts {
            title: Handle::default(),
            body: Handle::default(),
        });
        let asset_server = app.world().resource::<AssetServer>().clone();
        app.insert_resource(HudImages::load(&asset_server));
        app.init_resource::<HotbarSlotEntities>();
        // BL-82 EM-5.16 (T56.44 follow-up): `sync_hotbar_slots` now reads
        // `NonSend<Localization>` for a placeholder slot's "Empty" tooltip —
        // loads the REAL repo `common.ftl` catalog (via `VELOREN_ASSETS`/
        // `XINDELER_ASSETS`, exactly like `settings_window.rs`/`esc_menu.rs`'s
        // own hot-swap tests) so this suite's existing `tooltip == "Empty"`
        // assertions keep resolving to the real catalog value, not an
        // empty-catalog bare-key fallback.
        app.insert_non_send(Localization::load(
            &xindeler_ui::i18n::fallback_locale(),
            &["common.ftl"],
        ));
        app
    }

    #[test]
    fn short_glyph_takes_the_last_dotted_segment_uppercased() {
        assert_eq!(short_glyph("class.warrior.rally"), "RALL");
        assert_eq!(short_glyph("m1"), "M1");
    }

    /// BL-82 EM-5.17 "5+5 slot-holders" follow-up (Matías's ask: 5 holders on
    /// each background piece, 10 total, regardless of how many the sim
    /// currently populates): a real `NetAbilities` with FEWER than
    /// [`HOTBAR_SLOT_COUNT`] real slots still drives exactly
    /// [`HOTBAR_SLOT_COUNT`] spawned slot HOLDER entities — the extra ones
    /// beyond the mirror's real length render as empty placeholders (no
    /// icon/tooltip content), not as missing entities.
    #[test]
    fn sync_hotbar_slots_always_spawns_hotbar_slot_count_holders() {
        let mut app = new_app();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: Some("common.abilities.sword.primary".to_owned()),
            secondary: None,
            slots: vec![
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::MainWeapon(0),
                    ability_id: Some("common.abilities.sword.m1".to_owned()),
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Empty,
                    ability_id: None,
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Innate(0),
                    ability_id: Some("class.warrior.rally".to_owned()),
                },
            ],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("sync_hotbar_slots runs");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>();
        assert_eq!(
            slot_entities.0.len(),
            HOTBAR_SLOT_COUNT,
            "must always spawn the fixed 12-holder budget, not just the mirror's 3 real slots"
        );

        let world = app.world();
        let first_contents = world.get::<SlotContents>(slot_entities.0[0]).unwrap();
        assert_eq!(first_contents.icon_text, "M1");
        assert_eq!(first_contents.tooltip, "common.abilities.sword.m1");

        let empty_contents = world.get::<SlotContents>(slot_entities.0[1]).unwrap();
        assert_eq!(empty_contents.icon_text, "");
        assert_eq!(empty_contents.tooltip, "Empty");

        // Indices 3..HOTBAR_SLOT_COUNT are beyond the mirror's real 3 slots
        // — placeholder holders, same empty content as an in-range-but-
        // unassigned slot, not missing/uninitialized.
        for index in 3..HOTBAR_SLOT_COUNT {
            let placeholder = world.get::<SlotContents>(slot_entities.0[index]).unwrap();
            assert_eq!(
                placeholder.icon_text, "",
                "placeholder slot {index} must show no icon"
            );
            assert_eq!(
                placeholder.tooltip, "Empty",
                "placeholder slot {index} must show the neutral empty tooltip"
            );
        }
    }

    /// [`HOTBAR_SLOT_COUNT`] matches [`SLOT_INPUTS`]'s length — every rendered
    /// holder has a real keybind entry (`Slot1..Slot10`), not just the ones
    /// the sim currently populates with content.
    #[test]
    fn hotbar_slot_count_matches_the_keybind_table() {
        assert_eq!(HOTBAR_SLOT_COUNT, SLOT_INPUTS.len());
        assert_eq!(HOTBAR_SLOT_COUNT, SLOTS_PER_HALF * 2);
    }

    /// BL-82 HUD overhaul: [`SLOT_INPUTS`] pins Matías's exact 2×3 keybind map
    /// — left block `1,2,3 / 4,5,6`, right block `7,8,LMB / 9,0,RMB`. So the
    /// two mouse buttons sit at indices `8` (right block row 1, col 3) and `11`
    /// (right block row 2, col 3), NOT the trailing two indices, and `Slot9`/
    /// `Slot10` occupy indices `9`/`10`. Regression guard against a future edit
    /// silently drifting the map (especially back to the old flat-row scheme).
    #[test]
    fn slot_inputs_pin_the_exact_two_by_three_keybind_map() {
        assert_eq!(SLOT_INPUTS, [
            // Left block, row 1 / row 2.
            GameInput::Slot1,
            GameInput::Slot2,
            GameInput::Slot3,
            GameInput::Slot4,
            GameInput::Slot5,
            GameInput::Slot6,
            // Right block, row 1: 7, 8, LMB.
            GameInput::Slot7,
            GameInput::Slot8,
            GameInput::Primary,
            // Right block, row 2: 9, 0, RMB.
            GameInput::Slot9,
            GameInput::Slot10,
            GameInput::Secondary,
        ]);
    }

    /// BL-82 HUD polish round 4 (issue 3): every keybind label/icon now
    /// lives ONE level deeper than before — nested inside its own
    /// `keybind_badge_bundle` chip, not a direct child of the slot — so
    /// these helpers walk grandchildren, not just children.
    fn find_keybind_label_text(world: &World, slot_entity: Entity) -> Option<String> {
        let children = world.get::<Children>(slot_entity)?;
        for badge in children.iter() {
            let Some(grandchildren) = world.get::<Children>(badge) else {
                continue;
            };
            for child in grandchildren.iter() {
                if world.get::<HotbarKeybindLabel>(child).is_some() {
                    return Some(world.get::<Text>(child).unwrap().0.clone());
                }
            }
        }
        None
    }

    /// BL-82 HUD polish round 4 (issue 4): finds the [`HotbarKeybindIcon`]
    /// image handle nested under a slot's badge, if any.
    fn find_keybind_icon_handle(world: &World, slot_entity: Entity) -> Option<Handle<Image>> {
        let children = world.get::<Children>(slot_entity)?;
        for badge in children.iter() {
            let Some(grandchildren) = world.get::<Children>(badge) else {
                continue;
            };
            for child in grandchildren.iter() {
                if world.get::<HotbarKeybindIcon>(child).is_some() {
                    return Some(world.get::<ImageNode>(child).unwrap().image.clone());
                }
            }
        }
        None
    }

    /// BL-82 HUD overhaul: the 2×3 keybind map renders through
    /// [`sync_keybind_labels`] as Matías specified — the number holders show
    /// their live binding (`Slot8` -> `"8"`, `Slot9` -> `"9"`, `Slot10` ->
    /// `"0"` after this pass's `keybind.rs` rebind onto `Digit0`), and the two
    /// mouse-button holders (indices `8`/`11`, `Primary`/`Secondary`) carry a
    /// real [`HotbarKeybindIcon`] pointing at `mouse_click_left.png`/
    /// `mouse_click_right.png` rather than a text label.
    #[test]
    fn hotbar_slots_show_the_two_by_three_keybind_labels_and_mouse_icons() {
        let mut app = new_app();
        app.insert_resource(xindeler_input::KeyMap::default());
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn all 12 holders");
        app.update();
        app.world_mut()
            .run_system_once(sync_keybind_labels)
            .expect("labels sync from the live KeyMap");

        let slot_entities = app.world().resource::<HotbarSlotEntities>().0.clone();
        let images = app.world().resource::<HudImages>().clone();

        // Number holders show their live binding.
        assert_eq!(
            find_keybind_label_text(app.world(), slot_entities[7]).as_deref(),
            Some("8"),
            "index 7 is Slot8 -> \"8\""
        );
        assert_eq!(
            find_keybind_label_text(app.world(), slot_entities[9]).as_deref(),
            Some("9"),
            "index 9 is Slot9 -> \"9\""
        );
        assert_eq!(
            find_keybind_label_text(app.world(), slot_entities[10]).as_deref(),
            Some("0"),
            "index 10 is Slot10 -> \"0\" (rebound onto Digit0 this pass)"
        );

        // Mouse-button holders (indices 8, 11) carry an icon, not a text label.
        for mouse_index in [8usize, 11] {
            assert!(
                find_keybind_label_text(app.world(), slot_entities[mouse_index]).is_none(),
                "slot {mouse_index} is a mouse-button holder and must carry no text keybind label"
            );
        }
        assert_eq!(
            find_keybind_icon_handle(app.world(), slot_entities[8]),
            Some(images.get(HudImageKey::MouseClickLeft)),
            "index 8 (right block row 1, col 3) must show the left-mouse-click icon"
        );
        assert_eq!(
            find_keybind_icon_handle(app.world(), slot_entities[11]),
            Some(images.get(HudImageKey::MouseClickRight)),
            "index 11 (right block row 2, col 3) must show the right-mouse-click icon"
        );
        assert!(
            find_keybind_icon_handle(app.world(), slot_entities[7]).is_none(),
            "a plain numbered slot must not carry a mouse-click icon"
        );
    }

    /// Regression test for the "two overlapping rectangles" bug (Matías's
    /// screenshot, this module doc comment's own "Bugfix" section): a spawned
    /// hotbar slot's `BackgroundColor`/`BorderColor` — the flat chrome
    /// `slot_bundle` gives every slot by default — must be fully transparent,
    /// NOT the opaque `theme.palette.panel_bg`/`panel_border` bag/equip/trade
    /// slots keep (`xindeler_ui::slot`'s own default). Only the hotbar
    /// overrides these to `Color::NONE`, since [`SkillSlotBorderOverlay`]'s
    /// `skill_slot_border.png` is already the slot's ONLY intended frame —
    /// same "drop the flat chrome, let the art be the only frame" treatment
    /// PR #112 (`map_view.rs`'s `MinimapPanelRoot`) used for the minimap's
    /// analogous square-frame bug. Also asserts the
    /// [`xindeler_ui::slot::ChromelessSlot`] marker is present, so the
    /// global drag observers restore this same `Color::NONE` resting state
    /// (not the opaque theme colours) after a drag ends on this slot — see
    /// that marker's own doc comment.
    #[test]
    fn sync_hotbar_slots_carries_no_generic_panel_chrome() {
        use bevy::color::Alpha;

        let mut app = new_app();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default()],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("sync_hotbar_slots runs");
        app.update();

        // BL-82 EM-5.17 "5+5 slot-holders" follow-up: always HOTBAR_SLOT_COUNT
        // holders now, regardless of the mirror's real (here: 1) slot count —
        // see that constant's own doc comment. This test only cares about
        // entity 0's chrome, so the exact total isn't its focus, but the
        // assertion must match reality.
        let slot_entities = app.world().resource::<HotbarSlotEntities>();
        assert_eq!(slot_entities.0.len(), HOTBAR_SLOT_COUNT);
        let slot_entity = slot_entities.0[0];

        let world = app.world();
        let background = world
            .get::<BackgroundColor>(slot_entity)
            .expect("Node requires BackgroundColor to be present (as a component)");
        assert!(
            background.0.is_fully_transparent(),
            "a hotbar slot's BackgroundColor must be fully transparent, got {:?} — an opaque fill \
             here draws a second flat rectangle underneath SkillSlotBorderOverlay's ornate frame \
             art",
            background.0
        );

        let border = world
            .get::<BorderColor>(slot_entity)
            .expect("Node requires BorderColor to be present (as a component)");
        assert!(
            border.is_fully_transparent(),
            "a hotbar slot's BorderColor must be fully transparent, got {border:?} — same \
             doubled-rectangle bug as the background chrome"
        );

        assert!(
            world.get::<ChromelessSlot>(slot_entity).is_some(),
            "a hotbar slot must carry ChromelessSlot so xindeler_ui::slot's global drag observers \
             restore Color::NONE (not the opaque theme panel colours) once a drag touching this \
             slot ends/leaves/drops — see ChromelessSlot's own doc comment"
        );
    }

    /// BL-82 HUD polish round 5 — regression guard for the "half-empty slot"
    /// bug ([`SkillSlotBorderOverlay`]'s own doc comment): the spawned
    /// overlay's `ImageNode` must carry the tight
    /// [`hud_layout::SKILL_SLOT_BORDER_SOURCE_CROP`] rect with
    /// `NodeImageMode::Stretch`, not a plain uncropped `ImageNode::new` — a
    /// future regression back to the uncropped form would silently
    /// reintroduce the big visible gaps between individual hotbar slots no
    /// `HOTBAR_SLOT_GAP_PX` tuning alone can fix.
    #[test]
    fn skill_slot_border_overlay_uses_the_tight_opaque_crop() {
        let mut app = new_app();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default()],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("sync_hotbar_slots runs");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>().0.clone();
        let slot_entity = slot_entities[0];

        let world = app.world();
        let children = world
            .get::<Children>(slot_entity)
            .expect("the slot has children");
        let overlay = children
            .iter()
            .find(|&child| world.get::<SkillSlotBorderOverlay>(child).is_some())
            .expect("the slot has a SkillSlotBorderOverlay child");

        let image_node = world
            .get::<ImageNode>(overlay)
            .expect("the overlay carries an ImageNode");
        assert_eq!(
            image_node.rect,
            Some(crate::hud_layout::SKILL_SLOT_BORDER_SOURCE_CROP),
            "the overlay must crop to the tight opaque bounding box, not render the full \
             half-transparent canvas"
        );
        assert_eq!(
            image_node.image_mode,
            bevy::ui::widget::NodeImageMode::Stretch,
            "the cropped rect must be stretched onto the slot's square box"
        );
    }

    /// BL-82 HUD overhaul (goal #1) — [`SkillSlotBackground`] is now the EMPTY
    /// slot's grey-stone centre cue, not round 7's opaque black full-box plate
    /// (which Matías rejected as too heavy). Pins: it spawns UNDER the border
    /// frame (drawn beneath the ring), spawns hidden (`Display::None` — a
    /// filled slot must never flash it), fills only the ring's centre hole
    /// ([`EMPTY_SLOT_STONE_INSET_PX`] inset, NOT the full box, so the frame's
    /// notches stay see-through), and its colour is the theme's translucent
    /// mid-grey `slot_empty_stone` role, not a hardcoded literal and not opaque
    /// near-black.
    #[test]
    fn empty_slot_stone_cue_is_an_inset_translucent_grey_beneath_the_frame() {
        use bevy::color::Alpha;

        let mut app = new_app();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default()],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("sync_hotbar_slots runs");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>().0.clone();
        let slot_entity = slot_entities[0];

        let world = app.world();
        let children = world
            .get::<Children>(slot_entity)
            .expect("the slot has children");

        let bg_pos = children
            .iter()
            .position(|child| world.get::<SkillSlotBackground>(child).is_some())
            .expect("the slot has a SkillSlotBackground child");
        let border_pos = children
            .iter()
            .position(|child| world.get::<SkillSlotBorderOverlay>(child).is_some())
            .expect("the slot has a SkillSlotBorderOverlay child");
        assert!(
            bg_pos < border_pos,
            "SkillSlotBackground must be spawned BEFORE SkillSlotBorderOverlay so the stone cue \
             renders UNDER the ornate frame, not over it (got bg at {bg_pos}, border at \
             {border_pos})"
        );

        let bg_entity = children[bg_pos];
        let fill = world
            .get::<BackgroundColor>(bg_entity)
            .expect("the background child carries a BackgroundColor");
        assert!(
            !fill.0.is_fully_transparent(),
            "the stone cue must be visible (not fully transparent): {:?}",
            fill.0
        );
        assert_eq!(
            fill.0,
            app.world().resource::<HudTheme>().palette.slot_empty_stone,
            "the stone cue must resolve against the theme's slot_empty_stone role, not a literal"
        );

        let node = app
            .world()
            .get::<Node>(bg_entity)
            .expect("the background child carries a Node");
        assert_eq!(
            node.display,
            Display::None,
            "SkillSlotBackground must spawn hidden — sync_empty_slot_backgrounds reveals it only \
             for empty slots, so a filled slot never flashes a stone cue on the first frame"
        );
        // Inset to the ring's centre hole, NOT the full box — leaving the outer
        // band clear keeps the frame's own concave notches see-through (goal
        // #1's revert of the round-7 full-box plate).
        let inset_size = SLOT_SIZE_PX - 2.0 * EMPTY_SLOT_STONE_INSET_PX;
        assert_eq!(node.width, Val::Px(inset_size));
        assert_eq!(node.height, Val::Px(inset_size));
        assert_eq!(
            node.top,
            Val::Px(EMPTY_SLOT_STONE_INSET_PX - SLOT_BORDER_PX)
        );
        assert_eq!(
            node.left,
            Val::Px(EMPTY_SLOT_STONE_INSET_PX - SLOT_BORDER_PX)
        );
    }

    /// BL-82 HUD overhaul (goal #1) — [`sync_empty_slot_backgrounds`] toggles
    /// each slot's stone cue by its content: shown (`Display::Flex`) while the
    /// slot is empty, hidden (`Display::None`) once it holds an ability (the
    /// icon then occupies the centre). Regression guard so a filled slot never
    /// shows the "empty socket" cue behind its icon and an empty one always
    /// does.
    #[test]
    fn sync_empty_slot_backgrounds_toggles_the_stone_cue_by_content() {
        let mut app = new_app();
        // Slot 0 filled with a real ability, slot 1 an in-range empty.
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Innate(0),
                    ability_id: Some("class.warrior.rally".to_owned()),
                },
                NetHotbarSlot::default(),
            ],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn holders + write content");
        app.update();
        app.world_mut()
            .run_system_once(sync_empty_slot_backgrounds)
            .expect("toggle the stone cues");

        let slot_entities = app.world().resource::<HotbarSlotEntities>().0.clone();
        let stone_display = |world: &World, slot: Entity| -> Display {
            let children = world.get::<Children>(slot).expect("slot has children");
            for child in children.iter() {
                if world.get::<SkillSlotBackground>(child).is_some() {
                    return world.get::<Node>(child).unwrap().display;
                }
            }
            panic!("slot has no SkillSlotBackground child");
        };

        assert_eq!(
            stone_display(app.world(), slot_entities[0]),
            Display::None,
            "a FILLED slot must hide its stone cue (the icon occupies the centre)"
        );
        assert_eq!(
            stone_display(app.world(), slot_entities[1]),
            Display::Flex,
            "an EMPTY slot must show its stone cue"
        );
        // A placeholder holder beyond the mirror's real slots is also empty ->
        // shows the cue.
        assert_eq!(
            stone_display(app.world(), slot_entities[HOTBAR_SLOT_COUNT - 1]),
            Display::Flex,
            "an out-of-range placeholder holder is empty and must show the stone cue"
        );
    }

    /// BL-82 HUD redesign round 6 — replaces the deleted
    /// `action_bar_halves_stretch_their_background_to_fill_the_box` (that
    /// test guarded round 5's `NodeImageMode::Stretch` fix for the
    /// `action_bar_bg_left.png`/`_right.png` `ImageNode`s; round 6 deletes
    /// those `ImageNode`s entirely — see `spawn_slot_row_half`'s own doc
    /// comment). The real invariant round 6 needs pinned instead: neither
    /// slot-row half carries an `ImageNode` at all (no frame art rendering,
    /// per Matías's `hud-ejemplo-2.png` reference), and each half's box is
    /// sized to EXACTLY [`hud_layout::SLOT_ROW_WIDTH_PX`]×[`hud_layout::
    /// SLOT_ROW_HEIGHT_PX`] positioned per `hud_layout::CLUSTER`'s own
    /// `slot_row_left_half_left`/`slot_row_right_half_left` offsets — a
    /// future regression that re-adds a background image, or drifts the
    /// row's own size/position away from `hud_layout`'s arithmetic, fails
    /// here.
    #[test]
    fn slot_row_halves_have_no_background_art_and_match_hud_layout_geometry() {
        let mut app = new_app();
        app.world_mut()
            .run_system_once(spawn_hotbar)
            .expect("spawn_hotbar runs");
        app.update();

        let world = app.world();
        let left_half = world
            .iter_entities()
            .find(|e| world.get::<HotbarLeftHalf>(e.id()).is_some())
            .expect("spawn_hotbar spawns a HotbarLeftHalf entity")
            .id();
        let right_half = world
            .iter_entities()
            .find(|e| world.get::<HotbarRightHalf>(e.id()).is_some())
            .expect("spawn_hotbar spawns a HotbarRightHalf entity")
            .id();

        let expectations = [
            (
                "left",
                left_half,
                hud_layout::CLUSTER.slot_row_left_half_left,
            ),
            (
                "right",
                right_half,
                hud_layout::CLUSTER.slot_row_right_half_left,
            ),
        ];
        for (name, half, expected_left_offset) in expectations {
            assert!(
                world.get::<ImageNode>(half).is_none(),
                "the {name} slot-row half must NOT carry an ImageNode — the ornate action-bar \
                 frame art is removed entirely per the HUD redesign, the row is a bare \
                 transparent container"
            );
            let node = world
                .get::<Node>(half)
                .unwrap_or_else(|| panic!("the {name} slot-row half carries a Node"));
            assert_eq!(node.width, Val::Px(hud_layout::SLOT_ROW_WIDTH_PX));
            assert_eq!(node.height, Val::Px(hud_layout::SLOT_ROW_HEIGHT_PX));
            assert_eq!(node.bottom, Val::Px(hud_layout::CLUSTER_BOTTOM_PX));
            assert_eq!(node.margin.left, Val::Px(expected_left_offset));
        }
    }

    /// BL-82 EM-5.17 "5+5 slot-holders" follow-up: a LATER change to
    /// `NetAbilities` (fewer real slots — e.g. a weapon swap to a context
    /// with a shorter aux set) does NOT despawn any holder entities anymore
    /// — the entity count stays pinned at [`HOTBAR_SLOT_COUNT`] regardless;
    /// only the CONTENT of the now-out-of-range slots reverts to an empty
    /// placeholder.
    #[test]
    fn sync_hotbar_slots_keeps_all_holders_and_clears_content_when_the_mirror_shrinks() {
        let mut app = new_app();
        let player = app
            .world_mut()
            .spawn((NetLocalPlayer, NetAbilities {
                primary: None,
                secondary: None,
                slots: vec![
                    NetHotbarSlot {
                        aux: NetAuxiliaryAbility::Innate(0),
                        ability_id: Some("class.warrior.rally".to_owned()),
                    },
                    NetHotbarSlot::default(),
                    NetHotbarSlot::default(),
                ],
            }))
            .id();
        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("first run");
        app.update();
        assert_eq!(
            app.world().resource::<HotbarSlotEntities>().0.len(),
            HOTBAR_SLOT_COUNT
        );

        app.world_mut()
            .get_mut::<NetAbilities>(player)
            .unwrap()
            .slots = vec![NetHotbarSlot::default()];
        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("second run");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>();
        assert_eq!(
            slot_entities.0.len(),
            HOTBAR_SLOT_COUNT,
            "holder entities must never be despawned — only their content changes"
        );
        let world = app.world();
        let now_placeholder = world.get::<SlotContents>(slot_entities.0[0]).unwrap();
        assert_eq!(
            now_placeholder.icon_text, "",
            "a slot that lost its real content when the mirror shrank must clear to a placeholder"
        );
        assert_eq!(now_placeholder.tooltip, "Empty");
    }

    /// A [`SlotDropped`] entirely within the hotbar group swaps the two
    /// slots' `aux` values via two `AssignHotbarSlot` client messages — the
    /// literal EM-5.3 drag-to-assign acceptance bar.
    #[test]
    fn hotbar_internal_drag_drop_swaps_via_two_assign_messages() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<AssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::MainWeapon(0),
                    ability_id: Some("a".to_owned()),
                },
                NetHotbarSlot {
                    aux: NetAuxiliaryAbility::Innate(2),
                    ability_id: Some("b".to_owned()),
                },
            ],
        }));
        app.world_mut().write_message(SlotDropped {
            from_group: HOTBAR_GROUP,
            from_address: SlotAddress(0),
            to_group: HOTBAR_GROUP,
            to_address: SlotAddress(1),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<AssignHotbarSlot>>()
            .drain()
            .collect();
        assert_eq!(sent.len(), 2);
        assert!(sent.contains(&AssignHotbarSlot {
            slot: 1,
            ability: NetAuxiliaryAbility::MainWeapon(0),
        }));
        assert!(sent.contains(&AssignHotbarSlot {
            slot: 0,
            ability: NetAuxiliaryAbility::Innate(2),
        }));
    }

    /// BL-82 EM-5.7 (T56.24): a drag FROM the diary's Abilities tab
    /// (`crate::diary::DIARY_ABILITY_GROUP`) INTO a hotbar slot binds the
    /// packed ability into that slot via ONE real `AssignHotbarSlot` (EM-5.3's
    /// follow-up fix converged this onto the same real client message the
    /// hotbar-internal swap uses, retiring the old listen-server-only
    /// `LocalAssignHotbarSlot` shortcut) — no `abilities` lookup needed for
    /// the source side, the ability is decoded straight from the dragged
    /// address.
    #[test]
    fn diary_ability_drag_binds_the_dragged_ability_into_the_target_slot() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<AssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default(), NetHotbarSlot::default()],
        }));
        let dragged = NetAuxiliaryAbility::Innate(3);
        app.world_mut().write_message(SlotDropped {
            from_group: crate::diary::DIARY_ABILITY_GROUP,
            from_address: SlotAddress(dragged.to_slot_address_raw()),
            to_group: HOTBAR_GROUP,
            to_address: SlotAddress(1),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<AssignHotbarSlot>>()
            .drain()
            .collect();
        assert_eq!(sent, vec![AssignHotbarSlot {
            slot: 1,
            ability: dragged,
        }]);
    }

    /// BL-82 EM-5.17 "5+5 slot-holders" follow-up: a diary-ability drag onto
    /// a PLACEHOLDER holder (`to_index >= abilities.slots.len()`, i.e. beyond
    /// the mirror's real slot count) is a no-op — no `AssignHotbarSlot` is
    /// sent — the client-side guard `handle_hotbar_drag_drop` added for
    /// exactly this case (see its own doc comment: the server would silently
    /// discard the same request anyway, so this just skips sending it).
    #[test]
    fn diary_ability_drag_onto_a_placeholder_slot_sends_nothing() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<AssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default(), NetHotbarSlot::default()],
        }));
        let dragged = NetAuxiliaryAbility::Innate(3);
        app.world_mut().write_message(SlotDropped {
            from_group: crate::diary::DIARY_ABILITY_GROUP,
            from_address: SlotAddress(dragged.to_slot_address_raw()),
            to_group: HOTBAR_GROUP,
            // Only 2 real slots exist (indices 0..1) — index 5 is a
            // placeholder holder (well within HOTBAR_SLOT_COUNT == 10, so
            // the entity itself exists, but the mirror has no real slot
            // there).
            to_address: SlotAddress(5),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<AssignHotbarSlot>>()
            .drain()
            .collect();
        assert!(
            sent.is_empty(),
            "a drop onto a placeholder slot must not send AssignHotbarSlot"
        );
    }

    /// A drop where either end is NOT in the hotbar group is ignored — no
    /// message is written (the "silently ignored, not mis-applied"
    /// contract for a future cross-screen drag source).
    #[test]
    fn cross_group_drop_is_ignored() {
        let mut app = new_app();
        app.add_message::<SlotDropped>();
        app.add_message::<AssignHotbarSlot>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default(), NetHotbarSlot::default()],
        }));
        app.world_mut().write_message(SlotDropped {
            from_group: SlotGroup(99),
            from_address: SlotAddress(0),
            to_group: HOTBAR_GROUP,
            to_address: SlotAddress(1),
        });

        app.world_mut()
            .run_system_once(handle_hotbar_drag_drop)
            .expect("handler runs");

        let sent: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<AssignHotbarSlot>>()
            .drain()
            .collect();
        assert!(sent.is_empty());
    }

    /// The cooldown overlay's height fraction is derived from
    /// `remaining/total` where `total` is the FIRST-observed remaining value
    /// for that ability id — and clears back to 0% once the cooldown entry
    /// disappears (ability ready).
    #[test]
    fn cooldown_overlay_tracks_remaining_over_inferred_total() {
        let mut app = new_app();
        let player = app
            .world_mut()
            .spawn((
                NetLocalPlayer,
                NetAbilities {
                    primary: None,
                    secondary: None,
                    slots: vec![NetHotbarSlot {
                        aux: NetAuxiliaryAbility::Innate(0),
                        ability_id: Some("class.warrior.rally".to_owned()),
                    }],
                },
                NetCooldowns(vec![NetCooldownEntry {
                    ability_id: "class.warrior.rally".to_owned(),
                    remaining_secs: 8.0,
                }]),
            ))
            .id();

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn the slot entity first");
        app.update();

        // `sync_cooldown_overlays` is registered as a REAL, persistent
        // system (not called via `run_system_once` a second time) — its
        // `Local<HashMap<String, f32>>` "inferred total" state must survive
        // across ticks the same way it does in the real app (`Update`
        // schedule); `run_system_once` would re-register a fresh one-shot
        // system EVERY call, silently resetting that `Local` to empty each
        // time and defeating the whole "infer the total from the first
        // observation" heuristic this test exists to verify.
        app.add_systems(Update, sync_cooldown_overlays);
        app.update();

        let slot_entity = app.world().resource::<HotbarSlotEntities>().0[0];
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(slot_entity)
            .unwrap()
            .iter()
            .collect();
        let overlay = children
            .iter()
            .copied()
            .find(|&e| app.world().get::<HotbarCooldownOverlay>(e).is_some())
            .expect("overlay child exists");
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().height,
            Val::Percent(100.0),
            "at first observation, remaining == inferred total -> full overlay"
        );

        // Halfway through the cooldown: remaining is 4.0 of the inferred 8.0
        // total -> 50%.
        app.world_mut().get_mut::<NetCooldowns>(player).unwrap().0[0].remaining_secs = 4.0;
        app.update();
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().height,
            Val::Percent(50.0)
        );

        // Ability ready: the cooldown entry disappears -> overlay clears.
        app.world_mut()
            .get_mut::<NetCooldowns>(player)
            .unwrap()
            .0
            .clear();
        app.update();
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().height,
            Val::Percent(0.0)
        );
    }

    /// BL-82 EM-5.17 Phase 2 bugfix regression guard:
    /// [`HotbarCooldownOverlay`]'s `BackgroundColor` must be SOURCED from
    /// [`HudTheme::palette`]'s `cooldown_overlay` role, not a hardcoded
    /// literal — the module doc comment's "Bugfix" section explains why a
    /// raw `Color::srgba(0.0, 0.0, 0.0, 0.7)` literal here was the actual
    /// root cause of the reported "sweep never shows" bug (it composited
    /// invisibly over Phase 2's near-black `skill_slot_border.png`, not a
    /// z-order or logic problem — both of those were already correct and
    /// already covered by
    /// [`cooldown_overlay_tracks_remaining_over_inferred_total`] above). This
    /// test uses a deliberately non-default theme colour so it can't pass by
    /// coincidentally matching a default; it guards against a future
    /// refactor silently reintroducing a hardcoded literal that bypasses the
    /// theme (and, with it, the luminance-floor regression test on
    /// `HudPalette::cooldown_overlay` in `xindeler-ui::theme`).
    #[test]
    fn cooldown_overlay_background_colour_comes_from_the_theme() {
        let mut app = new_app();
        let mut theme = HudTheme::default();
        theme.palette.cooldown_overlay = Color::srgba(0.1, 0.9, 0.1, 0.5);
        app.insert_resource(theme);
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot {
                aux: NetAuxiliaryAbility::Innate(0),
                ability_id: Some("class.warrior.rally".to_owned()),
            }],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn the slot entity");
        app.update();

        let slot_entity = app.world().resource::<HotbarSlotEntities>().0[0];
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(slot_entity)
            .unwrap()
            .iter()
            .collect();
        let overlay = children
            .iter()
            .copied()
            .find(|&e| app.world().get::<HotbarCooldownOverlay>(e).is_some())
            .expect("overlay child exists");
        let background = app.world().get::<BackgroundColor>(overlay).unwrap();
        assert_eq!(background.0, Color::srgba(0.1, 0.9, 0.1, 0.5));
    }

    /// [`HOTBAR_SLOT_COUNT`] is FIXED at [`SLOTS_PER_HALF`]`*2`, so
    /// `div_ceil(2)` always gives an exact even split
    /// ([`SLOTS_PER_HALF`] each side) — even when the mirror currently reports
    /// FEWER real slots than that (here: 3), the first [`SLOTS_PER_HALF`]
    /// HOLDER entities go to the LEFT half and the rest to the RIGHT. Also the
    /// literal regression guard for the "5+5 slot-holders" bug: with the OLD
    /// "entity count == mirror length" behaviour, `div_ceil` on the shorter
    /// mirror length would have split the halves unevenly.
    #[test]
    fn slot_half_parenting_splits_evenly_regardless_of_mirror_length() {
        let mut app = new_app();
        let left_half = app.world_mut().spawn(HotbarLeftHalf).id();
        let right_half = app.world_mut().spawn(HotbarRightHalf).id();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![
                NetHotbarSlot::default(),
                NetHotbarSlot::default(),
                NetHotbarSlot::default(),
            ],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn all holders (3 real + placeholders)");
        app.update();
        app.world_mut()
            .run_system_once(sync_slot_half_parenting)
            .expect("split across halves");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>().0.clone();
        assert_eq!(slot_entities.len(), HOTBAR_SLOT_COUNT);

        let left_children: Vec<Entity> = app
            .world()
            .get::<Children>(left_half)
            .expect("left half got children")
            .iter()
            .collect();
        let right_children: Vec<Entity> = app
            .world()
            .get::<Children>(right_half)
            .expect("right half got children")
            .iter()
            .collect();

        assert_eq!(left_children, slot_entities[0..SLOTS_PER_HALF]);
        assert_eq!(
            right_children,
            slot_entities[SLOTS_PER_HALF..HOTBAR_SLOT_COUNT]
        );
    }

    /// A LATER change to the mirror's real slot COUNT (e.g. a weapon swap)
    /// must NOT change the even split — unlike the pre-fix behaviour (where
    /// the entity count itself tracked the mirror and a count change could
    /// shift the `div_ceil` boundary), the holder count is fixed today, so the
    /// same [`SLOTS_PER_HALF`] entities stay in each half no matter how
    /// `NetAbilities` changes.
    #[test]
    fn slot_half_parenting_stays_even_when_mirror_length_changes() {
        let mut app = new_app();
        let left_half = app.world_mut().spawn(HotbarLeftHalf).id();
        let right_half = app.world_mut().spawn(HotbarRightHalf).id();
        let player = app
            .world_mut()
            .spawn((NetLocalPlayer, NetAbilities {
                primary: None,
                secondary: None,
                slots: vec![NetHotbarSlot::default(), NetHotbarSlot::default()],
            }))
            .id();

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("spawn all holders (2 real + placeholders)");
        app.update();
        app.world_mut()
            .run_system_once(sync_slot_half_parenting)
            .expect("split across halves");
        app.update();
        assert_eq!(
            app.world()
                .get::<Children>(left_half)
                .unwrap()
                .iter()
                .count(),
            SLOTS_PER_HALF
        );
        assert_eq!(
            app.world()
                .get::<Children>(right_half)
                .unwrap()
                .iter()
                .count(),
            SLOTS_PER_HALF
        );

        app.world_mut()
            .get_mut::<NetAbilities>(player)
            .unwrap()
            .slots = vec![
            NetHotbarSlot::default(),
            NetHotbarSlot::default(),
            NetHotbarSlot::default(),
        ];
        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("still all holders (3 real + placeholders)");
        app.update();
        app.world_mut()
            .run_system_once(sync_slot_half_parenting)
            .expect("split stays stable");
        app.update();

        assert_eq!(
            app.world()
                .get::<Children>(left_half)
                .unwrap()
                .iter()
                .count(),
            SLOTS_PER_HALF,
            "the split must stay even after the mirror's real slot count changes"
        );
        assert_eq!(
            app.world()
                .get::<Children>(right_half)
                .unwrap()
                .iter()
                .count(),
            SLOTS_PER_HALF
        );
    }

    /// BL-82 EM-5.16 (T56.44 follow-up): switching the active locale
    /// re-localizes a placeholder slot's "Empty" tooltip live, using the REAL
    /// repo `common.ftl` catalog (via `VELOREN_ASSETS`/`XINDELER_ASSETS`) —
    /// the same real-catalog hot-swap idiom `esc_menu.rs`'s/`combat_hud.rs`'s
    /// own tests use. `SlotContents.tooltip` isn't `LocalizedText`/
    /// `LocalizedLabel`-tagged (it's a plain field `xindeler_ui::slot::
    /// update_slot_visuals` copies into a `Tooltip` component, in a crate
    /// this task doesn't own) — instead, exactly like `settings_window.rs`'s
    /// `SettingValueLabel`/`refresh_setting_labels` pair, `sync_hotbar_slots`
    /// itself re-resolves `common-empty` from the live `Localization` on
    /// every run (see that system's own doc comment for why it re-reads
    /// unconditionally every frame), so simply re-running it after a locale
    /// flip is the correct way to prove the hot-swap.
    #[test]
    fn switching_locale_relocalizes_the_empty_slot_tooltip_live() {
        let mut app = new_app();
        app.init_resource::<xindeler_ui::i18n::CurrentLocale>();
        app.world_mut().spawn((NetLocalPlayer, NetAbilities {
            primary: None,
            secondary: None,
            slots: vec![NetHotbarSlot::default()],
        }));

        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("first run resolves the en tooltip");
        app.update();

        let slot_entities = app.world().resource::<HotbarSlotEntities>().0.clone();
        let empty_tooltip = |world: &World| -> String {
            world
                .get::<SlotContents>(slot_entities[0])
                .expect("slot 0 has SlotContents")
                .tooltip
                .clone()
        };

        assert_eq!(
            empty_tooltip(app.world()),
            "Empty",
            "the placeholder tooltip must show the real en catalog's common-empty value at first \
             sync"
        );

        app.world_mut()
            .resource_mut::<xindeler_ui::i18n::CurrentLocale>()
            .0 = "es-419".to_owned();
        app.world_mut()
            .run_system_once(xindeler_ui::i18n::reload_localization_on_locale_change)
            .expect("reload runs");
        app.world_mut()
            .run_system_once(sync_hotbar_slots)
            .expect("second run re-resolves against the new locale");
        app.update();

        assert_eq!(
            empty_tooltip(app.world()),
            "Vacío",
            "must resolve to the REAL es-419 catalog's own common-empty value, not the en fallback"
        );
    }
}
