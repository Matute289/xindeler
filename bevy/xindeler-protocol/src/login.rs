//! BL-82 EM-4.2c: login/session handshake wire types.
//!
//! [`LoginRequest`] is the FIRST message a replicon client sends after
//! connecting, on the [`crate::XindelerChannel::Events`] lane — the
//! replicon-transport counterpart to the legacy wire protocol's
//! `ClientRegister` / `ClientGeneral::RequestCharacterList` /
//! `ClientGeneral::Character` trio (`common_net::msg::ClientRegister`,
//! `server/src/sys/msg/{register,character_screen}.rs`). The server-side
//! system that answers it lives in `xindeler-server-app` (not this crate,
//! which stays a pure type library over `common`) — see that crate's
//! `login` module doc comment for the full server-side design (a DEDICATED
//! `CharacterLoader` instance so its responses never race the legacy path's
//! own shared one, the v1 "auto-select the first character" policy, and the
//! documented IP-ban gap for this transport).
//!
//! ## v1 auto-selects rather than round-tripping a selection message
//! Character SELECT/CREATE UI is Phase 5 client work — out of this task's
//! scope (spec §1.2's scope boundary). [`LoginSuccess`] still carries the
//! full character list so a future client can build that UI without a wire
//! change, but for now the account's FIRST character (if any) is loaded
//! automatically as part of the very same login round trip.

use bevy::ecs::message::Message;
use common::character::CharacterId;
use serde::{Deserialize, Serialize};

/// Client → server: authenticate + (if the account has a character) load it.
/// Travels on [`crate::XindelerChannel::Events`] (ordered/reliable),
/// immediately after connecting — mirrors the legacy `ClientRegister`
/// message's shape (`common_net::msg::ClientRegister`), one level up.
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoginRequest {
    /// Either an auth-server token (online mode) or a raw username
    /// (offline/no-auth mode — hashed into a deterministic UUID
    /// server-side) — the SAME `token_or_username` shape
    /// `ClientRegister`/`LoginProvider::verify` already accept; both paths
    /// are handled identically by the server system that answers this.
    pub token_or_username: String,
    /// BCP-47-ish locale tag. Unused past acceptance today: no MOTD/
    /// server-description forwarding is wired for this transport yet (that
    /// forwarding lives in the legacy `character_screen.rs`'s
    /// `send_join_messages`, which needs a `Client` component this
    /// transport's entities don't have) — kept so the wire shape matches
    /// `ClientRegister` and a future consumer doesn't need a protocol
    /// change to add it.
    pub locale: String,
}

/// A minimal, wire-sized summary of one character — NOT the full
/// `common::character::CharacterItem` (which also carries a complete
/// `Inventory`): character list/creation UI is Phase 5 client work, so v1
/// only needs enough to report what exists and confirm what got auto-loaded.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetCharacterSummary {
    pub id: CharacterId,
    pub alias: String,
    pub body: common::comp::Body,
}

/// Typed login-failure reasons (BL-82 EM-4.2c). Deliberately its own enum,
/// not `common_net::msg::RegisterError` — this crate is a plain type
/// library over `common` only (see the crate doc comment) and would
/// otherwise need a fresh `common-net` dependency just for this one enum;
/// the server-side system maps `RegisterError`/persistence errors onto this.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LoginError {
    /// Auth-server token validation failed (online mode only).
    Auth(String),
    /// The account is banned (`server::login_provider::ban_applies`).
    Banned(String),
    /// The server has a non-empty whitelist and this account isn't on it.
    NotOnWhitelist,
    /// The server is at its configured player cap.
    TooManyPlayers,
    /// The account's alias failed `common::comp::Player::is_valid`.
    InvalidCharacter,
    /// The character list failed to load from the database.
    CharacterListFailed(String),
    /// The selected character's persisted data failed to load.
    CharacterDataFailed(String),
}

/// Server → client reply to [`LoginRequest`], on the same
/// [`crate::XindelerChannel::Events`] lane.
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoginResult {
    pub outcome: Result<LoginSuccess, LoginError>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LoginSuccess {
    /// The account's full character list (possibly empty — a fresh account
    /// with no characters yet is still a successful login, just with
    /// nothing to auto-load; character CREATION stays on the legacy path
    /// for now, see the crate doc comment).
    pub characters: Vec<NetCharacterSummary>,
    /// The character auto-loaded this round trip (the first entry of
    /// [`Self::characters`]), if any. When this is `Some(id)`, the sim-side
    /// entity has already reached `Presence::Character(id)` by the time
    /// this message is sent.
    pub selected: Option<CharacterId>,
}
