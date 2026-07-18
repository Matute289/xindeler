//! BL-82 EM-3.11-FH Phase C — the LOD-object zone mirror: server → client
//! broadcast of distant trees/structures (`common::lod::Object`) for the
//! far-terrain horizon, following [`crate::CompressedChunk`]/
//! [`crate::RemoveChunk`]'s exact "batch add/remove over the session" shape
//! (NOT [`crate::NetFarTerrain`]'s one-shot shape — zones stream in/out as the
//! player roams, just like chunks).
//!
//! ## Why this is the ONLY new wire surface Phase C needs
//! The client↔server LOD-zone request/response protocol
//! (`ClientGeneral::LodZoneRequest` / `ServerGeneral::LodZoneUpdate`), the
//! spiral-request-with-5s-throttle scheduling, the distance-based cull, AND
//! the server-side zone computation (`world::World::get_lod_zone`, cached
//! whole-world-upfront in `server::lod::Lod`) **all already exist**, verbatim,
//! in the shared (upstream, unmigrated) `client`/`server`/`world` crates —
//! this is exactly the "data already exists, sample+transport+render only"
//! shape the epic's spec §1 established for Phase A/B, extended one layer
//! further than the task board anticipated: the embedded local-player
//! [`client::Client`] (`xindeler-sim-bridge::EmbeddedPlayer`) already performs
//! the WHOLE request/response dance over its real TCP-loopback connection to
//! the embedded `server::Server`, entirely inside `Client::tick` — no new
//! server-side or embedded-client-side code was needed for that half. The
//! only missing piece was mirroring the ALREADY-POPULATED
//! `EmbeddedPlayer::lod_zones()` map to the pure-Bevy client, the same
//! "Embedded-Sim + Mirror" pattern every other subsystem in this migration
//! uses — that is what [`NetLodZone`]/[`NetLodZoneRemove`] are for.
//!
//! ## Reuses `common::lod::Object` verbatim — no new logic type
//! Per the epic's own constraint (spec §4: "reuse `common::lod::{Zone,
//! Object, ObjectKind, InstFlags, ZONE_SIZE}` — no new logic types"),
//! [`NetLodZone`] just lz4+bincode-compresses the SAME `Vec<common::lod::
//! Object>` `world::World::get_lod_zone` already produces (kind/pos/flags/
//! color) — exactly [`crate::NetFarTerrain`]'s "per-layer compressed blob"
//! scheme, reused for a single blob here since there is only one layer.

use bevy::ecs::message::Message;
use common::lod::Object;
use serde::{Deserialize, Serialize};

/// Hard cap on the DECOMPRESSED size (bytes) [`NetLodZone::decode`] will
/// accept, passed as `lz_fear::raw::decompress_raw`'s `output_limit`.
///
/// ## Why this matters (decompression-bomb hardening)
/// `decompress_raw`'s LZ4 back-reference ("duplicate") mechanism can expand a
/// SMALL compressed input into an arbitrarily large output buffer (the
/// classic zip-bomb amplification) — the crate's own doc comment on
/// `output_limit` says as much, and only bounds the back-reference path, not
/// literals (which are 1:1 with the input, so can't amplify on their own).
/// Passing `usize::MAX` here (as `crate::CompressedChunk`/`NetFarTerrain`/
/// `map::NetMapData` still do, over the trusted local loopback, an accepted
/// EM-4.2d-deferred posture — see `CompressedChunk::decode`'s own doc
/// comment) disables that ONLY protection entirely: a small malicious or
/// merely-corrupt `objects` blob could force an allocation far beyond any
/// real zone's size, a real DoS/OOM vector once this ever reaches a
/// non-loopback client. Unlike those pre-existing call sites, this one is
/// new — so it ships with a real bound from day one rather than inheriting
/// the deferred posture.
///
/// A legitimate zone is capped server-side at
/// `xindeler_sim_bridge::lod_objects::LOD_ZONE_MAX_OBJECTS` (800) objects;
/// even a generously-padded bincode encoding of `common::lod::Object`
/// (kind/pos/flags/color) is well under 64 bytes each, so 800 objects is
/// comfortably under 64 KiB. This cap is set two orders of magnitude above
/// that (giving headroom for that server-side cap to grow later without
/// touching this constant) while still bounding worst-case memory use to
/// something a client can always safely allocate.
///
/// ## What this does NOT cover (bevy-migration-reviewer finding)
/// This only bounds the DECOMPRESSED size — the amplification vector. The
/// COMPRESSED wire size (`NetLodZone::objects.len()` itself, i.e. the bytes
/// actually received before decompression even starts) is unbounded at this
/// layer; a peer could still send a large, non-amplifying blob, bounded only
/// by whatever the transport (replicon/quinnet) itself enforces as a max
/// message size — that is that layer's job, not this one's. "Hardened
/// against a decompression bomb" is not the same claim as "hardened against
/// every oversized payload."
const MAX_DECOMPRESSED_ZONE_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

/// Server → client: a batch of LOD objects (trees/structures) for zone `key`
/// (`common::lod::to_wpos`/`from_wpos` zone-coordinate convention, `ZONE_SIZE`
/// = 32 chunks per axis). Sent once per zone the whole time it stays known to
/// the embedded player's own `lod_zones()` cache — a zone key never repeats
/// unless it first left (see [`NetLodZoneRemove`]) and came back, mirroring
/// [`crate::CompressedChunk`]'s own "batch payload, sent on
/// upsert" semantics (not [`crate::NetFarTerrain`]'s strict "boot-once, never
/// resent" latch).
#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NetLodZone {
    /// Zone-grid key (`common::lod::{to_wpos, from_wpos}` convention).
    pub key: [i32; 2],
    /// lz4-compressed bincode of `Vec<common::lod::Object>` — verbatim server
    /// data, no re-derivation. Decode with [`Self::decode`].
    pub objects: Vec<u8>,
}

impl NetLodZone {
    /// Serializes (bincode `legacy()`) + compresses (lz4) a zone's object
    /// list — same scheme as [`crate::NetFarTerrain::encode`]'s per-layer
    /// blobs / [`crate::CompressedChunk::encode`].
    #[must_use]
    pub fn encode(key: [i32; 2], objects: &[Object]) -> Self {
        let raw = bincode::serde::encode_to_vec(objects, bincode::config::legacy())
            .expect("bincode serialization can only fail if a byte limit is set");
        let mut bytes = Vec::with_capacity(raw.len() / 4 + 16);
        let mut table = lz_fear::raw::U32Table::default();
        lz_fear::raw::compress2(&raw, 0, &mut table, &mut bytes)
            .expect("lz4 compression into a Vec<u8> is infallible");
        Self {
            key,
            objects: bytes,
        }
    }

    /// Decompresses + deserializes the object list. `None` on a corrupt
    /// payload (defensive; the local loopback can't corrupt it) OR one that
    /// claims to decompress past [`MAX_DECOMPRESSED_ZONE_BYTES`] (defensive
    /// against a decompression-bomb payload — see that constant's doc
    /// comment) — callers treat both the same "drop, don't crash" way
    /// `NetFarTerrain::decode_heights` etc. do.
    #[must_use]
    pub fn decode(&self) -> Option<Vec<Object>> {
        // The initial capacity hint is just a hint (never a trust boundary):
        // clamped to the same real cap so a huge/corrupt `objects.len()`
        // can't itself force an oversized upfront allocation before
        // `decompress_raw`'s own `output_limit` check even runs.
        let mut raw = Vec::with_capacity(
            self.objects
                .len()
                .saturating_mul(2)
                .min(MAX_DECOMPRESSED_ZONE_BYTES),
        );
        lz_fear::raw::decompress_raw(
            &self.objects,
            &[0; 0],
            &mut raw,
            MAX_DECOMPRESSED_ZONE_BYTES,
        )
        .ok()?;
        bincode::serde::decode_from_slice(&raw, bincode::config::legacy())
            .ok()
            .map(|(objects, _)| objects)
    }
}

/// Server → client: zone `key` fell out of the embedded player's own
/// `lod_zones()` cull radius — drop its objects (mesh + any cached data).
/// Same Terrain lane as [`NetLodZone`]/[`crate::RemoveChunk`]. ⚠️ Same
/// caveat as `RemoveChunk`'s own doc comment: the lane is UNORDERED, so over
/// a real (non-loopback) transport a remove could in principle overtake the
/// zone update it removes — the v1 loopback preserves order.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetLodZoneRemove {
    pub key: [i32; 2],
}

#[cfg(test)]
mod tests {
    use common::lod::{InstFlags, ObjectKind};
    use vek::{Rgb, Vec3};

    use super::*;

    fn sample_objects() -> Vec<Object> {
        vec![
            Object {
                kind: ObjectKind::Pine,
                pos: Vec3::new(10, -5, 42),
                flags: InstFlags::SNOW_COVERED,
                color: Rgb::new(20, 80, 30),
            },
            Object {
                kind: ObjectKind::House,
                pos: Vec3::new(-3, 7, 40),
                flags: InstFlags::empty(),
                color: Rgb::new(120, 60, 40),
            },
        ]
    }

    /// Encode/decode round-trips the object list exactly (kind/pos/flags/
    /// color all preserved) — the basic wire contract [`NetLodZone`] exists
    /// for.
    #[test]
    fn net_lod_zone_round_trips() {
        let objects = sample_objects();
        let encoded = NetLodZone::encode([3, -2], &objects);
        assert_eq!(encoded.key, [3, -2]);
        let decoded = encoded.decode().expect("must decode");
        assert_eq!(decoded.len(), objects.len());
        for (a, b) in decoded.iter().zip(objects.iter()) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.pos, b.pos);
            assert_eq!(a.flags.bits(), b.flags.bits());
            assert_eq!(a.color, b.color);
        }
    }

    /// An empty zone (no objects) round-trips to an empty (not `None`) list —
    /// distinct from `NetFarTerrain`'s "empty blob ⇒ layer absent" contract,
    /// since a real zone with zero objects (a bare plain) is a normal,
    /// meaningful state here, not a missing layer.
    /// (`common::lod::Object` doesn't implement `PartialEq` — a logic-crate
    /// type this crate must not edit per the isolation law — so this
    /// compares length/emptiness rather than `assert_eq!`ing the `Option`.)
    #[test]
    fn net_lod_zone_empty_zone_round_trips_to_empty_list() {
        let encoded = NetLodZone::encode([0, 0], &[]);
        let decoded = encoded.decode().expect("empty zone must still decode");
        assert!(decoded.is_empty());
    }

    /// A corrupted payload decodes to `None`, not a panic.
    #[test]
    fn net_lod_zone_rejects_corrupt_payload() {
        let mut encoded = NetLodZone::encode([1, 1], &sample_objects());
        encoded.objects.truncate(2);
        assert!(encoded.decode().is_none());
    }

    /// Decompression-bomb hardening: a small compressed blob that CLAIMS (via
    /// LZ4 back-references, the classic zip-bomb amplification mechanism —
    /// see [`MAX_DECOMPRESSED_ZONE_BYTES`]'s doc comment) to decompress to
    /// far more than [`MAX_DECOMPRESSED_ZONE_BYTES`] must be rejected
    /// (`None`), not allocated. Built by hand (not via `encode`, which only
    /// ever compresses real, small `Object` data) by lz4-compressing a
    /// large, highly-repetitive buffer — repetitive data compresses to a
    /// small fraction of its decompressed size (the whole point of the
    /// attack: a disproportionately small wire cost for a huge decompressed
    /// cost) while still exercising the REAL `decompress_raw` call `decode`
    /// makes, not a mocked stand-in.
    #[test]
    fn net_lod_zone_rejects_a_payload_that_decompresses_past_the_size_cap() {
        // Comfortably past MAX_DECOMPRESSED_ZONE_BYTES (8 MiB) once
        // decompressed, but highly repetitive so the COMPRESSED form is a
        // small fraction of that.
        const BOMB_DECOMPRESSED_LEN: usize = 32 * 1024 * 1024; // 32 MiB
        let huge_repetitive = vec![0x41_u8; BOMB_DECOMPRESSED_LEN];

        let mut compressed = Vec::new();
        let mut table = lz_fear::raw::U32Table::default();
        lz_fear::raw::compress2(&huge_repetitive, 0, &mut table, &mut compressed)
            .expect("lz4 compression into a Vec<u8> is infallible");
        // Confirms this test actually exercises the amplification case (a
        // disproportionately small wire payload for a huge decompressed
        // size — measured ~256x for this buffer with lz_fear's own encoder,
        // this asserts a much more conservative >16x so the test doesn't
        // become flaky against encoder/version differences), not a fluke.
        assert!(
            compressed.len() * 16 < huge_repetitive.len(),
            "test setup should compress the repetitive buffer to well under 1/16th its size, got \
             {} bytes for a {} byte input",
            compressed.len(),
            huge_repetitive.len()
        );

        let bomb = NetLodZone {
            key: [0, 0],
            objects: compressed,
        };
        assert!(
            bomb.decode().is_none(),
            "a payload claiming to decompress past MAX_DECOMPRESSED_ZONE_BYTES must be rejected, \
             not allocated"
        );
    }
}
