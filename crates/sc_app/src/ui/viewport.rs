//! The 3D viewport: routes input to tools and the camera, renders the model and tool feedback.

use crate::app::SketchCadApp;
use crate::document::Guide;
use crate::inference::{Axis, Kind};
use crate::tools::{Overlay, line};
use egui::{Color32, Pos2, Sense, Stroke, pos2, vec2};
use sc_math::{DVec2, DVec3};
use sc_render::{Camera, LineInstance, RenderScene, SceneOptions};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub struct ViewportState {
    /// Viewport rectangle in UI points (screen space).
    pub rect: egui::Rect,
    pub size: DVec2,
    pub cursor: DVec2,
    press: Option<DVec2>,
    dragged: bool,
    middle: Option<DVec3>,
    last_cursor: Option<DVec2>,
    last_camera: Option<Camera>,
    last_mods: egui::Modifiers,
    cache_key: u64,
    cache: Option<RenderScene>,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            rect: egui::Rect::NOTHING,
            size: DVec2::ONE,
            cursor: DVec2::ZERO,
            press: None,
            dragged: false,
            middle: None,
            last_cursor: None,
            last_camera: None,
            last_mods: Default::default(),
            cache_key: 0,
            cache: None,
        }
    }
}

const VCB_START: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', ',', '-', '[', '<', '/', '*', '\'', '"', '~'];

impl SketchCadApp {
    pub fn viewport_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            let resp = ui.allocate_rect(rect, Sense::click_and_drag());
            self.viewport.rect = rect;
            self.viewport.size = DVec2::new(rect.width() as f64, rect.height() as f64).max(DVec2::ONE);
            let ctx = ui.ctx().clone();

            let before = (self.doc.revision_key(), self.doc.selection.clone(), self.tool.id());
            self.handle_pointer(&ctx, rect, &resp);
            self.handle_keyboard(&ctx);
            // Panels drawn earlier this frame (tray, toolbars) must reflect what the tool just changed.
            if before != (self.doc.revision_key(), self.doc.selection.clone(), self.tool.id()) {
                ctx.request_repaint();
            }

            // --- Render --------------------------------------------------
            let mut ov = Overlay::default();
            self.with_tool_cx(|t, cx| t.draw(cx, &mut ov));
            if self.tool.uses_inference() && resp.hovered() {
                self.draw_inference(&mut ov);
            }
            // Tools and inferences work in the active context's local space.
            let to_world = self.doc.active_xform();
            if to_world != sc_math::DMat4::IDENTITY {
                let xf = to_world.as_mat4();
                for l in &mut ov.lines {
                    l.a = xf.transform_point3(l.a.into()).to_array();
                    l.b = xf.transform_point3(l.b.into()).to_array();
                }
            }
            if self.doc.model.style.show_guides {
                for g in &self.doc.model.guides {
                    if let Guide::Point { point } = g
                        && let Some(s) = self.doc.model.camera.project(*point, self.viewport.size) {
                            let p = pos2(s.x as f32, s.y as f32);
                            let st = Stroke::new(1.2, Color32::from_gray(60));
                            ov.shapes.push(egui::Shape::line_segment([p - vec2(4.0, 4.0), p + vec2(4.0, 4.0)], st));
                            ov.shapes.push(egui::Shape::line_segment([p - vec2(4.0, -4.0), p + vec2(4.0, -4.0)], st));
                        }
                }
            }

            let ppp = ctx.pixels_per_point();
            let size_px = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
            let mut scene = self.build_scene(&ov);
            scene.overlay = std::mem::take(&mut ov.lines);
            if let (Some(renderer), Some(rs)) = (self.renderer.as_mut(), frame.wgpu_render_state()) {
                let tex = renderer.render(rs, size_px, ppp, &self.doc.model.camera, &scene);
                ui.painter().image(tex, rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "GPU renderer unavailable", egui::FontId::proportional(16.0), Color32::RED);
            }
            // Put the static parts back in the cache (overlay is per-frame).
            scene.overlay.clear();
            self.viewport.cache = Some(scene);

            // Screen-space overlay.
            let painter = ui.painter().with_clip_rect(rect);
            let offset = rect.min.to_vec2();
            for mut s in ov.shapes {
                s.translate(offset);
                painter.add(s);
            }
            for (p, t) in &ov.tooltips {
                super::paint_tooltip(&painter, *p + offset, t);
            }
            if resp.hovered() {
                ctx.set_cursor_icon(self.tool.cursor());
            }
            // Right-click: select what's under the cursor (unless it's already selected) and show
            // the context menu.
            if resp.secondary_clicked() {
                self.with_tool_cx(|_, cx| {
                    if let Some(ent) = crate::tools::pick_entity(cx) {
                        use crate::tools::Entity;
                        let s = &cx.selection;
                        let already = match ent {
                            Entity::Edge(e) => s.edges.contains(&e),
                            Entity::Face(f) => s.faces.contains(&f),
                            Entity::Instance(i) => s.instances.contains(&i),
                        };
                        if !already {
                            cx.selection.clear();
                            match ent {
                                Entity::Edge(e) => cx.selection.edges.insert(e),
                                Entity::Face(f) => cx.selection.faces.insert(f),
                                Entity::Instance(i) => cx.selection.instances.insert(i),
                            };
                        }
                    }
                });
            }
            resp.context_menu(|ui| self.context_menu(ui));
            self.scene_tabs(ui, rect);
        });
    }

    fn handle_pointer(&mut self, ctx: &egui::Context, rect: egui::Rect, resp: &egui::Response) {
        let (hover, pos, delta, scroll, mods, primary_pressed, primary_released, double, middle_down, middle_pressed) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.latest_pos(),
                i.pointer.delta(),
                i.smooth_scroll_delta.y,
                i.modifiers,
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
                i.pointer.button_down(egui::PointerButton::Middle),
                i.pointer.button_pressed(egui::PointerButton::Middle),
            )
        });
        let inside = hover.is_some_and(|p| rect.contains(p)) && resp.hovered();
        let local = |p: Pos2| DVec2::new((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);
        if let Some(p) = pos
            && (inside || self.viewport.press.is_some() || self.viewport.middle.is_some()) {
                self.viewport.cursor = local(p);
            }
        self.track_modifiers(mods);
        let d = DVec2::new(delta.x as f64, delta.y as f64);

        // Scroll wheel: zoom toward the cursor.
        // Any direct camera input cancels a running scene transition.
        let camera_tool = matches!(self.tool.id(), crate::tools::ToolId::Orbit | crate::tools::ToolId::Pan | crate::tools::ToolId::Zoom);
        if inside && (scroll.abs() > 0.0 || middle_pressed || (primary_pressed && camera_tool)) {
            self.camera_anim = None;
        }
        if inside && scroll.abs() > 0.0 {
            let anchor = self.with_tool_cx(|_, cx| crate::tools::camera_anchor(cx));
            self.doc.model.camera.zoom(scroll as f64 / 60.0, anchor);
        }
        // Middle mouse: orbit, Shift+middle: pan (works inside every tool).
        if middle_pressed && inside {
            self.viewport.middle = Some(self.with_tool_cx(|_, cx| crate::tools::camera_anchor(cx)));
        }
        if !middle_down {
            self.viewport.middle = None;
        }
        if let Some(pivot) = self.viewport.middle
            && d.length() > 0.0 {
                if mods.shift {
                    self.doc.model.camera.pan(d.x, d.y, pivot, self.viewport.size.y);
                } else {
                    self.doc.model.camera.orbit(d.x, d.y, pivot);
                }
            }

        // Primary button drives the active tool.
        if primary_pressed && inside {
            self.viewport.press = Some(self.viewport.cursor);
            self.viewport.dragged = false;
            // Clicking the viewport takes keyboard focus away from text fields.
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            self.with_tool_cx(|t, cx| t.on_press(cx));
        }
        if let Some(p) = self.viewport.press
            && p.distance(self.viewport.cursor) > 4.0 {
                self.viewport.dragged = true;
            }
        let moved = self.viewport.last_cursor != Some(self.viewport.cursor)
            || self.viewport.last_camera != Some(self.doc.model.camera)
            || self.viewport.last_mods != mods;
        if (inside || self.viewport.press.is_some()) && moved && self.viewport.middle.is_none() {
            let cursor = self.viewport.cursor;
            let mut tool_delta = d;
            if self.viewport.last_cursor.is_none() {
                tool_delta = DVec2::ZERO;
            }
            self.with_tool_cx(|t, cx| {
                cx.cursor = cursor;
                cx.delta = tool_delta;
                t.on_move(cx);
            });
        }
        if primary_released && self.viewport.press.is_some() {
            let dragged = self.viewport.dragged;
            self.viewport.press = None;
            self.with_tool_cx(|t, cx| t.on_release(cx, dragged));
        }
        if double && inside {
            self.with_tool_cx(|t, cx| t.on_double_click(cx));
        }
        self.viewport.last_cursor = Some(self.viewport.cursor);
        self.viewport.last_camera = Some(self.doc.model.camera);
        self.viewport.last_mods = mods;
    }

    fn track_modifiers(&mut self, mods: egui::Modifiers) {
        let prev = self.prev_mods;
        self.prev_mods = mods;
        // Shift locks the current inference while held.
        if mods.shift && !prev.shift {
            let from = self.tool.start_point();
            if self.inference.lock.is_none() {
                self.inference.lock_current(from);
            }
        } else if !mods.shift && prev.shift && !self.arrow_locked {
            // Shift locks end on release; arrow-key locks persist until toggled or Esc.
            self.inference.unlock();
        }
        // Tapping the copy modifier toggles copy / new-face mode.
        let copy_now = if cfg!(target_os = "macos") { mods.alt } else { mods.ctrl };
        let copy_prev = if cfg!(target_os = "macos") { prev.alt } else { prev.ctrl };
        if copy_now && !copy_prev {
            self.with_tool_cx(|t, cx| t.on_copy_toggle(cx));
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        let mut consumed_text = false;
        for ev in &events {
            match ev {
                egui::Event::Text(t) => {
                    let starts = t.chars().next().is_some_and(|c| VCB_START.contains(&c));
                    if self.vcb_typing || starts {
                        if !self.vcb_typing {
                            self.vcb_typing = true;
                            self.vcb_text.clear();
                        }
                        self.vcb_text.push_str(t);
                        consumed_text = true;
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => match key {
                    egui::Key::Enter if self.vcb_typing => {
                        self.submit_vcb();
                        consumed_text = true;
                    }
                    egui::Key::Backspace if self.vcb_typing => {
                        self.vcb_text.pop();
                        if self.vcb_text.is_empty() {
                            self.vcb_typing = false;
                        }
                        consumed_text = true;
                    }
                    egui::Key::Escape => {
                        if self.vcb_typing {
                            self.vcb_typing = false;
                            self.vcb_text.clear();
                        } else {
                            self.with_tool_cx(|t, cx| t.cancel(cx));
                            self.inference.unlock();
                            self.arrow_locked = false;
                        }
                    }
                    egui::Key::ArrowUp | egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::ArrowDown if modifiers.is_none() => {
                        let k = *key;
                        if !self.with_tool_cx(|t, cx| t.on_key(cx, k))
                            && let Some(from) = self.tool.start_point() {
                                match k {
                                    egui::Key::ArrowUp => self.inference.lock_axis(from, Axis::Blue),
                                    egui::Key::ArrowRight => self.inference.lock_axis(from, Axis::Red),
                                    egui::Key::ArrowLeft => self.inference.lock_axis(from, Axis::Green),
                                    _ => self.inference.lock_current(Some(from)),
                                }
                                self.arrow_locked = self.inference.lock.is_some();
                            }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        // While typing a measurement, letters belong to the Measurements box, not to shortcuts.
        if consumed_text || self.vcb_typing {
            // Still allow modifier shortcuts (Ctrl+Z etc.).
            let cmds: Vec<_> = self.keymap.bindings.iter().filter(|(s, _)| s.modifiers.command || s.modifiers.ctrl).cloned().collect();
            let found = ctx.input_mut(|i| cmds.into_iter().find(|(s, _)| i.consume_shortcut(s)).map(|(_, c)| c));
            if let Some(c) = found {
                self.run(ctx, c);
            }
            return;
        }
        if let Some(cmd) = self.keymap.poll(ctx) {
            self.run(ctx, cmd);
        }
    }

    fn draw_inference(&self, ov: &mut Overlay) {
        let Some(inf) = self.inference.last else { return };
        let cam = self.doc.model.camera.to_local(&self.doc.active_xform());
        let size = self.viewport.size;
        let Some(sp) = cam.project(inf.point, size) else { return };
        let p = pos2(sp.x as f32, sp.y as f32);
        let outline = Stroke::new(1.0, Color32::from_gray(30));
        match inf.kind {
            Kind::Endpoint => ov.shapes.push(egui::Shape::circle_filled(p, 5.0, Color32::from_rgb(0, 200, 0))),
            Kind::Midpoint => ov.shapes.push(egui::Shape::circle_filled(p, 5.0, Color32::from_rgb(0, 210, 230))),
            Kind::Origin => ov.shapes.push(egui::Shape::circle_filled(p, 5.0, Color32::from_rgb(245, 210, 0))),
            Kind::GuidePoint => ov.shapes.push(egui::Shape::circle_filled(p, 4.0, Color32::BLACK)),
            Kind::OnEdge => ov.shapes.push(egui::Shape::rect_filled(egui::Rect::from_center_size(p, vec2(8.0, 8.0)), 0.0, Color32::from_rgb(230, 0, 0))),
            Kind::OnGuide => ov.shapes.push(egui::Shape::rect_filled(egui::Rect::from_center_size(p, vec2(7.0, 7.0)), 0.0, Color32::from_gray(40))),
            Kind::OnFace => {
                let d = 5.5;
                ov.shapes.push(egui::Shape::convex_polygon(vec![p + vec2(0.0, -d), p + vec2(d, 0.0), p + vec2(0.0, d), p + vec2(-d, 0.0)], Color32::from_rgb(40, 60, 255), Stroke::NONE));
            }
            Kind::OnAxis(_) | Kind::FromPoint(_) | Kind::Locked(_) | Kind::Free => {}
        }
        if inf.kind.is_point() {
            ov.shapes.push(egui::Shape::circle_stroke(p, 5.0, outline));
        }
        // Dotted inference line from the anchor.
        if let Some(a) = inf.anchor {
            let (color, width) = match inf.kind {
                Kind::OnAxis(ax) | Kind::FromPoint(ax) => (ax.color(), 1.5),
                Kind::Locked(Some(ax)) => (ax.color(), 3.0),
                Kind::Locked(None) => (Color32::from_rgb(200, 0, 200), 3.0),
                _ => (Color32::BLACK, 1.0),
            };
            ov.lines.push(line(a, inf.point, color, width).dashed(4.0));
        }
        let text = inf.kind.tooltip();
        if !text.is_empty() {
            ov.tooltips.push((p + vec2(12.0, 14.0), text));
        }
    }

    fn build_scene(&mut self, ov: &Overlay) -> RenderScene {
        let st = &self.doc.model.style;
        let mut h = DefaultHasher::new();
        self.doc.revision_key().hash(&mut h);
        let mut sel_i: Vec<_> = self.doc.selection.instances.iter().chain(ov.highlight_instances.iter()).collect();
        sel_i.sort();
        sel_i.hash(&mut h);
        let mut sel_e: Vec<_> = self.doc.selection.edges.iter().chain(ov.highlight_edges.iter()).collect();
        sel_e.sort();
        sel_e.hash(&mut h);
        let mut sel_f: Vec<_> = self.doc.selection.faces.iter().chain(ov.highlight_faces.iter()).collect();
        sel_f.sort();
        sel_f.hash(&mut h);
        format!("{:?}{}{}{}{}", st.face_style, st.back_edges, st.show_axes, st.show_guides, st.show_sky_ground).hash(&mut h);
        self.doc.model.guides.len().hash(&mut h);
        let key = h.finish();
        if key == self.viewport.cache_key
            && let Some(scene) = self.viewport.cache.take() {
                return scene;
            }
        let opts = SceneOptions { style: st.face_style, back_edges: st.back_edges, show_axes: st.show_axes, edge_width: 1.3 };
        let hl = crate::scene::Highlights { edges: &ov.highlight_edges, faces: &ov.highlight_faces, instances: &ov.highlight_instances };
        let mut scene = crate::scene::build_scene(&self.doc, &opts, &hl);
        scene.show_background = st.show_sky_ground;
        if st.show_guides {
            let big = 1.0e5;
            let gc = sc_render::scene_rgba([90, 90, 90, 255]);
            for g in &self.doc.model.guides {
                match g {
                    Guide::Line { point, dir } => scene.lines.push(LineInstance::new(*point - *dir * big, *point + *dir * big, gc, 1.0).dashed(4.0)),
                    Guide::Point { .. } => {} // drawn per frame in screen space
                }
            }
        }
        self.viewport.cache_key = key;
        scene
    }
}
