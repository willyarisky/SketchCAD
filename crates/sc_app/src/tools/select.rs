//! Select, Eraser and Paint Bucket.

use super::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    Edge(EdgeId),
    Face(FaceId),
    /// A group or component in the active context.
    Instance(sc_geom::InstanceId),
}

const PICK_PX: f64 = 5.0;

/// Screen distance from the cursor to an edge, if within the pick tolerance; plus its ray depth.
fn edge_hit(cx: &ToolCx, ray: &sc_math::Ray, a: DVec3, b: DVec3) -> Option<(f64, f64)> {
    let (sa, sb) = (cx.camera.project(a, cx.size)?, cx.camera.project(b, cx.size)?);
    let ab = sb - sa;
    let t = if ab.length_squared() > 0.0 { ((cx.cursor - sa).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    let d = cx.cursor.distance(sa + ab * t);
    if d > PICK_PX {
        return None;
    }
    let (te, _, _) = ray.closest_to_segment(a, b);
    Some((d, te))
}

/// Entity under the cursor in the active context. The nearest thing along the view ray wins;
/// at equal depth an edge beats the face it bounds (SketchUp's behavior).
pub fn pick_entity(cx: &ToolCx) -> Option<Entity> {
    let mesh = &*cx.mesh;
    let ray = cx.ray();
    // (depth, priority: 0 = edge / 1 = face, entity)
    let mut cands: Vec<(f64, u8, Entity)> = Vec::new();
    if let Some(h) = mesh.pick_face(&ray, 0.0) {
        cands.push((h.t, 1, Entity::Face(h.face)));
    }
    let mut best_edge: Option<(f64, f64, EdgeId)> = None;
    for (eid, e) in &mesh.edges {
        if e.hidden {
            continue;
        }
        let (a, b) = (mesh.pos(e.v[0]), mesh.pos(e.v[1]));
        if let Some((d, te)) = edge_hit(cx, &ray, a, b)
            && best_edge.is_none_or(|(bd, _, _)| d < bd) {
                best_edge = Some((d, te, eid));
            }
    }
    if let Some((_, te, e)) = best_edge {
        cands.push((te, 0, Entity::Edge(e)));
    }
    for (id, inst) in &mesh.instances {
        if !cx.view.instance_visible(inst) {
            continue;
        }
        let flat = cx.view.flatten_instance(inst);
        if let Some(h) = flat.pick_face(&ray, 0.0) {
            cands.push((h.t, 1, Entity::Instance(id)));
        }
        let near_edge = flat.edges.values().filter(|e| !e.hidden).filter_map(|e| edge_hit(cx, &ray, flat.pos(e.v[0]), flat.pos(e.v[1]))).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, te)) = near_edge {
            cands.push((te, 0, Entity::Instance(id)));
        }
    }
    let nearest = cands.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
    let eps = (nearest.abs() * 1e-4).max(0.01);
    cands.into_iter().filter(|c| c.0 <= nearest + eps).min_by_key(|c| c.1).map(|c| c.2)
}

#[derive(Default)]
pub struct SelectTool {
    press: Option<DVec2>,
    clicks: u32,
    last_click: Option<Instant>,
    last_entity: Option<Entity>,
}

fn toggle<T: std::hash::Hash + Eq + Copy>(set: &mut HashSet<T>, v: T, mods: egui::Modifiers) {
    let ctrl = mods.ctrl || mods.command;
    if ctrl && mods.shift {
        set.remove(&v);
    } else if mods.shift {
        if !set.remove(&v) {
            set.insert(v);
        }
    } else {
        set.insert(v);
    }
}

impl SelectTool {
    fn select_connected(cx: &mut ToolCx, start: Entity) {
        let mesh = &cx.mesh;
        let mut verts = HashSet::new();
        let mut stack = Vec::new();
        match start {
            Entity::Edge(e) => stack.extend(mesh.edges[e].v),
            Entity::Face(f) => stack.extend(mesh.faces[f].vertex_ids()),
            Entity::Instance(i) => {
                cx.selection.instances.insert(i);
                return;
            }
        }
        while let Some(v) = stack.pop() {
            if !verts.insert(v) {
                continue;
            }
            for e in mesh.vertex_edges(v) {
                stack.push(mesh.edges[e].other(v));
            }
        }
        let sel = &mut cx.selection;
        for (eid, e) in &mesh.edges {
            if verts.contains(&e.v[0]) {
                sel.edges.insert(eid);
            }
        }
        for (fid, f) in &mesh.faces {
            if verts.contains(&f.outer[0]) {
                sel.faces.insert(fid);
            }
        }
    }

    fn box_select(&self, cx: &mut ToolCx, a: DVec2, b: DVec2) {
        let (min, max) = (a.min(b), a.max(b));
        let crossing = b.x < a.x;
        let inside = |p: DVec2| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y;
        let mesh = &cx.mesh;
        let cam = &cx.camera;
        let proj = |v: sc_geom::VertexId| cam.project(mesh.pos(v), cx.size);
        let seg_hits = |p: DVec2, q: DVec2| -> bool {
            if inside(p) || inside(q) {
                return true;
            }
            // Segment vs rectangle edges.
            let corners = [min, DVec2::new(max.x, min.y), max, DVec2::new(min.x, max.y)];
            (0..4).any(|i| seg_cross(p, q, corners[i], corners[(i + 1) % 4]))
        };
        let ctrl_or_shift = cx.mods.ctrl || cx.mods.command || cx.mods.shift;
        if !ctrl_or_shift {
            cx.selection.clear();
        }
        let mods = cx.mods;
        let mut new_edges = Vec::new();
        for (eid, e) in &mesh.edges {
            let (Some(p), Some(q)) = (proj(e.v[0]), proj(e.v[1])) else { continue };
            let hit = if crossing { seg_hits(p, q) } else { inside(p) && inside(q) };
            if hit && !e.hidden {
                new_edges.push(eid);
            }
        }
        let mut new_faces = Vec::new();
        for (fid, f) in &mesh.faces {
            let pts: Option<Vec<DVec2>> = f.outer.iter().map(|v| proj(*v)).collect();
            let Some(pts) = pts else { continue };
            let hit = if crossing {
                (0..pts.len()).any(|i| seg_hits(pts[i], pts[(i + 1) % pts.len()]))
                    || sc_math::geom2d::point_in_polygon((min + max) * 0.5, &pts)
            } else {
                pts.iter().all(|p| inside(*p))
            };
            if hit {
                new_faces.push(fid);
            }
        }
        for e in new_edges {
            toggle(&mut cx.selection.edges, e, mods);
        }
        for f in new_faces {
            toggle(&mut cx.selection.faces, f, mods);
        }
    }
}

fn seg_cross(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let o = |p: DVec2, q: DVec2, r: DVec2| (q - p).perp_dot(r - p);
    let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

impl Tool for SelectTool {
    fn id(&self) -> ToolId {
        ToolId::Select
    }
    fn status(&self) -> String {
        "Select objects. Shift to extend select. Drag mouse to select multiple.".into()
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn cursor(&self) -> egui::CursorIcon {
        egui::CursorIcon::Default
    }

    fn on_press(&mut self, cx: &mut ToolCx) {
        self.press = Some(cx.cursor);
    }

    fn on_release(&mut self, cx: &mut ToolCx, dragged: bool) {
        let Some(start) = self.press.take() else { return };
        if dragged && start.distance(cx.cursor) > 3.0 {
            self.box_select(cx, start, cx.cursor);
            return;
        }
        let ent = pick_entity(cx);
        let now = Instant::now();
        let quick = self.last_click.is_some_and(|t| now - t < Duration::from_millis(450)) && ent == self.last_entity;
        self.clicks = if quick { self.clicks + 1 } else { 1 };
        self.last_click = Some(now);
        self.last_entity = ent;

        let mods = cx.mods;
        let additive = mods.ctrl || mods.command || mods.shift;
        match (self.clicks, ent) {
            (1, Some(e)) => {
                if !additive {
                    cx.selection.clear();
                }
                match e {
                    Entity::Edge(e) => toggle(&mut cx.selection.edges, e, mods),
                    Entity::Face(f) => toggle(&mut cx.selection.faces, f, mods),
                    Entity::Instance(i) => toggle(&mut cx.selection.instances, i, mods),
                }
            }
            // Double-clicking a group or component opens it for editing.
            (2, Some(Entity::Instance(i))) => *cx.request = Some(ToolRequest::Enter(i)),
            (2, Some(e)) => {
                // Double click: face + bounding edges, or edge + adjacent faces.
                let mesh = &cx.mesh;
                match e {
                    Entity::Face(f) => {
                        cx.selection.faces.insert(f);
                        for (a, b) in mesh.faces[f].loop_pairs() {
                            if let Some(e) = mesh.edge_between(a, b) {
                                cx.selection.edges.insert(e);
                            }
                        }
                    }
                    Entity::Edge(e) => {
                        cx.selection.edges.insert(e);
                        cx.selection.faces.extend(mesh.edges[e].faces.iter().copied());
                    }
                    Entity::Instance(_) => {}
                }
            }
            (n, Some(e)) if n >= 3 => Self::select_connected(cx, e),
            (_, None) if !additive => {
                // Clicking empty space while editing a group closes it, like SketchUp.
                if cx.ctx.is_some() && cx.selection.is_empty() {
                    *cx.request = Some(ToolRequest::Exit);
                }
                cx.selection.clear();
            }
            _ => {}
        }
    }

    fn cancel(&mut self, cx: &mut ToolCx) {
        self.press = None;
        cx.selection.clear();
    }
    fn deactivate(&mut self, _cx: &mut ToolCx) {
        self.press = None;
    }

    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        let Some(start) = self.press else { return };
        if start.distance(cx.cursor) < 3.0 {
            return;
        }
        let r = egui::Rect::from_two_pos(egui::pos2(start.x as f32, start.y as f32), egui::pos2(cx.cursor.x as f32, cx.cursor.y as f32));
        let stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
        if cx.cursor.x < start.x {
            // Crossing selection: dashed outline.
            let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            ov.shapes.extend(egui::Shape::dashed_line(&c, stroke, 4.0, 3.0));
        } else {
            ov.shapes.push(egui::Shape::rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside));
        }
    }
}

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct EraserTool {
    active: bool,
    marked: Vec<EdgeId>,
    marked_instances: Vec<sc_geom::InstanceId>,
}

impl EraserTool {
    fn mark_under_cursor(&mut self, cx: &ToolCx) {
        match pick_entity(cx) {
            Some(Entity::Edge(e)) if !self.marked.contains(&e) => self.marked.push(e),
            Some(Entity::Instance(i)) if !self.marked_instances.contains(&i) => self.marked_instances.push(i),
            _ => {}
        }
    }
}

impl Tool for EraserTool {
    fn id(&self) -> ToolId {
        ToolId::Eraser
    }
    fn status(&self) -> String {
        "Select objects to delete, or drag over multiple objects. Shift = Hide, Ctrl = Soften/Smooth, Ctrl+Shift = Unsoften.".into()
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        self.active = true;
        self.marked.clear();
        self.marked_instances.clear();
        self.mark_under_cursor(cx);
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        if self.active {
            self.mark_under_cursor(cx);
        }
    }
    fn on_release(&mut self, cx: &mut ToolCx, _dragged: bool) {
        self.active = false;
        let edges = std::mem::take(&mut self.marked);
        let insts = std::mem::take(&mut self.marked_instances);
        if edges.is_empty() && insts.is_empty() {
            return;
        }
        let ctrl = cx.mods.ctrl || cx.mods.command;
        if !insts.is_empty() && !ctrl {
            cx.checkpoint(if cx.mods.shift { "Hide" } else { "Erase" });
            for i in insts {
                if cx.mods.shift {
                    cx.mesh.instances[i].hidden = true;
                } else {
                    cx.mesh.instances.remove(i);
                }
            }
            cx.mesh.revision += 1;
            if edges.is_empty() {
                cx.selection.prune(cx.mesh);
                return;
            }
        }
        if ctrl {
            cx.checkpoint(if cx.mods.shift { "Unsoften" } else { "Soften" });
            for e in edges {
                let ed = &mut cx.mesh.edges[e];
                ed.soft = !cx.mods.shift;
                ed.smooth = !cx.mods.shift;
            }
            cx.mesh.revision += 1;
        } else if cx.mods.shift {
            cx.checkpoint("Hide");
            for e in edges {
                cx.mesh.edges[e].hidden = true;
            }
            cx.mesh.revision += 1;
        } else {
            cx.checkpoint("Erase");
            cx.mesh.erase_edges(&edges);
        }
        cx.selection.prune(cx.mesh);
    }
    fn cancel(&mut self, _cx: &mut ToolCx) {
        self.active = false;
        self.marked.clear();
        self.marked_instances.clear();
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        ov.highlight_edges.extend(self.marked.iter().copied());
        ov.highlight_instances.extend(self.marked_instances.iter().copied());
        // Eraser cursor hint: small square.
        let p = egui::pos2(cx.cursor.x as f32, cx.cursor.y as f32);
        ov.shapes.push(egui::Shape::rect_stroke(
            egui::Rect::from_center_size(p + egui::vec2(8.0, 8.0), egui::vec2(8.0, 8.0)),
            1.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 90, 140)),
            egui::StrokeKind::Inside,
        ));
    }
    fn cursor(&self) -> egui::CursorIcon {
        egui::CursorIcon::Default
    }
}

// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct PaintBucketTool;

impl Tool for PaintBucketTool {
    fn id(&self) -> ToolId {
        ToolId::PaintBucket
    }
    fn status(&self) -> String {
        "Select object to paint. Alt = Sample material.".into()
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn on_release(&mut self, cx: &mut ToolCx, _dragged: bool) {
        let ray = cx.ray();
        let Some(hit) = cx.mesh.pick_face(&ray, 0.0) else { return };
        let face = &cx.mesh.faces[hit.face];
        let back_side = face.plane.normal.dot(ray.dir) > 0.0;
        if cx.mods.alt {
            let c = if back_side { face.back } else { face.front };
            *cx.material = c.unwrap_or(if back_side { sc_render::scene::DEFAULT_BACK } else { sc_render::scene::DEFAULT_FRONT });
            return;
        }
        let targets: Vec<FaceId> = if cx.selection.faces.contains(&hit.face) { cx.selection.faces.iter().copied().collect() } else { vec![hit.face] };
        cx.checkpoint("Paint");
        let m = *cx.material;
        for f in targets {
            let face = &mut cx.mesh.faces[f];
            if back_side {
                face.back = Some(m);
            } else {
                face.front = Some(m);
            }
        }
        cx.mesh.revision += 1;
    }
    fn draw(&self, cx: &ToolCx, ov: &mut Overlay) {
        let p = egui::pos2(cx.cursor.x as f32, cx.cursor.y as f32) + egui::vec2(10.0, 10.0);
        let c = *cx.material;
        ov.shapes.push(egui::Shape::rect_filled(
            egui::Rect::from_center_size(p, egui::vec2(10.0, 10.0)),
            2.0,
            egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]),
        ));
    }
}
