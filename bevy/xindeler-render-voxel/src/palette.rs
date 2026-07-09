//! EM-3.4 — data-driven block palette (`block_palette.ron`, spec §4.2/§5.1).
//!
//! [`BlockPalette`] is a RON asset mapping `BlockKind` → texture-array layer
//! plus PBR parameters. It is THE source of truth for everything the voxel
//! material used to hardcode:
//! - the `kind → layer` mapping consumed by the EM-3.2 converter
//!   ([`BlockPalette::layer_lut`]),
//! - the per-layer look (base color, roughness, metallic, emissive mask) baked
//!   into procedural texture arrays ([`build_block_texture_arrays`]),
//! - the material-level knobs (`emissive_strength`, `ao_strength`) of
//!   [`crate::material::VoxelMaterialExt`].
//!
//! ## Anti-chaos sanitize (spec §5.1)
//! Same threat model as the atmosphere profiles: `.ron` under `assets/` is a
//! surface ORACLE-side tooling (or a hand-edit) can write garbage into, so
//! [`BlockPalette::sanitize`] runs on every ingestion path (the
//! [`BlockPaletteLoader`]) and forces every value finite and in range:
//! - colors → `0.0..=1.0` per channel (non-finite falls back to neutral),
//! - `roughness`/`metallic`/per-block `emissive_strength` → `0.0..=1.0`,
//! - material `emissive_strength` → `0.0..=1e6` (HDR luminance, finite),
//!   `ao_strength` → `0.0..=8.0`,
//! - layers → `< MAX_LAYERS` (out-of-range layers collapse to `default_layer`;
//!   a bad `default_layer` collapses to 0),
//! - `texture_size` → a power of two in `4..=128` (else the default 32).
//!
//! ## Procedural texture arrays with a REAL mip chain (spec §4.4)
//! [`build_block_texture_arrays`] emits albedo/normal/MRA `D2` array images
//! with a FULL mip chain (down to 1×1, simple 2×2 box downsample) and a
//! repeat + nearest-min/mag + LINEAR-mip sampler: nearest keeps texels crisp
//! up close while real mips kill the distant shimmer the EM-3.3 review
//! flagged (mip-less nearest sparkles under motion even with TAA). v1 look
//! is deliberately simple: per-layer base color × deterministic value noise,
//! FLAT normal maps; `texture` paths in the RON are parsed but unused (
//! reserved for future HD packs).
//!
//! ## Hot reload
//! The palette hot-reloads through the same `file_watcher` mechanism as the
//! atmosphere profiles. Because the layer index is BAKED per vertex
//! (`ATTRIBUTE_BLOCK_LAYER`), a palette edit requires re-meshing: the client
//! rebuilds the arrays, updates the material in place, and re-marks every
//! chunk dirty in the EM-3.5 pipeline. The visual change POPS (no lerp) —
//! accepted and documented: palettes are content edits, not ambience
//! transitions.
//!
//! Shrinking-reload window (accepted, documented): if a reload REDUCES
//! [`BlockPalette::layer_count`], live meshes keep indexing the OLD (larger)
//! layer range until the budgeted re-mesh drains — up to ~⌈chunks / budget⌉
//! frames. Out-of-range array layers in `textureSample` are safe under
//! WGSL/wgpu robustness rules (clamped/zeroed, never UB), so the worst case
//! is a few frames of wrong-but-harmless texels on those chunks; we chose
//! this over building reload arrays at `max(old, new)` layer count, which
//! would trade a transient for permanent VRAM overshoot.

use std::collections::HashMap;

use bevy::{
    asset::{Asset, AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    ecs::error::BevyError,
    image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    reflect::TypePath,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use common::terrain::BlockKind;
use serde::{Deserialize, Serialize};
use vek::Vec3;

/// Canonical asset path (relative to the `assets/` source root) of the block
/// palette. New namespace is deliberate (EM-3.4 board note): existing asset
/// names stay Veloren-verbatim, new Xindeler data lives under `xindeler/`.
pub const PALETTE_ASSET_PATH: &str = "xindeler/render/block_palette.ron";

/// Texture-array layer capacity guard. wgpu guarantees ≥ 256 2D-array
/// layers on every backend; 64 is our own sane cap (a palette needs one
/// layer per block LOOK, not per block kind — kinds may share).
pub const MAX_LAYERS: u32 = 64;

/// Reserved (v1: parsed but UNUSED) texture override paths for future HD
/// packs — when set, the loader will one day source the layer from real
/// images instead of the procedural generator.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockTexturePaths {
    pub albedo: Option<String>,
    pub normal: Option<String>,
    pub mra: Option<String>,
}

/// Palette entry for one block kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockLayerDef {
    /// Texture-array layer this kind renders with (< [`MAX_LAYERS`]).
    /// Multiple kinds may share a layer; the layer's LOOK is taken from the
    /// sharing kind with the lowest `BlockKind` discriminant (deterministic).
    pub layer: u32,
    /// sRGB base color of the procedural layer texture.
    pub base_color: [f32; 3],
    /// Perceptual roughness (`0.0..=1.0`; the shader clamps ≥ 0.045).
    pub roughness: f32,
    /// Metallic (`0.0..=1.0`).
    pub metallic: f32,
    /// Emissive mask (`0.0..=1.0`), baked into the MRA alpha channel and
    /// scaled by the material-level [`PaletteMaterialParams::
    /// emissive_strength`] (× albedo) in the shader — lava/crystal glow.
    pub emissive_strength: f32,
    /// Opacity (`0.0..=1.0`). Consumed today only by FLUID kinds (the
    /// interim transparent water material, until EM-3.9's water shader);
    /// opaque layers render opaque regardless.
    pub alpha: f32,
    /// Reserved HD-pack texture paths (v1: `None`, unused).
    pub texture: Option<BlockTexturePaths>,
}

impl Default for BlockLayerDef {
    fn default() -> Self {
        Self {
            layer: 0,
            base_color: [0.5, 0.5, 0.5],
            roughness: 0.9,
            metallic: 0.0,
            emissive_strength: 0.0,
            alpha: 1.0,
            texture: None,
        }
    }
}

/// Material-level knobs of [`crate::material::VoxelMaterialExt`] — data
/// since EM-3.4 (they used to be hardcoded in the client demo).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteMaterialParams {
    /// HDR luminance scale for the MRA emissive mask (feeds bloom; the
    /// default is tuned for the EV100 13 camera).
    pub emissive_strength: f32,
    /// Vertex-AO response remap (see `material/mod.rs::ao_strength` for why
    /// the v1 per-vertex bake needs > 1.0).
    pub ao_strength: f32,
}

impl Default for PaletteMaterialParams {
    fn default() -> Self {
        Self {
            emissive_strength: 60_000.0,
            ao_strength: 2.5,
        }
    }
}

/// The block palette asset (`xindeler/render/block_palette.ron`).
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockPalette {
    /// Layer for every kind without an explicit entry (< [`MAX_LAYERS`]).
    pub default_layer: u32,
    /// Per-layer texel resolution of the procedural arrays (power of two,
    /// `4..=128`).
    pub texture_size: u32,
    /// Material-level knobs (see [`PaletteMaterialParams`]).
    pub material: PaletteMaterialParams,
    /// Per-kind entries. RON keys are `BlockKind` variant names (`Rock`,
    /// `Earth`, `GlowingRock`, `Water`, ...).
    pub blocks: HashMap<BlockKind, BlockLayerDef>,
}

impl Default for BlockPalette {
    fn default() -> Self {
        Self {
            default_layer: 0,
            texture_size: 32,
            material: PaletteMaterialParams::default(),
            blocks: HashMap::new(),
        }
    }
}

/// `value` clamped into `(min, max)`; non-finite (NaN/±inf) falls back to
/// `default` (same contract as the atmosphere sanitizer).
fn sane(value: f32, (min, max): (f32, f32), default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

impl BlockPalette {
    /// Anti-chaos clamps (module docs). Runs on every ingestion path — the
    /// loader today, any future in-process caller. Idempotent.
    pub fn sanitize(&mut self) {
        let defaults = Self::default();
        if self.default_layer >= MAX_LAYERS {
            self.default_layer = 0;
        }
        if !self.texture_size.is_power_of_two() || !(4..=128).contains(&self.texture_size) {
            self.texture_size = defaults.texture_size;
        }
        self.material.emissive_strength = sane(
            self.material.emissive_strength,
            (0.0, 1.0e6),
            defaults.material.emissive_strength,
        );
        self.material.ao_strength = sane(
            self.material.ao_strength,
            (0.0, 8.0),
            defaults.material.ao_strength,
        );
        let entry_defaults = BlockLayerDef::default();
        for def in self.blocks.values_mut() {
            if def.layer >= MAX_LAYERS {
                def.layer = self.default_layer;
            }
            for i in 0..3 {
                def.base_color[i] =
                    sane(def.base_color[i], (0.0, 1.0), entry_defaults.base_color[i]);
            }
            def.roughness = sane(def.roughness, (0.0, 1.0), entry_defaults.roughness);
            def.metallic = sane(def.metallic, (0.0, 1.0), entry_defaults.metallic);
            def.emissive_strength = sane(
                def.emissive_strength,
                (0.0, 1.0),
                entry_defaults.emissive_strength,
            );
            def.alpha = sane(def.alpha, (0.0, 1.0), entry_defaults.alpha);
        }
    }

    /// Number of texture-array layers the palette uses (max referenced
    /// layer plus one), capped at [`MAX_LAYERS`]. The cap is idempotent for
    /// sanitized palettes (sanitize already collapses out-of-range layers)
    /// — it exists as a belt-and-braces guard for future in-process callers
    /// that skip [`Self::sanitize`]. Always `1..=MAX_LAYERS`.
    #[must_use]
    pub fn layer_count(&self) -> u32 {
        self.blocks
            .values()
            .map(|def| def.layer)
            .chain(std::iter::once(self.default_layer))
            .max()
            .unwrap_or(0)
            .min(MAX_LAYERS - 1)
            + 1
    }

    /// The `BlockKind as u8 → layer` lookup table consumed by the EM-3.2
    /// converter's `kind_to_layer` (and captured per EM-3.5 mesh task).
    /// Unknown kinds resolve to `default_layer`.
    #[must_use]
    pub fn layer_lut(&self) -> [u32; 256] {
        let mut lut = [self.default_layer; 256];
        for (kind, def) in &self.blocks {
            lut[*kind as u8 as usize] = def.layer;
        }
        lut
    }

    /// Per-layer looks in layer order (`layer_count` entries). Layers no
    /// kind maps to get the neutral [`BlockLayerDef::default`] look; layers
    /// shared by several kinds take the look of the kind with the lowest
    /// discriminant (deterministic regardless of `HashMap` iteration order).
    #[must_use]
    pub fn layer_defs(&self) -> Vec<BlockLayerDef> {
        let mut defs = vec![BlockLayerDef::default(); self.layer_count() as usize];
        let mut owner: Vec<Option<u8>> = vec![None; defs.len()];
        for (kind, def) in &self.blocks {
            let (layer, kind) = (def.layer as usize, *kind as u8);
            if layer >= defs.len() {
                // Unsanitized caller with an out-of-cap layer (layer_count
                // clamps to MAX_LAYERS): skip — sanitize would have
                // collapsed it to default_layer.
                continue;
            }
            if owner[layer].is_none_or(|current| kind < current) {
                owner[layer] = Some(kind);
                defs[layer] = def.clone();
            }
        }
        defs
    }
}

/// Async [`AssetLoader`] for the palette (AtmosphereProfileLoader pattern).
///
/// Registered for the plain `ron` extension: bevy 0.19 disambiguates
/// multiple loaders per extension by the requested asset type (verified in
/// bevy_asset-0.19.0 `server/loaders.rs::find`), so consumers MUST load it
/// typed (`asset_server.load::<BlockPalette>(...)`); other RON assets keep
/// their double extensions (`.atmo.ron`, ...).
#[derive(Default, TypePath)]
pub struct BlockPaletteLoader;

impl AssetLoader for BlockPaletteLoader {
    type Asset = BlockPalette;
    type Error = BevyError;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let mut palette: BlockPalette = ron::de::from_bytes(&bytes)?;
        // Anti-chaos (spec §5.1): asset files are untrusted input.
        palette.sanitize();
        Ok(palette)
    }

    fn extensions(&self) -> &[&str] { &["ron"] }
}

// ---------------------------------------------------------------------------
// Procedural texture arrays (base color + noise, flat normals, real mips)
// ---------------------------------------------------------------------------

/// The three texture arrays [`crate::material::VoxelMaterialExt`] binds.
pub struct BlockTextureArrays {
    /// sRGB albedo array.
    pub albedo: Image,
    /// Linear tangent-space normal array (flat in v1).
    pub normal: Image,
    /// Linear Metallic/Roughness/AO/emissive-mask array (channel convention
    /// in `material/mod.rs`).
    pub mra: Image,
}

/// Deterministic integer hash (same scheme as the mesher golden tests).
fn hash(p: Vec3<i32>) -> u32 {
    let mut h = (p.x as u32).wrapping_mul(0x9E37_79B9)
        ^ (p.y as u32).wrapping_mul(0x85EB_CA6B)
        ^ (p.z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h
}

/// 2D value noise in `[0, 1]` (per-layer salt keeps layers decorrelated).
#[expect(clippy::cast_possible_wrap, reason = "texel coords < 128")]
fn noise(x: u32, y: u32, salt: u32) -> f32 {
    let h = hash(Vec3::new(x as i32, y as i32, salt as i32));
    (h % 1024) as f32 / 1023.0
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "quantising [0,1] floats to u8"
)]
fn quantise(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0) as u8 }

/// Simple 2×2 box downsample of an RGBA8 level (`w`/`h` powers of two).
/// Done in storage space — NOT gamma-correct for the sRGB albedo; accepted
/// v1 simplification (slightly dark far mips beat shimmering no-mips).
fn downsample_rgba(prev: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    let texel = |x: u32, y: u32, c: u32| u32::from(prev[((y * w + x) * 4 + c) as usize]);
    for y in 0..nh {
        for x in 0..nw {
            let (sx, sy) = ((x * 2).min(w - 1), (y * 2).min(h - 1));
            let (sx1, sy1) = ((sx + 1).min(w - 1), (sy + 1).min(h - 1));
            for c in 0..4 {
                let sum =
                    texel(sx, sy, c) + texel(sx1, sy, c) + texel(sx, sy1, c) + texel(sx1, sy1, c);
                #[expect(clippy::cast_possible_truncation, reason = "u8 average of 4 u8s")]
                out.push(((sum + 2) / 4) as u8);
            }
        }
    }
    out
}

/// Repeat-addressed, nearest min/mag + LINEAR mip sampler (spec §4.4
/// anti-shimmer pairing — requires the real mip chain built below).
fn nearest_repeat_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    })
}

/// Assembles a `D2` array image with a FULL mip chain from a per-layer mip-0
/// generator. Data layout is wgpu's default `TextureDataOrder::LayerMajor`
/// (bevy_image 0.19 uploads with `Image::data_order`, default LayerMajor:
/// `Layer0Mip0 Layer0Mip1 … Layer1Mip0 …` — verified in
/// wgpu-types-29.0.4/src/texture.rs + bevy_render texture/gpu_image.rs).
fn array_image_with_mips(
    size: u32,
    layers: u32,
    format: TextureFormat,
    mut mip0: impl FnMut(u32) -> Vec<u8>,
) -> Image {
    let mip_levels = size.ilog2() + 1; // full chain down to 1×1
    let bytes_per_layer: usize = (0..mip_levels)
        .map(|m| {
            let s = (size >> m).max(1) as usize;
            s * s * 4
        })
        .sum();
    let mut data = Vec::with_capacity(bytes_per_layer * layers as usize);
    for layer in 0..layers {
        let mut level = mip0(layer);
        debug_assert_eq!(level.len(), (size * size * 4) as usize);
        data.extend_from_slice(&level);
        let (mut w, mut h) = (size, size);
        for _ in 1..mip_levels {
            level = downsample_rgba(&level, w, h);
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
            data.extend_from_slice(&level);
        }
    }

    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = mip_levels;
    image.data = Some(data);
    image.sampler = nearest_repeat_sampler();
    image
}

/// Builds the three block texture arrays from a (sanitized) palette:
/// albedo = base color × deterministic value noise (±18% brightness),
/// normal = flat (v1), MRA = per-layer metallic/roughness constants +
/// neutral texture-AO + the emissive mask (slightly noise-broken so glow
/// doesn't read as a flat decal). All arrays carry a full mip chain and the
/// nearest/linear-mip sampler (module docs).
#[must_use]
pub fn build_block_texture_arrays(palette: &BlockPalette) -> BlockTextureArrays {
    let size = palette.texture_size;
    let layers = palette.layer_count();
    // layer_count self-caps, so this only trips if the cap and this builder
    // ever drift apart — a guard for future in-process callers that skip
    // sanitize (the loader path always sanitizes).
    debug_assert!(
        layers <= MAX_LAYERS,
        "palette layer_count {layers} exceeds MAX_LAYERS {MAX_LAYERS}"
    );
    let defs = palette.layer_defs();

    let albedo = array_image_with_mips(size, layers, TextureFormat::Rgba8UnormSrgb, |layer| {
        let def = &defs[layer as usize];
        let mut data = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                // ±18% brightness variation around the base color.
                let n = 0.82 + 0.36 * noise(x, y, layer);
                data.extend_from_slice(&[
                    quantise(def.base_color[0] * n),
                    quantise(def.base_color[1] * n),
                    quantise(def.base_color[2] * n),
                    255,
                ]);
            }
        }
        data
    });

    // Flat tangent-space normal (128, 128, 255): v1 keeps relief to the
    // geometry; HD packs (BlockTexturePaths) will supply real normal maps.
    let normal = array_image_with_mips(size, layers, TextureFormat::Rgba8Unorm, |_layer| {
        [128, 128, 255, 255].repeat((size * size) as usize)
    });

    let mra = array_image_with_mips(size, layers, TextureFormat::Rgba8Unorm, |layer| {
        let def = &defs[layer as usize];
        let mut data = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                // Emissive mask: noise-thresholded veins when < 1, so glow
                // blocks read as veined rather than uniformly lit.
                let emissive =
                    if def.emissive_strength > 0.0 && noise(x / 2, y / 2, layer + 100) > 0.55 {
                        def.emissive_strength
                    } else {
                        0.0
                    };
                data.extend_from_slice(&[
                    quantise(def.metallic),
                    quantise(def.roughness),
                    255, // neutral texture AO (vertex AO does the work)
                    quantise(emissive),
                ]);
            }
        }
        data
    });

    BlockTextureArrays {
        albedo,
        normal,
        mra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped palette must parse, be already-sane (sanitize is a no-op)
    /// and resolve every demo block kind to its own layer.
    #[test]
    fn shipped_palette_parses_and_resolves_demo_kinds() {
        let text = include_str!("../../../assets/xindeler/render/block_palette.ron");
        let parsed: BlockPalette = ron::from_str(text).expect("block_palette.ron parses");
        let mut sanitized = parsed.clone();
        sanitized.sanitize();
        assert_eq!(parsed, sanitized, "shipped palette must already be sane");

        let lut = parsed.layer_lut();
        let layer = |kind: BlockKind| lut[kind as u8 as usize];
        // Demo kinds resolve (distinct looks for the three opaque kinds).
        assert_ne!(layer(BlockKind::Rock), layer(BlockKind::Earth));
        assert_ne!(layer(BlockKind::Rock), layer(BlockKind::GlowingRock));
        assert_ne!(layer(BlockKind::Earth), layer(BlockKind::GlowingRock));
        assert!(parsed.blocks.contains_key(&BlockKind::Water));
        // Unmapped kinds fall back to the default layer.
        assert_eq!(layer(BlockKind::Misc), parsed.default_layer);
        assert!(parsed.layer_count() <= MAX_LAYERS);
        // The glow kind actually carries an emissive mask.
        assert!(parsed.blocks[&BlockKind::GlowingRock].emissive_strength > 0.0);

        // Regression (BL-82 EM-3.11): tree wood/leaves (and other
        // structure-sourced solid content) must NOT resolve to Rock's
        // `default_layer` — that was the monochrome-gray-trees bug.
        assert_ne!(
            layer(BlockKind::Wood),
            parsed.default_layer,
            "Wood must not fall back to the default (Rock-gray) layer"
        );
        assert_ne!(
            layer(BlockKind::Leaves),
            parsed.default_layer,
            "Leaves must not fall back to the default (Rock-gray) layer"
        );
        assert_ne!(layer(BlockKind::Wood), layer(BlockKind::Leaves));
        assert_ne!(
            layer(BlockKind::GlowingMushroom),
            parsed.default_layer,
            "GlowingMushroom must not fall back to the default (Rock-gray) layer"
        );
        assert_ne!(
            layer(BlockKind::ArtLeaves),
            parsed.default_layer,
            "ArtLeaves must not fall back to the default (Rock-gray) layer"
        );
        // Leaves must read distinctly green (not another gray tone).
        let leaves = &parsed.blocks[&BlockKind::Leaves];
        assert!(
            leaves.base_color[1] > leaves.base_color[0]
                && leaves.base_color[1] > leaves.base_color[2],
            "Leaves base_color should be green-dominant, got {:?}",
            leaves.base_color
        );

        // Regression (BL-82 EM-3.11, follow-up): ordinary terrain-column
        // kinds must ALSO not resolve to Rock's default_layer — empirically
        // confirmed (forcing a demo-world column to BlockKind::Grass renders
        // Rock-gray, not green) that these carry no visual colour without an
        // explicit entry, despite worldgen embedding real per-voxel colour
        // that the render pipeline never reads.
        for kind in [
            BlockKind::Grass,
            BlockKind::Sand,
            BlockKind::Snow,
            BlockKind::WeakRock,
            BlockKind::GlowingWeakRock,
            BlockKind::Ice,
        ] {
            assert_ne!(
                layer(kind),
                parsed.default_layer,
                "{kind:?} must not fall back to the default (Rock-gray) layer"
            );
        }
        // Grass must read distinctly green (not another gray/brown tone).
        let grass = &parsed.blocks[&BlockKind::Grass];
        assert!(
            grass.base_color[1] > grass.base_color[0] && grass.base_color[1] > grass.base_color[2],
            "Grass base_color should be green-dominant, got {:?}",
            grass.base_color
        );
        // GlowingWeakRock must carry an emissive mask, same as GlowingRock.
        assert!(parsed.blocks[&BlockKind::GlowingWeakRock].emissive_strength > 0.0);
    }

    /// BL-82 EM-3.11 regression: the water entry's `alpha` must be high
    /// enough that its blue reads as dominant even when alpha-blended (`out =
    /// water*alpha + bg*(1-alpha)`, the exact `AlphaMode::Blend` compositing
    /// `palette_material.rs` wires up) over a fairly saturated GREEN
    /// background — a grassy river-bank / lakebed, or a moss-dark rock wall,
    /// both realistic terrain the fluid mesh renders in front of. The
    /// original 0.6-alpha, (0.15, 0.35, 0.6) tuning passed the EM-3.9b smoke
    /// screenshot (verified over pale sand/rock near the demo anchor) but a
    /// live playthrough found the SAME material reading as murky green over
    /// vegetation-heavy terrain (BL-82 EM-3.11 bug report — see
    /// `block_palette.ron`'s Water comment for the full writeup and the
    /// blend-math derivation this test encodes). This is a data guard, not a
    /// code-path check: it fails loudly if a future palette edit ever
    /// re-introduces a too-transparent or too-green Water tuning, without
    /// needing a screenshot to notice.
    #[test]
    fn shipped_palette_water_reads_blue_over_a_saturated_green_background() {
        let text = include_str!("../../../assets/xindeler/render/block_palette.ron");
        let parsed: BlockPalette = ron::from_str(text).expect("block_palette.ron parses");
        let water = &parsed.blocks[&BlockKind::Water];

        assert!(
            water.alpha >= 0.8,
            "water alpha {} is too low to stay blue-dominant against saturated backgrounds \
             (EM-3.11): the background contributes (1 - alpha) of the blend, so a low alpha lets \
             a green background wash the blue out",
            water.alpha
        );

        // A fairly saturated grass/moss green (matches the range Xindeler's
        // worldgen actually paints terrain — e.g. `world/src/layer/mod.rs`'s
        // `Rgb::new(10, 75, 90)`-family grass tones, generalized to a
        // stronger, more adversarial green so this is a real stress test).
        let bg = [0.15_f32, 0.75, 0.20];
        let a = water.alpha;
        let blended: Vec<f32> = (0..3)
            .map(|i| water.base_color[i] * a + bg[i] * (1.0 - a))
            .collect();
        assert!(
            blended[2] > blended[1] * 1.3,
            "blue ({}) must clearly dominate green ({}) once blended over a saturated green \
             background — this is the EM-3.11 bug reproduced in miniature: blended = {blended:?}",
            blended[2],
            blended[1]
        );
        assert!(
            blended[2] > blended[0],
            "blue ({}) must dominate red ({}) too — blended = {blended:?}",
            blended[2],
            blended[0]
        );
    }

    /// Anti-chaos: hostile values come out finite and in-bounds.
    #[test]
    fn sanitize_defuses_hostile_palettes() {
        let mut garbage = BlockPalette {
            default_layer: 9_999,
            texture_size: 7,
            material: PaletteMaterialParams {
                emissive_strength: f32::INFINITY,
                ao_strength: -3.0,
            },
            blocks: HashMap::from([(BlockKind::Rock, BlockLayerDef {
                layer: MAX_LAYERS + 5,
                base_color: [f32::NAN, -2.0, 42.0],
                roughness: f32::NEG_INFINITY,
                metallic: 7.0,
                emissive_strength: f32::NAN,
                alpha: -5.0,
                texture: None,
            })]),
        };
        // Even UNSANITIZED, layer_count self-caps (m2 guard for in-process
        // callers) and layer_defs skips the out-of-cap entry.
        assert_eq!(garbage.layer_count(), MAX_LAYERS);
        assert_eq!(garbage.layer_defs().len() as u32, MAX_LAYERS);
        garbage.sanitize();

        assert_eq!(garbage.default_layer, 0);
        assert_eq!(garbage.texture_size, 32);
        let mat_defaults = PaletteMaterialParams::default();
        assert!(
            (garbage.material.emissive_strength - mat_defaults.emissive_strength).abs()
                < f32::EPSILON
        );
        assert!((garbage.material.ao_strength - 0.0).abs() < f32::EPSILON);
        let rock = &garbage.blocks[&BlockKind::Rock];
        assert_eq!(rock.layer, 0, "bad layer collapses to default_layer");
        let entry_defaults = BlockLayerDef::default();
        assert_eq!(rock.base_color, [entry_defaults.base_color[0], 0.0, 1.0]);
        assert!((rock.roughness - entry_defaults.roughness).abs() < f32::EPSILON);
        assert!((rock.metallic - 1.0).abs() < f32::EPSILON);
        assert!((rock.emissive_strength - 0.0).abs() < f32::EPSILON);
        assert!((rock.alpha - 0.0).abs() < f32::EPSILON);

        // Sanitizing twice must not drift (loader + future callers).
        let once = garbage.clone();
        garbage.sanitize();
        assert_eq!(garbage, once);
    }

    /// The arrays must carry a REAL mip chain (EM-3.3 review: nearest
    /// min/mag + linear mip only anti-shimmers over actual mips) and exactly
    /// the LayerMajor data volume the descriptor promises.
    #[test]
    fn texture_arrays_have_real_mip_chains() {
        let text = include_str!("../../../assets/xindeler/render/block_palette.ron");
        let mut palette: BlockPalette = ron::from_str(text).expect("parses");
        palette.sanitize();
        let arrays = build_block_texture_arrays(&palette);

        for (name, image) in [
            ("albedo", &arrays.albedo),
            ("normal", &arrays.normal),
            ("mra", &arrays.mra),
        ] {
            let desc = &image.texture_descriptor;
            assert!(
                desc.mip_level_count > 1,
                "{name} must have a real mip chain"
            );
            assert_eq!(desc.mip_level_count, palette.texture_size.ilog2() + 1);
            assert_eq!(desc.size.depth_or_array_layers, palette.layer_count());
            let bytes_per_layer: usize = (0..desc.mip_level_count)
                .map(|m| {
                    let s = (palette.texture_size >> m).max(1) as usize;
                    s * s * 4
                })
                .sum();
            assert_eq!(
                image.data.as_ref().expect("has data").len(),
                bytes_per_layer * palette.layer_count() as usize,
                "{name} data must match the LayerMajor mip layout"
            );
            // Nearest min/mag + linear mip (spec §4.4).
            let ImageSampler::Descriptor(sampler) = &image.sampler else {
                panic!("{name} must carry an explicit sampler");
            };
            assert_eq!(sampler.min_filter, ImageFilterMode::Nearest);
            assert_eq!(sampler.mag_filter, ImageFilterMode::Nearest);
            assert_eq!(sampler.mipmap_filter, ImageFilterMode::Linear);
        }
    }
}
