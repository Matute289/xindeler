//! Transport abstraction seam (BL-82 EM-4.2b):
//! [`xindeler-server-app`](../xindeler_server_app) and [`xindeler-client`](../
//! xindeler_client) depend on [`ReplicaTransport`]/ [`TransportConfig`] here —
//! and ONLY on these two items — never on `bevy_replicon_quinnet` (or any
//! future transport crate) directly. `bevy_replicon_quinnet`/`bevy_quinnet`
//! types are confined to [`quinnet`]; a `grep -r bevy_replicon_quinnet` over
//! the rest of the workspace must return nothing outside that one module
//! (T47.4's hard acceptance bar).
//!
//! ## Why this crate exists (spec §0.1/§1.1)
//! Before this crate, no `bevy_replicon` connection anywhere in this codebase
//! ever crossed a real socket: `xindeler-server-app` only opened the legacy
//! `xindeler_network` quinn/TCP listener, and `xindeler-client`'s listen-server
//! mode ran replicon server-role-only, entirely in-process (replicon's local
//! `send_locally` loopback — one `App`, zero network bytes). This crate is the
//! FIRST real transport, wired into `xindeler-server-app` (server role,
//! dual-stack alongside the untouched legacy listener — see that crate's
//! `plugin.rs`) and `xindeler-client` (a new, genuinely remote client role —
//! `net_client.rs` there, deliberately NOT `listen_server.rs`'s in-process
//! path, which still hosts a whole embedded server and stays untouched).
//!
//! ## Trait, not a closed enum — and why the resulting non-object-safety is fine
//! [`ReplicaTransport`]'s methods return `impl PluginGroup` (return-position
//! `impl Trait` in traits, stable since Rust 1.75) rather than a boxed
//! `dyn PluginGroup` — Bevy's own `PluginGroup`/`PluginGroupBuilder` aren't
//! meaningfully dyn-safe either (`PluginGroupBuilder::add` is generic over the
//! concrete plugin type). That makes [`ReplicaTransport`] NOT object-safe, but
//! that is a non-issue here: which transport backend is compiled in is a
//! compile-time choice, never a runtime one — today the only impl is
//! [`quinnet::QuinnetTransport`]; a second impl (e.g. a future
//! `Renet2Transport`, once `bevy_replicon_renet2` catches up to our
//! `bevy`/`bevy_replicon` pins — see the renet2-fork evaluation in the EM-4.2b
//! PR description) would be selected via a config/feature-flag choice made by
//! the SHELL (`xindeler-server-app`/`xindeler-client`) at their own call site,
//! never via `dyn ReplicaTransport`. A closed enum was the documented
//! alternative (T47.4's brief explicitly leaves this call to the
//! implementer); the trait was chosen because it keeps `QuinnetTransport`'s
//! own plugin-composition logic (which `bevy_replicon_quinnet`/`bevy_quinnet`
//! plugins to bundle, plus the endpoint-opening `Startup` system) entirely
//! inside `quinnet.rs`, satisfying the grep bar trivially — an enum would need
//! its variants' payload types to also live outside `quinnet.rs` for the
//! calling crates to name them, which is a more awkward shape for a
//! single-impl seam than it sounds.
//!
//! ## The bar for "is this abstraction real, not decorative" (per the spec)
//! Adding a second transport later should cost exactly one new
//! `impl ReplicaTransport for Renet2Transport` block in a new sibling module +
//! a config variant at the shell's call site — touching ZERO lines in
//! `xindeler-server-app`/`xindeler-client` beyond that one config value
//! change. Both shells already call only `SomeTransport.server_plugins(&cfg)`/
//! `.client_plugins(&cfg)` through this trait; swapping `SomeTransport` for a
//! different concrete type (behind a config choice) is the only edit either
//! shell would need.
//!
//! ## Both `client`+`server` bevy_replicon roles compile into EVERY shell
//! `bevy_replicon`'s own `Cargo.toml` enables `client`+`server` by
//! default, and Cargo's feature unification means BOTH end up enabled for
//! every binary that (transitively) depends on it in this workspace —
//! including `xindeler-protocol`, which every shell links and which
//! deliberately depends on `bevy_replicon` with default features BY DESIGN
//! (its own doc comment: it registers the replication contract
//! symmetrically, "the same plugin runs on client and server"). Concretely
//! this means `xindeler-server-app` (server role) also compiles/registers
//! `bevy_replicon::client::ClientPlugin`/`ClientMessagePlugin`, and
//! `xindeler-client`'s `net_client` (client role) also compiles/registers
//! `ServerPlugin`/`ServerMessagePlugin` — in both cases dormant, since
//! neither process ever opens the OTHER role's transport connection
//! (`ClientState`/`ServerState` never leave their default/inactive value
//! for the role that isn't in use). Stripping this at either shell's own
//! `Cargo.toml` (`default-features = false` + an explicit role feature)
//! would NOT change the resolved feature set for a build that also links
//! `xindeler-protocol` — splitting that would need per-role variants of
//! `xindeler-protocol` itself, a larger change out of EM-4.2b's scope.

pub mod quinnet;

pub use quinnet::QuinnetTransport;

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
};

use bevy::app::PluginGroup;

/// Address/identity configuration a [`ReplicaTransport`] needs to open its
/// server or client role. One shape serves both roles — a role simply ignores
/// the field it doesn't need (see [`Self::server`]/[`Self::client`]'s doc
/// comments for which fields matter to which role).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportConfig {
    /// Local address to bind.
    ///
    /// Server role: the address to LISTEN on. Pick a port distinct from the
    /// legacy `xindeler_network` quinn/TCP listener(s)
    /// (`server::settings::Settings::gameserver_protocols`) — the two
    /// transports run side by side on one server process (dual-stack) and
    /// must never collide.
    ///
    /// Client role: the local address to bind the OUTGOING connection to —
    /// normally an OS-assigned ephemeral port on the matching wildcard
    /// interface (port `0`; see [`Self::client`]).
    pub bind_addr: SocketAddr,
    /// The remote peer's address.
    ///
    /// Client role: the server to dial. Server role: unused (a server has no
    /// single remote peer at boot; kept equal to [`Self::bind_addr`] by
    /// [`Self::server`] purely so the field is never a meaningless default).
    pub server_addr: SocketAddr,
    /// Subject name for the server's self-signed TLS certificate (server
    /// role) / the hostname claim the client connection is constructed with,
    /// and the key a [`Self::known_hosts_path`] entry is stored/looked up
    /// under (client role — see [`quinnet`]'s module doc comment for the
    /// trust-on-first-use scheme this now drives).
    pub server_hostname: String,
    /// **Client role only** (ignored server-side, same convention
    /// [`Self::server_addr`] documents for itself): path of the persistent
    /// "known hosts" fingerprint store [`quinnet`]'s trust-on-first-use
    /// verifier reads/writes (EM-8.5). Defaults to `<userdata>/known_hosts`
    /// (`xindeler_app::settings::userdata_dir()` — the same per-user data
    /// directory `XindelerSettings::path()` uses for `settings.ron`), so it
    /// survives restarts and honors `XINDELER_USERDATA` the same way every
    /// other piece of client-local persistent state in this codebase does.
    pub known_hosts_path: PathBuf,
}

impl TransportConfig {
    /// A server-role config listening on `bind_addr`. `known_hosts_path` is
    /// set for structural consistency (never a meaningless default, same
    /// reasoning as `server_addr` above) but never read server-side: a
    /// server presents its own self-signed certificate, it never verifies a
    /// peer's.
    #[must_use]
    pub fn server(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            server_addr: bind_addr,
            server_hostname: bind_addr.ip().to_string(),
            known_hosts_path: default_known_hosts_path(),
        }
    }

    /// A client-role config dialing `server_addr`, bound to an OS-assigned
    /// ephemeral port (`0`) on the wildcard interface matching `server_addr`'s
    /// IP family, with [`Self::known_hosts_path`] defaulted to
    /// `<userdata>/known_hosts`.
    #[must_use]
    pub fn client(server_addr: SocketAddr) -> Self {
        let unspecified = match server_addr {
            SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };
        Self {
            bind_addr: SocketAddr::new(unspecified, 0),
            server_addr,
            server_hostname: server_addr.ip().to_string(),
            known_hosts_path: default_known_hosts_path(),
        }
    }
}

/// `<userdata>/known_hosts` — see [`TransportConfig::known_hosts_path`].
fn default_known_hosts_path() -> PathBuf {
    xindeler_app::settings::userdata_dir().join("known_hosts")
}

/// The transport seam. `server_plugins`/`client_plugins` each bundle
/// EVERYTHING a role needs — the backend's own replicon-integration plugin(s)
/// plus whatever bootstrap opens the real socket — into one [`PluginGroup`]
/// the caller adds with a single `App::add_plugins` call. See the module doc
/// comment for why this is a trait rather than a closed enum, and why the
/// resulting non-object-safety doesn't matter.
pub trait ReplicaTransport {
    /// The server-role plugin bundle: registers the backend's replicon
    /// integration and opens the listening endpoint at `cfg.bind_addr`.
    fn server_plugins(&self, cfg: &TransportConfig) -> impl PluginGroup;

    /// The client-role plugin bundle: registers the backend's replicon
    /// integration and dials `cfg.server_addr`.
    fn client_plugins(&self, cfg: &TransportConfig) -> impl PluginGroup;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_binds_and_advertises_the_same_address() {
        let addr: SocketAddr = "127.0.0.1:14006".parse().expect("valid literal");
        let cfg = TransportConfig::server(addr);
        assert_eq!(cfg.bind_addr, addr);
        assert_eq!(cfg.server_addr, addr);
        assert_eq!(cfg.server_hostname, "127.0.0.1");
        assert_eq!(cfg.known_hosts_path, default_known_hosts_path());
    }

    #[test]
    fn client_config_binds_the_matching_unspecified_wildcard() {
        let v4: SocketAddr = "127.0.0.1:14006".parse().expect("valid literal");
        let cfg = TransportConfig::client(v4);
        assert_eq!(cfg.server_addr, v4);
        assert_eq!(cfg.bind_addr.ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(cfg.bind_addr.port(), 0);
        assert_eq!(cfg.server_hostname, "127.0.0.1");
        assert_eq!(cfg.known_hosts_path, default_known_hosts_path());

        let v6: SocketAddr = "[::1]:14006".parse().expect("valid literal");
        let cfg = TransportConfig::client(v6);
        assert_eq!(cfg.bind_addr.ip(), IpAddr::V6(Ipv6Addr::UNSPECIFIED));
    }
}
