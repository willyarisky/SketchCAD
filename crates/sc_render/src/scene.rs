//! CPU-side scene description handed to the renderer each frame.

use bytemuck::{Pod, Zeroable};
use sc_geom::{Color, EdgeId, FaceId, Mesh};
use sc_math::{DMat4, DVec3};

pub mod flags {
    pub const SELECTED: u32 = 1;
    pub const UNLIT: u32 = 2;
    pub const FADED: u32 = 4;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct FaceVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub front: [f32; 4],
    pub back: [f32; 4],
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct LineInstance {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [f32; 4],
    /// x: width px, y: dash px (0 = solid), z: depth bias factor (>1 pulls toward camera), w: unused
    pub style: [f32; 4],
}

impl LineInstance {
    pub fn new(a: DVec3, b: DVec3, color: [f32; 4], width: f32) -> Self {
        Self { a: a.as_vec3().to_array(), b: b.as_vec3().to_array(), color, style: [width, 0.0, 1.0005, 0.0] }
    }

    pub fn dashed(mut self, dash_px: f32) -> Self {
        self.style[1] = dash_px;
        self
    }
}

/// SketchUp face styles (View → Face Style).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FaceStyle {
    XRay,
    Wireframe,
    HiddenLine,
    Shaded,
    ShadedWithTextures,
    Monochrome,
}

/// sRGB bytes to the gamma-space floats the viewport renders with.
pub fn rgba(c: Color) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

pub const DEFAULT_FRONT: Color = [255, 255, 255, 255];
pub const DEFAULT_BACK: Color = [164, 178, 187, 255];
pub const EDGE_COLOR: Color = [0, 0, 0, 255];
pub const SELECTION_COLOR: Color = [0, 0, 255, 255];
pub const FADED_EDGE_COLOR: Color = [150, 150, 150, 255];

pub struct RenderScene {
    pub faces: Vec<FaceVertex>,
    pub lines: Vec<LineInstance>,
    /// Edges drawn only where hidden behind faces (Back Edges / X-ray), dashed.
    pub hidden_lines: Vec<LineInstance>,
    /// Tool feedback drawn on top of everything.
    pub overlay: Vec<LineInstance>,
    pub style: FaceStyle,
    pub show_background: bool,
    pub sky_top: [f32; 4],
    pub sky_horizon: [f32; 4],
    pub ground: [f32; 4],
}

pub struct SceneOptions {
    pub style: FaceStyle,
    pub back_edges: bool,
    pub show_axes: bool,
    pub edge_width: f32,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self { style: FaceStyle::Shaded, back_edges: false, show_axes: true, edge_width: 1.3 }
    }
}

impl RenderScene {
    pub fn empty(style: FaceStyle) -> Self {
        Self {
            faces: Vec::new(),
            lines: Vec::new(),
            hidden_lines: Vec::new(),
            overlay: Vec::new(),
            style,
            show_background: true,
            sky_top: rgba([140, 180, 225, 255]),
            sky_horizon: rgba([226, 236, 245, 255]),
            ground: rgba([204, 202, 192, 255]),
        }
    }

    pub fn build(
        mesh: &Mesh,
        opts: &SceneOptions,
        edge_selected: impl Fn(EdgeId) -> bool,
        face_selected: impl Fn(FaceId) -> bool,
    ) -> Self {
        let mut s = Self::empty(opts.style);
        s.add_mesh(mesh, &DMat4::IDENTITY, opts, 0, edge_selected, face_selected);
        if opts.show_axes {
            s.push_axes();
        }
        s
    }

    /// Add a mesh transformed by `xf`. `extra_flags` applies to every face (e.g. [`flags::FADED`]
    /// for geometry outside the context being edited, or [`flags::SELECTED`] for a selected group).
    pub fn add_mesh(
        &mut self,
        mesh: &Mesh,
        xf: &DMat4,
        opts: &SceneOptions,
        extra_flags: u32,
        edge_selected: impl Fn(EdgeId) -> bool,
        face_selected: impl Fn(FaceId) -> bool,
    ) {
        let style = opts.style;
        let identity = *xf == DMat4::IDENTITY;
        let t = |p: DVec3| if identity { p } else { xf.transform_point3(p) };
        let smooth = mesh.smooth_normals();
        let tn = |n: DVec3| if identity { n } else { sc_geom::transform_normal(xf, n) };
        for (fid, face) in &mesh.faces {
            let (front, back) = match style {
                FaceStyle::HiddenLine => ([255, 255, 255, 255], [255, 255, 255, 255]),
                FaceStyle::Monochrome => (DEFAULT_FRONT, DEFAULT_BACK),
                _ => (face.front.unwrap_or(DEFAULT_FRONT), face.back.unwrap_or(DEFAULT_BACK)),
            };
            let mut fl = extra_flags;
            if face_selected(fid) {
                fl |= flags::SELECTED;
            }
            if style == FaceStyle::HiddenLine {
                fl |= flags::UNLIT;
            }
            let flat = tn(face.plane.normal);
            let (front, back) = (rgba(front), rgba(back));
            for tri in mesh.triangulate_face_ids(fid) {
                for v in tri {
                    let n = smooth.get(&(fid, v)).map_or(flat, |n| tn(*n));
                    self.faces.push(FaceVertex { pos: t(mesh.pos(v)).as_vec3().to_array(), normal: n.as_vec3().to_array(), front, back, flags: fl });
                }
            }
        }
        let faded = extra_flags & flags::FADED != 0;
        let edge = rgba(if faded { FADED_EDGE_COLOR } else { EDGE_COLOR });
        let sel = rgba(SELECTION_COLOR);
        for (eid, e) in &mesh.edges {
            if e.hidden || e.soft {
                continue;
            }
            let (a, b) = (t(mesh.pos(e.v[0])), t(mesh.pos(e.v[1])));
            let color = if edge_selected(eid) { sel } else { edge };
            self.lines.push(LineInstance::new(a, b, color, opts.edge_width));
            if opts.back_edges || style == FaceStyle::XRay {
                self.hidden_lines.push(LineInstance::new(a, b, color, opts.edge_width * 0.8).dashed(3.0));
            }
        }
    }

    /// Box outline (selected groups/components show their bounds in blue, like SketchUp).
    pub fn push_box(&mut self, corners: &[DVec3; 8], color: [f32; 4], dash: f32) {
        const E: [(usize, usize); 12] = [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)];
        for (a, b) in E {
            let l = LineInstance::new(corners[a], corners[b], color, 1.2);
            self.lines.push(if dash > 0.0 { l.dashed(dash) } else { l });
        }
    }

    /// Red/green/blue drawing axes: solid on the positive side, dotted on the negative.
    pub fn push_axes(&mut self) {
        let len = 1.0e6;
        let axes = [(DVec3::X, [200u8, 0, 0, 255]), (DVec3::Y, [0, 155, 0, 255]), (DVec3::Z, [0, 0, 220, 255])];
        for (dir, c) in axes {
            let c = rgba(c);
            self.lines.push(LineInstance::new(DVec3::ZERO, dir * len, c, 1.3));
            self.lines.push(LineInstance::new(DVec3::ZERO, -dir * len, c, 1.0).dashed(3.0));
        }
    }
}
