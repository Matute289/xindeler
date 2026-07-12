//! BL-82 EM-5.5 — the map mirror: one-shot broadcast of the world's coarse
//! visual background + site/quest markers + named terrain features (peaks/
//! lakes), for the minimap + full-map HUD screens.
//!
//! ## Pattern: follows [`crate::NetFarTerrain`] exactly
//! Like the far-terrain grid, the map's background image and site list never
//! change during a session (sites are placed at worldgen time, same as the
//! coarse LOD height/colour layers `NetFarTerrain` already sends once) — so
//! this is a ONE-SHOT broadcast (`xindeler-sim-bridge::map::send_map_data_once`
//! mirrors `send_far_terrain_once`'s own "sent latch, `ClientState::
//! Disconnected`-gated" shape verbatim), not a per-tick replicated component.
//! The pixel payload reuses the SAME lz4+bincode compression scheme
//! [`crate::NetFarTerrain`] uses for its layers (`bincode::config::legacy()` +
//! `lz_fear`) — this module keeps its own small `compress`/`decompress` pair
//! rather than factoring a shared helper out of `NetFarTerrain`, deliberately:
//! Phase 5 has multiple sibling agents editing this same file tonight (EM-5.4
//! chat, EM-5.6 inventory, EM-5.8 social, EM-5.11 input), so a refactor
//! touching `NetFarTerrain`'s existing private methods is exactly the kind of
//! shared-code edit that maximizes merge-conflict blast radius for no
//! behavioural gain; a few duplicated private lines in a NEW module cost
//! nothing and touch nothing anyone else is editing.
//!
//! ## Project, don't dump (spec §3.2)
//! The sim's real [`common::map::Marker`] carries an `id` (dedup hash) and a
//! `site: Option<SiteId>` link the map HUD never needs (per-site economy is a
//! separate, on-demand `EconomyInfo` fetch — EM-5.6/5.15's job, not the map's);
//! [`NetMapMarker`] keeps only `kind`/`wpos`/a plain resolved `label`/the quest
//! flag. `common::map::PoiInfo`'s `kind: PoiKind` (which itself carries a
//! `u32` elevation/size payload the map HUD only needs the coarse Peak-vs-Lake
//! distinction from) becomes the plain [`NetPoiKind`] enum in [`NetMapPoi`].

use bevy::{ecs::message::Message, math::Vec2 as BevyVec2};
use serde::{Deserialize, Serialize};

/// A site/quest marker on the map (projected [`common::map::Marker`] — kind,
/// world position in BLOCKS, a plain resolved label, and the quest flag).
///
/// `label` is `None` whenever the sim's `Marker.label` is either unset or an
/// i18n [`common_i18n::Content`] key that isn't a plain literal string
/// (`Content::as_plain()` returns `None` for those) — the map screen falls
/// back to a name derived from `kind` client-side in that case (matches the
/// legacy HUD's own per-`MarkerKind` icon convention, spec §1.2). Full i18n
/// key resolution is EM-5.16's job (the depth epic for full multi-language
/// coverage), not this one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetMapMarker {
    pub kind: common::map::MarkerKind,
    /// World position, sim axes (x-east, y-north), in BLOCKS.
    pub wpos: [f32; 2],
    pub label: Option<String>,
    pub is_quest: bool,
}

/// The coarse terrain-feature kind a [`NetMapPoi`] names — collapses
/// `common::map::PoiKind`'s `Peak(u32)`/`Lake(u32)` payload (elevation/size in
/// world units, a worldgen-internal magnitude the map HUD's text label never
/// needs) down to which of the two it is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetPoiKind {
    Peak,
    Lake,
}

/// A named terrain feature (projected [`common::map::PoiInfo`] — a mountain
/// peak or lake the world map labels by name, no icon).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetMapPoi {
    pub name: String,
    /// World position, sim axes (x-east, y-north), in BLOCKS.
    pub wpos: [f32; 2],
    pub kind: NetPoiKind,
}

/// Server → client: the one-shot world map broadcast (BL-82 EM-5.5). Sent
/// ONCE per session — like [`crate::NetFarTerrain`] — since the background
/// image and site/POI lists never change while a world is running.
///
/// ## Background image
/// [`Self::image_size`] is a DOWNSAMPLED copy of `client::WorldData::
/// map_image()` (the client-core's own baked map texture — see
/// `xindeler-sim-bridge::map`'s module doc comment for the downsample
/// rationale), independent of [`crate::NetFarTerrain`]'s own
/// `LOD_ALT_MAX_DIM` cap — this is the coarse VISUAL background for the map
/// screens, not the far-mesh height layer. [`Self::world_size_chunks`] is the
/// ORIGINAL (pre-downsample) world size in chunks — carrying both lets a
/// client convert any sim-space `wpos` (blocks) into a background-image pixel
/// via [`wpos_to_screen_uv`] without needing to know the downsample stride
/// itself (a straight `image_size / world_size_chunks` ratio).
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetMapData {
    /// World size, in chunks (matches [`crate::NetFarTerrain::grid_size`]'s
    /// axis convention: `[x, y]`, pre-downsample).
    pub world_size_chunks: [u32; 2],
    /// Chunk edge length, in blocks (`common::terrain::TerrainChunkSize::
    /// RECT_SIZE`, always square in this codebase) — needed to convert a
    /// marker's `wpos` (blocks) into a chunk coordinate before the
    /// `world_size_chunks` ratio applies.
    pub chunk_size_blocks: u32,
    /// Downsampled background-image dimensions (row-major storage below).
    pub image_size: [u32; 2],
    /// lz4-compressed bincode of the row-major `Vec<[u8; 3]>` RGB pixels,
    /// `image_size[0] * image_size[1]` long. Decode with
    /// [`Self::decode_image`].
    pub image_rgb: Vec<u8>,
    pub markers: Vec<NetMapMarker>,
    pub pois: Vec<NetMapPoi>,
}

impl NetMapData {
    /// Serializes (bincode `legacy()`) + compresses (lz4) a downsampled
    /// background image alongside the already-projected marker/POI lists —
    /// same compression scheme [`crate::NetFarTerrain::encode`] uses for its
    /// layers (kept as an independent copy here, see this module's doc
    /// comment for why).
    #[must_use]
    pub fn encode(
        world_size_chunks: [u32; 2],
        chunk_size_blocks: u32,
        image_size: [u32; 2],
        image_rgb: &[[u8; 3]],
        markers: Vec<NetMapMarker>,
        pois: Vec<NetMapPoi>,
    ) -> Self {
        debug_assert_eq!(
            image_rgb.len(),
            (image_size[0] as usize) * (image_size[1] as usize),
            "image_rgb must be exactly image_size[0]*image_size[1] long"
        );
        Self {
            world_size_chunks,
            chunk_size_blocks,
            image_size,
            image_rgb: compress(image_rgb),
            markers,
            pois,
        }
    }

    /// Decompresses + deserializes the background image. `None` = corrupt
    /// payload or a length mismatch against [`Self::image_size`] (defensive —
    /// the local loopback can't corrupt, but a future real transport could).
    #[must_use]
    pub fn decode_image(&self) -> Option<Vec<[u8; 3]>> {
        let pixels: Vec<[u8; 3]> = decompress(&self.image_rgb)?;
        let expected = self.image_size[0] as usize * self.image_size[1] as usize;
        (pixels.len() == expected).then_some(pixels)
    }
}

/// Serializes + compresses a slice with the shared lz4+bincode scheme
/// [`crate::NetFarTerrain`]'s own private `compress` uses.
fn compress<T: Serialize>(items: &[T]) -> Vec<u8> {
    let raw = bincode::serde::encode_to_vec(items, bincode::config::legacy())
        .expect("bincode serialization can only fail if a byte limit is set");
    let mut bytes = Vec::with_capacity(raw.len() / 4 + 16);
    let mut table = lz_fear::raw::U32Table::default();
    lz_fear::raw::compress2(&raw, 0, &mut table, &mut bytes)
        .expect("lz4 compression into a Vec<u8> is infallible");
    bytes
}

/// Decompresses + deserializes a compressed blob. `None` for an empty blob or
/// a corrupt payload.
fn decompress<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<Vec<T>> {
    if bytes.is_empty() {
        return None;
    }
    let mut raw = Vec::with_capacity(bytes.len() * 2);
    lz_fear::raw::decompress_raw(bytes, &[0; 0], &mut raw, usize::MAX).ok()?;
    bincode::serde::decode_from_slice(&raw, bincode::config::legacy())
        .ok()
        .map(|(items, _)| items)
}

/// Converts a sim-space world position (`wpos`, in BLOCKS, x-east/y-north)
/// into a SCREEN-oriented UV fraction (`u`, `v` both in `[0, 1]`, origin
/// TOP-LEFT — Bevy UI's own convention) into [`NetMapData`]'s background
/// image.
///
/// The `v` axis is FLIPPED relative to the raw chunk-row math
/// (`client::sample_pos`'s own `posi = pos.y * map_size.x + pos.x` indexing,
/// which the server-side downsample in `xindeler-sim-bridge::map` preserves):
/// since sim `y` increases NORTHWARD but screen `v` increases DOWNWARD, a
/// larger `wpos.y` (further north) must map to a SMALLER `v` (closer to the
/// top of the screen) for the map to read north-up — the conventional map
/// orientation the legacy HUD also uses (spec §1.2, `map.rs`'s world-map
/// image). A zero/negative `world_size_chunks` or `chunk_size_blocks`
/// degrades to a clamped `[0, 1]` fraction rather than NaN/inf (spec §3.2's
/// "degrade clean" rule, applied to map math too — mirrors
/// `xindeler_ui::bar::BarValue::fraction`'s own zero-max guard).
#[must_use]
pub fn wpos_to_screen_uv(
    wpos: BevyVec2,
    world_size_chunks: [u32; 2],
    chunk_size_blocks: u32,
) -> BevyVec2 {
    let chunk_size = if chunk_size_blocks == 0 {
        1.0
    } else {
        chunk_size_blocks as f32
    };
    let world_w = (world_size_chunks[0].max(1)) as f32;
    let world_h = (world_size_chunks[1].max(1)) as f32;

    let u = ((wpos.x / chunk_size) / world_w).clamp(0.0, 1.0);
    let v_world = ((wpos.y / chunk_size) / world_h).clamp(0.0, 1.0);
    BevyVec2::new(u, 1.0 - v_world)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`NetMapData::encode`]/[`NetMapData::decode_image`] round-trip a
    /// downsampled RGB image exactly.
    #[test]
    fn map_data_encode_decode_round_trips_the_image() {
        let pixels: Vec<[u8; 3]> = (0..12u16)
            .map(|i| [i as u8, (i * 2) as u8, (i * 3) as u8])
            .collect();
        let data = NetMapData::encode(
            [64, 64],
            32,
            [4, 3],
            &pixels,
            vec![NetMapMarker {
                kind: common::map::MarkerKind::Town,
                wpos: [100.0, 200.0],
                label: Some("Bramblewick".to_owned()),
                is_quest: false,
            }],
            vec![NetMapPoi {
                name: "Mount Ashfall".to_owned(),
                wpos: [500.0, 600.0],
                kind: NetPoiKind::Peak,
            }],
        );

        let decoded = data.decode_image().expect("a well-formed payload decodes");
        assert_eq!(decoded, pixels);
        assert_eq!(data.markers[0].label.as_deref(), Some("Bramblewick"));
        assert_eq!(data.pois[0].kind, NetPoiKind::Peak);
    }

    /// A corrupt/truncated payload decodes to `None` rather than panicking —
    /// mirrors `NetFarTerrain::decode_heights`'s own defensive contract.
    #[test]
    fn decode_image_returns_none_for_a_length_mismatch() {
        let mut data = NetMapData::encode(
            [4, 4],
            32,
            [2, 2],
            &[[0, 0, 0], [1, 1, 1], [2, 2, 2], [3, 3, 3]],
            Vec::new(),
            Vec::new(),
        );
        // Corrupt the declared size so the decoded length no longer matches.
        data.image_size = [3, 3];
        assert!(data.decode_image().is_none());
    }

    /// North (larger `wpos.y`) maps to a SMALLER `v` (top of screen); east
    /// (larger `wpos.x`) maps to a larger `u` (right of screen) — the
    /// north-up orientation contract [`wpos_to_screen_uv`]'s doc comment
    /// promises.
    #[test]
    fn wpos_to_screen_uv_is_north_up_and_east_right() {
        let world = [64, 64];
        let chunk = 32;

        let origin = wpos_to_screen_uv(BevyVec2::new(0.0, 0.0), world, chunk);
        assert!((origin.x - 0.0).abs() < 1e-6);
        assert!(
            (origin.y - 1.0).abs() < 1e-6,
            "wpos (0,0) is the SOUTH edge -> bottom of screen"
        );

        let far_corner = wpos_to_screen_uv(BevyVec2::new(64.0 * 32.0, 64.0 * 32.0), world, chunk);
        assert!((far_corner.x - 1.0).abs() < 1e-6);
        assert!(
            (far_corner.y - 0.0).abs() < 1e-6,
            "the far-north-east corner is the top-right of the screen"
        );

        let center = wpos_to_screen_uv(BevyVec2::new(32.0 * 32.0, 32.0 * 32.0), world, chunk);
        assert!((center.x - 0.5).abs() < 1e-6);
        assert!((center.y - 0.5).abs() < 1e-6);
    }

    /// A zero `world_size_chunks`/`chunk_size_blocks` degrades to a clamped
    /// fraction, never NaN/inf — the "degrade clean" rule applied to map math.
    #[test]
    fn wpos_to_screen_uv_degrades_clean_on_zero_inputs() {
        let uv = wpos_to_screen_uv(BevyVec2::new(10.0, 10.0), [0, 0], 0);
        assert!(uv.x.is_finite());
        assert!(uv.y.is_finite());
        assert!((0.0..=1.0).contains(&uv.x));
        assert!((0.0..=1.0).contains(&uv.y));
    }
}
