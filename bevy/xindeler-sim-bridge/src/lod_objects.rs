//! BL-82 EM-3.11-FH Phase C — mirrors the embedded local-player's ALREADY
//! STREAMED LOD-object zones (distant trees/structures, `common::lod::Zone`)
//! to the pure-Bevy client, following [`crate::send_far_terrain_once`]'s own
//! "`EmbeddedPlayer`-gated, `ClientState::Disconnected`-only" pattern, but
//! shaped like [`crate::CompressedChunk`]/[`crate::RemoveChunk`]'s
//! add/remove-over-the-session broadcast (NOT a one-shot latch) — zones
//! stream in and cull out as the player roams, exactly like chunks do.
//!
//! ## No new request/response plumbing (see also `xindeler_protocol::
//! lod_objects`'s module doc)
//! [`EmbeddedPlayer::lod_zones`] is already populated end-to-end by code this
//! task did NOT write: the embedded `client::Client`'s own `tick` requests
//! zones in a spiral around its position (throttled ~5 s,
//! `ClientGeneral::LodZoneRequest`) over the real TCP-loopback connection to
//! the embedded `server::Server`, whose `msg::terrain` system answers with
//! `ServerGeneral::LodZoneUpdate { key, zone: lod.zone(key).clone() }` —
//! `server::lod::Lod` precomputes EVERY zone for the whole world up front
//! (`Lod::from_world`, called once at boot, in parallel) from
//! `world::World::get_lod_zone` (real tree-placement + site-plot data, no new
//! worldgen). [`send_lod_zone_updates`] below is the ONLY new code this phase
//! needed on the server-shell side: read the already-populated
//! [`EmbeddedPlayer::lod_zones`] map every frame, diff it against what the
//! Bevy client has already been told about, and broadcast the delta.
//!
//! ## Object cap per zone (perf)
//! A dense forest zone can carry many thousands of trees (`get_lod_zone`
//! scans a whole 32×32-chunk area); [`LOD_ZONE_MAX_OBJECTS`] bounds the
//! broadcast (and therefore the client's per-zone mesh) with a DETERMINISTIC
//! stride-subsample (mirrors `xindeler-sim-bridge::lod_alt_grid_dims`'s own
//! "cap regardless of world size" discipline for the far-terrain grid) rather
//! than a random truncation, so the same zone always yields the same reduced
//! set.

use bevy::{
    app::{App, Plugin, Update},
    ecs::{
        change_detection::NonSend, message::MessageWriter, resource::Resource,
        schedule::IntoScheduleConfigs, system::ResMut,
    },
    state::condition::in_state,
};
use bevy_replicon::prelude::{ClientState, SendTargets, ToClients};
use hashbrown::HashSet;
use xindeler_protocol::{NetLodZone, NetLodZoneRemove};

use crate::EmbeddedPlayer;

/// Hard cap on objects broadcast per zone — bounds both the wire payload and
/// the client's per-zone mesh vertex count regardless of how dense a real
/// zone's tree placement turns out to be. Tuned from the live listen-server
/// smoke measurement (see the epic's PR description for the observed
/// per-zone object counts and resulting frame-time cost); raise only with a
/// fresh measurement backing it.
pub const LOD_ZONE_MAX_OBJECTS: usize = 800;

/// Tracks which zone keys the Bevy client has already been told about, so
/// [`send_lod_zone_updates`] only broadcasts the DELTA each frame (new zones
/// as [`NetLodZone`], zones that fell out of [`EmbeddedPlayer::lod_zones`] as
/// [`NetLodZoneRemove`]) rather than resending everything.
#[derive(Resource, Default)]
pub struct LodZoneStreamState {
    known: HashSet<[i32; 2]>,
}

/// Registers [`LodZoneStreamState`] + [`send_lod_zone_updates`]. Same gate as
/// [`crate::LodAltStreamPlugin`]/[`crate::MapDataStreamPlugin`]
/// (`ClientState::Disconnected`, i.e. this App is the terrain/LOD-data
/// SOURCE). Add AFTER [`crate::PlayerBridgePlugin`] (reads [`EmbeddedPlayer`]).
pub struct LodZoneStreamPlugin;

impl Plugin for LodZoneStreamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LodZoneStreamState>().add_systems(
            Update,
            send_lod_zone_updates.run_if(in_state(ClientState::Disconnected)),
        );
    }
}

/// Deterministic stride-subsample down to at most [`LOD_ZONE_MAX_OBJECTS`]
/// entries — pure so it's unit-testable without a real `Zone`. Kept as a
/// borrow-in/owned-out helper (not a filter closure) so the caller can encode
/// the result directly without an intermediate collect.
fn capped_objects(objects: &[common::lod::Object]) -> Vec<common::lod::Object> {
    if objects.len() <= LOD_ZONE_MAX_OBJECTS {
        return objects.to_vec();
    }
    let stride = objects.len().div_ceil(LOD_ZONE_MAX_OBJECTS);
    objects.iter().step_by(stride).cloned().collect()
}

/// Every frame, diffs [`EmbeddedPlayer::lod_zones`] (already streamed by the
/// embedded client's own request/cull logic, see module doc) against
/// [`LodZoneStreamState::known`] and broadcasts only the delta: newly-known
/// zones as [`NetLodZone`] (capped via [`capped_objects`]), zones that fell
/// out of range as [`NetLodZoneRemove`].
fn send_lod_zone_updates(
    player: Option<NonSend<EmbeddedPlayer>>,
    mut state: ResMut<LodZoneStreamState>,
    mut zone_writer: MessageWriter<ToClients<NetLodZone>>,
    mut remove_writer: MessageWriter<ToClients<NetLodZoneRemove>>,
) {
    let Some(player) = player else { return };
    let zones = player.lod_zones();
    if zones.is_empty() && state.known.is_empty() {
        return; // nothing streamed yet — the common case before in-game
    }

    let current: HashSet<[i32; 2]> = zones.keys().map(|key| [key.x, key.y]).collect();

    for (key, zone) in zones.iter() {
        let key_arr = [key.x, key.y];
        if state.known.contains(&key_arr) {
            continue;
        }
        let raw_count = zone.objects.len();
        let objects = capped_objects(&zone.objects);
        tracing::info!(
            ?key_arr,
            raw_count,
            sent_count = objects.len(),
            "EM-3.11-FH Phase C: broadcasting a new LOD-object zone"
        );
        zone_writer.write(ToClients {
            targets: SendTargets::All,
            message: NetLodZone::encode(key_arr, &objects),
        });
    }

    for key in &state.known {
        if !current.contains(key) {
            tracing::debug!(
                ?key,
                "EM-3.11-FH Phase C: LOD-object zone fell out of range"
            );
            remove_writer.write(ToClients {
                targets: SendTargets::All,
                message: NetLodZoneRemove { key: *key },
            });
        }
    }

    state.known = current;
}

#[cfg(test)]
mod tests {
    use common::lod::{InstFlags, Object, ObjectKind};
    use vek::{Rgb, Vec3};

    use super::*;

    fn dummy_object(seed: i16) -> Object {
        Object {
            kind: ObjectKind::Pine,
            pos: Vec3::new(seed, seed, seed),
            flags: InstFlags::empty(),
            color: Rgb::new(10, 80, 20),
        }
    }

    /// A zone under the cap is returned verbatim (no subsampling).
    #[test]
    fn capped_objects_keeps_small_zones_whole() {
        let objects: Vec<_> = (0..10).map(dummy_object).collect();
        let capped = capped_objects(&objects);
        assert_eq!(capped.len(), 10);
    }

    /// A zone over the cap is stride-subsampled down to AT MOST the cap, and
    /// the same input always yields the same output length (deterministic —
    /// no RNG truncation).
    #[test]
    fn capped_objects_bounds_large_zones_deterministically() {
        let objects: Vec<_> = (0..(LOD_ZONE_MAX_OBJECTS as i16 * 3))
            .map(dummy_object)
            .collect();
        let capped_a = capped_objects(&objects);
        let capped_b = capped_objects(&objects);
        assert!(capped_a.len() <= LOD_ZONE_MAX_OBJECTS);
        assert!(!capped_a.is_empty());
        assert_eq!(capped_a.len(), capped_b.len());
        for (a, b) in capped_a.iter().zip(capped_b.iter()) {
            assert_eq!(a.pos, b.pos);
        }
    }

    /// BL-82 EM-3.11-FH Phase C acceptance (server side): boot the REAL sim +
    /// embedded player, add the LOD-zone broadcast stack, and tick until the
    /// embedded player reaches in-game AND its own (pre-existing,
    /// unmigrated) zone-streaming logic has requested + received at least
    /// one real zone from the world — proving the WHOLE chain end to end
    /// (embedded client's spiral request -> embedded server's `lod.zone()`
    /// reply -> `EmbeddedPlayer::lod_zones()` -> [`send_lod_zone_updates`]'s
    /// mirror -> a real [`NetLodZone`] broadcast), not just the pure
    /// encode/decode/cap unit tests above.
    #[test]
    #[ignore = "boots a real world: needs assets + LFS; run locally with XINDELER_ASSETS"]
    fn lod_zone_broadcast_after_real_world_boot_and_spawn() {
        use bevy::{
            MinimalPlugins,
            app::PluginGroup,
            ecs::message::Messages,
            state::app::StatesPlugin,
            time::{Fixed, Time, TimeUpdateStrategy},
        };
        use bevy_replicon::prelude::{RepliconPlugins, ServerPlugin};
        use xindeler_protocol::XindelerProtocolPlugin;

        // Character creation + spawn takes longer than the far-terrain
        // broadcast's own 200-tick budget (that one only needs `world_data()`,
        // populated at handshake — this needs the player fully in-game AND a
        // real network round-trip for its own zone request), so this budget
        // is larger.
        const MAX_TICKS: u32 = 600;

        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut sim = crate::boot_test_server(data_dir.path()).expect("failed to boot test server");
        let player = crate::boot_embedded_player(&mut sim).expect("failed to boot embedded player");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build())
            .add_plugins(StatesPlugin)
            .add_plugins(RepliconPlugins.set(ServerPlugin::new(bevy::app::PostUpdate)))
            .add_plugins((
                XindelerProtocolPlugin,
                crate::SimBridgePlugin,
                crate::PlayerBridgePlugin,
                LodZoneStreamPlugin,
            ))
            .finish();
        app.insert_resource(Time::<Fixed>::from_hz(crate::SIM_TICK_HZ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / crate::SIM_TICK_HZ),
        ));
        app.insert_non_send(sim);
        app.insert_non_send(player);

        let mut received: Option<NetLodZone> = None;
        for _ in 0..MAX_TICKS {
            app.update();
            if let Some(msg) = app
                .world_mut()
                .resource_mut::<Messages<NetLodZone>>()
                .drain()
                .next()
            {
                received = Some(msg);
                break;
            }
        }

        let msg = received.expect(
            "at least one real LOD zone must be broadcast within MAX_TICKS once the embedded \
             player reaches in-game",
        );
        // Decodes cleanly (may legitimately be empty — a zone can be a bare
        // plain with zero trees/structures — but must not be corrupt).
        msg.decode()
            .expect("a real broadcast zone must always decode");
    }
}
