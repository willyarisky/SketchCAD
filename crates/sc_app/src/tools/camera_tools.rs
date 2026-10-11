//! Orbit, Pan and Zoom as left-button tools (the middle button / wheel work in every tool).

use super::*;

/// World-space point under the cursor used as orbit pivot / zoom anchor: the nearest face in any
/// context, else the ground, else the camera target.
pub fn anchor_point(cx: &ToolCx) -> DVec3 {
    let ray = cx.ray();
    let hits = [cx.mesh.pick_face(&ray, 0.0), cx.extra.pick_face(&ray, 0.0)];
    if let Some(hit) = hits.into_iter().flatten().min_by(|a, b| a.t.total_cmp(&b.t)) {
        return cx.to_world.transform_point3(hit.point);
    }
    let cam = &*cx.world_camera;
    let wray = cam.ray(cx.cursor, cx.size);
    let ground = Plane::new(DVec3::ZERO, DVec3::Z);
    match ground.intersect_ray(&wray) {
        Some(t) if t > 0.0 && t < cam.distance() * 20.0 => wray.at(t),
        _ => cam.target,
    }
}

#[derive(Default)]
pub struct OrbitTool {
    pivot: Option<DVec3>,
}

impl Tool for OrbitTool {
    fn id(&self) -> ToolId {
        ToolId::Orbit
    }
    fn status(&self) -> String {
        "Drag in direction to orbit. Shift = Pan.".into()
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn cursor(&self) -> egui::CursorIcon {
        if self.pivot.is_some() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab }
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        self.pivot = Some(anchor_point(cx));
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let Some(pivot) = self.pivot else { return };
        if cx.mods.shift {
            cx.world_camera.pan(cx.delta.x, cx.delta.y, pivot, cx.size.y);
        } else {
            cx.world_camera.orbit(cx.delta.x, cx.delta.y, pivot);
        }
    }
    fn on_release(&mut self, _cx: &mut ToolCx, _dragged: bool) {
        self.pivot = None;
    }
}

#[derive(Default)]
pub struct PanTool {
    anchor: Option<DVec3>,
}

impl Tool for PanTool {
    fn id(&self) -> ToolId {
        ToolId::Pan
    }
    fn status(&self) -> String {
        "Drag in direction to pan.".into()
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn cursor(&self) -> egui::CursorIcon {
        egui::CursorIcon::Move
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        self.anchor = Some(anchor_point(cx));
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        if let Some(a) = self.anchor {
            cx.world_camera.pan(cx.delta.x, cx.delta.y, a, cx.size.y);
        }
    }
    fn on_release(&mut self, _cx: &mut ToolCx, _dragged: bool) {
        self.anchor = None;
    }
}

#[derive(Default)]
pub struct ZoomTool {
    anchor: Option<DVec3>,
}

impl Tool for ZoomTool {
    fn id(&self) -> ToolId {
        ToolId::Zoom
    }
    fn status(&self) -> String {
        "Drag up or down to zoom. Shift = change field of view.".into()
    }
    fn vcb_label(&self) -> &'static str {
        "Field of View"
    }
    fn uses_inference(&self) -> bool {
        false
    }
    fn cursor(&self) -> egui::CursorIcon {
        egui::CursorIcon::ZoomIn
    }
    fn on_press(&mut self, cx: &mut ToolCx) {
        self.anchor = Some(anchor_point(cx));
        cx.vcb.value = format!("{:.2} deg", cx.world_camera.fov_y);
    }
    fn on_move(&mut self, cx: &mut ToolCx) {
        let Some(a) = self.anchor else { return };
        let cam = &mut *cx.world_camera;
        if cx.mods.shift {
            cam.fov_y = (cam.fov_y + cx.delta.y * 0.2).clamp(1.0, 120.0);
        } else {
            cam.zoom(-cx.delta.y * 0.05, a);
        }
        cx.vcb.value = format!("{:.2} deg", cam.fov_y);
    }
    fn on_release(&mut self, _cx: &mut ToolCx, _dragged: bool) {
        self.anchor = None;
    }
    fn on_vcb(&mut self, cx: &mut ToolCx, text: &str) -> Result<(), String> {
        let v: f64 = text.trim().trim_end_matches("deg").trim().parse().map_err(|_| "Invalid field of view".to_string())?;
        if !(1.0..=120.0).contains(&v) {
            return Err("Field of view must be between 1 and 120 degrees".into());
        }
        cx.world_camera.fov_y = v;
        cx.vcb.value = format!("{v:.2} deg");
        Ok(())
    }
}
