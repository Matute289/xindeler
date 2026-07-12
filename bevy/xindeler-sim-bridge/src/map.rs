//! BL-82 EM-5.5 — the map mirror: the ONE-SHOT `NetMapData` broadcast (world
//! map background + site/quest markers + named terrain features), following
//! [`crate::send_far_terrain_once`]'s exact "sent latch, `EmbeddedPlayer`-
//! gated, `ClientState::Disconnected`-only" pattern (see that function's own
//! doc comment — this module mirrors its shape verbatim, just for a
//! different payload). Kept as its OWN file (per the isolation-minimizing
//! convention several sibling Phase-5 epics are following tonight) rather
//! than folded into `lib.rs`'s already-large `send_far_terrain_once`.
//!
//! ## Scope: listen-server / singleplayer only (inherited limitation)
//! Like [`crate::LodAltStreamPlugin`], this whole mechanism depends on a real
//! [`EmbeddedPlayer`] (the embedded loopback `client::Client` that only
//! exists in listen-server mode) for its source data — `xindeler-server-app`
//! (the genuinely-remote dedicated server) never boots one, so it never adds
//! [`MapDataStreamPlugin`] either, and a `net-client` (spectator) connecting
//! to a real dedicated server today receives no map data at all (its
//! `xindeler_client::map_view::MapViewPlugin` degrades clean — spec §3.2 —
//! rendering empty map screens, exactly like the far-terrain mesh already
//! does in that mode). Not a regression this task introduces; flagged so a
//! future reader doesn't assume this "just works" once real multiplayer is
//! exercised (bevy-migration-reviewer finding).
//!
//! ## Source data
//! Both halves come off the SAME [`EmbeddedPlayer`] the far-terrain broadcast
//! already reads: [`EmbeddedPlayer::world_data`]'s baked `map_image()` (a
//! `client::WorldData` field populated at the embedded `Client`'s initial
//! handshake, well before [`EmbeddedPlayer::is_in_game`]) for the visual
//! background, and the NEW [`EmbeddedPlayer::markers`]/[`EmbeddedPlayer::
//! pois`] accessors (this task's own additions to `player.rs`) for the site/
//! POI lists. No new sim-side plumbing: `client::Client` already computed and
//! exposed all of this during its handshake (`WorldMapMsg`, EM-5.5's research
//! confirmed — see the spec's §research note), this module only downsamples
//! + projects it onto the wire shape.
//!
//! ## Why downsample independently of `NetFarTerrain`'s `LOD_ALT_MAX_DIM`
//! `client::WorldData::map_image()` is ALREADY one pixel per world chunk (a
//! default Veloren world is up to 1024×1024 chunks) — a full-resolution copy
//! would be a needlessly large one-shot payload, exactly the concern
//! `NetFarTerrain`'s own downsample already addresses for the height/colour
//! layers. This is a SEPARATE cap ([`MAP_IMAGE_MAX_DIM`]) because the map
//! background is a different visual product (an already-shaded/coloured
//! bitmap, not a raw height/colour grid the far-mesh shader recombines) with
//! its own acceptable resolution trade-off — sharing `NetFarTerrain`'s
//! constant would silently couple two independent concerns.

use bevy::{
    app::{App, Plugin, Update},
    ecs::{
        change_detection::NonSend, message::MessageWriter, resource::Resource,
        schedule::IntoScheduleConfigs, system::ResMut,
    },
    state::condition::in_state,
};
use bevy_replicon::prelude::{ClientState, SendTargets, ToClients};
use common::{map::MarkerFlags, terrain::TerrainChunkSize, vol::RectVolSize};
use xindeler_protocol::map::{NetMapData, NetMapMarker, NetMapPoi, NetPoiKind};

use crate::EmbeddedPlayer;

/// Downsample cap: at most this many samples per axis in the background
/// image, regardless of world size — mirrors `crate::LOD_ALT_MAX_DIM`'s
/// reasoning but is an independent constant (see module doc comment).
const MAP_IMAGE_MAX_DIM: u32 = 256;

/// One-shot latch for the EM-5.5 `NetMapData` broadcast.
#[derive(Resource, Default)]
pub struct MapDataState {
    sent: bool,
}

/// Registers [`MapDataState`] + [`send_map_data_once`]. Same gate as
/// [`crate::LodAltStreamPlugin`] (`ClientState::Disconnected`, i.e. this App
/// is the terrain/map-data SOURCE). Add AFTER [`crate::PlayerBridgePlugin`]
/// (reads [`EmbeddedPlayer`]) — matching that plugin's own registration
/// convention exactly.
pub struct MapDataStreamPlugin;

impl Plugin for MapDataStreamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapDataState>().add_systems(
            Update,
            send_map_data_once.run_if(in_state(ClientState::Disconnected)),
        );
    }
}

/// Pure stride/grid-dimension math for the background-image downsample —
/// the SAME shape `crate::lod_alt_grid_dims` uses (kept independent per the
/// module doc comment's "no shared constant/helper" rationale), so it is
/// unit-testable without a real `WorldData`.
fn map_image_grid_dims(width: u32, height: u32) -> (u32, u32, u32) {
    let stride = width.max(height).div_ceil(MAP_IMAGE_MAX_DIM).max(1);
    let out_w = width.div_ceil(stride).max(1);
    let out_h = height.div_ceil(stride).max(1);
    (stride, out_w, out_h)
}

/// Broadcasts the downsampled map background + site/POI markers ONCE, as
/// soon as the embedded local-player [`EmbeddedPlayer`] exists (mirrors
/// [`crate::send_far_terrain_once`]'s own early-out/no-embedded-player/
/// zero-size guards verbatim — see that function's doc comment for why each
/// guard exists; not repeated here).
fn send_map_data_once(
    player: Option<NonSend<EmbeddedPlayer>>,
    mut state: ResMut<MapDataState>,
    mut writer: MessageWriter<ToClients<NetMapData>>,
) {
    if state.sent {
        return;
    }
    let Some(player) = player else { return };

    let world_data = player.world_data();
    let chunk_size = world_data.chunk_size(); // Vec2<u16>, chunk-grid dimensions
    if chunk_size.x == 0 || chunk_size.y == 0 {
        return; // not populated yet (shouldn't happen once the Client exists)
    }

    let map_image = world_data.map_image();
    let (img_w, img_h) = (map_image.width(), map_image.height());
    if img_w == 0 || img_h == 0 {
        return;
    }
    let rgb_image = map_image.to_rgb8();

    let (stride, out_w, out_h) = map_image_grid_dims(img_w, img_h);
    let mut pixels: Vec<[u8; 3]> = Vec::with_capacity((out_w * out_h) as usize);
    for j in 0..out_h {
        let y = (j * stride).min(img_h - 1);
        for i in 0..out_w {
            let x = (i * stride).min(img_w - 1);
            let p = rgb_image.get_pixel(x, y);
            pixels.push([p.0[0], p.0[1], p.0[2]]);
        }
    }

    let markers: Vec<NetMapMarker> = player
        .markers()
        .map(|marker| NetMapMarker {
            kind: marker.kind.clone(),
            wpos: [marker.wpos.x, marker.wpos.y],
            label: marker
                .label
                .as_ref()
                .and_then(|content| content.as_plain())
                .map(str::to_owned),
            is_quest: marker.flags.contains(MarkerFlags::IS_QUEST),
        })
        .collect();

    let pois: Vec<NetMapPoi> = player
        .pois()
        .iter()
        .map(|poi| NetMapPoi {
            name: poi.name.clone(),
            wpos: [poi.wpos.x as f32, poi.wpos.y as f32],
            kind: match poi.kind {
                common_net::msg::world_msg::PoiKind::Peak(_) => NetPoiKind::Peak,
                common_net::msg::world_msg::PoiKind::Lake(_) => NetPoiKind::Lake,
            },
        })
        .collect();

    let marker_count = markers.len();
    let poi_count = pois.len();

    writer.write(ToClients {
        targets: SendTargets::All,
        message: NetMapData::encode(
            [u32::from(chunk_size.x), u32::from(chunk_size.y)],
            TerrainChunkSize::RECT_SIZE.x,
            [out_w, out_h],
            &pixels,
            markers,
            pois,
        ),
    });
    state.sent = true;
    tracing::info!(
        out_w,
        out_h,
        stride,
        marker_count,
        poi_count,
        "map data broadcast (BL-82 EM-5.5)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world already at/below the cap samples 1:1 (stride 1, no shrink).
    #[test]
    fn map_image_grid_dims_keeps_small_worlds_at_full_resolution() {
        let (stride, w, h) = map_image_grid_dims(64, 32);
        assert_eq!(stride, 1);
        assert_eq!(w, 64);
        assert_eq!(h, 32);
    }

    /// A world above the cap downsamples so neither output axis exceeds it.
    #[test]
    fn map_image_grid_dims_caps_large_worlds() {
        let (stride, w, h) = map_image_grid_dims(1024, 1024);
        assert!(stride > 1);
        assert!(w <= MAP_IMAGE_MAX_DIM);
        assert!(h <= MAP_IMAGE_MAX_DIM);
    }

    /// A degenerate zero-sized axis never divides by zero (mirrors
    /// `lod_alt_grid_dims`'s own degenerate-input test).
    #[test]
    fn map_image_grid_dims_never_zero() {
        let (stride, w, h) = map_image_grid_dims(0, 0);
        assert!(stride >= 1);
        assert!(w >= 1);
        assert!(h >= 1);
    }
}
