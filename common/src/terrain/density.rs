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
}
