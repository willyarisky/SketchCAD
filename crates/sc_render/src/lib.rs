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
use sc_math::DVec2;

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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport layout"),
            bind_group_layouts: &[Some(&bgl)],
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

        let face_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x4, 4 => Uint32];
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

        let vb = wgpu::BufferUsages::VERTEX;
        Self {
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
