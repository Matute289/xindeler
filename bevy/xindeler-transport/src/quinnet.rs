//! [`QuinnetTransport`]: the sole [`crate::ReplicaTransport`] impl (v1),
//! backed by `bevy_quinnet`/`bevy_replicon_quinnet` (QUIC). This is the ONLY
//! module in the workspace allowed to name `bevy_replicon_quinnet` or
//! `bevy_quinnet` types directly (T47.4's grep bar) — every other crate goes
//! through [`crate::ReplicaTransport`]/[`crate::TransportConfig`] instead.
//!
//! ## Config surface
//! - **Ports:** [`TransportConfig::bind_addr`] (server role: the listen
//!   address; client role: the local bind, normally an OS-assigned ephemeral
//!   port — see [`TransportConfig::client`]) and
//!   [`TransportConfig::server_addr`] (client role: which server to dial). This
//!   is a SEPARATE listener from the legacy `xindeler_network` quinn/TCP
//!   port(s) (`server::settings::Settings::gameserver_protocols`,
//!   `xindeler-server-app`'s own `sim.rs` doc comment) — the two must bind
//!   different ports and run side by side (dual-stack); nothing here touches
//!   the legacy listener.
//! - **TLS:** QUIC mandates TLS 1.3. The server generates a SELF-SIGNED
//!   certificate at boot (`CertificateRetrievalMode::GenerateSelfSigned` —
//!   there is no CA-signed-certificate story yet, so a fresh cert is minted
//!   every time the server process starts). The client verifies it via
//!   `bevy_quinnet`'s built-in **trust-on-first-use** scheme
//!   (`CertificateVerificationMode::TrustOnFirstUse`, EM-8.5) instead of the
//!   `SkipVerification` this transport shipped with through EM-4.2b–EM-8.4: the
//!   FIRST time a client dials a given `server_hostname`, it records that
//!   server's certificate fingerprint to `TransportConfig::known_hosts_path`
//!   (`<userdata>/known_hosts` by default — see that field's own doc comment);
//!   every SUBSEQUENT connection to the same hostname compares the presented
//!   fingerprint against the stored one and only proceeds if it still matches.
//!   [`start_client_connection`]'s own doc comment spells out the exact
//!   per-status behavior (unknown/trusted/untrusted) and why an untrusted
//!   (changed) fingerprint fails CLOSED (aborts the connection) rather than
//!   blocking on a UI prompt this project doesn't have yet.
//!
//!   **What this does and does not protect against.** Like SSH's own
//!   known-hosts model, TOFU cannot authenticate the very FIRST connection to
//!   a server — a network-path attacker positioned during that first dial
//!   could still impersonate the server and get its OWN fingerprint recorded
//!   as trusted. What it closes relative to `SkipVerification` is every
//!   connection AFTER that: an attacker who shows up later (a different
//!   network position, a compromised router, DNS/route hijacking after the
//!   fact) now gets detected and rejected, because their certificate won't
//!   match the one already on file. This mirrors the trust posture
//!   `XINDELER_SERVER_NO_AUTH` already accepts for THIS milestone's login
//!   story (see `xindeler-server-app`'s `main.rs`) — a real CA-signed
//!   certificate (closing the first-connection gap too) remains a further
//!   upgrade, tracked separately, not blocking for the current trusted-LAN/
//!   loopback-first rollout this transport targets.
//! - **NAT/firewall:** QUIC runs over UDP. A firewall/NAT in front of the
//!   server must allow inbound UDP on [`TransportConfig::bind_addr`]'s port;
//!   unlike the legacy listener's TCP option, there is no fallback if UDP is
//!   filtered (the legacy listener's own QUIC option carries the identical
//!   caveat — it is marked `experimental` in `server/src/lib.rs` for exactly
//!   this reason).

use std::collections::HashMap;

use bevy::{
    app::{App, Plugin, PluginGroup, PluginGroupBuilder, Startup},
    ecs::{
        resource::Resource,
        system::{Res, ResMut},
    },
};
use bevy_quinnet::{
    client::{
        ClientConnectionConfiguration, ClientConnectionConfigurationDefaultables, QuinnetClient,
        certificate::{
            CertVerificationStatus, CertVerifierAction, CertVerifierBehaviour,
            CertificateVerificationMode, KnownHosts, TrustOnFirstUseConfig,
        },
        connection::ClientAddrConfiguration,
    },
    server::{
        EndpointAddrConfiguration, QuinnetServer, ServerEndpointConfiguration,
        ServerEndpointConfigurationDefaultables, certificate::CertificateRetrievalMode,
    },
};
use bevy_replicon::prelude::RepliconChannels;
use bevy_replicon_quinnet::{
    ChannelsConfigurationExt, client::RepliconQuinnetClientPlugin,
    server::RepliconQuinnetServerPlugin,
};

use crate::{ReplicaTransport, TransportConfig};

/// The sole [`ReplicaTransport`] impl (v1). See the module doc comment for
/// the full config surface (ports/TLS/NAT).
pub struct QuinnetTransport;

impl ReplicaTransport for QuinnetTransport {
    fn server_plugins(&self, cfg: &TransportConfig) -> impl PluginGroup {
        QuinnetServerPlugins {
            config: cfg.clone(),
        }
    }

    fn client_plugins(&self, cfg: &TransportConfig) -> impl PluginGroup {
        QuinnetClientPlugins {
            config: cfg.clone(),
        }
    }
}

/// Server-role bundle: `bevy_replicon_quinnet`'s server-side replicon
/// integration (`RepliconQuinnetServerPlugin` — inserts `QuinnetServer` +
/// wires replicon's `ServerSystems::{ReceivePackets,SendPackets}` to it) plus
/// [`OpenServerEndpoint`] (opens the real listening socket at `Startup`).
struct QuinnetServerPlugins {
    config: TransportConfig,
}

impl PluginGroup for QuinnetServerPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(RepliconQuinnetServerPlugin)
            .add(OpenServerEndpoint(self.config))
    }
}

/// Client-role bundle: `bevy_replicon_quinnet`'s client-side replicon
/// integration (`RepliconQuinnetClientPlugin` — inserts `QuinnetClient` +
/// wires replicon's `ClientSystems::{ReceivePackets,SendPackets}` to it) plus
/// [`OpenClientConnection`] (dials the real server at `Startup`).
struct QuinnetClientPlugins {
    config: TransportConfig,
}

impl PluginGroup for QuinnetClientPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(RepliconQuinnetClientPlugin)
            .add(OpenClientConnection(self.config))
    }
}

/// The [`TransportConfig`] a boot system reads once at `Startup`. Shared
/// shape for both roles; only one of [`OpenServerEndpoint`]/
/// [`OpenClientConnection`] is ever added to a given `App` (never both), so
/// there is no collision inserting the same resource type.
#[derive(Resource, Clone)]
struct BootConfig(TransportConfig);

/// Opens the server's QUIC endpoint at [`Startup`].
///
/// By `Startup` time both preconditions this needs already hold, REGARDLESS
/// of `add_plugins` call order: `RepliconQuinnetServerPlugin` (added just
/// before this, in [`QuinnetServerPlugins::build`]) inserts the `QuinnetServer`
/// resource synchronously inside its own `Plugin::build`, and the caller's
/// `RepliconPlugins` + `XindelerProtocolPlugin` (added before this whole
/// plugin group — see `xindeler-server-app::plugin::SimServerPlugin`)
/// populate `RepliconChannels` synchronously inside THEIR `Plugin::build`
/// calls too. Every plugin's `build` runs to completion, for every plugin in
/// the `App`, before any `Startup` system runs — so both resources are
/// guaranteed present here independent of which `add_plugins` call listed
/// which plugin first.
struct OpenServerEndpoint(TransportConfig);

impl Plugin for OpenServerEndpoint {
    fn build(&self, app: &mut App) {
        app.insert_resource(BootConfig(self.0.clone()))
            .add_systems(Startup, start_server_endpoint);
    }
}

fn start_server_endpoint(
    mut server: ResMut<QuinnetServer>,
    channels: Res<RepliconChannels>,
    config: Res<BootConfig>,
) {
    let cfg = &config.0;
    let outcome = server.start_endpoint(ServerEndpointConfiguration {
        addr_config: EndpointAddrConfiguration::from_addr(cfg.bind_addr),
        cert_mode: CertificateRetrievalMode::GenerateSelfSigned {
            server_hostname: cfg.server_hostname.clone(),
        },
        // No `..Default::default()`: with `recv_channels` deliberately OFF
        // (see this crate's Cargo.toml), `send_channels_cfg` is the only
        // field this struct has.
        defaultables: ServerEndpointConfigurationDefaultables {
            send_channels_cfg: channels.server_configs(),
        },
    });
    match outcome {
        Ok(_certificate) => {
            tracing::info!(
                addr = %cfg.bind_addr,
                "replicon/quinnet transport listening (EM-4.2b; dual-stack alongside the legacy \
                 xindeler_network listener)"
            );
        },
        Err(err) => {
            tracing::error!(
                ?err,
                addr = %cfg.bind_addr,
                "failed to start the replicon/quinnet server endpoint"
            );
        },
    }
}

/// Dials the server's QUIC endpoint at [`Startup`] (same insertion-order
/// reasoning as [`OpenServerEndpoint`]'s doc comment, mirrored for the client
/// role: `RepliconQuinnetClientPlugin` inserts `QuinnetClient` synchronously
/// just before this in [`QuinnetClientPlugins::build`], and the caller's
/// `RepliconPlugins`/`XindelerProtocolPlugin` populate `RepliconChannels`
/// synchronously too).
struct OpenClientConnection(TransportConfig);

impl Plugin for OpenClientConnection {
    fn build(&self, app: &mut App) {
        app.insert_resource(BootConfig(self.0.clone()))
            .add_systems(Startup, start_client_connection);
    }
}

/// The trust-on-first-use verifier behavior this project deliberately
/// deviates from `bevy_quinnet`'s own default
/// (`TrustOnFirstUseConfig::default()`) for exactly one status:
///
/// | Status                | `bevy_quinnet` default | This project (EM-8.5)       |
/// |------------------------|-------------------------|------------------------------|
/// | `UnknownCertificate`  | `TrustAndStore`         | `TrustAndStore` (unchanged) |
/// | `TrustedCertificate`  | `TrustOnce`             | `TrustOnce` (unchanged)     |
/// | `UntrustedCertificate`| `RequestClientAction`   | **`AbortConnection`**       |
///
/// `RequestClientAction` raises a `CertInteractionEvent` and blocks the
/// async connection task (`block_on`) until some system calls
/// `CertInteractionEvent::apply_cert_verifier_action` — but this client has
/// no cert-trust-prompt UI (v1, single-owner/small-scale usage per the task
/// brief), so nothing would ever answer that event and the connection
/// attempt would hang forever. Explicitly overriding to `AbortConnection`
/// instead fails CLOSED and immediately (matches the module doc comment's
/// "an attacker who shows up later... gets detected and rejected" claim,
/// and mirrors how a stock `ssh` client refuses to proceed past its own
/// "REMOTE HOST IDENTIFICATION HAS CHANGED" warning without an explicit,
/// separate, deliberate action from the operator — there is no equivalent
/// operator-facing prompt here yet, so refusing is the only safe default).
fn tofu_verifier_behaviour() -> HashMap<CertVerificationStatus, CertVerifierBehaviour> {
    HashMap::from([
        (
            CertVerificationStatus::UnknownCertificate,
            CertVerifierBehaviour::ImmediateAction(CertVerifierAction::TrustAndStore),
        ),
        (
            CertVerificationStatus::TrustedCertificate,
            CertVerifierBehaviour::ImmediateAction(CertVerifierAction::TrustOnce),
        ),
        (
            CertVerificationStatus::UntrustedCertificate,
            CertVerifierBehaviour::ImmediateAction(CertVerifierAction::AbortConnection),
        ),
    ])
}

fn start_client_connection(
    mut client: ResMut<QuinnetClient>,
    channels: Res<RepliconChannels>,
    config: Res<BootConfig>,
) {
    let cfg = &config.0;
    let outcome = client.open_connection(ClientConnectionConfiguration {
        addr_config: ClientAddrConfiguration::from_addrs_with_name(
            cfg.server_addr,
            cfg.server_hostname.clone(),
            cfg.bind_addr,
        ),
        // EM-8.5: trust-on-first-use, see this module's doc comment + this
        // function's own doc comment on [`tofu_verifier_behaviour`] for the
        // full reasoning (why TOFU, why the one deliberate deviation from
        // `bevy_quinnet`'s own default behaviour map).
        cert_mode: CertificateVerificationMode::TrustOnFirstUse(TrustOnFirstUseConfig {
            known_hosts: KnownHosts::HostsFile(cfg.known_hosts_path.to_string_lossy().into_owned()),
            verifier_behaviour: tofu_verifier_behaviour(),
        }),
        // No `..Default::default()`: same reasoning as the server side above.
        defaultables: ClientConnectionConfigurationDefaultables {
            send_channels_cfg: channels.client_configs(),
        },
    });
    match outcome {
        Ok(_connection_id) => {
            tracing::info!(
                server_addr = %cfg.server_addr,
                bind_addr = %cfg.bind_addr,
                "replicon/quinnet transport dialing server (EM-4.2b)"
            );
        },
        Err(err) => {
            tracing::error!(
                ?err,
                server_addr = %cfg.server_addr,
                "failed to open the replicon/quinnet client connection"
            );
        },
    }
}
