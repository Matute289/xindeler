use crate::{
    Client,
    settings::{AdminRecord, Ban, Banlist, WhitelistRecord, banlist::NormalizedIpAddr},
};
use authc::{AuthClient, AuthClientError, AuthToken, Uuid};
use chrono::Utc;
use common::comp::{AdminRole, Player};
use common_net::msg::RegisterError;
use hashbrown::HashMap;
use specs::{Component, Join, WorldExt};
use std::{str::FromStr, sync::Arc};
use tokio::{runtime::Runtime, sync::oneshot};
use tracing::{error, info};

/// Determines whether a user is banned, given a ban record connected to a user,
/// the `AdminRecord` of that user (if it exists), and the current time.
pub fn ban_applies(
    ban: &Ban,
    admin: Option<&AdminRecord>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    // Make sure the ban is active, and that we can't override it.
    //
    // If we are an admin and our role is at least as high as the role of the
    // person who banned us, we can override the ban; we negate this to find
    // people who cannot override it.
    let exceeds_ban_role = |admin: &AdminRecord| {
        AdminRole::from(admin.role) >= AdminRole::from(ban.performed_by_role())
    };
    !ban.is_expired(now) && !admin.is_some_and(exceeds_ban_role)
}

fn derive_uuid(username: &str) -> Uuid {
    let mut state = 144066263297769815596495629667062367629;

    for byte in username.as_bytes() {
        state ^= *byte as u128;
        state = state.wrapping_mul(309485009821345068724781371);
    }

    Uuid::from_u128(state)
}

/// derive Uuid for "singleplayer" is a pub fn
pub fn derive_singleplayer_uuid() -> Uuid { derive_uuid("singleplayer") }

/// Generalizes [`derive_singleplayer_uuid`] to an arbitrary username — the
/// same derivation the no-auth-server login path below
/// (`Ok(derive_uuid(username))`) uses at registration time, so a caller that
/// knows a disabled-auth server's client logs in under a specific username
/// can compute the SAME uuid that login will assign it.
///
/// `pub(crate)`: only [`crate::settings::EditableSettings::grant_admin`] uses
/// this today (BL-82: the listen-server embedded player logs in as
/// `"listen_host"`, not `"singleplayer"`, so [`EditableSettings::
/// singleplayer`]'s own `derive_singleplayer_uuid()` grant never covers it —
/// see that method's doc for the full story). Kept crate-internal rather than
/// `pub` since nothing outside `server` needs the raw derivation.
pub(crate) fn derive_uuid_for_username(username: &str) -> Uuid { derive_uuid(username) }

/// Counts entities currently carrying a `Player` component — the same
/// player-count-cap denominator `server/src/sys/msg/register.rs` computes
/// inline via its own `SystemData` join.
///
/// Exposed (BL-82 EM-4.2c) so a caller with no specs `System`/`SystemData`
/// context — e.g. a Bevy system driving the replicon login handshake
/// (`xindeler-server-app::login`) — can reuse the SAME check instead of
/// reaching past this crate's public API into raw `WriteStorage`/`Entities`.
/// This is an ADDITIVE new entry point (not a widened existing one) — see
/// [`LoginProvider::login_with_ip`]'s doc comment for why that distinction
/// matters under the isolation law.
#[must_use]
pub fn count_players(world: &specs::World) -> usize {
    let entities = world.entities();
    let players = world.read_storage::<Player>();
    (&entities, &players).join().count()
}

/// Finds an entity whose `Player` component has the given `uuid`, excluding
/// `exclude` (so a caller checking "is anyone ELSE already logged in as this
/// account" doesn't match its own not-yet-committed entity). Used for
/// duplicate-login detection (same account reconnecting) — see
/// [`count_players`]'s doc comment for why this exists as a public entry
/// point rather than requiring callers to touch specs storages directly.
#[must_use]
pub fn find_other_player_entity_by_uuid(
    world: &specs::World,
    uuid: Uuid,
    exclude: specs::Entity,
) -> Option<specs::Entity> {
    let entities = world.entities();
    let players = world.read_storage::<Player>();
    (&entities, &players)
        .join()
        .find(|(entity, player)| *entity != exclude && player.uuid() == uuid)
        .map(|(entity, _)| entity)
}

pub struct PendingLogin {
    pending_r: oneshot::Receiver<Result<(String, Uuid), RegisterError>>,
}

impl PendingLogin {
    pub(crate) fn new_success(username: String, uuid: Uuid) -> Self {
        let (pending_s, pending_r) = oneshot::channel();
        let _ = pending_s.send(Ok((username, uuid)));

        Self { pending_r }
    }
}

impl Component for PendingLogin {
    type Storage = specs::DenseVecStorage<Self>;
}

pub struct LoginProvider {
    runtime: Arc<Runtime>,
    auth_server: Option<Arc<AuthClient>>,
}

impl LoginProvider {
    pub fn new(auth_addr: Option<String>, runtime: Arc<Runtime>) -> Self {
        tracing::trace!(?auth_addr, "Starting LoginProvider");

        let auth_server = auth_addr.map(|addr| {
            let (scheme, authority) = addr.split_once("://").expect("invalid auth url");

            let scheme = scheme
                .parse::<authc::Scheme>()
                .expect("invalid auth url scheme");
            let authority = authority
                .parse::<authc::Authority>()
                .expect("invalid auth url authority");

            Arc::new(AuthClient::new(scheme, authority).expect("insecure auth scheme"))
        });

        Self {
            runtime,
            auth_server,
        }
    }

    pub fn verify(&self, username_or_token: &str) -> PendingLogin {
        let (pending_s, pending_r) = oneshot::channel();

        match &self.auth_server {
            // Token from auth server expected
            Some(srv) => {
                let srv = Arc::clone(srv);
                let username_or_token = username_or_token.to_string();
                self.runtime.spawn(async move {
                    let _ = pending_s.send(Self::query(srv, &username_or_token).await);
                });
            },
            // Username is expected
            None => {
                let username = username_or_token;
                let uuid = derive_uuid(username);
                let _ = pending_s.send(Ok((username.to_string(), uuid)));
            },
        }

        PendingLogin { pending_r }
    }

    /// Resolves a [`PendingLogin`] against the ban/whitelist/player-count
    /// rules for a legacy TCP/QUIC connection (`server/src/sys/
    /// msg/register.rs`), reading the connecting IP off `client`.
    ///
    /// `pub(crate)` — unchanged from upstream. BL-82 EM-4.2c needed this
    /// same logic reachable from `xindeler-server-app` (the replicon login
    /// handshake, which has no legacy `Client` to read an IP from) but does
    /// NOT widen this function to do it — see [`Self::login_with_ip`],
    /// which this delegates to, for the additive entry point that serves
    /// that caller instead. Keeping this signature/visibility identical to
    /// parent means the isolation-law surface crossed by EM-4.2c is limited
    /// to one new function, not a modified existing one.
    pub(crate) fn login<R>(
        pending: &mut PendingLogin,
        client: &Client,
        admins: &HashMap<Uuid, AdminRecord>,
        whitelist: &HashMap<Uuid, WhitelistRecord>,
        banlist: &Banlist,
        player_count_exceeded: impl FnOnce(String, Uuid) -> (bool, R),
        make_ip_ban_upgrade: impl FnOnce(NormalizedIpAddr, Uuid, String),
    ) -> Option<Result<R, RegisterError>> {
        // We ignore mpsc connections since those aren't to an external
        // process.
        let ip = client
            .connected_from_addr()
            .socket_addr()
            .map(|s| s.ip())
            .map(NormalizedIpAddr::from);
        Self::login_with_ip(
            pending,
            ip,
            admins,
            whitelist,
            banlist,
            player_count_exceeded,
            make_ip_ban_upgrade,
        )
    }

    /// Resolves a [`PendingLogin`] against the ban/whitelist/player-count
    /// rules, exactly as [`Self::login`] does for a legacy `Client` (which is
    /// now a thin wrapper around this that reads the IP off `client`).
    ///
    /// `ip` is the connecting socket's address, pre-resolved by the caller.
    /// This lets a caller with no legacy `Client` at all — e.g. the replicon
    /// +quinnet login handshake in `xindeler-server-app::login` (BL-82
    /// EM-4.2c) — reuse this EXACT check logic instead of reimplementing
    /// ban/whitelist/player-count-cap itself. `None` means either a
    /// loopback/mpsc connection (legacy behavior, unchanged) or a transport
    /// that doesn't yet surface a client IP (the replicon+quinnet transport,
    /// as of EM-4.2c — IP bans don't apply over it yet; UUID-based
    /// ban/whitelist/player-count-cap checks below are unaffected).
    ///
    /// Public and additive (added BL-82 EM-4.2c): this is a BRAND NEW entry
    /// point, not a widened one — [`Self::login`]'s own `pub(crate)`
    /// visibility and `&Client`-taking signature are unchanged from
    /// upstream, so `register.rs` (the legacy caller) needed zero edits.
    /// This is the isolation-law-compliant shape of the fix described in
    /// `docs/design/tasks/48-bl82-wave3-regression-fixes-tasks.md` (T48.4,
    /// Finding E, option (a)): "host the function inside `server` itself...
    /// so the crate boundary is not crossed by widening `login`."
    pub fn login_with_ip<R>(
        pending: &mut PendingLogin,
        ip: Option<NormalizedIpAddr>,
        admins: &HashMap<Uuid, AdminRecord>,
        whitelist: &HashMap<Uuid, WhitelistRecord>,
        banlist: &Banlist,
        player_count_exceeded: impl FnOnce(String, Uuid) -> (bool, R),
        make_ip_ban_upgrade: impl FnOnce(NormalizedIpAddr, Uuid, String),
    ) -> Option<Result<R, RegisterError>> {
        match pending.pending_r.try_recv() {
            Ok(Err(e)) => Some(Err(e)),
            Ok(Ok((username, uuid))) => {
                let now = Utc::now();
                // Hardcoded admins can always log in.
                let admin = admins.get(&uuid);
                if let Some(ban) = banlist
                    .uuid_bans()
                    .get(&uuid)
                    .and_then(|ban_entry| ban_entry.current.action.ban())
                    .into_iter()
                    .chain(ip.and_then(|ip| {
                        banlist
                            .ip_bans()
                            .get(&ip)
                            .and_then(|ban_entry| ban_entry.current.action.ban())
                    }))
                    .find(|ban| ban_applies(ban, admin, now))
                {
                    if let Some(ip) = ip
                        && ban.upgrade_to_ip
                    {
                        make_ip_ban_upgrade(ip, uuid, username.clone());
                    }

                    // Get ban info and send a copy of it
                    return Some(Err(RegisterError::Banned(ban.info())));
                }

                // non-admins can only join if the whitelist is empty (everyone can join)
                // or their name is in the whitelist.
                if admin.is_none() && !whitelist.is_empty() && !whitelist.contains_key(&uuid) {
                    return Some(Err(RegisterError::NotOnWhitelist));
                }

                // non-admins can only join if the player count has not been exceeded.
                let (player_count_exceeded, res) = player_count_exceeded(username, uuid);
                if admin.is_none() && player_count_exceeded {
                    return Some(Err(RegisterError::TooManyPlayers));
                }

                Some(Ok(res))
            },
            Err(oneshot::error::TryRecvError::Closed) => {
                error!("channel got closed to early, this shouldn't happen");
                Some(Err(RegisterError::AuthError(
                    "Internal Error verifying".to_string(),
                )))
            },
            Err(oneshot::error::TryRecvError::Empty) => None,
        }
    }

    async fn query(
        srv: Arc<AuthClient>,
        username_or_token: &str,
    ) -> Result<(String, Uuid), RegisterError> {
        info!(?username_or_token, "Validating token");
        // Parse token
        let token = AuthToken::from_str(username_or_token)
            .map_err(|e| RegisterError::AuthError(e.to_string()))?;
        // Validate token
        match async {
            let uuid = srv.validate(token).await?;
            let username = srv.uuid_to_username(uuid).await?;
            let r: Result<_, AuthClientError> = Ok((username, uuid));
            r
        }
        .await
        {
            Err(e) => Err(RegisterError::AuthError(e.to_string())),
            Ok((username, uuid)) => Ok((username, uuid)),
        }
    }

    pub fn username_to_uuid(&self, username: &str) -> Result<Uuid, AuthClientError> {
        match &self.auth_server {
            Some(srv) => {
                //TODO: optimize
                self.runtime.block_on(srv.username_to_uuid(&username))
            },
            None => Ok(derive_uuid(username)),
        }
    }

    pub fn uuid_to_username(
        &self,
        uuid: Uuid,
        fallback_alias: &str,
    ) -> Result<String, AuthClientError> {
        match &self.auth_server {
            Some(srv) => {
                //TODO: optimize
                self.runtime.block_on(srv.uuid_to_username(uuid))
            },
            None => Ok(fallback_alias.into()),
        }
    }
}
