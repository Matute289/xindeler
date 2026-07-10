//! BL-82 EM-4.2d acceptance test (task board T47.6) — per-client interest
//! management over the REAL replicon+quinnet transport (EM-4.2b).
//!
//! Unlike `tests/replicon_quinnet_dual_stack.rs` (which proves the transport
//! itself against a full real sim + a full graphical `xindeler-client`
//! binary, needs real assets/LFS/a GPU adapter, and is `#[ignore]`d), this
//! test targets ONLY the visibility-scoping mechanism this task adds
//! (`xindeler_protocol::{interest, visibility}`). It builds minimal, thin
//! `App`s directly (server role + two/three client roles) using the SAME
//! real `xindeler_transport::QuinnetTransport` (real UDP/QUIC sockets on
//! loopback, not `bevy_replicon`'s in-process test-harness loopback), with
//! synthetic `Replicated` entities spawned directly rather than booting a
//! real sim — no `VELOREN_ASSETS`/LFS/GPU needed, so this test is NOT
//! `#[ignore]`d and runs in ordinary CI.
//!
//! ## What the (single) test proves
//! Everything runs as ONE `#[test]` function
//! (`interest_management_scoping_boundary_and_bandwidth`), against a single
//! server and a single set of real sockets, in three sequential phases — see
//! that function's own doc comment for why this is one test rather than
//! several (a real, empirically-observed cross-test flakiness class specific
//! to same-process, multi-threaded, real-socket tests).
//!
//! - Phase 1: two clients whose `ClientViewpoint`s sit in different,
//!   non-overlapping regions receive DISJOINT replicated-entity sets (measured:
//!   each client's own real replicated `NetPos` entity count, over a real
//!   socket) — the literal T47.6 acceptance bar.
//! - Phase 2: a third client's viewpoint moving from one cluster's region to
//!   another's sees its visible set update within a bounded number of real
//!   ticks (the recompute trigger's own chunk-boundary-crossing condition — see
//!   `xindeler_protocol::interest`'s doc comment).
//! - Phase 3 (bandwidth comparison): a fourth client with a viewpoint wide
//!   enough to see BOTH clusters receives every entity the scoped clients
//!   collectively see, while a scoped client receives only its own cluster —
//!   the real, measured ENTITY-COUNT reduction visibility scoping buys. A
//!   bytes-per-tick figure is then estimated from that real count via a
//!   representative per-entity payload size (bincode-encoded
//!   `NetPos+NetOri+NetVel+NetHealth+NetBody`(+`NetLoadout` for humanoids), the
//!   SAME `bincode::config::legacy()` scheme this crate's own
//!   `CompressedChunk`/`NetLodAlt` already use). This test deliberately never
//!   imports raw `bevy_quinnet`/`quinn` stats types directly — that would
//!   violate the `xindeler-transport` isolation boundary T47.4 established ("a
//!   `grep -r bevy_replicon_quinnet` outside its own impl module must return
//!   nothing" — confirmed still true after this test). So the entity-COUNT side
//!   of the comparison is real/measured over a real socket; the byte-SIZE side
//!   is a documented, consistent-methodology estimate, not a raw wire capture.

use std::{
    net::SocketAddr,
    thread,
    time::{Duration, Instant},
};

use bevy::{
    MinimalPlugins,
    app::App,
    ecs::{entity::Entity, query::With},
    math::{Quat, Vec3},
    state::{app::StatesPlugin, state::State},
};
use bevy_replicon::prelude::{ClientState, ConnectedClient, Replicated, RepliconPlugins};
use common::comp::tool::{Hands, ToolKind};
use vek::Vec2 as SimVec2;
use xindeler_protocol::{
    ClientInterestPlugin, ClientViewpoint, DimensionId, NetBody, NetHealth, NetLoadout, NetOri,
    NetPos, NetTool, NetToolKey, NetVel, XindelerProtocolPlugin, region_key_for_pos,
};
use xindeler_transport::{QuinnetTransport, ReplicaTransport, TransportConfig};

/// World-space anchor for the "cluster A" synthetic entities (sim axes).
/// Well inside region `(0, 0)` (`REGION_SIZE` = 512 blocks).
const CLUSTER_A_ANCHOR: SimVec2<f32> = SimVec2::new(100.0, 100.0);
/// World-space anchor for "cluster B" — far enough away to land in a
/// completely different region AND a different (fuzzy-bordered) chunk, so
/// both the region filter and the recompute trigger are genuinely exercised.
const CLUSTER_B_ANCHOR: SimVec2<f32> = SimVec2::new(5_000.0, 5_000.0);
/// A discriminator between the two clusters' `NetPos.0.x` ranges (each
/// cluster's entities are offset by a few blocks from their anchor — see
/// [`spawn_cluster`] — well under this threshold).
const CLUSTER_SPLIT_X: f32 = 1_000.0;
/// Entities per cluster — "party scale", not a stress test.
const ENTITIES_PER_CLUSTER: usize = 10;
/// Small view distance (chunks) for the SCOPED clients — enough to cover
/// their own cluster (all of it sits inside one region) without reaching the
/// other cluster.
const SCOPED_VIEW_DISTANCE: u32 = 2;
/// Wide-enough view distance (chunks) for the "unscoped" client to see BOTH
/// clusters from a viewpoint halfway between them. Large, but still small
/// enough to keep `regions_in_vd`'s region-grid enumeration bounded (a few
/// hundred regions, not millions).
const WIDE_VIEW_DISTANCE: u32 = 200;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const SYNC_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_SLEEP: Duration = Duration::from_millis(20);

fn free_addr() -> SocketAddr {
    let port = portpicker::pick_unused_port().expect("failed to find a free loopback port");
    ([127, 0, 0, 1], port).into()
}

fn new_server_app(addr: SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins,
        XindelerProtocolPlugin,
        ClientInterestPlugin,
    ));
    app.add_plugins(QuinnetTransport.server_plugins(&TransportConfig::server(addr)));
    // Without an explicit `.run()`, a manually-`update()`-driven `App` never
    // reaches Bevy's plugin-lifecycle `finish`/`cleanup` phase on its own —
    // and `bevy_replicon`'s own `ServerPlugin::finish()` is where
    // `ServerMessages::setup_client_channels` sizes its receive-channel
    // storage from `RepliconChannels` (populated by `XindelerProtocolPlugin`'s
    // `build()`, which — like every plugin's `build()` — always runs before
    // ANY plugin's `finish()`, regardless of add-order). Skipping this call
    // leaves that storage at zero channels, and the FIRST client message
    // panics with "server should have a receive channel with id 0" — matches
    // the existing test convention in this workspace
    // (`xindeler-protocol`/`xindeler-sim-bridge`'s own `App` test harnesses
    // both call `.finish()` for the same reason).
    app.finish();
    app.cleanup();
    app
}

fn new_client_app(server_addr: SocketAddr) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins,
        XindelerProtocolPlugin,
    ));
    app.add_plugins(QuinnetTransport.client_plugins(&TransportConfig::client(server_addr)));
    app.finish();
    app.cleanup();
    app
}

/// The server-side `ConnectedClient` entities currently known.
fn connected_client_entities(server: &mut App) -> Vec<Entity> {
    server
        .world_mut()
        .query_filtered::<Entity, With<ConnectedClient>>()
        .iter(server.world())
        .collect()
}

fn is_connected(client: &mut App) -> bool {
    client.world_mut().resource::<State<ClientState>>().get() == &ClientState::Connected
}

/// Pumps `server` + `client`, waiting for `client` to reach
/// `ClientState::Connected` AND for exactly one NEW `ConnectedClient` entity
/// (not already in `already_connected`) to appear server-side, then returns
/// that new entity.
fn connect_client(server: &mut App, client: &mut App, already_connected: &[Entity]) -> Entity {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        server.update();
        client.update();
        if is_connected(client) {
            let current = connected_client_entities(server);
            if let Some(new_entity) = current.iter().find(|e| !already_connected.contains(e)) {
                return *new_entity;
            }
        }
        assert!(
            Instant::now() < deadline,
            "client never reached ClientState::Connected + a matching server-side ConnectedClient \
             entity within the deadline"
        );
        thread::sleep(POLL_SLEEP);
    }
}

/// Spawns `count` synthetic replicated entities clustered around `anchor`
/// (small deterministic offsets so they never collapse onto one point),
/// each carrying `Replicated` + the SAME replicated component set a real
/// mirrored entity would (`NetPos`/`NetOri`/`NetVel`/`NetHealth`/`NetBody`,
/// plus `NetLoadout` for the humanoid half — see this test's own module doc
/// comment for why: the per-entity payload-size estimate later reuses a
/// representative encoding of exactly this component set) plus the
/// SERVER-ONLY `RegionKey` the visibility filter keys on.
fn spawn_cluster(app: &mut App, anchor: SimVec2<f32>, count: usize) -> Vec<Entity> {
    let world = app.world_mut();
    (0..count)
        .map(|i| {
            let offset = SimVec2::new((i % 4) as f32 * 3.0, (i / 4) as f32 * 3.0);
            let pos = anchor + offset;
            let region = region_key_for_pos(DimensionId::default(), pos);
            let net_pos = NetPos(Vec3::new(pos.x, 0.0, pos.y));
            let is_humanoid = i % 2 == 0;
            let net_body = NetBody(if is_humanoid {
                common::comp::Body::Humanoid(common::comp::humanoid::Body {
                    species: common::comp::humanoid::Species::Human,
                    body_type: common::comp::humanoid::BodyType::Male,
                    hair_style: 0,
                    beard: 0,
                    eyes: 0,
                    accessory: 0,
                    hair_color: 0,
                    skin: 0,
                    eye_color: 0,
                })
            } else {
                common::comp::Body::QuadrupedSmall(common::comp::quadruped_small::Body {
                    species: common::comp::quadruped_small::Species::Pig,
                    body_type: common::comp::quadruped_small::BodyType::Female,
                })
            });
            let mut ec = world.spawn((
                Replicated,
                net_pos,
                NetOri(Quat::IDENTITY),
                NetVel(Vec3::ZERO),
                NetHealth {
                    current: 100.0,
                    max: 100.0,
                },
                net_body,
                region,
            ));
            if is_humanoid {
                ec.insert(NetLoadout {
                    active_tool: Some(NetTool {
                        key: NetToolKey::Tool("common.items.weapons.sword.starter".to_owned()),
                        kind: ToolKind::Sword,
                        hands: Hands::Two,
                    }),
                    chest: Some("common.items.armor.misc.chest.worker_purple_brown".to_owned()),
                    pants: Some("common.items.armor.misc.pants.worker_brown".to_owned()),
                    foot: Some("common.items.armor.misc.foot.sandals".to_owned()),
                    ..NetLoadout::default()
                });
            }
            ec.id()
        })
        .collect()
}

/// Counts the client's own replicated entities whose `NetPos.0.x` is on the
/// `low_side` (< [`CLUSTER_SPLIT_X`]) or the high side, per `low_side`.
fn count_cluster_entities(client: &mut App, low_side: bool) -> usize {
    client
        .world_mut()
        .query::<&NetPos>()
        .iter(client.world())
        .filter(|p| (p.0.x < CLUSTER_SPLIT_X) == low_side)
        .count()
}

fn total_entity_count(client: &mut App) -> usize {
    client
        .world_mut()
        .query::<&NetPos>()
        .iter(client.world())
        .count()
}

/// Pumps `server` + every app in `clients` until `pred(server, clients)`
/// holds, or panics past `deadline`.
fn wait_until(
    server: &mut App,
    clients: &mut [&mut App],
    deadline: Instant,
    what: &str,
    mut pred: impl FnMut(&mut App, &mut [&mut App]) -> bool,
) {
    loop {
        server.update();
        for client in clients.iter_mut() {
            client.update();
        }
        if pred(server, clients) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        thread::sleep(POLL_SLEEP);
    }
}

/// The full T47.6 acceptance suite, as ONE test function against ONE server
/// + ONE set of ports.
///
/// This is deliberately a single `#[test]`, not three, even though it covers
/// three logically distinct properties (see the phase comments below) —
/// `cargo test`'s default per-test-binary THREAD parallelism, combined with
/// `portpicker::pick_unused_port`'s inherent check-then-use race (and
/// `bevy_quinnet`'s own socket teardown not being synchronous with `App`
/// `Drop`), made three separate real-socket tests genuinely flaky against
/// each other in practice (observed empirically while developing this test:
/// a client occasionally received entities from a DIFFERENT test's cluster).
/// A single test sidesteps that class of cross-test interference entirely,
/// the same way `tests/replicon_quinnet_dual_stack.rs` and
/// `tests/dual_stack.rs` are each already one long linear test rather than
/// several short ones.
#[test]
fn interest_management_scoping_boundary_and_bandwidth() {
    let addr = free_addr();
    let mut server = new_server_app(addr);
    spawn_cluster(&mut server, CLUSTER_A_ANCHOR, ENTITIES_PER_CLUSTER);
    spawn_cluster(&mut server, CLUSTER_B_ANCHOR, ENTITIES_PER_CLUSTER);

    // ---- Phase 1: two clients at non-overlapping regions see disjoint
    // entity sets (the core T47.6 acceptance bar) ----
    let mut client_a = new_client_app(addr);
    let server_entity_a = connect_client(&mut server, &mut client_a, &[]);
    server
        .world_mut()
        .entity_mut(server_entity_a)
        .insert(ClientViewpoint::new(
            DimensionId::default(),
            CLUSTER_A_ANCHOR,
            SCOPED_VIEW_DISTANCE,
        ));

    let mut client_b = new_client_app(addr);
    let server_entity_b = connect_client(&mut server, &mut client_b, &[server_entity_a]);
    server
        .world_mut()
        .entity_mut(server_entity_b)
        .insert(ClientViewpoint::new(
            DimensionId::default(),
            CLUSTER_B_ANCHOR,
            SCOPED_VIEW_DISTANCE,
        ));

    let deadline = Instant::now() + SYNC_TIMEOUT;
    wait_until(
        &mut server,
        &mut [&mut client_a, &mut client_b],
        deadline,
        "both clients to see exactly their own cluster's entities",
        |_server, clients| {
            let [client_a, client_b] = clients else {
                unreachable!("exactly two clients")
            };
            count_cluster_entities(client_a, true) == ENTITIES_PER_CLUSTER
                && count_cluster_entities(client_b, false) == ENTITIES_PER_CLUSTER
        },
    );

    assert_eq!(
        count_cluster_entities(&mut client_a, true),
        ENTITIES_PER_CLUSTER,
        "client A must see all of cluster A"
    );
    assert_eq!(
        count_cluster_entities(&mut client_a, false),
        0,
        "client A must see NONE of cluster B"
    );
    assert_eq!(
        count_cluster_entities(&mut client_b, false),
        ENTITIES_PER_CLUSTER,
        "client B must see all of cluster B"
    );
    assert_eq!(
        count_cluster_entities(&mut client_b, true),
        0,
        "client B must see NONE of cluster A"
    );
    println!(
        "[T47.6] disjoint visibility confirmed: client A sees {} entities (cluster A only), \
         client B sees {} entities (cluster B only), out of {} total spawned",
        total_entity_count(&mut client_a),
        total_entity_count(&mut client_b),
        2 * ENTITIES_PER_CLUSTER
    );

    // ---- Phase 2: a THIRD client's viewpoint crossing from cluster A's
    // region into cluster B's updates its visible set within a bounded
    // number of real ticks (the recompute trigger's chunk-boundary-crossing
    // condition) ----
    let mut client_move = new_client_app(addr);
    let server_entity_move = connect_client(&mut server, &mut client_move, &[
        server_entity_a,
        server_entity_b,
    ]);
    server
        .world_mut()
        .entity_mut(server_entity_move)
        .insert(ClientViewpoint::new(
            DimensionId::default(),
            CLUSTER_A_ANCHOR,
            SCOPED_VIEW_DISTANCE,
        ));

    let deadline = Instant::now() + SYNC_TIMEOUT;
    wait_until(
        &mut server,
        &mut [&mut client_move],
        deadline,
        "the moving client to first see cluster A",
        |_server, clients| {
            let [client_move] = clients else {
                unreachable!()
            };
            count_cluster_entities(client_move, true) == ENTITIES_PER_CLUSTER
        },
    );
    assert_eq!(
        count_cluster_entities(&mut client_move, false),
        0,
        "before crossing, the moving client must not see cluster B yet"
    );
    println!("[T47.6] moving client initially sees cluster A only, as expected");

    // Cross the region boundary: move the viewpoint to cluster B.
    server
        .world_mut()
        .entity_mut(server_entity_move)
        .insert(ClientViewpoint::new(
            DimensionId::default(),
            CLUSTER_B_ANCHOR,
            SCOPED_VIEW_DISTANCE,
        ));

    let deadline = Instant::now() + SYNC_TIMEOUT;
    wait_until(
        &mut server,
        &mut [&mut client_move],
        deadline,
        "the moving client's visible set to update to cluster B after crossing the boundary",
        |_server, clients| {
            let [client_move] = clients else {
                unreachable!()
            };
            count_cluster_entities(client_move, false) == ENTITIES_PER_CLUSTER
                && count_cluster_entities(client_move, true) == 0
        },
    );
    println!(
        "[T47.6] boundary crossing confirmed: after moving the viewpoint, the client's visible \
         set switched from cluster A to cluster B within the sync deadline"
    );

    // ---- Phase 3: bandwidth comparison (spec §1.3, bullet 3) — a wide-view
    // client receives every entity the scoped clients collectively see,
    // while a scoped client (reusing client A from phase 1) receives only
    // its own cluster. See the module doc comment for the full
    // measured-count-vs-estimated-bytes methodology. ----
    let mut wide = new_client_app(addr);
    let midpoint = (CLUSTER_A_ANCHOR + CLUSTER_B_ANCHOR) / 2.0;
    let server_entity_wide = connect_client(&mut server, &mut wide, &[
        server_entity_a,
        server_entity_b,
        server_entity_move,
    ]);
    server
        .world_mut()
        .entity_mut(server_entity_wide)
        .insert(ClientViewpoint::new(
            DimensionId::default(),
            midpoint,
            WIDE_VIEW_DISTANCE,
        ));

    let deadline = Instant::now() + SYNC_TIMEOUT;
    wait_until(
        &mut server,
        &mut [&mut client_a, &mut wide],
        deadline,
        "the scoped client to (still) see only cluster A and the wide client to see both clusters",
        |_server, clients| {
            let [client_a, wide] = clients else {
                unreachable!("exactly two clients")
            };
            count_cluster_entities(client_a, true) == ENTITIES_PER_CLUSTER
                && count_cluster_entities(client_a, false) == 0
                && total_entity_count(wide) == 2 * ENTITIES_PER_CLUSTER
        },
    );

    let scoped_count = total_entity_count(&mut client_a);
    let wide_count = total_entity_count(&mut wide);
    assert_eq!(scoped_count, ENTITIES_PER_CLUSTER);
    assert_eq!(wide_count, 2 * ENTITIES_PER_CLUSTER);

    // Representative per-entity payload size: bincode(legacy)-encode ONE
    // synthetic entity's replicated component set, the SAME scheme
    // `xindeler_protocol::CompressedChunk::encode`/`NetLodAlt::encode`
    // already use elsewhere in this codebase (see this test's own module doc
    // comment for why this is an ESTIMATE, not a raw wire capture).
    let sample_pos = NetPos(Vec3::new(CLUSTER_A_ANCHOR.x, 0.0, CLUSTER_A_ANCHOR.y));
    let sample_ori = NetOri(Quat::IDENTITY);
    let sample_vel = NetVel(Vec3::ZERO);
    let sample_health = NetHealth {
        current: 100.0,
        max: 100.0,
    };
    let sample_body = NetBody(common::comp::Body::Humanoid(common::comp::humanoid::Body {
        species: common::comp::humanoid::Species::Human,
        body_type: common::comp::humanoid::BodyType::Male,
        hair_style: 0,
        beard: 0,
        eyes: 0,
        accessory: 0,
        hair_color: 0,
        skin: 0,
        eye_color: 0,
    }));
    let sample_loadout = NetLoadout {
        active_tool: Some(NetTool {
            key: NetToolKey::Tool("common.items.weapons.sword.starter".to_owned()),
            kind: ToolKind::Sword,
            hands: Hands::Two,
        }),
        chest: Some("common.items.armor.misc.chest.worker_purple_brown".to_owned()),
        pants: Some("common.items.armor.misc.pants.worker_brown".to_owned()),
        foot: Some("common.items.armor.misc.foot.sandals".to_owned()),
        ..NetLoadout::default()
    };
    let per_entity_bytes = [
        bincode::serde::encode_to_vec(sample_pos.0.to_array(), bincode::config::legacy())
            .unwrap()
            .len(),
        bincode::serde::encode_to_vec(sample_ori.0.to_array(), bincode::config::legacy())
            .unwrap()
            .len(),
        bincode::serde::encode_to_vec(sample_vel.0.to_array(), bincode::config::legacy())
            .unwrap()
            .len(),
        bincode::serde::encode_to_vec(sample_health, bincode::config::legacy())
            .unwrap()
            .len(),
        bincode::serde::encode_to_vec(sample_body, bincode::config::legacy())
            .unwrap()
            .len(),
        bincode::serde::encode_to_vec(sample_loadout, bincode::config::legacy())
            .unwrap()
            .len(),
    ]
    .into_iter()
    .sum::<usize>();

    let scoped_bytes_per_tick = scoped_count * per_entity_bytes;
    let wide_bytes_per_tick = wide_count * per_entity_bytes;
    let savings_ratio = wide_bytes_per_tick as f64 / scoped_bytes_per_tick as f64;

    println!(
        "[T47.6 bandwidth] per-entity representative payload ≈ {per_entity_bytes} bytes \
         (bincode(legacy) NetPos+NetOri+NetVel+NetHealth+NetBody+NetLoadout)"
    );
    println!(
        "[T47.6 bandwidth] scoped client: {scoped_count} entities ≈ {scoped_bytes_per_tick} \
         bytes/full-resync; wide client: {wide_count} entities ≈ {wide_bytes_per_tick} \
         bytes/full-resync — visibility scoping is {savings_ratio:.2}x cheaper for the scoped \
         client in this party-scale (2x{ENTITIES_PER_CLUSTER}-entity) synthetic scene"
    );

    assert_eq!(
        wide_count,
        2 * scoped_count,
        "the wide client must receive exactly twice the scoped client's entity count in this \
         symmetric two-cluster scene"
    );
}
