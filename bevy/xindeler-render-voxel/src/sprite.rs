//! EM-3.9 — block sprites (grass / flowers / props): manifest read, per-chunk
//! instance collection, and `.vox` → `bevy::Mesh` meshing.
//!
//! ## What a sprite is
//! In Veloren a *sprite* is an unfilled (non-terrain) block that carries a
//! [`SpriteKind`] — grass tufts, flowers, mushrooms, reeds, chests, furniture,
//! … Each `SpriteKind` maps (via `sprite_manifest.ron`) to one or more `.vox`
//! MODEL variations. The world is FULL of them (thousands of grass sprites per
//! chunk), so they are drawn as many INSTANCES of a handful of shared meshes,
//! never re-meshed per placement.
//!
//! ## The split (engine shell vs client glue), mirroring `figure`
//! This module is the render-crate half — it stays free of the asset system:
//! - [`SpriteManifest`] is a MINIMAL portable read of the real
//!   `assets/voxygen/voxel/sprite_manifest.ron` (names frozen — isolation law
//!   rule 3), keeping only what v1 needs (`SpriteKind → variations{model,
//!   offset}`); LOD levels, wind-sway and attribute FILTERS are ignored for v1
//!   (documented gaps → EM-3.9b). The manifest RON deserialises straight into
//!   it.
//! - [`collect_sprite_instances`] scans a [`TerrainChunk`]'s blocks and returns
//!   one [`SpriteInstance`] per sprite block (kind + chunk-local placement:
//!   world position, z-rotation, mirror), exactly how voxygen's
//!   `get_sprite_instances` walks the chunk (`block.get_sprite()` +
//!   `sprite_z_rot` + `sprite_mirror_vec`), minus LOD banding.
//! - [`sprite_model_to_bevy`] meshes ONE variation `.vox` into a coloured
//!   `bevy::Mesh`, reusing the figure segment mesher ([`segment_to_bevy`]) — a
//!   sprite is just a tiny static figure part.
//!
//! The client (`xindeler-client::sprite_view`) owns the `AssetServer`: it loads
//! the manifest + the referenced `.vox` files, calls [`sprite_model_to_bevy`]
//! ONCE per (kind, variation) to build a shared mesh, then spawns one entity
//! per [`SpriteInstance`] sharing that mesh handle (Bevy batches shared
//! mesh+material draws — effectively instanced).
//!
//! ## Placement / scale (matches voxygen `scene/terrain/mod.rs`)
//! Sprite `.vox` models are authored at 11× (like figures), so every instance
//! is scaled by [`SPRITE_SCALE`] = `1/11`. The instance transform is, in
//! Veloren space: translate to the block's world position (block CENTRE in xy,
//! block FLOOR in z), rotate about z by the block's sprite orientation, mirror
//! per the block's mirror attrs, then scale. The manifest `offset` recentres
//! the model around its origin and is fed to the MESHER (baked into the mesh),
//! exactly as voxygen does, so the per-instance transform carries none of it.
//! The final z-up→y-up map is applied by the caller when it builds the Bevy
//! `Transform` (see `sprite_view`), the SAME rotation `figure`/`convert` bake.

use std::collections::HashMap;

use bevy::mesh::Mesh as BevyMesh;
use common::{
    figure::Segment,
    terrain::{Block, SpriteKind, TerrainChunk},
    vol::{IntoVolIterator, RectRasterableVol},
};
use serde::Deserialize;
use vek::*;

use crate::figure::segment_to_bevy;

/// Sprite `.vox` models are authored 11× oversized (same convention as
/// figures); every instance is scaled down by this factor. Mirror of
/// `voxygen/src/scene/terrain/mod.rs`'s `SPRITE_SCALE`.
pub const SPRITE_SCALE: f32 = 1.0 / 11.0;

/// The frozen sprite manifest asset name (isolation law rule 3 — never
/// renamed). The client resolves this the same way figure manifests are
/// (`voxygen.voxel.sprite_manifest` → `voxygen/voxel/sprite_manifest.ron`).
pub const SPRITE_MANIFEST: &str = "voxygen.voxel.sprite_manifest";

/// The namespace figure/sprite `.vox` names in the manifest are relative to
/// (`voxygen.voxel`). A manifest `model: "voxygen.voxel.sprite.grass.grass_0"`
/// is already fully qualified, so — unlike the figure manifests — sprite model
/// names carry the whole path and need no prefixing.
pub const VOX_NAMESPACE: &str = "voxygen.voxel";

/// One model variation of a sprite: which `.vox` + the recentring offset baked
/// into its mesh. A minimal portable read of voxygen's `SpriteModelConfig`
/// (its `lod_axes` + `custom_indices` are dropped for v1).
#[derive(Deserialize, Clone, Debug)]
pub struct SpriteModelConfig {
    /// Fully-qualified `.vox` asset name (`voxygen.voxel.sprite.…`).
    pub model: String,
    /// Model centre offset (voxel units), fed to the mesher (baked into the
    /// mesh), exactly as voxygen does.
    pub offset: (f32, f32, f32),
}

/// A configuration group for a sprite kind (voxygen's `SpriteConfig`, minus the
/// attribute `filter` and `wind_sway` for v1). Only `variations` is read.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SpriteConfig {
    /// All model variations for this sprite; an instance picks one by a
    /// position seed (see [`variation_index`]).
    pub variations: Vec<SpriteModelConfig>,
}

/// The whole sprite manifest: `SpriteKind → [config]`. v1 uses the FIRST config
/// group per kind (voxygen selects a group by attribute filter; without filter
/// support v1 always takes the first — documented gap → EM-3.9b). Deserialises
/// directly from `sprite_manifest.ron`.
///
/// The manifest RON has a leading `#![enable(unwrap_newtypes, implicit_some)]`
/// and an `Empty: [()]` sentinel; both parse cleanly into this shape (the
/// `[()]` for kinds with no model becomes a config whose `variations` is
/// empty — such kinds are simply skipped at build time).
#[derive(Deserialize, Debug)]
pub struct SpriteManifest(pub HashMap<SpriteKind, Vec<SpriteConfig>>);

impl SpriteManifest {
    /// The first non-empty variation list for `kind`, or `None` if the kind is
    /// absent / has no model (e.g. `Empty`). v1 ignores attribute filters and
    /// takes the first config group.
    #[must_use]
    pub fn variations(&self, kind: SpriteKind) -> Option<&[SpriteModelConfig]> {
        self.0
            .get(&kind)?
            .iter()
            .map(|c| c.variations.as_slice())
            .find(|v| !v.is_empty())
    }
}

/// One sprite to draw: its kind + the chunk-LOCAL placement the caller turns
/// into a Bevy `Transform`. Positions are in Veloren block coordinates relative
/// to the CHUNK ORIGIN (add the chunk origin, then z-up→y-up, to get world
/// Bevy space — the caller does this so the chunk-entity translation convention
/// stays in one place).
#[derive(Clone, Copy, Debug)]
pub struct SpriteInstance {
    /// Which sprite (selects the shared mesh in the caller's cache).
    pub kind: SpriteKind,
    /// Block centre in xy, block floor in z, chunk-local (Veloren coords).
    /// voxygen places the model origin at the block's `(x + 0.5, y + 0.5, z)`.
    pub rel_pos: Vec3<f32>,
    /// Z rotation (radians) from the block's `Ori` sprite attribute.
    pub z_rot: f32,
    /// Per-axis mirror (±1) from the block's mirror attributes.
    pub mirror: Vec3<f32>,
    /// Position seed used to pick a variation deterministically (so the same
    /// block always shows the same model — voxygen's "awful PRNG").
    pub seed: u64,
}

impl SpriteInstance {
    /// Which variation this instance uses, given how many exist (mirrors
    /// voxygen's seed-mod selection). `variation_count` must be ≥ 1.
    #[must_use]
    pub fn variation_index(&self, variation_count: usize) -> usize {
        (self.seed as usize) % variation_count.max(1)
    }
}

/// Scans a chunk for sprite blocks and returns one [`SpriteInstance`] per
/// sprite, in chunk-local Veloren coordinates. Mirrors voxygen's
/// `get_sprite_instances` block walk (`block.get_sprite()` gate +
/// `sprite_z_rot` + `sprite_mirror_vec`), minus the LOD banding and glow
/// lookup. The caller filters by kind (v1 renders a whitelist of common
/// outdoor sprites) and applies a density budget.
#[must_use]
pub fn collect_sprite_instances(chunk: &TerrainChunk) -> Vec<SpriteInstance> {
    let mut out = Vec::new();
    let lo = Vec3::new(0, 0, chunk.get_min_z());
    let hi = Vec3::new(
        TerrainChunk::RECT_SIZE.x as i32,
        TerrainChunk::RECT_SIZE.y as i32,
        chunk.get_max_z() + 1,
    );
    for (rel, block) in chunk.vol_iter(lo, hi) {
        if let Some(instance) = sprite_instance_at(rel, block) {
            out.push(instance);
        }
    }
    out
}

/// Builds a [`SpriteInstance`] for a single block if it carries a sprite.
/// Split out so it can be unit-tested against a synthetic block without a
/// whole chunk.
#[must_use]
pub fn sprite_instance_at(rel_pos: Vec3<i32>, block: &Block) -> Option<SpriteInstance> {
    let kind = block.get_sprite()?;
    // `SpriteKind::Empty` is the "no sprite" sentinel that every plain air block
    // carries (`Block::empty()`), not a drawable — skip it.
    if kind == SpriteKind::Empty {
        return None;
    }
    // voxygen's per-position "awful PRNG" (scene/terrain/mod.rs) — deterministic
    // per world block, but here seeded on the chunk-local position (the caller
    // does not need cross-chunk determinism for v1 variation choice).
    let (x, y) = (rel_pos.x as u64, rel_pos.y as u64);
    let seed = x
        .wrapping_mul(3)
        .wrapping_add(y.wrapping_mul(7))
        .wrapping_add(x.wrapping_mul(y));
    Some(SpriteInstance {
        kind,
        // Model origin at the block CENTRE in xy, block FLOOR in z (voxygen).
        rel_pos: Vec3::new(
            rel_pos.x as f32 + 0.5,
            rel_pos.y as f32 + 0.5,
            rel_pos.z as f32,
        ),
        z_rot: block.sprite_z_rot().unwrap_or(0.0),
        mirror: block.sprite_mirror_vec(),
        seed,
    })
}

/// Meshes one sprite `.vox` variation into a coloured `bevy::Mesh`, reusing the
/// figure segment mesher (a sprite is a tiny static figure part). `offset` is
/// the manifest recentring offset (voxel units). Returns `None` if the model
/// meshes to nothing (empty `.vox`).
///
/// The `.vox` bytes come from the caller (this crate keeps `common`'s
/// `no-assets` — it never loads assets itself), parsed into a
/// [`common::figure::Segment`] with the given `model_index`.
#[must_use]
pub fn sprite_model_to_bevy(
    vox: &dot_vox::DotVoxData,
    model_index: usize,
    offset: Vec3<f32>,
) -> Option<BevyMesh> {
    // v1 ignores the manifest's `custom_indices` overrides (default material
    // index → Cell mapping only) — documented gap → EM-3.9b.
    let segment = Segment::from_vox(vox, false, model_index, None);
    segment_to_bevy(&segment, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest parses into our minimal portable shape and exposes grass
    /// variations. `#[ignore]` — needs the real asset; run locally with
    /// `XINDELER_ASSETS` pointing at `assets/`.
    #[test]
    #[ignore = "reads the real sprite_manifest.ron asset"]
    fn real_manifest_parses_and_has_grass() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/voxygen/voxel/sprite_manifest.ron");
        let bytes = std::fs::read(&path).expect("read sprite_manifest.ron");
        let manifest: SpriteManifest =
            ron::de::from_bytes(&bytes).expect("parse sprite_manifest.ron");
        let grass = manifest
            .variations(SpriteKind::ShortGrass)
            .expect("ShortGrass has variations");
        assert!(!grass.is_empty(), "grass must have ≥1 model variation");
        assert!(
            grass[0].model.starts_with("voxygen.voxel."),
            "sprite model names are fully qualified: {}",
            grass[0].model
        );
    }

    /// A non-sprite (air / solid) block yields no instance; a sprite block
    /// yields one with a sensible chunk-local placement.
    #[test]
    fn sprite_instance_only_for_sprite_blocks() {
        // Air: not a sprite.
        assert!(sprite_instance_at(Vec3::new(1, 2, 3), &Block::empty()).is_none());

        // A grass sprite block (unfilled + a SpriteKind).
        let grass = Block::air(SpriteKind::ShortGrass);
        let inst = sprite_instance_at(Vec3::new(1, 2, 3), &grass)
            .expect("a sprite block yields an instance");
        assert_eq!(inst.kind, SpriteKind::ShortGrass);
        // Centred in xy, floored in z.
        assert_eq!(inst.rel_pos, Vec3::new(1.5, 2.5, 3.0));
        // No orientation attr set → no rotation, unit mirror.
        assert_eq!(inst.z_rot, 0.0);
        assert_eq!(inst.mirror, Vec3::new(1.0, 1.0, 1.0));
    }

    /// Variation selection is deterministic and in range.
    #[test]
    fn variation_index_in_range() {
        let inst = sprite_instance_at(Vec3::new(5, 9, 0), &Block::air(SpriteKind::ShortGrass))
            .expect("grass instance");
        for count in 1..=8 {
            assert!(inst.variation_index(count) < count);
        }
        // Zero-safe (max(1)).
        assert_eq!(inst.variation_index(0), 0);
    }
}
