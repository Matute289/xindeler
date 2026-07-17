//! BL-82 EM-5.17 T57.10 — the shared HUD z-index vocabulary (design spec
//! §4.4).
//!
//! Today, **no `ZIndex`/`GlobalZIndex` exists anywhere in the HUD** (spec
//! §1.1) — draw/pick order is pure spawn-order, a real latent bug (spec §2
//! Bug D's "no `ZIndex` anywhere is a real latent pick-order risk versus the
//! hotbar's full-width band"). This module is ONLY the shared numeric
//! vocabulary every later phase's panel should apply via
//! [`bevy::ui::GlobalZIndex`] as it touches that panel — it does NOT itself
//! apply a `GlobalZIndex` to any existing HUD entity (that stays each
//! phase's own job as it reskins its own panel; retrofitting every current
//! panel here would be well outside this phase's "foundation, not every
//! screen" scope). Without this shared table, each later phase would
//! otherwise invent its own ad-hoc z-index numbers, risking silent
//! collisions between e.g. Phase 2's action bar and Phase 4's party frames.
//!
//! ## The scheme (spec §4.4, verbatim ordering, + round-3 addition below)
//! ```text
//! WORLD_OVERLAY (0)
//!   < ORBS / ACTION_BAR / PARTY_FRAMES / MINIMAP (20)
//!   < AMBIENT_CHROME_OVERLAY (21)
//!   < BOSS_NAMEPLATE (25)
//!   < CHAT (30)
//!   < MODAL_WINDOWS (100)
//!   < TOOLTIP (200)
//!   < TOAST (300)
//! ```
//! Rationale for the ordering: always-on ambient HUD chrome (orbs, action
//! bar, party frames, minimap) sits just above anything rendered as a
//! world-space overlay (e.g. in-world overhead health bars, EM-5.2); a
//! decorative element that deliberately overlaps a SIBLING within that same
//! ambient layer (BL-82 EM-5.17 orb-crop round 3 — see
//! [`AMBIENT_CHROME_OVERLAY`]'s own doc comment) sits one notch above it, the
//! same way the boss nameplate sits one notch above THAT so it's never
//! occluded by it; chat sits above the ambient layer too (it can be
//! interacted with while other HUD chrome is visible) but below anything
//! that should temporarily own the whole screen (modal windows — diary,
//! inventory, full map); tooltips must always draw over a modal window that
//! spawned them; toasts (queued notifications) sit above literally
//! everything, including tooltips, since they're transient and
//! time-boxed — the user should never miss one behind another panel.
//!
//! Each constant is a plain `i32`, ready to wrap in
//! [`bevy::ui::GlobalZIndex`] at the call site (e.g. `GlobalZIndex(zlayer::
//! ORBS_ACTION_BAR_PARTY_MINIMAP)`) — this module intentionally does not
//! wrap them itself, so a caller composing a `GlobalZIndex` alongside other
//! components in one spawn tuple doesn't need an extra `.0` unwrap.

/// Anything rendered as a world-space overlay projected into screen space
/// (e.g. in-world overhead health bars over other mirrored entities,
/// EM-5.2) — the lowest HUD-adjacent layer, since it's conceptually still
/// "in the world," not chrome.
pub const WORLD_OVERLAY: i32 = 0;

/// Always-on ambient HUD chrome: resource orbs + action bar (Phase 2), party
/// frames (Phase 4), and the minimap (Phase 3) — all share one layer since
/// none of them should ever occlude another; they live in disjoint screen
/// regions by layout, not by z-order.
pub const ORBS_ACTION_BAR_PARTY_MINIMAP: i32 = 20;

/// A decorative element that intentionally spills PAST its own entity's
/// bounding box onto a NEIGHBOURING ambient-chrome sibling — BL-82 EM-5.17
/// orb-crop round 3: the resource orbs' `orb_frame_*.png` art is a wide,
/// asymmetric statue/gargoyle-wing/tentacle illustration around a circular
/// hole, genuinely wider than the orb's own square hit-box (measured:
/// angel `879px`, cuthulhu `1165px` art vs the `768px`-tall canvas a square
/// crop is bounded by) — no square crop can show the full wings without
/// clipping them (see `xindeler-client::hud_layout`'s `*_FRAME_SOURCE_CROP`
/// doc comments for the measurements). The round-3 fix renders the frame in
/// its own NON-square, wider-than-the-orb box instead of forcing a square
/// crop, which means the wings now genuinely overlap the neighbouring
/// action-bar background piece (or, for the mana orb's outward-facing wing,
/// empty space) by design. Since `combat_hud.rs`'s orbs and `hotbar.rs`'s
/// action-bar backgrounds are two INDEPENDENT `Startup`-spawn plugins with
/// no ordering between them (see `hud_layout.rs`'s own module doc comment),
/// both currently carry the SAME [`ORBS_ACTION_BAR_PARTY_MINIMAP`]
/// `GlobalZIndex` — leaving the now-overlapping wing's paint order to
/// undefined sibling tie-breaking would make the wing randomly render
/// UNDER the action bar background on some runs (looking exactly like the
/// clipping bug this fix exists to solve). This tier, one notch above the
/// ambient layer for the same "never occluded by a sibling" reason
/// [`BOSS_NAMEPLATE`] already sits one notch above it, makes the overlap
/// deterministic: the decorative overlay always wins.
pub const AMBIENT_CHROME_OVERLAY: i32 = ORBS_ACTION_BAR_PARTY_MINIMAP + 1;

/// The boss/target nameplate (Phase 5) — one notch above the ambient layer
/// so it's never hidden behind it even if a future layout change causes an
/// overlap.
pub const BOSS_NAMEPLATE: i32 = 25;

/// The chat panel — above ambient HUD chrome (it's interactive and often
/// needs to sit visually forward of it) but below anything that temporarily
/// takes over the screen.
pub const CHAT: i32 = 30;

/// Modal/full-screen windows: diary (Phase 6), inventory (Phase 7), the
/// full (non-mini) map. These temporarily own the screen and must draw over
/// every always-on panel below this line.
pub const MODAL_WINDOWS: i32 = 100;

/// A modal opened from WITHIN another already-open modal (BL-82 EM-5.18
/// T58.9) — the equip-picker (a click-to-equip item list opened from inside
/// the already-open Inventory window) is the first consumer, but not
/// necessarily the only one: any future "modal-on-modal" screen should reuse
/// this same tier rather than inventing its own ad-hoc number.
pub const MODAL_WINDOWS_STACKED: i32 = 150;

/// Hover tooltips — must always draw over whatever spawned them, including
/// a modal window's own tooltip (e.g. an inventory item's tooltip while the
/// inventory modal is open).
pub const TOOLTIP: i32 = 200;

/// Queued toast notifications ([`crate::notification`]) — the topmost
/// layer. Transient and time-boxed, so nothing should ever occlude one.
pub const TOAST: i32 = 300;

#[cfg(test)]
mod tests {
    use super::*;

    /// The scheme's ordering is exactly the one spec §4.4 specifies —
    /// pins the constants against an accidental reordering as new layers
    /// are added later.
    #[test]
    fn layers_are_strictly_increasing_in_spec_order() {
        let ordered = [
            WORLD_OVERLAY,
            ORBS_ACTION_BAR_PARTY_MINIMAP,
            AMBIENT_CHROME_OVERLAY,
            BOSS_NAMEPLATE,
            CHAT,
            MODAL_WINDOWS,
            MODAL_WINDOWS_STACKED,
            TOOLTIP,
            TOAST,
        ];
        for pair in ordered.windows(2) {
            assert!(
                pair[0] < pair[1],
                "z-layer scheme must be strictly increasing: {pair:?}"
            );
        }
    }
}
