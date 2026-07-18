//! BL-82 EM-5.14 (T56.32): the character-list / character-creation wire
//! contract.
//!
//! Character selection is **account-scoped data with no mirrored world
//! entity** (the account is not a replicated ECS entity), so — exactly like
//! [`crate::social::NetPlayerList`] and [`crate::narrative::HudToast`] — the
//! roster travels as a broadcast MESSAGE ([`NetCharList`]), NOT a per-entity
//! `.replicate::<>()` component (spec §3.2's "bulk data = messages" rule).
//! It is change-deduped and re-sent only when the roster actually changes
//! (create / delete / relog), the same "send whenever it changes" posture
//! `NetPlayerList`/`NetGroupState` use — never a per-tick sample (there is no
//! per-frame character-list churn) and never a one-shot latch like
//! [`crate::map::NetMapData`] (the roster DOES change mid-session).
//!
//! The three player intents (create / delete / select) follow the exact
//! `crate::social` two-type split: a serde WIRE request ([`CharCreateRequest`]
//! /[`CharDeleteRequest`]/[`CharSelectRequest`], `add_client_message`, for a
//! future genuinely-remote client) plus an in-process [`LocalCharCreate`]/
//! [`LocalCharDelete`]/[`LocalCharSelect`] twin that actually drives the
//! listen server's own char-select UI today (the listen server runs replicon's
//! SERVER role only — no client role — so its UI hands intents to the bridge
//! as plain in-App messages, mirroring [`crate::LocalPlayerInput`] and
//! [`crate::social::LocalGroupAction`]). The bridge
//! (`xindeler_sim_bridge::charlist`) translates each 1:1 into the embedded
//! player's real `client::Client::{create_character,delete_character,
//! request_character}` calls — a genuine client→server round trip over the
//! loopback socket, never a direct sim-state write (isolation law).

use bevy::ecs::message::Message;
use common::{
    character::CharacterId,
    comp::{Background, Body, ClassKind, Ethos},
};
use serde::{Deserialize, Serialize};

/// One roster row (BL-82 EM-5.14). Richer than [`crate::login::
/// NetCharacterSummary`] (which the login handshake reply reuses): the select
/// screen also shows the hardcore flag + last location, and the 3D preview is
/// built from [`Self::body`] straight through the existing figure pipeline
/// (EM-3.8x) — no sim entity required (`common::comp::humanoid::Body`'s fields
/// are the whole appearance payload).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetCharListEntry {
    pub id: CharacterId,
    pub alias: String,
    pub body: Body,
    pub hardcore: bool,
    /// Human-readable last location / waypoint name, if the server knows one.
    pub location: Option<String>,
}

/// Server → client: the account's full character roster (BL-82 EM-5.14).
/// Broadcast + change-deduped (see this module's doc comment). `loading` lets
/// the select screen show a spinner while the sim's asynchronous
/// character-list load is still in flight (mirrors
/// `client::CharacterList::loading`).
#[derive(Message, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetCharList {
    pub characters: Vec<NetCharListEntry>,
    pub loading: bool,
}

/// The shared payload both [`CharCreateRequest`] (wire) and [`LocalCharCreate`]
/// (in-process) carry — a 1:1 mirror of `client::Client::create_character`'s
/// arguments so the bridge can forward it verbatim. The server re-validates
/// everything (alias, the class↔weapon whitelist in
/// `server::character_creator::valid_starter_items`, ethos clamping) — this is
/// a request, never trusted input.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharCreateParams {
    pub alias: String,
    /// Mainhand starter-weapon asset path (e.g.
    /// `"common.items.weapons.sword.starter"`), or `None` for weaponless.
    pub mainhand: Option<String>,
    /// Offhand starter-weapon asset path (Rogue dual-wield), or `None`.
    pub offhand: Option<String>,
    pub body: Body,
    pub hardcore: bool,
    pub class: ClassKind,
    pub ethos: Ethos,
    pub background: Background,
}

/// Client → server WIRE request: create a character (a future genuinely-remote
/// client's intent). Registered + round-trip-tested but NOT what drives the
/// listen server's own UI today — see [`LocalCharCreate`] and this module's
/// doc comment.
#[derive(Message, Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharCreateRequest(pub CharCreateParams);

/// Client → server WIRE request: delete the character with this id.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharDeleteRequest(pub CharacterId);

/// Client → server WIRE request: select (enter the world as) this character.
#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharSelectRequest(pub CharacterId);

/// In-process create intent that actually drives the listen server's char
/// creation today (mirrors [`crate::social::LocalGroupAction`]).
#[derive(Message, Clone, Debug, PartialEq)]
pub struct LocalCharCreate(pub CharCreateParams);

/// In-process delete intent (listen server).
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct LocalCharDelete(pub CharacterId);

/// In-process select intent (listen server).
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub struct LocalCharSelect(pub CharacterId);

/// Registers the EM-5.14 character-list/creation wire contract: [`NetCharList`]
/// as a broadcast server message (no entity references —
/// `make_message_independent` like [`crate::social::NetPlayerList`]), the three
/// `*Request` types as client messages (the `Events` lane, like
/// [`crate::login::LoginRequest`]), and the three `Local*` twins as plain Bevy
/// messages (dormant on any App that never writes them). Called from
/// [`crate::XindelerProtocolPlugin`].
pub(crate) fn register(app: &mut bevy::app::App) {
    use bevy_replicon::prelude::{ClientMessageAppExt, ServerMessageAppExt};

    app.add_server_message::<NetCharList>(crate::XindelerChannel::Events.delivery())
        .make_message_independent::<NetCharList>();

    app.add_client_message::<CharCreateRequest>(crate::XindelerChannel::Events.delivery());
    app.add_client_message::<CharDeleteRequest>(crate::XindelerChannel::Events.delivery());
    app.add_client_message::<CharSelectRequest>(crate::XindelerChannel::Events.delivery());

    app.add_message::<LocalCharCreate>();
    app.add_message::<LocalCharDelete>();
    app.add_message::<LocalCharSelect>();
}
