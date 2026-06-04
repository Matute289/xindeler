use super::{
    super::{AaMode, GlobalsLayouts, Vertex as VertexTrait},
    terrain::TerrainLayout,
};
use bytemuck::{Pod, Zeroable};
use std::mem;
use vek::*;

/// Vertex format for the smooth Transvoxel terrain pipeline.
///
/// Positions are stored as raw floats (chunk-local coords) so the interpolated
/// Transvoxel vertices are not rounded to the integer block grid.
/// Normals are packed as 10-10-10-2 snorm so smooth gradient normals are
/// preserved rather than being quantized to one of 6 axis-aligned directions.
/// Color is baked per-vertex at mesh time so no atlas texture sampler is
/// needed at draw time.
#[repr(C)]
#[derive(Copy, Clone, Debug, Zeroable, Pod)]
pub struct SmoothTerrainVertex {
    pos: [f32; 3],      // chunk-local float position (12 bytes)
    norm: u32,          // 10-10-10-2 snorm packed normal (4 bytes)
    col_light: u32,     // RGBA + light packed via TerrainVertex::make_col_light (4 bytes)
}

/// Pack a unit normal into 10-10-10-2 snorm representation.
/// Each component is mapped from [-1, 1] to a signed 10-bit integer [-511, 511].
/// The top 2 bits (w component) are unused and set to 0.
pub fn pack_norm_10_10_10_2(norm: Vec3<f32>) -> u32 {
    let pack = |v: f32| (v.clamp(-1.0, 1.0) * 511.0) as i32 as u32 & 0x3FF;
    pack(norm.x) | (pack(norm.y) << 10) | (pack(norm.z) << 20)
}

impl SmoothTerrainVertex {
    pub fn new(pos: Vec3<f32>, norm: Vec3<f32>, col_light: u32) -> Self {
        Self {
            pos: pos.into_array(),
            norm: pack_norm_10_10_10_2(norm),
            col_light,
        }
    }

    pub fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Uint32];
        wgpu::VertexBufferLayout {
            array_stride: Self::STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        }
    }
}

impl VertexTrait for SmoothTerrainVertex {
    const QUADS_INDEX: Option<wgpu::IndexFormat> = None; // triangles, no index buffer
    const STRIDE: wgpu::BufferAddress = mem::size_of::<Self>() as wgpu::BufferAddress;
}

pub struct SmoothTerrainPipeline {
    pub pipeline: wgpu::RenderPipeline,
}

impl SmoothTerrainPipeline {
    pub fn new(
        device: &wgpu::Device,
        vs_module: &wgpu::ShaderModule,
        fs_module: &wgpu::ShaderModule,
        global_layout: &GlobalsLayouts,
        terrain_layout: &TerrainLayout,
        aa_mode: AaMode,
        format: wgpu::TextureFormat,
    ) -> Self {
        // The smooth pipeline has no atlas texture at set 2.
        // Locals (model matrix) are bound at set 2, not set 3.
        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Smooth terrain pipeline layout"),
                push_constant_ranges: &[],
                bind_group_layouts: &[
                    &global_layout.globals,          // set 0
                    &global_layout.shadow_textures,  // set 1
                    &terrain_layout.locals,          // set 2 (locals only, no atlas)
                ],
            });

        let samples = aa_mode.samples();

        let render_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Smooth terrain pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: vs_module,
                    entry_point: Some("main"),
                    buffers: &[SmoothTerrainVertex::desc()],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::GreaterEqual,
                    stencil: wgpu::StencilState {
                        front: wgpu::StencilFaceState::IGNORE,
                        back: wgpu::StencilFaceState::IGNORE,
                        read_mask: !0,
                        write_mask: 0,
                    },
                    bias: wgpu::DepthBiasState {
                        constant: 0,
                        slope_scale: 0.0,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: fs_module,
                    entry_point: Some("main"),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Uint,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            });

        Self {
            pipeline: render_pipeline,
        }
    }
}
