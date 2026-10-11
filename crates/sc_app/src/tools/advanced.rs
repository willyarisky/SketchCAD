//! Follow Me and Scale.

use super::modify::transform_selection;
use super::select::{Entity, pick_entity};
use super::*;
use crate::document::Selection;
use sc_geom::VertexId;
use sc_math::units::VcbInput;
use sc_math::{Aabb, DMat4};
use std::collections::HashSet;

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct FollowMeTool {
    hover: Option<FaceId>,
}

impl FollowMeTool {
    /// Path edges: the selected edges, or the outer loops of selected faces (excluding `profile`).
    fn path_edges(cx: &ToolCx, profile: FaceId) -> Vec<EdgeId> {
        if !cx.selection.edges.is_empty() {
            return cx.selection.edges.iter().copied().filter(|e| !cx.mesh.faces[profile].loop_pairs().iter().any(|(a, b)| cx.mesh.edge_between(*a, *b) == Some(*e))).collect();
        }
        let mut out = Vec::new();
        for f in &cx.selection.faces {
            if *f == profile {
                continue;
            }
            if let Some(face) = cx.mesh.faces.get(*f) {
                let l = &face.outer;
                for i in 0..l.len() {
                    if let Some(e) = cx.mesh.edge_between(l[i], l[(i + 1) % l.len()]) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }
}

/// Order a Follow Me path for `profile`: points starting at the end (or, for loops, the vertex)
/// nearest the profile, whether it is closed, and which corners lie inside a curve (smoothed).
pub fn follow_me_path(mesh: &Mesh, profile: FaceId, edges: &[EdgeId]) -> Option<(Vec<DVec3>, bool, Vec<bool>)> {
    let (chain, closed) = mesh.edge_chain(edges)?;
    let mut pts: Vec<DVec3> = chain.iter().map(|v| mesh.pos(*v)).collect();
    let face = &mesh.faces[profile];
    let centroid = mesh.face_points(profile).iter().fold(DVec3::ZERO, |a, b| a + *b) / face.outer.len() as f64;
    let score = |p: &DVec3| face.plane.signed_distance(*p).abs() * 1000.0 + p.distance(centroid);
    let mut ids = chain.clone();
    if closed {
        let start = (0..pts.len()).min_by(|a, b| score(&pts[*a]).total_cmp(&score(&pts[*b]))).unwrap();
        pts.rotate_left(start);
        ids.rotate_left(start);
    } else if score(pts.last().unwrap()) < score(&pts[0]) {
        pts.reverse();
        ids.reverse();
    }
    let n = ids.len();
    let soft = (0..n)
        .map(|i| {
            if !closed && (i == 0 || i == n - 1) {
                return false;
            }
            let (a, b, c) = (ids[(i + n - 1) % n], ids[i], ids[(i + 1) % n]);
            let c1 = mesh.edge_between(a, b).and_then(|e| mesh.edges[e].curve);
            let c2 = mesh.edge_between(b, c).and_then(|e| mesh.edges[e].curve);
            c1.is_some() && c1 == c2
        })
        .collect();
    Some((pts, closed, soft))
}

impl Tool for FollowMeTool {
    fn id(&self) -> ToolId {
        ToolId::FollowMe
    }
    fn status(&self) -> String {
        "Select the path (edges, or a face whose edges form the path), then click the profile face.".into()
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let ray = cx.ray();
        self.hover = cx.mesh.pick_face(&ray, 0.0).map(|h| h.face);
    }
    fn on_release(&mut self, cx: &mut ToolCx, _dragged: bool) {
        let ray = cx.ray();
        let Some(hit) = cx.mesh.pick_face(&ray, 0.0) else { return };
        let profile = hit.face;
        let edges = Self::path_edges(cx, profile);
        let Some((pts, closed, soft)) = follow_me_path(cx.mesh, profile, &edges) else {
            cx.vcb.value = "Select a connected path first".into();
            return;
        };
        cx.checkpoint("Follow Me");
        cx.mesh.follow_me(profile, &pts, closed, &soft);
        cx.selection.clear();
    }
    fn draw(&self, _cx: &ToolCx, ov: &mut Overlay) {
        ov.highlight_faces.extend(self.hover);
    }
}

// ---------------------------------------------------------------------------

struct ScaleOp {
    base: Mesh,
    sel: Selection,
    verts: HashSet<VertexId>,
    grip: DVec3,
    anchor: DVec3,
    /// Which axes this grip scales.
    axes: [bool; 3],
    scale: DVec3,
}

#[derive(Default)]
pub struct ScaleTool {
    op: Option<ScaleOp>,
    hover_grip: Option<usize>,
    press_started: bool,
}

fn grip_index(i: usize, j: usize, k: usize) -> usize {
    i + j * 3 + k * 9
}

impl ScaleTool {
    fn bounds(cx: &ToolCx) -> Aabb {
        let sel = cx.selection.with_bounding_edges(cx.mesh);
        let mut b = Aabb::EMPTY;
        for v in sel.vertices(cx.mesh) {
            b.extend(cx.mesh.pos(v));
        }
        for i in &sel.instances {
            if let Some(inst) = cx.mesh.instances.get(*i) {
                let ib = cx.view.flatten_instance(inst).bounds();
                if !ib.is_empty() {
                    b.extend(ib.min);
                    b.extend(ib.max);
                }
            }
        }
        b
    }

    /// The 26 grips (corners, edge midpoints, face centers), skipping ones along zero-size axes.
    fn grips(b: &Aabb) -> Vec<(usize, DVec3, [usize; 3])> {
        let size = b.max - b.min;
        let flat = [size.x < sc_math::TOL, size.y < sc_math::TOL, size.z < sc_math::TOL];
        let mut out = Vec::new();
        for k in 0..3 {
            for j in 0..3 {
                for i in 0..3 {
                    let ijk = [i, j, k];
                    if ijk == [1, 1, 1] || (0..3).any(|a| flat[a] && ijk[a] != 0) {
                        continue;
                    }
                    let p = b.min + size * DVec3::new(i as f64 / 2.0, j as f64 / 2.0, k as f64 / 2.0);
                    out.push((grip_index(i, j, k), p, ijk));
                }
            }
        }
        out
    }

    fn apply(op: &mut ScaleOp, mesh: &mut Mesh, scale: DVec3) {
        op.scale = scale;
        *mesh = op.base.clone();
        mesh.revision = op.base.revision + 1;
        let m = DMat4::from_translation(op.anchor) * DMat4::from_scale(scale) * DMat4::from_translation(-op.anchor);
        transform_selection(mesh, &op.sel, &op.verts, false, 1, |_| m);
    }

    fn scale_vector(op: &ScaleOp, s: f64) -> DVec3 {
        DVec3::new(if op.axes[0] { s } else { 1.0 }, if op.axes[1] { s } else { 1.0 }, if op.axes[2] { s } else { 1.0 })
    }

    fn commit(&mut self, cx: &mut ToolCx) {
        let Some(op) = self.op.take() else { return };
        if (op.scale - DVec3::ONE).length() < 1e-9 {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
            return;
        }
        cx.checkpoint_with("Scale", op.base);
        cx.selection.prune(cx.mesh);
    }
}

impl Tool for ScaleTool {
    fn id(&self) -> ToolId {
        ToolId::Scale
    }
    fn status(&self) -> String {
        if self.op.is_some() {
            "Pick the scale or enter a value (factor like 2, or a length). Ctrl = about center.".into()
        } else {
            "Select a grip to scale. Ctrl = scale about center.".into()
        }
    }
    fn vcb_label(&self) -> &'static str {
        "Scale"
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn busy(&self) -> bool {
        self.op.is_some()
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        match &mut self.op {
            None => {
                self.hover_grip = None;
                if cx.selection.is_empty() {
                    return;
                }
                let b = Self::bounds(cx);
                let mut best = 9.0;
                for (idx, p, _) in Self::grips(&b) {
                    if let Some(s) = cx.camera.project(p, cx.size) {
                        let d = s.distance(cx.cursor);
                        if d < best {
                            best = d;
                            self.hover_grip = Some(idx);
                        }
                    }
                }
            }
            Some(op) => {
                let ray = cx.camera.ray(cx.cursor, cx.size);
                let dir = op.grip - op.anchor;
                let Some((_, p)) = ray.closest_to_line(op.anchor, dir) else { return };
                let s = (p - op.anchor).dot(dir) / dir.length_squared();
                // Snap to tenths, like SketchUp's scale readout.
                let s = (s * 100.0).round() / 100.0;
                let v = Self::scale_vector(op, s);
                Self::apply(op, cx.mesh, v);
                cx.vcb.value = format!("{s:.2}");
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
        if cx.selection.is_empty() {
            // First click selects what's under the cursor, then grips appear.
            match pick_entity(cx) {
                Some(Entity::Edge(e)) => {
                    cx.selection.edges.insert(e);
                }
                Some(Entity::Face(f)) => {
                    cx.selection.faces.insert(f);
                }
                Some(Entity::Instance(i)) => {
                    cx.selection.instances.insert(i);
                }
                None => {}
            }
            return;
        }
        let Some(idx) = self.hover_grip else { return };
        let b = Self::bounds(cx);
        let Some((_, grip, ijk)) = Self::grips(&b).into_iter().find(|g| g.0 == idx) else { return };
        let ctrl = cx.mods.ctrl || cx.mods.alt || cx.mods.command;
        let size = b.max - b.min;
        let anchor = if ctrl { b.center() } else { b.min + size * DVec3::new((2 - ijk[0]) as f64 / 2.0, (2 - ijk[1]) as f64 / 2.0, (2 - ijk[2]) as f64 / 2.0) };
        let sel = cx.selection.with_bounding_edges(cx.mesh);
        let verts = sel.vertices(cx.mesh);
        let axes = [ijk[0] != 1, ijk[1] != 1, ijk[2] != 1];
        self.op = Some(ScaleOp { base: cx.mesh.clone(), sel, verts, grip, anchor, axes, scale: DVec3::ONE });
        self.press_started = true;
    }
    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        if dragged && self.press_started {
            self.on_move(cx);
            self.commit(cx);
        }
        self.press_started = false;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let Some(op) = &mut self.op else { return Err("Pick a grip first".into()) };
        let t = text.trim();
        let s = match t.parse::<f64>() {
            Ok(f) => f,
            Err(_) => match cx.vcb_input(t) {
                Some(VcbInput::Lengths(v)) if v.len() == 1 => {
                    let cur = (op.grip - op.anchor).length();
                    if cur < sc_math::TOL {
                        return Err("Cannot scale a zero-size box".into());
                    }
                    v[0] / cur
                }
                _ => return Err("Enter a scale factor (e.g. 2) or a length".into()),
            },
        };
        if s.abs() < 1e-6 {
            return Err("Scale factor cannot be zero".into());
        }
        let v = Self::scale_vector(op, s);
        Self::apply(op, cx.mesh, v);
        self.commit(cx);
        Ok(())
    }
    fn cancel(&mut self, cx: &mut ToolCx) {
        if let Some(op) = self.op.take() {
            *cx.mesh = op.base;
            cx.mesh.revision += 1;
        }
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        if cx.selection.is_empty() {
            return;
        }
        let b = Self::bounds(cx);
        if b.is_empty() {
            return;
        }
        // Yellow bounding box with green grips (red when hovered), like SketchUp.
        let c = |i: usize| DVec3::new(if i & 1 == 0 { b.min.x } else { b.max.x }, if i & 2 == 0 { b.min.y } else { b.max.y }, if i & 4 == 0 { b.min.z } else { b.max.z });
        for (a, bb) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
            ov.lines.push(line(c(a), c(bb), egui::Color32::from_rgb(220, 180, 0), 1.2));
        }
        if self.op.is_some() {
            return;
        }
        for (idx, p, _) in Self::grips(&b) {
            if let Some(s) = cx.camera.project(p, cx.size) {
                let pos = egui::pos2(s.x as f32, s.y as f32);
                let fill = if Some(idx) == self.hover_grip { egui::Color32::from_rgb(230, 30, 30) } else { egui::Color32::from_rgb(40, 170, 40) };
                ov.shapes.push(egui::Shape::rect_filled(egui::Rect::from_center_size(pos, egui::vec2(7.0, 7.0)), 1.0, fill));
            }
        }
    }
}
