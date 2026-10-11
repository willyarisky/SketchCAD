//! SketchCAD's window chrome, laid out like SketchUp: menu bar, Getting Started toolbar on top,
//! Large Tool Set on the left, Default Tray on the right, status bar with Measurements box below.

pub mod dialogs;
pub mod icons;
pub mod panels;
pub mod tray;
pub mod viewport;

use crate::app::SketchCadApp;
use crate::commands::Command;
use crate::tools::ToolId;
use egui::{Color32, Frame, Margin, RichText};
use icons::{Icon, icon_button};
use sc_render::FaceStyle;

pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.global_style_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::from_rgb(236, 236, 238);
        v.window_fill = Color32::from_rgb(246, 246, 248);
        v.extreme_bg_color = Color32::WHITE;
        v.selection.bg_fill = Color32::from_rgb(64, 132, 230);
        v.selection.stroke.color = Color32::from_rgb(30, 90, 200);
        v.widgets.noninteractive.bg_stroke.color = Color32::from_gray(205);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
    });
}

/// SketchUp-style yellow inference tooltip.
pub fn paint_tooltip(painter: &egui::Painter, at: egui::Pos2, text: &str) {
    if text.is_empty() {
        return;
    }
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(12.0), Color32::BLACK);
    let rect = egui::Rect::from_min_size(at, galley.size() + egui::vec2(8.0, 4.0));
    painter.rect(rect, 2.0, Color32::from_rgb(255, 255, 210), egui::Stroke::new(1.0, Color32::from_gray(120)), egui::StrokeKind::Inside);
    painter.galley(at + egui::vec2(4.0, 2.0), galley, Color32::BLACK);
}

/// Area in the model's display units (squared).
pub fn format_area(sq_in: f64, units: &sc_math::units::UnitSettings) -> String {
    use sc_math::units::LengthUnit::*;
    let (val, suffix) = match units.unit {
        Architectural | Inches => (sq_in, "in²"),
        Feet => (sq_in / 144.0, "ft²"),
        Millimeters => (sq_in * 645.16, "mm²"),
        Centimeters => (sq_in * 6.4516, "cm²"),
        Meters => (sq_in * 0.000_645_16, "m²"),
    };
    format!("{:.*} {}", if units.unit == Meters { 3 } else { 2 }, val, suffix)
}

impl SketchCadApp {
    fn menu_item(&mut self, ui: &mut egui::Ui, cmd: Command, enabled: bool) {
        let mut btn = egui::Button::new(cmd.label());
        if let Some(s) = self.keymap.shortcut_for(cmd) {
            btn = btn.shortcut_text(ui.ctx().format_shortcut(&s));
        }
        if ui.add_enabled(enabled, btn).clicked() {
            let ctx = ui.ctx().clone();
            self.run(&ctx, cmd);
        }
    }

    fn menu_check(&mut self, ui: &mut egui::Ui, cmd: Command, checked: bool) {
        let mut btn = egui::Button::new(cmd.label()).selected(checked);
        if let Some(s) = self.keymap.shortcut_for(cmd) {
            btn = btn.shortcut_text(ui.ctx().format_shortcut(&s));
        }
        if ui.add(btn).clicked() {
            let ctx = ui.ctx().clone();
            self.run(&ctx, cmd);
        }
    }

    pub fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu_bar").frame(Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(Margin::symmetric(6, 3))).show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    self.menu_item(ui, Command::New, true);
                    self.menu_item(ui, Command::Open, true);
                    ui.separator();
                    self.menu_item(ui, Command::Save, true);
                    self.menu_item(ui, Command::SaveAs, true);
                    ui.separator();
                    self.menu_item(ui, Command::Quit, true);
                    self.plugin_menu_items(ui, "File");
                });
                ui.menu_button("Edit", |ui| {
                    let undo = self.doc.history.undo_name().map(|n| format!("Undo {n}")).unwrap_or_else(|| "Undo".into());
                    let redo = self.doc.history.redo_name().map(|n| format!("Redo {n}")).unwrap_or_else(|| "Redo".into());
                    let can_undo = self.doc.history.undo_name().is_some();
                    let can_redo = self.doc.history.redo_name().is_some();
                    let fmt = |c: Command, s: &Self| s.keymap.shortcut_for(c).map(|k| ui.ctx().format_shortcut(&k)).unwrap_or_default();
                    let (u, r) = (fmt(Command::Undo, self), fmt(Command::Redo, self));
                    if ui.add_enabled(can_undo, egui::Button::new(undo).shortcut_text(u)).clicked() {
                        self.run(&ui.ctx().clone(), Command::Undo);
                    }
                    if ui.add_enabled(can_redo, egui::Button::new(redo).shortcut_text(r)).clicked() {
                        self.run(&ui.ctx().clone(), Command::Redo);
                    }
                    ui.separator();
                    let has_sel = !self.doc.selection.is_empty();
                    self.menu_item(ui, Command::Delete, has_sel);
                    self.menu_item(ui, Command::DeleteGuides, !self.doc.model.guides.is_empty());
                    ui.separator();
                    self.menu_item(ui, Command::SelectAll, true);
                    self.menu_item(ui, Command::SelectNone, has_sel);
                    ui.separator();
                    self.menu_item(ui, Command::Hide, !self.doc.selection.edges.is_empty());
                    self.menu_item(ui, Command::UnhideAll, true);
                    ui.separator();
                    self.menu_item(ui, Command::ReverseFaces, !self.doc.selection.faces.is_empty());
                    ui.separator();
                    self.group_menu_items(ui);
                    self.plugin_menu_items(ui, "Edit");
                });
                ui.menu_button("View", |ui| {
                    ui.menu_button("Toolbars", |ui| {
                        ui.checkbox(&mut self.panels.getting_started, "Getting Started");
                        ui.checkbox(&mut self.panels.large_tool_set, "Large Tool Set");
                        if !self.plugins.toolbars.is_empty() {
                            ui.separator();
                            for t in &mut self.plugins.toolbars {
                                ui.checkbox(&mut t.visible, t.name.clone());
                            }
                        }
                    });
                    ui.checkbox(&mut self.panels.tray, "Default Tray");
                    ui.separator();
                    ui.menu_button("Animation", |ui| {
                        self.menu_item(ui, Command::AddScene, true);
                        let has = !self.doc.model.scenes.is_empty();
                        self.menu_item(ui, Command::PreviousScene, has);
                        self.menu_item(ui, Command::NextScene, has);
                    });
                    ui.separator();
                    let st = self.doc.model.style.clone();
                    self.menu_check(ui, Command::ToggleGuides, st.show_guides);
                    self.menu_check(ui, Command::ToggleAxes, st.show_axes);
                    ui.separator();
                    ui.menu_button("Edge Style", |ui| {
                        self.menu_check(ui, Command::ToggleBackEdges, st.back_edges);
                    });
                    ui.menu_button("Face Style", |ui| {
                        for fs in [FaceStyle::XRay, FaceStyle::Wireframe, FaceStyle::HiddenLine, FaceStyle::Shaded, FaceStyle::ShadedWithTextures, FaceStyle::Monochrome] {
                            self.menu_check(ui, Command::FaceStyle(fs), st.face_style == fs);
                        }
                    });
                    self.plugin_menu_items(ui, "View");
                });
                ui.menu_button("Camera", |ui| {
                    ui.menu_button("Standard Views", |ui| {
                        for (v, _) in Self::standard_views() {
                            self.menu_item(ui, Command::StandardView(v), true);
                        }
                    });
                    ui.separator();
                    let persp = self.doc.model.camera.perspective;
                    if ui.add(egui::Button::new("Parallel Projection").selected(!persp)).clicked() {
                        self.doc.model.camera.perspective = false;
                    }
                    if ui.add(egui::Button::new("Perspective").selected(persp)).clicked() {
                        self.doc.model.camera.perspective = true;
                    }
                    ui.separator();
                    for t in [ToolId::Orbit, ToolId::Pan, ToolId::Zoom] {
                        self.menu_item(ui, Command::Tool(t), true);
                    }
                    self.menu_item(ui, Command::ZoomExtents, true);
                    self.plugin_menu_items(ui, "Camera");
                });
                ui.menu_button("Draw", |ui| {
                    ui.menu_button("Lines", |ui| self.menu_item(ui, Command::Tool(ToolId::Line), true));
                    ui.menu_button("Arcs", |ui| self.menu_item(ui, Command::Tool(ToolId::Arc), true));
                    ui.menu_button("Shapes", |ui| {
                        for t in [ToolId::Rectangle, ToolId::Circle, ToolId::Polygon] {
                            self.menu_item(ui, Command::Tool(t), true);
                        }
                    });
                    self.plugin_menu_items(ui, "Draw");
                });
                ui.menu_button("Tools", |ui| {
                    for t in [ToolId::Select, ToolId::Eraser, ToolId::TapeMeasure, ToolId::PaintBucket] {
                        self.menu_item(ui, Command::Tool(t), true);
                    }
                    ui.separator();
                    for t in [ToolId::Move, ToolId::Rotate, ToolId::Scale, ToolId::PushPull, ToolId::FollowMe, ToolId::Offset] {
                        self.menu_item(ui, Command::Tool(t), true);
                    }
                    self.plugin_menu_items(ui, "Tools");
                });
                ui.menu_button("Window", |ui| {
                    self.menu_item(ui, Command::ModelInfo, true);
                    self.menu_item(ui, Command::Preferences, true);
                    ui.separator();
                    ui.checkbox(&mut self.panels.tray, "Default Tray");
                    self.plugin_menu_items(ui, "Window");
                });
                ui.menu_button("Extensions", |ui| self.extensions_menu(ui));
                ui.menu_button("Help", |ui| {
                    self.menu_item(ui, Command::Shortcuts, true);
                    ui.separator();
                    self.menu_item(ui, Command::About, true);
                    self.plugin_menu_items(ui, "Help");
                });
            });
        });
    }

    /// Group/component commands shared by the Edit menu and the context menu.
    pub fn group_menu_items(&mut self, ui: &mut egui::Ui) {
        let sel = &self.doc.selection;
        let has_geom = !sel.is_empty();
        let has_inst = !sel.instances.is_empty();
        let shared = sel.instances.iter().any(|i| {
            self.doc.active_mesh().instances.get(*i).is_some_and(|inst| self.doc.model.instance_count(inst.def) > 1)
        });
        let editing = !self.doc.edit_path.is_empty();
        self.menu_item(ui, Command::MakeGroup, has_geom);
        self.menu_item(ui, Command::MakeComponent, has_geom);
        self.menu_item(ui, Command::EditComponent, has_inst);
        self.menu_item(ui, Command::Explode, has_inst);
        self.menu_item(ui, Command::MakeUnique, shared);
        if editing {
            self.menu_item(ui, Command::CloseComponent, true);
        }
    }

    /// Right-click menu in the viewport, like SketchUp's.
    pub fn context_menu(&mut self, ui: &mut egui::Ui) {
        self.menu_item(ui, Command::Delete, !self.doc.selection.is_empty());
        self.menu_item(ui, Command::Hide, !self.doc.selection.is_empty());
        ui.separator();
        self.group_menu_items(ui);
        if !self.doc.selection.faces.is_empty() {
            ui.separator();
            self.menu_item(ui, Command::ReverseFaces, true);
        }
        ui.separator();
        self.menu_item(ui, Command::SelectAll, true);
        self.menu_item(ui, Command::ZoomExtents, true);
    }

    fn tool_btn(&mut self, ui: &mut egui::Ui, t: ToolId, size: f32) {
        let tip = match self.keymap.shortcut_for(Command::Tool(t)) {
            Some(s) => format!("{} ({})", t.name(), ui.ctx().format_shortcut(&s)),
            None => t.name().to_string(),
        };
        if icon_button(ui, Icon::Tool(t), self.tool.id() == t, true, size, &tip).clicked() {
            self.set_tool(t);
        }
    }

    fn cmd_btn(&mut self, ui: &mut egui::Ui, icon: Icon, cmd: Command, selected: bool, enabled: bool, size: f32) {
        let tip = match self.keymap.shortcut_for(cmd) {
            Some(s) => format!("{} ({})", cmd.label(), ui.ctx().format_shortcut(&s)),
            None => cmd.label(),
        };
        if icon_button(ui, icon, selected, enabled, size, &tip).clicked() {
            let ctx = ui.ctx().clone();
            self.run(&ctx, cmd);
        }
    }

    /// The "Getting Started" toolbar plus Views and Styles toolbars.
    pub fn top_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("getting_started").frame(Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(Margin::symmetric(6, 3))).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let s = 30.0;
                // Standard toolbar.
                self.cmd_btn(ui, Icon::New, Command::New, false, true, s);
                self.cmd_btn(ui, Icon::Open, Command::Open, false, true, s);
                self.cmd_btn(ui, Icon::Save, Command::Save, false, true, s);
                ui.separator();
                for t in [ToolId::Select, ToolId::Eraser, ToolId::Line, ToolId::Arc, ToolId::Rectangle, ToolId::PushPull, ToolId::Offset, ToolId::Move, ToolId::Rotate, ToolId::Scale, ToolId::TapeMeasure, ToolId::PaintBucket] {
                    self.tool_btn(ui, t, s);
                }
                ui.separator();
                for t in [ToolId::Orbit, ToolId::Pan, ToolId::Zoom] {
                    self.tool_btn(ui, t, s);
                }
                self.cmd_btn(ui, Icon::ZoomExtents, Command::ZoomExtents, false, true, s);
                ui.separator();
                let (cu, cr) = (self.doc.history.undo_name().is_some(), self.doc.history.redo_name().is_some());
                self.cmd_btn(ui, Icon::Undo, Command::Undo, false, cu, s);
                self.cmd_btn(ui, Icon::Redo, Command::Redo, false, cr, s);
                ui.separator();
                for (v, icon) in Self::standard_views() {
                    self.cmd_btn(ui, icon, Command::StandardView(v), false, true, s);
                }
                ui.separator();
                let st = self.doc.model.style.clone();
                self.cmd_btn(ui, Icon::XRay, Command::ToggleXRay, st.face_style == FaceStyle::XRay, true, s);
                self.cmd_btn(ui, Icon::BackEdges, Command::ToggleBackEdges, st.back_edges, true, s);
                for (fs, icon) in [(FaceStyle::Wireframe, Icon::Wireframe), (FaceStyle::HiddenLine, Icon::HiddenLine), (FaceStyle::Shaded, Icon::Shaded), (FaceStyle::Monochrome, Icon::Monochrome)] {
                    self.cmd_btn(ui, icon, Command::FaceStyle(fs), st.face_style == fs, true, s);
                }
            });
        });
    }

    /// Large Tool Set: two columns of tools on the left edge, in SketchUp's order.
    pub fn large_tool_set(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("large_tool_set").resizable(false).exact_size(84.0).frame(Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(Margin::same(4))).show(ui, |ui| {
            let rows: [[Option<ToolId>; 2]; 10] = [
                [Some(ToolId::Select), Some(ToolId::Eraser)],
                [Some(ToolId::PaintBucket), Some(ToolId::Line)],
                [Some(ToolId::Rectangle), Some(ToolId::Circle)],
                [Some(ToolId::Polygon), Some(ToolId::Arc)],
                [Some(ToolId::Move), Some(ToolId::PushPull)],
                [Some(ToolId::Rotate), Some(ToolId::FollowMe)],
                [Some(ToolId::Scale), Some(ToolId::Offset)],
                [Some(ToolId::TapeMeasure), None],
                [Some(ToolId::Orbit), Some(ToolId::Pan)],
                [Some(ToolId::Zoom), None],
            ];
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            for (i, row) in rows.iter().enumerate() {
                if i == 1 || i == 4 || i == 7 || i == 8 {
                    ui.add_space(4.0);
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    for t in row {
                        match t {
                            Some(t) => self.tool_btn(ui, *t, 36.0),
                            None if i == 9 => self.cmd_btn(ui, Icon::ZoomExtents, Command::ZoomExtents, false, true, 36.0),
                            None => {
                                ui.add_space(36.0);
                            }
                        }
                    }
                });
            }
        });
    }

    pub fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_bar").frame(Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(Margin::symmetric(8, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let msg = match &self.message {
                    Some((m, t, err)) if t.elapsed().as_secs_f32() < 4.0 => Some((m.clone(), *err)),
                    _ => None,
                };
                match msg {
                    Some((m, true)) => ui.label(RichText::new(m).color(Color32::from_rgb(190, 30, 30))),
                    Some((m, false)) => ui.label(RichText::new(m).color(Color32::from_rgb(20, 90, 40))),
                    None => ui.label(self.plugins.status_text.clone().unwrap_or_else(|| self.tool.status())),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let shown = if self.vcb_typing { self.vcb_text.clone() } else { self.vcb.value.clone() };
                    let id = egui::Id::new("vcb");
                    let mut text = shown;
                    let resp = ui.add(egui::TextEdit::singleline(&mut text).id(id).desired_width(150.0).font(egui::TextStyle::Monospace));
                    if resp.gained_focus() {
                        self.vcb_typing = true;
                        self.vcb_text.clear();
                        text.clear();
                    }
                    if resp.has_focus() {
                        self.vcb_text = text;
                    }
                    if resp.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.submit_vcb();
                        } else {
                            self.vcb_typing = false;
                        }
                    }
                    ui.label(self.tool.vcb_label());
                });
            });
        });
    }

    pub fn submit_vcb(&mut self) {
        let text = std::mem::take(&mut self.vcb_text);
        self.vcb_typing = false;
        if text.trim().is_empty() {
            return;
        }
        if let Err(e) = self.with_tool_cx(|t, cx| t.on_vcb(cx, &text)) {
            self.flash(e, true);
        }
    }
}
