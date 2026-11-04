//! wgpu renderer for the viewports: each viewport renders offscreen (MSAA + depth) into a
//! texture that egui shows as an image.

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::{self, wgpu};
use std::collections::HashMap;

pub const MSAA: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const DRAW_SLOT: u64 = 512;
/// Punctual lights per frame. The JS has no fixed limit; this keeps the frame uniform small
/// while covering scenes far beyond the old eight-light cap.
pub const MAX_LIGHTS: usize = 32;
/// Material textures live in one array; 64 layers of 256² RGBA is 16 MB.
const TEX_LAYERS: u32 = 64;
/// Shadow depth maps: 512² per face, 48 layers (eight point lights, or fewer mixed lights).
/// The JS uses 2048² per light; 512² keeps the array at 48 MB and is plenty for the viewport.
const SHADOW_SIZE: u32 = 512;
pub const SHADOW_LAYERS: u32 = 48;
/// uniform buffer slot for one shadow matrix (256 is the dynamic-offset alignment)
const SHADOW_SLOT: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub uv: [f32; 2],
    /// x = material texture layer (-1 = none), y = scalar tint multiplied with the sampled colour
    pub tex: [f32; 2],
    /// additive emissive colour (JS `emissive` / `incandescence`), applied after lighting
    pub emission: [f32; 3],
    /// Material map layers: x = roughness, y = metalness, z = emission, w = alpha (-1 = none).
    pub aux: [f32; 4],
    /// x = specular colour map, y = bump map (-1 = none); z = emissive scale, w = bump scale.
    pub extra: [f32; 4],
}

impl Default for MeshVertex {
    fn default() -> Self {
        MeshVertex { pos: [0.0; 3], normal: [0.0; 3], color: [0.0; 3], uv: [0.0; 2], tex: [-1.0, 1.0], emission: [0.0; 3], aux: [-1.0; 4], extra: [-1.0, -1.0, 1.0, 1.0] }
    }
}

/// One line endpoint. `normal`/`normal_b` are the normals of up to two faces sharing the edge
/// (the JS `cullA`/`cullB`), used to hide wires of faces turned away from the camera; zeros mean
/// never culled.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
    pub normal: [f32; 3],
    pub normal_b: [f32; 3],
}

/// One component-mode dot; drawn as a screen-space quad (instanced). `normal` is the owning
/// face's normal for culling (zero means never culled, e.g. vertex dots).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub struct PointVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
    pub normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuLight {
    pub pos: [f32; 4],
    pub dir: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
    /// Half-width and half-height axes of an area light (xyz, w = 0); zero for other types.
    pub half_u: [f32; 4],
    pub half_v: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub cam_pos: [f32; 4],
    pub cam_dir: [f32; 4],
    pub bg_top: [f32; 4],
    pub bg_bottom: [f32; 4],
    pub mode: [f32; 4],
    pub post: [f32; 4],
    /// x = tone mapping (0 none, 1 ACES, 2 AgX, 3 Neutral, 4 Linear), y = transparent background
    pub render: [f32; 4],
    /// x = sky present (All lighting), y = show background, z = tan(fov_y/2), w = aspect
    pub sky: [f32; 4],
    /// sky dome colour (rgb) and intensity (a)
    pub sky_col: [f32; 4],
    pub lights: [GpuLight; MAX_LIGHTS],
    /// light view-projection for the first face of a shadowing light
    pub shadow_mats: [[[f32; 4]; 4]; MAX_LIGHTS],
    /// x = first depth-array layer (-1 = no shadows), y = type (1 ortho, 2 spot, 3 point),
    /// z = PCF softness (JS `shadowRadius`)
    pub shadow_info: [[f32; 4]; MAX_LIGHTS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DrawUniform {
    pub model: [[f32; 4]; 4],
    pub normal: [[f32; 4]; 4],
    /// `view_proj * model`, filled per viewport when the draw list is uploaded: lines and dots
    /// would otherwise multiply two matrices for every vertex.
    pub mvp: [[f32; 4]; 4],
    pub tint: [f32; 4],
    pub params: [f32; 4],
    pub params2: [f32; 4],
    /// x = receives shadows, y = casts shadows (Render Stats; the shadow pass reads y),
    /// z = clearcoat amount, w = clearcoat roughness
    pub shadow: [f32; 4],
    /// rgb = specular tint (JS `specularColor`), a = metalness
    pub spec: [f32; 4],
    /// rgb = sheen colour pre-multiplied by the sheen amount (the JS three.js uniform), a = amount
    pub sheen: [f32; 4],
}

impl DrawUniform {
    pub fn new(model: glam::Mat4) -> Self {
        let n = model.inverse().transpose();
        DrawUniform { model: model.to_cols_array_2d(), normal: n.to_cols_array_2d(), mvp: model.to_cols_array_2d(), tint: [0.0; 4], params: [0.0, 1.0, 1.5, 0.0], params2: [0.5, 0.0, 0.0, 0.0], shadow: [1.0, 1.0, 0.0, 0.0], spec: [1.0, 1.0, 1.0, 0.0], sheen: [0.0; 4] }
    }
    pub fn tint(mut self, c: [f32; 3], a: f32) -> Self {
        self.tint = [c[0], c[1], c[2], a];
        self
    }
    pub fn unlit(mut self) -> Self {
        self.params[0] = 1.0;
        self
    }
    pub fn alpha(mut self, a: f32) -> Self {
        self.params[1] = a;
        self
    }
    pub fn spec(mut self, s: f32, rough: f32) -> Self {
        self.params[3] = s;
        self.params2[0] = rough;
        self
    }
    /// Specular tint and metalness (JS `specularColor`, `metalness`).
    pub fn spec_tint(mut self, c: [f32; 3], metal: f32) -> Self {
        self.spec = [c[0], c[1], c[2], metal];
        self
    }
    /// Specular IOR (JS Standard Surface `specularIOR`), used for the dielectric F0.
    pub fn ior(mut self, ior: f32) -> Self {
        self.params[2] = ior.max(1.0);
        self
    }
    /// Half-size in pixels of component dots (see `vs_point`).
    pub fn point_size(mut self, half: f32) -> Self {
        self.params2[1] = half;
        self
    }
    /// Cull line endpoints whose normals face away from the camera (see `vs_line`).
    pub fn cull_wire(mut self, on: bool) -> Self {
        self.params2[2] = if on { 1.0 } else { 0.0 };
        self
    }
    /// Sample the material colour texture (JS Hardware Texturing).
    pub fn textured(mut self, on: bool) -> Self {
        self.params2[3] = if on { 1.0 } else { 0.0 };
        self
    }
    /// Render Stats shadow flags: cast into light depth maps and/or receive shadows.
    pub fn shadows(mut self, cast: bool, receive: bool) -> Self {
        self.shadow[0] = if receive { 1.0 } else { 0.0 };
        self.shadow[1] = if cast { 1.0 } else { 0.0 };
        self
    }
    /// Standard Surface clearcoat (JS `coat` / `coatRoughness`).
    pub fn coat(mut self, amount: f32, rough: f32) -> Self {
        self.shadow[2] = amount;
        self.shadow[3] = rough;
        self
    }
    /// Standard Surface sheen; three.js premultiplies the colour by the amount.
    pub fn sheen(mut self, c: [f32; 3], amount: f32) -> Self {
        self.sheen = [c[0] * amount, c[1] * amount, c[2] * amount, amount];
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pass {
    /// lines drawn before any geometry, without depth writes (the grid)
    LinesUnder,
    /// opaque shaded triangles
    Mesh,
    /// transparent triangles (x-ray, selected-face overlay)
    MeshBlend,
    /// depth-tested lines
    Lines,
    /// depth-tested component dots (screen-space quads)
    Points,
    /// lines drawn on top of everything
    LinesTop,
}

pub enum Geo {
    /// cached mesh triangles by key
    MeshTris(String),
    /// cached mesh wireframe by key
    MeshWire(String),
    /// cached component dots by key
    MeshPoints(String),
    /// cached face-centre dots by key (Display > Polygons > Face Centers)
    MeshFacePoints(String),
    /// cached selected-edge overlay by key
    CompLines(String),
    /// cached selected-face overlay by key
    CompTris(String),
    TransientTris(Vec<MeshVertex>),
    TransientLines(Vec<LineVertex>),
}

/// Result of asking the GPU for a sky-dome `textureFile`.
#[derive(Clone, Debug)]
pub enum SkyLoad {
    /// The equirect texture is uploaded and bound.
    Ready,
    /// The first load failed; the caller should warn and use the procedural fallback.
    Failed(String),
    /// A previous load of the same file failed; stay on the procedural fallback silently.
    Unavailable,
}

pub struct DrawCmd {
    pub pass: Pass,
    pub geo: Geo,
    pub u: DrawUniform,
}

pub struct GpuMesh {
    pub key: u64,
    tris: Option<wgpu::Buffer>,
    tri_count: u32,
    wire: Option<wgpu::Buffer>,
    wire_count: u32,
    points: Option<wgpu::Buffer>,
    point_count: u32,
    points_key: Option<u64>,
    /// face-centre dots (Display > Polygons > Face Centers), separate from component dots
    face_points: Option<wgpu::Buffer>,
    face_point_count: u32,
    face_points_key: Option<u64>,
    /// selected-edge overlay (object space), cached by selection key
    comp_lines: Option<wgpu::Buffer>,
    comp_line_count: u32,
    comp_line_key: Option<u64>,
    /// selected-face overlay (object space), cached by selection key
    comp_tris: Option<wgpu::Buffer>,
    comp_tri_count: u32,
    comp_tri_key: Option<u64>,
}

impl Default for GpuMesh {
    fn default() -> Self {
        GpuMesh {
            key: 0,
            tris: None,
            tri_count: 0,
            wire: None,
            wire_count: 0,
            points: None,
            point_count: 0,
            points_key: None,
            face_points: None,
            face_point_count: 0,
            face_points_key: None,
            comp_lines: None,
            comp_line_count: 0,
            comp_line_key: None,
            comp_tris: None,
            comp_tri_count: 0,
            comp_tri_key: None,
        }
    }
}

struct Target {
    size: [u32; 2],
    _color: wgpu::Texture,
    color_view: wgpu::TextureView,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    egui_id: egui::TextureId,
    frame_buf: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    frame_bgl: wgpu::BindGroupLayout,
    draw_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    tex_array: wgpu::Texture,
    tex_view: wgpu::TextureView,
    tex_sampler: wgpu::Sampler,
    tex_bg: wgpu::BindGroup,
    /// texture id -> array layer
    tex_layers: HashMap<String, u32>,
    /// texture id -> attributes hash of the pixels currently in the layer
    tex_hashes: HashMap<String, u64>,
    shadow_views: Vec<wgpu::TextureView>,
    shadow_bg: wgpu::BindGroup,
    shadow_buf: wgpu::Buffer,
    shadow_ubg: wgpu::BindGroup,
    shadow_pipe: wgpu::RenderPipeline,
    /// group 2 binding 4: sky sampler
    sky_samp: wgpu::Sampler,
    /// resolved path of the loaded sky file, and whether it loaded
    sky_key: Option<String>,
    sky_ok: bool,
    bg_pipe: wgpu::RenderPipeline,
    mesh_pipe: wgpu::RenderPipeline,
    blend_pipe: wgpu::RenderPipeline,
    line_pipe: wgpu::RenderPipeline,
    line_top_pipe: wgpu::RenderPipeline,
    point_pipe: wgpu::RenderPipeline,
    draw_buf: wgpu::Buffer,
    draw_cap: u64,
    draw_bg: wgpu::BindGroup,
    meshes: HashMap<String, GpuMesh>,
    targets: HashMap<usize, Target>,
}

fn vb_layout<'a>(stride: u64, attrs: &'a [wgpu::VertexAttribute]) -> wgpu::VertexBufferLayout<'a> {
    wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: attrs }
}

impl Gpu {
    pub fn new(rs: &egui_wgpu::RenderState) -> Self {
        let device = rs.device.clone();
        let queue = rs.queue.clone();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("inca viewport"), source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()) });
        let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniform>() as u64) },
                count: None,
            }],
        });
        // group 2: material texture array + the sky dome equirect/irradiance. max_bind_groups is
        // 4, so the sky lives here rather than in a group of its own.
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let tex_array = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material textures"),
            size: wgpu::Extent3d { width: inca_core::materials::VIEW_TEX_SIZE, height: inca_core::materials::VIEW_TEX_SIZE, depth_or_array_layers: TEX_LAYERS },
            mip_level_count: inca_core::materials::VIEW_TEX_SIZE.ilog2() + 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let tex_view = tex_array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let tex_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sky_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sky_white = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky fallback"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let white = [half::f16::from_f32(1.0).to_bits(); 4];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &sky_white, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&white),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let sky_white_view = sky_white.create_view(&Default::default());
        let tex_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material textures"),
            layout: &tex_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&tex_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&sky_white_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&sky_white_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sky_samp) },
            ],
        });
        // group 3: the shadow depth array sampled by the mesh shader
        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadows"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
            ],
        });
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: SHADOW_LAYERS },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_views: Vec<wgpu::TextureView> = (0..SHADOW_LAYERS)
            .map(|i| {
                shadow_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i, array_layer_count: Some(1), ..Default::default() })
            })
            .collect();
        let shadow_view = shadow_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            // three.js uses LinearFilter on the shadow map, so each comparison tap is a
            // hardware 2x2 PCF average; the shader adds the five Vogel-disk taps.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let shadow_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadows"),
            layout: &shadow_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
            ],
        });
        // shadow pass: draw model (group 0, dynamic) + one light matrix (group 1, dynamic)
        let shadow_frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(64) },
                count: None,
            }],
        });
        let shadow_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("shadow matrices"), size: SHADOW_LAYERS as u64 * SHADOW_SLOT, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let shadow_ubg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow matrices"),
            layout: &shadow_frame_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &shadow_buf, offset: 0, size: wgpu::BufferSize::new(64) }),
            }],
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("inca shadow"), source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()) });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("shadow"), bind_group_layouts: &[Some(&draw_bgl), Some(&shadow_frame_bgl)], immediate_size: 0 });
        let shadow_attrs = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }];
        let shadow_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(vb_layout(std::mem::size_of::<MeshVertex>() as u64, &shadow_attrs))],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("inca"), bind_group_layouts: &[Some(&frame_bgl), Some(&draw_bgl), Some(&tex_bgl), Some(&shadow_bgl)], immediate_size: 0 });
        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x2, 4 => Float32x2, 5 => Float32x3, 6 => Float32x4, 7 => Float32x4];
        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32x3, 3 => Float32x3];
        let point_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32x3];
        let ms = wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false };
        let depth = |write: bool, compare: wgpu::CompareFunction, bias: wgpu::DepthBiasState| {
            Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(write), depth_compare: Some(compare), stencil: Default::default(), bias })
        };
        // JS `applyAttrs` sets polygonOffset(1, 1) on every shaded material, pushing the
        // surface back so depth-tested wires on its edges stay visible. A fixed clip-space
        // wire bias cannot do this: at the default camera distance it is larger than the
        // whole depth range of a small object and hidden back edges bleed through.
        let no_bias = wgpu::DepthBiasState::default();
        let mesh_bias = wgpu::DepthBiasState { constant: 1, slope_scale: 1.0, clamp: 0.0 };
        let target = |blend: Option<wgpu::BlendState>| [Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })];
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let mk = |label: &str, vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], topo: wgpu::PrimitiveTopology, ds: Option<wgpu::DepthStencilState>, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState { topology: topo, cull_mode: None, ..Default::default() },
                depth_stencil: ds,
                multisample: ms,
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &target(blend) }),
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_vb = [Some(vb_layout(std::mem::size_of::<MeshVertex>() as u64, &mesh_attrs))];
        let line_vb = [Some(vb_layout(std::mem::size_of::<LineVertex>() as u64, &line_attrs))];
        let mut point_vb = vb_layout(std::mem::size_of::<PointVertex>() as u64, &point_attrs);
        point_vb.step_mode = wgpu::VertexStepMode::Instance;
        let point_vbs = [Some(point_vb)];
        let bg_pipe = mk("bg", "vs_bg", "fs_bg", &[], wgpu::PrimitiveTopology::TriangleList, depth(false, wgpu::CompareFunction::Always, no_bias), None);
        let mesh_pipe = mk("mesh", "vs_mesh", "fs_mesh", &mesh_vb, wgpu::PrimitiveTopology::TriangleList, depth(true, wgpu::CompareFunction::LessEqual, mesh_bias), None);
        let blend_pipe = mk("blend", "vs_mesh", "fs_mesh", &mesh_vb, wgpu::PrimitiveTopology::TriangleList, depth(false, wgpu::CompareFunction::LessEqual, no_bias), alpha);
        let line_pipe = mk("line", "vs_line", "fs_line", &line_vb, wgpu::PrimitiveTopology::LineList, depth(true, wgpu::CompareFunction::LessEqual, no_bias), alpha);
        let line_top_pipe = mk("line top", "vs_line", "fs_line", &line_vb, wgpu::PrimitiveTopology::LineList, depth(false, wgpu::CompareFunction::Always, no_bias), alpha);
        let point_pipe = mk("points", "vs_point", "fs_point", &point_vbs, wgpu::PrimitiveTopology::TriangleList, depth(true, wgpu::CompareFunction::LessEqual, no_bias), None);
        let draw_cap = 256;
        let (draw_buf, draw_bg) = Self::make_draw_buf(&device, &draw_bgl, draw_cap);
        Gpu {
            device,
            queue,
            frame_bgl,
            draw_bgl,
            tex_bgl,
            tex_array,
            tex_view,
            tex_sampler,
            tex_bg,
            tex_layers: HashMap::new(),
            tex_hashes: HashMap::new(),
            shadow_views,
            shadow_bg,
            shadow_buf,
            shadow_ubg,
            shadow_pipe,
            sky_samp,
            sky_key: None,
            sky_ok: false,
            bg_pipe,
            mesh_pipe,
            blend_pipe,
            line_pipe,
            line_top_pipe,
            point_pipe,
            draw_buf,
            draw_cap,
            draw_bg,
            meshes: HashMap::new(),
            targets: HashMap::new(),
        }
    }

    fn make_draw_buf(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, cap: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("draws"), size: cap * DRAW_SLOT, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draws"),
            layout: bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &buf, offset: 0, size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniform>() as u64) }),
            }],
        });
        (buf, bg)
    }

    fn vbuf(&self, data: &[u8]) -> Option<wgpu::Buffer> {
        if data.is_empty() {
            return None;
        }
        use wgpu::util::DeviceExt;
        Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: data, usage: wgpu::BufferUsages::VERTEX }))
    }

    /// Layer of a material texture, uploading the pixels when they are new or `hash` changed.
    /// Returns None when the array is full (the material then draws with its flat colour).
    pub fn texture_layer(&mut self, key: &str, hash: u64, pixels: impl FnOnce() -> Option<Vec<u8>>) -> Option<u32> {
        if let Some(&layer) = self.tex_layers.get(key) {
            if self.tex_hashes.get(key) != Some(&hash) {
                if let Some(px) = pixels() {
                    self.write_texture_layer(layer, &px);
                    self.tex_hashes.insert(key.to_string(), hash);
                }
            }
            return Some(layer);
        }
        if self.tex_layers.len() as u32 >= TEX_LAYERS {
            return None;
        }
        let px = pixels()?;
        let layer = self.tex_layers.len() as u32;
        self.tex_layers.insert(key.to_string(), layer);
        self.tex_hashes.insert(key.to_string(), hash);
        self.write_texture_layer(layer, &px);
        Some(layer)
    }

    fn write_texture_layer(&self, layer: u32, px: &[u8]) {
        let size = inca_core::materials::VIEW_TEX_SIZE;
        if px.len() != (size * size * 4) as usize {
            return;
        }
        // Three.js generates mipmaps for material textures; box-filter the chain here so
        // minified surfaces don't shimmer and small textures keep their average colour.
        let mut w = size;
        let mut data = px.to_vec();
        let mut mip = 0u32;
        loop {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.tex_array, mip_level: mip, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                &data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(w) },
                wgpu::Extent3d { width: w, height: w, depth_or_array_layers: 1 },
            );
            if w == 1 {
                break;
            }
            let n = (w / 2).max(1);
            let mut next = vec![0u8; (n * n * 4) as usize];
            for y in 0..n {
                for x in 0..n {
                    for k in 0..4 {
                        let mut sum = 0u32;
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let sx = (x * 2 + dx).min(w - 1);
                                let sy = (y * 2 + dy).min(w - 1);
                                sum += data[((sy * w + sx) * 4 + k) as usize] as u32;
                            }
                        }
                        next[((y * n + x) * 4 + k) as usize] = ((sum + 2) / 4) as u8;
                    }
                }
            }
            w = n;
            data = next;
            mip += 1;
        }
    }

    /// Decode and upload a sky dome `textureFile` when it changed. Synchronous: the first
    /// frame after picking a file may pause while the image is decoded and prefiltered.
    pub fn prepare_sky(&mut self, project: Option<&str>, file: &str) -> SkyLoad {
        let path = inca_core::sky::resolve_path(project, file);
        let key = path.to_string_lossy().to_string();
        if self.sky_key.as_deref() == Some(key.as_str()) {
            return if self.sky_ok { SkyLoad::Ready } else { SkyLoad::Unavailable };
        }
        self.sky_key = Some(key);
        match inca_core::sky::load(&path) {
            Ok(img) => {
                self.upload_sky(&img);
                self.sky_ok = true;
                SkyLoad::Ready
            }
            Err(e) => {
                self.sky_ok = false;
                SkyLoad::Failed(e)
            }
        }
    }

    fn upload_sky(&mut self, img: &inca_core::sky::SkyImage) {
        let levels = inca_core::sky::mip_chain(img);
        let (w, h) = (levels[0].0, levels[0].1);
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky equirect"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, (lw, lh, px)) in levels.iter().enumerate() {
            let half_px: Vec<u16> = px.iter().map(|v| half::f16::from_f32(*v).to_bits()).collect();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: i as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                bytemuck::cast_slice(&half_px),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 8), rows_per_image: Some(*lh) },
                wgpu::Extent3d { width: *lw, height: *lh, depth_or_array_layers: 1 },
            );
        }
        let (iw, ih) = (64u32, 32u32);
        let irr = inca_core::sky::irradiance(img, iw as usize, ih as usize);
        let irr_half: Vec<u16> = irr.iter().map(|v| half::f16::from_f32(*v).to_bits()).collect();
        let irr_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sky irradiance"),
            size: wgpu::Extent3d { width: iw, height: ih, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &irr_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&irr_half),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(iw * 8), rows_per_image: Some(ih) },
            wgpu::Extent3d { width: iw, height: ih, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&Default::default());
        let irr_view = irr_tex.create_view(&Default::default());
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material textures"),
            layout: &self.tex_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.tex_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.tex_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&irr_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sky_samp) },
            ],
        });
        self.tex_bg = bg;
    }

    pub fn mesh_key(&self, name: &str) -> Option<u64> {
        self.meshes.get(name).map(|m| m.key)
    }

    pub fn point_key(&self, name: &str) -> Option<u64> {
        self.meshes.get(name).and_then(|m| m.points_key)
    }

    pub fn face_point_key(&self, name: &str) -> Option<u64> {
        self.meshes.get(name).and_then(|m| m.face_points_key)
    }

    pub fn comp_line_key(&self, name: &str) -> Option<u64> {
        self.meshes.get(name).and_then(|m| m.comp_line_key)
    }

    pub fn comp_tri_key(&self, name: &str) -> Option<u64> {
        self.meshes.get(name).and_then(|m| m.comp_tri_key)
    }

    pub fn upload_mesh(&mut self, name: &str, key: u64, tris: &[MeshVertex], wire: &[LineVertex]) {
        let m = GpuMesh { key, tri_count: tris.len() as u32, wire_count: wire.len() as u32, tris: self.vbuf(bytemuck::cast_slice(tris)), wire: self.vbuf(bytemuck::cast_slice(wire)), ..Default::default() };
        self.meshes.insert(name.to_string(), m);
    }

    /// Upload the component dots of a mesh; skipped while `key` is unchanged (the caller
    /// builds the list only when the key changes).
    pub fn upload_points(&mut self, name: &str, key: u64, points: &[PointVertex]) {
        let buf = self.vbuf(bytemuck::cast_slice(points));
        let m = self.meshes.entry(name.to_string()).or_default();
        m.points = buf;
        m.point_count = points.len() as u32;
        m.points_key = Some(key);
    }

    /// Upload the face-centre dots; skipped while `key` is unchanged.
    pub fn upload_face_points(&mut self, name: &str, key: u64, points: &[PointVertex]) {
        let buf = self.vbuf(bytemuck::cast_slice(points));
        let m = self.meshes.entry(name.to_string()).or_default();
        m.face_points = buf;
        m.face_point_count = points.len() as u32;
        m.face_points_key = Some(key);
    }

    /// Upload the selected-edge overlay; skipped while `key` is unchanged.
    pub fn upload_comp_lines(&mut self, name: &str, key: u64, lines: &[LineVertex]) {
        let buf = self.vbuf(bytemuck::cast_slice(lines));
        let m = self.meshes.entry(name.to_string()).or_default();
        m.comp_lines = buf;
        m.comp_line_count = lines.len() as u32;
        m.comp_line_key = Some(key);
    }

    /// Upload the selected-face overlay; skipped while `key` is unchanged.
    pub fn upload_comp_tris(&mut self, name: &str, key: u64, tris: &[MeshVertex]) {
        let buf = self.vbuf(bytemuck::cast_slice(tris));
        let m = self.meshes.entry(name.to_string()).or_default();
        m.comp_tris = buf;
        m.comp_tri_count = tris.len() as u32;
        m.comp_tri_key = Some(key);
    }

    pub fn retain_meshes(&mut self, keep: &std::collections::HashSet<String>) {
        self.meshes.retain(|k, _| keep.contains(k));
    }

    /// Make sure viewport `idx` has a render target of `size` pixels; returns its egui texture.
    pub fn target(&mut self, renderer: &mut egui_wgpu::Renderer, idx: usize, size: [u32; 2]) -> egui::TextureId {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        if let Some(t) = self.targets.get(&idx) {
            if t.size == size {
                return t.egui_id;
            }
        }
        let mut t = self.make_target(size);
        t.egui_id = match self.targets.get(&idx) {
            Some(old) => {
                renderer.update_egui_texture_from_wgpu_texture(&self.device, &t.color_view, wgpu::FilterMode::Linear, old.egui_id);
                old.egui_id
            }
            None => renderer.register_native_texture(&self.device, &t.color_view, wgpu::FilterMode::Linear),
        };
        let id = t.egui_id;
        self.targets.insert(idx, t);
        id
    }

    fn make_target(&self, size: [u32; 2]) -> Target {
        let ext = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let tex = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            self.device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: ext, mip_level_count: 1, sample_count: samples, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] })
        };
        let color = tex("vp color", COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let msaa = tex("vp msaa", COLOR_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = tex("vp depth", DEPTH_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let color_view = color.create_view(&Default::default());
        let frame_buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("frame"), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let frame_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("frame"), layout: &self.frame_bgl, entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() }] });
        Target { size, msaa_view: msaa.create_view(&Default::default()), depth_view: depth.create_view(&Default::default()), color_view, _color: color, egui_id: egui::TextureId::default(), frame_buf, frame_bg }
    }

    /// Render into a throwaway target and read the pixels back (sRGB RGBA, row-major): the
    /// "Inca Hardware" renderer.
    pub fn render_offscreen(&mut self, size: [u32; 2], frame: &FrameUniform, draws: &[DrawCmd]) -> Vec<u8> {
        let size = [size[0].clamp(1, 8192), size[1].clamp(1, 8192)];
        self.reserve_draws(draws.len() as u64);
        let t = self.make_target(size);
        self.draw_pass(&t, frame, draws);
        let row = (size[0] * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (row * size[1]) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t._color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(size[1]) } },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(data) = slice.get_mapped_range() else { return vec![] };
        // the target stores sRGB-encoded bytes of what the shader wrote, and the shader already
        // gamma-encodes (the viewport shows the decoded values): undo the storage encoding once
        let lut: Vec<u8> = (0..256).map(|i| {
            let x = i as f32 / 255.0;
            let l = if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
            (l * 255.0 + 0.5) as u8
        }).collect();
        let mut out = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for y in 0..size[1] {
            let r = (y * row) as usize;
            for px in data[r..r + (size[0] * 4) as usize].chunks(4) {
                out.extend_from_slice(&[lut[px[0] as usize], lut[px[1] as usize], lut[px[2] as usize], px[3]]);
            }
        }
        drop(data);
        buf.unmap();
        out
    }

    fn reserve_draws(&mut self, n: u64) {
        if n > self.draw_cap {
            self.draw_cap = n.next_power_of_two();
            let (b, g) = Self::make_draw_buf(&self.device, &self.draw_bgl, self.draw_cap);
            self.draw_buf = b;
            self.draw_bg = g;
        }
    }

    /// Render one viewport.
    pub fn render(&mut self, idx: usize, frame: &FrameUniform, draws: &[DrawCmd]) {
        self.reserve_draws(draws.len() as u64);
        let Some(t) = self.targets.get(&idx) else { return };
        self.draw_pass(t, frame, draws);
    }

    /// Point-light cube faces: (direction, up). The up vectors match the cube lookup in the
    /// mesh shader, so the rendered faces and `shadow_factor` agree.
    const POINT_FACES: [([f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 0.0, 1.0], [0.0, -1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, -1.0, 0.0]),
    ];

    /// Light view-projections for the depth maps needed this frame, with their array layers.
    /// Also uploads the matrices to the dynamic-offset buffer.
    fn prepare_shadow_faces(&self, frame: &FrameUniform) -> Vec<(u32, glam::Mat4)> {
        let mut slots = vec![0u8; (SHADOW_LAYERS as u64 * SHADOW_SLOT) as usize];
        let mut faces = vec![];
        for i in 0..MAX_LIGHTS {
            let info = frame.shadow_info[i];
            if info[0] < 0.0 {
                continue;
            }
            let base = info[0] as u32;
            if info[1] < 2.5 {
                // directional (ortho) or spot (perspective): one map
                let m = glam::Mat4::from_cols_array_2d(&frame.shadow_mats[i]);
                let a = m.to_cols_array();
                let b = bytemuck::bytes_of(&a);
                let o = (base as u64 * SHADOW_SLOT) as usize;
                slots[o..o + b.len()].copy_from_slice(b);
                faces.push((base, m));
            } else if info[1] > 2.5 {
                // point light: six 90° faces
                let p = glam::Vec3::new(frame.lights[i].pos[0], frame.lights[i].pos[1], frame.lights[i].pos[2]);
                let proj = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.5, 500.0);
                for (k, (d, up)) in Self::POINT_FACES.iter().enumerate() {
                    let (d, up) = (glam::Vec3::from(*d), glam::Vec3::from(*up));
                    let view = glam::Mat4::look_at_rh(p, p + d, up);
                    let m = proj * view;
                    let a = m.to_cols_array();
                    let b = bytemuck::bytes_of(&a);
                    let o = ((base + k as u32) as u64 * SHADOW_SLOT) as usize;
                    slots[o..o + b.len()].copy_from_slice(b);
                    faces.push((base + k as u32, m));
                }
            }
        }
        if !faces.is_empty() {
            self.queue.write_buffer(&self.shadow_buf, 0, &slots);
        }
        faces
    }

    fn draw_pass(&self, t: &Target, frame: &FrameUniform, draws: &[DrawCmd]) {
        let n = draws.len() as u64;
        self.queue.write_buffer(&t.frame_buf, 0, bytemuck::bytes_of(frame));
        let mut slots = vec![0u8; (n.max(1) * DRAW_SLOT) as usize];
        let vp = glam::Mat4::from_cols_array_2d(&frame.view_proj);
        for (i, d) in draws.iter().enumerate() {
            let mut u = d.u;
            u.mvp = (vp * glam::Mat4::from_cols_array_2d(&u.model)).to_cols_array_2d();
            let b = bytemuck::bytes_of(&u);
            slots[i * DRAW_SLOT as usize..i * DRAW_SLOT as usize + b.len()].copy_from_slice(b);
        }
        self.queue.write_buffer(&self.draw_buf, 0, &slots);
        // transient geometry packed into two buffers
        let mut ttris: Vec<MeshVertex> = vec![];
        let mut tlines: Vec<LineVertex> = vec![];
        let mut ranges = vec![(0u32, 0u32); draws.len()];
        for (i, d) in draws.iter().enumerate() {
            match &d.geo {
                Geo::TransientTris(v) => {
                    ranges[i] = (ttris.len() as u32, v.len() as u32);
                    ttris.extend_from_slice(v);
                }
                Geo::TransientLines(v) => {
                    ranges[i] = (tlines.len() as u32, v.len() as u32);
                    tlines.extend_from_slice(v);
                }
                _ => {}
            }
        }
        let tri_buf = self.vbuf(bytemuck::cast_slice(&ttris));
        let line_buf = self.vbuf(bytemuck::cast_slice(&tlines));
        let shadow_faces = self.prepare_shadow_faces(frame);
        // transparent renders keep a zero clear alpha so the saved PNG shows the background
        let clear = if frame.render[1] > 0.5 { wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 } } else { wgpu::Color::BLACK };
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        for (layer, _) in &shadow_faces {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow map"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_views[*layer as usize],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.shadow_pipe);
            for (i, d) in draws.iter().enumerate() {
                if d.u.shadow[1] < 0.5 {
                    continue;
                }
                let Geo::MeshTris(k) = &d.geo else { continue };
                let Some(GpuMesh { tris: Some(b), tri_count, .. }) = self.meshes.get(k) else { continue };
                if *tri_count == 0 {
                    continue;
                }
                rp.set_bind_group(0, &self.draw_bg, &[(i as u64 * DRAW_SLOT) as u32]);
                rp.set_bind_group(1, &self.shadow_ubg, &[(*layer as u64 * SHADOW_SLOT) as u32]);
                rp.set_vertex_buffer(0, b.slice(..));
                rp.draw(0..*tri_count, 0..1);
            }
        }
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&t.color_view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &t.frame_bg, &[]);
            rp.set_bind_group(1, &self.draw_bg, &[0]);
            rp.set_bind_group(2, &self.tex_bg, &[]);
            rp.set_bind_group(3, &self.shadow_bg, &[]);
            rp.set_pipeline(&self.bg_pipe);
            rp.draw(0..3, 0..1);
            for pass in [Pass::LinesUnder, Pass::Mesh, Pass::Lines, Pass::Points, Pass::MeshBlend, Pass::LinesTop] {
                rp.set_pipeline(match pass {
                    Pass::LinesUnder => &self.line_top_pipe,
                    Pass::Mesh => &self.mesh_pipe,
                    Pass::MeshBlend => &self.blend_pipe,
                    Pass::Lines => &self.line_pipe,
                    Pass::Points => &self.point_pipe,
                    Pass::LinesTop => &self.line_top_pipe,
                });
                for (i, d) in draws.iter().enumerate() {
                    if d.pass != pass {
                        continue;
                    }
                    let (buf, start, count) = match &d.geo {
                        Geo::MeshTris(k) => match self.meshes.get(k) {
                            Some(GpuMesh { tris: Some(b), tri_count, .. }) => (b, 0, *tri_count),
                            _ => continue,
                        },
                        Geo::MeshWire(k) => match self.meshes.get(k) {
                            Some(GpuMesh { wire: Some(b), wire_count, .. }) => (b, 0, *wire_count),
                            _ => continue,
                        },
                        Geo::MeshPoints(k) => match self.meshes.get(k) {
                            Some(GpuMesh { points: Some(b), point_count, .. }) => (b, 0, *point_count),
                            _ => continue,
                        },
                        Geo::MeshFacePoints(k) => match self.meshes.get(k) {
                            Some(GpuMesh { face_points: Some(b), face_point_count, .. }) => (b, 0, *face_point_count),
                            _ => continue,
                        },
                        Geo::CompLines(k) => match self.meshes.get(k) {
                            Some(GpuMesh { comp_lines: Some(b), comp_line_count, .. }) => (b, 0, *comp_line_count),
                            _ => continue,
                        },
                        Geo::CompTris(k) => match self.meshes.get(k) {
                            Some(GpuMesh { comp_tris: Some(b), comp_tri_count, .. }) => (b, 0, *comp_tri_count),
                            _ => continue,
                        },
                        Geo::TransientTris(_) => match &tri_buf {
                            Some(b) => (b, ranges[i].0, ranges[i].1),
                            None => continue,
                        },
                        Geo::TransientLines(_) => match &line_buf {
                            Some(b) => (b, ranges[i].0, ranges[i].1),
                            None => continue,
                        },
                    };
                    if count == 0 {
                        continue;
                    }
                    rp.set_bind_group(1, &self.draw_bg, &[(i as u64 * DRAW_SLOT) as u32]);
                    rp.set_vertex_buffer(0, buf.slice(..));
                    if pass == Pass::Points {
                        // one instanced quad (6 vertices) per dot
                        rp.draw(0..6, start..start + count);
                    } else {
                        rp.draw(start..start + count, 0..1);
                    }
                }
            }
        }
        self.queue.submit([enc.finish()]);
    }
}
