//! GPU upload and pipeline construction for embedded planar reference images.

use std::{mem::size_of, ops::Range};

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use wgpu::util::DeviceExt;

use crate::document::ReferenceImage;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ImageVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    unused: f32,
}

pub(super) struct Gpu {
    pub vertices: wgpu::Buffer,
    pub draws: Vec<(wgpu::BindGroup, Range<u32>)>,
}

pub(super) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    images: &[ReferenceImage],
) -> Gpu {
    let mut vertices = Vec::new();
    let mut decoded = Vec::new();
    for image in images.iter().filter(|image| image.visible) {
        let Ok(bitmap) = image::load_from_memory(&image.bytes).map(|image| image.to_rgba8()) else {
            continue;
        };
        let height = image.width_mm * f64::from(bitmap.height()) / f64::from(bitmap.width());
        let point = |x, y| image.plane.to_world(image.origin + DVec2::new(x, y));
        let first = vertices.len() as u32;
        for (point, uv) in [
            (point(0.0, 0.0), [0.0, 1.0]),
            (point(image.width_mm, 0.0), [1.0, 1.0]),
            (point(image.width_mm, height), [1.0, 0.0]),
            (point(0.0, 0.0), [0.0, 1.0]),
            (point(image.width_mm, height), [1.0, 0.0]),
            (point(0.0, height), [0.0, 0.0]),
        ] {
            vertices.push(ImageVertex {
                position: point.as_vec3().to_array(),
                normal: [0.0; 3],
                uv,
                unused: 0.0,
            });
        }
        decoded.push((bitmap, first..first + 6));
    }
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("reference image quads"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let draws = decoded
        .into_iter()
        .map(|(bitmap, range)| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("reference image"),
                size: wgpu::Extent3d {
                    width: bitmap.width(),
                    height: bitmap.height(),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                bitmap.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bitmap.width() * 4),
                    rows_per_image: Some(bitmap.height()),
                },
                texture.size(),
            );
            let view = texture.create_view(&Default::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("reference image texture"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            (group, range)
        })
        .collect();
    Gpu { vertices, draws }
}

pub(super) fn pipeline(
    device: &wgpu::Device,
    scene: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    samples: u32,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("reference image layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("reference image pipeline layout"),
        bind_group_layouts: &[Some(scene), Some(&textures)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("reference images"), layout: Some(&layout), vertex: wgpu::VertexState { module: shader, entry_point: Some("vs_reference"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: size_of::<ImageVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32] }] }, fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some("fs_reference"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }), primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() }, depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }), multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None });
    (pipeline, textures)
}
