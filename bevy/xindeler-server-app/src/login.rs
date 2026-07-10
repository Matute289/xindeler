//! BL-82 EM-4.2c: login/session handshake for the new replicon+quinnet
//! transport (spec §1.2). A [`xindeler_protocol::LoginRequest`] is answered by
//! calling the EXACT SAME public entry points the legacy TCP/QUIC path uses
//! (`server/src/sys/msg/{register,character_screen}.rs`):
//! `LoginProvider::verify`/`LoginProvider::login` for
//! auth+ban+whitelist+player-count-cap+duplicate-login, then
//! `CharacterLoader::load_character_list`/`load_character_data`, then
//! `StateExt::initialize_character_data`/`update_character_data` to set
//! `Presence`/`PresenceKind` and register the `CharacterId` in `IdMaps` — no
//! auth/persistence logic is reimplemented anywhere in this module.
//!
//! ## Why a SEPARATE `CharacterLoader` instance ([`RepliconCharacterLoader`])
//! `CharacterLoader::messages()` drains a single crossbeam channel with
//! exactly one legal consumer. That consumer already exists: `Server::tick`
//! unconditionally drains the specs-resource `CharacterLoader` every tick and
//! routes each response through `Server::notify_client` (a no-op for any
//! entity with no legacy `Client` component — silently safe, never a panic)
//! or `State::emit_event_now(UpdateCharacterDataEvent { .. })` (NOT gated on
//! `Client` at all). That second half is actually fine to share — but the
//! FIRST half is not: a `CharacterList` response's only effect is
//! `notify_client`, so if the shared instance's channel carried our
//! responses, we'd have no way to ever see the character list content (it
//! would be silently dropped for a `Client`-less entity) and couldn't decide
//! which character to auto-load. Rather than reach into `Server::tick`'s
//! internals or fake a legacy `Client` (which needs a real
//! `network::Participant` — exactly the "new auth machinery" this task must
//! avoid), this module opens a SECOND, independent `CharacterLoader`
//! pointed at the SAME sqlite path (`sim::server_data_dir().join("saves")`),
//! read-only, exactly like the existing one — sqlite already supports
//! multiple concurrent read-only connections (the existing loader's own doc
//! comment: "This connection -must- remain read-only to avoid lock
//! contention with the CharacterUpdater thread" — a second reader is the
//! same safe pattern, not a new one). Its channel is exclusively ours: zero
//! collision, zero interference with the legacy path.
//!
//! ## v1 auto-select policy
//! `LoginRequest` carries no character-selection field (spec §1.2's wire
//! shape is deliberately just `{token_or_username, locale}`) and character
//! select/create UI is Phase 5 client work (scope boundary) — so on a
//! successful list load, the FIRST character in the account's list (if any)
//! is loaded automatically. [`xindeler_protocol::LoginSuccess`] still carries
//! the full list so a future client can build interactive selection without
//! a protocol change.
//!
//! ## Known gaps (documented, not silently dropped)
//! - **No IP-ban enforcement over this transport yet**: `xindeler-transport`
//!   doesn't surface a connecting client's IP today, so [`LoginProvider::
//!   login`] is always called with `ip: None` here — UUID-based
//!   ban/whitelist/player-count-cap checks are fully enforced; only the
//!   IP-ban/IP-ban-upgrade half is inactive for this transport. Follow-up:
//!   thread a real IP through once `xindeler-transport`/`bevy_quinnet` exposes
//!   one.
//! - **No disconnect bridging for a CLEAN disconnect**: a replicon connection
//!   dropping on its own (network drop, client quit) does not yet emit
//!   `ClientDisconnectEvent` for the sim-side entity this module creates (there
//!   is no code today translating `bevy_replicon`'s own client-disconnected
//!   signal into the sim). That entity therefore only gets cleaned up today via
//!   THIS module's own failure paths (auth/list/data failure →
//!   `ClientDisconnectEvent`) or a LATER duplicate login for the same account
//!   (see below) — not via an actual clean network drop. Flagged as a follow-up
//!   rather than implemented here — it's a general "replicon client
//!   disconnected" concern that arguably belongs with EM-4.2d (interest
//!   management) or a dedicated task, not folded silently into the login
//!   handshake. Duplicate-login kicks ARE fully handled (see
//!   [`ActiveReplicaSessions`]): both the sim-side entity
//!   (`ClientDisconnectEvent`) AND the old replicon/quinnet connection itself
//!   (`bevy_replicon::prelude::DisconnectRequest`) are torn down, so this gap
//!   is specifically about an old client vanishing WITHOUT a new login ever
//!   arriving for that account. [`ActiveReplicaSessions`]'s own map entry leaks
//!   in tandem for exactly this case (only ever removed by the duplicate-login
//!   path) — same root cause, called out separately since it's a second,
//!   independent structure, not just the sim entity.
//! - **Player-count-cap is a per-batch snapshot**: multiple simultaneous logins
//!   landing in the exact same tick are checked against the SAME player-count
//!   snapshot (taken once per tick, not incremented per login within the batch)
//!   — a burst right at the cap boundary could all pass. The legacy path
//!   (`register.rs`) handles this via a mutex-guarded running count across its
//!   parallel dispatch; reproducing that here for a message-driven (not
//!   per-tick-for-every-entity) system wasn't judged worth the complexity for
//!   what `register.rs`'s own comment already calls a best-effort cap ("we
//!   should also cap the value elsewhere").

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, RwLock},
};

use bevy::ecs::{
    change_detection::NonSendMut,
    message::{MessageReader, MessageWriter},
    resource::Resource,
    system::{Res, ResMut},
};
use bevy_replicon::prelude::{ClientId, DisconnectRequest, FromClient, SendTargets, ToClients};
use common::{ViewDistances, character::CharacterId, comp, event::ClientDisconnectEvent};
use common_net::{msg::RegisterError, sync::WorldSyncExt};
use server::{
    Settings,
    login_provider::{LoginProvider, PendingLogin},
    persistence::{
        DatabaseSettings, SqlLogMode,
        character_loader::{CharacterLoader, CharacterScreenResponseKind, CharacterUpdaterMessage},
    },
    settings::EditableSettings,
    state_ext::StateExt,
};
use specs::{Builder, WorldExt};
use xindeler_protocol::{LoginError, LoginRequest, LoginResult, LoginSuccess, NetCharacterSummary};
use xindeler_sim_bridge::SimServer;

/// View distance granted to a replicon-authenticated character. `LoginRequest`
/// carries no requested view distance (see module doc comment) and per-client
/// interest management doesn't exist yet either (EM-4.2d, the NEXT task in
/// this wave) — a small constant stands in until both land.
const DEFAULT_VIEW_DISTANCE: u32 = 4;

/// A SECOND, independent [`CharacterLoader`] instance dedicated to
/// replicon-originated logins — see the module doc comment for why this must
/// not be the same instance the legacy path's specs resource uses.
#[derive(Resource)]
pub struct RepliconCharacterLoader(CharacterLoader);

/// Opens [`RepliconCharacterLoader`] against the SAME `saves/` sqlite path
/// `sim::server_data_dir()` resolves to (the exact path `boot_dedicated_server`
/// hands `xindeler_sim_bridge::boot_with_settings`), read-only.
///
/// # Panics
/// Panics if the background reader thread fails to spawn — the same failure
/// mode `Server::new`'s own `CharacterLoader::new` call would treat as fatal.
pub fn boot_replicon_character_loader(server_data_dir: &Path) -> RepliconCharacterLoader {
    let database_settings = Arc::new(RwLock::new(DatabaseSettings {
        db_dir: server_data_dir.join("saves"),
        sql_log_mode: SqlLogMode::Disabled,
    }));
    RepliconCharacterLoader(
        CharacterLoader::new(database_settings)
            .expect("failed to start the replicon-login CharacterLoader reader thread"),
    )
}

/// One in-flight replicon login, keyed by the client that sent the
/// [`LoginRequest`].
#[derive(Resource, Default)]
pub struct PendingLogins(HashMap<ClientId, LoginSession>);

/// Sim entity → `ClientId` for every replicon login that has FULLY completed
/// (reached `Presence::Character`) — unlike [`PendingLogins`] (which only
/// tracks logins still in flight and is pruned as soon as one finishes), this
/// persists for the lifetime of the session so a LATER duplicate login for
/// the same account can find the old client to kick. See the module doc
/// comment's "known gaps" section for what this does and doesn't cover.
#[derive(Resource, Default)]
pub struct ActiveReplicaSessions(HashMap<specs::Entity, ClientId>);

struct LoginSession {
    /// The sim entity created for this login (via `create_entity_synced`, so
    /// it already carries a `Uid` — same allocation path the legacy connect
    /// path uses).
    entity: specs::Entity,
    stage: LoginStage,
}

enum LoginStage {
    Auth(PendingLogin),
    CharacterList,
    CharacterData {
        character_id: CharacterId,
        /// Kept so the final `LoginResult` can report the full list
        /// alongside which entry got auto-selected.
        characters: Vec<NetCharacterSummary>,
    },
}

/// What [`LoginProvider::login`]'s `extra_checks` closure hands back on
/// success (mirrors `register.rs`'s own `extra_checks` closure's result
/// shape, simplified: no `PlayerListUpdate`/legacy-`Client` bookkeeping is
/// relevant to a replicon-only entity).
///
/// Deliberately does NOT carry a duplicate-login flag: `extra_checks` only
/// sees a per-tick SNAPSHOT of the `Player` storage (taken once, before the
/// loop below applies any writes), so computing "is anyone else already
/// logged in as this account" here would miss a second `LoginRequest` for
/// the SAME account landing in the SAME tick's batch — both would see the
/// snapshot as duplicate-free and neither would kick the other. The
/// duplicate check instead happens in the SEQUENTIAL apply loop below
/// (`advance_auth`'s second loop), against LIVE state that already reflects
/// every earlier iteration's own insert this same tick.
struct LoginOutcome {
    player: comp::Player,
    admin_role: Option<comp::AdminRole>,
}

/// Answers [`LoginRequest`]s. See the module doc comment for the full design.
///
/// No-ops until [`SimServer`] is inserted (mirrors every other
/// `xindeler-sim-bridge`/`xindeler-server-app` system gated on
/// `Option<NonSendMut<SimServer>>`).
pub fn handle_replicon_logins(
    sim: Option<NonSendMut<SimServer>>,
    mut logins: ResMut<PendingLogins>,
    mut active: ResMut<ActiveReplicaSessions>,
    loader: Res<RepliconCharacterLoader>,
    mut requests: MessageReader<FromClient<LoginRequest>>,
    mut results: MessageWriter<ToClients<LoginResult>>,
    mut disconnects: MessageWriter<DisconnectRequest>,
) {
    let Some(mut sim) = sim else { return };

    intake_login_requests(&mut sim, &mut logins, &mut requests);
    advance_auth(
        &mut sim,
        &mut logins,
        &mut active,
        &loader.0,
        &mut results,
        &mut disconnects,
    );
    advance_character_loads(&mut sim, &mut logins, &mut active, &loader.0, &mut results);
}

/// Phase 1: for each new [`LoginRequest`], allocate a sim entity (`Uid` only
/// so far — `Player`/`Admin` are added once auth resolves in
/// [`advance_auth`]) and kick off `LoginProvider::verify` (handles BOTH
/// online-mode `authc` and offline-mode derived-UUID paths identically,
/// exactly as the legacy path does).
fn intake_login_requests(
    sim: &mut SimServer,
    logins: &mut PendingLogins,
    requests: &mut MessageReader<FromClient<LoginRequest>>,
) {
    for FromClient { client_id, message } in requests.read() {
        let client_id = *client_id;
        if logins.0.contains_key(&client_id) {
            tracing::warn!(
                ?client_id,
                "duplicate LoginRequest from an in-flight login; ignoring"
            );
            continue;
        }

        let entity = sim
            .server
            .state_mut()
            .ecs_mut()
            .create_entity_synced()
            .build();
        let pending = sim
            .server
            .state()
            .ecs()
            .read_resource::<LoginProvider>()
            .verify(&message.token_or_username);
        tracing::debug!(?client_id, ?entity, "replicon login: verifying credentials");
        logins.0.insert(client_id, LoginSession {
            entity,
            stage: LoginStage::Auth(pending),
        });
    }
}

/// Phase 2: advances every session in [`LoginStage::Auth`] by polling
/// [`LoginProvider::login`] — the SAME ban/whitelist/player-count-cap checks
/// and duplicate-login detection the legacy path uses (see that function's
/// doc comment for how it was widened to accept an IP-agnostic caller).
fn advance_auth(
    sim: &mut SimServer,
    logins: &mut PendingLogins,
    active: &mut ActiveReplicaSessions,
    loader: &CharacterLoader,
    results: &mut MessageWriter<ToClients<LoginResult>>,
    disconnects: &mut MessageWriter<DisconnectRequest>,
) {
    let mut resolved: Vec<(ClientId, specs::Entity, Result<LoginOutcome, RegisterError>)> =
        Vec::new();
    {
        let state = sim.server.state();
        let world = state.ecs();
        let editable = world.read_resource::<EditableSettings>();
        let settings = world.read_resource::<Settings>();
        let max_players = usize::from(settings.max_players);
        let current_player_count = server::login_provider::count_players(world);

        for (client_id, session) in logins.0.iter_mut() {
            let LoginStage::Auth(pending) = &mut session.stage else {
                continue;
            };
            let outcome = LoginProvider::login(
                pending,
                // No client IP surfaced by this transport yet — see the
                // module doc comment's "known gaps" section.
                None,
                &editable.admins,
                &editable.whitelist,
                &editable.banlist,
                |username, uuid| {
                    let admin_role: Option<comp::AdminRole> =
                        editable.admins.get(&uuid).map(|admin| admin.role.into());
                    let player = comp::Player::new(
                        username,
                        settings.gameplay.battle_mode.default_mode(),
                        uuid,
                        None,
                    );
                    (current_player_count >= max_players, LoginOutcome {
                        player,
                        admin_role,
                    })
                },
                |_ip, _uuid, _username| {
                    unreachable!(
                        "ip-ban upgrade can only trigger for a Some(ip) match, and `ip` above is \
                         always None for this transport (see this module's doc comment)"
                    )
                },
            );
            if let Some(result) = outcome {
                resolved.push((*client_id, session.entity, result));
            }
        }
    }

    // Applied SEQUENTIALLY (not batched like the snapshot-based checks
    // above): each iteration's duplicate lookup + `Player` insert is visible
    // to every LATER iteration this same tick, so two `LoginRequest`s for the
    // SAME account landing in the SAME tick's batch still resolve in order —
    // the second one finds the first's just-inserted `Player` and kicks it.
    // See `LoginOutcome`'s doc comment for why this can't happen earlier.
    for (client_id, entity, outcome) in resolved {
        match outcome {
            Err(err) => {
                reply(results, client_id, Err(map_register_error(err)));
                kick(sim, entity, comp::DisconnectReason::Kicked);
                logins.0.remove(&client_id);
            },
            Ok(outcome) => {
                if !outcome.player.is_valid() {
                    reply(results, client_id, Err(LoginError::InvalidCharacter));
                    kick(sim, entity, comp::DisconnectReason::Kicked);
                    logins.0.remove(&client_id);
                    continue;
                }
                let uuid = outcome.player.uuid();
                let duplicate_entity = server::login_provider::find_other_player_entity_by_uuid(
                    sim.server.state().ecs(),
                    uuid,
                    entity,
                );
                if let Some(old_entity) = duplicate_entity {
                    kick(sim, old_entity, comp::DisconnectReason::NewerLogin);
                    // Also close the OLD client's actual replicon/quinnet
                    // connection (`kick` above only cleans up the SIM-side
                    // entity) — see `ActiveReplicaSessions`'s doc comment.
                    if let Some(old_client_id) = active.0.remove(&old_entity)
                        && let Some(old_client_entity) = old_client_id.entity()
                    {
                        disconnects.write(DisconnectRequest {
                            client: old_client_entity,
                        });
                    }
                }
                let uuid_string = uuid.to_string();
                let admin_role = outcome.admin_role;
                let state = sim.server.state_mut();
                state.write_component_ignore_entity_dead(entity, outcome.player);
                if let Some(role) = admin_role {
                    state.write_component_ignore_entity_dead(entity, comp::Admin(role));
                }
                loader.load_character_list(entity, uuid_string);
                if let Some(session) = logins.0.get_mut(&client_id) {
                    session.stage = LoginStage::CharacterList;
                }
            },
        }
    }
}

/// Phase 3: drains [`RepliconCharacterLoader`]'s OWN message channel (never
/// the legacy shared one — see module doc comment) and advances
/// `CharacterList`/`CharacterData` sessions.
fn advance_character_loads(
    sim: &mut SimServer,
    logins: &mut PendingLogins,
    active: &mut ActiveReplicaSessions,
    loader: &CharacterLoader,
    results: &mut MessageWriter<ToClients<LoginResult>>,
) {
    for message in loader.messages() {
        // `DatabaseBatchCompletion` is only ever produced by `CharacterUpdater`
        // (writes) — this module never touches that resource, only
        // `CharacterLoader` (reads) — so this arm should be unreachable in
        // practice; treated as a no-op rather than `unreachable!()` so a
        // future change to this enum's variants can't panic a live server.
        let CharacterUpdaterMessage::CharacterScreenResponse(response) = message else {
            continue;
        };
        let target_entity = response.target_entity;
        let Some(client_id) = logins
            .0
            .iter()
            .find_map(|(id, session)| (session.entity == target_entity).then_some(*id))
        else {
            // Not one of ours — cannot happen given the dedicated channel,
            // but harmless to skip defensively.
            continue;
        };

        match response.response_kind {
            CharacterScreenResponseKind::CharacterList(list_result) => {
                let awaiting_list = matches!(
                    logins.0.get(&client_id).map(|session| &session.stage),
                    Some(LoginStage::CharacterList)
                );
                if !awaiting_list {
                    continue;
                }
                handle_character_list(
                    sim,
                    logins,
                    loader,
                    results,
                    client_id,
                    target_entity,
                    list_result,
                );
            },
            CharacterScreenResponseKind::CharacterData(data_result) => {
                let awaiting_data = match logins.0.get(&client_id).map(|session| &session.stage) {
                    Some(LoginStage::CharacterData {
                        character_id,
                        characters,
                    }) => Some((*character_id, characters.clone())),
                    _ => None,
                };
                let Some((character_id, characters)) = awaiting_data else {
                    continue;
                };
                handle_character_data(
                    sim,
                    active,
                    results,
                    client_id,
                    target_entity,
                    character_id,
                    characters,
                    *data_result,
                );
                logins.0.remove(&client_id);
            },
            // Never produced for this dedicated instance: it never calls
            // `create_character`/`edit_character` (character creation stays
            // on the legacy path — scope boundary, see the crate-level doc
            // comment in `xindeler_protocol::login`).
            CharacterScreenResponseKind::CharacterCreation(_)
            | CharacterScreenResponseKind::CharacterEdit(_) => {},
        }
    }
}

fn handle_character_list(
    sim: &mut SimServer,
    logins: &mut PendingLogins,
    loader: &CharacterLoader,
    results: &mut MessageWriter<ToClients<LoginResult>>,
    client_id: ClientId,
    target_entity: specs::Entity,
    list_result: Result<
        Vec<common::character::CharacterItem>,
        server::persistence::error::PersistenceError,
    >,
) {
    match list_result {
        Err(err) => {
            reply(
                results,
                client_id,
                Err(LoginError::CharacterListFailed(err.to_string())),
            );
            kick(sim, target_entity, comp::DisconnectReason::Kicked);
            logins.0.remove(&client_id);
        },
        Ok(list) => {
            let characters: Vec<NetCharacterSummary> = list
                .iter()
                .filter_map(|item| {
                    item.character.id.map(|id| NetCharacterSummary {
                        id,
                        alias: item.character.alias.clone(),
                        body: item.body,
                    })
                })
                .collect();
            match characters.first() {
                None => {
                    // A successful login with nothing to auto-load — a fresh
                    // account with no characters yet stays on the legacy path
                    // for character CREATION (scope boundary), and v1 has no
                    // replicon-side character-select UI to keep this session
                    // alive for. Reply success (the LOGIN itself genuinely
                    // succeeded), then kick the placeholder sim entity —
                    // WITHOUT this, it would sit in the ECS forever with a
                    // `Player` component (permanently counting against
                    // `max_players`, see `advance_auth`'s
                    // `count_players`) until a LATER login for the same
                    // account happened to find and kick it as a "duplicate".
                    // The replicon/quinnet CONNECTION itself is left open
                    // (only the sim-side placeholder is torn down) — a
                    // client can create a character via the legacy path and
                    // then resend `LoginRequest` on this same connection to
                    // pick it up.
                    reply(
                        results,
                        client_id,
                        Ok(LoginSuccess {
                            characters,
                            selected: None,
                        }),
                    );
                    kick(sim, target_entity, comp::DisconnectReason::Kicked);
                    logins.0.remove(&client_id);
                },
                Some(first) => {
                    let character_id = first.id;
                    // Mirrors `character_screen.rs`'s own guard
                    // (`ClientGeneral::Character`'s handler) before calling
                    // `load_character_data`: a delete/edit for this
                    // character may already be queued (e.g. by a concurrent
                    // LEGACY-path session for the same account) and not yet
                    // applied to the DB — the fresh `load_character_list`
                    // query above can't see that in-memory queue, so it may
                    // already be stale for this specific character. v1 has
                    // no "wait a few seconds and retry" UI over replicon
                    // (scope boundary), so this is a hard failure + kick
                    // rather than the legacy path's softer retry-later
                    // message.
                    let has_pending_action = sim
                        .server
                        .state()
                        .ecs()
                        .read_resource::<server::persistence::character_updater::CharacterUpdater>()
                        .has_pending_database_action(character_id);
                    if has_pending_action {
                        reply(
                            results,
                            client_id,
                            Err(LoginError::CharacterDataFailed(
                                "a character update was already pending for this character; try \
                                 again in a few seconds"
                                    .to_owned(),
                            )),
                        );
                        kick(sim, target_entity, comp::DisconnectReason::Kicked);
                        logins.0.remove(&client_id);
                        return;
                    }
                    let view_distances = ViewDistances {
                        terrain: DEFAULT_VIEW_DISTANCE,
                        entity: DEFAULT_VIEW_DISTANCE,
                    }
                    .clamp(sim.server.settings().max_view_distance);
                    // Mirrors `handle_initialize_character`: transitions
                    // `Presence` to `PresenceKind::LoadingCharacter(id)` — a
                    // precondition `update_character_data` (called below,
                    // once the async load completes) checks for.
                    sim.server.state_mut().initialize_character_data(
                        target_entity,
                        character_id,
                        view_distances,
                    );
                    let uuid_string = sim
                        .server
                        .state()
                        .read_component_cloned::<comp::Player>(target_entity)
                        .map(|player| player.uuid().to_string())
                        .unwrap_or_default();
                    loader.load_character_data(target_entity, uuid_string, character_id);
                    if let Some(session) = logins.0.get_mut(&client_id) {
                        session.stage = LoginStage::CharacterData {
                            character_id,
                            characters,
                        };
                    }
                },
            }
        },
    }
}

fn handle_character_data(
    sim: &mut SimServer,
    active: &mut ActiveReplicaSessions,
    results: &mut MessageWriter<ToClients<LoginResult>>,
    client_id: ClientId,
    target_entity: specs::Entity,
    character_id: CharacterId,
    characters: Vec<NetCharacterSummary>,
    data_result: Result<
        (
            server::persistence::PersistedComponents,
            common::event::UpdateCharacterMetadata,
        ),
        server::persistence::error::PersistenceError,
    >,
) {
    match data_result {
        Ok((components, _metadata)) => {
            // The exact entry point `handle_loaded_character_data` calls:
            // transitions `PresenceKind::LoadingCharacter(id)` →
            // `PresenceKind::Character(id)` and registers the `CharacterId`
            // in `IdMaps`.
            match sim
                .server
                .state_mut()
                .update_character_data(target_entity, components)
            {
                Ok(()) => {
                    // NOTE: currently a no-op for this entity —
                    // `initialize_region_subscription` only does anything if
                    // the entity has a legacy `Client` component (`server/
                    // src/sys/subscription.rs`), which a replicon-originated
                    // entity never does. Left in place (rather than removed)
                    // for parity with `handle_loaded_character_data`'s exact
                    // call, and because interest management for THIS
                    // transport (EM-4.2d/T47.6) will likely want an
                    // equivalent hook here once it exists — replicon
                    // visibility today is the EM-4.2b default (all entities
                    // to all clients), not gated on this call at all.
                    server::sys::subscription::initialize_region_subscription(
                        sim.server.state().ecs(),
                        target_entity,
                    );
                    // Track this as a fully logged-in session so a LATER
                    // duplicate login for the same account can find (and
                    // kick) this client — see `ActiveReplicaSessions`'s doc
                    // comment.
                    active.0.insert(target_entity, client_id);
                    reply(
                        results,
                        client_id,
                        Ok(LoginSuccess {
                            characters,
                            selected: Some(character_id),
                        }),
                    );
                },
                Err(err) => {
                    // Diverges from `handle_loaded_character_data`'s own
                    // failure branch, which calls `handle_exit_ingame`
                    // (return to character-select, entity stays alive) rather
                    // than a full kick — a deliberate v1 simplification since
                    // there is no character-select UI over replicon to return
                    // TO (scope boundary); a real failure here just ends the
                    // session instead.
                    reply(
                        results,
                        client_id,
                        Err(LoginError::CharacterDataFailed(err)),
                    );
                    kick(sim, target_entity, comp::DisconnectReason::Kicked);
                },
            }
        },
        Err(err) => {
            reply(
                results,
                client_id,
                Err(LoginError::CharacterDataFailed(err.to_string())),
            );
            kick(sim, target_entity, comp::DisconnectReason::Kicked);
        },
    }
}

/// Emits `ClientDisconnectEvent` the same way `register.rs`/`lib.rs` already
/// do from outside a specs `System` (`State::emit_event_now`) — the sim's own
/// (protocol-agnostic) disconnect pipeline (`handle_client_disconnect`) then
/// cleans up the entity generically on its normal per-tick pass, exactly as
/// it would for a legacy client. Reused for BOTH failure cleanup and
/// duplicate-login kicks.
fn kick(sim: &SimServer, entity: specs::Entity, reason: comp::DisconnectReason) {
    sim.server
        .state()
        .emit_event_now(ClientDisconnectEvent(entity, reason));
}

fn map_register_error(err: RegisterError) -> LoginError {
    match err {
        RegisterError::AuthError(msg) => LoginError::Auth(msg),
        RegisterError::Banned(info) => LoginError::Banned(info.reason),
        RegisterError::NotOnWhitelist => LoginError::NotOnWhitelist,
        RegisterError::TooManyPlayers => LoginError::TooManyPlayers,
        // Never actually produced by `LoginProvider::login` today, but
        // mapped for exhaustiveness rather than left to panic if that ever
        // changes.
        RegisterError::Kicked(msg) => LoginError::Auth(msg),
        RegisterError::InvalidCharacter => LoginError::InvalidCharacter,
    }
}

fn reply(
    results: &mut MessageWriter<ToClients<LoginResult>>,
    client_id: ClientId,
    outcome: Result<LoginSuccess, LoginError>,
) {
    results.write(ToClients {
        targets: SendTargets::Single(client_id),
        message: LoginResult { outcome },
    });
}
