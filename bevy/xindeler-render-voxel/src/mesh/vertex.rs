// LIFT-COPY (BL-82 EM-3.1, Mapper C5-C7) from voxygen/src/render/{mod.rs,
// renderer/mod.rs, pipelines/{mod,terrain,fluid,sprite,particle,figure}.rs}
// @ 6f9afd978c — ADAPTED: portable replacements for voxygen's wgpu-packed
// vertex + atlas types.
//
// This is the ONE deliberately-rewritten file of the EM-3.1 port. The mesh
// algorithms (greedy.rs / terrain.rs / segment.rs) are byte-similar copies and
// only see these types through the same constructor/associated-fn calls as
// upstream, so upstream fixes to the *meshers* diff-port cleanly; upstream
// changes to the *packed vertex formats* must instead be re-mapped here (and
// in the EM-3.2 `bevy::Mesh` conversion) by hand.
//
// ## Packed → portable mapping (bit layouts quoted from the frozen reference)
//
// ### `TerrainVertex` ← voxygen/src/render/pipelines/terrain.rs `Vertex`
//
// Terrain packing (`Vertex::new`):
//
//     pos_norm:  u32 = x:6b[0..6) | y:6b[6..12)
//                    | (z+32768 clamped u16):16b[12..28)
//                    | meta:1b[28] | norm:3b[29..32)
//                      (norm code: 0=-x, 1=+x, 2=-y, 3=+y, 4=-z, 5=+z)
//     atlas_pos: u32 = x:16b[0..16) | y:16b[16..32)
//
// Figure packing (`Vertex::new_figure`):
//
//     pos_norm:  u32 = (x*2+256):9b[0..9) | (y*2+256):9b[9..18)
//                    | (z*2+256):9b[18..27)
//                    | bone_idx:4b[27..31) | norm_is_positive:1b[31]
//     atlas_pos: u32 = norm_axis:2b[0..2) | x:15b[2..17) | y:15b[17..32)
//
// Portable: `pos: Vec3<f32>` (unquantized), `norm: Vec3<f32>` (signed unit
// axis — carries both the axis and sign bits), `atlas_pos: Vec2<u16>`,
// `meta: bool` (terrain: touching water), `bone_idx: u8` (figures; 0 for
// terrain).
//
// ### `ColLight` ← `Vertex::make_col_light` texel (`[u8; 4]`)
//
//     [0] = light:5b[3..8) | col.r low bits:3b[0..3)
//     [1] = glow:5b[3..8)  | col.b low bits:3b[0..3)
//     [2] = col.r hi nibble | col.b hi nibble
//     [3] = col.g:7b[1..8) | ao:1b[0]
//
// Portable: `light: u8` (0..=31), `glow: u8` (0..=31), `col: Rgb<u8>`
// (full 8-bit, no bit-stealing), `ao: bool` (upstream thresholds the averaged
// 0.0..=1.0 AO at > 0.7 before packing — see greedy.rs `draw_texels`).
//
// ### `FigureColLight` ← `Vertex::make_col_light_figure` texel (`[u8; 4]`)
//
//     [0] = light:5b | col.r low:3b; [1] = surf:5b | col.b low:3b;
//     [2] = col.r hi | col.b hi;     [3] = col.g (unscathed)
//
// Portable: `light: u8` (0..=31), `col: Rgb<u8>`, `surf: CellSurface`.
//
// ### `FluidVertex` ← voxygen/src/render/pipelines/fluid.rs `Vertex`
//
//     pos_norm: u32 = x:6b[0..6) | y:6b[6..12) | (z+65536 clamped):17b[12..29)
//                   | norm:3b[29..32) (code = axis<<1 | positive)
//     vel:      u32 = (vel.x*1000+32768.9) as u16
//                   | ((vel.y*1000+32768.9) as u16) << 16
//
// Portable: `pos: Vec3<f32>`, `norm: Vec3<f32>`, `river_velocity: Vec2<f32>`.
//
// ### `SpriteVertex` ← voxygen/src/render/pipelines/sprite.rs `Vertex`
//
//     pos_norm:  u32 = (x+128):8b[0..8) | (y+128):8b[8..16)
//                    | (z+128 clamp 12b):12b[16..28)
//                    | norm:3b[29..32) (same code as terrain)
//     atlas_pos: u32 = x:16b | y:16b
//
// Portable: `pos: Vec3<f32>`, `norm: Vec3<f32>`, `atlas_pos: Vec2<u16>`.
//
// ### `ParticleVertex` ← voxygen/src/render/pipelines/particle.rs `Vertex`
//
//     pos: [f32; 3] (already unpacked) + norm_ao: u32 = norm:3b (same code)
//
// Portable: `pos: Vec3<f32>`, `norm: Vec3<f32>`.
//
// ## Trait adaptations
// - `Vertex` (voxygen/src/render/mod.rs:66): dropped the `bytemuck::Pod` bound
//   and the wgpu `STRIDE` const (both GPU-upload concerns — EM-3.2 owns the
//   `bevy::Mesh` attribute layout). Kept `QUADS_INDEX` (the meshers branch on
//   it for 4-vs-6 vertices per quad); `wgpu::IndexFormat` → local
//   `IndexFormat`.
// - `AtlasData` (voxygen/src/render/pipelines/mod.rs:342): kept `TEXTURES`,
//   `SliceMut`, `blank_with_size`, `slice_mut` (all the meshers use); dropped
//   `as_texture_data`/`layout`/`create_textures` (wgpu texture creation). This
//   also drops voxygen's `generic_const_exprs` requirement.
// - `AltIndices` lifted verbatim from voxygen/src/render/renderer/mod.rs:1667.

use common::figure::CellSurface;
use vek::*;

/// Portable stand-in for `wgpu::IndexFormat` (only used to signal whether a
/// vertex type is drawn through the shared quad index buffer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexFormat {
    Uint16,
    Uint32,
}

/// Portable stand-in for voxygen's `render::Vertex` trait.
pub trait Vertex: Clone + Copy {
    // Whether these types of verts use the quad index buffer for drawing them
    const QUADS_INDEX: Option<IndexFormat>;
}

/// Portable terrain/figure vertex (see module docs for the packed layout this
/// replaces). Produced by `create_opaque` closures in terrain.rs/segment.rs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainVertex {
    pub pos: Vec3<f32>,
    /// Signed unit axis normal (greedy meshing only ever emits axis-aligned
    /// faces).
    pub norm: Vec3<f32>,
    pub atlas_pos: Vec2<u16>,
    /// NOTE: meta is true when the terrain vertex is touching water.
    pub meta: bool,
    /// Figure bone index in [0, 15]; 0 for terrain vertices.
    pub bone_idx: u8,
}

impl TerrainVertex {
    /// NOTE: meta is true when the terrain vertex is touching water.
    pub fn new(atlas_pos: Vec2<u16>, pos: Vec3<f32>, norm: Vec3<f32>, meta: bool) -> Self {
        Self {
            pos,
            norm,
            atlas_pos,
            meta,
            bone_idx: 0,
        }
    }

    pub fn new_figure(atlas_pos: Vec2<u16>, pos: Vec3<f32>, norm: Vec3<f32>, bone_idx: u8) -> Self {
        Self {
            pos,
            norm,
            atlas_pos,
            meta: false,
            bone_idx,
        }
    }

    /// Portable equivalent of the upstream `[u8; 4]` col-light texel packer.
    /// Keeps upstream's clamping semantics (light/glow saturate at 31).
    pub fn make_col_light(
        // 0 to 31
        light: u8,
        // 0 to 31
        glow: u8,
        col: Rgb<u8>,
        ao: bool,
    ) -> ColLight {
        ColLight {
            light: light.min(31),
            glow: glow.min(31),
            col,
            ao,
        }
    }

    /// Portable equivalent of the upstream figure/sprite col-light packer.
    pub fn make_col_light_figure(
        // 0 to 31
        light: u8,
        col: Rgb<u8>,
        surf: CellSurface,
    ) -> FigureColLight {
        debug_assert!((surf as u8) < 32);
        FigureColLight {
            light: light.min(31),
            col,
            surf,
        }
    }

    /// Set the bone_idx for an existing figure vertex.
    pub fn set_bone_idx(&mut self, bone_idx: u8) { self.bone_idx = bone_idx & 0xF; }
}

impl Vertex for TerrainVertex {
    const QUADS_INDEX: Option<IndexFormat> = Some(IndexFormat::Uint32);
}

/// Portable fluid (water) vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FluidVertex {
    pub pos: Vec3<f32>,
    pub norm: Vec3<f32>,
    pub river_velocity: Vec2<f32>,
}

impl FluidVertex {
    pub fn new(pos: Vec3<f32>, norm: Vec3<f32>, river_velocity: Vec2<f32>) -> Self {
        Self {
            pos,
            norm,
            river_velocity,
        }
    }
}

impl Vertex for FluidVertex {
    const QUADS_INDEX: Option<IndexFormat> = Some(IndexFormat::Uint16);
}

/// Portable sprite vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteVertex {
    pub pos: Vec3<f32>,
    pub norm: Vec3<f32>,
    pub atlas_pos: Vec2<u16>,
}

impl SpriteVertex {
    // NOTE: Limit to 16 (x) × 16 (y) × 32 (z).
    pub fn new(atlas_pos: Vec2<u16>, pos: Vec3<f32>, norm: Vec3<f32>) -> Self {
        Self {
            pos,
            norm,
            atlas_pos,
        }
    }
}

impl Default for SpriteVertex {
    fn default() -> Self { Self::new(Vec2::zero(), Vec3::zero(), Vec3::zero()) }
}

impl Vertex for SpriteVertex {
    const QUADS_INDEX: Option<IndexFormat> = Some(IndexFormat::Uint16);
}

/// Portable particle vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleVertex {
    pub pos: Vec3<f32>,
    pub norm: Vec3<f32>,
}

impl ParticleVertex {
    pub fn new(pos: Vec3<f32>, norm: Vec3<f32>) -> Self { Self { pos, norm } }
}

impl Vertex for ParticleVertex {
    const QUADS_INDEX: Option<IndexFormat> = Some(IndexFormat::Uint16);
}

/// Portable terrain atlas texel (upstream: packed `[u8; 4]`, see module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColLight {
    /// Sunlight, 0..=31.
    pub light: u8,
    /// Glow (non-sun light), 0..=31.
    pub glow: u8,
    pub col: Rgb<u8>,
    /// Baked vertex-ish AO flag: upstream averages a 0.0..=1.0 AO over the 4
    /// blocks sharing the texel corner and thresholds it at > 0.7.
    pub ao: bool,
}

/// Portable figure/sprite atlas texel (upstream: packed `[u8; 4]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FigureColLight {
    /// Light, 0..=31.
    pub light: u8,
    pub col: Rgb<u8>,
    pub surf: CellSurface,
}

/// A trait implemented by texture atlas groups.
///
/// Terrain, figures, sprites, etc. all use texture atlases but have different
/// requirements, such as that layers provided by each atlas. This trait
/// abstracts over these cases.
///
/// Portable subset of voxygen's `AtlasData` — only what the meshers consume;
/// GPU texture creation is EM-3.2/EM-3.3 territory.
pub trait AtlasData {
    /// The number of texture channels that this atlas has.
    const TEXTURES: usize;
    /// Abstracts over a slice into the texture data, as returned by
    /// [`AtlasData::slice_mut`].
    type SliceMut<'a>: Iterator
    where
        Self: 'a;

    /// Return blank atlas data upon which texels can be applied.
    fn blank_with_size(sz: Vec2<u16>) -> Self;

    /// Take a sub-slice of the texture data for each layer in the atlas.
    fn slice_mut(&mut self, range: std::ops::Range<usize>) -> Self::SliceMut<'_>;
}

/// Represents texture that can be converted into texture atlases for terrain.
pub struct TerrainAtlasData {
    pub col_lights: Vec<ColLight>,
    pub kinds: Vec<u8>,
}

impl AtlasData for TerrainAtlasData {
    type SliceMut<'a> =
        std::iter::Zip<std::slice::IterMut<'a, ColLight>, std::slice::IterMut<'a, u8>>;

    const TEXTURES: usize = 2;

    fn blank_with_size(sz: Vec2<u16>) -> Self {
        let col_lights = vec![
            TerrainVertex::make_col_light(254, 0, Rgb::broadcast(254), true);
            sz.as_().product()
        ];
        let kinds = vec![0; sz.as_().product()];
        Self { col_lights, kinds }
    }

    fn slice_mut(&mut self, range: std::ops::Range<usize>) -> Self::SliceMut<'_> {
        self.col_lights[range.clone()]
            .iter_mut()
            .zip(self.kinds[range].iter_mut())
    }
}

/// Represents texture data for figures and sprites.
pub struct FigureSpriteAtlasData {
    pub col_lights: Vec<FigureColLight>,
}

impl AtlasData for FigureSpriteAtlasData {
    type SliceMut<'a> = std::slice::IterMut<'a, FigureColLight>;

    const TEXTURES: usize = 1;

    fn blank_with_size(sz: Vec2<u16>) -> Self {
        let col_lights =
            vec![
                TerrainVertex::make_col_light_figure(254, Rgb::broadcast(254), CellSurface::Matte);
                sz.as_().product()
            ];
        Self { col_lights }
    }

    fn slice_mut(&mut self, range: std::ops::Range<usize>) -> Self::SliceMut<'_> {
        self.col_lights[range].iter_mut()
    }
}

/// Lifted from voxygen/src/render/renderer/mod.rs:1667 — vertex-range markers
/// separating deep/underground/surface quads for culling.
#[derive(Clone, Copy, Debug, Default)]
pub struct AltIndices {
    pub deep_end: usize,
    pub underground_end: usize,
}
