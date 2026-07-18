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

/// Hard cap on the DECOMPRESSED size (bytes) [`NetMapData::decode_image`] will
/// accept, passed as `lz_fear::raw::decompress_raw`'s `output_limit` (BL-82
/// EM-8.5 — closes the decompression-bomb gap this module's `decompress`
/// previously left open with `usize::MAX`, same hardening
/// [`crate::lod_objects::MAX_DECOMPRESSED_ZONE_BYTES`] already applied to its
/// own sibling call site; see that constant's doc comment for the full
/// "why this matters" background on LZ4's back-reference amplification).
///
/// ## Derivation (a real bound, not a guess)
/// The ONLY thing this module ever passes through [`decompress`] is
/// [`NetMapData::image_rgb`] — `markers`/`pois` are plain (uncompressed)
/// message fields. The server-side producer
/// (`xindeler_sim_bridge::map::send_map_data_once`) downsamples the
/// background image to at most [`MAP_IMAGE_MAX_DIM`] samples per axis (see
/// that constant's own doc comment — `1024`, chosen and bandwidth-measured
/// for exactly this reason), so the real maximum pre-compression payload is
/// `MAP_IMAGE_MAX_DIM * MAP_IMAGE_MAX_DIM * 3` bytes (one `[u8; 3]` RGB pixel
/// per sample) = `1024 * 1024 * 3` = 3,145,728 bytes (~3 MiB), plus a few
/// bytes of bincode `Vec` length-prefix overhead. This cap is set to 16 MiB —
/// a little over 5x that real maximum, comfortably clearing bincode/framing
/// overhead and any future small bump to [`MAP_IMAGE_MAX_DIM`] without ever
/// approaching "unbounded."
const MAX_DECOMPRESSED_MAP_IMAGE_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Downsample cap for [`NetMapData::image_size`]: at most this many samples
/// per axis, regardless of world size. Lives HERE (shared between
/// `xindeler-sim-bridge::map::send_map_data_once`, which downsamples to it,
/// and `xindeler_client::map_view`'s minimap crop math, which consumes the
/// result) instead of as an independent constant in each crate — the BL-82
/// Phase 5 follow-up root cause below is exactly that the two were
/// previously uncorrelated.
///
/// ## Root cause of the "pixelated/blurry minimap" bug (BL-82 Phase 5
/// follow-up, found via a real play session)
/// The sampler was already correct (`xindeler_client::map_view::
/// receive_map_data` sets `ImageFilterMode::Linear` on the decoded texture,
/// not the project's usual voxel-texture `Nearest` default) — ruled out
/// first, per the investigation's own instruction to check the sampler
/// before touching resolution. The REAL cause: the always-on minimap crops a
/// `xindeler_client::map_view::MINIMAP_HALF_EXTENT`-wide UV window (~12% of
/// the world) into a 160px on-screen viewport, but the previous cap here
/// (256px) was chosen independently, matching `NetFarTerrain`'s height-layer
/// budget rather than this specific crop. For the shipped default world
/// (1024x1024 chunks — `world::sim::MapSizeLg::new(10, 10)`,
/// `world::sim::DEFAULT_WORLD_MAP`), that crop covered only ~31 SOURCE
/// pixels, magnified ~5.2x onto the viewport — real under-resolution no
/// amount of correct bilinear filtering can hide, since linear filtering
/// interpolates BETWEEN existing texels, it can't invent detail the
/// downsample already discarded.
///
/// `1024` (instead of an even larger cap) is deliberately chosen to land
/// close to 1:1 for THIS crop on the shipped default world (~123 source px
/// into the 160px viewport, ~1.3x) while remaining an independent, bounded
/// cap rather than "always full per-chunk resolution regardless of world
/// size" (a future larger world still downsamples). Bandwidth cost measured
/// (not guessed) via `bandwidth_measurement::compressed_size_at_the_shipped_
/// cap_stays_small` on a synthetic biome-like image (large contiguous colour
/// regions with per-pixel dither — the realistic case, not uniform noise,
/// which would over-estimate the real cost): the previous 256 cap compressed
/// to ~22.9 KB, 512 to ~49.8 KB, and this 1024 cap to ~173.6 KB. This is a
/// ONE-SHOT payload (sent once per session, identical shape to
/// `NetFarTerrain`'s own one-shot layers) — a ~151 KB increase is negligible
/// next to the terrain streaming that already happens on connect.
pub const MAP_IMAGE_MAX_DIM: u32 = 1024;

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

/// Decompresses + deserializes a compressed blob. `None` for an empty blob, a
/// corrupt payload, OR one that claims to decompress past
/// [`MAX_DECOMPRESSED_MAP_IMAGE_BYTES`] (decompression-bomb hardening,
/// BL-82 EM-8.5 — see that constant's doc comment) — callers treat all three
/// the same "drop, don't crash" way [`crate::NetFarTerrain::decode_heights`]/
/// [`crate::lod_objects::NetLodZone::decode`] already do.
fn decompress<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<Vec<T>> {
    if bytes.is_empty() {
        return None;
    }
    // The initial capacity hint is just a hint (never a trust boundary): same
    // "clamp the hint, don't just clamp the real limit" defense
    // `NetLodZone::decode` applies, so a huge/corrupt `bytes.len()` can't
    // itself force an oversized upfront allocation before `decompress_raw`'s
    // own `output_limit` check even runs.
    let mut raw = Vec::with_capacity(
        bytes
            .len()
            .saturating_mul(2)
            .min(MAX_DECOMPRESSED_MAP_IMAGE_BYTES),
    );
    lz_fear::raw::decompress_raw(bytes, &[0; 0], &mut raw, MAX_DECOMPRESSED_MAP_IMAGE_BYTES)
        .ok()?;
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

    /// Regression proof (BL-82 EM-8.5): a REAL, legitimately-sized background
    /// image — at the actual shipped [`MAP_IMAGE_MAX_DIM`] cap, the largest a
    /// real server ever produces — still decodes fine under the new bound.
    /// Guards against picking [`MAX_DECOMPRESSED_MAP_IMAGE_BYTES`] too tight.
    #[test]
    fn decode_image_still_works_at_the_real_max_image_dimension() {
        // A real background image at the actual dimension a server would
        // send is not uniform (worldgen has real colour variety) — a small
        // repeating pattern is enough to prove the round trip at full size
        // without needing worldgen data in a unit test.
        let dim = MAP_IMAGE_MAX_DIM as usize;
        let pixels: Vec<[u8; 3]> = (0..dim * dim)
            .map(|i| {
                [
                    (i % 256) as u8,
                    ((i / 3) % 256) as u8,
                    ((i / 7) % 256) as u8,
                ]
            })
            .collect();
        let data = NetMapData::encode(
            [1024, 1024],
            32,
            [MAP_IMAGE_MAX_DIM, MAP_IMAGE_MAX_DIM],
            &pixels,
            Vec::new(),
            Vec::new(),
        );
        let decoded = data
            .decode_image()
            .expect("a real, max-sized background image must still decode under the new bound");
        assert_eq!(decoded.len(), pixels.len());
    }

    /// Decompression-bomb hardening (BL-82 EM-8.5): a small compressed blob
    /// that CLAIMS (via LZ4 back-references — the classic zip-bomb
    /// amplification mechanism, see [`MAX_DECOMPRESSED_MAP_IMAGE_BYTES`]'s doc
    /// comment) to decompress to far more than that cap must be rejected
    /// (`None`), not allocated. Same construction
    /// `lod_objects::tests::net_lod_zone_rejects_a_payload_that_decompresses_
    /// past_the_size_cap` uses: a large, highly-repetitive buffer compresses
    /// to a small fraction of its decompressed size, so this exercises the
    /// REAL `decompress_raw` call `decode_image` makes, not a mocked
    /// stand-in.
    #[test]
    fn decode_image_rejects_a_payload_that_decompresses_past_the_size_cap() {
        const BOMB_DECOMPRESSED_LEN: usize = 64 * 1024 * 1024; // 64 MiB
        let huge_repetitive = vec![0x42_u8; BOMB_DECOMPRESSED_LEN];

        let mut compressed = Vec::new();
        let mut table = lz_fear::raw::U32Table::default();
        lz_fear::raw::compress2(&huge_repetitive, 0, &mut table, &mut compressed)
            .expect("lz4 compression into a Vec<u8> is infallible");
        assert!(
            compressed.len() * 16 < huge_repetitive.len(),
            "test setup should compress the repetitive buffer to well under 1/16th its size, got \
             {} bytes for a {} byte input",
            compressed.len(),
            huge_repetitive.len()
        );

        let bomb = NetMapData {
            world_size_chunks: [1024, 1024],
            chunk_size_blocks: 32,
            image_size: [8192, 8192],
            image_rgb: compressed,
            markers: Vec::new(),
            pois: Vec::new(),
        };
        assert!(
            bomb.decode_image().is_none(),
            "a payload claiming to decompress past MAX_DECOMPRESSED_MAP_IMAGE_BYTES must be \
             rejected, not allocated"
        );
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

#[cfg(test)]
mod bandwidth_measurement {
    use super::*;

    /// Synthetic biome-like RGB buffer: large (32px) contiguous colour
    /// regions with a small per-pixel dither, approximating the low-entropy
    /// "large same-colour landmass/biome" structure a real
    /// `client::WorldData::map_image()` has (as opposed to uniform noise,
    /// which would be a worst-case, unrealistic compression estimate).
    fn synthetic_map_pixels(w: u32, h: u32) -> Vec<[u8; 3]> {
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let region_x = x / 32;
                let region_y = y / 32;
                let base_r = (region_x.wrapping_mul(37) % 200) as u8;
                let base_g = (region_y.wrapping_mul(53) % 200) as u8;
                let base_b = ((region_x + region_y).wrapping_mul(19) % 200) as u8;
                let dither = ((x ^ y) % 8) as u8;
                out.push([
                    base_r.saturating_add(dither),
                    base_g.saturating_add(dither),
                    base_b.saturating_add(dither),
                ]);
            }
        }
        out
    }

    /// Bandwidth regression guard for [`MAP_IMAGE_MAX_DIM`]: the ONE-SHOT
    /// payload at the chosen 1024 cap must stay well under a couple MB —
    /// measured (not guessed) at authoring time via this exact synthetic
    /// buffer: `dim=256 -> 22,945 B`, `dim=512 -> 49,756 B`,
    /// `dim=1024 -> 173,556 B` (all real `lz_fear` compression, not an
    /// estimate). Asserts a generous 1 MiB ceiling at the shipped cap — a
    /// regression here (e.g. someone bumping the cap further without
    /// re-measuring) fails loudly instead of silently ballooning a one-shot
    /// broadcast every client pays on connect.
    #[test]
    fn compressed_size_at_the_shipped_cap_stays_small() {
        let pixels = synthetic_map_pixels(MAP_IMAGE_MAX_DIM, MAP_IMAGE_MAX_DIM);
        let compressed = compress(&pixels);
        assert!(
            compressed.len() < 1024 * 1024,
            "compressed {}x{} map background grew to {} bytes (>1 MiB) — re-measure before \
             shipping a bigger MAP_IMAGE_MAX_DIM",
            MAP_IMAGE_MAX_DIM,
            MAP_IMAGE_MAX_DIM,
            compressed.len()
        );
    }

    /// A quartered resolution (256, the PREVIOUS cap that caused the
    /// "pixelated minimap" bug) compresses noticeably smaller than the
    /// shipped 1024 cap — sanity-checks that [`synthetic_map_pixels`] scales
    /// realistically with resolution (not a flat/degenerate buffer that
    /// would make this whole measurement meaningless).
    #[test]
    fn compressed_size_grows_with_resolution() {
        let small = compress(&synthetic_map_pixels(256, 256));
        let large = compress(&synthetic_map_pixels(MAP_IMAGE_MAX_DIM, MAP_IMAGE_MAX_DIM));
        assert!(
            large.len() > small.len(),
            "a 1024x1024 buffer must compress larger than a 256x256 one"
        );
    }
}
