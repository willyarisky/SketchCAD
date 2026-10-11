//! Interactive tools. Each tool is a small state machine driven by viewport events.

mod advanced;
mod camera_tools;
mod draw;
mod modify;
mod select;

pub use advanced::{FollowMeTool, ScaleTool, follow_me_path};
pub use camera_tools::{OrbitTool, PanTool, ZoomTool, anchor_point as camera_anchor};
pub use draw::{ArcTool, CircleTool, LineTool, RectangleTool};
pub use modify::{MoveTool, OffsetTool, PushPullTool, RotateTool, TapeMeasureTool, transform_selection};
pub use select::{Entity, EraserTool, PaintBucketTool, SelectTool, pick_entity};

use crate::document::{Definition, Guide, History, Selection, Tag, UndoEntry};
use crate::inference::{Inference, InferenceEngine, Query};
use sc_geom::{Color, DefId, EdgeId, FaceId, Instance, Mesh};
use sc_math::units::UnitSettings;
use sc_math::{DMat4, DVec2, DVec3, Plane};
use sc_render::{Camera, LineInstance};
use slotmap::SlotMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolId {
    Select,
    Eraser,
    PaintBucket,
    Line,
    Arc,
    Rectangle,
    Circle,
    Polygon,
    PushPull,
    Move,
    Rotate,
    Scale,
    FollowMe,
    Offset,
    TapeMeasure,
    Orbit,
    Pan,
    Zoom,
}

impl ToolId {
    pub fn name(self) -> &'static str {
        match self {
            ToolId::Select => "Select",
            ToolId::Eraser => "Eraser",
            ToolId::PaintBucket => "Paint Bucket",
            ToolId::Line => "Line",
            ToolId::Arc => "2 Point Arc",
            ToolId::Rectangle => "Rectangle",
            ToolId::Circle => "Circle",
            ToolId::Polygon => "Polygon",
            ToolId::PushPull => "Push/Pull",
            ToolId::Move => "Move",
            ToolId::Rotate => "Rotate",
            ToolId::Scale => "Scale",
            ToolId::FollowMe => "Follow Me",
            ToolId::Offset => "Offset",
            ToolId::TapeMeasure => "Tape Measure",
            ToolId::Orbit => "Orbit",
            ToolId::Pan => "Pan",
            ToolId::Zoom => "Zoom",
        }
    }

    /// Instructor text: what the tool does and its modifier keys.
    pub fn instructor(self) -> &'static str {
        match self {
            ToolId::Select => "Select entities to modify.\n\n• Click: select one entity\n• Ctrl: add to selection\n• Shift: toggle selection\n• Ctrl+Shift: subtract\n• Double-click a face: face + its edges\n• Triple-click: all connected geometry\n• Drag left to right: window select\n• Drag right to left: crossing select",
            ToolId::Eraser => "Erase edges by clicking or dragging over them. Faces bounded by erased edges are removed; coplanar faces heal.\n\n• Shift: hide instead of erase\n• Ctrl: soften/smooth\n• Ctrl+Shift: unsoften",
            ToolId::PaintBucket => "Apply the current material to faces.\n\n• Alt: sample a material from a face\n• Choose materials in the Materials tray",
            ToolId::Line => "Draw edges. Closed coplanar loops become faces automatically.\n\n• Type a length and press Enter\n• Type [x,y,z] for absolute or <x,y,z> for relative coordinates\n• Arrow keys: lock Red (Right), Green (Left), Blue (Up) axis\n• Shift: lock current inference\n• Esc: stop the line chain",
            ToolId::Arc => "Draw a 2 point arc: click start, click end, then set the bulge.\n\n• Type a bulge distance, or a segment count like 12s",
            ToolId::Rectangle => "Draw rectangles on faces or the ground.\n\n• Type width,height and press Enter\n• Watch for the Square inference",
            ToolId::Circle => "Draw circles: click the center, then the radius.\n\n• Type a radius and press Enter\n• Type 24s to set the number of sides",
            ToolId::Polygon => "Draw regular polygons: click the center, then the radius.\n\n• Type a radius, or 6s to set the number of sides",
            ToolId::PushPull => "Push or pull faces to add or remove volume.\n\n• Click a face, move, click again\n• Type a distance and press Enter\n• Double-click a face to repeat the last distance\n• Ctrl: create a new starting face",
            ToolId::Move => "Move selected entities (or the hovered entity).\n\n• Click a start point, then a destination\n• Ctrl: move a copy\n• After a copy type 3x or /3 for arrays\n• Arrow keys lock axes",
            ToolId::Rotate => "Rotate selected entities.\n\n• Click the center, a start point, then the angle\n• Type an angle in degrees\n• Ctrl: rotate a copy",
            ToolId::Scale => "Scale the selection with grips.\n\n• Corner grips scale uniformly\n• Edge grips scale two axes, face grips one axis\n• Ctrl: scale about the center\n• Type a factor (2, 0.5, -1 to mirror) or a length",
            ToolId::FollowMe => "Sweep a profile along a path.\n\n• Select the path edges first (or a face: its edges become the path)\n• Then click the profile face\n• Circles as paths make lathed shapes",
            ToolId::Offset => "Offset a face's edges inward or outward.\n\n• Click a face, move, click again\n• Type a distance and press Enter\n• Double-click to repeat",
            ToolId::TapeMeasure => "Measure distances and create guides.\n\n• Click two points to measure\n• Click an edge and drag to create a parallel guide line\n• Ctrl: toggle guide creation",
            ToolId::Orbit => "Orbit the camera around the model.\n\n• Drag to orbit\n• Shift: pan\n• Shortcut: hold the middle mouse button",
            ToolId::Pan => "Pan the view.\n\n• Drag to pan\n• Shortcut: Shift + middle mouse button",
            ToolId::Zoom => "Zoom the view.\n\n• Drag up/down to zoom\n• Scroll wheel zooms toward the cursor\n• Shift+Z: Zoom Extents",
        }
    }
}

/// A finished piece of feedback for the frame: 3D lines and screen-space shapes.
#[derive(Default)]
pub struct Overlay {
    /// Lines drawn on top of the model (rubber bands, previews).
    pub lines: Vec<LineInstance>,
    pub shapes: Vec<egui::Shape>,
    pub tooltips: Vec<(egui::Pos2, String)>,
    pub highlight_faces: Vec<FaceId>,
    pub highlight_edges: Vec<EdgeId>,
    pub highlight_instances: Vec<sc_geom::InstanceId>,
}

/// Measurements box state shared by the status bar and tools.
#[derive(Default)]
pub struct Vcb {
    pub label: &'static str,
    pub value: String,
}

pub struct ToolCx<'a> {
    /// The context being edited (model root, or the open group/component), in its local space.
    pub mesh: &'a mut Mesh,
    /// Read-only access to definitions and tags (for instances inside `mesh`).
    pub view: ModelView<'a>,
    /// Everything outside `mesh`, flattened into local coordinates (inference and occlusion).
    pub extra: &'a Mesh,
    /// The view camera expressed in local coordinates.
    pub camera: Camera,
    /// The real camera, for Orbit / Pan / Zoom.
    pub world_camera: &'a mut Camera,
    /// Local → world transform of the active context.
    pub to_world: DMat4,
    pub ctx: Option<DefId>,
    pub selection: &'a mut Selection,
    pub history: &'a mut History,
    /// Guides are stored in world space.
    pub guides: &'a mut Vec<Guide>,
    pub dirty: &'a mut bool,
    pub units: UnitSettings,
    pub size: DVec2,
    pub cursor: DVec2,
    pub inference: &'a mut InferenceEngine,
    pub mods: egui::Modifiers,
    pub vcb: &'a mut Vcb,
    pub material: &'a mut Color,
    /// Pixel delta of the pointer since the previous event (camera tools).
    pub delta: DVec2,
    /// Changes to the edit context, applied by the app after the tool returns.
    pub request: &'a mut Option<ToolRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRequest {
    /// Open a group/component in the active context for editing.
    Enter(sc_geom::InstanceId),
    /// Close the innermost open group/component.
    Exit,
}

impl ToolCx<'_> {
    fn local_guides(&self) -> Vec<Guide> {
        if self.to_world == DMat4::IDENTITY {
            return self.guides.clone();
        }
        let inv = self.to_world.inverse();
        self.guides.iter().map(|g| g.transformed(&inv)).collect()
    }

    pub fn infer(&mut self, from: Option<DVec3>, plane: Option<Plane>) -> Inference {
        let guides = self.local_guides();
        let q = Query { mesh: self.mesh, extra: Some(self.extra), guides: &guides, camera: &self.camera, size: self.size, cursor: self.cursor, from, plane };
        self.inference.infer(&q)
    }

    /// Inference against another mesh (the pre-operation copy), so geometry being moved
    /// never snaps to itself.
    pub fn infer_on(&mut self, mesh: &Mesh, from: Option<DVec3>, plane: Option<Plane>) -> Inference {
        let guides = self.local_guides();
        let q = Query { mesh, extra: Some(self.extra), guides: &guides, camera: &self.camera, size: self.size, cursor: self.cursor, from, plane };
        self.inference.infer(&q)
    }

    pub fn ray(&self) -> sc_math::Ray {
        self.camera.ray(self.cursor, self.size)
    }

    pub fn fmt_len(&self, inches: f64) -> String {
        self.units.format(inches)
    }

    pub fn vcb_input(&self, s: &str) -> Option<sc_math::units::VcbInput> {
        sc_math::units::parse_vcb(s, self.units.unit)
    }

    /// Record the active context before an operation (shown as "Undo <name>").
    pub fn checkpoint(&mut self, name: &str) {
        let m = self.mesh.clone();
        self.checkpoint_with(name, m);
    }

    pub fn checkpoint_with(&mut self, name: &str, before: Mesh) {
        self.history.push(UndoEntry { name: name.to_string(), ctx: self.ctx, mesh: before, guides: self.guides.clone(), defs: None });
        *self.dirty = true;
    }

    /// Add a guide given in local coordinates.
    pub fn add_guide(&mut self, g: Guide) {
        self.guides.push(g.transformed(&self.to_world));
    }
}

/// Read-only view of the parts of the model a tool may need besides its own context.
#[derive(Clone, Copy)]
pub struct ModelView<'a> {
    pub defs: &'a SlotMap<DefId, Definition>,
    pub tags: &'a [Tag],
}

impl ModelView<'_> {
    pub fn instance_visible(&self, i: &Instance) -> bool {
        !i.hidden && i.tag.and_then(|t| self.tags.get(t as usize)).is_none_or(|t| t.visible)
    }

    /// Flattened copy of an instance's definition (and visible nested instances), transformed by `xf`.
    pub fn flatten_into(&self, def: DefId, xf: &DMat4, out: &mut Mesh) {
        let Some(d) = self.defs.get(def) else { return };
        out.append_raw(&d.mesh, xf);
        for i in d.mesh.instances.values() {
            if self.instance_visible(i) {
                self.flatten_into(i.def, &(*xf * i.xform), out);
            }
        }
    }

    pub fn flatten_instance(&self, i: &Instance) -> Mesh {
        let mut m = Mesh::new();
        self.flatten_into(i.def, &i.xform, &mut m);
        m
    }
}

pub trait Tool {
    fn id(&self) -> ToolId;
    fn status(&self) -> String;
    fn vcb_label(&self) -> &'static str {
        ""
    }
    /// Whether the inference marker/tooltip should be shown.
    fn uses_inference(&self) -> bool {
        true
    }
    fn on_move(&mut self, _cx: &mut ToolCx) {}
    fn on_press(&mut self, _cx: &mut ToolCx) {}
    fn on_release(&mut self, _cx: &mut ToolCx, _dragged: bool) {}
    fn on_double_click(&mut self, _cx: &mut ToolCx) {}
    /// Return true if the key was consumed.
    fn on_key(&mut self, _cx: &mut ToolCx, _key: egui::Key) -> bool {
        false
    }
    fn on_vcb(&mut self, _cx: &mut ToolCx, _text: &str) -> Result<(), String> {
        Err("This tool takes no measurements".into())
    }
    /// Esc: abandon the current operation, restoring any preview.
    fn cancel(&mut self, _cx: &mut ToolCx) {}
    /// Switching to another tool: abandon the operation but keep the selection.
    fn deactivate(&mut self, cx: &mut ToolCx) {
        self.cancel(cx);
    }
    fn draw(&self, _cx: &ToolCx, _ov: &mut Overlay) {}
    fn cursor(&self) -> egui::CursorIcon {
        egui::CursorIcon::Crosshair
    }
    /// An operation is in progress (preview geometry may differ from the committed model).
    fn busy(&self) -> bool {
        self.start_point().is_some()
    }
    /// The copy modifier (Ctrl, or Option on macOS) was tapped.
    fn on_copy_toggle(&mut self, _cx: &mut ToolCx) {}
    /// Start point of the operation in progress (anchor for axis locks and inferences).
    fn start_point(&self) -> Option<DVec3> {
        None
    }
}

pub fn make_tool(id: ToolId) -> Box<dyn Tool> {
    match id {
        ToolId::Select => Box::new(SelectTool::default()),
        ToolId::Eraser => Box::new(EraserTool::default()),
        ToolId::PaintBucket => Box::new(PaintBucketTool),
        ToolId::Line => Box::new(LineTool::default()),
        ToolId::Arc => Box::new(ArcTool::default()),
        ToolId::Rectangle => Box::new(RectangleTool::default()),
        ToolId::Circle => Box::new(CircleTool::new(false)),
        ToolId::Polygon => Box::new(CircleTool::new(true)),
        ToolId::PushPull => Box::new(PushPullTool::default()),
        ToolId::Move => Box::new(MoveTool::default()),
        ToolId::Rotate => Box::new(RotateTool::default()),
        ToolId::Scale => Box::new(ScaleTool::default()),
        ToolId::FollowMe => Box::new(FollowMeTool::default()),
        ToolId::Offset => Box::new(OffsetTool::default()),
        ToolId::TapeMeasure => Box::new(TapeMeasureTool::default()),
        ToolId::Orbit => Box::new(OrbitTool::default()),
        ToolId::Pan => Box::new(PanTool::default()),
        ToolId::Zoom => Box::new(ZoomTool::default()),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers

pub fn rgba32(c: egui::Color32) -> [f32; 4] {
    sc_render::scene_rgba([c.r(), c.g(), c.b(), c.a()])
}

pub const RUBBER_BAND: egui::Color32 = egui::Color32::BLACK;

/// Color of a line from `a` to `b`: axis-colored when parallel to an axis, like SketchUp.
pub fn axis_color(a: DVec3, b: DVec3) -> egui::Color32 {
    let d = b - a;
    for ax in crate::inference::Axis::ALL {
        if sc_math::parallel(d, ax.dir()) {
            return ax.color();
        }
    }
    RUBBER_BAND
}

pub fn line(a: DVec3, b: DVec3, c: egui::Color32, width: f32) -> LineInstance {
    LineInstance::new(a, b, rgba32(c), width)
}

/// A basis for drawing on `plane`, aligned with world axes when possible.
pub fn plane_axes(normal: DVec3) -> (DVec3, DVec3) {
    let n = normal.normalize();
    let mut u = DVec3::X - n * DVec3::X.dot(n);
    if u.length() < 1e-6 {
        u = DVec3::Y - n * DVec3::Y.dot(n);
    }
    let u = u.normalize();
    (u, n.cross(u).normalize())
}

/// The plane a shape tool should draw on, given where its first click landed.
pub fn drawing_plane(cx: &ToolCx, inf: &Inference) -> Plane {
    // On a face, or on an edge of one (from any context): draw on that face's plane.
    if let Some(n) = inf.face_normal {
        return Plane::new(inf.point, n);
    }
    // Free space: the axis plane most facing the camera (SketchUp picks the ground unless looking edge-on).
    let f = cx.camera.forward();
    let n = if f.z.abs() > 0.25 { DVec3::Z } else if f.x.abs() > f.y.abs() { DVec3::X } else { DVec3::Y };
    Plane::new(inf.point, n)
}

/// Point on `plane` under the cursor, honoring inferences that already lie on it.
pub fn point_on_plane(cx: &mut ToolCx, plane: &Plane, from: DVec3) -> (DVec3, Inference) {
    let inf = cx.infer(Some(from), Some(*plane));
    if plane.contains(inf.point) {
        return (inf.point, inf);
    }
    let ray = cx.ray();
    let p = plane.intersect_ray(&ray).map(|t| ray.at(t)).unwrap_or_else(|| plane.project(inf.point));
    // A snapped point off the plane is projected onto it (SketchUp keeps the shape planar).
    let p = if inf.kind.is_point() { plane.project(inf.point) } else { p };
    (p, inf)
}
