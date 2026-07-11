//! Multi-body offscreen wgpu renderer and padded BGRA readback.

use std::{collections::HashMap, ops::Range};

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Quat, Vec3};
use wgpu::util::DeviceExt;

use crate::{
    camera::OrbitCamera,
    document::{
        BodyId, ConstructionAxis, ConstructionPlane, ConstructionPoint, Material, ReferenceImage,
        SelItem,
    },
    gizmo::Handle,
    kernel::BodyMesh,
    sketch::{Sketch, SketchEntity, SketchPlane},
    theme::CanvasTheme,
    viewport::{
        AnalysisMode, DisplayMode,
        grid::adaptive_pitch,
        orientation_cube::{self, Region},
        reference_image,
    },
};

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const SAMPLE_COUNT: u32 = 4;
const MODEL_STRIDE: u64 = 256;
const MAX_MODEL_SLOTS: u64 = 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    curvature: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RibbonVertex {
    position: [f32; 3],
    partner: [f32; 3],
    side: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RibbonUniform {
    viewport_width_px: f32,
    viewport_height_px: f32,
    line_width_px: f32,
    _padding: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CubeVertex {
    position: [f32; 3],
    normal: [f32; 3],
    label_uv: [f32; 2],
    label_cell: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_projection: [[f32; 4]; 4],
    eye_pitch: [f32; 4],
    clip_plane: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    grid_minor: [f32; 4],
    grid_major: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    body: [f32; 4],
    edge: [f32; 4],
    section_cap: [f32; 4],
    analysis: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Tint {
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Model {
    matrix: [[f32; 4]; 4],
    base_color_metallic: [f32; 4],
    roughness: [f32; 4],
}

struct BodyDrawRanges {
    id: BodyId,
    mesh: Range<u32>,
    edge_lines: Range<u32>,
    faces: Vec<Range<u32>>,
    ribbon_edges: Vec<Range<u32>>,
    model_slot: u32,
    material: Material,
    double_sided: bool,
}

/// Gizmo placement and hover state for one rendered frame.
pub struct GizmoRender {
    pub pivot: Vec3,
    pub scale: f32,
    pub hovered: Option<Handle>,
}

/// Placement and hover state for a face-extrusion arrow.
pub struct ExtrudeArrowRender {
    pub origin: Vec3,
    pub normal: Vec3,
    pub scale: f32,
    pub hovered: bool,
}

/// Per-frame orientation cube placement and hover state.
pub struct OrientationCubeRender {
    pub device_scale: f32,
    pub hovered: Option<Region>,
}

struct PreviewGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    transform: Mat4,
}

struct SketchGpu {
    lines: wgpu::Buffer,
    line_count: u32,
    defined_lines: wgpu::Buffer,
    defined_line_count: u32,
    construction_lines: wgpu::Buffer,
    construction_line_count: u32,
    selected: wgpu::Buffer,
    selected_count: u32,
    pending: wgpu::Buffer,
    pending_count: u32,
    fill_vertices: wgpu::Buffer,
    fill_indices: wgpu::Buffer,
    profile_ranges: Vec<(SelItem, Range<u32>)>,
}

struct GizmoGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    ranges: [(Handle, Range<u32>); 7],
    normal_groups: Vec<wgpu::BindGroup>,
    bright_groups: Vec<wgpu::BindGroup>,
}

struct ArrowGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    rim_range: Range<u32>,
    fill_range: Range<u32>,
    rim_group: wgpu::BindGroup,
    fill_group: wgpu::BindGroup,
    hover_group: wgpu::BindGroup,
}

struct OrientationCubeGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    ranges: Vec<(Region, Range<u32>)>,
    uniform_buffer: wgpu::Buffer,
    normal_groups: Vec<wgpu::BindGroup>,
    normal_tint_buffers: Vec<wgpu::Buffer>,
    hover_groups: Vec<wgpu::BindGroup>,
    hover_tint_buffers: Vec<wgpu::Buffer>,
    label_bind_group: wgpu::BindGroup,
    label_texture: wgpu::Texture,
}

/// A CPU image returned in gpui's native BGRA byte order.
pub struct RenderedFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Owns wgpu state and the currently uploaded document scene.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    uniform_buffer: wgpu::Buffer,
    model_buffer: wgpu::Buffer,
    base_bind_group: wgpu::BindGroup,
    base_tint_buffer: wgpu::Buffer,
    hidden_edge_bind_group: wgpu::BindGroup,
    hidden_edge_tint_buffer: wgpu::Buffer,
    hover_bind_group: wgpu::BindGroup,
    hover_tint_buffer: wgpu::Buffer,
    selected_bind_group: wgpu::BindGroup,
    selected_tint_buffer: wgpu::Buffer,
    sketch_hover_bind_group: wgpu::BindGroup,
    sketch_selected_bind_group: wgpu::BindGroup,
    hover_ribbon_bind_group: wgpu::BindGroup,
    hover_ribbon_tint_buffer: wgpu::Buffer,
    selected_ribbon_bind_group: wgpu::BindGroup,
    selected_ribbon_tint_buffer: wgpu::Buffer,
    visualize_selected_bind_group: wgpu::BindGroup,
    preview_bind_group: wgpu::BindGroup,
    interference_bind_group: wgpu::BindGroup,
    sketch_line_bind_group: wgpu::BindGroup,
    sketch_line_tint_buffer: wgpu::Buffer,
    sketch_defined_bind_group: wgpu::BindGroup,
    sketch_defined_tint_buffer: wgpu::Buffer,
    sketch_construction_bind_group: wgpu::BindGroup,
    sketch_construction_tint_buffer: wgpu::Buffer,
    sketch_fill_bind_group: wgpu::BindGroup,
    sketch_fill_tint_buffer: wgpu::Buffer,
    sketch_accent_bind_group: wgpu::BindGroup,
    mesh_vertices: wgpu::Buffer,
    mesh_indices: wgpu::Buffer,
    edge_vertices: wgpu::Buffer,
    ribbon_vertices: wgpu::Buffer,
    ribbon_indices: wgpu::Buffer,
    hover_ribbon_width: wgpu::Buffer,
    selected_ribbon_width: wgpu::Buffer,
    hover_ribbon_width_group: wgpu::BindGroup,
    selected_ribbon_width_group: wgpu::BindGroup,
    measure_vertices: wgpu::Buffer,
    measure_count: u32,
    grid_vertices: wgpu::Buffer,
    body_ranges: Vec<BodyDrawRanges>,
    scene_cache: Vec<(BodyId, BodyMesh, bool, Material)>,
    analysis: AnalysisMode,
    suppressed_body: Option<BodyId>,
    preview_transforms: HashMap<BodyId, Mat4>,
    preview_mesh: Option<PreviewGpu>,
    preview_is_interference: bool,
    sketch: Option<SketchGpu>,
    grid_transform: Mat4,
    gizmo: GizmoGpu,
    arrow: ArrowGpu,
    orientation_cube: OrientationCubeGpu,
    background_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    surface_mesh_pipeline: wgpu::RenderPipeline,
    xray_pipeline: wgpu::RenderPipeline,
    surface_xray_pipeline: wgpu::RenderPipeline,
    section_cap_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    preview_pipeline: wgpu::RenderPipeline,
    fill_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    body_edge_pipeline: wgpu::RenderPipeline,
    hidden_edge_pipeline: wgpu::RenderPipeline,
    accent_line_pipeline: wgpu::RenderPipeline,
    ribbon_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    gizmo_pipeline: wgpu::RenderPipeline,
    cube_pipeline: wgpu::RenderPipeline,
    reference_image_pipeline: wgpu::RenderPipeline,
    reference_image_layout: wgpu::BindGroupLayout,
    reference_images: Option<reference_image::Gpu>,
    canvas_theme: CanvasTheme,
}

impl Renderer {
    /// Initializes a low-power adapter with an empty scene.
    pub fn new(canvas_theme: CanvasTheme) -> Result<Self, String> {
        pollster::block_on(Self::new_async(canvas_theme))
    }

    async fn new_async(canvas_theme: CanvasTheme) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("no wgpu adapter: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Free3D device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("wgpu device creation failed: {error}"))?;

        let dummy_vertex = [Vertex {
            position: [0.0; 3],
            normal: [0.0; 3],
            curvature: 0.0,
        }];
        let mesh_vertices = vertex_buffer(&device, "mesh vertices", &dummy_vertex);
        let mesh_indices = index_buffer(&device, "mesh indices", &[0]);
        let edge_vertices = vertex_buffer(&device, "BRep edge vertices", &dummy_vertex);
        let ribbon_vertices =
            ribbon_vertex_buffer(&device, "BRep ribbon vertices", &[RibbonVertex::zeroed()]);
        let ribbon_indices = index_buffer(&device, "BRep ribbon indices", &[0]);
        let measure_vertices = vertex_buffer(&device, "measure line", &dummy_vertex);
        let extent = 10_000.0;
        let grid_vertices = vertex_buffer(
            &device,
            "grid plane",
            &[
                Vertex {
                    position: [-extent, -extent, -0.02],
                    normal: [0.0; 3],
                    curvature: 0.0,
                },
                Vertex {
                    position: [extent, -extent, -0.02],
                    normal: [0.0; 3],
                    curvature: 0.0,
                },
                Vertex {
                    position: [extent, extent, -0.02],
                    normal: [0.0; 3],
                    curvature: 0.0,
                },
                Vertex {
                    position: [-extent, -extent, -0.02],
                    normal: [0.0; 3],
                    curvature: 0.0,
                },
                Vertex {
                    position: [extent, extent, -0.02],
                    normal: [0.0; 3],
                    curvature: 0.0,
                },
                Vertex {
                    position: [-extent, extent, -0.02],
                    normal: [0.0; 3],
                    curvature: 0.0,
                },
            ],
        );
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dynamic body model matrices"),
            size: MODEL_STRIDE * MAX_MODEL_SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera and tint layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    // The Model slot carries the per-body material since W11,
                    // so the fragment stage reads it too.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(size_of::<Model>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let (base_bind_group, base_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "base tint",
            canvas_theme.edge,
        );
        let (hidden_edge_bind_group, hidden_edge_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "hidden edge tint",
            canvas_theme.hidden_edge,
        );
        let (hover_bind_group, hover_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "hover tint",
            canvas_theme.face_hover,
        );
        let (selected_bind_group, selected_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "selected tint",
            canvas_theme.face_selected,
        );
        let (sketch_hover_bind_group, _) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "sketch profile hover tint",
            [1.0, 0.58, 0.30, 0.48],
        );
        let (sketch_selected_bind_group, _) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "sketch profile selected tint",
            [1.0, 0.31, 0.08, 0.72],
        );
        let (hover_ribbon_bind_group, hover_ribbon_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "hover ribbon tint",
            canvas_theme.edge_ribbon_hover,
        );
        let (selected_ribbon_bind_group, selected_ribbon_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "selected ribbon tint",
            canvas_theme.edge_ribbon_selected,
        );
        let (visualize_selected_bind_group, _) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "visualize selected tint",
            [0.18, 0.61, 0.91, 0.20],
        );
        let (preview_bind_group, _) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "extrude preview tint",
            [1.0, 0.38, 0.12, 0.36],
        );
        let (interference_bind_group, _) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "interference tint",
            [0.95, 0.05, 0.04, 0.62],
        );
        let (sketch_line_bind_group, sketch_line_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "sketch line tint",
            canvas_theme.sketch,
        );
        let (sketch_defined_bind_group, sketch_defined_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "defined sketch line tint",
            canvas_theme.sketch_defined,
        );
        let (sketch_construction_bind_group, sketch_construction_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "construction sketch line tint",
            canvas_theme.sketch_construction,
        );
        let (sketch_fill_bind_group, sketch_fill_tint_buffer) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "sketch profile fill tint",
            canvas_theme.sketch_fill,
        );
        let (sketch_accent_bind_group, _) = tint_bind_group(
            &device,
            &bind_group_layout,
            &uniform_buffer,
            &model_buffer,
            "sketch pending tint",
            [1.0, 0.43, 0.20, 1.0],
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let ribbon_width_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ribbon width layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<RibbonUniform>() as u64),
                    },
                    count: None,
                }],
            });
        let ribbon_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ribbon pipeline layout"),
                bind_group_layouts: &[Some(&bind_group_layout), None, Some(&ribbon_width_layout)],
                immediate_size: 0,
            });
        let (hover_ribbon_width, hover_ribbon_width_group) =
            ribbon_width_bind_group(&device, &ribbon_width_layout, "hover ribbon width");
        let (selected_ribbon_width, selected_ribbon_width_group) =
            ribbon_width_bind_group(&device, &ribbon_width_layout, "selected ribbon width");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let background_pipeline = create_background_pipeline(&device, &pipeline_layout, &shader);
        let (reference_image_pipeline, reference_image_layout) = reference_image::pipeline(
            &device,
            &bind_group_layout,
            &shader,
            COLOR_FORMAT,
            SAMPLE_COUNT,
        );
        let mesh_pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_main",
            "fs_mesh",
            wgpu::PrimitiveTopology::TriangleList,
            true,
            Some(wgpu::BlendState::REPLACE),
        );
        let surface_mesh_pipeline = create_special_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_main",
            "fs_mesh",
            wgpu::PrimitiveTopology::TriangleList,
            None,
            wgpu::CompareFunction::LessEqual,
            true,
            Some(wgpu::BlendState::REPLACE),
        );
        // X-Ray is intentionally a fixed-alpha, unsorted approximation: back
        // faces are culled and depth writes are disabled while body order stays
        // stable. Feature edges are drawn afterward at full strength.
        let xray_pipeline = create_special_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_main",
            "fs_xray",
            wgpu::PrimitiveTopology::TriangleList,
            Some(wgpu::Face::Back),
            wgpu::CompareFunction::LessEqual,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let surface_xray_pipeline = create_special_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_main",
            "fs_xray",
            wgpu::PrimitiveTopology::TriangleList,
            None,
            wgpu::CompareFunction::LessEqual,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let section_cap_pipeline = create_special_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_main",
            "fs_cap",
            wgpu::PrimitiveTopology::TriangleList,
            Some(wgpu::Face::Front),
            wgpu::CompareFunction::LessEqual,
            true,
            Some(wgpu::BlendState::REPLACE),
        );
        // Overlays must cull back faces: hidden far-side triangles otherwise
        // leak through the depth test near silhouettes (the vs_overlay z-pull
        // dwarfs the front/back depth gap there) and render as a thick rim.
        let overlay_pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_overlay",
            "fs_tint_clipped",
            wgpu::PrimitiveTopology::TriangleList,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let preview_pipeline = create_preview_pipeline(&device, &pipeline_layout, &shader);
        let fill_pipeline = create_fill_pipeline(&device, &pipeline_layout, &shader);
        let line_pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_line",
            "fs_tint",
            wgpu::PrimitiveTopology::LineList,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let body_edge_pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_line",
            "fs_tint_clipped",
            wgpu::PrimitiveTopology::LineList,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let hidden_edge_pipeline = create_special_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_line",
            "fs_tint_clipped",
            wgpu::PrimitiveTopology::LineList,
            None,
            wgpu::CompareFunction::Greater,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let accent_line_pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_line",
            "fs_tint",
            wgpu::PrimitiveTopology::LineList,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let ribbon_pipeline = create_ribbon_pipeline(&device, &ribbon_pipeline_layout, &shader);
        let grid_pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            "vs_main",
            "fs_grid",
            wgpu::PrimitiveTopology::TriangleList,
            true,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let gizmo_pipeline = create_gizmo_pipeline(&device, &pipeline_layout, &shader);
        let (gizmo_vertices, gizmo_indices, gizmo_ranges) = gizmo_geometry();
        let gizmo_colors = [
            [0.82, 0.22, 0.24, 0.90],
            [0.25, 0.72, 0.38, 0.90],
            [0.22, 0.47, 0.88, 0.90],
            [0.82, 0.22, 0.24, 0.88],
            [0.25, 0.72, 0.38, 0.88],
            [0.22, 0.47, 0.88, 0.88],
            [0.76, 0.79, 0.83, 0.92],
        ];
        let normal_groups = gizmo_colors
            .iter()
            .enumerate()
            .map(|(index, &color)| {
                tint_bind_group(
                    &device,
                    &bind_group_layout,
                    &uniform_buffer,
                    &model_buffer,
                    &format!("gizmo tint {index}"),
                    color,
                )
                .0
            })
            .collect();
        let bright_groups = gizmo_colors
            .iter()
            .enumerate()
            .map(|(index, &color)| {
                let bright = [
                    (color[0] * 1.35).min(1.0),
                    (color[1] * 1.35).min(1.0),
                    (color[2] * 1.35).min(1.0),
                    1.0,
                ];
                tint_bind_group(
                    &device,
                    &bind_group_layout,
                    &uniform_buffer,
                    &model_buffer,
                    &format!("bright gizmo tint {index}"),
                    bright,
                )
                .0
            })
            .collect();
        let gizmo = GizmoGpu {
            vertices: vertex_buffer(&device, "gizmo vertices", &gizmo_vertices),
            indices: index_buffer(&device, "gizmo indices", &gizmo_indices),
            ranges: gizmo_ranges,
            normal_groups,
            bright_groups,
        };
        let (arrow_vertices, arrow_indices, rim_range, fill_range) = double_arrow_geometry();
        let arrow_group = |label, color| {
            tint_bind_group(
                &device,
                &bind_group_layout,
                &uniform_buffer,
                &model_buffer,
                label,
                color,
            )
            .0
        };
        let arrow = ArrowGpu {
            vertices: vertex_buffer(&device, "double-headed arrow vertices", &arrow_vertices),
            indices: index_buffer(&device, "double-headed arrow indices", &arrow_indices),
            rim_range,
            fill_range,
            rim_group: arrow_group("arrow light rim", [0.72, 0.76, 1.0, 0.92]),
            fill_group: arrow_group("arrow indigo", [0.357, 0.357, 0.839, 1.0]),
            hover_group: arrow_group("arrow hover indigo", [0.45, 0.48, 1.0, 1.0]),
        };
        let cube_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("orientation cube uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (cube_vertices, cube_indices, cube_ranges) = orientation_cube_geometry();
        let mut cube_normal_groups = Vec::with_capacity(cube_ranges.len());
        let mut cube_normal_tint_buffers = Vec::with_capacity(cube_ranges.len());
        let mut cube_hover_groups = Vec::with_capacity(cube_ranges.len());
        let mut cube_hover_tint_buffers = Vec::with_capacity(cube_ranges.len());
        for (index, (region, _)) in cube_ranges.iter().enumerate() {
            let (group, buffer) = tint_bind_group(
                &device,
                &bind_group_layout,
                &cube_uniform_buffer,
                &model_buffer,
                &format!("orientation cube region {index}"),
                cube_region_color(canvas_theme, *region),
            );
            cube_normal_groups.push(group);
            cube_normal_tint_buffers.push(buffer);
            let (group, buffer) = tint_bind_group(
                &device,
                &bind_group_layout,
                &cube_uniform_buffer,
                &model_buffer,
                &format!("orientation cube hover {index}"),
                canvas_theme.cube_hover,
            );
            cube_hover_groups.push(group);
            cube_hover_tint_buffers.push(buffer);
        }
        let (cube_pipeline, label_bind_group, label_texture) = create_cube_pipeline(
            &device,
            &queue,
            &bind_group_layout,
            &shader,
            orientation_cube::build_label_atlas(),
        );
        let orientation_cube = OrientationCubeGpu {
            vertices: cube_vertex_buffer(&device, "orientation cube vertices", &cube_vertices),
            indices: index_buffer(&device, "orientation cube indices", &cube_indices),
            ranges: cube_ranges,
            uniform_buffer: cube_uniform_buffer,
            normal_groups: cube_normal_groups,
            normal_tint_buffers: cube_normal_tint_buffers,
            hover_groups: cube_hover_groups,
            hover_tint_buffers: cube_hover_tint_buffers,
            label_bind_group,
            label_texture,
        };
        Ok(Self {
            device,
            queue,
            uniform_buffer,
            model_buffer,
            base_bind_group,
            base_tint_buffer,
            hidden_edge_bind_group,
            hidden_edge_tint_buffer,
            hover_bind_group,
            hover_tint_buffer,
            selected_bind_group,
            selected_tint_buffer,
            sketch_hover_bind_group,
            sketch_selected_bind_group,
            hover_ribbon_bind_group,
            hover_ribbon_tint_buffer,
            selected_ribbon_bind_group,
            selected_ribbon_tint_buffer,
            visualize_selected_bind_group,
            preview_bind_group,
            interference_bind_group,
            sketch_line_bind_group,
            sketch_line_tint_buffer,
            sketch_defined_bind_group,
            sketch_defined_tint_buffer,
            sketch_construction_bind_group,
            sketch_construction_tint_buffer,
            sketch_fill_bind_group,
            sketch_fill_tint_buffer,
            sketch_accent_bind_group,
            mesh_vertices,
            mesh_indices,
            edge_vertices,
            ribbon_vertices,
            ribbon_indices,
            hover_ribbon_width,
            selected_ribbon_width,
            hover_ribbon_width_group,
            selected_ribbon_width_group,
            measure_vertices,
            measure_count: 0,
            grid_vertices,
            body_ranges: Vec::new(),
            scene_cache: Vec::new(),
            analysis: AnalysisMode::Off,
            suppressed_body: None,
            preview_transforms: HashMap::new(),
            preview_mesh: None,
            preview_is_interference: false,
            sketch: None,
            grid_transform: Mat4::IDENTITY,
            gizmo,
            arrow,
            orientation_cube,
            background_pipeline,
            mesh_pipeline,
            surface_mesh_pipeline,
            xray_pipeline,
            surface_xray_pipeline,
            section_cap_pipeline,
            overlay_pipeline,
            preview_pipeline,
            fill_pipeline,
            line_pipeline,
            body_edge_pipeline,
            hidden_edge_pipeline,
            accent_line_pipeline,
            ribbon_pipeline,
            grid_pipeline,
            gizmo_pipeline,
            cube_pipeline,
            reference_image_pipeline,
            reference_image_layout,
            reference_images: None,
            canvas_theme,
        })
    }

    /// Rebuilds the orientation-cube label atlas for the current language.
    pub fn refresh_orientation_labels(&mut self) {
        let atlas = orientation_cube::build_label_atlas().unwrap_or_else(|| {
            vec![
                0;
                (orientation_cube::LABEL_ATLAS_WIDTH * orientation_cube::LABEL_CELL_SIZE) as usize
            ]
        });
        self.queue.write_texture(
            self.orientation_cube.label_texture.as_image_copy(),
            &atlas,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(orientation_cube::LABEL_ATLAS_WIDTH),
                rows_per_image: Some(orientation_cube::LABEL_CELL_SIZE),
            },
            self.orientation_cube.label_texture.size(),
        );
    }

    /// Uploads visible embedded images as non-pickable plane-local textured quads.
    ///
    /// An empty list clears the state entirely: wgpu forbids slicing an empty
    /// vertex buffer, so the draw block must be skipped when nothing renders.
    pub fn upload_reference_images(&mut self, images: &[ReferenceImage]) {
        if images.is_empty() {
            self.reference_images = None;
            return;
        }
        self.reference_images = Some(reference_image::upload(
            &self.device,
            &self.queue,
            &self.reference_image_layout,
            images,
        ));
    }

    /// Applies a new canvas palette to subsequent frames and persistent tints.
    pub fn set_canvas_theme(&mut self, canvas_theme: CanvasTheme) {
        self.canvas_theme = canvas_theme;
        for (buffer, color) in [
            (&self.base_tint_buffer, canvas_theme.edge),
            (&self.hidden_edge_tint_buffer, canvas_theme.hidden_edge),
            (&self.hover_tint_buffer, canvas_theme.face_hover),
            (&self.selected_tint_buffer, canvas_theme.face_selected),
            (&self.sketch_line_tint_buffer, canvas_theme.sketch),
            (
                &self.sketch_defined_tint_buffer,
                canvas_theme.sketch_defined,
            ),
            (&self.sketch_fill_tint_buffer, canvas_theme.sketch_fill),
            (
                &self.sketch_construction_tint_buffer,
                canvas_theme.sketch_construction,
            ),
        ] {
            self.queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&Tint { color }));
        }
        for (buffer, color) in [
            (
                &self.hover_ribbon_tint_buffer,
                canvas_theme.edge_ribbon_hover,
            ),
            (
                &self.selected_ribbon_tint_buffer,
                canvas_theme.edge_ribbon_selected,
            ),
        ] {
            self.queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&Tint { color }));
        }
        for (index, (region, _)) in self.orientation_cube.ranges.iter().enumerate() {
            for (buffer, color) in [
                (
                    &self.orientation_cube.normal_tint_buffers[index],
                    cube_region_color(canvas_theme, *region),
                ),
                (
                    &self.orientation_cube.hover_tint_buffers[index],
                    canvas_theme.cube_hover,
                ),
            ] {
                self.queue
                    .write_buffer(buffer, 0, bytemuck::bytes_of(&Tint { color }));
            }
        }
    }

    /// Rebuilds concatenated GPU buffers and stable per-body draw ranges.
    pub fn upload_scene(&mut self, scene: &[(BodyId, &BodyMesh, bool, Material)]) {
        self.scene_cache = scene
            .iter()
            .map(|(id, mesh, visible, material)| (*id, (*mesh).clone(), *visible, *material))
            .collect();
        self.rebuild_scene_buffers();
    }

    /// Changes the surface analysis and lazily rebuilds curvature attributes.
    pub fn set_analysis(&mut self, analysis: AnalysisMode) {
        if self.analysis != analysis {
            self.analysis = analysis;
            self.rebuild_scene_buffers();
        }
    }

    fn rebuild_scene_buffers(&mut self) {
        if self.analysis == AnalysisMode::Curvature {
            for (_, mesh, _, _) in &mut self.scene_cache {
                mesh.ensure_curvature();
            }
        }
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut edge_vertices = Vec::new();
        let mut ribbon_vertices = Vec::new();
        let mut ribbon_indices = Vec::new();
        let mut body_ranges = Vec::new();
        for (id, mesh, double_sided, material) in &self.scene_cache {
            let vertex_base = vertices.len() as u32;
            let index_base = indices.len() as u32;
            vertices.extend(mesh.positions.iter().enumerate().map(|(index, &position)| {
                Vertex {
                    position,
                    normal: mesh.normals.get(index).copied().unwrap_or([0.0, 0.0, 1.0]),
                    curvature: mesh
                        .curvature
                        .as_ref()
                        .and_then(|values| values.get(index))
                        .copied()
                        .unwrap_or(0.0),
                }
            }));
            indices.extend(mesh.indices.iter().map(|index| vertex_base + index));
            let mesh_range = index_base..indices.len() as u32;
            let faces = mesh
                .face_ranges
                .iter()
                .map(|range| index_base + range.start..index_base + range.end)
                .collect();
            let edge_base = edge_vertices.len() as u32;
            edge_vertices.extend(mesh.edge_vertices.iter().map(|&position| Vertex {
                position,
                normal: [0.0; 3],
                curvature: 0.0,
            }));
            let edge_lines = edge_base..edge_vertices.len() as u32;
            let ribbon_edges = mesh
                .edges
                .iter()
                .map(|edge| {
                    debug_assert!(edge.range.start <= edge.range.end);
                    let start = ribbon_indices.len() as u32;
                    for segment in edge.points.windows(2) {
                        append_ribbon_segment(
                            &mut ribbon_vertices,
                            &mut ribbon_indices,
                            Vec3::from(segment[0]),
                            Vec3::from(segment[1]),
                        );
                    }
                    start..ribbon_indices.len() as u32
                })
                .collect();
            let model_slot = body_ranges.len() as u32 + 1;
            body_ranges.push(BodyDrawRanges {
                id: *id,
                mesh: mesh_range,
                edge_lines,
                faces,
                ribbon_edges,
                model_slot,
                material: *material,
                double_sided: *double_sided,
            });
        }
        self.mesh_vertices = if vertices.is_empty() {
            vertex_buffer(&self.device, "mesh vertices", &[Vertex::zeroed()])
        } else {
            vertex_buffer(&self.device, "mesh vertices", &vertices)
        };
        self.mesh_indices = if indices.is_empty() {
            index_buffer(&self.device, "mesh indices", &[0])
        } else {
            index_buffer(&self.device, "mesh indices", &indices)
        };
        self.edge_vertices = if edge_vertices.is_empty() {
            vertex_buffer(&self.device, "BRep edge vertices", &[Vertex::zeroed()])
        } else {
            vertex_buffer(&self.device, "BRep edge vertices", &edge_vertices)
        };
        self.ribbon_vertices = if ribbon_vertices.is_empty() {
            ribbon_vertex_buffer(
                &self.device,
                "BRep ribbon vertices",
                &[RibbonVertex::zeroed()],
            )
        } else {
            ribbon_vertex_buffer(&self.device, "BRep ribbon vertices", &ribbon_vertices)
        };
        self.ribbon_indices = if ribbon_indices.is_empty() {
            index_buffer(&self.device, "BRep ribbon indices", &[0])
        } else {
            index_buffer(&self.device, "BRep ribbon indices", &ribbon_indices)
        };
        self.body_ranges = body_ranges;
    }

    /// Uploads or clears the two-point world-space measurement guide.
    pub fn set_measure_line(&mut self, points: Option<[Vec3; 2]>) {
        let vertices = points.map(|points| points.map(line_vertex));
        self.measure_count = if vertices.is_some() { 2 } else { 0 };
        self.measure_vertices = vertices.map_or_else(
            || vertex_buffer(&self.device, "measure line", &[Vertex::zeroed()]),
            |vertices| vertex_buffer(&self.device, "measure line", &vertices),
        );
    }

    /// Sets a temporary world transform for one body without changing geometry.
    pub fn set_preview_transform(&mut self, body_id: BodyId, transform: Mat4) {
        self.preview_transforms.insert(body_id, transform);
    }

    /// Removes all temporary body transforms.
    pub fn clear_preview_transforms(&mut self) {
        self.preview_transforms.clear();
    }

    /// Temporarily omits one base body while a replacement preview is shown.
    pub fn set_suppressed_body(&mut self, body_id: Option<BodyId>) {
        self.suppressed_body = body_id;
    }

    /// Uploads or clears a coarse translucent tool preview mesh.
    pub fn set_preview_mesh(&mut self, preview: Option<(BodyMesh, Mat4)>) {
        self.preview_is_interference = false;
        self.upload_preview_mesh(preview);
    }

    /// Uploads or clears a red inspection/interference overlay.
    pub fn set_interference_mesh(&mut self, preview: Option<(BodyMesh, Mat4)>) {
        self.preview_is_interference = preview.is_some();
        self.upload_preview_mesh(preview);
    }

    fn upload_preview_mesh(&mut self, preview: Option<(BodyMesh, Mat4)>) {
        self.preview_mesh = preview.and_then(|(mesh, transform)| {
            if mesh.positions.is_empty() || mesh.indices.is_empty() {
                return None;
            }
            let vertices: Vec<_> = mesh
                .positions
                .iter()
                .enumerate()
                .map(|(index, &position)| Vertex {
                    position,
                    normal: mesh.normals.get(index).copied().unwrap_or([0.0, 0.0, 1.0]),
                    curvature: 0.0,
                })
                .collect();
            Some(PreviewGpu {
                vertices: vertex_buffer(&self.device, "preview vertices", &vertices),
                indices: index_buffer(&self.device, "preview indices", &mesh.indices),
                index_count: mesh.indices.len() as u32,
                transform,
            })
        });
    }

    /// Moves the adaptive grid to a sketch plane, or back to world XY.
    pub fn set_grid_plane(&mut self, plane: Option<SketchPlane>) {
        let plane = plane.unwrap_or_else(SketchPlane::xy);
        self.grid_transform = Mat4::from_cols(
            plane.x_axis.as_vec3().extend(0.0),
            plane.y_axis.as_vec3().extend(0.0),
            plane.normal().as_vec3().extend(0.0),
            plane.origin.as_vec3().extend(1.0),
        );
    }

    /// Rebuilds committed curves, cached profile fills, and rubber-band lines.
    pub fn upload_sketches(
        &mut self,
        sketches: &[Sketch],
        planes: &[ConstructionPlane],
        axes: &[ConstructionAxis],
        points: &[ConstructionPoint],
        pending: &[(Vec3, Vec3)],
        selected: &[SelItem],
    ) {
        let mut lines = Vec::new();
        let mut defined_lines = Vec::new();
        let mut construction_lines = Vec::new();
        let mut selected_lines = Vec::new();
        let mut fill_vertices = Vec::new();
        let mut fill_indices = Vec::new();
        let mut profile_ranges = Vec::new();
        for sketch in sketches.iter().filter(|sketch| sketch.visible) {
            let mut parameter_offset = 0;
            for (entity_index, entity) in sketch.entities.iter().enumerate() {
                let parameter_count = match &entity.geo {
                    SketchEntity::Line { .. } => 4,
                    SketchEntity::Circle { .. } => 3,
                    SketchEntity::Ellipse { .. } => 5,
                    SketchEntity::Arc { .. } => 6,
                    SketchEntity::Spline { points } => points.len() * 2,
                    SketchEntity::CvSpline { control, .. } => control.len() * 2,
                    SketchEntity::EllipseArc { .. } => 7,
                    SketchEntity::Point { .. } => 2,
                };
                let locked = (parameter_offset..parameter_offset + parameter_count)
                    .all(|parameter| sketch.pinned.contains(&parameter));
                let lines = if locked
                    || selected.contains(&SelItem::SketchEntity(sketch.id, entity_index))
                {
                    &mut selected_lines
                } else if entity.construction {
                    &mut construction_lines
                } else if sketch.defined.get(entity_index) == Some(&true) {
                    &mut defined_lines
                } else {
                    &mut lines
                };
                match &entity.geo {
                    SketchEntity::Line { a, b } => {
                        lines.push(line_vertex(sketch.plane.to_world(*a).as_vec3()));
                        lines.push(line_vertex(sketch.plane.to_world(*b).as_vec3()));
                    }
                    SketchEntity::Circle { center, radius } => {
                        const SEGMENTS: usize = 64;
                        for segment in 0..SEGMENTS {
                            let angle_a = segment as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
                            let angle_b =
                                (segment + 1) as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
                            for angle in [angle_a, angle_b] {
                                let point =
                                    *center + glam::DVec2::new(angle.cos(), angle.sin()) * *radius;
                                lines.push(line_vertex(sketch.plane.to_world(point).as_vec3()));
                            }
                        }
                    }
                    SketchEntity::Ellipse {
                        center,
                        major,
                        minor_ratio,
                    } => {
                        let points =
                            crate::sketch::sample_ellipse(*center, *major, *minor_ratio, 64);
                        for pair in points.windows(2) {
                            lines.push(line_vertex(sketch.plane.to_world(pair[0]).as_vec3()));
                            lines.push(line_vertex(sketch.plane.to_world(pair[1]).as_vec3()));
                        }
                    }
                    SketchEntity::Arc { start, end, mid } => {
                        let points = crate::sketch::sample_arc(*start, *mid, *end, 32);
                        for pair in points.windows(2) {
                            lines.push(line_vertex(sketch.plane.to_world(pair[0]).as_vec3()));
                            lines.push(line_vertex(sketch.plane.to_world(pair[1]).as_vec3()));
                        }
                    }
                    SketchEntity::Spline { points } => {
                        let points = sketch.spline_polyline(entity_index, points);
                        for pair in points.windows(2) {
                            lines.push(line_vertex(sketch.plane.to_world(pair[0]).as_vec3()));
                            lines.push(line_vertex(sketch.plane.to_world(pair[1]).as_vec3()));
                        }
                    }
                    SketchEntity::CvSpline { control, degree } => {
                        let points =
                            crate::sketch::sample_cv_spline(control, *degree, sketch.plane);
                        for pair in points.windows(2) {
                            lines.push(line_vertex(sketch.plane.to_world(pair[0]).as_vec3()));
                            lines.push(line_vertex(sketch.plane.to_world(pair[1]).as_vec3()));
                        }
                    }
                    SketchEntity::EllipseArc {
                        center,
                        major,
                        minor_ratio,
                        start_angle,
                        end_angle,
                    } => {
                        let points = crate::sketch::sample_ellipse_arc(
                            *center,
                            *major,
                            *minor_ratio,
                            *start_angle,
                            *end_angle,
                            32,
                        );
                        for pair in points.windows(2) {
                            lines.push(line_vertex(sketch.plane.to_world(pair[0]).as_vec3()));
                            lines.push(line_vertex(sketch.plane.to_world(pair[1]).as_vec3()));
                        }
                    }
                    SketchEntity::Point { at } => {
                        let d = 0.8;
                        for axis in [glam::DVec2::X, glam::DVec2::Y] {
                            lines
                                .push(line_vertex(sketch.plane.to_world(*at - axis * d).as_vec3()));
                            lines
                                .push(line_vertex(sketch.plane.to_world(*at + axis * d).as_vec3()));
                        }
                    }
                }
                if locked {
                    let anchor = match &entity.geo {
                        SketchEntity::Line { a, .. } | SketchEntity::Arc { start: a, .. } => {
                            Some(*a)
                        }
                        SketchEntity::Circle { center, .. }
                        | SketchEntity::Ellipse { center, .. } => Some(*center),
                        SketchEntity::Spline { points } => points.first().copied(),
                        SketchEntity::CvSpline { control, .. } => control.first().copied(),
                        SketchEntity::EllipseArc { center, .. } => Some(*center),
                        SketchEntity::Point { at } => Some(*at),
                    };
                    if let Some(anchor) = anchor {
                        let d = 0.55;
                        let corners = [
                            anchor + glam::DVec2::new(-d, -d),
                            anchor + glam::DVec2::new(d, -d),
                            anchor + glam::DVec2::new(d, d),
                            anchor + glam::DVec2::new(-d, d),
                        ];
                        for index in 0..4 {
                            selected_lines
                                .push(line_vertex(sketch.plane.to_world(corners[index]).as_vec3()));
                            selected_lines.push(line_vertex(
                                sketch.plane.to_world(corners[(index + 1) % 4]).as_vec3(),
                            ));
                        }
                    }
                }
                parameter_offset += parameter_count;
            }
            for (profile_index, profile) in sketch.profiles().iter().enumerate() {
                let Some(face) = sketch.to_face(profile) else {
                    continue;
                };
                let Ok(mesh) = face.as_shape().mesh(0.5) else {
                    continue;
                };
                let base = fill_vertices.len() as u32;
                let start = fill_indices.len() as u32;
                fill_vertices.extend(mesh.positions.iter().map(|point| Vertex {
                    position: point.as_vec3().to_array(),
                    normal: sketch.plane.normal().as_vec3().to_array(),
                    curvature: 0.0,
                }));
                fill_indices.extend(mesh.indices.iter().map(|index| base + *index));
                profile_ranges.push((
                    SelItem::Profile(sketch.id, profile_index),
                    start..fill_indices.len() as u32,
                ));
            }
        }
        for datum in planes.iter().filter(|plane| plane.visible) {
            let corners = [
                glam::DVec2::new(-60.0, -60.0),
                glam::DVec2::new(60.0, -60.0),
                glam::DVec2::new(60.0, 60.0),
                glam::DVec2::new(-60.0, 60.0),
            ];
            for index in 0..4 {
                lines.push(line_vertex(datum.plane.to_world(corners[index]).as_vec3()));
                lines.push(line_vertex(
                    datum.plane.to_world(corners[(index + 1) % 4]).as_vec3(),
                ));
            }
            let base = fill_vertices.len() as u32;
            let start = fill_indices.len() as u32;
            fill_vertices.extend(corners.map(|point| Vertex {
                position: datum.plane.to_world(point).as_vec3().to_array(),
                normal: datum.plane.normal().as_vec3().to_array(),
                curvature: 0.0,
            }));
            fill_indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            profile_ranges.push((SelItem::Plane(datum.id), start..fill_indices.len() as u32));
        }
        for datum in axes.iter().filter(|axis| axis.visible) {
            let target = if selected.contains(&SelItem::Axis(datum.id)) {
                &mut selected_lines
            } else {
                &mut construction_lines
            };
            target.push(line_vertex(
                (datum.origin - datum.direction * 60.0).as_vec3(),
            ));
            target.push(line_vertex(
                (datum.origin + datum.direction * 60.0).as_vec3(),
            ));
        }
        for datum in points.iter().filter(|point| point.visible) {
            let target = if selected.contains(&SelItem::Point(datum.id)) {
                &mut selected_lines
            } else {
                &mut construction_lines
            };
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                target.push(line_vertex((datum.position - axis * 1.2).as_vec3()));
                target.push(line_vertex((datum.position + axis * 1.2).as_vec3()));
            }
        }
        let pending_vertices: Vec<_> = pending
            .iter()
            .flat_map(|(a, b)| [line_vertex(*a), line_vertex(*b)])
            .collect();
        self.sketch = Some(SketchGpu {
            line_count: lines.len() as u32,
            lines: vertex_buffer_or_dummy(&self.device, "sketch lines", &lines),
            defined_line_count: defined_lines.len() as u32,
            defined_lines: vertex_buffer_or_dummy(
                &self.device,
                "defined sketch lines",
                &defined_lines,
            ),
            construction_line_count: construction_lines.len() as u32,
            construction_lines: vertex_buffer_or_dummy(
                &self.device,
                "construction sketch lines",
                &construction_lines,
            ),
            selected_count: selected_lines.len() as u32,
            selected: vertex_buffer_or_dummy(
                &self.device,
                "selected sketch lines",
                &selected_lines,
            ),
            pending_count: pending_vertices.len() as u32,
            pending: vertex_buffer_or_dummy(
                &self.device,
                "pending sketch lines",
                &pending_vertices,
            ),
            fill_vertices: vertex_buffer_or_dummy(
                &self.device,
                "sketch profile vertices",
                &fill_vertices,
            ),
            fill_indices: if fill_indices.is_empty() {
                index_buffer(&self.device, "sketch profile indices", &[0])
            } else {
                index_buffer(&self.device, "sketch profile indices", &fill_indices)
            },
            profile_ranges,
        });
    }

    fn upload_model_matrices(
        &self,
        gizmo: Option<&GizmoRender>,
        extrude_arrow: Option<&ExtrudeArrowRender>,
        section_arrow: Option<&ExtrudeArrowRender>,
    ) {
        let slot_count = self.body_ranges.len() + 5;
        assert!(
            slot_count as u64 <= MAX_MODEL_SLOTS,
            "too many visible bodies"
        );
        let mut bytes = vec![0_u8; slot_count * MODEL_STRIDE as usize];
        let mut write_slot = |slot: usize, matrix: Mat4, material: Material| {
            let model = Model {
                matrix: matrix.to_cols_array_2d(),
                base_color_metallic: [
                    material.base_color[0],
                    material.base_color[1],
                    material.base_color[2],
                    material.metallic,
                ],
                roughness: [material.roughness, 0.0, 0.0, 0.0],
            };
            let start = slot * MODEL_STRIDE as usize;
            bytes[start..start + size_of::<Model>()].copy_from_slice(bytemuck::bytes_of(&model));
        };
        write_slot(0, self.grid_transform, Material::default());
        for body in &self.body_ranges {
            write_slot(
                body.model_slot as usize,
                self.preview_transforms
                    .get(&body.id)
                    .copied()
                    .unwrap_or(Mat4::IDENTITY),
                body.material,
            );
        }
        if let Some(preview) = &self.preview_mesh {
            write_slot(
                self.body_ranges.len() + 1,
                preview.transform,
                Material::default(),
            );
        }
        if let Some(gizmo) = gizmo {
            write_slot(
                self.body_ranges.len() + 2,
                Mat4::from_translation(gizmo.pivot) * Mat4::from_scale(Vec3::splat(gizmo.scale)),
                Material::default(),
            );
        } else if let Some(arrow) = extrude_arrow {
            let rotation = Quat::from_rotation_arc(Vec3::Z, arrow.normal.normalize_or_zero());
            let translation = arrow.origin + arrow.normal.normalize_or_zero() * arrow.scale * 0.06;
            write_slot(
                self.body_ranges.len() + 2,
                Mat4::from_translation(translation)
                    * Mat4::from_quat(rotation)
                    * Mat4::from_scale(Vec3::splat(arrow.scale)),
                Material::default(),
            );
        }
        write_slot(
            self.body_ranges.len() + 3,
            Mat4::IDENTITY,
            Material::default(),
        );
        if let Some(arrow) = section_arrow {
            let rotation = Quat::from_rotation_arc(Vec3::Z, arrow.normal.normalize_or_zero());
            let translation = arrow.origin + arrow.normal.normalize_or_zero() * arrow.scale * 0.06;
            write_slot(
                self.body_ranges.len() + 4,
                Mat4::from_translation(translation)
                    * Mat4::from_quat(rotation)
                    * Mat4::from_scale(Vec3::splat(arrow.scale)),
                Material::default(),
            );
        }
        self.queue.write_buffer(&self.model_buffer, 0, &bytes);
    }

    /// Renders one frame and synchronously reads its resolved pixels.
    pub fn render(
        &self,
        camera: &OrbitCamera,
        width: u32,
        height: u32,
        show_grid: bool,
        display_mode: DisplayMode,
        analysis: AnalysisMode,
        visualize: bool,
        hovered: Option<SelItem>,
        hovered_edge: Option<(BodyId, u32)>,
        selected: &[SelItem],
        gizmo: Option<GizmoRender>,
        extrude_arrow: Option<ExtrudeArrowRender>,
        section_arrow: Option<ExtrudeArrowRender>,
        clip_plane: Option<[f32; 4]>,
        orientation_cube: OrientationCubeRender,
    ) -> RenderedFrame {
        let width = width.max(1);
        let height = height.max(1);
        let pitch = adaptive_pitch(camera.distance);
        let uniforms = Uniforms {
            view_projection: camera.view_projection().to_cols_array_2d(),
            eye_pitch: [camera.eye().x, camera.eye().y, camera.eye().z, pitch],
            clip_plane: clip_plane.unwrap_or([0.0, 0.0, 0.0, f32::MAX]),
            bg_top: self.canvas_theme.bg_top,
            bg_bottom: self.canvas_theme.bg_bottom,
            grid_minor: self.canvas_theme.grid_minor,
            grid_major: self.canvas_theme.grid_major,
            axis_x: self.canvas_theme.axis_x,
            axis_y: self.canvas_theme.axis_y,
            body: self.canvas_theme.body,
            edge: self.canvas_theme.edge,
            section_cap: self.canvas_theme.section_cap,
            analysis: match analysis {
                AnalysisMode::Off => [0.0, 0.0, 0.0, visualize as u8 as f32],
                AnalysisMode::Zebra => [1.0, 0.0, 0.0, visualize as u8 as f32],
                AnalysisMode::Curvature => [0.0, 1.0, 0.0, visualize as u8 as f32],
            },
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        let ribbon_uniform = |line_width_px| RibbonUniform {
            viewport_width_px: width as f32,
            viewport_height_px: height as f32,
            line_width_px,
            _padding: 0.0,
        };
        self.queue.write_buffer(
            &self.hover_ribbon_width,
            0,
            bytemuck::bytes_of(&ribbon_uniform(
                2.5 * orientation_cube.device_scale.max(1.0),
            )),
        );
        self.queue.write_buffer(
            &self.selected_ribbon_width,
            0,
            bytemuck::bytes_of(&ribbon_uniform(
                3.0 * orientation_cube.device_scale.max(1.0),
            )),
        );
        self.upload_model_matrices(
            gizmo.as_ref(),
            extrude_arrow.as_ref(),
            section_arrow.as_ref(),
        );
        let cube_uniforms = Uniforms {
            view_projection: orientation_cube::view_projection(camera.yaw, camera.pitch)
                .to_cols_array_2d(),
            eye_pitch: [0.0; 4],
            clip_plane: [0.0, 0.0, 0.0, f32::MAX],
            bg_top: self.canvas_theme.bg_top,
            bg_bottom: self.canvas_theme.bg_bottom,
            grid_minor: self.canvas_theme.grid_minor,
            grid_major: self.canvas_theme.grid_major,
            axis_x: self.canvas_theme.axis_x,
            axis_y: self.canvas_theme.axis_y,
            body: self.canvas_theme.body,
            edge: self.canvas_theme.cube_edge,
            section_cap: self.canvas_theme.section_cap,
            analysis: [0.0; 4],
        };
        self.queue.write_buffer(
            &self.orientation_cube.uniform_buffer,
            0,
            bytemuck::bytes_of(&cube_uniforms),
        );
        let output = texture(
            &self.device,
            "resolved BGRA",
            width,
            height,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = texture(
            &self.device,
            "MSAA color",
            width,
            height,
            SAMPLE_COUNT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());
        let msaa_view = msaa.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&output_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.047,
                            g: 0.067,
                            b: 0.094,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.base_bind_group, &[0]);
            pass.set_pipeline(&self.background_pipeline);
            pass.draw(0..3, 0..1);
            if show_grid {
                pass.set_pipeline(&self.grid_pipeline);
                pass.set_vertex_buffer(0, self.grid_vertices.slice(..));
                pass.draw(0..6, 0..1);
            }
            if let Some(images) = &self.reference_images {
                pass.set_pipeline(&self.reference_image_pipeline);
                pass.set_vertex_buffer(0, images.vertices.slice(..));
                for (group, range) in &images.draws {
                    pass.set_bind_group(1, group, &[]);
                    pass.draw(range.clone(), 0..1);
                }
            }
            if display_mode != DisplayMode::Wireframe {
                // Approximate closed-solid caps: front-face culling exposes the
                // clipped body's inward-facing shell in a flat cap colour.
                if clip_plane.is_some() {
                    pass.set_pipeline(&self.section_cap_pipeline);
                    pass.set_vertex_buffer(0, self.mesh_vertices.slice(..));
                    pass.set_index_buffer(self.mesh_indices.slice(..), wgpu::IndexFormat::Uint32);
                    for body in &self.body_ranges {
                        if self.suppressed_body != Some(body.id) && !body.double_sided {
                            pass.set_bind_group(
                                0,
                                &self.base_bind_group,
                                &[body.model_slot * MODEL_STRIDE as u32],
                            );
                            pass.draw_indexed(body.mesh.clone(), 0, 0..1);
                        }
                    }
                }
                pass.set_vertex_buffer(0, self.mesh_vertices.slice(..));
                pass.set_index_buffer(self.mesh_indices.slice(..), wgpu::IndexFormat::Uint32);
                for body in &self.body_ranges {
                    if self.suppressed_body == Some(body.id) {
                        continue;
                    }
                    pass.set_pipeline(match (display_mode, body.double_sided) {
                        (DisplayMode::XRay, true) => &self.surface_xray_pipeline,
                        (DisplayMode::XRay, false) => &self.xray_pipeline,
                        (_, true) => &self.surface_mesh_pipeline,
                        (_, false) => &self.mesh_pipeline,
                    });
                    pass.set_bind_group(
                        0,
                        &self.base_bind_group,
                        &[body.model_slot * MODEL_STRIDE as u32],
                    );
                    pass.draw_indexed(body.mesh.clone(), 0, 0..1);
                }
            }
            pass.set_pipeline(&self.body_edge_pipeline);
            pass.set_vertex_buffer(0, self.edge_vertices.slice(..));
            for body in &self.body_ranges {
                if self.suppressed_body == Some(body.id) {
                    continue;
                }
                pass.set_bind_group(
                    0,
                    &self.base_bind_group,
                    &[body.model_slot * MODEL_STRIDE as u32],
                );
                pass.draw(body.edge_lines.clone(), 0..1);
            }
            if display_mode == DisplayMode::HiddenEdges {
                pass.set_pipeline(&self.hidden_edge_pipeline);
                for body in &self.body_ranges {
                    if self.suppressed_body == Some(body.id) {
                        continue;
                    }
                    pass.set_bind_group(
                        0,
                        &self.hidden_edge_bind_group,
                        &[body.model_slot * MODEL_STRIDE as u32],
                    );
                    pass.draw(body.edge_lines.clone(), 0..1);
                }
            }

            if self.measure_count > 0 {
                let offset = (self.body_ranges.len() as u32 + 3) * MODEL_STRIDE as u32;
                pass.set_pipeline(&self.accent_line_pipeline);
                pass.set_bind_group(0, &self.sketch_accent_bind_group, &[offset]);
                pass.set_vertex_buffer(0, self.measure_vertices.slice(..));
                pass.draw(0..self.measure_count, 0..1);
            }

            if let Some(preview) = &self.preview_mesh {
                let offset = (self.body_ranges.len() as u32 + 1) * MODEL_STRIDE as u32;
                pass.set_pipeline(&self.preview_pipeline);
                let group = if self.preview_is_interference {
                    &self.interference_bind_group
                } else {
                    &self.preview_bind_group
                };
                pass.set_bind_group(0, group, &[offset]);
                pass.set_vertex_buffer(0, preview.vertices.slice(..));
                pass.set_index_buffer(preview.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..preview.index_count, 0, 0..1);
            }

            if let Some(sketch) = &self.sketch {
                let offset = (self.body_ranges.len() as u32 + 3) * MODEL_STRIDE as u32;
                pass.set_pipeline(&self.fill_pipeline);
                pass.set_vertex_buffer(0, sketch.fill_vertices.slice(..));
                pass.set_index_buffer(sketch.fill_indices.slice(..), wgpu::IndexFormat::Uint32);
                for (item, range) in &sketch.profile_ranges {
                    let group = if selected.contains(item) {
                        &self.sketch_selected_bind_group
                    } else if hovered == Some(*item) {
                        &self.sketch_hover_bind_group
                    } else {
                        &self.sketch_fill_bind_group
                    };
                    pass.set_bind_group(0, group, &[offset]);
                    pass.draw_indexed(range.clone(), 0, 0..1);
                }
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.sketch_line_bind_group, &[offset]);
                pass.set_vertex_buffer(0, sketch.lines.slice(..));
                pass.draw(0..sketch.line_count, 0..1);
                pass.set_bind_group(0, &self.sketch_defined_bind_group, &[offset]);
                pass.set_vertex_buffer(0, sketch.defined_lines.slice(..));
                pass.draw(0..sketch.defined_line_count, 0..1);
                pass.set_bind_group(0, &self.sketch_construction_bind_group, &[offset]);
                pass.set_vertex_buffer(0, sketch.construction_lines.slice(..));
                pass.draw(0..sketch.construction_line_count, 0..1);
                pass.set_pipeline(&self.accent_line_pipeline);
                pass.set_bind_group(0, &self.sketch_accent_bind_group, &[offset]);
                pass.set_vertex_buffer(0, sketch.selected.slice(..));
                pass.draw(0..sketch.selected_count, 0..1);
                pass.set_vertex_buffer(0, sketch.pending.slice(..));
                pass.draw(0..sketch.pending_count, 0..1);
            }

            if let Some(item) = hovered {
                self.draw_highlight(&mut pass, item, false, visualize);
            }
            if let Some((body, edge)) = hovered_edge {
                self.draw_highlight(&mut pass, SelItem::Edge(body, edge), false, visualize);
            }
            for &item in selected {
                self.draw_highlight(&mut pass, item, true, visualize);
            }
            if let Some(gizmo) = &gizmo {
                self.draw_gizmo(&mut pass, gizmo);
            } else if let Some(arrow) = &extrude_arrow {
                self.draw_arrow(&mut pass, arrow, self.body_ranges.len() + 2);
            }
            if let Some(arrow) = &section_arrow {
                self.draw_arrow(&mut pass, arrow, self.body_ranges.len() + 4);
            }
        }
        {
            let rect = orientation_cube::cube_rect(width, height, orientation_cube.device_scale);
            let scissor_x = rect.x.round() as u32;
            let scissor_y = rect.y.round() as u32;
            let scissor_width = rect.size.round() as u32;
            let scissor_height = rect.size.round() as u32;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("orientation cube pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&output_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(rect.x, rect.y, rect.size, rect.size, 0.0, 1.0);
            pass.set_scissor_rect(scissor_x, scissor_y, scissor_width, scissor_height);
            pass.set_pipeline(&self.cube_pipeline);
            pass.set_bind_group(1, &self.orientation_cube.label_bind_group, &[]);
            pass.set_vertex_buffer(0, self.orientation_cube.vertices.slice(..));
            pass.set_index_buffer(
                self.orientation_cube.indices.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (index, (region, range)) in self.orientation_cube.ranges.iter().enumerate() {
                let groups = if orientation_cube.hovered == Some(*region) {
                    &self.orientation_cube.hover_groups
                } else {
                    &self.orientation_cube.normal_groups
                };
                pass.set_bind_group(0, &groups[index], &[0]);
                pass.draw_indexed(range.clone(), 0, 0..1);
            }
        }
        let unpadded_bytes_per_row = width * 4;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BGRA readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("wgpu polling failed");
        receiver
            .recv()
            .expect("map callback dropped")
            .expect("readback mapping failed");
        let mapped = readback.slice(..).get_mapped_range();
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for row in mapped
            .chunks_exact(padded_bytes_per_row as usize)
            .take(height as usize)
        {
            bgra.extend_from_slice(&row[..unpadded_bytes_per_row as usize]);
        }
        drop(mapped);
        readback.unmap();
        RenderedFrame {
            width,
            height,
            bgra,
        }
    }

    fn draw_highlight<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        item: SelItem,
        selected: bool,
        visualize: bool,
    ) {
        let Some(body_id) = item.body_id() else {
            return;
        };
        let Some(body) = self.body_ranges.iter().find(|body| body.id == body_id) else {
            return;
        };
        if self.suppressed_body == Some(body.id) {
            return;
        }
        pass.set_bind_group(
            0,
            if selected {
                if visualize {
                    &self.visualize_selected_bind_group
                } else {
                    &self.selected_bind_group
                }
            } else {
                &self.hover_bind_group
            },
            &[body.model_slot * MODEL_STRIDE as u32],
        );
        match item {
            SelItem::Body(_) => {
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_vertex_buffer(0, self.mesh_vertices.slice(..));
                pass.set_index_buffer(self.mesh_indices.slice(..), wgpu::IndexFormat::Uint32);
                for range in &body.faces {
                    pass.draw_indexed(range.clone(), 0, 0..1);
                }
            }
            SelItem::Face(_, face) => {
                if let Some(range) = body.faces.get(face as usize) {
                    pass.set_pipeline(&self.overlay_pipeline);
                    pass.set_vertex_buffer(0, self.mesh_vertices.slice(..));
                    pass.set_index_buffer(self.mesh_indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(range.clone(), 0, 0..1);
                }
            }
            SelItem::Edge(_, edge) => {
                if let Some(range) = body.ribbon_edges.get(edge as usize) {
                    pass.set_pipeline(&self.ribbon_pipeline);
                    pass.set_bind_group(
                        0,
                        if selected {
                            &self.selected_ribbon_bind_group
                        } else {
                            &self.hover_ribbon_bind_group
                        },
                        &[body.model_slot * MODEL_STRIDE as u32],
                    );
                    pass.set_bind_group(
                        2,
                        if selected {
                            &self.selected_ribbon_width_group
                        } else {
                            &self.hover_ribbon_width_group
                        },
                        &[],
                    );
                    pass.set_vertex_buffer(0, self.ribbon_vertices.slice(..));
                    pass.set_index_buffer(self.ribbon_indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(range.clone(), 0, 0..1);
                }
            }
            SelItem::Profile(_, _)
            | SelItem::SketchEntity(_, _)
            | SelItem::Plane(_)
            | SelItem::Axis(_)
            | SelItem::Point(_) => {}
        }
    }

    fn draw_gizmo<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, state: &GizmoRender) {
        let offset = (self.body_ranges.len() as u32 + 2) * MODEL_STRIDE as u32;
        pass.set_pipeline(&self.gizmo_pipeline);
        pass.set_vertex_buffer(0, self.gizmo.vertices.slice(..));
        pass.set_index_buffer(self.gizmo.indices.slice(..), wgpu::IndexFormat::Uint32);
        for (index, (handle, range)) in self.gizmo.ranges.iter().enumerate() {
            let groups = if state.hovered == Some(*handle) {
                &self.gizmo.bright_groups
            } else {
                &self.gizmo.normal_groups
            };
            pass.set_bind_group(0, &groups[index], &[offset]);
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }

    fn draw_arrow<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        state: &ExtrudeArrowRender,
        model_slot: usize,
    ) {
        let offset = model_slot as u32 * MODEL_STRIDE as u32;
        pass.set_pipeline(&self.gizmo_pipeline);
        pass.set_vertex_buffer(0, self.arrow.vertices.slice(..));
        pass.set_index_buffer(self.arrow.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group(0, &self.arrow.rim_group, &[offset]);
        pass.draw_indexed(self.arrow.rim_range.clone(), 0, 0..1);
        pass.set_bind_group(
            0,
            if state.hovered {
                &self.arrow.hover_group
            } else {
                &self.arrow.fill_group
            },
            &[offset],
        );
        pass.draw_indexed(self.arrow.fill_range.clone(), 0, 0..1);
    }
}

fn tint_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    model_buffer: &wgpu::Buffer,
    label: &str,
    color: [f32; 4],
) -> (wgpu::BindGroup, wgpu::Buffer) {
    let tint = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::bytes_of(&Tint { color }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: tint.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: model_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<Model>() as u64),
                }),
            },
        ],
    });
    (bind_group, tint)
}

fn ribbon_width_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    label: &str,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::bytes_of(&RibbonUniform::zeroed()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    (buffer, group)
}

fn vertex_buffer(device: &wgpu::Device, label: &str, vertices: &[Vertex]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn ribbon_vertex_buffer(
    device: &wgpu::Device,
    label: &str,
    vertices: &[RibbonVertex],
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn append_ribbon_segment(
    vertices: &mut Vec<RibbonVertex>,
    indices: &mut Vec<u32>,
    a: Vec3,
    b: Vec3,
) {
    let base = vertices.len() as u32;
    vertices.extend([
        RibbonVertex {
            position: a.to_array(),
            partner: b.to_array(),
            side: -1.0,
        },
        RibbonVertex {
            position: a.to_array(),
            partner: b.to_array(),
            side: 1.0,
        },
        RibbonVertex {
            position: b.to_array(),
            partner: a.to_array(),
            side: 1.0,
        },
        RibbonVertex {
            position: b.to_array(),
            partner: a.to_array(),
            side: -1.0,
        },
    ]);
    indices.extend([base, base + 2, base + 1, base + 1, base + 2, base + 3]);
}

#[cfg(test)]
fn ribbon_perpendicular_offset(
    a: glam::Vec4,
    b: glam::Vec4,
    viewport: glam::Vec2,
    line_width_px: f32,
    side: f32,
) -> glam::Vec2 {
    if a.w.abs() <= f32::EPSILON || b.w.abs() <= f32::EPSILON {
        return glam::Vec2::ZERO;
    }
    let delta = (b.truncate().truncate() / b.w - a.truncate().truncate() / a.w) * viewport;
    let length = delta.length();
    if !length.is_finite() || length <= f32::EPSILON {
        return glam::Vec2::ZERO;
    }
    glam::Vec2::new(-delta.y, delta.x) / length * line_width_px * side
        / viewport.max(glam::Vec2::ONE)
}

fn vertex_buffer_or_dummy(device: &wgpu::Device, label: &str, vertices: &[Vertex]) -> wgpu::Buffer {
    if vertices.is_empty() {
        vertex_buffer(device, label, &[Vertex::zeroed()])
    } else {
        vertex_buffer(device, label, vertices)
    }
}

fn line_vertex(position: Vec3) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: [0.0; 3],
        curvature: 0.0,
    }
}

fn cube_vertex_buffer(device: &wgpu::Device, label: &str, vertices: &[CubeVertex]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn index_buffer(device: &wgpu::Device, label: &str, indices: &[u32]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::INDEX,
    })
}

fn create_background_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vertical gradient background"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_background"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_background"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn cull_mode(fragment: &str, topology: wgpu::PrimitiveTopology) -> Option<wgpu::Face> {
    // Solid-body passes cull back faces (OCCT per-face winding is outward);
    // the grid quad must stay visible from below and lines have no facing.
    (topology == wgpu::PrimitiveTopology::TriangleList
        && matches!(fragment, "fs_mesh" | "fs_tint" | "fs_tint_clipped"))
    .then_some(wgpu::Face::Back)
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex: &str,
    fragment: &str,
    topology: wgpu::PrimitiveTopology,
    depth_write: bool,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                // Must match vs inputs exactly: curvature is always present in
                // the vertex struct (0.0 outside analysis mode), so every
                // pipeline over Vertex declares all three attributes.
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            cull_mode: cull_mode(fragment, topology),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: if fragment == "fs_grid" {
                wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                }
            } else {
                Default::default()
            },
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn create_ribbon_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("emphasized edge ribbon"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_ribbon"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<RibbonVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_tint_clipped"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn create_special_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex: &str,
    fragment: &str,
    topology: wgpu::PrimitiveTopology,
    cull_mode: Option<wgpu::Face>,
    depth_compare: wgpu::CompareFunction,
    depth_write: bool,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn create_cube_pipeline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    atlas: Option<Vec<u8>>,
) -> (wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Texture) {
    let label_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("orientation cube label layout"),
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
        label: Some("orientation cube pipeline layout"),
        bind_group_layouts: &[Some(scene_layout), Some(&label_layout)],
        immediate_size: 0,
    });
    let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("orientation cube label atlas"),
        size: wgpu::Extent3d {
            width: orientation_cube::LABEL_ATLAS_WIDTH,
            height: orientation_cube::LABEL_CELL_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let atlas = atlas.unwrap_or_else(|| {
        vec![0; (orientation_cube::LABEL_ATLAS_WIDTH * orientation_cube::LABEL_CELL_SIZE) as usize]
    });
    queue.write_texture(
        atlas_texture.as_image_copy(),
        &atlas,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(orientation_cube::LABEL_ATLAS_WIDTH),
            rows_per_image: Some(orientation_cube::LABEL_CELL_SIZE),
        },
        atlas_texture.size(),
    );
    let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("orientation cube label sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let label_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("orientation cube labels"),
        layout: &label_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("orientation cube"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_cube"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<CubeVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3,
                    1 => Float32x3,
                    2 => Float32x2,
                    3 => Float32
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_cube"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });
    (pipeline, label_bind_group, atlas_texture)
}

fn cube_region_color(theme: CanvasTheme, region: Region) -> [f32; 4] {
    match region.axis_count() {
        1 => theme.cube_face,
        2 => theme.cube_chamfer,
        _ => theme.cube_edge,
    }
}

fn create_gizmo_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("always-visible gizmo"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_tint"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn create_preview_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_translucent_pipeline(
        device,
        layout,
        shader,
        "translucent extrude preview",
        "vs_main",
        "fs_tint_clipped",
    )
}

/// Sketch profile fills are coplanar with the sketch-plane grid, which writes
/// depth first; without the vs_overlay z-pull they lose the depth test and
/// disappear.
fn create_fill_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_translucent_pipeline(
        device,
        layout,
        shader,
        "sketch profile fill",
        "vs_overlay",
        "fs_tint_clipped",
    )
}

fn create_translucent_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    vertex_entry: &str,
    fragment_entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex_entry),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                // Must match vs inputs exactly: curvature is always present in
                // the vertex struct (0.0 outside analysis mode), so every
                // pipeline over Vertex declares all three attributes.
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: COLOR_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn orientation_cube_geometry() -> (Vec<CubeVertex>, Vec<u32>, Vec<(Region, Range<u32>)>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut ranges = Vec::with_capacity(Region::ALL.len());
    for region in Region::ALL {
        let start = indices.len() as u32;
        let signs = region.signs;
        let active: Vec<_> = (0..3).filter(|&axis| signs[axis] != 0).collect();
        let points = match active.as_slice() {
            &[axis] => {
                let others: Vec<_> = (0..3).filter(|&other| other != axis).collect();
                let mut center = Vec3::ZERO;
                center[axis] = f32::from(signs[axis]);
                let mut first = Vec3::ZERO;
                first[others[0]] = orientation_cube::FACE_ZONE;
                let mut second = Vec3::ZERO;
                second[others[1]] = orientation_cube::FACE_ZONE;
                vec![
                    center - first - second,
                    center + first - second,
                    center + first + second,
                    center - first + second,
                ]
            }
            &[first_axis, second_axis] => {
                let free_axis = (0..3)
                    .find(|axis| *axis != first_axis && *axis != second_axis)
                    .expect("edge has one free axis");
                let mut first = Vec3::ZERO;
                first[first_axis] = f32::from(signs[first_axis]);
                first[second_axis] = f32::from(signs[second_axis]) * orientation_cube::FACE_ZONE;
                first[free_axis] = -orientation_cube::FACE_ZONE;
                let mut second = Vec3::ZERO;
                second[first_axis] = f32::from(signs[first_axis]) * orientation_cube::FACE_ZONE;
                second[second_axis] = f32::from(signs[second_axis]);
                second[free_axis] = -orientation_cube::FACE_ZONE;
                let mut third = second;
                third[free_axis] = orientation_cube::FACE_ZONE;
                let mut fourth = first;
                fourth[free_axis] = orientation_cube::FACE_ZONE;
                vec![first, second, third, fourth]
            }
            &[0, 1, 2] => {
                let signed = Vec3::new(
                    f32::from(signs[0]),
                    f32::from(signs[1]),
                    f32::from(signs[2]),
                );
                vec![
                    signed
                        * Vec3::new(
                            1.0,
                            orientation_cube::FACE_ZONE,
                            orientation_cube::FACE_ZONE,
                        ),
                    signed
                        * Vec3::new(
                            orientation_cube::FACE_ZONE,
                            1.0,
                            orientation_cube::FACE_ZONE,
                        ),
                    signed
                        * Vec3::new(
                            orientation_cube::FACE_ZONE,
                            orientation_cube::FACE_ZONE,
                            1.0,
                        ),
                ]
            }
            _ => unreachable!("orientation cube region must have one to three axes"),
        };
        add_cube_polygon(&mut vertices, &mut indices, &points, region);
        ranges.push((region, start..indices.len() as u32));
    }
    (vertices, indices, ranges)
}

fn add_cube_polygon(
    vertices: &mut Vec<CubeVertex>,
    indices: &mut Vec<u32>,
    points: &[Vec3],
    region: Region,
) {
    let base = vertices.len() as u32;
    let label = orientation_cube::face_label_for(region);
    vertices.extend(points.iter().map(|point| {
        let (label_uv, label_cell) = label.map_or(([0.0; 2], -1.0), |label| {
            let right = Vec3::new(
                f32::from(label.right[0]),
                f32::from(label.right[1]),
                f32::from(label.right[2]),
            );
            let up = Vec3::new(
                f32::from(label.up[0]),
                f32::from(label.up[1]),
                f32::from(label.up[2]),
            );
            const LABEL_FRACTION: f32 = 0.55;
            let margin = (1.0 - LABEL_FRACTION) * 0.5;
            let face_u = 0.5 + point.dot(right) / orientation_cube::FACE_ZONE * 0.5;
            let face_v = 0.5 - point.dot(up) / orientation_cube::FACE_ZONE * 0.5;
            let cell_u = (face_u - margin) / LABEL_FRACTION;
            let cell_v = (face_v - margin) / LABEL_FRACTION;
            (
                [
                    (label.cell as f32 + cell_u) / orientation_cube::LABEL_CELL_COUNT as f32,
                    cell_v,
                ],
                label.cell as f32,
            )
        });
        CubeVertex {
            position: point.to_array(),
            normal: region.direction().to_array(),
            label_uv,
            label_cell,
        }
    }));
    let reversed = (points[1] - points[0])
        .cross(points[2] - points[0])
        .dot(region.direction())
        < 0.0;
    for index in 1..points.len() - 1 {
        if reversed {
            indices.extend_from_slice(&[base, base + index as u32 + 1, base + index as u32]);
        } else {
            indices.extend_from_slice(&[base, base + index as u32, base + index as u32 + 1]);
        }
    }
}

fn gizmo_geometry() -> (Vec<Vertex>, Vec<u32>, [(Handle, Range<u32>); 7]) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut ranges = Vec::new();
    for (handle, axis) in [
        (Handle::AxisX, Vec3::X),
        (Handle::AxisY, Vec3::Y),
        (Handle::AxisZ, Vec3::Z),
    ] {
        let start = indices.len() as u32;
        add_arrow(&mut vertices, &mut indices, axis);
        ranges.push((handle, start..indices.len() as u32));
    }
    for (handle, normal) in [
        (Handle::RingX, Vec3::X),
        (Handle::RingY, Vec3::Y),
        (Handle::RingZ, Vec3::Z),
    ] {
        let start = indices.len() as u32;
        add_torus(&mut vertices, &mut indices, normal);
        ranges.push((handle, start..indices.len() as u32));
    }
    let start = indices.len() as u32;
    add_sphere(&mut vertices, &mut indices);
    ranges.push((Handle::Center, start..indices.len() as u32));
    let ranges: [(Handle, Range<u32>); 7] = ranges.try_into().expect("seven gizmo parts");
    (vertices, indices, ranges)
}

fn perpendicular_basis(axis: Vec3) -> (Vec3, Vec3) {
    let first = if axis.x.abs() < 0.8 {
        axis.cross(Vec3::X).normalize()
    } else {
        axis.cross(Vec3::Y).normalize()
    };
    (first, axis.cross(first).normalize())
}

fn add_arrow(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, axis: Vec3) {
    const SEGMENTS: u32 = 16;
    let (u, v) = perpendicular_basis(axis);
    let base = vertices.len() as u32;
    for height in [0.16, 0.76] {
        for segment in 0..SEGMENTS {
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let radial = u * angle.cos() + v * angle.sin();
            vertices.push(Vertex {
                position: (axis * height + radial * 0.025).to_array(),
                normal: radial.to_array(),
                curvature: 0.0,
            });
        }
    }
    for segment in 0..SEGMENTS {
        let next = (segment + 1) % SEGMENTS;
        indices.extend_from_slice(&[
            base + segment,
            base + SEGMENTS + segment,
            base + SEGMENTS + next,
            base + segment,
            base + SEGMENTS + next,
            base + next,
        ]);
    }
    let cone_base = vertices.len() as u32;
    for segment in 0..SEGMENTS {
        let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let radial = u * angle.cos() + v * angle.sin();
        vertices.push(Vertex {
            position: (axis * 0.73 + radial * 0.075).to_array(),
            normal: (radial * 0.92 + axis * 0.38).normalize().to_array(),
            curvature: 0.0,
        });
    }
    let tip = vertices.len() as u32;
    vertices.push(Vertex {
        position: axis.to_array(),
        normal: axis.to_array(),
        curvature: 0.0,
    });
    for segment in 0..SEGMENTS {
        indices.extend_from_slice(&[
            cone_base + segment,
            tip,
            cone_base + (segment + 1) % SEGMENTS,
        ]);
    }
}

fn double_arrow_geometry() -> (Vec<Vertex>, Vec<u32>, Range<u32>, Range<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let rim_start = indices.len() as u32;
    add_double_arrow(&mut vertices, &mut indices, 0.032, 0.080, 0.300, 0.158);
    let rim_range = rim_start..indices.len() as u32;
    let fill_start = indices.len() as u32;
    add_double_arrow(&mut vertices, &mut indices, 0.023, 0.070, 0.291, 0.164);
    let fill_range = fill_start..indices.len() as u32;
    (vertices, indices, rim_range, fill_range)
}

fn add_double_arrow(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    shaft_radius: f32,
    head_radius: f32,
    tip: f32,
    head_base: f32,
) {
    const SEGMENTS: u32 = 20;
    let base = vertices.len() as u32;
    for z in [-head_base, head_base] {
        for segment in 0..SEGMENTS {
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let radial = Vec3::new(angle.cos(), angle.sin(), 0.0);
            vertices.push(Vertex {
                position: [radial.x * shaft_radius, radial.y * shaft_radius, z],
                normal: radial.to_array(),
                curvature: 0.0,
            });
        }
    }
    for segment in 0..SEGMENTS {
        let next = (segment + 1) % SEGMENTS;
        indices.extend_from_slice(&[
            base + segment,
            base + SEGMENTS + segment,
            base + SEGMENTS + next,
            base + segment,
            base + SEGMENTS + next,
            base + next,
        ]);
    }
    for sign in [-1.0_f32, 1.0] {
        let cone_base = vertices.len() as u32;
        for segment in 0..SEGMENTS {
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let radial = Vec3::new(angle.cos(), angle.sin(), 0.0);
            vertices.push(Vertex {
                position: [
                    radial.x * head_radius,
                    radial.y * head_radius,
                    sign * head_base,
                ],
                normal: (radial * 0.88 + Vec3::Z * sign * 0.48)
                    .normalize()
                    .to_array(),
                curvature: 0.0,
            });
        }
        let tip_index = vertices.len() as u32;
        vertices.push(Vertex {
            position: [0.0, 0.0, sign * tip],
            normal: (Vec3::Z * sign).to_array(),
            curvature: 0.0,
        });
        for segment in 0..SEGMENTS {
            let next = (segment + 1) % SEGMENTS;
            if sign > 0.0 {
                indices.extend_from_slice(&[cone_base + segment, tip_index, cone_base + next]);
            } else {
                indices.extend_from_slice(&[cone_base + next, tip_index, cone_base + segment]);
            }
        }
    }
}

fn add_torus(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, normal: Vec3) {
    const MAJOR: u32 = 64;
    const MINOR: u32 = 6;
    let (u, v) = perpendicular_basis(normal);
    let base = vertices.len() as u32;
    for major in 0..MAJOR {
        let angle = major as f32 / MAJOR as f32 * std::f32::consts::TAU;
        let radial = u * angle.cos() + v * angle.sin();
        for minor in 0..MINOR {
            let tube_angle = minor as f32 / MINOR as f32 * std::f32::consts::TAU;
            let tube_normal = radial * tube_angle.cos() + normal * tube_angle.sin();
            vertices.push(Vertex {
                position: (radial * (0.70 + 0.016 * tube_angle.cos())
                    + normal * (0.016 * tube_angle.sin()))
                .to_array(),
                normal: tube_normal.to_array(),
                curvature: 0.0,
            });
        }
    }
    for major in 0..MAJOR {
        for minor in 0..MINOR {
            let a = base + major * MINOR + minor;
            let b = base + ((major + 1) % MAJOR) * MINOR + minor;
            let c = base + ((major + 1) % MAJOR) * MINOR + (minor + 1) % MINOR;
            let d = base + major * MINOR + (minor + 1) % MINOR;
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
}

fn add_sphere(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    const LATITUDE: u32 = 10;
    const LONGITUDE: u32 = 16;
    let base = vertices.len() as u32;
    for latitude in 0..=LATITUDE {
        let polar = latitude as f32 / LATITUDE as f32 * std::f32::consts::PI;
        for longitude in 0..LONGITUDE {
            let azimuth = longitude as f32 / LONGITUDE as f32 * std::f32::consts::TAU;
            let normal = Vec3::new(
                polar.sin() * azimuth.cos(),
                polar.sin() * azimuth.sin(),
                polar.cos(),
            );
            vertices.push(Vertex {
                position: (normal * 0.11).to_array(),
                normal: normal.to_array(),
                curvature: 0.0,
            });
        }
    }
    for latitude in 0..LATITUDE {
        for longitude in 0..LONGITUDE {
            let next = (longitude + 1) % LONGITUDE;
            let a = base + latitude * LONGITUDE + longitude;
            let b = base + (latitude + 1) * LONGITUDE + longitude;
            let c = base + (latitude + 1) * LONGITUDE + next;
            let d = base + latitude * LONGITUDE + next;
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    sample_count: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage,
        view_formats: &[],
    })
}

const SHADER: &str = r#"
struct Uniforms {
    view_projection: mat4x4<f32>,
    eye_pitch: vec4<f32>,
    clip_plane: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    grid_minor: vec4<f32>,
    grid_major: vec4<f32>,
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    body: vec4<f32>,
    edge: vec4<f32>,
    section_cap: vec4<f32>,
    analysis: vec4<f32>,
};
struct Tint { color: vec4<f32> };
struct RibbonUniform { viewport_width_px: f32, viewport_height_px: f32, line_width_px: f32, padding: f32 };
struct Model {
    matrix: mat4x4<f32>,
    base_color_metallic: vec4<f32>,
    roughness: vec4<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<uniform> tint: Tint;
@group(0) @binding(2) var<uniform> model: Model;
@group(2) @binding(0) var<uniform> ribbon: RibbonUniform;
@group(1) @binding(0) var cube_labels: texture_2d<f32>;
@group(1) @binding(1) var cube_label_sampler: sampler;
struct Out { @builtin(position) clip: vec4<f32>, @location(0) world: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) local: vec3<f32>, @location(3) curvature: f32 };
struct CubeOut { @builtin(position) clip: vec4<f32>, @location(0) label_uv: vec2<f32>, @location(1) @interpolate(flat) label_cell: f32 };
struct BackgroundOut { @builtin(position) clip: vec4<f32>, @location(0) vertical: f32 };
@vertex fn vs_background(@builtin(vertex_index) index: u32) -> BackgroundOut {
    let positions = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: BackgroundOut; out.clip = vec4(positions[index], 0.999, 1.0); out.vertical = positions[index].y * 0.5 + 0.5; return out;
}
@fragment fn fs_background(input: BackgroundOut) -> @location(0) vec4<f32> {
    return mix(uniforms.bg_bottom, uniforms.bg_top, clamp(input.vertical, 0.0, 1.0));
}
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) curvature: f32) -> Out {
    let world = model.matrix * vec4(position, 1.0);
    var out: Out; out.world = world.xyz; out.local = position; out.normal = (model.matrix * vec4(normal, 0.0)).xyz; out.curvature = curvature; out.clip = uniforms.view_projection * world; return out;
}
@vertex fn vs_cube(
    @location(0) position: vec3<f32>,
    @location(1) _normal: vec3<f32>,
    @location(2) label_uv: vec2<f32>,
    @location(3) label_cell: f32,
) -> CubeOut {
    let world = model.matrix * vec4(position, 1.0);
    var out: CubeOut;
    out.clip = uniforms.view_projection * world;
    out.label_uv = label_uv;
    out.label_cell = label_cell;
    return out;
}
@vertex fn vs_reference(@location(0) position: vec3<f32>, @location(1) _normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) _unused: f32) -> CubeOut {
    var out: CubeOut; out.clip = uniforms.view_projection * vec4(position, 1.0); out.label_uv = uv; out.label_cell = 0.0; return out;
}
@fragment fn fs_reference(input: CubeOut) -> @location(0) vec4<f32> {
    let color=textureSample(cube_labels,cube_label_sampler,input.label_uv); return vec4(color.rgb,color.a*0.9);
}
@vertex fn vs_overlay(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) curvature: f32) -> Out {
    let world = model.matrix * vec4(position, 1.0);
    var out: Out; out.world = world.xyz; out.local = position; out.normal = (model.matrix * vec4(normal, 0.0)).xyz; out.curvature = curvature; out.clip = uniforms.view_projection * world; out.clip.z -= 1e-4 * out.clip.w; return out;
}
@vertex fn vs_line(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) curvature: f32) -> Out {
    let world = model.matrix * vec4(position, 1.0);
    var out: Out; out.world = world.xyz; out.local = position; out.normal = (model.matrix * vec4(normal, 0.0)).xyz; out.curvature = curvature; out.clip = uniforms.view_projection * world; out.clip.z -= 2e-4 * out.clip.w; return out;
}
@vertex fn vs_ribbon(@location(0) position: vec3<f32>, @location(1) partner: vec3<f32>, @location(2) side: f32) -> Out {
    let world = model.matrix * vec4(position, 1.0);
    let partner_world = model.matrix * vec4(partner, 1.0);
    var clip = uniforms.view_projection * world;
    let partner_clip = uniforms.view_projection * partner_world;
    let viewport = max(vec2(ribbon.viewport_width_px, ribbon.viewport_height_px), vec2(1.0));
    var delta = vec2(0.0);
    if (abs(clip.w) > 1e-6 && abs(partner_clip.w) > 1e-6) {
        delta = (partner_clip.xy / partner_clip.w - clip.xy / clip.w) * viewport;
    }
    let segment_length = length(delta);
    var offset = vec2(0.0);
    if (segment_length > 1e-6) {
        offset = vec2(-delta.y, delta.x) / segment_length * ribbon.line_width_px * side / viewport;
    }
    clip = vec4(clip.xy + offset * clip.w, clip.z, clip.w);
    var out: Out;
    out.world = world.xyz;
    out.local = position;
    out.normal = vec3(0.0);
    out.curvature = 0.0;
    out.clip = clip;
    return out;
}
@fragment fn fs_mesh(input: Out) -> @location(0) vec4<f32> {
    if (dot(input.world, uniforms.clip_plane.xyz) > uniforms.clip_plane.w) { discard; }
    let n = normalize(input.normal);
    let key = max(dot(n, normalize(vec3(0.35, -0.45, 0.82))), 0.0);
    let fill = max(dot(n, normalize(vec3(-0.55, 0.25, 0.45))), 0.0);
    // Lifted ambient keeps back-lit faces readable on the light canvas while
    // preserving enough key/fill contrast for the dark theme.
    let base_color = model.base_color_metallic.rgb;
    let light = 0.50 + 0.52 * key + 0.12 * fill;
    var color = base_color * light;
    if (uniforms.analysis.w > 0.5) {
        let view = normalize(uniforms.eye_pitch.xyz - input.world);
        let key_dir = normalize(vec3(0.35, -0.45, 0.82));
        let half_vector = normalize(key_dir + view);
        let roughness = clamp(model.roughness.x, 0.04, 1.0);
        let metallic = clamp(model.base_color_metallic.a, 0.0, 1.0);
        // Blinn-Phong remap of GGX roughness: broad on rough materials and
        // tight on polished ones, without extra textures or lookup tables.
        let exponent = max(2.0, 2.0 / (roughness * roughness) - 2.0);
        let specular_lobe = pow(max(dot(n, half_vector), 0.0), exponent);
        let f0 = mix(vec3(0.04), base_color, metallic);
        let fresnel = f0 + (vec3(1.0) - f0) * pow(1.0 - max(dot(n, view), 0.0), 5.0);
        let hemisphere = mix(vec3(0.17, 0.15, 0.13), vec3(0.48, 0.55, 0.66), n.z * 0.5 + 0.5);
        let diffuse = base_color * (1.0 - metallic * 0.72) * (0.34 + 0.58 * key + 0.15 * fill);
        let specular = fresnel * specular_lobe * (0.30 + 0.85 * (1.0 - roughness));
        let rim = fresnel * pow(1.0 - max(dot(n, view), 0.0), 3.0) * (0.06 + 0.22 * metallic);
        color = diffuse + base_color * hemisphere * 0.28 + specular + rim;
    }
    if (uniforms.analysis.x > 0.5) {
        let view = normalize(uniforms.eye_pitch.xyz - input.world);
        let reflected = reflect(-view, n);
        let stripe = step(0.5, fract(dot(reflected, normalize(vec3(0.82, 0.31, 0.48))) * 12.0));
        color = mix(color, vec3(stripe), 0.70);
    } else if (uniforms.analysis.y > 0.5) {
        let value = clamp(input.curvature, 0.0, 1.0);
        let low = mix(vec3(0.05, 0.20, 0.95), vec3(0.05, 0.85, 0.25), value * 2.0);
        let high = mix(vec3(0.05, 0.85, 0.25), vec3(0.95, 0.12, 0.05), (value - 0.5) * 2.0);
        color = select(low, high, value >= 0.5);
    }
    return vec4(color, uniforms.body.a);
}
@fragment fn fs_cap(input: Out) -> @location(0) vec4<f32> {
    if (dot(input.world, uniforms.clip_plane.xyz) > uniforms.clip_plane.w) { discard; }
    return uniforms.section_cap;
}
@fragment fn fs_xray(input: Out) -> @location(0) vec4<f32> {
    if (dot(input.world, uniforms.clip_plane.xyz) > uniforms.clip_plane.w) { discard; }
    return vec4(model.base_color_metallic.rgb, 0.35);
}
@fragment fn fs_tint(_input: Out) -> @location(0) vec4<f32> { return tint.color; }
@fragment fn fs_cube(input: CubeOut) -> @location(0) vec4<f32> {
    let cell_u = input.label_uv.x * 6.0 - input.label_cell;
    if (input.label_cell < 0.0 || cell_u < 0.0 || cell_u > 1.0 || input.label_uv.y < 0.0 || input.label_uv.y > 1.0) {
        return tint.color;
    }
    let coverage = textureSample(cube_labels, cube_label_sampler, input.label_uv).r;
    let label_color = uniforms.edge.rgb * 0.58;
    return vec4(mix(tint.color.rgb, label_color, coverage), tint.color.a);
}
@fragment fn fs_tint_clipped(input: Out) -> @location(0) vec4<f32> {
    if (dot(input.world, uniforms.clip_plane.xyz) > uniforms.clip_plane.w) { discard; }
    return tint.color;
}
fn grid_line(value: f32, pitch: f32) -> f32 {
    let coord = value / pitch; let width = fwidth(coord); return 1.0 - smoothstep(0.0, width * 1.2, abs(fract(coord - 0.5) - 0.5));
}
@fragment fn fs_grid(input: Out) -> @location(0) vec4<f32> {
    let pitch = uniforms.eye_pitch.w;
    let minor = max(grid_line(input.local.x, pitch), grid_line(input.local.y, pitch));
    let major = max(grid_line(input.local.x, pitch * 5.0), grid_line(input.local.y, pitch * 5.0));
    let x_axis = grid_line(input.local.y, pitch * 100000.0);
    let y_axis = grid_line(input.local.x, pitch * 100000.0);
    let fade = exp(-length(input.local.xy) / (pitch * 42.0));
    var color = mix(uniforms.grid_minor.rgb, uniforms.grid_major.rgb, major);
    color = mix(color, uniforms.axis_x.rgb, x_axis);
    color = mix(color, uniforms.axis_y.rgb, y_axis);
    let alpha = max(
        max(minor * uniforms.grid_minor.a, major * uniforms.grid_major.a),
        max(x_axis * uniforms.axis_x.a, y_axis * uniforms.axis_y.a)
    ) * fade;
    return vec4(color, alpha);
}
"#;

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec4};

    use super::ribbon_perpendicular_offset;

    #[test]
    fn ribbon_offsets_are_symmetric_and_degenerate_segments_are_finite() {
        let viewport = Vec2::new(800.0, 600.0);
        let a = Vec4::new(-0.5, 0.0, 0.3, 1.0);
        let b = Vec4::new(0.5, 0.0, 0.3, 1.0);
        let positive = ribbon_perpendicular_offset(a, b, viewport, 3.0, 1.0);
        let negative = ribbon_perpendicular_offset(a, b, viewport, 3.0, -1.0);
        assert!((positive + negative).length() < 1.0e-7);
        assert!(positive.is_finite() && positive.length() > 0.0);

        let degenerate = ribbon_perpendicular_offset(a, a, viewport, 3.0, 1.0);
        assert_eq!(degenerate, Vec2::ZERO);
        assert!(degenerate.is_finite());
    }
}
