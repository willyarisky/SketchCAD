//! Scenes: saved views with animated transitions.

use crate::app::SketchCadApp;
use crate::document::Scene;
use sc_render::Camera;
use std::time::Instant;

/// Seconds a scene transition takes (SketchUp's default is 2 s; snappier here).
const TRANSITION: f32 = 0.8;

pub struct CameraAnimation {
    from: Camera,
    to: Camera,
    start: Instant,
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl SketchCadApp {
    pub fn add_scene(&mut self) {
        let n = self.doc.model.scenes.len() + 1;
        let name = (n..).map(|k| format!("Scene {k}")).find(|nm| !self.doc.model.scenes.iter().any(|s| &s.name == nm)).unwrap();
        let scene = self.capture_scene(name);
        self.doc.model.scenes.push(scene);
        self.active_scene = Some(self.doc.model.scenes.len() - 1);
        self.doc.dirty = true;
    }

    fn capture_scene(&self, name: String) -> Scene {
        let m = &self.doc.model;
        Scene {
            name,
            camera: m.camera,
            style: m.style.clone(),
            hidden_tags: m.tags.iter().enumerate().filter(|(_, t)| !t.visible).map(|(i, _)| i as u32).collect(),
        }
    }

    pub fn update_scene(&mut self, i: usize) {
        if let Some(s) = self.doc.model.scenes.get(i) {
            let name = s.name.clone();
            self.doc.model.scenes[i] = self.capture_scene(name);
            self.doc.dirty = true;
        }
    }

    pub fn delete_scene(&mut self, i: usize) {
        if i < self.doc.model.scenes.len() {
            self.doc.model.scenes.remove(i);
            self.active_scene = None;
            self.doc.dirty = true;
        }
    }

    /// Switch to a scene: style and tags apply at once, the camera animates.
    pub fn go_to_scene(&mut self, i: usize) {
        let Some(scene) = self.doc.model.scenes.get(i).cloned() else { return };
        let m = &mut self.doc.model;
        m.style = scene.style;
        for (k, t) in m.tags.iter_mut().enumerate() {
            t.visible = !scene.hidden_tags.contains(&(k as u32));
        }
        self.camera_anim = Some(CameraAnimation { from: m.camera, to: scene.camera, start: Instant::now() });
        self.active_scene = Some(i);
    }

    pub fn step_scene(&mut self, delta: isize) {
        let n = self.doc.model.scenes.len() as isize;
        if n == 0 {
            return;
        }
        let cur = self.active_scene.map_or(-1, |i| i as isize);
        let next = (cur + delta).rem_euclid(n) as usize;
        self.go_to_scene(next);
    }

    /// Advance a running camera transition. Any orbit/pan/zoom by the user cancels it.
    pub fn animate_camera(&mut self, ctx: &egui::Context) {
        let Some(anim) = &self.camera_anim else { return };
        let t = smoothstep((anim.start.elapsed().as_secs_f32() / TRANSITION) as f64);
        let (a, b) = (anim.from, anim.to);
        let cam = &mut self.doc.model.camera;
        cam.eye = a.eye.lerp(b.eye, t);
        cam.target = a.target.lerp(b.target, t);
        cam.fov_y = a.fov_y + (b.fov_y - a.fov_y) * t;
        cam.perspective = if t < 0.5 { a.perspective } else { b.perspective };
        if t >= 1.0 {
            *cam = b;
            self.camera_anim = None;
        } else {
            ctx.request_repaint();
        }
    }

    pub fn scenes_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Add Scene").on_hover_text("Save the current view as a scene").clicked() {
                self.add_scene();
            }
            let has = self.active_scene.is_some_and(|i| i < self.doc.model.scenes.len());
            if ui.add_enabled(has, egui::Button::new("Update")).on_hover_text("Replace the selected scene with the current view").clicked() {
                self.update_scene(self.active_scene.unwrap());
            }
            if ui.add_enabled(has, egui::Button::new("Delete")).clicked() {
                self.delete_scene(self.active_scene.unwrap());
            }
        });
        let mut go = None;
        let mut swap = None;
        let n = self.doc.model.scenes.len();
        for i in 0..n {
            ui.horizontal(|ui| {
                let selected = self.active_scene == Some(i);
                let name = &mut self.doc.model.scenes[i].name;
                if ui.add(egui::Button::new("Go").small().selected(selected)).on_hover_text("Go to scene").clicked() {
                    go = Some(i);
                }
                if ui.add(egui::TextEdit::singleline(name).desired_width(120.0)).changed() {
                    self.doc.dirty = true;
                }
                if ui.add_enabled(i > 0, egui::Button::new("Up").small()).clicked() {
                    swap = Some((i, i - 1));
                }
                if ui.add_enabled(i + 1 < n, egui::Button::new("Down").small()).clicked() {
                    swap = Some((i, i + 1));
                }
            });
        }
        if let Some((a, b)) = swap {
            self.doc.model.scenes.swap(a, b);
            if self.active_scene == Some(a) {
                self.active_scene = Some(b);
            }
            self.doc.dirty = true;
        }
        if let Some(i) = go {
            self.go_to_scene(i);
        }
        if n == 0 {
            ui.label(egui::RichText::new("No scenes yet.").weak());
        }
    }

    /// Scene tabs across the top of the viewport.
    pub fn scene_tabs(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if self.doc.model.scenes.is_empty() {
            return;
        }
        let mut go = None;
        let area = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 4.0), egui::vec2(rect.width() - 12.0, 24.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (i, s) in self.doc.model.scenes.iter().enumerate() {
                    let selected = self.active_scene == Some(i);
                    let fill = if selected { egui::Color32::from_rgb(250, 250, 250) } else { egui::Color32::from_rgba_unmultiplied(225, 225, 228, 230) };
                    let btn = egui::Button::new(egui::RichText::new(&s.name).size(12.0)).fill(fill).corner_radius(egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 });
                    if ui.add(btn).clicked() {
                        go = Some(i);
                    }
                }
            });
        });
        if let Some(i) = go {
            self.go_to_scene(i);
        }
    }
}
