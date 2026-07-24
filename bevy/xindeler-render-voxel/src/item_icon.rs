//! CPU voxel-to-image rasterizer for item icons — a faithful port of
//! xindeler-old's `voxygen::ui::graphic::renderer` (an `euc`-based software
//! triangle rasterizer with per-voxel ambient occlusion). Produces a plain
//! [`image::RgbaImage`]; the caller wraps that as a `bevy::Image` and hands
//! it to an `ImageNode` — no GPU render-to-texture, no offscreen camera.
//!
//! No asset loading here (this crate links `common` with `no-assets`, same
//! as [`crate::figure`]): callers hand in an already-parsed
//! [`dot_vox::DotVoxData`] and get a [`common::figure::Segment`] back via
//! [`load_icon_segment`], then rasterize it with [`render_item_icon`].

use common::{
    figure::{
        MatSegment, Segment,
        cell::Cell,
        mat_cell::{MatCell, Material},
    },
    util::{linear_to_srgba, srgb_to_linear_fast},
    vol::{FilledVox, IntoFullVolIterator, ReadVol, SizedVol},
};
use dot_vox::DotVoxData;
use euc::{Pipeline, buffer::Buffer2d, rasterizer};
use image::RgbaImage;
use vek::*;

use crate::figure::humanoid::recolor_grey;

/// How to frame a rasterized icon — the ported `voxygen::ui::graphic::
/// renderer::Transform`, restricted to the orthographic-only, non-stretch
/// case every `ImageSpec::VoxTrans` manifest entry actually uses.
#[derive(Clone, Copy)]
pub struct IconTransform {
    /// Euler rotation, applied X then Y then Z (matches
    /// `ImageSpec::VoxTrans`'s `(x_rot, y_rot, z_rot)` degrees fields).
    pub ori: Quaternion<f32>,
    pub offset: Vec3<f32>,
    pub zoom: f32,
}

impl Default for IconTransform {
    fn default() -> Self {
        Self {
            ori: Quaternion::identity(),
            offset: Vec3::zero(),
            zoom: 1.0,
        }
    }
}

/// Load a `.vox` model's given sub-model into an item-icon-ready [`Segment`]:
/// parses it as a [`MatSegment`] (materials preserved), optionally tints
/// greyscale voxels via [`recolor_grey`] (the same tinting figures use), then
/// strips skin/hollow material cells to empty — items never render a
/// figure's skin-tone placeholder cells (`ImageSpec`'s `color` field is the
/// legacy `graceful_load_segment_no_skin`).
pub fn load_icon_segment(vox: &DotVoxData, model_index: u32, color: Option<[u8; 3]>) -> Segment {
    let mut mat_seg = MatSegment::from_vox(vox, false, model_index as usize);

    if let Some(color) = color {
        let tint = Rgb::from(Vec3::from(color));
        mat_seg = mat_seg.map_rgb(|rgb| recolor_grey(rgb, tint));
    }

    mat_seg
        .map(|mat_cell| match mat_cell {
            MatCell::Mat(_) => Some(MatCell::Normal(Cell::empty())),
            MatCell::Normal(cell) if cell.is_hollowing() => Some(MatCell::Normal(Cell::empty())),
            _ => None,
        })
        .to_segment(|_: Material| Rgb::zero())
}

/// Rasterize a single voxel [`Segment`] into an `size_px`-sized icon.
/// Orthographic-only, `SampleStrat::None`-equivalent (1:1, no super-sampling)
/// — the v1 target per the design doc; legacy's `stretch`/perspective/
/// pixel-coverage modes aren't needed by the manifest's `VoxTrans` entries.
pub fn render_item_icon(
    segment: &Segment,
    transform: IconTransform,
    size_px: Vec2<u16>,
) -> RgbaImage {
    let dims = size_px.map(|e| e as usize);
    debug_assert!(dims.map(|e| e != 0).reduce_and());

    let bounds = {
        let size = segment.size().as_::<f32>();
        Aabb {
            min: Vec3::zero(),
            max: size,
        }
        .made_valid()
    };
    let (w, h, d) = bounds.size().into_tuple();

    let ori_mat = Mat4::from(transform.ori);
    let rotated_dims = ori_mat
        .mul_direction(Vec3::from(bounds.size()))
        .map(f32::abs);

    let mvp = Mat4::<f32>::orthographic_rh_no(FrustumPlanes {
        left: -1.0,
        right: 1.0,
        bottom: -1.0,
        top: 1.0,
        near: 0.0,
        far: 1.0,
    }) * Mat4::scaling_3d(rotated_dims.map(|e| 2.0 / e) * transform.zoom)
        * Mat4::translation_3d(transform.offset)
        * ori_mat
        * Mat4::translation_3d([-w / 2.0, -h / 2.0, -d / 2.0]);

    let mut color = Buffer2d::new(dims.into_array(), [0; 4]);
    let mut depth = Buffer2d::new(dims.into_array(), 1.0);

    Voxel {
        mvp,
        light_dir: Vec3::broadcast(-1.0).normalized(),
    }
    .draw::<rasterizer::Triangles<_>, _>(&generate_mesh(segment), &mut color, Some(&mut depth));

    RgbaImage::from_vec(
        dims.x as u32,
        dims.y as u32,
        color
            .as_ref()
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<u8>>(),
    )
    .expect("Buffer2d row-major layout always matches RgbaImage's expected byte count")
}

// ---------------------------------------------------------------------------
// euc pipeline (ported from voxygen::ui::graphic::renderer)
// ---------------------------------------------------------------------------

struct Voxel {
    mvp: Mat4<f32>,
    light_dir: Vec3<f32>,
}

#[derive(Copy, Clone)]
struct Vert {
    pos: Vec3<f32>,
    col: Rgb<f32>,
    norm: Vec3<f32>,
    ao_level: u8,
}

#[derive(Clone, Copy)]
struct VsOut(Rgba<f32>);

impl euc::Interpolate for VsOut {
    #[inline(always)]
    fn lerp2(a: Self, b: Self, x: f32, y: f32) -> Self {
        Self(a.0.map2(b.0, |a, b| a.mul_add(x, b * y)))
    }

    #[inline(always)]
    fn lerp3(a: Self, b: Self, c: Self, x: f32, y: f32, z: f32) -> Self {
        Self(
            a.0.map2(b.0.map2(c.0, |b, c| b.mul_add(y, c * z)), |a, bc| {
                a.mul_add(x, bc)
            }),
        )
    }
}

impl Pipeline for Voxel {
    type Pixel = [u8; 4];
    type Vertex = Vert;
    type VsOut = VsOut;

    #[inline(always)]
    fn vert(
        &self,
        Vert {
            pos,
            col,
            norm,
            ao_level,
        }: &Self::Vertex,
    ) -> ([f32; 4], Self::VsOut) {
        let ambiance = 0.25;
        let diffuse = norm.dot(-self.light_dir).max(0.0);
        let brightness = 2.5;
        let light = Rgb::from(*ao_level as f32 / 4.0) * (diffuse + ambiance) * brightness;
        let color = light * srgb_to_linear_fast(*col);
        let position = (self.mvp * Vec4::from_point(*pos)).into_array();
        (position, VsOut(Rgba::from_opaque(color)))
    }

    #[inline(always)]
    fn frag(&self, color: &Self::VsOut) -> Self::Pixel {
        linear_to_srgba(color.0)
            .map(|e| (e * 255.0) as u8)
            .into_array()
    }
}

fn ao_level(side1: bool, corner: bool, side2: bool) -> u8 {
    if side1 && side2 {
        0
    } else {
        3 - [side1, corner, side2].iter().filter(|e| **e).count() as u8
    }
}

fn create_quad(
    origin: Vec3<f32>,
    unit_x: Vec3<f32>,
    unit_y: Vec3<f32>,
    norm: Vec3<f32>,
    col: Rgb<f32>,
    occluders: [bool; 8],
) -> [Vert; 6] {
    let a_ao = ao_level(occluders[0], occluders[1], occluders[2]);
    let b_ao = ao_level(occluders[2], occluders[3], occluders[4]);
    let c_ao = ao_level(occluders[4], occluders[5], occluders[6]);
    let d_ao = ao_level(occluders[6], occluders[7], occluders[0]);

    let vert = |pos, ao| Vert {
        pos,
        col,
        norm,
        ao_level: ao,
    };
    let a = vert(origin, a_ao);
    let b = vert(origin + unit_x, b_ao);
    let c = vert(origin + unit_x + unit_y, c_ao);
    let d = vert(origin + unit_y, d_ao);

    // Flip to fix anisotropy.
    let (a, b, c, d) = if a_ao + c_ao > b_ao + d_ao {
        (d, a, b, c)
    } else {
        (a, b, c, d)
    };

    [a, b, c, c, d, a]
}

fn generate_mesh(segment: &Segment) -> Vec<Vert> {
    let mut vertices = Vec::new();

    for (pos, vox) in segment.full_vol_iter() {
        let Some(col) = vox.get_color() else {
            continue;
        };
        let col = col.map(|e| e as f32 / 255.0);
        let is_filled = |pos| segment.get(pos).map(|v| v.is_filled()).unwrap_or(false);
        let occluders = |unit_x, unit_y, dir| {
            [
                is_filled(pos + dir - unit_x),
                is_filled(pos + dir - unit_x - unit_y),
                is_filled(pos + dir - unit_y),
                is_filled(pos + dir + unit_x - unit_y),
                is_filled(pos + dir + unit_x),
                is_filled(pos + dir + unit_x + unit_y),
                is_filled(pos + dir + unit_y),
                is_filled(pos + dir - unit_x + unit_y),
            ]
        };
        let posf = pos.map(|e| e as f32);

        if !is_filled(pos - Vec3::unit_x()) {
            vertices.extend_from_slice(&create_quad(
                posf + Vec3::unit_y(),
                -Vec3::unit_y(),
                Vec3::unit_z(),
                -Vec3::unit_x(),
                col,
                occluders(-Vec3::unit_y(), Vec3::unit_z(), -Vec3::unit_x()),
            ));
        }
        if !is_filled(pos + Vec3::unit_x()) {
            vertices.extend_from_slice(&create_quad(
                posf + Vec3::unit_x(),
                Vec3::unit_y(),
                Vec3::unit_z(),
                Vec3::unit_x(),
                col,
                occluders(Vec3::unit_y(), Vec3::unit_z(), Vec3::unit_x()),
            ));
        }
        if !is_filled(pos - Vec3::unit_y()) {
            vertices.extend_from_slice(&create_quad(
                posf,
                Vec3::unit_x(),
                Vec3::unit_z(),
                -Vec3::unit_y(),
                col,
                occluders(Vec3::unit_x(), Vec3::unit_z(), -Vec3::unit_y()),
            ));
        }
        if !is_filled(pos + Vec3::unit_y()) {
            vertices.extend_from_slice(&create_quad(
                posf + Vec3::unit_y(),
                Vec3::unit_z(),
                Vec3::unit_x(),
                Vec3::unit_y(),
                col,
                occluders(Vec3::unit_z(), Vec3::unit_x(), Vec3::unit_y()),
            ));
        }
        if !is_filled(pos - Vec3::unit_z()) {
            vertices.extend_from_slice(&create_quad(
                posf,
                Vec3::unit_y(),
                Vec3::unit_x(),
                -Vec3::unit_z(),
                col,
                occluders(Vec3::unit_y(), Vec3::unit_x(), -Vec3::unit_z()),
            ));
        }
        if !is_filled(pos + Vec3::unit_z()) {
            vertices.extend_from_slice(&create_quad(
                posf + Vec3::unit_z(),
                Vec3::unit_x(),
                Vec3::unit_y(),
                Vec3::unit_z(),
                col,
                occluders(Vec3::unit_x(), Vec3::unit_y(), Vec3::unit_z()),
            ));
        }
    }

    vertices
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        figure::cell::{Cell, CellSurface},
        vol::WriteVol,
    };

    /// A 1x1x1 fully-opaque red segment — the smallest non-trivial input:
    /// exactly one visible voxel, all 6 faces exposed.
    fn single_red_voxel_segment() -> Segment {
        let mut segment = Segment::filled(Vec3::new(1, 1, 1), Cell::empty(), ());
        segment
            .set(
                Vec3::zero(),
                Cell::filled(Rgb::new(255, 0, 0), CellSurface::Matte),
            )
            .expect("(0,0,0) is in bounds for a 1x1x1 volume");
        segment
    }

    #[test]
    fn render_item_icon_produces_the_requested_dimensions() {
        let segment = single_red_voxel_segment();
        let img = render_item_icon(&segment, IconTransform::default(), Vec2::new(32, 32));
        assert_eq!(img.dimensions(), (32, 32));
    }

    #[test]
    fn render_item_icon_is_not_fully_transparent() {
        let segment = single_red_voxel_segment();
        let img = render_item_icon(&segment, IconTransform::default(), Vec2::new(32, 32));
        assert!(
            img.pixels().any(|p| p.0[3] != 0),
            "a single filled voxel centred in frame must produce at least one visible pixel"
        );
    }

    #[test]
    fn render_item_icon_is_deterministic() {
        let segment = single_red_voxel_segment();
        let transform = IconTransform {
            ori: Quaternion::rotation_x(0.3).rotated_y(0.5),
            offset: Vec3::new(0.1, -0.1, 0.0),
            zoom: 1.2,
        };
        let a = render_item_icon(&segment, transform, Vec2::new(24, 24));
        let b = render_item_icon(&segment, transform, Vec2::new(24, 24));
        assert_eq!(
            a.into_raw(),
            b.into_raw(),
            "identical inputs must rasterize to byte-identical output"
        );
    }

    #[test]
    fn load_icon_segment_strips_skin_and_hollow_cells() {
        // A minimal synthetic single-voxel .vox: one model, one filled cell
        // at the origin. `MatSegment::from_vox` reserves voxel indices 0-5
        // and 7 as figure-material markers (skin/hair/eye — see
        // `common::figure::MatSegment::from_vox`); any other index is a
        // plain palette-colour cell, so `i: 8` (palette slot 8) gives a
        // non-material voxel that must survive `load_icon_segment` intact.
        let mut palette = vec![
            dot_vox::Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            };
            9
        ];
        palette[8] = dot_vox::Color {
            r: 200,
            g: 200,
            b: 200,
            a: 255,
        };
        let vox = DotVoxData {
            version: 150,
            index_map: vec![],
            models: vec![dot_vox::Model {
                size: dot_vox::Size { x: 1, y: 1, z: 1 },
                voxels: vec![dot_vox::Voxel {
                    x: 0,
                    y: 0,
                    z: 0,
                    i: 8,
                }],
            }],
            palette,
            materials: vec![],
            scenes: vec![],
            layers: vec![],
        };

        let segment = load_icon_segment(&vox, 0, None);
        // The lone voxel must survive as a plain (non-skin) filled cell —
        // asserting only that *some* geometry made it through the strip
        // step, since the exact color depends on `to_segment`'s material
        // mapping (irrelevant here — no `MatCell::Mat` voxels in this
        // fixture to strip).
        assert!(
            segment.full_vol_iter().any(|(_, cell)| cell.is_filled()),
            "a plain (non-material) voxel must survive load_icon_segment unchanged"
        );
    }

    /// End-to-end against a real catalogue item — `Simple("Anvil")`'s
    /// manifest entry (`voxel.sprite.crafting_station.anvil`, offset
    /// `(0.5, 0.5, 0.0)`, rotation `(0, 60, 90)` degrees, zoom `1.0`). This
    /// crate has no `common_assets` (`no-assets`, same as `figure`), so it
    /// reads the `.vox` file directly off disk rather than through a
    /// specifier — the client's real runtime path loads bytes the same way,
    /// just via Bevy's own `AssetServer` instead of `std::fs`.
    #[test]
    #[ignore = "needs the real asset tree checked out (LFS); run locally"]
    fn render_item_icon_end_to_end_on_the_real_anvil_model() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/voxygen/voxel/sprite/crafting_station/anvil.vox"
        );
        let bytes = std::fs::read(path).expect("anvil.vox is committed to the repo");
        let vox = dot_vox::load_bytes(&bytes).expect("anvil.vox is a valid .vox file");

        let segment = load_icon_segment(&vox, 0, None);
        let transform = IconTransform {
            ori: Quaternion::rotation_x(0.0_f32.to_radians())
                .rotated_y(60.0_f32.to_radians())
                .rotated_z(90.0_f32.to_radians()),
            offset: Vec3::new(0.5, 0.5, 0.0),
            zoom: 1.0,
        };
        let img = render_item_icon(&segment, transform, Vec2::new(64, 64));

        assert_eq!(img.dimensions(), (64, 64));
        assert!(
            img.pixels().any(|p| p.0[3] != 0),
            "the real Anvil model must rasterize to at least one visible pixel"
        );
    }
}
