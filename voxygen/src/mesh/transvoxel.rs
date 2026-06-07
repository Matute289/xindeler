//! Transvoxel terrain meshing algorithm (Eric Lengyel, 2010).
//!
//! Converts a `DensityField` into smooth triangle meshes by marching over every
//! 2×2×2 cell of the field and generating interpolated iso-surface triangles.
//!
//! Lookup tables ported from the public-domain C++ reference:
//!   <https://transvoxel.org/Transvoxel.cpp>
//!
//! # Table population (Task 8)
//!
//! `REGULAR_VERTEX_DATA` currently holds placeholder zeros — the algorithm will
//! produce no interior triangles until it is populated from Lengyel's
//! reference. `all_empty` and `all_solid` tests pass regardless because those
//! cases are short-circuited (case 0 and case 255 both skip triangle
//! generation).

use common::terrain::density::DensityField;
use vek::*;

/// Default iso-surface threshold: voxels with density > `THRESHOLD` are
/// considered solid.
pub const THRESHOLD: u8 = 127;

// ---------------------------------------------------------------------------
// Lookup table types
// ---------------------------------------------------------------------------

/// Per-cell geometry: how many vertices and triangles, and the vertex indices.
/// `counts >> 4` = vertex count; `counts & 0xF` = triangle count.
/// `indices[..triangle_count * 3]` are the vertex indices for each triangle.
#[derive(Clone, Copy)]
struct RegularCellData {
    counts: u8,
    indices: [u8; 15],
}

impl RegularCellData {
    const fn vertex_count(self) -> usize { (self.counts >> 4) as usize }

    const fn triangle_count(self) -> usize { (self.counts & 0x0F) as usize }
}

// ---------------------------------------------------------------------------
// REGULAR_CELL_CLASS[256]
//
// Maps each 8-bit corner-occupancy case to an equivalence class index.
// The high bit (0x80) means the normal should be flipped (complement
// orientation). `class_index = value & 0x0F`, `flip_normal = (value & 0x80) !=
// 0`.
//
// Source: regularCellClass[256] in Transvoxel.cpp (public domain).
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const REGULAR_CELL_CLASS: [u8; 256] = [
    0x00, 0x01, 0x01, 0x03, 0x01, 0x03, 0x02, 0x04,
    0x01, 0x02, 0x03, 0x05, 0x03, 0x05, 0x04, 0x06,
    0x01, 0x03, 0x02, 0x05, 0x02, 0x05, 0x06, 0x0B,
    0x03, 0x05, 0x04, 0x09, 0x05, 0x0A, 0x07, 0x0D,
    0x01, 0x02, 0x03, 0x05, 0x03, 0x04, 0x05, 0x09,
    0x02, 0x06, 0x05, 0x0B, 0x04, 0x07, 0x09, 0x0D,
    0x03, 0x05, 0x05, 0x0A, 0x04, 0x09, 0x07, 0x0E,
    0x05, 0x0B, 0x09, 0x0C, 0x07, 0x0D, 0x0E, 0x00,
    0x01, 0x03, 0x02, 0x05, 0x02, 0x05, 0x06, 0x0B,
    0x03, 0x04, 0x05, 0x09, 0x05, 0x07, 0x0B, 0x0D,
    0x02, 0x05, 0x06, 0x0B, 0x06, 0x0B, 0x08, 0x0F,
    0x05, 0x09, 0x0B, 0x0C, 0x0B, 0x0D, 0x0F, 0x00,
    0x03, 0x05, 0x05, 0x0A, 0x05, 0x09, 0x0B, 0x0E,
    0x04, 0x07, 0x09, 0x0E, 0x09, 0x0E, 0x0C, 0x00,
    0x05, 0x0A, 0x0B, 0x0C, 0x0B, 0x0E, 0x0F, 0x00,
    0x09, 0x0E, 0x0C, 0x00, 0x0E, 0x00, 0x00, 0x00,
    // Complement cases (128-255): same class as (255-case), with flip bit set.
    0x00, 0x8E, 0x8E, 0x8D, 0x8E, 0x8D, 0x8C, 0x8B,
    0x8E, 0x8C, 0x8D, 0x88, 0x8D, 0x88, 0x8B, 0x87,
    0x8E, 0x8D, 0x8C, 0x88, 0x8C, 0x88, 0x87, 0x86,
    0x8D, 0x88, 0x8B, 0x85, 0x88, 0x84, 0x86, 0x83,
    0x8E, 0x8C, 0x8D, 0x88, 0x8D, 0x8B, 0x88, 0x85,
    0x8C, 0x87, 0x88, 0x86, 0x8B, 0x86, 0x85, 0x83,
    0x8D, 0x88, 0x88, 0x84, 0x8B, 0x85, 0x86, 0x82,
    0x88, 0x86, 0x85, 0x81, 0x86, 0x83, 0x82, 0x8E,
    0x8E, 0x8D, 0x8C, 0x88, 0x8C, 0x88, 0x87, 0x86,
    0x8D, 0x8B, 0x88, 0x85, 0x88, 0x86, 0x86, 0x83,
    0x8C, 0x88, 0x87, 0x86, 0x87, 0x86, 0x85, 0x82,
    0x88, 0x85, 0x86, 0x81, 0x86, 0x83, 0x82, 0x8E,
    0x8D, 0x88, 0x88, 0x84, 0x88, 0x85, 0x86, 0x82,
    0x8B, 0x86, 0x85, 0x82, 0x85, 0x82, 0x81, 0x8E,
    0x88, 0x84, 0x86, 0x81, 0x86, 0x82, 0x82, 0x8E,
    0x85, 0x82, 0x81, 0x8E, 0x82, 0x8E, 0x8E, 0x00,
];

// ---------------------------------------------------------------------------
// REGULAR_CELL_DATA[16]
//
// Geometry for each of the 16 regular-cell equivalence classes.
// Source: regularCellData[16] in Transvoxel.cpp (public domain).
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const REGULAR_CELL_DATA: [RegularCellData; 16] = [
    // Class 0: 0 verts, 0 tris — fully empty or fully solid cell
    RegularCellData { counts: 0x00, indices: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 1: 3 verts, 1 tri
    RegularCellData { counts: 0x31, indices: [0, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 2: 4 verts, 2 tris
    RegularCellData { counts: 0x42, indices: [0, 1, 2, 0, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 3: 4 verts, 2 tris
    RegularCellData { counts: 0x42, indices: [0, 1, 2, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 4: 4 verts, 2 tris (bowtie / tunnel — ambiguous)
    RegularCellData { counts: 0x42, indices: [0, 1, 3, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    // Class 5: 5 verts, 3 tris
    RegularCellData { counts: 0x53, indices: [0, 1, 2, 0, 2, 3, 2, 4, 3, 0, 0, 0, 0, 0, 0] },
    // Class 6: 5 verts, 3 tris
    RegularCellData { counts: 0x53, indices: [0, 1, 4, 1, 3, 4, 1, 2, 3, 0, 0, 0, 0, 0, 0] },
    // Class 7: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 0, 2, 3, 4, 5, 0, 4, 0, 3, 0, 0, 0] },
    // Class 8: 5 verts, 3 tris
    RegularCellData { counts: 0x53, indices: [0, 1, 2, 3, 4, 0, 3, 0, 2, 0, 0, 0, 0, 0, 0] },
    // Class 9: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2, 0, 0, 0] },
    // Class A: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 3, 4, 5, 0, 2, 5, 0, 5, 3, 0, 0, 0] },
    // Class B: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 4, 0, 4, 5, 1, 2, 3, 1, 3, 4, 0, 0, 0] },
    // Class C: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 3, 2, 0, 1, 3, 4, 5, 1, 4, 1, 0, 0, 0, 0] },
    // Class D: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 2, 3, 5, 4, 0, 4, 1, 1, 4, 5, 0, 0, 0] },
    // Class E: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 4, 5, 0, 3, 4, 1, 2, 5, 1, 5, 4, 0, 0, 0] },
    // Class F: 6 verts, 4 tris
    RegularCellData { counts: 0x64, indices: [0, 1, 5, 1, 4, 5, 1, 2, 4, 2, 3, 4, 0, 0, 0] },
];

// ---------------------------------------------------------------------------
// REGULAR_VERTEX_DATA[256][12]
//
// For each of the 256 corner cases, up to 12 vertex descriptors.
// Each u16 encodes:
//   bits 15-8: reuse / direction flags (ignored — we always emit new vertices)
//   bits  7-4: first corner index (0-7)  ← decode as (v >> 4) & 0xF
//   bits  3-0: second corner index (0-7) ← decode as v & 0xF
//
// Source: regularVertexData[256][12] in Transvoxel.cpp (public domain,
// https://github.com/EricLengyel/Transvoxel/blob/main/Transvoxel.cpp).
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const REGULAR_VERTEX_DATA: [[u16; 12]; 256] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x3304, 0x2315, 0x4113, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x1326, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x6201, 0x4223, 0x1326, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x1326, 0x3304, 0x2315, 0x4113, 0, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x8337, 0x4223, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x8337, 0x4223, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x3304, 0x2315, 0x8337, 0x4223, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4113, 0x8337, 0x1326, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x8337, 0x1326, 0x3304, 0x6201, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x8337, 0x1326, 0x5102, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x2315, 0x8337, 0x1326, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x1146, 0x2245, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x4113, 0x5102, 0x1146, 0x2245, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x1326, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0, 0],
    [0x1146, 0x2245, 0x6201, 0x4223, 0x1326, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x2245, 0x6201, 0x2315, 0x4113, 0x5102, 0x4223, 0x1326, 0, 0, 0],
    [0x4223, 0x1326, 0x1146, 0x2245, 0x2315, 0x4113, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x4113, 0x8337, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x1146, 0x2245, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0, 0],
    [0x4223, 0x6201, 0x2315, 0x8337, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0],
    [0x4223, 0x8337, 0x2315, 0x2245, 0x1146, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4113, 0x8337, 0x1326, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0],
    [0x4113, 0x8337, 0x1326, 0x1146, 0x2245, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x8337, 0x1326, 0x5102, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0],
    [0x2245, 0x2315, 0x8337, 0x1326, 0x1146, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x6201, 0x2245, 0x8157, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x2245, 0x8157, 0x4113, 0x5102, 0x3304, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x1326, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4223, 0x1326, 0x3304, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0],
    [0x6201, 0x2245, 0x8157, 0x4113, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0, 0],
    [0x4223, 0x1326, 0x3304, 0x2245, 0x8157, 0x4113, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x4113, 0x8337, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x4223, 0x4113, 0x8337, 0x2315, 0x2245, 0x8157, 0, 0, 0],
    [0x8337, 0x4223, 0x6201, 0x2245, 0x8157, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x3304, 0x2245, 0x8157, 0x8337, 0x4223, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4113, 0x8337, 0x1326, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0],
    [0x4113, 0x8337, 0x1326, 0x3304, 0x6201, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x8337, 0x8157, 0x2245, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x8157, 0x8337, 0x1326, 0x3304, 0x2245, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x3304, 0x1146, 0x8157, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x1146, 0x8157, 0x2315, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x8157, 0x4113, 0x6201, 0, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x5102, 0x1146, 0x8157, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x3304, 0x1146, 0x8157, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0, 0],
    [0x1326, 0x4223, 0x6201, 0x2315, 0x8157, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x8157, 0x4113, 0x6201, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0],
    [0x1326, 0x1146, 0x8157, 0x4113, 0x4223, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x3304, 0x1146, 0x8157, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x1146, 0x8157, 0x2315, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x8157, 0x8337, 0x4223, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x5102, 0x1146, 0x8157, 0x8337, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x3304, 0x1146, 0x8157, 0x5102, 0x4113, 0x8337, 0x1326, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x8337, 0x1326, 0x1146, 0x8157, 0x2315, 0, 0, 0, 0, 0],
    [0x6201, 0x3304, 0x1146, 0x8157, 0x8337, 0x1326, 0x5102, 0, 0, 0, 0, 0],
    [0x1326, 0x1146, 0x8157, 0x8337, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x3304, 0x2315, 0x4113, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x6201, 0x4223, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x8267, 0x1146, 0x6201, 0x2315, 0x4113, 0, 0, 0, 0, 0],
    [0x1146, 0x8267, 0x4223, 0x4113, 0x2315, 0x3304, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x8337, 0x4223, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x4223, 0x4113, 0x8337, 0x1326, 0x8267, 0x1146, 0, 0, 0],
    [0x6201, 0x2315, 0x8337, 0x4223, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0],
    [0x5102, 0x3304, 0x2315, 0x8337, 0x4223, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0],
    [0x8267, 0x1146, 0x5102, 0x4113, 0x8337, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x8337, 0x8267, 0x1146, 0x3304, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x8337, 0x8267, 0x1146, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x1146, 0x3304, 0x2315, 0x8337, 0x8267, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1326, 0x8267, 0x2245, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x8267, 0x2245, 0x6201, 0x5102, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1326, 0x8267, 0x2245, 0x6201, 0x2315, 0x4113, 0, 0, 0, 0, 0],
    [0x1326, 0x8267, 0x2245, 0x2315, 0x4113, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x8267, 0x2245, 0x3304, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4223, 0x8267, 0x2245, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x8267, 0x2245, 0x3304, 0x6201, 0x2315, 0x4113, 0, 0, 0, 0],
    [0x4113, 0x4223, 0x8267, 0x2245, 0x2315, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1326, 0x8267, 0x2245, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0, 0],
    [0x1326, 0x8267, 0x2245, 0x6201, 0x5102, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0],
    [0x3304, 0x1326, 0x8267, 0x2245, 0x4223, 0x6201, 0x2315, 0x8337, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x8267, 0x2245, 0x2315, 0x8337, 0x4223, 0, 0, 0, 0, 0],
    [0x3304, 0x2245, 0x8267, 0x8337, 0x4113, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x8337, 0x8267, 0x2245, 0x6201, 0x4113, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x6201, 0x2315, 0x8337, 0x8267, 0x2245, 0x3304, 0, 0, 0, 0, 0],
    [0x2315, 0x8337, 0x8267, 0x2245, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8157, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x2315, 0x2245, 0x8157, 0x1326, 0x8267, 0x1146, 0, 0, 0],
    [0x6201, 0x2245, 0x8157, 0x4113, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0, 0],
    [0x2245, 0x8157, 0x4113, 0x5102, 0x3304, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0],
    [0x4223, 0x8267, 0x1146, 0x5102, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0, 0],
    [0x3304, 0x6201, 0x4223, 0x8267, 0x1146, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0],
    [0x4223, 0x8267, 0x1146, 0x5102, 0x6201, 0x2245, 0x8157, 0x4113, 0, 0, 0, 0],
    [0x3304, 0x2245, 0x8157, 0x4113, 0x4223, 0x8267, 0x1146, 0, 0, 0, 0, 0],
    [0x4223, 0x4113, 0x8337, 0x2315, 0x2245, 0x8157, 0x1326, 0x8267, 0x1146, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x4223, 0x4113, 0x8337, 0x2315, 0x2245, 0x8157, 0x1326, 0x8267, 0x1146],
    [0x8337, 0x4223, 0x6201, 0x2245, 0x8157, 0x1326, 0x8267, 0x1146, 0, 0, 0, 0],
    [0x4223, 0x5102, 0x3304, 0x2245, 0x8157, 0x8337, 0x1326, 0x8267, 0x1146, 0, 0, 0],
    [0x8267, 0x1146, 0x5102, 0x4113, 0x8337, 0x2315, 0x2245, 0x8157, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x8337, 0x8267, 0x1146, 0x3304, 0x2315, 0x2245, 0x8157, 0, 0, 0],
    [0x8337, 0x8267, 0x1146, 0x5102, 0x6201, 0x2245, 0x8157, 0, 0, 0, 0, 0],
    [0x3304, 0x2245, 0x8157, 0x8337, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x8157, 0x2315, 0x3304, 0x1326, 0x8267, 0, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x8157, 0x2315, 0x6201, 0x5102, 0x1326, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x1326, 0x3304, 0x6201, 0x4113, 0x8157, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x8157, 0x4113, 0x5102, 0x1326, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x8267, 0x8157, 0x2315, 0x3304, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x6201, 0x4223, 0x8267, 0x8157, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x5102, 0x4223, 0x8267, 0x8157, 0x4113, 0x6201, 0, 0, 0, 0, 0],
    [0x4113, 0x4223, 0x8267, 0x8157, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x8157, 0x2315, 0x3304, 0x1326, 0x8267, 0x4223, 0x4113, 0x8337, 0, 0, 0, 0],
    [0x8157, 0x2315, 0x6201, 0x5102, 0x1326, 0x8267, 0x4223, 0x4113, 0x8337, 0, 0, 0],
    [0x8157, 0x8337, 0x4223, 0x6201, 0x3304, 0x1326, 0x8267, 0, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x8267, 0x8157, 0x8337, 0x4223, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x8157, 0x2315, 0x3304, 0x5102, 0x4113, 0x8337, 0, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x8337, 0x8267, 0x8157, 0x2315, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x3304, 0x5102, 0x8337, 0x8267, 0x8157, 0, 0, 0, 0, 0, 0],
    [0x8337, 0x8267, 0x8157, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x3304, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x3304, 0x2315, 0x4113, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x1326, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4223, 0x1326, 0x3304, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x5102, 0x4223, 0x1326, 0x8337, 0x8157, 0x8267, 0, 0, 0],
    [0x4223, 0x1326, 0x3304, 0x2315, 0x4113, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0],
    [0x4113, 0x8157, 0x8267, 0x4223, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x4113, 0x8157, 0x8267, 0x6201, 0x5102, 0x3304, 0, 0, 0, 0, 0],
    [0x8157, 0x8267, 0x4223, 0x6201, 0x2315, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x2315, 0x8157, 0x8267, 0x4223, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x5102, 0x4113, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0, 0],
    [0x8157, 0x4113, 0x6201, 0x3304, 0x1326, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x5102, 0x6201, 0x2315, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x1326, 0x3304, 0x2315, 0x8157, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x2245, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x1146, 0x2245, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x3304, 0x1146, 0x2245, 0x8337, 0x8157, 0x8267, 0, 0, 0],
    [0x2315, 0x4113, 0x5102, 0x1146, 0x2245, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0],
    [0x5102, 0x4223, 0x1326, 0x3304, 0x1146, 0x2245, 0x8337, 0x8157, 0x8267, 0, 0, 0],
    [0x1146, 0x2245, 0x6201, 0x4223, 0x1326, 0x8337, 0x8157, 0x8267, 0, 0, 0, 0],
    [0x6201, 0x2315, 0x4113, 0x5102, 0x4223, 0x1326, 0x3304, 0x1146, 0x2245, 0x8337, 0x8157, 0x8267],
    [0x4113, 0x4223, 0x1326, 0x1146, 0x2245, 0x2315, 0x8337, 0x8157, 0x8267, 0, 0, 0],
    [0x4223, 0x4113, 0x8157, 0x8267, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0, 0],
    [0x6201, 0x5102, 0x1146, 0x2245, 0x4223, 0x4113, 0x8157, 0x8267, 0, 0, 0, 0],
    [0x8157, 0x8267, 0x4223, 0x6201, 0x2315, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0],
    [0x2315, 0x8157, 0x8267, 0x4223, 0x5102, 0x1146, 0x2245, 0, 0, 0, 0, 0],
    [0x1326, 0x5102, 0x4113, 0x8157, 0x8267, 0x3304, 0x1146, 0x2245, 0, 0, 0, 0],
    [0x1326, 0x1146, 0x2245, 0x6201, 0x4113, 0x8157, 0x8267, 0, 0, 0, 0, 0],
    [0x5102, 0x6201, 0x2315, 0x8157, 0x8267, 0x1326, 0x3304, 0x1146, 0x2245, 0, 0, 0],
    [0x1326, 0x1146, 0x2245, 0x2315, 0x8157, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8267, 0x8337, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8267, 0x8337, 0x6201, 0x5102, 0x3304, 0, 0, 0, 0, 0],
    [0x4113, 0x6201, 0x2245, 0x8267, 0x8337, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4113, 0x8337, 0x8267, 0x2245, 0x3304, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8267, 0x8337, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0, 0],
    [0x6201, 0x4223, 0x1326, 0x3304, 0x8337, 0x2315, 0x2245, 0x8267, 0, 0, 0, 0],
    [0x4113, 0x6201, 0x2245, 0x8267, 0x8337, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0],
    [0x4113, 0x4223, 0x1326, 0x3304, 0x2245, 0x8267, 0x8337, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8267, 0x4223, 0x4113, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x2245, 0x8267, 0x4223, 0x4113, 0x6201, 0x5102, 0x3304, 0, 0, 0, 0],
    [0x6201, 0x2245, 0x8267, 0x4223, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x2245, 0x8267, 0x4223, 0x5102, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4113, 0x2315, 0x2245, 0x8267, 0x1326, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x2315, 0x2245, 0x8267, 0x1326, 0x3304, 0x6201, 0, 0, 0, 0, 0],
    [0x5102, 0x6201, 0x2245, 0x8267, 0x1326, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x2245, 0x8267, 0x1326, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x8337, 0x2315, 0x3304, 0x1146, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x1146, 0x8267, 0x8337, 0x2315, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1146, 0x8267, 0x8337, 0x4113, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x8337, 0x4113, 0x5102, 0x1146, 0x8267, 0, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x8337, 0x2315, 0x3304, 0x1146, 0x5102, 0x4223, 0x1326, 0, 0, 0, 0],
    [0x1146, 0x8267, 0x8337, 0x2315, 0x6201, 0x4223, 0x1326, 0, 0, 0, 0, 0],
    [0x8267, 0x8337, 0x4113, 0x6201, 0x3304, 0x1146, 0x5102, 0x4223, 0x1326, 0, 0, 0],
    [0x4113, 0x4223, 0x1326, 0x1146, 0x8267, 0x8337, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x2315, 0x4113, 0x4223, 0x8267, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x6201, 0x5102, 0x1146, 0x8267, 0x4223, 0x4113, 0, 0, 0, 0, 0],
    [0x1146, 0x8267, 0x4223, 0x6201, 0x3304, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x1146, 0x8267, 0x4223, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x8267, 0x1326, 0x5102, 0x4113, 0x2315, 0x3304, 0x1146, 0, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x2315, 0x1326, 0x1146, 0x8267, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x3304, 0x1146, 0x8267, 0x1326, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x1146, 0x8267, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x8337, 0x8157, 0x1146, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x8337, 0x8157, 0x1146, 0x1326, 0x6201, 0x5102, 0x3304, 0, 0, 0, 0, 0],
    [0x8337, 0x8157, 0x1146, 0x1326, 0x6201, 0x2315, 0x4113, 0, 0, 0, 0, 0],
    [0x4113, 0x5102, 0x3304, 0x2315, 0x1326, 0x8337, 0x8157, 0x1146, 0, 0, 0, 0],
    [0x8337, 0x8157, 0x1146, 0x5102, 0x4223, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4223, 0x8337, 0x8157, 0x1146, 0x3304, 0, 0, 0, 0, 0, 0],
    [0x8337, 0x8157, 0x1146, 0x5102, 0x4223, 0x6201, 0x2315, 0x4113, 0, 0, 0, 0],
    [0x4223, 0x8337, 0x8157, 0x1146, 0x3304, 0x2315, 0x4113, 0, 0, 0, 0, 0],
    [0x4223, 0x4113, 0x8157, 0x1146, 0x1326, 0, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x4113, 0x8157, 0x1146, 0x1326, 0x6201, 0x5102, 0x3304, 0, 0, 0, 0],
    [0x1146, 0x8157, 0x2315, 0x6201, 0x4223, 0x1326, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x5102, 0x3304, 0x2315, 0x8157, 0x1146, 0x1326, 0, 0, 0, 0, 0],
    [0x4113, 0x8157, 0x1146, 0x5102, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x8157, 0x1146, 0x3304, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x8157, 0x1146, 0x5102, 0x6201, 0, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x8157, 0x1146, 0x3304, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x2245, 0x3304, 0x1326, 0x8337, 0x8157, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2245, 0x8157, 0x8337, 0x1326, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x2245, 0x3304, 0x1326, 0x8337, 0x8157, 0x6201, 0x2315, 0x4113, 0, 0, 0, 0],
    [0x2245, 0x2315, 0x4113, 0x5102, 0x1326, 0x8337, 0x8157, 0, 0, 0, 0, 0],
    [0x4223, 0x8337, 0x8157, 0x2245, 0x3304, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x8157, 0x2245, 0x6201, 0x4223, 0x8337, 0, 0, 0, 0, 0, 0, 0],
    [0x2245, 0x3304, 0x5102, 0x4223, 0x8337, 0x8157, 0x4113, 0x6201, 0x2315, 0, 0, 0],
    [0x4223, 0x8337, 0x8157, 0x2245, 0x2315, 0x4113, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x8157, 0x2245, 0x3304, 0x1326, 0x4223, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x4223, 0x4113, 0x8157, 0x2245, 0x6201, 0x5102, 0, 0, 0, 0, 0],
    [0x8157, 0x2245, 0x3304, 0x1326, 0x4223, 0x6201, 0x2315, 0, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x4223, 0x2315, 0x8157, 0x2245, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x5102, 0x4113, 0x8157, 0x2245, 0, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x8157, 0x2245, 0x6201, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x6201, 0x2315, 0x8157, 0x2245, 0x3304, 0, 0, 0, 0, 0, 0],
    [0x2315, 0x8157, 0x2245, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x1146, 0x1326, 0x8337, 0x2315, 0x2245, 0, 0, 0, 0, 0, 0, 0],
    [0x1146, 0x1326, 0x8337, 0x2315, 0x2245, 0x6201, 0x5102, 0x3304, 0, 0, 0, 0],
    [0x6201, 0x2245, 0x1146, 0x1326, 0x8337, 0x4113, 0, 0, 0, 0, 0, 0],
    [0x2245, 0x1146, 0x1326, 0x8337, 0x4113, 0x5102, 0x3304, 0, 0, 0, 0, 0],
    [0x5102, 0x1146, 0x2245, 0x2315, 0x8337, 0x4223, 0, 0, 0, 0, 0, 0],
    [0x1146, 0x3304, 0x6201, 0x4223, 0x8337, 0x2315, 0x2245, 0, 0, 0, 0, 0],
    [0x8337, 0x4113, 0x6201, 0x2245, 0x1146, 0x5102, 0x4223, 0, 0, 0, 0, 0],
    [0x4223, 0x8337, 0x4113, 0x3304, 0x2245, 0x1146, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x2315, 0x2245, 0x1146, 0x1326, 0x4223, 0, 0, 0, 0, 0, 0],
    [0x1146, 0x1326, 0x4223, 0x4113, 0x2315, 0x2245, 0x6201, 0x5102, 0x3304, 0, 0, 0],
    [0x1326, 0x4223, 0x6201, 0x2245, 0x1146, 0, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x5102, 0x3304, 0x2245, 0x1146, 0x1326, 0, 0, 0, 0, 0, 0],
    [0x2245, 0x1146, 0x5102, 0x4113, 0x2315, 0, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x2315, 0x2245, 0x1146, 0x3304, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x2245, 0x1146, 0x5102, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x2245, 0x1146, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1326, 0x8337, 0x2315, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x8337, 0x2315, 0x6201, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x3304, 0x1326, 0x8337, 0x4113, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x8337, 0x4113, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x4223, 0x8337, 0x2315, 0x3304, 0x5102, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4223, 0x8337, 0x2315, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x5102, 0x4223, 0x8337, 0x4113, 0x6201, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x4223, 0x8337, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x4113, 0x2315, 0x3304, 0x1326, 0x4223, 0, 0, 0, 0, 0, 0, 0],
    [0x1326, 0x4223, 0x4113, 0x2315, 0x6201, 0x5102, 0, 0, 0, 0, 0, 0],
    [0x3304, 0x1326, 0x4223, 0x6201, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x1326, 0x4223, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x5102, 0x4113, 0x2315, 0x3304, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x4113, 0x2315, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x6201, 0x3304, 0x5102, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

// ---------------------------------------------------------------------------
// Corner offsets for a 2×2×2 cell.
// Bit i of the case index corresponds to CORNER_OFFSETS[i].
// ---------------------------------------------------------------------------

const CORNER_OFFSETS: [Vec3<i32>; 8] = [
    Vec3::new(0, 0, 0), // bit 0
    Vec3::new(1, 0, 0), // bit 1
    Vec3::new(0, 1, 0), // bit 2
    Vec3::new(1, 1, 0), // bit 3
    Vec3::new(0, 0, 1), // bit 4
    Vec3::new(1, 0, 1), // bit 5
    Vec3::new(0, 1, 1), // bit 6
    Vec3::new(1, 1, 1), // bit 7
];

// ---------------------------------------------------------------------------
// Output type
// ---------------------------------------------------------------------------

/// A single triangle generated by the Transvoxel algorithm.
/// Positions are in density-field-local block coordinates (float).
/// Normals point outward from the solid surface.
#[derive(Clone, Debug)]
pub struct TransvoxelTriangle {
    /// Vertex positions in field-local block coordinates.
    pub positions: [Vec3<f32>; 3],
    /// Per-vertex surface normals (gradient-based, outward-pointing).
    pub normals: [Vec3<f32>; 3],
    /// BlockKind as u8 per vertex.
    pub kinds: [u8; 3],
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Estimate the outward surface normal at `pos` via central-differences on the
/// density field. "Outward" means pointing from solid into air (away from the
/// terrain surface, toward the viewer).
///
/// The raw gradient points toward increasing density (i.e. into solid), so we
/// negate it to get the outward direction.
/// Trilinear interpolation of density at a fractional position.
///
/// Evaluating at the exact vertex position (not truncated to integer) gives
/// nonzero gradients for thin vertical walls (1-block thick) where integer
/// sampling gives d(W+1)==d(W-1) (both air) → gradient=0 → wrong upward normal.
fn sample_trilinear(field: &DensityField, pos: Vec3<f32>) -> f32 {
    let p0 = pos.map(|e| e.floor() as i32);
    let t = pos - pos.map(|e| e.floor());
    let d = |dx: i32, dy: i32, dz: i32| field.get_or_zero(p0 + Vec3::new(dx, dy, dz)) as f32;
    let c00 = d(0, 0, 0) * (1.0 - t.x) + d(1, 0, 0) * t.x;
    let c10 = d(0, 1, 0) * (1.0 - t.x) + d(1, 1, 0) * t.x;
    let c01 = d(0, 0, 1) * (1.0 - t.x) + d(1, 0, 1) * t.x;
    let c11 = d(0, 1, 1) * (1.0 - t.x) + d(1, 1, 1) * t.x;
    let c0 = c00 * (1.0 - t.y) + c10 * t.y;
    let c1 = c01 * (1.0 - t.y) + c11 * t.y;
    c0 * (1.0 - t.z) + c1 * t.z
}

fn density_gradient(field: &DensityField, pos: Vec3<f32>) -> Vec3<f32> {
    // Central-difference gradient evaluated at the exact fractional vertex
    // position via trilinear interpolation. This gives correct nonzero
    // horizontal gradients for thin walls (1-block thick) where integer
    // sampling would see equal air densities on both sides → gradient=0.
    let dx = sample_trilinear(field, pos + Vec3::new(1.0, 0.0, 0.0))
        - sample_trilinear(field, pos + Vec3::new(-1.0, 0.0, 0.0));
    let dy = sample_trilinear(field, pos + Vec3::new(0.0, 1.0, 0.0))
        - sample_trilinear(field, pos + Vec3::new(0.0, -1.0, 0.0));
    let dz = sample_trilinear(field, pos + Vec3::new(0.0, 0.0, 1.0))
        - sample_trilinear(field, pos + Vec3::new(0.0, 0.0, -1.0));
    let g = Vec3::new(dx, dy, dz);
    // Negate: raw gradient points into solid, we want outward (into air).
    // Fall back to up-vector when the gradient is degenerate.
    if g.magnitude_squared() < 0.001 {
        Vec3::unit_z()
    } else {
        (-g).normalized()
    }
}

// ---------------------------------------------------------------------------
// Main meshing function
// ---------------------------------------------------------------------------

/// Find the block kind at a fractional density-field position.
/// Samples the 8 surrounding integer voxels, returns the kind of the most
/// dense solid voxel among them. Falls back to Rock (0x10) if none are solid.
fn kind_at_vertex(field: &DensityField, pos: Vec3<f32>, threshold: u8) -> u8 {
    let base = pos.map(|e| e.floor() as i32);
    let mut best_kind = 0u8;
    let mut best_density = 0u8;
    for dz in 0..=1i32 {
        for dy in 0..=1i32 {
            for dx in 0..=1i32 {
                let p = base + Vec3::new(dx, dy, dz);
                let d = field.get_or_zero(p);
                if d > threshold && d > best_density {
                    best_density = d;
                    let k = field.get_kind_or_default(p);
                    if k != 0 {
                        best_kind = k;
                    }
                }
            }
        }
    }
    if best_kind == 0 {
        0x10 // Rock fallback
    } else {
        best_kind
    }
}

/// Run the Transvoxel algorithm over `field` and return all generated
/// triangles.
///
/// Triangle positions are in field-local block coordinates (0.0 to field.size).
/// An optional `field_offset` (world-space position of field corner) is NOT
/// applied here — callers add it when building GPU vertices.
///
/// # Note on lookup tables
/// Until `REGULAR_VERTEX_DATA` is populated (Task 8), this function generates
/// no triangles for interior cells (case 1–254). The smoke tests for case 0 and
/// case 255 still pass because those cases are short-circuited.
pub fn mesh_transvoxel(field: &DensityField, threshold: u8) -> Vec<TransvoxelTriangle> {
    let mut triangles = Vec::new();
    let size = field.size.map(|e| e as i32);

    // Start at index 1 (skip the outer padding layer) so the mesh stays
    // within the actual chunk boundary and doesn't generate floating geometry
    // from the ±1 border added during DensityField construction.
    for cx in 1..size.x - 1 {
        for cy in 1..size.y - 1 {
            for cz in 1..size.z - 1 {
                let cell = Vec3::new(cx, cy, cz);

                // Sample the 8 corner densities.
                let mut corners = [0u8; 8];
                for (i, &off) in CORNER_OFFSETS.iter().enumerate() {
                    corners[i] = field.get_or_zero(cell + off);
                }

                // Build the 8-bit case index.
                let mut case_idx: u8 = 0;
                for (i, &d) in corners.iter().enumerate() {
                    if d > threshold {
                        case_idx |= 1 << i;
                    }
                }

                // All-empty or all-solid cells have no surface crossing — skip.
                if case_idx == 0 || case_idx == 255 {
                    continue;
                }

                let class_raw = REGULAR_CELL_CLASS[case_idx as usize];
                let class_idx = (class_raw & 0x0F) as usize;
                let flip_normal = (class_raw & 0x80) != 0;

                let cell_data = REGULAR_CELL_DATA[class_idx];
                let vtx_count = cell_data.vertex_count();
                let tri_count = cell_data.triangle_count();

                // Generate vertex positions along the cell edges.
                let vertex_data = REGULAR_VERTEX_DATA[case_idx as usize];
                let mut vtx_pos = [Vec3::zero(); 12];
                let mut vtx_norm = [Vec3::unit_z(); 12];

                for v in 0..vtx_count {
                    let desc = vertex_data[v];
                    // Bits 7-4: first corner index; bits 3-0: second corner index.
                    // Bits 15-8 contain reuse/direction flags which we ignore
                    // (we always emit new vertices).
                    let c0 = ((desc >> 4) & 0x0F) as usize;
                    let c1 = (desc & 0x0F) as usize;

                    let p0 = (cell + CORNER_OFFSETS[c0]).map(|e| e as f32);
                    let p1 = (cell + CORNER_OFFSETS[c1]).map(|e| e as f32);
                    let d0 = corners[c0] as f32;
                    let d1 = corners[c1] as f32;

                    // Linear interpolation to the iso-surface crossing point.
                    let t = if (d1 - d0).abs() < 0.001 {
                        0.5
                    } else {
                        (threshold as f32 - d0) / (d1 - d0)
                    };

                    let pos = p0 + (p1 - p0) * t;
                    vtx_pos[v] = pos;
                    vtx_norm[v] = density_gradient(field, pos);
                }

                // Emit one triangle per group of 3 vertex indices.
                for t in 0..tri_count {
                    let i0 = cell_data.indices[t * 3] as usize;
                    let i1 = cell_data.indices[t * 3 + 1] as usize;
                    let i2 = cell_data.indices[t * 3 + 2] as usize;

                    // Flip winding order for complement (majority-solid) cases.
                    let (i0, i2) = if flip_normal { (i2, i0) } else { (i0, i2) };

                    let vtx_kinds: [u8; 3] = [
                        kind_at_vertex(field, vtx_pos[i0], threshold),
                        kind_at_vertex(field, vtx_pos[i1], threshold),
                        kind_at_vertex(field, vtx_pos[i2], threshold),
                    ];

                    triangles.push(TransvoxelTriangle {
                        positions: [vtx_pos[i0], vtx_pos[i1], vtx_pos[i2]],
                        normals: [vtx_norm[i0], vtx_norm[i1], vtx_norm[i2]],
                        kinds: vtx_kinds,
                    });
                }
            }
        }
    }

    triangles
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use common::terrain::density::{DensityField, smooth_density_field};

    #[test]
    fn all_empty_produces_no_triangles() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        assert!(mesh_transvoxel(&field, THRESHOLD).is_empty());
    }

    #[test]
    fn all_solid_produces_no_triangles() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        field.data.fill(255);
        assert!(mesh_transvoxel(&field, THRESHOLD).is_empty());
    }

    #[test]
    fn half_solid_produces_triangles() {
        let mut field = DensityField::new(Vec3::new(6, 6, 6));
        for x in 0..6i32 {
            for y in 0..6i32 {
                for z in 0..6i32 {
                    field.set(Vec3::new(x, y, z), if z < 3 { 255 } else { 0 });
                }
            }
        }
        smooth_density_field(&mut field, 1);
        let tris = mesh_transvoxel(&field, THRESHOLD);
        assert!(
            !tris.is_empty(),
            "expected triangles at the solid/air boundary"
        );
    }

    #[test]
    fn transvoxel_vertex_kind_is_rock_for_rock_chunk() {
        let mut field = DensityField::new(Vec3::new(6, 6, 6));
        // Fill bottom 3 z-slices as solid Rock (kind = 0x10)
        for x in 0..6i32 {
            for y in 0..6i32 {
                for z in 0..3i32 {
                    field.set(Vec3::new(x, y, z), 255);
                    field.set_kind(Vec3::new(x, y, z), 0x10); // Rock
                }
            }
        }
        smooth_density_field(&mut field, 1);
        let tris = mesh_transvoxel(&field, THRESHOLD);
        assert!(!tris.is_empty(), "expected triangles at solid/air boundary");
        // All vertices should have Rock kind (0x10) or fallback (also 0x10)
        for tri in &tris {
            for &k in &tri.kinds {
                assert!(k == 0x10 || k == 0, "unexpected kind {k:#x}");
            }
        }
    }
}
