//! Modification tools: Push/Pull, Move, Rotate, Offset, Tape Measure.

use super::select::{Entity, pick_entity};
use super::*;
use crate::document::{Guide, Selection};
use sc_geom::{Mesh, PushPullMode, VertexId};
use sc_math::units::VcbInput;
use sc_math::{DMat4, DVec2, geom2d};
use std::collections::HashSet;

fn tooltip_at(ov: &mut Overlay, cx: &ToolCx, text: &str) {
    let p = egui::pos2(cx.cursor.x as f32, cx.cursor.y as f32) + egui::vec2(14.0, 18.0);
    ov.tooltips.push((p, text.to_string()));
}

fn plus_badge(ov: &mut Overlay, cx: &ToolCx) {
    let p = egui::pos2(cx.cursor.x as f32, cx.cursor.y as f32) + egui::vec2(12.0, -12.0);
    let s = egui::Stroke::new(2.0, egui::Color32::BLACK);
    ov.shapes.push(egui::Shape::line_segment([p - egui::vec2(4.0, 0.0), p + egui::vec2(4.0, 0.0)], s));
    ov.shapes.push(egui::Shape::line_segment([p - egui::vec2(0.0, 4.0), p + egui::vec2(0.0, 4.0)], s));
}

// ---------------------------------------------------------------------------

struct PushPullOp {
    face: FaceId,
    base: Mesh,
    start: DVec3,
    normal: DVec3,
    distance: f64,
    mode: PushPullMode,
}

#[derive(Default)]
pub struct PushPullTool {
    op: Option<PushPullOp>,
    hover: Option<FaceId>,
    new_face_mode: bool,
    last_distance: Option<f64>,
    /// Base mesh + face + mode of the last commit, so the distance can be retyped.
    last: Option<(Mesh, FaceId, PushPullMode)>,
    press_started: bool,
}

impl PushPullTool {
    fn apply(op: &PushPullOp, mesh: &mut Mesh, d: f64) {
        *mesh = op.base.clone();
        mesh.revision = op.base.revision + 1;
        mesh.push_pull(op.face, d, op.mode);
    }

    fn mode(&self) -> PushPullMode {
        if self.new_face_mode { PushPullMode::NewStartingFace } else { PushPullMode::Auto }
    }

    fn commit(&mut self, cx: &mut ToolCx) {
        let Some(op) = self.op.take() else { return };
        if op.distance.abs() < sc_math::TOL {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
            return;
        }
        cx.checkpoint_with("Push/Pull", op.base.clone());
        self.last_distance = Some(op.distance);
        self.last = Some((op.base, op.face, op.mode));
        cx.selection.prune(cx.mesh);
    }
}

impl Tool for PushPullTool {
    fn id(&self) -> ToolId {
        ToolId::PushPull
    }
    fn busy(&self) -> bool {
        self.op.is_some()
    }
    fn status(&self) -> String {
        if self.op.is_none() {
            "Pick a face to push or pull. Ctrl = toggle create new starting face. Double-click = repeat.".into()
        } else {
            "Pick distance or enter value.".into()
        }
    }
    fn vcb_label(&self) -> &'static str {
        "Distance"
    }
    fn uses_inference(&self) -> bool {
        self.op.is_some()
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let Some(op) = &mut self.op else {
            let ray = cx.ray();
            self.hover = cx.mesh.pick_face(&ray, 0.0).map(|h| h.face);
            return;
        };
        let inf = cx.infer_on(&op.base, None, None);
        let d = if inf.kind.is_point() || matches!(inf.kind, crate::inference::Kind::OnEdge) {
            (inf.point - op.start).dot(op.normal)
        } else {
            let ray = cx.camera.ray(cx.cursor, cx.size);
            match ray.closest_to_line(op.start, op.normal) {
                Some((_, p)) => (p - op.start).dot(op.normal),
                None => op.distance,
            }
        };
        op.distance = d;
        Self::apply(op, cx.mesh, d);
        cx.vcb.value = cx.units.format(d.abs());
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        if self.op.is_some() {
            self.on_move(cx);
            self.commit(cx);
            self.press_started = false;
            return;
        }
        let ray = cx.ray();
        let Some(hit) = cx.mesh.pick_face(&ray, 0.0) else { return };
        let normal = cx.mesh.faces[hit.face].plane.normal;
        self.op = Some(PushPullOp { face: hit.face, base: cx.mesh.clone(), start: hit.point, normal, distance: 0.0, mode: self.mode() });
        self.press_started = true;
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        if dragged && self.press_started {
            self.on_move(cx);
            self.commit(cx);
        }
        self.press_started = false;
    }
    fn on_double_click(&mut self, cx: &mut ToolCx) {
        // The first click of the double-click started (and the second ended) a zero-length op.
        let Some(d) = self.last_distance else { return };
        let ray = cx.ray();
        let Some(hit) = cx.mesh.pick_face(&ray, 0.0) else { return };
        let base = cx.mesh.clone();
        cx.mesh.push_pull(hit.face, d, self.mode());
        cx.checkpoint_with("Push/Pull", base.clone());
        self.last = Some((base, hit.face, self.mode()));
    }
    fn on_copy_toggle(&mut self, cx: &mut ToolCx) {
        self.new_face_mode = !self.new_face_mode;
        if let Some(op) = &mut self.op {
            op.mode = if self.new_face_mode { PushPullMode::NewStartingFace } else { PushPullMode::Auto };
            let d = op.distance;
            Self::apply(op, cx.mesh, d);
        }
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let Some(VcbInput::Lengths(v)) = cx.vcb_input(text) else { return Err("Invalid distance".into()) };
        let [d] = v.as_slice() else { return Err("Enter one distance".into()) };
        if let Some(op) = &mut self.op {
            let signed = if op.distance < 0.0 { -d } else { *d };
            op.distance = signed;
            Self::apply(op, cx.mesh, signed);
            self.commit(cx);
            return Ok(());
        }
        let Some((base, face, mode)) = &self.last else { return Err("Pick a face first".into()) };
        let sign = self.last_distance.map_or(1.0, f64::signum);
        *cx.mesh = base.clone();
        cx.mesh.revision += base.revision + 1;
        cx.mesh.push_pull(*face, d * sign, *mode);
        self.last_distance = Some(d * sign);
        cx.vcb.value = cx.fmt_len(*d);
        Ok(())
    }
    fn cancel(&mut self, cx: &mut ToolCx) {
        if let Some(op) = self.op.take() {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
        }
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        if self.new_face_mode {
            plus_badge(ov, cx);
        }
        match &self.op {
            None => ov.highlight_faces.extend(self.hover),
            Some(op) => {
                ov.lines.push(line(op.start, op.start + op.normal * op.distance, egui::Color32::from_gray(60), 1.0).dashed(4.0));
            }
        }
    }
}

// ---------------------------------------------------------------------------

struct MoveOp {
    base: Mesh,
    from: DVec3,
    sel: Selection,
    verts: HashSet<VertexId>,
    auto_selected: bool,
    delta: DVec3,
}

#[derive(Default)]
pub struct MoveTool {
    op: Option<MoveOp>,
    copy: bool,
    /// Last copy: (base, selection, delta) for `3x` / `/3` arrays.
    last_copy: Option<(Mesh, Selection, DVec3)>,
    press_started: bool,
}

impl MoveTool {
    fn apply(&self, mesh: &mut Mesh, op: &MoveOp, delta: DVec3, copies: u32, divide: bool) {
        *mesh = op.base.clone();
        mesh.revision = op.base.revision + 1;
        apply_move(mesh, &op.sel, &op.verts, delta, self.copy, copies, divide);
    }

    fn commit(&mut self, cx: &mut ToolCx) {
        let Some(op) = self.op.take() else { return };
        if op.delta.length() < sc_math::TOL {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
            if op.auto_selected {
                cx.selection.clear();
            }
            return;
        }
        cx.checkpoint_with(if self.copy { "Copy" } else { "Move" }, op.base.clone());
        if self.copy {
            self.last_copy = Some((op.base, op.sel, op.delta));
        } else {
            self.last_copy = None;
        }
        cx.selection.prune(cx.mesh);
        if op.auto_selected {
            cx.selection.clear();
        }
        cx.inference.unlock();
    }
}

fn apply_move(mesh: &mut Mesh, sel: &Selection, verts: &HashSet<VertexId>, delta: DVec3, copy: bool, copies: u32, divide: bool) {
    let n = copies.max(1);
    transform_selection(mesh, sel, verts, copy, n, |i| {
        let k = if divide { i as f64 / n as f64 } else { i as f64 };
        DMat4::from_translation(delta * k)
    });
}

/// Move (or copy `copies` times) the selection. `xf(i)` is the transform of copy `i` (1-based);
/// without copying only `xf(1)` is applied, to vertices and to instance placements.
pub fn transform_selection(mesh: &mut Mesh, sel: &Selection, verts: &HashSet<VertexId>, copy: bool, copies: u32, xf: impl Fn(u32) -> DMat4) {
    if copy {
        // Copy from a snapshot so later copies never see (or split) earlier ones' ids.
        let edges: Vec<_> = sel.edges.iter().copied().collect();
        let faces: Vec<_> = sel.faces.iter().copied().collect();
        let insts: Vec<_> = sel.instances.iter().copied().collect();
        let src = mesh.clone().extract(&edges, &faces, &insts);
        for i in 1..=copies.max(1) {
            mesh.merge_mesh(&src, xf(i));
        }
    } else {
        let m = xf(1);
        mesh.transform_vertices(verts, |p| m.transform_point3(p));
        if m.determinant() < 0.0 {
            // Mirroring turns faces inside out; flip the ones that moved entirely.
            let flipped: Vec<_> = mesh.faces.iter().filter(|(_, f)| f.vertex_ids().all(|v| verts.contains(&v))).map(|(id, _)| id).collect();
            for f in flipped {
                mesh.reverse_loops(f);
            }
        }
        for id in &sel.instances {
            if let Some(inst) = mesh.instances.get_mut(*id) {
                inst.xform = m * inst.xform;
            }
        }
        mesh.revision += 1;
    }
}

/// Select whatever is under the cursor (Move / Rotate act on the hovered entity when nothing is selected).
fn auto_select(cx: &mut ToolCx) -> bool {
    match pick_entity(cx) {
        Some(Entity::Edge(e)) => cx.selection.edges.insert(e),
        Some(Entity::Face(f)) => cx.selection.faces.insert(f),
        Some(Entity::Instance(i)) => cx.selection.instances.insert(i),
        None => false,
    }
}

impl Tool for MoveTool {
    fn id(&self) -> ToolId {
        ToolId::Move
    }
    fn status(&self) -> String {
        if self.op.is_none() { "Select objects to move, then pick the start point. Ctrl = toggle Copy.".into() } else { "Pick destination or enter distance. Arrows lock axes.".into() }
    }
    fn vcb_label(&self) -> &'static str {
        "Distance"
    }
    fn cursor(&self) -> egui::CursorIcon {
        egui::CursorIcon::Move
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let Some(op) = &self.op else {
            cx.infer(None, None);
            return;
        };
        let inf = cx.infer_on(&op.base, Some(op.from), None);
        let delta = inf.point - op.from;
        let mut mesh = std::mem::take(&mut *cx.mesh);
        self.apply(&mut mesh, op, delta, 1, false);
        *cx.mesh = mesh;
        if let Some(op) = &mut self.op {
            op.delta = delta;
        }
        cx.vcb.value = cx.fmt_len(delta.length());
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        if self.op.is_some() {
            self.on_move(cx);
            self.commit(cx);
            self.press_started = false;
            return;
        }
        let mut auto_selected = false;
        if cx.selection.is_empty() {
            if !auto_select(cx) {
                return;
            }
            auto_selected = true;
        }
        let inf = cx.infer(None, None);
        let sel = cx.selection.with_bounding_edges(cx.mesh);
        let verts = sel.vertices(cx.mesh);
        self.op = Some(MoveOp { base: cx.mesh.clone(), from: inf.point, sel, verts, auto_selected, delta: DVec3::ZERO });
        self.press_started = true;
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        if dragged && self.press_started {
            self.on_move(cx);
            self.commit(cx);
        }
        self.press_started = false;
    }
    fn on_copy_toggle(&mut self, cx: &mut ToolCx) {
        self.copy = !self.copy;
        if self.op.is_some() {
            self.on_move(cx);
        }
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        match cx.vcb_input(text) {
            Some(VcbInput::Lengths(v)) if v.len() == 1 => {
                let Some(op) = &mut self.op else { return Err("Pick a start point first".into()) };
                if op.delta.length() < sc_math::TOL {
                    return Err("Move the mouse to set a direction".into());
                }
                op.delta = op.delta.normalize() * v[0];
                let mut mesh = std::mem::take(&mut *cx.mesh);
                let op = self.op.as_ref().unwrap();
                self.apply(&mut mesh, op, op.delta, 1, false);
                *cx.mesh = mesh;
                self.commit(cx);
                Ok(())
            }
            Some(VcbInput::Relative(d)) | Some(VcbInput::Absolute(d)) => {
                let Some(op) = &mut self.op else { return Err("Pick a start point first".into()) };
                let d = DVec3::from_array(d);
                op.delta = if text.trim_start().starts_with('[') { d - op.from } else { d };
                let mut mesh = std::mem::take(&mut *cx.mesh);
                let op = self.op.as_ref().unwrap();
                self.apply(&mut mesh, op, op.delta, 1, false);
                *cx.mesh = mesh;
                self.commit(cx);
                Ok(())
            }
            Some(VcbInput::Multiply(n)) | Some(VcbInput::Divide(n)) if n >= 1 => {
                let divide = matches!(cx.vcb_input(text), Some(VcbInput::Divide(_)));
                let Some((base, sel, delta)) = &self.last_copy else { return Err("Arrays need a copy (Ctrl+Move) first".into()) };
                let mut mesh = base.clone();
                mesh.revision = cx.mesh.revision + 1;
                let verts = sel.vertices(base);
                apply_move(&mut mesh, sel, &verts, *delta, true, n, divide);
                *cx.mesh = mesh;
                Ok(())
            }
            _ => Err("Invalid value".into()),
        }
    }
    fn cancel(&mut self, cx: &mut ToolCx) {
        if let Some(op) = self.op.take() {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
            if op.auto_selected {
                cx.selection.clear();
            }
        }
        cx.inference.unlock();
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        if self.copy {
            plus_badge(ov, cx);
        }
        if let Some(op) = &self.op {
            ov.lines.push(line(op.from, op.from + op.delta, axis_color(op.from, op.from + op.delta), 1.5).dashed(5.0));
        }
    }
    fn start_point(&self) -> Option<DVec3> {
        self.op.as_ref().map(|o| o.from)
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RotateOp {
    base: Mesh,
    center: DVec3,
    normal: DVec3,
    reference: Option<DVec3>,
    angle: f64,
    sel: Selection,
    verts: HashSet<VertexId>,
    auto_selected: bool,
}

#[derive(Default)]
pub struct RotateTool {
    op: Option<RotateOp>,
    copy: bool,
}

fn rotate_point(p: DVec3, c: DVec3, axis: DVec3, angle: f64) -> DVec3 {
    let q = sc_math::DQuat::from_axis_angle(axis.normalize(), angle);
    c + q * (p - c)
}

impl RotateTool {
    fn preview(&self, mesh: &mut Mesh) {
        let Some(op) = &self.op else { return };
        *mesh = op.base.clone();
        mesh.revision = op.base.revision + 1;
        let (c, n, a) = (op.center, op.normal, op.angle);
        let rot = DMat4::from_translation(c) * DMat4::from_axis_angle(n.normalize(), a) * DMat4::from_translation(-c);
        transform_selection(mesh, &op.sel, &op.verts, self.copy, 1, |_| rot);
    }

    fn commit(&mut self, cx: &mut ToolCx) {
        let Some(op) = self.op.take() else { return };
        if op.angle.abs() < 1e-9 {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
        } else {
            cx.checkpoint_with("Rotate", op.base.clone());
            cx.selection.prune(cx.mesh);
        }
        if op.auto_selected {
            cx.selection.clear();
        }
    }
}

impl Tool for RotateTool {
    fn id(&self) -> ToolId {
        ToolId::Rotate
    }
    fn status(&self) -> String {
        match &self.op {
            None => "Pick the center point of rotation. Ctrl = toggle Copy.".into(),
            Some(op) if op.reference.is_none() => "Pick a point to start the rotation.".into(),
            _ => "Pick the rotation angle or enter value.".into(),
        }
    }
    fn vcb_label(&self) -> &'static str {
        "Angle"
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let Some(op) = &self.op else {
            cx.infer(None, None);
            return;
        };
        let plane = Plane::new(op.center, op.normal);
        let base = op.base.clone();
        let inf = cx.infer_on(&base, Some(op.center), Some(plane));
        let ray = cx.ray();
        let p = if plane.contains(inf.point) { inf.point } else { plane.intersect_ray(&ray).map(|t| ray.at(t)).unwrap_or(inf.point) };
        let op = self.op.as_mut().unwrap();
        let Some(r) = op.reference else { return };
        let (v0, v1) = ((r - op.center).normalize(), (p - op.center));
        if v1.length() < sc_math::TOL {
            return;
        }
        let v1 = v1.normalize();
        let mut angle = v0.cross(v1).dot(op.normal).atan2(v0.dot(v1));
        // Snap to 15° increments, like SketchUp's default angle snapping.
        let snap = 15f64.to_radians();
        let k = (angle / snap).round();
        if (angle - k * snap).abs() < 2f64.to_radians() {
            angle = k * snap;
        }
        op.angle = angle;
        cx.vcb.value = format!("{:.1}", angle.to_degrees());
        let mut mesh = std::mem::take(&mut *cx.mesh);
        self.preview(&mut mesh);
        *cx.mesh = mesh;
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        match &self.op {
            None => {
                let mut auto_selected = false;
                if cx.selection.is_empty() {
                    if !auto_select(cx) {
                        return;
                    }
                    auto_selected = true;
                }
                let inf = cx.infer(None, None);
                let normal = drawing_plane(cx, &inf).normal;
                let sel = cx.selection.with_bounding_edges(cx.mesh);
                let verts = sel.vertices(cx.mesh);
                self.op = Some(RotateOp { base: cx.mesh.clone(), center: inf.point, normal, reference: None, angle: 0.0, sel, verts, auto_selected });
            }
            Some(op) if op.reference.is_none() => {
                let plane = Plane::new(op.center, op.normal);
                let (p, _) = point_on_plane(cx, &plane, op.center);
                if p.distance(op.center) > sc_math::TOL {
                    self.op.as_mut().unwrap().reference = Some(p);
                }
            }
            Some(_) => {
                self.on_move(cx);
                self.commit(cx);
            }
        }
    }
    fn on_key(&mut self, _cx: &mut ToolCx, key: egui::Key) -> bool {
        // Arrow keys choose the protractor plane before the center is picked.
        let Some(op) = &mut self.op else { return false };
        if op.reference.is_some() {
            return false;
        }
        op.normal = match key {
            egui::Key::ArrowUp => DVec3::Z,
            egui::Key::ArrowRight => DVec3::X,
            egui::Key::ArrowLeft => DVec3::Y,
            _ => return false,
        };
        true
    }
    fn on_copy_toggle(&mut self, cx: &mut ToolCx) {
        self.copy = !self.copy;
        let mut mesh = std::mem::take(&mut *cx.mesh);
        if self.op.as_ref().is_some_and(|o| o.reference.is_some()) {
            self.preview(&mut mesh);
        }
        *cx.mesh = mesh;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let deg: f64 = text.trim().trim_end_matches('°').parse().map_err(|_| "Invalid angle".to_string())?;
        let Some(op) = &mut self.op else { return Err("Pick the center first".into()) };
        if op.reference.is_none() {
            return Err("Pick the start of the rotation first".into());
        }
        op.angle = deg.to_radians();
        let mut mesh = std::mem::take(&mut *cx.mesh);
        self.preview(&mut mesh);
        *cx.mesh = mesh;
        self.commit(cx);
        Ok(())
    }
    fn cancel(&mut self, cx: &mut ToolCx) {
        if let Some(op) = self.op.take() {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
            if op.auto_selected {
                cx.selection.clear();
            }
        }
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        if self.copy {
            plus_badge(ov, cx);
        }
        let Some(op) = &self.op else { return };
        // Protractor: a circle ~50px across in the rotation plane, colored by its axis.
        let r = cx.camera.units_per_pixel(op.center, cx.size.y) * 45.0;
        let (u, v) = plane_axes(op.normal);
        let color = crate::inference::Axis::ALL
            .into_iter()
            .find(|a| sc_math::parallel(a.dir(), op.normal))
            .map_or(egui::Color32::from_rgb(120, 0, 160), |a| a.color());
        let n = 48;
        for i in 0..n {
            let a0 = i as f64 / n as f64 * std::f64::consts::TAU;
            let a1 = (i + 1) as f64 / n as f64 * std::f64::consts::TAU;
            ov.lines.push(line(op.center + (u * a0.cos() + v * a0.sin()) * r, op.center + (u * a1.cos() + v * a1.sin()) * r, color, 1.5));
        }
        if let Some(rf) = op.reference {
            ov.lines.push(line(op.center, rf, egui::Color32::from_gray(60), 1.0).dashed(4.0));
            let cur = rotate_point(rf, op.center, op.normal, op.angle);
            ov.lines.push(line(op.center, cur, egui::Color32::BLACK, 1.5).dashed(4.0));
        }
    }
    fn start_point(&self) -> Option<DVec3> {
        self.op.as_ref().map(|o| o.center)
    }
}

// ---------------------------------------------------------------------------

struct OffsetOp {
    base: Mesh,
    face: FaceId,
    distance: f64,
}

#[derive(Default)]
pub struct OffsetTool {
    op: Option<OffsetOp>,
    hover: Option<FaceId>,
    last_distance: Option<f64>,
    press_started: bool,
}

/// Offset a closed CCW polygon inward by `d` (negative = outward) using mitered corners.
pub fn offset_polygon(pts: &[DVec2], d: f64) -> Vec<DVec2> {
    let n = pts.len();
    let ccw = geom2d::signed_area(pts) > 0.0;
    let s = if ccw { 1.0 } else { -1.0 };
    (0..n)
        .map(|i| {
            let (p0, p1, p2) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let e1 = (p1 - p0).normalize();
            let e2 = (p2 - p1).normalize();
            let n1 = DVec2::new(-e1.y, e1.x) * s;
            let n2 = DVec2::new(-e2.y, e2.x) * s;
            let denom = 1.0 + n1.dot(n2);
            if denom.abs() < 1e-9 { p1 + n1 * d } else { p1 + (n1 + n2) * (d / denom) }
        })
        .collect()
}

impl OffsetTool {
    fn preview(op: &OffsetOp, mesh: &mut Mesh) {
        *mesh = op.base.clone();
        mesh.revision = op.base.revision + 1;
        if op.distance.abs() < sc_math::TOL {
            return;
        }
        let face = &op.base.faces[op.face];
        let pts: Vec<DVec2> = face.outer.iter().map(|v| face.plane.to_2d(op.base.pos(*v))).collect();
        let off = offset_polygon(&pts, op.distance);
        let pts3: Vec<DVec3> = off.iter().map(|p| face.plane.to_3d(*p)).collect();
        mesh.add_polyline(&pts3, true, false);
    }

    fn signed_distance(cx: &mut ToolCx, op: &OffsetOp) -> f64 {
        let face = &op.base.faces[op.face];
        let ray = cx.ray();
        let Some(t) = face.plane.intersect_ray(&ray) else { return op.distance };
        let p = face.plane.to_2d(ray.at(t));
        let pts: Vec<DVec2> = face.outer.iter().map(|v| face.plane.to_2d(op.base.pos(*v))).collect();
        let d = geom2d::distance_to_boundary(p, &pts);
        if geom2d::point_in_polygon(p, &pts) { d } else { -d }
    }

    fn commit(&mut self, cx: &mut ToolCx) {
        let Some(op) = self.op.take() else { return };
        if op.distance.abs() < sc_math::TOL {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
            return;
        }
        cx.checkpoint_with("Offset", op.base);
        self.last_distance = Some(op.distance);
    }
}

impl Tool for OffsetTool {
    fn id(&self) -> ToolId {
        ToolId::Offset
    }
    fn busy(&self) -> bool {
        self.op.is_some()
    }
    fn status(&self) -> String {
        if self.op.is_none() { "Pick a face to offset. Double-click = repeat last offset.".into() } else { "Pick offset distance or enter value.".into() }
    }
    fn vcb_label(&self) -> &'static str {
        "Distance"
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        match &self.op {
            None => {
                let ray = cx.ray();
                self.hover = cx.mesh.pick_face(&ray, 0.0).map(|h| h.face);
            }
            Some(op) => {
                let d = Self::signed_distance(cx, op);
                let op = self.op.as_mut().unwrap();
                op.distance = d;
                Self::preview(op, cx.mesh);
                cx.vcb.value = cx.units.format(d.abs());
            }
        }
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        if self.op.is_some() {
            self.on_move(cx);
            self.commit(cx);
            self.press_started = false;
            return;
        }
        let ray = cx.ray();
        let Some(hit) = cx.mesh.pick_face(&ray, 0.0) else { return };
        self.op = Some(OffsetOp { base: cx.mesh.clone(), face: hit.face, distance: 0.0 });
        self.press_started = true;
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        if dragged && self.press_started {
            self.on_move(cx);
            self.commit(cx);
        }
        self.press_started = false;
    }
    fn on_double_click(&mut self, cx: &mut ToolCx) {
        let Some(d) = self.last_distance else { return };
        let ray = cx.ray();
        let Some(hit) = cx.mesh.pick_face(&ray, 0.0) else { return };
        let op = OffsetOp { base: cx.mesh.clone(), face: hit.face, distance: d };
        Self::preview(&op, cx.mesh);
        cx.checkpoint_with("Offset", op.base);
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let Some(VcbInput::Lengths(v)) = cx.vcb_input(text) else { return Err("Invalid distance".into()) };
        let [d] = v.as_slice() else { return Err("Enter one distance".into()) };
        let Some(op) = &mut self.op else { return Err("Pick a face first".into()) };
        op.distance = if op.distance < 0.0 { -d } else { *d };
        Self::preview(op, cx.mesh);
        self.commit(cx);
        Ok(())
    }
    fn cancel(&mut self, cx: &mut ToolCx) {
        if let Some(op) = self.op.take() {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
        }
    }
    fn draw(&self, _cx: &ToolCx, ov: &mut Overlay) {
        if self.op.is_none() {
            ov.highlight_faces.extend(self.hover);
        }
    }
}

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TapeMeasureTool {
    start: Option<DVec3>,
    /// Direction of the edge the measurement started on (parallel guide line mode).
    edge_dir: Option<DVec3>,
    cur: DVec3,
    no_guides: bool,
}

impl TapeMeasureTool {
    fn finish(&mut self, cx: &mut ToolCx, end: DVec3) {
        let Some(s) = self.start.take() else { return };
        let dir = self.edge_dir.take();
        if self.no_guides {
            return;
        }
        cx.checkpoint("Guide");
        match dir {
            Some(d) => cx.add_guide(Guide::Line { point: end, dir: d }),
            None => {
                if !sc_math::points_eq(s, end) {
                    cx.add_guide(Guide::Point { point: end });
                    cx.add_guide(Guide::Line { point: s, dir: (end - s).normalize() });
                }
            }
        }
    }
}

impl Tool for TapeMeasureTool {
    fn id(&self) -> ToolId {
        ToolId::TapeMeasure
    }
    fn status(&self) -> String {
        if self.start.is_none() {
            "Select start point to measure. Ctrl = toggle create guide.".into()
        } else {
            "Select end point or enter value.".into()
        }
    }
    fn vcb_label(&self) -> &'static str {
        "Length"
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let inf = cx.infer(self.start, None);
        self.cur = inf.point;
        if let Some(s) = self.start {
            // In guide-line mode the measured offset is perpendicular to the edge.
            let dist = match self.edge_dir {
                Some(d) => {
                    let v = inf.point - s;
                    (v - d * v.dot(d)).length()
                }
                None => s.distance(self.cur),
            };
            cx.vcb.value = cx.fmt_len(dist);
        }
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        if self.start.is_none() {
            let inf = cx.infer(None, None);
            self.start = Some(inf.point);
            self.cur = inf.point;
            self.edge_dir = match (inf.kind, inf.edge) {
                (crate::inference::Kind::OnEdge, Some(e)) => {
                    let (a, b) = cx.mesh.edge_points(e);
                    Some((b - a).normalize())
                }
                _ => None,
            };
        } else {
            self.on_move(cx);
            let end = self.cur;
            self.finish(cx, end);
        }
    }
    fn on_copy_toggle(&mut self, _cx: &mut ToolCx) {
        self.no_guides = !self.no_guides;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let Some(VcbInput::Lengths(v)) = cx.vcb_input(text) else { return Err("Invalid length".into()) };
        let [d] = v.as_slice() else { return Err("Enter one length".into()) };
        let s = self.start.ok_or("Pick a start point first")?;
        let dir = self.cur - s;
        if dir.length() < sc_math::TOL {
            return Err("Move the mouse to set a direction".into());
        }
        let end = s + dir.normalize() * *d;
        self.finish(cx, end);
        Ok(())
    }
    fn cancel(&mut self, _cx: &mut ToolCx) {
        self.start = None;
        self.edge_dir = None;
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        if !self.no_guides {
            plus_badge(ov, cx);
        }
        if let Some(s) = self.start {
            if let Some(d) = self.edge_dir {
                // Preview the parallel guide line.
                let big = cx.camera.distance() * 10.0;
                ov.lines.push(line(self.cur - d * big, self.cur + d * big, egui::Color32::from_gray(90), 1.0).dashed(5.0));
            }
            ov.lines.push(line(s, self.cur, axis_color(s, self.cur), 1.5).dashed(6.0));
            tooltip_at(ov, cx, &cx.vcb.value);
        }
    }
    fn start_point(&self) -> Option<DVec3> {
        self.start
    }
}
