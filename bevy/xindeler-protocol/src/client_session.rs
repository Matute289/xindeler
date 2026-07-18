//! BL-82 EM-8.2 — client↔player identity correlation (technical-debt ledger
//! `docs/design/specs/2026-07-18-bl82-technical-debt-ledger.md` Part A2):
//! answers "which connected replicon client controls sim player X" (and the
//! reverse) so a mirror system can target [`bevy_replicon::prelude::
//! SendTargets::Single`] instead of broadcasting private per-player state to
//! every connected client.
//!
//! ## Relocated from `xindeler-server-app::login`
//! [`ActiveReplicaSessions`] used to be a private type defined inside
//! `xindeler-server-app::login` (BL-82 EM-4.2c), keyed by `specs::Entity`.
//! That worked fine for its ORIGINAL, sole purpose (the login handshake
//! finding an old session to kick on a duplicate login, entirely within the
//! SAME file) but couldn't serve `xindeler-sim-bridge::social`'s mirror
//! systems, which need the identical correlation and are investigated first
//! per this task's own instructions rather than inventing a parallel
//! resource:
//!
//! - `xindeler-server-app` is a **binary-only package** (no `[lib]` target, see
//!   `xindeler_protocol::interest`'s own module doc comment for why that
//!   matters) — no OTHER crate can `use` anything out of its `src/` at all, so
//!   a type defined there is structurally unreachable from
//!   `xindeler-sim-bridge`.
//! - `xindeler-sim-bridge` cannot become a dependency of `xindeler-server-app`
//!   either without inverting the shell → logic/shared-crate dependency
//!   direction this workspace's crate layering already establishes
//!   (`xindeler-server-app` depends on `xindeler-sim-bridge`, never the
//!   reverse).
//! - `xindeler-protocol` is the one crate BOTH already depend on (directly), is
//!   deliberately the low-level, few-dependency shared wire crate every Bevy
//!   shell links (client AND server — see this crate's own module doc comment),
//!   and already hosts the sibling per-client correlation type
//!   [`crate::interest::ClientViewpoint`] for exactly the same reason. So this
//!   module relocates the TYPE here — `xindeler-server-app::login` keeps owning
//!   every WRITE to it (nothing about who populates the map changes, only where
//!   the map itself lives).
//!
//! ## Keyed by `Uid`, not `specs::Entity`
//! The original `specs::Entity`-keyed shape cannot simply move verbatim:
//! `xindeler-protocol` is deliberately **specs-free** — the pure Bevy client
//! links it too (`xindeler-sim-bridge`'s own module doc comment: "This crate
//! and `xindeler-server-app::login`... are the two legal `specs` consumers
//! under `bevy/`" — a THIRD consumer, especially one reachable from the
//! client, would quietly break that invariant even though the CI
//! engine-isolation grep wouldn't catch it (it only checks
//! `bevy/xindeler-client/src`, not this crate's own sources or Cargo
//! manifest)). [`common::uid::Uid`]'s plain `u64` inner value is already how
//! this crate correlates sim identity everywhere else it needs to
//! (`crate::NetUid`, `crate::owner_visibility::ClientOwnedUid`) — a `Uid` is
//! also the MORE correct identity to key a long-lived session map by: it is
//! the sim's own stable, network-facing player identity, unlike
//! `specs::Entity`'s ECS-internal (generation-counted) handle.
//!
//! `xindeler-server-app::login` already reads the target entity's `Uid` at
//! the exact call site that used to insert into the old
//! `specs::Entity`-keyed map (to build [`crate::owner_visibility::
//! ClientOwnedUid`]) — this relocation reuses that SAME read, not a new one.

use std::collections::HashMap;

use bevy::ecs::resource::Resource;
use bevy_replicon::prelude::ClientId;

/// `Uid` (as its plain `u64` inner value) → `ClientId`, for every replicon
/// login that has FULLY completed (reached `Presence::Character`) — see this
/// module's doc comment for the full design, and
/// `xindeler-server-app::login`'s own module doc comment (the "known gaps"
/// section) for what this does and doesn't cover (e.g. a clean network drop
/// without a later duplicate login leaks the entry).
///
/// Always empty on the listen server: the embedded local player authenticates
/// over the legacy TCP/QUIC loopback `xindeler-client-core::Client`, never
/// through the replicon login handshake this map is populated by — so it
/// never appears here at all (matching `crate::owner_visibility::
/// ClientOwnedUid`'s own doc comment on the identical listen-server case).
/// [`crate::XindelerProtocolPlugin`] does NOT auto-initialize this resource
/// (unlike the plain replicated components it registers) — whichever plugin
/// actually needs to READ it (`xindeler-sim-bridge::social::
/// SocialMirrorPlugin`) initializes it defensively via `init_resource`
/// (idempotent alongside `xindeler-server-app`'s own explicit insert), so a
/// system taking a non-`Option` `Res<ActiveReplicaSessions>` never panics for
/// want of the resource existing.
#[derive(Resource, Default, Debug)]
pub struct ActiveReplicaSessions(HashMap<u64, ClientId>);

impl ActiveReplicaSessions {
    /// Records that the sim player identified by `uid` is now controlled by
    /// `client_id`'s connection — called once a login fully completes.
    /// Returns the previous session for `uid`, if any (should never happen in
    /// practice: a `Uid` is only ever assigned to one live character at a
    /// time, and the duplicate-login path removes the old entry via
    /// [`Self::remove`] before a new one for the same account is inserted).
    pub fn insert(&mut self, uid: u64, client_id: ClientId) -> Option<ClientId> {
        self.0.insert(uid, client_id)
    }

    /// Removes and returns the session for `uid`, if any — called when a
    /// duplicate login for the same account kicks the OLD session.
    pub fn remove(&mut self, uid: u64) -> Option<ClientId> { self.0.remove(&uid) }

    /// The connected client currently controlling the sim player identified
    /// by `uid`, if any — `None` for a player with no active replicon session
    /// (the listen-server's embedded local player, or a dedicated-server
    /// player whose login hasn't completed yet).
    #[must_use]
    pub fn client_for_uid(&self, uid: u64) -> Option<ClientId> { self.0.get(&uid).copied() }
}

#[cfg(test)]
mod tests {
    use bevy_replicon::prelude::ClientId;

    use super::ActiveReplicaSessions;

    /// A fresh map has no sessions — the correlation resource degrades clean
    /// (`None`, never a panic) for any uid before the login handshake ever
    /// inserts anything, matching the listen-server's permanent state.
    #[test]
    fn empty_by_default() {
        let sessions = ActiveReplicaSessions::default();
        assert_eq!(sessions.client_for_uid(1), None);
    }

    /// insert/lookup/remove round-trip exactly, and removal is idempotent
    /// (a second remove is a harmless `None`, not a panic) — the duplicate-
    /// login kick path relies on this for an already-removed/never-inserted
    /// entity.
    #[test]
    fn insert_lookup_remove_round_trip() {
        let mut sessions = ActiveReplicaSessions::default();
        let client = ClientId::Server;

        assert_eq!(sessions.insert(42, client), None);
        assert_eq!(sessions.client_for_uid(42), Some(client));

        assert_eq!(sessions.remove(42), Some(client));
        assert_eq!(sessions.client_for_uid(42), None);
        assert_eq!(sessions.remove(42), None);
    }
}
