//! The Default Tray: Entity Info, Materials, Styles, Instructor.

use super::format_area;
use super::icons::{Icon, paint};
use crate::app::SketchCadApp;
use crate::tools::ToolId;
use egui::{Color32, CollapsingHeader, Frame, Margin, RichText};
use sc_geom::Color;
use sc_render::FaceStyle;

/// Built-in material library (colors only for now; textures come with the material editor).
pub const MATERIALS: &[(&str, Color)] = &[
    ("Brick Red", [178, 34, 34, 255]),
    ("Terracotta", [204, 102, 51, 255]),
    ("Sandstone", [222, 196, 150, 255]),
    ("Concrete", [170, 170, 165, 255]),
    ("Asphalt", [70, 70, 75, 255]),
    ("Light Wood", [214, 170, 110, 255]),
    ("Dark Wood", [120, 72, 40, 255]),
    ("Grass", [96, 150, 60, 255]),
    ("Hedge", [50, 100, 45, 255]),
    ("Water", [64, 140, 200, 255]),
    ("Glass", [160, 200, 230, 110]),
    ("Roof Slate", [90, 100, 115, 255]),
    ("Plaster", [240, 236, 225, 255]),
    ("Charcoal", [40, 40, 40, 255]),
    ("Gold", [212, 175, 55, 255]),
    ("Sky Blue", [135, 190, 235, 255]),
];

fn c32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

impl SketchCadApp {
    pub fn tray(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("default_tray").resizable(true).default_size(270.0).frame(Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(Margin::same(8))).show(ui, |ui| {
            ui.label(RichText::new("Default Tray").strong());
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                CollapsingHeader::new("Entity Info").default_open(true).show(ui, |ui| self.entity_info(ui));
                CollapsingHeader::new("Materials").default_open(true).show(ui, |ui| self.materials(ui));
                CollapsingHeader::new("Components").default_open(false).show(ui, |ui| self.components_panel(ui));
                CollapsingHeader::new("Styles").default_open(false).show(ui, |ui| self.styles(ui));
                CollapsingHeader::new("Tags").default_open(false).show(ui, |ui| self.tags_panel(ui));
                CollapsingHeader::new("Scenes").default_open(false).show(ui, |ui| self.scenes_panel(ui));
                CollapsingHeader::new("Outliner").default_open(false).show(ui, |ui| self.outliner(ui));
                CollapsingHeader::new("Instructor").default_open(true).show(ui, |ui| self.instructor(ui));
            });
        });
    }

    fn entity_info(&mut self, ui: &mut egui::Ui) {
        let units = self.doc.model.units;
        let sel = self.doc.selection.clone();
        if sel.len() == 1 && sel.instances.len() == 1 {
            let id = *sel.instances.iter().next().unwrap();
            self.instance_info(ui, id);
            return;
        }
        let mesh = self.doc.active_mesh();
        let (ne, nf) = (sel.edges.len(), sel.faces.len());
        let mut edit: Option<(sc_geom::EdgeId, bool, bool, bool)> = None;
        egui::Grid::new("entity_info").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| match (ne, nf) {
            (0, 0) => {
                ui.label("No Selection");
                ui.end_row();
                ui.label("Edges");
                ui.label(mesh.edges.len().to_string());
                ui.end_row();
                ui.label("Faces");
                ui.label(mesh.faces.len().to_string());
                ui.end_row();
            }
            (1, 0) => {
                let e = *sel.edges.iter().next().unwrap();
                ui.label(RichText::new("Edge").strong());
                ui.end_row();
                ui.label("Length");
                ui.label(units.format(mesh.edge_length(e)));
                ui.end_row();
                let Some(ed) = mesh.edges.get(e) else { return };
                let (mut soft, mut smooth, mut hidden) = (ed.soft, ed.smooth, ed.hidden);
                let changed = ui.checkbox(&mut soft, "Soft").changed() | ui.checkbox(&mut smooth, "Smooth").changed();
                ui.end_row();
                let changed = changed | ui.checkbox(&mut hidden, "Hidden").changed();
                ui.end_row();
                if changed {
                    edit = Some((e, soft, smooth, hidden));
                }
            }
            (0, 1) => {
                let f = *sel.faces.iter().next().unwrap();
                let Some(face) = mesh.faces.get(f) else { return };
                ui.label(RichText::new("Face").strong());
                ui.end_row();
                ui.label("Area");
                ui.label(format_area(mesh.face_area(f), &units));
                ui.end_row();
                ui.label("Front");
                swatch(ui, face.front.unwrap_or(sc_render::scene::DEFAULT_FRONT));
                ui.end_row();
                ui.label("Back");
                swatch(ui, face.back.unwrap_or(sc_render::scene::DEFAULT_BACK));
                ui.end_row();
            }
            _ => multi(ui, ne, nf, mesh, &sel, &units),
        });
        if let Some((e, soft, smooth, hidden)) = edit {
            self.doc.checkpoint("Edit Edge");
            let m = self.doc.active_mesh_mut();
            let ed = &mut m.edges[e];
            (ed.soft, ed.smooth, ed.hidden) = (soft, smooth, hidden);
            m.revision += 1;
        }
    }

    fn materials(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            swatch(ui, self.material);
            let name = MATERIALS.iter().find(|(_, c)| *c == self.material).map_or("Custom", |(n, _)| n);
            ui.label(name);
        });
        ui.add_space(4.0);
        let per_row = 6;
        egui::Grid::new("materials").spacing([4.0, 4.0]).show(ui, |ui| {
            for (i, (name, c)) in MATERIALS.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                ui.painter().rect_filled(rect, 3.0, c32(*c));
                if *c == self.material {
                    ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(2.0, Color32::from_rgb(30, 90, 200)), egui::StrokeKind::Outside);
                }
                if resp.on_hover_text(*name).clicked() {
                    self.material = *c;
                    self.set_tool(ToolId::PaintBucket);
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Custom");
            let mut rgba = egui::Rgba::from(c32(self.material));
            if egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::OnlyBlend).changed() {
                let c = Color32::from(rgba);
                self.material = [c.r(), c.g(), c.b(), c.a()];
                self.set_tool(ToolId::PaintBucket);
            }
        });
    }

    fn styles(&mut self, ui: &mut egui::Ui) {
        let st = &mut self.doc.model.style;
        ui.label("Face Style");
        ui.horizontal_wrapped(|ui| {
            for (fs, icon, label) in [
                (FaceStyle::XRay, Icon::XRay, "X-ray"),
                (FaceStyle::Wireframe, Icon::Wireframe, "Wireframe"),
                (FaceStyle::HiddenLine, Icon::HiddenLine, "Hidden Line"),
                (FaceStyle::Shaded, Icon::Shaded, "Shaded"),
                (FaceStyle::Monochrome, Icon::Monochrome, "Monochrome"),
            ] {
                if super::icons::icon_button(ui, icon, st.face_style == fs, true, 32.0, label).clicked() {
                    st.face_style = fs;
                }
            }
        });
        ui.checkbox(&mut st.back_edges, "Back Edges");
        ui.checkbox(&mut st.show_sky_ground, "Sky and Ground");
        ui.checkbox(&mut st.show_axes, "Axes");
        ui.checkbox(&mut st.show_guides, "Guides");
    }

    fn instructor(&mut self, ui: &mut egui::Ui) {
        let t = self.tool.id();
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
            paint(ui.painter(), rect, Icon::Tool(t), true);
            ui.label(RichText::new(t.name()).heading());
        });
        ui.label(t.instructor());
    }
}

fn swatch(ui: &mut egui::Ui, c: Color) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 18.0), egui::Sense::hover());
    ui.painter().rect(rect, 2.0, c32(c), egui::Stroke::new(1.0, Color32::from_gray(120)), egui::StrokeKind::Inside);
}

fn multi(ui: &mut egui::Ui, ne: usize, nf: usize, mesh: &sc_geom::Mesh, sel: &crate::document::Selection, units: &sc_math::units::UnitSettings) {
    ui.label(RichText::new(format!("{} Entities", sel.len())).strong());
    ui.end_row();
    if !sel.instances.is_empty() {
        ui.label("Groups/Components");
        ui.label(sel.instances.len().to_string());
        ui.end_row();
    }
    ui.label("Edges");
    ui.label(ne.to_string());
    ui.end_row();
    ui.label("Faces");
    ui.label(nf.to_string());
    ui.end_row();
    if nf > 0 {
        let area: f64 = sel.faces.iter().map(|f| mesh.face_area(*f)).sum();
        ui.label("Area");
        ui.label(format_area(area, units));
        ui.end_row();
    }
}
