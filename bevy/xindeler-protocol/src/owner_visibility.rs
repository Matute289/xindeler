//! BL-82 EM-5.6 — per-OWNER visibility for private per-player state
//! ([`crate::NetInventory`]/[`crate::NetTrade`]/
//! [`crate::NetIncomingTradeInvite`]), following the exact
//! [`bevy_replicon::prelude::VisibilityFilter`] pattern
//! [`crate::visibility::RegionKey`] already established for region-scoped
//! entity visibility — see that module's doc comment for the shared
//! machinery this reuses (`AppVisibilityExt::add_visibility_filter`,
//! `#[component(immutable)]`).
//!
//! ## Why a SEPARATE filter, not reusing `RegionKey`
//! `RegionKey`'s `Scope` is [`bevy_replicon::prelude::Entity`] (whole-entity
//! visibility, "can this client see this entity at all"). A player's own
//! inventory/trade state must stay hidden from every OTHER client that CAN
//! otherwise see that same entity (its position/health/loadout are public —
//! its bag contents are not) — `bevy_replicon`'s `VisibilityFilter::Scope`
//! supports exactly this via [`bevy_replicon::prelude::SingleComponent`]/
//! tuples ("hide only a single component on the entity" — see that trait's
//! own doc comment), so [`NetOwnerOnly`]'s `Scope` names ONLY the three
//! private components, leaving `RegionKey`'s entity-level scoping (and every
//! other replicated component on the same entity) completely independent —
//! a client can see an entity's `NetPos`/`NetHealth` while being denied its
//! `NetInventory`/`NetTrade`, which is exactly the desired shape.
//!
//! ## `ClientOwnedUid` — who owns which client connection
//! [`xindeler-server-app`]'s login handshake
//! (`login.rs::handle_character_data`) is the ONE place a connected client's
//! replicon connection entity is correlated to the sim `Uid` it controls (it
//! already inserts a sibling correlation,
//! `xindeler_sim_bridge::PlayerDimensionSession`, at the exact same call site)
//! — this module's [`ClientOwnedUid`] is the wire-visibility half of that same
//! correlation, inserted right alongside it. The bridge side needs NO reverse
//! lookup at all: [`crate::inventory::NetInventory`]/
//! [`crate::trade::NetTrade`] are tagged with [`NetOwnerOnly`] carrying the
//! MIRRORED ENTITY'S OWN `Uid` (already read for `NetUid` — see
//! `xindeler-sim-bridge::mirror_sim_entities`), and `is_visible` just
//! compares that against whichever client's `ClientOwnedUid` (if any) is
//! being evaluated.
//!
//! ## The listen-server (embedded local player) case
//! A listen server acting as its own client (`ClientState::Disconnected`,
//! no separate `ConnectedClient` entity) never evaluates ANY
//! `VisibilityFilter` at all for its own local rendering — the client-side
//! systems in that mode query the SAME `World` the mirror writes into
//! directly (no serialization boundary), which is precisely why
//! `RegionKey`'s "blind until scoped" default has never blocked local
//! solo play either. This filter therefore only ever matters for a genuine
//! REMOTE second client (`xindeler-server-app`'s dedicated-server shell,
//! EM-4.2b) — the ONLY case where leaking one player's bag contents to
//! another connected human is even possible.

use bevy::ecs::{component::Component, entity::Entity};
use bevy_replicon::prelude::VisibilityFilter;

use crate::{NetIncomingTradeInvite, NetInventory, NetTrade};

/// Marks a connected client's OWN connection entity with the sim `Uid` it
/// controls. Inserted once, at login, by whichever shell resolves a
/// connection to a sim entity (`xindeler-server-app::login`); absent for a
/// client that hasn't finished logging in yet (or, in listen-server mode, at
/// all — see the module doc comment for why that's harmless there).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[component(immutable)]
pub struct ClientOwnedUid(pub u64);

/// Tags a mirrored entity's private per-player components (see module doc
/// comment) with that SAME entity's own `Uid` — never a different value.
/// `xindeler-sim-bridge`'s inventory/trade mirrors write this alongside
/// [`crate::NetInventory`]/[`crate::NetTrade`]/
/// [`crate::NetIncomingTradeInvite`] on every tick those are (re)inserted.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[component(immutable)]
pub struct NetOwnerOnly(pub u64);

impl VisibilityFilter for NetOwnerOnly {
    type ClientComponent = ClientOwnedUid;
    // Hide ONLY these three components when the check below fails — every
    // OTHER replicated component on the same entity (NetPos/NetHealth/...)
    // stays governed by its own (entity-level) filter, unaffected.
    type Scope = (NetInventory, NetTrade, NetIncomingTradeInvite);

    fn is_visible(&self, _client: Entity, component: Option<&Self::ClientComponent>) -> bool {
        component.is_some_and(|owned| owned.0 == self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client whose `ClientOwnedUid` matches the entity's `NetOwnerOnly`
    /// sees the private components; a mismatched or absent one does not —
    /// the core acceptance bar for this filter (spec §3.2 "interest-managed").
    #[test]
    fn only_the_matching_owner_uid_is_visible() {
        let filter = NetOwnerOnly(42);
        assert!(filter.is_visible(Entity::PLACEHOLDER, Some(&ClientOwnedUid(42))));
        assert!(!filter.is_visible(Entity::PLACEHOLDER, Some(&ClientOwnedUid(7))));
        assert!(!filter.is_visible(Entity::PLACEHOLDER, None));
    }
}
