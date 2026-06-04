use crate::{terrain::Block, vol::ReadVol};
use vek::Vec3;

/// A 3-D scalar field where 255 = fully solid, 0 = fully empty.
///
/// Indexing is row-major: `x * size.y * size.z + y * size.z + z`.
/// This is the shared representation used by both the Transvoxel visual mesher
/// and the smooth-collision physics extractor.
pub struct DensityField {
    pub data: Vec<u8>,
    pub size: Vec3<u32>,
}

impl DensityField {
    pub fn new(size: Vec3<u32>) -> Self {
        Self {
            data: vec![0u8; (size.x * size.y * size.z) as usize],
            size,
        }
    }

    #[inline]
    fn flat_index(&self, pos: Vec3<i32>) -> Option<usize> {
        if pos.x < 0
            || pos.y < 0
            || pos.z < 0
            || pos.x >= self.size.x as i32
            || pos.y >= self.size.y as i32
            || pos.z >= self.size.z as i32
        {
            return None;
        }
        Some(
            (pos.x as u32 * self.size.y * self.size.z + pos.y as u32 * self.size.z + pos.z as u32)
                as usize,
        )
    }

    pub fn get(&self, pos: Vec3<i32>) -> Option<u8> {
        self.flat_index(pos).and_then(|i| self.data.get(i).copied())
    }

    pub fn get_or_zero(&self, pos: Vec3<i32>) -> u8 { self.get(pos).unwrap_or(0) }

    pub fn set(&mut self, pos: Vec3<i32>, val: u8) {
        if let Some(i) = self.flat_index(pos) {
            self.data[i] = val;
        }
    }
}

/// Converts a voxel volume into a `DensityField`.
///
/// `offset` is the world-space position of the field's (0,0,0) corner.
/// `size` is how many voxels to sample in each axis.
///
/// Mapping:
/// - filled block → 255
/// - air / water / any non-filled block → 0
/// - out-of-bounds → 0
pub fn convert_chunk_to_density_field<V>(
    vol: &V,
    offset: Vec3<i32>,
    size: Vec3<u32>,
) -> DensityField
where
    V: ReadVol<Vox = Block>,
{
    let mut field = DensityField::new(size);
    for x in 0..size.x as i32 {
        for y in 0..size.y as i32 {
            for z in 0..size.z as i32 {
                let pos = Vec3::new(x, y, z);
                let val = match vol.get(offset + pos) {
                    Ok(block) if block.is_filled() => 255,
                    _ => 0,
                };
                field.set(pos, val);
            }
        }
    }
    field
}

/// Applies a 3×3×3 box-filter blur to a `DensityField` in-place.
///
/// Out-of-bounds neighbours are treated as density 0. Rounding is to nearest:
/// `(sum + 13) / 27`.
pub fn smooth_density_field(field: &mut DensityField) {
    let snapshot = field.data.clone();
    let snap = DensityField {
        data: snapshot,
        size: field.size,
    };

    for x in 0..field.size.x as i32 {
        for y in 0..field.size.y as i32 {
            for z in 0..field.size.z as i32 {
                let mut sum: u32 = 0;
                for dx in -1i32..=1 {
                    for dy in -1i32..=1 {
                        for dz in -1i32..=1 {
                            sum += snap.get_or_zero(Vec3::new(x + dx, y + dy, z + dz)) as u32;
                        }
                    }
                }
                let blended = ((sum + 13) / 27) as u8;
                field.set(Vec3::new(x, y, z), blended);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_field_get_set_roundtrip() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        let pos = Vec3::new(2, 1, 3);
        field.set(pos, 200);
        assert_eq!(field.get(pos), Some(200));
    }

    #[test]
    fn density_field_out_of_bounds_returns_none() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        assert_eq!(field.get(Vec3::new(-1, 0, 0)), None);
        assert_eq!(field.get(Vec3::new(4, 0, 0)), None);
    }

    #[test]
    fn density_field_get_or_zero_oob() {
        let field = DensityField::new(Vec3::new(4, 4, 4));
        assert_eq!(field.get_or_zero(Vec3::new(99, 99, 99)), 0);
    }

    #[test]
    fn density_field_new_all_zero() {
        let field = DensityField::new(Vec3::new(3, 3, 3));
        assert!(field.data.iter().all(|&v| v == 0));
    }

    #[test]
    fn density_field_set_oob_is_noop() {
        let mut field = DensityField::new(Vec3::new(4, 4, 4));
        field.set(Vec3::new(-1, 0, 0), 255); // must not panic
        assert!(field.data.iter().all(|&v| v == 0));
    }

    #[test]
    fn smooth_reduces_sharp_boundary() {
        // Solid half (x=0..3) vs air half (x=3..5) in a 3D field so all 27
        // neighbours are in-bounds for interior voxels.
        let mut field = DensityField::new(Vec3::new(5, 3, 3));
        for x in 0..3i32 {
            for y in 0..3i32 {
                for z in 0..3i32 {
                    field.set(Vec3::new(x, y, z), 255);
                }
            }
        }
        // x=3,4 stay 0

        smooth_density_field(&mut field);

        // Deep interior of solid region: all 27 neighbours are 255 → stays 255.
        let v1 = field.get(Vec3::new(1, 1, 1)).unwrap();
        assert!(
            v1 > 200,
            "deep interior of solid should stay high, got {v1}"
        );

        // First air voxel adjacent to the solid wall: some neighbours are 255.
        let v3 = field.get(Vec3::new(3, 1, 1)).unwrap();
        assert!(
            v3 > 0,
            "edge of air gets blended with neighbour solids, got {v3}"
        );
    }

    #[test]
    fn smooth_all_solid_stays_high() {
        let mut field = DensityField::new(Vec3::new(3, 3, 3));
        field.data.fill(255);
        smooth_density_field(&mut field);
        // Interior stays at max; edges get blended with OOB zeros so may be < 255
        let center = field.get(Vec3::new(1, 1, 1)).unwrap();
        assert_eq!(center, 255);
    }
}
