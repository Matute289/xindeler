//! BL-82 EM-8.5 acceptance test — proves the switch away from
//! `CertificateVerificationMode::SkipVerification` to `bevy_quinnet`'s
//! trust-on-first-use scheme (`quinnet.rs`'s own module doc comment) actually
//! does two real things, over a REAL loopback QUIC endpoint (not a mock):
//!
//! 1. The FIRST connection to a never-before-seen server records that server's
//!    certificate fingerprint to `TransportConfig::known_hosts_path` and
//!    connects successfully (`UnknownCertificate` -> `TrustAndStore`).
//! 2. A SECOND, independent connection to the SAME server, reusing the SAME
//!    known_hosts file, reuses/matches the already-recorded fingerprint without
//!    rewriting it (`TrustedCertificate` -> `TrustOnce`) and connects
//!    successfully too.
//! 3. A connection whose known_hosts file already contains a DIFFERENT (bogus)
//!    fingerprint for that hostname is refused (`UntrustedCertificate` ->
//!    `AbortConnection`, this project's own deliberate fail-closed override —
//!    see `quinnet.rs`'s `tofu_verifier_behaviour` doc comment) rather than
//!    hanging or silently connecting anyway.
//!
//! ## Harness
//! Both the "server" and "client" sides here are real, separate Bevy `App`s
//! in the SAME test process, each running the exact plugin recipe production
//! code uses (`(MinimalPlugins, StatesPlugin, RepliconPlugins,
//! QuinnetTransport.{server,client}_plugins(&cfg))` — see
//! `xindeler-server-app::plugin`'s `SimServerPlugin::build` / this crate's own
//! `net_client.rs`), manually pumped with `app.update()` — the SAME idiom
//! `xindeler-protocol`'s own `lib.rs` test module already establishes for a
//! from-scratch `RepliconPlugins` test app (`.finish()` before the first
//! `update()`, since `RepliconChannels`/replicon's `ServerPlugin`/
//! `ClientPlugin` populate their resources in `Plugin::finish`, not
//! `Plugin::build`). Unlike that module's tests, this one does NOT use
//! replicon's transport-less `ServerTestAppExt` loopback — the whole point
//! here is a REAL UDP/QUIC socket and a REAL TLS handshake, so
//! `xindeler_transport::QuinnetTransport` is exercised exactly as a real
//! client/server process would.
//!
//! No real game assets/LFS are needed — this is pure networking, so (unlike
//! most `#[ignore]`d full-world acceptance tests elsewhere in `bevy/*`) these
//! tests run unconditionally under `cargo test -p xindeler-transport`.

use std::{
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

use bevy::{MinimalPlugins, prelude::*, state::app::StatesPlugin};
use bevy_quinnet::client::QuinnetClient;
use bevy_replicon::prelude::RepliconPlugins;
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

/// Every `TransportConfig` in this test dials/binds loopback v4, so
/// `server_hostname` (derived from the IP by both `TransportConfig::server`/
/// `::client`) is always this literal.
const HOSTNAME: &str = "127.0.0.1";

/// A bogus, never-matching certificate fingerprint: 32 zero bytes,
/// base64-encoded (the exact encoding `bevy_quinnet`'s known_hosts file
/// format uses — see `bevy_quinnet::client::certificate`'s doc comment). A
/// real SHA-256 fingerprint of an actual certificate will never collide with
/// this.
const BOGUS_FINGERPRINT_B64: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn free_loopback_addr() -> SocketAddr {
    let port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    ([127, 0, 0, 1], port).into()
}

/// Builds a real server-role `App` listening on `bind_addr`, using the exact
/// plugin recipe production code uses (see module doc comment).
fn new_server_app(bind_addr: SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins,
        QuinnetTransport.server_plugins(&TransportConfig::server(bind_addr)),
    ))
    .finish();
    app
}

/// Builds a real client-role `App` dialing `server_addr`, verifying the
/// server's certificate via trust-on-first-use against `known_hosts_path`.
fn new_client_app(server_addr: SocketAddr, known_hosts_path: PathBuf) -> App {
    let cfg = TransportConfig {
        known_hosts_path,
        ..TransportConfig::client(server_addr)
    };
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins,
        QuinnetTransport.client_plugins(&cfg),
    ))
    .finish();
    app
}

fn client_is_connected(app: &App) -> bool { app.world().resource::<QuinnetClient>().is_connected() }

fn client_is_disconnected(app: &App) -> bool {
    app.world().resource::<QuinnetClient>().is_disconnected()
}

/// Pumps `server`+`client` together until `done(client)` is true, or panics
/// once `deadline` passes. A short sleep between iterations lets
/// `bevy_quinnet`'s background tokio tasks (the real async QUIC/TLS work)
/// make progress between polls — the same idiom
/// `xindeler-server-app`'s own integration tests use for a real, separately-
/// clocked process.
fn pump_until(
    server: &mut App,
    client: &mut App,
    deadline: Instant,
    what: &str,
    mut done: impl FnMut(&App) -> bool,
) {
    loop {
        server.update();
        client.update();
        if done(client) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn tofu_records_and_reuses_trusted_fingerprint_across_reconnect() {
    let server_addr = free_loopback_addr();
    let mut server = new_server_app(server_addr);

    let tmp = tempfile::tempdir().expect("tempdir");
    let known_hosts = tmp.path().join("known_hosts");
    assert!(
        !known_hosts.exists(),
        "starts with no prior trust store on disk"
    );

    // ---- first connection: UnknownCertificate -> TrustAndStore ----
    let mut client_a = new_client_app(server_addr, known_hosts.clone());
    pump_until(
        &mut server,
        &mut client_a,
        Instant::now() + Duration::from_secs(20),
        "first connection (unknown certificate) to reach Connected",
        client_is_connected,
    );
    println!("[test] first connection trusted+recorded the server's certificate and connected");

    assert!(
        known_hosts.exists(),
        "the TrustAndStore action must have written the known_hosts file to disk"
    );
    let recorded_after_first =
        std::fs::read_to_string(&known_hosts).expect("read known_hosts after first connect");
    assert!(
        recorded_after_first.contains(HOSTNAME),
        "known_hosts must record an entry for {HOSTNAME}, got: {recorded_after_first:?}"
    );

    // ---- second, independent connection: SAME server, SAME known_hosts file
    // -> TrustedCertificate -> TrustOnce ----
    let mut client_b = new_client_app(server_addr, known_hosts.clone());
    pump_until(
        &mut server,
        &mut client_b,
        Instant::now() + Duration::from_secs(20),
        "second connection (already-trusted certificate) to reach Connected",
        client_is_connected,
    );
    println!(
        "[test] second, independent connection reused the SAME recorded fingerprint and connected \
         too"
    );

    let recorded_after_second =
        std::fs::read_to_string(&known_hosts).expect("read known_hosts after second connect");
    assert_eq!(
        recorded_after_first, recorded_after_second,
        "a matching reconnect must REUSE the existing known_hosts entry, not rewrite it"
    );
}

#[test]
fn tofu_aborts_when_the_known_fingerprint_no_longer_matches() {
    let server_addr = free_loopback_addr();
    let mut server = new_server_app(server_addr);

    let tmp = tempfile::tempdir().expect("tempdir");
    let known_hosts = tmp.path().join("known_hosts");
    // Pre-seed a known_hosts entry for this hostname with a fingerprint that
    // can never match the server's REAL (freshly self-signed) certificate —
    // simulating "the server's identity changed since we last trusted it"
    // (a MITM, or a legitimately re-keyed server the operator hasn't
    // consciously re-trusted).
    std::fs::write(
        &known_hosts,
        format!("{HOSTNAME} {BOGUS_FINGERPRINT_B64}\n"),
    )
    .expect("seed a bogus known_hosts entry");

    let mut client = new_client_app(server_addr, known_hosts.clone());
    // Short deadline: the abort happens during the TLS handshake itself
    // (`tofu_verifier_behaviour`'s `AbortConnection` arm returns an
    // `rustls::Error` from `verify_server_cert`), so this should resolve in
    // well under a second — generous headroom kept anyway for slow CI.
    let deadline = Instant::now() + Duration::from_secs(10);
    pump_until(
        &mut server,
        &mut client,
        deadline,
        "the mismatched-fingerprint connection to be aborted (reach Disconnected)",
        client_is_disconnected,
    );
    assert!(
        !client_is_connected(&client),
        "a mismatched fingerprint must NEVER be allowed to reach Connected (fail closed, not fail \
         open)"
    );

    // The bogus entry must not have been silently overwritten by a
    // TrustAndStore-style action — this project's verifier behaviour map
    // never does that for `UntrustedCertificate` (see `quinnet.rs`'s
    // `tofu_verifier_behaviour` doc comment).
    let known_hosts_after =
        std::fs::read_to_string(&known_hosts).expect("read known_hosts after the abort");
    assert!(
        known_hosts_after.contains(BOGUS_FINGERPRINT_B64),
        "the untrusted-path abort must leave the known_hosts file untouched, got: \
         {known_hosts_after:?}"
    );
}
