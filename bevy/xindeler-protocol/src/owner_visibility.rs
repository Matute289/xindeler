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

use crate::{NetAbilityPool, NetIncomingTradeInvite, NetInventory, NetSkillSet, NetTrade};

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
    // Hide ONLY these components when the check below fails — every OTHER
    // replicated component on the same entity (NetPos/NetHealth/...) stays
    // governed by its own (entity-level) filter, unaffected.
    //
    // BL-82 EM-5.7 (bevy-migration-reviewer + ecs-design-reviewer blocker,
    // fixed): `NetSkillSet`/`NetAbilityPool` were tagged with the
    // `NetOwnerOnly` COMPONENT by `xindeler-sim-bridge::skillset` and their
    // own doc comments claimed "self-scoped, same posture as `NetInventory`"
    // — but `bevy_replicon`'s `VisibilityFilter::Scope` only hides the
    // components literally NAMED in this tuple. Tagging alone did nothing;
    // both types silently fell back to `RegionKey`'s entity-level (whole-
    // entity, region-based) visibility, meaning every client that could see
    // the entity at all received every player's full unlocked-skill map and
    // qualifying-ability pool — a real cross-client privacy leak once a
    // second genuine client exists (EM-4.2b). Adding both here is the fix;
    // see `tests::skillset_and_ability_pool_are_owner_scoped_too` for a
    // regression guard that exercises the real two-client filter, not just
    // `is_visible` in isolation.
    type Scope = (
        NetInventory,
        NetTrade,
        NetIncomingTradeInvite,
        NetSkillSet,
        NetAbilityPool,
    );

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

    /// BL-82 EM-5.7 regression guard (bevy-migration-reviewer +
    /// ecs-design-reviewer blocker): exercises the REAL two-client
    /// `VisibilityFilter::Scope` registration end to end — not just
    /// `is_visible` in isolation, which is exactly what let the missing
    /// `NetSkillSet`/`NetAbilityPool` entries in `Scope` slip through
    /// undetected. Two connected clients see the SAME `Replicated` entity
    /// (no `RegionKey` on it, so that filter doesn't gate it at all); one is
    /// tagged the real owner, the other isn't. The owner must receive
    /// `NetSkillSet`; the non-owner must receive the entity (its `NetUid`,
    /// say) but NEVER `NetSkillSet` — proving `Scope` actually hides it, not
    /// merely that `NetOwnerOnly`'s own `is_visible` logic is correct.
    #[test]
    fn skillset_is_owner_scoped_across_two_real_clients() {
        use bevy::{
            MinimalPlugins,
            app::{App, PluginGroup, PostUpdate},
        };
        use bevy_replicon::{
            RepliconPlugins,
            prelude::{ConnectedClient, Replicated, ServerPlugin},
            test_app::{ServerTestAppExt, TestClientEntity},
        };

        use crate::{XindelerProtocolPlugin, skillset::NetSkillSet};

        fn new_app() -> App {
            let mut app = App::new();
            app.add_plugins((
                MinimalPlugins,
                bevy::state::app::StatesPlugin,
                RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
                XindelerProtocolPlugin,
            ))
            .finish();
            app
        }

        /// Finds a connected client's own server-side connection entity by
        /// matching the `TestClientEntity` handshake resource `connect_client`
        /// stashes on the CLIENT app — the entity carrying `ConnectedClient`
        /// server-side is a genuine `Entity` shared between both apps in this
        /// test harness (spawned once, referenced by both sides), so a direct
        /// equality match is exact — no ordering/"newest" heuristic needed
        /// (an earlier draft of this test assumed `Entity`'s `Ord` reflected
        /// connection order across TWO independently-connecting clients,
        /// which does not hold once `bevy_replicon`'s own internal handshake
        /// entities are accounted for; this fixes that).
        fn server_connection_entity(server_app: &mut App, client_app: &App) -> Entity {
            let target = **client_app.world().resource::<TestClientEntity>();
            server_app
                .world_mut()
                .query::<(Entity, &ConnectedClient)>()
                .iter(server_app.world())
                .map(|(e, _)| e)
                .find(|&e| e == target)
                .expect("the client's own connection entity exists server-side")
        }

        let mut server_app = new_app();
        let mut owner_client = new_app();
        let mut other_client = new_app();

        server_app.connect_client(&mut owner_client);
        let owner_entity = server_connection_entity(&mut server_app, &owner_client);
        server_app
            .world_mut()
            .entity_mut(owner_entity)
            .insert(ClientOwnedUid(42));

        server_app.connect_client(&mut other_client);
        let other_entity = server_connection_entity(&mut server_app, &other_client);
        server_app
            .world_mut()
            .entity_mut(other_entity)
            .insert(ClientOwnedUid(99));

        let skillset = NetSkillSet {
            groups: vec![],
            skills: vec![],
        };
        server_app
            .world_mut()
            .spawn((Replicated, NetOwnerOnly(42), skillset));

        server_app.update();
        server_app.exchange_with_client(&mut owner_client);
        owner_client.update();
        server_app.exchange_with_client(&mut other_client);
        other_client.update();

        let mut owner_query = owner_client.world_mut().query::<&NetSkillSet>();
        assert_eq!(
            owner_query.iter(owner_client.world()).count(),
            1,
            "the owning client must receive NetSkillSet"
        );

        let mut other_query = other_client.world_mut().query::<&NetSkillSet>();
        assert_eq!(
            other_query.iter(other_client.world()).count(),
            0,
            "a non-owning client must NEVER receive another player's NetSkillSet — this is \
             exactly the leak the Scope-tuple fix closes"
        );
    }
}
