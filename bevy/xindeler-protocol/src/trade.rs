//! BL-82 EM-5.6 — the two-party trade mirror (spec §3.2/§6, §9 Q7=A "full
//! parity, not sub-scoped"; tasks T56.18/T56.21).
//!
//! ## Where trade state actually lives sim-side
//! The sim already has the WHOLE trade state machine (`common::trade`,
//! `server::events::trade`/`invite`) — invites are a per-entity
//! `common::comp::invite::Invite` component on the INVITEE, and an
//! accepted trade is a `common::trade::PendingTrade` inside the
//! `common::trade::Trades` resource, keyed by an opaque `common::trade::
//! TradeId`. This module does NOT reinvent any of that: it projects those
//! sim types into small, read-only `Net*` mirrors (following the
//! `NetHealth`/EM-5.2 pattern exactly) and reuses the sim's OWN
//! `TradeId`/`TradeAction`/`TradePhase` types verbatim on the wire (they
//! already derive `Serialize + Deserialize` — see `common::trade`) rather
//! than inventing parallel copies. `TradeId`'s inner field is private (no
//! public constructor/accessor outside `common::trade`), which is exactly
//! why round-tripping the type ITSELF — not an unpacked integer — is the
//! only workable wire shape; this crate never needs to construct one, only
//! echo one back.
//!
//! ## Why the client never needs the counterparty's raw inventory
//! [`common::trade::PendingTrade::offers`] stores each party's OWN
//! `InvSlotId`s — a party can only ever offer items from their OWN bag. So
//! [`NetTrade`] carries BOTH sides' offers already resolved to display data
//! (item id/name/quality/amount) by the bridge (which has read access to
//! both parties' `Inventory` components for exactly this projection) — the
//! counterparty's full [`crate::NetInventory`] is never replicated to this
//! client at all (nor could it be — see [`crate::owner_visibility`]).
//!
//! ## Visibility (spec §3.2 "interest-managed")
//! [`NetTrade`] and [`NetIncomingTradeInvite`] are scoped with
//! [`crate::owner_visibility::NetOwnerOnly`] exactly like [`crate::
//! NetInventory`] — attached to EACH party's own mirror entity with that
//! party's own `Uid`, so each side only ever sees their OWN copy of the
//! trade (which happens to already contain both offers, since that's what
//! the UI needs) — never the other party's mirror entity's copy.

use common::{
    comp::inventory::slot::InvSlotId,
    trade::{TradeAction, TradeId, TradePhase},
};
use serde::{Deserialize, Serialize};

use bevy::ecs::{component::Component, message::Message};

use crate::inventory::NetItemStack;

/// One resolved trade-offer line (one side's one item), projected for the
/// trade window's offer grid. Embeds [`NetItemStack`] for the identity/
/// name/quality/amount fields (the SAME shape [`crate::inventory::
/// NetInventorySlot`] uses) rather than duplicating them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetTradeOfferEntry {
    /// The offering party's OWN bag slot (round-trips into a
    /// [`TradeActionRequest`]'s `TradeAction::AddItem`/`RemoveItem`).
    pub slot: InvSlotId,
    /// The item's identity/name/quality; `amount` here is the total stack
    /// size in that slot (see [`Self::offered`] for how much of it is
    /// actually offered).
    pub item: NetItemStack,
    /// How much of this slot is currently OFFERED in the trade.
    pub offered: u32,
    /// How much of this item the offering party actually OWNS in that slot
    /// (lets the UI cap a quantity slider/stepper without a round-trip).
    pub owned: u32,
}

/// The full two-party trade projection, mirrored onto EACH party's own
/// mirror entity (see the module doc comment for why both sides get their
/// own copy rather than one shared entity).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetTrade {
    /// Echoed back verbatim in every [`TradeActionRequest`] this client
    /// sends for this trade.
    pub trade_id: TradeId,
    /// The other party's stable identity (for the "trading with ..."
    /// header — full name resolution is a `NetPlayerList`/EM-5.8 concern;
    /// v1 shows the uid, matching this epic's other "raw id, i18n/lookup
    /// depth is a named follow-up" placeholders).
    pub counterparty_uid: u64,
    pub phase: TradePhase,
    /// This client's OWN offered items.
    pub my_offer: Vec<NetTradeOfferEntry>,
    /// The counterparty's offered items (resolved server-side — see module
    /// doc comment for why the client never needs their raw inventory).
    pub their_offer: Vec<NetTradeOfferEntry>,
    /// Whether THIS party has accepted the current phase.
    pub my_accepted: bool,
    /// Whether the COUNTERPARTY has accepted the current phase.
    pub their_accepted: bool,
}

/// An incoming, not-yet-accepted trade invite — mirrors the sim's
/// `common::comp::invite::Invite` component (which lives on the INVITEE's
/// own entity), so this is attached to the CLIENT's own mirror entity the
/// same way [`NetTrade`] is. `None`/absent = no pending invite; the UI shows
/// nothing.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetIncomingTradeInvite {
    pub from_uid: u64,
}

/// Client → sim: propose a trade with the entity carrying `target_uid` (must
/// be within trading range and alive — the SAME `InitiateInviteEvent` +
/// `InviteKind::Trade` checks `server::events::invite` already enforces; this
/// message is real replicated intent, not a new check surface).
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradeInviteRequest {
    pub target_uid: u64,
}

/// Client → sim: accept or decline the CURRENT [`NetIncomingTradeInvite`]
/// (there is at most one at a time, matching legacy's own single-pending-
/// trade-invite UX).
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradeInviteResponseRequest {
    pub accept: bool,
}

/// Client → sim: mutate/accept/decline an ALREADY-BEGUN trade. Wraps the
/// sim's own `TradeId`/`TradeAction` verbatim — see the module doc comment.
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TradeActionRequest {
    pub trade_id: TradeId,
    pub action: TradeAction,
}

/// The one [`InviteKind`]/[`InviteResponse`] pair this epic's trade flow
/// needs — re-exported so `xindeler-sim-bridge`/`xindeler-client` name them
/// off this module rather than reaching into `common` directly for a type
/// this crate already depends on anyway (keeps the "which crate owns which
/// re-export" convention this file's sibling modules already follow).
pub use common::comp::invite::{InviteKind as NetInviteKind, InviteResponse as NetInviteResponse};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_invite_kind_and_response_are_the_real_sim_types() {
        // Compile-time proof these re-exports are literally the sim's own
        // types (not a shadow copy) — matches the module doc comment's claim.
        let _: NetInviteKind = common::comp::invite::InviteKind::Trade;
        let _: NetInviteResponse = common::comp::invite::InviteResponse::Accept;
    }

    #[test]
    fn net_trade_offer_entry_round_trips_bincode() {
        use common::comp::inventory::item::{ItemDefinitionIdOwned, Quality, item_key::ItemKey};

        let entry = NetTradeOfferEntry {
            slot: InvSlotId::new(0, 2),
            item: NetItemStack {
                item_id: ItemDefinitionIdOwned::Simple("common.items.utility.coins".to_owned()),
                name: "Coins".to_owned(),
                quality: Quality::Common,
                amount: 20,
                is_two_handed: false,
                // Coins are non-equippable, matching the field's own
                // documented `[]` default for that case.
                equippable_slots: Vec::new(),
                icon_key: ItemKey::Simple("common.items.utility.coins".to_owned()),
            },
            offered: 5,
            owned: 20,
        };
        let bytes =
            bincode::serde::encode_to_vec(&entry, bincode::config::legacy()).expect("serializes");
        let (decoded, _): (NetTradeOfferEntry, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::legacy())
                .expect("deserializes");
        assert_eq!(decoded, entry);
    }
}
