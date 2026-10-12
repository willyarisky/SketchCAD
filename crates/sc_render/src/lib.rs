//! SketchCAD viewport renderer (wgpu).
//!
//! The 3D view is rendered off-screen (4× MSAA, reversed-Z depth) into a texture that the egui UI
//! shows as an image. This keeps the 3D pipeline independent of egui's own render pass.

pub mod camera;
pub mod scene;

pub use camera::{Camera, StandardView};
pub use scene::{FaceStyle, FaceVertex, LineInstance, RenderScene, SceneOptions, flags, rgba as scene_rgba};

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::{self, wgpu};
use sc_geom::{TexId, Texture};
use sc_math::DVec2;
use std::collections::HashMap;

const SAMPLES: u32 = 4;
// egui composites in gamma space, so the viewport renders gamma-space colors into a plain UNORM target.
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    viewport: [f32; 4],
    forward: [f32; 4],
    light: [f32; 4],
    sky_top: [f32; 4],
    sky_horizon: [f32; 4],
    ground: [f32; 4],
}

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    resolve: wgpu::TextureView,
    depth: wgpu::TextureView,
}

struct DynBuffer {
    buf: wgpu::Buffer,
    cap: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl DynBuffer {
    fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages) -> Self {
        let cap = 1024;
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: cap, usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { buf, cap, usage, label }
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if data.len() as u64 > self.cap {
            self.cap = (data.len() as u64).next_power_of_two();
            self.buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.cap,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buf, 0, data);
    }
}

pub struct ViewportRenderer {
    bg: wgpu::RenderPipeline,
    faces: wgpu::RenderPipeline,
    faces_xray: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    lines_hidden: wgpu::RenderPipeline,
    lines_overlay: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    face_vb: DynBuffer,
    line_vb: DynBuffer,
    hidden_vb: DynBuffer,
    overlay_vb: DynBuffer,
    targets: Option<Targets>,
    texture_id: Option<egui::TextureId>,
    tex_bgl: wgpu::BindGroupLayout,
    textures: FaceTextures,
}

/// Every face texture in one array, so a model draws in a single pass however many it uses.
/// Images are resampled to a common size, which an array requires.
struct FaceTextures {
    bind_group: wgpu::BindGroup,
    layers: HashMap<TexId, u32>,
    /// What the array currently holds, to skip the upload when nothing changed.
    key: Vec<TexId>,
}

/// Edge length every texture is resampled to.
pub const TEX_SIZE: u32 = 256;
/// Textures beyond this are shown as plain colour; keeps the array under ~43 MB.
pub const MAX_TEXTURES: usize = 128;

fn mip_levels() -> u32 {
    TEX_SIZE.ilog2() + 1
}

/// Build the bind group for `layers` worth of image data (`data[layer][mip]`, RGBA8).
fn make_tex_bind_group(device: &wgpu::Device, queue: &wgpu::Queue, bgl: &wgpu::BindGroupLayout, data: &[Vec<Vec<u8>>]) -> wgpu::BindGroup {
    let count = data.len().max(1) as u32;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("face textures"),
        size: wgpu::Extent3d { width: TEX_SIZE, height: TEX_SIZE, depth_or_array_layers: count },
        mip_level_count: mip_levels(),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Plain UNORM, not _Srgb: the viewport works in gamma space (see COLOR_FORMAT), so
        // texels must arrive the same way face colours do rather than being converted to linear.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, mips) in data.iter().enumerate() {
        for (level, px) in mips.iter().enumerate() {
            let size = (TEX_SIZE >> level).max(1);
            if px.len() < (size * size * 4) as usize {
                continue;
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                px,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
        }
    }
    let view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("face textures"),
        // Textures tile across a face, so both axes repeat.
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("face textures"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }],
    })
}

/// Decode an image and resample it to the array's size, with a full mip chain.
fn decode_mips(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let img = image::load_from_memory(data).ok()?.to_rgba8();
    let mut level = image::imageops::resize(&img, TEX_SIZE, TEX_SIZE, image::imageops::FilterType::CatmullRom);
    let mut out = vec![level.as_raw().clone()];
    for i in 1..mip_levels() {
        let size = (TEX_SIZE >> i).max(1);
        level = image::imageops::resize(&level, size, size, image::imageops::FilterType::Triangle);
        out.push(level.as_raw().clone());
    }
    Some(out)
}

impl ViewportRenderer {
    pub fn new(rs: &egui_wgpu::RenderState) -> Self {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sketchcad shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("face textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
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
            label: Some("viewport layout"),
            bind_group_layouts: &[Some(&bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let face_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x4, 4 => Uint32, 5 => Float32x4, 6 => Sint32x2];
        let face_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FaceVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &face_attrs,
        };
        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x4];
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &line_attrs,
        };

        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffers: &[Option<wgpu::VertexBufferLayout>],
                    depth_write: bool,
                    compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction as C;
        let bg = make("background", "vs_bg", "fs_bg", &[], false, C::Always);
        let faces = make("faces", "vs_face", "fs_face", &[Some(face_layout.clone())], true, C::Greater);
        let faces_xray = make("faces xray", "vs_face", "fs_face", &[Some(face_layout)], false, C::Greater);
        let lines = make("lines", "vs_line", "fs_line", &[Some(line_layout.clone())], false, C::GreaterEqual);
        let lines_hidden = make("hidden lines", "vs_line", "fs_line", &[Some(line_layout.clone())], false, C::Less);
        let lines_overlay = make("overlay lines", "vs_line", "fs_line", &[Some(line_layout)], false, C::Always);

        let textures = FaceTextures { bind_group: make_tex_bind_group(device, &rs.queue, &tex_bgl, &[]), layers: HashMap::new(), key: Vec::new() };
        let vb = wgpu::BufferUsages::VERTEX;
        Self {
            tex_bgl,
            textures,
            bg,
            faces,
            faces_xray,
            lines,
            lines_hidden,
            lines_overlay,
            globals_buf,
            globals_bg,
            face_vb: DynBuffer::new(device, "faces", vb),
            line_vb: DynBuffer::new(device, "lines", vb),
            hidden_vb: DynBuffer::new(device, "hidden lines", vb),
            overlay_vb: DynBuffer::new(device, "overlay", vb),
            targets: None,
            texture_id: None,
        }
    }

    fn ensure_targets(&mut self, rs: &egui_wgpu::RenderState, size: [u32; 2]) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let device = &rs.device;
        let mk = |label: &str, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let msaa = mk("viewport msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = mk("viewport color", COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let depth = mk("viewport depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let mut renderer = rs.renderer.write();
        match self.texture_id {
            Some(id) => renderer.update_egui_texture_from_wgpu_texture(device, &resolve, wgpu::FilterMode::Linear, id),
            None => self.texture_id = Some(renderer.register_native_texture(device, &resolve, wgpu::FilterMode::Linear)),
        }
        self.targets = Some(Targets { size, msaa, resolve, depth });
    }

    /// Upload the model's textures, replacing whatever the array held. Does nothing when the same
    /// set is already resident, so this is cheap to call every frame.
    ///
    /// Images that cannot be decoded, and any past [`MAX_TEXTURES`], are left out; faces using
    /// them fall back to their plain colour.
    pub fn sync_textures(&mut self, rs: &egui_wgpu::RenderState, textures: &slotmap::SlotMap<TexId, Texture>) {
        let key: Vec<TexId> = textures.keys().collect();
        if key == self.textures.key {
            return;
        }
        let mut layers = HashMap::new();
        let mut data = Vec::new();
        for (id, t) in textures {
            if data.len() >= MAX_TEXTURES {
                log::warn!("model has more than {MAX_TEXTURES} textures; the rest are shown as plain colours");
                break;
            }
            let Some(mips) = decode_mips(&t.data) else {
                log::warn!("could not decode texture {:?}", t.name);
                continue;
            };
            layers.insert(id, data.len() as u32);
            data.push(mips);
        }
        self.textures = FaceTextures { bind_group: make_tex_bind_group(&rs.device, &rs.queue, &self.tex_bgl, &data), layers, key };
    }

    /// Which array layer each model texture went to; feeds [`SceneOptions::tex_layers`].
    pub fn tex_layers(&self) -> &HashMap<TexId, u32> {
        &self.textures.layers
    }

    /// Render the scene and return the egui texture holding the image.
    /// `px_scale` is device pixels per UI point; line widths in the scene are in UI points.
    pub fn render(&mut self, rs: &egui_wgpu::RenderState, size: [u32; 2], px_scale: f32, camera: &Camera, scene: &RenderScene) -> egui::TextureId {
        let size = [size[0].max(1), size[1].max(1)];
        self.ensure_targets(rs, size);
        let (device, queue) = (&rs.device, &rs.queue);

        let vp = DVec2::new(size[0] as f64, size[1] as f64);
        let view_proj = camera.view_proj(vp);
        let fwd = camera.forward();
        // Light comes from over the viewer's left shoulder, like SketchUp's default shading.
        let light = (-fwd + camera.up() * 0.8 - camera.right() * 0.5 + sc_math::DVec3::Z * 0.6).normalize();
        let f4 = |v: sc_math::DVec3, w: f32| [v.x as f32, v.y as f32, v.z as f32, w];
        let globals = Globals {
            view_proj: view_proj.as_mat4().to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().as_mat4().to_cols_array_2d(),
            eye: f4(camera.eye, 1.0),
            viewport: [size[0] as f32, size[1] as f32, if camera.perspective { 1.0 } else { 0.0 }, if scene.style == FaceStyle::XRay { 0.5 } else { 1.0 }],
            forward: f4(fwd, px_scale),
            light: f4(light, 0.0),
            sky_top: scene.sky_top,
            sky_horizon: scene.sky_horizon,
            ground: scene.ground,
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));
        self.face_vb.write(device, queue, bytemuck::cast_slice(&scene.faces));
        self.line_vb.write(device, queue, bytemuck::cast_slice(&scene.lines));
        self.hidden_vb.write(device, queue, bytemuck::cast_slice(&scene.hidden_lines));
        self.overlay_vb.write(device, queue, bytemuck::cast_slice(&scene.overlay));

        let t = self.targets.as_ref().unwrap();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    depth_slice: None,
                    resolve_target: Some(&t.resolve),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(1, &self.textures.bind_group, &[]);
            if scene.show_background {
                pass.set_pipeline(&self.bg);
                pass.draw(0..3, 0..1);
            }
            let draw_faces = !scene.faces.is_empty() && scene.style != FaceStyle::Wireframe;
            if draw_faces {
                pass.set_pipeline(if scene.style == FaceStyle::XRay { &self.faces_xray } else { &self.faces });
                pass.set_vertex_buffer(0, self.face_vb.buf.slice(..));
                pass.draw(0..scene.faces.len() as u32, 0..1);
            }
            if !scene.hidden_lines.is_empty() && draw_faces {
                pass.set_pipeline(&self.lines_hidden);
                pass.set_vertex_buffer(0, self.hidden_vb.buf.slice(..));
                pass.draw(0..6, 0..scene.hidden_lines.len() as u32);
            }
            if !scene.lines.is_empty() {
                pass.set_pipeline(&self.lines);
                pass.set_vertex_buffer(0, self.line_vb.buf.slice(..));
                pass.draw(0..6, 0..scene.lines.len() as u32);
            }
            if !scene.overlay.is_empty() {
                pass.set_pipeline(&self.lines_overlay);
                pass.set_vertex_buffer(0, self.overlay_vb.buf.slice(..));
                pass.draw(0..6, 0..scene.overlay.len() as u32);
            }
        }
        queue.submit([encoder.finish()]);
        self.texture_id.unwrap()
    }
}
